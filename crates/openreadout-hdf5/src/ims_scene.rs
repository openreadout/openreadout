//! Imaris scene objects (`Scene8/Content/<Object>`): the statistics Imaris computed for
//! filaments, spots, surfaces and cells, pivoted into one table per object category, and the
//! objects' own record datasets (vertices, edges, segments, …) as tables. Layout from corpus
//! files browsed with h5py; see `docs/formats/ims.md` § Scene objects and
//! `docs/provenance/ims.md`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use hdf5_pure::{Datatype, DatatypeByteOrder};
use openreadout_core::bytes::{Endian, until_nul};
use openreadout_core::model::{ColumnInfo, TableInfo};
use openreadout_core::{Error, Result};
use serde_json::json;

use crate::ims::IMS_FORMAT_ID;

/// Largest compound dataset read whole (bytes).
const MAX_RECORD_BYTES: u64 = 512 << 20;
/// Datasets of an object group that describe the statistics or labels rather than records.
const NOT_RECORDS: &[&str] = &[
    "StatisticsType",
    "StatisticsValue",
    "StatisticsValueTimeOffset",
    "Factor",
    "FactorList",
    "Category",
    "CreationParameters",
];

/// One field of a compound record, decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// Integer or floating-point field (integers above 2^53 lose precision).
    Number(f64),
    /// Fixed-length text field (trailing NULs and spaces removed).
    Text(String),
}

impl FieldValue {
    fn number(&self) -> Option<f64> {
        match self {
            FieldValue::Number(v) => Some(*v),
            FieldValue::Text(_) => None,
        }
    }
    fn text(&self) -> String {
        match self {
            FieldValue::Number(v) => format!("{v}"),
            FieldValue::Text(s) => s.clone(),
        }
    }
}

/// A 1-D compound dataset read whole: field names and one row per element.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Records {
    /// Field names in declaration order.
    pub fields: Vec<String>,
    /// `true` for text fields.
    pub text: Vec<bool>,
    /// Rows of decoded fields.
    pub rows: Vec<Vec<FieldValue>>,
}

impl Records {
    fn col(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f == name)
    }
}

/// A field decoder: (text?, width in bytes, decode).
type FieldDecoder = (bool, usize, Box<dyn Fn(&[u8]) -> FieldValue>);

fn field_decoder(dt: &Datatype) -> Option<FieldDecoder> {
    match dt {
        Datatype::FixedPoint {
            size,
            byte_order,
            layout,
        } => {
            let n = *size as usize;
            if !matches!(n, 1 | 2 | 4 | 8) {
                return None;
            }
            let be = matches!(byte_order, DatatypeByteOrder::BigEndian);
            let signed = layout.signed;
            Some((
                false,
                n,
                Box::new(move |b: &[u8]| {
                    let mut buf = [0u8; 8];
                    if be {
                        for (i, x) in b.iter().rev().enumerate() {
                            buf[i] = *x;
                        }
                    } else {
                        buf[..b.len()].copy_from_slice(b);
                    }
                    let u = u64::from_le_bytes(buf);
                    let v = if signed {
                        let shift = 64 - 8 * b.len() as u32;
                        ((u << shift) as i64 >> shift) as f64
                    } else {
                        u as f64
                    };
                    FieldValue::Number(v)
                }),
            ))
        }
        Datatype::FloatingPoint {
            size, byte_order, ..
        } => {
            let n = *size as usize;
            let e = if matches!(byte_order, DatatypeByteOrder::BigEndian) {
                Endian::Big
            } else {
                Endian::Little
            };
            match n {
                4 => Some((
                    false,
                    4,
                    Box::new(move |b: &[u8]| {
                        FieldValue::Number(f64::from(e.f32(b, 0).unwrap_or(0.0)))
                    }),
                )),
                8 => Some((
                    false,
                    8,
                    Box::new(move |b: &[u8]| FieldValue::Number(e.f64(b, 0).unwrap_or(0.0))),
                )),
                _ => None,
            }
        }
        Datatype::String { size, .. } => {
            let n = *size as usize;
            Some((
                true,
                n,
                Box::new(|b: &[u8]| {
                    FieldValue::Text(String::from_utf8_lossy(until_nul(b)).trim_end().to_string())
                }),
            ))
        }
        _ => None,
    }
}

/// Read a 1-D compound dataset of numeric and fixed-length text fields. `None` when the
/// dataset is not such a dataset (other fields are skipped; none left → `None`).
pub fn read_records(file: &hdf5_pure::File, path: &str) -> Result<Option<Records>> {
    let Ok(ds) = file.dataset(path) else {
        return Ok(None);
    };
    let Ok(Datatype::Compound { size, members }) = ds.datatype() else {
        return Ok(None);
    };
    let shape = ds
        .shape()
        .map_err(|e| Error::corrupt(IMS_FORMAT_ID, format!("{path}: {e}")))?;
    if shape.len() != 1 {
        return Ok(None);
    }
    let n = shape[0];
    let elem = u64::from(size);
    if n.saturating_mul(elem) > MAX_RECORD_BYTES {
        return Err(Error::unsupported(
            IMS_FORMAT_ID,
            format!("{path}: {n} records of {elem} bytes"),
            "Scene record datasets above 512 MiB are not read.",
        ));
    }
    let mut decoders = Vec::new();
    let mut out = Records::default();
    for m in &members {
        if let Some((text, width, f)) = field_decoder(&m.datatype) {
            let off = usize::try_from(m.byte_offset).unwrap_or(usize::MAX);
            if off.saturating_add(width) <= size as usize {
                out.fields.push(m.name.clone());
                out.text.push(text);
                decoders.push((off, width, f));
            }
        }
    }
    if decoders.is_empty() {
        return Ok(None);
    }
    let raw = if n == 0 {
        Vec::new()
    } else {
        ds.read_raw()
            .map_err(|e| Error::corrupt(IMS_FORMAT_ID, format!("{path}: {e}")))?
    };
    let elem = elem as usize;
    if raw.len() < (n as usize).saturating_mul(elem) {
        return Err(Error::corrupt(
            IMS_FORMAT_ID,
            format!("{path}: {} bytes for {n} records of {elem}", raw.len()),
        ));
    }
    out.rows = raw
        .chunks_exact(elem)
        .take(n as usize)
        .map(|rec| {
            decoders
                .iter()
                .map(|(off, w, f)| f(&rec[*off..*off + *w]))
                .collect()
        })
        .collect();
    Ok(Some(out))
}

/// How a table of a scene object is built.
#[derive(Debug, Clone, PartialEq)]
pub enum SceneTableKind {
    /// Statistics of one category, one row per (time, object).
    Statistics {
        /// Category id (`Category/ID`).
        category: i64,
    },
    /// A record dataset of the object group.
    Records {
        /// Dataset path.
        path: String,
    },
}

/// A column of a statistics table.
#[derive(Debug, Clone, PartialEq)]
pub enum StatColumn {
    /// Object id.
    Id,
    /// Time index (`ID_Time`).
    Time,
    /// A factor that varies by object (`Depth`, `Level`, `Type`): its level per row, as a
    /// number, or as a code into the categories when some level is not a number.
    Factor(String, Option<Vec<String>>),
    /// A statistic (`StatisticsType` ids that fill it, by the splitting factors' levels).
    Statistic(Vec<i64>),
}

/// A table of a scene object.
#[derive(Debug, Clone)]
pub struct SceneTable {
    /// Object group (`Filaments0`).
    pub object: String,
    /// Object display name (`Filaments 1`).
    pub object_name: Option<String>,
    /// What the table holds.
    pub kind: SceneTableKind,
    /// Table info (name, rows, columns).
    pub info: TableInfo,
    /// Statistics tables: how each column is filled.
    pub stat_columns: Vec<StatColumn>,
}

/// A statistic type: its name, unit, category and factor levels.
#[derive(Debug, Clone, PartialEq)]
struct StatType {
    name: String,
    unit: String,
    category: i64,
    factors: Vec<(String, String)>,
}

fn load_types(file: &hdf5_pure::File, g: &str) -> Result<Option<TypesAndCategories>> {
    let (Some(types), Some(values)) = (
        read_records(file, &format!("{g}/StatisticsType"))?,
        file.dataset(&format!("{g}/StatisticsValue")).ok(),
    ) else {
        return Ok(None);
    };
    let _ = values;
    let mut categories = BTreeMap::new();
    if let Some(c) = read_records(file, &format!("{g}/Category"))? {
        let (id, name) = (c.col("ID"), c.col("CategoryName").or_else(|| c.col("Name")));
        for r in &c.rows {
            if let (Some(i), Some(n)) = (id, name)
                && let Some(v) = r[i].number()
            {
                categories.insert(v as i64, r[n].text());
            }
        }
    }
    let mut factors: HashMap<i64, Vec<(String, String)>> = HashMap::new();
    if let Some(f) = read_records(file, &format!("{g}/Factor"))? {
        let (l, n, v) = (f.col("ID_List"), f.col("Name"), f.col("Level"));
        if let (Some(l), Some(n), Some(v)) = (l, n, v) {
            for r in &f.rows {
                if let Some(id) = r[l].number() {
                    factors
                        .entry(id as i64)
                        .or_default()
                        .push((r[n].text(), r[v].text()));
                }
            }
        }
    }
    let (id, cat, fl, name, unit) = (
        types.col("ID"),
        types.col("ID_Category"),
        types.col("ID_FactorList"),
        types.col("Name"),
        types.col("Unit"),
    );
    let (Some(id), Some(cat), Some(name)) = (id, cat, name) else {
        return Ok(None);
    };
    let mut out = HashMap::new();
    for r in &types.rows {
        let Some(tid) = r[id].number() else { continue };
        out.insert(
            tid as i64,
            StatType {
                name: r[name].text(),
                unit: unit.map(|u| r[u].text()).unwrap_or_default(),
                category: r[cat].number().map_or(-1, |v| v as i64),
                factors: fl
                    .and_then(|f| r[f].number())
                    .and_then(|f| factors.get(&(f as i64)).cloned())
                    .unwrap_or_default(),
            },
        );
    }
    Ok(Some((out, categories)))
}

/// Statistic types by id, and category names by id.
type TypesAndCategories = (HashMap<i64, StatType>, BTreeMap<i64, String>);
/// (statistic name, splitting factors' levels) → (unit, type ids).
type StatColumns = BTreeMap<(String, Vec<(String, String)>), (String, Vec<i64>)>;

/// Values of the statistics: (`ID_Time`, `ID_Object`, `ID_StatisticsType`, `Value`).
fn load_values(file: &hdf5_pure::File, g: &str) -> Result<Vec<(i64, i64, i64, f64)>> {
    let Some(v) = read_records(file, &format!("{g}/StatisticsValue"))? else {
        return Ok(Vec::new());
    };
    let (time, object, kind, value) = (
        v.col("ID_Time"),
        v.col("ID_Object"),
        v.col("ID_StatisticsType"),
        v.col("Value"),
    );
    let (Some(time), Some(object), Some(kind), Some(value)) = (time, object, kind, value) else {
        return Ok(Vec::new());
    };
    Ok(v.rows
        .iter()
        .filter_map(|r| {
            Some((
                r[time].number()? as i64,
                r[object].number()? as i64,
                r[kind].number()? as i64,
                r[value].number()?,
            ))
        })
        .collect())
}

/// The pivot plan of one category: attribute factors, statistic columns, rows.
struct Pivot {
    /// Attribute factors with their levels (sorted).
    attr_factors: Vec<(String, Vec<String>)>,
    /// (column name, unit, type ids, qualifier levels)
    stats: Vec<(String, String, Vec<i64>)>,
    rows: Vec<(i64, i64)>,
}

fn pivot(types: &HashMap<i64, StatType>, values: &[(i64, i64, i64, f64)], category: i64) -> Pivot {
    // Types of this category, and the factor structure: a factor whose level is the same for
    // every type of a name describes the name (dropped); one with several levels for one
    // (object, name) splits the statistic into columns; any other describes the object.
    let ids: BTreeSet<i64> = types
        .iter()
        .filter(|(_, t)| t.category == category)
        .map(|(k, _)| *k)
        .collect();
    let mut by_name_factor: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
    for id in &ids {
        let t = &types[id];
        for (f, l) in &t.factors {
            by_name_factor
                .entry((t.name.as_str(), f.as_str()))
                .or_default()
                .insert(l.as_str());
        }
    }
    let mut per_object: HashMap<(i64, i64, &str, &str), BTreeSet<&str>> = HashMap::new();
    for (tm, ob, ty, _) in values {
        if let Some(t) = types.get(ty).filter(|_| ids.contains(ty)) {
            for (f, l) in &t.factors {
                per_object
                    .entry((*tm, *ob, t.name.as_str(), f.as_str()))
                    .or_default()
                    .insert(l.as_str());
            }
        }
    }
    let mut splitting: BTreeSet<&str> = BTreeSet::new();
    for ((_, _, _, f), levels) in &per_object {
        if levels.len() > 1 {
            splitting.insert(f);
        }
    }
    let mut describes_name: BTreeSet<&str> = BTreeSet::new();
    let mut all_factors: BTreeSet<&str> = BTreeSet::new();
    for ((_, f), levels) in &by_name_factor {
        all_factors.insert(f);
        if levels.len() == 1 {
            describes_name.insert(f);
        }
    }
    // A factor with a single level per name for every name is a name property; if it varies
    // across objects of one name it is an attribute.
    let attr_factors: Vec<(String, Vec<String>)> = all_factors
        .iter()
        .filter(|f| !splitting.contains(**f))
        .filter(|f| {
            by_name_factor
                .iter()
                .any(|((_, g), levels)| g == *f && levels.len() > 1)
        })
        .map(|f| {
            let levels: BTreeSet<&str> = by_name_factor
                .iter()
                .filter(|((_, g), _)| g == f)
                .flat_map(|(_, l)| l.iter().copied())
                .collect();
            (
                (*f).to_string(),
                levels.into_iter().map(str::to_string).collect(),
            )
        })
        .collect();
    let _ = describes_name;
    // Statistic columns: name + splitting factors' levels.
    let mut cols: StatColumns = BTreeMap::new();
    let mut order: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut sorted_ids: Vec<i64> = ids.iter().copied().collect();
    sorted_ids.sort_by(|a, b| {
        types[a]
            .name
            .cmp(&types[b].name)
            .then(types[a].factors.cmp(&types[b].factors))
            .then(a.cmp(b))
    });
    for id in sorted_ids {
        let t = &types[&id];
        let qual: Vec<(String, String)> = t
            .factors
            .iter()
            .filter(|(f, _)| splitting.contains(f.as_str()))
            .cloned()
            .collect();
        let key = (t.name.clone(), qual);
        if !cols.contains_key(&key) {
            order.push(key.clone());
        }
        cols.entry(key)
            .or_insert_with(|| (t.unit.clone(), Vec::new()))
            .1
            .push(id);
    }
    let stats = order
        .into_iter()
        .map(|key| {
            let (unit, ids) = cols.remove(&key).unwrap_or_default();
            let name = if key.1.is_empty() {
                key.0
            } else {
                format!(
                    "{} [{}]",
                    key.0,
                    key.1
                        .iter()
                        .map(|(f, l)| format!("{f}={l}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            (name, unit, ids)
        })
        .collect();
    let mut rows: BTreeSet<(i64, i64)> = BTreeSet::new();
    for (tm, ob, ty, _) in values {
        if ids.contains(ty) {
            rows.insert((*tm, *ob));
        }
    }
    Pivot {
        attr_factors,
        stats,
        rows: rows.into_iter().collect(),
    }
}

fn column(index: usize, name: &str, dtype: &str, unit: Option<&str>) -> ColumnInfo {
    ColumnInfo {
        index: index as u32,
        name: name.to_string(),
        label: None,
        dtype: dtype.into(),
        unit: unit.filter(|u| !u.is_empty()).map(str::to_string),
        range: None,
        extra: BTreeMap::new(),
    }
}

/// Discover the scene objects of `Scene8/Content` and their tables (headers only: record
/// counts and the statistics' structure; values are read by [`read_scene_table`]).
pub fn scene_tables(file: &hdf5_pure::File) -> Result<Vec<SceneTable>> {
    let Ok(content) = file.group("Scene8/Content") else {
        return Ok(Vec::new());
    };
    let mut objects = content.groups().unwrap_or_default();
    objects.sort_by_key(|a| natural_key(a));
    let mut out = Vec::new();
    for obj in objects {
        let g = format!("Scene8/Content/{obj}");
        let name = file
            .group(&g)
            .and_then(|x| x.attrs())
            .ok()
            .and_then(|a| crate::h5util::attrs_text(&a).get("Name").cloned());
        let label = name.clone().unwrap_or_else(|| obj.clone());
        if let Some((types, categories)) = load_types(file, &g)? {
            let values = load_values(file, &g)?;
            let cat_ids: BTreeSet<i64> = types.values().map(|t| t.category).collect();
            for cat in cat_ids {
                let p = pivot(&types, &values, cat);
                if p.rows.is_empty() {
                    continue;
                }
                let cat_name = categories
                    .get(&cat)
                    .cloned()
                    .unwrap_or_else(|| format!("category {cat}"));
                let mut columns = vec![
                    column(0, "ID", "int64", None),
                    column(1, "Time", "int64", None),
                ];
                let mut stat_columns = vec![StatColumn::Id, StatColumn::Time];
                for (f, levels) in &p.attr_factors {
                    if levels.iter().all(|l| l.parse::<f64>().is_ok()) {
                        columns.push(column(columns.len(), f, "float64", None));
                        stat_columns.push(StatColumn::Factor(f.clone(), None));
                    } else {
                        let mut c = column(columns.len(), f, "uint32", None);
                        c.extra.insert("categories".into(), json!(levels));
                        columns.push(c);
                        stat_columns.push(StatColumn::Factor(f.clone(), Some(levels.clone())));
                    }
                }
                for (n, u, ids) in &p.stats {
                    columns.push(column(columns.len(), n, "float64", Some(u)));
                    stat_columns.push(StatColumn::Statistic(ids.clone()));
                }
                let mut extra = BTreeMap::new();
                extra.insert("object".into(), json!(obj));
                extra.insert("object_name".into(), json!(name));
                extra.insert("category".into(), json!(cat_name));
                extra.insert("kind".into(), json!("statistics"));
                out.push(SceneTable {
                    object: obj.clone(),
                    object_name: name.clone(),
                    kind: SceneTableKind::Statistics { category: cat },
                    info: TableInfo {
                        index: 0,
                        name: Some(format!("{label}: {cat_name} statistics")),
                        row_count: p.rows.len() as u64,
                        columns,
                        extra,
                    },
                    stat_columns,
                });
            }
        }
        // Record datasets (direct children and one level of subgroups, e.g. Points/).
        let mut paths: Vec<String> = Vec::new();
        if let Ok(gr) = file.group(&g) {
            for d in gr.datasets().unwrap_or_default() {
                paths.push(format!("{g}/{d}"));
            }
            for sub in gr.groups().unwrap_or_default() {
                if let Ok(sg) = file.group(&format!("{g}/{sub}")) {
                    for d in sg.datasets().unwrap_or_default() {
                        paths.push(format!("{g}/{sub}/{d}"));
                    }
                }
            }
        }
        paths.sort();
        for path in paths {
            let short = path.trim_start_matches(&format!("{g}/")).to_string();
            let leaf = short.rsplit('/').next().unwrap_or(&short);
            if NOT_RECORDS.contains(&leaf) || leaf.starts_with("Label") {
                continue;
            }
            // headers only: shape and field names
            let Ok(ds) = file.dataset(&path) else {
                continue;
            };
            let Ok(Datatype::Compound { members, .. }) = ds.datatype() else {
                continue;
            };
            let Ok(shape) = ds.shape() else { continue };
            if shape.len() != 1 || shape[0] == 0 {
                continue;
            }
            let fields: Vec<(String, bool)> = members
                .iter()
                .filter_map(|m| field_decoder(&m.datatype).map(|(t, _, _)| (m.name.clone(), t)))
                .collect();
            let text_fields: Vec<&str> = fields
                .iter()
                .filter(|(_, t)| *t)
                .map(|(n, _)| n.as_str())
                .collect();
            let columns: Vec<ColumnInfo> = fields
                .iter()
                .filter(|(_, t)| !*t)
                .enumerate()
                .map(|(i, (n, _))| column(i, n, "float64", None))
                .collect();
            if columns.is_empty() {
                continue;
            }
            let mut extra = BTreeMap::new();
            extra.insert("object".into(), json!(obj));
            extra.insert("object_name".into(), json!(name));
            extra.insert("dataset".into(), json!(short));
            extra.insert("kind".into(), json!("records"));
            if !text_fields.is_empty() {
                extra.insert("text_fields_omitted".into(), json!(text_fields));
            }
            out.push(SceneTable {
                object: obj.clone(),
                object_name: name.clone(),
                kind: SceneTableKind::Records { path: path.clone() },
                info: TableInfo {
                    index: 0,
                    name: Some(format!("{label}: {short}")),
                    row_count: shape[0],
                    columns,
                    extra,
                },
                stat_columns: Vec::new(),
            });
        }
    }
    Ok(out)
}

/// `Filaments10` after `Filaments2`.
fn natural_key(s: &str) -> (String, u64) {
    let digits: String = s.chars().rev().take_while(char::is_ascii_digit).collect();
    let n = digits
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    (s[..s.len() - digits.len()].to_string(), n)
}

/// All values of a scene table, column-major (text fields of record tables are left out).
#[allow(clippy::many_single_char_names)]
pub fn read_scene_table(file: &hdf5_pure::File, t: &SceneTable) -> Result<Vec<Vec<f64>>> {
    let g = format!("Scene8/Content/{}", t.object);
    match &t.kind {
        SceneTableKind::Statistics { category } => {
            let (types, _) = load_types(file, &g)?.ok_or_else(|| {
                Error::corrupt(IMS_FORMAT_ID, format!("{g}: statistics disappeared"))
            })?;
            let values = load_values(file, &g)?;
            let p = pivot(&types, &values, *category);
            let row_of: HashMap<(i64, i64), usize> =
                p.rows.iter().enumerate().map(|(i, k)| (*k, i)).collect();
            let n = p.rows.len();
            let mut cols: Vec<Vec<f64>> = vec![vec![f64::NAN; n]; t.stat_columns.len()];
            let col_of_type: HashMap<i64, usize> = t
                .stat_columns
                .iter()
                .enumerate()
                .flat_map(|(c, sc)| match sc {
                    StatColumn::Statistic(ids) => ids.iter().map(|i| (*i, c)).collect(),
                    _ => Vec::new(),
                })
                .collect();
            let factor_col: HashMap<&str, (usize, Option<&Vec<String>>)> = t
                .stat_columns
                .iter()
                .enumerate()
                .filter_map(|(c, sc)| match sc {
                    StatColumn::Factor(f, cats) => Some((f.as_str(), (c, cats.as_ref()))),
                    _ => None,
                })
                .collect();
            for (i, (tm, ob)) in p.rows.iter().enumerate() {
                cols[0][i] = *ob as f64;
                cols[1][i] = *tm as f64;
            }
            for (tm, ob, ty, v) in &values {
                let (Some(&r), Some(&c)) = (row_of.get(&(*tm, *ob)), col_of_type.get(ty)) else {
                    continue;
                };
                cols[c][r] = *v;
                if let Some(st) = types.get(ty) {
                    for (f, l) in &st.factors {
                        let Some(&(fc, cats)) = factor_col.get(f.as_str()) else {
                            continue;
                        };
                        let v = match cats {
                            Some(c) => c.iter().position(|x| x == l).map(|k| k as f64),
                            None => l.parse::<f64>().ok(),
                        };
                        if let Some(v) = v {
                            cols[fc][r] = v;
                        }
                    }
                }
            }
            Ok(cols)
        }
        SceneTableKind::Records { path } => {
            let r = read_records(file, path)?.ok_or_else(|| {
                Error::corrupt(IMS_FORMAT_ID, format!("{path}: not a record dataset"))
            })?;
            Ok(r.text
                .iter()
                .enumerate()
                .filter(|(_, t)| !**t)
                .map(|(ci, _)| {
                    r.rows
                        .iter()
                        .map(|row| row[ci].number().unwrap_or(f64::NAN))
                        .collect()
                })
                .collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(name: &str, cat: i64, f: &[(&str, &str)]) -> StatType {
        StatType {
            name: name.into(),
            unit: "um".into(),
            category: cat,
            factors: f.iter().map(|(a, b)| ((*a).into(), (*b).into())).collect(),
        }
    }

    #[test]
    fn pivot_splits_and_attributes() {
        // Depth varies by object (attribute), Channel splits one statistic, Collection
        // describes a name (dropped).
        let mut types = HashMap::new();
        types.insert(1, st("Length", 0, &[("Depth", "1")]));
        types.insert(2, st("Length", 0, &[("Depth", "2")]));
        types.insert(3, st("Intensity", 0, &[("Channel", "1")]));
        types.insert(4, st("Intensity", 0, &[("Channel", "2")]));
        types.insert(5, st("Position X", 0, &[("Collection", "Position")]));
        types.insert(6, st("Other", 1, &[]));
        let values = vec![
            (0, 10, 1, 5.0),
            (0, 11, 2, 7.0),
            (0, 10, 3, 1.0),
            (0, 10, 4, 2.0),
            (0, 11, 3, 3.0),
            (0, 10, 5, 9.0),
            (0, 99, 6, 1.0),
        ];
        let p = pivot(&types, &values, 0);
        assert_eq!(
            p.attr_factors,
            vec![("Depth".to_string(), vec!["1".to_string(), "2".to_string()])]
        );
        let names: Vec<&str> = p.stats.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Intensity [Channel=1]",
                "Intensity [Channel=2]",
                "Length",
                "Position X"
            ]
        );
        assert_eq!(p.rows, vec![(0, 10), (0, 11)]);
        assert_eq!(natural_key("Filaments10").1, 10);
    }
}
