//! PL2 layout, derived from hex dumps of the corpus files only (no Plexon reader or document):
//! file header, channel-header records and the data-record walk. Layout and names:
//! `docs/formats/plexon.md`.

use openreadout_core::source::SourceFile as File;
use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::bytes::{Block, read_block};
use openreadout_core::model::Finding;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::plx::SampleBlock;

/// `PLEXON` at byte 10 of a PL2 file.
pub const PL2_MAGIC: &[u8; 6] = b"PLEXON";
/// Offset of [`PL2_MAGIC`].
pub const PL2_MAGIC_AT: usize = 10;
/// Length of the file header; the channel-header records follow it.
pub const PL2_FILE_HEADER_LEN: u64 = 0x480;
/// Length of every record header.
pub const RECORD_HEADER_LEN: u64 = 16;
/// Record type of a spike-channel header.
pub const REC_SPIKE_HEADER: u8 = 0xD5;
/// Record type of an analog-channel header.
pub const REC_ANALOG_HEADER: u8 = 0xD4;
/// Record type of a digital-channel header.
pub const REC_DIGITAL_HEADER: u8 = 0xD6;
/// Record type of a run of analog samples.
pub const REC_ANALOG: u8 = 0x42;
/// Record type of a run of spike waveforms.
pub const REC_SPIKES: u8 = 0x31;
/// Record type of a run of digital events.
pub const REC_EVENTS: u8 = 0x5A;
/// Record type that ends the recording.
pub const REC_END: u8 = 0x59;
/// Word count of OmniPlex's end record (the one that holds the recording length).
pub const END_RECORD_WORDS: u64 = 10;
/// Unit slots counted in a spike-channel header.
pub const UNIT_SLOTS: usize = 256;
/// Most channel headers of one kind accepted.
pub const MAX_PL2_CHANNELS: u32 = 1 << 16;

/// Total length of a record whose header says `words` 16-bit words follow.
pub fn record_len(words: u64) -> u64 {
    RECORD_HEADER_LEN + (2 * words).div_ceil(16) * 16
}

/// The file header.
#[derive(Debug, Clone, Default)]
pub struct Pl2Header {
    /// End of the channel-header records.
    pub headers_end: u64,
    /// First data record.
    pub data_start: u64,
    /// First footer record (end of the data records).
    pub footer_start: u64,
    /// Start of the footer's per-channel index.
    pub index_start: u64,
    /// Start count recorded in the header, clock ticks.
    pub start_count: u64,
    /// Recording length, clock ticks.
    pub duration_ticks: u64,
    /// Free-text comment.
    pub comment: String,
    /// Application that wrote the file.
    pub application: String,
    /// Its version.
    pub application_version: String,
    /// Recording start, `YYYY-MM-DDThh:mm:ss` local time, when the calendar fields are valid.
    pub recorded_at: Option<String>,
    /// Timestamp clock, ticks per second.
    pub clock_hz: f64,
    /// Channel headers: total, spike, analog, digital.
    pub channel_counts: [u32; 4],
}

/// Fields every channel-header record carries.
#[derive(Debug, Clone, Default)]
pub struct Pl2Channel {
    /// Channel name.
    pub name: String,
    /// Source (device) number; data records name it in byte 1.
    pub source: u8,
    /// Channel number within the source.
    pub channel: u32,
    /// Channel enabled.
    pub enabled: bool,
    /// Recording enabled (only such channels have data).
    pub recording: bool,
    /// Unit text of the values (`Volts`); empty for digital channels.
    pub units: String,
    /// Sampling rate, Hz (0 for digital channels).
    pub rate_hz: f64,
    /// Physical units per count.
    pub units_per_count: f64,
    /// Samples per spike waveform (spike channels).
    pub waveform_samples: u32,
    /// Detection threshold, counts (spike channels).
    pub threshold: i32,
    /// Waveform samples before the threshold crossing (spike channels).
    pub pre_threshold: u32,
    /// Spikes per unit recorded in the header (spike channels).
    pub unit_counts: Vec<u64>,
    /// Device name (analog channels).
    pub device: String,
}

/// A run of spikes (one data record).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpikeRun {
    /// File offset of the record header.
    pub offset: u64,
    /// Source number.
    pub source: u8,
    /// Channel number.
    pub channel: u16,
    /// Samples per waveform.
    pub waveform_samples: u16,
    /// Spikes in the record.
    pub count: u32,
    /// Row of the first spike in the spike table.
    pub first_row: u64,
}

/// A run of digital events (one data record).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRun {
    /// File offset of the record header.
    pub offset: u64,
    /// Source number.
    pub source: u8,
    /// Channel number.
    pub channel: u16,
    /// Events in the record.
    pub count: u32,
    /// Row of the first event in the event table.
    pub first_row: u64,
}

/// Everything found by walking the data records.
#[derive(Debug, Clone, Default)]
pub struct Pl2Index {
    /// Analog records per (source, channel), in file order.
    pub analog: BTreeMap<(u8, u16), Vec<SampleBlock>>,
    /// Spike records, in file order.
    pub spikes: Vec<SpikeRun>,
    /// Spikes in all records.
    pub spike_rows: u64,
    /// Longest waveform, samples.
    pub max_waveform: u16,
    /// Event records, in file order.
    pub events: Vec<EventRun>,
    /// Events in all records.
    pub event_rows: u64,
    /// Records per type.
    pub record_counts: BTreeMap<u8, u64>,
    /// Recording length from the end-of-recording record, ticks.
    pub end_duration: Option<u64>,
    /// Where the walk stopped.
    pub walk_end: u64,
    /// Problems found.
    pub findings: Vec<Finding>,
}

/// A parsed PL2 file.
#[derive(Debug, Clone, Default)]
pub struct Pl2File {
    /// File header.
    pub header: Pl2Header,
    /// Spike-channel headers.
    pub spike_channels: Vec<Pl2Channel>,
    /// Analog-channel headers.
    pub analog_channels: Vec<Pl2Channel>,
    /// Digital-channel headers.
    pub digital_channels: Vec<Pl2Channel>,
    /// File length.
    pub file_len: u64,
    /// The data-record walk.
    pub index: Pl2Index,
}

/// True when `head` starts like a PL2 file.
pub fn looks_like_pl2(head: &[u8]) -> bool {
    head.first() == Some(&0xFE)
        && head.get(PL2_MAGIC_AT..PL2_MAGIC_AT + PL2_MAGIC.len()) == Some(&PL2_MAGIC[..])
}

fn need<T>(b: &Block, at: u64, read: fn(&Block, u64) -> Option<T>) -> Result<T> {
    read(b, at).ok_or_else(|| Error::corrupt_at(FORMAT_ID, at, "PL2 header cut off"))
}

/// Parse the file header (the first [`PL2_FILE_HEADER_LEN`] bytes).
pub fn parse_pl2_header(b: &Block) -> Result<Pl2Header> {
    if !looks_like_pl2(&b.bytes) {
        return Err(Error::corrupt_at(FORMAT_ID, 0, "no PL2 signature"));
    }
    if (b.len() as u64) < PL2_FILE_HEADER_LEN {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            b.len() as u64,
            format!(
                "the file is {} bytes, shorter than the {PL2_FILE_HEADER_LEN}-byte PL2 header",
                b.len()
            ),
        ));
    }
    let tm: Vec<u32> = (0..9)
        .map(|k| need(b, 0x230 + 4 * k, Block::u32_at))
        .collect::<Result<_>>()?;
    let recorded_at = (tm[0] < 62
        && tm[1] < 60
        && tm[2] < 24
        && (1..=31).contains(&tm[3])
        && tm[4] < 12
        && (0..=300).contains(&tm[5]))
    .then(|| {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            1900 + tm[5],
            tm[4] + 1,
            tm[3],
            tm[2],
            tm[1],
            tm[0]
        )
    });
    Ok(Pl2Header {
        headers_end: need(b, 0x20, Block::u64_at)?,
        data_start: need(b, 0x28, Block::u64_at)?,
        footer_start: need(b, 0x30, Block::u64_at)?,
        index_start: need(b, 0x38, Block::u64_at)?,
        start_count: need(b, 0x40, Block::u64_at)?,
        duration_ticks: need(b, 0x48, Block::u64_at)?,
        comment: b.text_at(0xE0, 256).unwrap_or_default(),
        application: b.text_at(0x1E0, 64).unwrap_or_default(),
        application_version: b.text_at(0x220, 16).unwrap_or_default(),
        recorded_at,
        clock_hz: b.f64_at(0x258).unwrap_or(0.0),
        channel_counts: [
            need(b, 0x260, Block::u32_at)?,
            need(b, 0x264, Block::u32_at)?,
            need(b, 0x26C, Block::u32_at)?,
            need(b, 0x274, Block::u32_at)?,
        ],
    })
}

fn channel(b: &Block, at: u64, kind: u8) -> Result<Pl2Channel> {
    let rec = b
        .u8_at(at)
        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, at, "channel header cut off"))?;
    if rec != kind {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            at,
            format!("channel header of type {rec:#04x} where {kind:#04x} was expected"),
        ));
    }
    let mut c = Pl2Channel {
        name: b.text_at(at + 16, 64).unwrap_or_default(),
        source: b.u8_at(at + 1).unwrap_or(0),
        channel: need(b, at + 0x54, Block::u32_at)?,
        enabled: need(b, at + 0x58, Block::u32_at)? != 0,
        recording: need(b, at + 0x5C, Block::u32_at)? != 0,
        ..Pl2Channel::default()
    };
    if kind != REC_DIGITAL_HEADER {
        c.units = b.text_at(at + 0x60, 16).unwrap_or_default();
        c.rate_hz = b.f64_at(at + 0x70).unwrap_or(0.0);
        c.units_per_count = b.f64_at(at + 0x78).unwrap_or(0.0);
    }
    if kind == REC_SPIKE_HEADER {
        c.waveform_samples = need(b, at + 0x80, Block::u32_at)?;
        c.threshold = b.i32_at(at + 0x84).unwrap_or(0);
        c.pre_threshold = need(b, at + 0x88, Block::u32_at)?;
        c.unit_counts = (0..UNIT_SLOTS as u64)
            .map(|u| b.u64_at(at + 0xA0 + 8 * u).unwrap_or(0))
            .collect();
        while c.unit_counts.last() == Some(&0) {
            c.unit_counts.pop();
        }
    }
    if kind == REC_ANALOG_HEADER {
        c.device = b.text_at(at + 0xD8, 64).unwrap_or_default();
    }
    Ok(c)
}

/// Parse the headers and walk the data records.
pub fn parse_pl2(f: &mut File, path: &Path, file_len: u64) -> Result<Pl2File> {
    let hb = read_block(f, path, 0, PL2_FILE_HEADER_LEN, file_len)?;
    let header = parse_pl2_header(&hb)?;
    let [_, ns, na, nd] = header.channel_counts;
    if [ns, na, nd].iter().any(|n| *n > MAX_PL2_CHANNELS) {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            0x260,
            "implausible channel-header counts",
        ));
    }
    let lens = [2592u64, 512, 368];
    let end = PL2_FILE_HEADER_LEN
        + u64::from(ns) * lens[0]
        + u64::from(na) * lens[1]
        + u64::from(nd) * lens[2];
    if end > file_len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            file_len,
            format!(
                "the channel headers end at byte {end}, past the end of the file ({file_len} bytes)"
            ),
        ));
    }
    let b = read_block(
        f,
        path,
        PL2_FILE_HEADER_LEN,
        end - PL2_FILE_HEADER_LEN,
        file_len,
    )?;
    let mut at = PL2_FILE_HEADER_LEN;
    let mut groups: [Vec<Pl2Channel>; 3] = Default::default();
    for (k, (n, kind)) in [
        (ns, REC_SPIKE_HEADER),
        (na, REC_ANALOG_HEADER),
        (nd, REC_DIGITAL_HEADER),
    ]
    .into_iter()
    .enumerate()
    {
        for _ in 0..n {
            groups[k].push(channel(&b, at, kind)?);
            at += lens[k];
        }
    }
    let [spike_channels, analog_channels, digital_channels] = groups;
    let data_start = if header.data_start >= end && header.data_start <= file_len {
        header.data_start
    } else {
        end
    };
    let stop = if header.footer_start > data_start && header.footer_start <= file_len {
        header.footer_start
    } else {
        file_len
    };
    let index = walk_records(f, path, data_start, stop, file_len)?;
    Ok(Pl2File {
        header,
        spike_channels,
        analog_channels,
        digital_channels,
        file_len,
        index,
    })
}

/// Walk the data records from `start` to `stop` (the footer).
pub fn walk_records(
    f: &mut File,
    path: &Path,
    start: u64,
    stop: u64,
    file_len: u64,
) -> Result<Pl2Index> {
    let mut ix = Pl2Index::default();
    let mut pos = start;
    while pos < stop {
        if pos + RECORD_HEADER_LEN > file_len {
            ix.findings.push(
                Finding::error(
                    "truncated",
                    format!("a record header at byte {pos} is cut off"),
                )
                .at(pos),
            );
            break;
        }
        let h = read_block(f, path, pos, RECORD_HEADER_LEN, file_len)?;
        let kind = h.u8_at(pos).unwrap_or(0);
        let source = h.u8_at(pos + 1).unwrap_or(0);
        let words = u64::from(h.u16_at(pos + 2).unwrap_or(0));
        let ch = h.u16_at(pos + 4).unwrap_or(0);
        let mut len = record_len(words);
        match kind {
            REC_ANALOG => {
                let n = h.u16_at(pos + 6).unwrap_or(0);
                ix.analog
                    .entry((source, ch))
                    .or_default()
                    .push(SampleBlock {
                        offset: pos + RECORD_HEADER_LEN,
                        timestamp: h.u64_at(pos + 8).unwrap_or(0),
                        samples: u32::from(n),
                    });
            }
            REC_SPIKES => {
                let wf = h.u16_at(pos + 6).unwrap_or(0);
                let n = h.u32_at(pos + 8).unwrap_or(0);
                // the word count is 16 bits wide; the spike count gives the true length
                len = record_len(u64::from(n) * (5 + u64::from(wf)));
                ix.spikes.push(SpikeRun {
                    offset: pos,
                    source,
                    channel: ch,
                    waveform_samples: wf,
                    count: n,
                    first_row: ix.spike_rows,
                });
                ix.spike_rows += u64::from(n);
                ix.max_waveform = ix.max_waveform.max(wf);
            }
            REC_EVENTS => {
                let n = h.u16_at(pos + 6).unwrap_or(0);
                len = record_len(5 * u64::from(n));
                ix.events.push(EventRun {
                    offset: pos,
                    source,
                    channel: ch,
                    count: u32::from(n),
                    first_row: ix.event_rows,
                });
                ix.event_rows += u64::from(n);
            }
            // OmniPlex writes a 48-byte end record (word count 10) with the recording length at
            // byte 24; offline-written (merged) files write a longer record that does not hold it.
            REC_END if words == END_RECORD_WORDS => {
                ix.end_duration = read_block(f, path, pos + 24, 8, file_len)?.u64_at(pos + 24);
            }
            _ => {}
        }
        *ix.record_counts.entry(kind).or_default() += 1;
        if pos + len > file_len {
            ix.findings.push(
                Finding::error(
                    "truncated",
                    format!(
                        "the record at byte {pos} (type {kind:#04x}) needs {len} bytes but the file ends {} bytes later",
                        file_len - pos
                    ),
                )
                .at(pos),
            );
            // keep what is whole: drop the cut-off record's entries
            match kind {
                REC_ANALOG => {
                    if let Some(v) = ix.analog.get_mut(&(source, ch)) {
                        v.pop();
                    }
                }
                REC_SPIKES => {
                    if let Some(r) = ix.spikes.pop() {
                        ix.spike_rows -= u64::from(r.count);
                    }
                }
                REC_EVENTS => {
                    if let Some(r) = ix.events.pop() {
                        ix.event_rows -= u64::from(r.count);
                    }
                }
                _ => {}
            }
            pos = file_len;
            break;
        }
        pos += len;
    }
    ix.walk_end = pos;
    Ok(ix)
}
