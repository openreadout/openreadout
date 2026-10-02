//! mzMLb: mzML inside an HDF5 file (Bhamber et al., J. Proteome Res. 2021, 20:172–183; PSI-MS
//! terms MS:1002841–1002843).
//!
//! The HDF5 file holds the mzML document, with every `<binary>` left empty, as the byte dataset
//! `mzML` (attribute `version` = `mzMLb 1.0`); the arrays as numeric datasets named by their
//! type (`spectrum_MS_1000514_double`, `chromatogram_MS_1000595_double`, ...), many spectra's
//! arrays concatenated in one dataset; and the offset index as `mzML_spectrumIndex` /
//! `mzML_chromatogramIndex` (int64 byte offsets into `mzML`, one more than there are elements:
//! the last is the end) with the ids, NUL-separated, in `mzML_spectrumIndex_idRef` /
//! `mzML_chromatogramIndex_idRef`. Each `binaryDataArray` names its dataset (*external HDF5
//! dataset*), the first element (*external offset*) and the element count (*external array
//! length*). Compression is HDF5's (deflate + shuffle, or the Blosc filter 32001), so a
//! `zlib compression` term describes the dataset filter, not the values.
//!
//! The XML is read through a byte view of the `mzML` dataset by the ordinary mzML reader
//! ([`MzmlDataset`]); arrays come from `H5Arrays`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use hdf5_pure::DType;
use openreadout_core::model::FormatDescriptor;
use openreadout_core::source::{ByteSource, Input};
use openreadout_core::{Error, Result};
use openreadout_hdf5::h5util::{
    FILTER_DEFLATE, FILTER_FLETCHER32, FILTER_LZ4, FILTER_SHUFFLE, attr_text, big_endian,
};
use serde_json::{Value, json};

use crate::binary::{ArrayEncoding, ArrayError, Compression};
use crate::mzml::MzmlDataset;
use crate::scan::{Located, WrittenIndex};

/// Format id of mzMLb.
pub(crate) const FMT: &str = "mzmlb";
/// Name of the dataset holding the XML.
const XML_DATASET: &str = "mzML";
/// HDF5 filter id of Blosc (registered by the Blosc project).
const FILTER_BLOSC: u16 = 32001;

/// Arrays stored outside the XML (implemented for mzMLb's HDF5 datasets).
pub(crate) trait ExternalArrays: Send + Sync + std::fmt::Debug {
    /// Elements `offset .. offset + count` of dataset `name`, as values; `enc` is what the
    /// `binaryDataArray` declares (MS-Numpress arrays are stored as bytes and decoded here).
    fn values(
        &self,
        name: &str,
        offset: u64,
        count: u64,
        enc: &ArrayEncoding,
    ) -> std::result::Result<Vec<f64>, ArrayError>;
}

/// `hdf5-pure` reading through a byte source (in-memory buffers, host callbacks).
#[derive(Debug)]
struct Src(Arc<dyn ByteSource>, u64);

impl hdf5_pure::Source for Src {
    fn len(&self) -> u64 {
        self.1
    }

    fn read_at(
        &self,
        offset: u64,
        buf: &mut [u8],
    ) -> std::result::Result<(), hdf5_pure::FormatError> {
        let end =
            offset
                .checked_add(buf.len() as u64)
                .ok_or(hdf5_pure::FormatError::OffsetOverflow {
                    offset,
                    length: buf.len() as u64,
                })?;
        if end > self.1 {
            return Err(hdf5_pure::FormatError::UnexpectedEof {
                expected: usize::try_from(end).unwrap_or(usize::MAX),
                available: usize::try_from(self.1).unwrap_or(usize::MAX),
            });
        }
        self.0
            .read_exact_at(offset, buf)
            .map_err(|e| hdf5_pure::FormatError::Source(e.to_string()))
    }
}

fn h5err(what: &str, e: &hdf5_pure::Error) -> Error {
    match e {
        hdf5_pure::Error::Io(io) => Error::Other(format!("{what}: {io}")),
        other => Error::corrupt(FMT, format!("{what}: {other}")),
    }
}

/// One 1-D dataset, read by element range.
#[derive(Debug)]
struct Rows {
    name: String,
    ds: hdf5_pure::Dataset,
    dtype: DType,
    elem: usize,
    len: u64,
    big_endian: bool,
    /// Chunks decoded here rather than by `hdf5-pure` (filters it lacks, such as Blosc):
    /// `(first element, file address, stored size, filter mask)`, the chunk length in
    /// elements and the filter pipeline.
    manual: Option<Manual>,
    /// Raw bytes of the file, for manual chunks.
    raw: Arc<dyn ByteSource>,
    /// The last chunk decoded manually.
    cache: Mutex<Option<(usize, Arc<Vec<u8>>)>>,
}

#[derive(Debug)]
struct Manual {
    chunk: u64,
    chunks: Vec<(u64, u64, u64, u32)>,
    filters: Vec<(u16, Vec<u32>)>,
}

impl Rows {
    fn open(file: &hdf5_pure::File, name: &str, raw: Arc<dyn ByteSource>) -> Result<Self> {
        let ds = file
            .dataset(name)
            .map_err(|e| h5err(&format!("dataset {name}"), &e))?;
        let shape = ds
            .shape()
            .map_err(|e| h5err(&format!("dataset {name}"), &e))?;
        let [len] = shape.as_slice() else {
            return Err(Error::corrupt(
                FMT,
                format!("dataset {name} has shape {shape:?}; mzMLb datasets are one-dimensional"),
            ));
        };
        let mut dtype = ds
            .dtype()
            .map_err(|e| h5err(&format!("dataset {name}"), &e))?;
        // psims stores MS-Numpress bytes as a 1-byte opaque type (tag `NUMPY:|u1`): bytes.
        if matches!(dtype, DType::Other(_)) && ds.element_size().ok() == Some(1) {
            dtype = DType::U8;
        }
        let elem = match dtype {
            DType::I8 | DType::U8 => 1,
            DType::I16 | DType::U16 => 2,
            DType::I32 | DType::U32 | DType::F32 => 4,
            DType::I64 | DType::U64 | DType::F64 => 8,
            ref other => {
                return Err(Error::unsupported(
                    FMT,
                    format!("dataset {name} of type {other}"),
                    "mzMLb arrays are numeric (8- to 64-bit integers or floats).",
                ));
            }
        };
        let big = ds.datatype().is_ok_and(|d| big_endian(&d));
        let filters: Vec<(u16, Vec<u32>)> = ds
            .filter_pipeline()
            .into_iter()
            .map(|f| (f.id, f.client_data))
            .collect();
        // `hdf5-pure` allocates a contiguous dataset's whole storage, or a whole chunk, before
        // it finds the file too short: bound both by the file first, so a tiny file cannot make
        // it allocate gigabytes. Unfiltered data are stored as they are, so neither their
        // declared size nor a contiguous run can exceed the file; a chunk (decoded) may exceed
        // it only up to the plane limit.
        if let Ok(file_len) = raw.size() {
            let stored = len.saturating_mul(elem as u64);
            let too_big = |what: String| {
                Error::corrupt(
                    FMT,
                    format!("dataset {name} {what} in a file of {file_len} bytes"),
                )
            };
            if filters.is_empty() && stored > file_len {
                return Err(too_big(format!(
                    "declares {len} elements ({stored} bytes, unfiltered)"
                )));
            }
            match ds.layout() {
                Ok(hdf5_pure::Layout::Contiguous { size, .. }) if size > file_len => {
                    return Err(too_big(format!(
                        "declares {size} bytes of contiguous storage"
                    )));
                }
                Ok(hdf5_pure::Layout::Chunked { chunk_shape, .. }) => {
                    let chunk = chunk_shape
                        .iter()
                        .fold(elem as u64, |acc, &d| acc.saturating_mul(d));
                    if chunk > openreadout_core::limits::plane_limit(file_len) {
                        return Err(too_big(format!("declares chunks of {chunk} bytes")));
                    }
                    // Each stored chunk is read whole before it is decoded.
                    if let Ok(chunks) = ds.chunks()
                        && let Some(c) = chunks
                            .iter()
                            .find(|c| c.address.saturating_add(c.storage_size) > file_len)
                    {
                        return Err(too_big(format!(
                            "stores a chunk of {} bytes at {}",
                            c.storage_size, c.address
                        )));
                    }
                }
                _ => {}
            }
        }
        let manual = if filters.iter().any(|(id, _)| matches!(*id, FILTER_BLOSC)) {
            let chunk = ds
                .chunk_shape()
                .ok()
                .flatten()
                .and_then(|c| c.first().copied())
                .filter(|&c| c > 0)
                .ok_or_else(|| Error::corrupt(FMT, format!("dataset {name}: no chunk shape")))?;
            let mut chunks: Vec<(u64, u64, u64, u32)> = ds
                .chunks()
                .map_err(|e| h5err(&format!("dataset {name} chunk index"), &e))?
                .into_iter()
                .map(|c| {
                    (
                        c.offset.first().copied().unwrap_or(0),
                        c.address,
                        c.storage_size,
                        c.filter_mask,
                    )
                })
                .collect();
            chunks.sort_unstable_by_key(|c| c.0);
            Some(Manual {
                chunk,
                chunks,
                filters: filters.clone(),
            })
        } else {
            None
        };
        Ok(Rows {
            name: name.to_string(),
            ds,
            dtype,
            elem,
            len: *len,
            big_endian: big,
            manual,
            raw,
            cache: Mutex::new(None),
        })
    }

    /// Stored bytes (file byte order) of elements `first .. first + n`.
    fn bytes(&self, first: u64, n: u64) -> std::result::Result<Vec<u8>, String> {
        if first.checked_add(n).is_none_or(|e| e > self.len) {
            return Err(format!(
                "elements {first}..{} lie past the end of dataset {} ({} elements)",
                first.saturating_add(n),
                self.name,
                self.len
            ));
        }
        if n == 0 {
            return Ok(Vec::new());
        }
        let Some(m) = &self.manual else {
            return self
                .ds
                .read_raw_rows(first, n)
                .map_err(|e| format!("dataset {}: {e}", self.name));
        };
        let want = usize::try_from(n.saturating_mul(self.elem as u64))
            .map_err(|_| "read too large".to_string())?;
        let mut out = Vec::with_capacity(want.min(1 << 26));
        let mut at = first;
        let end = first + n;
        while at < end {
            let ci = m.chunks.partition_point(|c| c.0 <= at).saturating_sub(1);
            let Some(&(c0, _, _, _)) = m.chunks.get(ci) else {
                return Err(format!(
                    "dataset {}: no chunk holds element {at}",
                    self.name
                ));
            };
            if at < c0 || at >= c0 + m.chunk {
                return Err(format!(
                    "dataset {}: element {at} lies in an unallocated chunk",
                    self.name
                ));
            }
            let data = self.chunk(m, ci)?;
            let lo = usize::try_from((at - c0) * self.elem as u64).unwrap_or(usize::MAX);
            let upto = end.min(c0 + m.chunk);
            let hi = usize::try_from((upto - c0) * self.elem as u64).unwrap_or(usize::MAX);
            let part = data.get(lo..hi).ok_or_else(|| {
                format!(
                    "dataset {}: chunk at element {c0} decodes to {} bytes, too short",
                    self.name,
                    data.len()
                )
            })?;
            out.extend_from_slice(part);
            at = upto;
        }
        Ok(out)
    }

    fn chunk(&self, m: &Manual, ci: usize) -> std::result::Result<Arc<Vec<u8>>, String> {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((i, d)) = cache.as_ref()
            && *i == ci
        {
            return Ok(d.clone());
        }
        let (_, addr, size, mask) = m.chunks[ci];
        let size = usize::try_from(size).map_err(|_| "chunk too large".to_string())?;
        let mut data = vec![0u8; size];
        self.raw
            .read_exact_at(addr, &mut data)
            .map_err(|e| format!("dataset {}: chunk at byte {addr}: {e}", self.name))?;
        let expect = usize::try_from(m.chunk.saturating_mul(self.elem as u64)).unwrap_or(0);
        for (k, (id, _)) in m.filters.iter().enumerate().rev() {
            if mask & (1 << k) != 0 {
                continue;
            }
            let codec = |e: openreadout_codecs::CodecError| format!("dataset {}: {e}", self.name);
            data = match *id {
                FILTER_BLOSC => openreadout_codecs::blosc_decode(&data).map_err(codec)?,
                FILTER_DEFLATE => openreadout_codecs::zlib_decode(&data, expect).map_err(codec)?,
                FILTER_LZ4 => openreadout_codecs::hdf5_lz4_decode(&data).map_err(codec)?,
                FILTER_SHUFFLE => unshuffle(&data, self.elem),
                FILTER_FLETCHER32 => {
                    let keep = data.len().saturating_sub(4);
                    data.truncate(keep);
                    data
                }
                other => {
                    return Err(format!(
                        "dataset {}: HDF5 filter {other} is not supported",
                        self.name
                    ));
                }
            };
        }
        let d = Arc::new(data);
        *cache = Some((ci, d.clone()));
        Ok(d)
    }

    /// Elements as `f64`.
    fn to_f64(&self, raw: &[u8]) -> Vec<f64> {
        let be = self.big_endian;
        macro_rules! conv {
            ($t:ty, $w:expr) => {
                raw.as_chunks::<$w>()
                    .0
                    .iter()
                    .map(|b| {
                        (if be {
                            <$t>::from_be_bytes(*b)
                        } else {
                            <$t>::from_le_bytes(*b)
                        }) as f64
                    })
                    .collect()
            };
        }
        match self.dtype {
            DType::I8 => raw.iter().map(|&b| f64::from(b as i8)).collect(),
            DType::U8 => raw.iter().map(|&b| f64::from(b)).collect(),
            DType::I16 => conv!(i16, 2),
            DType::U16 => conv!(u16, 2),
            DType::I32 => conv!(i32, 4),
            DType::U32 => conv!(u32, 4),
            DType::F32 => conv!(f32, 4),
            DType::I64 => conv!(i64, 8),
            DType::U64 => conv!(u64, 8),
            _ => conv!(f64, 8),
        }
    }
}

/// HDF5 byte shuffle, undone (`elem` bytes per element).
fn unshuffle(data: &[u8], elem: usize) -> Vec<u8> {
    if elem <= 1 {
        return data.to_vec();
    }
    let n = data.len() / elem;
    let mut out = data.to_vec();
    for i in 0..n {
        for b in 0..elem {
            out[i * elem + b] = data[b * n + i];
        }
    }
    out
}

/// The array datasets of an mzMLb file, opened on first use.
#[derive(Debug)]
pub(crate) struct H5Arrays {
    file: hdf5_pure::File,
    raw: Arc<dyn ByteSource>,
    open: Mutex<HashMap<String, Arc<Rows>>>,
}

impl H5Arrays {
    fn rows(&self, name: &str) -> std::result::Result<Arc<Rows>, ArrayError> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(r) = open.get(name) {
            return Ok(r.clone());
        }
        let r = Arc::new(
            Rows::open(&self.file, name, self.raw.clone())
                .map_err(|e| ArrayError::Layout(e.to_string()))?,
        );
        open.insert(name.to_string(), r.clone());
        Ok(r)
    }
}

impl ExternalArrays for H5Arrays {
    fn values(
        &self,
        name: &str,
        offset: u64,
        count: u64,
        enc: &ArrayEncoding,
    ) -> std::result::Result<Vec<f64>, ArrayError> {
        let rows = self.rows(name)?;
        let raw = rows.bytes(offset, count).map_err(ArrayError::Layout)?;
        match enc.compression {
            // HDF5's filters did the general-purpose compression: the values are plain.
            Compression::NoCompression | Compression::Zlib | Compression::Zstd => {
                Ok(rows.to_f64(&raw))
            }
            // MS-Numpress output is bytes: stored in a byte dataset, `count` bytes long. A
            // numeric dataset under a Numpress term holds plain values (psims writes such
            // chromatogram arrays): the dataset's type is authoritative, as for pyteomics.
            Compression::NumpressLinear(_)
            | Compression::NumpressPic(_)
            | Compression::NumpressSlof(_) => {
                if rows.elem == 1 {
                    crate::binary::decode_values(raw, enc, None)
                } else {
                    Ok(rows.to_f64(&raw))
                }
            }
            Compression::Unsupported(what) => Err(ArrayError::Unsupported(what.to_string())),
        }
    }
}

/// The byte view of the `mzML` dataset that the XML reader reads.
#[derive(Debug)]
struct XmlView {
    rows: Arc<Rows>,
    name: String,
}

impl ByteSource for XmlView {
    fn size(&self) -> std::io::Result<u64> {
        Ok(self.rows.len)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (buf.len() as u64).min(self.rows.len.saturating_sub(offset));
        if n == 0 {
            return Ok(0);
        }
        let b = self
            .rows
            .bytes(offset, n)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let k = b.len().min(buf.len());
        buf[..k].copy_from_slice(&b[..k]);
        Ok(k)
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

/// An index kept as `mzML_<kind>Index` (int64 offsets, one extra at the end) and
/// `mzML_<kind>Index_idRef` (NUL-separated ids).
fn read_index(
    file: &hdf5_pure::File,
    kind: &str,
    raw: &Arc<dyn ByteSource>,
) -> Result<Option<Vec<Located>>> {
    let off_name = format!("mzML_{kind}Index");
    let id_name = format!("mzML_{kind}Index_idRef");
    if file.dataset(&off_name).is_err() {
        return Ok(None);
    }
    let offs = Rows::open(file, &off_name, raw.clone())?;
    let ids = Rows::open(file, &id_name, raw.clone())?;
    // An index larger than the file could hold is a damaged header, not something to allocate.
    let size = raw.size().unwrap_or(0);
    if offs.len.saturating_mul(8) > size || ids.len > size {
        return Err(Error::corrupt(
            FMT,
            format!(
                "{off_name} declares {} entries and {id_name} {} bytes, more than the {size}-byte file holds",
                offs.len, ids.len
            ),
        ));
    }
    let o = offs
        .bytes(0, offs.len)
        .map_err(|e| Error::corrupt(FMT, e))?;
    let offsets = offs.to_f64(&o);
    let idb = ids.bytes(0, ids.len).map_err(|e| Error::corrupt(FMT, e))?;
    let id_list: Vec<String> = idb
        .split(|&b| b == 0)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    // offsets has one more entry (the end) than there are elements
    let n = offsets.len().saturating_sub(1);
    if id_list.len() < n {
        return Err(Error::corrupt(
            FMT,
            format!(
                "{id_name} names {} ids for {n} {kind} offsets",
                id_list.len()
            ),
        ));
    }
    Ok(Some(
        offsets
            .iter()
            .take(n)
            .zip(id_list)
            .map(|(&o, id)| Located {
                offset: o as u64,
                id,
            })
            .collect(),
    ))
}

/// What `info` reports about the container.
#[derive(Debug, Clone)]
pub(crate) struct Container {
    pub(crate) facts: Value,
}

/// Open an mzMLb file.
pub(crate) fn open(input: &Input) -> Result<(MzmlDataset, Container)> {
    let path = input.path();
    let raw = input.fs().source(path).map_err(|e| Error::io(path, e))?;
    let size = raw.size().map_err(|e| Error::io(path, e))?;
    let file = if input.is_local() {
        hdf5_pure::File::open_streaming(path)
    } else {
        hdf5_pure::File::from_source(Src(raw.clone(), size))
    }
    .map_err(|e| match e {
        hdf5_pure::Error::Io(io) => Error::io(path, io),
        other => Error::corrupt(FMT, format!("HDF5: {other}")),
    })?;
    let xml = file.dataset(XML_DATASET).map_err(|_| Error::Corrupt {
        format: FMT,
        detail: "no `mzML` dataset: not an mzMLb file".into(),
        offset: None,
    })?;
    let version = xml
        .attrs()
        .ok()
        .and_then(|a| a.get("version").and_then(attr_text));
    let rows = Arc::new(Rows::open(&file, XML_DATASET, raw.clone())?);
    let spectra = read_index(&file, "spectrum", &raw)?;
    let chromatograms = read_index(&file, "chromatogram", &raw)?;
    let mut filters = rows.ds.filters();
    filters.sort_unstable();
    filters.dedup();
    let mut datasets = Vec::new();
    if let Ok(names) = file.root().datasets() {
        for n in names {
            if n.starts_with("spectrum_") || n.starts_with("chromatogram_") {
                datasets.push(n);
            }
        }
    }
    let name = path
        .file_name()
        .map_or_else(|| "mzML".into(), |n| n.to_string_lossy().into_owned());
    let view = Input::from_source(format!("{name}.xml"), Arc::new(XmlView { rows, name }));
    let given = match (&spectra, &chromatograms) {
        (None, None) => None,
        _ => Some(WrittenIndex {
            at: 0,
            lists: vec![
                ("spectrum".into(), spectra.clone().unwrap_or_default()),
                (
                    "chromatogram".into(),
                    chromatograms.clone().unwrap_or_default(),
                ),
            ],
        }),
    };
    let mut ds = MzmlDataset::open_in_with(view.fs(), view.path(), given)?;
    let arrays = H5Arrays {
        file,
        raw,
        open: Mutex::new(HashMap::new()),
    };
    ds.hdf5 = Some(Arc::new(arrays));
    let facts = json!({
        "container": "hdf5",
        "mzmlb_version": version,
        "xml_bytes": ds_len(&ds),
        "array_datasets": datasets,
        "xml_filters": filters.iter().map(|f| filter_name(*f)).collect::<Vec<_>>(),
        "index": if spectra.is_some() { "mzML_spectrumIndex" } else { "none (the XML was scanned)" },
    });
    Ok((ds, Container { facts }))
}

fn ds_len(ds: &MzmlDataset) -> u64 {
    ds.file_len()
}

fn filter_name(id: u16) -> String {
    match id {
        FILTER_DEFLATE => "deflate".into(),
        FILTER_SHUFFLE => "shuffle".into(),
        FILTER_FLETCHER32 => "fletcher32".into(),
        FILTER_LZ4 => "lz4".into(),
        FILTER_BLOSC => "blosc".into(),
        other => format!("filter {other}"),
    }
}

/// True when the head is HDF5 and names mzMLb's datasets (or the path ends in `.mzMLb`).
pub(crate) fn sniff(head: &[u8], path: &Path) -> Option<bool> {
    if !openreadout_hdf5::h5util::looks_like_hdf5(head) {
        return None;
    }
    let named = head
        .windows(b"mzML_spectrumIndex".len())
        .any(|w| w == b"mzML_spectrumIndex")
        || head.windows(b"mzMLb 1.".len()).any(|w| w == b"mzMLb 1.");
    let ext = openreadout_core::reader::has_extension(path, &["mzmlb"]);
    if named || ext { Some(named) } else { None }
}

/// The mzMLb format descriptor.
pub(crate) fn descriptor() -> FormatDescriptor {
    FormatDescriptor {
        id: FMT.into(),
        name: "mzMLb (mzML in HDF5)".into(),
        vendor: "HUPO-PSI open standard (converted from any vendor)".into(),
        extensions: vec!["mzmlb".into()],
        family: "mass-spectrometry".into(),
        can_read: true,
        can_write: false,
        confidence: crate::assurance::MZMLB.confidence,
        known_gaps: vec![
            "The truncation + prediction encodings (MS:1003089/1003090) are recognised but not decoded (exit 6)".into(),
            "HDF5 filters other than deflate, shuffle, Fletcher-32, LZ4 and Blosc (e.g. SZIP) are not decoded".into(),
            "Everything the mzML reader does not do applies here too (see mzml)".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::unshuffle;

    #[test]
    fn unshuffle_round_trip() {
        // two 4-byte elements shuffled: all first bytes, all second bytes, ...
        let shuffled = [1u8, 5, 2, 6, 3, 7, 4, 8];
        assert_eq!(unshuffle(&shuffled, 4), vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(unshuffle(&[9, 8], 1), vec![9, 8]);
    }
}
