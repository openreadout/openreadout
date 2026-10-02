//! WinWCP `.wcp` files (Strathclyde Electrophysiology Software): a 1024-byte `KEY=value` text
//! header, then `NR` records of an analysis header and interleaved int16 samples. One trace whose
//! channels are the recorded signals and whose sweeps are the records. Notes and vocabulary:
//! `docs/formats/winwcp.md`; provenance: `docs/provenance/winwcp.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{latin1, read_block};
use openreadout_core::model::{
    CheckReport, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry, SignalChannelInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

/// Format id of WinWCP files.
pub const WINWCP_FORMAT_ID: &str = "winwcp";
/// Bytes of the text header.
pub const WCP_HEADER_LEN: u64 = 1024;
/// Bytes of a sector (analysis and data blocks count in sectors).
pub const WCP_SECTOR: u64 = 512;
/// Channels accepted at most.
pub const MAX_WCP_CHANNELS: usize = 128;
/// Records accepted at most.
pub const MAX_WCP_RECORDS: u64 = 10_000_000;
/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_WCP_READ: u64 = 1 << 26;

/// One channel of the header.
#[derive(Debug, Clone, PartialEq)]
pub struct WcpChannel {
    /// Name (`YN`).
    pub name: String,
    /// Unit (`YU`).
    pub unit: String,
    /// Gain in V per unit (`YG`): value = raw × `VMax` / `ADCMAX` / `YG`.
    pub gain_v_per_unit: f64,
    /// Position in each interleaved sample frame (`YO`).
    pub offset_in_frame: usize,
    /// Zero level in ADC counts (`YZ`), reported, not applied.
    pub zero_level: f64,
}

/// One record's analysis header.
#[derive(Debug, Clone, PartialEq)]
pub struct WcpRecord {
    /// `ACCEPTED` / `REJECTED`.
    pub status: String,
    /// Record type (`TEST`, …).
    pub record_type: String,
    /// Group number.
    pub group: f32,
    /// Start in seconds since the first record.
    pub time_s: f32,
    /// Sampling interval, seconds.
    pub interval_s: f32,
    /// Full-scale voltage per channel.
    pub vmax: Vec<f32>,
}

/// The parsed file.
#[derive(Debug, Clone)]
pub struct WcpFile {
    /// Every `KEY=value` of the text header, as written.
    pub header: BTreeMap<String, String>,
    /// File version (`VER`).
    pub version: i64,
    /// ADC full scale (`ADCMAX`).
    pub adc_max: f64,
    /// Channels in header order.
    pub channels: Vec<WcpChannel>,
    /// Records in file order.
    pub records: Vec<WcpRecord>,
    /// Analysis-header sectors per record (`NBA`).
    pub analysis_sectors: u64,
    /// Data sectors per record (`NBD`).
    pub data_sectors: u64,
    /// Samples per channel per record.
    pub samples: u64,
    /// File length.
    pub file_len: u64,
    /// Problems found while parsing.
    pub findings: Vec<Finding>,
}

fn number(v: &str) -> Option<f64> {
    v.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
}

/// True when `head` starts like a WinWCP header (`VER=` and the record keys).
pub fn looks_like_wcp(head: &[u8]) -> bool {
    let t = latin1(head.get(..1024).unwrap_or(head));
    t.starts_with("VER=") && t.contains("\r\nNC=") && t.contains("\r\nNBD=")
}

impl WcpFile {
    /// Bytes of one record.
    pub fn record_bytes(&self) -> u64 {
        (self.analysis_sectors + self.data_sectors) * WCP_SECTOR
    }
    /// Offset of record `k`'s analysis header.
    pub fn record_offset(&self, k: u64) -> u64 {
        WCP_HEADER_LEN + k * self.record_bytes()
    }
    /// Scale of channel `c` in record `k` (value per ADC count).
    pub fn gain(&self, k: usize, c: usize) -> f64 {
        let ch = &self.channels[c];
        let vmax = self
            .records
            .get(k)
            .and_then(|r| r.vmax.get(c))
            .copied()
            .unwrap_or(f32::NAN);
        f64::from(vmax) / self.adc_max / ch.gain_v_per_unit
    }
    /// Sampling interval (median over records, as stored in float32), seconds.
    pub fn interval_s(&self) -> f64 {
        let mut v: Vec<f64> = self
            .records
            .iter()
            .map(|r| f64::from(r.interval_s))
            .filter(|x| x.is_finite() && *x > 0.0)
            .collect();
        v.sort_by(f64::total_cmp);
        match v.len() {
            0 => self
                .header
                .get("DT")
                .and_then(|d| number(d))
                .unwrap_or(f64::NAN),
            n if n % 2 == 1 => v[n / 2],
            n => f64::midpoint(v[n / 2 - 1], v[n / 2]),
        }
    }
    /// Recording start (`RTIME`, else `CTIME`), `yyyy-mm-ddThh:mm:ss` local time.
    pub fn recorded_at(&self) -> Option<String> {
        let t = self
            .header
            .get("RTIME")
            .or_else(|| self.header.get("CTIME"))?;
        let (d, h) = t.trim().split_once(' ')?;
        let mut p = d.split('/');
        let (day, month, year) = (p.next()?, p.next()?, p.next()?);
        let (day, month, year): (u32, u32, u32) =
            (day.parse().ok()?, month.parse().ok()?, year.parse().ok()?);
        let hms: Vec<u32> = h.split(':').filter_map(|x| x.parse().ok()).collect();
        if !(1..=31).contains(&day)
            || !(1..=12).contains(&month)
            || hms.len() != 3
            || hms[0] > 23
            || hms[1] > 59
            || hms[2] > 60
        {
            return None;
        }
        Some(format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
            hms[0], hms[1], hms[2]
        ))
    }
}

/// Parse the header and every record's analysis header.
pub fn parse_wcp(f: &mut SourceFile, path: &Path, file_len: u64) -> Result<WcpFile> {
    let head = read_block(f, path, 0, WCP_HEADER_LEN, file_len)?;
    let text = latin1(&head.bytes);
    let mut header = BTreeMap::new();
    for line in text.split(['\r', '\n']) {
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric()) {
                header.insert(k.to_string(), v.trim_end_matches('\0').trim().to_string());
            }
        }
    }
    let int = |k: &str| header.get(k).and_then(|v| v.trim().parse::<i64>().ok());
    let (Some(version), Some(nc), Some(nr), Some(nba), Some(nbd)) =
        (int("VER"), int("NC"), int("NR"), int("NBA"), int("NBD"))
    else {
        return Err(Error::corrupt(
            WINWCP_FORMAT_ID,
            "the text header lacks one of VER, NC, NR, NBA, NBD",
        ));
    };
    let adc_max = header
        .get("ADCMAX")
        .and_then(|v| number(v))
        .unwrap_or(f64::NAN);
    let usable = (1..=MAX_WCP_CHANNELS as i64).contains(&nc)
        && (0..=MAX_WCP_RECORDS as i64).contains(&nr)
        && (1..=64).contains(&nba)
        && (1..=1 << 24).contains(&nbd)
        && adc_max.is_finite()
        && adc_max > 0.0;
    if !usable {
        return Err(Error::corrupt(
            WINWCP_FORMAT_ID,
            format!(
                "header values NC={nc} NR={nr} NBA={nba} NBD={nbd} ADCMAX={adc_max} are not usable"
            ),
        ));
    }
    let nc = nc as usize;
    let mut channels = Vec::with_capacity(nc);
    for c in 0..nc {
        let get = |k: &str| header.get(&format!("{k}{c}")).cloned().unwrap_or_default();
        channels.push(WcpChannel {
            name: get("YN"),
            unit: get("YU"),
            gain_v_per_unit: number(&get("YG")).unwrap_or(f64::NAN),
            offset_in_frame: get("YO").trim().parse().unwrap_or(c),
            zero_level: number(&get("YZ")).unwrap_or(0.0),
        });
    }
    // YO is used as the column only when the offsets are a permutation of 0..NC
    let mut cols: Vec<usize> = channels.iter().map(|c| c.offset_in_frame).collect();
    cols.sort_unstable();
    let mut findings = Vec::new();
    if cols != (0..nc).collect::<Vec<_>>() {
        findings.push(Finding::warning(
            "channel_offsets",
            "the channels' frame offsets (YO) are not 0…NC−1; header order is used",
        ));
        for (c, ch) in channels.iter_mut().enumerate() {
            ch.offset_in_frame = c;
        }
    }
    let data_bytes = nbd as u64 * WCP_SECTOR;
    let samples = data_bytes / 2 / nc as u64;
    let mut out = WcpFile {
        header,
        version,
        adc_max,
        channels,
        records: Vec::new(),
        analysis_sectors: nba as u64,
        data_sectors: nbd as u64,
        samples,
        file_len,
        findings,
    };
    let nr = nr as u64;
    let rec = out.record_bytes();
    let complete = file_len.saturating_sub(WCP_HEADER_LEN) / rec;
    if complete < nr {
        out.findings.push(
            Finding::error(
                "truncated",
                format!(
                    "the header declares {nr} records; the file holds {complete} complete ones"
                ),
            )
            .at(WCP_HEADER_LEN + complete * rec),
        );
    }
    for k in 0..nr.min(complete) {
        let at = out.record_offset(k);
        let need = 24 + 4 * nc as u64;
        let b = read_block(f, path, at, need, file_len)?;
        let text = |o: u64, n: usize| b.text_at(at + o, n).unwrap_or_default();
        out.records.push(WcpRecord {
            status: text(0, 8),
            record_type: text(8, 4),
            group: b.f32_at(at + 12).unwrap_or(f32::NAN),
            time_s: b.f32_at(at + 16).unwrap_or(f32::NAN),
            interval_s: b.f32_at(at + 20).unwrap_or(f32::NAN),
            vmax: (0..nc as u64)
                .map(|c| b.f32_at(at + 24 + 4 * c).unwrap_or(f32::NAN))
                .collect(),
        });
    }
    Ok(out)
}

/// The WinWCP reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinWcpReader;

impl FormatReader for WinWcpReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::WINWCP)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: WINWCP_FORMAT_ID.into(),
            name: "WinWCP data file (.wcp)".into(),
            vendor: "Strathclyde Electrophysiology Software (WinWCP)".into(),
            extensions: vec!["wcp".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::WINWCP.confidence,
            known_gaps: vec![
                "The channel zero level (YZ) is reported, not subtracted (as Neo does); both public files have 0".into(),
                "Record status (accepted/rejected), type and group are reported per sweep; rejected records are not dropped".into(),
                "Stimulus protocols (.vpr) and analysis results are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if looks_like_wcp(head) {
            return Some(Detection {
                format_id: WINWCP_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["wcp"]).then(|| Detection {
            format_id: WINWCP_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("WinWCP extension but no `VER=` text header".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(WinWcpDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(WinWcpDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened `.wcp`.
#[derive(Debug)]
pub struct WinWcpDataset {
    path: PathBuf,
    fs: Fs,
    handle: Option<SourceFile>,
    file: WcpFile,
}

impl WinWcpDataset {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let fs = input.fs().clone();
        let mut f = fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let file_len = f.size().map_err(|e| Error::io(&path, e))?;
        let file = parse_wcp(&mut f, &path, file_len)?;
        Ok(WinWcpDataset {
            path,
            fs,
            handle: Some(f),
            file,
        })
    }

    /// The parsed file.
    pub fn wcp(&self) -> &WcpFile {
        &self.file
    }

    fn handle(&mut self) -> Result<&mut SourceFile> {
        if self.handle.is_none() {
            self.handle = Some(
                self.fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?,
            );
        }
        Ok(self.handle.as_mut().expect("just opened"))
    }

    fn gain_varies(&self) -> bool {
        let f = &self.file;
        (0..f.channels.len()).any(|c| {
            f.records
                .iter()
                .any(|r| r.vmax.get(c) != f.records.first().and_then(|r0| r0.vmax.get(c)))
        })
    }
}

impl Dataset for WinWcpDataset {
    fn info(&self) -> Result<FileInfo> {
        let f = &self.file;
        let mut notes = vec![format!(
            "WinWCP file version {}: one trace, {} channel(s), one sweep per record ({} records)",
            f.version,
            f.channels.len(),
            f.records.len()
        )];
        if self.gain_varies() {
            notes.push("the full-scale voltage (VMax) differs between records: each sweep is scaled with its own; channels[].scale is record 0's".into());
        }
        if f.findings
            .iter()
            .any(|x| x.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        let mut traces = Vec::new();
        if f.records.is_empty() {
            notes.push("the file holds no complete record".into());
        } else {
            let interval = f.interval_s();
            let mut extra = BTreeMap::new();
            extra.insert("file_version".into(), json!(f.version));
            if let Some(v) = f.header.get("VERPROG") {
                extra.insert("application".into(), json!("WinWCP"));
                extra.insert("application_version".into(), json!(v));
            }
            if let Some(t) = f.recorded_at() {
                extra.insert("recorded_at".into(), json!(t));
            }
            if let Some(id) = f.header.get("ID").filter(|s| !s.is_empty()) {
                extra.insert("comment".into(), json!(id));
            }
            extra.insert(
                "sweep_starts_s".into(),
                json!(
                    f.records
                        .iter()
                        .map(|r| f64::from(r.time_s))
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert(
                "record_status".into(),
                json!(
                    f.records
                        .iter()
                        .map(|r| r.status.clone())
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert(
                "record_type".into(),
                json!(
                    f.records
                        .iter()
                        .map(|r| r.record_type.clone())
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert(
                "record_group".into(),
                json!(
                    f.records
                        .iter()
                        .map(|r| f64::from(r.group))
                        .collect::<Vec<_>>()
                ),
            );
            let channels = f
                .channels
                .iter()
                .enumerate()
                .map(|(c, ch)| {
                    let mut e = BTreeMap::new();
                    e.insert("zero_level_adc".into(), json!(ch.zero_level));
                    e.insert("gain_v_per_unit".into(), json!(ch.gain_v_per_unit));
                    e.insert(
                        "vmax_v".into(),
                        json!(
                            f.records
                                .first()
                                .and_then(|r| r.vmax.get(c))
                                .map(|v| f64::from(*v))
                        ),
                    );
                    SignalChannelInfo {
                        index: c as u32,
                        name: if ch.name.is_empty() {
                            format!("ch{c}")
                        } else {
                            ch.name.clone()
                        },
                        unit: (!ch.unit.is_empty()).then(|| ch.unit.clone()),
                        dtype: "int16".into(),
                        scale: f.gain(0, c),
                        offset: 0.0,
                        extra: e,
                    }
                })
                .collect();
            traces.push(TraceInfo {
                index: 0,
                name: Some("record".into()),
                sample_rate_hz: 1.0 / interval,
                sample_count: f.samples,
                sweep_count: u32::try_from(f.records.len()).unwrap_or(u32::MAX),
                channels,
                start_s: Some(0.0),
                extra,
            });
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: f.file_len,
            format: WinWcpReader.descriptor(),
            format_version: Some(f.version.to_string()),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let f = &self.file;
        Ok(json!({
            "header": f.header,
            "records": f.records.iter().map(|r| json!({
                "status": r.status, "type": r.record_type, "group": r.group,
                "time_s": r.time_s, "interval_s": r.interval_s, "vmax_v": r.vmax,
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::PriorArt),
            ("traces[].sample_rate_hz", Source::PriorArt),
            ("traces[].sample_count", Source::PriorArt),
            ("traces[].sweep_count", Source::PriorArt),
            ("traces[].channels[].name", Source::PriorArt),
            ("traces[].channels[].unit", Source::PriorArt),
            ("traces[].channels[].scale", Source::PriorArt),
            ("traces[].extra.sweep_starts_s", Source::Inferred),
            ("traces[].extra.recorded_at", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let f = &self.file;
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: "text header".into(),
            offset: Some(0),
            size: Some(WCP_HEADER_LEN),
            image: None,
            details: json!({"keys": f.header.len()}),
        }];
        for (k, r) in f.records.iter().enumerate() {
            out.push(LsEntry {
                kind: "record".into(),
                name: format!("record {} ({})", k + 1, r.status),
                offset: Some(f.record_offset(k as u64)),
                size: Some(f.record_bytes()),
                image: None,
                details: json!({"time_s": r.time_s, "group": r.group}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            WINWCP_FORMAT_ID,
            "image planes",
            "WinWCP files hold sampled signals: use `openreadout trace`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if index != 0 || self.file.records.is_empty() {
            return Err(Error::Usage(format!("trace index {index} out of range")));
        }
        let k = sweep as usize;
        if k >= self.file.records.len() {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({} records)",
                self.file.records.len()
            )));
        }
        let n = self.file.samples;
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let count = max.min(n - first).min(MAX_WCP_READ);
        let nc = self.file.channels.len() as u64;
        let at = self.file.record_offset(k as u64)
            + self.file.analysis_sectors * WCP_SECTOR
            + first * nc * 2;
        let len = count * nc * 2;
        let file_len = self.file.file_len;
        let path = self.path.clone();
        let block = read_block(self.handle()?, &path, at, len, file_len)?;
        if (block.len() as u64) < len {
            return Err(Error::corrupt_at(
                WINWCP_FORMAT_ID,
                at,
                "samples cut off by the end of the file",
            ));
        }
        let f = &self.file;
        let mut channels = vec![Vec::with_capacity(count as usize); f.channels.len()];
        let frames = block.bytes.as_chunks::<2>().0;
        for (c, ch) in f.channels.iter().enumerate() {
            let g = f.gain(k, c);
            channels[c].extend(
                frames
                    .iter()
                    .skip(ch.offset_in_frame)
                    .step_by(f.channels.len())
                    .map(|x| f64::from(i16::from_le_bytes(*x)) * g),
            );
        }
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let f = &self.file;
        let mut r = CheckReport::new(self.path.display().to_string(), WINWCP_FORMAT_ID);
        r.performed("text header: VER, NC, NR, NBA, NBD, ADCMAX present and usable; channel keys YN/YU/YG/YO per channel");
        r.performed("file length against NR records of (NBA + NBD) × 512 bytes; every record's analysis header read");
        for x in &f.findings {
            r.push(x.clone());
        }
        let extra = f.file_len.saturating_sub(WCP_HEADER_LEN) % f.record_bytes();
        if extra != 0 {
            r.push(Finding::warning(
                "extra_bytes",
                format!("{extra} bytes after the last complete record"),
            ));
        }
        for (c, ch) in f.channels.iter().enumerate() {
            if !(ch.gain_v_per_unit.is_finite() && ch.gain_v_per_unit != 0.0) {
                r.push(Finding::error(
                    "bad_gain",
                    format!("channel {c}: YG is missing or zero"),
                ));
            }
        }
        let bad_dt = f
            .records
            .iter()
            .filter(|x| !(x.interval_s.is_finite() && x.interval_s > 0.0))
            .count();
        if bad_dt > 0 {
            r.push(Finding::warning(
                "bad_interval",
                format!("{bad_dt} record(s) without a positive sampling interval"),
            ));
        }
        let rejected = f
            .records
            .iter()
            .filter(|x| x.status.eq_ignore_ascii_case("REJECTED"))
            .count();
        if rejected > 0 {
            r.push(Finding::info(
                "rejected_records",
                format!("{rejected} record(s) are marked REJECTED"),
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests;
