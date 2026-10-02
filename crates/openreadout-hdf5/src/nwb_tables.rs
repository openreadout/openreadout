//! NWB tables: every `DynamicTable` (electrodes, units, trials, …), the spike times of a units
//! table, and `SpikeEventSeries`, as numeric tables. Meanings from the NWB and HDMF-common schemas
//! (open standards); see `docs/formats/hdf5.md`.

use std::collections::{BTreeMap, HashMap, HashSet};

use hdf5_pure::DType;
use openreadout_core::model::{ColumnInfo, Table, TableInfo};
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::h5util::{attr_text, dtype_name};
use crate::nwb::{NWB_FORMAT_ID, read_f64_rows};

/// Most category values kept for a text column (more → the column is skipped).
pub const MAX_CATEGORIES: usize = 10_000;
/// Most rows of a text column read to build its categories.
pub const MAX_TEXT_ROWS: u64 = 1_000_000;
/// Most columns a 2-D column or a spike waveform expands to.
pub const MAX_EXPANDED_COLUMNS: u64 = 4096;
/// Most rows returned by one `read_table` call: `MAX_TABLE_ROWS`, or more for narrow tables as
/// long as rows × columns stays within [`MAX_TABLE_VALUES`].
pub const MAX_TABLE_ROWS: u64 = 1 << 20;
/// Most values (rows × columns) returned by one `read_table` call.
pub const MAX_TABLE_VALUES: u64 = 1 << 26;

/// Where one table column comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum NwbColumn {
    /// A rank-1 numeric (or boolean) dataset, one value per row.
    Numeric {
        /// Dataset path.
        path: String,
    },
    /// Column `k` of a rank-2 numeric dataset.
    Matrix {
        /// Dataset path.
        path: String,
        /// Column within each row.
        k: u64,
        /// Values per row.
        width: u64,
    },
    /// A text dataset as codes into `categories`.
    Categorical {
        /// Dataset path.
        path: String,
        /// Distinct values in order of first appearance.
        categories: Vec<String>,
    },
    /// Values per row of a ragged column (from its `_index` dataset).
    RaggedCount {
        /// Index dataset path.
        index: String,
    },
}

/// What a table reads.
#[derive(Debug, Clone, PartialEq)]
pub enum NwbTableKind {
    /// A `DynamicTable`: one row per `id`.
    Dynamic {
        /// Columns in table order.
        columns: Vec<NwbColumn>,
    },
    /// The spike times of a units table: one row per spike.
    SpikeTimes {
        /// `spike_times` dataset.
        data: String,
        /// Exclusive end offset of each unit's spikes.
        ends: Vec<u64>,
        /// Unit ids, one per unit.
        ids: Vec<f64>,
    },
    /// A `SpikeEventSeries`: one row per event.
    SpikeEvents {
        /// `data` dataset.
        data: String,
        /// `timestamps` dataset.
        timestamps: String,
        /// Values per event (channels × samples).
        width: u64,
        /// Multiplier.
        conversion: f64,
        /// Added after the multiplier.
        offset: f64,
    },
}

/// One table of an NWB file.
#[derive(Debug, Clone)]
pub struct NwbTable {
    /// Group path of the table (or series).
    pub path: String,
    /// What to read.
    pub kind: NwbTableKind,
    /// The normalized description (index, name, rows, columns).
    pub info: TableInfo,
}

fn err(path: &str, e: impl std::fmt::Display) -> Error {
    Error::corrupt(NWB_FORMAT_ID, format!("{path}: {e}"))
}

fn column(index: usize, name: &str, dtype: &str, unit: Option<&str>) -> ColumnInfo {
    ColumnInfo {
        index: index as u32,
        name: name.into(),
        label: None,
        dtype: dtype.into(),
        unit: unit.map(str::to_string),
        range: None,
        extra: BTreeMap::new(),
    }
}

/// Whole-dataset values as f64, including booleans (a one-byte enum).
fn numeric_rows(file: &hdf5_pure::File, path: &str, first: u64, n: u64) -> Result<Vec<f64>> {
    let d = file.dataset(path).map_err(|e| err(path, e))?;
    match d.dtype().map_err(|e| err(path, e))? {
        DType::Enum(_) => {
            let size = d.element_size().map_err(|e| err(path, e))?;
            if size != 1 {
                return Err(err(path, format!("{size}-byte enum")));
            }
            let raw = d.read_raw_rows(first, n).map_err(|e| err(path, e))?;
            Ok(raw.into_iter().map(f64::from).collect())
        }
        _ => read_f64_rows(file, path, first, n),
    }
}

fn is_numeric(t: &DType) -> bool {
    matches!(
        t,
        DType::F32
            | DType::F64
            | DType::I8
            | DType::I16
            | DType::I32
            | DType::I64
            | DType::U8
            | DType::U16
            | DType::U32
            | DType::U64
    )
}

fn text_rows(file: &hdf5_pure::File, path: &str, first: u64, n: u64) -> Result<Vec<String>> {
    let d = file.dataset(path).map_err(|e| err(path, e))?;
    d.read_string_rows(first, n).map_err(|e| err(path, e))
}

/// All values of a rank-1 integer dataset.
fn u64_all(file: &hdf5_pure::File, path: &str) -> Result<Vec<u64>> {
    let d = file.dataset(path).map_err(|e| err(path, e))?;
    let n = d
        .shape()
        .map_err(|e| err(path, e))?
        .first()
        .copied()
        .unwrap_or(0);
    Ok(read_f64_rows(file, path, 0, n)?
        .into_iter()
        .map(|v| {
            if v.is_finite() && v >= 0.0 {
                v as u64
            } else {
                0
            }
        })
        .collect())
}

fn attr(file: &hdf5_pure::File, path: &str, key: &str, group: bool) -> Option<String> {
    let a = if group {
        file.group(path).and_then(|g| g.attrs()).ok()?
    } else {
        file.dataset(path).and_then(|d| d.attrs()).ok()?
    };
    a.get(key).and_then(attr_text)
}

/// The rows of a `DynamicTable`'s `id`, when `group` is one.
pub fn table_rows(file: &hdf5_pure::File, group: &str) -> Option<u64> {
    file.dataset(&format!("{group}/id"))
        .and_then(|d| d.shape())
        .ok()
        .and_then(|s| s.first().copied())
}

/// Describe the `DynamicTable` at `group` (and, for a units table, its spike-times table). `index`
/// is the table index of the first returned table.
#[allow(clippy::many_single_char_names)]
pub fn describe_dynamic(
    file: &hdf5_pure::File,
    group: &str,
    neurodata_type: &str,
    index: u32,
) -> Result<Vec<NwbTable>> {
    let rows = table_rows(file, group).ok_or_else(|| err(group, "a table without an id column"))?;
    let g = file.group(group).map_err(|e| err(group, e))?;
    let attrs = g.attrs().unwrap_or_default();
    let mut names: Vec<String> = match attrs.get("colnames") {
        Some(
            hdf5_pure::AttrValue::StringArray(a)
            | hdf5_pure::AttrValue::AsciiStringArray(a)
            | hdf5_pure::AttrValue::VarLenStringArray(a)
            | hdf5_pure::AttrValue::VarLenAsciiStringArray(a)
            | hdf5_pure::AttrValue::StringArraySized { values: a, .. }
            | hdf5_pure::AttrValue::AsciiStringArraySized { values: a, .. },
        ) => a.clone(),
        Some(v) => attr_text(v).map(|s| vec![s]).unwrap_or_default(),
        None => Vec::new(),
    };
    if names.is_empty() {
        let mut ds = g.datasets().unwrap_or_default();
        ds.sort();
        names = ds
            .into_iter()
            .filter(|d| d != "id" && !d.ends_with("_index"))
            .collect();
    }
    let mut columns = vec![NwbColumn::Numeric {
        path: format!("{group}/id"),
    }];
    let mut infos = vec![column(0, "id", "int64", None)];
    let mut skipped = Vec::new();
    let mut spike_times = None;
    for name in names {
        let path = format!("{group}/{name}");
        let index_path = format!("{path}_index");
        let Ok(d) = file.dataset(&path) else {
            skipped.push(json!({"column": name, "reason": "not a dataset"}));
            continue;
        };
        let label = attr(file, &path, "description", false).filter(|s| !s.is_empty());
        if file.dataset(&index_path).is_ok() {
            if name == "spike_times" {
                spike_times = Some((path.clone(), index_path.clone()));
            }
            let mut c = column(infos.len(), &format!("{name}_count"), "uint64", None);
            c.label = label;
            c.extra.insert("ragged".into(), json!(name));
            infos.push(c);
            columns.push(NwbColumn::RaggedCount { index: index_path });
            continue;
        }
        let shape = d.shape().unwrap_or_default();
        let dtype = d.dtype().ok();
        let unit = attr(file, &path, "unit", false);
        match (&dtype, shape.as_slice()) {
            (Some(DType::ObjectReference), _) => {
                skipped.push(json!({"column": name, "reason": "object references"}));
            }
            (Some(t), [n]) if *n == rows && (is_numeric(t) || matches!(t, DType::Enum(_))) => {
                let mut c = column(
                    infos.len(),
                    &name,
                    &if matches!(t, DType::Enum(_)) {
                        "uint8".to_string()
                    } else {
                        dtype_name(t)
                    },
                    unit.as_deref(),
                );
                c.label = label;
                infos.push(c);
                columns.push(NwbColumn::Numeric { path });
            }
            (Some(t), [n, w]) if *n == rows && is_numeric(t) && *w <= 64 => {
                for k in 0..*w {
                    let mut c = column(
                        infos.len(),
                        &format!("{name}[{k}]"),
                        &dtype_name(t),
                        unit.as_deref(),
                    );
                    c.label.clone_from(&label);
                    infos.push(c);
                    columns.push(NwbColumn::Matrix {
                        path: path.clone(),
                        k,
                        width: *w,
                    });
                }
            }
            (Some(DType::String | DType::VariableLengthString), [n])
                if *n == rows && rows <= MAX_TEXT_ROWS =>
            {
                let values = text_rows(file, &path, 0, rows)?;
                let mut categories: Vec<String> = Vec::new();
                let mut seen: HashSet<&str> = HashSet::new();
                for v in &values {
                    if seen.insert(v.as_str()) {
                        categories.push(v.clone());
                    }
                }
                if categories.len() > MAX_CATEGORIES {
                    skipped.push(json!({"column": name, "reason": "more distinct text values than categories kept"}));
                    continue;
                }
                let mut c = column(infos.len(), &name, "uint32", None);
                c.label = label;
                c.extra.insert("categories".into(), json!(categories));
                infos.push(c);
                columns.push(NwbColumn::Categorical { path, categories });
            }
            (t, s) => {
                skipped.push(json!({"column": name, "reason": format!("{} of shape {s:?}", t.as_ref().map_or_else(|| "unknown type".into(), dtype_name))}));
            }
        }
    }
    let mut extra = BTreeMap::new();
    extra.insert("path".into(), json!(group));
    extra.insert("neurodata_type".into(), json!(neurodata_type));
    if let Some(d) = attrs
        .get("description")
        .and_then(attr_text)
        .filter(|s| !s.is_empty())
    {
        extra.insert("description".into(), json!(d));
    }
    if !skipped.is_empty() {
        extra.insert("skipped_columns".into(), Value::Array(skipped));
    }
    let name = group.trim_start_matches('/').to_string();
    let mut out = vec![NwbTable {
        path: group.to_string(),
        kind: NwbTableKind::Dynamic { columns },
        info: TableInfo {
            index,
            name: Some(name.clone()),
            row_count: rows,
            columns: infos,
            extra,
        },
    }];
    if let Some((data, index_path)) = spike_times {
        let ends = u64_all(file, &index_path)?;
        let ids = read_f64_rows(file, &format!("{group}/id"), 0, rows)?;
        let total = file
            .dataset(&data)
            .and_then(|d| d.shape())
            .map_err(|e| err(&data, e))?
            .first()
            .copied()
            .unwrap_or(0);
        if ends.windows(2).any(|w| w[1] < w[0]) || ends.last().is_some_and(|e| *e > total) {
            return Err(err(
                &index_path,
                "spike_times_index is not a non-decreasing list of offsets within spike_times",
            ));
        }
        let mut extra = BTreeMap::new();
        extra.insert("path".into(), json!(data));
        extra.insert("neurodata_type".into(), json!("VectorData"));
        extra.insert(
            "description".into(),
            json!(
                "one row per spike: the unit's row and id in the units table, and the spike time"
            ),
        );
        out.push(NwbTable {
            path: data.clone(),
            kind: NwbTableKind::SpikeTimes {
                data,
                ends: ends.clone(),
                ids,
            },
            info: TableInfo {
                index: index + 1,
                name: Some(format!("{name}/spike_times")),
                row_count: ends.last().copied().unwrap_or(0),
                columns: vec![
                    column(0, "unit_row", "uint64", None),
                    column(1, "unit_id", "int64", None),
                    column(2, "spike_time", "float64", Some("s")),
                ],
                extra,
            },
        });
    }
    Ok(out)
}

/// Describe the `SpikeEventSeries` at `group` as a table.
pub fn describe_spike_events(file: &hdf5_pure::File, group: &str, index: u32) -> Result<NwbTable> {
    let data = format!("{group}/data");
    let timestamps = format!("{group}/timestamps");
    let d = file.dataset(&data).map_err(|e| err(&data, e))?;
    let shape = d.shape().map_err(|e| err(&data, e))?;
    let n = shape.first().copied().unwrap_or(0);
    let nt = file
        .dataset(&timestamps)
        .and_then(|t| t.shape())
        .map_err(|e| err(&timestamps, e))?
        .first()
        .copied()
        .unwrap_or(0);
    if nt != n {
        return Err(err(group, format!("{nt} timestamps for {n} events")));
    }
    let (channels, samples) = match shape.as_slice() {
        [_] => (0, 1),
        [_, s] => (0, *s),
        [_, c, s] => (*c, *s),
        other => return Err(err(&data, format!("shape {other:?}"))),
    };
    let width = channels.max(1) * samples;
    if width > MAX_EXPANDED_COLUMNS {
        return Err(Error::unsupported(
            NWB_FORMAT_ID,
            format!("{data}: {width} values per event"),
            "SpikeEventSeries with more than 4096 values per event are listed, not read.",
        ));
    }
    let a = d.attrs().unwrap_or_default();
    let num = |k: &str| {
        a.get(k)
            .and_then(attr_text)
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|v| v.is_finite())
    };
    let unit = a.get("unit").and_then(attr_text);
    let dtype = d.dtype().map(|t| dtype_name(&t)).unwrap_or_default();
    let mut cols = vec![column(0, "time_s", "float64", Some("s"))];
    for c in 0..channels.max(1) {
        for s in 0..samples {
            let name = match shape.len() {
                1 => "value".to_string(),
                2 => format!("w{s}"),
                _ => format!("c{c}_w{s}"),
            };
            cols.push(column(cols.len(), &name, &dtype, unit.as_deref()));
        }
    }
    let mut extra = BTreeMap::new();
    extra.insert("path".into(), json!(group));
    extra.insert("neurodata_type".into(), json!("SpikeEventSeries"));
    if let Ok(rows) = u64_all(file, &format!("{group}/electrodes")) {
        extra.insert("electrode_rows".into(), json!(rows));
    }
    if let Some(desc) =
        attr(file, group, "description", true).filter(|s| !s.is_empty() && s != "no description")
    {
        extra.insert("description".into(), json!(desc));
    }
    Ok(NwbTable {
        path: group.to_string(),
        kind: NwbTableKind::SpikeEvents {
            data,
            timestamps,
            width,
            conversion: num("conversion").unwrap_or(1.0),
            offset: num("offset").unwrap_or(0.0),
        },
        info: TableInfo {
            index,
            name: Some(group.trim_start_matches('/').to_string()),
            row_count: n,
            columns: cols,
            extra,
        },
    })
}

impl NwbTable {
    /// Rows `[first, first + n)` (at most [`MAX_TABLE_ROWS`], more for narrow tables).
    pub fn read(&self, file: &hdf5_pure::File, first: u64, max: u64) -> Result<Table> {
        let total = self.info.row_count;
        if first > total {
            return Err(Error::Usage(format!(
                "first row {first} is past the last row ({total} rows)"
            )));
        }
        let width = (self.info.columns.len() as u64).max(1);
        let cap = MAX_TABLE_ROWS.max(MAX_TABLE_VALUES / width);
        let n = max.min(total - first).min(cap);
        let columns = match &self.kind {
            NwbTableKind::Dynamic { columns } => {
                let mut out = Vec::with_capacity(columns.len());
                let mut matrices: HashMap<String, Vec<f64>> = HashMap::new();
                for c in columns {
                    out.push(match c {
                        NwbColumn::Numeric { path } => numeric_rows(file, path, first, n)?,
                        NwbColumn::Matrix { path, k, width } => {
                            if !matrices.contains_key(path) {
                                matrices.insert(path.clone(), read_f64_rows(file, path, first, n)?);
                            }
                            let m = &matrices[path];
                            (0..n)
                                .map(|r| {
                                    m.get((r * width + k) as usize).copied().unwrap_or(f64::NAN)
                                })
                                .collect()
                        }
                        NwbColumn::Categorical { path, categories } => {
                            let code: HashMap<&str, f64> = categories
                                .iter()
                                .enumerate()
                                .map(|(i, s)| (s.as_str(), i as f64))
                                .collect();
                            text_rows(file, path, first, n)?
                                .iter()
                                .map(|s| code.get(s.as_str()).copied().unwrap_or(f64::NAN))
                                .collect()
                        }
                        NwbColumn::RaggedCount { index } => {
                            let lo = first.saturating_sub(1);
                            let v = read_f64_rows(file, index, lo, n + (first - lo))?;
                            let prev = if first == 0 {
                                0.0
                            } else {
                                v.first().copied().unwrap_or(0.0)
                            };
                            let tail = if first == 0 { &v[..] } else { &v[1..] };
                            let mut last = prev;
                            tail.iter()
                                .map(|e| {
                                    let c = e - last;
                                    last = *e;
                                    c
                                })
                                .collect()
                        }
                    });
                }
                out
            }
            NwbTableKind::SpikeTimes { data, ends, ids } => {
                let times = read_f64_rows(file, data, first, n)?;
                let mut unit = ends.partition_point(|e| *e <= first);
                let (mut rows, mut uid) = (
                    Vec::with_capacity(n as usize),
                    Vec::with_capacity(n as usize),
                );
                for r in first..first + n {
                    while unit < ends.len() && ends[unit] <= r {
                        unit += 1;
                    }
                    rows.push(unit as f64);
                    uid.push(ids.get(unit).copied().unwrap_or(f64::NAN));
                }
                vec![rows, uid, times]
            }
            NwbTableKind::SpikeEvents {
                data,
                timestamps,
                width,
                conversion,
                offset,
            } => {
                let ts = read_f64_rows(file, timestamps, first, n)?;
                let raw = read_f64_rows(file, data, first, n)?;
                if raw.len() as u64 != n * width {
                    return Err(err(
                        data,
                        format!("read {} values, expected {}", raw.len(), n * width),
                    ));
                }
                let mut out = vec![ts];
                for k in 0..*width {
                    out.push(
                        (0..n)
                            .map(|r| raw[(r * width + k) as usize] * conversion + offset)
                            .collect(),
                    );
                }
                out
            }
        };
        Ok(Table {
            table: self.info.index,
            first_row: first,
            columns,
        })
    }
}
