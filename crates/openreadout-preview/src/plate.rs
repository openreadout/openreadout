//! Plate previews: a heat map of a multi-well plate table.
//!
//! Two layouts are recognized:
//! - **wide**: columns named like wells (`A1`, `A01`, … `P24`); each well shows the mean of its
//!   column over all rows (one row for an end-point read, the kinetic mean otherwise);
//! - **long**: `row` + `column`/`col` columns (1-based unless a 0 occurs) or a `well` column
//!   (numeric well number, row-major, 1-based unless a 0 occurs), plus a value column (`column`
//!   option; default `value`, else the first column that is not a position, read, wavelength or
//!   time). When the table holds several reads, wavelengths or time points (the plate-reader
//!   layout `well,row,col,read,wavelength_nm,time_s,value`), the map shows the first read at its
//!   first wavelength and its last time point (the end of a kinetic run); other rows with the
//!   same well are averaged (replicates).
//!
//! Geometry comes from the table's `extra.plate_rows` / `extra.plate_columns` when the reader
//! records them, else 8 × 12 unless a well falls outside it (then 16 × 24, then 32 × 48).

use openreadout_core::model::{FileInfo, TableInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};

use crate::canvas::{Canvas, text_width};
use crate::color::sequential;
use crate::{PlatePreview, PreviewOutput, PreviewRequest};

/// Most rows read from a plate table.
const MAX_ROWS: u64 = 1_000_000;

/// `A1` / `a01` / `P24` → zero-based (row, column).
pub(crate) fn parse_well(name: &str) -> Option<(u32, u32)> {
    let name = name.trim();
    let mut chars = name.chars();
    let r = chars.next()?.to_ascii_uppercase();
    if !r.is_ascii_uppercase() || r > 'P' {
        return None;
    }
    let digits = chars.as_str();
    if digits.is_empty() || digits.len() > 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let c: u32 = digits.parse().ok()?;
    if !(1..=24).contains(&c) {
        return None;
    }
    Some((u32::from(r as u8 - b'A'), c - 1))
}

fn col_index(t: &TableInfo, names: &[&str]) -> Option<usize> {
    t.columns
        .iter()
        .position(|c| names.iter().any(|n| c.name.trim().eq_ignore_ascii_case(n)))
}

fn mean(v: &[f64]) -> Option<f64> {
    let f: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    (!f.is_empty()).then(|| f.iter().sum::<f64>() / f.len() as f64)
}

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<Canvas> {
    let ti = req.table.unwrap_or(0);
    let t = info
        .tables
        .iter()
        .find(|t| t.index == ti)
        .ok_or_else(|| {
            if info.tables.is_empty() {
                Error::unsupported(
                    "preview",
                    format!("plate preview of a {} file", info.format.name),
                    "This file holds no tables; see `openreadout info` for what it contains.",
                )
            } else {
                Error::Usage(format!(
                    "table {ti} out of range (file has {} tables)",
                    info.tables.len()
                ))
            }
        })?
        .clone();
    let wide: Vec<(usize, (u32, u32))> = t
        .columns
        .iter()
        .enumerate()
        .filter_map(|(i, c)| parse_well(&c.name).map(|w| (i, w)))
        .collect();
    let row_col = col_index(&t, &["row"]);
    let colc_col = col_index(&t, &["column", "col"]);
    // Row and column win over a well number: they need no guess about the plate width.
    let well_col = col_index(&t, &["well"]).filter(|_| row_col.is_none() || colc_col.is_none());
    let long = well_col.is_some() || (row_col.is_some() && colc_col.is_some());
    if wide.len() < 2 && !long {
        return Err(Error::unsupported(
            "preview",
            format!("preview of table {ti} (no well columns)"),
            "Plate previews need columns named like wells (A1..P24) or `well` / `row`+`column` columns. Read other tables as rows with the MCP tool openreadout_table or `openreadout export FILE --to csv`.",
        ));
    }
    let rows = t.row_count.min(MAX_ROWS);
    if rows < t.row_count {
        out.notes
            .push(format!("read the first {rows} of {} rows", t.row_count));
    }
    let tab = ds.read_table(ti, 0, rows)?;
    let mut cells: Vec<((u32, u32), f64)> = Vec::new();
    let (layout, value) = if wide.len() >= 2 && !long {
        for (i, w) in &wide {
            if let Some(m) = tab.columns.get(*i).and_then(|c| mean(c)) {
                cells.push((*w, m));
            }
        }
        let n = tab.columns.first().map_or(0, Vec::len);
        (
            "wide",
            if n == 1 {
                "value of the single row".to_string()
            } else {
                format!("mean of {n} rows")
            },
        )
    } else {
        let skip = [
            well_col,
            row_col,
            colc_col,
            col_index(&t, &["well"]),
            col_index(&t, &["read"]),
            col_index(&t, &["wavelength_nm"]),
            col_index(&t, &["time_s"]),
        ];
        let vi = if let Some(name) = &req.column {
            t.columns
                .iter()
                .position(|c| c.name == *name)
                .ok_or_else(|| {
                    Error::Usage(format!(
                        "column '{name}' not in table {ti} ({})",
                        t.columns
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })?
        } else {
            col_index(&t, &["value"])
                .or_else(|| (0..t.columns.len()).find(|i| !skip.contains(&Some(*i))))
                .ok_or_else(|| Error::Usage(format!("table {ti} has no value column")))?
        };
        let get = |i: usize| tab.columns.get(i).cloned().unwrap_or_default();
        let mut vals = get(vi);
        let what = keep_one_measurement(&t, &tab.columns, &mut vals);
        if let Some(w) = &what {
            out.notes.push(format!(
                "showing {w}; read the other rows with openreadout_table or `openreadout export FILE --to csv`"
            ));
        }
        let positions: Vec<Option<(u32, u32)>> = if let Some(wc) = well_col {
            let w = get(wc);
            let base = if w.contains(&0.0) { 0.0 } else { 1.0 };
            let maxw = w
                .iter()
                .copied()
                .filter(|x| x.is_finite())
                .fold(0.0, f64::max)
                - base;
            let per_row = if maxw >= 96.0 { 24.0 } else { 12.0 };
            w.iter()
                .map(|&x| {
                    let k = x - base;
                    (x.is_finite() && k >= 0.0 && k.fract() == 0.0 && k < per_row * 32.0)
                        .then(|| ((k / per_row).floor() as u32, (k % per_row) as u32))
                })
                .collect()
        } else {
            let (r, c) = (get(row_col.unwrap_or(0)), get(colc_col.unwrap_or(0)));
            let rb = if r.contains(&0.0) { 0.0 } else { 1.0 };
            let cb = if c.contains(&0.0) { 0.0 } else { 1.0 };
            r.iter()
                .zip(&c)
                .map(|(&r, &c)| {
                    let (r, c) = (r - rb, c - cb);
                    (r >= 0.0
                        && c >= 0.0
                        && r.fract() == 0.0
                        && c.fract() == 0.0
                        && r < 32.0
                        && c < 48.0)
                        .then_some((r as u32, c as u32))
                })
                .collect()
        };
        // several rows per well (replicates, kinetic reads): average them
        let mut acc: std::collections::BTreeMap<(u32, u32), (f64, u32)> = Default::default();
        for (p, v) in positions.iter().zip(&vals) {
            if let Some(p) = p
                && v.is_finite()
            {
                let e = acc.entry(*p).or_insert((0.0, 0));
                e.0 += v;
                e.1 += 1;
            }
        }
        cells = acc
            .into_iter()
            .map(|(p, (s, n))| (p, s / f64::from(n)))
            .collect();
        (
            "long",
            match what {
                Some(w) => format!("{} ({w})", t.columns[vi].name),
                None => t.columns[vi].name.clone(),
            },
        )
    };
    let declared = |k: &str| {
        t.extra
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| (1..=64).contains(v))
    };
    let (nr, nc) = if let (Some(r), Some(c)) = (declared("plate_rows"), declared("plate_columns"))
        && cells.iter().all(|((wr, wc), _)| *wr < r && *wc < c)
    {
        (r, c)
    } else if cells.iter().all(|((r, c), _)| *r < 8 && *c < 12) {
        (8u32, 12u32)
    } else if cells.iter().all(|((r, c), _)| *r < 16 && *c < 24) {
        (16, 24)
    } else {
        (32, 48)
    };
    let (lo, hi) = cells
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), (_, v)| {
            (a.min(*v), b.max(*v))
        });
    let range = (lo.is_finite() && hi.is_finite()).then_some((lo, hi));
    let scale: i64 = if nc <= 12 { 2 } else { 1 };
    let margin = 6 * scale + 8;
    let avail = i64::from(req.max_size.max(64)) - margin - 4;
    let cell = (avail / i64::from(nc)).clamp(6, 64);
    let width = (margin + cell * i64::from(nc) + 4) as u32;
    let legend_h = 5 * scale + 8;
    let height = (margin + cell * i64::from(nr) + 4 + legend_h) as u32;
    let mut canvas = Canvas::new(width, height, [255, 255, 255]);
    let label = [90, 90, 90];
    for c in 0..nc {
        let s = (c + 1).to_string();
        let x = margin + i64::from(c) * cell + (cell - text_width(&s, scale)) / 2;
        if cell >= text_width(&s, scale) {
            canvas.text(x, 4, scale, label, &s);
        }
    }
    for r in 0..nr {
        let s = char::from(b'A' + (r as u8 % 26)).to_string();
        let y = margin + i64::from(r) * cell + (cell - 5 * scale) / 2;
        if cell >= 5 * scale {
            canvas.text(4, y, scale, label, &s);
        }
    }
    for r in 0..nr {
        for c in 0..nc {
            canvas.fill_rect(
                margin + i64::from(c) * cell,
                margin + i64::from(r) * cell,
                cell - 1,
                cell - 1,
                [230, 230, 230],
            );
        }
    }
    for ((r, c), v) in &cells {
        let t = match range {
            Some((lo, hi)) if hi > lo => (v - lo) / (hi - lo),
            _ => 0.5,
        };
        canvas.fill_rect(
            margin + i64::from(*c) * cell,
            margin + i64::from(*r) * cell,
            cell - 1,
            cell - 1,
            sequential(t),
        );
    }
    // Colour scale: "<min> [ramp] <max>" under the plate.
    if let Some((lo, hi)) = range {
        let y = margin + cell * i64::from(nr) + 4;
        let lo_s = crate::trace::fmt_num(lo);
        let hi_s = crate::trace::fmt_num(hi);
        let x = margin + canvas.text(margin, y + 1, scale, label, &lo_s) + 2 * scale;
        let ramp_w = (i64::from(width) / 3).clamp(16, 256);
        for i in 0..ramp_w {
            let t = if hi > lo {
                i as f64 / (ramp_w - 1).max(1) as f64
            } else {
                0.5
            };
            canvas.fill_rect(x + i, y, 1, 5 * scale + 2, sequential(t));
        }
        canvas.text(x + ramp_w + 2 * scale, y + 1, scale, label, &hi_s);
    }
    out.plate = Some(PlatePreview {
        table: ti,
        layout: layout.into(),
        rows: nr,
        columns: nc,
        wells: cells.len() as u32,
        value,
        min: range.map(|r| r.0),
        max: range.map(|r| r.1),
    });
    Ok(canvas)
}

/// Plate-reader tables hold one row per well, read, wavelength and time point. Keep only the
/// rows of the first read at its first wavelength and last time point (set the others' values to
/// NaN) and say what was kept; `None` when the table has at most one measurement per well.
fn keep_one_measurement(t: &TableInfo, cols: &[Vec<f64>], vals: &mut [f64]) -> Option<String> {
    let get = |names: &[&str]| col_index(t, names).and_then(|i| cols.get(i));
    let (read, wl, time) = (get(&["read"]), get(&["wavelength_nm"]), get(&["time_s"]));
    let distinct = |c: Option<&Vec<f64>>| {
        c.map_or(0, |c| {
            let mut v: Vec<u64> = c
                .iter()
                .filter(|x| x.is_finite())
                .map(|x| x.to_bits())
                .collect();
            v.sort_unstable();
            v.dedup();
            v.len()
        })
    };
    if distinct(read) <= 1 && distinct(wl) <= 1 && distinct(time) <= 1 {
        return None;
    }
    let n = vals.len();
    let at = |c: Option<&Vec<f64>>, i: usize| c.and_then(|c| c.get(i)).copied().unwrap_or(f64::NAN);
    let mut keep: Vec<bool> = (0..n).map(|i| vals[i].is_finite()).collect();
    let mut parts = Vec::new();
    // Narrow step by step: first read, then its first wavelength, then its last time point.
    let mut narrow = |c: Option<&Vec<f64>>, pick_max: bool, label: &str, unit: &str| {
        let candidates = (0..n)
            .filter(|&i| keep[i])
            .map(|i| at(c, i))
            .filter(|x| x.is_finite());
        let chosen = if pick_max {
            candidates.fold(f64::NEG_INFINITY, f64::max)
        } else {
            candidates.fold(f64::INFINITY, f64::min)
        };
        if !chosen.is_finite() {
            return;
        }
        let differs = (0..n).any(|i| keep[i] && at(c, i).to_bits() != chosen.to_bits());
        if differs {
            for (i, k) in keep.iter_mut().enumerate() {
                *k = *k && at(c, i).to_bits() == chosen.to_bits();
            }
            parts.push(format!("{label} {chosen}{unit}"));
        }
    };
    narrow(read, false, "read", "");
    narrow(wl, false, "wavelength", " nm");
    narrow(time, true, "time", " s");
    for (v, k) in vals.iter_mut().zip(&keep) {
        if !k {
            *v = f64::NAN;
        }
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wells_parse() {
        assert_eq!(parse_well("A1"), Some((0, 0)));
        assert_eq!(parse_well("h12"), Some((7, 11)));
        assert_eq!(parse_well("P24"), Some((15, 23)));
        assert_eq!(parse_well("B07"), Some((1, 6)));
        assert_eq!(parse_well("Q1"), None);
        assert_eq!(parse_well("A25"), None);
        assert_eq!(parse_well("FSC-A"), None);
    }
}
