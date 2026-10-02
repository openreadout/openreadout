//! The one `Dataset` both UNICORN readers return.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Measurement, MeasurementKind, Method, Origin,
    Quantity, Sample,
};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, LsEntry, Severity, SignalChannelInfo, Table,
    TableInfo, Trace, TraceInfo,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::SourceFile;
use openreadout_core::vocab;
use openreadout_core::{ColumnInfo, Error, Result};
use serde_json::{Value, json};

use crate::export::{Export, curve_points};
use crate::model::{Curve, CurveKind, PEAK_COLUMNS, Parsed, Points};

/// Where the file's bytes are.
#[derive(Debug)]
pub(crate) enum Backing {
    /// A `.res` file.
    Res { file: SourceFile, len: u64 },
    /// A UNICORN 6/7 export.
    Zip(Box<Export>),
}

/// An opened UNICORN result (`.res` or a result export `.zip`).
#[derive(Debug)]
pub struct FplcDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    file_len: u64,
    backing: Backing,
    parsed: Parsed,
    /// Decoded curves: trace → (volumes, values).
    cache: BTreeMap<u32, (Vec<f64>, Vec<f64>)>,
}

/// A stored integer's scale factor: a factor of 1/k (0.001, 0.01, 0.1) divides by k, so the
/// value is the decimal the instrument meant (15328 × 0.001 → 15.328, not 15.328000000000001).
#[derive(Debug, Clone, Copy)]
enum Scale {
    Divide(f64),
    Multiply(f64),
}

impl Scale {
    fn of(factor: f64) -> Scale {
        if factor > 0.0 && factor < 1.0 {
            let k = 1.0 / factor;
            if (k - k.round()).abs() < 1e-6 * k {
                return Scale::Divide(k.round());
            }
        }
        Scale::Multiply(factor)
    }
    fn apply(self, v: i32) -> f64 {
        match self {
            Scale::Divide(k) => f64::from(v) / k,
            Scale::Multiply(f) => f64::from(v) * f,
        }
    }
}

/// Round-trip-safe JSON number (non-finite → null).
fn num(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

impl FplcDataset {
    pub(crate) fn new(
        descriptor: FormatDescriptor,
        path: PathBuf,
        file_len: u64,
        backing: Backing,
        parsed: Parsed,
    ) -> Self {
        FplcDataset {
            descriptor,
            path,
            file_len,
            backing,
            parsed,
            cache: BTreeMap::new(),
        }
    }

    fn format_id(&self) -> &'static str {
        match self.backing {
            Backing::Res { .. } => crate::res::FORMAT_ID,
            Backing::Zip(_) => crate::export::FORMAT_ID,
        }
    }

    fn curve(&self, index: u32) -> Result<&Curve> {
        self.parsed.curves.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} curves)",
                self.parsed.curves.len()
            ))
        })
    }

    /// Volumes and values of a curve, decoded once.
    fn points(&mut self, index: u32) -> Result<&(Vec<f64>, Vec<f64>)> {
        if !self.cache.contains_key(&index) {
            let c = self.curve(index)?.clone();
            let decoded = self.decode(&c)?;
            self.cache.insert(index, decoded);
        }
        self.cache
            .get(&index)
            .ok_or_else(|| Error::Other("curve cache".into()))
    }

    fn decode(&self, c: &Curve) -> Result<(Vec<f64>, Vec<f64>)> {
        match (&self.backing, &c.points) {
            (
                Backing::Res { file, len },
                Points::Res {
                    offset,
                    volume_factor,
                    value_factor,
                },
            ) => {
                let bytes = c
                    .samples
                    .checked_mul(8)
                    .ok_or_else(|| Error::corrupt(self.format_id(), "curve size overflows"))?;
                let end = offset
                    .checked_add(bytes)
                    .filter(|e| *e <= *len)
                    .ok_or_else(|| {
                        Error::corrupt_at(
                            self.format_id(),
                            *offset,
                            format!(
                                "curve `{}` runs past the end of the file (truncated)",
                                c.name
                            ),
                        )
                    })?;
                let n = usize::try_from(end - offset)
                    .map_err(|_| Error::Other("curve larger than memory".into()))?;
                let mut buf = vec![0u8; n];
                file.read_exact_at(*offset, &mut buf)
                    .map_err(|e| Error::io(&self.path, e))?;
                let mut vol = Vec::with_capacity(n / 8);
                let mut val = Vec::with_capacity(n / 8);
                let (sv, sy) = (Scale::of(*volume_factor), Scale::of(*value_factor));
                for rec in buf.as_chunks::<8>().0 {
                    let v = i32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
                    let y = i32::from_le_bytes([rec[4], rec[5], rec[6], rec[7]]);
                    vol.push(sv.apply(v));
                    val.push(sy.apply(y));
                }
                Ok((vol, val))
            }
            (
                Backing::Zip(ex),
                Points::Zip {
                    member,
                    volume_grid,
                },
            ) => {
                let z = ex.nested(&crate::export::logical(member))?.ok_or_else(|| {
                    Error::corrupt(
                        self.format_id(),
                        format!("curve `{}`: empty point file", c.name),
                    )
                })?;
                let (v, a) = curve_points(&z, &c.name, *volume_grid)?;
                if v.len() != a.len() {
                    return Err(Error::corrupt(
                        self.format_id(),
                        format!(
                            "curve `{}`: {} volumes but {} values",
                            c.name,
                            v.len(),
                            a.len()
                        ),
                    ));
                }
                Ok((v, a))
            }
            _ => Err(Error::Other("curve storage does not match the file".into())),
        }
    }

    fn trace_info(&self, index: u32, c: &Curve) -> TraceInfo {
        let mut extra = c.extra.clone();
        extra.insert("kind".into(), json!(c.kind.as_str()));
        extra.insert("original".into(), json!(c.original));
        extra.insert(
            "axis".into(),
            json!({"quantity": "retention_volume", "unit": "ml", "irregular": true, "channel": 1}),
        );
        if let Some(w) = c.wavelength_nm {
            extra.insert("wavelength_nm".into(), json!(w));
        }
        if c.interval_min > 0.0 {
            extra.insert("interval_min".into(), num(c.interval_min));
            extra.insert("start_min".into(), num(c.start_min));
        }
        let rate = if c.interval_min > 0.0 {
            1.0 / (c.interval_min * 60.0)
        } else {
            0.0
        };
        let dtype = match c.points {
            Points::Res { .. } => "int32",
            Points::Zip { .. } => "float32",
        };
        let (vscale, yscale) = match c.points {
            Points::Res {
                volume_factor,
                value_factor,
                ..
            } => (volume_factor, value_factor),
            Points::Zip { .. } => (1.0, 1.0),
        };
        TraceInfo {
            index,
            name: Some(c.name.clone()),
            sample_rate_hz: if rate.is_finite() { rate } else { 0.0 },
            sample_count: c.samples,
            sweep_count: 1,
            channels: vec![
                SignalChannelInfo {
                    index: 0,
                    name: c.kind.as_str().to_string(),
                    unit: c.unit.clone(),
                    dtype: dtype.into(),
                    scale: yscale,
                    offset: 0.0,
                    extra: BTreeMap::new(),
                },
                SignalChannelInfo {
                    index: 1,
                    name: "volume".into(),
                    unit: Some("ml".into()),
                    dtype: dtype.into(),
                    scale: vscale,
                    offset: 0.0,
                    extra: BTreeMap::new(),
                },
            ],
            start_s: (c.interval_min > 0.0).then_some(c.start_min * 60.0),
            extra,
        }
    }

    fn event_table_info(&self, index: u32, l: &crate::model::EventList) -> TableInfo {
        let categories: Vec<&str> = l.events.iter().map(|e| e.text.as_str()).collect();
        let mut uniq: Vec<&str> = Vec::new();
        for c in categories {
            if !uniq.contains(&c) {
                uniq.push(c);
            }
        }
        TableInfo {
            index,
            name: Some(l.name.clone()),
            row_count: l.events.len() as u64,
            columns: vec![
                col(0, "time", Some("min"), "float64", BTreeMap::new()),
                col(1, "volume", Some("ml"), "float64", BTreeMap::new()),
                col(
                    2,
                    l.label,
                    None,
                    "uint32",
                    BTreeMap::from([("categories".to_string(), json!(uniq))]),
                ),
            ],
            extra: BTreeMap::from([("kind".to_string(), json!("events"))]),
        }
    }

    fn peak_table_info(&self, index: u32, t: &crate::model::PeakTable) -> TableInfo {
        let mut columns = Vec::new();
        for (k, (ours, _, unit)) in PEAK_COLUMNS.iter().enumerate() {
            let u = match unit {
                'r' => Some(t.retention_unit.clone()),
                'h' => t.height_unit.clone(),
                'a' => t
                    .height_unit
                    .as_ref()
                    .map(|h| format!("{h}·{}", t.retention_unit)),
                'p' => Some("%".into()),
                'c' => Some("mS/cm".into()),
                _ => None,
            };
            columns.push(col(
                k as u32,
                ours,
                u.as_deref(),
                "float64",
                BTreeMap::new(),
            ));
        }
        let names: Vec<&str> = t.peaks.iter().map(|p| p.name.as_str()).collect();
        if names.iter().any(|n| !n.is_empty()) {
            let mut uniq: Vec<&str> = Vec::new();
            for n in &names {
                if !uniq.contains(n) {
                    uniq.push(n);
                }
            }
            columns.push(col(
                columns.len() as u32,
                "name",
                None,
                "uint32",
                BTreeMap::from([("categories".to_string(), json!(uniq))]),
            ));
        }
        let mut extra = t.extra.clone();
        extra.insert("kind".into(), json!("vendor_peaks"));
        extra.insert("retention_basis".into(), json!(t.basis));
        if let Some(tr) = t.trace {
            extra.insert("trace".into(), json!(tr));
        }
        TableInfo {
            index,
            name: Some(t.name.clone()),
            row_count: t.peaks.len() as u64,
            columns,
            extra,
        }
    }

    fn info_notes(&self) -> Vec<String> {
        let mut notes = self.parsed.notes.clone();
        if let Some(z) = self.parsed.zero_volume_ml {
            notes.push(format!(
                "volumes are counted from the method start; UNICORN displays retention volumes from the last injection, at {z} ml (subtract it to match UNICORN's axis)"
            ));
        }
        let errors: Vec<&Finding> = self
            .parsed
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect();
        if let Some(first) = errors.first() {
            notes.push(format!(
                "the file is damaged ({}{}); run `check` for the list",
                first.message,
                if errors.len() > 1 {
                    format!(" and {} more", errors.len() - 1)
                } else {
                    String::new()
                }
            ));
        }
        notes
    }
}

fn col(
    index: u32,
    name: &str,
    unit: Option<&str>,
    dtype: &str,
    extra: BTreeMap<String, Value>,
) -> ColumnInfo {
    ColumnInfo {
        index,
        name: name.into(),
        label: None,
        dtype: dtype.into(),
        unit: unit.map(str::to_string),
        range: None,
        extra,
    }
}

impl Dataset for FplcDataset {
    fn info(&self) -> Result<FileInfo> {
        let traces = self
            .parsed
            .curves
            .iter()
            .enumerate()
            .map(|(i, c)| self.trace_info(i as u32, c))
            .collect();
        let mut tables = Vec::new();
        for l in &self.parsed.events {
            tables.push(self.event_table_info(tables.len() as u32, l));
        }
        for t in &self.parsed.peak_tables {
            tables.push(self.peak_table_info(tables.len() as u32, t));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: self.descriptor.clone(),
            format_version: self.parsed.format_version.clone(),
            plane_count: 0,
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            notes: self.info_notes(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(self.parsed.vendor.clone())
    }

    fn provenance(&self) -> ProvenanceMap {
        self.parsed.provenance.clone()
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self.parsed.entries.clone())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "a UNICORN result holds chromatogram curves (traces) and tables, not images; use `trace` or `table`".into(),
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        r.performed("every curve decoded: volumes and values counted, checked for non-finite numbers and for volumes that run backwards");
        for i in 0..self.parsed.curves.len() as u32 {
            let (name, samples, original) = {
                let c = &self.parsed.curves[i as usize];
                (c.name.clone(), c.samples, c.original)
            };
            match self.points(i) {
                Err(e) => r.push(Finding::error(
                    "unreadable_curve",
                    format!("trace {i} ({name}): {e}"),
                )),
                Ok((vol, val)) => {
                    if vol.len() as u64 != samples {
                        r.push(Finding::error(
                            "sample_count",
                            format!(
                                "trace {i} ({name}): {} samples decoded, {samples} declared",
                                vol.len()
                            ),
                        ));
                    }
                    let bad = val
                        .iter()
                        .chain(vol.iter())
                        .filter(|v| !v.is_finite())
                        .count();
                    if bad > 0 {
                        r.push(Finding::warning(
                            "non_finite_values",
                            format!("trace {i} ({name}) holds {bad} NaN or infinite numbers"),
                        ));
                    }
                    let back = vol
                        .windows(2)
                        .filter(|w| w[1] < w[0] - 1e-6 * w[0].abs().max(1.0))
                        .count();
                    if back > 0 && original {
                        r.push(Finding::warning(
                            "volume_decreases",
                            format!(
                                "trace {i} ({name}): the retention volume decreases {back} times (a reset or a reversed flow)"
                            ),
                        ));
                    }
                }
            }
        }
        Ok(r)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        match self.backing {
            Backing::Res { .. } => {
                r.performed("header and block directory parsed; every block's byte range checked against the file size");
            }
            Backing::Zip(_) => {
                r.performed("zip directory, Result.xml and the chromatogram documents parsed; each curve's nested zip indexed (CRC-32 checked on every member read)");
            }
        }
        for f in &self.parsed.findings {
            r.push(f.clone());
        }
        Ok(r)
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (a curve has one sweep)"
            )));
        }
        let (vol, val) = self.points(index)?;
        let n = vol.len() as u64;
        let first = first_sample.min(n);
        let len = max_samples.min(n - first);
        let (a, b) = (first as usize, (first + len) as usize);
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels: vec![val[a..b].to_vec(), vol[a..b].to_vec()],
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let ne = self.parsed.events.len();
        let slice = |n: usize| {
            let a = (first_row.min(n as u64)) as usize;
            let b = (first_row.saturating_add(max_rows).min(n as u64)) as usize;
            (a, b)
        };
        if let Some(l) = self.parsed.events.get(index as usize) {
            let mut uniq: Vec<&str> = Vec::new();
            for e in &l.events {
                if !uniq.contains(&e.text.as_str()) {
                    uniq.push(&e.text);
                }
            }
            let (a, b) = slice(l.events.len());
            let rows = &l.events[a..b];
            return Ok(Table {
                table: index,
                first_row: a as u64,
                columns: vec![
                    rows.iter().map(|e| e.time_min).collect(),
                    rows.iter().map(|e| e.volume_ml).collect(),
                    rows.iter()
                        .map(|e| uniq.iter().position(|u| *u == e.text).unwrap_or(0) as f64)
                        .collect(),
                ],
            });
        }
        let t = (index as usize)
            .checked_sub(ne)
            .and_then(|k| self.parsed.peak_tables.get(k))
            .ok_or_else(|| {
                Error::Usage(format!(
                    "table {index} out of range (file has {} tables)",
                    ne + self.parsed.peak_tables.len()
                ))
            })?;
        let (a, b) = slice(t.peaks.len());
        let rows = &t.peaks[a..b];
        let mut columns: Vec<Vec<f64>> = PEAK_COLUMNS
            .iter()
            .map(|(k, _, _)| {
                rows.iter()
                    .map(|p| p.values.get(k).copied().unwrap_or(f64::NAN))
                    .collect()
            })
            .collect();
        if t.peaks.iter().any(|p| !p.name.is_empty()) {
            let mut uniq: Vec<&str> = Vec::new();
            for p in &t.peaks {
                if !uniq.contains(&p.name.as_str()) {
                    uniq.push(&p.name);
                }
            }
            columns.push(
                rows.iter()
                    .map(|p| uniq.iter().position(|u| *u == p.name).unwrap_or(0) as f64)
                    .collect(),
            );
        }
        Ok(Table {
            table: index,
            first_row: a as u64,
            columns,
        })
    }

    fn experiment(&self) -> Option<Experiment> {
        experiment_of(&self.parsed, self.format_id()).map(|mut e| {
            crate::complete_provenance(&mut e);
            e
        })
    }
}

fn origin(s: Source, from: &str) -> Origin {
    Origin {
        source: s,
        from: from.to_string(),
    }
}

/// The experiment facts, with their origins.
fn experiment_of(p: &Parsed, format: &str) -> Option<Experiment> {
    let f = &p.facts;
    let src = Source::Inferred;
    let mut exp = Experiment::default();
    if let Some((v, from)) = &f.sample_id {
        exp.sample = Some(Sample {
            id: Some(v.clone()),
            source_field: Some(from.clone()),
            ..Sample::default()
        });
        exp.provenance.insert("sample.id".into(), origin(src, from));
    }
    let mut ins = ExperimentInstrument {
        vendor: Some("Cytiva (ÄKTA)".into()),
        software: Some("UNICORN".into()),
        kind: vocab::term("OBI:0000485"),
        ..ExperimentInstrument::default()
    };
    exp.provenance
        .insert("instrument.vendor".into(), origin(Source::Spec, format));
    exp.provenance
        .insert("instrument.software".into(), origin(Source::Spec, format));
    for (slot, key, fact) in [
        (&mut ins.model, "instrument.model", &f.model),
        (&mut ins.serial, "instrument.serial", &f.serial),
        (
            &mut ins.software_version,
            "instrument.software_version",
            &f.software_version,
        ),
    ] {
        if let Some((v, from)) = fact {
            *slot = Some(v.clone());
            exp.provenance.insert(key.into(), origin(src, from));
        }
    }
    exp.instrument = Some(ins);
    let mut method = Method::default();
    if let Some((v, from)) = &f.method_name {
        method.name = Some(v.clone());
        exp.provenance
            .insert("method.name".into(), origin(src, from));
    }
    method.technique = f
        .technique
        .and_then(vocab::term)
        .or_else(|| vocab::term("CHMO:0001004"));
    for (k, (q, from)) in &f.parameters {
        method.parameters.insert(k.clone(), q.clone());
        exp.provenance
            .insert(format!("method.parameters.{k}"), origin(src, from));
    }
    if let Some((v, from)) = &f.firmware {
        method
            .parameters
            .insert("firmware".into(), Quantity::plain(v.clone()));
        exp.provenance
            .insert("method.parameters.firmware".into(), origin(src, from));
    }
    exp.method = Some(method);
    let mut acq = Acquisition::default();
    if let Some((v, from, s)) = &f.started_at {
        acq.started_at = Some(v.clone());
        exp.provenance
            .insert("acquisition.started_at".into(), origin(*s, from));
    }
    if let Some((v, from, s)) = &f.ended_at {
        acq.ended_at = Some(v.clone());
        exp.provenance
            .insert("acquisition.ended_at".into(), origin(*s, from));
    }
    if let Some((v, from)) = &f.operator {
        acq.operator = Some(v.clone());
        exp.provenance
            .insert("acquisition.operator".into(), origin(src, from));
    }
    let dur = p
        .curves
        .iter()
        .filter(|c| c.original && c.interval_min > 0.0)
        .map(|c| (c.start_min + c.interval_min * c.samples.saturating_sub(1) as f64) * 60.0)
        .fold(0.0_f64, f64::max);
    if dur > 0.0 {
        acq.duration_s = Some(dur);
        exp.provenance.insert(
            "acquisition.duration_s".into(),
            origin(
                src,
                "the longest recorded curve: start + interval × samples",
            ),
        );
    }
    if acq != Acquisition::default() {
        exp.acquisition = Some(acq);
    }
    if !p.curves.is_empty() {
        let mut what: Vec<String> = Vec::new();
        for c in p.curves.iter().filter(|c| c.original) {
            let w = match (c.kind, c.wavelength_nm) {
                (CurveKind::Uv, Some(nm)) => format!("UV {nm} nm"),
                (CurveKind::Uv, None) => "UV".into(),
                (CurveKind::Conductivity, _) => "conductivity".into(),
                (CurveKind::Ph, _) => "pH".into(),
                (CurveKind::ConcentrationB, _) => "%B".into(),
                _ => continue,
            };
            if !what.contains(&w) {
                what.push(w);
            }
        }
        let fractions = p
            .events
            .iter()
            .filter(|l| l.name.ends_with("fractions"))
            .map(|l| l.events.len())
            .sum::<usize>();
        let mut parameters = BTreeMap::new();
        parameters.insert("curves".into(), Quantity::plain(p.curves.len()));
        if fractions > 0 {
            parameters.insert("fraction_marks".into(), Quantity::plain(fractions));
        }
        exp.measurements.push(Measurement {
            kind: MeasurementKind::Trace,
            indices: (0..p.curves.len() as u32).collect(),
            what: format!(
                "liquid chromatography (ÄKTA): {}; {} curves{}",
                if what.is_empty() {
                    "curves".into()
                } else {
                    what.join(", ")
                },
                p.curves.len(),
                if fractions > 0 {
                    format!(", {fractions} fraction marks")
                } else {
                    String::new()
                }
            ),
            technique: m_technique(p),
            terms: Vec::new(),
            parameters,
        });
    }
    (!exp.is_empty()).then_some(exp)
}

fn m_technique(p: &Parsed) -> Option<openreadout_core::vocab::Term> {
    p.facts
        .technique
        .and_then(vocab::term)
        .or_else(|| vocab::term("CHMO:0001004"))
}
