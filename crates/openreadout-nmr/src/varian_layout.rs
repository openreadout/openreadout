//! Varian/Agilent VnmrJ data directory layout: the `fid` file header and block headers, sample
//! decoding and path resolution. See `docs/formats/varian-nmr.md`.
//!
//! A `fid` file is a 32-byte big-endian file header followed by `block_count` blocks of
//! `block_bytes` each: `block_headers` 28-byte block headers, then `traces_per_block` traces
//! of `trace_bytes` (`points` interleaved real/imaginary values of `element_bytes` each).

use std::io::Read;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{be_f32, be_i16, be_i32};
use openreadout_core::source::Fs;

/// Bytes of the file header.
pub const FILE_HEADER_BYTES: u64 = 32;
/// Bytes of one block header (and of a hypercomplex block header).
pub const BLOCK_HEADER_BYTES: u64 = 28;

/// Stored sample type, from the file header's status bits and element size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarianSampleType {
    /// 16-bit two's-complement integers (status bits 0x4 and 0x8 clear).
    Int16,
    /// 32-bit two's-complement integers (status bit 0x4).
    Int32,
    /// IEEE-754 binary32 (status bit 0x8).
    Float32,
}

impl VarianSampleType {
    /// Bytes per value.
    pub fn width(self) -> u64 {
        match self {
            VarianSampleType::Int16 => 2,
            VarianSampleType::Int32 | VarianSampleType::Float32 => 4,
        }
    }
    /// NumPy dtype name.
    pub fn dtype(self) -> &'static str {
        match self {
            VarianSampleType::Int16 => "int16",
            VarianSampleType::Int32 => "int32",
            VarianSampleType::Float32 => "float32",
        }
    }
}

/// Decode big-endian stored values to f64 (exact for every type).
pub fn decode_varian(bytes: &[u8], ty: VarianSampleType) -> Vec<f64> {
    match ty {
        VarianSampleType::Int16 => bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&a| f64::from(i16::from_be_bytes(a)))
            .collect(),
        VarianSampleType::Int32 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&a| f64::from(i32::from_be_bytes(a)))
            .collect(),
        VarianSampleType::Float32 => bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&a| f64::from(f32::from_be_bytes(a)))
            .collect(),
    }
}

/// The 32-byte file header of `fid` (six 32-bit counts, two 16-bit codes, one 32-bit count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FidHeader {
    /// Number of blocks.
    pub block_count: i32,
    /// Traces in each block.
    pub traces_per_block: i32,
    /// Values per trace (real and imaginary counted separately).
    pub points: i32,
    /// Bytes per value.
    pub element_bytes: i32,
    /// Bytes per trace.
    pub trace_bytes: i32,
    /// Bytes per block, block headers included.
    pub block_bytes: i32,
    /// Software/file version code.
    pub version_code: i16,
    /// Status bits of the whole file (see `status_names`).
    pub status: i16,
    /// Block headers at the start of each block.
    pub block_headers: i32,
}

/// Status bits as nmrglue names them, with our names.
const STATUS_BITS: &[(u16, &str)] = &[
    (0x1, "data"),
    (0x2, "spectrum"),
    (0x4, "int32"),
    (0x8, "float32"),
    (0x10, "complex"),
    (0x20, "hypercomplex"),
    (0x80, "acquisition_parameters"),
    (0x100, "secondary_fourier_transform"),
    (0x200, "transposed"),
    (0x800, "np_dimension"),
    (0x1000, "nf_dimension"),
    (0x2000, "ni_dimension"),
    (0x4000, "ni2_dimension"),
];

impl FidHeader {
    /// Parse the first 32 bytes (big-endian).
    pub fn parse(b: &[u8]) -> Option<Self> {
        Some(FidHeader {
            block_count: be_i32(b, 0)?,
            traces_per_block: be_i32(b, 4)?,
            points: be_i32(b, 8)?,
            element_bytes: be_i32(b, 12)?,
            trace_bytes: be_i32(b, 16)?,
            block_bytes: be_i32(b, 20)?,
            version_code: be_i16(b, 24)?,
            status: be_i16(b, 26)?,
            block_headers: be_i32(b, 28)?,
        })
    }

    /// The sample type the status bits and element size declare, or why they are unusable.
    pub fn sample_type(&self) -> std::result::Result<VarianSampleType, String> {
        let st = self.status as u16;
        let ty = if st & 0x8 != 0 {
            VarianSampleType::Float32
        } else if st & 0x4 != 0 {
            VarianSampleType::Int32
        } else {
            VarianSampleType::Int16
        };
        if i64::from(self.element_bytes) != ty.width() as i64 {
            return Err(format!(
                "status 0x{st:x} declares {} but the element size is {} bytes",
                ty.dtype(),
                self.element_bytes
            ));
        }
        Ok(ty)
    }

    /// Structural problems: negative or zero counts, sizes that do not add up.
    pub fn problems(&self) -> Vec<String> {
        let mut p = Vec::new();
        if self.block_count < 0 {
            p.push(format!("negative block count {}", self.block_count));
        }
        if self.traces_per_block < 1 {
            p.push(format!("{} traces per block", self.traces_per_block));
        }
        if self.points < 1 {
            p.push(format!("{} points per trace", self.points));
        }
        if !matches!(self.element_bytes, 2 | 4) {
            p.push(format!("element size {} bytes", self.element_bytes));
        }
        if !(0..=64).contains(&self.block_headers) {
            p.push(format!("{} block headers per block", self.block_headers));
        }
        let tb = i64::from(self.points) * i64::from(self.element_bytes);
        if tb != i64::from(self.trace_bytes) {
            p.push(format!(
                "trace size {} bytes; {} points of {} bytes need {tb}",
                self.trace_bytes, self.points, self.element_bytes
            ));
        }
        let bb = i64::from(self.traces_per_block) * i64::from(self.trace_bytes)
            + i64::from(self.block_headers) * BLOCK_HEADER_BYTES as i64;
        if bb != i64::from(self.block_bytes) {
            p.push(format!(
                "block size {} bytes; {} traces and {} block headers need {bb}",
                self.block_bytes, self.traces_per_block, self.block_headers
            ));
        }
        if let Err(e) = self.sample_type() {
            p.push(e);
        }
        p
    }

    /// Names of the status bits that are set.
    pub fn status_names(&self) -> Vec<&'static str> {
        let st = self.status as u16;
        STATUS_BITS
            .iter()
            .filter(|(bit, _)| st & bit != 0)
            .map(|(_, n)| *n)
            .collect()
    }

    /// Bytes the header declares: header plus every block.
    pub fn declared_len(&self) -> u64 {
        FILE_HEADER_BYTES.saturating_add(
            u64::try_from(self.block_count)
                .unwrap_or(0)
                .saturating_mul(u64::try_from(self.block_bytes).unwrap_or(0)),
        )
    }
}

/// One 28-byte block header (four 16-bit codes, a 32-bit scan count, four floats).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockHeader {
    /// Scaling exponent of the block (0 in every corpus file; not applied).
    pub scale: i16,
    /// Status bits of the block.
    pub status: i16,
    /// Block number, 1-based.
    pub index: i16,
    /// Mode bits.
    pub mode: i16,
    /// Scans completed for this block.
    pub completed_scans: i32,
    /// Left phase.
    pub left_phase: f32,
    /// Right phase.
    pub right_phase: f32,
    /// Level (drift correction).
    pub level: f32,
    /// Tilt (drift correction).
    pub tilt: f32,
}

impl BlockHeader {
    /// Parse 28 big-endian bytes.
    pub fn parse(b: &[u8]) -> Option<Self> {
        Some(BlockHeader {
            scale: be_i16(b, 0)?,
            status: be_i16(b, 2)?,
            index: be_i16(b, 4)?,
            mode: be_i16(b, 6)?,
            completed_scans: be_i32(b, 8)?,
            left_phase: be_f32(b, 12)?,
            right_phase: be_f32(b, 16)?,
            level: be_f32(b, 20)?,
            tilt: be_f32(b, 24)?,
        })
    }
}

/// The 32-byte header of `path` when it is a plausible `fid` file: a consistent header whose
/// declared size fits the file (a truncated file still qualifies when at least the header and
/// the sizes agree).
pub fn read_fid_header(path: &Path) -> Option<FidHeader> {
    read_fid_header_in(&Fs::local(), path)
}

/// [`read_fid_header`] in the namespace `fs`.
pub(crate) fn read_fid_header_in(fs: &Fs, path: &Path) -> Option<FidHeader> {
    let mut f = fs.open(path).ok()?;
    let mut b = [0u8; 32];
    f.read_exact(&mut b).ok()?;
    let h = FidHeader::parse(&b)?;
    h.problems().is_empty().then_some(h)
}

/// Resolve a path to its data directory: a directory holding `fid` with a valid header (and
/// usually `procpar`), or the directory of such a `fid`, `procpar`, `text` or `log` file.
/// Bruker experiment directories (with `acqus`) are not Varian directories.
pub fn resolve_varian_dir(path: &Path) -> Option<PathBuf> {
    resolve_varian_dir_in(&Fs::local(), path)
}

/// [`resolve_varian_dir`] in the namespace `fs`.
pub(crate) fn resolve_varian_dir_in(fs: &Fs, path: &Path) -> Option<PathBuf> {
    let is_dir = |d: &Path| {
        !fs.exists(&d.join("acqus"))
            && !fs.exists(&d.join("acqu"))
            && fs.is_file(&d.join("fid"))
            && read_fid_header_in(fs, &d.join("fid")).is_some()
    };
    let meta = fs.metadata(path).ok()?;
    if meta.is_dir() {
        return is_dir(path).then(|| path.to_path_buf());
    }
    let name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    (["fid", "procpar", "text", "log"].contains(&name) && is_dir(parent))
        .then(|| parent.to_path_buf())
}

/// Data directories directly inside `dir` (a study folder of `*.fid` directories), sorted by
/// name.
pub fn find_varian_experiments(dir: &Path) -> Vec<PathBuf> {
    find_varian_experiments_in(&Fs::local(), dir)
}

/// [`find_varian_experiments`] in the namespace `fs`.
pub(crate) fn find_varian_experiments_in(fs: &Fs, dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs.read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .take(10_000)
        .filter(|p| resolve_varian_dir_in(fs, p).as_deref() == Some(p.as_path()))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(status: i16, eb: i32) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [2i32, 1, 4, eb, 4 * eb, 4 * eb + 28] {
            b.extend_from_slice(&v.to_be_bytes());
        }
        b.extend_from_slice(&0i16.to_be_bytes());
        b.extend_from_slice(&status.to_be_bytes());
        b.extend_from_slice(&1i32.to_be_bytes());
        b
    }

    #[test]
    fn header_types_and_problems() {
        let h = FidHeader::parse(&header(0xc9, 4)).unwrap();
        assert_eq!(h.sample_type(), Ok(VarianSampleType::Float32));
        assert!(h.problems().is_empty());
        assert_eq!(h.declared_len(), 32 + 2 * 44);
        assert!(h.status_names().contains(&"float32"));
        let h = FidHeader::parse(&header(0x45, 4)).unwrap();
        assert_eq!(h.sample_type(), Ok(VarianSampleType::Int32));
        let h = FidHeader::parse(&header(0x41, 2)).unwrap();
        assert_eq!(h.sample_type(), Ok(VarianSampleType::Int16));
        let bad = FidHeader::parse(&header(0x41, 4)).unwrap();
        assert!(!bad.problems().is_empty());
        assert!(FidHeader::parse(&[0u8; 10]).is_none());
    }

    #[test]
    fn decode_types() {
        assert_eq!(
            decode_varian(&[0xff, 0xfe, 0, 3], VarianSampleType::Int16),
            vec![-2.0, 3.0]
        );
        assert_eq!(
            decode_varian(&(-7i32).to_be_bytes(), VarianSampleType::Int32),
            vec![-7.0]
        );
        assert_eq!(
            decode_varian(&1.5f32.to_be_bytes(), VarianSampleType::Float32),
            vec![1.5]
        );
    }
}
