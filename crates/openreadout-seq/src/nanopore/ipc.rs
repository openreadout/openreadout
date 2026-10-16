//! A small, bounds-checked reader of Apache Arrow IPC files for the column types POD5 tables
//! use: integers, floats, booleans, timestamps, fixed-size and variable-size binary and strings
//! (32- and 64-bit offsets), lists and large lists of integers, string-to-string maps and
//! dictionary-encoded strings.
//!
//! The FlatBuffer metadata (footer, schema, messages) is read through the verified accessors
//! of `arrow-ipc`; the buffers themselves are interpreted here, every offset and length checked
//! against the body, so a damaged file gives an error rather than a panic. Compressed IPC
//! bodies are refused (POD5 writers compress the signal column themselves, not the body).

use std::collections::HashMap;

use arrow_ipc::{Type, root_as_footer, root_as_message};

/// An error message (the caller adds the format and the table).
pub type IpcResult<T> = std::result::Result<T, String>;

/// Largest IPC file read into memory as a whole (reads and run-info tables).
pub const MAX_TABLE_BYTES: u64 = 1 << 30;

/// The type of a column, as far as this reader decodes it.
#[derive(Debug, Clone, PartialEq)]
pub enum ColType {
    /// Signed or unsigned integer of `bits` bits (also timestamps, dates and durations).
    Int { bits: u8, signed: bool },
    /// 16-, 32- or 64-bit float.
    Float { bits: u8 },
    /// Boolean (bit-packed).
    Bool,
    /// Fixed-size binary of `width` bytes.
    FixedBinary { width: usize },
    /// Variable-size binary or UTF-8 (`large`: 64-bit offsets).
    Binary { large: bool },
    /// List of a child type (`large`: 64-bit offsets).
    List { large: bool, item: Box<ColType> },
    /// Map of entries (a struct of key and value).
    Map { key: Box<ColType>, value: Box<ColType> },
    /// Struct of children.
    Struct(Vec<ColType>),
    /// Dictionary-encoded values: integer indices into dictionary `id`.
    Dict { id: i64, index_bits: u8, value: Box<ColType> },
    /// Anything else (decoded as nothing; its buffers are skipped).
    Other(u8),
}

/// A column of the schema.
#[derive(Debug, Clone)]
pub struct Field {
    /// Column name.
    pub name: String,
    /// Its type.
    pub ty: ColType,
    /// `ARROW:extension:name` of the field, when it has one.
    pub extension: Option<String>,
}

/// A decoded column.
#[derive(Debug, Clone, PartialEq)]
pub enum Column {
    /// Integers (unsigned values above `i64::MAX` saturate).
    Int(Vec<Option<i64>>),
    /// Floats.
    Float(Vec<Option<f64>>),
    /// Booleans.
    Bool(Vec<Option<bool>>),
    /// Bytes (binary, fixed binary and strings).
    Bytes(Vec<Option<Vec<u8>>>),
    /// Lists of integers.
    IntList(Vec<Option<Vec<i64>>>),
    /// Maps of strings.
    Map(Vec<Option<Vec<(String, String)>>>),
    /// Dictionary-encoded strings, resolved.
    Text(Vec<Option<String>>),
    /// A column this reader does not decode.
    Skipped,
}

/// One record batch of a file: its location and row count.
#[derive(Debug, Clone, Copy)]
pub struct BatchBlock {
    /// Offset of the message in the IPC file.
    pub offset: u64,
    /// Metadata length (with prefix and padding).
    pub meta_len: u64,
    /// Body length.
    pub body_len: u64,
    /// Rows.
    pub rows: u64,
}

/// An Arrow IPC file held in memory (or its footer only, for large tables read batch by batch).
#[derive(Debug, Clone)]
pub struct IpcFile {
    /// Columns.
    pub fields: Vec<Field>,
    /// Schema-level custom metadata.
    pub metadata: Vec<(String, String)>,
    /// Record batches in order.
    pub batches: Vec<BatchBlock>,
    /// Dictionary blocks.
    pub dict_blocks: Vec<BatchBlock>,
}

fn int_bits(i: arrow_ipc::Int<'_>) -> u8 {
    match i.bitWidth() {
        8 => 8,
        16 => 16,
        32 => 32,
        _ => 64,
    }
}

fn field_type(f: arrow_ipc::Field<'_>, depth: usize) -> ColType {
    if depth > 8 {
        return ColType::Other(255);
    }
    let children: Vec<arrow_ipc::Field<'_>> = f
        .children()
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    let base = match f.type_type() {
        Type::Int => f.type_as_int().map_or(ColType::Other(2), |i| ColType::Int {
            bits: int_bits(i),
            signed: i.is_signed(),
        }),
        Type::FloatingPoint => {
            let bits = match f.type_as_floating_point().map(|p| p.precision().0) {
                Some(0) => 16,
                Some(1) => 32,
                _ => 64,
            };
            ColType::Float { bits }
        }
        Type::Bool => ColType::Bool,
        Type::Timestamp | Type::Date | Type::Duration | Type::Time => {
            let bits = if f.type_type() == Type::Date {
                // Date32 (days) or Date64 (ms)
                match f.type_as_date().map(|d| d.unit().0) {
                    Some(0) => 32,
                    _ => 64,
                }
            } else if f.type_type() == Type::Time {
                match f.type_as_time().map(|t| t.bitWidth()) {
                    Some(32) => 32,
                    _ => 64,
                }
            } else {
                64
            };
            ColType::Int { bits, signed: true }
        }
        Type::FixedSizeBinary => ColType::FixedBinary {
            width: f
                .type_as_fixed_size_binary()
                .map_or(0, |b| usize::try_from(b.byteWidth()).unwrap_or(0)),
        },
        Type::Binary | Type::Utf8 => ColType::Binary { large: false },
        Type::LargeBinary | Type::LargeUtf8 => ColType::Binary { large: true },
        Type::List | Type::LargeList => ColType::List {
            large: f.type_type() == Type::LargeList,
            item: Box::new(
                children
                    .first()
                    .map_or(ColType::Other(0), |c| field_type(*c, depth + 1)),
            ),
        },
        Type::Map => {
            let entries = children.first().map(|c| field_type(*c, depth + 1));
            match entries {
                Some(ColType::Struct(kv)) if kv.len() == 2 => ColType::Map {
                    key: Box::new(kv[0].clone()),
                    value: Box::new(kv[1].clone()),
                },
                _ => ColType::Other(17),
            }
        }
        Type::Struct_ => ColType::Struct(children.iter().map(|c| field_type(*c, depth + 1)).collect()),
        t => ColType::Other(t.0),
    };
    if let Some(d) = f.dictionary() {
        return ColType::Dict {
            id: d.id(),
            index_bits: d.indexType().map_or(32, int_bits),
            value: Box::new(base),
        };
    }
    base
}

fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at.checked_add(4)?)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parse the footer and schema of an IPC file whose bytes are `file` (or whose first 8 bytes
/// and last `file.len()` bytes are given: only the tail is needed when `tail_offset` > 0, the
/// position of `file[0]` in the IPC file).
pub fn parse_footer(file: &[u8], tail_offset: u64, total_len: u64) -> IpcResult<IpcFile> {
    let n = file.len();
    if n < 10 || &file[n - 6..] != b"ARROW1" {
        return Err("embedded table is not an Arrow IPC file (no ARROW1 trailer)".into());
    }
    let flen = le_u32(file, n - 10).ok_or("truncated Arrow footer")? as usize;
    let fstart = (n - 10)
        .checked_sub(flen)
        .ok_or("Arrow footer length exceeds the table")?;
    let footer = root_as_footer(&file[fstart..n - 10])
        .map_err(|e| format!("Arrow footer is damaged: {e}"))?;
    let schema = footer.schema().ok_or("Arrow footer has no schema")?;
    let mut fields = Vec::new();
    if let Some(fs) = schema.fields() {
        for f in fs.iter() {
            let mut extension = None;
            if let Some(md) = f.custom_metadata() {
                for kv in md.iter() {
                    if kv.key() == Some("ARROW:extension:name") {
                        extension = kv.value().map(str::to_string);
                    }
                }
            }
            fields.push(Field {
                name: f.name().unwrap_or_default().to_string(),
                ty: field_type(f, 0),
                extension,
            });
        }
    }
    let mut metadata = Vec::new();
    if let Some(md) = schema.custom_metadata() {
        for kv in md.iter() {
            metadata.push((
                kv.key().unwrap_or_default().to_string(),
                kv.value().unwrap_or_default().to_string(),
            ));
        }
    }
    let blocks = |v: Vec<(i64, i32, i64)>| -> IpcResult<Vec<BatchBlock>> {
        let mut out = Vec::new();
        for (o, m, l) in v {
            if o < 0 || m < 0 || l < 0 {
                return Err("negative Arrow block offset or length".into());
            }
            let (o, m, l) = (o as u64, m as u64, l as u64);
            if o.saturating_add(m).saturating_add(l) > total_len {
                return Err("Arrow record batch runs past the end of the table".into());
            }
            out.push(BatchBlock {
                offset: o,
                meta_len: m,
                body_len: l,
                rows: 0,
            });
        }
        Ok(out)
    };
    let _ = tail_offset;
    let batches = footer
        .recordBatches()
        .map(|v| {
            v.iter()
                .map(|b| (b.offset(), b.metaDataLength(), b.bodyLength()))
                .collect()
        })
        .unwrap_or_default();
    let dicts = footer
        .dictionaries()
        .map(|v| {
            v.iter()
                .map(|b| (b.offset(), b.metaDataLength(), b.bodyLength()))
                .collect()
        })
        .unwrap_or_default();
    Ok(IpcFile {
        fields,
        metadata,
        batches: blocks(batches)?,
        dict_blocks: blocks(dicts)?,
    })
}

/// A record batch message: rows, field nodes `(length, nulls)` and buffers `(offset, length)`
/// relative to the body, plus (for a dictionary batch) the dictionary id.
#[derive(Debug, Clone)]
pub struct BatchMeta {
    /// Rows.
    pub rows: u64,
    /// Field nodes.
    pub nodes: Vec<(u64, u64)>,
    /// Buffers.
    pub buffers: Vec<(u64, u64)>,
    /// Dictionary id, for a dictionary batch.
    pub dict_id: Option<i64>,
}

/// Parse the message metadata at the start of `meta` (the `meta_len` bytes of a block).
pub fn parse_message(meta: &[u8]) -> IpcResult<BatchMeta> {
    let mut at = 0usize;
    if le_u32(meta, 0) == Some(0xFFFF_FFFF) {
        at = 4;
    }
    let len = le_u32(meta, at).ok_or("truncated Arrow message")? as usize;
    let body = meta
        .get(at + 4..(at + 4).checked_add(len).ok_or("bad Arrow message length")?)
        .ok_or("Arrow message runs past its block")?;
    let msg = root_as_message(body).map_err(|e| format!("Arrow message is damaged: {e}"))?;
    let (rb, dict_id) = if let Some(rb) = msg.header_as_record_batch() {
        (rb, None)
    } else if let Some(db) = msg.header_as_dictionary_batch() {
        if db.isDelta() {
            return Err("delta dictionaries are not read".into());
        }
        (db.data().ok_or("dictionary batch without data")?, Some(db.id()))
    } else {
        return Err("Arrow block is not a record batch".into());
    };
    if rb.compression().is_some() {
        return Err("compressed Arrow bodies are not read".into());
    }
    let rows = u64::try_from(rb.length()).map_err(|_| "negative row count")?;
    let nodes = rb
        .nodes()
        .map(|v| {
            v.iter()
                .map(|n| (n.length().max(0) as u64, n.null_count().max(0) as u64))
                .collect()
        })
        .unwrap_or_default();
    let buffers = rb
        .buffers()
        .map(|v| {
            v.iter()
                .map(|b| (b.offset().max(0) as u64, b.length().max(0) as u64))
                .collect()
        })
        .unwrap_or_default();
    Ok(BatchMeta {
        rows,
        nodes,
        buffers,
        dict_id,
    })
}

/// Decodes the columns of one batch from its body.
#[derive(Debug)]
pub struct BatchReader<'a> {
    body: &'a [u8],
    meta: &'a BatchMeta,
    node: usize,
    buf: usize,
    dicts: &'a HashMap<i64, Vec<Option<String>>>,
}

impl<'a> BatchReader<'a> {
    /// A reader over `body` described by `meta`; `dicts` resolves dictionary columns.
    pub fn new(
        body: &'a [u8],
        meta: &'a BatchMeta,
        dicts: &'a HashMap<i64, Vec<Option<String>>>,
    ) -> Self {
        BatchReader {
            body,
            meta,
            node: 0,
            buf: 0,
            dicts,
        }
    }

    fn next_node(&mut self) -> IpcResult<(usize, u64)> {
        let n = *self
            .meta
            .nodes
            .get(self.node)
            .ok_or("Arrow batch has fewer field nodes than its schema")?;
        self.node += 1;
        let len = usize::try_from(n.0).map_err(|_| "Arrow field too long")?;
        if n.0 > self.body.len() as u64 * 8 + 1 && n.0 > (1 << 32) {
            return Err("Arrow field length larger than its batch".into());
        }
        Ok((len, n.1))
    }

    fn next_buf(&mut self) -> IpcResult<&'a [u8]> {
        let (o, l) = *self
            .meta
            .buffers
            .get(self.buf)
            .ok_or("Arrow batch has fewer buffers than its schema")?;
        self.buf += 1;
        let end = o.checked_add(l).ok_or("Arrow buffer overflows")?;
        if end > self.body.len() as u64 {
            return Err("Arrow buffer runs past the batch body".into());
        }
        Ok(&self.body[o as usize..end as usize])
    }

    fn valid(bits: &[u8], nulls: u64, i: usize) -> bool {
        if nulls == 0 || bits.is_empty() {
            return true;
        }
        bits.get(i / 8).is_some_and(|b| b >> (i % 8) & 1 == 1)
    }

    fn ints(data: &[u8], bits: u8, signed: bool, len: usize) -> IpcResult<Vec<i64>> {
        let w = (bits / 8) as usize;
        if data.len() < len.checked_mul(w).ok_or("overflow")? {
            return Err("Arrow integer buffer shorter than its column".into());
        }
        Ok(data
            .chunks_exact(w)
            .take(len)
            .map(|c| match (w, signed) {
                (1, true) => i64::from(c[0] as i8),
                (1, false) => i64::from(c[0]),
                (2, true) => i64::from(i16::from_le_bytes([c[0], c[1]])),
                (2, false) => i64::from(u16::from_le_bytes([c[0], c[1]])),
                (4, true) => i64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])),
                (4, false) => i64::from(u32::from_le_bytes([c[0], c[1], c[2], c[3]])),
                (_, true) => i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]),
                (_, false) => {
                    let u = u64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]);
                    i64::try_from(u).unwrap_or(i64::MAX)
                }
            })
            .collect())
    }

    fn offsets(data: &[u8], large: bool, len: usize) -> IpcResult<Vec<u64>> {
        let w = if large { 8 } else { 4 };
        let need = len.checked_add(1).and_then(|n| n.checked_mul(w)).ok_or("overflow")?;
        if len == 0 && data.is_empty() {
            return Ok(vec![0]);
        }
        if data.len() < need {
            return Err("Arrow offsets buffer shorter than its column".into());
        }
        let v: Vec<u64> = data
            .chunks_exact(w)
            .take(len + 1)
            .map(|c| {
                if large {
                    i64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]).max(0) as u64
                } else {
                    i64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])).max(0) as u64
                }
            })
            .collect();
        if v.windows(2).any(|p| p[1] < p[0]) {
            return Err("Arrow offsets decrease".into());
        }
        Ok(v)
    }

    /// Decode the next column of type `ty` (columns must be read in schema order).
    pub fn column(&mut self, ty: &ColType) -> IpcResult<Column> {
        let (len, nulls) = self.next_node()?;
        match ty {
            ColType::Int { bits, signed } => {
                let v = self.next_buf()?;
                let d = self.next_buf()?;
                let x = Self::ints(d, *bits, *signed, len)?;
                Ok(Column::Int(
                    x.into_iter()
                        .enumerate()
                        .map(|(i, x)| Self::valid(v, nulls, i).then_some(x))
                        .collect(),
                ))
            }
            ColType::Float { bits } => {
                let v = self.next_buf()?;
                let d = self.next_buf()?;
                let w = (*bits / 8) as usize;
                if d.len() < len.checked_mul(w).ok_or("overflow")? {
                    return Err("Arrow float buffer shorter than its column".into());
                }
                Ok(Column::Float(
                    d.chunks_exact(w)
                        .take(len)
                        .enumerate()
                        .map(|(i, c)| {
                            Self::valid(v, nulls, i).then(|| match w {
                                4 => f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
                                8 => f64::from_le_bytes([
                                    c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7],
                                ]),
                                _ => f64::NAN,
                            })
                        })
                        .collect(),
                ))
            }
            ColType::Bool => {
                let v = self.next_buf()?;
                let d = self.next_buf()?;
                if d.len() * 8 < len {
                    return Err("Arrow boolean buffer shorter than its column".into());
                }
                Ok(Column::Bool(
                    (0..len)
                        .map(|i| Self::valid(v, nulls, i).then(|| d[i / 8] >> (i % 8) & 1 == 1))
                        .collect(),
                ))
            }
            ColType::FixedBinary { width } => {
                let v = self.next_buf()?;
                let d = self.next_buf()?;
                if *width == 0 || d.len() < len.checked_mul(*width).ok_or("overflow")? {
                    return Err("Arrow fixed-size binary buffer shorter than its column".into());
                }
                Ok(Column::Bytes(
                    d.chunks_exact(*width)
                        .take(len)
                        .enumerate()
                        .map(|(i, c)| Self::valid(v, nulls, i).then(|| c.to_vec()))
                        .collect(),
                ))
            }
            ColType::Binary { large } => {
                let v = self.next_buf()?;
                let o = Self::offsets(self.next_buf()?, *large, len)?;
                let d = self.next_buf()?;
                let mut out = Vec::with_capacity(len);
                for i in 0..len {
                    let (a, b) = (o[i] as usize, o[i + 1] as usize);
                    let s = d.get(a..b).ok_or("Arrow binary value runs past its buffer")?;
                    out.push(Self::valid(v, nulls, i).then(|| s.to_vec()));
                }
                Ok(Column::Bytes(out))
            }
            ColType::List { large, item } => {
                let v = self.next_buf()?;
                let o = Self::offsets(self.next_buf()?, *large, len)?;
                let child = self.column(item)?;
                let Column::Int(items) = child else {
                    return Ok(Column::Skipped);
                };
                let mut out = Vec::with_capacity(len);
                for i in 0..len {
                    let (a, b) = (o[i] as usize, o[i + 1] as usize);
                    let s = items.get(a..b).ok_or("Arrow list runs past its values")?;
                    out.push(
                        Self::valid(v, nulls, i).then(|| s.iter().map(|x| x.unwrap_or(0)).collect()),
                    );
                }
                Ok(Column::IntList(out))
            }
            ColType::Map { key, value } => {
                let v = self.next_buf()?;
                let o = Self::offsets(self.next_buf()?, false, len)?;
                // the entries struct: a node and a validity buffer, then key and value
                let _ = self.next_node()?;
                let _ = self.next_buf()?;
                let k = self.column(key)?;
                let val = self.column(value)?;
                let (Column::Bytes(k), Column::Bytes(val)) = (k, val) else {
                    return Ok(Column::Skipped);
                };
                let text = |b: &Option<Vec<u8>>| {
                    b.as_ref()
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default()
                };
                let mut out = Vec::with_capacity(len);
                for i in 0..len {
                    let (a, b) = (o[i] as usize, o[i + 1] as usize);
                    if b > k.len() || b > val.len() || a > b {
                        return Err("Arrow map runs past its entries".into());
                    }
                    out.push(Self::valid(v, nulls, i).then(|| {
                        (a..b).map(|j| (text(&k[j]), text(&val[j]))).collect()
                    }));
                }
                Ok(Column::Map(out))
            }
            ColType::Struct(children) => {
                let _ = self.next_buf()?;
                for c in children {
                    self.column(c)?;
                }
                Ok(Column::Skipped)
            }
            ColType::Dict { id, index_bits, .. } => {
                let v = self.next_buf()?;
                let d = self.next_buf()?;
                let idx = Self::ints(d, *index_bits, true, len)?;
                let empty = Vec::new();
                let dict = self.dicts.get(id).unwrap_or(&empty);
                Ok(Column::Text(
                    idx.into_iter()
                        .enumerate()
                        .map(|(i, k)| {
                            if !Self::valid(v, nulls, i) {
                                return None;
                            }
                            usize::try_from(k)
                                .ok()
                                .and_then(|k| dict.get(k).cloned().flatten())
                        })
                        .collect(),
                ))
            }
            ColType::Other(t) => Err(format!("Arrow column type {t} is not read")),
        }
    }
}

/// Read every batch (and dictionary) of a table held in memory as `file`; returns each
/// column's values over all batches, in schema order.
pub fn read_all(file: &[u8], ipc: &mut IpcFile) -> IpcResult<Vec<Column>> {
    let mut dicts: HashMap<i64, Vec<Option<String>>> = HashMap::new();
    for b in ipc.dict_blocks.clone() {
        let (meta, body) = block_bytes(file, &b)?;
        let m = parse_message(meta)?;
        let Some(id) = m.dict_id else { continue };
        let value_ty = ipc
            .fields
            .iter()
            .find_map(|f| match &f.ty {
                ColType::Dict { id: i, value, .. } if *i == id => Some((**value).clone()),
                _ => None,
            })
            .unwrap_or(ColType::Binary { large: false });
        let mut r = BatchReader::new(body, &m, &dicts);
        let col = r.column(&value_ty)?;
        if let Column::Bytes(v) = col {
            dicts.insert(
                id,
                v.into_iter()
                    .map(|b| b.map(|b| String::from_utf8_lossy(&b).into_owned()))
                    .collect(),
            );
        }
    }
    let mut cols: Vec<Column> = Vec::new();
    for bi in 0..ipc.batches.len() {
        let b = ipc.batches[bi];
        let (meta, body) = block_bytes(file, &b)?;
        let m = parse_message(meta)?;
        ipc.batches[bi].rows = m.rows;
        let mut r = BatchReader::new(body, &m, &dicts);
        for (k, f) in ipc.fields.iter().enumerate() {
            let c = r.column(&f.ty).unwrap_or(Column::Skipped);
            if cols.len() <= k {
                cols.push(c);
            } else {
                append(&mut cols[k], c);
            }
        }
    }
    if cols.len() < ipc.fields.len() {
        cols.resize(ipc.fields.len(), Column::Skipped);
    }
    Ok(cols)
}

fn append(a: &mut Column, b: Column) {
    match (a, b) {
        (Column::Int(x), Column::Int(y)) => x.extend(y),
        (Column::Float(x), Column::Float(y)) => x.extend(y),
        (Column::Bool(x), Column::Bool(y)) => x.extend(y),
        (Column::Bytes(x), Column::Bytes(y)) => x.extend(y),
        (Column::IntList(x), Column::IntList(y)) => x.extend(y),
        (Column::Map(x), Column::Map(y)) => x.extend(y),
        (Column::Text(x), Column::Text(y)) => x.extend(y),
        (a, _) => *a = Column::Skipped,
    }
}

/// The metadata and body bytes of a block of `file`.
pub fn block_bytes<'a>(file: &'a [u8], b: &BatchBlock) -> IpcResult<(&'a [u8], &'a [u8])> {
    let o = usize::try_from(b.offset).map_err(|_| "offset")?;
    let m = usize::try_from(b.meta_len).map_err(|_| "length")?;
    let l = usize::try_from(b.body_len).map_err(|_| "length")?;
    let meta = file
        .get(o..o.checked_add(m).ok_or("overflow")?)
        .ok_or("Arrow block runs past the table")?;
    let body = file
        .get(o + m..(o + m).checked_add(l).ok_or("overflow")?)
        .ok_or("Arrow block runs past the table")?;
    Ok((meta, body))
}

/// The index of field `name`.
pub fn field_index(ipc: &IpcFile, name: &str) -> Option<usize> {
    ipc.fields.iter().position(|f| f.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_is_an_error() {
        assert!(parse_footer(b"", 0, 0).is_err());
        assert!(parse_footer(b"xxxxxxxxxxARROW1", 0, 16).is_err());
        let mut b = vec![0u8; 64];
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(b"ARROW1");
        assert!(parse_footer(&b, 0, b.len() as u64).is_err());
        assert!(parse_message(&[0xff, 0xff, 0xff, 0xff, 9, 0, 0, 0, 1]).is_err());
    }
}
