//! `Dataset`s: an NSx file is one trace (data packets are sweeps); a NEV file is one table of
//! data packets.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_block;
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::nev::{COMMENT_PACKET, MAX_ELECTRODE_ID, NevFile, nev_packet, parse_nev};
use crate::nsx::{NsxFile, NsxSpec, parse_nsx};
use crate::{BlackrockReader, FORMAT_ID};

/// Rows decoded per `read_table` call at most.
pub const MAX_TABLE_READ: u64 = 1 << 20;
/// Comments collected for `info --view full` at most.
pub const MAX_COMMENTS: usize = 10_000;

fn handle<'a>(fs: &Fs, h: &'a mut Option<SourceFile>, path: &Path) -> Result<&'a mut SourceFile> {
    if h.is_none() {
        *h = Some(fs.open(path).map_err(|e| Error::io(path, e))?);
    }
    Ok(h.as_mut().expect("just opened"))
}

fn not_images() -> Error {
    Error::unsupported(
        FORMAT_ID,
        "image planes",
        "Blackrock files hold signals (NSx) or events (NEV), not images: use `openreadout trace` or `openreadout export --to csv`.",
    )
}

// ------------------------------------------------------------------ NSx

/// An opened NSx file.
#[derive(Debug)]
pub struct NsxDataset {
    path: PathBuf,
    nsx: NsxFile,
    handle: Option<SourceFile>,
    /// Where the file is read from.
    fs: Fs,
}

/// Electrode id → digitization factor (nV) from the NEV next to a spec 2.1 NSx file.
fn companion_factors(fs: &Fs, path: &Path) -> Option<(String, BTreeMap<u16, u16>)> {
    let nev = path.with_extension("nev");
    let nev = if fs.exists(&nev) {
        nev
    } else {
        path.with_extension("NEV")
    };
    let mut f = fs.open(&nev).ok()?;
    let len = f.metadata().ok()?.len();
    let h = parse_nev(&mut f, &nev, len).ok()?;
    let m: BTreeMap<u16, u16> = h
        .waveforms
        .iter()
        .map(|(k, w)| (*k, w.digitization_nv))
        .collect();
    Some((nev.file_name()?.to_string_lossy().into_owned(), m))
}

impl NsxDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let mut head = [0u8; 8];
        let n = std::io::Read::read(&mut f, &mut head).map_err(|e| Error::io(path, e))?;
        let factors = if n == 8 && &head == b"NEURALSG" {
            companion_factors(fs, path)
        } else {
            None
        };
        let nsx = parse_nsx(&mut f, path, len, factors, false)?;
        Ok(NsxDataset {
            path: path.to_path_buf(),
            nsx,
            handle: Some(f),
            fs: fs.clone(),
        })
    }

    /// The parsed header and packet index.
    pub fn nsx(&self) -> &NsxFile {
        &self.nsx
    }

    fn trace_info(&self) -> TraceInfo {
        let n = &self.nsx;
        let res = f64::from(n.time_resolution.max(1));
        let mut extra = BTreeMap::new();
        extra.insert("spec".into(), json!(n.spec.name()));
        if let Some((a, b)) = n.version {
            extra.insert("file_version".into(), json!(format!("{a}.{b}")));
        }
        extra.insert("label".into(), json!(n.label));
        if !n.comment.is_empty() {
            extra.insert("comment".into(), json!(n.comment));
        }
        extra.insert("period".into(), json!(n.period));
        extra.insert("timestamp_resolution_hz".into(), json!(n.time_resolution));
        if let Some(t) = &n.recorded_at {
            extra.insert("recorded_at".into(), json!(t));
        }
        extra.insert("ptp".into(), json!(n.ptp_stride.is_some()));
        if let Some(src) = &n.scaling_source {
            extra.insert("scaling_source".into(), json!(src));
        }
        let counts: Vec<u64> = n.sweeps.iter().map(|s| s.sample_count).collect();
        if counts.windows(2).any(|w| w[0] != w[1]) {
            extra.insert("sweep_sample_counts".into(), json!(counts));
        }
        if n.spec != NsxSpec::V21 {
            extra.insert(
                "sweep_starts_s".into(),
                json!(
                    n.sweeps
                        .iter()
                        .map(|s| s.timestamp as f64 / res)
                        .collect::<Vec<_>>()
                ),
            );
            extra.insert(
                "sweep_start_timestamps".into(),
                json!(n.sweeps.iter().map(|s| s.timestamp).collect::<Vec<_>>()),
            );
        }
        let channels = n
            .channels
            .iter()
            .map(|c| {
                let mut e = BTreeMap::new();
                e.insert("electrode_id".into(), json!(c.electrode_id));
                if let Some(v) = c.connector {
                    e.insert("connector".into(), json!(v));
                }
                if let Some(v) = c.pin {
                    e.insert("pin".into(), json!(v));
                }
                if let (Some(a), Some(b), Some(x), Some(y)) = (c.min_digital, c.max_digital, c.min_analog, c.max_analog) {
                    e.insert("digital_range".into(), json!([a, b]));
                    e.insert("analog_range".into(), json!([x, y]));
                }
                let filt = |f: Option<(u32, u32, u16)>| f.map(|(hz, order, kind)| json!({"corner_hz": f64::from(hz) / 1000.0, "order": order, "kind_code": kind}));
                if let Some(v) = filt(c.highpass) {
                    e.insert("highpass".into(), v);
                }
                if let Some(v) = filt(c.lowpass) {
                    e.insert("lowpass".into(), v);
                }
                SignalChannelInfo {
                    index: c.index,
                    name: if c.label.is_empty() { format!("elec{}", c.electrode_id) } else { c.label.clone() },
                    unit: (!c.units.is_empty()).then(|| c.units.clone()),
                    dtype: "int16".into(),
                    scale: c.scale,
                    offset: c.offset,
                    extra: e,
                }
            })
            .collect();
        TraceInfo {
            index: 0,
            name: (!n.label.is_empty()).then(|| n.label.clone()),
            sample_rate_hz: n.sample_rate_hz(),
            sample_count: n.max_sweep_len(),
            sweep_count: n.sweeps.len() as u32,
            channels,
            start_s: n
                .sweeps
                .first()
                .filter(|_| n.spec != NsxSpec::V21)
                .map(|s| s.timestamp as f64 / res),
            extra,
        }
    }
}

impl Dataset for NsxDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut notes = vec!["one trace: all channels share the sampling rate; data packets (pauses) are sweeps; values = raw × scale + offset in each channel's unit".to_string()];
        if self
            .nsx
            .findings
            .iter()
            .any(|f| f.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.nsx.file_len,
            format: BlackrockReader.descriptor(),
            format_version: Some(match self.nsx.version {
                Some((a, b)) => format!("{a}.{b}"),
                None => "2.1".into(),
            }),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: vec![self.trace_info()],
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let n = &self.nsx;
        Ok(json!({
            "spec": n.spec.name(), "version": n.version, "label": n.label, "comment": n.comment,
            "period": n.period, "timestamp_resolution": n.time_resolution, "header_len": n.header_len,
            "channels": n.channels.iter().map(|c| json!({
                "electrode_id": c.electrode_id, "label": c.label, "connector": c.connector, "pin": c.pin,
                "min_digital": c.min_digital, "max_digital": c.max_digital, "min_analog": c.min_analog,
                "max_analog": c.max_analog, "units": c.units, "highpass": c.highpass, "lowpass": c.lowpass,
            })).collect::<Vec<_>>(),
            "packets": n.packets.iter().take(10_000).map(|p| json!({"offset": p.offset, "timestamp": p.timestamp, "points": p.points})).collect::<Vec<_>>(),
            "ptp_packet_count": n.ptp_packet_count,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("traces[].sample_rate_hz", Source::VendorImpl),
            ("traces[].channels[].scale", Source::VendorImpl),
            ("traces[].channels[].offset", Source::VendorImpl),
            ("traces[].channels[].unit", Source::VendorImpl),
            ("traces[].channels[].name", Source::VendorImpl),
            ("traces[].sweep_count", Source::VendorImpl),
            ("traces[].extra.recorded_at", Source::VendorImpl),
            ("traces[].channels[spec 2.1].scale", Source::PriorArt),
            ("traces[].channels[spec 2.1].name", Source::PriorArt),
            ("traces[].sweep_count[ptp]", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let n = &self.nsx;
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: format!("NSx {} header", n.spec.name()),
            offset: Some(0),
            size: Some(n.header_len),
            image: None,
            details: json!({"channels": n.channels.len()}),
        }];
        for (i, s) in n.sweeps.iter().enumerate() {
            let start = n.sample_offset(s, 0).unwrap_or(0);
            let size = match n.ptp_stride {
                Some(stride) => s.packet_count * stride,
                None => s.sample_count * n.frame_len(),
            };
            out.push(LsEntry {
                kind: "sweep".into(),
                name: format!("data packet {i}"),
                offset: Some(start),
                size: Some(size),
                image: None,
                details: json!({"samples": s.sample_count, "timestamp": s.timestamp, "packets": s.packet_count}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(not_images())
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "trace {index} out of range (an NSx file holds one trace, index 0)"
            )));
        }
        let s = self
            .nsx
            .sweeps
            .get(sweep as usize)
            .cloned()
            .ok_or_else(|| {
                Error::Usage(format!(
                    "sweep {sweep} out of range (file has {} data packets)",
                    self.nsx.sweeps.len()
                ))
            })?;
        if first_sample > s.sample_count {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end of sweep {sweep} ({} samples)",
                s.sample_count
            )));
        }
        let n = max_samples.min(s.sample_count - first_sample).min(1 << 24);
        let nch = self.nsx.channels.len();
        let mut chans: Vec<Vec<f64>> = vec![Vec::with_capacity(n as usize); nch];
        let (path, file_len) = (self.path.clone(), self.nsx.file_len);
        let frame = self.nsx.frame_len();
        let stride = self.nsx.ptp_stride.unwrap_or(frame);
        let mut done = 0u64;
        while done < n {
            let k = (n - done).min(65_536);
            let at = self
                .nsx
                .sample_offset(&s, first_sample + done)
                .ok_or_else(|| Error::corrupt(FORMAT_ID, "sample offset overflow"))?;
            let len = (k - 1) * stride + frame;
            if at + len > file_len {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    at,
                    "samples run past the end of the file (truncated)",
                ));
            }
            let b = read_block(
                handle(&self.fs, &mut self.handle, &path)?,
                &path,
                at,
                len,
                file_len,
            )?;
            for j in 0..k {
                let base = at + j * stride;
                for (c, ch) in self.nsx.channels.iter().enumerate() {
                    let raw = b
                        .i16_at(base + 2 * c as u64)
                        .ok_or_else(|| Error::corrupt_at(FORMAT_ID, base, "sample cut off"))?;
                    chans[c].push(f64::from(raw) * ch.scale + ch.offset);
                }
            }
            done += k;
        }
        Ok(Trace {
            trace: 0,
            sweep,
            first_sample,
            channels: chans,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed(
            "signature and basic header; header length = 314 + 66 × channels; `CC` channel headers",
        );
        r.performed("every data packet: 0x01 header byte, declared sample count inside the file (truncation)");
        r.performed("PTP files: every packet timestamp read; gaps reported");
        let (path, len) = (self.path.clone(), self.nsx.file_len);
        let full = if self.nsx.ptp_stride.is_some() {
            let f = handle(&self.fs, &mut self.handle, &path)?;
            let factors = None;
            let mut again = parse_nsx(f, &path, len, factors, true)?;
            again.channels = self.nsx.channels.clone();
            again
        } else {
            self.nsx.clone()
        };
        for f in &full.findings {
            r.push(f.clone());
        }
        if full.sweeps.len() > 1 {
            r.push(Finding::info(
                "segments",
                format!(
                    "{} gap-free sweeps (recording paused or samples lost)",
                    full.sweeps.len()
                ),
            ));
        }
        // a packet that starts before the previous one ended means the clock was reset
        let ticks_per_sample = f64::from(full.period) / crate::nsx::PERIOD_CLOCK_HZ
            * f64::from(full.time_resolution.max(1));
        let resets = full
            .sweeps
            .windows(2)
            .filter(|w| {
                (w[1].timestamp as f64)
                    < w[0].timestamp as f64 + w[0].sample_count as f64 * ticks_per_sample - 0.5
            })
            .count();
        if resets > 0 {
            r.push(Finding::warning(
                "clock_reset",
                format!("{resets} data packet(s) start before the previous one ends (clock reset)"),
            ));
        }
        if self.nsx.channels.iter().any(|c| c.units.is_empty()) {
            r.push(Finding::info(
                "no_units",
                "some channels have no unit (raw counts)",
            ));
        }
        Ok(r)
    }
}

// ------------------------------------------------------------------ NEV

/// An opened NEV file.
#[derive(Debug)]
pub struct NevDataset {
    path: PathBuf,
    nev: NevFile,
    handle: Option<SourceFile>,
    /// Where the file is read from.
    fs: Fs,
}

/// Packet kinds in the `kind` column.
pub const KIND_DIGITAL: f64 = 0.0;
pub const KIND_SPIKE: f64 = 1.0;
pub const KIND_COMMENT: f64 = 2.0;
pub const KIND_OTHER: f64 = 3.0;

impl NevDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let nev = parse_nev(&mut f, path, len)?;
        Ok(NevDataset {
            path: path.to_path_buf(),
            nev,
            handle: Some(f),
            fs: fs.clone(),
        })
    }

    /// The parsed header.
    pub fn nev(&self) -> &NevFile {
        &self.nev
    }

    fn columns(&self) -> Vec<ColumnInfo> {
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
            col(1, "packet_id".into(), "uint16", None),
            col(2, "kind".into(), "uint8", None),
            col(3, "code".into(), "uint8", None),
            col(4, "digital".into(), "uint16", None),
        ];
        for i in 0..self.nev.max_waveform_samples() as usize {
            v.push(col(v.len(), format!("w{i}"), "float64", Some("µV")));
        }
        v
    }

    fn read_packets(&mut self, first: u64, n: u64) -> Result<Vec<crate::nev::NevPacket>> {
        let pl = self.nev.packet_len;
        let at = self.nev.header_len + first * pl;
        let (path, len) = (self.path.clone(), self.nev.file_len);
        let b = read_block(
            handle(&self.fs, &mut self.handle, &path)?,
            &path,
            at,
            n * pl,
            len,
        )?;
        (0..n)
            .map(|k| {
                nev_packet(&self.nev, &b, at + k * pl)
                    .ok_or_else(|| Error::corrupt_at(FORMAT_ID, at + k * pl, "data packet cut off"))
            })
            .collect()
    }
}

impl Dataset for NevDataset {
    fn info(&self) -> Result<FileInfo> {
        let n = &self.nev;
        let mut extra = BTreeMap::new();
        extra.insert("signature".into(), json!(n.signature));
        extra.insert(
            "file_version".into(),
            json!(format!("{}.{}", n.version.0, n.version.1)),
        );
        extra.insert("timestamp_resolution_hz".into(), json!(n.time_resolution));
        extra.insert("waveform_rate_hz".into(), json!(n.sample_resolution));
        extra.insert("packet_size".into(), json!(n.packet_len));
        if let Some(t) = &n.recorded_at {
            extra.insert("recorded_at".into(), json!(t));
        }
        if !n.application.is_empty() {
            extra.insert("application".into(), json!(n.application));
        }
        if !n.comment.is_empty() {
            extra.insert("comment".into(), json!(n.comment));
        }
        extra.insert(
            "kinds".into(),
            json!({"0": "digital/serial input", "1": "spike", "2": "comment", "3": "other"}),
        );
        extra.insert("code".into(), json!("spike: unit class (0 unsorted, 255 noise); digital: insertion reason bits; comment: character set"));
        extra.insert(
            "electrodes".into(),
            json!(n.waveforms.values().map(|w| json!({
                "electrode_id": w.electrode_id, "label": n.labels.get(&w.electrode_id), "connector": w.connector, "pin": w.pin,
                "uv_per_count": crate::nsx::spec21_factor(w.digitization_nv) / 1000.0,
                "high_threshold_uv": w.high_threshold_uv, "low_threshold_uv": w.low_threshold_uv,
                "sorted_units": w.sorted_units, "spike_width": w.spike_width,
            })).collect::<Vec<_>>()),
        );
        if !n.digital_labels.is_empty() {
            extra.insert("digital_labels".into(), json!(n.digital_labels.iter().map(|(l, m)| json!({"label": l, "mode": if *m == 1 { "parallel" } else { "serial" }})).collect::<Vec<_>>()));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: n.file_len,
            format: BlackrockReader.descriptor(),
            format_version: Some(format!("{}.{}", n.version.0, n.version.1)),
            images: Vec::new(),
            tables: vec![TableInfo {
                index: 0,
                name: Some("packets".into()),
                row_count: n.packet_count(),
                columns: self.columns(),
                extra,
            }],
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count: 0,
            notes: vec!["one table row per data packet in file order: kind 0 digital/serial, 1 spike (waveform w0..wN in µV), 2 comment (text in `info --view full`), 3 other".into()],
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let n = self.nev.clone();
        let mut me = NevDataset {
            path: self.path.clone(),
            nev: n.clone(),
            handle: None,
            fs: self.fs.clone(),
        };
        let mut comments = Vec::new();
        let mut counts: BTreeMap<u16, u64> = BTreeMap::new();
        let total = n.packet_count();
        let mut pos = 0;
        while pos < total {
            let k = 65_536.min(total - pos);
            for p in me.read_packets(pos, k)? {
                *counts.entry(p.packet_id).or_default() += 1;
                if p.packet_id == COMMENT_PACKET && comments.len() < MAX_COMMENTS {
                    comments.push(json!({"time_s": p.timestamp as f64 / f64::from(n.time_resolution.max(1)), "text": p.text}));
                }
            }
            pos += k;
        }
        Ok(json!({
            "signature": n.signature, "version": [n.version.0, n.version.1], "flags": n.flags,
            "header_len": n.header_len, "packet_len": n.packet_len,
            "extended_headers": n.ext_headers.iter().map(|e| json!({"id": e.id, "offset": e.offset})).collect::<Vec<_>>(),
            "comments": comments,
            "packet_id_counts": counts,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("tables[].columns", Source::VendorImpl),
            ("tables[].extra.electrodes", Source::VendorImpl),
            ("tables[].extra.recorded_at", Source::VendorImpl),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let n = &self.nev;
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: "NEV basic header".into(),
            offset: Some(0),
            size: Some(336),
            image: None,
            details: json!({"signature": n.signature}),
        }];
        for e in &n.ext_headers {
            out.push(LsEntry {
                kind: "extended-header".into(),
                name: e.id.clone(),
                offset: Some(e.offset),
                size: Some(32),
                image: None,
                details: Value::Null,
            });
        }
        out.push(LsEntry {
            kind: "records".into(),
            name: "data packets".into(),
            offset: Some(n.header_len),
            size: Some(n.packet_count() * n.packet_len),
            image: None,
            details: json!({"packets": n.packet_count(), "packet_size": n.packet_len}),
        });
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(not_images())
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "table {index} out of range (a NEV file holds 1 table)"
            )));
        }
        let total = self.nev.packet_count();
        if first_row > total {
            return Err(Error::Usage(format!(
                "first row {first_row} is past the last packet ({total} packets)"
            )));
        }
        let n = max_rows.min(total - first_row).min(MAX_TABLE_READ);
        let width = self.nev.max_waveform_samples() as usize;
        let mut cols: Vec<Vec<f64>> = vec![Vec::with_capacity(n as usize); 5 + width];
        let res = f64::from(self.nev.time_resolution.max(1));
        for p in self.read_packets(first_row, n)? {
            cols[0].push(p.timestamp as f64 / res);
            cols[1].push(f64::from(p.packet_id));
            let (kind, spike) = match p.packet_id {
                0 => (KIND_DIGITAL, false),
                COMMENT_PACKET => (KIND_COMMENT, false),
                e if (1..=MAX_ELECTRODE_ID).contains(&e) => (KIND_SPIKE, true),
                _ => (KIND_OTHER, false),
            };
            cols[2].push(kind);
            cols[3].push(if kind == KIND_OTHER {
                f64::NAN
            } else {
                f64::from(p.code)
            });
            cols[4].push(p.digital.map_or(f64::NAN, f64::from));
            let uv = if spike {
                self.nev.uv_per_count(p.packet_id).unwrap_or(1.0)
            } else {
                1.0
            };
            for i in 0..width {
                cols[5 + i].push(p.waveform.get(i).map_or(f64::NAN, |v| f64::from(*v) * uv));
            }
        }
        Ok(Table {
            table: 0,
            first_row,
            columns: cols,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("signature and basic header; header length = 336 + 32 × extended headers");
        r.performed("data section is a whole number of fixed-size packets; timestamps do not decrease within a recording");
        for f in &self.nev.findings {
            r.push(f.clone());
        }
        let total = self.nev.packet_count();
        let mut prev = 0u64;
        let mut back = 0u64;
        let mut undocumented = 0u64;
        let mut pos = 0;
        while pos < total {
            let k = 65_536.min(total - pos);
            for p in self.read_packets(pos, k)? {
                if p.timestamp < prev {
                    back += 1;
                }
                prev = p.timestamp;
                if p.packet_id > MAX_ELECTRODE_ID && p.packet_id < 0xFFF9 {
                    undocumented += 1;
                }
            }
            pos += k;
        }
        if back > 0 {
            r.push(Finding::warning(
                "timestamps_decrease",
                format!("{back} packet timestamps go backwards (clock reset or restart)"),
            ));
        }
        if undocumented > 0 {
            r.push(Finding::info(
                "unknown_packets",
                format!("{undocumented} packets with undocumented ids"),
            ));
        }
        if total == 0 {
            r.push(Finding::warning(
                "no_packets",
                "the file holds no data packets",
            ));
        }
        Ok(r)
    }
}
