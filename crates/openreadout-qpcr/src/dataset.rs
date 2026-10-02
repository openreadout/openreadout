//! `Dataset` over the normalized model: the results table, curve tables, traces, listing,
//! integrity checks and provenance.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Method, Origin, Quantity,
};
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::Input;
use openreadout_core::{Error, Plane, Result};

use crate::analysis::melt_derivative;
use crate::model::{Assay, Dialect, QpcrData, Reaction, Run};
use crate::{EDS_FORMAT_ID, IXO_FORMAT_ID, RDML_FORMAT_ID, REX_FORMAT_ID, descriptor_of};
use openreadout_core::zip::ZipIndex;

/// Columns of table 0 (`results`), in order: one row per well × target.
pub const RESULT_COLUMNS: [&str; 23] = [
    "well",
    "row",
    "col",
    "cq",
    "target",
    "sample",
    "dye",
    "task",
    "quantity",
    "cq_mean",
    "cq_sd",
    "tm",
    "tm2",
    "threshold",
    "baseline_start",
    "baseline_end",
    "amp_status",
    "calculated_quantity",
    "efficiency",
    "excluded",
    "cq_confidence",
    "run",
    "cq_status",
];

/// Categories of the `cq_status` column (codes 0, 1, 2).
pub const CQ_STATUSES: [&str; 3] = [
    crate::report::CQ_DETERMINED,
    crate::report::CQ_UNDETERMINED,
    crate::report::CQ_NO_RESULT,
];

/// `cq_status` code of an assay (`None`: a well without a result).
fn cq_status_code(a: Option<&Assay>) -> f64 {
    match a {
        Some(a) if a.cq.is_some() => 0.0,
        Some(a) if a.cq_undetermined => 1.0,
        _ => 2.0,
    }
}

const AMP_COLUMNS: [&str; 9] = [
    "well",
    "row",
    "col",
    "target",
    "dye",
    "cycle",
    "fluorescence",
    "corrected",
    "run",
];
const MELT_COLUMNS: [&str; 8] = [
    "well",
    "row",
    "col",
    "target",
    "dye",
    "temperature",
    "fluorescence",
    "run",
];

/// Largest file read (qPCR documents are kilobytes to tens of megabytes).
const MAX_FILE: u64 = 2 << 30;

/// Reference to one assay: (run, reaction, assay).
type AssayRef = (usize, usize, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TraceKind {
    Amplification,
    Corrected,
    Melt,
    MeltDerivative,
    Multicomponent,
}

#[derive(Debug, Clone)]
struct TraceSpec {
    name: String,
    kind: TraceKind,
    run: usize,
    dye: String,
    /// (reaction, assay or signal index)
    channels: Vec<(usize, usize)>,
    len: u64,
}

/// Category lists for the categorical columns.
#[derive(Debug, Default, Clone)]
struct Cats {
    wells: Vec<String>,
    samples: Vec<String>,
    targets: Vec<String>,
    dyes: Vec<String>,
    tasks: Vec<String>,
    statuses: Vec<String>,
    runs: Vec<String>,
}

fn code(list: &mut Vec<String>, v: &str) -> f64 {
    let i = list.iter().position(|x| x == v).unwrap_or_else(|| {
        list.push(v.to_string());
        list.len() - 1
    });
    i as f64
}

fn opt_code(list: &mut Vec<String>, v: Option<&str>) -> f64 {
    v.map_or(f64::NAN, |v| code(list, v))
}

fn o(v: Option<f64>) -> f64 {
    v.unwrap_or(f64::NAN)
}

/// An opened qPCR file (RDML, `.eds`, `.rex`).
#[derive(Debug)]
pub struct QpcrDataset {
    path: PathBuf,
    size: u64,
    format_id: &'static str,
    pub(crate) data: QpcrData,
    /// Zip members (name, size, compressed size, encrypted), for `info --view structure` and
    /// `check`.
    members: Vec<(String, u64, u64, bool)>,
    input: Input,
    /// Result rows: one per assay, or (run, reaction, usize::MAX) for a well without assays.
    rows: Vec<AssayRef>,
    amp_rows: Vec<(AssayRef, u64)>,
    melt_rows: Vec<(AssayRef, u64)>,
    traces: Vec<TraceSpec>,
    cats: Cats,
}

impl QpcrDataset {
    /// Open a qPCR file of any supported dialect, detecting it from its content.
    pub fn open(path: &Path) -> Result<Self> {
        let input = Input::local(path);
        let head = {
            let mut f = input.open()?;
            let mut buf = vec![0u8; 4096];
            let n = std::io::Read::read(&mut f, &mut buf).map_err(|e| Error::io(path, e))?;
            buf.truncate(n);
            buf
        };
        let id = if openreadout_core::zip::is_zip(&head) {
            let z = ZipIndex::open(input.fs(), path, RDML_FORMAT_ID)?;
            if crate::eds::layout(&z).is_some() {
                EDS_FORMAT_ID
            } else {
                RDML_FORMAT_ID
            }
        } else if String::from_utf8_lossy(&head).contains("<RexHeader>") {
            REX_FORMAT_ID
        } else if String::from_utf8_lossy(&head).contains("signature=\"IXOS\"") {
            IXO_FORMAT_ID
        } else {
            RDML_FORMAT_ID
        };
        Self::open_as(&input, id)
    }

    /// Open an input as the given format.
    pub(crate) fn open_as(input: &Input, format_id: &'static str) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let size = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
        if size > MAX_FILE {
            return Err(Error::unsupported(
                format_id,
                format!("a {} MiB file", size >> 20),
                "qPCR files are at most tens of megabytes; this is probably not one.",
            ));
        }
        let mut members = Vec::new();
        let data = match format_id {
            RDML_FORMAT_ID | EDS_FORMAT_ID => {
                let mut head = [0u8; 8];
                {
                    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
                    let _ =
                        std::io::Read::read(&mut f, &mut head).map_err(|e| Error::io(path, e))?;
                }
                if openreadout_core::zip::is_zip(&head) {
                    let z = ZipIndex::open(fs, path, format_id)?;
                    members = z
                        .members
                        .iter()
                        .map(|m| (m.name.clone(), m.size, m.compressed_size, m.encrypted))
                        .collect();
                    if format_id == EDS_FORMAT_ID {
                        crate::eds::parse_eds(&z)?
                    } else {
                        let names: Vec<String> = z.members.iter().map(|m| m.name.clone()).collect();
                        let mut found = None;
                        for n in crate::rdml::main_member(&names) {
                            let t = z.read_text(&n)?.unwrap_or_default();
                            if crate::rdml::looks_like_rdml(t.get(..4096).unwrap_or(&t)) {
                                found = Some(t);
                                break;
                            }
                        }
                        let t = found.ok_or_else(|| {
                            Error::corrupt(
                                RDML_FORMAT_ID,
                                "the zip holds no RDML document (no rdml_data.xml, no .xml member with an <rdml> root)",
                            )
                        })?;
                        let mut d = crate::rdml::parse_rdml(&t)?;
                        crate::lc96::apply(&z, &mut d)?;
                        d
                    }
                } else if format_id == RDML_FORMAT_ID {
                    let bytes = fs.read(path).map_err(|e| Error::io(path, e))?;
                    crate::rdml::parse_rdml(&openreadout_core::zip::text(&bytes))?
                } else {
                    return Err(Error::corrupt(
                        EDS_FORMAT_ID,
                        "not a zip archive (an .eds experiment document is a zip)",
                    ));
                }
            }
            IXO_FORMAT_ID => {
                let bytes = fs.read(path).map_err(|e| Error::io(path, e))?;
                crate::ixo::parse_ixo(&bytes)?
            }
            _ => {
                let bytes = fs.read(path).map_err(|e| Error::io(path, e))?;
                crate::rex::parse_rex(&openreadout_core::zip::text(&bytes))?
            }
        };
        let mut ds = QpcrDataset {
            path: path.to_path_buf(),
            size,
            format_id,
            data,
            members,
            input: input.clone(),
            rows: Vec::new(),
            amp_rows: Vec::new(),
            melt_rows: Vec::new(),
            traces: Vec::new(),
            cats: Cats::default(),
        };
        ds.index();
        Ok(ds)
    }

    /// The file's format id (`rdml`, `applied-biosystems-eds`, `rotor-gene-rex`).
    pub fn format_id(&self) -> &'static str {
        self.format_id
    }

    /// The path that was opened.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn assay(&self, r: AssayRef) -> Option<(&Run, &Reaction, Option<&Assay>)> {
        let run = self.data.runs.get(r.0)?;
        let rx = run.reactions.get(r.1)?;
        Some((run, rx, rx.assays.get(r.2)))
    }

    /// Build row and trace indices and the category lists.
    fn index(&mut self) {
        let mut cats = Cats {
            samples: self.data.samples.iter().map(|s| s.name.clone()).collect(),
            targets: self.data.targets.iter().map(|t| t.name.clone()).collect(),
            dyes: self.data.dye_names(),
            runs: self.data.runs.iter().map(|r| r.name.clone()).collect(),
            ..Cats::default()
        };
        for (ri, run) in self.data.runs.iter().enumerate() {
            for (xi, rx) in run.reactions.iter().enumerate() {
                let w = run.well_name(rx.row, rx.column);
                code(&mut cats.wells, &w);
                if rx.assays.is_empty() {
                    self.rows.push((ri, xi, usize::MAX));
                }
                for (ai, a) in rx.assays.iter().enumerate() {
                    self.rows.push((ri, xi, ai));
                    if let Some(c) = &a.amplification {
                        self.amp_rows
                            .push(((ri, xi, ai), c.fluorescence.len() as u64));
                    }
                    if let Some(m) = &a.melt {
                        self.melt_rows
                            .push(((ri, xi, ai), m.temperature.len() as u64));
                    }
                }
            }
        }
        // traces per run and dye
        for (ri, run) in self.data.runs.iter().enumerate() {
            let prefix = if self.data.runs.len() > 1 {
                format!("{}: ", run.name)
            } else {
                String::new()
            };
            let mut dyes: Vec<String> = Vec::new();
            for rx in &run.reactions {
                for a in &rx.assays {
                    let d = a.dye.clone().unwrap_or_else(|| "?".into());
                    if !dyes.contains(&d) {
                        dyes.push(d);
                    }
                }
            }
            for dye in &dyes {
                let pick = |f: &dyn Fn(&Assay) -> Option<u64>| -> (Vec<(usize, usize)>, u64) {
                    let mut ch = Vec::new();
                    let mut len = 0u64;
                    for (xi, rx) in run.reactions.iter().enumerate() {
                        for (ai, a) in rx.assays.iter().enumerate() {
                            if a.dye.as_deref().unwrap_or("?") != dye {
                                continue;
                            }
                            if let Some(n) = f(a) {
                                ch.push((xi, ai));
                                len = len.max(n);
                            }
                        }
                    }
                    (ch, len)
                };
                let kinds: [(TraceKind, String, &dyn Fn(&Assay) -> Option<u64>); 4] = [
                    (
                        TraceKind::Amplification,
                        format!("{prefix}amplification {dye}"),
                        &|a: &Assay| {
                            a.amplification
                                .as_ref()
                                .map(|c| c.fluorescence.len() as u64)
                        },
                    ),
                    (
                        TraceKind::Corrected,
                        format!("{prefix}amplification {dye} baseline-corrected"),
                        &|a: &Assay| {
                            a.amplification
                                .as_ref()
                                .and_then(|c| c.corrected.as_ref())
                                .map(|v| v.len() as u64)
                        },
                    ),
                    (
                        TraceKind::Melt,
                        format!("{prefix}melt {dye}"),
                        &|a: &Assay| a.melt.as_ref().map(|m| m.fluorescence.len() as u64),
                    ),
                    (
                        TraceKind::MeltDerivative,
                        format!("{prefix}melt {dye} -dF/dT"),
                        &|a: &Assay| {
                            a.melt.as_ref().map(|m| {
                                m.derivative
                                    .as_ref()
                                    .map_or(m.fluorescence.len(), |d| d.1.len())
                                    as u64
                            })
                        },
                    ),
                ];
                for (kind, name, f) in kinds {
                    let (channels, len) = pick(f);
                    if !channels.is_empty() && len > 0 {
                        self.traces.push(TraceSpec {
                            name,
                            kind,
                            run: ri,
                            dye: dye.clone(),
                            channels,
                            len,
                        });
                    }
                }
            }
            // multicomponent signals (every dye, including the passive reference)
            let mut sdyes: Vec<String> = Vec::new();
            for rx in &run.reactions {
                for s in &rx.signals {
                    if !sdyes.contains(&s.dye) {
                        sdyes.push(s.dye.clone());
                    }
                }
            }
            for dye in sdyes {
                let mut channels = Vec::new();
                let mut len = 0u64;
                for (xi, rx) in run.reactions.iter().enumerate() {
                    if let Some(si) = rx.signals.iter().position(|s| s.dye == dye) {
                        channels.push((xi, si));
                        len = len.max(rx.signals[si].values.len() as u64);
                    }
                }
                if len > 0 {
                    self.traces.push(TraceSpec {
                        name: format!("{prefix}multicomponent {dye}"),
                        kind: TraceKind::Multicomponent,
                        run: ri,
                        dye,
                        channels,
                        len,
                    });
                }
            }
        }
        // category lists grow with what the rows use
        for &r in &self.rows.clone() {
            if let Some((run, rx, a)) = self.assay(r) {
                let _ = run;
                opt_code(&mut cats.samples, rx.sample.as_deref());
                if let Some(a) = a {
                    opt_code(&mut cats.targets, a.target.as_deref());
                    opt_code(&mut cats.dyes, a.dye.as_deref());
                    opt_code(&mut cats.tasks, a.task.as_deref());
                    opt_code(&mut cats.statuses, a.amp_status.as_deref());
                }
            }
        }
        self.cats = cats;
    }

    fn well_of(&self, run: &Run, rx: &Reaction) -> f64 {
        let w = run.well_name(rx.row, rx.column);
        self.cats
            .wells
            .iter()
            .position(|x| *x == w)
            .map_or(f64::NAN, |i| i as f64)
    }

    fn cat(list: &[String], v: Option<&str>) -> f64 {
        v.and_then(|v| list.iter().position(|x| x == v))
            .map_or(f64::NAN, |i| i as f64)
    }

    fn result_row(&self, r: AssayRef) -> Vec<f64> {
        let Some((run, rx, a)) = self.assay(r) else {
            return vec![f64::NAN; RESULT_COLUMNS.len()];
        };
        let c = &self.cats;
        let well = self.well_of(run, rx);
        let base = [well, f64::from(rx.row + 1), f64::from(rx.column + 1)];
        let run_code = Self::cat(&c.runs, Some(&run.name));
        let sample = Self::cat(&c.samples, rx.sample.as_deref());
        let Some(a) = a else {
            let mut v = vec![f64::NAN; RESULT_COLUMNS.len()];
            v[..3].copy_from_slice(&base);
            v[5] = sample;
            v[19] = if rx.omitted { 1.0 } else { 0.0 };
            v[21] = run_code;
            v[22] = cq_status_code(None);
            return v;
        };
        vec![
            base[0],
            base[1],
            base[2],
            o(a.cq),
            Self::cat(&c.targets, a.target.as_deref()),
            sample,
            Self::cat(&c.dyes, a.dye.as_deref()),
            Self::cat(&c.tasks, a.task.as_deref()),
            o(a.quantity),
            o(a.cq_mean),
            o(a.cq_sd),
            o(a.tm.first().copied()),
            o(a.tm.get(1).copied()),
            o(a.threshold),
            o(a.baseline_start.map(f64::from)),
            o(a.baseline_end.map(f64::from)),
            Self::cat(&c.statuses, a.amp_status.as_deref()),
            o(a.calculated_quantity),
            o(a.efficiency),
            if a.excluded.is_some() || rx.omitted {
                1.0
            } else {
                0.0
            },
            o(a.cq_confidence),
            run_code,
            cq_status_code(Some(a)),
        ]
    }

    fn column(
        index: u32,
        name: &str,
        dtype: &str,
        unit: Option<&str>,
        label: &str,
        categories: Option<&[String]>,
    ) -> ColumnInfo {
        let mut extra = BTreeMap::new();
        if let Some(c) = categories {
            extra.insert("categories".into(), json!(c));
        }
        ColumnInfo {
            index,
            name: name.into(),
            label: Some(label.into()),
            dtype: dtype.into(),
            unit: unit.map(str::to_string),
            range: None,
            extra,
        }
    }

    fn results_info(&self) -> TableInfo {
        let c = &self.cats;
        let statuses: Vec<String> = CQ_STATUSES.iter().map(|s| (*s).to_string()).collect();
        let labels: [(&str, Option<&[String]>, &str, Option<&str>); 23] = [
            (
                "uint32",
                Some(&c.wells),
                "well name, as a code into extra.categories",
                None,
            ),
            ("uint16", None, "plate row, 1-based (A = 1)", None),
            ("uint16", None, "plate column, 1-based", None),
            (
                "float64",
                None,
                "quantification cycle (Cq/Ct) as the vendor software or RDML file reports it; NaN = no Cq (cq_status says whether it is undetermined or there is no result)",
                None,
            ),
            (
                "uint32",
                Some(&c.targets),
                "target (assay, detector, gene), code into extra.categories",
                None,
            ),
            (
                "uint32",
                Some(&c.samples),
                "sample, code into extra.categories",
                None,
            ),
            (
                "uint32",
                Some(&c.dyes),
                "reporter dye, code into extra.categories",
                None,
            ),
            (
                "uint32",
                Some(&c.tasks),
                "role of the well for this target: unknown, standard, ntc, ... (code into extra.categories)",
                None,
            ),
            ("float64", None, "given quantity (standards)", None),
            (
                "float64",
                None,
                "mean Cq of the replicate group (vendor)",
                None,
            ),
            (
                "float64",
                None,
                "standard deviation of the replicate group's Cq (vendor)",
                None,
            ),
            (
                "float64",
                None,
                "melting temperature of the first melt peak (vendor)",
                Some("°C"),
            ),
            (
                "float64",
                None,
                "melting temperature of a second melt peak (vendor)",
                Some("°C"),
            ),
            (
                "float64",
                None,
                "fluorescence threshold the Cq was called at (vendor setting or result)",
                None,
            ),
            ("float64", None, "first cycle of the baseline window", None),
            ("float64", None, "last cycle of the baseline window", None),
            (
                "uint32",
                Some(&c.statuses),
                "amplification status (vendor), code into extra.categories",
                None,
            ),
            (
                "float64",
                None,
                "quantity calculated by the vendor software (standard curve)",
                None,
            ),
            (
                "float64",
                None,
                "amplification efficiency of the reaction (fold per cycle, 2 = 100 %)",
                None,
            ),
            (
                "uint8",
                None,
                "1 when the well or result is excluded/omitted",
                None,
            ),
            ("float64", None, "Cq confidence (vendor)", None),
            (
                "uint32",
                Some(&c.runs),
                "run (plate), code into extra.categories",
                None,
            ),
            (
                "uint8",
                Some(&statuses),
                "Cq status: determined (a Cq), undetermined (no amplification; cq is NaN), no result; code into extra.categories",
                None,
            ),
        ];
        let columns = RESULT_COLUMNS
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(i, (name, (dt, cats, label, unit)))| {
                Self::column(i as u32, name, dt, unit, label, cats)
            })
            .collect();
        let mut extra: BTreeMap<String, Value> = BTreeMap::new();
        let d = &self.data;
        extra.insert("dialect".into(), json!(d.dialect.id()));
        let mut instrument = serde_json::Map::new();
        for (k, v) in [
            ("manufacturer", &d.instrument.manufacturer),
            ("model", &d.instrument.model),
            ("serial_number", &d.instrument.serial_number),
            ("software", &d.instrument.software),
            ("software_version", &d.instrument.software_version),
            ("firmware_version", &d.instrument.firmware_version),
        ] {
            if let Some(v) = v {
                instrument.insert(k.into(), json!(v));
            }
        }
        if !instrument.is_empty() {
            extra.insert("instrument".into(), Value::Object(instrument));
        }
        let mut put = |k: &str, v: Option<Value>| {
            if let Some(v) = v {
                extra.insert(k.into(), v);
            }
        };
        put("experiment_name", d.name.as_ref().map(|v| json!(v)));
        put("description", d.description.as_ref().map(|v| json!(v)));
        put(
            "experiment_type",
            d.experiment_type.as_ref().map(|v| json!(v)),
        );
        put("chemistry", d.chemistry.as_ref().map(|v| json!(v)));
        put("operator", d.operator.as_ref().map(|v| json!(v)));
        put("run_state", d.run_state.as_ref().map(|v| json!(v)));
        put("created_at", d.created_at.as_ref().map(|v| json!(v)));
        put("acquired_at", d.started_at.as_ref().map(|v| json!(v)));
        put("ended_at", d.ended_at.as_ref().map(|v| json!(v)));
        put(
            "passive_reference",
            d.passive_reference.as_ref().map(|v| json!(v)),
        );
        put(
            "reference_targets",
            (!d.reference_targets.is_empty()).then(|| json!(d.reference_targets)),
        );
        put(
            "calibrator_sample",
            d.calibrator_sample.as_ref().map(|v| json!(v)),
        );
        put(
            "experimenters",
            (!d.experimenters.is_empty()).then(|| {
                json!(
                    d.experimenters
                        .iter()
                        .map(|p| {
                            let mut m = serde_json::Map::new();
                            m.insert("id".into(), json!(p.id));
                            for (k, v) in [
                                ("first_name", &p.first_name),
                                ("last_name", &p.last_name),
                                ("email", &p.email),
                                ("lab", &p.lab),
                            ] {
                                if let Some(v) = v {
                                    m.insert(k.into(), json!(v));
                                }
                            }
                            Value::Object(m)
                        })
                        .collect::<Vec<_>>()
                )
            }),
        );
        extra.insert(
            "targets".into(),
            json!(
                d.targets
                    .iter()
                    .map(|t| {
                        let mut m = serde_json::Map::new();
                        m.insert("name".into(), json!(t.name));
                        for (k, v) in [
                            ("kind", &t.kind),
                            ("dye", &t.dye),
                            ("quencher", &t.quencher),
                            ("efficiency_method", &t.efficiency_method),
                            ("description", &t.description),
                        ] {
                            if let Some(v) = v {
                                m.insert(k.into(), json!(v));
                            }
                        }
                        for (k, v) in [
                            ("efficiency", t.efficiency),
                            ("efficiency_se", t.efficiency_se),
                            ("melting_temperature_c", t.melting_temperature),
                        ] {
                            if let Some(v) = v {
                                m.insert(k.into(), json!(v));
                            }
                        }
                        if !t.sequences.is_empty() {
                            m.insert(
                                "sequences".into(),
                                Value::Object(
                                    t.sequences
                                        .iter()
                                        .map(|(k, v)| (k.clone(), json!(v)))
                                        .collect(),
                                ),
                            );
                        }
                        Value::Object(m)
                    })
                    .collect::<Vec<_>>()
            ),
        );
        extra.insert(
            "samples".into(),
            json!(
                d.samples
                    .iter()
                    .map(|s| {
                        let mut m = serde_json::Map::new();
                        m.insert("name".into(), json!(s.name));
                        if let Some(i) = &s.id {
                            m.insert("id".into(), json!(i));
                        }
                        if let Some(k) = &s.kind {
                            m.insert("kind".into(), json!(k));
                        }
                        if let Some(q) = s.quantity {
                            m.insert("quantity".into(), json!(q));
                        }
                        if let Some(u) = &s.quantity_unit {
                            m.insert("quantity_unit".into(), json!(u));
                        }
                        if !s.per_target.is_empty() {
                            m.insert(
                                "per_target".into(),
                                json!(
                                    s.per_target
                                        .iter()
                                        .map(|(t, k, q)| json!({"target": t, "kind": k, "quantity": q}))
                                        .collect::<Vec<_>>()
                                ),
                            );
                        }
                        if !s.annotations.is_empty() {
                            m.insert(
                                "annotations".into(),
                                Value::Object(
                                    s.annotations
                                        .iter()
                                        .map(|(k, v)| (k.clone(), json!(v)))
                                        .collect(),
                                ),
                            );
                        }
                        Value::Object(m)
                    })
                    .collect::<Vec<_>>()
            ),
        );
        extra.insert("dyes".into(), json!(d.dye_names()));
        extra.insert(
            "programs".into(),
            json!(d.programs.iter().map(program_json).collect::<Vec<_>>()),
        );
        extra.insert(
            "runs".into(),
            json!(
                d.runs
                    .iter()
                    .map(|r| {
                        let mut m = serde_json::Map::new();
                        m.insert("name".into(), json!(r.name));
                        for (k, v) in [
                            ("experiment", &r.experiment),
                            ("description", &r.description),
                            ("instrument", &r.instrument),
                            ("software", &r.software),
                            ("started_at", &r.started_at),
                            ("cq_method", &r.cq_method),
                            ("background_method", &r.background_method),
                        ] {
                            if let Some(v) = v {
                                m.insert(k.into(), json!(v));
                            }
                        }
                        m.insert("rows".into(), json!(r.rows));
                        m.insert("columns".into(), json!(r.columns));
                        m.insert("wells_used".into(), json!(r.reactions.len()));
                        if let Some(p) = r.program.and_then(|i| d.programs.get(i)) {
                            m.insert("program".into(), json!(p.name));
                        }
                        Value::Object(m)
                    })
                    .collect::<Vec<_>>()
            ),
        );
        if let Some(r) = d.runs.first() {
            extra.insert("plate_rows".into(), json!(r.rows));
            extra.insert("plate_columns".into(), json!(r.columns));
        }
        let mut tasks: BTreeMap<String, u64> = BTreeMap::new();
        let mut undetermined = 0u64;
        let mut with_cq = 0u64;
        for &(ri, xi, ai) in &self.rows {
            if let Some(a) = self
                .data
                .runs
                .get(ri)
                .and_then(|r| r.reactions.get(xi))
                .and_then(|x| x.assays.get(ai))
            {
                *tasks
                    .entry(a.task.clone().unwrap_or_else(|| "unassigned".into()))
                    .or_default() += 1;
                if a.cq.is_some() {
                    with_cq += 1;
                }
                if a.cq_undetermined {
                    undetermined += 1;
                }
            }
        }
        extra.insert("rows_by_task".into(), json!(tasks));
        extra.insert("rows_with_cq".into(), json!(with_cq));
        extra.insert("undetermined".into(), json!(undetermined));
        if !d.standard_curves.is_empty() {
            extra.insert(
                "vendor_standard_curves".into(),
                json!(
                    d.standard_curves
                        .iter()
                        .map(|s| json!({
                            "target": s.target,
                            "dye": s.dye,
                            "slope": s.slope,
                            "intercept": s.intercept,
                            "r2": s.r2,
                            "efficiency_percent": s.efficiency_percent,
                        }))
                        .collect::<Vec<_>>()
                ),
            );
        }
        TableInfo {
            index: 0,
            name: Some("results".into()),
            row_count: self.rows.len() as u64,
            columns,
            extra,
        }
    }

    /// Category lists of the `genotypes` table: wells, samples, markers, tasks, calls,
    /// genotypes, methods.
    fn genotype_categories(&self) -> [Vec<String>; 7] {
        let run = self.data.runs.first();
        let mut cats: [Vec<String>; 7] = Default::default();
        for g in &self.data.genotypes {
            let well = run.map_or_else(
                || g.position.to_string(),
                |r| {
                    let cols = r.columns.max(1);
                    r.well_name(g.position / cols, g.position % cols)
                },
            );
            for (i, v) in [
                Some(well),
                g.sample.clone(),
                g.marker.clone(),
                g.task.clone(),
                Some(g.call()),
                g.genotype(),
                g.method.clone(),
            ]
            .into_iter()
            .enumerate()
            {
                if let Some(v) = v
                    && !cats[i].contains(&v)
                {
                    cats[i].push(v);
                }
            }
        }
        cats
    }

    fn genotype_rows(&self) -> Vec<Vec<f64>> {
        let cats = self.genotype_categories();
        let run = self.data.runs.first();
        let cols = run.map_or(1, |r| r.columns.max(1));
        let c = |i: usize, v: Option<&str>| {
            v.and_then(|v| cats[i].iter().position(|x| x == v))
                .map_or(f64::NAN, |k| k as f64)
        };
        self.data
            .genotypes
            .iter()
            .map(|g| {
                let well = run.map_or_else(
                    || g.position.to_string(),
                    |r| r.well_name(g.position / cols, g.position % cols),
                );
                let o = |v: Option<f64>| v.unwrap_or(f64::NAN);
                vec![
                    c(0, Some(&well)),
                    f64::from(g.position / cols + 1),
                    f64::from(g.position % cols + 1),
                    c(1, g.sample.as_deref()),
                    c(2, g.marker.as_deref()),
                    c(3, g.task.as_deref()),
                    c(4, Some(&g.call())),
                    c(5, g.genotype().as_deref()),
                    g.code.map_or(f64::NAN, |v| v as f64),
                    o(g.rn_x),
                    o(g.rn_y),
                    o(g.reference),
                    o(g.confidence),
                    c(6, g.method.as_deref()),
                ]
            })
            .collect()
    }

    /// The `genotypes` table: one row per genotyped well (`.eds` genotyping runs).
    fn genotypes_info(&self, index: u32) -> TableInfo {
        let cats = self.genotype_categories();
        let spec: [(&str, &str, &str, Option<usize>); 14] = [
            (
                "well",
                "uint32",
                "well name, code into extra.categories",
                Some(0),
            ),
            ("row", "uint16", "plate row, 1-based", None),
            ("col", "uint16", "plate column, 1-based", None),
            (
                "sample",
                "uint32",
                "sample, code into extra.categories",
                Some(1),
            ),
            (
                "marker",
                "uint32",
                "SNP assay (marker), code into extra.categories",
                Some(2),
            ),
            (
                "task",
                "uint32",
                "role of the well, code into extra.categories",
                Some(3),
            ),
            (
                "call",
                "uint32",
                "the vendor's call: allele 1/allele 1, allele 1/allele 2, allele 2/allele 2, negative control, undetermined (code into extra.categories)",
                Some(4),
            ),
            (
                "genotype",
                "uint32",
                "the call in the marker's allele names (A/T), code into extra.categories",
                Some(5),
            ),
            (
                "call_code",
                "float64",
                "the call code as the file stores it (1, 2, 3, 0, -1)",
                None,
            ),
            (
                "rn_x",
                "float64",
                "allele-1 reporter signal (normalized, end point)",
                None,
            ),
            (
                "rn_y",
                "float64",
                "allele-2 reporter signal (normalized, end point)",
                None,
            ),
            ("reference", "float64", "passive-reference signal", None),
            (
                "confidence",
                "float64",
                "the vendor's call confidence",
                None,
            ),
            (
                "method",
                "uint32",
                "Auto or Manual call, code into extra.categories",
                Some(6),
            ),
        ];
        let columns = spec
            .iter()
            .enumerate()
            .map(|(i, (n, dt, label, cat))| {
                Self::column(
                    i as u32,
                    n,
                    dt,
                    None,
                    label,
                    cat.map(|k| cats[k].as_slice()),
                )
            })
            .collect();
        let mut extra = BTreeMap::new();
        let mut markers = serde_json::Map::new();
        for g in &self.data.genotypes {
            if let (Some(m), Some((a, b))) = (&g.marker, &g.alleles) {
                markers.insert(m.clone(), json!({"allele_1": a, "allele_2": b}));
            }
        }
        extra.insert("markers".into(), Value::Object(markers));
        extra.insert("source".into(), json!("vendor"));
        TableInfo {
            index,
            name: Some("genotypes".into()),
            row_count: self.data.genotypes.len() as u64,
            columns,
            extra,
        }
    }

    fn curve_info(&self, index: u32) -> TableInfo {
        let c = &self.cats;
        let (names, rows, name): (&[&str], u64, &str) = if index == 1 {
            (
                &AMP_COLUMNS,
                self.amp_rows.iter().map(|r| r.1).sum(),
                "amplification",
            )
        } else {
            (
                &MELT_COLUMNS,
                self.melt_rows.iter().map(|r| r.1).sum(),
                "melt",
            )
        };
        let columns = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let (dt, cats, label, unit): (&str, Option<&[String]>, &str, Option<&str>) = match *n {
                    "well" => ("uint32", Some(&c.wells), "well name, code into extra.categories", None),
                    "row" => ("uint16", None, "plate row, 1-based", None),
                    "col" => ("uint16", None, "plate column, 1-based", None),
                    "target" => ("uint32", Some(&c.targets), "target, code into extra.categories", None),
                    "dye" => ("uint32", Some(&c.dyes), "reporter dye, code into extra.categories", None),
                    "cycle" => ("float64", None, "PCR cycle", None),
                    "fluorescence" => (
                        "float64",
                        None,
                        if index == 1 {
                            "fluorescence as stored (Rn for Applied Biosystems results, RDML adp values, multicomponent signal otherwise)"
                        } else {
                            "fluorescence during the melt"
                        },
                        None,
                    ),
                    "corrected" => ("float64", None, "baseline-corrected fluorescence computed by the vendor software (ΔRn); NaN when not stored", None),
                    "temperature" => ("float64", None, "sample temperature", Some("°C")),
                    _ => ("uint32", Some(&c.runs), "run (plate), code into extra.categories", None),
                };
                Self::column(i as u32, n, dt, unit, label, cats)
            })
            .collect();
        TableInfo {
            index,
            name: Some(name.into()),
            row_count: rows,
            columns,
            extra: BTreeMap::new(),
        }
    }

    fn trace_info(&self, index: u32, t: &TraceSpec) -> TraceInfo {
        let run = &self.data.runs[t.run];
        let mut channels = Vec::with_capacity(t.channels.len());
        let mut derivative_sources: Vec<&str> = Vec::new();
        for (ci, &(xi, ai)) in t.channels.iter().enumerate() {
            let rx = &run.reactions[xi];
            let well = run.well_name(rx.row, rx.column);
            let mut extra = BTreeMap::new();
            extra.insert("well".into(), json!(well));
            if let Some(s) = &rx.sample {
                extra.insert("sample".into(), json!(s));
            }
            let target = if t.kind == TraceKind::Multicomponent {
                None
            } else {
                rx.assays.get(ai).and_then(|a| a.target.clone())
            };
            if let Some(tg) = &target {
                extra.insert("target".into(), json!(tg));
            }
            if t.kind == TraceKind::MeltDerivative
                && let Some(a) = rx.assays.get(ai)
            {
                let src = if a.melt.as_ref().is_some_and(|m| m.derivative.is_some()) {
                    "vendor"
                } else {
                    "computed"
                };
                extra.insert("derivative".into(), json!(src));
                if !derivative_sources.contains(&src) {
                    derivative_sources.push(src);
                }
            }
            channels.push(SignalChannelInfo {
                index: ci as u32,
                name: target.map_or_else(|| well.clone(), |tg| format!("{well} {tg}")),
                unit: None,
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra,
            });
        }
        let mut extra = BTreeMap::new();
        let kind = match t.kind {
            TraceKind::Amplification => "amplification",
            TraceKind::Corrected => "amplification baseline-corrected",
            TraceKind::Melt => "melt",
            TraceKind::MeltDerivative => "melt derivative",
            TraceKind::Multicomponent => "multicomponent",
        };
        extra.insert("kind".into(), json!(kind));
        extra.insert("dye".into(), json!(t.dye));
        extra.insert("run".into(), json!(run.name));
        extra.insert("plot".into(), json!("overlay"));
        let axis = match t.kind {
            TraceKind::Melt | TraceKind::MeltDerivative => {
                let (first, last) = self.melt_range(t);
                json!({"quantity": "temperature", "unit": "°C", "first": first, "last": last,
                       "note": "nominal: each well has its own temperatures (table 2 `melt`); sample i is the i-th read"})
            }
            _ => json!({"quantity": "cycle", "unit": "", "first": 1.0, "step": 1.0}),
        };
        extra.insert("axis".into(), axis);
        if !derivative_sources.is_empty() {
            extra.insert(
                "derivative".into(),
                json!(if derivative_sources.len() > 1 {
                    "mixed"
                } else {
                    derivative_sources[0]
                }),
            );
        }
        let q = match t.kind {
            TraceKind::Amplification => {
                let quantities: Vec<&str> = t
                    .channels
                    .iter()
                    .filter_map(|&(xi, ai)| {
                        run.reactions[xi]
                            .assays
                            .get(ai)?
                            .amplification
                            .as_ref()
                            .map(|c| c.quantity)
                    })
                    .collect();
                quantities.first().copied()
            }
            TraceKind::Corrected => Some("ΔRn"),
            TraceKind::Multicomponent => Some("multicomponent"),
            _ => None,
        };
        if let Some(q) = q {
            extra.insert("quantity".into(), json!(q));
        }
        TraceInfo {
            index,
            name: Some(t.name.clone()),
            sample_rate_hz: 0.0,
            sample_count: t.len,
            sweep_count: 1,
            channels,
            start_s: None,
            extra,
        }
    }

    /// Mean first and last temperature of a melt trace's channels.
    fn melt_range(&self, t: &TraceSpec) -> (f64, f64) {
        let run = &self.data.runs[t.run];
        let (mut first_sum, mut last_sum, mut count) = (0.0, 0.0, 0.0);
        for &(xi, ai) in &t.channels {
            let Some(melt) = run.reactions[xi]
                .assays
                .get(ai)
                .and_then(|assay| assay.melt.as_ref())
            else {
                continue;
            };
            let temps = match (&melt.derivative, t.kind) {
                (Some((dt, _)), TraceKind::MeltDerivative) => dt,
                _ => &melt.temperature,
            };
            if let (Some(first), Some(last)) = (temps.first(), temps.last())
                && first.is_finite()
                && last.is_finite()
            {
                first_sum += first;
                last_sum += last;
                count += 1.0;
            }
        }
        if count > 0.0 {
            (first_sum / count, last_sum / count)
        } else {
            (f64::NAN, f64::NAN)
        }
    }

    fn series(&self, t: &TraceSpec, xi: usize, ai: usize) -> Vec<f64> {
        let rx = &self.data.runs[t.run].reactions[xi];
        if t.kind == TraceKind::Multicomponent {
            rx.signals
                .get(ai)
                .map(|s| s.values.clone())
                .unwrap_or_default()
        } else {
            let Some(a) = rx.assays.get(ai) else {
                return Vec::new();
            };
            match t.kind {
                TraceKind::Amplification => {
                    a.amplification.as_ref().map(|c| c.fluorescence.clone())
                }
                TraceKind::Corrected => a.amplification.as_ref().and_then(|c| c.corrected.clone()),
                TraceKind::Melt => a.melt.as_ref().map(|m| m.fluorescence.clone()),
                _ => a.melt.as_ref().map(|m| match &m.derivative {
                    Some((_, d)) => d.clone(),
                    None => melt_derivative(&m.temperature, &m.fluorescence),
                }),
            }
            .unwrap_or_default()
        }
    }
}

fn program_json(p: &crate::model::Program) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), json!(p.name));
    if let Some(v) = &p.description {
        m.insert("description".into(), json!(v));
    }
    if let Some(v) = p.lid_temperature_c {
        m.insert("lid_temperature_c".into(), json!(v));
    }
    if let Some(v) = p.sample_volume_ul {
        m.insert("sample_volume_ul".into(), json!(v));
    }
    if let Some(v) = &p.run_mode {
        m.insert("run_mode".into(), json!(v));
    }
    if let Some(c) = p.cycles() {
        m.insert("cycles".into(), json!(c));
    }
    if let Some(t) = p.acquisition_temperature() {
        m.insert("acquisition_temperature_c".into(), json!(t));
    }
    let reads = p.acquisition_temperatures();
    if reads.len() > 1 {
        m.insert("acquisition_temperatures_c".into(), json!(reads));
    }
    m.insert(
        "stages".into(),
        json!(
            p.stages
                .iter()
                .map(|s| {
                    json!({
                        "kind": s.kind,
                        "repeats": s.repeats,
                        "steps": s.steps.iter().map(|st| {
                            let mut sm = serde_json::Map::new();
                            sm.insert("kind".into(), json!(st.kind));
                            for (k, v) in [
                                ("temperature_c", st.temperature_c),
                                ("high_temperature_c", st.high_temperature_c),
                                ("low_temperature_c", st.low_temperature_c),
                                ("hold_s", st.hold_s),
                                ("ramp_c_per_s", st.ramp_c_per_s),
                            ] {
                                if let Some(v) = v {
                                    sm.insert(k.into(), json!(v));
                                }
                            }
                            if let Some(v) = &st.measure {
                                sm.insert("measure".into(), json!(v));
                            }
                            if let Some(v) = st.goto {
                                sm.insert("goto".into(), json!(v));
                            }
                            if let Some(v) = st.repeat {
                                sm.insert("repeat".into(), json!(v));
                            }
                            Value::Object(sm)
                        }).collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>()
        ),
    );
    Value::Object(m)
}

impl Dataset for QpcrDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut tables = vec![self.results_info()];
        if !self.amp_rows.is_empty() {
            tables.push(self.curve_info(1));
        }
        if !self.melt_rows.is_empty() {
            let mut t = self.curve_info(2);
            t.index = tables.len() as u32;
            tables.push(t);
        }
        if !self.data.genotypes.is_empty() {
            tables.push(self.genotypes_info(tables.len() as u32));
        }
        let traces = self
            .traces
            .iter()
            .enumerate()
            .map(|(i, t)| self.trace_info(i as u32, t))
            .collect();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: descriptor_of(self.format_id),
            format_version: self.data.format_version.clone(),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes: self.data.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(self.data.vendor.clone())
    }

    fn provenance(&self) -> ProvenanceMap {
        let (results, structure) = match self.data.dialect {
            Dialect::Rdml => (Source::Spec, Source::Spec),
            Dialect::Rex | Dialect::Ixo => (Source::PriorArt, Source::Inferred),
            _ => (Source::Inferred, Source::Inferred),
        };
        let mut m = ProvenanceMap::new();
        for c in RESULT_COLUMNS {
            m.insert(format!("tables[0].columns[name={c}]"), results);
        }
        for c in [
            "well", "row", "col", "target", "sample", "dye", "task", "quantity",
        ] {
            m.insert(format!("tables[0].columns[name={c}]"), structure);
        }
        m.insert("tables[0].extra.programs".into(), structure);
        // RDML names the instrument per run (`runs`), not as a file-level block
        if !matches!(self.data.dialect, Dialect::Rdml) {
            m.insert("tables[0].extra.instrument".into(), Source::Inferred);
        }
        m.insert("traces[].channels".into(), structure);
        m.insert("traces[].extra.axis".into(), structure);
        // −dF/dT: the vendor's when stored, else computed by us (`extra.derivative` says which)
        m.insert("traces[].extra.derivative".into(), Source::Inferred);
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (name, size, csize, enc) in &self.members {
            out.push(LsEntry {
                kind: "member".into(),
                name: name.clone(),
                offset: None,
                size: Some(*size),
                image: None,
                details: json!({"compressed_size": csize, "encrypted": enc}),
            });
        }
        for r in &self.data.runs {
            out.push(LsEntry {
                kind: "run".into(),
                name: r.name.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({
                    "rows": r.rows,
                    "columns": r.columns,
                    "wells_used": r.reactions.len(),
                    "assays": r.reactions.iter().map(|x| x.assays.len()).sum::<usize>(),
                }),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            self.format_id,
            "images",
            "qPCR files hold tables and curves: read them with `table`, `trace` or `analyze qpcr`.",
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.path.display().to_string(), self.format_id);
        if !self.members.is_empty() {
            rep.performed("zip central directory and every member's CRC-32");
            let z = ZipIndex::open(self.input.fs(), self.input.path(), self.format_id)?;
            for m in &z.members {
                if m.encrypted {
                    rep.push(Finding::error(
                        "encrypted_member",
                        format!("member {} is encrypted", m.name),
                    ));
                    continue;
                }
                if let Err(e) = z.read(m) {
                    rep.push(Finding::error("member_unreadable", e.to_string()));
                }
            }
        }
        rep.performed("parsed setup, results and curves");
        for n in &self.data.notes {
            let sev = if n.contains("could not be read") {
                Finding::error("part_unreadable", n.clone())
            } else {
                Finding::info("note", n.clone())
            };
            rep.push(sev);
        }
        rep.performed("curve lengths against the cycle count");
        for run in &self.data.runs {
            for rx in &run.reactions {
                for a in &rx.assays {
                    if let Some(c) = &a.amplification
                        && c.cycles.len() != c.fluorescence.len()
                    {
                        rep.push(Finding::warning(
                            "curve_length",
                            format!(
                                "{} {}: {} cycles but {} values",
                                run.well_name(rx.row, rx.column),
                                a.target.as_deref().unwrap_or("?"),
                                c.cycles.len(),
                                c.fluorescence.len()
                            ),
                        ));
                    }
                }
            }
        }
        if self.data.runs.iter().all(|r| r.reactions.is_empty()) {
            rep.push(Finding::warning(
                "no_reactions",
                "the file holds no reactions",
            ));
        }
        Ok(rep)
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let mut tables = vec![0u32];
        if !self.amp_rows.is_empty() {
            tables.push(1);
        }
        if !self.melt_rows.is_empty() {
            tables.push(2);
        }
        if !self.data.genotypes.is_empty() {
            tables.push(3);
        }
        let kind = *tables.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {index} out of range (file has {} tables)",
                tables.len()
            ))
        })?;
        let end = |total: u64| first_row.saturating_add(max_rows).min(total);
        if kind == 3 {
            let (info, rows) = (self.genotypes_info(index), self.genotype_rows());
            let total = rows.len() as u64;
            let mut cols = vec![Vec::new(); info.columns.len()];
            for r in &rows[first_row.min(total) as usize..end(total) as usize] {
                for (c, v) in cols.iter_mut().zip(r) {
                    c.push(*v);
                }
            }
            return Ok(Table {
                table: index,
                first_row,
                columns: cols,
            });
        }
        if kind == 0 {
            let total = self.rows.len() as u64;
            let mut cols = vec![Vec::new(); RESULT_COLUMNS.len()];
            for i in first_row.min(total)..end(total) {
                let r = self.result_row(self.rows[i as usize]);
                for (c, v) in cols.iter_mut().zip(r) {
                    c.push(v);
                }
            }
            Ok(Table {
                table: index,
                first_row,
                columns: cols,
            })
        } else {
            let amp = kind == 1;
            let refs = if amp { &self.amp_rows } else { &self.melt_rows };
            let total: u64 = refs.iter().map(|r| r.1).sum();
            let ncol = if amp {
                AMP_COLUMNS.len()
            } else {
                MELT_COLUMNS.len()
            };
            let mut cols = vec![Vec::new(); ncol];
            let (start, stop) = (first_row.min(total), end(total));
            let mut offset = 0u64;
            for &(r, n) in refs {
                let (lo, hi) = (offset, offset + n);
                offset = hi;
                if hi <= start || lo >= stop {
                    continue;
                }
                let Some((run, rx, Some(a))) = self.assay(r) else {
                    continue;
                };
                let well = self.well_of(run, rx);
                let target = Self::cat(&self.cats.targets, a.target.as_deref());
                let dye = Self::cat(&self.cats.dyes, a.dye.as_deref());
                let run_code = Self::cat(&self.cats.runs, Some(&run.name));
                for k in start.max(lo)..stop.min(hi) {
                    let i = (k - lo) as usize;
                    let mut row = vec![
                        well,
                        f64::from(rx.row + 1),
                        f64::from(rx.column + 1),
                        target,
                        dye,
                    ];
                    if amp {
                        let c = a.amplification.as_ref();
                        row.push(c.and_then(|c| c.cycles.get(i)).copied().unwrap_or(f64::NAN));
                        row.push(
                            c.and_then(|c| c.fluorescence.get(i))
                                .copied()
                                .unwrap_or(f64::NAN),
                        );
                        row.push(
                            c.and_then(|c| c.corrected.as_ref())
                                .and_then(|v| v.get(i))
                                .copied()
                                .unwrap_or(f64::NAN),
                        );
                    } else {
                        let m = a.melt.as_ref();
                        row.push(
                            m.and_then(|m| m.temperature.get(i))
                                .copied()
                                .unwrap_or(f64::NAN),
                        );
                        row.push(
                            m.and_then(|m| m.fluorescence.get(i))
                                .copied()
                                .unwrap_or(f64::NAN),
                        );
                    }
                    row.push(run_code);
                    for (c, v) in cols.iter_mut().zip(row) {
                        c.push(v);
                    }
                }
            }
            Ok(Table {
                table: index,
                first_row,
                columns: cols,
            })
        }
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let t = self.traces.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                self.traces.len()
            ))
        })?;
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (qPCR traces have one sweep)"
            )));
        }
        let start = first_sample.min(t.len);
        let stop = first_sample.saturating_add(max_samples).min(t.len);
        let mut channels = Vec::with_capacity(t.channels.len());
        for &(xi, ai) in &t.channels {
            let s = self.series(&t, xi, ai);
            channels.push(
                (start..stop)
                    .map(|i| s.get(i as usize).copied().unwrap_or(f64::NAN))
                    .collect(),
            );
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: start,
            channels,
        })
    }

    fn experiment(&self) -> Option<Experiment> {
        let d = &self.data;
        let mut e = Experiment::default();
        let origin = |from: &str| Origin {
            source: if d.dialect == Dialect::Rdml {
                Source::Spec
            } else {
                Source::Inferred
            },
            from: from.to_string(),
        };
        let inst = ExperimentInstrument {
            vendor: d.instrument.manufacturer.clone(),
            model: d
                .instrument
                .model
                .clone()
                .or_else(|| d.runs.first().and_then(|r| r.instrument.clone())),
            serial: d.instrument.serial_number.clone(),
            software: d
                .instrument
                .software
                .clone()
                .or_else(|| d.runs.first().and_then(|r| r.software.clone())),
            software_version: d.instrument.software_version.clone(),
            kind: None,
        };
        if inst.model.is_some() || inst.vendor.is_some() || inst.software.is_some() {
            e.instrument = Some(inst);
            e.provenance
                .insert("instrument".into(), origin("instrument fields"));
        }
        let operator = d.operator.clone().or_else(|| {
            d.experimenters.first().map(|p| {
                [p.first_name.clone(), p.last_name.clone()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        });
        let started = d
            .started_at
            .clone()
            .or_else(|| d.runs.first().and_then(|r| r.started_at.clone()));
        if operator.is_some() || started.is_some() || d.ended_at.is_some() {
            e.acquisition = Some(Acquisition {
                started_at: started,
                ended_at: d.ended_at.clone(),
                operator,
                ..Acquisition::default()
            });
            e.provenance
                .insert("acquisition".into(), origin("run times, operator"));
        }
        let mut method = Method {
            name: d.experiment_type.clone(),
            ..Method::default()
        };
        if let Some(p) = d.programs.first() {
            if let Some(t) = p.acquisition_temperature() {
                method
                    .parameters
                    .insert("acquisition_temperature".into(), Quantity::number(t, "°C"));
            }
            if let Some(c) = p.cycles() {
                method
                    .parameters
                    .insert("cycles".into(), Quantity::plain(c));
            }
            if let Some(v) = p.sample_volume_ul {
                method
                    .parameters
                    .insert("reaction_volume".into(), Quantity::number(v, "µL"));
            }
        }
        if !d.targets.is_empty() {
            method.parameters.insert(
                "targets".into(),
                Quantity::plain(json!(
                    d.targets.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
                )),
            );
        }
        if method.name.is_some() || !method.parameters.is_empty() {
            e.method = Some(method);
            e.provenance
                .insert("method".into(), origin("thermal program, targets"));
        }
        // One provenance entry per value (`book/src/guides/metadata.md`): every value under a
        // group takes the group's origin. The merge into the derived experiment keeps only
        // keys that name a value, so group keys alone would leave these values without one.
        let groups: Vec<(String, Origin)> = std::mem::take(&mut e.provenance).into_iter().collect();
        for p in e.value_paths() {
            if let Some((_, o)) = groups
                .iter()
                .find(|(g, _)| p == *g || p.starts_with(&format!("{g}.")))
            {
                e.provenance.insert(p, o.clone());
            }
        }
        (!e.is_empty()).then_some(e)
    }
}
