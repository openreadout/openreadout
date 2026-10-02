//! Segment chain, file header, subblock directory, attachments. See `docs/formats/czi.md`.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{le_f32, le_i32, le_u32, le_u64};
use openreadout_core::limits::{checked_metadata_len, remaining};
use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// Every segment starts with this many bytes.
pub const SEGMENT_HEADER_LEN: u64 = 32;

/// Known segment kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentId {
    File,
    Directory,
    SubBlock,
    Metadata,
    Attachment,
    AttachmentDirectory,
    Deleted,
    Unknown,
}

impl SegmentId {
    pub fn parse(raw: &[u8]) -> Self {
        let s = raw
            .iter()
            .take_while(|&&b| b != 0)
            .copied()
            .collect::<Vec<u8>>();
        match s.as_slice() {
            b"ZISRAWFILE" => SegmentId::File,
            b"ZISRAWDIRECTORY" => SegmentId::Directory,
            b"ZISRAWSUBBLOCK" => SegmentId::SubBlock,
            b"ZISRAWMETADATA" => SegmentId::Metadata,
            b"ZISRAWATTACH" => SegmentId::Attachment,
            b"ZISRAWATTDIR" => SegmentId::AttachmentDirectory,
            b"DELETED" => SegmentId::Deleted,
            _ => SegmentId::Unknown,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            SegmentId::File => "ZISRAWFILE",
            SegmentId::Directory => "ZISRAWDIRECTORY",
            SegmentId::SubBlock => "ZISRAWSUBBLOCK",
            SegmentId::Metadata => "ZISRAWMETADATA",
            SegmentId::Attachment => "ZISRAWATTACH",
            SegmentId::AttachmentDirectory => "ZISRAWATTDIR",
            SegmentId::Deleted => "DELETED",
            SegmentId::Unknown => "?",
        }
    }
}

/// The 32-byte header in front of every segment.
#[derive(Debug, Clone, Copy)]
pub struct SegmentHeader {
    pub segment_id: SegmentId,
    pub allocated_size: u64,
    pub used_size: u64,
}

/// `ZISRAWFILE` payload.
#[derive(Debug, Clone)]
pub struct FileHeader {
    pub version_major: u32,
    pub version_minor: u32,
    pub primary_file_guid: [u8; 16],
    pub file_guid: [u8; 16],
    pub file_part: u32,
    pub directory_position: u64,
    pub metadata_position: u64,
    pub update_pending: u32,
    pub attachment_directory_position: u64,
}

/// Pixel types from the directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelTypeId {
    Gray8,
    Gray16,
    Gray32Float,
    Bgr24,
    Bgr48,
    Bgr96Float,
    Bgra32,
    Gray64ComplexFloat,
    Bgr192ComplexFloat,
    Gray32,
    Gray64,
    Unknown(u32),
}

impl PixelTypeId {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => PixelTypeId::Gray8,
            1 => PixelTypeId::Gray16,
            2 => PixelTypeId::Gray32Float,
            3 => PixelTypeId::Bgr24,
            4 => PixelTypeId::Bgr48,
            8 => PixelTypeId::Bgr96Float,
            9 => PixelTypeId::Bgra32,
            10 => PixelTypeId::Gray64ComplexFloat,
            11 => PixelTypeId::Bgr192ComplexFloat,
            12 => PixelTypeId::Gray32,
            13 => PixelTypeId::Gray64,
            o => PixelTypeId::Unknown(o),
        }
    }
    pub fn name(self) -> String {
        match self {
            PixelTypeId::Gray8 => "gray8".into(),
            PixelTypeId::Gray16 => "gray16".into(),
            PixelTypeId::Gray32Float => "gray32_float".into(),
            PixelTypeId::Bgr24 => "bgr24".into(),
            PixelTypeId::Bgr48 => "bgr48".into(),
            PixelTypeId::Bgr96Float => "bgr96_float".into(),
            PixelTypeId::Bgra32 => "bgra32".into(),
            PixelTypeId::Gray64ComplexFloat => "gray64_complex_float".into(),
            PixelTypeId::Bgr192ComplexFloat => "bgr192_complex_float".into(),
            PixelTypeId::Gray32 => "gray32".into(),
            PixelTypeId::Gray64 => "gray64".into(),
            PixelTypeId::Unknown(v) => format!("unknown({v})"),
        }
    }
    /// Samples stored per pixel in the file (including alpha for `Bgra32`).
    pub fn samples_per_pixel(self) -> u32 {
        match self {
            PixelTypeId::Bgr24 | PixelTypeId::Bgr48 | PixelTypeId::Bgr96Float => 3,
            PixelTypeId::Bgra32 => 4,
            PixelTypeId::Gray64ComplexFloat => 2,
            PixelTypeId::Bgr192ComplexFloat => 6,
            _ => 1,
        }
    }
    /// Bytes per sample.
    pub fn bytes_per_sample(self) -> u32 {
        match self {
            PixelTypeId::Gray8 | PixelTypeId::Bgr24 | PixelTypeId::Bgra32 => 1,
            PixelTypeId::Gray16 | PixelTypeId::Bgr48 => 2,
            PixelTypeId::Gray32Float
            | PixelTypeId::Bgr96Float
            | PixelTypeId::Gray32
            | PixelTypeId::Gray64ComplexFloat
            | PixelTypeId::Bgr192ComplexFloat => 4,
            PixelTypeId::Gray64 => 8,
            PixelTypeId::Unknown(_) => 0,
        }
    }
    pub fn bytes_per_pixel(self) -> u32 {
        self.samples_per_pixel() * self.bytes_per_sample()
    }
}

/// Compression ids from the directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionId {
    Uncompressed,
    Jpeg,
    Lzw,
    JpegLossless,
    JpegXr,
    Zstd0,
    Zstd1,
    Chunked(u32),
    Unknown(u32),
}

impl CompressionId {
    pub fn from_raw(v: u32) -> Self {
        match v {
            0 => CompressionId::Uncompressed,
            1 => CompressionId::Jpeg,
            2 => CompressionId::Lzw,
            3 => CompressionId::JpegLossless,
            4 => CompressionId::JpegXr,
            5 => CompressionId::Zstd0,
            6 => CompressionId::Zstd1,
            v if v == 7 || v >= 1000 => CompressionId::Chunked(v),
            o => CompressionId::Unknown(o),
        }
    }
    pub fn name(self) -> String {
        match self {
            CompressionId::Uncompressed => "uncompressed".into(),
            CompressionId::Jpeg => "jpeg".into(),
            CompressionId::Lzw => "lzw".into(),
            CompressionId::JpegLossless => "jpeg_lossless".into(),
            CompressionId::JpegXr => "jpeg_xr".into(),
            CompressionId::Zstd0 => "zstd0".into(),
            CompressionId::Zstd1 => "zstd1".into(),
            CompressionId::Chunked(v) => format!("chunked({v})"),
            CompressionId::Unknown(v) => format!("unknown({v})"),
        }
    }
}

/// One dimension of a subblock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DimensionEntry {
    pub dimension: char,
    pub start: i32,
    pub size: u32,
    pub start_coordinate: f32,
    pub stored_size: u32,
}

/// One subblock directory entry.
#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub pixel_type: PixelTypeId,
    pub file_position: u64,
    pub file_part: u32,
    pub compression: CompressionId,
    pub pyramid_type: u8,
    pub dimensions: Vec<DimensionEntry>,
}

impl DirectoryEntry {
    pub fn dim(&self, d: char) -> Option<&DimensionEntry> {
        self.dimensions.iter().find(|e| e.dimension == d)
    }
    /// Index along a non-spatial dimension (0 when absent).
    pub fn index(&self, d: char) -> i32 {
        self.dim(d).map_or(0, |e| e.start)
    }
    /// True for full-resolution subblocks (stored extent not smaller than the logical extent).
    pub fn is_level0(&self) -> bool {
        self.pyramid_type == 0
            && self.dim('X').is_some_and(|x| x.stored_size >= x.size)
            && self.dim('Y').is_some_and(|y| y.stored_size >= y.size)
    }
    /// Stored/logical ratio on X (1 when the logical size is 0): above 1 for super-resolved
    /// renderings (PALM), below 1 for pyramid levels and Airyscan fast-scan subblocks.
    pub fn stored_ratio(&self) -> f64 {
        self.dim('X').map_or(1.0, |x| {
            if x.size == 0 {
                1.0
            } else {
                f64::from(x.stored_size) / f64::from(x.size)
            }
        })
    }
    /// Stored/logical ratio for subblocks rendered above their logical resolution (1 normally).
    pub fn upsample_factor(&self) -> i64 {
        self.dim('X').map_or(1, |x| {
            if x.size == 0 || x.stored_size <= x.size {
                1
            } else {
                i64::from(x.stored_size / x.size)
            }
        })
    }
    /// Does this subblock contain index `want` along dimension `d`? Absent dimensions cover 0.
    pub fn covers(&self, d: char, want: i32) -> bool {
        match self.dim(d) {
            Some(x) => {
                want >= x.start && i64::from(want) < i64::from(x.start) + i64::from(x.size.max(1))
            }
            None => want == 0,
        }
    }
    /// Downsampling factor (1 for level 0).
    pub fn scale(&self) -> u32 {
        self.dim('X')
            .map_or(1, |x| x.size.checked_div(x.stored_size).unwrap_or(1).max(1))
    }
    /// Byte length of this entry as stored.
    pub fn encoded_len(&self) -> usize {
        32 + 20 * self.dimensions.len()
    }
}

/// Header of a `ZISRAWSUBBLOCK` payload.
#[derive(Debug, Clone)]
pub struct SubBlockHeader {
    pub metadata_size: u32,
    pub attachment_size: u32,
    pub data_size: u64,
    pub entry: DirectoryEntry,
    /// Bytes from payload start to the subblock metadata.
    pub header_len: u64,
}

/// One attachment directory entry.
#[derive(Debug, Clone)]
pub struct AttachmentEntry {
    pub file_position: u64,
    pub file_part: u32,
    pub content_guid: [u8; 16],
    pub content_file_type: String,
    pub name: String,
    /// Payload size from the `ZISRAWATTACH` segment (0 when that segment could not be read).
    pub data_size: u64,
}

/// A file of a multi-file document (`file_part > 0`), found next to the master.
#[derive(Debug, Clone)]
pub struct FilePart {
    pub file_part: u32,
    /// Where we looked: `<master stem> (<file_part>).<ext>` in the master's directory.
    pub path: PathBuf,
    pub present: bool,
    pub file_len: u64,
    /// Why the part is unusable or suspicious (missing, wrong header, GUID mismatch).
    pub part_problem: Option<String>,
}

/// Bytes from the start of a `ZISRAWATTACH` segment to its payload.
pub const ATTACHMENT_DATA_OFFSET: u64 = SEGMENT_HEADER_LEN + 256;

/// A walked CZI container.
#[derive(Debug)]
pub struct CziFile {
    pub path: PathBuf,
    pub file_len: u64,
    pub header: FileHeader,
    pub xml: String,
    pub entries: Vec<DirectoryEntry>,
    pub attachments: Vec<AttachmentEntry>,
    /// Segment headers in file order (from the sequential walk).
    pub segments: Vec<(u64, SegmentHeader)>,
    /// Problems found while walking; `check` reports them.
    pub problems: Vec<(Option<u64>, String)>,
    /// Following parts referenced by directory or attachment entries (`file_part > 0`).
    pub parts: Vec<FilePart>,
}

impl CziFile {
    /// The file holding `file_part` (the master for 0), if it is present.
    pub fn part_file(&self, file_part: u32) -> Option<(&Path, u64)> {
        if file_part == 0 {
            return Some((&self.path, self.file_len));
        }
        self.parts
            .iter()
            .find(|p| p.file_part == file_part && p.present)
            .map(|p| (p.path.as_path(), p.file_len))
    }
}

/// Read `len` bytes at `off`. The length is checked against the file size before the
/// buffer is allocated, so a corrupt size field yields `corrupt_file`, not an OOM abort.
pub(crate) fn read_at(f: &mut SourceFile, path: &Path, off: u64, len: usize) -> Result<Vec<u8>> {
    let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
    if (len as u64) > remaining(file_len, off) {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!("file ends inside a {len}-byte read (truncated file)"),
        ));
    }
    let mut buf = vec![0u8; len];
    f.seek(SeekFrom::Start(off))
        .map_err(|e| Error::io(path, e))?;
    f.read_exact(&mut buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::corrupt_at(
                FORMAT_ID,
                off,
                format!("file ends inside a {len}-byte read (truncated file)"),
            )
        } else {
            Error::io(path, e)
        }
    })?;
    Ok(buf)
}

pub(crate) fn read_segment_header(
    f: &mut SourceFile,
    path: &Path,
    off: u64,
) -> Result<SegmentHeader> {
    let b = read_at(f, path, off, SEGMENT_HEADER_LEN as usize)?;
    Ok(SegmentHeader {
        segment_id: SegmentId::parse(&b[..16]),
        allocated_size: le_u64(&b, 16).unwrap_or(0),
        used_size: le_u64(&b, 24).unwrap_or(0),
    })
}

/// Parse one directory entry starting at `b[off..]`. Returns the entry and its length.
pub(crate) fn parse_entry(b: &[u8], off: usize) -> Result<(DirectoryEntry, usize)> {
    if off.checked_add(32).is_none_or(|e| b.len() < e) || &b[off..off + 2] != b"DV" {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("directory entry at +{off} does not start with schema 'DV'"),
        ));
    }
    let dimension_count = le_u32(b, off + 28).unwrap_or(0) as usize;
    if dimension_count > 32 {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("directory entry claims {dimension_count} dimensions"),
        ));
    }
    let len = 32 + 20 * dimension_count;
    if b.len() - off < len {
        return Err(Error::corrupt(FORMAT_ID, "directory entry truncated"));
    }
    let mut dimensions = Vec::with_capacity(dimension_count);
    for k in 0..dimension_count {
        let o = off + 32 + 20 * k;
        dimensions.push(DimensionEntry {
            dimension: b[o] as char,
            start: le_i32(b, o + 4).unwrap_or(0),
            size: le_u32(b, o + 8).unwrap_or(0),
            start_coordinate: le_f32(b, o + 12).unwrap_or(0.0),
            stored_size: le_u32(b, o + 16).unwrap_or(0),
        });
    }
    Ok((
        DirectoryEntry {
            pixel_type: PixelTypeId::from_raw(le_u32(b, off + 2).unwrap_or(0)),
            file_position: le_u64(b, off + 6).unwrap_or(0),
            file_part: le_u32(b, off + 14).unwrap_or(0),
            compression: CompressionId::from_raw(le_u32(b, off + 18).unwrap_or(0)),
            pyramid_type: b[off + 22],
            dimensions,
        },
        len,
    ))
}

/// Read a subblock's header (not its data) at the given segment offset.
pub(crate) fn read_subblock_header(
    f: &mut SourceFile,
    path: &Path,
    seg_off: u64,
) -> Result<(SegmentHeader, SubBlockHeader)> {
    let sh = read_segment_header(f, path, seg_off)?;
    if sh.segment_id != SegmentId::SubBlock {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            seg_off,
            format!("expected ZISRAWSUBBLOCK, found {}", sh.segment_id.name()),
        ));
    }
    let b = read_at(
        f,
        path,
        seg_off.saturating_add(SEGMENT_HEADER_LEN),
        256.min(sh.used_size as usize).max(48),
    )?;
    Ok((sh, parse_subblock_header(&b)?))
}

/// Parse the fixed part of a `ZISRAWSUBBLOCK` payload (sizes + embedded directory entry).
pub(crate) fn parse_subblock_header(b: &[u8]) -> Result<SubBlockHeader> {
    let (entry, elen) = parse_entry(b, 16)?;
    let header_len = (16 + elen).max(256) as u64;
    Ok(SubBlockHeader {
        metadata_size: le_u32(b, 0).unwrap_or(0),
        attachment_size: le_u32(b, 4).unwrap_or(0),
        data_size: le_u64(b, 8).unwrap_or(0),
        entry,
        header_len,
    })
}

pub(crate) fn parse_attachment_entry(b: &[u8], off: usize) -> Option<AttachmentEntry> {
    if off.checked_add(128).is_none_or(|e| b.len() < e) || &b[off..off + 2] != b"A1" {
        return None;
    }
    let mut guid = [0u8; 16];
    guid.copy_from_slice(&b[off + 24..off + 40]);
    let ctype = String::from_utf8_lossy(&b[off + 40..off + 48])
        .trim_end_matches('\0')
        .to_string();
    let name = String::from_utf8_lossy(&b[off + 48..off + 128])
        .trim_end_matches('\0')
        .to_string();
    Some(AttachmentEntry {
        file_position: le_u64(b, off + 12).unwrap_or(0),
        file_part: le_u32(b, off + 20).unwrap_or(0),
        content_guid: guid,
        content_file_type: ctype,
        name,
        data_size: 0,
    })
}

/// Path of following part `k` of a multi-file document: `<stem> (k).<ext>` next to `master`.
pub fn part_path(master: &Path, k: u32) -> PathBuf {
    let stem = master
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().to_string());
    let name = match master.extension() {
        Some(ext) => format!("{stem} ({k}).{}", ext.to_string_lossy()),
        None => format!("{stem} ({k})"),
    };
    master.with_file_name(name)
}

/// `<stem>(k).<ext>` (no space): the other sibling spelling ZEN produces; tried second.
fn part_path_compact(master: &Path, k: u32) -> PathBuf {
    let stem = master
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().to_string());
    let name = match master.extension() {
        Some(ext) => format!("{stem}({k}).{}", ext.to_string_lossy()),
        None => format!("{stem}({k})"),
    };
    master.with_file_name(name)
}

/// For a file named `<stem> (<k>).<ext>` or `<stem>(<k>).<ext>`, the master's name `<stem>.<ext>`.
pub fn master_path(part: &Path) -> Option<PathBuf> {
    let stem = part.file_stem()?.to_string_lossy().to_string();
    let open = stem.rfind('(')?;
    let inner = stem.get(open + 1..)?.strip_suffix(')')?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let base = stem[..open].strip_suffix(' ').unwrap_or(&stem[..open]);
    if base.is_empty() {
        return None;
    }
    Some(match part.extension() {
        Some(ext) => part.with_file_name(format!("{base}.{}", ext.to_string_lossy())),
        None => part.with_file_name(base),
    })
}

/// Locate and validate following part `k` of the document whose master header is `master`.
fn probe_part(fs: &Fs, master_path_: &Path, master: &FileHeader, k: u32) -> FilePart {
    let spaced = part_path(master_path_, k);
    let compact = part_path_compact(master_path_, k);
    let path = if !fs.exists(&spaced) && fs.exists(&compact) {
        compact
    } else {
        spaced
    };
    let mut part = FilePart {
        file_part: k,
        path: path.clone(),
        present: false,
        file_len: 0,
        part_problem: None,
    };
    let Ok(mut f) = fs.open(&path) else {
        part.part_problem = Some(format!(
            "file part {k} is missing: expected `{}` next to the master file",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        return part;
    };
    part.file_len = f.metadata().map_or(0, |m| m.len());
    part.present = true;
    let header = read_segment_header(&mut f, &path, 0)
        .and_then(|sh| {
            if sh.segment_id == SegmentId::File {
                read_at(&mut f, &path, SEGMENT_HEADER_LEN, 80)
            } else {
                Err(Error::corrupt(FORMAT_ID, "not a CZI file"))
            }
        })
        .ok();
    match header {
        None => {
            part.part_problem = Some(format!(
                "`{}` does not start with a ZISRAWFILE segment",
                path.display()
            ));
        }
        Some(h) => {
            let got_part = le_u32(&h, 48).unwrap_or(0);
            let primary = &h[16..32];
            if got_part != k {
                part.part_problem = Some(format!(
                    "`{}` says it is file part {got_part}, expected {k}",
                    path.display()
                ));
            } else if primary != master.primary_file_guid {
                part.part_problem = Some(format!(
                    "`{}`: primary_file_guid does not match the master's (a part of another document?)",
                    path.display()
                ));
            }
        }
    }
    part
}

impl CziFile {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Open `path` in the namespace `fs` (following parts are looked up next to it there).
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let file_len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        let mut problems: Vec<(Option<u64>, String)> = Vec::new();

        let sh = read_segment_header(&mut f, path, 0)?;
        if sh.segment_id != SegmentId::File {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                0,
                "file does not start with a ZISRAWFILE segment",
            ));
        }
        let h = read_at(&mut f, path, SEGMENT_HEADER_LEN, 80)?;
        let mut pg = [0u8; 16];
        pg.copy_from_slice(&h[16..32]);
        let mut fg = [0u8; 16];
        fg.copy_from_slice(&h[32..48]);
        let header = FileHeader {
            version_major: le_u32(&h, 0).unwrap_or(0),
            version_minor: le_u32(&h, 4).unwrap_or(0),
            primary_file_guid: pg,
            file_guid: fg,
            file_part: le_u32(&h, 48).unwrap_or(0),
            directory_position: le_u64(&h, 52).unwrap_or(0),
            metadata_position: le_u64(&h, 60).unwrap_or(0),
            update_pending: le_u32(&h, 68).unwrap_or(0),
            attachment_directory_position: le_u64(&h, 72).unwrap_or(0),
        };
        if header.file_part > 0 {
            let master = master_path(path).map_or_else(
                || "the master file (file_part 0)".to_string(),
                |m| format!("`{}`", m.display()),
            );
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(
                    "this file is following part {} of a multi-file CZI document",
                    header.file_part
                ),
                format!(
                    "Open {master} instead; its directory lists the subblocks stored in this part."
                ),
            ));
        }
        if header.update_pending != 0 {
            problems.push((
                Some(SEGMENT_HEADER_LEN + 68),
                "update_pending flag set: the writer did not finish this file".into(),
            ));
        }

        // Metadata XML.
        let mut xml = String::new();
        if header.metadata_position != 0
            && header.metadata_position.saturating_add(SEGMENT_HEADER_LEN) <= file_len
        {
            let msh = read_segment_header(&mut f, path, header.metadata_position)?;
            if msh.segment_id == SegmentId::Metadata {
                let mh = read_at(
                    &mut f,
                    path,
                    header.metadata_position + SEGMENT_HEADER_LEN,
                    8,
                )?;
                let xml_size = u64::from(le_u32(&mh, 0).unwrap_or(0));
                let xoff = header.metadata_position + SEGMENT_HEADER_LEN + 256;
                if xoff.saturating_add(xml_size) <= file_len {
                    let xml_size = checked_metadata_len(
                        FORMAT_ID,
                        xml_size,
                        remaining(file_len, xoff),
                        "metadata XML",
                    )?;
                    let xb = read_at(&mut f, path, xoff, xml_size)?;
                    xml = String::from_utf8_lossy(&xb)
                        .trim_end_matches('\0')
                        .to_string();
                } else {
                    problems.push((Some(xoff), "metadata XML extends past end of file".into()));
                }
            } else {
                problems.push((
                    Some(header.metadata_position),
                    format!(
                        "metadata_position points at {} not ZISRAWMETADATA",
                        msh.segment_id.name()
                    ),
                ));
            }
        } else if header.metadata_position != 0 {
            problems.push((
                Some(header.metadata_position),
                "metadata_position is beyond end of file".into(),
            ));
        }

        // Subblock directory.
        let mut entries = Vec::new();
        let mut dir_ok = false;
        if header.directory_position != 0
            && header.directory_position.saturating_add(SEGMENT_HEADER_LEN) <= file_len
        {
            let dsh = read_segment_header(&mut f, path, header.directory_position)?;
            let dir_end =
                (header.directory_position + SEGMENT_HEADER_LEN).saturating_add(dsh.used_size);
            if dsh.segment_id == SegmentId::Directory && (dsh.used_size < 128 || dir_end > file_len)
            {
                problems.push((
                    Some(header.directory_position),
                    format!(
                        "subblock directory used_size {} does not fit the file (truncated file)",
                        dsh.used_size
                    ),
                ));
            } else if dsh.segment_id == SegmentId::Directory {
                let doff = header.directory_position + SEGMENT_HEADER_LEN;
                let dlen = checked_metadata_len(
                    FORMAT_ID,
                    dsh.used_size,
                    remaining(file_len, doff),
                    "subblock directory",
                )?;
                let d = read_at(&mut f, path, doff, dlen)?;
                let count = le_u32(&d, 0).unwrap_or(0) as usize;
                let mut off = 128;
                for i in 0..count {
                    match parse_entry(&d, off) {
                        Ok((e, len)) => {
                            entries.push(e);
                            off += len;
                        }
                        Err(e) => {
                            problems.push((
                                Some(header.directory_position),
                                format!("directory entry {i}: {e}"),
                            ));
                            break;
                        }
                    }
                }
                dir_ok = entries.len() == count;
            } else {
                problems.push((
                    Some(header.directory_position),
                    format!(
                        "directory_position points at {} not ZISRAWDIRECTORY",
                        dsh.segment_id.name()
                    ),
                ));
            }
        } else if header.directory_position != 0 {
            problems.push((
                Some(header.directory_position),
                "directory_position is beyond end of file".into(),
            ));
        }

        // Sequential segment walk (cheap: one 32-byte read per segment). Also the fallback
        // directory when the file was not finalized.
        let mut segments = Vec::new();
        let mut off = 0u64;
        let mut scanned = Vec::new();
        while off + SEGMENT_HEADER_LEN <= file_len {
            let s = match read_segment_header(&mut f, path, off) {
                Ok(s) => s,
                Err(_) => break,
            };
            if s.segment_id == SegmentId::Unknown {
                problems.push((Some(off), "unknown segment id; walk stopped".into()));
                break;
            }
            let end = (off + SEGMENT_HEADER_LEN).saturating_add(s.allocated_size);
            if s.used_size > s.allocated_size {
                problems.push((
                    Some(off),
                    format!(
                        "{}: used_size {} > allocated_size {}",
                        s.segment_id.name(),
                        s.used_size,
                        s.allocated_size
                    ),
                ));
            }
            if end > file_len {
                problems.push((
                    Some(off),
                    format!(
                        "{} segment extends past end of file by {} bytes (truncated file)",
                        s.segment_id.name(),
                        end - file_len
                    ),
                ));
                segments.push((off, s));
                break;
            }
            if s.segment_id == SegmentId::SubBlock
                && !dir_ok
                && let Ok((_, sb)) = read_subblock_header(&mut f, path, off)
            {
                scanned.push(sb.entry);
            }
            segments.push((off, s));
            if s.allocated_size == 0 {
                problems.push((Some(off), "zero-length segment; walk stopped".into()));
                break;
            }
            off = end;
        }
        if !dir_ok {
            if scanned.is_empty() {
                problems.push((
                    header.directory_position.into(),
                    "no usable subblock directory".into(),
                ));
            } else {
                problems.push((
                    None,
                    format!(
                        "subblock directory unusable; recovered {} subblocks by scanning",
                        scanned.len()
                    ),
                ));
                entries = scanned;
            }
        }

        // Attachment directory.
        let mut attachments = Vec::new();
        if header.attachment_directory_position != 0
            && header
                .attachment_directory_position
                .saturating_add(SEGMENT_HEADER_LEN)
                <= file_len
        {
            let ash = read_segment_header(&mut f, path, header.attachment_directory_position)?;
            let att_end = (header.attachment_directory_position + SEGMENT_HEADER_LEN)
                .saturating_add(ash.used_size);
            if ash.segment_id == SegmentId::AttachmentDirectory
                && (ash.used_size < 4 || att_end > file_len)
            {
                problems.push((
                    Some(header.attachment_directory_position),
                    format!(
                        "attachment directory used_size {} does not fit the file (truncated file)",
                        ash.used_size
                    ),
                ));
            } else if ash.segment_id == SegmentId::AttachmentDirectory {
                let aoff = header.attachment_directory_position + SEGMENT_HEADER_LEN;
                let alen = checked_metadata_len(
                    FORMAT_ID,
                    ash.used_size,
                    remaining(file_len, aoff),
                    "attachment directory",
                )?;
                let d = read_at(&mut f, path, aoff, alen)?;
                let count = le_u32(&d, 0).unwrap_or(0) as usize;
                for i in 0..count {
                    let at = 128usize.saturating_mul(i).saturating_add(256);
                    if let Some(a) = parse_attachment_entry(&d, at) {
                        attachments.push(a);
                    } else {
                        problems.push((
                            Some(header.attachment_directory_position),
                            format!("attachment entry {i} malformed"),
                        ));
                        break;
                    }
                }
            }
        }

        // Following parts of a multi-file document: every file_part > 0 that an entry names.
        let referenced: std::collections::BTreeSet<u32> = entries
            .iter()
            .map(|e| e.file_part)
            .chain(attachments.iter().map(|a| a.file_part))
            .filter(|&k| k > 0)
            .collect();
        let parts: Vec<FilePart> = referenced
            .into_iter()
            .map(|k| probe_part(fs, path, &header, k))
            .collect();
        for p in &parts {
            if let Some(why) = &p.part_problem {
                problems.push((None, why.clone()));
            }
        }

        // Attachment payload sizes (one 36-byte read each, in whichever part holds it).
        for a in &mut attachments {
            let located = if a.file_part == 0 {
                Some((path.to_path_buf(), file_len))
            } else {
                parts
                    .iter()
                    .find(|p| p.file_part == a.file_part && p.present)
                    .map(|p| (p.path.clone(), p.file_len))
            };
            let Some((ppath, plen)) = located else {
                continue;
            };
            if a.file_position.saturating_add(ATTACHMENT_DATA_OFFSET) > plen {
                problems.push((
                    Some(a.file_position),
                    format!("attachment `{}` points past the end of its file", a.name),
                ));
                continue;
            }
            let sized = fs
                .open(&ppath)
                .map_err(|e| Error::io(&ppath, e))
                .and_then(|mut pf| {
                    let sh = read_segment_header(&mut pf, &ppath, a.file_position)?;
                    let b = read_at(&mut pf, &ppath, a.file_position + SEGMENT_HEADER_LEN, 4)?;
                    Ok((sh, u64::from(le_u32(&b, 0).unwrap_or(0))))
                });
            match sized {
                Ok((sh, n)) if sh.segment_id == SegmentId::Attachment => {
                    if a.file_position
                        .saturating_add(ATTACHMENT_DATA_OFFSET)
                        .saturating_add(n)
                        > plen
                    {
                        problems.push((
                            Some(a.file_position),
                            format!(
                                "attachment `{}` data extends past end of file (truncated file)",
                                a.name
                            ),
                        ));
                    }
                    a.data_size = n;
                }
                Ok((sh, _)) => problems.push((
                    Some(a.file_position),
                    format!(
                        "attachment `{}` points at {} not ZISRAWATTACH",
                        a.name,
                        sh.segment_id.name()
                    ),
                )),
                Err(e) => problems.push((
                    Some(a.file_position),
                    format!("attachment `{}`: {e}", a.name),
                )),
            }
        }

        Ok(CziFile {
            path: path.to_path_buf(),
            file_len,
            header,
            xml,
            entries,
            attachments,
            segments,
            problems,
            parts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_and_master_names() {
        let m = Path::new("/data/run 7.czi");
        assert_eq!(part_path(m, 2), Path::new("/data/run 7 (2).czi"));
        assert_eq!(master_path(&part_path(m, 12)).as_deref(), Some(m));
        assert_eq!(master_path(Path::new("/data/run.czi")), None);
        assert_eq!(master_path(Path::new("/data/run (x).czi")), None);
        assert_eq!(master_path(Path::new("/data/run ().czi")), None);
        assert_eq!(
            master_path(Path::new("/data/Image_1(3).czi")).as_deref(),
            Some(Path::new("/data/Image_1.czi"))
        );
        assert_eq!(master_path(Path::new("/data/(3).czi")), None);
        assert_eq!(part_path_compact(m, 1), Path::new("/data/run 7(1).czi"));
    }

    #[test]
    fn directory_entry_parsing_rejects_garbage() {
        assert!(parse_entry(&[0u8; 10], 0).is_err());
        let mut b = vec![0u8; 32];
        b[..2].copy_from_slice(b"DV");
        b[28] = 200; // dimension_count far beyond the buffer
        assert!(parse_entry(&b, 0).is_err());
        b[28] = 0;
        let (e, len) = parse_entry(&b, 0).unwrap();
        assert_eq!((len, e.dimensions.len()), (32, 0));
    }
}
