//! The legacy "Open Ephys format": one `.continuous` file per channel (1024-byte text header,
//! then 2070-byte records of 1024 big-endian int16 samples), `.events` and `.spikes` files;
//! later experiments add `_2`, `_3` … to the file names.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_block;
use openreadout_core::model::Finding;
use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};

use super::OPEN_EPHYS_FORMAT_ID;

/// Bytes of every file's text header.
pub const LEGACY_HEADER_LEN: u64 = 1024;
/// Samples per continuous record.
pub const RECORD_SAMPLES: u64 = 1024;
/// Bytes per continuous record.
pub const RECORD_LEN: u64 = 8 + 2 + 2 + 2 * RECORD_SAMPLES + 10;
/// Bytes per event record.
pub const EVENT_RECORD_LEN: u64 = 16;
/// The 10-byte marker that ends every continuous record.
pub const RECORD_MARKER: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 255];
/// Records checked for markers and timestamps per file at most (the whole file below this).
pub const MAX_RECORDS: u64 = 50_000_000;
/// Continuous files accepted at most.
pub const MAX_LEGACY_FILES: usize = 4096;

/// The `header.<key> = <value>;` pairs of a legacy header.
pub fn parse_header(b: &[u8]) -> BTreeMap<String, String> {
    let text = String::from_utf8_lossy(b);
    let mut out = BTreeMap::new();
    for stmt in text.split(';') {
        let s = stmt.trim();
        let Some(rest) = s.strip_prefix("header.") else {
            continue;
        };
        if let Some((k, v)) = rest.split_once('=') {
            out.insert(
                k.trim().to_string(),
                v.trim().trim_matches('\'').trim().to_string(),
            );
        }
    }
    out
}

/// True when `head` is a legacy Open Ephys header.
pub fn looks_like_legacy(head: &[u8]) -> bool {
    head.starts_with(b"header.format = 'Open Ephys Data Format'")
}

/// A gap-free run of records of one channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// First record index.
    pub first_record: u64,
    /// Records.
    pub records: u64,
    /// Timestamp (sample number) of the first sample.
    pub timestamp: i64,
    /// Recording number of the run's records.
    pub recording: u16,
}

/// One `.continuous` file.
#[derive(Debug, Clone)]
pub struct LegacyChannel {
    /// The file.
    pub path: PathBuf,
    /// `header.channel` (the file name's channel part when absent).
    pub name: String,
    /// The file-name prefix before the channel (`100`, `127_RhythmData-A`).
    pub source: String,
    /// Experiment segment from the file-name suffix (1 when none).
    pub segment: u32,
    /// `header.sampleRate`.
    pub sample_rate: f64,
    /// `header.bitVolts`.
    pub bit_volts: f64,
    /// `header.version`.
    pub version: String,
    /// `header.date_created`.
    pub date_created: String,
    /// Gap-free runs.
    pub runs: Vec<Run>,
    /// Why the channel's samples are not read, if they are not.
    pub refused: Option<String>,
}

/// A trace: channels of one node directory, source and segment on the same run grid.
#[derive(Debug, Clone)]
pub struct LegacyTrace {
    /// Node directory (relative to the input, "" for the input itself).
    pub node: String,
    /// Channel indices (into the dataset's channels).
    pub channels: Vec<usize>,
}

fn split_name(stem: &str) -> Option<(String, String, u32)> {
    // <source>_<CHn>[_<k>]: the channel part starts with letters and ends with digits
    let parts: Vec<&str> = stem.split('_').collect();
    if parts.len() < 2 {
        return None;
    }
    let (seg, chan_at) = match parts.last().and_then(|p| p.parse::<u32>().ok()) {
        Some(k) if parts.len() >= 3 => (k, parts.len() - 2),
        _ => (1, parts.len() - 1),
    };
    let chan = parts[chan_at].to_string();
    let source = parts[..chan_at].join("_");
    Some((source, chan, seg))
}

/// Order key of a channel name: CH, then AUX, then ADC, then others; by number.
pub fn channel_order(name: &str) -> (u8, u64, String) {
    let digits_at = name
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(name.len());
    let (prefix, num) = name.split_at(digits_at);
    let rank = match prefix {
        "CH" => 0,
        "AUX" => 1,
        "ADC" => 2,
        _ => 3,
    };
    (rank, num.parse().unwrap_or(u64::MAX), name.to_string())
}

/// Read one `.continuous` file's header and walk its record headers.
pub fn channel(fs: &Fs, path: &Path, findings: &mut Vec<Finding>) -> Result<LegacyChannel> {
    let mut file = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = file.size().map_err(|e| Error::io(path, e))?;
    let head = read_block(&mut file, path, 0, LEGACY_HEADER_LEN, len)?;
    if head.len() < LEGACY_HEADER_LEN as usize || !looks_like_legacy(&head.bytes) {
        return Err(Error::corrupt(
            OPEN_EPHYS_FORMAT_ID,
            format!("{}: no Open Ephys header", path.display()),
        ));
    }
    let hdr = parse_header(&head.bytes);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let (source, chan, segment) = split_name(&stem).unwrap_or((String::new(), stem.clone(), 1));
    let num = |k: &str| {
        hdr.get(k)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(f64::NAN)
    };
    let mut chan = LegacyChannel {
        path: path.to_path_buf(),
        name: hdr
            .get("channel")
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or(chan),
        source,
        segment,
        sample_rate: num("sampleRate"),
        bit_volts: num("bitVolts"),
        version: hdr.get("version").cloned().unwrap_or_default(),
        date_created: hdr.get("date_created").cloned().unwrap_or_default(),
        runs: Vec::new(),
        refused: None,
    };
    let body = len - LEGACY_HEADER_LEN;
    let records = body / RECORD_LEN;
    if !body.is_multiple_of(RECORD_LEN) {
        chan.refused = Some(format!(
            "{body} bytes after the header are not a whole number of {RECORD_LEN}-byte records"
        ));
    }
    if records > MAX_RECORDS {
        return Err(Error::unsupported(
            OPEN_EPHYS_FORMAT_ID,
            format!("continuous files with more than {MAX_RECORDS} records"),
            "the file is larger than this reader walks.",
        ));
    }
    let usable_rate = chan.sample_rate.is_finite() && chan.sample_rate > 0.0;
    if !usable_rate || !chan.bit_volts.is_finite() {
        chan.refused = Some(format!(
            "unusable sample rate {} or bitVolts {}",
            chan.sample_rate, chan.bit_volts
        ));
    }
    let per_chunk = 4096u64;
    let mut prev: Option<(i64, u16)> = None;
    let mut done = 0u64;
    while done < records && chan.refused.is_none() {
        let batch = per_chunk.min(records - done);
        let at = LEGACY_HEADER_LEN + done * RECORD_LEN;
        let chunk = read_block(&mut file, path, at, batch * RECORD_LEN, len)?;
        for j in 0..batch {
            let pos = at + j * RECORD_LEN;
            let ts = chunk.i64_at(pos).unwrap_or(0);
            let count = chunk.u16_at(pos + 8).unwrap_or(0);
            let rec = chunk.u16_at(pos + 10).unwrap_or(0);
            let marker = chunk.slice(pos + RECORD_LEN - 10, 10);
            if marker != Some(&RECORD_MARKER[..]) {
                chan.refused = Some(format!("record {} has no record marker", done + j));
                break;
            }
            if u64::from(count) != RECORD_SAMPLES {
                chan.refused = Some(format!(
                    "record {} holds {count} samples (only full 1024-sample records are read)",
                    done + j
                ));
                break;
            }
            let contiguous = prev.is_some_and(|(prev_ts, prev_rec)| {
                prev_rec == rec && ts.checked_sub(prev_ts) == Some(RECORD_SAMPLES as i64)
            });
            if contiguous {
                if let Some(last) = chan.runs.last_mut() {
                    last.records += 1;
                }
            } else {
                if prev.is_some_and(|(prev_ts, _)| ts <= prev_ts) {
                    chan.refused = Some(format!(
                        "record {} starts at sample {ts}, not after the previous record",
                        done + j
                    ));
                    break;
                }
                chan.runs.push(Run {
                    first_record: done + j,
                    records: 1,
                    timestamp: ts,
                    recording: rec,
                });
            }
            prev = Some((ts, rec));
        }
        done += batch;
    }
    // runs shorter than a handful of records between irregular steps: timestamps that step by
    // something other than 1024 samples throughout are not a gap-free recording
    if chan.refused.is_none() && chan.runs.len() > 2 && chan.runs.len() as u64 * 2 > records {
        chan.refused = Some(format!(
            "timestamps step irregularly ({} runs in {records} records)",
            chan.runs.len()
        ));
    }
    if let Some(why) = &chan.refused {
        findings.push(Finding::warning(
            "unreadable_channel",
            format!("{}: {why}; its samples are not read", path.display()),
        ));
    }
    Ok(chan)
}

/// Continuous files (with their node directory), event files and spike files of a directory.
pub type LegacyFiles = (Vec<(String, PathBuf)>, Vec<PathBuf>, Vec<PathBuf>);

/// Every `.continuous` file in `dir` and its `Record Node` subdirectories.
pub fn discover(fs: &Fs, dir: &Path) -> Result<LegacyFiles> {
    let mut cont = Vec::new();
    let mut events = Vec::new();
    let mut spikes = Vec::new();
    let mut dirs = vec![(String::new(), dir.to_path_buf())];
    if let Ok(rd) = fs.read_dir(dir) {
        let mut subs: Vec<(String, PathBuf)> = rd
            .filter_map(std::result::Result::ok)
            .filter(|e| e.metadata().is_ok_and(|m| m.is_dir()))
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.path()))
            .filter(|(n, _)| n.starts_with("Record"))
            .collect();
        subs.sort();
        dirs.extend(subs);
    }
    for (node, d) in dirs {
        let Ok(rd) = fs.read_dir(&d) else {
            continue;
        };
        let mut files: Vec<PathBuf> = rd
            .filter_map(std::result::Result::ok)
            .filter(|e| e.metadata().is_ok_and(|m| m.is_file()))
            .map(|e| e.path())
            .collect();
        files.sort();
        for p in files {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.ends_with(".continuous") {
                cont.push((node.clone(), p));
            } else if name.ends_with(".events") && !name.starts_with("messages") {
                events.push(p);
            } else if name.ends_with(".spikes") {
                spikes.push(p);
            }
        }
    }
    if cont.len() > MAX_LEGACY_FILES {
        return Err(Error::unsupported(
            OPEN_EPHYS_FORMAT_ID,
            format!("more than {MAX_LEGACY_FILES} continuous files"),
            "open one Record Node directory at a time.",
        ));
    }
    Ok((cont, events, spikes))
}

/// Group readable channels into traces: same node, source, segment, sample rate and run grid.
pub fn layout(nodes: &[String], channels: &[LegacyChannel]) -> Vec<LegacyTrace> {
    let mut order: Vec<usize> = (0..channels.len())
        .filter(|&i| channels[i].refused.is_none() && !channels[i].runs.is_empty())
        .collect();
    order.sort_by(|&a, &b| {
        let (ca, cb) = (&channels[a], &channels[b]);
        (&nodes[a], &ca.source, ca.segment, channel_order(&ca.name)).cmp(&(
            &nodes[b],
            &cb.source,
            cb.segment,
            channel_order(&cb.name),
        ))
    });
    let mut out: Vec<LegacyTrace> = Vec::new();
    for i in order {
        let c = &channels[i];
        if let Some(t) = out.iter_mut().find(|t| {
            let f = &channels[t.channels[0]];
            nodes[t.channels[0]] == nodes[i]
                && f.source == c.source
                && f.segment == c.segment
                && f.sample_rate.to_bits() == c.sample_rate.to_bits()
                && f.runs == c.runs
        }) {
            t.channels.push(i);
        } else {
            out.push(LegacyTrace {
                node: nodes[i].clone(),
                channels: vec![i],
            });
        }
    }
    out
}

/// One `.spikes` file.
#[derive(Debug, Clone)]
pub struct LegacySpikes {
    /// The file.
    pub path: PathBuf,
    /// `header.electrode`.
    pub electrode: String,
    /// `header.sampleRate`.
    pub sample_rate: f64,
    /// Channels per spike.
    pub channels: u16,
    /// Samples per channel per spike.
    pub samples: u16,
    /// Bytes per record.
    pub record_len: u64,
    /// Records.
    pub records: u64,
}

/// Read a `.spikes` header and its record geometry (from the first record).
pub fn spikes(fs: &Fs, path: &Path, findings: &mut Vec<Finding>) -> Result<LegacySpikes> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = f.size().map_err(|e| Error::io(path, e))?;
    let h = read_block(&mut f, path, 0, LEGACY_HEADER_LEN + 23, len)?;
    if !looks_like_legacy(&h.bytes) {
        return Err(Error::corrupt(
            OPEN_EPHYS_FORMAT_ID,
            format!("{}: no Open Ephys header", path.display()),
        ));
    }
    let hdr = parse_header(h.slice(0, LEGACY_HEADER_LEN as usize).unwrap_or(&h.bytes));
    let sample_rate = hdr
        .get("sampleRate")
        .and_then(|v| v.parse().ok())
        .unwrap_or(f64::NAN);
    let (nch, ns) = if len >= LEGACY_HEADER_LEN + 23 {
        (
            h.u16_at(LEGACY_HEADER_LEN + 19).unwrap_or(0),
            h.u16_at(LEGACY_HEADER_LEN + 21).unwrap_or(0),
        )
    } else {
        (0, 0)
    };
    let record_len = 42 + 2 * u64::from(nch) * u64::from(ns) + 6 * u64::from(nch) + 2;
    let body = len.saturating_sub(LEGACY_HEADER_LEN);
    let records = if nch == 0 { 0 } else { body / record_len };
    if nch > 0 && body % record_len != 0 {
        findings.push(Finding::warning(
            "partial_record",
            format!(
                "{}: {body} bytes are not a whole number of {record_len}-byte spike records",
                path.display()
            ),
        ));
    }
    Ok(LegacySpikes {
        path: path.to_path_buf(),
        electrode: hdr.get("electrode").cloned().unwrap_or_default(),
        sample_rate,
        channels: nch,
        samples: ns,
        record_len,
        records,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_order() {
        assert_eq!(
            split_name("100_CH3_2"),
            Some(("100".into(), "CH3".into(), 2))
        );
        assert_eq!(
            split_name("127_RhythmData-A_CH12"),
            Some(("127_RhythmData-A".into(), "CH12".into(), 1))
        );
        assert!(channel_order("CH2") < channel_order("CH10"));
        assert!(channel_order("CH64") < channel_order("AUX1"));
        let h = parse_header(
            b"header.format = 'Open Ephys Data Format';\nheader.sampleRate = 30000;\n",
        );
        assert_eq!(h.get("sampleRate").map(String::as_str), Some("30000"));
    }
}
