//! The `.tsq` index of a TDT block: a run of 40-byte records (size in 4-byte words, event type,
//! four-character store name, channel, sort code, float64 time stamp in seconds since 1970, an
//! offset into the data file or a float64 strobe value, data format and float32 sampling rate).
//! Record 0 holds the file length, record 1 the start mark and the last record the stop mark.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::bytes::{Block, read_block};
use openreadout_core::model::Finding;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use super::TDT_FORMAT_ID;

/// Bytes of one `.tsq` record.
pub const TSQ_RECORD_LEN: u64 = 40;
/// Records accepted at most in one `.tsq` (a 2 GB index).
pub const MAX_TSQ_RECORDS: u64 = 50_000_000;
/// Strobe-on (epoc onset) records.
pub const EVENT_STROBE_ON: i32 = 0x101;
/// Strobe-off (epoc offset) records.
pub const EVENT_STROBE_OFF: i32 = 0x102;
/// Scalar records.
pub const EVENT_SCALAR: i32 = 0x201;
/// Stream chunks (compared after masking with [`EVENT_STREAM_MASK`]).
pub const EVENT_STREAM: i32 = 0x8101;
/// Snippets (spike waveforms).
pub const EVENT_SNIPPET: i32 = 0x8201;
/// Marks (block start and stop).
pub const EVENT_MARK: i32 = 0x8801;
/// Mask applied to the event type before comparing it with [`EVENT_STREAM`].
pub const EVENT_STREAM_MASK: i32 = 0xFF0F;

/// Sample type of a data format code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    /// Code 0.
    Float32,
    /// Code 1.
    Int32,
    /// Code 2.
    Int16,
    /// Code 3.
    Int8,
    /// Code 4.
    Float64,
}

impl SampleFormat {
    /// The format of a code, if known.
    pub fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            0 => Self::Float32,
            1 => Self::Int32,
            2 => Self::Int16,
            3 => Self::Int8,
            4 => Self::Float64,
            _ => return None,
        })
    }
    /// Bytes per sample.
    pub fn width(self) -> u64 {
        match self {
            Self::Int8 => 1,
            Self::Int16 => 2,
            Self::Float32 | Self::Int32 => 4,
            Self::Float64 => 8,
        }
    }
    /// NumPy-style name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Float32 => "float32",
            Self::Int32 => "int32",
            Self::Int16 => "int16",
            Self::Int8 => "int8",
            Self::Float64 => "float64",
        }
    }
    /// Decode `b` (little-endian samples) into `out`.
    pub fn decode(self, b: &[u8], out: &mut Vec<f64>) {
        match self {
            Self::Float32 => out.extend(
                b.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|x| f64::from(f32::from_le_bytes(*x))),
            ),
            Self::Int32 => out.extend(
                b.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|x| f64::from(i32::from_le_bytes(*x))),
            ),
            Self::Int16 => out.extend(
                b.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|x| f64::from(i16::from_le_bytes(*x))),
            ),
            Self::Int8 => out.extend(b.iter().map(|x| f64::from(*x as i8))),
            Self::Float64 => {
                out.extend(b.as_chunks::<8>().0.iter().map(|x| f64::from_le_bytes(*x)));
            }
        }
    }
}

/// A store name as text (four bytes, NULs dropped, Latin-1).
pub fn store_text(b: [u8; 4]) -> String {
    b.iter()
        .filter(|c| **c != 0)
        .map(|c| char::from(*c))
        .collect()
}

/// One channel of a stream store in one block.
#[derive(Debug, Clone, Default)]
pub struct StreamChannel {
    /// Data-file offset of every chunk, in index order.
    pub offsets: Vec<u64>,
    /// Time stamp of the first chunk, seconds since 1970.
    pub first_time: f64,
    /// Time stamp of the last chunk seen.
    pub last_time: f64,
    /// Chunks that start later than one chunk after the previous one.
    pub gaps: u64,
}

/// A stream store of one block: chunks of `chunk_samples` samples per channel.
#[derive(Debug, Clone)]
pub struct StreamStore {
    /// Store name.
    pub name: String,
    /// Data format code of the first chunk.
    pub format_code: i32,
    /// Size field (4-byte words) of the first chunk.
    pub size_words: i32,
    /// Sampling rate of the first chunk.
    pub sample_rate_hz: f32,
    /// Channels by channel number.
    pub channels: BTreeMap<u16, StreamChannel>,
    /// Why the store cannot be read, if it cannot.
    pub unreadable: Option<String>,
}

impl StreamStore {
    /// Sample type, when the format code is known.
    pub fn format(&self) -> Option<SampleFormat> {
        SampleFormat::from_code(self.format_code)
    }
    /// Bytes of data per chunk.
    pub fn chunk_bytes(&self) -> u64 {
        (i64::from(self.size_words) * 4 - TSQ_RECORD_LEN as i64).max(0) as u64
    }
    /// Samples per chunk.
    pub fn chunk_samples(&self) -> u64 {
        self.format().map_or(0, |f| self.chunk_bytes() / f.width())
    }
}

/// One snippet (spike waveform) record.
#[derive(Debug, Clone, Copy)]
pub struct Snippet {
    /// Time stamp, seconds since 1970.
    pub time: f64,
    /// Index into [`TsqIndex::snippet_stores`].
    pub store: u16,
    /// Channel number.
    pub channel: u16,
    /// Sort code (0 unsorted).
    pub sort_code: u16,
    /// Offset of the waveform in the data file.
    pub offset: u64,
    /// Bytes of waveform data.
    pub bytes: u32,
    /// Data format code.
    pub format_code: i32,
}

/// One strobe (epoc) record.
#[derive(Debug, Clone, Copy)]
pub struct Strobe {
    /// Time stamp, seconds since 1970.
    pub time: f64,
    /// Store name.
    pub store: [u8; 4],
    /// For strobe-off records, the strobe-on store they close (from the channel and sort code
    /// bytes).
    pub onset_store: [u8; 4],
    /// The value (the offset field read as float64).
    pub value: f64,
    /// True for strobe-off records.
    pub off: bool,
}

/// The parsed index of one block.
#[derive(Debug, Clone, Default)]
pub struct TsqIndex {
    /// Length of the `.tsq`.
    pub file_len: u64,
    /// Records in the file.
    pub record_count: u64,
    /// Start mark time, seconds since 1970.
    pub start_time: Option<f64>,
    /// Stop mark time, seconds since 1970.
    pub stop_time: Option<f64>,
    /// Stream stores by name.
    pub streams: BTreeMap<String, StreamStore>,
    /// Names of the snippet stores, in order of first appearance.
    pub snippet_stores: Vec<String>,
    /// Snippet records in index order.
    pub snippets: Vec<Snippet>,
    /// Strobe records in index order.
    pub strobes: Vec<Strobe>,
    /// Record counts by event type.
    pub type_counts: BTreeMap<i32, u64>,
    /// Problems found while parsing.
    pub findings: Vec<Finding>,
}

impl TsqIndex {
    /// Scalar records and records of unknown types.
    pub fn undecoded_records(&self) -> u64 {
        self.type_counts
            .iter()
            .filter(|(t, _)| !is_known_type(**t))
            .map(|(_, n)| *n)
            .sum::<u64>()
            + self.type_counts.get(&EVENT_SCALAR).copied().unwrap_or(0)
    }
}

fn is_known_type(t: i32) -> bool {
    t == 0
        || t & EVENT_STREAM_MASK == EVENT_STREAM
        || matches!(
            t,
            EVENT_SNIPPET | EVENT_STROBE_ON | EVENT_STROBE_OFF | EVENT_MARK | EVENT_SCALAR
        )
}

/// True when `head` starts like a `.tsq` (record 1 is a start mark).
pub fn looks_like_tsq(head: &[u8]) -> bool {
    let at = |o: usize| -> Option<[u8; 4]> { head.get(o..o + 4).and_then(|b| b.try_into().ok()) };
    matches!(
        (at(4), at(44), at(48)),
        (Some([0, 0, 0, 0]), Some(t), Some([1, 0, 0, 0])) if i32::from_le_bytes(t) == EVENT_MARK
    )
}

/// Parse a `.tsq`.
#[allow(clippy::many_single_char_names)] // f, n, k, r: file, records, record index, record bytes
pub fn parse_tsq(f: &mut SourceFile, path: &Path, file_len: u64) -> Result<TsqIndex> {
    let n = file_len / TSQ_RECORD_LEN;
    if n > MAX_TSQ_RECORDS {
        return Err(Error::unsupported(
            TDT_FORMAT_ID,
            format!("a .tsq index of {n} records"),
            format!(
                "{} holds more than {MAX_TSQ_RECORDS} records; split the block or check that it is a .tsq file",
                path.display()
            ),
        ));
    }
    if n < 2 {
        return Err(Error::corrupt(
            TDT_FORMAT_ID,
            format!(
                "{} is too short for a .tsq index ({file_len} bytes)",
                path.display()
            ),
        ));
    }
    let mut out = TsqIndex {
        file_len,
        record_count: n,
        ..TsqIndex::default()
    };
    if !file_len.is_multiple_of(TSQ_RECORD_LEN) {
        out.findings.push(
            Finding::warning(
                "partial_record",
                format!(
                    "{} bytes after the last whole record",
                    file_len % TSQ_RECORD_LEN
                ),
            )
            .at(n * TSQ_RECORD_LEN),
        );
    }
    let window = 1u64 << 22;
    let mut buf = Block::default();
    let mut snippet_index: BTreeMap<[u8; 4], u16> = BTreeMap::new();
    for k in 0..n {
        let at = k * TSQ_RECORD_LEN;
        if at + TSQ_RECORD_LEN > buf.end() || at < buf.origin {
            buf = read_block(f, path, at, window, file_len)?;
        }
        let Some(r) = buf.slice(at, TSQ_RECORD_LEN as usize) else {
            return Err(Error::corrupt_at(TDT_FORMAT_ID, at, "index record cut off"));
        };
        let i32_at = |o: usize| i32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]]);
        let u16_at = |o: usize| u16::from_le_bytes([r[o], r[o + 1]]);
        let mut eight = [0u8; 8];
        eight.copy_from_slice(&r[16..24]);
        let time = f64::from_le_bytes(eight);
        eight.copy_from_slice(&r[24..32]);
        let offset_bytes = eight;
        let size = i32_at(0);
        let kind = i32_at(4);
        let name = [r[8], r[9], r[10], r[11]];
        let channel = u16_at(12);
        let sort_code = u16_at(14);
        let format_code = i32_at(32);
        let rate = f32::from_le_bytes([r[36], r[37], r[38], r[39]]);
        *out.type_counts.entry(kind).or_insert(0) += 1;
        if k == 0 {
            if kind != 0 || u64::try_from(size).ok() != Some(file_len) {
                out.findings.push(Finding::warning(
                    "index_header",
                    format!("record 0 has type {kind} and size {size}, not type 0 and the file length {file_len}"),
                ));
            }
            continue;
        }
        if kind == EVENT_MARK {
            match name {
                [1, 0, 0, 0] => out.start_time = Some(time),
                [2, 0, 0, 0] => out.stop_time = Some(time),
                _ => {}
            }
        } else if kind & EVENT_STREAM_MASK == EVENT_STREAM {
            let store = store_text(name);
            let s = out
                .streams
                .entry(store.clone())
                .or_insert_with(|| StreamStore {
                    name: store.clone(),
                    format_code,
                    size_words: size,
                    sample_rate_hz: rate,
                    channels: BTreeMap::new(),
                    unreadable: None,
                });
            if s.unreadable.is_none() {
                if s.format_code != format_code {
                    s.unreadable = Some(format!(
                        "its data format changes from {} to {format_code}",
                        s.format_code
                    ));
                } else if s.size_words != size {
                    s.unreadable = Some(format!(
                        "its chunk size changes from {} to {size} words",
                        s.size_words
                    ));
                } else if s.format().is_none() {
                    s.unreadable = Some(format!("data format code {format_code} is not known"));
                } else if s.chunk_samples() == 0 {
                    s.unreadable = Some(format!("chunks of {size} words hold no samples"));
                }
            }
            let per_chunk = s.chunk_samples() as f64 / f64::from(s.sample_rate_hz);
            let c = s.channels.entry(channel).or_insert_with(|| StreamChannel {
                first_time: time,
                last_time: time,
                ..StreamChannel::default()
            });
            if !c.offsets.is_empty() && time - c.last_time > 1.5 * per_chunk {
                c.gaps += 1;
            }
            c.last_time = time;
            c.offsets.push(u64::from_le_bytes(offset_bytes));
        } else if kind == EVENT_SNIPPET {
            let next = u16::try_from(snippet_index.len()).unwrap_or(u16::MAX);
            let store = *snippet_index.entry(name).or_insert_with(|| {
                out.snippet_stores.push(store_text(name));
                next
            });
            out.snippets.push(Snippet {
                time,
                store,
                channel,
                sort_code,
                offset: u64::from_le_bytes(offset_bytes),
                bytes: u32::try_from((i64::from(size) * 4 - TSQ_RECORD_LEN as i64).max(0))
                    .unwrap_or(0),
                format_code,
            });
        } else if kind == EVENT_STROBE_ON || kind == EVENT_STROBE_OFF {
            out.strobes.push(Strobe {
                time,
                store: name,
                onset_store: [r[12], r[13], r[14], r[15]],
                value: f64::from_le_bytes(offset_bytes),
                off: kind == EVENT_STROBE_OFF,
            });
        }
    }
    if out.start_time.is_none() {
        out.findings.push(Finding::warning(
            "no_start_mark",
            "the index has no block start mark; times count from the first record",
        ));
    }
    if out.stop_time.is_none() {
        out.findings.push(Finding::warning(
            "no_stop_mark",
            "the index has no block stop mark (the recording may not have been closed)",
        ));
    }
    Ok(out)
}
