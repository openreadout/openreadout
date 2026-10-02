//! Record-batch sources: a table, the sweeps of a trace, or the spectra of a run, read from a
//! [`Dataset`] a bounded chunk at a time.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::types::Int32Type;
use arrow_array::{
    Array, ArrayRef, BooleanArray, DictionaryArray, Float32Array, Float64Array, Int8Array,
    Int16Array, Int32Array, Int64Array, RecordBatch, StringArray, UInt8Array, UInt16Array,
    UInt32Array, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use openreadout_core::experiment::InfoOutput;
use openreadout_core::model::{ColumnInfo, TableInfo, TraceInfo};
use openreadout_core::provenance::ProvenanceMap;
use openreadout_core::{Dataset, Error, Result, Spectrum, SpectrumView};
use serde_json::{Value, json};

use crate::meta::{
    KEY_DTYPE, KEY_EXTRA, KEY_LABEL, KEY_PROVENANCE, KEY_SCALING, KEY_UNIT, Part, file_metadata,
    provenance_json, put,
};

/// Table rows or trace samples read per batch.
const CHUNK_ROWS: u64 = 65_536;
/// Spectrum points gathered before a batch is emitted.
const CHUNK_POINTS: usize = 1 << 18;

/// Produces the record batches of one export.
pub(crate) trait BatchSource {
    fn schema(&self) -> SchemaRef;
    fn next_batch(&mut self, ds: &mut dyn Dataset) -> Result<Option<RecordBatch>>;
    /// Rows of the source that will be written in total (for progress and reports).
    fn total_rows(&self) -> u64;
}

fn arrow_err(e: arrow_schema::ArrowError) -> Error {
    Error::Other(format!("building an Arrow batch: {e}"))
}

/// Unique column names: a repeated name gets `_1`, `_2`, ... appended.
fn dedupe(names: &mut [String]) {
    let mut seen: HashMap<String, u32> = HashMap::new();
    for n in names.iter_mut() {
        let mut candidate = n.clone();
        while let Some(k) = seen.get_mut(&candidate) {
            *k += 1;
            candidate = format!("{n}_{k}");
        }
        seen.insert(candidate.clone(), 0);
        *n = candidate;
    }
}

// ---------------------------------------------------------------------------------------------
// tables
// ---------------------------------------------------------------------------------------------

/// How one table column is stored.
#[derive(Debug, Clone)]
enum ColKind {
    F32,
    F64,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    /// Values index `extra.categories`; stored as a dictionary of those names.
    Categories(Arc<StringArray>),
}

impl ColKind {
    fn of(c: &ColumnInfo) -> Self {
        if let Some(cats) = c.extra.get("categories").and_then(Value::as_array) {
            let names: Vec<Option<String>> = cats
                .iter()
                .map(|v| Some(v.as_str().map_or_else(|| v.to_string(), str::to_string)))
                .collect();
            if i32::try_from(names.len()).is_ok() {
                return ColKind::Categories(Arc::new(StringArray::from(names)));
            }
        }
        match c.dtype.as_str() {
            "float32" => ColKind::F32,
            "int8" => ColKind::I8,
            "int16" => ColKind::I16,
            "int32" => ColKind::I32,
            "int64" => ColKind::I64,
            "uint8" => ColKind::U8,
            "uint16" => ColKind::U16,
            "uint32" => ColKind::U32,
            "uint64" => ColKind::U64,
            _ => ColKind::F64,
        }
    }

    fn data_type(&self) -> DataType {
        match self {
            ColKind::F32 => DataType::Float32,
            ColKind::F64 => DataType::Float64,
            ColKind::I8 => DataType::Int8,
            ColKind::I16 => DataType::Int16,
            ColKind::I32 => DataType::Int32,
            ColKind::I64 => DataType::Int64,
            ColKind::U8 => DataType::UInt8,
            ColKind::U16 => DataType::UInt16,
            ColKind::U32 => DataType::UInt32,
            ColKind::U64 => DataType::UInt64,
            ColKind::Categories(_) => {
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
            }
        }
    }
}

/// `v` as an integer in `[lo, hi]`: `Ok(None)` for NaN (written as null), an error when `v` is
/// not an integer in range (the reader's dtype does not hold its values).
fn int_value(v: f64, lo: f64, hi: f64, name: &str, dtype: &str) -> Result<Option<f64>> {
    if v.is_nan() {
        return Ok(None);
    }
    if v.fract() == 0.0 && v >= lo && v <= hi {
        return Ok(Some(v));
    }
    Err(Error::Other(format!(
        "column {name:?}: value {v} is not a {dtype}; the column cannot be written losslessly"
    )))
}

macro_rules! ints {
    ($vals:expr, $arr:ty, $t:ty, $name:expr, $dtype:expr) => {{
        let mut out: Vec<Option<$t>> = Vec::with_capacity($vals.len());
        for &v in $vals {
            out.push(
                int_value(v, <$t>::MIN as f64, <$t>::MAX as f64, $name, $dtype)?.map(|x| x as $t),
            );
        }
        Arc::new(<$arr>::from(out)) as ArrayRef
    }};
}

fn column_array(kind: &ColKind, vals: &[f64], name: &str) -> Result<ArrayRef> {
    Ok(match kind {
        ColKind::F64 => Arc::new(Float64Array::from(vals.to_vec())),
        ColKind::F32 => {
            let mut out = Vec::with_capacity(vals.len());
            for &v in vals {
                let f = v as f32;
                if f64::from(f).to_bits() != v.to_bits() && !v.is_nan() {
                    return Err(Error::Other(format!(
                        "column {name:?}: value {v} is not a float32; the column cannot be written losslessly"
                    )));
                }
                out.push(f);
            }
            Arc::new(Float32Array::from(out))
        }
        ColKind::I8 => ints!(vals, Int8Array, i8, name, "int8"),
        ColKind::I16 => ints!(vals, Int16Array, i16, name, "int16"),
        ColKind::I32 => ints!(vals, Int32Array, i32, name, "int32"),
        // Values arrive as f64: the i64/u64 bounds are the nearest representable limits.
        ColKind::I64 => {
            let mut out: Vec<Option<i64>> = Vec::with_capacity(vals.len());
            for &v in vals {
                out.push(
                    int_value(
                        v,
                        -9_223_372_036_854_775_808.0,
                        9_223_372_036_854_774_784.0,
                        name,
                        "int64",
                    )?
                    .map(|x| x as i64),
                );
            }
            Arc::new(Int64Array::from(out))
        }
        ColKind::U8 => ints!(vals, UInt8Array, u8, name, "uint8"),
        ColKind::U16 => ints!(vals, UInt16Array, u16, name, "uint16"),
        ColKind::U32 => ints!(vals, UInt32Array, u32, name, "uint32"),
        ColKind::U64 => {
            let mut out: Vec<Option<u64>> = Vec::with_capacity(vals.len());
            for &v in vals {
                out.push(
                    int_value(v, 0.0, 18_446_744_073_709_549_568.0, name, "uint64")?
                        .map(|x| x as u64),
                );
            }
            Arc::new(UInt64Array::from(out))
        }
        ColKind::Categories(names) => {
            let n = names.len() as f64;
            let mut keys: Vec<Option<i32>> = Vec::with_capacity(vals.len());
            for &v in vals {
                keys.push(if v >= 0.0 && v < n && v.fract() == 0.0 {
                    Some(v as i32)
                } else if v.is_nan() {
                    None
                } else {
                    return Err(Error::Other(format!(
                        "column {name:?}: value {v} indexes no entry of extra.categories"
                    )));
                });
            }
            Arc::new(
                DictionaryArray::<Int32Type>::try_new(
                    Int32Array::from(keys),
                    names.clone() as ArrayRef,
                )
                .map_err(arrow_err)?,
            )
        }
    })
}

/// One table, rows `[first, end)`.
pub(crate) struct TableSource {
    schema: SchemaRef,
    table: u32,
    kinds: Vec<ColKind>,
    names: Vec<String>,
    pos: u64,
    first: u64,
    end: u64,
}

impl TableSource {
    pub(crate) fn new(
        info: &InfoOutput,
        prov: &ProvenanceMap,
        t: &TableInfo,
        (first, end): (u64, u64),
        source_file: &str,
    ) -> Self {
        let part = Part {
            root: "tables",
            index: t.index,
            kind: None,
        };
        let col_prov = provenance_json(prov, part, &[".columns"]);
        let kinds: Vec<ColKind> = t.columns.iter().map(ColKind::of).collect();
        let mut names: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
        dedupe(&mut names);
        let fields: Vec<Field> = t
            .columns
            .iter()
            .zip(&kinds)
            .zip(&names)
            .map(|((c, k), n)| {
                let mut m = HashMap::new();
                put(&mut m, KEY_UNIT, c.unit.clone());
                put(&mut m, KEY_LABEL, c.label.clone());
                put(&mut m, KEY_DTYPE, Some(c.dtype.clone()));
                let mut extra = c.extra.clone();
                if let Some(r) = c.range {
                    extra.insert("range".into(), json!(r));
                }
                if !extra.is_empty() {
                    put(&mut m, KEY_EXTRA, serde_json::to_string(&extra).ok());
                }
                put(&mut m, KEY_PROVENANCE, col_prov.clone());
                // Integer columns carry NaN as null (e.g. an FCS value outside its range mask).
                Field::new(n, k.data_type(), true).with_metadata(m)
            })
            .collect();
        let mut object = serde_json::to_value(t).unwrap_or(Value::Null);
        if let Some(o) = object.as_object_mut() {
            o.insert("first_row".into(), json!(first));
            o.insert("rows_written".into(), json!(end - first));
        }
        let meta = file_metadata(info, prov, part, "table", &object, source_file);
        TableSource {
            schema: Arc::new(Schema::new_with_metadata(fields, meta)),
            table: t.index,
            kinds,
            names,
            pos: first,
            first,
            end,
        }
    }
}

impl BatchSource for TableSource {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn total_rows(&self) -> u64 {
        self.end - self.first
    }

    fn next_batch(&mut self, ds: &mut dyn Dataset) -> Result<Option<RecordBatch>> {
        if self.pos >= self.end {
            return Ok(None);
        }
        let n = CHUNK_ROWS.min(self.end - self.pos);
        let chunk = ds.read_table(self.table, self.pos, n)?;
        let rows = chunk.columns.first().map_or(0, Vec::len);
        if chunk.columns.len() != self.kinds.len() || rows as u64 != n {
            return Err(Error::Other(format!(
                "reader returned {rows} rows x {} columns for a request of {n} x {}",
                chunk.columns.len(),
                self.kinds.len()
            )));
        }
        let cols = chunk
            .columns
            .iter()
            .zip(&self.kinds)
            .zip(&self.names)
            .map(|((v, k), name)| column_array(k, v, name))
            .collect::<Result<Vec<_>>>()?;
        self.pos += n;
        RecordBatch::try_new(self.schema.clone(), cols)
            .map(Some)
            .map_err(arrow_err)
    }
}

// ---------------------------------------------------------------------------------------------
// traces
// ---------------------------------------------------------------------------------------------

/// The abscissa column of a trace export.
#[derive(Debug, Clone)]
enum Axis {
    /// `first + i * step` (`chemical_shift_ppm`, `wavenumber_1/CM`, ...).
    Regular { first: f64, step: f64 },
    /// `origin + i / rate`, seconds on the clock `trace` reports (`start_s`): the trace's
    /// start for a single sweep, the sweep start for multi-sweep traces.
    Time { origin: f64, rate: f64 },
    /// The sample index, when neither is known.
    Sample,
}

/// A trace's regular abscissa from `extra.axis` when it is not time: `(first, step, quantity,
/// unit)`, as the CSV export names it (`<quantity>_<unit>`).
fn regular_axis(t: &TraceInfo) -> Option<(f64, f64, String, Option<String>)> {
    let a = t.extra.get("axis")?;
    let quantity = a.get("quantity").and_then(Value::as_str).unwrap_or("x");
    if quantity == "time" {
        return None;
    }
    let first = a.get("first")?.as_f64()?;
    let step = a.get("step")?.as_f64()?;
    let unit = a
        .get("unit")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .map(str::to_string);
    Some((first, step, quantity.to_string(), unit))
}

/// The sweeps of one trace, samples `[first, last]` of each (clipped to the sweep's length).
pub(crate) struct TraceSource {
    schema: SchemaRef,
    trace: u32,
    axis: Axis,
    nch: usize,
    /// `(sweep, first, end)` still to write, in order.
    sweeps: Vec<(u32, u64, u64)>,
    cur: usize,
    pos: u64,
    total: u64,
}

impl TraceSource {
    pub(crate) fn new(
        info: &InfoOutput,
        prov: &ProvenanceMap,
        t: &TraceInfo,
        sweeps: &[u32],
        rows: (u64, Option<u64>),
        source_file: &str,
    ) -> Self {
        let kind = t.extra.get("kind").and_then(Value::as_str);
        let part = Part {
            root: "traces",
            index: t.index,
            kind,
        };
        let mut spans = Vec::new();
        let mut total = 0u64;
        for &s in sweeps {
            let n = openreadout_core::trace::sweep_samples(t, s);
            let first = rows.0.min(n);
            let end = rows.1.map_or(n, |l| l.saturating_add(1).min(n));
            let end = end.max(first);
            total += end - first;
            spans.push((s, first, end));
        }
        let (axis, axis_name, axis_unit) = match regular_axis(t) {
            Some((first, step, quantity, unit)) => (
                Axis::Regular { first, step },
                match &unit {
                    Some(u) => format!("{quantity}_{u}"),
                    None => quantity,
                },
                unit,
            ),
            None if t.sample_rate_hz > 0.0 => (
                Axis::Time {
                    origin: openreadout_core::trace::sweep_origin_s(t),
                    rate: t.sample_rate_hz,
                },
                "time_s".to_string(),
                Some("s".to_string()),
            ),
            None => (Axis::Sample, "sample".to_string(), None),
        };
        let mut names = vec!["sweep".to_string(), axis_name];
        names.extend(t.channels.iter().map(|c| c.name.clone()));
        dedupe(&mut names);
        let mut fields = Vec::with_capacity(names.len());
        let mut m = HashMap::new();
        put(
            &mut m,
            KEY_LABEL,
            Some("sweep (episode, segment, page) index, zero-based".into()),
        );
        if let Some(sa) = t.extra.get("sweep_axis") {
            put(
                &mut m,
                KEY_EXTRA,
                Some(json!({ "sweep_axis": sa }).to_string()),
            );
        }
        fields.push(Field::new(&names[0], DataType::UInt32, false).with_metadata(m));
        let mut m = HashMap::new();
        put(&mut m, KEY_UNIT, axis_unit);
        put(
            &mut m,
            KEY_LABEL,
            Some(match axis {
                Axis::Regular { .. } => "abscissa: first + i * step (extra.axis)".into(),
                Axis::Time { .. } => {
                    "time: start_s (single-sweep traces) + sample index / sample rate".into()
                }
                Axis::Sample => "sample index within the sweep".into(),
            }),
        );
        if let Some(a) = t.extra.get("axis") {
            put(&mut m, KEY_EXTRA, Some(json!({ "axis": a }).to_string()));
        }
        put(
            &mut m,
            KEY_PROVENANCE,
            provenance_json(prov, part, &[".extra.axis", ".sample_rate_hz", ".start_s"]),
        );
        fields.push(
            Field::new(
                &names[1],
                match axis {
                    Axis::Sample => DataType::UInt64,
                    _ => DataType::Float64,
                },
                false,
            )
            .with_metadata(m),
        );
        let ch_prov = provenance_json(prov, part, &[".channels"]);
        for (c, n) in t.channels.iter().zip(&names[2..]) {
            let mut m = HashMap::new();
            put(&mut m, KEY_UNIT, c.unit.clone());
            put(&mut m, KEY_DTYPE, Some(c.dtype.clone()));
            put(
                &mut m,
                KEY_SCALING,
                Some(json!([c.scale, c.offset]).to_string()),
            );
            if !c.extra.is_empty() {
                put(&mut m, KEY_EXTRA, serde_json::to_string(&c.extra).ok());
            }
            put(&mut m, KEY_PROVENANCE, ch_prov.clone());
            fields.push(Field::new(n, DataType::Float64, true).with_metadata(m));
        }
        let mut object = serde_json::to_value(t).unwrap_or(Value::Null);
        if let Some(o) = object.as_object_mut() {
            o.insert("sweeps_written".into(), json!(sweeps));
            o.insert("first_sample".into(), json!(rows.0));
        }
        let meta = file_metadata(info, prov, part, "trace", &object, source_file);
        TraceSource {
            schema: Arc::new(Schema::new_with_metadata(fields, meta)),
            trace: t.index,
            axis,
            nch: t.channels.len(),
            pos: spans.first().map_or(0, |s| s.1),
            sweeps: spans,
            cur: 0,
            total,
        }
    }
}

impl BatchSource for TraceSource {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn total_rows(&self) -> u64 {
        self.total
    }

    fn next_batch(&mut self, ds: &mut dyn Dataset) -> Result<Option<RecordBatch>> {
        loop {
            let Some(&(sweep, _, end)) = self.sweeps.get(self.cur) else {
                return Ok(None);
            };
            if self.pos >= end {
                self.cur += 1;
                if let Some(next) = self.sweeps.get(self.cur) {
                    self.pos = next.1;
                }
                continue;
            }
            let n = CHUNK_ROWS.min(end - self.pos);
            let chunk = ds.read_trace(self.trace, sweep, self.pos, n)?;
            let rows = chunk.channels.first().map_or(0, Vec::len);
            if chunk.channels.len() != self.nch || (self.nch > 0 && rows as u64 != n) {
                return Err(Error::Other(format!(
                    "reader returned {rows} samples x {} channels for a request of {n} x {}",
                    chunk.channels.len(),
                    self.nch
                )));
            }
            let n_us = n as usize;
            let mut cols: Vec<ArrayRef> = Vec::with_capacity(self.nch + 2);
            cols.push(Arc::new(UInt32Array::from(vec![sweep; n_us])));
            let start = self.pos;
            cols.push(match self.axis {
                Axis::Regular { first, step } => Arc::new(Float64Array::from_iter_values(
                    (start..start + n).map(|i| first + step * i as f64),
                )),
                Axis::Time { origin, rate } => Arc::new(Float64Array::from_iter_values(
                    (start..start + n).map(|i| origin + i as f64 / rate),
                )),
                Axis::Sample => Arc::new(UInt64Array::from_iter_values(start..start + n)),
            });
            for ch in chunk.channels {
                cols.push(Arc::new(Float64Array::from(ch)));
            }
            self.pos += n;
            return RecordBatch::try_new(self.schema.clone(), cols)
                .map(Some)
                .map_err(arrow_err);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// spectra
// ---------------------------------------------------------------------------------------------

/// Per-scan summary rows, gathered while the points are streamed.
#[derive(Debug, Default)]
struct ScanRows {
    index: Vec<u64>,
    scan: Vec<u64>,
    native_id: Vec<Option<String>>,
    ms_level: Vec<u32>,
    rt_s: Vec<Option<f64>>,
    polarity: Vec<String>,
    centroided: Vec<bool>,
    precursor_mz: Vec<Option<f64>>,
    precursor_charge: Vec<Option<i32>>,
    precursor_intensity: Vec<Option<f64>>,
    isolation_lower_mz: Vec<Option<f64>>,
    isolation_upper_mz: Vec<Option<f64>>,
    activation: Vec<Option<String>>,
    collision_energy: Vec<Option<f64>>,
    inverse_reduced_mobility: Vec<Option<f64>>,
    scan_window_lower_mz: Vec<Option<f64>>,
    scan_window_upper_mz: Vec<Option<f64>>,
    total_ion_current: Vec<Option<f64>>,
    base_peak_mz: Vec<Option<f64>>,
    base_peak_intensity: Vec<Option<f64>>,
    scan_filter: Vec<Option<String>>,
    point_count: Vec<u64>,
}

impl ScanRows {
    fn push(&mut self, s: &Spectrum) {
        self.index.push(s.index);
        self.scan.push(s.scan_number);
        self.native_id.push(s.native_id.clone());
        self.ms_level.push(s.ms_level);
        self.rt_s.push(s.rt_s);
        self.polarity.push(s.polarity.clone());
        self.centroided.push(s.centroided);
        self.precursor_mz.push(s.precursor_mz);
        self.precursor_charge.push(s.precursor_charge);
        self.precursor_intensity.push(s.precursor_intensity);
        self.isolation_lower_mz
            .push(s.isolation_window_mz.map(|w| w[0]));
        self.isolation_upper_mz
            .push(s.isolation_window_mz.map(|w| w[1]));
        self.activation.push(s.activation.clone());
        self.collision_energy.push(s.collision_energy);
        self.inverse_reduced_mobility
            .push(s.inverse_reduced_mobility);
        self.scan_window_lower_mz
            .push(s.scan_window_mz.map(|w| w[0]));
        self.scan_window_upper_mz
            .push(s.scan_window_mz.map(|w| w[1]));
        self.total_ion_current.push(s.total_ion_current);
        self.base_peak_mz.push(s.base_peak_mz);
        self.base_peak_intensity.push(s.base_peak_intensity);
        self.scan_filter.push(s.scan_filter.clone());
        self.point_count.push(s.mz.len() as u64);
    }
}

fn field(name: &str, dt: DataType, nullable: bool, unit: Option<&str>, label: &str) -> Field {
    let mut m = HashMap::new();
    put(&mut m, KEY_UNIT, unit.map(str::to_string));
    put(&mut m, KEY_LABEL, Some(label.to_string()));
    Field::new(name, dt, nullable).with_metadata(m)
}

/// The spectra of one run: one row per point (long form), and a per-scan summary.
pub(crate) struct SpectraSource {
    schema: SchemaRef,
    summary_schema: SchemaRef,
    run: u32,
    view: SpectrumView,
    next: u64,
    count: u64,
    scans: ScanRows,
    points: u64,
}

impl SpectraSource {
    pub(crate) fn new(
        info: &InfoOutput,
        prov: &ProvenanceMap,
        run: u32,
        centroid: bool,
        source_file: &str,
    ) -> Result<Self> {
        let s = info
            .spectra
            .iter()
            .find(|s| s.index == run)
            .ok_or_else(|| {
                Error::Usage(format!(
                    "--run {run} out of range (file has {} spectra runs)",
                    info.spectra.len()
                ))
            })?;
        let part = Part {
            root: "spectra",
            index: run,
            kind: None,
        };
        let view_name = if centroid { "centroid" } else { "primary" };
        let mut object = serde_json::to_value(s).unwrap_or(Value::Null);
        if let Some(o) = object.as_object_mut() {
            o.insert("view".into(), json!(view_name));
        }
        let with_prov = |mut f: Field, keys: &[&str]| {
            if let Some(p) = provenance_json(prov, part, keys) {
                f.metadata_mut().insert(KEY_PROVENANCE, p);
            }
            f
        };
        let fields = vec![
            field(
                "scan",
                DataType::UInt64,
                false,
                None,
                "instrument scan number (see `openreadout spectra --scan`)",
            ),
            with_prov(
                field(
                    "rt_s",
                    DataType::Float64,
                    true,
                    Some("s"),
                    "retention time (null when the file states none)",
                ),
                &[".rt_range_s"],
            ),
            field(
                "ms_level",
                DataType::UInt32,
                false,
                None,
                "MS level (1 = full scan, 2 = MS/MS, ...)",
            ),
            field(
                "precursor_mz",
                DataType::Float64,
                true,
                Some("m/z"),
                "precursor m/z (MS2 and above)",
            ),
            field(
                "mz",
                DataType::Float64,
                false,
                Some("m/z"),
                "mass-to-charge ratio of the point",
            ),
            field(
                "intensity",
                DataType::Float32,
                false,
                None,
                "intensity of the point",
            ),
        ];
        let meta = file_metadata(info, prov, part, "spectra", &object, source_file);
        let summary = vec![
            field(
                "index",
                DataType::UInt64,
                false,
                None,
                "zero-based spectrum index in the run",
            ),
            field(
                "scan",
                DataType::UInt64,
                false,
                None,
                "instrument scan number",
            ),
            field(
                "native_id",
                DataType::Utf8,
                true,
                None,
                "the spectrum's identifier in its file",
            ),
            field("ms_level", DataType::UInt32, false, None, "MS level"),
            field(
                "rt_s",
                DataType::Float64,
                true,
                Some("s"),
                "retention time (null when the file states none)",
            ),
            field(
                "polarity",
                DataType::Utf8,
                false,
                None,
                "positive, negative or unknown",
            ),
            field(
                "centroided",
                DataType::Boolean,
                false,
                None,
                "true for centroid peaks, false for a profile",
            ),
            field(
                "precursor_mz",
                DataType::Float64,
                true,
                Some("m/z"),
                "precursor m/z",
            ),
            field(
                "precursor_charge",
                DataType::Int32,
                true,
                None,
                "precursor charge state",
            ),
            field(
                "precursor_intensity",
                DataType::Float64,
                true,
                None,
                "precursor intensity",
            ),
            field(
                "isolation_lower_mz",
                DataType::Float64,
                true,
                Some("m/z"),
                "isolation window lower bound",
            ),
            field(
                "isolation_upper_mz",
                DataType::Float64,
                true,
                Some("m/z"),
                "isolation window upper bound",
            ),
            field(
                "activation",
                DataType::Utf8,
                true,
                None,
                "dissociation method (CID, HCD, ...)",
            ),
            field(
                "collision_energy",
                DataType::Float64,
                true,
                None,
                "collision energy as recorded",
            ),
            field(
                "inverse_reduced_mobility",
                DataType::Float64,
                true,
                Some("V·s/cm²"),
                "ion mobility 1/K0",
            ),
            field(
                "scan_window_lower_mz",
                DataType::Float64,
                true,
                Some("m/z"),
                "scan window lower bound",
            ),
            field(
                "scan_window_upper_mz",
                DataType::Float64,
                true,
                Some("m/z"),
                "scan window upper bound",
            ),
            field(
                "total_ion_current",
                DataType::Float64,
                true,
                None,
                "sum of intensities (TIC)",
            ),
            field(
                "base_peak_mz",
                DataType::Float64,
                true,
                Some("m/z"),
                "m/z of the most intense peak",
            ),
            field(
                "base_peak_intensity",
                DataType::Float64,
                true,
                None,
                "intensity of the most intense peak",
            ),
            field(
                "scan_filter",
                DataType::Utf8,
                true,
                None,
                "instrument scan filter",
            ),
            field(
                "point_count",
                DataType::UInt64,
                false,
                None,
                "points of the spectrum in the points file",
            ),
        ];
        let summary_meta = file_metadata(info, prov, part, "scans", &object, source_file);
        Ok(SpectraSource {
            schema: Arc::new(Schema::new_with_metadata(fields, meta)),
            summary_schema: Arc::new(Schema::new_with_metadata(summary, summary_meta)),
            run,
            view: if centroid {
                SpectrumView::Centroid
            } else {
                SpectrumView::Primary
            },
            next: 0,
            count: s.scan_count,
            scans: ScanRows::default(),
            points: 0,
        })
    }

    pub(crate) fn spectra_read(&self) -> u64 {
        self.next
    }

    pub(crate) fn summary_schema(&self) -> SchemaRef {
        self.summary_schema.clone()
    }

    /// The per-scan table (call after the points are exhausted).
    pub(crate) fn summary_batch(&self) -> Result<RecordBatch> {
        let r = &self.scans;
        let cols: Vec<ArrayRef> = vec![
            Arc::new(UInt64Array::from(r.index.clone())),
            Arc::new(UInt64Array::from(r.scan.clone())),
            Arc::new(StringArray::from(r.native_id.clone())),
            Arc::new(UInt32Array::from(r.ms_level.clone())),
            Arc::new(Float64Array::from(r.rt_s.clone())),
            Arc::new(StringArray::from(r.polarity.clone())),
            Arc::new(BooleanArray::from(r.centroided.clone())),
            Arc::new(Float64Array::from(r.precursor_mz.clone())),
            Arc::new(Int32Array::from(r.precursor_charge.clone())),
            Arc::new(Float64Array::from(r.precursor_intensity.clone())),
            Arc::new(Float64Array::from(r.isolation_lower_mz.clone())),
            Arc::new(Float64Array::from(r.isolation_upper_mz.clone())),
            Arc::new(StringArray::from(r.activation.clone())),
            Arc::new(Float64Array::from(r.collision_energy.clone())),
            Arc::new(Float64Array::from(r.inverse_reduced_mobility.clone())),
            Arc::new(Float64Array::from(r.scan_window_lower_mz.clone())),
            Arc::new(Float64Array::from(r.scan_window_upper_mz.clone())),
            Arc::new(Float64Array::from(r.total_ion_current.clone())),
            Arc::new(Float64Array::from(r.base_peak_mz.clone())),
            Arc::new(Float64Array::from(r.base_peak_intensity.clone())),
            Arc::new(StringArray::from(r.scan_filter.clone())),
            Arc::new(UInt64Array::from(r.point_count.clone())),
        ];
        RecordBatch::try_new(self.summary_schema.clone(), cols).map_err(arrow_err)
    }
}

impl BatchSource for SpectraSource {
    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    /// Spectra (not points) for progress; points are only known once read.
    fn total_rows(&self) -> u64 {
        self.points
    }

    fn next_batch(&mut self, ds: &mut dyn Dataset) -> Result<Option<RecordBatch>> {
        if self.next >= self.count {
            return Ok(None);
        }
        let mut scan = Vec::new();
        let mut rt = Vec::new();
        let mut level = Vec::new();
        let mut prec = Vec::new();
        let mut mz = Vec::new();
        let mut inten: Vec<f32> = Vec::new();
        while self.next < self.count && mz.len() < CHUNK_POINTS {
            let s = ds.read_spectrum_view(self.run, self.next, self.view)?;
            if s.mz.len() != s.intensity.len() {
                return Err(Error::Other(format!(
                    "spectrum {}: {} m/z values but {} intensities",
                    self.next,
                    s.mz.len(),
                    s.intensity.len()
                )));
            }
            let n = s.mz.len();
            scan.extend(std::iter::repeat_n(s.scan_number, n));
            rt.extend(std::iter::repeat_n(s.rt_s, n));
            level.extend(std::iter::repeat_n(s.ms_level, n));
            prec.extend(std::iter::repeat_n(s.precursor_mz, n));
            mz.extend_from_slice(&s.mz);
            inten.extend_from_slice(&s.intensity);
            self.scans.push(&s);
            self.next += 1;
        }
        self.points += mz.len() as u64;
        let cols: Vec<ArrayRef> = vec![
            Arc::new(UInt64Array::from(scan)),
            Arc::new(Float64Array::from(rt)),
            Arc::new(UInt32Array::from(level)),
            Arc::new(Float64Array::from(prec)),
            Arc::new(Float64Array::from(mz)),
            Arc::new(Float32Array::from(inten)),
        ];
        RecordBatch::try_new(self.schema.clone(), cols)
            .map(Some)
            .map_err(arrow_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_names_get_suffixes() {
        let mut n: Vec<String> = ["a", "b", "a", "a", "a_1"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        dedupe(&mut n);
        assert_eq!(n, ["a", "b", "a_1", "a_2", "a_1_1"]);
    }

    #[test]
    fn integer_columns_reject_lossy_values() {
        let a = column_array(&ColKind::U16, &[0.0, 65535.0, f64::NAN], "x").unwrap();
        assert_eq!(a.null_count(), 1);
        assert!(column_array(&ColKind::U16, &[65536.0], "x").is_err());
        assert!(column_array(&ColKind::I8, &[1.5], "x").is_err());
        assert!(column_array(&ColKind::F32, &[0.1], "x").is_err());
        assert!(column_array(&ColKind::F32, &[f64::from(0.1f32)], "x").is_ok());
    }
}
