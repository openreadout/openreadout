//! `Dataset` for AIA/ANDI netCDF files: the chromatography template (one trace from
//! `ordinate_values`, the peak table as a table) and the mass-spectrometry template (one spectra
//! run from `scan_index`/`point_count`/`mass_values`/`intensity_values`, and the total-ion
//! chromatogram as a trace when the scans are evenly spaced). Vocabulary:
//! `docs/formats/andi-chrom.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, InstrumentInfo, LsEntry, SignalChannelInfo,
    SpectraInfo, Spectrum, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::binary::tidy;
use crate::netcdf::{AttrValue, NcType, NetCdf, Variable, read_header_in};
use crate::{ANDI_ID, AndiReader};

/// Which ANDI template a file follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndiTemplate {
    /// ASTM E1947 chromatography (`aia_template_revision`, `ordinate_values`).
    Chromatography,
    /// ASTM E2077 mass spectrometry (`ms_template_revision`, `mass_values`).
    MassSpectrometry,
    /// A netCDF file without either marker.
    Unknown,
}

impl AndiTemplate {
    /// Our name for the template (JSON).
    pub fn name(self) -> &'static str {
        match self {
            AndiTemplate::Chromatography => "chromatography",
            AndiTemplate::MassSpectrometry => "mass-spectrometry",
            AndiTemplate::Unknown => "unknown",
        }
    }
}

/// Classify a parsed header.
pub fn andi_template(nc: &NetCdf) -> AndiTemplate {
    if nc.attr("ms_template_revision").is_some() || nc.var("mass_values").is_some() {
        AndiTemplate::MassSpectrometry
    } else if nc.attr("aia_template_revision").is_some() || nc.var("ordinate_values").is_some() {
        AndiTemplate::Chromatography
    } else {
        AndiTemplate::Unknown
    }
}

/// ANDI time stamps `YYYYMMDDhhmmss±hhmm` to ISO-8601; all-zero stamps give `None`.
pub fn andi_timestamp(s: &str) -> Option<String> {
    // `20130507123000 + 0000` (Chrom-Card) has spaces around the sign
    let s: String = s.split_whitespace().collect();
    let s = s.as_str();
    let digits: String = s.chars().take(14).collect();
    if digits.len() < 14
        || !digits.chars().all(|c| c.is_ascii_digit())
        || digits.starts_with("0000")
    {
        return None;
    }
    let n = |a: usize, b: usize| digits[a..b].parse::<u32>().ok();
    let zone = s.get(14..19).and_then(|z| {
        let sign = match z.chars().next()? {
            '+' => 1,
            '-' => -1,
            _ => return None,
        };
        let h: i32 = z.get(1..3)?.parse().ok()?;
        let m: i32 = z.get(3..5)?.parse().ok()?;
        Some(sign * (h * 60 + m))
    });
    crate::binary::iso(
        n(0, 4)?,
        n(4, 6)?,
        n(6, 8)?,
        n(8, 10)?,
        n(10, 12)?,
        n(12, 14)?,
        zone,
    )
}

/// Evenly spaced times: `(first, step)` when every step is within 1 % of the mean step.
pub(crate) fn uniform_times(t: &[f64]) -> Option<(f64, f64)> {
    if t.len() < 2 {
        return None;
    }
    let step = (t[t.len() - 1] - t[0]) / (t.len() - 1) as f64;
    if step.is_nan() || step <= 0.0 {
        return None;
    }
    let ok = t
        .windows(2)
        .all(|w| ((w[1] - w[0]) - step).abs() <= 0.01 * step);
    ok.then_some((t[0], step))
}

/// An opened ANDI netCDF file.
#[derive(Debug)]
pub struct AndiDataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    size: u64,
    nc: NetCdf,
    template: AndiTemplate,
    /// MS: per-scan retention time (s), start index, point count, TIC.
    scans: Option<Scans>,
    handle: Option<SourceFile>,
}

#[derive(Debug, Clone)]
struct Scans {
    times: Vec<f64>,
    index: Vec<f64>,
    count: Vec<f64>,
    tic: Option<Vec<f64>>,
}

impl AndiDataset {
    /// Polarity (`test_ionization_polarity`) and centroid mode (`experiment_type`) of every
    /// ANDI/MS scan: the template records them once per file.
    fn ms_polarity_and_mode(&self) -> (String, bool) {
        let polarity =
            text_attr(&self.nc, "test_ionization_polarity").map_or("unknown".into(), |p| {
                let p = p.to_ascii_lowercase();
                if p.starts_with("pos") {
                    "positive".to_string()
                } else if p.starts_with("neg") {
                    "negative".to_string()
                } else {
                    "unknown".to_string()
                }
            });
        let centroided = text_attr(&self.nc, "experiment_type")
            .is_some_and(|t| t.to_ascii_lowercase().starts_with("centroid"));
        (polarity, centroided)
    }
}

fn text_attr(nc: &NetCdf, name: &str) -> Option<String> {
    nc.attr(name)
        .and_then(AttrValue::as_text)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn var_attr<'a>(v: &'a Variable, name: &str) -> Option<&'a AttrValue> {
    v.attributes
        .iter()
        .find(|a| a.name == name)
        .map(|a| &a.value)
}

/// `scale_factor`/`add_offset` of a variable (1, 0 when absent).
fn var_scaling(v: &Variable) -> (f64, f64) {
    let s = var_attr(v, "scale_factor")
        .and_then(AttrValue::as_f64)
        .filter(|x| x.is_finite() && *x != 0.0)
        .unwrap_or(1.0);
    let o = var_attr(v, "add_offset")
        .and_then(AttrValue::as_f64)
        .filter(|x| x.is_finite())
        .unwrap_or(0.0);
    (s, o)
}

/// Global attributes mapped into `extra` (ANDI name → our name).
const CHROM_ATTRS: &[(&str, &str)] = &[
    ("sample_name", "sample_name"),
    ("sample_id", "sample_id"),
    ("sample_type", "sample_type"),
    ("sample_id_comments", "sample_comments"),
    ("operator_name", "operator"),
    ("dataset_origin", "origin"),
    ("dataset_owner", "owner"),
    ("experiment_title", "title"),
    ("separation_experiment_type", "separation"),
    ("company_method_name", "method"),
    ("detection_method_name", "detection_method"),
    ("detector_name", "detector"),
    ("source_file_reference", "source_file"),
    ("aia_template_revision", "template_revision"),
    ("ms_template_revision", "template_revision"),
    ("dataset_completeness", "completeness"),
    ("experiment_type", "experiment_type"),
    ("test_separation_type", "separation"),
    ("test_ms_inlet", "inlet"),
    ("test_ionization_mode", "ionization"),
    ("test_ionization_polarity", "polarity"),
    ("test_detector_type", "detector"),
    ("test_scan_function", "scan_function"),
];

impl AndiDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let size = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
        let nc = read_header_in(fs, path)?;
        let template = andi_template(&nc);
        let mut ds = AndiDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            size,
            nc,
            template,
            scans: None,
            handle: None,
        };
        if template == AndiTemplate::MassSpectrometry {
            ds.scans = ds.load_scans().ok();
        }
        Ok(ds)
    }

    /// The parsed netCDF header (for library users).
    pub fn netcdf(&self) -> &NetCdf {
        &self.nc
    }

    /// The template this file follows.
    pub fn template(&self) -> AndiTemplate {
        self.template
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

    fn read_all(&mut self, name: &str) -> Result<Vec<f64>> {
        let v = self
            .nc
            .var(name)
            .cloned()
            .ok_or_else(|| Error::corrupt(ANDI_ID, format!("variable {name} is missing")))?;
        let n = self.nc.element_count(&v);
        let nc = self.nc.clone();
        let path = self.path.clone();
        nc.read_f64(self.handle()?, &path, &v, 0, n)
    }

    /// Scalar variable (or global attribute of the same name) as a number.
    fn scalar(&mut self, name: &str) -> Option<f64> {
        if let Some(v) = self.nc.var(name).cloned()
            && self.nc.element_count(&v) >= 1
        {
            let nc = self.nc.clone();
            let path = self.path.clone();
            let h = self.handle().ok()?;
            return nc
                .read_f64(h, &path, &v, 0, 1)
                .ok()?
                .first()
                .copied()
                .filter(|x| x.is_finite() && *x > -9999.0);
        }
        self.nc.attr(name).and_then(AttrValue::as_f64)
    }

    fn load_scans(&mut self) -> Result<Scans> {
        let times = self.read_all("scan_acquisition_time")?;
        let index = self.read_all("scan_index")?;
        let count = self.read_all("point_count")?;
        let tic = self.read_all("total_intensity").ok();
        if index.len() != times.len() || count.len() != times.len() {
            return Err(Error::corrupt(
                ANDI_ID,
                "scan_acquisition_time, scan_index and point_count differ in length",
            ));
        }
        Ok(Scans {
            times,
            index,
            count,
            tic,
        })
    }

    fn base_extra(&mut self) -> BTreeMap<String, Value> {
        let mut m = BTreeMap::new();
        m.insert("template".into(), json!(self.template.name()));
        for (andi, ours) in CHROM_ATTRS {
            if m.contains_key(*ours) {
                continue;
            }
            if let Some(s) = text_attr(&self.nc, andi) {
                m.insert((*ours).into(), json!(s));
            }
        }
        for (andi, ours) in [
            ("injection_date_time_stamp", "acquired_at"),
            ("experiment_date_time_stamp", "acquired_at"),
            ("dataset_date_time_stamp", "dataset_date"),
            ("netcdf_file_date_time_stamp", "file_date"),
        ] {
            if m.contains_key(ours) {
                continue;
            }
            if let Some(iso) = text_attr(&self.nc, andi).and_then(|s| andi_timestamp(&s)) {
                m.insert(ours.into(), json!(iso));
            }
        }
        for (andi, ours) in [
            ("sample_injection_volume", "injection_volume"),
            ("sample_amount", "sample_amount"),
        ] {
            if let Some(v) = self.nc.attr(andi).and_then(AttrValue::as_f64) {
                m.insert(ours.into(), json!(v));
            }
        }
        m
    }

    fn instrument(&mut self) -> Option<InstrumentInfo> {
        let mut get = |name: &str| -> Option<String> {
            let v = self.nc.var(name)?.clone();
            let nc = self.nc.clone();
            let path = self.path.clone();
            nc.read_strings(self.handle().ok()?, &path, &v)
                .ok()?
                .into_iter()
                .find(|s| !s.is_empty())
        };
        let i = InstrumentInfo {
            manufacturer: get("instrument_mfr"),
            model: get("instrument_model").or_else(|| get("instrument_name")),
            software: None,
            software_version: get("instrument_sw_version"),
            detector: None,
        };
        (i != InstrumentInfo::default()).then_some(i)
    }

    fn chrom_trace(&mut self) -> Option<TraceInfo> {
        let v = self.nc.var("ordinate_values")?.clone();
        let n = self.nc.element_count(&v);
        let mut extra = self.base_extra();
        let retention_unit = text_attr(&self.nc, "retention_unit");
        // times are seconds unless retention_unit says minutes
        let to_s = if retention_unit
            .as_deref()
            .is_some_and(|u| u.to_ascii_lowercase().starts_with("min"))
        {
            60.0
        } else {
            1.0
        };
        let interval = self.scalar("actual_sampling_interval").map(|x| x * to_s);
        let delay = self.scalar("actual_delay_time").map(|x| x * to_s);
        let run = self.scalar("actual_run_time_length").map(|x| x * to_s);
        if let Some(u) = &retention_unit {
            extra.insert("retention_unit".into(), json!(u));
        }
        if let Some(r) = run {
            extra.insert("run_time_s".into(), json!(tidy(r)));
        }
        let uniform = var_attr(&v, "uniform_sampling_flag")
            .and_then(AttrValue::as_text)
            .is_none_or(|s| !s.trim().eq_ignore_ascii_case("N"));
        if !uniform {
            extra.insert("uniform_sampling".into(), json!(false));
        }
        if let Some(p) = var_attr(&v, "autosampler_position").and_then(AttrValue::as_text)
            && !p.trim().is_empty()
        {
            extra.insert("autosampler_position".into(), json!(p.trim()));
        }
        let start = delay.unwrap_or(0.0);
        if let Some(step) = interval.filter(|s| *s > 0.0) {
            extra.insert("x_start_min".into(), json!(tidy(start / 60.0)));
            extra.insert(
                "x_end_min".into(),
                json!(tidy((start + step * n.saturating_sub(1) as f64) / 60.0)),
            );
            extra.insert(
                "axis".into(),
                json!({"quantity": "retention_time", "unit": "min", "first": tidy(start / 60.0), "step": tidy(step / 60.0)}),
            );
        }
        for (andi, ours) in [
            ("detector_maximum_value", "detector_maximum"),
            ("detector_minimum_value", "detector_minimum"),
        ] {
            if let Some(x) = self.scalar(andi) {
                extra.insert(ours.into(), json!(x));
            }
        }
        let (scale, offset) = var_scaling(&v);
        let name = text_attr(&self.nc, "detector_name").unwrap_or_else(|| "ordinate_values".into());
        Some(TraceInfo {
            index: 0,
            name: Some(name.clone()),
            sample_rate_hz: interval.filter(|s| *s > 0.0).map_or(0.0, |s| tidy(1.0 / s)),
            sample_count: n,
            sweep_count: 1,
            channels: vec![SignalChannelInfo {
                index: 0,
                name,
                unit: text_attr(&self.nc, "detector_unit"),
                dtype: v.nc_type.dtype().into(),
                scale,
                offset,
                extra: BTreeMap::new(),
            }],
            start_s: Some(tidy(start)),
            extra,
        })
    }

    fn tic_trace(&mut self, index: u32) -> Option<TraceInfo> {
        let s = self.scans.clone()?;
        let tic = s.tic.as_ref()?;
        let (first, step) = uniform_times(&s.times)?;
        let mut extra = self.base_extra();
        extra.insert("source".into(), json!("total_intensity"));
        extra.insert("x_start_min".into(), json!(tidy(first / 60.0)));
        extra.insert(
            "x_end_min".into(),
            json!(tidy(s.times[s.times.len() - 1] / 60.0)),
        );
        extra.insert(
            "axis".into(),
            json!({"quantity": "retention_time", "unit": "min", "first": tidy(first / 60.0), "step": tidy(step / 60.0)}),
        );
        extra.insert(
            "sampling".into(),
            json!("scan times evenly spaced within 1 % (actual times: spectra rt_s)"),
        );
        let unit = self
            .nc
            .var("total_intensity")
            .and_then(|v| var_attr(v, "units"))
            .and_then(AttrValue::as_text);
        Some(TraceInfo {
            index,
            name: Some("TIC".into()),
            sample_rate_hz: tidy(1.0 / step),
            sample_count: tic.len() as u64,
            sweep_count: 1,
            channels: vec![SignalChannelInfo {
                index: 0,
                name: "TIC".into(),
                unit,
                dtype: self
                    .nc
                    .var("total_intensity")
                    .map_or("float64", |v| v.nc_type.dtype())
                    .into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            }],
            start_s: Some(tidy(first)),
            extra,
        })
    }

    fn traces(&mut self) -> Vec<TraceInfo> {
        match self.template {
            AndiTemplate::Chromatography => self.chrom_trace().into_iter().collect(),
            AndiTemplate::MassSpectrometry => self.tic_trace(0).into_iter().collect(),
            AndiTemplate::Unknown => Vec::new(),
        }
    }

    /// Numeric variables along `peak_number` (the peak table), and string ones.
    fn peak_columns(&self) -> (Vec<Variable>, Vec<Variable>) {
        let Some(pd) = self.nc.dims.iter().position(|d| d.name == "peak_number") else {
            return (Vec::new(), Vec::new());
        };
        let mut num = Vec::new();
        let mut text = Vec::new();
        for v in &self.nc.variables {
            if v.dim_ids.first() != Some(&pd) {
                continue;
            }
            if v.nc_type == NcType::Char && v.dim_ids.len() == 2 {
                text.push(v.clone());
            } else if v.dim_ids.len() == 1 && v.nc_type != NcType::Char {
                num.push(v.clone());
            }
        }
        (num, text)
    }

    fn tables(&mut self) -> Vec<TableInfo> {
        let (num, text) = self.peak_columns();
        let Some(pd) = self.nc.dims.iter().find(|d| d.name == "peak_number") else {
            return Vec::new();
        };
        let rows = pd.len;
        if num.is_empty() || rows == 0 {
            return Vec::new();
        }
        let mut extra = BTreeMap::new();
        let nc = self.nc.clone();
        let path = self.path.clone();
        for v in &text {
            if let Ok(h) = self.handle()
                && let Ok(s) = nc.read_strings(h, &path, v)
                && s.iter().any(|x| !x.is_empty())
            {
                extra.insert(v.name.clone(), json!(s));
            }
        }
        if let Some(u) = text_attr(&self.nc, "peak_amount_unit") {
            extra.insert("peak_amount_unit".into(), json!(u));
        }
        vec![TableInfo {
            index: 0,
            name: Some("peaks".into()),
            row_count: rows,
            columns: num
                .iter()
                .enumerate()
                .map(|(i, v)| ColumnInfo {
                    index: i as u32,
                    name: v.name.clone(),
                    label: None,
                    dtype: v.nc_type.dtype().into(),
                    unit: None,
                    range: None,
                    extra: BTreeMap::new(),
                })
                .collect(),
            extra,
        }]
    }
}

impl Dataset for AndiDataset {
    fn info(&self) -> Result<FileInfo> {
        // `info` takes &self; reads go through a fresh dataset handle
        let mut me = AndiDataset {
            path: self.path.clone(),
            fs: self.fs.clone(),
            size: self.size,
            nc: self.nc.clone(),
            template: self.template,
            scans: self.scans.clone(),
            handle: None,
        };
        let traces = me.traces();
        let tables = me.tables();
        let mut spectra = Vec::new();
        let mut notes = Vec::new();
        if let Some(s) = me.scans.clone() {
            let mut extra = me.base_extra();
            let total: f64 = s.count.iter().sum();
            extra.insert("points".into(), json!(total as u64));
            if let Some(mf) = me
                .nc
                .var("mass_values")
                .and_then(|v| var_attr(v, "units"))
                .and_then(AttrValue::as_text)
            {
                extra.insert("mass_unit".into(), json!(mf));
            }
            spectra.push(SpectraInfo {
                index: 0,
                name: text_attr(&me.nc, "experiment_title"),
                scan_count: s.times.len() as u64,
                ms_levels: vec![1],
                rt_range_s: s.times.first().zip(s.times.last()).map(|(a, b)| [*a, *b]),
                instrument: me.instrument(),
                extra,
            });
            if traces.is_empty() {
                notes.push("scan times are not evenly spaced, so the total-ion chromatogram is not exposed as a trace; use spectra[].total_ion_current".into());
            }
        } else if me.template == AndiTemplate::MassSpectrometry {
            notes.push(
                "mass-spectrometry template, but the scan variables could not be read; run `check`"
                    .into(),
            );
        }
        if me.template == AndiTemplate::Unknown {
            notes.push("netCDF file without ANDI template attributes (aia_template_revision / ms_template_revision): variables are listed by `info --view structure` and `info --view full`, nothing is mapped".into());
        }
        let version = text_attr(&me.nc, "aia_template_revision")
            .or_else(|| text_attr(&me.nc, "ms_template_revision"));
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: AndiReader.descriptor(),
            format_version: version,
            images: Vec::new(),
            tables,
            spectra,
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut attrs = serde_json::Map::new();
        for a in &self.nc.attributes {
            attrs.insert(a.name.clone(), a.value.to_json());
        }
        let mut vars = serde_json::Map::new();
        let mut f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        for v in &self.nc.variables {
            let mut va = serde_json::Map::new();
            for a in &v.attributes {
                va.insert(a.name.clone(), a.value.to_json());
            }
            let dims: Vec<String> = v
                .dim_ids
                .iter()
                .map(|&d| self.nc.dims[d].name.clone())
                .collect();
            let mut j = json!({"dimensions": dims, "shape": self.nc.shape(v), "type": v.nc_type.dtype(), "attributes": va});
            let n = self.nc.element_count(v);
            if v.nc_type == NcType::Char && n <= 4096 {
                if let Ok(s) = self.nc.read_strings(&mut f, &self.path, v) {
                    j["values"] = json!(s);
                }
            } else if n <= 1
                && let Ok(x) = self.nc.read_f64(&mut f, &self.path, v, 0, 1)
            {
                j["values"] = json!(x);
            }
            vars.insert(v.name.clone(), j);
        }
        let dims: serde_json::Map<String, Value> = self
            .nc
            .dims
            .iter()
            .map(|d| {
                (
                    d.name.clone(),
                    json!(if d.unlimited {
                        json!({"unlimited": true, "len": d.len})
                    } else {
                        json!(d.len)
                    }),
                )
            })
            .collect();
        Ok(
            json!({"netcdf_version": self.nc.version, "dimensions": dims, "global_attributes": attrs, "variables": vars}),
        )
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Inferred),
            ("traces[].sample_rate_hz", Source::Inferred),
            ("traces[].start_s", Source::Inferred),
            ("traces[].channels[].unit", Source::Inferred),
            ("traces[].extra", Source::Inferred),
            ("tables[].columns", Source::Inferred),
            ("spectra[].scan_count", Source::Inferred),
            ("spectra[].rt_range_s", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: "netCDF header".into(),
            offset: Some(0),
            size: Some(self.nc.header_len),
            image: None,
            details: json!({"version": self.nc.version, "records": self.nc.numrecs, "record_size": self.nc.record_size}),
        }];
        for v in &self.nc.variables {
            let dims: Vec<String> = v
                .dim_ids
                .iter()
                .map(|&d| self.nc.dims[d].name.clone())
                .collect();
            out.push(LsEntry {
                kind: "variable".into(),
                name: v.name.clone(),
                offset: Some(v.begin),
                size: Some(self.nc.data_end(v).saturating_sub(v.begin)),
                image: None,
                details: json!({"type": v.nc_type.dtype(), "dimensions": dims, "shape": self.nc.shape(v), "record": v.is_record}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            ANDI_ID,
            "image planes",
            "ANDI files hold chromatograms and mass spectra: use `openreadout trace` or `export --format csv`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let traces = self.traces();
        let info = traces.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                traces.len()
            ))
        })?;
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (one sweep)"
            )));
        }
        if first_sample > info.sample_count {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({} samples)",
                info.sample_count
            )));
        }
        let n = max_samples.min(info.sample_count - first_sample);
        let values = if self.template == AndiTemplate::Chromatography {
            let var = self
                .nc
                .var("ordinate_values")
                .cloned()
                .ok_or_else(|| Error::corrupt(ANDI_ID, "ordinate_values missing"))?;
            let (scale, offset) = var_scaling(&var);
            let nc = self.nc.clone();
            let path = self.path.clone();
            nc.read_f64(self.handle()?, &path, &var, first_sample, n)?
                .into_iter()
                .map(|x| x * scale + offset)
                .collect()
        } else {
            let tic = self
                .scans
                .as_ref()
                .and_then(|scans| scans.tic.clone())
                .unwrap_or_default();
            tic[first_sample as usize..(first_sample + n) as usize].to_vec()
        };
        Ok(Trace {
            trace: index,
            sweep: 0,
            first_sample,
            channels: vec![values],
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 {
            return Err(Error::Usage(format!("table {index} out of range")));
        }
        let (num, _) = self.peak_columns();
        let rows = self
            .nc
            .dims
            .iter()
            .find(|d| d.name == "peak_number")
            .map_or(0, |d| d.len);
        if num.is_empty() {
            return Err(Error::Usage("this file has no peak table".into()));
        }
        if first_row > rows {
            return Err(Error::Usage(format!(
                "first row {first_row} past the end ({rows} rows)"
            )));
        }
        let n = max_rows.min(rows - first_row);
        let nc = self.nc.clone();
        let path = self.path.clone();
        let mut columns = Vec::new();
        for v in &num {
            columns.push(nc.read_f64(self.handle()?, &path, v, first_row, n)?);
        }
        Ok(Table {
            table: 0,
            first_row,
            columns,
        })
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        if run != 0 {
            return Err(Error::Usage(format!(
                "spectra run {run} out of range (one run)"
            )));
        }
        let Some(s) = self.scans.as_ref() else {
            return Err(Error::Usage(
                "this file holds no mass spectra (not the ANDI/MS template)".into(),
            ));
        };
        let (polarity, centroided) = self.ms_polarity_and_mode();
        for i in usize::try_from(first).unwrap_or(usize::MAX)..s.times.len() {
            let h = openreadout_core::ScanHeader {
                index: i as u64,
                scan_number: i as u64 + 1,
                ms_level: 1,
                rt_s: Some(s.times[i]),
                polarity: polarity.clone(),
                centroided,
                total_ion_current: s.tic.as_ref().and_then(|t| t.get(i)).copied(),
                point_count: s
                    .count
                    .get(i)
                    .filter(|c| **c >= 0.0 && c.is_finite())
                    .map(|c| *c as u64),
                ..openreadout_core::ScanHeader::default()
            };
            if !visit(h) {
                break;
            }
        }
        Ok(true)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "spectra run {index} out of range (one run)"
            )));
        }
        let s = self.scans.clone().ok_or_else(|| {
            Error::Usage("this file holds no mass spectra (not the ANDI/MS template)".into())
        })?;
        let i = spectrum as usize;
        let (Some(&start), Some(&count), Some(&rt)) =
            (s.index.get(i), s.count.get(i), s.times.get(i))
        else {
            return Err(Error::Usage(format!(
                "spectrum {spectrum} out of range ({} scans)",
                s.times.len()
            )));
        };
        if start < 0.0 || count < 0.0 {
            return Err(Error::corrupt(
                ANDI_ID,
                format!("scan {spectrum}: negative scan_index/point_count"),
            ));
        }
        let nc = self.nc.clone();
        let path = self.path.clone();
        let mv = nc
            .var("mass_values")
            .cloned()
            .ok_or_else(|| Error::corrupt(ANDI_ID, "mass_values missing"))?;
        let iv = nc
            .var("intensity_values")
            .cloned()
            .ok_or_else(|| Error::corrupt(ANDI_ID, "intensity_values missing"))?;
        let total = nc.element_count(&mv);
        if start as u64 + count as u64 > total {
            return Err(Error::corrupt(
                ANDI_ID,
                format!(
                    "scan {spectrum}: points {start}..{} past the {total} stored",
                    start + count
                ),
            ));
        }
        let (ms, mo) = var_scaling(&mv);
        let (is, io) = var_scaling(&iv);
        let h = self.handle()?;
        let mz: Vec<f64> = nc
            .read_f64(h, &path, &mv, start as u64, count as u64)?
            .into_iter()
            .map(|x| x * ms + mo)
            .collect();
        let intensity: Vec<f32> = nc
            .read_f64(h, &path, &iv, start as u64, count as u64)?
            .into_iter()
            .map(|x| (x * is + io) as f32)
            .collect();
        let (polarity, centroided) = self.ms_polarity_and_mode();
        Ok(Spectrum {
            index: spectrum,
            scan_number: spectrum + 1,
            ms_level: 1,
            rt_s: Some(rt),
            polarity,
            centroided,
            precursor_mz: None,
            precursor_charge: None,
            scan_filter: None,
            total_ion_current: s.tic.as_ref().and_then(|t| t.get(i)).copied(),
            mz,
            intensity,
            ..Spectrum::default()
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), ANDI_ID);
        r.performed("netCDF classic header: dimensions, attributes, variables, types");
        r.performed("every variable's data lies inside the file");
        r.performed("ANDI template recognised; required variables present");
        r.performed(
            "chromatography: point count against actual_run_time_length / actual_sampling_interval",
        );
        r.performed("mass spectrometry: scan_index + point_count inside the stored points, monotonic scan times");
        for v in &self.nc.variables {
            let end = self.nc.data_end(v);
            if end > self.size {
                r.push(
                    Finding::error(
                        "truncated",
                        format!(
                            "variable {} needs bytes up to {end}, the file has {}",
                            v.name, self.size
                        ),
                    )
                    .at(v.begin),
                );
            }
        }
        match self.template {
            AndiTemplate::Unknown => r.push(Finding::warning(
                "not_andi",
                "no aia_template_revision / ms_template_revision attribute and no ordinate_values / mass_values variable",
            )),
            AndiTemplate::Chromatography => {
                if self.nc.var("ordinate_values").is_none() {
                    r.push(Finding::error("missing_variable", "ordinate_values is missing"));
                } else {
                    let n = self.nc.var("ordinate_values").map_or(0, |v| self.nc.element_count(v));
                    if let (Some(run), Some(dt)) = (self.scalar("actual_run_time_length"), self.scalar("actual_sampling_interval"))
                        && dt > 0.0
                    {
                        let expected = run / dt;
                        if (expected - n as f64).abs() > 1.5 {
                            r.push(Finding::info(
                                "run_length_mismatch",
                                format!("{n} points, but actual_run_time_length / actual_sampling_interval = {expected:.1}"),
                            ));
                        }
                    } else {
                        r.push(Finding::warning("missing_variable", "actual_sampling_interval is missing: the time axis is unknown"));
                    }
                }
            }
            AndiTemplate::MassSpectrometry => {
                for name in ["scan_acquisition_time", "scan_index", "point_count", "mass_values", "intensity_values"] {
                    if self.nc.var(name).is_none() {
                        r.push(Finding::error("missing_variable", format!("{name} is missing")));
                    }
                }
                if r.ok {
                    match self.load_scans() {
                        Err(e) => r.push(Finding::error("bad_scan_index", e.to_string())),
                        Ok(s) => {
                            let total = self.nc.var("mass_values").map_or(0, |v| self.nc.element_count(v)) as f64;
                            let mut bad = 0;
                            let mut expect = 0.0;
                            let mut gaps = 0;
                            for (i, (&a, &c)) in s.index.iter().zip(&s.count).enumerate() {
                                if a < 0.0 || c < 0.0 || a + c > total {
                                    bad += 1;
                                    if bad <= 3 {
                                        r.push(Finding::error("bad_scan_index", format!("scan {i}: points {a}..{} outside the {total} stored", a + c)));
                                    }
                                }
                                if (a - expect).abs() > 0.5 {
                                    gaps += 1;
                                }
                                expect = a + c;
                            }
                            if gaps > 0 {
                                r.push(Finding::info("scan_index_gaps", format!("{gaps} scans do not start where the previous one ended")));
                            }
                            let sum: f64 = s.count.iter().sum();
                            if (sum - total).abs() > 0.5 {
                                r.push(Finding::warning("point_count_mismatch", format!("point_count sums to {sum}, {total} points stored")));
                            }
                            if s.times.windows(2).any(|w| w[1] < w[0]) {
                                r.push(Finding::warning("time_not_monotonic", "scan_acquisition_time decreases somewhere"));
                            }
                        }
                    }
                }
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(
            andi_timestamp("20131119132742+0900").as_deref(),
            Some("2013-11-19T13:27:42+09:00")
        );
        assert_eq!(andi_timestamp("00000000000000+0000"), None);
        assert_eq!(
            andi_timestamp("20130507123000 + 0000").as_deref(),
            Some("2013-05-07T12:30:00+00:00")
        );
        assert_eq!(andi_timestamp("2013"), None);
        assert!(uniform_times(&[0.0, 1.0, 2.001, 3.0]).is_some());
        assert!(uniform_times(&[0.0, 1.0, 2.5, 3.0]).is_none());
    }
}
