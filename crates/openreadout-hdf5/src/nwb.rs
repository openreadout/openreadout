//! Neurodata Without Borders (NWB 2.x, HDF5): session fields; `TimeSeries`, `ElectricalSeries`
//! and `SpatialSeries` under `acquisition/` and `processing/`, and the intracellular
//! `PatchClampSeries` family there and under `stimulus/presentation/`, as traces (electrode
//! metadata per channel, clamp settings per sweep); every `DynamicTable` (electrodes, units, trials, …), units' spike times and
//! `SpikeEventSeries` as tables. The NWB schema is an open standard
//! (<https://nwb-schema.readthedocs.io>); see `docs/formats/hdf5.md` and `docs/provenance/hdf5.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::find;
use openreadout_core::model::{
    CheckReport, DetectConfidence, FileInfo, FormatDescriptor, LsEntry, SignalChannelInfo, Table,
    Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Finding, Plane, Result};
use serde_json::{Value, json};

use crate::h5util::{attr_text, attrs_json, dtype_name, looks_like_hdf5, open_h5_in, walk};
use crate::nwb_tables::{NwbTable, describe_dynamic, describe_spike_events, table_rows};

/// Neurodata types read as traces (under `acquisition/` and `processing/`).
pub const TRACE_TYPES: [&str; 3] = ["TimeSeries", "ElectricalSeries", "SpatialSeries"];
/// Intracellular (icephys) neurodata types read as traces, under `acquisition/`, `processing/`
/// and `stimulus/presentation/`: one sweep each.
pub const ICEPHYS_TYPES: [&str; 6] = [
    "PatchClampSeries",
    "CurrentClampSeries",
    "IZeroClampSeries",
    "CurrentClampStimulusSeries",
    "VoltageClampSeries",
    "VoltageClampStimulusSeries",
];
/// Scalar datasets of a patch-clamp series reported as clamp settings.
pub const ICEPHYS_SETTINGS: [&str; 11] = [
    "gain",
    "bias_current",
    "bridge_balance",
    "capacitance_compensation",
    "capacitance_fast",
    "capacitance_slow",
    "resistance_comp_bandwidth",
    "resistance_comp_correction",
    "resistance_comp_prediction",
    "whole_cell_capacitance_comp",
    "whole_cell_series_resistance_comp",
];
/// Path of the extracellular electrodes table.
pub const ELECTRODES_PATH: &str = "/general/extracellular_ephys/electrodes";

/// Format id used on the command line and in JSON.
pub const NWB_FORMAT_ID: &str = "nwb";

/// Largest number of samples read into memory by one `read_trace` call.
const MAX_READ_SAMPLES: u64 = 1 << 26;
/// Relative tolerance under which timestamps count as uniformly sampled.
const UNIFORM_TOLERANCE: f64 = 1e-6;
/// Session fields read from the root and `general/`.
const SESSION_FIELDS: [&str; 5] = [
    "session_description",
    "identifier",
    "session_start_time",
    "timestamps_reference_time",
    "file_create_date",
];
const GENERAL_FIELDS: [&str; 10] = [
    "experimenter",
    "experiment_description",
    "session_id",
    "institution",
    "lab",
    "keywords",
    "related_publications",
    "protocol",
    "notes",
    "stimulus",
];

/// The NWB reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct NwbReader;

impl FormatReader for NwbReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::NWB)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: NWB_FORMAT_ID.into(),
            name: "NWB (Neurodata Without Borders 2.x, HDF5)".into(),
            vendor: "open standard (NWB)".into(),
            extensions: vec!["nwb".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: true,
            confidence: crate::assurance::NWB.confidence,
            known_gaps: vec![
                "Traces are TimeSeries, ElectricalSeries and SpatialSeries under acquisition/ and processing/, and patch-clamp series (current/voltage clamp, their stimuli) there and under stimulus/presentation/, one trace per sweep (intracellular recording tables are tables, not used to group sweeps); ImageSeries, optical-physiology and other series types are listed by `info --view structure`, not decoded".into(),
                "Tables hold numbers: text columns become category codes (`extra.categories`), ragged columns a per-row count (units' spike times also a table of their own), object-reference columns are left out".into(),
                "NWB 1.x files and the Zarr backend are not read".into(),
                "Timestamps are compared for uniform sampling; irregular series are returned with a `time` channel".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !looks_like_hdf5(head) {
            return None;
        }
        let marked = find(head, b"NWBFile").is_some();
        if has_extension(path, &["nwb"]) || marked {
            return Some(Detection {
                format_id: NWB_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some(if marked {
                    "HDF5 file whose root is an NWBFile".into()
                } else {
                    "HDF5 file with the .nwb extension".into()
                }),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(NwbDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(NwbDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// One plain `TimeSeries`.
#[derive(Debug, Clone, PartialEq)]
pub struct NwbSeries {
    /// Group path (`/acquisition/...`).
    pub path: String,
    pub samples: u64,
    /// Columns of a 2-D `data` (1 for 1-D).
    pub columns: u64,
    pub dtype: String,
    pub unit: Option<String>,
    pub conversion: f64,
    pub offset: f64,
    /// `starting_time` and its `rate`, when the series is regularly sampled that way.
    pub starting_time: Option<f64>,
    pub rate: Option<f64>,
    /// Whether the series stores `timestamps` (irregular unless they are uniform).
    pub has_timestamps: bool,
    /// Uniform timestamps: (first, interval).
    pub uniform: Option<(f64, f64)>,
    pub description: Option<String>,
    /// The series' neurodata type (`TimeSeries`, `ElectricalSeries`, `SpatialSeries`).
    pub neurodata_type: String,
    /// Per-column factor applied after `conversion` (`ElectricalSeries.channel_conversion`).
    pub channel_conversion: Vec<f64>,
    /// Row in the electrodes table of each column (`ElectricalSeries.electrodes`).
    pub electrode_rows: Vec<u64>,
    /// Patch-clamp series: `sweep_number`, `stimulus_description` and the clamp settings
    /// (`ICEPHYS_SETTINGS`) the group holds.
    pub icephys: BTreeMap<String, Value>,
}

impl NwbSeries {
    fn irregular(&self) -> bool {
        self.rate.is_none() && self.uniform.is_none() && self.has_timestamps
    }
}

/// An opened NWB file.
pub struct NwbDataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    file: hdf5_pure::File,
    pub version: Option<String>,
    pub session: BTreeMap<String, String>,
    pub series: Vec<NwbSeries>,
    /// Other neurodata objects (path, type), listed only.
    pub others: Vec<(String, String)>,
    /// Tables: `DynamicTable`s, units' spike times, `SpikeEventSeries`.
    pub tables: Vec<NwbTable>,
    /// Electrodes-table row → the row's id and scalar values (for channel metadata).
    pub electrodes: Vec<Value>,
    /// Neurodata objects that could not be described (path: reason), reported by `check`.
    pub unreadable: Vec<String>,
    notes: Vec<String>,
}

impl std::fmt::Debug for NwbDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NwbDataset")
            .field("path", &self.path)
            .field("series", &self.series)
            .finish_non_exhaustive()
    }
}

fn corrupt(d: impl Into<String>) -> Error {
    Error::corrupt(NWB_FORMAT_ID, d.into())
}

/// A scalar or 1-element dataset as text.
fn dataset_text(file: &hdf5_pure::File, path: &str) -> Option<String> {
    let d = file.dataset(path).ok()?;
    if let Ok(v) = d.read_string() {
        let s = v.join(", ");
        let s = s.trim();
        return (!s.is_empty()).then(|| s.to_string());
    }
    let v = d.read_f64().ok()?;
    (!v.is_empty()).then(|| {
        v.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    })
}

fn attr_f64(
    attrs: &std::collections::HashMap<String, hdf5_pure::AttrValue>,
    k: &str,
) -> Option<f64> {
    attrs
        .get(k)
        .and_then(attr_text)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// `sweep_number`, `stimulus_description` and the scalar clamp settings of a patch-clamp series.
fn icephys_fields(file: &hdf5_pure::File, group: &str) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    if let Ok(a) = file.group(group).and_then(|g| g.attrs()) {
        if let Some(v) = attr_f64(&a, "sweep_number") {
            out.insert("sweep_number".into(), json!(v as u64));
        }
        if let Some(v) = a.get("stimulus_description").and_then(attr_text) {
            out.insert("stimulus_description".into(), json!(v));
        }
    }
    for k in ICEPHYS_SETTINGS {
        let path = format!("{group}/{k}");
        if let Ok(d) = file.dataset(&path)
            && let Ok(v) = d.read_f64()
            && let Some(x) = v.first().copied().filter(|x| x.is_finite())
        {
            // a float32 setting at its shortest decimal (the value the writer set), as the
            // series' float32 `conversion` attribute is read
            #[allow(clippy::cast_possible_truncation)]
            let x = if d.dtype().is_ok_and(|t| dtype_name(&t) == "float32") {
                (x as f32).to_string().parse::<f64>().unwrap_or(x)
            } else {
                x
            };
            let unit = d
                .attrs()
                .ok()
                .and_then(|a| a.get("unit").and_then(attr_text));
            out.insert(
                k.to_string(),
                match unit {
                    Some(u) => json!({"value": x, "unit": u}),
                    None => json!(x),
                },
            );
        }
    }
    out
}

/// Every value of a numeric dataset as f64 (rows `[first, first + n)` of dimension 0).
pub fn read_f64_rows(file: &hdf5_pure::File, path: &str, first: u64, n: u64) -> Result<Vec<f64>> {
    use hdf5_pure::DType as T;
    let d = file
        .dataset(path)
        .map_err(|e| corrupt(format!("{path}: {e}")))?;
    let err = |e: hdf5_pure::Error| corrupt(format!("{path}: {e}"));
    let dt = d.dtype().map_err(err)?;
    Ok(match dt {
        T::F64 => d.read_f64_rows(first, n).map_err(err)?,
        T::F32 => d
            .read_f32_rows(first, n)
            .map_err(err)?
            .into_iter()
            .map(f64::from)
            .collect(),
        T::I8 => d
            .read_i8_rows(first, n)
            .map_err(err)?
            .into_iter()
            .map(f64::from)
            .collect(),
        T::I16 => d
            .read_i16_rows(first, n)
            .map_err(err)?
            .into_iter()
            .map(f64::from)
            .collect(),
        T::I32 => d
            .read_i32_rows(first, n)
            .map_err(err)?
            .into_iter()
            .map(f64::from)
            .collect(),
        T::I64 => d
            .read_i64_rows(first, n)
            .map_err(err)?
            .into_iter()
            .map(|v| v as f64)
            .collect(),
        T::U8 => d
            .read_u8_rows(first, n)
            .map_err(err)?
            .into_iter()
            .map(f64::from)
            .collect(),
        T::U16 => d
            .read_u16_rows(first, n)
            .map_err(err)?
            .into_iter()
            .map(f64::from)
            .collect(),
        T::U32 => d
            .read_u32_rows(first, n)
            .map_err(err)?
            .into_iter()
            .map(f64::from)
            .collect(),
        T::U64 => d
            .read_u64_rows(first, n)
            .map_err(err)?
            .into_iter()
            .map(|v| v as f64)
            .collect(),
        other => {
            return Err(Error::unsupported(
                NWB_FORMAT_ID,
                format!("{path}: data of type {other}"),
                "Only numeric TimeSeries data are read.",
            ));
        }
    })
}

/// (first, interval) when `ts` is uniformly spaced within [`UNIFORM_TOLERANCE`].
fn uniform(ts: &[f64]) -> Option<(f64, f64)> {
    if ts.len() < 2 {
        return None;
    }
    let dt = (ts[ts.len() - 1] - ts[0]) / (ts.len() - 1) as f64;
    if !(dt.is_finite() && dt > 0.0) {
        return None;
    }
    ts.windows(2)
        .all(|w| ((w[1] - w[0]) - dt).abs() <= UNIFORM_TOLERANCE * dt)
        .then_some((ts[0], dt))
}

impl NwbDataset {
    /// Open an NWB file: session fields and the TimeSeries index (no sample data beyond
    /// timestamps needed to decide whether sampling is uniform).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let file = open_h5_in(fs, path, NWB_FORMAT_ID)?;
        let root = file
            .root()
            .attrs()
            .map_err(|e| corrupt(format!("root attributes: {e}")))?;
        let kind = root.get("neurodata_type").and_then(attr_text);
        if kind.as_deref() != Some("NWBFile") {
            return Err(Error::unsupported(
                NWB_FORMAT_ID,
                "an HDF5 file whose root is not an NWBFile (NWB 1.x or another HDF5 layout)",
                "Only NWB 2.x files are read; `openreadout info --view structure` with --format hdf5 lists any HDF5 file.",
            ));
        }
        let version = root.get("nwb_version").and_then(attr_text);
        let mut session = BTreeMap::new();
        for f in SESSION_FIELDS {
            if let Some(v) = dataset_text(&file, f) {
                session.insert(f.to_string(), v);
            }
        }
        for f in GENERAL_FIELDS {
            if let Some(v) = dataset_text(&file, &format!("general/{f}")) {
                session.insert(format!("general/{f}"), v);
            }
        }
        for f in [
            "subject_id",
            "species",
            "sex",
            "age",
            "genotype",
            "strain",
            "description",
        ] {
            if let Some(v) = dataset_text(&file, &format!("general/subject/{f}")) {
                session.insert(format!("general/subject/{f}"), v);
            }
        }
        let (nodes, _) = walk(&file, crate::h5util::walk_limit(fs, path, 200_000));
        let mut series = Vec::new();
        let mut others = Vec::new();
        let mut tables: Vec<NwbTable> = Vec::new();
        let mut unreadable = Vec::new();
        for n in nodes.iter().filter(|n| n.is_group) {
            if n.path == "/" || n.path.starts_with("/specifications") {
                continue;
            }
            let t = n
                .attributes
                .get("neurodata_type")
                .and_then(Value::as_str)
                .unwrap_or("");
            let in_data = n.path.starts_with("/acquisition/") || n.path.starts_with("/processing/");
            let index = u32::try_from(tables.len()).unwrap_or(u32::MAX);
            let icephys = ICEPHYS_TYPES.contains(&t)
                && (in_data || n.path.starts_with("/stimulus/presentation/"));
            if (TRACE_TYPES.contains(&t) && in_data) || icephys {
                match Self::describe(&file, &n.path, t) {
                    Ok(s) => series.push(s),
                    Err(e) => {
                        unreadable.push(format!("{}: {e}", n.path));
                        others.push((n.path.clone(), format!("{t} (unreadable: {e})")));
                    }
                }
            } else if t == "SpikeEventSeries" {
                match describe_spike_events(&file, &n.path, index) {
                    Ok(tb) => tables.push(tb),
                    Err(e) => {
                        unreadable.push(format!("{}: {e}", n.path));
                        others.push((n.path.clone(), format!("{t} (unreadable: {e})")));
                    }
                }
            } else if n.attributes.get("colnames").is_some() && table_rows(&file, &n.path).is_some()
            {
                let kind = if t.is_empty() { "DynamicTable" } else { t };
                match describe_dynamic(&file, &n.path, kind, index) {
                    Ok(mut tb) => tables.append(&mut tb),
                    Err(e) => {
                        unreadable.push(format!("{}: {e}", n.path));
                        others.push((n.path.clone(), format!("{kind} (unreadable: {e})")));
                    }
                }
            } else if !t.is_empty() {
                others.push((n.path.clone(), t.to_string()));
            }
        }
        let electrodes = if series.iter().any(|s| !s.electrode_rows.is_empty()) {
            electrode_rows_json(&file, &tables)
        } else {
            Vec::new()
        };
        let mut notes = Vec::new();
        if let Some(d) = session.get("session_description") {
            notes.push(format!("session: {d}"));
        }
        if let Some(s) = session.get("session_start_time") {
            notes.push(format!("session start: {s}"));
        }
        for k in [
            "general/lab",
            "general/institution",
            "general/experimenter",
            "general/session_id",
        ] {
            if let Some(v) = session.get(k) {
                notes.push(format!("{}: {v}", k.trim_start_matches("general/")));
            }
        }
        if !others.is_empty() {
            let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
            for (_, t) in &others {
                *kinds.entry(t.as_str()).or_default() += 1;
            }
            notes.push(format!(
                "NWB objects listed by `info --view structure`, not decoded: {}",
                kinds
                    .iter()
                    .map(|(k, n)| format!("{k} ({n})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if series.is_empty() {
            notes.push(
                "no TimeSeries, ElectricalSeries or SpatialSeries under acquisition/ or processing/: nothing to return as traces"
                    .into(),
            );
        }
        if !tables.is_empty() {
            notes.push(format!(
                "{} tables: {}",
                tables.len(),
                tables
                    .iter()
                    .filter_map(|t| t.info.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(NwbDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            file,
            version,
            session,
            series,
            others,
            tables,
            electrodes,
            unreadable,
            notes,
        })
    }

    fn describe(file: &hdf5_pure::File, group: &str, neurodata_type: &str) -> Result<NwbSeries> {
        let data_path = format!("{group}/data");
        let d = file
            .dataset(&data_path)
            .map_err(|e| corrupt(format!("{data_path}: {e}")))?;
        let shape = d
            .shape()
            .map_err(|e| corrupt(format!("{data_path}: {e}")))?;
        let (samples, columns) = match shape.as_slice() {
            [n] => (*n, 1),
            [n, c] => (*n, *c),
            other => {
                return Err(Error::unsupported(
                    NWB_FORMAT_ID,
                    format!("{data_path}: shape {other:?}"),
                    "TimeSeries data of rank 1 or 2 are read.",
                ));
            }
        };
        if columns == 0 {
            return Err(corrupt(format!("{data_path}: no columns")));
        }
        let a = d.attrs().unwrap_or_default();
        let dtype = d.dtype().map(|t| dtype_name(&t)).unwrap_or_default();
        let (mut channel_conversion, mut electrode_rows) = (Vec::new(), Vec::new());
        if neurodata_type == "ElectricalSeries" {
            let cc = format!("{group}/channel_conversion");
            if file.dataset(&cc).is_ok() {
                channel_conversion = read_f64_rows(file, &cc, 0, columns)?;
                if channel_conversion.len() as u64 != columns {
                    return Err(corrupt(format!(
                        "{cc}: {} factors for {columns} channels",
                        channel_conversion.len()
                    )));
                }
            }
            let el = format!("{group}/electrodes");
            if let Ok(e) = file.dataset(&el) {
                let n = e.shape().ok().and_then(|s| s.first().copied()).unwrap_or(0);
                electrode_rows = read_f64_rows(file, &el, 0, n)?
                    .into_iter()
                    .map(|v| {
                        if v.is_finite() && v >= 0.0 {
                            v as u64
                        } else {
                            u64::MAX
                        }
                    })
                    .collect();
            }
        }
        let st_path = format!("{group}/starting_time");
        let (starting_time, rate) = match file.dataset(&st_path) {
            Ok(st) => (
                st.read_f64().ok().and_then(|v| v.first().copied()),
                st.attrs()
                    .ok()
                    .and_then(|a| attr_f64(&a, "rate"))
                    .filter(|r| *r > 0.0),
            ),
            Err(_) => (None, None),
        };
        let ts_path = format!("{group}/timestamps");
        let has_timestamps = file.dataset(&ts_path).is_ok();
        let uniform = if rate.is_none() && has_timestamps && samples <= MAX_READ_SAMPLES {
            uniform(&read_f64_rows(file, &ts_path, 0, samples)?)
        } else {
            None
        };
        let desc = file
            .group(group)
            .and_then(|g| g.attrs())
            .ok()
            .and_then(|a| a.get("description").and_then(attr_text))
            .filter(|s| s != "no description");
        Ok(NwbSeries {
            path: group.to_string(),
            samples,
            columns,
            dtype,
            unit: a.get("unit").and_then(attr_text),
            conversion: attr_f64(&a, "conversion").unwrap_or(1.0),
            offset: attr_f64(&a, "offset").unwrap_or(0.0),
            starting_time,
            rate,
            has_timestamps,
            uniform,
            description: desc,
            neurodata_type: neurodata_type.to_string(),
            channel_conversion,
            electrode_rows,
            icephys: if ICEPHYS_TYPES.contains(&neurodata_type) {
                icephys_fields(file, group)
            } else {
                BTreeMap::new()
            },
        })
    }

    fn trace_info(&self, i: usize, s: &NwbSeries) -> TraceInfo {
        let name = s.path.rsplit('/').next().unwrap_or(&s.path).to_string();
        let mut channels = Vec::new();
        if s.irregular() {
            channels.push(SignalChannelInfo {
                index: 0,
                name: "time".into(),
                unit: Some("s".into()),
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            });
        }
        for c in 0..s.columns {
            let mut extra = BTreeMap::new();
            if let Some(&row) = s.electrode_rows.get(c as usize) {
                extra.insert("electrode_row".into(), json!(row));
                if let Some(e) = usize::try_from(row)
                    .ok()
                    .and_then(|r| self.electrodes.get(r))
                {
                    extra.insert("electrode".into(), e.clone());
                }
            }
            let cc = s.channel_conversion.get(c as usize).copied();
            if let Some(f) = cc {
                extra.insert("channel_conversion".into(), json!(f));
            }
            channels.push(SignalChannelInfo {
                index: channels.len() as u32,
                name: if s.columns == 1 {
                    name.clone()
                } else {
                    format!("{name}[{c}]")
                },
                unit: s.unit.clone().filter(|u| u != "n/a"),
                dtype: s.dtype.clone(),
                scale: s.conversion * cc.unwrap_or(1.0),
                offset: s.offset,
                extra,
            });
        }
        let mut extra = BTreeMap::new();
        extra.insert("path".into(), json!(s.path));
        extra.insert("neurodata_type".into(), json!(s.neurodata_type));
        if let Some(d) = &s.description {
            extra.insert("description".into(), json!(d));
        }
        if let Some(t) = self.session.get("session_start_time") {
            extra.insert("session_start_time".into(), json!(t));
        }
        if let Some(n) = s.icephys.get("sweep_number") {
            extra.insert("sweep_number".into(), n.clone());
        }
        if !s.icephys.is_empty() {
            extra.insert("icephys".into(), json!(s.icephys));
        }
        let (rate, start) = if let Some(r) = s.rate {
            extra.insert("time_base".into(), json!("starting_time + rate"));
            (r, s.starting_time)
        } else if let Some((t0, dt)) = s.uniform {
            extra.insert("time_base".into(), json!("uniform timestamps"));
            (1.0 / dt, Some(t0))
        } else if s.has_timestamps {
            extra.insert("irregular_sampling".into(), json!(true));
            extra.insert("time_base".into(), json!("timestamps"));
            (0.0, None)
        } else {
            (0.0, None)
        };
        TraceInfo {
            index: i as u32,
            name: Some(name),
            sample_rate_hz: rate,
            sample_count: s.samples,
            sweep_count: 1,
            channels,
            start_s: start,
            extra,
        }
    }
}

impl NwbDataset {
    /// The session's own facts (NWB schema, `NWBFile`): `session_start_time` (with its zone),
    /// `general/experimenter`, `session_description`, `general/subject/subject_id`.
    fn session_experiment(&self) -> Option<openreadout_core::experiment::Experiment> {
        use openreadout_core::experiment::{Acquisition, Experiment, Origin, Sample};
        let get = |k: &str| {
            self.session
                .get(k)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let origin = |from: &str| Origin {
            source: Source::Spec,
            from: from.into(),
        };
        let mut e = Experiment::default();
        let mut a = Acquisition::default();
        if let Some(t) = get("session_start_time") {
            a.started_at = Some(t);
            e.provenance.insert(
                "acquisition.started_at".into(),
                origin("session_start_time"),
            );
        }
        if let Some(x) = get("general/experimenter") {
            a.operator = Some(x);
            e.provenance.insert(
                "acquisition.operator".into(),
                origin("general/experimenter"),
            );
        }
        if let Some(d) = get("session_description") {
            a.comment = Some(d);
            e.provenance
                .insert("acquisition.comment".into(), origin("session_description"));
        }
        if a != Acquisition::default() {
            e.acquisition = Some(a);
        }
        if let Some(s) = get("general/subject/subject_id") {
            e.sample = Some(Sample {
                id: Some(s),
                source_field: Some("general/subject/subject_id".into()),
                ..Sample::default()
            });
            e.provenance
                .insert("sample.id".into(), origin("general/subject/subject_id"));
        }
        (e != Experiment::default()).then_some(e)
    }
}

impl Dataset for NwbDataset {
    fn experiment(&self) -> Option<openreadout_core::experiment::Experiment> {
        self.session_experiment()
    }

    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: NwbReader.descriptor(),
            format_version: self.version.clone(),
            images: Vec::new(),
            tables: self.tables.iter().map(|t| t.info.clone()).collect(),
            spectra: Vec::new(),
            traces: self
                .series
                .iter()
                .enumerate()
                .map(|(i, s)| self.trace_info(i, s))
                .collect(),
            plane_count: 0,
            notes: self.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let root = self
            .file
            .root()
            .attrs()
            .map(|a| attrs_json(&a))
            .unwrap_or_default();
        Ok(json!({
            "root_attributes": root,
            "session": self.session,
            "objects": self.others.iter().map(|(p, t)| json!({"path": p, "neurodata_type": t})).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "format_version",
            "traces[].sample_rate_hz",
            "traces[].sample_count",
            "traces[].channels",
            "traces[].start_s",
            "traces[].channels[].extra.electrode",
            "tables[].columns",
            "tables[].row_count",
        ] {
            p.insert(k.into(), Source::Spec);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (i, s) in self.series.iter().enumerate() {
            out.push(LsEntry {
                kind: "trace".into(),
                name: s.path.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({"trace": i, "samples": s.samples, "columns": s.columns, "dtype": s.dtype, "unit": s.unit, "rate": s.rate, "timestamps": s.has_timestamps}),
            });
        }
        for t in &self.tables {
            out.push(LsEntry {
                kind: "table".into(),
                name: t.path.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({"table": t.info.index, "rows": t.info.row_count, "columns": t.info.columns.len()}),
            });
        }
        for (p, t) in &self.others {
            out.push(LsEntry {
                kind: "object".into(),
                name: p.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({"neurodata_type": t, "decoded": false}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            NWB_FORMAT_ID,
            "images",
            "NWB files are read as traces (`trace`, `export --to csv`); ImageSeries are listed, not decoded.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let s = self.series.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (0..{})",
                self.series.len()
            ))
        })?;
        if sweep != 0 {
            return Err(Error::Usage(format!("trace {index} has one sweep (0)")));
        }
        let first = first_sample.min(s.samples);
        let n = max_samples.min(s.samples - first).min(MAX_READ_SAMPLES);
        let raw = read_f64_rows(&self.file, &format!("{}/data", s.path), first, n)?;
        let cols = s.columns as usize;
        if raw.len() != n as usize * cols {
            return Err(corrupt(format!(
                "{}/data: read {} values, expected {}",
                s.path,
                raw.len(),
                n as usize * cols
            )));
        }
        let mut channels = Vec::new();
        if s.irregular() {
            channels.push(read_f64_rows(
                &self.file,
                &format!("{}/timestamps", s.path),
                first,
                n,
            )?);
        }
        for c in 0..cols {
            channels.push(
                raw.iter()
                    .skip(c)
                    .step_by(cols)
                    .map(|v| {
                        let v = v * s.conversion;
                        match s.channel_conversion.get(c) {
                            Some(f) => v * f + s.offset,
                            None => v + s.offset,
                        }
                    })
                    .collect(),
            );
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let t = self.tables.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {index} out of range (file has {} tables)",
                self.tables.len()
            ))
        })?;
        t.read(&self.file, first_row, max_rows)
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), NWB_FORMAT_ID);
        r.performed(
            "HDF5 structure parses; the root is an NWBFile with the required session fields",
        );
        r.performed("every TimeSeries, ElectricalSeries and SpatialSeries under acquisition/ and processing/ has rank-1/2 numeric data, a time base (starting_time + rate, or timestamps of the same length), and its last sample is readable");
        r.performed("ElectricalSeries electrode rows fall inside the electrodes table; channel_conversion has one factor per channel");
        r.performed("every table's last row is readable; ragged indices are non-decreasing offsets inside their data");
        for u in &self.unreadable {
            r.push(Finding::error("unreadable", u.clone()));
        }
        let electrode_count = table_rows(&self.file, ELECTRODES_PATH);
        for s in &self.series {
            if let Some(bad) = s
                .electrode_rows
                .iter()
                .find(|row| electrode_count.is_none_or(|n| **row >= n))
            {
                r.push(Finding::error(
                    "electrodes",
                    format!(
                        "{}: electrode row {bad} outside the electrodes table ({} rows)",
                        s.path,
                        electrode_count.unwrap_or(0)
                    ),
                ));
            }
            if !s.electrode_rows.is_empty() && s.electrode_rows.len() as u64 != s.columns {
                r.push(Finding::warning(
                    "electrodes",
                    format!(
                        "{}: {} electrode rows for {} channels",
                        s.path,
                        s.electrode_rows.len(),
                        s.columns
                    ),
                ));
            }
        }
        for t in &self.tables {
            if t.info.row_count > 0
                && let Err(e) = t.read(&self.file, t.info.row_count - 1, 1)
            {
                r.push(Finding::error(
                    "table",
                    format!("{}: last row: {e}", t.path),
                ));
            }
        }
        for f in ["session_description", "identifier", "session_start_time"] {
            if !self.session.contains_key(f) && self.file.dataset(f).is_err() {
                r.push(Finding::warning(
                    "session",
                    format!("required field {f} is missing"),
                ));
            }
        }
        let flen = self.fs.metadata(&self.path).map_or(0, |m| m.len());
        let sb = self.file.superblock();
        let eof = sb.base_address.get().saturating_add(sb.eof_address);
        if eof > flen {
            r.push(Finding::error(
                "truncated",
                format!("file is {flen} bytes but its HDF5 superblock says it ends at {eof}"),
            ));
        }
        for s in self.series.clone() {
            if s.rate.is_none() && !s.has_timestamps {
                r.push(Finding::warning(
                    "time_base",
                    format!("{}: neither starting_time/rate nor timestamps", s.path),
                ));
            }
            if s.has_timestamps {
                let n = self
                    .file
                    .dataset(&format!("{}/timestamps", s.path))
                    .and_then(|d| d.shape())
                    .ok()
                    .and_then(|v| v.first().copied());
                if n != Some(s.samples) {
                    r.push(Finding::error(
                        "timestamps",
                        format!("{}: {n:?} timestamps for {} samples", s.path, s.samples),
                    ));
                }
            }
            if s.samples > 0
                && let Err(e) =
                    read_f64_rows(&self.file, &format!("{}/data", s.path), s.samples - 1, 1)
            {
                r.push(Finding::error(
                    "data",
                    format!("{}: last sample: {e}", s.path),
                ));
            }
        }
        Ok(r)
    }
}

/// Electrodes-table rows as JSON objects (`row`, `id` and every numeric or text column).
fn electrode_rows_json(file: &hdf5_pure::File, tables: &[NwbTable]) -> Vec<Value> {
    let Some(t) = tables.iter().find(|t| t.path == ELECTRODES_PATH) else {
        return Vec::new();
    };
    let rows = t.info.row_count.min(1 << 16);
    let Ok(tab) = t.read(file, 0, rows) else {
        return Vec::new();
    };
    (0..rows as usize)
        .map(|r| {
            let mut o = serde_json::Map::new();
            o.insert("row".into(), json!(r));
            for (c, col) in t.info.columns.iter().zip(&tab.columns) {
                let v = col.get(r).copied().unwrap_or(f64::NAN);
                let value = match c.extra.get("categories").and_then(Value::as_array) {
                    Some(cats) => cats.get(v as usize).cloned().unwrap_or(Value::Null),
                    None if v.is_finite() => json!(v),
                    None => Value::Null,
                };
                o.insert(c.name.clone(), value);
            }
            Value::Object(o)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_timestamps() {
        assert_eq!(uniform(&[1.0, 1.5, 2.0, 2.5]), Some((1.0, 0.5)));
        assert_eq!(uniform(&[1.0, 1.5, 2.1]), None);
        assert_eq!(uniform(&[1.0]), None);
        assert_eq!(uniform(&[1.0, 1.0]), None);
    }
}
