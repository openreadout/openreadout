//! Axona DacqUSB recordings: a `.set` file of `key value` lines, the raw `.bin` (432-byte packets
//! of 3 samples × 64 int16 channels) and tetrode spike files `.1` … `.32`, all sharing a stem.
//! The raw channels of the active tetrodes are one trace; the tetrode files are the `spikes`
//! table. Notes and vocabulary: `docs/formats/axona-dacqusb.md`; provenance:
//! `docs/provenance/axona-dacqusb.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{Block, latin1, read_block, read_file_capped_in};
use openreadout_core::model::{
    CheckReport, ColumnInfo, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry,
    SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

/// Format id of Axona DacqUSB recordings.
pub const AXONA_FORMAT_ID: &str = "axona-dacqusb";
/// Bytes of one `.bin` packet.
pub const BIN_PACKET: u64 = 432;
/// Bytes before the samples in a packet.
pub const BIN_PACKET_HEADER: u64 = 32;
/// Samples per channel in a packet.
pub const BIN_SAMPLES_PER_PACKET: u64 = 3;
/// Channels in a packet.
pub const BIN_CHANNELS: usize = 64;
/// Tetrode files looked for (`.1` … `.32`).
pub const MAX_TETRODES: u32 = 32;
/// Bytes of a `.set` (or any header) read at most.
pub const MAX_HEADER_BYTES: u64 = 4 << 20;
/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_AXONA_READ: u64 = 1 << 24;

/// Position of channel `i` (0-based, tetrode t's electrodes are 4t−4 … 4t−1) among the 64
/// int16 values of one sample in a `.bin` packet: blocks of eight channels alternate between
/// the upper and the lower half.
pub fn bin_channel_slot(i: usize) -> usize {
    let (g, r) = (i / 8, i % 8);
    if g % 2 == 0 { 32 + g / 2 * 8 + r } else { g / 2 * 8 + r }
}

/// Parse `key value` header lines up to `data_start` (or the end). Returns the entries in file
/// order and the offset of the first data byte (after `data_start`), if present.
pub fn parse_header(bytes: &[u8]) -> (Vec<(String, String)>, Option<usize>) {
    let data = openreadout_core::bytes::find(bytes, b"data_start");
    let text = latin1(&bytes[..data.unwrap_or(bytes.len())]);
    let mut out = Vec::new();
    for line in text.split(['\r', '\n']) {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        out.push((k.to_string(), v.trim().to_string()));
    }
    (out, data.map(|d| d + b"data_start".len()))
}

fn number(v: &str) -> Option<f64> {
    v.split_whitespace()
        .next()?
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
}

/// True when `head` starts like an Axona header (`trial_date` first, `data_start` or `.set` keys).
pub fn looks_like_axona(head: &[u8]) -> bool {
    head.starts_with(b"trial_date ")
}

/// One tetrode file.
#[derive(Debug, Clone)]
pub struct TetrodeFile {
    /// Tetrode number (the file extension).
    pub tetrode: u32,
    /// The file.
    pub path: PathBuf,
    /// Header entries.
    pub header: Vec<(String, String)>,
    /// Offset of the first spike record.
    pub data_offset: u64,
    /// Spikes the header declares (`num_spikes`).
    pub spikes: u64,
    /// Spikes whose records lie wholly in the file.
    pub complete_spikes: u64,
    /// Time-stamp clock (`timebase`), Hz.
    pub timebase_hz: f64,
    /// Samples per channel per spike (`samples_per_spike`).
    pub samples_per_spike: u64,
    /// Channels per spike (`num_chans`).
    pub channels: u64,
}

impl TetrodeFile {
    /// Bytes of one spike record.
    pub fn record_bytes(&self) -> u64 {
        self.channels * (4 + self.samples_per_spike)
    }
}

/// The parsed session.
#[derive(Debug, Clone, Default)]
pub struct AxonaSession {
    /// The directory and stem.
    pub dir: PathBuf,
    /// Shared file stem.
    pub stem: String,
    /// `.set` entries in file order.
    pub set: Vec<(String, String)>,
    /// The `.bin` raw file, if present.
    pub bin: Option<PathBuf>,
    /// `.bin` length.
    pub bin_len: u64,
    /// Active tetrodes (`collectMask_<n> 1`).
    pub active_tetrodes: Vec<u32>,
    /// Tetrode spike files.
    pub tetrode_files: Vec<TetrodeFile>,
    /// Other session files (`.eeg`, `.egf`, `.pos`, …), listed, not read.
    pub other_files: Vec<PathBuf>,
    /// Problems found while parsing.
    pub findings: Vec<Finding>,
}

impl AxonaSession {
    /// A `.set` value.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.set
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    /// Raw sampling rate (`rawRate`).
    pub fn raw_rate_hz(&self) -> Option<f64> {
        self.get("rawRate").and_then(number).filter(|r| *r > 0.0)
    }
    /// Whole packets in the `.bin`.
    pub fn packets(&self) -> u64 {
        self.bin_len / BIN_PACKET
    }
    /// Channel indices (0-based) of the active tetrodes, in order.
    pub fn raw_channels(&self) -> Vec<usize> {
        self.active_tetrodes
            .iter()
            .flat_map(|t| (0..4).map(move |e| (*t as usize - 1) * 4 + e))
            .collect()
    }
    /// µV per count of channel `i`: 1000 × `ADC_fullscale_mv` / (`gain_ch_<i>` × 32768).
    pub fn raw_scale(&self, i: usize) -> f64 {
        let full = self
            .get("ADC_fullscale_mv")
            .and_then(number)
            .unwrap_or(f64::NAN);
        let gain = self
            .get(&format!("gain_ch_{i}"))
            .and_then(number)
            .unwrap_or(f64::NAN);
        1000.0 * full / (gain * 32768.0)
    }
    /// Recording start (`trial_date` + `trial_time`), local time without zone.
    pub fn recorded_at(&self) -> Option<String> {
        let date = self.get("trial_date")?;
        let time = self.get("trial_time")?;
        let date = date.split_once(',').map_or(date, |(_, d)| d).trim();
        let mut q = date.split_whitespace();
        let day: u32 = q.next()?.parse().ok()?;
        let mon = q.next()?.to_ascii_lowercase();
        let year: u32 = q.next()?.parse().ok()?;
        let month = [
            "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
        ]
        .iter()
        .position(|m| mon.starts_with(m))? as u32
            + 1;
        let hms: Vec<u32> = time.split(':').filter_map(|x| x.trim().parse().ok()).collect();
        if !(1..=31).contains(&day) || hms.len() != 3 || hms[0] > 23 || hms[1] > 59 || hms[2] > 60
        {
            return None;
        }
        Some(format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
            hms[0], hms[1], hms[2]
        ))
    }
}

fn ext_of(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn read_head(fs: &Fs, path: &Path) -> Result<Vec<u8>> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = f.size().map_err(|e| Error::io(path, e))?;
    Ok(read_block(&mut f, path, 0, MAX_HEADER_BYTES.min(len), len)?.bytes)
}

/// Find and parse the session an input names (any of its files).
pub fn parse_session(input: &Input) -> Result<AxonaSession> {
    let fs = input.fs();
    let path = input.path();
    let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let set_path = dir.join(format!("{stem}.set"));
    if !fs.is_file(&set_path) {
        return Err(Error::unsupported(
            AXONA_FORMAT_ID,
            "Axona files without their .set file",
            format!(
                "{} has no {stem}.set next to it; the .set file holds the sampling rate, gains and active tetrodes (copy the whole session)",
                path.display()
            ),
        ));
    }
    let set_bytes = read_file_capped_in(fs, &set_path, MAX_HEADER_BYTES, AXONA_FORMAT_ID, ".set file")?;
    let (set, _) = parse_header(&set_bytes);
    let mut s = AxonaSession {
        dir: dir.clone(),
        stem: stem.clone(),
        set,
        ..AxonaSession::default()
    };
    let mut active: Vec<u32> = s
        .set
        .iter()
        .filter_map(|(k, v)| {
            let t: u32 = k.strip_prefix("collectMask_")?.parse().ok()?;
            ((1..=16).contains(&t) && number(v) == Some(1.0)).then_some(t)
        })
        .collect();
    active.sort_unstable();
    active.dedup();
    s.active_tetrodes = active;
    let bin = dir.join(format!("{stem}.bin"));
    if fs.is_file(&bin) {
        s.bin_len = fs.metadata(&bin).map_err(|e| Error::io(&bin, e))?.len();
        if s.bin_len % BIN_PACKET != 0 {
            s.findings.push(Finding::warning(
                "partial_packet",
                format!("{} bytes after the last whole .bin packet", s.bin_len % BIN_PACKET),
            ));
        }
        s.bin = Some(bin);
    }
    for t in 1..=MAX_TETRODES {
        let p = dir.join(format!("{stem}.{t}"));
        if !fs.is_file(&p) {
            break;
        }
        let len = fs.metadata(&p).map_err(|e| Error::io(&p, e))?.len();
        let head = read_head(fs, &p)?;
        let (header, data) = parse_header(&head);
        let get = |k: &str| header.iter().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
        let Some(data) = data else {
            s.findings.push(Finding::error(
                "no_data_start",
                format!("{}: no data_start in the first {} bytes", p.display(), head.len()),
            ));
            continue;
        };
        let spikes = get("num_spikes").and_then(number).unwrap_or(0.0).max(0.0) as u64;
        let tf = TetrodeFile {
            tetrode: t,
            path: p.clone(),
            data_offset: data as u64,
            spikes,
            complete_spikes: 0,
            timebase_hz: get("timebase").and_then(number).unwrap_or(f64::NAN),
            samples_per_spike: get("samples_per_spike").and_then(number).unwrap_or(0.0) as u64,
            channels: get("num_chans").and_then(number).unwrap_or(0.0) as u64,
            header: header.clone(),
        };
        let usable = tf.channels == 4
            && (1..=1024).contains(&tf.samples_per_spike)
            && get("bytes_per_sample").and_then(number) == Some(1.0)
            && get("bytes_per_timestamp").and_then(number) == Some(4.0)
            && tf.timebase_hz > 0.0;
        if !usable {
            s.findings.push(Finding::error(
                "tetrode_layout",
                format!(
                    "{}: only 4 channels of 1-byte samples with 4-byte time stamps are read (num_chans {}, samples_per_spike {})",
                    p.display(),
                    tf.channels,
                    tf.samples_per_spike
                ),
            ));
            continue;
        }
        let complete = len.saturating_sub(tf.data_offset) / tf.record_bytes();
        let mut tf = tf;
        tf.complete_spikes = complete.min(spikes);
        if tf.complete_spikes < spikes {
            s.findings.push(Finding::error(
                "truncated",
                format!(
                    "{}: {spikes} spikes declared, {} in the file",
                    p.display(),
                    tf.complete_spikes
                ),
            ));
        }
        s.tetrode_files.push(tf);
    }
    if let Ok(rd) = fs.read_dir(&dir) {
        let mut others: Vec<PathBuf> = rd
            .filter_map(std::result::Result::ok)
            .take(10_000)
            .map(|e| e.path())
            .filter(|p| {
                p.file_stem().is_some_and(|x| x.to_string_lossy() == stem)
                    && !matches!(ext_of(p).as_str(), "set" | "bin")
                    && !s.tetrode_files.iter().any(|t| t.path == *p)
            })
            .collect();
        others.sort();
        s.other_files = others;
    }
    Ok(s)
}

/// The Axona reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct AxonaReader;

impl FormatReader for AxonaReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::AXONA)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: AXONA_FORMAT_ID.into(),
            name: "Axona DacqUSB recording (.set + .bin, tetrode files)".into(),
            vendor: "Axona (DacqUSB)".into(),
            extensions: vec!["set".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::AXONA.confidence,
            known_gaps: vec![
                "Input is the .set file (or another file of the session); the raw .bin and the tetrode files .1 … .32 are read".into(),
                "EEG (.eeg, .egf), position (.pos) and other session files are listed, not read".into(),
                "Tetrode waveforms are returned as stored (int8 counts); raw .bin samples are scaled to µV with the .set gains".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if has_extension(path, &["set"]) && looks_like_axona(head) {
            return Some(Detection {
                format_id: AXONA_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("reads the session's .bin and tetrode files".into()),
            });
        }
        let ext = ext_of(path);
        let tetrode = ext.parse::<u32>().is_ok_and(|t| (1..=MAX_TETRODES).contains(&t));
        if (tetrode || matches!(ext.as_str(), "eeg" | "egf" | "pos")) && looks_like_axona(head) {
            return Some(Detection {
                format_id: AXONA_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("reads the session this file belongs to (needs its .set)".into()),
            });
        }
        if ext == "bin" && head.starts_with(b"ADU") && head.len() >= 436 && head[432..435] == *b"ADU"
        {
            return Some(Detection {
                format_id: AXONA_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("Axona raw packets; reads the session (needs its .set)".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(AxonaDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(AxonaDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened session.
#[derive(Debug)]
pub struct AxonaDataset {
    path: PathBuf,
    fs: Fs,
    /// The parsed session.
    pub session: AxonaSession,
    bin: Option<SourceFile>,
    window: Block,
}

impl AxonaDataset {
    /// Open a local path.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let session = parse_session(input)?;
        Ok(AxonaDataset {
            path: input.path().to_path_buf(),
            fs: input.fs().clone(),
            session,
            bin: None,
            window: Block::default(),
        })
    }

    fn has_trace(&self) -> bool {
        let s = &self.session;
        s.bin.is_some() && s.packets() > 0 && !s.active_tetrodes.is_empty() && s.raw_rate_hz().is_some()
    }

    fn spike_rows(&self) -> u64 {
        self.session
            .tetrode_files
            .iter()
            .map(|t| t.complete_spikes)
            .sum()
    }

    fn width(&self) -> u64 {
        self.session
            .tetrode_files
            .iter()
            .map(|t| t.samples_per_spike)
            .max()
            .unwrap_or(0)
    }

    fn spike_columns(&self) -> Vec<ColumnInfo> {
        let col = |i: usize, name: String, dtype: &str, unit: Option<&str>| ColumnInfo {
            index: i as u32,
            name,
            label: None,
            dtype: dtype.into(),
            unit: unit.map(str::to_string),
            range: None,
            extra: BTreeMap::new(),
        };
        let mut v = vec![
            col(0, "time_s".into(), "float64", Some("s")),
            col(1, "timestamp".into(), "int32", None),
            col(2, "tetrode".into(), "uint8", None),
        ];
        for c in 1..=4 {
            for i in 0..self.width() {
                v.push(col(v.len(), format!("ch{c}_w{i}"), "int8", None));
            }
        }
        v
    }

    fn bin_bytes(&mut self, at: u64, n: u64) -> Result<&[u8]> {
        let path = self
            .session
            .bin
            .clone()
            .ok_or_else(|| Error::Other("no .bin".into()))?;
        let len = self.session.bin_len;
        if at < self.window.origin || at + n > self.window.end() {
            if self.bin.is_none() {
                self.bin = Some(self.fs.open(&path).map_err(|e| Error::io(&path, e))?);
            }
            let f = self.bin.as_mut().expect("just opened");
            self.window = read_block(f, &path, at, (4u64 << 20).max(n), len)?;
        }
        self.window
            .slice(at, n as usize)
            .ok_or_else(|| Error::corrupt_at(AXONA_FORMAT_ID, at, "packet cut off by the end of the .bin"))
    }
}

impl Dataset for AxonaDataset {
    fn info(&self) -> Result<FileInfo> {
        let s = &self.session;
        let mut notes = vec![format!(
            "Axona DacqUSB session {}: {} active tetrode(s), {} tetrode file(s)",
            s.stem,
            s.active_tetrodes.len(),
            s.tetrode_files.len()
        )];
        if !s.other_files.is_empty() {
            notes.push(format!(
                "not read: {}",
                s.other_files
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if s.findings
            .iter()
            .any(|f| f.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found (e.g. a cut-short tetrode file); run `check`".into());
        }
        let recorded = s.recorded_at();
        let mut traces = Vec::new();
        if self.has_trace() {
            let mut extra = BTreeMap::new();
            if let Some(t) = &recorded {
                extra.insert("recorded_at".into(), json!(t));
            }
            if let Some(v) = s.get("sw_version") {
                extra.insert("application".into(), json!("DacqUSB"));
                extra.insert("application_version".into(), json!(v));
            }
            if let Some(v) = s.get("duration").and_then(number) {
                extra.insert("trial_duration_s".into(), json!(v));
            }
            extra.insert("active_tetrodes".into(), json!(s.active_tetrodes));
            traces.push(TraceInfo {
                index: 0,
                name: Some("raw".into()),
                sample_rate_hz: s.raw_rate_hz().unwrap_or(f64::NAN),
                sample_count: s.packets() * BIN_SAMPLES_PER_PACKET,
                sweep_count: 1,
                channels: s
                    .raw_channels()
                    .into_iter()
                    .enumerate()
                    .map(|(k, i)| {
                        let mut e = BTreeMap::new();
                        e.insert("channel_number".into(), json!(i));
                        e.insert("tetrode".into(), json!(i / 4 + 1));
                        if let Some(g) = s.get(&format!("gain_ch_{i}")).and_then(number) {
                            e.insert("gain".into(), json!(g));
                        }
                        SignalChannelInfo {
                            index: k as u32,
                            name: format!("{}{}", i / 4 + 1, ['a', 'b', 'c', 'd'][i % 4]),
                            unit: Some("µV".into()),
                            dtype: "int16".into(),
                            scale: s.raw_scale(i),
                            offset: 0.0,
                            extra: e,
                        }
                    })
                    .collect(),
                start_s: Some(0.0),
                extra,
            });
        }
        let mut tables = Vec::new();
        if !s.tetrode_files.is_empty() {
            let mut extra = BTreeMap::new();
            extra.insert(
                "tetrodes".into(),
                json!(
                    s.tetrode_files
                        .iter()
                        .map(|t| json!({"tetrode": t.tetrode, "spikes": t.complete_spikes, "timebase_hz": t.timebase_hz}))
                        .collect::<Vec<_>>()
                ),
            );
            if let Some(t) = &recorded {
                extra.insert("recorded_at".into(), json!(t));
            }
            tables.push(TableInfo {
                index: 0,
                name: Some("spikes".into()),
                row_count: self.spike_rows(),
                columns: self.spike_columns(),
                extra,
            });
        }
        let size = [Some(self.dir_file("set")), s.bin.clone()]
            .into_iter()
            .flatten()
            .chain(s.tetrode_files.iter().map(|t| t.path.clone()))
            .filter_map(|p| self.fs.metadata(&p).ok().map(|m| m.len()))
            .sum();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: size,
            format: AxonaReader.descriptor(),
            format_version: s.get("sw_version").map(str::to_string),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let s = &self.session;
        Ok(json!({
            "set": s.set.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>(),
            "tetrode_files": s.tetrode_files.iter().map(|t| json!({
                "tetrode": t.tetrode,
                "header": t.header.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>(),
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("traces[].sample_rate_hz", Source::PriorArt),
            ("traces[].sample_count", Source::PriorArt),
            ("traces[].channels[].name", Source::PriorArt),
            ("traces[].channels[].scale", Source::PriorArt),
            ("traces[].extra.recorded_at", Source::Inferred),
            ("tables[].columns", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let s = &self.session;
        let mut out = Vec::new();
        let mut add = |p: &Path, kind: &str, details: Value| {
            out.push(LsEntry {
                kind: kind.into(),
                name: p.display().to_string(),
                offset: None,
                size: self.fs.metadata(p).ok().map(|m| m.len()),
                image: None,
                details,
            });
        };
        add(&self.dir_file("set"), "setup", json!({"keys": s.set.len()}));
        if let Some(b) = &s.bin {
            add(b, "raw", json!({"packets": s.packets()}));
        }
        for t in &s.tetrode_files {
            add(&t.path, "tetrode", json!({"tetrode": t.tetrode, "spikes": t.spikes}));
        }
        for p in &s.other_files {
            add(p, "not read", json!({}));
        }
        Ok(out)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let s = &self.session;
        let mut v = vec![self.dir_file("set")];
        v.extend(s.bin.clone());
        v.extend(s.tetrode_files.iter().map(|t| t.path.clone()));
        v
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            AXONA_FORMAT_ID,
            "image planes",
            "Axona sessions hold sampled signals and spikes: use `openreadout trace` or `openreadout table`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if index != 0 || !self.has_trace() {
            return Err(Error::Usage(format!("trace index {index} out of range")));
        }
        if sweep != 0 {
            return Err(Error::Usage(format!("sweep {sweep} out of range (1 sweep)")));
        }
        let n = self.session.packets() * BIN_SAMPLES_PER_PACKET;
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let count = max.min(n - first).min(MAX_AXONA_READ);
        let chans = self.session.raw_channels();
        let scales: Vec<f64> = chans.iter().map(|i| self.session.raw_scale(*i)).collect();
        let mut out: Vec<Vec<f64>> = vec![Vec::with_capacity(count as usize); chans.len()];
        let mut k = first;
        while k < first + count {
            let packet = k / BIN_SAMPLES_PER_PACKET;
            let at = packet * BIN_PACKET;
            let bytes = self.bin_bytes(at, BIN_PACKET)?.to_vec();
            let mut sub = k % BIN_SAMPLES_PER_PACKET;
            while sub < BIN_SAMPLES_PER_PACKET && k < first + count {
                let base = (BIN_PACKET_HEADER + sub * BIN_CHANNELS as u64 * 2) as usize;
                for (c, i) in chans.iter().enumerate() {
                    let o = base + bin_channel_slot(*i) * 2;
                    let v = i16::from_le_bytes([bytes[o], bytes[o + 1]]);
                    out[c].push(f64::from(v) * scales[c]);
                }
                sub += 1;
                k += 1;
            }
        }
        Ok(Trace {
            trace: 0,
            sweep: 0,
            first_sample: first,
            channels: out,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 || self.session.tetrode_files.is_empty() {
            return Err(Error::Usage(format!("table index {index} out of range")));
        }
        let total = self.spike_rows();
        if first_row > total {
            return Err(Error::Usage(format!(
                "first row {first_row} is past the end ({total} rows)"
            )));
        }
        let n = max_rows.min(total - first_row);
        let width = self.width() as usize;
        let mut cols: Vec<Vec<f64>> = (0..3 + 4 * width)
            .map(|_| Vec::with_capacity(n as usize))
            .collect();
        let mut skip = first_row;
        let mut left = n;
        let files = self.session.tetrode_files.clone();
        for t in &files {
            if left == 0 {
                break;
            }
            if skip >= t.complete_spikes {
                skip -= t.complete_spikes;
                continue;
            }
            let take = left.min(t.complete_spikes - skip);
            let rb = t.record_bytes();
            let mut f = self.fs.open(&t.path).map_err(|e| Error::io(&t.path, e))?;
            let len = f.size().map_err(|e| Error::io(&t.path, e))?;
            let at = t.data_offset + skip * rb;
            let block = read_block(&mut f, &t.path, at, take * rb, len)?;
            if (block.len() as u64) < take * rb {
                return Err(Error::corrupt_at(AXONA_FORMAT_ID, at, "spike records cut off"));
            }
            let w = t.samples_per_spike as usize;
            for r in block.bytes.chunks_exact(rb as usize) {
                let ts = i32::from_be_bytes([r[0], r[1], r[2], r[3]]);
                cols[0].push(f64::from(ts) / t.timebase_hz);
                cols[1].push(f64::from(ts));
                cols[2].push(f64::from(t.tetrode));
                for c in 0..4 {
                    let g = &r[c * (4 + w) + 4..(c + 1) * (4 + w)];
                    for i in 0..width {
                        cols[3 + c * width + i].push(g.get(i).map_or(f64::NAN, |b| f64::from(*b as i8)));
                    }
                }
            }
            left -= take;
            skip = 0;
        }
        Ok(Table {
            table: 0,
            first_row,
            columns: cols,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), AXONA_FORMAT_ID);
        r.performed(".set read; active tetrodes, rawRate, ADC_fullscale_mv and gains present");
        r.performed("every .bin packet's ADU magic and packet counter; every tetrode file's header and length");
        for f in &self.session.findings {
            r.push(f.clone());
        }
        let s = self.session.clone();
        if s.bin.is_some() {
            if s.raw_rate_hz().is_none() {
                r.push(Finding::error("no_raw_rate", "the .set has no usable rawRate"));
            }
            for i in s.raw_channels() {
                if !s.raw_scale(i).is_finite() {
                    r.push(Finding::error(
                        "bad_gain",
                        format!("channel {i}: gain_ch_{i} or ADC_fullscale_mv missing or zero"),
                    ));
                }
            }
            let mut bad = 0u64;
            let mut jumps = 0u64;
            let mut prev: Option<u32> = None;
            for p in 0..s.packets() {
                let b = self.bin_bytes(p * BIN_PACKET, 8)?;
                if &b[..3] != b"ADU" {
                    bad += 1;
                }
                let c = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                if prev.is_some_and(|x| x.wrapping_add(1) != c) {
                    jumps += 1;
                }
                prev = Some(c);
            }
            if bad > 0 {
                r.push(Finding::error(
                    "bad_packets",
                    format!("{bad} .bin packet(s) do not start with ADU"),
                ));
            }
            if jumps > 0 {
                r.push(Finding::warning(
                    "packet_gaps",
                    format!("the .bin packet counter skips {jumps} time(s): samples may be missing"),
                ));
            }
        }
        Ok(r)
    }
}

impl AxonaDataset {
    fn dir_file(&self, ext: &str) -> PathBuf {
        self.session
            .dir
            .join(format!("{}.{ext}", self.session.stem))
    }
}
