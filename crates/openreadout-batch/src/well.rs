//! Plate well names: `A1`, `A01`, `a001`, `B-12`, `AF48` (1536-well plates have rows A–AF and
//! columns 1–48), `R3C7`, and row/column pairs. Every form becomes a zero-based (row, column)
//! and a canonical name `A01`.

/// Largest plate handled: 1536 wells (32 rows × 48 columns). Names beyond it are not wells.
pub const MAX_ROWS: u32 = 32;
/// See [`MAX_ROWS`].
pub const MAX_COLS: u32 = 48;

/// A well position, zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Well {
    /// Row (A = 0).
    pub row: u32,
    /// Column (1 = 0).
    pub col: u32,
}

impl Well {
    /// Canonical name: row letter(s) and a two-digit column (`A01`, `P24`, `AF48`).
    pub fn name(self) -> String {
        format!("{}{:02}", row_letters(self.row), self.col + 1)
    }
}

/// `A` for row 0, …, `Z` for 25, `AA` for 26, … (Excel-style).
pub fn row_letters(row: u32) -> String {
    let mut n = row + 1;
    let mut s = Vec::new();
    while n > 0 {
        let r = (n - 1) % 26;
        s.push(char::from(b'A' + r as u8));
        n = (n - 1) / 26;
    }
    s.iter().rev().collect()
}

/// Row index of `A`…`Z`, `AA`…`AF` (case-insensitive).
pub fn row_index(letters: &str) -> Option<u32> {
    if letters.is_empty() || letters.len() > 2 {
        return None;
    }
    let mut n: u32 = 0;
    for c in letters.chars() {
        let c = c.to_ascii_uppercase();
        if !c.is_ascii_uppercase() {
            return None;
        }
        n = n * 26 + (c as u32 - 'A' as u32 + 1);
    }
    let r = n - 1;
    (r < MAX_ROWS).then_some(r)
}

/// Parse a well name. Accepts `A1`, `A01`, `A001`, `A-1`, `A_01`, `A 1`, `R1C1` and `r01c01`.
pub fn parse(s: &str) -> Option<Well> {
    let t = s.trim();
    if t.is_empty() || t.len() > 12 {
        return None;
    }
    // R<row>C<col> (numeric rows, 1-based)
    let up = t.to_ascii_uppercase();
    if let Some(rest) = up.strip_prefix('R')
        && let Some((r, c)) = rest.split_once('C')
        && !r.is_empty()
        && !c.is_empty()
        && r.bytes().all(|b| b.is_ascii_digit())
        && c.bytes().all(|b| b.is_ascii_digit())
    {
        let r: u32 = r.parse().ok()?;
        let c: u32 = c.parse().ok()?;
        return make(r.checked_sub(1)?, c.checked_sub(1)?);
    }
    let letters: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
    let rest = t[letters.len()..].trim_start_matches(['-', '_', ' ']);
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) || rest.len() > 3 {
        return None;
    }
    let row = row_index(&letters)?;
    let col: u32 = rest.parse().ok()?;
    make(row, col.checked_sub(1)?)
}

fn make(row: u32, col: u32) -> Option<Well> {
    (row < MAX_ROWS && col < MAX_COLS).then_some(Well { row, col })
}

/// Plate format that holds every well given: 6, 12, 24, 48, 96, 384 or 1536 wells.
pub fn plate_size(wells: impl Iterator<Item = Well>) -> u32 {
    let (mut r, mut c) = (0, 0);
    for w in wells {
        r = r.max(w.row + 1);
        c = c.max(w.col + 1);
    }
    for (rows, cols, n) in [
        (2, 3, 6),
        (3, 4, 12),
        (4, 6, 24),
        (6, 8, 48),
        (8, 12, 96),
        (16, 24, 384),
        (32, 48, 1536),
    ] {
        if r <= rows && c <= cols {
            return n;
        }
    }
    1536
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms() {
        let a1 = Well { row: 0, col: 0 };
        for s in [
            "A1", "a01", "A001", "A-1", "A_01", " A 1 ", "R1C1", "r01c01",
        ] {
            assert_eq!(parse(s), Some(a1), "{s}");
        }
        assert_eq!(parse("H12").unwrap().name(), "H12");
        assert_eq!(parse("p24").unwrap().name(), "P24");
        assert_eq!(parse("AF48").unwrap(), Well { row: 31, col: 47 });
        assert_eq!(parse("AG1"), None);
        assert_eq!(parse("A49"), None);
        assert_eq!(parse("A0"), None);
        assert_eq!(parse("sample1"), None);
        assert_eq!(parse("ABC1"), None);
        assert_eq!(parse("12"), None);
        assert_eq!(row_letters(26), "AA");
        assert_eq!(row_index("AF"), Some(31));
    }

    #[test]
    fn plate_sizes() {
        let w = |s: &str| parse(s).unwrap();
        assert_eq!(plate_size([w("A1"), w("H12")].into_iter()), 96);
        assert_eq!(plate_size([w("P24")].into_iter()), 384);
        assert_eq!(plate_size([w("B3")].into_iter()), 6);
        assert_eq!(plate_size([w("AF1")].into_iter()), 1536);
    }
}
