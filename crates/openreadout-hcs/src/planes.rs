//! Reading the plates' plane files: plain TIFF pages, decoded by the TIFF crate
//! (`docs/formats/tiff.md`). One file is opened per read; nothing is kept open, so a plate of
//! 100,000 files costs nothing until its planes are read.

use std::path::Path;

use openreadout_core::{Error, Fs, PixelType, Plane, Result};
use openreadout_tiff::{PageLayout, SampleSelect, TiffFile, read_page};

/// Geometry of one page of a plane file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageShape {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Sample type.
    pub pixel_type: PixelType,
    /// Samples per pixel (1 for grayscale).
    pub samples_per_pixel: u32,
    /// Pages in the file.
    pub pages: u32,
}

/// Open `path` and describe page `page` (headers only).
pub fn page_shape(fs: &Fs, path: &Path, page: u32) -> Result<PageShape> {
    let (tf, _) = TiffFile::open_in(fs, path)?;
    let ifd = tf.ifds.get(page as usize).ok_or_else(|| {
        Error::corrupt(
            "tiff",
            format!(
                "{} has {} page(s); page {page} is expected",
                path.display(),
                tf.ifds.len()
            ),
        )
    })?;
    let layout = PageLayout::from_ifd(ifd, tf.header.byte_order)?;
    Ok(PageShape {
        width: layout.width,
        height: layout.height,
        pixel_type: layout.pixel_type()?,
        samples_per_pixel: u32::from(layout.samples_per_pixel),
        pages: tf.ifds.len() as u32,
    })
}

/// Structural problems of one plane file (headers and strip/tile offsets only): an empty list
/// when the page is readable as `(width, height, pixel_type)`.
pub fn check_file(
    fs: &Fs,
    path: &Path,
    page: u32,
    want: Option<(u32, u32, PixelType)>,
) -> Vec<String> {
    let mut out = Vec::new();
    let (tf, _) = match TiffFile::open_in(fs, path) {
        Ok(v) => v,
        Err(e) => {
            out.push(format!("cannot be read as TIFF: {e}"));
            return out;
        }
    };
    for p in &tf.problems {
        out.push(format!("{} at offset {}: {}", p.code, p.offset, p.detail));
    }
    let Some(ifd) = tf.ifds.get(page as usize) else {
        out.push(format!(
            "has {} page(s); page {page} is expected",
            tf.ifds.len()
        ));
        return out;
    };
    let layout = match PageLayout::from_ifd(ifd, tf.header.byte_order) {
        Ok(l) => l,
        Err(e) => {
            out.push(format!("page {page}: {e}"));
            return out;
        }
    };
    if let Some((w, h, pt)) = want {
        let got = layout.pixel_type().ok();
        if layout.width != w || layout.height != h || got != Some(pt) {
            out.push(format!(
                "is {}x{} {}, the plate index says {w}x{h} {}",
                layout.width,
                layout.height,
                got.map_or("(unsupported sample type)", |p| p.ome_name()),
                pt.ome_name()
            ));
        }
    }
    for (i, (&off, &n)) in layout.offsets.iter().zip(&layout.byte_counts).enumerate() {
        match off.checked_add(n) {
            Some(end) if end <= tf.file_len => {}
            _ => {
                out.push(format!(
                    "strip/tile {i} ({n} bytes at offset {off}) runs past the end of the file ({} bytes): truncated",
                    tf.file_len
                ));
                break;
            }
        }
    }
    if (layout.offsets.len() as u64) < layout.expected_chunks() {
        out.push(format!(
            "page {page} has {} strips/tiles, {} expected",
            layout.offsets.len(),
            layout.expected_chunks()
        ));
    }
    out
}

/// Read page `page` of `path`: all samples, little-endian.
pub fn read_file_plane(fs: &Fs, path: &Path, page: u32) -> Result<Plane> {
    let (tf, mut src) = TiffFile::open_in(fs, path)?;
    let ifd = tf.ifds.get(page as usize).ok_or_else(|| {
        Error::corrupt(
            "tiff",
            format!(
                "{} has {} page(s); page {page} is expected",
                path.display(),
                tf.ifds.len()
            ),
        )
    })?;
    let layout = PageLayout::from_ifd(ifd, tf.header.byte_order)?;
    let data = read_page(&mut src, &layout, SampleSelect::All)?;
    Ok(Plane {
        width: layout.width,
        height: layout.height,
        pixel_type: layout.pixel_type()?,
        samples_per_pixel: u32::from(layout.samples_per_pixel),
        data,
    })
}

/// The `ImageDescription` text and the `(ifd, byte source)` of page 0, for readers that
/// decode MetaMorph metadata from plane files.
pub fn open_first_page(fs: &Fs, path: &Path) -> Result<(TiffFile, openreadout_tiff::ByteSource)> {
    TiffFile::open_in(fs, path)
}

/// A blank plane (planes the instrument never acquired or recorded).
pub fn blank_plane(width: u32, height: u32, pixel_type: PixelType) -> Result<Plane> {
    let n = openreadout_core::pixel::plane_bytes_checked(
        "hcs",
        width,
        height,
        pixel_type.bytes_per_sample(),
    )?;
    Ok(Plane {
        width,
        height,
        pixel_type,
        samples_per_pixel: 1,
        data: vec![0u8; n],
    })
}
