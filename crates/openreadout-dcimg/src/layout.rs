//! DCIMG file structure: file header, session header, frame geometry, per-frame counters and
//! time stamps, and the stored values of the pixels the camera overwrites. Layout and
//! vocabulary: `docs/formats/dcimg.md`; provenance: `docs/provenance/dcimg.md`.
//!
//! Every offset read from the file is checked against the file length before it is used; a
//! structure that points outside the file is reported (`problems`) instead of trusted.

use openreadout_core::source::SourceFile as File;
use std::path::Path;

use openreadout_core::bytes::{Block, read_block};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// The eight bytes every DCIMG file starts with.
pub const DCIMG_MAGIC: &[u8; 8] = b"DCIMG\0\0\0";
/// Format version of files whose frames are stored back to back, with the frame counters,
/// time stamps and stored pixels in tables after the last frame.
pub const VERSION_PACKED: u32 = 7;
/// Format version of files in which every frame is followed by its own trailer.
pub const VERSION_FRAMED: u32 = 0x0100_0000;

/// Largest session header or footer we load (real ones are under 1 KiB).
const MAX_STRUCTURE_BYTES: u64 = 1 << 20;
/// Offset of the block table in a framed session header, and the base its offsets count from.
const FRAMED_TABLE_AT: u64 = 0xf0;
const FRAMED_BLOCK_BASE: u64 = 0xa0;
/// Size of the camera text block in a framed session header.
const CAMERA_BLOCK_BYTES: u32 = 0xf8;
/// Block kind that announces stored pixels in a framed session header.
const CORRECTION_BLOCK_KIND: u32 = 4;
/// Offset of the stored pixels inside a frame trailer.
const TRAILER_PIXELS_AT: u64 = 12;

/// How frames and their per-frame data are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameLayout {
    /// Version 7: frames back to back, per-frame data in tables after the last frame.
    Packed,
    /// Version 0x1000000: each frame followed by a trailer with its counter and time stamp.
    Framed,
}

/// The pixels of one row that the camera overwrites in every frame, and where their real
/// values are stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPixels {
    /// Row (0 = first stored row) whose first pixels are overwritten.
    pub row: u32,
    /// First overwritten column.
    pub column: u32,
    /// Number of overwritten pixels.
    pub count: u32,
    /// Packed: absolute offset of the table holding `count` values per frame. Framed: `None`
    /// (the values sit in each frame's trailer).
    pub table_offset: Option<u64>,
}

/// Text the framed session header records about the camera.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CameraText {
    /// Camera model number (`C11440-22C`, `C15440-20UP`).
    pub model: Option<String>,
    /// Serial number (the text after `S/N:`).
    pub serial: Option<String>,
    /// The other text fields, in file order (version strings; meaning not documented).
    pub versions: Vec<String>,
}

/// Something wrong with the structure, found while parsing (reported by `check`).
#[derive(Debug, Clone)]
pub struct Problem {
    /// Stable code: `truncated`, `bad_footer`, `bad_table`, ...
    pub code: &'static str,
    /// Plain-English description.
    pub detail: String,
    /// Byte offset the problem refers to.
    pub offset: Option<u64>,
}

/// Everything needed to read the frames of a DCIMG file.
#[derive(Debug, Clone)]
pub struct DcimgLayout {
    /// Format version (`VERSION_PACKED` or `VERSION_FRAMED`).
    pub version: u32,
    /// Frame layout that goes with the version.
    pub layout: FrameLayout,
    /// Number of sessions (always 1 in the corpus; only the first is read).
    pub session_count: u32,
    /// Size of the file header = offset of the session header.
    pub header_bytes: u64,
    /// File size the header declares.
    pub declared_size: u64,
    /// Actual file size.
    pub file_len: u64,
    /// Size of the session (header, frames and per-frame tables) the session header declares.
    pub session_bytes: u64,
    /// Number of frames (time points).
    pub frame_count: u32,
    /// Bytes per pixel (1 or 2).
    pub bytes_per_pixel: u32,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Bytes per stored row (at least `width × bytes_per_pixel`).
    pub row_bytes: u32,
    /// Bytes per stored frame (`row_bytes × height`).
    pub frame_bytes: u64,
    /// Absolute offset of frame 0.
    pub data_offset: u64,
    /// Distance between the starts of consecutive frames.
    pub frame_stride: u64,
    /// Framed: bytes of the trailer after each frame (0 for packed files).
    pub trailer_bytes: u64,
    /// Packed: absolute offset and size of the footer after the last frame.
    pub footer: Option<(u64, u64)>,
    /// Packed: absolute offset of the frame counter table (u32 per frame).
    pub counter_table: Option<u64>,
    /// Packed: absolute offset of the time stamp table (u32 seconds + u32 microseconds per frame).
    pub stamp_table: Option<u64>,
    /// The overwritten pixels and where their values are, when the file stores them.
    pub stored_pixels: Option<StoredPixels>,
    /// Camera text (framed files).
    pub camera: CameraText,
    /// Sensor sub-array `[x, width, y, height]` in sensor pixels (framed files).
    pub sub_array: Option<[u16; 4]>,
    /// Problems found while parsing.
    pub problems: Vec<Problem>,
}

/// One frame's counter and time stamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameStamp {
    /// The camera's frame counter.
    pub counter: u32,
    /// Seconds since 1970-01-01 UTC.
    pub seconds: u32,
    /// Microseconds within the second.
    pub micros: u32,
}

impl FrameStamp {
    /// Seconds since 1970-01-01 UTC as a float.
    pub fn unix_seconds(&self) -> f64 {
        f64::from(self.seconds) + f64::from(self.micros) / 1e6
    }
    /// ISO-8601 UTC with microseconds.
    pub fn iso8601(&self) -> String {
        let base = openreadout_core::time::unix_to_iso8601(i64::from(self.seconds), 0);
        // `YYYY-MM-DDTHH:MM:SS.000Z` → microseconds instead of milliseconds.
        let stem = base.strip_suffix(".000Z").unwrap_or(&base);
        format!("{stem}.{:06}Z", self.micros.min(999_999))
    }
}

/// Does this look like a DCIMG file?
pub fn looks_like_dcimg(head: &[u8]) -> bool {
    head.starts_with(DCIMG_MAGIC)
}

fn need<T>(v: Option<T>, what: &str, at: u64) -> Result<T> {
    v.ok_or_else(|| {
        Error::corrupt_at(
            FORMAT_ID,
            at,
            format!("{what} at offset {at} lies past the end of the file (truncated header)"),
        )
    })
}

impl DcimgLayout {
    /// Parse the file header, session header and (packed files) the footer.
    pub fn read(file: &mut File, path: &Path) -> Result<Self> {
        let file_len = file.metadata().map_err(|e| Error::io(path, e))?.len();
        let head = read_block(file, path, 0, 0x50, file_len)?;
        if head.slice(0, 8).is_none_or(|m| m != DCIMG_MAGIC) {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                0,
                "missing DCIMG signature (the file must start with `DCIMG` and three NUL bytes)",
            ));
        }
        let version = need(head.u32_at(8), "format version", 8)?;
        let layout = match version {
            VERSION_PACKED => FrameLayout::Packed,
            VERSION_FRAMED => FrameLayout::Framed,
            v => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("DCIMG format version {v:#x}"),
                    "Only versions 7 and 0x1000000 are known (docs/formats/dcimg.md); please share a sample file.",
                ));
            }
        };
        let session_count = need(head.u32_at(0x20), "session count", 0x20)?;
        let header_frames = need(head.u32_at(0x24), "frame count", 0x24)?;
        let header_bytes = u64::from(need(head.u32_at(0x28), "header size", 0x28)?);
        let declared_size = need(head.u64_at(0x30), "file size", 0x30)?;
        if !(0x30..=MAX_STRUCTURE_BYTES).contains(&header_bytes) {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                0x28,
                format!("implausible header size {header_bytes}"),
            ));
        }
        let mut problems = Vec::new();
        if session_count != 1 {
            problems.push(Problem {
                code: "sessions",
                detail: format!(
                    "the header declares {session_count} sessions; only the first is read"
                ),
                offset: Some(0x20),
            });
        }
        let s = header_bytes;
        let sess = read_block(file, path, s, 0x400, file_len)?;
        let mut me = match layout {
            FrameLayout::Packed => Self::packed_session(&sess, s)?,
            FrameLayout::Framed => Self::framed_session(&sess, s, &mut problems)?,
        };
        me.version = version;
        me.session_count = session_count;
        me.header_bytes = header_bytes;
        me.declared_size = declared_size;
        me.file_len = file_len;
        if me.frame_count != header_frames {
            problems.push(Problem {
                code: "frame_count",
                detail: format!(
                    "the file header declares {header_frames} frames, the session header {}; the session header is used",
                    me.frame_count
                ),
                offset: Some(0x24),
            });
        }
        me.problems = problems;
        me.validate_geometry()?;
        if layout == FrameLayout::Packed {
            me.read_footer(file, path)?;
        }
        Ok(me)
    }

    fn blank(layout: FrameLayout) -> Self {
        DcimgLayout {
            version: 0,
            layout,
            session_count: 0,
            header_bytes: 0,
            declared_size: 0,
            file_len: 0,
            session_bytes: 0,
            frame_count: 0,
            bytes_per_pixel: 0,
            width: 0,
            height: 0,
            row_bytes: 0,
            frame_bytes: 0,
            data_offset: 0,
            frame_stride: 0,
            trailer_bytes: 0,
            footer: None,
            counter_table: None,
            stamp_table: None,
            stored_pixels: None,
            camera: CameraText::default(),
            sub_array: None,
            problems: Vec::new(),
        }
    }

    /// Version 7 session header (see `docs/formats/dcimg.md` § Packed layout).
    fn packed_session(b: &Block, s: u64) -> Result<Self> {
        let mut me = Self::blank(FrameLayout::Packed);
        me.session_bytes = need(b.u64_at(s), "session size", s)?;
        me.frame_count = need(b.u32_at(s + 0x20), "frame count", s + 0x20)?;
        me.bytes_per_pixel = need(b.u32_at(s + 0x24), "bytes per pixel", s + 0x24)?;
        me.width = need(b.u32_at(s + 0x2c), "width", s + 0x2c)?;
        me.row_bytes = need(b.u32_at(s + 0x30), "bytes per row", s + 0x30)?;
        me.height = need(b.u32_at(s + 0x34), "height", s + 0x34)?;
        me.frame_bytes = u64::from(need(b.u32_at(s + 0x38), "bytes per frame", s + 0x38)?);
        let data = u64::from(need(b.u32_at(s + 0x44), "data offset", s + 0x44)?);
        let data_size = need(b.u64_at(s + 0x48), "session data size", s + 0x48)?;
        me.data_offset = s
            .checked_add(data)
            .ok_or_else(|| Error::corrupt_at(FORMAT_ID, s + 0x44, "data offset overflows"))?;
        me.frame_stride = me.frame_bytes;
        me.footer = s.checked_add(data_size).map(|f| (f, 0));
        Ok(me)
    }

    /// Version 0x1000000 session header (see `docs/formats/dcimg.md` § Framed layout).
    fn framed_session(b: &Block, s: u64, problems: &mut Vec<Problem>) -> Result<Self> {
        let mut me = Self::blank(FrameLayout::Framed);
        me.session_bytes = need(b.u64_at(s), "session size", s)?;
        me.frame_count = need(b.u32_at(s + 0x3c), "frame count", s + 0x3c)?;
        me.bytes_per_pixel = need(b.u32_at(s + 0x40), "bytes per pixel", s + 0x40)?;
        me.width = need(b.u32_at(s + 0x48), "width", s + 0x48)?;
        me.height = need(b.u32_at(s + 0x4c), "height", s + 0x4c)?;
        me.row_bytes = need(b.u32_at(s + 0x50), "bytes per row", s + 0x50)?;
        me.frame_bytes = u64::from(need(b.u32_at(s + 0x54), "bytes per frame", s + 0x54)?);
        let data = need(b.u64_at(s + 0x60), "data offset", s + 0x60)?;
        me.data_offset = s
            .checked_add(data)
            .ok_or_else(|| Error::corrupt_at(FORMAT_ID, s + 0x60, "data offset overflows"))?;
        let stride = u64::from(b.u32_at(s + 0x74).unwrap_or(0));
        let trailer = u64::from(b.u32_at(s + 0x7c).unwrap_or(0));
        if stride == me.frame_bytes.saturating_add(trailer) && (12..=4096).contains(&trailer) {
            me.frame_stride = stride;
            me.trailer_bytes = trailer;
        } else {
            // The frame stride and trailer size are not where two corpus variants put them:
            // assume the 32-byte trailer the `dcimg` package documents.
            me.trailer_bytes = 32;
            me.frame_stride = me.frame_bytes.saturating_add(32);
            problems.push(Problem {
                code: "trailer",
                detail: format!(
                    "frame stride {stride} and trailer size {trailer} disagree with {} bytes per frame; assuming a 32-byte trailer",
                    me.frame_bytes
                ),
                offset: Some(s + 0x74),
            });
        }
        me.read_blocks(b, s, problems);
        Ok(me)
    }

    /// The block table of a framed session header: camera text, sub-array, stored pixels.
    fn read_blocks(&mut self, b: &Block, s: u64, problems: &mut Vec<Problem>) {
        let base = s + FRAMED_BLOCK_BASE;
        let mut at = s + FRAMED_TABLE_AT;
        let mut first_block = u64::MAX;
        let mut entries = Vec::new();
        while at < first_block && entries.len() < 32 {
            let (Some(kind), Some(size), Some(off)) =
                (b.u32_at(at), b.u32_at(at + 4), b.u64_at(at + 8))
            else {
                break;
            };
            if kind != 0 {
                let start = base.saturating_add(off);
                if start.saturating_add(u64::from(size)) > b.end() || off == 0 {
                    problems.push(Problem {
                        code: "bad_table",
                        detail: format!(
                            "session block (kind {kind}, {size} bytes) at {start} lies outside the session header"
                        ),
                        offset: Some(at),
                    });
                    break;
                }
                first_block = first_block.min(start);
                entries.push((kind, size, start));
            }
            at += 16;
        }
        for &(kind, size, start) in &entries {
            if kind == CORRECTION_BLOCK_KIND && size >= 24 {
                let in_frame = b.u32_at(start + 16).map(u64::from);
                let bytes = b.u32_at(start + 12);
                if let (Some(in_frame), Some(bytes)) = (in_frame, bytes)
                    && self.row_bytes > 0
                    && self.bytes_per_pixel > 0
                    && self.trailer_bytes >= TRAILER_PIXELS_AT + u64::from(bytes)
                {
                    let row = in_frame / u64::from(self.row_bytes);
                    let col =
                        (in_frame % u64::from(self.row_bytes)) / u64::from(self.bytes_per_pixel);
                    let count = bytes / self.bytes_per_pixel;
                    if row < u64::from(self.height)
                        && col + u64::from(count) <= u64::from(self.width)
                        && count > 0
                    {
                        self.stored_pixels = Some(StoredPixels {
                            row: row as u32,
                            column: col as u32,
                            count,
                            table_offset: None,
                        });
                    }
                }
            }
            if size == CAMERA_BLOCK_BYTES {
                let text = |o: u64, n: usize| b.text_at(start + o, n).filter(|t| !t.is_empty());
                let mut versions = Vec::new();
                for (o, n) in [(0u64, 32usize), (0x20, 32), (0x80, 16)] {
                    if let Some(t) = text(o, n) {
                        versions.push(t);
                    }
                }
                self.camera = CameraText {
                    model: text(0x40, 64),
                    serial: text(0x90, 32).map(|t| {
                        t.strip_prefix("S/N:")
                            .map_or_else(|| t.clone(), |r| r.trim().to_string())
                    }),
                    versions,
                };
                let sub = [0u64, 2, 4, 6].map(|k| b.u16_at(start + 0xc8 + k).unwrap_or(0));
                if sub[1] > 0 && sub[3] > 0 {
                    self.sub_array = Some(sub);
                }
            }
        }
    }

    fn validate_geometry(&self) -> Result<()> {
        if !matches!(self.bytes_per_pixel, 1 | 2) {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("{} bytes per pixel", self.bytes_per_pixel),
                "Only 8- and 16-bit frames are known (docs/formats/dcimg.md).",
            ));
        }
        let min_row = u64::from(self.width) * u64::from(self.bytes_per_pixel);
        if self.width == 0
            || self.height == 0
            || u64::from(self.row_bytes) < min_row
            || self.frame_bytes != u64::from(self.row_bytes) * u64::from(self.height)
        {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "inconsistent frame geometry: {} × {} pixels of {} bytes, {} bytes per row, {} bytes per frame",
                    self.width, self.height, self.bytes_per_pixel, self.row_bytes, self.frame_bytes
                ),
            ));
        }
        Ok(())
    }

    /// Version 7 footer: frame counter, time stamp and stored-pixel tables.
    fn read_footer(&mut self, file: &mut File, path: &Path) -> Result<()> {
        let Some((f, _)) = self.footer else {
            return Ok(());
        };
        let blk = read_block(file, path, f, 0x100, self.file_len)?;
        let (Some(ver), Some(second), Some(size)) =
            (blk.u32_at(f), blk.u64_at(f + 8), blk.u32_at(f + 0x28))
        else {
            self.problems.push(Problem {
                code: "truncated",
                detail: format!(
                    "the footer after the last frame (offset {f}) is missing: frame counters, time stamps and stored pixels are unavailable"
                ),
                offset: Some(f),
            });
            self.footer = None;
            return Ok(());
        };
        let size = u64::from(size);
        if ver != VERSION_PACKED || second >= size || size > MAX_STRUCTURE_BYTES {
            self.problems.push(Problem {
                code: "bad_footer",
                detail: format!(
                    "footer at {f}: version {ver}, second structure at +{second}, size {size}: not a footer"
                ),
                offset: Some(f),
            });
            self.footer = None;
            return Ok(());
        }
        self.footer = Some((f, size));
        let s2 = f + second;
        let blk2 = read_block(file, path, s2, 0x70, self.file_len)?;
        let n = u64::from(self.frame_count);
        let table = |rel: Option<u64>, per_frame: u64| {
            rel.filter(|r| r.saturating_add(per_frame.saturating_mul(n)) <= size)
                .map(|r| f + r)
        };
        self.counter_table = table(blk2.u64_at(s2 + 0x30), 4);
        self.stamp_table = table(blk2.u64_at(s2 + 0x40), 8);
        let pix_rel = blk2.u64_at(s2 + 0x58);
        let in_frame = blk2.u32_at(s2 + 0x64).map(u64::from);
        let bytes = blk2.u64_at(s2 + 0x68);
        if let (Some(in_frame), Some(bytes)) = (in_frame, bytes)
            && bytes > 0
            && bytes <= 64
            && self.row_bytes > 0
        {
            let bpp = u64::from(self.bytes_per_pixel);
            let count = bytes / bpp;
            let row = in_frame / u64::from(self.row_bytes);
            let col = (in_frame % u64::from(self.row_bytes)) / bpp;
            if let Some(tab) = table(pix_rel, bytes)
                && row < u64::from(self.height)
                && col + count <= u64::from(self.width)
            {
                self.stored_pixels = Some(StoredPixels {
                    row: row as u32,
                    column: col as u32,
                    count: count as u32,
                    table_offset: Some(tab),
                });
            }
        }
        if f + size > self.file_len {
            self.problems.push(Problem {
                code: "truncated",
                detail: format!(
                    "the footer needs {size} bytes at {f} but the file ends at {}",
                    self.file_len
                ),
                offset: Some(self.file_len),
            });
        }
        Ok(())
    }

    /// Absolute offset of frame `t`.
    pub fn frame_offset(&self, t: u32) -> Option<u64> {
        u64::from(t)
            .checked_mul(self.frame_stride)?
            .checked_add(self.data_offset)
    }

    /// End of the last frame (and its trailer).
    pub fn frames_end(&self) -> Option<u64> {
        self.frame_offset(self.frame_count)
    }

    /// Frames that lie completely inside the file.
    pub fn complete_frames(&self) -> u32 {
        if self.frame_stride == 0 {
            return 0;
        }
        let avail = self.file_len.saturating_sub(self.data_offset);
        let whole = avail / self.frame_stride;
        // A last frame without its trailer still has its pixels.
        let extra = u64::from(avail % self.frame_stride >= self.frame_bytes);
        u32::try_from((whole + extra).min(u64::from(self.frame_count))).unwrap_or(u32::MAX)
    }

    /// Counters and time stamps of frames `first..first + n` (fewer when the file is cut).
    pub fn stamps(
        &self,
        file: &mut File,
        path: &Path,
        first: u32,
        n: u32,
    ) -> Result<Vec<FrameStamp>> {
        let end = first.saturating_add(n).min(self.frame_count);
        let mut out = Vec::new();
        match self.layout {
            FrameLayout::Packed => {
                let (Some(ct), Some(st)) = (self.counter_table, self.stamp_table) else {
                    return Ok(out);
                };
                let k = u64::from(end.saturating_sub(first));
                let counters =
                    read_block(file, path, ct + 4 * u64::from(first), 4 * k, self.file_len)?;
                let stamps =
                    read_block(file, path, st + 8 * u64::from(first), 8 * k, self.file_len)?;
                for t in first..end {
                    let i = u64::from(t - first);
                    let (Some(counter), Some(seconds), Some(micros)) = (
                        counters.u32_at(counters.origin + 4 * i),
                        stamps.u32_at(stamps.origin + 8 * i),
                        stamps.u32_at(stamps.origin + 8 * i + 4),
                    ) else {
                        break;
                    };
                    out.push(FrameStamp {
                        counter,
                        seconds,
                        micros,
                    });
                }
            }
            FrameLayout::Framed => {
                for t in first..end {
                    let Some(at) = self
                        .frame_offset(t)
                        .and_then(|o| o.checked_add(self.frame_bytes))
                    else {
                        break;
                    };
                    let b = read_block(file, path, at, 12, self.file_len)?;
                    let (Some(counter), Some(seconds), Some(micros)) =
                        (b.u32_at(at), b.u32_at(at + 4), b.u32_at(at + 8))
                    else {
                        break;
                    };
                    out.push(FrameStamp {
                        counter,
                        seconds,
                        micros,
                    });
                }
            }
        }
        Ok(out)
    }

    /// The stored values of the overwritten pixels of frame `t` (little-endian samples), or
    /// `None` when the file stores none or they lie past the end of the file.
    pub fn stored_values(&self, file: &mut File, path: &Path, t: u32) -> Result<Option<Vec<u8>>> {
        let Some(sp) = &self.stored_pixels else {
            return Ok(None);
        };
        let bytes = u64::from(sp.count) * u64::from(self.bytes_per_pixel);
        let at = match sp.table_offset {
            Some(tab) => u64::from(t)
                .checked_mul(bytes)
                .and_then(|o| o.checked_add(tab)),
            None => self
                .frame_offset(t)
                .and_then(|o| o.checked_add(self.frame_bytes + TRAILER_PIXELS_AT)),
        };
        let Some(at) = at else {
            return Ok(None);
        };
        let b = read_block(file, path, at, bytes, self.file_len)?;
        Ok((b.len() as u64 == bytes).then_some(b.bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_text() {
        let s = FrameStamp {
            counter: 0,
            seconds: 1_725_555_197,
            micros: 563_737,
        };
        assert_eq!(s.iso8601(), "2024-09-05T16:53:17.563737Z");
        assert!((s.unix_seconds() - 1_725_555_197.563_737).abs() < 1e-6);
    }

    #[test]
    fn magic() {
        assert!(looks_like_dcimg(b"DCIMG\0\0\0\x07\0\0\0"));
        assert!(!looks_like_dcimg(b"DCIMF\0\0\0"));
    }
}
