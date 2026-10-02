//! A uniform cell grid for every export: delimited text files become one sheet of text cells,
//! workbooks (XLSX, XLS, XLSB, ODS) become one sheet per worksheet with typed cells.

use std::path::Path;

use calamine::{Data, Reader};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use crate::text::{self, Delimiter, Encoding};

/// One spreadsheet cell.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Cell {
    Empty,
    Text(String),
    Number(f64),
    /// Excel date/time serial (days since 1899-12-30; the fraction is the time of day).
    Date(f64),
}

impl Cell {
    /// The cell as text: numbers in their shortest round-trip form, dates as serials.
    pub(crate) fn text(&self) -> String {
        match self {
            Cell::Empty => String::new(),
            Cell::Text(s) => s.clone(),
            Cell::Number(v) => fmt_num(*v),
            // a time of day alone (Gen5's Excel `Time` cell) is a fraction of a day
            Cell::Date(v) if (0.0..1.0).contains(v) => {
                let s = (v * 86_400.0).round() as u32;
                format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
            }
            Cell::Date(v) => crate::datetime::excel_serial(*v).unwrap_or_else(|| fmt_num(*v)),
        }
    }
    pub(crate) fn trimmed(&self) -> String {
        self.text().trim().to_string()
    }
    pub(crate) fn is_blank(&self) -> bool {
        match self {
            Cell::Empty => true,
            Cell::Text(s) => s.trim().is_empty(),
            _ => false,
        }
    }
    /// A numeric value: a number cell, or text that is a plain decimal number.
    pub(crate) fn number(&self) -> Option<f64> {
        match self {
            Cell::Number(v) => Some(*v),
            Cell::Text(s) => parse_number(s),
            _ => None,
        }
    }
}

/// Shortest text that parses back to the same f64; integers without a fraction.
pub(crate) fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// Parse a plain decimal number (`-1.5`, `2.1e3`, `+7`). Rejects `NaN`, `inf`, thousands
/// separators and decimal commas: those are reported as non-numeric, not guessed.
pub(crate) fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let mut digits = 0;
    let mut seen_e = false;
    let mut prev = ' ';
    for c in body.chars() {
        match c {
            '0'..='9' => digits += 1,
            '.' if !seen_e => {}
            'e' | 'E' if digits > 0 && !seen_e => seen_e = true,
            '+' | '-' if matches!(prev, 'e' | 'E') => {}
            _ => return None,
        }
        prev = c;
    }
    if digits == 0 {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// One worksheet (or the single sheet of a text export).
#[derive(Debug, Clone, Default)]
pub(crate) struct Sheet {
    pub(crate) name: String,
    pub(crate) rows: Vec<Vec<Cell>>,
}

const EMPTY: Cell = Cell::Empty;

impl Sheet {
    pub(crate) fn cell(&self, r: usize, c: usize) -> &Cell {
        self.rows
            .get(r)
            .and_then(|row| row.get(c))
            .unwrap_or(&EMPTY)
    }
    pub(crate) fn text(&self, r: usize, c: usize) -> String {
        self.cell(r, c).trimmed()
    }
    /// Index of the last non-blank cell in row `r`, if any.
    pub(crate) fn row_len(&self, r: usize) -> usize {
        self.rows.get(r).map_or(0, |row| {
            row.iter().rposition(|c| !c.is_blank()).map_or(0, |i| i + 1)
        })
    }
    pub(crate) fn row_is_blank(&self, r: usize) -> bool {
        self.row_len(r) == 0
    }
    /// The non-blank cells of row `r`, trimmed, in order.
    pub(crate) fn row_texts(&self, r: usize) -> Vec<String> {
        self.rows.get(r).map_or_else(Vec::new, |row| {
            row.iter()
                .filter(|c| !c.is_blank())
                .map(Cell::trimmed)
                .collect()
        })
    }
    /// The whole row joined with single spaces (for pattern matching on titles).
    pub(crate) fn row_line(&self, r: usize) -> String {
        self.row_texts(r).join(" ")
    }
}

/// How the export was stored.
#[derive(Debug, Clone)]
pub(crate) enum Container {
    Text {
        encoding: Encoding,
        delimiter: Delimiter,
    },
    Workbook {
        kind: &'static str,
    },
    /// A vendor binary document (SoftMax Pro `.pda`/`.sda`, Gen5 `.xpt`).
    Binary {
        kind: &'static str,
    },
}

impl Container {
    pub(crate) fn describe(&self) -> serde_json::Value {
        match self {
            Container::Text {
                encoding,
                delimiter,
            } => {
                serde_json::json!({"kind": "text", "encoding": encoding.name(), "delimiter": delimiter.name()})
            }
            Container::Workbook { kind } | Container::Binary { kind } => {
                serde_json::json!({"kind": kind})
            }
        }
    }
}

/// A loaded export: its sheets and how they were stored.
#[derive(Debug, Clone)]
pub(crate) struct Book {
    pub(crate) sheets: Vec<Sheet>,
    pub(crate) container: Container,
}

/// Is this a workbook container (by signature: ZIP for XLSX/XLSB/ODS, OLE2 for XLS)?
pub(crate) fn is_workbook(head: &[u8]) -> bool {
    head.starts_with(b"PK\x03\x04")
        || head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1])
}

pub(crate) fn text_book(bytes: &[u8]) -> Book {
    let (text, encoding) = text::decode(bytes);
    let lines = text::lines(&text);
    let delimiter = text::sniff_delimiter(&lines);
    let rows = lines
        .iter()
        .map(|l| {
            text::split(l, delimiter)
                .into_iter()
                .map(|f| {
                    if f.is_empty() {
                        Cell::Empty
                    } else {
                        Cell::Text(f)
                    }
                })
                .collect()
        })
        .collect();
    Book {
        sheets: vec![Sheet {
            name: String::new(),
            rows,
        }],
        container: Container::Text {
            encoding,
            delimiter,
        },
    }
}

fn cell_from(d: &Data) -> Cell {
    match d {
        Data::Empty => Cell::Empty,
        Data::String(s) | Data::DateTimeIso(s) | Data::DurationIso(s) => {
            if s.is_empty() {
                Cell::Empty
            } else {
                Cell::Text(s.clone())
            }
        }
        Data::Float(f) => Cell::Number(*f),
        Data::Int(i) => Cell::Number(*i as f64),
        Data::Bool(b) => Cell::Text(if *b { "TRUE" } else { "FALSE" }.into()),
        Data::DateTime(dt) => Cell::Date(dt.as_f64()),
        Data::Error(e) => Cell::Text(format!("#{e:?}")),
    }
}

/// Most cells a worksheet may expand to. Plate exports hold a few thousand; the bound keeps a
/// damaged workbook (one cell at A1, another at XFD1048576) from sizing a dense grid of
/// billions of cells.
const MAX_SHEET_CELLS: usize = 4 << 20;

pub(crate) fn workbook_book(path: &Path) -> Result<Book> {
    guarded(|| book_of(calamine::open_workbook_auto(path).map_err(bad_workbook)?))
}

/// [`workbook_book`] for a workbook held in memory (read from a byte source); `name`'s
/// extension picks the reader like `calamine::open_workbook_auto` does for a path.
pub(crate) fn workbook_book_bytes(name: &Path, bytes: Vec<u8>) -> Result<Book> {
    use calamine::{Ods, Sheets, Xls, Xlsb, Xlsx, open_workbook_from_rs};
    guarded(|| {
        let c = std::io::Cursor::new(bytes);
        let ext = name
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let wb = match ext.as_deref() {
            Some("xls" | "xla") => Sheets::Xls(
                open_workbook_from_rs::<Xls<_>, _>(c).map_err(|e| bad_workbook(e.into()))?,
            ),
            Some("xlsx" | "xlsm" | "xlam") => Sheets::Xlsx(
                open_workbook_from_rs::<Xlsx<_>, _>(c).map_err(|e| bad_workbook(e.into()))?,
            ),
            Some("xlsb") => Sheets::Xlsb(
                open_workbook_from_rs::<Xlsb<_>, _>(c).map_err(|e| bad_workbook(e.into()))?,
            ),
            Some("ods") => Sheets::Ods(
                open_workbook_from_rs::<Ods<_>, _>(c).map_err(|e| bad_workbook(e.into()))?,
            ),
            _ => calamine::open_workbook_auto_from_rs(c).map_err(bad_workbook)?,
        };
        book_of(wb)
    })
}

/// The workbook parsers are third-party code; a panic in them (seen: an arithmetic overflow
/// on an absurd cell reference in debug builds) becomes a corrupt-file error where panics
/// unwind (tests, the Python extension).
fn guarded(f: impl FnOnce() -> Result<Book>) -> Result<Book> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| {
        Err(Error::corrupt(
            FORMAT_ID,
            "the workbook parser failed on this file (damaged workbook)",
        ))
    })
}

fn bad_workbook(e: calamine::Error) -> Error {
    Error::corrupt(FORMAT_ID, format!("workbook could not be read: {e}"))
}

fn book_of<RS: std::io::Read + std::io::Seek>(mut wb: calamine::Sheets<RS>) -> Result<Book> {
    let bad = bad_workbook;
    let kind = match &wb {
        calamine::Sheets::Xls(_) => "xls",
        calamine::Sheets::Xlsx(_) => "xlsx",
        calamine::Sheets::Xlsb(_) => "xlsb",
        calamine::Sheets::Ods(_) => "ods",
    };
    let names = wb.sheet_names();
    let mut sheets = Vec::with_capacity(names.len());
    for name in names {
        let rows = match &mut wb {
            // Streamed cell by cell: `worksheet_range` would allocate the dense bounding box of
            // every cell first, whatever the coordinates say.
            calamine::Sheets::Xlsx(x) => {
                let mut rd = x
                    .worksheet_cells_reader(&name)
                    .map_err(|e| bad(calamine::Error::Xlsx(e)))?;
                let mut rows: Vec<Vec<Cell>> = Vec::new();
                let mut cells = 0usize;
                while let Some(c) = rd.next_cell().map_err(|e| bad(calamine::Error::Xlsx(e)))? {
                    let value = cell_from(&Data::from(c.get_value().clone()));
                    if value.is_blank() {
                        continue;
                    }
                    let (r, col) = c.get_position();
                    let (r, col) = (r as usize, col as usize);
                    let grow = r
                        .saturating_add(1)
                        .saturating_sub(rows.len())
                        .saturating_add(
                            col.saturating_add(1)
                                .saturating_sub(rows.get(r).map_or(0, Vec::len)),
                        );
                    cells = cells.saturating_add(grow);
                    if cells > MAX_SHEET_CELLS {
                        return Err(Error::corrupt(
                            FORMAT_ID,
                            format!(
                                "worksheet {name:?} spans more than {MAX_SHEET_CELLS} cells (a value at row {}, column {})",
                                r + 1,
                                col + 1
                            ),
                        ));
                    }
                    if rows.len() <= r {
                        rows.resize(r + 1, Vec::new());
                    }
                    let row = &mut rows[r];
                    if row.len() <= col {
                        row.resize(col + 1, Cell::Empty);
                    }
                    row[col] = value;
                }
                rows
            }
            other => {
                let range = other.worksheet_range(&name).map_err(bad)?;
                let (r0, c0) = range
                    .start()
                    .map_or((0, 0), |(r, c)| (r as usize, c as usize));
                let (h, w) = range.get_size();
                let span = r0.saturating_add(h).saturating_mul(c0.saturating_add(w));
                if span > MAX_SHEET_CELLS {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("worksheet {name:?} spans more than {MAX_SHEET_CELLS} cells"),
                    ));
                }
                let mut rows: Vec<Vec<Cell>> = vec![Vec::new(); r0];
                for row in range.rows() {
                    let mut out = vec![Cell::Empty; c0];
                    out.extend(row.iter().map(cell_from));
                    rows.push(out);
                }
                rows
            }
        };
        sheets.push(Sheet { name, rows });
    }
    Ok(Book {
        sheets,
        container: Container::Workbook { kind },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(parse_number(" 2.100 "), Some(2.1));
        assert_eq!(parse_number("-0.066"), Some(-0.066));
        assert_eq!(parse_number("1e3"), Some(1000.0));
        assert_eq!(parse_number("OVRFLW"), None);
        assert_eq!(parse_number("NaN"), None);
        assert_eq!(parse_number("inf"), None);
        assert_eq!(parse_number("0,02"), None);
        assert_eq!(parse_number("1.2.3"), None);
        assert_eq!(parse_number("-"), None);
        assert_eq!(parse_number("."), None);
        assert_eq!(fmt_num(96.0), "96");
        assert_eq!(fmt_num(0.1), "0.1");
    }

    #[test]
    fn text_sheet() {
        let b = text_book(b"a\tb\r\n\t1\t\r\n");
        let s = &b.sheets[0];
        assert_eq!(s.text(0, 1), "b");
        assert_eq!(s.cell(1, 1).number(), Some(1.0));
        assert_eq!(s.row_len(1), 2);
        assert!(s.cell(9, 9).is_blank());
    }
}
