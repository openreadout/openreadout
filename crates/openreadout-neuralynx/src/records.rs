//! Record layouts (Neuralynx "Data File Formats", Rev 1.1) and the continuous-record index.

use std::path::Path;

use openreadout_core::bytes::{Block, read_block};
use openreadout_core::model::Finding;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::header::HEADER_LEN;

/// Samples per continuous record.
pub const NCS_SAMPLES: u64 = 512;
/// Bytes of a continuous (CSC) record: u64 timestamp, u32 channel, u32 rate, u32 valid, `i16[512]`.
pub const NCS_RECORD_LEN: u64 = 20 + 2 * NCS_SAMPLES;
/// Bytes of an event record.
pub const NEV_RECORD_LEN: u64 = 184;
/// Samples per spike waveform and channel.
pub const SPIKE_SAMPLES: u64 = 32;
/// Features per spike record.
pub const SPIKE_FEATURES: usize = 8;
/// Bytes before the waveform in a spike record.
pub const SPIKE_PREFIX_LEN: u64 = 48;
/// Bytes of a video-tracker record: u16 ×3, u64 timestamp, `u32[400]` points, i16, i32 ×3,
/// `i32[50]` targets.
pub const NVT_RECORD_LEN: u64 = 1828;
/// A video-tracker record's first field.
pub const NVT_RECORD_START: u16 = 0x800;
/// Colour-transition points and targets per video-tracker record.
pub const NVT_POINTS: usize = 400;
pub const NVT_TARGETS: usize = 50;

/// What a file holds, from its extension, `-FileType` and `-RecordSize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// `.ncs`: continuously sampled channel.
    Continuous,
    /// `.nev`: events.
    Events,
    /// `.nse` / `.nst` / `.ntt`: spike waveforms from 1, 2 or 4 electrodes.
    Spikes { electrodes: u8 },
    /// `.nvt`: video-tracker positions.
    Video,
}

impl FileKind {
    pub fn record_len(self) -> u64 {
        match self {
            FileKind::Continuous => NCS_RECORD_LEN,
            FileKind::Events => NEV_RECORD_LEN,
            FileKind::Video => NVT_RECORD_LEN,
            FileKind::Spikes { electrodes } => {
                SPIKE_PREFIX_LEN + 2 * SPIKE_SAMPLES * u64::from(electrodes)
            }
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            FileKind::Continuous => "ncs",
            FileKind::Events => "nev",
            FileKind::Video => "nvt",
            FileKind::Spikes { electrodes: 1 } => "nse",
            FileKind::Spikes { electrodes: 2 } => "nst",
            FileKind::Spikes { .. } => "ntt",
        }
    }
    /// From an extension (case-insensitive).
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "ncs" => Some(FileKind::Continuous),
            "nev" => Some(FileKind::Events),
            "nvt" => Some(FileKind::Video),
            "nse" => Some(FileKind::Spikes { electrodes: 1 }),
            "nst" => Some(FileKind::Spikes { electrodes: 2 }),
            "ntt" => Some(FileKind::Spikes { electrodes: 4 }),
            _ => None,
        }
    }
    /// From `-RecordSize` alone.
    pub fn from_record_len(n: u64) -> Option<Self> {
        match n {
            NCS_RECORD_LEN => Some(FileKind::Continuous),
            NEV_RECORD_LEN => Some(FileKind::Events),
            NVT_RECORD_LEN => Some(FileKind::Video),
            112 => Some(FileKind::Spikes { electrodes: 1 }),
            176 => Some(FileKind::Spikes { electrodes: 2 }),
            304 => Some(FileKind::Spikes { electrodes: 4 }),
            _ => None,
        }
    }
}

/// Header fields of one continuous record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHead {
    pub timestamp_us: u64,
    pub channel_number: u32,
    pub rate_hz: u32,
    pub valid: u32,
}

/// A run of records whose timestamps follow each other without a gap (one sweep).
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub first_record: u64,
    pub record_count: u64,
    pub sample_count: u64,
    pub first_timestamp_us: u64,
}

/// Where every sample of a continuous file is.
#[derive(Debug, Clone)]
pub struct ContinuousIndex {
    pub record_count: u64,
    /// Valid samples per record, when not every record is full (else empty).
    pub valid: Vec<u16>,
    pub segments: Vec<Segment>,
    /// Median µs between samples, from the timestamps of consecutive full records.
    pub sample_interval_us: Option<f64>,
    /// True when every record header was read (not only the first and last).
    pub scanned: bool,
    pub first: Option<RecordHead>,
    pub last: Option<RecordHead>,
    pub findings: Vec<Finding>,
}

impl ContinuousIndex {
    /// Valid samples of record `r`.
    pub fn valid_in(&self, r: u64) -> u64 {
        match self.valid.get(r as usize) {
            Some(v) => u64::from(*v),
            None if r + 1 == self.record_count => self
                .last
                .map_or(NCS_SAMPLES, |l| u64::from(l.valid.min(512))),
            None => NCS_SAMPLES,
        }
    }
}

/// Parse one continuous-record header at `at` in `b`.
pub fn record_head(b: &Block, at: u64) -> Option<RecordHead> {
    Some(RecordHead {
        timestamp_us: b.u64_at(at)?,
        channel_number: b.u32_at(at + 8)?,
        rate_hz: b.u32_at(at + 12)?,
        valid: b.u32_at(at + 16)?,
    })
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

/// Build segments from full record heads: a new segment where the next timestamp departs from
/// `ts + valid × dt` by more than half a sample.
pub fn segments_from(
    heads: &[RecordHead],
    header_rate: Option<f64>,
) -> (Vec<Segment>, Option<f64>) {
    let deltas: Vec<f64> = heads
        .windows(2)
        .filter(|w| w[0].valid == 512 && w[1].timestamp_us > w[0].timestamp_us)
        .map(|w| (w[1].timestamp_us - w[0].timestamp_us) as f64 / NCS_SAMPLES as f64)
        .collect();
    let dt = median(deltas).or_else(|| header_rate.filter(|r| *r > 0.0).map(|r| 1e6 / r));
    let mut segs: Vec<Segment> = Vec::new();
    for (i, h) in heads.iter().enumerate() {
        let valid = u64::from(h.valid.min(512));
        let start_new = match (segs.last(), i) {
            (None, _) => true,
            (Some(_), i) => {
                let p = &heads[i - 1];
                match dt {
                    Some(dt) => {
                        let expect = p.timestamp_us as f64 + f64::from(p.valid.min(512)) * dt;
                        (h.timestamp_us as f64 - expect).abs() > dt / 2.0
                    }
                    None => false,
                }
            }
        };
        if start_new {
            segs.push(Segment {
                first_record: i as u64,
                record_count: 0,
                sample_count: 0,
                first_timestamp_us: h.timestamp_us,
            });
        }
        let s = segs.last_mut().expect("pushed above");
        s.record_count += 1;
        s.sample_count += valid;
    }
    (segs, dt)
}

/// Read the record index of a continuous file. With `full`, every record header is read;
/// otherwise only the first and last when they show one gap-free run of full records.
pub fn index_continuous(
    f: &mut SourceFile,
    path: &Path,
    file_len: u64,
    header_rate: Option<f64>,
    full: bool,
) -> Result<ContinuousIndex> {
    let data_len = file_len.saturating_sub(HEADER_LEN);
    let record_count = data_len / NCS_RECORD_LEN;
    let mut findings = Vec::new();
    if !data_len.is_multiple_of(NCS_RECORD_LEN) {
        findings.push(
            Finding::error(
                "truncated",
                format!(
                    "{} bytes after the last whole record (a {NCS_RECORD_LEN}-byte record is cut off)",
                    data_len % NCS_RECORD_LEN
                ),
            )
            .at(HEADER_LEN + record_count * NCS_RECORD_LEN),
        );
    }
    let mut idx = ContinuousIndex {
        record_count,
        valid: Vec::new(),
        segments: Vec::new(),
        sample_interval_us: None,
        scanned: false,
        first: None,
        last: None,
        findings,
    };
    if record_count == 0 {
        return Ok(idx);
    }
    let first = record_head(&read_block(f, path, HEADER_LEN, 20, file_len)?, HEADER_LEN)
        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, HEADER_LEN, "first record unreadable"))?;
    let last_at = HEADER_LEN + (record_count - 1) * NCS_RECORD_LEN;
    let last = record_head(&read_block(f, path, last_at, 20, file_len)?, last_at)
        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, last_at, "last record unreadable"))?;
    idx.first = Some(first);
    idx.last = Some(last);
    if !full
        && first.valid == 512
        && record_count > 1
        && let Some(rate) = header_rate.filter(|hz| *hz > 0.0)
    {
        {
            let dt = 1e6 / rate;
            let expect = first.timestamp_us as f64 + (record_count - 1) as f64 * 512.0 * dt;
            if (last.timestamp_us as f64 - expect).abs() <= dt / 2.0 && last.valid <= 512 {
                idx.sample_interval_us = Some(
                    (last.timestamp_us - first.timestamp_us) as f64
                        / ((record_count - 1) * NCS_SAMPLES) as f64,
                );
                idx.segments = vec![Segment {
                    first_record: 0,
                    record_count,
                    sample_count: (record_count - 1) * NCS_SAMPLES + u64::from(last.valid),
                    first_timestamp_us: first.timestamp_us,
                }];
                return Ok(idx);
            }
        }
    }
    if record_count == 1 {
        idx.scanned = true;
        idx.segments = vec![Segment {
            first_record: 0,
            record_count: 1,
            sample_count: u64::from(first.valid.min(512)),
            first_timestamp_us: first.timestamp_us,
        }];
        return Ok(idx);
    }
    let per_chunk = 4096u64;
    let mut heads = Vec::with_capacity(usize::try_from(record_count).unwrap_or(0).min(1 << 24));
    let mut done = 0u64;
    while done < record_count {
        let count = per_chunk.min(record_count - done);
        let at = HEADER_LEN + done * NCS_RECORD_LEN;
        let chunk = read_block(f, path, at, count * NCS_RECORD_LEN, file_len)?;
        for k in 0..count {
            let head = record_head(&chunk, at + k * NCS_RECORD_LEN).ok_or_else(|| {
                Error::corrupt_at(FORMAT_ID, at + k * NCS_RECORD_LEN, "record unreadable")
            })?;
            heads.push(head);
        }
        done += count;
    }
    for (i, head) in heads.iter().enumerate() {
        if head.valid > 512 {
            idx.findings.push(
                Finding::error(
                    "bad_valid_count",
                    format!(
                        "record {i} claims {} valid samples (at most 512)",
                        head.valid
                    ),
                )
                .at(HEADER_LEN + i as u64 * NCS_RECORD_LEN),
            );
        }
    }
    let non_monotonic = heads
        .windows(2)
        .filter(|w| w[1].timestamp_us < w[0].timestamp_us)
        .count();
    if non_monotonic > 0 {
        idx.findings.push(Finding::warning(
            "timestamps_decrease",
            format!("{non_monotonic} record timestamps go backwards"),
        ));
    }
    let channels: std::collections::BTreeSet<u32> =
        heads.iter().map(|head| head.channel_number).collect();
    if channels.len() > 1 {
        idx.findings.push(Finding::warning(
            "mixed_channels",
            format!("records carry {} different channel numbers", channels.len()),
        ));
    }
    if heads.iter().any(|head| head.valid != 512) {
        idx.valid = heads
            .iter()
            .map(|head| head.valid.min(512) as u16)
            .collect();
    }
    let (segs, dt) = segments_from(&heads, header_rate);
    idx.segments = segs;
    idx.sample_interval_us = dt;
    idx.scanned = true;
    Ok(idx)
}

/// One event record.
#[derive(Debug, Clone, PartialEq)]
pub struct EventRecord {
    pub packet_id: i16,
    pub data_size: i16,
    pub timestamp_us: u64,
    pub event_id: i16,
    pub ttl: i16,
    pub extra: [i32; 8],
    pub label: String,
}

/// Parse the event record at `at`.
pub fn event_record(b: &Block, at: u64) -> Option<EventRecord> {
    let mut extra = [0i32; 8];
    for (k, e) in extra.iter_mut().enumerate() {
        *e = b.i32_at(at + 24 + 4 * k as u64)?;
    }
    Some(EventRecord {
        packet_id: b.i16_at(at + 2)?,
        data_size: b.i16_at(at + 4)?,
        timestamp_us: b.u64_at(at + 6)?,
        event_id: b.i16_at(at + 14)?,
        ttl: b.i16_at(at + 16)?,
        extra,
        label: crate::header::decode_text(b.slice(at + 56, 128)?)
            .trim()
            .to_string(),
    })
}

/// One video-tracker record (the colour-transition points are not kept).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoRecord {
    /// Always `NVT_RECORD_START` in a well-formed record.
    pub start: u16,
    pub system_id: u16,
    pub data_size: u16,
    pub timestamp_us: u64,
    /// Extracted target position, pixels (0, 0 when nothing was tracked).
    pub x: i32,
    pub y: i32,
    /// Head angle, degrees clockwise from +Y (0 when angle tracking is off).
    pub angle: i32,
    /// Non-zero entries of the 50-element target array.
    pub target_count: u8,
}

/// Parse the video-tracker record at `at`.
pub fn video_record(b: &Block, at: u64) -> Option<VideoRecord> {
    let after_points = at + 14 + 4 * NVT_POINTS as u64 + 2;
    let mut target_count = 0u8;
    for k in 0..NVT_TARGETS as u64 {
        if b.i32_at(after_points + 12 + 4 * k)? != 0 {
            target_count += 1;
        }
    }
    Some(VideoRecord {
        start: b.u16_at(at)?,
        system_id: b.u16_at(at + 2)?,
        data_size: b.u16_at(at + 4)?,
        timestamp_us: b.u64_at(at + 6)?,
        x: b.i32_at(after_points)?,
        y: b.i32_at(after_points + 4)?,
        angle: b.i32_at(after_points + 8)?,
        target_count,
    })
}

/// One spike record (waveform in [point, electrode] order, raw).
#[derive(Debug, Clone, PartialEq)]
pub struct SpikeRecord {
    pub timestamp_us: u64,
    pub entity: u32,
    pub cell: u32,
    pub features: [i32; SPIKE_FEATURES],
    pub samples: Vec<i16>,
}

/// Parse the spike record at `at` for `electrodes` electrodes.
pub fn spike_record(b: &Block, at: u64, electrodes: u8) -> Option<SpikeRecord> {
    let mut features = [0i32; SPIKE_FEATURES];
    for (k, x) in features.iter_mut().enumerate() {
        *x = b.i32_at(at + 16 + 4 * k as u64)?;
    }
    let n = SPIKE_SAMPLES * u64::from(electrodes);
    let samples = (0..n)
        .map(|i| b.i16_at(at + SPIKE_PREFIX_LEN + 2 * i))
        .collect::<Option<Vec<_>>>()?;
    Some(SpikeRecord {
        timestamp_us: b.u64_at(at)?,
        entity: b.u32_at(at + 8)?,
        cell: b.u32_at(at + 12)?,
        features,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(ts: u64, valid: u32) -> RecordHead {
        RecordHead {
            timestamp_us: ts,
            channel_number: 0,
            rate_hz: 32000,
            valid,
        }
    }

    #[test]
    fn segments_split_on_gaps_and_partial_records() {
        // 32 kHz: 512 samples = 16000 µs
        let heads = vec![
            head(0, 512),
            head(16_000, 512),
            head(32_000, 31),
            head(5_000_000, 512),
            head(5_016_000, 512),
            head(5_032_001, 100), // 1 µs jitter is not a gap
        ];
        let (segs, dt) = segments_from(&heads, Some(32_000.0));
        assert!((dt.unwrap() - 31.25).abs() < 1e-9);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].sample_count, 512 + 512 + 31);
        assert_eq!(segs[1].first_record, 3);
        assert_eq!(segs[1].sample_count, 512 + 512 + 100);
    }

    #[test]
    fn kinds() {
        assert_eq!(FileKind::from_extension("NCS"), Some(FileKind::Continuous));
        assert_eq!(FileKind::Spikes { electrodes: 4 }.record_len(), 304);
        assert_eq!(FileKind::Spikes { electrodes: 1 }.record_len(), 112);
        assert_eq!(
            FileKind::from_record_len(176),
            Some(FileKind::Spikes { electrodes: 2 })
        );
    }
}
