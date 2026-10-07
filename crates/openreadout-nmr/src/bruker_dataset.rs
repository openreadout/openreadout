//! `Dataset` for a Bruker experiment directory: one trace for `fid`/`ser`, one per processed
//! `pdata/<procno>` spectrum.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::bruker_layout::{
    DSP_GROUP_DELAY, Experiment, ProcLayout, RawLayout, decode_samples, find_experiments_in,
    resolve_experiment_dir_in,
};
use crate::bruker_params::ParamFile;
use crate::{BRUKER_FORMAT_ID, BrukerReader};

/// Most samples decoded by one `read_trace` call's I/O buffer (values, not bytes).
const MAX_READ_VALUES: u64 = 1 << 28;

/// A layout problem remembered at open time and reported when data is requested.
#[derive(Debug, Clone)]
enum Fault {
    Corrupt(String),
    Unsupported(String, String),
}

impl Fault {
    fn from_error(e: &Error) -> Self {
        match e {
            Error::Unsupported { feature, hint, .. } => {
                Fault::Unsupported(feature.clone(), hint.clone().unwrap_or_default())
            }
            other => Fault::Corrupt(other.to_string()),
        }
    }
    fn to_error(&self) -> Error {
        match self {
            Fault::Corrupt(m) => Error::corrupt(BRUKER_FORMAT_ID, m.clone()),
            Fault::Unsupported(f, h) => Error::unsupported(BRUKER_FORMAT_ID, f.clone(), h.clone()),
        }
    }
    fn message(&self) -> String {
        match self {
            Fault::Corrupt(m) => m.clone(),
            Fault::Unsupported(f, _) => format!("unsupported: {f}"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum TraceSource {
    Raw,
    Proc(usize),
}

/// An opened Bruker experiment directory.
#[derive(Debug)]
pub struct BrukerDataset {
    path: PathBuf,
    exp: Experiment,
    raw: Option<std::result::Result<RawLayout, Fault>>,
    procs: Vec<(u32, std::result::Result<ProcLayout, Fault>)>,
    sources: Vec<TraceSource>,
}

/// Digital-filter group delay in points and where it came from: `GRPDLY` when positive, else
/// the `DSPFVS`/`DECIM` table (firmware 10–13), else `None`.
pub fn group_delay(acqus: &ParamFile) -> (Option<f64>, &'static str) {
    if let Some(g) = acqus.float("GRPDLY")
        && g > 0.0
    {
        return (Some(g), "GRPDLY");
    }
    let dspfvs = acqus.int("DSPFVS");
    let decim = acqus.float("DECIM");
    if let (Some(v), Some(d)) = (dspfvs, decim)
        && d.fract() == 0.0
        && let Some((_, table)) = DSP_GROUP_DELAY.iter().find(|(f, _)| *f == v)
        && let Some((_, g)) = table.iter().find(|(k, _)| *k == d as i64)
    {
        return (Some(*g), "DSPFVS/DECIM table");
    }
    match dspfvs {
        Some(v) if v >= 14 => (None, "not recorded (GRPDLY absent or not positive)"),
        _ => (None, "not determined (DSPFVS/DECIM not in the table)"),
    }
}

fn put(m: &mut BTreeMap<String, Value>, k: &str, v: Option<Value>) {
    if let Some(v) = v {
        m.insert(k.into(), v);
    }
}

fn text(p: &ParamFile, k: &str) -> Option<Value> {
    p.text(k).map(|s| json!(s))
}

fn num(p: &ParamFile, k: &str) -> Option<Value> {
    p.get(k).and_then(|v| {
        if let Some(i) = v.as_i64()
            && matches!(v, crate::ParamValue::Int(_))
        {
            Some(json!(i))
        } else {
            v.as_f64().filter(|f| f.is_finite()).map(|f| json!(f))
        }
    })
}

/// `FnMODE` of an indirect dimension, as nmrglue names it.
fn fn_mode_name(code: Option<i64>) -> Option<&'static str> {
    Some(match code? {
        0 => "undefined",
        1 => "QF",
        2 => "QSEQ",
        3 => "TPPI",
        4 => "States",
        5 => "States-TPPI",
        6 => "Echo-Antiecho",
        _ => return None,
    })
}

fn pow2(e: i64) -> f64 {
    let e = e.clamp(-1000, 1000) as i32;
    2f64.powi(e)
}

fn software(exp: &Experiment) -> Option<(String, Option<String>)> {
    exp.acqus.software()
}

impl BrukerDataset {
    /// Open an experiment directory (or a path inside one: `fid`, `ser`, `acqus`, `pdata/<n>`, `1r`, …).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, or an experiment directory held in memory).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let Some(dir) = resolve_experiment_dir_in(fs, path) else {
            if fs.is_dir(path) {
                let exps = find_experiments_in(fs, path);
                if !exps.is_empty() {
                    let names: Vec<String> = exps
                        .iter()
                        .take(12)
                        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                        .collect();
                    return Err(Error::Usage(format!(
                        "{} is a Bruker data set holding {} experiments ({}{}); pass one experiment directory, e.g. {}",
                        path.display(),
                        exps.len(),
                        names.join(", "),
                        if exps.len() > 12 { ", …" } else { "" },
                        exps[0].display()
                    )));
                }
            }
            return Err(Error::UnknownFormat {
                path: path.to_path_buf(),
            });
        };
        let exp = Experiment::load_in(fs, &dir)?;
        let raw = exp
            .raw_file
            .as_ref()
            .map(|_| RawLayout::from_experiment(&exp).map_err(|e| Fault::from_error(&e)));
        let procs: Vec<(u32, std::result::Result<ProcLayout, Fault>)> = exp
            .processing
            .iter()
            .filter_map(|p| {
                ProcLayout::from_processing(p)
                    .map(|r| (p.procno, r.map_err(|e| Fault::from_error(&e))))
            })
            .collect();
        let mut sources = Vec::new();
        if raw.is_some() {
            sources.push(TraceSource::Raw);
        }
        for i in 0..procs.len() {
            sources.push(TraceSource::Proc(i));
        }
        Ok(BrukerDataset {
            path: path.to_path_buf(),
            exp,
            raw,
            procs,
            sources,
        })
    }

    /// The parsed experiment (for library users).
    pub fn experiment(&self) -> &Experiment {
        &self.exp
    }

    fn raw_trace_info(&self, index: u32, layout: Option<&RawLayout>) -> TraceInfo {
        let a = &self.exp.acqus;
        let sw_h = a.float("SW_h").filter(|v| v.is_finite() && *v > 0.0);
        let complex = matches!(a.int("AQ_mod"), Some(1 | 3));
        let rate = sw_h.map_or(0.0, |s| if complex { s } else { 2.0 * s });
        let nc = a.int("NC").unwrap_or(0);
        let scale = pow2(nc);
        let dtype = layout.map_or("int32", |l| l.sample_type.dtype());
        let mut channels = vec![SignalChannelInfo {
            index: 0,
            name: "real".into(),
            unit: None,
            dtype: dtype.into(),
            scale,
            offset: 0.0,
            extra: BTreeMap::new(),
        }];
        if complex {
            channels.push(SignalChannelInfo {
                index: 1,
                name: "imag".into(),
                unit: None,
                dtype: dtype.into(),
                scale,
                offset: 0.0,
                extra: BTreeMap::new(),
            });
        }
        let samples = layout.map_or_else(
            || {
                a.int("TD")
                    .and_then(|t| u64::try_from(t).ok())
                    .map_or(0, |t| if complex { t / 2 } else { t })
            },
            RawLayout::samples_per_row,
        );
        let mut e = BTreeMap::new();
        e.insert("kind".into(), json!("time_domain"));
        e.insert(
            "file".into(),
            json!(self.exp.raw_file.as_ref().map(|(n, _)| n)),
        );
        if rate > 0.0 {
            e.insert(
                "axis".into(),
                json!({"quantity": "time", "unit": "s", "first": 0.0, "step": 1.0 / rate, "size": samples}),
            );
        }
        put(&mut e, "nucleus", text(a, "NUC1"));
        put(&mut e, "spectrometer_frequency_mhz", num(a, "SFO1"));
        put(&mut e, "base_frequency_mhz", num(a, "BF1"));
        put(&mut e, "carrier_offset_hz", num(a, "O1"));
        put(&mut e, "spectral_width_hz", num(a, "SW_h"));
        put(&mut e, "spectral_width_ppm", num(a, "SW"));
        put(&mut e, "time_domain_size", num(a, "TD"));
        put(&mut e, "scans", num(a, "NS"));
        put(&mut e, "dummy_scans", num(a, "DS"));
        put(&mut e, "receiver_gain", num(a, "RG"));
        put(&mut e, "pulse_program", text(a, "PULPROG"));
        put(&mut e, "experiment", text(a, "EXP"));
        put(&mut e, "solvent", text(a, "SOLVENT"));
        put(&mut e, "temperature_k", num(a, "TE"));
        put(
            &mut e,
            "acquired_at",
            a.int("DATE")
                .filter(|&d| d > 0)
                .map(|d| json!(openreadout_core::time::unix_to_iso8601(d, 0))),
        );
        put(&mut e, "instrument", text(a, "INSTRUM"));
        put(&mut e, "probe", text(a, "PROBHD"));
        if let Some((name, version)) = software(&self.exp) {
            e.insert("software".into(), json!(name));
            put(&mut e, "software_version", version.map(|v| json!(v)));
        }
        e.insert(
            "quadrature".into(),
            json!(if complex { "complex" } else { "real" }),
        );
        put(&mut e, "acquisition_mode_code", num(a, "AQ_mod"));
        e.insert("normalization_exponent".into(), json!(nc));
        if let Some(l) = layout {
            e.insert("sample_type".into(), json!(l.sample_type.dtype()));
            e.insert("byte_order".into(), json!(l.byte_order.name()));
            if l.file_name == "ser" {
                e.insert("row_stride_bytes".into(), json!(l.row_stride));
            }
        }
        let (gd, gd_src) = group_delay(a);
        put(&mut e, "group_delay_points", gd.map(|g| json!(g)));
        e.insert("group_delay_source".into(), json!(gd_src));
        put(&mut e, "decimation", num(a, "DECIM"));
        put(&mut e, "dsp_firmware", num(a, "DSPFVS"));
        if !self.exp.acqu_n.is_empty() {
            let dims: Vec<Value> = self
                .exp
                .acqu_n
                .iter()
                .enumerate()
                .map(|(k, p)| {
                    let mut d = BTreeMap::new();
                    d.insert("dimension".to_string(), json!(k + 2));
                    d.insert("parameter_file".to_string(), json!(p.name));
                    put(&mut d, "nucleus", text(p, "NUC1"));
                    put(&mut d, "time_domain_size", num(p, "TD"));
                    put(&mut d, "spectral_width_hz", num(p, "SW_h"));
                    put(&mut d, "spectral_width_ppm", num(p, "SW"));
                    put(&mut d, "spectrometer_frequency_mhz", num(p, "SFO1"));
                    put(
                        &mut d,
                        "encoding",
                        fn_mode_name(p.int("FnMODE")).map(|s| json!(s)),
                    );
                    json!(d)
                })
                .collect();
            e.insert("indirect_dimensions".into(), json!(dims));
        }
        if a.int("FnTYPE") == Some(2) {
            let mut nus = BTreeMap::new();
            put(
                &mut nus,
                "nuslist_entries",
                self.exp.nuslist_len.map(|n| json!(n)),
            );
            put(&mut nus, "amount_percent", num(a, "NusAMOUNT"));
            put(
                &mut nus,
                "full_time_domain_size",
                self.exp.acqu_n.first().and_then(|p| num(p, "NusTD")),
            );
            e.insert("non_uniform_sampling".into(), json!(nus));
        }
        TraceInfo {
            index,
            name: self.exp.raw_file.as_ref().map(|(n, _)| n.clone()),
            sample_rate_hz: rate,
            sample_count: samples,
            sweep_count: layout.map_or(1, |l| u32::try_from(l.rows).unwrap_or(u32::MAX)),
            channels,
            start_s: Some(0.0),
            extra: e,
        }
    }

    fn proc_trace_info(&self, index: u32, procno: u32, l: Option<&ProcLayout>) -> TraceInfo {
        let p = self
            .exp
            .processing
            .iter()
            .find(|p| p.procno == procno)
            .expect("procno from the same experiment");
        let procs = p.procs.first();
        let nc = procs.and_then(|pf| pf.int("NC_proc")).unwrap_or(0);
        let scale = pow2(nc);
        let mut e = BTreeMap::new();
        e.insert("kind".into(), json!("processed_spectrum"));
        e.insert("procno".into(), json!(procno));
        let axis = |pf: &ParamFile, size: u64| -> Option<Value> {
            let off = pf.float("OFFSET")?;
            let sw = pf.float("SW_p")?;
            let sf = pf.float("SF").filter(|s| *s != 0.0)?;
            if size == 0 {
                return None;
            }
            let step = -sw / (sf * size as f64);
            Some(json!({
                "quantity": "chemical_shift", "unit": "ppm", "first": off, "step": step,
                "last": off + step * (size as f64 - 1.0), "size": size,
                "spectral_width_hz": sw, "spectrometer_frequency_mhz": sf,
            }))
        };
        let mut channels = Vec::new();
        let (mut samples, mut sweeps) = (0u64, 1u32);
        if let Some(l) = l {
            samples = l.si[0];
            if l.dims >= 2 {
                sweeps = u32::try_from(l.si[1..].iter().product::<u64>()).unwrap_or(u32::MAX);
            }
            for (i, (name, file, _)) in l.components.iter().enumerate() {
                let mut ce = BTreeMap::new();
                ce.insert("file".to_string(), json!(file));
                channels.push(SignalChannelInfo {
                    index: i as u32,
                    name: name.clone(),
                    unit: None,
                    dtype: l.sample_type.dtype().into(),
                    scale,
                    offset: 0.0,
                    extra: ce,
                });
            }
            e.insert("sample_type".into(), json!(l.sample_type.dtype()));
            e.insert("byte_order".into(), json!(l.byte_order.name()));
            e.insert(
                "files".into(),
                json!(l.components.iter().map(|c| &c.1).collect::<Vec<_>>()),
            );
            if l.dims >= 2 {
                e.insert(
                    "submatrix".into(),
                    json!(l.xdim.iter().rev().collect::<Vec<_>>()),
                );
                e.insert("shape".into(), json!(l.si.iter().rev().collect::<Vec<_>>()));
                e.insert(
                    "indirect_axes".into(),
                    json!(
                        p.procs
                            .iter()
                            .zip(&l.si)
                            .skip(1)
                            .map(|(pf, &size)| axis(pf, size))
                            .collect::<Vec<_>>()
                    ),
                );
            }
        }
        if let Some(pf) = procs {
            put(&mut e, "axis", axis(pf, samples));
            put(&mut e, "nucleus", text(pf, "AXNUC"));
            put(&mut e, "spectral_width_hz", num(pf, "SW_p"));
            put(&mut e, "spectrometer_frequency_mhz", num(pf, "SF"));
            put(&mut e, "transform_size", num(pf, "FTSIZE"));
            put(&mut e, "phase0_deg", num(pf, "PHC0"));
            put(&mut e, "phase1_deg", num(pf, "PHC1"));
            put(&mut e, "line_broadening_hz", num(pf, "LB"));
            put(&mut e, "window_function_code", num(pf, "WDW"));
        }
        e.insert("normalization_exponent".into(), json!(nc));
        if let Some(pf2) = p.procs.get(1)
            && l.is_some_and(|l| l.dims == 2)
        {
            let n1 = u64::from(sweeps);
            let mut f1 = BTreeMap::new();
            put(&mut f1, "axis", axis(pf2, n1));
            put(&mut f1, "nucleus", text(pf2, "AXNUC"));
            put(&mut f1, "spectral_width_hz", num(pf2, "SW_p"));
            put(&mut f1, "spectrometer_frequency_mhz", num(pf2, "SF"));
            e.insert("sweep_axis".into(), json!(f1));
        }
        TraceInfo {
            index,
            name: Some(format!("pdata/{procno}")),
            sample_rate_hz: 0.0,
            sample_count: samples,
            sweep_count: sweeps,
            channels,
            start_s: None,
            extra: e,
        }
    }

    /// The `nuslist` as a table: one row per sampled increment, one column per indirect
    /// dimension (`index_1` for F1, …), in file order.
    fn nuslist_table(&self) -> Option<TableInfo> {
        let rows = self.exp.nuslist.as_ref()?;
        let width = rows.first().map_or(0, Vec::len);
        Some(TableInfo {
            index: 0,
            name: Some("nuslist".into()),
            row_count: rows.len() as u64,
            columns: (0..width)
                .map(|k| ColumnInfo {
                    index: k as u32,
                    name: format!("index_{}", k + 1),
                    dtype: "uint32".into(),
                    ..ColumnInfo::default()
                })
                .collect(),
            extra: BTreeMap::from([
                ("kind".to_string(), json!("sampling_schedule")),
                (
                    "note".to_string(),
                    json!(
                        "increments acquired, in acquisition order; the ser rows are these increments (NUS reconstruction is not performed)"
                    ),
                ),
            ]),
        })
    }

    fn trace_infos(&self) -> Vec<TraceInfo> {
        self.sources
            .iter()
            .enumerate()
            .map(|(i, s)| match s {
                TraceSource::Raw => {
                    self.raw_trace_info(i as u32, self.raw.as_ref().and_then(|r| r.as_ref().ok()))
                }
                TraceSource::Proc(k) => {
                    let (procno, l) = &self.procs[*k];
                    self.proc_trace_info(i as u32, *procno, l.as_ref().ok())
                }
            })
            .collect()
    }

    fn read_bytes(fs: &Fs, path: &Path, offset: u64, len: u64) -> Result<Vec<u8>> {
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let flen = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::corrupt(BRUKER_FORMAT_ID, "read range overflows"))?;
        if end > flen {
            return Err(Error::corrupt_at(
                BRUKER_FORMAT_ID,
                offset,
                format!(
                    "{} needs bytes up to {end} but has {flen} (truncated)",
                    path.file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().to_string())
                ),
            ));
        }
        f.seek(SeekFrom::Start(offset))
            .map_err(|e| Error::io(path, e))?;
        let mut buf = vec![
            0u8;
            usize::try_from(len)
                .map_err(|_| Error::corrupt(BRUKER_FORMAT_ID, "read too large"))?
        ];
        f.read_exact(&mut buf).map_err(|e| Error::io(path, e))?;
        Ok(buf)
    }

    fn read_raw(
        &self,
        index: u32,
        l: &RawLayout,
        sweep: u32,
        first: u64,
        max: u64,
    ) -> Result<Trace> {
        let sweep64 = u64::from(sweep);
        if sweep64 >= l.rows {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({} rows)",
                l.rows
            )));
        }
        let n = l.samples_per_row();
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let count = max.min(n - first).min(MAX_READ_VALUES);
        let per = if l.complex { 2 } else { 1 };
        let w = l.sample_type.width();
        let offset = sweep64 * l.row_stride + first * per * w;
        let bytes = Self::read_bytes(
            &self.exp.fs,
            &self.exp.dir.join(&l.file_name),
            offset,
            count * per * w,
        )?;
        let vals = decode_samples(&bytes, l.sample_type, l.byte_order);
        let scale = pow2(self.exp.acqus.int("NC").unwrap_or(0));
        let mut channels = vec![Vec::with_capacity(count as usize); per as usize];
        for (i, v) in vals.into_iter().enumerate() {
            channels[i % per as usize].push(v * scale);
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn read_proc(
        &self,
        index: u32,
        l: &ProcLayout,
        sweep: u32,
        first: u64,
        max: u64,
    ) -> Result<Trace> {
        let rows = l.si[1..].iter().product::<u64>();
        if u64::from(sweep) >= rows {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({rows} rows)"
            )));
        }
        let n = l.si[0];
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let count = max.min(n - first).min(MAX_READ_VALUES);
        let p = self
            .exp
            .processing
            .iter()
            .find(|p| p.procno == l.procno)
            .expect("procno from the same experiment");
        let nc = p
            .procs
            .first()
            .and_then(|pf| pf.int("NC_proc"))
            .unwrap_or(0);
        let scale = pow2(nc);
        let w = l.sample_type.width();
        let mut channels = Vec::new();
        for (_, file, _) in &l.components {
            let path = p.dir.join(file);
            let mut col = Vec::with_capacity(count as usize);
            for (off, k) in l.row_ranges(u64::from(sweep), first, count) {
                let bytes = Self::read_bytes(&self.exp.fs, &path, off, k * w)?;
                col.extend(
                    decode_samples(&bytes, l.sample_type, l.byte_order)
                        .into_iter()
                        .map(|v| v * scale),
                );
            }
            channels.push(col);
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }
}

fn rel(exp: &Experiment, p: &Path) -> String {
    p.strip_prefix(&exp.dir)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

impl Dataset for BrukerDataset {
    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        crate::bruker_experiment::facts(&self.exp)
    }

    fn info(&self) -> Result<FileInfo> {
        let mut notes = Vec::new();
        if let Some(Ok(l)) = &self.raw {
            notes.extend(l.notes.iter().cloned());
            if l.rows_in_file < l.rows {
                notes.push(format!(
                    "{} holds {} of {} rows (truncated or a stopped acquisition); run `check`",
                    l.file_name, l.rows_in_file, l.rows
                ));
            }
        }
        if let Some(Err(f)) = &self.raw {
            notes.push(format!("time-domain data not readable: {}", f.message()));
        }
        for (procno, r) in &self.procs {
            if let Err(f) = r {
                notes.push(format!("pdata/{procno} not readable: {}", f.message()));
            }
        }
        if self.raw.is_none() {
            notes.push("no fid or ser file: only processed data is available".into());
        }
        notes.push("time-domain values are scaled by 2^NC and processed values by 2^NC_proc (each channel's `scale`); the digital-filter group delay is reported in extra.group_delay_points, not removed".into());
        let issues = std::iter::once(&self.exp.acqus)
            .chain(self.exp.acqu_n.iter())
            .chain(self.exp.processing.iter().flat_map(|p| p.procs.iter()))
            .filter(|p| !p.issues.is_empty() || !p.ended)
            .count();
        if issues > 0 {
            notes.push(format!(
                "{issues} parameter file(s) have syntax problems; run `check`"
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.exp.total_size,
            format: BrukerReader.descriptor(),
            format_version: software(&self.exp).map(|(n, v)| match v {
                Some(v) => format!("{n} {v}"),
                None => n,
            }),
            images: Vec::new(),
            tables: self.nuslist_table().into_iter().collect(),
            spectra: Vec::new(),
            traces: self.trace_infos(),
            plane_count: 0,
            notes,
        })
    }

    fn read_table(&mut self, index: u32, first: u64, max: u64) -> Result<Table> {
        let rows = self
            .exp
            .nuslist
            .as_ref()
            .filter(|_| index == 0)
            .ok_or_else(|| Error::Usage(format!("table {index} does not exist")))?;
        let n = rows.len() as u64;
        if first > n {
            return Err(Error::Usage(format!(
                "first row {first} is past the end ({n} rows)"
            )));
        }
        let (lo, hi) = (first as usize, first.saturating_add(max).min(n) as usize);
        let width = rows.first().map_or(0, Vec::len);
        let columns = (0..width)
            .map(|k| rows[lo..hi].iter().map(|r| f64::from(r[k])).collect())
            .collect();
        Ok(Table {
            table: index,
            first_row: first,
            columns,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut acq = serde_json::Map::new();
        acq.insert(self.exp.acqus.name.clone(), self.exp.acqus.to_json());
        for p in &self.exp.acqu_n {
            acq.insert(p.name.clone(), p.to_json());
        }
        let mut pdata = serde_json::Map::new();
        for p in &self.exp.processing {
            let mut m = serde_json::Map::new();
            for pf in &p.procs {
                m.insert(pf.name.clone(), pf.to_json());
            }
            pdata.insert(p.procno.to_string(), Value::Object(m));
        }
        Ok(json!({
            "directory": self.exp.dir.display().to_string(),
            "acquisition": acq,
            "pdata": pdata,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::PriorArt),
            ("traces[kind=time_domain].sample_count", Source::PriorArt),
            ("traces[kind=time_domain].sweep_count", Source::PriorArt),
            ("traces[kind=time_domain].sample_rate_hz", Source::PriorArt),
            (
                "traces[kind=time_domain].channels[].dtype",
                Source::PriorArt,
            ),
            (
                "traces[kind=time_domain].channels[].scale",
                Source::Inferred,
            ),
            ("traces[kind=time_domain].extra.axis", Source::PriorArt),
            (
                "traces[kind=time_domain].extra.acquired_at",
                Source::Inferred,
            ),
            (
                "traces[kind=time_domain].extra.group_delay_points",
                Source::PriorArt,
            ),
            (
                "traces[kind=time_domain].extra.row_stride_bytes",
                Source::PriorArt,
            ),
            (
                "traces[kind=time_domain].extra.indirect_dimensions[].encoding",
                Source::PriorArt,
            ),
            (
                "traces[kind=time_domain].extra.non_uniform_sampling",
                Source::PriorArt,
            ),
            ("traces[kind=time_domain].extra.nucleus", Source::PriorArt),
            (
                "traces[kind=time_domain].extra.spectrometer_frequency_mhz",
                Source::PriorArt,
            ),
            (
                "traces[kind=time_domain].extra.spectral_width_hz",
                Source::PriorArt,
            ),
            (
                "traces[kind=time_domain].extra.temperature_k",
                Source::Inferred,
            ),
            ("traces[kind=time_domain].extra.scans", Source::Inferred),
            (
                "traces[kind=time_domain].extra.pulse_program",
                Source::Inferred,
            ),
            ("traces[kind=time_domain].extra.solvent", Source::Inferred),
            (
                "traces[kind=time_domain].extra.instrument",
                Source::Inferred,
            ),
            ("traces[kind=time_domain].extra.probe", Source::Inferred),
            (
                "traces[kind=processed_spectrum].sample_count",
                Source::PriorArt,
            ),
            (
                "traces[kind=processed_spectrum].sweep_count",
                Source::PriorArt,
            ),
            (
                "traces[kind=processed_spectrum].channels[].scale",
                Source::PriorArt,
            ),
            (
                "traces[kind=processed_spectrum].extra.axis",
                Source::PriorArt,
            ),
            (
                "traces[kind=processed_spectrum].extra.submatrix",
                Source::PriorArt,
            ),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        let raw_name = self.exp.raw_file.as_ref().map(|(n, _)| n.as_str());
        for (name, size) in &self.exp.files {
            let leaf = name.rsplit('/').next().unwrap_or(name);
            let in_pdata = name.starts_with("pdata/");
            let kind = if Some(name.as_str()) == raw_name {
                "time-domain"
            } else if in_pdata && crate::bruker_layout::ProcLayout::is_data_name(leaf) {
                "processed-data"
            } else if leaf.starts_with("acqu") || leaf.starts_with("proc") {
                "parameters"
            } else {
                "file"
            };
            let mut details = Value::Null;
            if kind == "time-domain"
                && let Some(Ok(l)) = &self.raw
            {
                details = json!({
                    "sample_type": l.sample_type.dtype(), "byte_order": l.byte_order.name(),
                    "complex": l.complex, "td": l.td, "rows": l.rows, "rows_in_file": l.rows_in_file,
                    "row_stride_bytes": l.row_stride,
                });
            }
            if kind == "parameters" {
                let pf = std::iter::once(&self.exp.acqus)
                    .chain(self.exp.acqu_n.iter())
                    .find(|p| p.name == name.as_str())
                    .or_else(|| {
                        self.exp
                            .processing
                            .iter()
                            .flat_map(|p| p.procs.iter().map(move |f| (p, f)))
                            .find(|(p, f)| rel(&self.exp, &p.dir.join(&f.name)) == *name)
                            .map(|(_, f)| f)
                    });
                if let Some(pf) = pf {
                    details = json!({"parameters": pf.params.len(), "title": pf.title, "complete": pf.ended});
                }
            }
            out.push(LsEntry {
                kind: kind.into(),
                name: name.clone(),
                offset: None,
                size: Some(*size),
                image: None,
                details,
            });
        }
        if self.exp.files_truncated {
            out.push(LsEntry {
                kind: "note".into(),
                name: "listing truncated".into(),
                offset: None,
                size: None,
                image: None,
                details: json!({"listed": self.exp.files.len()}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            BRUKER_FORMAT_ID,
            "image planes",
            "Bruker NMR data are traces (FIDs and spectra), not images: use `openreadout trace`, `openreadout export --format csv`, or the openreadout_trace MCP tool.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let src = *self.sources.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace index {index} out of range (0..{})",
                self.sources.len()
            ))
        })?;
        match src {
            TraceSource::Raw => match self.raw.as_ref().expect("raw source has a layout") {
                Ok(l) => self.read_raw(index, l, sweep, first_sample, max_samples),
                Err(f) => Err(f.to_error()),
            },
            TraceSource::Proc(k) => match &self.procs[k].1 {
                Ok(l) => self.read_proc(index, l, sweep, first_sample, max_samples),
                Err(f) => Err(f.to_error()),
            },
        }
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), BRUKER_FORMAT_ID);
        r.performed("parameter files (acqus, acquNs, procs, procNs) parse: records, (0..n) arrays, <strings>, ##END=");
        r.performed("required acquisition parameters present (TD, and DTYPA/BYTORDA/AQ_mod when given are known values)");
        r.performed("fid/ser length against TD (ser rows on 1024-byte boundaries) and the declared number of rows");
        r.performed("processed files (1D/2D/3D components) length against SI and XDIM tiling");
        r.performed("digital-filter group delay determinable from GRPDLY or DSPFVS/DECIM");
        let files: Vec<&ParamFile> = std::iter::once(&self.exp.acqus)
            .chain(self.exp.acqu_n.iter())
            .collect();
        for pf in &files {
            for m in &pf.issues {
                r.push(Finding::warning(
                    "parameter_syntax",
                    format!("{}: {m}", pf.name),
                ));
            }
            if !pf.ended {
                r.push(Finding::warning(
                    "parameter_unterminated",
                    format!("{} has no ##END= (truncated or hand-edited)", pf.name),
                ));
            }
            if pf.latin1 {
                r.push(Finding::info(
                    "non_utf8",
                    format!("{} is not UTF-8; read as Latin-1", pf.name),
                ));
            }
        }
        if self.exp.acqus.name == "acqu" {
            r.push(Finding::warning(
                "acqus_missing",
                "no acqus (status parameters); acqu (setup parameters) used instead",
            ));
        }
        match &self.raw {
            None => r.push(Finding::error(
                "missing_data",
                "no fid or ser file in the experiment directory",
            )),
            Some(Err(Fault::Unsupported(f, _))) => {
                r.push(Finding::error("unsupported_sample_type", f.clone()));
            }
            Some(Err(Fault::Corrupt(m))) => r.push(Finding::error("bad_parameters", m.clone())),
            Some(Ok(l)) => {
                for n in &l.notes {
                    r.push(Finding::info("layout", n.clone()));
                }
                if l.file_name == "fid" {
                    let need = l.row_bytes();
                    if l.file_len < need {
                        r.push(
                            Finding::error(
                                "truncated",
                                format!(
                                    "fid has {} bytes but TD {} × {} bytes needs {need}",
                                    l.file_len,
                                    l.td,
                                    l.sample_type.width()
                                ),
                            )
                            .at(l.file_len),
                        );
                    } else if l.file_len > need {
                        r.push(Finding::info(
                            "fid_padding",
                            format!(
                                "fid has {} bytes after the TD values (block padding)",
                                l.file_len - need
                            ),
                        ));
                    }
                } else {
                    if self.exp.acqu_n.is_empty() {
                        r.push(Finding::warning(
                            "missing_dimension",
                            "ser without acqu2s: the number of rows is taken from the file size",
                        ));
                    }
                    if l.rows_in_file < l.rows {
                        let at = l.rows_in_file.saturating_mul(l.row_stride);
                        r.push(Finding::error("truncated", format!("ser holds {} of {} rows ({} bytes, {}-byte rows): a truncated copy or a stopped acquisition", l.rows_in_file, l.rows, l.file_len, l.row_stride)).at(at));
                    } else if l.rows_in_file > l.rows {
                        r.push(Finding::warning(
                            "extra_rows",
                            format!(
                                "ser holds {} rows; the parameters declare {}",
                                l.rows_in_file, l.rows
                            ),
                        ));
                    }
                }
                if let Some(m) = &self.exp.nuslist_issue {
                    r.push(Finding::warning("nuslist_unreadable", m.clone()));
                }
                if self.exp.acqus.int("FnTYPE") == Some(2) {
                    match (
                        self.exp.nuslist_len,
                        self.exp.acqu_n.first().and_then(|p| p.int("TD")),
                    ) {
                        (None, _) => r.push(Finding::warning(
                            "nuslist_missing",
                            "FnTYPE = 2 (non-uniform sampling) but no nuslist file",
                        )),
                        (Some(n), Some(td2)) if (n as i64) * 2 != td2 && n as i64 != td2 => {
                            r.push(Finding::warning(
                                "nuslist_mismatch",
                                format!("nuslist has {n} entries but acqu2s TD is {td2}"),
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
        if self.exp.acqus.int("TD").is_none() {
            r.push(Finding::error("missing_parameter", "acqus has no TD"));
        }
        if self.exp.acqus.float("SW_h").is_none() {
            r.push(Finding::warning(
                "missing_parameter",
                "acqus has no SW_h: no sampling rate",
            ));
        }
        let (gd, src) = group_delay(&self.exp.acqus);
        if gd.is_none() && self.exp.acqus.get("DSPFVS").is_some() {
            r.push(Finding::info(
                "group_delay",
                format!("digital-filter group delay {src}"),
            ));
        }
        for p in &self.exp.processing {
            for pf in &p.procs {
                for m in &pf.issues {
                    r.push(Finding::warning(
                        "parameter_syntax",
                        format!("pdata/{}/{}: {m}", p.procno, pf.name),
                    ));
                }
                if !pf.ended {
                    r.push(Finding::warning(
                        "parameter_unterminated",
                        format!("pdata/{}/{} has no ##END=", p.procno, pf.name),
                    ));
                }
            }
            if p.procs.is_empty() && !p.data_files.is_empty() {
                r.push(Finding::error(
                    "missing_parameters",
                    format!("pdata/{} has processed data but no procs", p.procno),
                ));
            }
        }
        for (procno, lr) in &self.procs {
            match lr {
                Err(f) => r.push(Finding::error(
                    "bad_processed_data",
                    format!("pdata/{procno}: {}", f.message()),
                )),
                Ok(l) => {
                    let need = l.values() * l.sample_type.width();
                    for (_, file, size) in &l.components {
                        if *size < need {
                            r.push(
                                Finding::error(
                                    "truncated",
                                    format!(
                                        "pdata/{procno}/{file} has {size} bytes; SI needs {need}"
                                    ),
                                )
                                .at(*size),
                            );
                        } else if *size > need {
                            r.push(Finding::warning(
                                "size_mismatch",
                                format!("pdata/{procno}/{file} has {size} bytes; SI needs {need}"),
                            ));
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
    use crate::bruker_params::parse_param_file;

    #[test]
    fn group_delay_sources() {
        let a = parse_param_file(
            "acqus",
            b"##$GRPDLY= 67.98\n##$DSPFVS= 20\n##$DECIM= 1600\n##END=\n",
        );
        assert_eq!(group_delay(&a), (Some(67.98), "GRPDLY"));
        let b = parse_param_file(
            "acqus",
            b"##$GRPDLY= -1\n##$DSPFVS= 10\n##$DECIM= 6\n##END=\n",
        );
        assert_eq!(group_delay(&b).0, Some(59.083_333_333_333_333));
        let c = parse_param_file(
            "acqus",
            b"##$GRPDLY= -1\n##$DSPFVS= 20\n##$DECIM= 6\n##END=\n",
        );
        assert_eq!(group_delay(&c).0, None);
    }
}
