//! The 32-bit Spike2 `.smr` layout: a 512-byte file header, 140-byte channel headers, and per
//! channel a chain of data blocks (20-byte header, then items). Offsets and meanings from Neo's
//! `Spike2RawIO` (BSD-3) and the corpus (`docs/formats/ced-spike2.md`). The model here
//! ([`SmrFile`], [`SmrChannel`], [`SmrBlock`]) also carries 64-bit `.smrx` files (`file64.rs`).

use std::path::Path;

use openreadout_core::bytes::{Block, latin1, latin1_field, le_i16, read_block, until_nul};
use openreadout_core::model::Finding;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use super::SPIKE2_FORMAT_ID;

/// Bytes of the file header.
pub const SMR_HEADER_LEN: u64 = 512;
/// Bytes of one channel header.
pub const SMR_CHANNEL_LEN: u64 = 140;
/// Bytes of a data-block header.
pub const SMR_BLOCK_HEADER_LEN: u64 = 20;
/// Channel headers accepted at most.
pub const MAX_SMR_CHANNELS: usize = 1024;
/// Blocks followed per channel at most (bounds a damaged chain).
pub const MAX_SMR_BLOCKS: usize = 4_000_000;
/// Text-mark items decoded into the text list at open, at most.
pub const MAX_TEXT_MARKS: usize = 100_000;
/// Copyright text at byte 2 of every `.smr`.
pub const SMR_COPYRIGHT: &[u8; 10] = b"(C) CED 87";

/// What a channel holds (the channel header's kind byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    /// 1: int16 waveform.
    Adc,
    /// 2: falling-edge event times.
    EventFall,
    /// 3: rising-edge event times.
    EventRise,
    /// 4: level (both-edge) event times.
    EventBoth,
    /// 5: event times with four marker bytes.
    Marker,
    /// 6: markers with an int16 waveform (spike shapes).
    AdcMark,
    /// 7: markers with float32 values.
    RealMark,
    /// 8: markers with text.
    TextMark,
    /// 9: float32 waveform.
    RealWave,
}

impl ChannelKind {
    /// From the kind byte (0 = unused → `None`).
    pub fn from_code(c: u8) -> Option<Self> {
        Some(match c {
            1 => Self::Adc,
            2 => Self::EventFall,
            3 => Self::EventRise,
            4 => Self::EventBoth,
            5 => Self::Marker,
            6 => Self::AdcMark,
            7 => Self::RealMark,
            8 => Self::TextMark,
            9 => Self::RealWave,
            _ => return None,
        })
    }
    /// Our name for it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Adc => "adc",
            Self::EventFall => "event-falling",
            Self::EventRise => "event-rising",
            Self::EventBoth => "event-level",
            Self::Marker => "marker",
            Self::AdcMark => "adc-mark",
            Self::RealMark => "real-mark",
            Self::TextMark => "text-mark",
            Self::RealWave => "real-wave",
        }
    }
    /// A sampled waveform (a trace).
    pub fn is_waveform(self) -> bool {
        matches!(self, Self::Adc | Self::RealWave)
    }
    /// Markers with waveforms (spikes table).
    pub fn is_spikes(self) -> bool {
        matches!(self, Self::AdcMark | Self::RealMark)
    }
    /// Plain events, markers and text marks (events table).
    pub fn is_events(self) -> bool {
        matches!(
            self,
            Self::EventFall | Self::EventRise | Self::EventBoth | Self::Marker | Self::TextMark
        )
    }
}

/// One data block of a channel.
#[derive(Debug, Clone, Copy)]
pub struct SmrBlock {
    /// Byte offset of the first item.
    pub offset: u64,
    /// Items in the block.
    pub items: u64,
    /// Time of the first item (ticks).
    pub start: i64,
    /// Time of the last item (ticks).
    pub end: i64,
}

/// One channel header and its block chain.
#[derive(Debug, Clone)]
pub struct SmrChannel {
    /// 1-based channel number (as Spike2 shows it).
    pub number: u32,
    /// Kind.
    pub kind: ChannelKind,
    /// Title.
    pub title: String,
    /// Comment.
    pub comment: String,
    /// Physical (ADC port) channel, −1 when none.
    pub physical: i16,
    /// Ideal sampling or event rate, Hz.
    pub ideal_rate: f64,
    /// Bytes after the time and marker of each marker item (waveform or text).
    pub extra_bytes: u16,
    /// Points before the trigger in each AdcMark waveform.
    pub pre_trigger: i16,
    /// Waveform and AdcMark scale (int16 → unit is × scale / 6553.6).
    pub scale: f64,
    /// Waveform and AdcMark offset.
    pub offset: f64,
    /// Unit text.
    pub unit: String,
    /// Traces interleaved in each AdcMark waveform (1 when not interleaved).
    pub interleave: u16,
    /// Sample interval in ticks (waveforms and AdcMark), when usable.
    pub interval_ticks: Option<u64>,
    /// Level of a level-event channel before its first event is low.
    pub initially_low: Option<bool>,
    /// The block chain.
    pub blocks: Vec<SmrBlock>,
    /// Block count in the header.
    pub header_blocks: u16,
    /// Item times are 64-bit (`.smrx`; 32-bit in `.smr`).
    pub wide_times: bool,
}

impl SmrChannel {
    /// Bytes per item.
    pub fn item_len(&self) -> u64 {
        match self.kind {
            ChannelKind::Adc => 2,
            ChannelKind::RealWave => 4,
            ChannelKind::EventFall | ChannelKind::EventRise | ChannelKind::EventBoth => {
                self.time_len()
            }
            ChannelKind::Marker => self.head_len(),
            ChannelKind::AdcMark | ChannelKind::RealMark | ChannelKind::TextMark => {
                self.head_len() + u64::from(self.extra_bytes)
            }
        }
    }
    /// Bytes of an item's time (4 in `.smr`, 8 in `.smrx`).
    pub fn time_len(&self) -> u64 {
        if self.wide_times { 8 } else { 4 }
    }
    /// Bytes of a marker item's time and marker codes, before its data (8 or 16).
    pub fn head_len(&self) -> u64 {
        2 * self.time_len()
    }
    /// Items over all blocks.
    pub fn item_count(&self) -> u64 {
        self.blocks.iter().map(|b| b.items).sum()
    }
    /// Values per waveform of a marker channel.
    pub fn wave_points(&self) -> u64 {
        match self.kind {
            ChannelKind::AdcMark => u64::from(self.extra_bytes) / 2,
            ChannelKind::RealMark => u64::from(self.extra_bytes) / 4,
            _ => 0,
        }
    }
    /// Scale from stored int16 to the unit.
    pub fn gain(&self) -> f64 {
        self.scale / 6553.6
    }
}

/// The parsed file.
#[derive(Debug, Clone)]
pub struct SmrFile {
    /// Header version (`system_id`).
    pub system_id: i16,
    /// The writing program's code (`S2071431` = Spike2 7.14 build 31, as CED stamps it).
    pub creator: String,
    /// Microseconds (× `time_base_s` / 1e-6) per clock tick.
    pub us_per_time: i16,
    /// Ticks per ADC conversion (before system id 6).
    pub time_per_adc: i16,
    /// Seconds per time unit (`dtime_base`; 1 µs before system id 6).
    pub time_base_s: f64,
    /// Seconds per tick.
    pub tick_s: f64,
    /// Last time in the file (ticks).
    pub max_time: i64,
    /// Recording date and time from the header, when valid.
    pub recorded_at: Option<String>,
    /// File comments.
    pub comments: Vec<String>,
    /// Channels in use.
    pub channels: Vec<SmrChannel>,
    /// Channel headers the header declares.
    pub channel_slots: usize,
    /// File length.
    pub file_len: u64,
    /// Problems met while walking the blocks.
    pub findings: Vec<Finding>,
    /// Distinct text-mark texts, in order of first appearance.
    pub texts: Vec<String>,
    /// For a 64-bit `.smrx`, the two version bytes of its header (6, 7).
    pub son64: Option<[u8; 2]>,
}

fn pascal(b: &Block, at: u64, n: usize) -> String {
    let Some(s) = b.slice(at, n) else {
        return String::new();
    };
    let len = usize::from(s[0]).min(n - 1);
    latin1_field(&s[1..=len])
}

fn printable(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| !c.is_control() && c != '\u{FFFD}')
}

/// True when `head` starts like a 32-bit `.smr`.
pub fn looks_like_smr(head: &[u8]) -> bool {
    head.get(2..12) == Some(&SMR_COPYRIGHT[..])
        && le_i16(head, 0).is_some_and(|v| (1..=9).contains(&v))
}

/// True when `head` starts like a 64-bit `.smrx`.
pub fn looks_like_smrx(head: &[u8]) -> bool {
    head.get(..3) == Some(b"S64")
}

fn date(b: &Block) -> Option<String> {
    let s = b.slice(52, 6)?;
    let year = b.u16_at(58)?;
    let (cs, sec, min, hour, day, month) = (s[0], s[1], s[2], s[3], s[4], s[5]);
    if !(1980..=2100).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || min > 59
        || sec > 59
        || cs > 99
    {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}.{:03}",
        u32::from(cs) * 10
    ))
}

/// Parse the headers and walk every channel's block chain.
pub fn parse_smr(f: &mut SourceFile, path: &Path, file_len: u64) -> Result<SmrFile> {
    let head = read_block(f, path, 0, SMR_HEADER_LEN, file_len)?;
    if head.len() < SMR_HEADER_LEN as usize {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            format!(
                "file is {} bytes, shorter than the 512-byte header",
                head.len()
            ),
        ));
    }
    if head.slice(2, 10) != Some(&SMR_COPYRIGHT[..]) {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            "no `(C) CED 87` signature at byte 2",
        ));
    }
    let system_id = head.i16_at(0).unwrap_or(0);
    if !(1..=9).contains(&system_id) {
        return Err(Error::unsupported(
            SPIKE2_FORMAT_ID,
            format!("Spike2 file header version {system_id}"),
            "the header version is outside 1–9, the 32-bit .smr versions this reader knows; re-save the file from Spike2.",
        ));
    }
    let us_per_time = head.i16_at(20).unwrap_or(0);
    let time_per_adc = head.i16_at(22).unwrap_or(0);
    let nch = head.i16_at(30).unwrap_or(0);
    let max_time = i64::from(head.i32_at(40).unwrap_or(0));
    let time_base_s = if system_id >= 6 {
        head.f64_at(44).unwrap_or(f64::NAN)
    } else {
        1e-6
    };
    let tick_s = f64::from(us_per_time) * time_base_s;
    if !(tick_s.is_finite() && tick_s > 0.0) {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            format!("unusable clock: {us_per_time} time units of {time_base_s} s per tick"),
        ));
    }
    if nch < 0 || nch as usize > MAX_SMR_CHANNELS {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            format!("channel count {nch} is not usable"),
        ));
    }
    let nch = nch as usize;
    let table_len = SMR_CHANNEL_LEN * nch as u64;
    if SMR_HEADER_LEN + table_len > file_len {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            format!(
                "{nch} channel headers need {} bytes but the file has {file_len}",
                SMR_HEADER_LEN + table_len
            ),
        ));
    }
    let comments = (0..5)
        .map(|k| pascal(&head, 107 + 80 * k, 80))
        .filter(|c| printable(c))
        .collect();
    let recorded_at = date(&head);
    let creator = head.text_at(12, 8).unwrap_or_default();
    let ch = read_block(f, path, SMR_HEADER_LEN, table_len, file_len)?;
    let disk = if system_id == 9 { 512 } else { 1 };
    let mut findings = Vec::new();
    let mut channels = Vec::new();
    for i in 0..nch {
        let o = SMR_HEADER_LEN + SMR_CHANNEL_LEN * i as u64;
        let Some(kind) = ch.u8_at(o + 122).and_then(ChannelKind::from_code) else {
            continue;
        };
        let number = i as u32 + 1;
        let first = i64::from(ch.i32_at(o + 6).unwrap_or(-1));
        let last = i64::from(ch.i32_at(o + 10).unwrap_or(-1));
        let header_blocks = ch.u16_at(o + 14).unwrap_or(0);
        let extra_bytes = ch.u16_at(o + 16).unwrap_or(0);
        let pre_trigger = ch.i16_at(o + 18).unwrap_or(0);
        let divider = i64::from(ch.i32_at(o + 102).unwrap_or(0));
        let wave_like = matches!(
            kind,
            ChannelKind::Adc | ChannelKind::AdcMark | ChannelKind::RealMark | ChannelKind::RealWave
        );
        let (scale, offset) = if matches!(kind, ChannelKind::Adc | ChannelKind::AdcMark) {
            (
                ch.f32_at(o + 124).unwrap_or(f32::NAN),
                ch.f32_at(o + 128).unwrap_or(0.0),
            )
        } else {
            (1.0, 0.0)
        };
        let unit = if wave_like {
            pascal(&ch, o + 132, 6)
        } else {
            String::new()
        };
        let d138 = ch.i16_at(o + 138).unwrap_or(0);
        // before system id 6 the field after the unit is the ADC divide; later the interleave
        let (divide, interleave) = if system_id < 6 {
            (i64::from(d138), 1)
        } else {
            (0, d138.max(1) as u16)
        };
        let interval = if system_id < 6 {
            divide.checked_mul(i64::from(time_per_adc))
        } else {
            Some(divider)
        };
        let interval_ticks = interval.filter(|v| *v > 0).map(|v| v as u64);
        let initially_low = (kind == ChannelKind::EventBoth).then(|| ch.u8_at(o + 124) == Some(1));
        let mut c = SmrChannel {
            number,
            kind,
            title: pascal(&ch, o + 108, 10),
            comment: pascal(&ch, o + 26, 72),
            physical: ch.i16_at(o + 106).unwrap_or(-1),
            ideal_rate: f64::from(ch.f32_at(o + 118).unwrap_or(0.0)),
            extra_bytes,
            pre_trigger,
            scale: f64::from(scale),
            offset: f64::from(offset),
            unit,
            interleave: if kind == ChannelKind::AdcMark {
                interleave
            } else {
                1
            },
            interval_ticks: if wave_like { interval_ticks } else { None },
            initially_low,
            blocks: Vec::new(),
            header_blocks,
            wide_times: false,
        };
        // walk the chain: from the first block along next-block links
        let mut pos = first;
        let item = c.item_len();
        let mut last_seen = -1;
        while pos != -1 && c.blocks.len() < MAX_SMR_BLOCKS {
            let Some(at) = u64::try_from(pos).ok().and_then(|p| p.checked_mul(disk)) else {
                findings.push(Finding::error(
                    "bad_block_link",
                    format!("channel {number}: block link {pos} is not an offset"),
                ));
                break;
            };
            if at + SMR_BLOCK_HEADER_LEN > file_len {
                findings.push(
                    Finding::error(
                        "truncated",
                        format!(
                            "channel {number}: block {} at byte {at} lies past the end of the file",
                            c.blocks.len()
                        ),
                    )
                    .at(at),
                );
                break;
            }
            let h = read_block(f, path, at, SMR_BLOCK_HEADER_LEN, file_len)?;
            let succ = i64::from(h.i32_at(at + 4).unwrap_or(-1));
            let start = i64::from(h.i32_at(at + 8).unwrap_or(0));
            let end = i64::from(h.i32_at(at + 12).unwrap_or(0));
            let items = u64::from(h.u16_at(at + 18).unwrap_or(0));
            let data = at + SMR_BLOCK_HEADER_LEN;
            if data + items * item > file_len {
                findings.push(
                    Finding::error(
                        "truncated",
                        format!(
                            "channel {number}: block at byte {at} holds {items} items that run past the end of the file"
                        ),
                    )
                    .at(at),
                );
                break;
            }
            c.blocks.push(SmrBlock {
                offset: data,
                items,
                start,
                end,
            });
            last_seen = pos;
            if succ == pos {
                findings.push(Finding::error(
                    "bad_block_link",
                    format!("channel {number}: block at byte {at} links to itself"),
                ));
                break;
            }
            pos = succ;
        }
        if c.blocks.len() != usize::from(header_blocks) {
            findings.push(Finding::warning(
                "block_count_mismatch",
                format!(
                    "channel {number}: the header counts {header_blocks} blocks, the chain holds {}",
                    c.blocks.len()
                ),
            ));
        }
        if !c.blocks.is_empty() && last_seen != last {
            findings.push(Finding::warning(
                "last_block_mismatch",
                format!(
                    "channel {number}: the chain ends at block {last_seen}, the header says {last}"
                ),
            ));
        }
        if wave_like && c.interval_ticks.is_none() && !c.blocks.is_empty() {
            findings.push(Finding::error(
                "bad_interval",
                format!("channel {number}: sample interval is not positive"),
            ));
        }
        channels.push(c);
    }
    let mut out = SmrFile {
        system_id,
        creator,
        us_per_time,
        time_per_adc,
        time_base_s,
        tick_s,
        max_time,
        recorded_at,
        comments,
        channels,
        channel_slots: nch,
        file_len,
        findings,
        texts: Vec::new(),
        son64: None,
    };
    out.texts = collect_texts(f, path, &out)?;
    Ok(out)
}

/// Text of one text-mark item.
pub fn item_text(bytes: &[u8]) -> String {
    latin1(until_nul(bytes))
}

pub(crate) fn collect_texts(
    f: &mut SourceFile,
    path: &Path,
    file: &SmrFile,
) -> Result<Vec<String>> {
    let mut texts: Vec<String> = Vec::new();
    let mut n = 0usize;
    for c in file
        .channels
        .iter()
        .filter(|c| c.kind == ChannelKind::TextMark)
    {
        let item = c.item_len();
        for b in &c.blocks {
            let block = read_block(f, path, b.offset, b.items * item, file.file_len)?;
            for k in 0..b.items {
                n += 1;
                if n > MAX_TEXT_MARKS {
                    return Ok(texts);
                }
                let at = b.offset + k * item + c.head_len();
                let t = block
                    .slice(at, usize::from(c.extra_bytes))
                    .map(item_text)
                    .unwrap_or_default();
                if !texts.contains(&t) {
                    texts.push(t);
                }
            }
        }
    }
    Ok(texts)
}

/// Gap-free runs (sweeps) of a waveform channel: (first block, block count, samples).
pub fn segments(c: &SmrChannel) -> Vec<(usize, usize, u64)> {
    let mut out: Vec<(usize, usize, u64)> = Vec::new();
    let interval = c.interval_ticks.map_or(0, |v| v as i64);
    let mut prev_end: Option<i64> = None;
    for (k, b) in c.blocks.iter().enumerate() {
        if b.items == 0 {
            continue;
        }
        let join = prev_end.is_some_and(|e| b.start - e <= interval) && !out.is_empty();
        if join {
            let last = out.last_mut().expect("non-empty");
            last.1 = k + 1 - last.0;
            last.2 += b.items;
        } else {
            out.push((k, 1, b.items));
        }
        prev_end = Some(b.end);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_and_signatures() {
        assert_eq!(ChannelKind::from_code(0), None);
        assert!(ChannelKind::from_code(1).unwrap().is_waveform());
        assert!(ChannelKind::from_code(6).unwrap().is_spikes());
        assert!(ChannelKind::from_code(8).unwrap().is_events());
        let mut h = vec![0u8; 16];
        h[0] = 6;
        h[2..12].copy_from_slice(SMR_COPYRIGHT);
        assert!(looks_like_smr(&h));
        h[0] = 12;
        assert!(!looks_like_smr(&h));
        assert!(looks_like_smrx(b"S64pl\0"));
    }

    #[test]
    fn dates() {
        let mut b = vec![0u8; 64];
        b[52..58].copy_from_slice(&[17, 23, 29, 10, 10, 6]);
        b[58..60].copy_from_slice(&2022u16.to_le_bytes());
        let blk = Block {
            origin: 0,
            bytes: b.clone(),
        };
        assert_eq!(date(&blk).as_deref(), Some("2022-06-10T10:29:23.170"));
        let blk = Block {
            origin: 0,
            bytes: vec![0u8; 64],
        };
        assert_eq!(date(&blk), None);
    }

    fn chan(blocks: &[(i64, i64, u64)]) -> SmrChannel {
        SmrChannel {
            number: 1,
            kind: ChannelKind::Adc,
            title: String::new(),
            comment: String::new(),
            physical: 0,
            ideal_rate: 0.0,
            extra_bytes: 0,
            pre_trigger: 0,
            scale: 1.0,
            offset: 0.0,
            unit: String::new(),
            interleave: 1,
            interval_ticks: Some(10),
            initially_low: None,
            blocks: blocks
                .iter()
                .map(|&(start, end, items)| SmrBlock {
                    offset: 0,
                    items,
                    start,
                    end,
                })
                .collect(),
            header_blocks: blocks.len() as u16,
            wide_times: false,
        }
    }

    #[test]
    fn pauses_split_segments() {
        // two contiguous blocks, a pause, one block
        let c = chan(&[(0, 90, 10), (100, 190, 10), (500, 590, 10)]);
        assert_eq!(segments(&c), vec![(0, 2, 20), (2, 1, 10)]);
    }
}
