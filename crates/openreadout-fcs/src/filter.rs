//! `table --filter 'FITC-A > 1000' --count`: rows of a whole table that meet conditions, counted
//! (and paged) without exporting. Works on any reader's table; FCS values can be compensated
//! and transformed first (the conditions then test the processed values).

use std::path::Path;

use openreadout_core::model::{TableFilterSummary, TableSlice};
use openreadout_core::{Error, Registry, Result};

use crate::FORMAT_ID;
use crate::analysis::{TableOptions, processed_rows};

/// Rows read per block while testing a whole table.
const FILTER_CHUNK_ROWS: u64 = 1 << 18;

/// A comparison operator of a row condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    /// `>`
    Greater,
    /// `>=`
    GreaterEqual,
    /// `<`
    Less,
    /// `<=`
    LessEqual,
    /// `==` (or `=`)
    Equal,
    /// `!=`
    NotEqual,
}

impl CompareOp {
    /// The operator as written.
    pub fn symbol(self) -> &'static str {
        match self {
            CompareOp::Greater => ">",
            CompareOp::GreaterEqual => ">=",
            CompareOp::Less => "<",
            CompareOp::LessEqual => "<=",
            CompareOp::Equal => "==",
            CompareOp::NotEqual => "!=",
        }
    }

    /// `value OP threshold`; false for NaN.
    pub fn test(self, value: f64, threshold: f64) -> bool {
        match self {
            CompareOp::Greater => value > threshold,
            CompareOp::GreaterEqual => value >= threshold,
            CompareOp::Less => value < threshold,
            CompareOp::LessEqual => value <= threshold,
            #[allow(clippy::float_cmp)] // an exact test is what `==` asks for
            CompareOp::Equal => value == threshold,
            #[allow(clippy::float_cmp)]
            CompareOp::NotEqual => !value.is_nan() && value != threshold,
        }
    }
}

/// One row condition: `column OP threshold`.
#[derive(Debug, Clone, PartialEq)]
pub struct RowCondition {
    /// Column name (`$PnN`), label (`$PnS`) or `gate:<path>` column, as given.
    pub column: String,
    /// The comparison.
    pub op: CompareOp,
    /// The number compared against.
    pub threshold: f64,
}

impl std::fmt::Display for RowCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {} {}", self.column, self.op.symbol(), self.threshold)
    }
}

/// Parse `FITC-A > 1000`. Several conditions in one text are joined with `&&` or ` and `.
pub fn parse_conditions(texts: &[String]) -> Result<Vec<RowCondition>> {
    let mut out = Vec::new();
    for t in texts {
        for part in split_and(t) {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            out.push(parse_condition(part)?);
        }
    }
    Ok(out)
}

fn split_and(t: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for a in t.split("&&") {
        let lower = a.to_ascii_lowercase();
        let mut start = 0;
        let mut search = 0;
        while let Some(i) = lower[search..].find(" and ") {
            let at = search + i;
            parts.push(a[start..at].to_string());
            start = at + 5;
            search = start;
        }
        parts.push(a[start..].to_string());
    }
    parts
}

fn parse_condition(text: &str) -> Result<RowCondition> {
    const OPS: [(&str, CompareOp); 7] = [
        (">=", CompareOp::GreaterEqual),
        ("<=", CompareOp::LessEqual),
        ("!=", CompareOp::NotEqual),
        ("==", CompareOp::Equal),
        (">", CompareOp::Greater),
        ("<", CompareOp::Less),
        ("=", CompareOp::Equal),
    ];
    let bad = || {
        Error::Usage(format!(
            "condition `{text}`: write COLUMN OP NUMBER with OP one of > >= < <= == != (e.g. `FITC-A > 1000`)"
        ))
    };
    // the first operator character: column names hold `-`, `:`, `/` or spaces, never these
    let i = text.find(['<', '>', '=', '!']).ok_or_else(bad)?;
    let (sym, op) = OPS
        .iter()
        .find(|(s, _)| text[i..].starts_with(s))
        .copied()
        .ok_or_else(bad)?;
    let column = text[..i].trim();
    let value = text[i + sym.len()..].trim().replace('_', "");
    if column.is_empty() {
        return Err(bad());
    }
    let threshold: f64 = value.parse().map_err(|_| bad())?;
    Ok(RowCondition {
        column: column.to_string(),
        op,
        threshold,
    })
}

/// Column index of a condition: exact name, then label, then case-insensitive name or label.
fn column_index(c: &RowCondition, names: &[String], labels: &[Option<String>]) -> Result<usize> {
    names
        .iter()
        .position(|n| n == &c.column)
        .or_else(|| labels.iter().position(|l| l.as_deref() == Some(&c.column)))
        .or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(&c.column)))
        .or_else(|| {
            labels.iter().position(|l| {
                l.as_deref()
                    .is_some_and(|l| l.eq_ignore_ascii_case(&c.column))
            })
        })
        .ok_or_else(|| {
            Error::Usage(format!(
                "condition `{c}`: no column `{}`; columns: {}",
                c.column,
                names.join(", ")
            ))
        })
}

/// One block of a table, column-major.
struct Block {
    names: Vec<String>,
    labels: Vec<Option<String>>,
    columns: Vec<Vec<f64>>,
    total_rows: u64,
    processing: Option<openreadout_core::flow::TableProcessing>,
}

/// `table` with row conditions: every row of the whole table is tested; the matching rows from
/// the `first_match`-th on (at most `max_rows`) are returned, and `filter` counts them all.
/// `max_rows` 0 counts only.
pub fn filtered_slice(
    reg: &Registry,
    file: &Path,
    table: u32,
    first_match: u64,
    max_rows: u64,
    opts: &TableOptions,
    conditions: &[RowCondition],
) -> Result<TableSlice> {
    let (det, mut ds) = reg.open(file)?;
    let processed = opts.is_active();
    if processed && det.format_id != FORMAT_ID {
        return Err(Error::Usage(format!(
            "compensation, transforms and gates apply to FCS files; this is {}",
            det.format_id
        )));
    }
    let info = if processed { None } else { Some(ds.info()?) };
    let tinfo = match &info {
        Some(i) => Some(
            i.tables
                .iter()
                .find(|t| t.index == table)
                .cloned()
                .ok_or_else(|| {
                    Error::Usage(format!(
                        "table {table} not found (file has {} tables)",
                        i.tables.len()
                    ))
                })?,
        ),
        None => None,
    };
    let mut read_block = |first: u64, n: u64| -> Result<Block> {
        if processed {
            let p = processed_rows(file, table, first, n, opts)?;
            Ok(Block {
                names: p.names,
                labels: p.labels,
                columns: p.columns,
                total_rows: p.total_rows,
                processing: Some(p.processing),
            })
        } else {
            let t = tinfo
                .as_ref()
                .ok_or_else(|| Error::Usage("no table".into()))?;
            let tab = ds.read_table(table, first, n)?;
            Ok(Block {
                names: t.columns.iter().map(|c| c.name.clone()).collect(),
                labels: t.columns.iter().map(|c| c.label.clone()).collect(),
                columns: tab.columns,
                total_rows: t.row_count,
                processing: None,
            })
        }
    };
    let mut first = read_block(0, FILTER_CHUNK_ROWS)?;
    let cols: Vec<usize> = conditions
        .iter()
        .map(|c| column_index(c, &first.names, &first.labels))
        .collect::<Result<_>>()?;
    let total = first.total_rows;
    let mut matched = 0u64;
    let mut rows: Vec<Vec<f64>> = Vec::new();
    let mut at = 0u64;
    loop {
        let n = first.columns.first().map_or(0, Vec::len);
        for r in 0..n {
            let ok = conditions
                .iter()
                .zip(&cols)
                .all(|(c, &k)| c.op.test(first.columns[k][r], c.threshold));
            if !ok {
                continue;
            }
            if matched >= first_match && (rows.len() as u64) < max_rows {
                rows.push(first.columns.iter().map(|c| c[r]).collect());
            }
            matched += 1;
        }
        at += n as u64;
        if n == 0 || at >= total {
            break;
        }
        let names = std::mem::take(&mut first.names);
        let labels = std::mem::take(&mut first.labels);
        let processing = first.processing.take();
        first = read_block(at, FILTER_CHUNK_ROWS)?;
        first.names = names;
        first.labels = labels;
        first.processing = processing;
    }
    #[allow(clippy::cast_precision_loss)]
    let percent = if total == 0 {
        0.0
    } else {
        matched as f64 / total as f64 * 100.0
    };
    Ok(TableSlice {
        path: file.display().to_string(),
        format: det.format_id.into(),
        table,
        first_row: first_match,
        total_rows: total,
        columns: first.names,
        labels: first.labels,
        truncated: first_match + (rows.len() as u64) < matched,
        rows,
        processing: first.processing,
        filter: Some(TableFilterSummary {
            conditions: conditions.iter().map(ToString::to_string).collect(),
            matched_rows: matched,
            total_rows: total,
            percent,
            values: if processed { "processed" } else { "raw" }.into(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_parse() {
        let c = parse_conditions(&["FITC-A > 1000".into()]).unwrap();
        assert_eq!(c[0].column, "FITC-A");
        assert_eq!(c[0].op, CompareOp::Greater);
        assert!((c[0].threshold - 1000.0).abs() < 1e-12);
        let c = parse_conditions(&[
            "FSC-A>=5e4 && SSC-A < 1_000".into(),
            "gate:/Lymph = 1".into(),
        ])
        .unwrap();
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].op, CompareOp::GreaterEqual);
        assert!((c[0].threshold - 5e4).abs() < 1e-9);
        assert_eq!(c[1].op, CompareOp::Less);
        assert!((c[1].threshold - 1000.0).abs() < 1e-12);
        assert_eq!(c[2].column, "gate:/Lymph");
        assert_eq!(c[2].op, CompareOp::Equal);
        let c = parse_conditions(&["CD4 PE-A <= -3 and CD8 != 0".into()]).unwrap();
        assert_eq!(c[0].column, "CD4 PE-A");
        assert_eq!(c[0].op, CompareOp::LessEqual);
        assert!((c[0].threshold + 3.0).abs() < 1e-12);
        assert_eq!(c[1].op, CompareOp::NotEqual);
        assert!(parse_conditions(&["FITC-A".into()]).is_err());
        assert!(parse_conditions(&["> 3".into()]).is_err());
        assert!(parse_conditions(&["FITC-A > lots".into()]).is_err());
    }

    #[test]
    fn nan_meets_no_condition() {
        for op in [
            CompareOp::Greater,
            CompareOp::Less,
            CompareOp::Equal,
            CompareOp::NotEqual,
        ] {
            assert!(!op.test(f64::NAN, 0.0), "{op:?}");
        }
        assert!(CompareOp::GreaterEqual.test(2.0, 2.0));
        assert!(!CompareOp::Greater.test(2.0, 2.0));
    }

    #[test]
    fn columns_by_name_or_label() {
        let names = vec!["FSC-A".to_string(), "FL1-A".to_string()];
        let labels = vec![None, Some("CD4 FITC".to_string())];
        let c = |s: &str| parse_conditions(&[s.into()]).unwrap().remove(0);
        assert_eq!(column_index(&c("FL1-A > 1"), &names, &labels).unwrap(), 1);
        assert_eq!(
            column_index(&c("CD4 FITC > 1"), &names, &labels).unwrap(),
            1
        );
        assert_eq!(column_index(&c("fsc-a > 1"), &names, &labels).unwrap(), 0);
        assert!(column_index(&c("PE-A > 1"), &names, &labels).is_err());
    }
}
