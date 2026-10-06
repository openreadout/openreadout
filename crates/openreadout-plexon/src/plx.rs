//! PLX ("Plexon 1") layout: file header, channel headers and the data-block walk. Layout and
//! names: `docs/formats/plexon.md`.

use openreadout_core::source::SourceFile as File;
use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::bytes::{Block, read_block};
use openreadout_core::model::Finding;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// Bytes 0–3 of a PLX file.
pub const PLX_MAGIC: [u8; 4] = *b"PLEX";
/// Length of the file header.
pub const FILE_HEADER_LEN: u64 = 7504;
/// Length of one spike-channel header.
pub const SPIKE_HEADER_LEN: u64 = 1020;
/// Length of one event-channel header.
pub const EVENT_HEADER_LEN: u64 = 296;
/// Length of one continuous-channel header.
pub const CONTINUOUS_HEADER_LEN: u64 = 296;
/// Length of a data-block header.
pub const BLOCK_HEADER_LEN: u64 = 16;
/// Block type of a spike waveform.
pub const BLOCK_SPIKE: u16 = 1;
/// Block type of an event.
pub const BLOCK_EVENT: u16 = 4;
/// Block type of a run of continuous samples.
pub const BLOCK_CONTINUOUS: u16 = 5;
/// Most channel headers of one kind accepted.
pub const MAX_CHANNEL_HEADERS: i32 = 1 << 16;

/// The file header.
#[derive(Debug, Clone, Default)]
pub struct PlxHeader {
    /// Format version (100–106 are known).
    pub version: i32,
    /// Free-text comment.
    pub comment: String,
    /// Timestamp clock, ticks per second.
    pub clock_hz: i32,
    /// Number of spike-channel headers.
    pub spike_channel_count: i32,
    /// Number of event-channel headers.
    pub event_channel_count: i32,
    /// Number of continuous-channel headers.
    pub continuous_channel_count: i32,
    /// Samples per spike waveform.
    pub waveform_points: i32,
    /// Waveform samples before the threshold crossing.
    pub pre_threshold_points: i32,
    /// Recording start, `YYYY-MM-DDThh:mm:ss` (no zone), when the date fields are valid.
    pub recorded_at: Option<String>,
    /// Waveform sampling rate (Hz; 0 in old files).
    pub waveform_rate_hz: i32,
    /// Timestamp of the last data block, in ticks, as the header records it.
    pub last_timestamp: f64,
    /// Electrodes per spike channel (1 single, 2 stereotrode, 4 tetrode; version ≥ 103).
    pub electrodes_per_channel: u8,
    /// Electrodes per spike channel in the data blocks (version ≥ 103).
    pub data_electrodes_per_channel: u8,
    /// Bits per spike sample (version ≥ 103).
    pub spike_bits: u8,
    /// Bits per continuous sample (version ≥ 103).
    pub continuous_bits: u8,
    /// Spike input range, ± mV (version ≥ 103).
    pub spike_max_mv: u16,
    /// Continuous input range, ± mV (version ≥ 103).
    pub continuous_max_mv: u16,
    /// Spike preamplifier gain (version ≥ 105).
    pub spike_preamp_gain: u16,
    /// Acquisition application (version ≥ 106).
    pub acquired_with: String,
    /// Application that last processed the file (version ≥ 106).
    pub processed_with: String,
}

/// One spike-channel header.
#[derive(Debug, Clone, Default)]
pub struct SpikeChannel {
    /// Channel name.
    pub name: String,
    /// Name of the signal the channel records.
    pub signal_name: String,
    /// Channel number used in data blocks.
    pub channel: i32,
    /// Amplifier gain.
    pub gain: i32,
    /// Filter setting.
    pub filter: i32,
    /// Detection threshold (counts).
    pub threshold: i32,
    /// Sorted units.
    pub unit_count: i32,
    /// Comment (version ≥ 105).
    pub comment: String,
}

/// One event-channel header.
#[derive(Debug, Clone, Default)]
pub struct EventChannel {
    /// Channel name.
    pub name: String,
    /// Channel number used in data blocks.
    pub channel: i32,
    /// Comment (version ≥ 105).
    pub comment: String,
}

/// One continuous-channel header.
#[derive(Debug, Clone, Default)]
pub struct ContinuousChannel {
    /// Channel name.
    pub name: String,
    /// Channel number used in data blocks.
    pub channel: i32,
    /// Sampling rate, Hz.
    pub rate_hz: i32,
    /// Amplifier gain.
    pub gain: i32,
    /// Recording enabled.
    pub enabled: bool,
    /// Preamplifier gain.
    pub preamp_gain: i32,
    /// Comment (version ≥ 105).
    pub comment: String,
}

/// A run of continuous samples: where its samples start and when.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SampleBlock {
    /// File offset of the first sample.
    pub offset: u64,
    /// Timestamp of the first sample, ticks.
    pub timestamp: u64,
    /// Samples in the block.
    pub samples: u32,
}

/// One continuous channel's blocks in file order, stored compactly.
///
/// Long recordings can hold tens of millions of small blocks (a corpus file has 31 million
/// blocks of about 10 samples), so a plain list of [`SampleBlock`] would cost 24 bytes per
/// block. Blocks are stored in groups of `GROUP`. The first block of a group is kept whole.
/// Each later block is stored as how much the offset step, the timestamp step and the sample
/// count changed from the block before it (variable-length numbers, usually a few bytes in
/// all, since these rarely change). Finding a block decodes at most one group. The encoded
/// bytes go into fixed-size chunks, so the list does not copy itself as it grows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockList {
    /// Encoded groups; a group does not span two chunks.
    chunks: Vec<Vec<u8>>,
    /// First block of each group, and the chunk and position where the rest of it starts.
    groups: Vec<(SampleBlock, u32, u32)>,
    len: usize,
    /// The last block pushed and the steps that led to it.
    last: Steps,
}

/// A block and its differences from the block before it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Steps {
    block: SampleBlock,
    offset_step: u64,
    timestamp_step: u64,
}

impl Steps {
    fn start(block: SampleBlock) -> Self {
        Steps {
            block,
            offset_step: 0,
            timestamp_step: 0,
        }
    }
}

impl BlockList {
    const GROUP: usize = 64;
    const CHUNK: usize = 64 << 10;
    /// Longest encoding of one block: three 10-byte numbers.
    const MAX_BLOCK_BYTES: usize = 30;

    /// Number of blocks.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when the channel has no blocks.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append a block.
    pub fn push(&mut self, b: SampleBlock) {
        let prev = self.last;
        match self.chunks.last_mut() {
            Some(out) if !self.len.is_multiple_of(Self::GROUP) => {
                let offset_step = b.offset.wrapping_sub(prev.block.offset);
                let timestamp_step = b.timestamp.wrapping_sub(prev.block.timestamp);
                put_varint(out, zigzag(offset_step.wrapping_sub(prev.offset_step)));
                put_varint(
                    out,
                    zigzag(timestamp_step.wrapping_sub(prev.timestamp_step)),
                );
                put_varint(
                    out,
                    zigzag(u64::from(b.samples).wrapping_sub(u64::from(prev.block.samples))),
                );
                self.last = Steps {
                    block: b,
                    offset_step,
                    timestamp_step,
                };
            }
            _ => {
                let room = self
                    .chunks
                    .last()
                    .map_or(0, |c| Self::CHUNK.saturating_sub(c.len()));
                if room < Self::GROUP * Self::MAX_BLOCK_BYTES {
                    self.chunks.push(Vec::with_capacity(Self::CHUNK));
                }
                let chunk = self.chunks.len() - 1;
                let pos = self.chunks[chunk].len();
                self.groups.push((
                    b,
                    u32::try_from(chunk).unwrap_or(u32::MAX),
                    u32::try_from(pos).unwrap_or(u32::MAX),
                ));
                self.last = Steps::start(b);
            }
        }
        self.len += 1;
    }

    /// Blocks `first..first + count` (fewer at the end of the list).
    pub fn range(&self, first: usize, count: usize) -> impl Iterator<Item = SampleBlock> + '_ {
        let end = first.saturating_add(count).min(self.len);
        let group = first / Self::GROUP;
        BlockIter {
            list: self,
            next: group * Self::GROUP,
            end,
            last: Steps::default(),
            bytes: &[],
            pos: 0,
        }
        .skip(first - group * Self::GROUP)
    }

    /// Every block, in file order.
    pub fn iter(&self) -> impl Iterator<Item = SampleBlock> + '_ {
        self.range(0, self.len)
    }
}

struct BlockIter<'a> {
    list: &'a BlockList,
    next: usize,
    end: usize,
    last: Steps,
    bytes: &'a [u8],
    pos: usize,
}

impl Iterator for BlockIter<'_> {
    type Item = SampleBlock;

    fn next(&mut self) -> Option<SampleBlock> {
        if self.next >= self.end {
            return None;
        }
        let i = self.next;
        self.next += 1;
        if i.is_multiple_of(BlockList::GROUP) {
            let (b, chunk, pos) = *self.list.groups.get(i / BlockList::GROUP)?;
            self.bytes = self.list.chunks.get(chunk as usize)?;
            self.pos = pos as usize;
            self.last = Steps::start(b);
            return Some(b);
        }
        let prev = self.last;
        let offset_step = prev
            .offset_step
            .wrapping_add(unzigzag(get_varint(self.bytes, &mut self.pos)?));
        let timestamp_step = prev
            .timestamp_step
            .wrapping_add(unzigzag(get_varint(self.bytes, &mut self.pos)?));
        let samples = u64::from(prev.block.samples)
            .wrapping_add(unzigzag(get_varint(self.bytes, &mut self.pos)?));
        let b = SampleBlock {
            offset: prev.block.offset.wrapping_add(offset_step),
            timestamp: prev.block.timestamp.wrapping_add(timestamp_step),
            samples: u32::try_from(samples).ok()?,
        };
        self.last = Steps {
            block: b,
            offset_step,
            timestamp_step,
        };
        Some(b)
    }
}

/// Map a wrapping difference near zero, either side, to a small number.
fn zigzag(v: u64) -> u64 {
    (v << 1) ^ (v >> 63).wrapping_neg()
}

fn unzigzag(v: u64) -> u64 {
    (v >> 1) ^ (v & 1).wrapping_neg()
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn get_varint(b: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(*pos)?;
        *pos += 1;
        v |= u64::from(byte & 0x7F) << shift;
        if byte < 0x80 {
            return Some(v);
        }
    }
    None
}

/// One event block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRecord {
    /// Timestamp, ticks.
    pub timestamp: u64,
    /// Event channel number.
    pub channel: u16,
    /// Event value (a strobed word, or 0).
    pub value: u16,
}

/// A gap-free run of one continuous channel's blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// Index of the first block (into the channel's block list).
    pub first_block: usize,
    /// Blocks in the run.
    pub block_count: usize,
    /// Samples in the run.
    pub samples: u64,
    /// Timestamp of the first sample, ticks.
    pub timestamp: u64,
}

/// Everything found by walking the data blocks.
#[derive(Debug, Clone, Default)]
pub struct PlxIndex {
    /// Continuous blocks per channel number, in file order.
    pub continuous: BTreeMap<u16, BlockList>,
    /// File offsets of the spike blocks (block header), in file order.
    pub spikes: Vec<u64>,
    /// Longest spike waveform (waveforms × words), samples.
    pub max_waveform: u32,
    /// Event blocks, in file order.
    pub events: Vec<EventRecord>,
    /// Blocks per type (1, 4, 5, and any other).
    pub block_counts: BTreeMap<u16, u64>,
    /// End of the last whole block.
    pub data_end: u64,
    /// Problems found.
    pub findings: Vec<Finding>,
}

/// A parsed PLX file: header, channel headers and the block index.
#[derive(Debug, Clone, Default)]
pub struct PlxFile {
    /// File header.
    pub header: PlxHeader,
    /// Spike-channel headers.
    pub spike_channels: Vec<SpikeChannel>,
    /// Event-channel headers.
    pub event_channels: Vec<EventChannel>,
    /// Continuous-channel headers.
    pub continuous_channels: Vec<ContinuousChannel>,
    /// Offset of the first data block.
    pub data_start: u64,
    /// File length.
    pub file_len: u64,
    /// The block walk.
    pub index: PlxIndex,
}

fn need<T>(b: &Block, at: u64, read: fn(&Block, u64) -> Option<T>) -> Result<T> {
    read(b, at).ok_or_else(|| Error::corrupt_at(FORMAT_ID, at, "header cut off"))
}

/// Parse the file header (the first [`FILE_HEADER_LEN`] bytes).
pub fn parse_file_header(b: &Block) -> Result<PlxHeader> {
    if b.slice(0, 4) != Some(&PLX_MAGIC[..]) {
        return Err(Error::corrupt_at(FORMAT_ID, 0, "no PLEX signature"));
    }
    if (b.len() as u64) < FILE_HEADER_LEN {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            b.len() as u64,
            format!(
                "the file is {} bytes, shorter than the {FILE_HEADER_LEN}-byte file header",
                b.len()
            ),
        ));
    }
    let version = need(b, 4, Block::i32_at)?;
    let date: Vec<i32> = (0..6)
        .map(|k| need(b, 160 + 4 * k, Block::i32_at))
        .collect::<Result<_>>()?;
    let recorded_at = ((1900..=2200).contains(&date[0])
        && (1..=12).contains(&date[1])
        && (1..=31).contains(&date[2])
        && (0..24).contains(&date[3])
        && (0..60).contains(&date[4])
        && (0..62).contains(&date[5]))
    .then(|| {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            date[0], date[1], date[2], date[3], date[4], date[5]
        )
    });
    let v103 = version >= 103;
    let u8f = |at| if v103 { b.u8_at(at).unwrap_or(0) } else { 0 };
    let u16f = |at, min| {
        if version >= min {
            b.u16_at(at).unwrap_or(0)
        } else {
            0
        }
    };
    Ok(PlxHeader {
        version,
        comment: b.text_at(8, 128).unwrap_or_default(),
        clock_hz: need(b, 136, Block::i32_at)?,
        spike_channel_count: need(b, 140, Block::i32_at)?,
        event_channel_count: need(b, 144, Block::i32_at)?,
        continuous_channel_count: need(b, 148, Block::i32_at)?,
        waveform_points: need(b, 152, Block::i32_at)?,
        pre_threshold_points: need(b, 156, Block::i32_at)?,
        recorded_at,
        waveform_rate_hz: need(b, 188, Block::i32_at)?,
        last_timestamp: b.f64_at(192).unwrap_or(0.0),
        electrodes_per_channel: u8f(200),
        data_electrodes_per_channel: u8f(201),
        spike_bits: u8f(202),
        continuous_bits: u8f(203),
        spike_max_mv: u16f(204, 103),
        continuous_max_mv: u16f(206, 103),
        spike_preamp_gain: u16f(208, 105),
        acquired_with: if version >= 106 {
            b.text_at(210, 18).unwrap_or_default()
        } else {
            String::new()
        },
        processed_with: if version >= 106 {
            b.text_at(228, 18).unwrap_or_default()
        } else {
            String::new()
        },
    })
}

impl PlxHeader {
    /// mV per count of a continuous channel (Neo's formulas per version; see the format notes).
    pub fn continuous_scale(&self, c: &ContinuousChannel) -> f64 {
        let gain = f64::from(c.gain);
        match self.version {
            ..=101 => 5000.0 / (2048.0 * gain * 1000.0),
            102 => 5000.0 / (2048.0 * gain * f64::from(c.preamp_gain)),
            _ => {
                f64::from(self.continuous_max_mv)
                    / (0.5
                        * f64::from(2u32.pow(u32::from(self.spike_bits.min(31))))
                        * gain
                        * f64::from(c.preamp_gain))
            }
        }
    }

    /// mV per count of a spike channel's waveform samples.
    pub fn spike_scale(&self, c: &SpikeChannel) -> f64 {
        let gain = f64::from(c.gain);
        let full = f64::from(2u32.pow(u32::from(self.spike_bits.min(31))));
        match self.version {
            ..=102 => 3000.0 / (2048.0 * gain * 1000.0),
            103 | 104 => f64::from(self.spike_max_mv) / (0.5 * full * gain * 1000.0),
            _ => {
                f64::from(self.spike_max_mv)
                    / (0.5 * full * gain * f64::from(self.spike_preamp_gain))
            }
        }
    }
}

/// Parse the headers and walk every data block.
pub fn parse_plx(f: &mut File, path: &Path, file_len: u64) -> Result<PlxFile> {
    let hb = read_block(f, path, 0, FILE_HEADER_LEN, file_len)?;
    let header = parse_file_header(&hb)?;
    for (n, what) in [
        (header.spike_channel_count, "spike"),
        (header.event_channel_count, "event"),
        (header.continuous_channel_count, "continuous"),
    ] {
        if !(0..=MAX_CHANNEL_HEADERS).contains(&n) {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                140,
                format!("{n} {what} channel headers"),
            ));
        }
    }
    let ns = u64::try_from(header.spike_channel_count).unwrap_or(0);
    let ne = u64::try_from(header.event_channel_count).unwrap_or(0);
    let nc = u64::try_from(header.continuous_channel_count).unwrap_or(0);
    let data_start = FILE_HEADER_LEN
        + ns * SPIKE_HEADER_LEN
        + ne * EVENT_HEADER_LEN
        + nc * CONTINUOUS_HEADER_LEN;
    if data_start > file_len {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            file_len,
            format!(
                "the channel headers end at byte {data_start}, past the end of the file ({file_len} bytes)"
            ),
        ));
    }
    let b = read_block(
        f,
        path,
        FILE_HEADER_LEN,
        data_start - FILE_HEADER_LEN,
        file_len,
    )?;
    let v105 = header.version >= 105;
    let mut at = FILE_HEADER_LEN;
    let mut spike_channels = Vec::new();
    for _ in 0..ns {
        spike_channels.push(SpikeChannel {
            name: b.text_at(at, 32).unwrap_or_default(),
            signal_name: b.text_at(at + 32, 32).unwrap_or_default(),
            channel: need(&b, at + 64, Block::i32_at)?,
            gain: need(&b, at + 80, Block::i32_at)?,
            filter: need(&b, at + 84, Block::i32_at)?,
            threshold: need(&b, at + 88, Block::i32_at)?,
            unit_count: need(&b, at + 96, Block::i32_at)?,
            comment: if v105 {
                b.text_at(at + 848, 128).unwrap_or_default()
            } else {
                String::new()
            },
        });
        at += SPIKE_HEADER_LEN;
    }
    let mut event_channels = Vec::new();
    for _ in 0..ne {
        event_channels.push(EventChannel {
            name: b.text_at(at, 32).unwrap_or_default(),
            channel: need(&b, at + 32, Block::i32_at)?,
            comment: if v105 {
                b.text_at(at + 36, 128).unwrap_or_default()
            } else {
                String::new()
            },
        });
        at += EVENT_HEADER_LEN;
    }
    let mut continuous_channels = Vec::new();
    for _ in 0..nc {
        continuous_channels.push(ContinuousChannel {
            name: b.text_at(at, 32).unwrap_or_default(),
            channel: need(&b, at + 32, Block::i32_at)?,
            rate_hz: need(&b, at + 36, Block::i32_at)?,
            gain: need(&b, at + 40, Block::i32_at)?,
            enabled: need(&b, at + 44, Block::i32_at)? != 0,
            preamp_gain: need(&b, at + 48, Block::i32_at)?,
            comment: if v105 {
                b.text_at(at + 56, 128).unwrap_or_default()
            } else {
                String::new()
            },
        });
        at += CONTINUOUS_HEADER_LEN;
    }
    let index = walk_blocks(f, path, data_start, file_len)?;
    Ok(PlxFile {
        header,
        spike_channels,
        event_channels,
        continuous_channels,
        data_start,
        file_len,
        index,
    })
}

/// Read the 16-byte block header at `at`: (type, timestamp, channel, unit, waveforms, words).
pub fn block_header(b: &Block, at: u64) -> Option<(u16, u64, u16, u16, u16, u16)> {
    let kind = b.u16_at(at)?;
    let high = u64::from(b.u16_at(at + 2)?);
    let low = u64::from(b.u32_at(at + 4)?);
    Some((
        kind,
        (high << 32) | low,
        b.u16_at(at + 8)?,
        b.u16_at(at + 10)?,
        b.u16_at(at + 12)?,
        b.u16_at(at + 14)?,
    ))
}

/// Walk every data block from `start` to the end of the file.
pub fn walk_blocks(f: &mut File, path: &Path, start: u64, file_len: u64) -> Result<PlxIndex> {
    const CHUNK: u64 = 8 << 20;
    let mut ix = PlxIndex::default();
    let mut pos = start;
    let mut buf = Block::default();
    while pos < file_len {
        if pos + BLOCK_HEADER_LEN > file_len {
            ix.findings.push(
                Finding::error(
                    "truncated",
                    format!(
                        "{} bytes after the last whole data block (a block header is cut off)",
                        file_len - pos
                    ),
                )
                .at(pos),
            );
            break;
        }
        if pos < buf.origin || pos + BLOCK_HEADER_LEN > buf.end() {
            buf = read_block(f, path, pos, CHUNK, file_len)?;
        }
        let Some((kind, ts, ch, unit, waves, words)) = block_header(&buf, pos) else {
            break;
        };
        let samples = u64::from(waves) * u64::from(words);
        let len = BLOCK_HEADER_LEN + 2 * samples;
        if pos + len > file_len {
            ix.findings.push(
                Finding::error(
                    "truncated",
                    format!(
                        "the data block at byte {pos} declares {samples} samples but the file ends {} bytes later (cut off)",
                        file_len - pos
                    ),
                )
                .at(pos),
            );
            break;
        }
        *ix.block_counts.entry(kind).or_default() += 1;
        match kind {
            BLOCK_SPIKE => {
                ix.spikes.push(pos);
                ix.max_waveform = ix
                    .max_waveform
                    .max(u32::try_from(samples).unwrap_or(u32::MAX));
            }
            BLOCK_EVENT => ix.events.push(EventRecord {
                timestamp: ts,
                channel: ch,
                value: unit,
            }),
            BLOCK_CONTINUOUS => ix.continuous.entry(ch).or_default().push(SampleBlock {
                offset: pos + BLOCK_HEADER_LEN,
                timestamp: ts,
                samples: u32::try_from(samples).unwrap_or(u32::MAX),
            }),
            other => {
                ix.findings.push(
                    Finding::error(
                        "bad_block",
                        format!("data block of unknown type {other} at byte {pos}; the rest of the file is not read"),
                    )
                    .at(pos),
                );
                break;
            }
        }
        pos += len;
        ix.data_end = pos;
    }
    if ix.data_end == 0 {
        ix.data_end = start;
    }
    Ok(ix)
}

/// Split one channel's blocks into gap-free runs: a block that starts more than half a sample
/// period away from where the previous one ended starts a new run.
pub fn runs(
    blocks: impl IntoIterator<Item = SampleBlock>,
    clock_hz: f64,
    rate_hz: f64,
) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let ticks = if rate_hz > 0.0 {
        clock_hz / rate_hz
    } else {
        0.0
    };
    let mut expect: Option<f64> = None;
    for (i, b) in blocks.into_iter().enumerate() {
        let start = b.timestamp as f64;
        let joins = expect.is_some_and(|e| (start - e).abs() <= ticks / 2.0);
        match out.last_mut() {
            Some(r) if joins => {
                r.block_count += 1;
                r.samples += u64::from(b.samples);
            }
            _ => out.push(Run {
                first_block: i,
                block_count: 1,
                samples: u64::from(b.samples),
                timestamp: b.timestamp,
            }),
        }
        expect = Some(start + f64::from(b.samples) * ticks);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_list_round_trips() {
        let mut want = Vec::new();
        let mut list = BlockList::default();
        let (mut offset, mut ts) = (1000u64, 5u64 << 33);
        for i in 0..50_000u64 {
            offset += 36 + (i * 7919) % 5000;
            // Mostly forward, sometimes backwards, once a large jump.
            ts = match i {
                500 => ts + (1 << 40),
                _ if i % 97 == 0 => ts - 3,
                _ => ts + 400,
            };
            let b = SampleBlock {
                offset,
                timestamp: ts,
                samples: (10 + i % 3) as u32,
            };
            want.push(b);
            list.push(b);
        }
        assert_eq!(list.len(), want.len());
        assert_eq!(list.iter().collect::<Vec<_>>(), want);
        for (first, count) in [
            (0, 1),
            (63, 2),
            (64, 64),
            (130, 500),
            (49_990, 50),
            (50_000, 5),
        ] {
            let got: Vec<_> = list.range(first, count).collect();
            let end = (first + count).min(want.len());
            assert_eq!(got, want[first.min(end)..end], "range {first}+{count}");
        }
        assert!(BlockList::default().iter().next().is_none());
    }
}
