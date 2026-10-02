//! Plate geometry, well names, and the matrix scanner that finds `1 2 3 …` / `A B C …` grids
//! anywhere in a sheet.

use crate::sheet::{Cell, Sheet};

/// Standard microplate formats: (wells, rows, columns).
pub(crate) const PLATE_FORMATS: [(u32, u32, u32); 8] = [
    (6, 2, 3),
    (12, 3, 4),
    (24, 4, 6),
    (48, 6, 8),
    (96, 8, 12),
    (384, 16, 24),
    (1536, 32, 48),
    (3456, 48, 72),
];

/// Rows × columns of a standard plate with this many wells.
pub(crate) fn dims_for_wells(wells: u32) -> Option<(u32, u32)> {
    PLATE_FORMATS
        .iter()
        .find(|(w, _, _)| *w == wells)
        .map(|(_, r, c)| (*r, *c))
}

/// The smallest standard plate that holds a well at row `max_row` and column `max_col`
/// (zero-based); falls back to the exact size for non-standard layouts.
pub(crate) fn smallest_plate(max_row: u32, max_col: u32) -> (u32, u32) {
    PLATE_FORMATS
        .iter()
        .find(|(_, r, c)| max_row < *r && max_col < *c)
        .map_or(
            (max_row.saturating_add(1), max_col.saturating_add(1)),
            |(_, r, c)| (*r, *c),
        )
}

/// Canonical row label: `A`…`Z`, then `AA`, `AB`, … (zero-based row index).
pub(crate) fn row_label(r: u32) -> String {
    let mut n = r;
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Canonical well name, `A1`, `P24`, `AF48` (zero-based indices).
pub(crate) fn well_name(r: u32, c: u32) -> String {
    format!("{}{}", row_label(r), c + 1)
}

/// Parse a row label. Upper-case `A`…`Z` and `AA`…`AZ` follow the canonical order; a single
/// lower-case letter is a row after `Z` (BMG writes rows 27–32 of a 1536-well plate as `a`…`f`)
/// unless `lower_is_first` (a grid labelled only in lower case).
pub(crate) fn parse_row_label(s: &str, lower_is_first: bool) -> Option<u32> {
    let label = s.trim();
    let bytes = label.as_bytes();
    match bytes {
        [c] if c.is_ascii_uppercase() => Some(u32::from(c - b'A')),
        [c] if c.is_ascii_lowercase() => {
            Some(u32::from(c - b'a') + if lower_is_first { 0 } else { 26 })
        }
        [a, c] if a.is_ascii_uppercase() && c.is_ascii_uppercase() => {
            Some((u32::from(a - b'A') + 1) * 26 + u32::from(c - b'A'))
        }
        _ => None,
    }
}

/// Parse a well name such as `A1`, `A01`, `p24`, `AF48`, `e12` into zero-based (row, column).
pub(crate) fn parse_well(s: &str) -> Option<(u32, u32)> {
    let t = s.trim();
    let split = t.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = t.split_at(split);
    if letters.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) || digits.len() > 3 {
        return None;
    }
    let col: u32 = digits.parse().ok()?;
    if col == 0 {
        return None;
    }
    Some((parse_row_label(letters, false)?, col - 1))
}

/// Column number of a header cell: `1`, `01`, `1.0` (XLSX numbers).
pub(crate) fn column_number(c: &Cell) -> Option<u32> {
    match c {
        Cell::Number(v) if v.fract() == 0.0 && *v >= 1.0 && *v <= 999.0 => Some(*v as u32),
        Cell::Text(s) => {
            let t = s.trim();
            if !t.is_empty() && t.len() <= 3 && t.chars().all(|c| c.is_ascii_digit()) {
                t.parse().ok().filter(|v| *v >= 1)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// A plate matrix found in a sheet.
#[derive(Debug, Clone)]
pub(crate) struct Grid {
    /// Row index of the column-number header line.
    pub(crate) header_row: usize,
    /// Column index of the row labels (the header's corner cell).
    pub(crate) label_col: usize,
    /// The header's corner cell text (`<>`, `Abs`, `RLU`, `Sample`, empty, …).
    pub(crate) corner: String,
    /// Plate column number (1-based) of each data column, in sheet order.
    pub(crate) columns: Vec<u32>,
    /// One entry per labelled sheet row: (sheet row, zero-based plate row).
    pub(crate) rows: Vec<(usize, u32)>,
    /// First sheet row after the grid.
    pub(crate) end_row: usize,
}

impl Grid {
    /// Non-blank cells as (plate row, plate column, cell), zero-based.
    pub(crate) fn cells<'a>(
        &'a self,
        sheet: &'a Sheet,
    ) -> impl Iterator<Item = (u32, u32, &'a Cell)> + 'a {
        self.rows.iter().flat_map(move |&(sr, pr)| {
            self.columns
                .iter()
                .enumerate()
                .filter_map(move |(i, &col)| {
                    let c = sheet.cell(sr, self.label_col + 1 + i);
                    (!c.is_blank()).then_some((pr, col - 1, c))
                })
        })
    }
}

/// Is row `r` a column-number header starting after column `c0`? Returns the column numbers
/// (at least 2, consecutive and increasing).
fn header_at(sheet: &Sheet, r: usize, c0: usize) -> Option<Vec<u32>> {
    let mut cols = Vec::new();
    let mut c = c0 + 1;
    while let Some(n) = column_number(sheet.cell(r, c)) {
        if let Some(&last) = cols.last()
            && n != last + 1
        {
            break;
        }
        cols.push(n);
        c += 1;
    }
    (cols.len() >= 2).then_some(cols)
}

/// Find every plate matrix in the sheet: a header line of consecutive column numbers followed
/// by lines whose first cell is an increasing row letter. Rows need not be contiguous (partial
/// plates). `continuation` accepts lines with a blank label as further reads of the previous
/// row (Gen5); they are not part of the returned rows.
pub(crate) fn find_grids(sheet: &Sheet) -> Vec<Grid> {
    let mut out = Vec::new();
    let mut r = 0;
    while r < sheet.rows.len() {
        let mut found = None;
        let width = sheet.rows[r].len();
        for c0 in 0..width.min(64) {
            // the corner cell is never itself a column number
            if column_number(sheet.cell(r, c0)).is_some() {
                continue;
            }
            if let Some(cols) = header_at(sheet, r, c0)
                && let Some(g) = grid_rows(sheet, r, c0, cols)
            {
                found = Some(g);
                break;
            }
        }
        if let Some(g) = found {
            r = g.end_row;
            out.push(g);
        } else {
            r += 1;
        }
    }
    out
}

fn grid_rows(sheet: &Sheet, header_row: usize, c0: usize, columns: Vec<u32>) -> Option<Grid> {
    let labels: Vec<(usize, String)> = (header_row + 1..sheet.rows.len())
        .map(|r| (r, sheet.text(r, c0)))
        .take_while(|(_, t)| parse_row_label(t, false).is_some())
        .collect();
    if labels.is_empty() {
        return None;
    }
    let lower_only = labels
        .iter()
        .all(|(_, t)| t.len() == 1 && t.chars().all(|c| c.is_ascii_lowercase()));
    let mut rows = Vec::new();
    let mut last: Option<u32> = None;
    for (r, t) in &labels {
        let pr = parse_row_label(t, lower_only)?;
        if last.is_some_and(|l| pr <= l) {
            break;
        }
        last = Some(pr);
        rows.push((*r, pr));
    }
    let end_row = rows.last().map_or(header_row + 1, |(r, _)| r + 1);
    Some(Grid {
        header_row,
        label_col: c0,
        corner: sheet.text(header_row, c0),
        columns,
        rows,
        end_row,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::text_book;

    #[test]
    fn names() {
        assert_eq!(well_name(0, 0), "A1");
        assert_eq!(well_name(15, 23), "P24");
        assert_eq!(well_name(26, 0), "AA1");
        assert_eq!(well_name(31, 47), "AF48");
        assert_eq!(parse_well("A01"), Some((0, 0)));
        assert_eq!(parse_well("e12"), Some((30, 11)));
        assert_eq!(parse_well("AF48"), Some((31, 47)));
        assert_eq!(parse_well("X"), None);
        assert_eq!(parse_well("A0"), None);
        assert_eq!(parse_row_label("a", true), Some(0));
        assert_eq!(dims_for_wells(384), Some((16, 24)));
        assert_eq!(smallest_plate(12, 23), (16, 24));
        assert_eq!(smallest_plate(7, 11), (8, 12));
    }

    #[test]
    fn scan() {
        let b =
            text_book(b"title\n,1,2,3\nA,1,2,3\nB,4,,6\n\nx\n<>,1,2\nA,7,8\nE,9,10\nI,1,1\nfoo\n");
        let s = &b.sheets[0];
        let g = find_grids(s);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].rows.len(), 2);
        assert_eq!(g[0].cells(s).count(), 5);
        assert_eq!(g[1].corner, "<>");
        assert_eq!(
            g[1].rows.iter().map(|r| r.1).collect::<Vec<_>>(),
            vec![0, 4, 8]
        );
    }
}
