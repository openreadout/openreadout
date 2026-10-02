//! A minimal, read-only reader of SQLite database files: enough to read whole ordinary
//! (rowid) tables, including a write-ahead log left next to the database.
//!
//! Written from the public-domain file-format document <https://www.sqlite.org/fileformat2.html>
//! (see `docs/provenance/bruker-tdf.md`). Every read is bounds-checked; a damaged file is an
//! error, never a panic.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{Endian, be_f64, be_u16, be_u32};
use openreadout_core::source::{Fs, SourceFile};
use serde_json::Value;

/// Why the database could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqliteError {
    Io(String),
    Corrupt(String),
    Unsupported(String),
}

impl std::fmt::Display for SqliteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SqliteError::Io(m) => write!(f, "I/O: {m}"),
            SqliteError::Corrupt(m) => write!(f, "damaged SQLite database: {m}"),
            SqliteError::Unsupported(m) => write!(f, "unsupported SQLite feature: {m}"),
        }
    }
}

type R<T> = std::result::Result<T, SqliteError>;

fn corrupt<T>(m: impl Into<String>) -> R<T> {
    Err(SqliteError::Corrupt(m.into()))
}

/// One stored value.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl SqlValue {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            SqlValue::Integer(v) => Some(*v),
            SqlValue::Real(v) if v.fract() == 0.0 && v.abs() < 9.0e15 => Some(*v as i64),
            SqlValue::Text(t) => t.trim().parse().ok(),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            SqlValue::Integer(v) => Some(*v as f64),
            SqlValue::Real(v) => Some(*v),
            SqlValue::Text(t) => t.trim().parse().ok(),
            _ => None,
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            SqlValue::Text(t) => Some(t),
            _ => None,
        }
    }
    /// JSON for `info --view full` (blobs as their length).
    pub fn to_json(&self) -> Value {
        match self {
            SqlValue::Null => Value::Null,
            SqlValue::Integer(v) => Value::from(*v),
            SqlValue::Real(v) => {
                serde_json::Number::from_f64(*v).map_or(Value::Null, Value::Number)
            }
            SqlValue::Text(t) => Value::String(t.clone()),
            SqlValue::Blob(b) => serde_json::json!({ "blob_bytes": b.len() }),
        }
    }
}

/// A whole table: column names from its `CREATE TABLE` statement, rows in rowid order.
#[derive(Debug, Clone, Default)]
pub struct SqlTable {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlValue>>,
}

impl SqlTable {
    pub fn column(&self, name: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(name))
    }
    /// Value of column `name` in row `row` (`Null` when the column does not exist).
    pub fn value(&self, row: usize, name: &str) -> &SqlValue {
        static NULL: SqlValue = SqlValue::Null;
        self.column(name)
            .and_then(|c| self.rows.get(row).and_then(|r| r.get(c)))
            .unwrap_or(&NULL)
    }
}

/// A `sqlite_schema` row.
#[derive(Debug, Clone)]
struct SchemaEntry {
    kind: String,
    name: String,
    rootpage: u32,
    sql: String,
}

/// Committed WAL frames: page number → byte offset of the page image in the WAL file.
#[derive(Debug)]
struct Wal {
    file: SourceFile,
    pages: HashMap<u32, u64>,
    db_pages: u32,
    frames: u32,
}

/// An open database.
#[derive(Debug)]
pub struct SqliteDb {
    path: PathBuf,
    file: SourceFile,
    file_len: u64,
    page_size: u32,
    usable: u32,
    page_count: u32,
    wal: Option<Wal>,
    schema: Vec<SchemaEntry>,
}

fn need<T>(b: &[u8], at: usize, read: fn(&[u8], usize) -> Option<T>) -> R<T> {
    read(b, at).ok_or_else(|| SqliteError::Corrupt(format!("read past page end at {at}")))
}

/// SQLite varint at `at`: value and length.
fn varint(b: &[u8], at: usize) -> R<(u64, usize)> {
    let mut v: u64 = 0;
    for i in 0..9 {
        let c = *b
            .get(at + i)
            .ok_or_else(|| SqliteError::Corrupt("varint runs past the end".into()))?;
        if i == 8 {
            v = (v << 8) | u64::from(c);
            return Ok((v, 9));
        }
        v = (v << 7) | u64::from(c & 0x7f);
        if c & 0x80 == 0 {
            return Ok((v, i + 1));
        }
    }
    unreachable!("loop returns by the 9th byte")
}

/// The WAL checksum over `data` (a multiple of 8 bytes), continuing from `(s0, s1)`.
fn wal_checksum(data: &[u8], big_endian: bool, mut s0: u32, mut s1: u32) -> (u32, u32) {
    let order = if big_endian {
        Endian::Big
    } else {
        Endian::Little
    };
    for pair in data.as_chunks::<8>().0 {
        let (a, b) = (
            order.u32(pair, 0).unwrap_or(0),
            order.u32(pair, 4).unwrap_or(0),
        );
        s0 = s0.wrapping_add(a).wrapping_add(s1);
        s1 = s1.wrapping_add(b).wrapping_add(s0);
    }
    (s0, s1)
}

fn read_exact_at(f: &mut SourceFile, at: u64, n: usize) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(at))?;
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Read the committed frames of `<db>-wal`, if it exists and is valid for this page size.
fn open_wal(fs: &Fs, db: &Path, page_size: u32) -> R<Option<Wal>> {
    let mut name = db.as_os_str().to_owned();
    name.push("-wal");
    let wp = PathBuf::from(name);
    let Ok(mut file) = fs.open(&wp) else {
        return Ok(None);
    };
    let len = file
        .metadata()
        .map_err(|e| SqliteError::Io(e.to_string()))?
        .len();
    if len < 32 {
        return Ok(None);
    }
    let hdr = read_exact_at(&mut file, 0, 32).map_err(|e| SqliteError::Io(e.to_string()))?;
    let magic = need(&hdr, 0, be_u32)?;
    let big = match magic {
        0x377f_0682 => false,
        0x377f_0683 => true,
        _ => return Ok(None),
    };
    let wal_page = need(&hdr, 8, be_u32)?;
    if wal_page != page_size {
        return Ok(None);
    }
    let (salt1, salt2) = (need(&hdr, 16, be_u32)?, need(&hdr, 20, be_u32)?);
    let (c0, c1) = wal_checksum(&hdr[..24], big, 0, 0);
    if (c0, c1) != (need(&hdr, 24, be_u32)?, need(&hdr, 28, be_u32)?) {
        return Ok(None); // an invalid header means the WAL is ignored, as SQLite does
    }
    let frame_len = 24 + u64::from(page_size);
    let mut committed = HashMap::new();
    let mut pending: Vec<(u32, u64)> = Vec::new();
    let mut db_pages = 0;
    let mut frames = 0;
    let (mut s0, mut s1) = (c0, c1);
    let mut at = 32u64;
    while at + frame_len <= len {
        let fh = read_exact_at(&mut file, at, 24).map_err(|e| SqliteError::Io(e.to_string()))?;
        if (need(&fh, 8, be_u32)?, need(&fh, 12, be_u32)?) != (salt1, salt2) {
            break;
        }
        let page = read_exact_at(&mut file, at + 24, page_size as usize)
            .map_err(|e| SqliteError::Io(e.to_string()))?;
        let (a, b) = wal_checksum(&fh[..8], big, s0, s1);
        let (a, b) = wal_checksum(&page, big, a, b);
        if (a, b) != (need(&fh, 16, be_u32)?, need(&fh, 20, be_u32)?) {
            break;
        }
        (s0, s1) = (a, b);
        let pgno = need(&fh, 0, be_u32)?;
        pending.push((pgno, at + 24));
        let commit = need(&fh, 4, be_u32)?;
        if commit != 0 {
            for (p, off) in pending.drain(..) {
                committed.insert(p, off);
            }
            db_pages = commit;
            frames += 1;
        }
        at += frame_len;
    }
    if committed.is_empty() {
        return Ok(None);
    }
    Ok(Some(Wal {
        file,
        pages: committed,
        db_pages,
        frames,
    }))
}

impl SqliteDb {
    /// Open `path` read-only (a `<path>-wal` next to it is applied, never modified).
    pub fn open(path: &Path) -> R<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// [`SqliteDb::open`] for `path` in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> R<Self> {
        let mut file = fs
            .open(path)
            .map_err(|e| SqliteError::Io(format!("{}: {e}", path.display())))?;
        let file_len = file
            .metadata()
            .map_err(|e| SqliteError::Io(e.to_string()))?
            .len();
        if file_len < 100 {
            return corrupt("shorter than the 100-byte database header");
        }
        let hdr = read_exact_at(&mut file, 0, 100).map_err(|e| SqliteError::Io(e.to_string()))?;
        if &hdr[..16] != b"SQLite format 3\0" {
            return corrupt("no `SQLite format 3` header");
        }
        let raw = need(&hdr, 16, be_u16)?;
        let page_size: u32 = if raw == 1 { 65_536 } else { u32::from(raw) };
        if !(512..=65_536).contains(&page_size) || !page_size.is_power_of_two() {
            return corrupt(format!("page size {page_size}"));
        }
        let reserved = u32::from(hdr[20]);
        let usable = page_size.saturating_sub(reserved);
        if usable < 480 {
            return corrupt("usable page size below 480 bytes");
        }
        let encoding = need(&hdr, 56, be_u32)?;
        if encoding > 1 {
            return Err(SqliteError::Unsupported(format!(
                "text encoding {encoding} (only UTF-8 is read)"
            )));
        }
        let wal = open_wal(fs, path, page_size)?;
        let file_pages = u32::try_from(file_len / u64::from(page_size)).unwrap_or(u32::MAX);
        let page_count = match &wal {
            Some(w) if w.db_pages > 0 => w.db_pages,
            _ => file_pages,
        };
        let mut db = SqliteDb {
            path: path.to_path_buf(),
            file,
            file_len,
            page_size,
            usable,
            page_count,
            wal,
            schema: Vec::new(),
        };
        // the header on page 1 may itself have been rewritten in the WAL
        let p1 = db.page(1)?;
        if need(&p1, 56, be_u32)? > 1 {
            return Err(SqliteError::Unsupported(
                "text encoding other than UTF-8".into(),
            ));
        }
        let rows = db.btree_rows(1)?;
        for (_, r) in rows {
            let text = |i: usize| match r.get(i) {
                Some(SqlValue::Text(t)) => t.clone(),
                _ => String::new(),
            };
            let rootpage = r.get(3).and_then(SqlValue::as_i64).unwrap_or(0);
            db.schema.push(SchemaEntry {
                kind: text(0),
                name: text(1),
                rootpage: u32::try_from(rootpage).unwrap_or(0),
                sql: text(4),
            });
        }
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of committed WAL frames that were applied (0 without a WAL).
    pub fn wal_frames(&self) -> u32 {
        self.wal.as_ref().map_or(0, |w| w.frames)
    }

    /// Names of the ordinary tables.
    pub fn tables(&self) -> Vec<String> {
        self.schema
            .iter()
            .filter(|e| e.kind == "table" && !e.name.starts_with("sqlite_"))
            .map(|e| e.name.clone())
            .collect()
    }

    pub fn has_table(&self, name: &str) -> bool {
        self.schema
            .iter()
            .any(|e| e.kind == "table" && e.name.eq_ignore_ascii_case(name))
    }

    fn page(&mut self, n: u32) -> R<Vec<u8>> {
        if n == 0 || n > self.page_count.max(1) {
            return corrupt(format!(
                "page {n} out of range (database has {})",
                self.page_count
            ));
        }
        let ps = self.page_size as usize;
        if let Some(w) = &mut self.wal
            && let Some(&off) = w.pages.get(&n)
        {
            return read_exact_at(&mut w.file, off, ps).map_err(|e| SqliteError::Io(e.to_string()));
        }
        let at = u64::from(n - 1) * u64::from(self.page_size);
        if at + u64::from(self.page_size) > self.file_len {
            return corrupt(format!("page {n} lies past the end of the database file"));
        }
        read_exact_at(&mut self.file, at, ps).map_err(|e| SqliteError::Io(e.to_string()))
    }

    /// Every record of the b-tree rooted at `root`, in key order: `(rowid, record)` for a table
    /// b-tree, `(None, record)` for an index b-tree (how `WITHOUT ROWID` tables are stored).
    fn btree_rows(&mut self, root: u32) -> R<Vec<(Option<i64>, Vec<SqlValue>)>> {
        enum Step {
            Visit(u32, usize),
            Emit(Vec<SqlValue>),
        }
        let mut out = Vec::new();
        let mut stack = vec![Step::Visit(root, 0)];
        let mut seen = HashSet::new();
        while let Some(step) = stack.pop() {
            let (pg, depth) = match step {
                Step::Emit(rec) => {
                    out.push((None, rec));
                    continue;
                }
                Step::Visit(pg, depth) => (pg, depth),
            };
            if depth > 64 || !seen.insert(pg) {
                return corrupt(format!("b-tree loop or excessive depth at page {pg}"));
            }
            let p = self.page(pg)?;
            let off = if pg == 1 { 100 } else { 0 };
            let kind = *p
                .get(off)
                .ok_or_else(|| SqliteError::Corrupt("empty page".into()))?;
            let ncells = usize::from(need(&p, off + 3, be_u16)?);
            match kind {
                0x05 => {
                    let right = need(&p, off + 8, be_u32)?;
                    stack.push(Step::Visit(right, depth + 1));
                    for i in (0..ncells).rev() {
                        let cp = usize::from(need(&p, off + 12 + 2 * i, be_u16)?);
                        stack.push(Step::Visit(need(&p, cp, be_u32)?, depth + 1));
                    }
                }
                0x0d => {
                    for i in 0..ncells {
                        let cp = usize::from(need(&p, off + 8 + 2 * i, be_u16)?);
                        let (plen, a) = varint(&p, cp)?;
                        let (rowid, b) = varint(&p, cp + a)?;
                        let payload = self.payload(&p, cp + a + b, plen, false)?;
                        out.push((Some(rowid.cast_signed()), decode_record(&payload)?));
                    }
                }
                0x02 => {
                    // in-order: child i, then the entry stored in cell i; the right-most child last
                    let right = need(&p, off + 8, be_u32)?;
                    stack.push(Step::Visit(right, depth + 1));
                    for i in (0..ncells).rev() {
                        let cp = usize::from(need(&p, off + 12 + 2 * i, be_u16)?);
                        let child = need(&p, cp, be_u32)?;
                        let (plen, a) = varint(&p, cp + 4)?;
                        let payload = self.payload(&p, cp + 4 + a, plen, true)?;
                        stack.push(Step::Emit(decode_record(&payload)?));
                        stack.push(Step::Visit(child, depth + 1));
                    }
                }
                0x0a => {
                    for i in 0..ncells {
                        let cp = usize::from(need(&p, off + 8 + 2 * i, be_u16)?);
                        let (plen, a) = varint(&p, cp)?;
                        let payload = self.payload(&p, cp + a, plen, true)?;
                        out.push((None, decode_record(&payload)?));
                    }
                }
                k => return corrupt(format!("page {pg} has b-tree type {k:#04x}")),
            }
        }
        Ok(out)
    }

    /// The payload of a cell starting at `at` (`index`: index b-tree limits), following
    /// overflow pages.
    /// Pages physically present: the database file's, plus the WAL's committed page images.
    fn physical_pages(&self) -> u64 {
        let wal = self.wal.as_ref().map_or(0, |w| w.pages.len() as u64);
        (self.file_len / u64::from(self.page_size.max(1))).saturating_add(wal)
    }

    fn payload(&mut self, page: &[u8], at: usize, plen: u64, index: bool) -> R<Vec<u8>> {
        let u = u64::from(self.usable);
        let x = if index {
            ((u - 12) * 64 / 255) - 23
        } else {
            u - 35
        };
        // The payload is stored in this page and a chain of overflow pages, so it cannot be
        // longer than every page the database and its WAL physically hold. A record header
        // may declare up to 2^64 bytes; refuse that before allocating.
        let physical = self.physical_pages();
        if plen > physical.saturating_add(1).saturating_mul(u) {
            return corrupt(format!(
                "cell payload of {plen} bytes is larger than the database ({physical} pages)"
            ));
        }
        let plen_us =
            usize::try_from(plen).map_err(|_| SqliteError::Corrupt("payload too large".into()))?;
        if plen <= x {
            return page
                .get(at..at.saturating_add(plen_us))
                .map(<[u8]>::to_vec)
                .ok_or_else(|| SqliteError::Corrupt("cell payload past page end".into()));
        }
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + ((plen - m) % (u - 4));
        let local = usize::try_from(if k <= x { k } else { m }).unwrap_or(0);
        let mut out = Vec::with_capacity(plen_us);
        out.extend_from_slice(
            page.get(at..at.saturating_add(local))
                .ok_or_else(|| SqliteError::Corrupt("cell payload past page end".into()))?,
        );
        let mut next = need(page, at + local, be_u32)?;
        let mut hops = 0u32;
        while out.len() < plen_us {
            if next == 0 || u64::from(hops) > physical {
                return corrupt("overflow chain ends early or loops");
            }
            let op = self.page(next)?;
            next = need(&op, 0, be_u32)?;
            let take = (plen_us - out.len()).min(self.usable as usize - 4);
            out.extend_from_slice(
                op.get(4..4 + take)
                    .ok_or_else(|| SqliteError::Corrupt("overflow page too short".into()))?,
            );
            hops += 1;
        }
        Ok(out)
    }

    /// Number of rows of a table, counted from its b-tree pages without decoding records.
    pub fn count_rows(&mut self, name: &str) -> R<u64> {
        let root = self
            .schema
            .iter()
            .find(|e| e.kind == "table" && e.name.eq_ignore_ascii_case(name))
            .map(|e| e.rootpage)
            .ok_or_else(|| SqliteError::Corrupt(format!("no table {name}")))?;
        if root == 0 {
            return Ok(0);
        }
        let mut n = 0u64;
        let mut stack = vec![(root, 0usize)];
        let mut seen = HashSet::new();
        while let Some((pg, depth)) = stack.pop() {
            if depth > 64 || !seen.insert(pg) {
                return corrupt(format!("b-tree loop or excessive depth at page {pg}"));
            }
            let p = self.page(pg)?;
            let off = if pg == 1 { 100 } else { 0 };
            let kind = *p
                .get(off)
                .ok_or_else(|| SqliteError::Corrupt("empty page".into()))?;
            let ncells = usize::from(need(&p, off + 3, be_u16)?);
            match kind {
                0x0d | 0x0a => n += ncells as u64,
                0x05 | 0x02 => {
                    if kind == 0x02 {
                        n += ncells as u64;
                    }
                    stack.push((need(&p, off + 8, be_u32)?, depth + 1));
                    for i in 0..ncells {
                        let cp = usize::from(need(&p, off + 12 + 2 * i, be_u16)?);
                        stack.push((need(&p, cp, be_u32)?, depth + 1));
                    }
                }
                k => return corrupt(format!("page {pg} has b-tree type {k:#04x}")),
            }
        }
        Ok(n)
    }

    /// Read a whole table.
    pub fn read_table(&mut self, name: &str) -> R<SqlTable> {
        let entry = self
            .schema
            .iter()
            .find(|e| e.kind == "table" && e.name.eq_ignore_ascii_case(name))
            .cloned()
            .ok_or_else(|| SqliteError::Corrupt(format!("no table {name}")))?;
        if entry.rootpage == 0 {
            return Err(SqliteError::Unsupported(format!("virtual table {name}")));
        }
        let cols = parse_columns(&entry.sql)?;
        let (columns, rowid_col) = (cols.names.clone(), cols.rowid);
        let rows = self.btree_rows(entry.rootpage)?;
        let mut out = SqlTable {
            name: entry.name.clone(),
            columns,
            rows: Vec::with_capacity(rows.len()),
        };
        for (rowid, rec) in rows {
            let mut rec = if cols.without_rowid {
                // stored as the PRIMARY KEY columns, then the others in declaration order
                let mut order: Vec<usize> = cols.pk.clone();
                order.extend((0..cols.names.len()).filter(|i| !cols.pk.contains(i)));
                let mut v = vec![SqlValue::Null; cols.names.len()];
                for (k, val) in rec.into_iter().enumerate() {
                    if let Some(&c) = order.get(k) {
                        v[c] = val;
                    }
                }
                v
            } else {
                rec
            };
            rec.resize(out.columns.len().max(rec.len()), SqlValue::Null);
            rec.truncate(out.columns.len());
            if let (Some(c), Some(rowid)) = (rowid_col, rowid)
                && let Some(v) = rec.get_mut(c)
                && *v == SqlValue::Null
            {
                *v = SqlValue::Integer(rowid);
            }
            out.rows.push(rec);
        }
        Ok(out)
    }
}

/// Decode one record (header of serial types, then values).
fn decode_record(p: &[u8]) -> R<Vec<SqlValue>> {
    let (hlen, n) = varint(p, 0)?;
    let hlen = usize::try_from(hlen).map_err(|_| SqliteError::Corrupt("record header".into()))?;
    if hlen > p.len() || hlen < n {
        return corrupt("record header longer than the record");
    }
    let mut types = Vec::new();
    let mut at = n;
    while at < hlen {
        let (t, k) = varint(p, at)?;
        types.push(t);
        at += k;
    }
    let mut body = hlen;
    let mut out = Vec::with_capacity(types.len());
    let int = |b: &[u8]| -> i64 {
        let mut v: i64 = if b.first().is_some_and(|x| x & 0x80 != 0) {
            -1
        } else {
            0
        };
        for &x in b {
            v = (v << 8) | i64::from(x);
        }
        v
    };
    for t in types {
        let len: usize = match t {
            0 | 8 | 9 => 0,
            1 => 1,
            2 => 2,
            3 => 3,
            4 => 4,
            5 => 6,
            6 | 7 => 8,
            10 | 11 => return corrupt("reserved serial type"),
            n => usize::try_from((n - 12) / 2)
                .map_err(|_| SqliteError::Corrupt("value too long".into()))?,
        };
        let b = p
            .get(body..body + len)
            .ok_or_else(|| SqliteError::Corrupt("record value past the end".into()))?;
        body += len;
        out.push(match t {
            0 => SqlValue::Null,
            1..=6 => SqlValue::Integer(int(b)),
            7 => SqlValue::Real(be_f64(b, 0).unwrap_or(0.0)),
            8 => SqlValue::Integer(0),
            9 => SqlValue::Integer(1),
            n if n % 2 == 0 => SqlValue::Blob(b.to_vec()),
            _ => SqlValue::Text(String::from_utf8_lossy(b).into_owned()),
        });
    }
    Ok(out)
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 2
        && matches!(
            (b[0], b[b.len() - 1]),
            (b'"', b'"') | (b'`', b'`') | (b'[', b']') | (b'\'', b'\'')
        )
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// What a `CREATE TABLE` statement says about the stored records.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Columns {
    names: Vec<String>,
    /// The rowid-alias column (`INTEGER PRIMARY KEY`) of a rowid table.
    rowid: Option<usize>,
    without_rowid: bool,
    /// PRIMARY KEY columns in key order.
    pk: Vec<usize>,
}

/// Parse the column list of a `CREATE TABLE` statement.
fn parse_columns(sql: &str) -> R<Columns> {
    let open = sql
        .find('(')
        .ok_or_else(|| SqliteError::Corrupt(format!("CREATE TABLE without columns: {sql}")))?;
    let close = sql
        .rfind(')')
        .ok_or_else(|| SqliteError::Corrupt("unbalanced CREATE TABLE".into()))?;
    if close <= open {
        return corrupt("unbalanced CREATE TABLE");
    }
    let tail = sql[close + 1..].to_ascii_uppercase();
    let without_rowid = tail.contains("WITHOUT") && tail.contains("ROWID");
    // split on top-level commas
    let body = &sql[open + 1..close];
    let mut parts = Vec::new();
    let (mut depth, mut start, mut quote) = (0i32, 0usize, None::<char>);
    for (i, c) in body.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'' | '`') => quote = Some(c),
            (None, '[') => quote = Some(']'),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, ',') if depth == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    let mut cols = Vec::new();
    let mut rowid = None;
    let mut table_pk: Vec<String> = Vec::new();
    let mut column_pk: Option<usize> = None;
    let mut types: Vec<String> = Vec::new();
    for part in parts {
        let p = part.trim();
        let up = p.to_ascii_uppercase();
        if [
            "PRIMARY KEY",
            "UNIQUE",
            "CHECK",
            "FOREIGN KEY",
            "CONSTRAINT",
        ]
        .iter()
        .any(|k| up.starts_with(k))
        {
            if up.starts_with("PRIMARY KEY")
                && let (Some(a), Some(b)) = (p.find('('), p.find(')'))
                && a < b
            {
                table_pk = p[a + 1..b]
                    .split(',')
                    .map(|c| unquote(c.split_whitespace().next().unwrap_or("")))
                    .collect();
            }
            continue;
        }
        // name: quoted or bare first token
        let (name, rest) = if let Some(q) = p
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '`' | '[' | '\''))
        {
            let endq = if q == '[' { ']' } else { q };
            match p[1..].find(endq) {
                Some(e) => (p[1..=e].to_string(), &p[e + 2..]),
                None => return corrupt(format!("unterminated column name in {p}")),
            }
        } else {
            let e = p.find(char::is_whitespace).unwrap_or(p.len());
            (p[..e].to_string(), &p[e..])
        };
        let rest_up = rest.trim().to_ascii_uppercase();
        let ty = rest_up.split_whitespace().next().unwrap_or("").to_string();
        if rest_up.contains("PRIMARY KEY") {
            column_pk = Some(cols.len());
            if ty == "INTEGER" && !rest_up.contains("DESC") {
                rowid = Some(cols.len());
            }
        }
        types.push(ty);
        cols.push(name);
    }
    let pk: Vec<usize> = if table_pk.is_empty() {
        column_pk.into_iter().collect()
    } else {
        table_pk
            .iter()
            .filter_map(|n| cols.iter().position(|c| c.eq_ignore_ascii_case(n)))
            .collect()
    };
    if rowid.is_none() && pk.len() == 1 && types[pk[0]] == "INTEGER" {
        rowid = Some(pk[0]);
    }
    if without_rowid {
        rowid = None;
    }
    Ok(Columns {
        names: cols,
        rowid,
        without_rowid,
        pk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints() {
        assert_eq!(varint(&[0x05], 0).unwrap(), (5, 1));
        assert_eq!(varint(&[0x81, 0x00], 0).unwrap(), (128, 2));
        assert_eq!(varint(&[0xff; 9], 0).unwrap().1, 9);
        assert!(varint(&[0x81], 0).is_err());
    }

    #[test]
    fn columns() {
        let c = parse_columns(
            "CREATE TABLE Frames (Id INTEGER PRIMARY KEY, Time REAL NOT NULL, \"Polarity\" CHAR(1) CHECK (Polarity IN ('+', '-')), FOREIGN KEY (Id) REFERENCES X(Id))",
        )
        .unwrap();
        assert_eq!(c.names, vec!["Id", "Time", "Polarity"]);
        assert_eq!(c.rowid, Some(0));
        let c = parse_columns("CREATE TABLE G (Key TEXT, Value TEXT, PRIMARY KEY (Key))").unwrap();
        assert_eq!(c.names, vec!["Key", "Value"]);
        assert_eq!(c.rowid, None);
        let c = parse_columns("CREATE TABLE W (G INTEGER NOT NULL, B INTEGER, M REAL,\n PRIMARY KEY(G, B)) WITHOUT ROWID").unwrap();
        assert!(c.without_rowid);
        assert_eq!(c.pk, vec![0, 1]);
        assert_eq!(c.rowid, None);
    }

    #[test]
    fn record() {
        // header: len 4, types: int8 (1), text len 2 (17), real (7)
        let mut p = vec![4u8, 1, 17, 7, 0xfe, b'h', b'i'];
        p.extend_from_slice(&1.5f64.to_be_bytes());
        let v = decode_record(&p).unwrap();
        assert_eq!(
            v,
            vec![
                SqlValue::Integer(-2),
                SqlValue::Text("hi".into()),
                SqlValue::Real(1.5)
            ]
        );
        assert!(decode_record(&[9, 1]).is_err());
    }
}
