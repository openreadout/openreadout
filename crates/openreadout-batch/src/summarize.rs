//! Group summaries of a batch table: n, mean, sd, sem, median, min, max and CV % per group and
//! value column, optionally averaging technical replicates first and comparing every group with
//! a control group (Welch's t-test or Mann–Whitney U).
//!
//! Groups are the distinct combinations of the `by` columns plus the table's *measurement
//! dimensions* that vary (channel, parameter, population, read, trace, unit, …): a mean over
//! the GFP and DAPI channels together is never what anyone wants. Images, wells, sweeps and
//! files are pooled. Rows with an error or without a value are left out and counted.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use openreadout_core::{Error, Result};
use serde::Serialize;

use crate::stat::{describe, mann_whitney, welch};
use crate::table::{Column, ColumnType, Role, Table, Value};

/// Columns that name *what* was measured: kept apart in summaries whenever they vary. Each
/// family lists alternatives, best first: the first that every row fills is used (a channel's
/// name before its index, since index 0 may be a different dye in another file; an FCS
/// parameter's `$PnN` before its `$PnS` label, which files word differently).
pub const DIMENSION_FAMILIES: &[&[&str]] = &[
    &["channel_name", "channel"],
    &["parameter", "label"],
    &["population"],
    &["trace_name", "trace"],
    &["read"],
    &["wavelength_nm"],
    &["unit"],
    &["x_unit"],
    &["t"],
    &["table"],
];

/// Every column name in [`DIMENSION_FAMILIES`].
pub const DIMENSIONS: &[&str] = &[
    "channel_name",
    "channel",
    "parameter",
    "label",
    "population",
    "trace_name",
    "trace",
    "read",
    "wavelength_nm",
    "unit",
    "x_unit",
    "t",
    "table",
];

/// Two-sample test against the control group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestKind {
    /// Welch's unequal-variance t-test.
    Welch,
    /// Mann–Whitney U (Wilcoxon rank-sum).
    MannWhitney,
}

impl TestKind {
    /// Parse `welch`/`t` or `mann-whitney`/`mwu`/`wilcoxon`.
    pub fn parse(s: &str) -> Option<TestKind> {
        match s.to_ascii_lowercase().replace('_', "-").as_str() {
            "welch" | "t" | "t-test" | "ttest" => Some(TestKind::Welch),
            "mann-whitney" | "mwu" | "wilcoxon" | "rank-sum" | "u" => Some(TestKind::MannWhitney),
            _ => None,
        }
    }
    fn name(self) -> &'static str {
        match self {
            TestKind::Welch => "welch",
            TestKind::MannWhitney => "mann-whitney",
        }
    }
}

/// A row filter: `column=value` or `column!=value` (case-insensitive text comparison; numbers
/// compare numerically).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// Column.
    pub column: String,
    /// Value.
    pub value: String,
    /// True for `!=`.
    pub negate: bool,
}

impl Filter {
    /// Parse `col=value` / `col!=value`.
    pub fn parse(s: &str) -> Result<Filter> {
        if let Some((c, v)) = s.split_once("!=") {
            return Ok(Filter {
                column: c.trim().into(),
                value: v.trim().into(),
                negate: true,
            });
        }
        let (c, v) = s.split_once('=').ok_or_else(|| {
            Error::Usage(format!(
                "--where {s}: expected COLUMN=VALUE or COLUMN!=VALUE"
            ))
        })?;
        Ok(Filter {
            column: c.trim().into(),
            value: v.trim().into(),
            negate: false,
        })
    }
    /// Does a cell pass the filter?
    #[allow(clippy::float_cmp)] // `dose=10` selects exactly the value 10
    pub fn keeps(&self, v: &Value) -> bool {
        let eq = match (v.as_f64(), crate::table::parse_number(&self.value)) {
            (Some(a), Some(b)) if !matches!(v, Value::Text(_)) => a == b,
            _ => v.text().trim().eq_ignore_ascii_case(self.value.trim()),
        };
        eq != self.negate
    }
}

/// What to summarize.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SummarizeRequest {
    /// Grouping columns (annotations such as `condition`, `dose`, `donor`).
    pub by: Vec<String>,
    /// Value columns (default: the measure's primary values, else every numeric value column).
    pub values: Vec<String>,
    /// Average rows within each replicate (a column such as `replicate` or `well`) first; `n`
    /// then counts replicates.
    pub replicate: Option<String>,
    /// Compare every group with the control group.
    pub test: Option<TestKind>,
    /// The control: a value of the first `by` column.
    pub control: Option<String>,
    /// Only rows passing every filter.
    pub filters: Vec<Filter>,
    /// Group by exactly `by` (do not add varying measurement dimensions).
    pub exact_by: bool,
}

/// A summary: one row per group and value column.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SummaryOutput {
    /// Columns that define a group: `by` plus the measurement dimensions added.
    pub group_by: Vec<String>,
    /// Dimensions added automatically.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added_dimensions: Vec<String>,
    /// Value columns summarized.
    pub values: Vec<String>,
    /// Replicate column, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicate: Option<String>,
    /// Test and control, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test: Option<String>,
    /// The control group (value of the first `by` column).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
    /// Table rows used / left out (errors, filters, missing values).
    pub rows_used: u64,
    /// Rows left out by filters or errors.
    pub rows_excluded: u64,
    /// The summary table.
    pub table: Table,
    /// Things to know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Default value columns of a batch measure (what a scientist reports).
pub fn primary_values(measure: &str, t: &Table) -> Vec<String> {
    let want: &[&str] = match measure {
        "stats" => &["mean", "median"],
        "trace" => &["mean", "max"],
        "table" => &["median", "value"],
        "gate" => &["count", "percent_of_parent"],
        // the analysis measures (measures/analysis.rs): the values a scientist compares first
        "peaks" => &["area", "height", "main_area", "main_area_percent"],
        "chromatogram" => &["apex_rt_min", "apex_intensity"],
        "assay" => &[
            "value",
            "back_calculated",
            "final_concentration",
            "ec50",
            "max_slope_per_min",
            "doubling_time_h",
            "z_prime",
            "mean",
        ],
        "nmr-peaks" => &["ppm", "height", "value", "main_peak_ppm"],
        "ephys-features" => &[
            "spike_count",
            "rheobase_pa",
            "input_resistance_mohm",
            "peak_mv",
        ],
        "spikes" => &["spike_count", "rate_hz"],
        "qpcr" => &["cq", "rq", "efficiency_percent"],
        "well-stats" => &["mean", "median"],
        _ => &[],
    };
    let mut out: Vec<String> = want
        .iter()
        .filter(|w| t.index_of(w).is_some())
        .map(|w| (*w).to_string())
        .collect();
    if measure == "gate" {
        out.extend(
            t.columns
                .iter()
                .filter(|c| c.name.starts_with("median:"))
                .map(|c| c.name.clone()),
        );
    }
    if out.is_empty() {
        out = t
            .columns
            .iter()
            .filter(|c| {
                c.role == Role::Value
                    && matches!(c.kind, ColumnType::Integer | ColumnType::Float)
                    && !DIMENSIONS.contains(&c.name.as_str())
            })
            .map(|c| c.name.clone())
            .collect();
    }
    out
}

fn cmp_values(a: &Value, b: &Value) -> Ordering {
    match (a.is_null(), b.is_null()) {
        (true, true) => return Ordering::Equal,
        (true, false) => return Ordering::Greater,
        (false, true) => return Ordering::Less,
        _ => {}
    }
    match (a, b) {
        (Value::Text(x), Value::Text(y)) => natural(x, y),
        _ => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x.total_cmp(&y),
            _ => natural(&a.text(), &b.text()),
        },
    }
}

/// Case-insensitive order with digit runs compared as numbers (`donor 2` < `donor 10`).
fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    n1.push(c);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(d) = y.peek().copied().filter(char::is_ascii_digit) {
                    n2.push(d);
                    y.next();
                }
                let o = n1
                    .trim_start_matches('0')
                    .len()
                    .cmp(&n2.trim_start_matches('0').len())
                    .then_with(|| n1.trim_start_matches('0').cmp(n2.trim_start_matches('0')));
                if o != Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}

#[derive(Clone)]
struct Key(Vec<Value>);

impl PartialEq for Key {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Key {
    fn cmp(&self, o: &Self) -> Ordering {
        for (a, b) in self.0.iter().zip(&o.0) {
            let c = cmp_values(a, b);
            if c != Ordering::Equal {
                return c;
            }
        }
        Ordering::Equal
    }
}

/// Summarize `t` (`measure`: the batch measure that made it, for the default value columns).
pub fn summarize(t: &Table, measure: &str, req: &SummarizeRequest) -> Result<SummaryOutput> {
    let col = |n: &str| {
        t.index_of(n).ok_or_else(|| {
            Error::Usage(format!(
                "no column `{n}` in the table; columns: {}",
                t.columns
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
    };
    if req.by.is_empty() {
        return Err(Error::Usage(
            "pass --by COLUMN (a sample-sheet column such as condition, dose or donor)".into(),
        ));
    }
    let by_idx: Vec<usize> = req.by.iter().map(|b| col(b)).collect::<Result<_>>()?;
    let filters: Vec<(usize, &Filter)> = req
        .filters
        .iter()
        .map(|f| col(&f.column).map(|i| (i, f)))
        .collect::<Result<_>>()?;
    let err_col = t.index_of("error");
    let rows: Vec<usize> = (0..t.rows.len())
        .filter(|&r| err_col.is_none_or(|e| t.rows[r][e].is_null()))
        .filter(|&r| filters.iter().all(|(i, f)| f.keeps(&t.rows[r][*i])))
        .collect();
    let excluded = (t.rows.len() - rows.len()) as u64;
    // measurement dimensions that vary among the rows used
    let mut added = Vec::new();
    if !req.exact_by {
        for family in DIMENSION_FAMILIES {
            if family.iter().any(|d| req.by.iter().any(|b| b == d)) {
                continue;
            }
            let pick = family.iter().find_map(|d| {
                let i = t.index_of(d)?;
                rows.iter()
                    .all(|&r| !t.rows[r][i].is_null())
                    .then_some((*d, i))
            });
            if let Some((d, i)) = pick {
                let first = rows.first().map(|&r| &t.rows[r][i]);
                if rows.iter().any(|&r| Some(&t.rows[r][i]) != first) {
                    added.push(d.to_string());
                }
            }
        }
    }
    let group_cols: Vec<String> = req
        .by
        .iter()
        .cloned()
        .chain(added.iter().cloned())
        .collect();
    let g_idx: Vec<usize> = group_cols.iter().map(|g| col(g)).collect::<Result<_>>()?;
    let values: Vec<String> = if req.values.is_empty() {
        primary_values(measure, t)
    } else {
        req.values
            .iter()
            .flat_map(|v| v.split(','))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect()
    };
    if values.is_empty() {
        return Err(Error::Usage(
            "no numeric value column to summarize; name one with --value".into(),
        ));
    }
    let v_idx: Vec<usize> = values.iter().map(|v| col(v)).collect::<Result<_>>()?;
    let rep_idx = req.replicate.as_deref().map(col).transpose()?;
    let mut notes = Vec::new();
    for (v, &i) in values.iter().zip(&v_idx) {
        if !rows.iter().any(|&r| t.rows[r][i].as_f64().is_some()) {
            notes.push(format!("`{v}` has no numeric value in the rows used"));
        }
    }
    // group → value → observations (replicate means when asked)
    let mut obs: BTreeMap<Key, Vec<Vec<f64>>> = BTreeMap::new();
    let mut rows_per_group: BTreeMap<Key, u64> = BTreeMap::new();
    let mut null_groups = 0u64;
    if let Some(ri) = rep_idx {
        let mut reps: BTreeMap<(Key, Key), Vec<Vec<f64>>> = BTreeMap::new();
        for &r in &rows {
            let k = Key(g_idx.iter().map(|&i| t.rows[r][i].clone()).collect());
            if k.0.iter().take(by_idx.len()).any(Value::is_null) {
                null_groups += 1;
            }
            *rows_per_group.entry(k.clone()).or_default() += 1;
            let e = reps
                .entry((k, Key(vec![t.rows[r][ri].clone()])))
                .or_insert_with(|| vec![Vec::new(); v_idx.len()]);
            for (j, &vi) in v_idx.iter().enumerate() {
                if let Some(x) = t.rows[r][vi].as_f64() {
                    e[j].push(x);
                }
            }
        }
        for ((k, _), vals) in reps {
            let e = obs
                .entry(k)
                .or_insert_with(|| vec![Vec::new(); v_idx.len()]);
            for (j, v) in vals.into_iter().enumerate() {
                if let Some(m) = describe(&v).mean {
                    e[j].push(m);
                }
            }
        }
    } else {
        for &r in &rows {
            let k = Key(g_idx.iter().map(|&i| t.rows[r][i].clone()).collect());
            if k.0.iter().take(by_idx.len()).any(Value::is_null) {
                null_groups += 1;
            }
            *rows_per_group.entry(k.clone()).or_default() += 1;
            let e = obs
                .entry(k)
                .or_insert_with(|| vec![Vec::new(); v_idx.len()]);
            for (j, &vi) in v_idx.iter().enumerate() {
                if let Some(x) = t.rows[r][vi].as_f64() {
                    e[j].push(x);
                }
            }
        }
    }
    if null_groups > 0 {
        notes.push(format!(
            "{null_groups} rows have no value in {} (not matched by the sample sheet?): they form the empty group",
            req.by.join("/")
        ));
    }
    // control lookup
    let control = match (&req.test, &req.control) {
        (Some(_), None) => {
            return Err(Error::Usage(
                "--test needs --control VALUE (a value of the first --by column)".into(),
            ));
        }
        (None, Some(_)) => {
            return Err(Error::Usage(
                "--control needs --test welch|mann-whitney".into(),
            ));
        }
        (Some(_), Some(c)) => {
            let present = obs
                .keys()
                .any(|k| Filter::parse(&format!("x={c}")).is_ok_and(|f| f.keeps(&k.0[0])));
            if !present {
                return Err(Error::Usage(format!(
                    "--control {c}: no group has {} = {c}",
                    req.by[0]
                )));
            }
            Some(c.clone())
        }
        _ => None,
    };
    let is_control = |v: &Value| {
        control.as_ref().is_some_and(|c| {
            Filter {
                column: String::new(),
                value: c.clone(),
                negate: false,
            }
            .keeps(v)
        })
    };
    // output
    let mut out_rows: Vec<Vec<Value>> = Vec::new();
    for (k, per_value) in &obs {
        for (j, v) in values.iter().enumerate() {
            let d = describe(&per_value[j]);
            let mut row: Vec<Value> = k.0.clone();
            row.push(Value::Text(v.clone()));
            row.push(Value::count(d.n as u64));
            if rep_idx.is_some() {
                row.push(Value::count(*rows_per_group.get(k).unwrap_or(&0)));
            }
            for x in [d.mean, d.sd, d.sem, d.median, d.min, d.max, d.cv_percent] {
                row.push(Value::float_opt(x));
            }
            if let Some(test) = req.test {
                let ctrl_key = || {
                    obs.iter().find(|(ck, _)| {
                        is_control(&ck.0[0])
                            && ck
                                .0
                                .iter()
                                .skip(1)
                                .zip(k.0.iter().skip(1))
                                .all(|(a, b)| cmp_values(a, b) == Ordering::Equal)
                    })
                };
                match ctrl_key() {
                    Some((ck, cv)) if !is_control(&k.0[0]) => {
                        let c = &cv[j];
                        let cd = describe(c);
                        row.push(ck.0[0].clone());
                        row.push(Value::float_opt(d.mean.zip(cd.mean).map(|(a, b)| a - b)));
                        row.push(Value::float_opt(
                            d.mean
                                .zip(cd.mean)
                                .filter(|(_, b)| *b != 0.0)
                                .map(|(a, b)| a / b),
                        ));
                        let r = match test {
                            TestKind::Welch => welch(&per_value[j], c),
                            TestKind::MannWhitney => mann_whitney(&per_value[j], c),
                        };
                        row.push(Value::float_opt(r.map(|r| r.statistic)));
                        row.push(Value::float_opt(r.and_then(|r| r.df)));
                        row.push(Value::float_opt(r.map(|r| r.p_value)));
                        row.push(Value::text_opt(r.map(|r| r.method)));
                    }
                    _ => row.extend(std::iter::repeat_n(Value::Null, 7)),
                }
            }
            out_rows.push(row);
        }
    }
    let mut cols: Vec<Column> = group_cols
        .iter()
        .map(|g| {
            let src = &t.columns[t.index_of(g).unwrap_or(0)];
            Column {
                name: g.clone(),
                kind: src.kind,
                unit: src.unit.clone(),
                role: Role::Key,
                description: src.description.clone(),
            }
        })
        .collect();
    let mut add = |name: &str, kind: ColumnType, description: &str| {
        cols.push(Column {
            name: name.into(),
            kind,
            unit: None,
            role: Role::Value,
            description: Some(description.into()),
        });
    };
    add("value", ColumnType::String, "the column summarized");
    add(
        "n",
        ColumnType::Integer,
        if rep_idx.is_some() {
            "replicates with a value (each averaged over its rows)"
        } else {
            "rows with a value"
        },
    );
    if rep_idx.is_some() {
        add("n_rows", ColumnType::Integer, "table rows in the group");
    }
    add("mean", ColumnType::Float, "mean");
    add("sd", ColumnType::Float, "sample standard deviation (n − 1)");
    add("sem", ColumnType::Float, "standard error of the mean");
    add("median", ColumnType::Float, "median");
    add("min", ColumnType::Float, "smallest");
    add("max", ColumnType::Float, "largest");
    add("cv_percent", ColumnType::Float, "sd / |mean| × 100");
    if let Some(test) = req.test {
        add(
            "control",
            ColumnType::String,
            "the control group compared with",
        );
        add("diff", ColumnType::Float, "mean − control mean");
        add("ratio", ColumnType::Float, "mean / control mean");
        add(
            "statistic",
            ColumnType::Float,
            match test {
                TestKind::Welch => "Welch's t",
                TestKind::MannWhitney => "Mann–Whitney U of this group",
            },
        );
        add("df", ColumnType::Float, "degrees of freedom (Welch)");
        add(
            "p_value",
            ColumnType::Float,
            "two-sided p-value (not corrected for multiple comparisons)",
        );
        add(
            "test_method",
            ColumnType::String,
            "welch, exact or asymptotic",
        );
    }
    let mut table = Table {
        columns: cols,
        rows: out_rows,
    };
    table.infer_types();
    if req.test.is_some() {
        notes.push("p-values are two-sided and not corrected for multiple comparisons".to_string());
    }
    Ok(SummaryOutput {
        group_by: group_cols,
        added_dimensions: added,
        values,
        replicate: req.replicate.clone(),
        test: req.test.map(|t| t.name().to_string()),
        control,
        rows_used: rows.len() as u64,
        rows_excluded: excluded,
        table,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Row, assemble};

    fn table() -> Table {
        let mut rows = Vec::new();
        for (cond, rep, ch, v) in [
            ("ctrl", 1, "GFP", 1.0),
            ("ctrl", 1, "GFP", 3.0),
            ("ctrl", 2, "GFP", 2.0),
            ("ctrl", 3, "GFP", 4.0),
            ("drug", 1, "GFP", 10.0),
            ("drug", 2, "GFP", 12.0),
            ("drug", 3, "GFP", 14.0),
            ("ctrl", 1, "DAPI", 100.0),
            ("drug", 1, "DAPI", 100.0),
        ] {
            rows.push(
                Row::new()
                    .with("path", format!("{cond}{rep}"))
                    .with("channel_name", ch)
                    .with("condition", cond)
                    .with("replicate", rep as i64)
                    .with("mean", v),
            );
        }
        rows.push(
            Row::new()
                .with("path", "bad")
                .with("error", "corrupt")
                .with("condition", "ctrl"),
        );
        assemble(rows, &[], &|n| match n {
            "path" => Role::Id,
            "channel_name" => Role::Key,
            "condition" | "replicate" => Role::Annotation,
            "error" => Role::Error,
            _ => Role::Value,
        })
    }

    #[test]
    fn groups_keep_channels_apart_and_skip_errors() {
        let req = SummarizeRequest {
            by: vec!["condition".into()],
            ..Default::default()
        };
        let s = summarize(&table(), "stats", &req).unwrap();
        assert_eq!(s.group_by, ["condition", "channel_name"]);
        assert_eq!(s.rows_excluded, 1);
        // ctrl/DAPI, ctrl/GFP, drug/DAPI, drug/GFP × mean (median absent)
        assert_eq!(s.values, ["mean"]);
        let t = &s.table;
        assert_eq!(t.rows.len(), 4);
        assert_eq!(t.get(1, "channel_name"), &Value::Text("GFP".into()));
        assert_eq!(t.get(1, "n"), &Value::Int(4));
        assert_eq!(t.get(1, "mean"), &Value::Float(2.5));
        assert_eq!(t.get(1, "median"), &Value::Float(2.5));
    }

    #[test]
    fn replicates_are_averaged_first_and_tests_run_against_the_control() {
        let req = SummarizeRequest {
            by: vec!["condition".into()],
            replicate: Some("replicate".into()),
            filters: vec![Filter::parse("channel_name=gfp").unwrap()],
            test: Some(TestKind::Welch),
            control: Some("ctrl".into()),
            ..Default::default()
        };
        let s = summarize(&table(), "stats", &req).unwrap();
        let t = &s.table;
        assert_eq!(t.rows.len(), 2);
        // ctrl replicate means: 2, 2, 4
        assert_eq!(t.get(0, "n"), &Value::Int(3));
        assert_eq!(t.get(0, "n_rows"), &Value::Int(4));
        assert!((t.get(0, "mean").as_f64().unwrap() - 8.0 / 3.0).abs() < 1e-12);
        assert_eq!(t.get(0, "p_value"), &Value::Null);
        assert_eq!(t.get(1, "control"), &Value::Text("ctrl".into()));
        assert!((t.get(1, "diff").as_f64().unwrap() - (12.0 - 8.0 / 3.0)).abs() < 1e-12);
        assert!(t.get(1, "p_value").as_f64().unwrap() < 0.01);
    }

    #[test]
    fn usage_errors() {
        let t = table();
        let mut req = SummarizeRequest::default();
        assert!(summarize(&t, "stats", &req).is_err());
        req.by = vec!["nope".into()];
        assert_eq!(summarize(&t, "stats", &req).unwrap_err().exit_code(), 2);
        req.by = vec!["condition".into()];
        req.test = Some(TestKind::Welch);
        assert!(summarize(&t, "stats", &req).is_err());
        req.control = Some("absent".into());
        assert!(summarize(&t, "stats", &req).is_err());
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural("donor 2", "donor 10"), Ordering::Less);
        assert_eq!(natural("B", "a"), Ordering::Greater);
    }
}
