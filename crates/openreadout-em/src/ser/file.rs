//! TIA / ES Vision series file (`.ser`) structure: header, dimension array, offset arrays,
//! element headers and tags (see `docs/formats/ser.md`). All values are little-endian.

use openreadout_core::{Error, PixelType, Result};

use super::FORMAT_ID;
use openreadout_core::bytes::{le_f64, le_i32, le_u16, le_u32, le_u64};

use crate::util::Blob;

/// `ByteOrder` word of every series file (`II`).
pub const BYTE_ORDER_LE: u16 = 0x4949;
/// Series identification word.
pub const SERIES_ID: u16 = 0x0197;
/// Data type id of 1-D elements (spectra).
pub const ELEMENTS_1D: u32 = 0x4120;
/// Data type id of 2-D elements (images).
pub const ELEMENTS_2D: u32 = 0x4122;
/// Tag type id: time only.
pub const TAG_TIME: u32 = 0x4152;
/// Tag type id: time and 2-D position.
pub const TAG_TIME_POSITION: u32 = 0x4142;

/// One entry of the dimension array (a scan axis of the series, not of the elements).
#[derive(Debug, Clone, PartialEq)]
pub struct SerDimension {
    pub size: u32,
    pub calibration_offset: f64,
    pub calibration_delta: f64,
    pub calibration_element: i32,
    pub description: String,
    pub units: String,
}

/// The series header and its arrays.
#[derive(Debug, Clone, PartialEq)]
pub struct SerHeader {
    /// 0x0210 (4-byte offsets) or 0x0220 (8-byte offsets).
    pub version: u16,
    pub data_type_id: u32,
    pub tag_type_id: u32,
    pub total_elements: u32,
    pub valid_elements: u32,
    pub offset_array_offset: u64,
    pub dimensions: Vec<SerDimension>,
    /// Absolute offsets of the data elements (`total_elements` entries).
    pub data_offsets: Vec<u64>,
    /// Absolute offsets of the tags (`total_elements` entries).
    pub tag_offsets: Vec<u64>,
}

impl SerHeader {
    /// Width of offsets in the offset arrays.
    pub fn offset_width(&self) -> u64 {
        if self.version >= 0x0220 { 8 } else { 4 }
    }
}

/// Element sample type (element header `DataType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerDataType {
    Uint8,
    Uint16,
    Uint32,
    Int8,
    Int16,
    Int32,
    Float32,
    Float64,
    Complex64,
    Complex128,
    Other(u16),
}

impl SerDataType {
    pub fn from_code(c: u16) -> SerDataType {
        match c {
            1 => SerDataType::Uint8,
            2 => SerDataType::Uint16,
            3 => SerDataType::Uint32,
            4 => SerDataType::Int8,
            5 => SerDataType::Int16,
            6 => SerDataType::Int32,
            7 => SerDataType::Float32,
            8 => SerDataType::Float64,
            9 => SerDataType::Complex64,
            10 => SerDataType::Complex128,
            other => SerDataType::Other(other),
        }
    }

    pub fn bytes(self) -> Option<u64> {
        Some(match self {
            SerDataType::Uint8 | SerDataType::Int8 => 1,
            SerDataType::Uint16 | SerDataType::Int16 => 2,
            SerDataType::Uint32 | SerDataType::Int32 | SerDataType::Float32 => 4,
            SerDataType::Float64 | SerDataType::Complex64 => 8,
            SerDataType::Complex128 => 16,
            SerDataType::Other(_) => return None,
        })
    }

    pub fn pixel_type(self) -> Option<PixelType> {
        Some(match self {
            SerDataType::Uint8 => PixelType::Uint8,
            SerDataType::Uint16 => PixelType::Uint16,
            SerDataType::Uint32 => PixelType::Uint32,
            SerDataType::Int8 => PixelType::Int8,
            SerDataType::Int16 => PixelType::Int16,
            SerDataType::Int32 => PixelType::Int32,
            SerDataType::Float32 => PixelType::Float,
            SerDataType::Float64 => PixelType::Double,
            SerDataType::Complex64 => PixelType::ComplexFloat,
            SerDataType::Complex128 => PixelType::ComplexDouble,
            SerDataType::Other(_) => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            SerDataType::Uint8 => "uint8",
            SerDataType::Uint16 => "uint16",
            SerDataType::Uint32 => "uint32",
            SerDataType::Int8 => "int8",
            SerDataType::Int16 => "int16",
            SerDataType::Int32 => "int32",
            SerDataType::Float32 => "float32",
            SerDataType::Float64 => "float64",
            SerDataType::Complex64 => "complex64",
            SerDataType::Complex128 => "complex128",
            SerDataType::Other(_) => "unknown",
        }
    }
}

/// Calibration of one element axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisCalibration {
    pub offset: f64,
    pub delta: f64,
    pub element: i32,
}

/// Header of one data element.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementHeader {
    /// Absolute offset of the element header.
    pub offset: u64,
    pub data_type: SerDataType,
    /// Samples per row (2-D) or array length (1-D).
    pub size_x: u32,
    /// Rows (1 for 1-D elements).
    pub size_y: u32,
    pub calibration_x: AxisCalibration,
    /// `None` for 1-D elements.
    pub calibration_y: Option<AxisCalibration>,
    /// Absolute offset of the first sample.
    pub data_offset: u64,
}

impl ElementHeader {
    pub fn data_len(&self) -> Option<u64> {
        u64::from(self.size_x)
            .checked_mul(u64::from(self.size_y))?
            .checked_mul(self.data_type.bytes()?)
    }
}

/// A data tag (per-element time and optional position).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementTag {
    pub tag_type: u16,
    /// Seconds since 1970-01-01.
    pub time: u32,
    pub position: Option<(f64, f64)>,
}

fn cal(b: &[u8], off: usize) -> Option<AxisCalibration> {
    Some(AxisCalibration {
        offset: le_f64(b, off)?,
        delta: le_f64(b, off + 8)?,
        element: le_i32(b, off + 16)?,
    })
}

/// Header bytes that look like a series file.
pub fn looks_like_ser(b: &[u8]) -> bool {
    le_u16(b, 0) == Some(BYTE_ORDER_LE) && le_u16(b, 2) == Some(SERIES_ID)
}

/// Read the header, the dimension array and both offset arrays.
pub(crate) fn read_header(blob: &mut Blob) -> Result<SerHeader> {
    let head = blob.read_upto(0, 4096)?;
    if !looks_like_ser(&head) {
        return Err(Error::corrupt(
            FORMAT_ID,
            "file does not start with the series-file byte-order word 'II' and series id 0x0197",
        ));
    }
    let short = || Error::corrupt(FORMAT_ID, "series header runs past the end of the file");
    let version = le_u16(&head, 4).ok_or_else(short)?;
    let data_type_id = le_u32(&head, 6).ok_or_else(short)?;
    let tag_type_id = le_u32(&head, 10).ok_or_else(short)?;
    let total_elements = le_u32(&head, 14).ok_or_else(short)?;
    let valid_elements = le_u32(&head, 18).ok_or_else(short)?;
    let wide = version >= 0x0220;
    let (offset_array_offset, mut pos) = if wide {
        (le_u64(&head, 22).ok_or_else(short)?, 30usize)
    } else {
        (u64::from(le_u32(&head, 22).ok_or_else(short)?), 26usize)
    };
    let ndims = le_u32(&head, pos).ok_or_else(short)?;
    pos += 4;
    if ndims > 16 {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            pos as u64 - 4,
            format!("series declares {ndims} dimensions"),
        ));
    }
    let mut dimensions = Vec::new();
    for _ in 0..ndims {
        let size = le_u32(&head, pos).ok_or_else(short)?;
        let calibration_offset = le_f64(&head, pos + 4).ok_or_else(short)?;
        let calibration_delta = le_f64(&head, pos + 12).ok_or_else(short)?;
        let calibration_element = le_i32(&head, pos + 20).ok_or_else(short)?;
        let dl = le_u32(&head, pos + 24).ok_or_else(short)? as usize;
        let description = head
            .get(pos + 28..pos + 28 + dl)
            .map(crate::util::text)
            .ok_or_else(short)?;
        pos += 28 + dl;
        let ul = le_u32(&head, pos).ok_or_else(short)? as usize;
        let units = head
            .get(pos + 4..pos + 4 + ul)
            .map(crate::util::text)
            .ok_or_else(short)?;
        pos += 4 + ul;
        dimensions.push(SerDimension {
            size,
            calibration_offset,
            calibration_delta,
            calibration_element,
            description,
            units,
        });
    }
    let w = if wide { 8u64 } else { 4 };
    let n = u64::from(total_elements);
    let arrays_len = n
        .checked_mul(2 * w)
        .filter(|l| *l <= blob.len)
        .ok_or_else(|| {
            Error::corrupt_at(
                FORMAT_ID,
                14,
                format!("{total_elements} elements' offset arrays cannot fit in the file"),
            )
        })?;
    let raw = blob.read_at(FORMAT_ID, offset_array_offset, arrays_len)?;
    let at = |k: u64| -> u64 {
        let o = usize::try_from(k * w).unwrap_or(usize::MAX);
        if wide {
            le_u64(&raw, o).unwrap_or(0)
        } else {
            u64::from(le_u32(&raw, o).unwrap_or(0))
        }
    };
    let data_offsets = (0..n).map(at).collect();
    let tag_offsets = (n..2 * n).map(at).collect();
    Ok(SerHeader {
        version,
        data_type_id,
        tag_type_id,
        total_elements,
        valid_elements,
        offset_array_offset,
        dimensions,
        data_offsets,
        tag_offsets,
    })
}

/// Read the header of the element at `offset`.
pub(crate) fn read_element(
    blob: &mut Blob,
    data_type_id: u32,
    offset: u64,
) -> Result<ElementHeader> {
    if data_type_id == ELEMENTS_2D {
        let b = blob.read_at(FORMAT_ID, offset, 50)?;
        let bad = || Error::corrupt_at(FORMAT_ID, offset, "element header is unreadable");
        Ok(ElementHeader {
            offset,
            calibration_x: cal(&b, 0).ok_or_else(bad)?,
            calibration_y: Some(cal(&b, 20).ok_or_else(bad)?),
            data_type: SerDataType::from_code(le_u16(&b, 40).ok_or_else(bad)?),
            size_x: le_u32(&b, 42).ok_or_else(bad)?,
            size_y: le_u32(&b, 46).ok_or_else(bad)?,
            data_offset: offset + 50,
        })
    } else if data_type_id == ELEMENTS_1D {
        let b = blob.read_at(FORMAT_ID, offset, 26)?;
        let bad = || Error::corrupt_at(FORMAT_ID, offset, "element header is unreadable");
        Ok(ElementHeader {
            offset,
            calibration_x: cal(&b, 0).ok_or_else(bad)?,
            calibration_y: None,
            data_type: SerDataType::from_code(le_u16(&b, 20).ok_or_else(bad)?),
            size_x: le_u32(&b, 22).ok_or_else(bad)?,
            size_y: 1,
            data_offset: offset + 26,
        })
    } else {
        Err(Error::unsupported(
            FORMAT_ID,
            format!("series data type id 0x{data_type_id:04x}"),
            "Series files hold 1-D (0x4120) or 2-D (0x4122) elements; this value is neither. Run `openreadout check`.",
        ))
    }
}

/// Read the tag at `offset` (8 bytes; 24 with a position).
pub(crate) fn read_tag(blob: &mut Blob, offset: u64) -> Option<ElementTag> {
    let b = blob.read_upto(offset, 24).ok()?;
    let tag_type = le_u16(&b, 0)?;
    let time = le_u32(&b, 4)?;
    let position = if u32::from(tag_type) == TAG_TIME_POSITION {
        Some((le_f64(&b, 8)?, le_f64(&b, 16)?))
    } else {
        None
    };
    Some(ElementTag {
        tag_type,
        time,
        position,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A version-0x0220 series of `n` 2-D uint16 elements of `w` x `h`, with time tags.
    pub(crate) fn series(n: u32, w: u32, h: u32, delta: f64) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&BYTE_ORDER_LE.to_le_bytes());
        b.extend_from_slice(&SERIES_ID.to_le_bytes());
        b.extend_from_slice(&0x0220u16.to_le_bytes());
        b.extend_from_slice(&ELEMENTS_2D.to_le_bytes());
        b.extend_from_slice(&TAG_TIME.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
        let oao_at = b.len();
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes());
        // dimension: size n, offset 0, delta 1, element 0, "Number", no units
        b.extend_from_slice(&n.to_le_bytes());
        b.extend_from_slice(&0f64.to_le_bytes());
        b.extend_from_slice(&1f64.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes());
        b.extend_from_slice(&6u32.to_le_bytes());
        b.extend_from_slice(b"Number");
        b.extend_from_slice(&0u32.to_le_bytes());
        let oao = b.len() as u64;
        b[oao_at..oao_at + 8].copy_from_slice(&oao.to_le_bytes());
        let arrays = b.len();
        b.resize(arrays + 16 * n as usize, 0);
        let mut data_offs = Vec::new();
        for k in 0..n {
            data_offs.push(b.len() as u64);
            for _ in 0..2 {
                b.extend_from_slice(&0f64.to_le_bytes());
                b.extend_from_slice(&delta.to_le_bytes());
                b.extend_from_slice(&0i32.to_le_bytes());
            }
            b.extend_from_slice(&2u16.to_le_bytes());
            b.extend_from_slice(&w.to_le_bytes());
            b.extend_from_slice(&h.to_le_bytes());
            for p in 0..w * h {
                b.extend_from_slice(&((k * 1000 + p) as u16).to_le_bytes());
            }
        }
        let mut tag_offs = Vec::new();
        for k in 0..n {
            tag_offs.push(b.len() as u64);
            b.extend_from_slice(&(TAG_TIME as u16).to_le_bytes());
            b.extend_from_slice(&[0, 0]);
            b.extend_from_slice(&(1_600_000_000 + k).to_le_bytes());
        }
        for (k, o) in data_offs.iter().chain(tag_offs.iter()).enumerate() {
            b[arrays + 8 * k..arrays + 8 * k + 8].copy_from_slice(&o.to_le_bytes());
        }
        b
    }

    #[test]
    fn parses_header_elements_and_tags() {
        let bytes = series(3, 4, 2, 2e-10);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s_1.ser");
        std::fs::write(&p, &bytes).unwrap();
        let mut blob = Blob::open(&openreadout_core::source::Fs::local(), &p).unwrap();
        let h = read_header(&mut blob).unwrap();
        assert_eq!(
            (h.version, h.total_elements, h.valid_elements),
            (0x0220, 3, 3)
        );
        assert_eq!(h.dimensions[0].description, "Number");
        let e = read_element(&mut blob, h.data_type_id, h.data_offsets[1]).unwrap();
        assert_eq!(
            (e.size_x, e.size_y, e.data_type),
            (4, 2, SerDataType::Uint16)
        );
        assert_eq!(e.data_len(), Some(16));
        let t = read_tag(&mut blob, h.tag_offsets[2]).unwrap();
        assert_eq!(t.time, 1_600_000_002);
        assert!(t.position.is_none());
    }
}
