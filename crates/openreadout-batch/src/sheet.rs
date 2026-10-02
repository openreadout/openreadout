//! Sample sheets and plate layouts.
//!
//! A *sample sheet* is a table with a header row and one row per sample (CSV, TSV, a text
//! export, or a worksheet of an XLSX/XLS/XLSB/ODS workbook). A *plate layout* ("plate map") is
//! one or more plate-shaped grids, one per annotation: a header row numbering the columns
//! (`1 … 12`, `1 … 24`, `1 … 48`) and one row per plate row (`A … H`, `A … P`, `A … AF`); the
//! annotation's name is the grid's top-left cell, or a line of its own just above the grid, or
//! the worksheet's name. Both are read into the same long form: named columns and text cells,
//! where a layout has a `well` column (`A01`) plus one column per grid.
//!
//! ```text
//! condition,1,2,3,…          dose_uM
//! A,ctrl,ctrl,drug,…         ,1,2,3,…
//! B,…                        A,0,0,10,…
//! ```

use std::path::Path;

use openreadout_core::{Error, Result};

use crate::well;

/// Most cells read from one worksheet or text file.
const MAX_CELLS: usize = 4 << 20;
/// Largest sample-sheet file read (they are kilobytes).
const MAX_BYTES: u64 = 64 << 20;

/// How the sheet was laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SheetKind {
    /// A header row, one row per sample.
    Table,
    /// Plate-shaped grids, one per annotation, turned into one row per well.
    PlateLayout,
}

/// A sample sheet or plate layout in long form.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct SampleSheet {
    /// The file.
    pub path: String,
    /// Worksheet read (workbooks only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worksheet: Option<String>,
    /// Table or plate layout.
    pub kind: SheetKind,
    /// Column names (a layout: `well`, then one per grid).
    pub columns: Vec<String>,
    /// Rows of trimmed text cells, one per column.
    #[serde(skip)]
    pub rows: Vec<Vec<String>>,
    /// Plate format of a layout (96, 384, 1536, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plate_wells: Option<u32>,
    /// What was assumed while reading.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl SampleSheet {
    /// Index of column `name` (exact, then case-insensitive).
    pub fn column(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == name).or_else(|| {
            self.columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case(name))
        })
    }
}

fn usage(path: &Path, msg: impl std::fmt::Display) -> Error {
    Error::Usage(format!("sample sheet {}: {msg}", path.display()))
}

/// Read a sample sheet or plate layout. `worksheet` picks a workbook sheet by name (default:
/// every sheet holding plate grids when there are any, else the first non-empty sheet).
pub fn read(path: &Path, worksheet: Option<&str>) -> Result<SampleSheet> {
    let meta = std::fs::metadata(path).map_err(|e| Error::io(path, e))?;
    if meta.is_dir() {
        return Err(usage(path, "is a directory; pass a CSV, TSV or XLSX file"));
    }
    if meta.len() > MAX_BYTES {
        return Err(usage(
            path,
            format!("larger than {MAX_BYTES} bytes; is it really a sample sheet?"),
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let workbook = bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    let grids: Vec<(String, Vec<Vec<String>>)> = if workbook {
        workbook_sheets(path)?
    } else {
        vec![(String::new(), text_grid(&bytes, path)?)]
    };
    let display = path.display().to_string();
    if let Some(name) = worksheet {
        let Some((n, g)) = grids.iter().find(|(n, _)| n == name) else {
            return Err(usage(
                path,
                format!(
                    "no worksheet `{name}`; it has: {}",
                    grids
                        .iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        };
        return interpret(&display, Some(n.clone()), &[(n.clone(), g.clone())]);
    }
    let with_grids: Vec<(String, Vec<Vec<String>>)> = grids
        .iter()
        .filter(|(_, g)| !find_blocks(g).is_empty())
        .cloned()
        .collect();
    if !with_grids.is_empty() {
        let ws = workbook.then(|| {
            with_grids
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        });
        return interpret(&display, ws, &with_grids);
    }
    let Some((n, g)) = grids.iter().find(|(_, g)| g.iter().any(|r| !row_blank(r))) else {
        return Err(usage(path, "is empty"));
    };
    let mut s = interpret(
        &display,
        workbook.then(|| n.clone()),
        &[(n.clone(), g.clone())],
    )?;
    if workbook && grids.len() > 1 {
        s.notes.push(format!(
            "read worksheet `{n}` of {}; pick another with --sheet",
            grids.len()
        ));
    }
    Ok(s)
}

fn row_blank(r: &[String]) -> bool {
    r.iter().all(|c| c.trim().is_empty())
}

/// Turn grids into the long form: plate blocks if any, else a header-row table.
fn interpret(
    path: &str,
    worksheet: Option<String>,
    sheets: &[(String, Vec<Vec<String>>)],
) -> Result<SampleSheet> {
    let mut blocks: Vec<(String, Block)> = Vec::new();
    for (sheet_name, g) in sheets {
        let found = find_blocks(g);
        let single = found.len() == 1;
        for b in found {
            let name = block_name(g, &b)
                .or_else(|| (single && !sheet_name.is_empty()).then(|| sheet_name.clone()))
                .unwrap_or_else(|| format!("layout_{}", blocks.len() + 1));
            blocks.push((name, b));
        }
    }
    if !blocks.is_empty() {
        return Ok(layout(path, worksheet, sheets, &blocks));
    }
    let (_, g) = &sheets[0];
    table(path, worksheet, g)
}

/// A plate grid: the header row, its first column-number cell, the numbers, the letter rows.
#[derive(Debug, Clone)]
struct Block {
    header: usize,
    /// Column of the row letters (the cell left of `1`).
    letter_col: usize,
    /// `(grid column, plate column)` for each numbered header cell.
    cols: Vec<(usize, u32)>,
    /// `(grid row, plate row)`.
    rows: Vec<(usize, u32)>,
}

fn find_blocks(g: &[Vec<String>]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut r = 0;
    while r < g.len() {
        if let Some(b) = block_at(g, r) {
            r = b.rows.last().map_or(r + 1, |x| x.0 + 1);
            out.push(b);
        } else {
            r += 1;
        }
    }
    out
}

/// A header row at `r` numbering plate columns 1, 2, 3, … (at least three), followed by rows
/// lettered A, B, C, … (at least two) in the column left of the `1`.
fn block_at(g: &[Vec<String>], r: usize) -> Option<Block> {
    let row = &g[r];
    let start = row.iter().position(|c| c.trim() == "1")?;
    if start == 0 {
        return None;
    }
    let mut cols = Vec::new();
    for (i, c) in row.iter().enumerate().skip(start) {
        let t = c.trim();
        let expected = cols.len() as u32 + 1;
        if t.parse::<u32>().ok() == Some(expected) && expected <= well::MAX_COLS {
            cols.push((i, expected - 1));
        } else {
            break;
        }
    }
    if cols.len() < 3 {
        return None;
    }
    let letter_col = start - 1;
    let mut rows = Vec::new();
    for (rr, line) in g.iter().enumerate().skip(r + 1) {
        let t = line.get(letter_col).map_or("", |s| s.trim());
        match well::row_index(t) {
            Some(i) if i == rows.len() as u32 => rows.push((rr, i)),
            _ => break,
        }
    }
    (rows.len() >= 2).then_some(Block {
        header: r,
        letter_col,
        cols,
        rows,
    })
}

/// The grid's annotation name: its top-left cell, else a lone cell on the row above.
fn block_name(g: &[Vec<String>], b: &Block) -> Option<String> {
    let corner = g[b.header]
        .get(b.letter_col)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && well::row_index(s).is_none());
    if corner.is_some() {
        return corner;
    }
    if let Some(prev) = b.header.checked_sub(1).and_then(|i| g.get(i)) {
        let texts: Vec<&str> = prev
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if texts.len() == 1 {
            return Some(texts[0].to_string());
        }
    }
    None
}

fn layout(
    path: &str,
    worksheet: Option<String>,
    sheets: &[(String, Vec<Vec<String>>)],
    blocks: &[(String, Block)],
) -> SampleSheet {
    // Blocks are listed sheet by sheet in `sheets` order; find each block's grid again.
    let mut grids = Vec::new();
    for (_, g) in sheets {
        for b in find_blocks(g) {
            grids.push((g, b.header));
        }
    }
    let mut names: Vec<String> = Vec::new();
    for (n, _) in blocks {
        let mut name = n.clone();
        let mut k = 2;
        while names.contains(&name) || name.eq_ignore_ascii_case("well") {
            name = format!("{n}_{k}");
            k += 1;
        }
        names.push(name);
    }
    let mut wells: std::collections::BTreeMap<well::Well, Vec<String>> = Default::default();
    for (bi, ((_, b), (g, _))) in blocks.iter().zip(&grids).enumerate() {
        for &(gr, pr) in &b.rows {
            for &(gc, pc) in &b.cols {
                let v = g
                    .get(gr)
                    .and_then(|r| r.get(gc))
                    .map_or(String::new(), |s| s.trim().to_string());
                let w = well::Well { row: pr, col: pc };
                let cells = wells
                    .entry(w)
                    .or_insert_with(|| vec![String::new(); blocks.len()]);
                cells[bi] = v;
            }
        }
    }
    let plate = well::plate_size(wells.keys().copied());
    let mut columns = vec!["well".to_string()];
    columns.extend(names.iter().cloned());
    let rows = wells
        .into_iter()
        .filter(|(_, cells)| cells.iter().any(|c| !c.is_empty()))
        .map(|(w, cells)| {
            let mut r = vec![w.name()];
            r.extend(cells);
            r
        })
        .collect();
    SampleSheet {
        path: path.to_string(),
        worksheet,
        kind: SheetKind::PlateLayout,
        columns,
        rows,
        plate_wells: Some(plate),
        notes: vec![format!(
            "plate layout: {} grid(s) ({}), {plate}-well plate; empty wells left out",
            names.len(),
            names.join(", ")
        )],
    }
}

fn table(path: &str, worksheet: Option<String>, g: &[Vec<String>]) -> Result<SampleSheet> {
    let Some(h) = g.iter().position(|r| !row_blank(r)) else {
        return Err(Error::Usage(format!("sample sheet {path}: is empty")));
    };
    let width = g[h..]
        .iter()
        .map(|r| {
            r.iter()
                .rposition(|c| !c.trim().is_empty())
                .map_or(0, |i| i + 1)
        })
        .max()
        .unwrap_or(0);
    let mut columns: Vec<String> = Vec::new();
    let mut notes = Vec::new();
    for i in 0..width {
        let raw = g[h].get(i).map_or("", |s| s.trim());
        let base = if raw.is_empty() {
            format!("column_{}", i + 1)
        } else {
            raw.to_string()
        };
        let mut name = base.clone();
        let mut k = 2;
        while columns.contains(&name) {
            name = format!("{base}_{k}");
            k += 1;
        }
        if name != raw {
            notes.push(format!("column {} named `{name}`", i + 1));
        }
        columns.push(name);
    }
    let mut rows: Vec<Vec<String>> = g[h + 1..]
        .iter()
        .filter(|r| !row_blank(r))
        .map(|r| {
            (0..width)
                .map(|i| r.get(i).map_or(String::new(), |s| s.trim().to_string()))
                .collect()
        })
        .collect();
    // A long-form layout with row and column numbers but no well: add one.
    let find = |names: &[&str]| {
        columns
            .iter()
            .position(|c| names.iter().any(|n| c.eq_ignore_ascii_case(n)))
    };
    let mut plate_wells = None;
    if find(&["well", "well_id", "wellid", "well id", "well name"]).is_none()
        && let (Some(rc), Some(cc)) = (
            find(&["row", "plate_row", "row_letter"]),
            find(&["col", "column", "plate_column", "column_number"]),
        )
    {
        let mut all = true;
        let mut ws = Vec::new();
        for r in &rows {
            let (rt, ct) = (&r[rc], &r[cc]);
            let row = well::row_index(rt).or_else(|| rt.parse::<u32>().ok()?.checked_sub(1));
            let col = ct.parse::<u32>().ok().and_then(|c| c.checked_sub(1));
            match (row, col) {
                (Some(row), Some(col)) if row < well::MAX_ROWS && col < well::MAX_COLS => {
                    ws.push(Some(well::Well { row, col }));
                }
                _ => {
                    all = false;
                    ws.push(None);
                }
            }
        }
        if all && !rows.is_empty() {
            columns.push("well".into());
            for (r, w) in rows.iter_mut().zip(&ws) {
                r.push(w.map(well::Well::name).unwrap_or_default());
            }
            plate_wells = Some(well::plate_size(ws.iter().flatten().copied()));
            notes.push(format!(
                "well built from columns `{}` and `{}`",
                columns[rc], columns[cc]
            ));
        }
    }
    Ok(SampleSheet {
        path: path.to_string(),
        worksheet,
        kind: SheetKind::Table,
        columns,
        rows,
        plate_wells,
        notes,
    })
}

// ---------------------------------------------------------------- text files

fn decode(bytes: &[u8]) -> String {
    if let Some(b) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(b).into_owned();
    }
    let utf16 = |b: &[u8], le: bool| {
        let (pairs, _) = b.as_chunks::<2>();
        let units: Vec<u16> = pairs
            .iter()
            .map(|c| {
                if le {
                    u16::from_le_bytes(*c)
                } else {
                    u16::from_be_bytes(*c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(b) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(b, true);
    }
    if let Some(b) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(b, false);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        // Windows-1252/Latin-1 exports: map bytes to code points (exact for Latin-1)
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Split text into records and fields (RFC 4180 quoting: `"a,b"`, `""` for a quote, newlines
/// inside quotes).
pub(crate) fn split_records(text: &str, delim: char) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut at_start = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if at_start => {
                quoted = true;
                at_start = false;
            }
            c if c == delim => {
                row.push(std::mem::take(&mut field));
                at_start = true;
            }
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                out.push(std::mem::take(&mut row));
                at_start = true;
            }
            c => {
                field.push(c);
                at_start = false;
            }
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        out.push(row);
    }
    out
}

fn text_grid(bytes: &[u8], path: &Path) -> Result<Vec<Vec<String>>> {
    let text = decode(bytes);
    if text.contains('\0') {
        return Err(usage(
            path,
            "is a binary file, not a CSV/TSV text or workbook",
        ));
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let delim = match ext.as_str() {
        "tsv" | "tab" => '\t',
        _ => sniff(&text, ext == "csv"),
    };
    let g = split_records(&text, delim);
    let cells: usize = g.iter().map(Vec::len).sum();
    if cells > MAX_CELLS {
        return Err(usage(path, format!("more than {MAX_CELLS} cells")));
    }
    Ok(g)
}

/// The delimiter: whichever of comma, tab and semicolon occurs on the most of the first 20
/// non-blank lines (ties: comma for `.csv` files, else tab). A `.csv` written in a
/// decimal-comma locale uses semicolons and is split by them.
fn sniff(text: &str, csv: bool) -> char {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .collect();
    let order = if csv {
        [',', '\t', ';']
    } else {
        ['\t', ',', ';']
    };
    let mut best = (order[0], 0usize);
    for d in order {
        let n = lines.iter().filter(|l| l.contains(d)).count();
        if n > best.1 {
            best = (d, n);
        }
    }
    best.0
}

// ---------------------------------------------------------------- workbooks

fn workbook_sheets(path: &Path) -> Result<Vec<(String, Vec<Vec<String>>)>> {
    use calamine::{Data, Reader};
    let bad = |e: String| usage(path, format!("workbook could not be read: {e}"));
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut wb = calamine::open_workbook_auto(path).map_err(|e| bad(e.to_string()))?;
        let mut out = Vec::new();
        for name in wb.sheet_names() {
            let range = wb.worksheet_range(&name).map_err(|e| bad(e.to_string()))?;
            let (h, w) = range.get_size();
            if h.saturating_mul(w) > MAX_CELLS {
                return Err(usage(
                    path,
                    format!("worksheet `{name}` has more than {MAX_CELLS} cells"),
                ));
            }
            let (r0, c0) = range.start().unwrap_or((0, 0));
            let mut g: Vec<Vec<String>> = vec![Vec::new(); r0 as usize];
            for row in range.rows() {
                let mut cells = vec![String::new(); c0 as usize];
                for d in row {
                    cells.push(match d {
                        Data::Empty => String::new(),
                        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
                        Data::Float(f) => crate::table::fmt_float(*f),
                        Data::Int(i) => i.to_string(),
                        Data::Bool(b) => b.to_string(),
                        Data::DateTime(dt) => crate::table::fmt_float(dt.as_f64()),
                        Data::Error(e) => format!("#{e:?}"),
                    });
                }
                g.push(cells);
            }
            out.push((name, g));
        }
        Ok(out)
    }))
    .unwrap_or_else(|_| Err(bad("the workbook parser failed (damaged workbook)".into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn csv_table_with_quotes_blank_rows_and_duplicate_headers() {
        let d = tempfile::tempdir().unwrap();
        let p = write(
            d.path(),
            "s.csv",
            "\u{feff}file,condition,condition,\r\n\r\n\"a,1.fcs\",\"treated \"\"x\"\"\",1\n b.fcs ,ctrl,2,note\n",
        );
        let s = read(&p, None).unwrap();
        assert_eq!(s.kind, SheetKind::Table);
        assert_eq!(s.columns, ["file", "condition", "condition_2", "column_4"]);
        assert_eq!(s.rows[0], ["a,1.fcs", "treated \"x\"", "1", ""]);
        assert_eq!(s.rows[1][0], "b.fcs");
    }

    #[test]
    fn tsv_and_semicolons_are_sniffed() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "s.txt", "file\tdonor\nx.fcs\t3\n");
        assert_eq!(read(&p, None).unwrap().columns, ["file", "donor"]);
        let p = write(d.path(), "t.csv", "file;dose\nx;1,5\n");
        let s = read(&p, None).unwrap();
        assert_eq!(s.rows[0], ["x", "1,5"]);
    }

    #[test]
    fn plate_layout_grids_become_one_row_per_well() {
        let d = tempfile::tempdir().unwrap();
        let text = "\
condition,1,2,3,4
A,ctrl,ctrl,drug,drug
B,ctrl,ctrl,drug,

dose_uM
,1,2,3,4
A,0,0,10,20
B,0,0,10,20
";
        let p = write(d.path(), "map.csv", text);
        let s = read(&p, None).unwrap();
        assert_eq!(s.kind, SheetKind::PlateLayout);
        assert_eq!(s.columns, ["well", "condition", "dose_uM"]);
        assert_eq!(s.rows.len(), 8);
        assert_eq!(s.rows[0], ["A01", "ctrl", "0"]);
        assert_eq!(s.rows[3], ["A04", "drug", "20"]);
        assert_eq!(s.rows[7], ["B04", "", "20"]);
        assert_eq!(s.plate_wells, Some(12));
    }

    #[test]
    fn long_layout_with_row_and_column_gets_a_well() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "l.csv", "Row,Column,treatment\nA,1,x\nH,12,y\n");
        let s = read(&p, None).unwrap();
        assert_eq!(s.columns.last().unwrap(), "well");
        assert_eq!(s.rows[1].last().unwrap(), "H12");
        assert_eq!(s.plate_wells, Some(96));
    }

    #[test]
    fn empty_and_binary_files_are_usage_errors() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), "e.csv", "\n\n");
        assert_eq!(read(&p, None).unwrap_err().exit_code(), 2);
        let p = d.path().join("b.csv");
        std::fs::write(&p, [0u8, 1, 2, 3]).unwrap();
        assert_eq!(read(&p, None).unwrap_err().exit_code(), 2);
        let p = d.path().join("z.xlsx");
        std::fs::write(&p, b"PK\x03\x04garbage").unwrap();
        assert_eq!(read(&p, None).unwrap_err().exit_code(), 2);
    }
}
