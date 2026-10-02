//! A minimal reader for the netCDF classic format (CDF-1) and its 64-bit-offset variant (CDF-2):
//! header (dimensions, attributes, variables) and variable data, fixed-size and record
//! variables. Written from the public Unidata "NetCDF Classic Format Specification"; see
//! `docs/formats/andi-chrom.md`. All numbers in the file are big-endian.

use std::path::Path;

use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use openreadout_core::bytes::read_range;

/// Element type of a variable or attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NcType {
    Byte,
    Char,
    Short,
    Int,
    Float,
    Double,
}

impl NcType {
    fn from_code(c: u32) -> Option<NcType> {
        Some(match c {
            1 => NcType::Byte,
            2 => NcType::Char,
            3 => NcType::Short,
            4 => NcType::Int,
            5 => NcType::Float,
            6 => NcType::Double,
            _ => return None,
        })
    }
    /// Bytes per element.
    pub fn size(self) -> u64 {
        match self {
            NcType::Byte | NcType::Char => 1,
            NcType::Short => 2,
            NcType::Int | NcType::Float => 4,
            NcType::Double => 8,
        }
    }
    /// Our name for the type (NumPy-style dtype; `char` for text).
    pub fn dtype(self) -> &'static str {
        match self {
            NcType::Byte => "int8",
            NcType::Char => "char",
            NcType::Short => "int16",
            NcType::Int => "int32",
            NcType::Float => "float32",
            NcType::Double => "float64",
        }
    }
    fn decode(self, b: &[u8]) -> f64 {
        match self {
            NcType::Byte => f64::from(b[0] as i8),
            NcType::Char => f64::from(b[0]),
            NcType::Short => f64::from(i16::from_be_bytes([b[0], b[1]])),
            NcType::Int => f64::from(i32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            NcType::Float => f64::from(f32::from_be_bytes([b[0], b[1], b[2], b[3]])),
            NcType::Double => f64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        }
    }
}

/// A named dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dimension {
    pub name: String,
    /// Length; for the record (unlimited) dimension, the number of records.
    pub len: u64,
    pub unlimited: bool,
}

/// An attribute value.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    Text(String),
    Numbers(Vec<f64>),
}

impl AttrValue {
    /// Text, or a single number formatted as text.
    pub fn as_text(&self) -> Option<String> {
        match self {
            AttrValue::Text(s) => Some(s.clone()),
            AttrValue::Numbers(v) if v.len() == 1 => Some(v[0].to_string()),
            AttrValue::Numbers(_) => None,
        }
    }
    /// The first number (text that parses as a number counts).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            AttrValue::Numbers(v) => v.first().copied(),
            AttrValue::Text(s) => s.trim().parse().ok(),
        }
    }
    /// As JSON.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            AttrValue::Text(s) => serde_json::Value::String(s.clone()),
            AttrValue::Numbers(v) if v.len() == 1 => json_num(v[0]),
            AttrValue::Numbers(v) => {
                serde_json::Value::Array(v.iter().map(|x| json_num(*x)).collect())
            }
        }
    }
}

fn json_num(v: f64) -> serde_json::Value {
    serde_json::Number::from_f64(v).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

/// A named attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub nc_type: NcType,
    pub value: AttrValue,
}

/// A variable: shape (dimension ids), type and where its data lives.
#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub name: String,
    /// Indices into `NetCdf::dims`, outermost first.
    pub dim_ids: Vec<usize>,
    pub attributes: Vec<Attribute>,
    pub nc_type: NcType,
    /// Bytes per variable (fixed) or per record (record variables), as written.
    pub vsize: u64,
    /// File offset of the data (of the first record for record variables).
    pub begin: u64,
    /// True when the outermost dimension is the record dimension.
    pub is_record: bool,
}

/// A parsed netCDF header.
#[derive(Debug, Clone, PartialEq)]
pub struct NetCdf {
    /// 1 (classic) or 2 (64-bit offsets).
    pub version: u8,
    /// Number of records.
    pub numrecs: u64,
    pub dims: Vec<Dimension>,
    pub attributes: Vec<Attribute>,
    pub variables: Vec<Variable>,
    /// Bytes from the start of one record to the next.
    pub record_size: u64,
    /// Bytes of the header.
    pub header_len: u64,
}

/// Why a header could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    /// The bytes end before the header does (read more and retry).
    Short,
    /// Not a netCDF classic or 64-bit-offset file.
    NotNetCdf,
    /// CDF-5 (64-bit data) is recognised but not read.
    Cdf5,
    /// The header is malformed.
    Bad(String),
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn u32(&mut self) -> std::result::Result<u32, HeaderError> {
        let v = openreadout_core::bytes::be_u32(self.b, self.pos).ok_or(HeaderError::Short)?;
        self.pos += 4;
        Ok(v)
    }
    fn u64(&mut self) -> std::result::Result<u64, HeaderError> {
        let hi = u64::from(self.u32()?);
        let lo = u64::from(self.u32()?);
        Ok(hi << 32 | lo)
    }
    fn bytes(&mut self, n: usize) -> std::result::Result<&[u8], HeaderError> {
        let end = self.pos.checked_add(n).ok_or(HeaderError::Short)?;
        let s = self.b.get(self.pos..end).ok_or(HeaderError::Short)?;
        // values are padded to a 4-byte boundary
        self.pos = end + (4 - n % 4) % 4;
        Ok(s)
    }
    fn name(&mut self) -> std::result::Result<String, HeaderError> {
        let n = self.u32()? as usize;
        if n > 1 << 16 {
            return Err(HeaderError::Bad(format!("name length {n}")));
        }
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
    fn count(&mut self, what: &str) -> std::result::Result<usize, HeaderError> {
        let n = self.u32()? as usize;
        // every element takes at least 4 bytes, so a count larger than the input is corrupt
        if n > self.b.len() {
            return Err(HeaderError::Short);
        }
        if n > 1 << 24 {
            return Err(HeaderError::Bad(format!("{what} count {n}")));
        }
        Ok(n)
    }
    fn attrs(&mut self) -> std::result::Result<Vec<Attribute>, HeaderError> {
        let tag = self.u32()?;
        let n = self.count("attribute")?;
        if tag == 0 && n == 0 {
            return Ok(Vec::new());
        }
        if tag != 0x0C {
            return Err(HeaderError::Bad(format!("attribute list tag 0x{tag:X}")));
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let name = self.name()?;
            let code = self.u32()?;
            let t = NcType::from_code(code)
                .ok_or_else(|| HeaderError::Bad(format!("attribute {name}: type {code}")))?;
            let count = self.count("attribute value")?;
            let raw = self.bytes(count * t.size() as usize)?;
            let value = if t == NcType::Char {
                let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                AttrValue::Text(raw[..end].iter().map(|&c| c as char).collect::<String>())
            } else {
                AttrValue::Numbers(
                    raw.chunks_exact(t.size() as usize)
                        .map(|c| t.decode(c))
                        .collect(),
                )
            };
            out.push(Attribute {
                name,
                nc_type: t,
                value,
            });
        }
        Ok(out)
    }
}

impl NetCdf {
    /// Parse the header from the first bytes of a file.
    pub fn parse(b: &[u8]) -> std::result::Result<NetCdf, HeaderError> {
        if b.len() < 4 {
            return Err(HeaderError::Short);
        }
        if &b[..3] != b"CDF" {
            return Err(HeaderError::NotNetCdf);
        }
        let version = b[3];
        match version {
            1 | 2 => {}
            5 => return Err(HeaderError::Cdf5),
            _ => return Err(HeaderError::NotNetCdf),
        }
        let mut c = Cursor { b, pos: 4 };
        let mut numrecs = u64::from(c.u32()?);
        let streaming = numrecs == 0xFFFF_FFFF;
        // dimensions
        let tag = c.u32()?;
        let n = c.count("dimension")?;
        let mut dims = Vec::with_capacity(n);
        if !(tag == 0 && n == 0) {
            if tag != 0x0A {
                return Err(HeaderError::Bad(format!("dimension list tag 0x{tag:X}")));
            }
            for _ in 0..n {
                let name = c.name()?;
                let len = u64::from(c.u32()?);
                dims.push(Dimension {
                    name,
                    len,
                    unlimited: len == 0,
                });
            }
        }
        let attributes = c.attrs()?;
        let tag = c.u32()?;
        let n = c.count("variable")?;
        let mut variables = Vec::with_capacity(n);
        if !(tag == 0 && n == 0) {
            if tag != 0x0B {
                return Err(HeaderError::Bad(format!("variable list tag 0x{tag:X}")));
            }
            for _ in 0..n {
                let name = c.name()?;
                let nd = c.count("dimension id")?;
                let mut dim_ids = Vec::with_capacity(nd);
                for _ in 0..nd {
                    let id = c.u32()? as usize;
                    if id >= dims.len() {
                        return Err(HeaderError::Bad(format!(
                            "variable {name}: dimension id {id} of {}",
                            dims.len()
                        )));
                    }
                    dim_ids.push(id);
                }
                let vatts = c.attrs()?;
                let code = c.u32()?;
                let nc_type = NcType::from_code(code)
                    .ok_or_else(|| HeaderError::Bad(format!("variable {name}: type {code}")))?;
                let vsize = u64::from(c.u32()?);
                let begin = if version == 1 {
                    u64::from(c.u32()?)
                } else {
                    c.u64()?
                };
                let is_record = dim_ids.first().is_some_and(|&d| dims[d].unlimited);
                variables.push(Variable {
                    name,
                    dim_ids,
                    attributes: vatts,
                    nc_type,
                    vsize,
                    begin,
                    is_record,
                });
            }
        }
        let header_len = c.pos as u64;
        let mut nc = NetCdf {
            version,
            numrecs,
            dims,
            attributes,
            variables,
            record_size: 0,
            header_len,
        };
        let recs: Vec<usize> = (0..nc.variables.len())
            .filter(|&i| nc.variables[i].is_record)
            .collect();
        let single = recs.len() == 1;
        let mut size = 0u64;
        for &i in &recs {
            let slab = nc.slab_bytes(i);
            size = size.saturating_add(if single { slab } else { slab.div_ceil(4) * 4 });
        }
        nc.record_size = size;
        if streaming {
            numrecs = 0;
            nc.numrecs = numrecs;
        }
        for d in &mut nc.dims {
            if d.unlimited {
                d.len = nc.numrecs;
            }
        }
        Ok(nc)
    }

    /// Bytes of one record of variable `i` (or of the whole variable when fixed), unpadded.
    fn slab_bytes(&self, i: usize) -> u64 {
        let v = &self.variables[i];
        let inner = if v.is_record {
            &v.dim_ids[1..]
        } else {
            &v.dim_ids[..]
        };
        inner.iter().fold(v.nc_type.size(), |acc, &d| {
            acc.saturating_mul(self.dims[d].len)
        })
    }

    /// Find a variable by name.
    pub fn var(&self, name: &str) -> Option<&Variable> {
        self.variables.iter().find(|v| v.name == name)
    }

    /// Find a global attribute by name.
    pub fn attr(&self, name: &str) -> Option<&AttrValue> {
        self.attributes
            .iter()
            .find(|a| a.name == name)
            .map(|a| &a.value)
    }

    /// Shape of a variable (dimension lengths, outermost first).
    pub fn shape(&self, v: &Variable) -> Vec<u64> {
        v.dim_ids.iter().map(|&d| self.dims[d].len).collect()
    }

    /// Number of elements of a variable.
    pub fn element_count(&self, v: &Variable) -> u64 {
        self.shape(v).iter().fold(1u64, |a, &b| a.saturating_mul(b))
    }

    /// Byte offset one past the end of a variable's data.
    pub fn data_end(&self, v: &Variable) -> u64 {
        let idx = self.variables.iter().position(|x| x == v).unwrap_or(0);
        if v.is_record {
            if self.numrecs == 0 {
                return v.begin;
            }
            v.begin
                .saturating_add((self.numrecs - 1).saturating_mul(self.record_size))
                .saturating_add(self.slab_bytes(idx))
        } else {
            v.begin.saturating_add(self.slab_bytes(idx))
        }
    }

    /// Elements `[first, first + count)` of a variable, flattened in C order, as f64.
    pub fn read_f64(
        &self,
        file: &mut SourceFile,
        path: &Path,
        var: &Variable,
        first: u64,
        count: u64,
    ) -> Result<Vec<f64>> {
        let total = self.element_count(var);
        if first > total {
            return Err(Error::Usage(format!(
                "element {first} past the end of {} ({total} elements)",
                var.name
            )));
        }
        let count = count.min(total - first);
        let size = var.nc_type.size();
        let mut out = Vec::with_capacity(usize::try_from(count).unwrap_or(0).min(1 << 26));
        if count == 0 {
            return Ok(out);
        }
        if !var.is_record {
            let bytes = read_range(file, path, var.begin + first * size, count * size)?;
            if (bytes.len() as u64) < count * size {
                return Err(Error::corrupt_at(
                    crate::ANDI_ID,
                    var.begin + first * size,
                    format!(
                        "variable {} runs past the end of the file (truncated)",
                        var.name
                    ),
                ));
            }
            out.extend(
                bytes
                    .chunks_exact(size as usize)
                    .map(|c| var.nc_type.decode(c)),
            );
            return Ok(out);
        }
        // record variable: `per` elements per record, records `record_size` bytes apart
        let per: u64 = var.dim_ids[1..]
            .iter()
            .fold(1u64, |a, &d| a.saturating_mul(self.dims[d].len));
        if per == 0 {
            return Ok(out);
        }
        let r0 = first / per;
        let r1 = (first + count - 1) / per;
        // read in chunks of records to bound memory
        let chunk = (4 << 20) / self.record_size.max(1);
        let chunk = chunk.max(1);
        let mut record = r0;
        while record <= r1 {
            let rn = chunk.min(r1 - record + 1);
            let start = var.begin + record * self.record_size;
            let len = (rn - 1) * self.record_size + per * size;
            let bytes = read_range(file, path, start, len)?;
            if (bytes.len() as u64) < len {
                return Err(Error::corrupt_at(
                    crate::ANDI_ID,
                    start,
                    format!(
                        "record variable {} runs past the end of the file (truncated)",
                        var.name
                    ),
                ));
            }
            for k in 0..rn {
                let rec = record + k;
                let base = (k * self.record_size) as usize;
                for j in 0..per {
                    let elem = rec * per + j;
                    if elem < first || elem >= first + count {
                        continue;
                    }
                    let at = base + (j * size) as usize;
                    out.push(var.nc_type.decode(&bytes[at..at + size as usize]));
                }
            }
            record += rn;
        }
        Ok(out)
    }

    /// A char variable as strings: the innermost dimension is the string length.
    pub fn read_strings(
        &self,
        f: &mut SourceFile,
        path: &Path,
        v: &Variable,
    ) -> Result<Vec<String>> {
        if v.nc_type != NcType::Char {
            return Err(Error::Usage(format!("{} is not a char variable", v.name)));
        }
        let shape = self.shape(v);
        let width = shape.last().copied().unwrap_or(1).max(1);
        let all = self.read_f64(f, path, v, 0, self.element_count(v))?;
        Ok(all
            .chunks(width as usize)
            .map(|c| {
                c.iter()
                    .map(|&x| x as u8)
                    .take_while(|&b| b != 0)
                    .map(char::from)
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .collect())
    }
}

/// Read and parse the header of a netCDF file (reading more of the file when the header is long).
pub fn read_header(path: &Path) -> Result<NetCdf> {
    read_header_in(&Fs::local(), path)
}

/// [`read_header`] for `path` in the namespace `fs`.
pub(crate) fn read_header_in(fs: &Fs, path: &Path) -> Result<NetCdf> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
    let mut want = len.min(1 << 20);
    loop {
        let b = read_range(&mut f, path, 0, want)?;
        match NetCdf::parse(&b) {
            Ok(nc) => return Ok(nc),
            Err(HeaderError::Short) if want < len && want < (256 << 20) => {
                want = (want * 8).min(len);
            }
            Err(HeaderError::Short) => {
                return Err(Error::corrupt(
                    crate::ANDI_ID,
                    "netCDF header ends before the end of its dimension/attribute/variable lists (truncated)",
                ));
            }
            Err(HeaderError::NotNetCdf) => {
                return Err(Error::corrupt(
                    crate::ANDI_ID,
                    "not a netCDF classic file (no `CDF\\x01`/`CDF\\x02` at byte 0)",
                ));
            }
            Err(HeaderError::Cdf5) => {
                return Err(Error::unsupported(
                    crate::ANDI_ID,
                    "netCDF CDF-5 (64-bit data) files",
                    "Only netCDF classic (CDF-1) and 64-bit-offset (CDF-2) files are read; ANDI files are normally CDF-1.",
                ));
            }
            Err(HeaderError::Bad(m)) => return Err(Error::corrupt(crate::ANDI_ID, m)),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a small CDF-1 file: dims `n`=3 and unlimited `rec`; global text attr; a fixed
    /// double variable `x`[n], and two record variables `a`[rec] (float) and `b`[rec] (short).
    pub(crate) fn sample() -> Vec<u8> {
        fn name(o: &mut Vec<u8>, s: &str) {
            o.extend((s.len() as u32).to_be_bytes());
            o.extend(s.as_bytes());
            while !o.len().is_multiple_of(4) {
                o.push(0);
            }
        }
        let mut o = b"CDF\x01".to_vec();
        o.extend(2u32.to_be_bytes()); // numrecs
        o.extend(0x0Au32.to_be_bytes());
        o.extend(2u32.to_be_bytes());
        name(&mut o, "n");
        o.extend(3u32.to_be_bytes());
        name(&mut o, "rec");
        o.extend(0u32.to_be_bytes());
        o.extend(0x0Cu32.to_be_bytes());
        o.extend(1u32.to_be_bytes());
        name(&mut o, "title");
        o.extend(2u32.to_be_bytes());
        o.extend(2u32.to_be_bytes());
        o.extend(b"hi\0\0");
        o.extend(0x0Bu32.to_be_bytes());
        o.extend(3u32.to_be_bytes());
        let vars: [(&str, u32, u32, u32); 3] = [("x", 0, 6, 24), ("a", 1, 5, 4), ("b", 1, 3, 4)];
        let mut begins = Vec::new();
        for (nm, dim, t, vsize) in vars {
            name(&mut o, nm);
            o.extend(1u32.to_be_bytes());
            o.extend(dim.to_be_bytes());
            o.extend([0u8; 8]);
            o.extend(t.to_be_bytes());
            o.extend(vsize.to_be_bytes());
            begins.push(o.len());
            o.extend(0u32.to_be_bytes());
        }
        let data = o.len() as u32;
        o[begins[0]..begins[0] + 4].copy_from_slice(&data.to_be_bytes());
        o[begins[1]..begins[1] + 4].copy_from_slice(&(data + 24).to_be_bytes());
        o[begins[2]..begins[2] + 4].copy_from_slice(&(data + 28).to_be_bytes());
        for v in [1.0f64, 2.0, 3.0] {
            o.extend(v.to_be_bytes());
        }
        for r in 0..2 {
            o.extend((10.5f32 + r as f32).to_be_bytes());
            o.extend((-(r as i16) - 1).to_be_bytes());
            o.extend([0, 0]);
        }
        o
    }

    #[test]
    fn parse_and_read() {
        let image = sample();
        let nc = NetCdf::parse(&image).unwrap();
        assert_eq!(nc.numrecs, 2);
        assert_eq!(nc.record_size, 8);
        assert_eq!(nc.attr("title"), Some(&AttrValue::Text("hi".into())));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.cdf");
        std::fs::write(&path, &image).unwrap();
        let mut file = SourceFile::open_local(&path).unwrap();
        let var_x = nc.var("x").unwrap().clone();
        assert_eq!(
            nc.read_f64(&mut file, &path, &var_x, 1, 5).unwrap(),
            vec![2.0, 3.0]
        );
        let var_a = nc.var("a").unwrap().clone();
        assert_eq!(
            nc.read_f64(&mut file, &path, &var_a, 0, 9).unwrap(),
            vec![10.5, 11.5]
        );
        let bv = nc.var("b").unwrap().clone();
        assert_eq!(
            nc.read_f64(&mut file, &path, &bv, 0, 9).unwrap(),
            vec![-1.0, -2.0]
        );
        assert_eq!(nc.data_end(&bv), image.len() as u64 - 2);
        for cut in [3, 10, 30, 60] {
            assert!(NetCdf::parse(&image[..cut]).is_err());
        }
        assert_eq!(NetCdf::parse(b"CDF\x05...."), Err(HeaderError::Cdf5));
    }
}
