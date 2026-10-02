//! Plate values as the analyses see them: one observation per well, read, time point and
//! wavelength, from any dataset whose table has the plate columns
//! (`well,row,col,read,wavelength_nm,time_s,value`, docs/formats/plate-readers.md), or from a
//! long CSV with a `well` and a `value` column.

use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use serde_json::Value;

use crate::layout::{self, Layout};

/// One measurement channel.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadInfo {
    /// 1-based read number.
    pub number: u32,
    /// The read's label as the file writes it.
    pub label: String,
    /// Detection mode, when known.
    pub mode: Option<String>,
    /// Unit of the values, when known.
    pub unit: Option<String>,
    /// Values derived by the vendor software.
    pub calculated: bool,
}

/// One value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Obs {
    /// Zero-based row.
    pub row: u32,
    /// Zero-based column.
    pub col: u32,
    /// 1-based read number.
    pub read: u32,
    /// Wavelength of the value (spectra), else the read's.
    pub wavelength_nm: Option<f64>,
    /// Elapsed time (kinetic reads).
    pub time_s: Option<f64>,
    /// The value (NaN when the cell was not a number).
    pub value: f64,
}

/// A plate: its reads, values and any layout the file embeds.
#[derive(Debug, Clone, Default)]
pub struct PlateData {
    /// Table (plate) name.
    pub name: Option<String>,
    /// Format id of the source.
    pub format: String,
    /// Plate rows and columns, when known.
    pub rows: u32,
    /// Plate columns.
    pub cols: u32,
    /// Reads.
    pub reads: Vec<ReadInfo>,
    /// Values.
    pub obs: Vec<Obs>,
    /// Layout embedded in the export (sample names, concentrations), if any.
    pub embedded: Option<Layout>,
}

fn find(cols: &[openreadout_core::model::ColumnInfo], name: &str) -> Option<usize> {
    cols.iter().position(|c| c.name == name)
}

fn opt(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

impl PlateData {
    /// Read table `table` of a dataset (a plate-reader export, or any table with the plate
    /// columns).
    pub fn from_dataset(ds: &mut dyn Dataset, table: u32) -> Result<PlateData> {
        let info = ds.info()?;
        let format = info.format.id.clone();
        if info.tables.is_empty() {
            return Err(Error::Unsupported {
                format: "assay",
                feature: format!("a {format} file without tables"),
                hint: Some("Assay analysis reads plate-reader exports (Gen5, SoftMax Pro, BMG, EnVision, Kaleido, Tecan, SkanIt, plate matrices) or a long CSV with `well` and `value` columns.".into()),
            });
        }
        let t = info.tables.get(table as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {table} does not exist (the file has {} table(s): 0..{}); see `openreadout info`",
                info.tables.len(),
                info.tables.len() - 1
            ))
        })?;
        let cols = &t.columns;
        let (Some(ci_row), Some(ci_col), Some(ci_val)) =
            (find(cols, "row"), find(cols, "col"), find(cols, "value"))
        else {
            return Err(Error::Unsupported {
                format: "assay",
                feature: format!("table {table} of this {format} file has no plate columns (row, col, value)"),
                hint: Some("Assay analysis needs a plate table (`openreadout info` → tables[].columns well,row,col,read,…,value).".into()),
            });
        };
        let ci_read = find(cols, "read");
        let ci_wl = find(cols, "wavelength_nm");
        let ci_t = find(cols, "time_s");
        let data = ds.read_table(table, 0, t.row_count)?;
        let col = |i: usize| data.columns.get(i).map_or(&[][..], Vec::as_slice);
        let n = col(ci_val).len();
        let mut obs = Vec::with_capacity(n);
        for r in 0..n {
            let row = col(ci_row).get(r).copied().unwrap_or(f64::NAN);
            let c = col(ci_col).get(r).copied().unwrap_or(f64::NAN);
            if !(row >= 1.0 && c >= 1.0 && row < 1e6 && c < 1e6) {
                continue;
            }
            obs.push(Obs {
                row: row as u32 - 1,
                col: c as u32 - 1,
                read: ci_read
                    .and_then(|i| col(i).get(r).copied())
                    .filter(|v| *v >= 1.0 && *v < 1e6)
                    .map_or(1, |v| v as u32),
                wavelength_nm: ci_wl.and_then(|i| col(i).get(r).copied()).and_then(opt),
                time_s: ci_t.and_then(|i| col(i).get(r).copied()).and_then(opt),
                value: col(ci_val).get(r).copied().unwrap_or(f64::NAN),
            });
        }
        let reads = if let Some(list) = t.extra.get("reads").and_then(Value::as_array) {
            list.iter()
                .enumerate()
                .map(|(i, r)| ReadInfo {
                    number: r
                        .get("read")
                        .and_then(Value::as_u64)
                        .map_or(i as u32 + 1, |v| v as u32),
                    label: r
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    mode: r.get("mode").and_then(Value::as_str).map(str::to_string),
                    unit: r.get("unit").and_then(Value::as_str).map(str::to_string),
                    calculated: r
                        .get("calculated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
                .collect()
        } else {
            let mut nums: Vec<u32> = obs.iter().map(|o| o.read).collect();
            nums.sort_unstable();
            nums.dedup();
            nums.into_iter()
                .map(|n| ReadInfo {
                    number: n,
                    label: format!("read {n}"),
                    mode: None,
                    unit: cols[ci_val].unit.clone(),
                    calculated: false,
                })
                .collect()
        };
        let dim = |k: &str| t.extra.get(k).and_then(Value::as_u64).map(|v| v as u32);
        // `layout` (every dialect) then `layout_definitions` (SkanIt: groups, concentrations,
        // dilutions)
        let embedded = ["layout", "layout_definitions"]
            .iter()
            .filter_map(|k| t.extra.get(*k).and_then(layout::from_embedded))
            .reduce(|mut a, b| {
                a.merge(b);
                a
            });
        Ok(PlateData {
            name: t.name.clone(),
            format,
            rows: dim("plate_rows")
                .unwrap_or_else(|| obs.iter().map(|o| o.row + 1).max().unwrap_or(0)),
            cols: dim("plate_columns")
                .unwrap_or_else(|| obs.iter().map(|o| o.col + 1).max().unwrap_or(0)),
            reads,
            obs,
            embedded,
        })
    }

    /// Whether `text` looks like a long CSV/TSV of plate values: a header with a `well` column
    /// and a value column.
    pub fn is_long_csv(text: &str) -> bool {
        let first = text
            .trim_start_matches('\u{feff}')
            .lines()
            .next()
            .unwrap_or("");
        let cells: Vec<String> = first
            .split([',', '\t', ';'])
            .map(|c| c.trim().trim_matches('"').to_ascii_lowercase())
            .collect();
        cells.iter().any(|c| c == "well") && cells.iter().any(|c| value_column(c))
    }

    /// Parse a long CSV/TSV: `well` and a value column (`value`, `signal`, `od`, `absorbance`,
    /// `fluorescence`, `luminescence`, `rfu`, `rlu`, `response`), optionally `time_s` (or
    /// `time_min`, `time_h`, `time`), `read`, `wavelength_nm`; layout columns (`role`,
    /// `sample`, `concentration`, …) in the same file become the embedded layout.
    pub fn from_long_csv(text: &str, source: &str) -> Result<PlateData> {
        let text = text.trim_start_matches('\u{feff}');
        let delim = if text.lines().next().unwrap_or("").contains('\t') {
            '\t'
        } else if text.lines().next().unwrap_or("").matches(';').count()
            > text.lines().next().unwrap_or("").matches(',').count()
        {
            ';'
        } else {
            ','
        };
        let mut lines = text.lines();
        let header: Vec<String> = lines
            .next()
            .unwrap_or("")
            .split(delim)
            .map(|c| c.trim().trim_matches('"').to_string())
            .collect();
        let lower: Vec<String> = header.iter().map(|h| h.to_ascii_lowercase()).collect();
        let usage = |m: String| Error::Usage(format!("{source}: {m}"));
        let wi = lower
            .iter()
            .position(|c| c == "well")
            .ok_or_else(|| usage("no `well` column".into()))?;
        let vi = lower
            .iter()
            .position(|c| value_column(c))
            .ok_or_else(|| usage("no value column (`value`, `signal`, `od`, …)".into()))?;
        let (ti, t_scale) = [
            ("time_s", 1.0),
            ("time", 1.0),
            ("time_min", 60.0),
            ("time_h", 3600.0),
            ("time (s)", 1.0),
            ("time (min)", 60.0),
            ("time (h)", 3600.0),
        ]
        .iter()
        .find_map(|(n, s)| lower.iter().position(|c| c == n).map(|i| (Some(i), *s)))
        .unwrap_or((None, 1.0));
        let ri = lower.iter().position(|c| c == "read");
        let li = lower
            .iter()
            .position(|c| c == "wavelength_nm" || c == "wavelength");
        let mut obs = Vec::new();
        let mut read_labels: Vec<String> = Vec::new();
        for (n, line) in lines.enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let cells: Vec<&str> = line
                .split(delim)
                .map(|c| c.trim().trim_matches('"'))
                .collect();
            let get = |i: usize| cells.get(i).copied().unwrap_or("");
            let (row, col) = layout::parse_well(get(wi)).ok_or_else(|| {
                usage(format!("line {}: `{}` is not a well name", n + 2, get(wi)))
            })?;
            let read = match ri {
                None => 1,
                Some(i) => {
                    let label = get(i).to_string();
                    match label.parse::<u32>() {
                        Ok(v) if v >= 1 => v,
                        _ => {
                            let pos =
                                read_labels
                                    .iter()
                                    .position(|l| *l == label)
                                    .unwrap_or_else(|| {
                                        read_labels.push(label.clone());
                                        read_labels.len() - 1
                                    });
                            pos as u32 + 1
                        }
                    }
                }
            };
            obs.push(Obs {
                row,
                col,
                read,
                wavelength_nm: li.and_then(|i| get(i).parse().ok()),
                time_s: ti.and_then(|i| get(i).parse::<f64>().ok().map(|v| v * t_scale)),
                value: get(vi).parse().unwrap_or(f64::NAN),
            });
        }
        if obs.is_empty() {
            return Err(usage("no rows".into()));
        }
        let mut nums: Vec<u32> = obs.iter().map(|o| o.read).collect();
        nums.sort_unstable();
        nums.dedup();
        let reads = nums
            .into_iter()
            .map(|n| ReadInfo {
                number: n,
                label: read_labels
                    .get(n as usize - 1)
                    .cloned()
                    .unwrap_or_else(|| header[vi].clone()),
                mode: None,
                unit: None,
                calculated: false,
            })
            .collect();
        // layout columns in the same file
        let skip = [Some(vi), ti, ri, li];
        let keep: Vec<usize> = (0..header.len())
            .filter(|i| !skip.contains(&Some(*i)))
            .collect();
        let embedded = if keep.len() > 1 {
            let sub: String = text
                .lines()
                .map(|l| {
                    let cells: Vec<&str> = l.split(delim).collect();
                    keep.iter()
                        .map(|&i| cells.get(i).copied().unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join(&delim.to_string())
                })
                .collect::<Vec<_>>()
                .join("\n");
            layout::parse_layout(&sub, &format!("columns of {source}")).ok()
        } else {
            None
        };
        let rows = obs.iter().map(|o| o.row + 1).max().unwrap_or(0);
        let cols = obs.iter().map(|o| o.col + 1).max().unwrap_or(0);
        Ok(PlateData {
            name: None,
            format: "long-csv".into(),
            rows,
            cols,
            reads,
            obs,
            embedded,
        })
    }
}

fn value_column(c: &str) -> bool {
    matches!(
        c,
        "value"
            | "signal"
            | "od"
            | "absorbance"
            | "fluorescence"
            | "luminescence"
            | "rfu"
            | "rlu"
            | "response"
            | "y"
            | "od600"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_csv() {
        let text = "well,time_min,value,role\nA1,0,0.1,blank\nA1,10,0.2,blank\nB2,0,0.3,sample\n";
        assert!(PlateData::is_long_csv(text));
        let p = PlateData::from_long_csv(text, "t.csv").unwrap();
        assert_eq!(p.obs.len(), 3);
        assert_eq!(p.obs[1].time_s, Some(600.0));
        assert_eq!(p.obs[2].row, 1);
        let lay = p.embedded.unwrap();
        assert_eq!(lay.wells[&(0, 0)].role, Some(layout::Role::Blank));
        assert!(!PlateData::is_long_csv("a,b\n1,2\n"));
        assert!(PlateData::from_long_csv("well,value\nQQ,1\n", "t").is_err());
    }
}
