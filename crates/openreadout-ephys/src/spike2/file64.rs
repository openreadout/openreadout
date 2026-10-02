//! The 64-bit Spike2 `.smrx` layout, derived from the files and the Spike2 software's own exports
//! of them (no CED documentation or library; `docs/provenance/ced-spike2.md`): a header stream
//! (the first 64 KiB plus extra header blocks) holding the file header, 272-byte channel records
//! and a string table; per channel an index tree of 4 KiB index blocks over 64 KiB data blocks.
//! Parsed into the same [`SmrFile`] model as `.smr` files (`docs/formats/ced-spike2.md`).

use std::collections::HashSet;
use std::path::Path;

use openreadout_core::bytes::{Block, latin1, le_u32, read_block};
use openreadout_core::model::Finding;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use super::SPIKE2_FORMAT_ID;
use super::file::{ChannelKind, MAX_SMR_BLOCKS, MAX_SMR_CHANNELS, SmrBlock, SmrChannel, SmrFile};

/// Bytes of the first header block (and of every data block).
pub const SMRX_BLOCK_LEN: u64 = 1 << 16;
/// Bytes of an index block.
pub const SMRX_INDEX_LEN: u64 = 4096;
/// Bytes of a block header (parent and level, channel, generation, count).
pub const SMRX_BLOCK_HEADER_LEN: u64 = 16;
/// Entries of an index block at most ((4096 − 16) / 16).
pub const SMRX_INDEX_ENTRIES: u64 = 255;
/// Extra header blocks accepted at most (the header stream stays below 64 MiB).
pub const MAX_SMRX_HEADER_BLOCKS: u64 = 1024;
/// Strings of the string table accepted at most.
pub const MAX_SMRX_STRINGS: u32 = 1 << 20;
/// Index-tree levels followed at most.
pub const MAX_SMRX_LEVELS: u32 = 6;

/// The header stream: the first 64 KiB of the file, then each extra header block after its
/// 16-byte block header. Offsets in the file header and channel records count in this stream.
#[derive(Debug, Clone, Default)]
pub struct HeaderStream {
    /// The concatenated bytes.
    pub bytes: Vec<u8>,
}

impl HeaderStream {
    fn block(&self) -> Block {
        Block {
            origin: 0,
            bytes: self.bytes.clone(),
        }
    }
}

/// Read the header stream (file header, channel records, string table).
pub fn header_stream(f: &mut SourceFile, path: &Path, file_len: u64) -> Result<HeaderStream> {
    let first = read_block(f, path, 0, SMRX_BLOCK_LEN.min(file_len), file_len)?;
    if first.len() < 1024 + 8 {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            format!(
                "file is {} bytes, shorter than the .smrx header",
                first.len()
            ),
        ));
    }
    let mut bytes = first.bytes;
    let extra = u64::from(le_u32(&bytes, 996).unwrap_or(0));
    if extra > MAX_SMRX_HEADER_BLOCKS {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            format!("{extra} extra header blocks declared"),
        ));
    }
    let list = Block {
        origin: 0,
        bytes: bytes.clone(),
    };
    for k in 0..extra {
        let Some(at) = list.u64_at(1024 + 8 * k) else {
            return Err(Error::corrupt(
                SPIKE2_FORMAT_ID,
                "the header block list runs past the first header block",
            ));
        };
        let end = at.saturating_add(SMRX_BLOCK_LEN);
        if at < SMRX_BLOCK_LEN || end > file_len {
            return Err(Error::corrupt_at(
                SPIKE2_FORMAT_ID,
                at,
                format!("header block {k} at byte {at} lies outside the file"),
            ));
        }
        let b = read_block(
            f,
            path,
            at + SMRX_BLOCK_HEADER_LEN,
            SMRX_BLOCK_LEN - SMRX_BLOCK_HEADER_LEN,
            file_len,
        )?;
        bytes.extend_from_slice(&b.bytes);
    }
    Ok(HeaderStream { bytes })
}

/// The string table: 1-based string `k` is `strings[k − 1]`.
pub fn string_table(h: &Block, at: u64) -> Vec<String> {
    let mut out = Vec::new();
    let Some(count) = h.u32_at(at + 4) else {
        return out;
    };
    let mut o = at + 8;
    for _ in 0..count.min(MAX_SMRX_STRINGS) {
        let Some(rest) = h.slice(o + 4, h.bytes.len().saturating_sub((o + 4) as usize)) else {
            break;
        };
        let Some(nul) = rest.iter().position(|&c| c == 0) else {
            break;
        };
        out.push(latin1(&rest[..nul]));
        let len = (nul as u64 + 1 + 3) & !3;
        o += 4 + len;
    }
    out
}

fn text(strings: &[String], k: u32) -> String {
    usize::try_from(k)
        .ok()
        .and_then(|k| k.checked_sub(1))
        .and_then(|k| strings.get(k))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn date(h: &Block) -> Option<String> {
    let s = h.slice(24, 6)?;
    let year = h.u16_at(30)?;
    let (cs, sec, min, hour, day, month) = (s[0], s[1], s[2], s[3], s[4], s[5]);
    if !(1980..=2200).contains(&year)
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

/// Level (0 data, 1 index of data blocks, 2 index of index blocks, …) of a block's first word.
pub fn block_level(word: u64) -> u32 {
    ((word >> 8) & 0xF) as u32
}

struct Walk<'a> {
    f: &'a mut SourceFile,
    path: &'a Path,
    file_len: u64,
    chan: u16,
    generation: u16,
    live: u64,
    data_blocks: u64,
    visited: HashSet<u64>,
    findings: Vec<Finding>,
    blocks: Vec<SmrBlock>,
    number: u32,
}

impl Walk<'_> {
    fn bad(&mut self, at: u64, what: String) {
        self.findings
            .push(Finding::error("truncated", format!("channel {}: {what}", self.number)).at(at));
    }

    /// Follow index block `at` (its level from its own header).
    fn index(&mut self, at: u64, depth: u32, c: &SmrChannel) -> Result<()> {
        if depth > MAX_SMRX_LEVELS || !self.visited.insert(at) {
            self.findings.push(Finding::error(
                "bad_block_link",
                format!(
                    "channel {}: index block at byte {at} repeats or nests too deep",
                    self.number
                ),
            ));
            return Ok(());
        }
        if at
            .checked_add(SMRX_INDEX_LEN)
            .is_none_or(|e| e > self.file_len)
        {
            self.bad(
                at,
                format!("index block at byte {at} lies past the end of the file"),
            );
            return Ok(());
        }
        let h = read_block(self.f, self.path, at, SMRX_INDEX_LEN, self.file_len)?;
        let level = block_level(h.u64_at(at).unwrap_or(0));
        let n = u64::from(h.u32_at(at + 12).unwrap_or(0)).min(SMRX_INDEX_ENTRIES);
        for k in 0..n {
            if self.data_blocks >= self.live || self.blocks.len() >= MAX_SMR_BLOCKS {
                return Ok(());
            }
            let e = at + SMRX_BLOCK_HEADER_LEN + 16 * k;
            let child = h.u64_at(e + 8).unwrap_or(0);
            if level > 1 {
                self.index(child, depth + 1, c)?;
            } else {
                self.data(child, c)?;
            }
        }
        Ok(())
    }

    /// One data block: skipped unless its channel and generation are the record's.
    fn data(&mut self, at: u64, c: &SmrChannel) -> Result<()> {
        if at
            .checked_add(SMRX_BLOCK_HEADER_LEN)
            .is_none_or(|e| e > self.file_len)
        {
            self.bad(
                at,
                format!("data block at byte {at} lies past the end of the file"),
            );
            return Ok(());
        }
        let head = read_block(self.f, self.path, at, SMRX_BLOCK_HEADER_LEN, self.file_len)?;
        let chan = head.u16_at(at + 8).unwrap_or(u16::MAX);
        let generation = head.u16_at(at + 10).unwrap_or(u16::MAX);
        if chan != self.chan || generation != self.generation {
            return Ok(()); // a block left over from a deleted channel in this slot
        }
        self.data_blocks += 1;
        let count = u64::from(head.u32_at(at + 12).unwrap_or(0));
        let block_end = at.saturating_add(SMRX_BLOCK_LEN).min(self.file_len);
        let first = at + SMRX_BLOCK_HEADER_LEN;
        if c.kind.is_waveform() {
            let width = c.item_len();
            let interval = c.interval_ticks.unwrap_or(0) as i64;
            let mut pos = first;
            for _ in 0..count {
                if pos + 16 > block_end {
                    self.bad(pos, format!("run header at byte {pos} runs past its block"));
                    break;
                }
                let run = read_block(self.f, self.path, pos, 16, self.file_len)?;
                let start = run.i64_at(pos).unwrap_or(0);
                let items = run.u64_at(pos + 8).unwrap_or(0);
                let end_byte = items
                    .checked_mul(width)
                    .and_then(|bytes| bytes.checked_add(pos + 16))
                    .unwrap_or(u64::MAX);
                if end_byte > block_end {
                    self.bad(
                        pos,
                        format!("run of {items} samples at byte {pos} runs past its block"),
                    );
                    break;
                }
                let last = i64::try_from(items.saturating_sub(1))
                    .ok()
                    .and_then(|k| k.checked_mul(interval))
                    .and_then(|d| start.checked_add(d))
                    .unwrap_or(start);
                self.blocks.push(SmrBlock {
                    offset: pos + 16,
                    items,
                    start,
                    end: last,
                });
                pos = end_byte;
            }
        } else {
            let item = c.item_len();
            let end_byte = count
                .checked_mul(item)
                .and_then(|bytes| bytes.checked_add(first))
                .unwrap_or(u64::MAX);
            if end_byte > block_end {
                self.bad(
                    at,
                    format!("{count} items at byte {at} run past their block"),
                );
                return Ok(());
            }
            let (mut start, mut end) = (0, 0);
            if count > 0 {
                let first_item = read_block(self.f, self.path, first, 8, self.file_len)?;
                start = first_item.i64_at(first).unwrap_or(0);
                let last_at = first + (count - 1) * item;
                let last_item = read_block(self.f, self.path, last_at, 8, self.file_len)?;
                end = last_item.i64_at(last_at).unwrap_or(0);
            }
            self.blocks.push(SmrBlock {
                offset: first,
                items: count,
                start,
                end,
            });
        }
        Ok(())
    }
}

/// Parse the header stream and walk every channel's index tree.
pub fn parse_smrx(f: &mut SourceFile, path: &Path, file_len: u64) -> Result<SmrFile> {
    let hs = header_stream(f, path, file_len)?;
    let h = hs.block();
    if h.slice(0, 3) != Some(b"S64") {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            "no `S64` signature at byte 0",
        ));
    }
    let version = [h.u8_at(6).unwrap_or(0), h.u8_at(7).unwrap_or(0)];
    let tick_s = h.f64_at(32).unwrap_or(f64::NAN);
    if !(tick_s.is_finite() && tick_s > 0.0 && tick_s < 1.0) {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            format!("unusable clock: {tick_s} s per tick"),
        ));
    }
    let table = u64::from(h.u32_at(44).unwrap_or(0));
    let strings_at = u64::from(h.u32_at(52).unwrap_or(0));
    let nch = h.u32_at(56).unwrap_or(0) as usize;
    let rec = u64::from(h.u32_at(60).unwrap_or(0));
    if nch > MAX_SMR_CHANNELS || !(0x70..=4096).contains(&rec) {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            format!("{nch} channel records of {rec} bytes are not usable"),
        ));
    }
    let table_end = table.saturating_add(rec * nch as u64);
    if table_end > h.bytes.len() as u64 || strings_at > h.bytes.len() as u64 {
        return Err(Error::corrupt(
            SPIKE2_FORMAT_ID,
            format!(
                "the channel table (bytes {table}–{table_end}) or the string table (byte {strings_at}) lies outside the {}-byte header",
                h.bytes.len()
            ),
        ));
    }
    let strings = string_table(&h, strings_at);
    let comments: Vec<String> = (0..5)
        .filter_map(|k| h.u32_at(64 + 4 * k))
        .filter(|&k| k != 0)
        .map(|k| text(&strings, k))
        .filter(|comment| !comment.is_empty())
        .collect();
    let creator = h.text_at(16, 8).unwrap_or_default();
    let max_time = h.i64_at(1016).unwrap_or(0);
    let mut findings = Vec::new();
    let mut channels = Vec::new();
    for i in 0..nch {
        let rec_at = table + rec * i as u64;
        let Some(kind) = h.u8_at(rec_at + 0x2e).and_then(ChannelKind::from_code) else {
            continue;
        };
        let number = i as u32 + 1;
        let root = h.u64_at(rec_at).unwrap_or(0);
        let live = h.u64_at(rec_at + 0x10).unwrap_or(0);
        let item = u64::from(h.u32_at(rec_at + 0x20).unwrap_or(0));
        let points = h.u16_at(rec_at + 0x24).unwrap_or(0);
        let traces = h.u16_at(rec_at + 0x26).unwrap_or(0).max(1);
        let pre = h.u16_at(rec_at + 0x28).unwrap_or(0);
        let generation = h.u16_at(rec_at + 0x2c).unwrap_or(0);
        let divide = h.u64_at(rec_at + 0x40).unwrap_or(0);
        let spikes = kind.is_spikes() || kind == ChannelKind::TextMark;
        let wave_like = kind.is_waveform() || kind.is_spikes();
        let extra_bytes = if spikes {
            u16::try_from(item.saturating_sub(16)).unwrap_or(u16::MAX)
        } else {
            0
        };
        let mut chan = SmrChannel {
            number,
            kind,
            title: text(&strings, h.u32_at(rec_at + 0x34).unwrap_or(0)),
            unit: text(&strings, h.u32_at(rec_at + 0x38).unwrap_or(0)),
            comment: text(&strings, h.u32_at(rec_at + 0x3c).unwrap_or(0)),
            physical: i16::try_from(h.i32_at(rec_at + 0x30).unwrap_or(-1)).unwrap_or(-1),
            ideal_rate: h.f64_at(rec_at + 0x48).unwrap_or(0.0),
            extra_bytes,
            pre_trigger: i16::try_from(pre).unwrap_or(0),
            scale: if matches!(kind, ChannelKind::Adc | ChannelKind::AdcMark) {
                h.f64_at(rec_at + 0x50).unwrap_or(f64::NAN)
            } else {
                1.0
            },
            offset: if matches!(kind, ChannelKind::Adc | ChannelKind::AdcMark) {
                h.f64_at(rec_at + 0x58).unwrap_or(0.0)
            } else {
                0.0
            },
            interleave: if kind == ChannelKind::AdcMark {
                traces
            } else {
                1
            },
            interval_ticks: (wave_like && divide > 0).then_some(divide),
            initially_low: None,
            blocks: Vec::new(),
            header_blocks: u16::try_from(live).unwrap_or(u16::MAX),
            wide_times: true,
        };
        let expected_item = match kind {
            ChannelKind::Adc => Some(2),
            ChannelKind::RealWave => Some(4),
            ChannelKind::EventFall | ChannelKind::EventRise | ChannelKind::EventBoth => Some(8),
            ChannelKind::Marker => Some(16),
            _ => None,
        };
        if expected_item.is_some_and(|size| size != item) || (spikes && item < 16) {
            findings.push(Finding::error(
                "bad_item_size",
                format!("channel {number} ({}): items of {item} bytes", kind.name()),
            ));
            continue;
        }
        if kind == ChannelKind::AdcMark
            && u64::from(points) * u64::from(traces) * 2 != u64::from(extra_bytes)
        {
            findings.push(Finding::warning(
                "marker_size_mismatch",
                format!(
                    "channel {number}: {points} points × {traces} traces do not fill {extra_bytes} bytes"
                ),
            ));
        }
        if wave_like && chan.interval_ticks.is_none() {
            findings.push(Finding::error(
                "bad_interval",
                format!("channel {number}: sample interval is not positive"),
            ));
        }
        if root != 0 && live > 0 {
            let mut walk = Walk {
                f,
                path,
                file_len,
                chan: u16::try_from(i).unwrap_or(u16::MAX),
                generation,
                live,
                data_blocks: 0,
                visited: HashSet::new(),
                findings: Vec::new(),
                blocks: Vec::new(),
                number,
            };
            walk.index(root, 1, &chan)?;
            if walk.data_blocks != live {
                walk.findings.push(Finding::warning(
                    "block_count_mismatch",
                    format!(
                        "channel {number}: the record counts {live} data blocks, the index holds {} of this channel",
                        walk.data_blocks
                    ),
                ));
            }
            findings.append(&mut walk.findings);
            chan.blocks = walk.blocks;
        }
        channels.push(chan);
    }
    let mut out = SmrFile {
        system_id: 0,
        creator,
        us_per_time: 1,
        time_per_adc: 1,
        time_base_s: tick_s,
        tick_s,
        max_time,
        recorded_at: date(&h),
        comments,
        channels,
        channel_slots: nch,
        file_len,
        findings,
        texts: Vec::new(),
        son64: Some(version),
    };
    out.texts = super::file::collect_texts(f, path, &out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_strings() {
        assert_eq!(block_level(0x0002_d101), 1);
        assert_eq!(block_level(0x200), 2);
        assert_eq!(block_level(0x0002_0005), 0);
        let mut b = vec![0u8; 8];
        b[4] = 3; // three strings
        for (r, s) in [(1u32, "A"), (2, "mV"), (1, "abcd")] {
            b.extend_from_slice(&r.to_le_bytes());
            let mut t = s.as_bytes().to_vec();
            t.push(0);
            while t.len() % 4 != 0 {
                t.push(0);
            }
            b.extend_from_slice(&t);
        }
        let blk = Block {
            origin: 0,
            bytes: b,
        };
        let s = string_table(&blk, 0);
        assert_eq!(s, vec!["A", "mV", "abcd"]);
        assert_eq!(text(&s, 2), "mV");
        assert_eq!(text(&s, 0), "");
        assert_eq!(text(&s, 9), "");
    }

    #[test]
    fn header_dates() {
        let mut b = vec![0u8; 40];
        b[24..30].copy_from_slice(&[0x45, 0x39, 0x20, 0x0b, 0x03, 0x03]);
        b[30..32].copy_from_slice(&2020u16.to_le_bytes());
        let blk = Block {
            origin: 0,
            bytes: b,
        };
        assert_eq!(date(&blk).as_deref(), Some("2020-03-03T11:32:57.690"));
    }
}
