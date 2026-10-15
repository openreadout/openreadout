//! Plate previews: a heat map of a multi-well plate table, or, for a kinetic read, a grid of
//! small growth curves ([`crate::kinetic`]).
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

use openreadout_figure::{
    Anchor, Baseline, Face, Figure, Path, Rect, Stroke, TextStyle, color_bar, text_width,
};

use crate::{Frame, PlatePreview, PreviewOutput, PreviewRequest};

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

/// The values of a plate table, one per well, as the heat map shows them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PlateGrid {
    /// Table index.
    pub table: u32,
    /// `wide` (one column per well) or `long` (`well` or `row` + `column` columns).
    pub layout: &'static str,
    /// Plate rows (8 for a 96-well plate).
    pub rows: u32,
    /// Plate columns (12 for a 96-well plate).
    pub columns: u32,
    /// What each value is (a column name, or `mean of N rows`).
    pub value: String,
    /// `((row, column), value)` of each well with a finite value, zero-based.
    pub cells: Vec<((u32, u32), f64)>,
    /// What was left out (rows past the read cap, other reads or time points).
    pub notes: Vec<String>,
}

/// The per-well values of table `table` (default 0); `column` picks the value column of a long
/// table. Errors when the table has no well layout.
pub fn plate_grid(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    table: Option<u32>,
    column: Option<&str>,
) -> Result<PlateGrid> {
    let ti = table.unwrap_or(0);
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
            format!("a picture of table {ti}, which is not a plate (no well columns)"),
            "Previews draw images, traces, spectra and plate tables (columns named like wells, A1..P24, or `well` / `row`+`column` columns). Read this table's rows with openreadout_table (CLI: `openreadout export FILE --format csv`).",
        ));
    }
    let rows = t.row_count.min(MAX_ROWS);
    let mut notes = Vec::new();
    if rows < t.row_count {
        notes.push(format!("read the first {rows} of {} rows", t.row_count));
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
        let vi = if let Some(name) = column {
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
            notes.push(format!(
                "showing {w}; read the other rows with openreadout_table or `openreadout export FILE --format csv`"
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
    Ok(PlateGrid {
        table: ti,
        layout,
        rows: nr,
        columns: nc,
        value,
        cells,
        notes,
    })
}

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    if req.columns.len() > 1 {
        return Err(Error::Usage(format!(
            "a plate preview shows one value column; got {}",
            req.columns.join(", ")
        )));
    }
    if let Some(drawn) = crate::kinetic::render(ds, info, req, out)? {
        return Ok(drawn);
    }
    let PlateGrid {
        table: ti,
        layout,
        rows: nr,
        columns: nc,
        value,
        cells,
        notes,
    } = plate_grid(ds, info, req.table, req.columns.first().map(String::as_str))?;
    out.notes.extend(notes);
    let (lo, hi) = cells
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), (_, v)| {
            (a.min(*v), b.max(*v))
        });
    let range = (lo.is_finite() && hi.is_finite()).then_some((lo, hi));
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let size = req.max_size.max(64) as f32;
    let label = TextStyle::new(fs, theme.muted);
    let row_w = text_width("WW", Face::Sans, fs) + fs * 0.6;
    let top = fs * 3.2;
    let legend_h = fs * 3.4;
    let pad = fs * 0.8;
    let cell = ((size - row_w - 2.0 * pad) / nc as f32)
        .min((size * 0.8 - top - legend_h) / nr as f32)
        .clamp(4.0, 72.0);
    let grid_w = cell * nc as f32;
    let grid_h = cell * nr as f32;
    let width = (pad * 2.0 + row_w + grid_w).ceil();
    let height = (top + grid_h + legend_h).ceil();
    let mut fig = Figure::new(width, height, Some(theme.paper));
    let (gx, gy) = (pad + row_w, top);
    fig.text(
        pad,
        fs * 1.3,
        &value,
        &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
    );
    // column numbers and row letters, thinned when the cells are small
    let every = |n: u32, w: f32| -> u32 {
        let need = text_width(&n.to_string(), Face::Sans, fs) + fs * 0.4;
        (need / w).ceil().max(1.0) as u32
    };
    let ce = every(nc, cell);
    for c in 0..nc {
        if (c + 1) % ce == 0 || c == 0 {
            fig.text(
                gx + (c as f32 + 0.5) * cell,
                top - fs * 0.5,
                &(c + 1).to_string(),
                &label.clone().align(Anchor::Middle, Baseline::Alphabetic),
            );
        }
    }
    let re = ((fs * 1.1) / cell).ceil().max(1.0) as u32;
    for r in 0..nr {
        if r % re == 0 {
            fig.text(
                gx - fs * 0.5,
                gy + (r as f32 + 0.5) * cell,
                &crate::annotate::row_name(r),
                &label.clone().align(Anchor::End, Baseline::Middle),
            );
        }
    }
    let inset = (cell * 0.08).clamp(0.5, 3.0);
    let round = cell >= 14.0;
    let shape = |x: f32, y: f32| -> Path {
        let (cx, cy) = (x + cell / 2.0, y + cell / 2.0);
        if round {
            Path::circle(cx, cy, cell / 2.0 - inset)
        } else {
            Path::rect(x + inset * 0.5, y + inset * 0.5, cell - inset, cell - inset)
        }
    };
    let mut frame_cells = Vec::with_capacity((nr * nc) as usize);
    let have: std::collections::BTreeMap<(u32, u32), f64> = cells.iter().copied().collect();
    for r in 0..nr {
        for c in 0..nc {
            let (x, y) = (gx + c as f32 * cell, gy + r as f32 * cell);
            frame_cells.push(((r, c), Rect::new(x, y, cell, cell)));
            match have.get(&(r, c)) {
                Some(v) => {
                    let t = match range {
                        Some((lo, hi)) if hi > lo => (v - lo) / (hi - lo),
                        _ => 0.5,
                    };
                    fig.fill(shape(x, y), theme.sequential.at(t));
                }
                None => {
                    fig.stroke(shape(x, y), Stroke::new(theme.rule, 1.0));
                }
            }
        }
    }
    if let Some((lo, hi)) = range {
        let bw = (grid_w * 0.4).clamp(80.0, 360.0);
        color_bar(
            &mut fig,
            &theme,
            Rect::new(gx, gy + grid_h + fs * 1.1, bw, fs * 0.7),
            &theme.sequential,
            lo,
            hi,
            None,
        );
    }
    out.plate = Some(PlatePreview {
        table: ti,
        layout: layout.into(),
        view: "heatmap".into(),
        rows: nr,
        columns: nc,
        wells: cells.len() as u32,
        value,
        min: range.map(|r| r.0),
        max: range.map(|r| r.1),
        ..PlatePreview::default()
    });
    Ok((fig, Frame::Wells(frame_cells)))
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
