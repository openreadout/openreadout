//! Joining a sample sheet or plate layout onto a batch table.
//!
//! Every row of the table has candidate keys: its data set's relative path, file name and stem,
//! the sample id, sample name, barcode and vial/sequence position the file records
//! (`experiment.sample`), its plate well (the row's own `well`, e.g. a plate read, else the
//! file's `sample.well`), its run order (rank of the acquisition start time), and any other
//! column of the table. A sheet column *matches* a key when its values, normalised the same
//! way (case, path separators, `A1`/`A01`, globs such as `ctrl_*.fcs` for names and paths),
//! equal the row's. The key is chosen from the data: the (sheet column, key) pair that matches
//! the most rows, then the most data sets, then the more specific key (path, file, stem, sample
//! id, sample name, barcode, well, position, run order). If rows still match several sheet rows
//! that disagree, a second (and third) column is added to the key when it resolves them (plate
//! barcode + well). `--key COLUMN=FIELD` fixes the key instead. Nothing is dropped: the report
//! lists the key chosen and the runners-up, unmatched data sets, unmatched sheet rows and
//! ambiguous rows (left without annotations).

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use openreadout_core::{Error, Result};
use serde::Serialize;

use crate::run::BatchResult;
use crate::sheet::SampleSheet;
use crate::table::{Column, ColumnType, Role, Value, parse_number};
use crate::well;

/// A key of the table's rows a sheet column can match.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyField {
    /// The data set's path relative to its input (or any trailing part of it).
    Path,
    /// The file name with its extension.
    File,
    /// The file name without its extension(s) (or with them: `a` and `a.fcs` both match).
    Stem,
    /// `experiment.sample.id`.
    SampleId,
    /// `experiment.sample.name`.
    SampleName,
    /// `experiment.sample.barcode` (plate or tube barcode).
    Barcode,
    /// The row's plate well (`well` column, else `experiment.sample.well`).
    Well,
    /// `experiment.sample.sequence_position` (vial, autosampler position).
    Position,
    /// 1-based rank of the data set's acquisition start time.
    RunOrder,
    /// Any column of the table (`column:<name>` on the command line).
    Column(String),
}

impl KeyField {
    /// Parse a field name: `path`, `file`, `stem`, `sample_id`, `sample_name`, `barcode`,
    /// `well`, `position` (`vial`), `run_order`, or `column:<name>`.
    pub fn parse(s: &str) -> Option<KeyField> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "path" | "relative_path" => KeyField::Path,
            "file" | "filename" | "file_name" | "name" => KeyField::File,
            "stem" => KeyField::Stem,
            "sample" | "sample_id" | "sample.id" => KeyField::SampleId,
            "sample_name" | "sample.name" => KeyField::SampleName,
            "barcode" | "plate" | "plate_barcode" => KeyField::Barcode,
            "well" => KeyField::Well,
            "position" | "vial" | "sequence_position" => KeyField::Position,
            "run_order" | "order" | "injection" => KeyField::RunOrder,
            _ => {
                let c = s.trim();
                let name = c
                    .strip_prefix("column:")
                    .or_else(|| c.strip_prefix("col:"))?;
                KeyField::Column(name.to_string())
            }
        })
    }
    /// The name used in reports and on the command line.
    pub fn name(&self) -> String {
        match self {
            KeyField::Path => "path".into(),
            KeyField::File => "file".into(),
            KeyField::Stem => "stem".into(),
            KeyField::SampleId => "sample_id".into(),
            KeyField::SampleName => "sample_name".into(),
            KeyField::Barcode => "barcode".into(),
            KeyField::Well => "well".into(),
            KeyField::Position => "position".into(),
            KeyField::RunOrder => "run_order".into(),
            KeyField::Column(c) => format!("column:{c}"),
        }
    }
    fn is_name_like(&self) -> bool {
        matches!(self, KeyField::Path | KeyField::File | KeyField::Stem)
    }
    fn auto_candidates() -> [KeyField; 9] {
        [
            KeyField::Path,
            KeyField::File,
            KeyField::Stem,
            KeyField::SampleId,
            KeyField::SampleName,
            KeyField::Barcode,
            KeyField::Well,
            KeyField::Position,
            KeyField::RunOrder,
        ]
    }
    fn priority(&self) -> usize {
        match self {
            KeyField::Path => 0,
            KeyField::File => 1,
            KeyField::Stem => 2,
            KeyField::SampleId => 3,
            KeyField::SampleName => 4,
            KeyField::Barcode => 5,
            KeyField::Well => 6,
            KeyField::Position => 7,
            KeyField::RunOrder => 8,
            KeyField::Column(_) => 9,
        }
    }
}

/// How to join.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct JoinOptions {
    /// Fixed key: (sheet column, table key). Empty: chosen from the data.
    pub keys: Vec<(String, KeyField)>,
}

impl JoinOptions {
    /// Parse `COLUMN=FIELD` pairs (`--key filename=file --key Well=well`).
    pub fn parse_keys(specs: &[String]) -> Result<JoinOptions> {
        let mut o = JoinOptions::default();
        for s in specs {
            let (c, f) = s.split_once('=').ok_or_else(|| {
                Error::Usage(format!(
                    "--key {s}: expected SHEET_COLUMN=FIELD, e.g. filename=file, Well=well, Sample=sample_id"
                ))
            })?;
            let field = KeyField::parse(f).ok_or_else(|| {
                Error::Usage(format!(
                    "--key {s}: unknown field `{f}` (path, file, stem, sample_id, sample_name, barcode, well, position, run_order, column:NAME)"
                ))
            })?;
            o.keys.push((c.trim().to_string(), field));
        }
        Ok(o)
    }
}

/// One key pair tried.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct KeyCandidate {
    /// Sheet column.
    pub sheet_column: String,
    /// Table key.
    pub field: KeyField,
    /// Table rows it matches.
    pub matched_rows: u64,
    /// Data sets it matches.
    pub matched_datasets: u64,
}

/// What a join did.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct JoinReport {
    /// The sheet read.
    pub sheet: SampleSheet,
    /// The key used: one or more (sheet column, field) pairs, all of which must agree.
    pub key: Vec<KeyCandidate>,
    /// True when the key was chosen from the data (no `--key`).
    pub auto: bool,
    /// The next best keys (auto only), to judge how clear the choice was.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<KeyCandidate>,
    /// Columns added to the table.
    pub annotations: Vec<String>,
    /// Table rows that got annotations.
    pub matched_rows: u64,
    /// Rows in the table.
    pub rows: u64,
    /// Data sets with at least one annotated row.
    pub matched_datasets: u64,
    /// Data sets in the table.
    pub datasets: u64,
    /// Data sets no sheet row matched (first 20).
    pub unmatched_datasets: Vec<String>,
    /// How many data sets no sheet row matched.
    pub unmatched_datasets_count: u64,
    /// Sheet rows that matched no table row, by their key values (first 20).
    pub unmatched_sheet_rows: Vec<String>,
    /// How many sheet rows matched nothing.
    pub unmatched_sheet_rows_count: u64,
    /// Table rows that matched several sheet rows that disagree (first 20); left without
    /// annotations.
    pub ambiguous: Vec<String>,
    /// How many rows were ambiguous.
    pub ambiguous_count: u64,
    /// Things to check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn norm_text(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn norm_path(s: &str) -> String {
    let t = s.trim().replace('\\', "/").to_lowercase();
    let t = t.trim_start_matches("./").trim_end_matches('/');
    t.to_string()
}

fn norm_alnum(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_lowercase()
}

/// File name without its extension(s): `a.ome.tiff` → `a.ome` and `a`.
fn stems(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut s = name;
    while let Some((head, _)) = s.rsplit_once('.') {
        if head.is_empty() {
            break;
        }
        out.push(head.to_string());
        s = head;
        if out.len() >= 3 {
            break;
        }
    }
    if out.is_empty() {
        out.push(name.to_string());
    }
    out
}

/// Normalise a sheet cell for matching against `field`; `None` when it cannot match.
fn sheet_key(field: &KeyField, v: &str) -> Option<String> {
    let t = v.trim();
    if t.is_empty() {
        return None;
    }
    match field {
        KeyField::Path | KeyField::File | KeyField::Stem => Some(norm_path(t)),
        KeyField::Well => well::parse(t).map(well::Well::name),
        KeyField::Position => Some(norm_alnum(t)).filter(|s| !s.is_empty()),
        KeyField::RunOrder => t
            .parse::<f64>()
            .ok()
            .filter(|x| x.fract() == 0.0 && *x >= 1.0)
            .map(|x| format!("{x:.0}")),
        KeyField::Column(_) => Some(match parse_number(t) {
            Some(x) => crate::table::fmt_float(x),
            None => norm_text(t),
        }),
        _ => Some(norm_text(t)),
    }
}

/// The row's keys for `field` (alternatives; any may match).
fn row_keys(
    res: &BatchResult,
    row: usize,
    field: &KeyField,
    ranks: &[Option<usize>],
) -> Vec<String> {
    let di = res.row_dataset.get(row).copied().unwrap_or(0);
    let Some(d) = res.datasets.get(di) else {
        return Vec::new();
    };
    let file = d
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let opt = |s: &Option<String>| s.as_deref().map(norm_text).into_iter().collect::<Vec<_>>();
    match field {
        KeyField::Path => {
            let rel = norm_path(&d.relative.to_string_lossy());
            let full = norm_path(&d.path.to_string_lossy());
            let mut out = vec![full.clone(), rel.clone()];
            let parts: Vec<&str> = full.split('/').collect();
            for i in 1..parts.len() {
                out.push(parts[i..].join("/"));
            }
            out.sort();
            out.dedup();
            out
        }
        KeyField::File => vec![norm_path(&file)],
        KeyField::Stem => {
            let mut v: Vec<String> = stems(&file).iter().map(|s| norm_path(s)).collect();
            v.push(norm_path(&file));
            v
        }
        KeyField::SampleId => opt(&d.keys.id),
        KeyField::SampleName => opt(&d.keys.name),
        KeyField::Barcode => opt(&d.keys.barcode),
        KeyField::Position => d
            .keys
            .position
            .as_deref()
            .map(norm_alnum)
            .filter(|s| !s.is_empty())
            .into_iter()
            .collect(),
        KeyField::Well => {
            let own = res.table.get(row, "well").text();
            let w = if own.is_empty() {
                d.keys.well.clone().unwrap_or_default()
            } else {
                own
            };
            well::parse(&w).map(well::Well::name).into_iter().collect()
        }
        KeyField::RunOrder => ranks
            .get(di)
            .copied()
            .flatten()
            .map(|r| (r + 1).to_string())
            .into_iter()
            .collect(),
        KeyField::Column(c) => {
            let v = res.table.get(row, c);
            if v.is_null() {
                Vec::new()
            } else {
                vec![match v.as_f64() {
                    Some(x) if !matches!(v, Value::Text(_)) => crate::table::fmt_float(x),
                    _ => norm_text(&v.text()),
                }]
            }
        }
    }
}

/// A sheet column indexed for one field: exact keys and glob patterns.
struct Index {
    exact: HashMap<String, Vec<usize>>,
    globs: Vec<(glob::Pattern, usize)>,
}

fn index(sheet: &SampleSheet, col: usize, field: &KeyField) -> Index {
    let mut exact: HashMap<String, Vec<usize>> = HashMap::new();
    let mut globs = Vec::new();
    for (i, r) in sheet.rows.iter().enumerate() {
        let Some(v) = r.get(col) else { continue };
        let Some(k) = sheet_key(field, v) else {
            continue;
        };
        if field.is_name_like() && k.contains(['*', '?', '[']) {
            if let Ok(p) = glob::Pattern::new(&k) {
                globs.push((p, i));
            }
            continue;
        }
        exact.entry(k).or_default().push(i);
    }
    Index { exact, globs }
}

fn lookup(ix: &Index, keys: &[String]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    for k in keys {
        if let Some(v) = ix.exact.get(k) {
            out.extend(v.iter().copied());
        }
        for (p, i) in &ix.globs {
            if p.matches(k) {
                out.insert(*i);
            }
        }
    }
    out
}

/// Column name hints that make a weak key (position, run order) worth trying automatically.
fn hinted(field: &KeyField, column: &str, sheet: &SampleSheet, col: usize) -> bool {
    let c = column.to_lowercase();
    match field {
        KeyField::RunOrder => ["order", "injection", "run", "seq", "acquisition"]
            .iter()
            .any(|h| c.contains(h)),
        KeyField::Position => {
            ["vial", "position", "pos", "tray", "slot", "rack"]
                .iter()
                .any(|h| c.contains(h))
                || sheet
                    .rows
                    .iter()
                    .filter_map(|r| r.get(col))
                    .any(|v| !v.trim().is_empty() && parse_number(v).is_none())
        }
        _ => true,
    }
}

type Matches = Vec<BTreeSet<usize>>;

fn match_all(
    res: &BatchResult,
    sheet: &SampleSheet,
    col: usize,
    field: &KeyField,
    ranks: &[Option<usize>],
) -> Matches {
    let ix = index(sheet, col, field);
    (0..res.table.rows.len())
        .map(|r| lookup(&ix, &row_keys(res, r, field, ranks)))
        .collect()
}

fn score(res: &BatchResult, m: &Matches) -> (u64, u64) {
    let rows = m.iter().filter(|s| !s.is_empty()).count() as u64;
    let ds: BTreeSet<usize> = m
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.is_empty())
        .map(|(r, _)| res.row_dataset[r])
        .collect();
    (rows, ds.len() as u64)
}

/// Do these sheet rows carry the same annotations (on the non-key columns)?
fn agree(sheet: &SampleSheet, rows: &BTreeSet<usize>, key_cols: &[usize]) -> bool {
    let mut it = rows.iter();
    let Some(&first) = it.next() else {
        return true;
    };
    it.all(|&r| {
        (0..sheet.columns.len())
            .filter(|c| !key_cols.contains(c))
            .all(|c| sheet.rows[r].get(c) == sheet.rows[first].get(c))
    })
}

fn ambiguous_count(sheet: &SampleSheet, m: &Matches, key_cols: &[usize]) -> usize {
    m.iter()
        .filter(|s| s.len() > 1 && !agree(sheet, s, key_cols))
        .count()
}

fn ranks_of(res: &BatchResult) -> Vec<Option<usize>> {
    let mut timed: Vec<(i64, usize)> = res
        .datasets
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            d.keys
                .started_at
                .as_deref()
                .and_then(openreadout_index::tables::micros_from_iso)
                .map(|t| (t, i))
        })
        .collect();
    timed.sort_unstable();
    let mut out = vec![None; res.datasets.len()];
    for (rank, (_, i)) in timed.into_iter().enumerate() {
        out[i] = Some(rank);
    }
    out
}

/// The sheet column typed: numbers when every non-empty cell is one.
fn typed(values: &[&str]) -> Vec<Value> {
    let numeric = values.iter().any(|v| !v.trim().is_empty())
        && values
            .iter()
            .all(|v| v.trim().is_empty() || parse_number(v).is_some());
    values
        .iter()
        .map(|v| {
            let t = v.trim();
            if t.is_empty() {
                Value::Null
            } else if numeric {
                let x = parse_number(t).unwrap_or(f64::NAN);
                if x.fract() == 0.0 && x.abs() < 9e15 && !t.contains(['.', 'e', 'E']) {
                    Value::Int(x as i64)
                } else {
                    Value::Float(x)
                }
            } else {
                Value::Text(t.to_string())
            }
        })
        .collect()
}

/// Join `sheet` onto `res.table` (see the module documentation).
pub fn join(res: &mut BatchResult, sheet: SampleSheet, opts: &JoinOptions) -> Result<JoinReport> {
    let ranks = ranks_of(res);
    let n_rows = res.table.rows.len();
    let mut alternatives: Vec<(KeyCandidate, usize, Matches)> = Vec::new();
    let auto = opts.keys.is_empty();
    let mut chosen: Vec<(KeyCandidate, usize, Matches)> = Vec::new();
    if auto {
        let mut all = Vec::new();
        for (ci, cname) in sheet.columns.iter().enumerate() {
            for f in KeyField::auto_candidates() {
                if !hinted(&f, cname, &sheet, ci) {
                    continue;
                }
                let m = match_all(res, &sheet, ci, &f, &ranks);
                let (rows, ds) = score(res, &m);
                if rows == 0 {
                    continue;
                }
                all.push((
                    KeyCandidate {
                        sheet_column: cname.clone(),
                        field: f,
                        matched_rows: rows,
                        matched_datasets: ds,
                    },
                    ci,
                    m,
                ));
            }
        }
        all.sort_by(|a, b| {
            (b.0.matched_rows, b.0.matched_datasets)
                .cmp(&(a.0.matched_rows, a.0.matched_datasets))
                .then(a.0.field.priority().cmp(&b.0.field.priority()))
                .then(a.1.cmp(&b.1))
        });
        if all.is_empty() {
            let sample_keys: Vec<String> = res
                .datasets
                .iter()
                .take(3)
                .map(|d| {
                    format!(
                        "{} (sample id {})",
                        d.relative.display(),
                        d.keys.id.as_deref().unwrap_or("none")
                    )
                })
                .collect();
            return Err(Error::Usage(format!(
                "sample sheet {}: no column matches the data sets by path, file name, stem, sample id or name, barcode, well, vial or run order. Sheet columns: {}. First data sets: {}. Name the key with --key SHEET_COLUMN=FIELD (e.g. --key {}=file)",
                sheet.path,
                sheet.columns.join(", "),
                sample_keys.join("; "),
                sheet.columns.first().map_or("filename", String::as_str)
            )));
        }
        let first = all.remove(0);
        chosen.push(first);
        alternatives = all;
    } else {
        for (cname, f) in &opts.keys {
            let ci = sheet.column(cname).ok_or_else(|| {
                Error::Usage(format!(
                    "--key {cname}={}: the sheet has no column `{cname}` (it has: {})",
                    f.name(),
                    sheet.columns.join(", ")
                ))
            })?;
            if let KeyField::Column(c) = f
                && res.table.index_of(c).is_none()
            {
                return Err(Error::Usage(format!(
                    "--key {cname}={}: the table has no column `{c}`",
                    f.name()
                )));
            }
            let m = match_all(res, &sheet, ci, f, &ranks);
            let (rows, ds) = score(res, &m);
            chosen.push((
                KeyCandidate {
                    sheet_column: sheet.columns[ci].clone(),
                    field: f.clone(),
                    matched_rows: rows,
                    matched_datasets: ds,
                },
                ci,
                m,
            ));
        }
    }
    // combined matches: intersection of every key's matches
    let combine = |keys: &[&Matches]| -> Matches {
        (0..n_rows)
            .map(|r| {
                let mut it = keys.iter();
                let mut s = it.next().map(|m| m[r].clone()).unwrap_or_default();
                for m in it {
                    s = s.intersection(&m[r]).copied().collect();
                }
                s
            })
            .collect()
    };
    let mut current = combine(&chosen.iter().map(|c| &c.2).collect::<Vec<_>>());
    if auto {
        // add columns to the key while that resolves ambiguity without losing matches
        for _ in 0..2 {
            let key_cols: Vec<usize> = chosen.iter().map(|c| c.1).collect();
            let amb = ambiguous_count(&sheet, &current, &key_cols);
            if amb == 0 {
                break;
            }
            let (matched, _) = score(res, &current);
            let mut best: Option<(usize, usize)> = None; // (alternative index, ambiguity)
            for (ai, alt) in alternatives.iter().enumerate() {
                if key_cols.contains(&alt.1) {
                    continue;
                }
                let trial = combine(&[&current, &alt.2]);
                let (m2, _) = score(res, &trial);
                if m2 < matched {
                    continue;
                }
                let mut kc = key_cols.clone();
                kc.push(alt.1);
                let a2 = ambiguous_count(&sheet, &trial, &kc);
                if a2 < amb && best.is_none_or(|(_, b)| a2 < b) {
                    best = Some((ai, a2));
                }
            }
            let Some((ai, _)) = best else { break };
            let alt = alternatives.remove(ai);
            current = combine(&[&current, &alt.2]);
            chosen.push(alt);
        }
    }
    let key_cols: Vec<usize> = chosen.iter().map(|c| c.1).collect();
    // annotation columns: every sheet column not in the key
    let ann_cols: Vec<usize> = (0..sheet.columns.len())
        .filter(|c| !key_cols.contains(c))
        .collect();
    let mut names = Vec::new();
    for &c in &ann_cols {
        let base = sheet.columns[c].clone();
        let mut n = base.clone();
        if res.table.index_of(&n).is_some() || names.contains(&n) {
            n = format!("{base}_sheet");
        }
        let mut k = 2;
        while res.table.index_of(&n).is_some() || names.contains(&n) {
            n = format!("{base}_sheet{k}");
            k += 1;
        }
        names.push(n);
    }
    let typed_cols: Vec<Vec<Value>> = ann_cols
        .iter()
        .map(|&c| {
            let vals: Vec<&str> = sheet
                .rows
                .iter()
                .map(|r| r.get(c).map_or("", String::as_str))
                .collect();
            typed(&vals)
        })
        .collect();
    // fill
    let mut matched_rows = 0u64;
    let mut amb_list = Vec::new();
    let mut amb_n = 0u64;
    let mut used_sheet = vec![false; sheet.rows.len()];
    let mut matched_ds = BTreeSet::new();
    let mut new_cells: Vec<Vec<Value>> = Vec::with_capacity(n_rows);
    for (r, s) in current.iter().enumerate() {
        let mut cells = vec![Value::Null; ann_cols.len()];
        if !s.is_empty() {
            for &i in s {
                used_sheet[i] = true;
            }
            if s.len() > 1 && !agree(&sheet, s, &key_cols) {
                amb_n += 1;
                if amb_list.len() < 20 {
                    amb_list.push(format!(
                        "{} matches {} sheet rows ({})",
                        row_label(res, r),
                        s.len(),
                        s.iter()
                            .take(4)
                            .map(|i| format!("row {}", i + 2))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            } else if let Some(&i) = s.iter().next() {
                matched_rows += 1;
                matched_ds.insert(res.row_dataset[r]);
                for (k, col) in typed_cols.iter().enumerate() {
                    cells[k] = col[i].clone();
                }
            }
        }
        new_cells.push(cells);
    }
    // unmatched data sets (error rows included: they have no values anyway)
    let mut all_ds: BTreeSet<usize> = BTreeSet::new();
    all_ds.extend(res.row_dataset.iter().copied());
    let unmatched: Vec<usize> = all_ds
        .iter()
        .filter(|d| !matched_ds.contains(d))
        .copied()
        .collect();
    let unmatched_sheet: Vec<usize> = (0..sheet.rows.len()).filter(|&i| !used_sheet[i]).collect();
    // insert annotation columns after identity, keys and metadata
    let at = res
        .table
        .columns
        .iter()
        .position(|c| matches!(c.role, Role::Value | Role::Error | Role::Annotation))
        .map_or(res.table.columns.len(), |p| {
            // after existing annotations
            let mut q = p;
            while q < res.table.columns.len() && res.table.columns[q].role == Role::Annotation {
                q += 1;
            }
            q
        });
    for (k, n) in names.iter().enumerate() {
        let col_vals: Vec<Value> = new_cells.iter().map(|c| c[k].clone()).collect();
        let kind = crate::table::infer(col_vals.iter());
        res.table.columns.insert(
            at + k,
            Column {
                name: n.clone(),
                kind: if kind == ColumnType::Boolean {
                    ColumnType::String
                } else {
                    kind
                },
                unit: None,
                role: Role::Annotation,
                description: Some(format!(
                    "from {} column `{}`",
                    Path::new(&sheet.path)
                        .file_name()
                        .map_or_else(|| sheet.path.clone(), |f| f.to_string_lossy().into_owned()),
                    sheet.columns[ann_cols[k]]
                )),
            },
        );
        for (r, row) in res.table.rows.iter_mut().enumerate() {
            row.insert(at + k, col_vals[r].clone());
        }
    }
    let mut warnings = Vec::new();
    let datasets = all_ds.len() as u64;
    if !unmatched.is_empty() {
        warnings.push(format!(
            "{} of {datasets} data sets matched no sheet row: their annotation columns are empty",
            unmatched.len()
        ));
    }
    if !unmatched_sheet.is_empty() {
        warnings.push(format!(
            "{} of {} sheet rows matched no data set (a missing or misnamed file?)",
            unmatched_sheet.len(),
            sheet.rows.len()
        ));
    }
    if amb_n > 0 {
        warnings.push(format!(
            "{amb_n} rows matched several sheet rows that disagree and were left without annotations; add a key column with --key"
        ));
    }
    if auto
        && let Some(alt) = alternatives.first()
        && alt.0.matched_rows == chosen[0].0.matched_rows
        && alt.0.sheet_column != chosen[0].0.sheet_column
    {
        warnings.push(format!(
            "the key was a close call: `{}` = {} matches as many rows; pass --key to be explicit",
            alt.0.sheet_column,
            alt.0.field.name()
        ));
    }
    let key_desc = chosen
        .iter()
        .map(|c| format!("`{}` = {}", c.0.sheet_column, c.0.field.name()))
        .collect::<Vec<_>>()
        .join(" and ");
    res.warnings.push(format!(
        "joined {} on {key_desc}: {matched_rows} of {n_rows} rows annotated",
        Path::new(&sheet.path)
            .file_name()
            .map_or_else(|| sheet.path.clone(), |f| f.to_string_lossy().into_owned())
    ));
    let label_sheet_row = |i: usize| {
        let keys: Vec<String> = key_cols
            .iter()
            .map(|&c| sheet.rows[i].get(c).cloned().unwrap_or_default())
            .collect();
        format!("row {}: {}", i + 2, keys.join(" / "))
    };
    Ok(JoinReport {
        key: chosen.iter().map(|c| c.0.clone()).collect(),
        auto,
        alternatives: alternatives.iter().take(4).map(|a| a.0.clone()).collect(),
        annotations: names,
        matched_rows,
        rows: n_rows as u64,
        matched_datasets: matched_ds.len() as u64,
        datasets,
        unmatched_datasets: unmatched
            .iter()
            .take(20)
            .map(|&d| res.datasets[d].path.display().to_string())
            .collect(),
        unmatched_datasets_count: unmatched.len() as u64,
        unmatched_sheet_rows: unmatched_sheet
            .iter()
            .take(20)
            .map(|&i| label_sheet_row(i))
            .collect(),
        unmatched_sheet_rows_count: unmatched_sheet.len() as u64,
        ambiguous: amb_list,
        ambiguous_count: amb_n,
        warnings,
        sheet,
    })
}

fn row_label(res: &BatchResult, r: usize) -> String {
    let d = &res.datasets[res.row_dataset[r]];
    let mut s = d.relative.display().to_string();
    for g in &res.grain {
        let v = res.table.get(r, g);
        if !v.is_null() {
            s.push_str(&format!(" {g}={v}"));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{DatasetRef, SampleKeys};
    use crate::sheet::SheetKind;
    use crate::table::{Row, assemble};

    fn result(files: &[(&str, Option<&str>)], per_file_wells: &[&str]) -> BatchResult {
        let mut rows = Vec::new();
        let mut row_dataset = Vec::new();
        let mut datasets = Vec::new();
        for (i, (f, id)) in files.iter().enumerate() {
            datasets.push(DatasetRef {
                path: format!("/data/{f}").into(),
                relative: (*f).into(),
                format: Some("fcs".into()),
                members: vec![],
                keys: SampleKeys {
                    id: id.map(str::to_string),
                    started_at: Some(format!("2024-01-0{}T00:00:00Z", 9 - i)),
                    ..Default::default()
                },
            });
            if per_file_wells.is_empty() {
                rows.push(Row::new().with("path", *f).with("mean", i as f64));
                row_dataset.push(i);
            } else {
                for w in per_file_wells {
                    rows.push(
                        Row::new()
                            .with("path", *f)
                            .with("well", *w)
                            .with("value", 1.0),
                    );
                    row_dataset.push(i);
                }
            }
        }
        BatchResult {
            measure: "t".into(),
            grain: vec!["well".into()],
            table: assemble(rows, &[], &|n| {
                if n == "path" { Role::Id } else { Role::Value }
            }),
            datasets,
            row_dataset,
            ..Default::default()
        }
    }

    fn sheet(cols: &[&str], rows: &[&[&str]]) -> SampleSheet {
        SampleSheet {
            path: "s.csv".into(),
            worksheet: None,
            kind: SheetKind::Table,
            columns: cols.iter().map(|s| (*s).to_string()).collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|s| (*s).to_string()).collect())
                .collect(),
            plate_wells: None,
            notes: vec![],
        }
    }

    #[test]
    fn auto_picks_the_file_column_and_types_annotations() {
        let mut r = result(&[("a.fcs", None), ("b.fcs", None), ("c.fcs", None)], &[]);
        let s = sheet(
            &["condition", "File", "dose"],
            &[
                &["ctrl", "A.fcs", "0"],
                &["drug", "b", "1.5"],
                &["x", "zz.fcs", "2"],
            ],
        );
        let rep = join(&mut r, s, &JoinOptions::default()).unwrap();
        assert_eq!(rep.key[0].sheet_column, "File");
        assert_eq!(
            rep.key[0].field,
            KeyField::Stem,
            "names with and without extension"
        );
        assert_eq!(rep.matched_rows, 2);
        let mut r = result(&[("a.fcs", None), ("b.fcs", None), ("c.fcs", None)], &[]);
        let s = sheet(
            &["condition", "File", "dose"],
            &[
                &["ctrl", "a", "0"],
                &["drug", "b", "1.5"],
                &["x", "zz", "2"],
            ],
        );
        let rep = join(&mut r, s, &JoinOptions::default()).unwrap();
        assert_eq!(rep.key[0].field, KeyField::Stem);
        assert_eq!(rep.matched_rows, 2);
        assert_eq!(rep.unmatched_datasets, ["/data/c.fcs"]);
        assert_eq!(rep.unmatched_sheet_rows_count, 1);
        assert_eq!(r.table.get(1, "dose"), &Value::Float(1.5));
        assert_eq!(r.table.get(0, "condition"), &Value::Text("ctrl".into()));
        assert_eq!(r.table.get(2, "condition"), &Value::Null);
        let names: Vec<&str> = r.table.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["path", "condition", "dose", "mean"]);
    }

    #[test]
    fn globs_sample_ids_and_run_order() {
        let mut r = result(
            &[("ctrl_1.fcs", Some("S-01")), ("drug_1.fcs", Some("S-02"))],
            &[],
        );
        let s = sheet(
            &["pattern", "group"],
            &[&["ctrl_*", "control"], &["drug_*", "treated"]],
        );
        let rep = join(&mut r, s, &JoinOptions::default()).unwrap();
        assert_eq!(rep.matched_rows, 2);
        assert_eq!(r.table.get(1, "group"), &Value::Text("treated".into()));
        let mut r = result(&[("x.fcs", Some("S-01")), ("y.fcs", Some("S-02"))], &[]);
        let s = sheet(&["Sample", "donor"], &[&["s-02", "3"], &["S-01", "1"]]);
        let rep = join(&mut r, s, &JoinOptions::default()).unwrap();
        assert_eq!(rep.key[0].field, KeyField::SampleId);
        assert_eq!(r.table.get(0, "donor"), &Value::Int(1));
        // started_at puts y.fcs first
        let mut r = result(&[("x.fcs", None), ("y.fcs", None)], &[]);
        let s = sheet(
            &["injection_order", "arm"],
            &[&["1", "early"], &["2", "late"]],
        );
        let rep = join(&mut r, s, &JoinOptions::default()).unwrap();
        assert_eq!(rep.key[0].field, KeyField::RunOrder);
        assert_eq!(r.table.get(1, "arm"), &Value::Text("early".into()));
    }

    #[test]
    fn plate_wells_and_composite_keys() {
        let mut r = result(&[("p1.csv", None), ("p2.csv", None)], &["A1", "A2"]);
        let s = sheet(
            &["plate", "well", "treatment"],
            &[
                &["p1", "A01", "x"],
                &["p1", "A02", "y"],
                &["p2", "A01", "z"],
                &["p2", "A02", "w"],
            ],
        );
        let rep = join(&mut r, s, &JoinOptions::default()).unwrap();
        assert_eq!(rep.ambiguous_count, 0, "{rep:?}");
        assert_eq!(rep.key.len(), 2);
        let t: Vec<String> = (0..4).map(|i| r.table.get(i, "treatment").text()).collect();
        assert_eq!(t, ["x", "y", "z", "w"]);
    }

    #[test]
    fn ambiguous_rows_are_reported_not_guessed() {
        let mut r = result(&[("a.fcs", None)], &[]);
        let s = sheet(&["file", "group"], &[&["a.fcs", "x"], &["a.fcs", "y"]]);
        let rep = join(&mut r, s, &JoinOptions::default()).unwrap();
        assert_eq!(rep.ambiguous_count, 1);
        assert_eq!(r.table.get(0, "group"), &Value::Null);
        // identical duplicates are fine
        let mut r = result(&[("a.fcs", None)], &[]);
        let s = sheet(&["file", "group"], &[&["a.fcs", "x"], &["a.fcs", "x"]]);
        let rep = join(&mut r, s, &JoinOptions::default()).unwrap();
        assert_eq!(rep.ambiguous_count, 0);
        assert_eq!(r.table.get(0, "group"), &Value::Text("x".into()));
    }

    #[test]
    fn explicit_keys_and_no_match_errors() {
        let mut r = result(&[("a.fcs", None)], &[]);
        let s = sheet(&["id", "g"], &[&["nothing", "x"]]);
        assert_eq!(
            join(&mut r, s, &JoinOptions::default())
                .unwrap_err()
                .exit_code(),
            2
        );
        let mut r = result(&[("a.fcs", None)], &[]);
        let s = sheet(&["id", "g"], &[&["a.fcs", "x"]]);
        let o = JoinOptions::parse_keys(&["id=file".into()]).unwrap();
        let rep = join(&mut r, s, &o).unwrap();
        assert!(!rep.auto);
        assert_eq!(r.table.get(0, "g"), &Value::Text("x".into()));
        assert!(JoinOptions::parse_keys(&["id".into()]).is_err());
        assert!(JoinOptions::parse_keys(&["id=bogus".into()]).is_err());
    }
}
