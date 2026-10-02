//! One module per export dialect, plus helpers they share.

pub(crate) mod bmg;
pub(crate) mod cursor;
pub(crate) mod envision;
pub(crate) mod gen5;
pub(crate) mod gen5_xpt;
pub(crate) mod generic;
pub(crate) mod skanit;
pub(crate) mod softmax;
pub(crate) mod softmax_pda;
pub(crate) mod softmax_sda;
pub(crate) mod tecan;
pub(crate) mod tecan_csv;

use crate::grid::{Grid, find_grids};
use crate::model::{Block, Channel, first_number};
use crate::sheet::Sheet;

/// A plate matrix with the non-blank lines between the previous matrix and its header.
#[derive(Debug, Clone)]
pub(crate) struct Titled {
    pub(crate) grid: Grid,
    /// Titles, nearest last, each as the line's non-blank cells joined by a space.
    pub(crate) titles: Vec<String>,
}

impl Titled {
    /// The nearest title line (empty when the matrix follows another directly).
    pub(crate) fn title(&self) -> &str {
        self.titles.last().map_or("", String::as_str)
    }
    /// The nearest title line that satisfies `f`.
    pub(crate) fn find(&self, f: impl Fn(&str) -> bool) -> Option<&str> {
        self.titles.iter().rev().map(String::as_str).find(|t| f(t))
    }
}

pub(crate) fn titled_grids(sheet: &Sheet) -> Vec<Titled> {
    let mut out = Vec::new();
    let mut prev_end = 0;
    for grid in find_grids(sheet) {
        let titles = (prev_end..grid.header_row)
            .filter(|&r| !sheet.row_is_blank(r))
            .map(|r| sheet.row_line(r))
            .collect();
        prev_end = grid.end_row;
        out.push(Titled { grid, titles });
    }
    out
}

/// Copy a matrix into a block under one channel.
pub(crate) fn push_grid(
    sheet: &Sheet,
    grid: &Grid,
    block: &mut Block,
    ch: u32,
    time_s: Option<f64>,
) {
    for (r, c, cell) in grid.cells(sheet) {
        let cell = cell.clone();
        block.push_cell(r, c, ch, time_s, None, &cell);
    }
}

/// `Key: value` cells of a line, each cell holding one pair (`User: USER,Path: C:\…`).
pub(crate) fn colon_pairs(sheet: &Sheet, r: usize) -> Vec<(String, String)> {
    sheet
        .row_texts(r)
        .iter()
        .filter_map(|t| t.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty() && k.len() <= 64)
        .collect()
}

/// A line of the form `key<cells>value[<cells>unit]`: the first cell and the next non-blank one.
pub(crate) fn key_value(sheet: &Sheet, r: usize) -> Option<(String, String)> {
    let row = sheet.rows.get(r)?;
    let mut it = row.iter().enumerate().filter(|(_, c)| !c.is_blank());
    let (_, k) = it.next()?;
    let (_, v) = it.next()?;
    Some((
        k.trimmed().trim_end_matches(':').trim().to_string(),
        v.trimmed(),
    ))
}

/// Excitation/emission from a filter label: `580/620`, `480-14/520-30`, `485 520`, `450`.
/// Returns (excitation, emission or single wavelength, excitation bandwidth, emission bandwidth).
pub(crate) fn filter_pair(s: &str) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    let part = |p: &str| {
        let mut it = p.split('-');
        let wl = it.next().and_then(first_number);
        let bw = it.next().and_then(first_number);
        (wl, bw)
    };
    if let Some((a, b)) = s.split_once('/') {
        let (ex, exb) = part(a);
        let (em, emb) = part(b);
        return (ex, em, exb, emb);
    }
    let nums: Vec<&str> = s.split_whitespace().collect();
    if let [a, b] = nums.as_slice()
        && a.chars().all(|c| c.is_ascii_digit() || c == '.')
        && b.chars().all(|c| c.is_ascii_digit() || c == '.')
    {
        return (first_number(a), first_number(b), None, None);
    }
    let (wl, bw) = part(s);
    (None, wl, None, bw)
}

/// Text inside the first parentheses: `Raw Data (450)` → `450`.
pub(crate) fn parenthesized(s: &str) -> Option<&str> {
    let a = s.find('(')?;
    let b = s[a..].find(')')? + a;
    Some(s[a + 1..b].trim())
}

/// A calculated channel for vendor-derived matrices (blank correction, ratios, curves).
pub(crate) fn calculated(label: &str, like: Option<&Channel>) -> Channel {
    let mut c = Channel::new(label, like.map_or(crate::model::Mode::Unknown, |c| c.mode));
    if let Some(l) = like {
        c.mode_basis = l.mode_basis;
        c.wavelength_nm = l.wavelength_nm;
        c.excitation_nm = l.excitation_nm;
        c.emission_nm = l.emission_nm;
    }
    c.calculated = true;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters() {
        assert_eq!(
            filter_pair("580/620"),
            (Some(580.0), Some(620.0), None, None)
        );
        assert_eq!(
            filter_pair("480-14/520-30"),
            (Some(480.0), Some(520.0), Some(14.0), Some(30.0))
        );
        assert_eq!(filter_pair("450"), (None, Some(450.0), None, None));
        assert_eq!(
            filter_pair("485 520"),
            (Some(485.0), Some(520.0), None, None)
        );
        assert_eq!(parenthesized("Raw Data (No filter)"), Some("No filter"));
    }
}
