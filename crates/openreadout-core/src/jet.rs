//! A reader for Microsoft Jet 4 / ACE databases (the `.mdb`/`.accdb` container that Arbin
//! `.res` result files use): the catalog, table definitions and the rows of a table, column by
//! column. Written from the documentation of two permissively licensed readers, access_parser
//! (Apache-2.0) and Jackcess (Apache-2.0); see `docs/provenance/arbin-res.md`. Read-only;
//! encrypted and Jet 3 (Access 97) databases are refused. Errors carry the calling format's id.
//!
//! Rows come from every data page a table owns, in page order and, within a page, in the
//! page's row order; deleted rows are skipped and overflow rows followed. Numbers (Boolean, Byte,
//! Integer, Long, Currency, Single, Double, Date/Time as OLE Automation days) are returned as
//! `f64` (NaN when null); Text and Memo as strings. Other column types (binary, OLE, GUID,
//! decimal) are listed but not decoded.

use std::collections::BTreeMap;

use crate::bytes::{le_u16, le_u32, utf16le};
use crate::error::{Error, Result};

/// Page size of Jet 4 and later databases.
pub const PAGE: usize = 4096;

/// First four bytes of every Jet database.
pub const JET_SIGNATURE: [u8; 4] = [0x00, 0x01, 0x00, 0x00];

/// Column types (the type byte of a column definition).
pub mod kind {
    /// Yes/No, stored in the row's null mask.
    pub const BOOLEAN: u8 = 1;
    /// Unsigned 8-bit integer.
    pub const BYTE: u8 = 2;
    /// Signed 16-bit integer.
    pub const INTEGER: u8 = 3;
    /// Signed 32-bit integer.
    pub const LONG: u8 = 4;
    /// Signed 64-bit integer in units of 1/10000.
    pub const CURRENCY: u8 = 5;
    /// IEEE single.
    pub const SINGLE: u8 = 6;
    /// IEEE double.
    pub const DOUBLE: u8 = 7;
    /// OLE Automation date: days since 1899-12-30 as a double.
    pub const DATETIME: u8 = 8;
    /// Short binary.
    pub const BINARY: u8 = 9;
    /// Text (UTF-16LE, or compressed).
    pub const TEXT: u8 = 10;
    /// Long binary (OLE object).
    pub const OLE: u8 = 11;
    /// Long text.
    pub const MEMO: u8 = 12;
    /// GUID.
    pub const GUID: u8 = 15;
    /// Decimal (17 bytes).
    pub const NUMERIC: u8 = 16;
    /// Complex-column id (a signed 32-bit integer).
    pub const COMPLEX: u8 = 18;
}

/// Longest chain of table-definition pages, overflow rows or long-value pages followed.
const MAX_HOPS: usize = 1 << 16;

/// A column of a table definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JetColumn {
    /// Column name.
    pub name: String,
    /// Type byte (see [`kind`]).
    pub kind: u8,
    /// Column number: its bit in a row's null mask.
    pub number: u16,
    /// Slot among the variable-length columns (variable columns only).
    pub var_index: u16,
    /// Position in the table's column order.
    pub index: u16,
    /// True when stored at a fixed offset in the row.
    pub fixed: bool,
    /// Offset in the fixed-length area (fixed columns only).
    pub fixed_offset: u16,
    /// Declared length in bytes.
    pub length: u16,
}

impl JetColumn {
    /// True for the types whose values this module returns as numbers.
    pub fn is_number(&self) -> bool {
        matches!(
            self.kind,
            kind::BOOLEAN
                | kind::BYTE
                | kind::INTEGER
                | kind::LONG
                | kind::CURRENCY
                | kind::SINGLE
                | kind::DOUBLE
                | kind::DATETIME
                | kind::COMPLEX
        )
    }

    /// True for Text and Memo columns.
    pub fn is_text(&self) -> bool {
        matches!(self.kind, kind::TEXT | kind::MEMO)
    }
}

/// The values of one column, row by row.
#[derive(Debug, Clone, PartialEq)]
pub enum JetValues {
    /// Numeric column: NaN where null.
    Numbers(Vec<f64>),
    /// Text column: `None` where null.
    Texts(Vec<Option<String>>),
    /// A type this module does not decode.
    Undecoded,
}

/// A table read column by column.
#[derive(Debug, Clone, PartialEq)]
pub struct JetTable {
    /// Table name.
    pub name: String,
    /// The columns read, in table order.
    pub columns: Vec<JetColumn>,
    /// Values of each column in `columns`.
    pub values: Vec<JetValues>,
    /// Rows read.
    pub rows: usize,
    /// Row count the table definition declares.
    pub declared_rows: u32,
}

impl JetTable {
    /// The numbers of a column by name (`None` when absent or not numeric).
    pub fn numbers(&self, name: &str) -> Option<&[f64]> {
        let k = self.columns.iter().position(|c| c.name == name)?;
        match self.values.get(k)? {
            JetValues::Numbers(v) => Some(v),
            _ => None,
        }
    }

    /// The texts of a column by name (`None` when absent or not text).
    pub fn texts(&self, name: &str) -> Option<&[Option<String>]> {
        let k = self.columns.iter().position(|c| c.name == name)?;
        match self.values.get(k)? {
            JetValues::Texts(v) => Some(v),
            _ => None,
        }
    }
}

/// An open Jet database over its bytes.
#[derive(Debug)]
pub struct Jet<'a> {
    b: &'a [u8],
    format: &'static str,
    version: u32,
    /// Data pages by owning table-definition page, in page order.
    owned: BTreeMap<u32, Vec<u32>>,
    /// User tables: name and table-definition page, in catalog order.
    tables: Vec<(String, u32)>,
}

/// A parsed table definition.
struct Tdef {
    rows: u32,
    columns: Vec<JetColumn>,
}

impl<'a> Jet<'a> {
    /// Open a database. `format` names the calling format in errors.
    ///
    /// # Errors
    /// Not a Jet database, a Jet 3 or encrypted database (unsupported), or a damaged catalog.
    pub fn open(b: &'a [u8], format: &'static str) -> Result<Self> {
        if b.len() < PAGE * 3 || b[..4] != JET_SIGNATURE {
            return Err(Error::corrupt(
                format,
                "not a Jet database (bad signature or too short)",
            ));
        }
        let name = b.get(4..19).unwrap_or(&[]);
        if name != b"Standard Jet DB" && name != b"Standard ACE DB" {
            return Err(Error::corrupt(
                format,
                "not a Jet database (no engine name)",
            ));
        }
        let version = le_u32(b, 0x14).unwrap_or(u32::MAX);
        if version == 0 {
            return Err(Error::unsupported(
                format,
                "a Jet 3 (Access 97) database",
                "open the file in the vendor software and save it in a current version",
            ));
        }
        if version > 5 {
            return Err(Error::unsupported(
                format,
                format!("Jet database version {version}"),
                "report the file with `openreadout report FILE`; the versions read are Jet 4 to ACE 16",
            ));
        }
        let pages = b.len() / PAGE;
        let mut owned: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for p in 1..pages {
            let off = p * PAGE;
            if b.get(off..off + 2) == Some(&[1, 1])
                && let Some(owner) = le_u32(b, off + 4)
                && let Ok(p) = u32::try_from(p)
            {
                owned.entry(owner).or_default().push(p);
            }
        }
        let mut db = Jet {
            b,
            format,
            version,
            owned,
            tables: Vec::new(),
        };
        if b.get(2 * PAGE..2 * PAGE + 2) != Some(&[2, 1]) {
            return Err(Error::unsupported(
                format,
                "an encrypted Jet database (the catalog does not parse)",
                "remove the database encryption in the software that wrote it",
            ));
        }
        let cat = db.read_page_table("MSysObjects", 2, Some(&["Id", "Name", "Type", "Flags"]))?;
        let (Some(ids), Some(names), Some(types), Some(flags)) = (
            cat.numbers("Id"),
            cat.texts("Name"),
            cat.numbers("Type"),
            cat.numbers("Flags"),
        ) else {
            return Err(Error::corrupt(
                format,
                "the database catalog lacks its columns",
            ));
        };
        let mut tables = Vec::new();
        for i in 0..cat.rows {
            let (Some(Some(name)), Some(&id), Some(&ty), Some(&fl)) =
                (names.get(i), ids.get(i), types.get(i), flags.get(i))
            else {
                continue;
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let fl = fl as i64 as u32;
            if (ty - 1.0).abs() > f64::EPSILON || fl & 0x8000_0002 != 0 || name.starts_with("MSys")
            {
                continue;
            }
            if !(0.0..f64::from(u32::MAX)).contains(&id) {
                continue;
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            tables.push((name.clone(), id as u32));
        }
        db.tables = tables;
        Ok(db)
    }

    /// Database version byte (1 Jet 4, 2 ACE 12, 3 ACE 14, 4-5 later ACE).
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Engine name of the version.
    pub fn version_name(&self) -> &'static str {
        match self.version {
            1 => "Jet 4",
            2 => "ACE 12",
            3 => "ACE 14",
            4 => "ACE 15",
            _ => "ACE 16",
        }
    }

    /// User tables in catalog order.
    pub fn tables(&self) -> impl Iterator<Item = &str> {
        self.tables.iter().map(|(n, _)| n.as_str())
    }

    /// True when the database has a user table of this name.
    pub fn has_table(&self, name: &str) -> bool {
        self.tables.iter().any(|(n, _)| n == name)
    }

    /// The columns of a user table, in table order.
    ///
    /// # Errors
    /// Unknown table or damaged definition.
    pub fn columns(&self, table: &str) -> Result<Vec<JetColumn>> {
        let page = self.table_page(table)?;
        Ok(self.tdef(page)?.columns)
    }

    /// Declared row count of a user table.
    ///
    /// # Errors
    /// Unknown table or damaged definition.
    pub fn declared_rows(&self, table: &str) -> Result<u32> {
        let page = self.table_page(table)?;
        Ok(self.tdef(page)?.rows)
    }

    /// Read a user table (only the named columns, when given; absent names are ignored).
    ///
    /// # Errors
    /// Unknown table, damaged definition or damaged row.
    pub fn read(&self, table: &str, wanted: Option<&[&str]>) -> Result<JetTable> {
        let page = self.table_page(table)?;
        self.read_page_table(table, page, wanted)
    }

    fn table_page(&self, table: &str) -> Result<u32> {
        self.tables
            .iter()
            .find(|(n, _)| n == table)
            .map(|(_, p)| *p)
            .ok_or_else(|| Error::corrupt(self.format, format!("no table {table:?}")))
    }

    fn page(&self, n: u32) -> Option<&'a [u8]> {
        let s = (n as usize).checked_mul(PAGE)?;
        self.b.get(s..s.checked_add(PAGE)?)
    }

    fn corrupt(&self, what: impl Into<String>) -> Error {
        Error::corrupt(self.format, what)
    }

    /// Parse the table definition at `page` (following continuation pages).
    fn tdef(&self, page: u32) -> Result<Tdef> {
        let mut t: Vec<u8> = Vec::new();
        let mut p = page;
        for hop in 0.. {
            if hop > MAX_HOPS {
                return Err(self.corrupt("table definition pages loop"));
            }
            let pg = self.page(p).ok_or_else(|| {
                self.corrupt(format!("table definition page {p} outside the file"))
            })?;
            if pg[..2] != [2, 1] {
                return Err(self.corrupt(format!("page {p} is not a table definition")));
            }
            t.extend_from_slice(&pg[8..]);
            p = le_u32(pg, 4).unwrap_or(0);
            if p == 0 {
                break;
            }
        }
        let bad = || self.corrupt(format!("table definition at page {page} is damaged"));
        // offsets below are relative to byte 8 of the first page
        let rows = le_u32(&t, 8).ok_or_else(bad)?;
        let ncols = usize::from(le_u16(&t, 37).ok_or_else(bad)?);
        let real_idx = le_u32(&t, 43).ok_or_else(bad)? as usize;
        if real_idx > 1000 {
            return Err(bad());
        }
        let mut pos = 55 + real_idx * 12;
        let mut columns = Vec::with_capacity(ncols);
        for _ in 0..ncols {
            let c = t.get(pos..pos + 25).ok_or_else(bad)?;
            columns.push(JetColumn {
                name: String::new(),
                kind: c[0],
                number: le_u16(c, 5).ok_or_else(bad)?,
                var_index: le_u16(c, 7).ok_or_else(bad)?,
                index: le_u16(c, 9).ok_or_else(bad)?,
                fixed: c[15] & 0x01 != 0,
                fixed_offset: le_u16(c, 21).ok_or_else(bad)?,
                length: le_u16(c, 23).ok_or_else(bad)?,
            });
            pos += 25;
        }
        for c in &mut columns {
            let n = usize::from(le_u16(&t, pos).ok_or_else(bad)?);
            let raw = t.get(pos + 2..pos + 2 + n).ok_or_else(bad)?;
            c.name = utf16le(raw).replace('\0', "");
            pos += 2 + n;
        }
        columns.sort_by_key(|c| c.index);
        Ok(Tdef { rows, columns })
    }

    fn read_page_table(&self, name: &str, page: u32, wanted: Option<&[&str]>) -> Result<JetTable> {
        let def = self.tdef(page)?;
        let columns: Vec<JetColumn> = def
            .columns
            .into_iter()
            .filter(|c| wanted.is_none_or(|w| w.contains(&c.name.as_str())))
            .collect();
        let mut values: Vec<JetValues> = columns
            .iter()
            .map(|c| {
                if c.is_number() {
                    JetValues::Numbers(Vec::new())
                } else if c.is_text() {
                    JetValues::Texts(Vec::new())
                } else {
                    JetValues::Undecoded
                }
            })
            .collect();
        let mut rows = 0usize;
        let empty = Vec::new();
        for &p in self.owned.get(&page).unwrap_or(&empty) {
            let pg = self
                .page(p)
                .ok_or_else(|| self.corrupt("data page outside the file"))?;
            let n = usize::from(le_u16(pg, 12).unwrap_or(0));
            for r in 0..n {
                let Some(start) = le_u16(pg, 14 + 2 * r) else {
                    return Err(self.corrupt(format!("data page {p}: row table past the page")));
                };
                if start & 0x8000 != 0 {
                    continue; // deleted (or the target of an overflow pointer)
                }
                let row = self.row_bytes(pg, p, r, 0)?;
                self.decode_row(row, &columns, &mut values)?;
                rows += 1;
            }
        }
        Ok(JetTable {
            name: name.to_string(),
            columns,
            values,
            rows,
            declared_rows: def.rows,
        })
    }

    /// The bytes of row `r` of page `pg` (number `p`), following overflow pointers.
    fn row_bytes(&self, pg: &'a [u8], p: u32, r: usize, hops: usize) -> Result<&'a [u8]> {
        if hops > 16 {
            return Err(self.corrupt(format!("data page {p}: overflow rows loop")));
        }
        let raw = le_u16(pg, 14 + 2 * r)
            .ok_or_else(|| self.corrupt(format!("data page {p}: row {r} not in the row table")))?;
        let start = usize::from(raw & 0x1FFF);
        let end = if r == 0 {
            PAGE
        } else {
            usize::from(le_u16(pg, 14 + 2 * (r - 1)).unwrap_or(0) & 0x1FFF)
        };
        let bytes = pg
            .get(start..end)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| self.corrupt(format!("data page {p}: row {r} has bad bounds")))?;
        if raw & 0x4000 != 0 {
            let ptr = le_u32(bytes, 0)
                .ok_or_else(|| self.corrupt(format!("data page {p}: short overflow pointer")))?;
            let (tp, tr) = (ptr >> 8, (ptr & 0xFF) as usize);
            let tpg = self
                .page(tp)
                .filter(|g| g[..2] == [1, 1])
                .ok_or_else(|| self.corrupt(format!("overflow row points to page {tp}")))?;
            return self.row_bytes(tpg, tp, tr, hops + 1);
        }
        Ok(bytes)
    }

    fn decode_row(
        &self,
        row: &[u8],
        columns: &[JetColumn],
        values: &mut [JetValues],
    ) -> Result<()> {
        let bad = |what: &str| self.corrupt(format!("damaged row: {what}"));
        let ncols = usize::from(le_u16(row, 0).ok_or_else(|| bad("no column count"))?);
        let mask_len = ncols.div_ceil(8);
        if mask_len + 2 > row.len() {
            return Err(bad("null mask longer than the row"));
        }
        let mask = &row[row.len() - mask_len..];
        let present = |c: &JetColumn| -> bool {
            let k = usize::from(c.number);
            k < ncols && mask.get(k / 8).is_some_and(|m| m & (1 << (k % 8)) != 0)
        };
        // variable-length offsets, read from the end of the row backwards
        let var_base = row.len() - mask_len;
        let nvar = if columns.iter().any(|c| !c.fixed) {
            usize::from(le_u16(row, var_base.saturating_sub(2)).unwrap_or(0))
        } else {
            0
        };
        let var_slot = |k: usize| -> Option<usize> {
            let at = var_base.checked_sub(4 + 2 * k)?;
            le_u16(row, at).map(usize::from)
        };
        for (c, out) in columns.iter().zip(values.iter_mut()) {
            let has = present(c);
            if c.kind == kind::BOOLEAN {
                if let JetValues::Numbers(v) = out {
                    v.push(if has { 1.0 } else { 0.0 });
                }
                continue;
            }
            let data: Option<&[u8]> = if !has {
                None
            } else if c.fixed {
                let s = 2 + usize::from(c.fixed_offset);
                let e = s + usize::from(c.length);
                Some(
                    row.get(s..e)
                        .ok_or_else(|| bad("fixed column past the row"))?,
                )
            } else {
                let k = usize::from(c.var_index);
                if k >= nvar {
                    None
                } else {
                    let (s, e) = (
                        var_slot(k).ok_or_else(|| bad("variable offsets"))?,
                        var_slot(k + 1).ok_or_else(|| bad("variable offsets"))?,
                    );
                    if s > e || e > var_base {
                        return Err(bad("variable column bounds"));
                    }
                    Some(&row[s..e])
                }
            };
            match out {
                JetValues::Numbers(v) => {
                    v.push(data.and_then(|d| number(c.kind, d)).unwrap_or(f64::NAN));
                }
                JetValues::Texts(v) => v.push(match data {
                    None => None,
                    Some(d) if c.kind == kind::MEMO => Some(text(&self.long_value(d)?)),
                    Some(d) => Some(text(d)),
                }),
                JetValues::Undecoded => {}
            }
        }
        Ok(())
    }

    /// The bytes of a long value (Memo/OLE) from its 12-byte definition and inline data.
    fn long_value(&self, def: &[u8]) -> Result<Vec<u8>> {
        let bad = |what: &str| self.corrupt(format!("long value: {what}"));
        let head = le_u32(def, 0).ok_or_else(|| bad("short definition"))?;
        let len = (head & 0x3FFF_FFFF) as usize;
        let ptr = le_u32(def, 4).ok_or_else(|| bad("short definition"))?;
        match head >> 30 {
            2 => {
                // stored in the row
                let d = def.get(12..).unwrap_or(&[]);
                Ok(d[..len.min(d.len())].to_vec())
            }
            1 => {
                let (p, r) = (ptr >> 8, (ptr & 0xFF) as usize);
                let pg = self.page(p).ok_or_else(|| bad("page outside the file"))?;
                let bytes = self.lval_row(pg, p, r)?;
                Ok(bytes[..len.min(bytes.len())].to_vec())
            }
            0 => {
                let mut out = Vec::with_capacity(len.min(1 << 24));
                let (mut p, mut r) = (ptr >> 8, (ptr & 0xFF) as usize);
                for _ in 0..MAX_HOPS {
                    if out.len() >= len {
                        break;
                    }
                    let pg = self.page(p).ok_or_else(|| bad("page outside the file"))?;
                    let bytes = self.lval_row(pg, p, r)?;
                    let next = le_u32(bytes, 0).ok_or_else(|| bad("short chunk"))?;
                    let chunk = &bytes[4..];
                    let take = chunk.len().min(len - out.len());
                    out.extend_from_slice(&chunk[..take]);
                    if next == 0 {
                        break;
                    }
                    (p, r) = (next >> 8, (next & 0xFF) as usize);
                }
                Ok(out)
            }
            _ => Err(bad("unknown storage type")),
        }
    }

    fn lval_row(&self, pg: &'a [u8], p: u32, r: usize) -> Result<&'a [u8]> {
        if pg[..2] != [1, 1] {
            return Err(self.corrupt(format!("long value page {p} is not a data page")));
        }
        let raw = le_u16(pg, 14 + 2 * r)
            .ok_or_else(|| self.corrupt(format!("long value row {r} of page {p} missing")))?;
        let start = usize::from(raw & 0x1FFF);
        let end = if r == 0 {
            PAGE
        } else {
            usize::from(le_u16(pg, 14 + 2 * (r - 1)).unwrap_or(0) & 0x1FFF)
        };
        pg.get(start..end)
            .ok_or_else(|| self.corrupt(format!("long value row {r} of page {p} has bad bounds")))
    }
}

/// A fixed-size number of a column type (`None` for other types or short data).
fn number(k: u8, d: &[u8]) -> Option<f64> {
    let a = |n: usize| d.get(..n);
    Some(match k {
        kind::BYTE => f64::from(*d.first()?),
        kind::INTEGER => f64::from(i16::from_le_bytes(a(2)?.try_into().ok()?)),
        kind::LONG | kind::COMPLEX => f64::from(i32::from_le_bytes(a(4)?.try_into().ok()?)),
        #[allow(clippy::cast_precision_loss)]
        kind::CURRENCY => i64::from_le_bytes(a(8)?.try_into().ok()?) as f64 / 10_000.0,
        kind::SINGLE => f64::from(f32::from_le_bytes(a(4)?.try_into().ok()?)),
        kind::DOUBLE | kind::DATETIME => f64::from_le_bytes(a(8)?.try_into().ok()?),
        _ => return None,
    })
}

/// Decode a Text value: UTF-16LE, or "compressed" (`FF FE`, then segments separated by a zero
/// byte that alternate between one byte per character and UTF-16LE, starting with one byte).
pub fn text(b: &[u8]) -> String {
    if b.len() > 1 && b[0] == 0xFF && b[1] == 0xFE {
        let mut units: Vec<u16> = Vec::with_capacity(b.len());
        let mut compressed = true;
        let mut seg: Vec<u8> = Vec::new();
        let flush = |seg: &mut Vec<u8>, compressed: bool, units: &mut Vec<u16>| {
            if compressed {
                units.extend(seg.iter().map(|&c| u16::from(c)));
            } else {
                units.extend(
                    seg.as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes(*c)),
                );
            }
            seg.clear();
        };
        for &c in &b[2..] {
            if c == 0 {
                flush(&mut seg, compressed, &mut units);
                compressed = !compressed;
            } else {
                seg.push(c);
            }
        }
        flush(&mut seg, compressed, &mut units);
        return String::from_utf16_lossy(&units).replace('\0', "");
    }
    utf16le(b).replace('\0', "")
}

/// An OLE Automation date (days since 1899-12-30, local time) as ISO-8601 without a zone,
/// rounded to the millisecond; `None` outside years 1900-9999.
pub fn ole_date_iso(days: f64) -> Option<String> {
    if !days.is_finite() || !(1.0..2_958_466.0).contains(&days) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let ms = ((days - 25_569.0) * 86_400_000.0).round() as i64;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let s = crate::time::unix_to_iso8601(ms.div_euclid(1000), ms.rem_euclid(1000) as u32);
    let s = s.trim_end_matches('Z');
    Some(s.strip_suffix(".000").unwrap_or(s).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_text() {
        assert_eq!(text(&[0xFF, 0xFE, b'a', b'b']), "ab");
        // "a", a zero byte, a UTF-16LE segment (U+0141), a zero byte, one byte per character
        assert_eq!(
            text(&[0xFF, 0xFE, b'a', 0, 0x41, 0x01, 0, b'c']),
            "a\u{141}c"
        );
        assert_eq!(text(&[b'h', 0, b'i', 0]), "hi");
        assert_eq!(text(&[]), "");
    }

    #[test]
    fn dates() {
        assert_eq!(
            ole_date_iso(42_587.678_043_981_48).as_deref(),
            Some("2016-08-05T16:16:23")
        );
        assert_eq!(
            ole_date_iso(25_569.5).as_deref(),
            Some("1970-01-01T12:00:00")
        );
        assert_eq!(ole_date_iso(f64::NAN), None);
        assert_eq!(ole_date_iso(-3.0), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(number(kind::INTEGER, &(-5i16).to_le_bytes()), Some(-5.0));
        assert_eq!(
            number(kind::CURRENCY, &12_345i64.to_le_bytes()),
            Some(1.2345)
        );
        assert_eq!(number(kind::SINGLE, &1.5f32.to_le_bytes()), Some(1.5));
        assert_eq!(number(kind::LONG, &[1, 2]), None);
        assert_eq!(number(kind::GUID, &[0; 16]), None);
    }

    #[test]
    fn refuses_non_jet() {
        let mut b = vec![0u8; PAGE * 3];
        assert!(Jet::open(&b, "x").is_err());
        b[..4].copy_from_slice(&JET_SIGNATURE);
        b[4..19].copy_from_slice(b"Standard Jet DB");
        // version 0: Jet 3
        assert!(matches!(Jet::open(&b, "x"), Err(Error::Unsupported { .. })));
        b[0x14] = 1;
        // no catalog
        assert!(matches!(Jet::open(&b, "x"), Err(Error::Unsupported { .. })));
    }
}
