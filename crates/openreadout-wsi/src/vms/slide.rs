//! The slide model of a VMS data set: the settings, the grid of slide images with their JPEG
//! layouts, the map, the levels and where each MCU row starts. `docs/formats/hamamatsu-vms.md`.

use std::path::{Path, PathBuf};

use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use super::jpeg::JpegLayout;
use super::{FORMAT_ID, VMS_SECTION, VMU_SECTION};
use crate::mirax::ini::{Ini, IniSection, decode_text};

/// Largest settings file read.
const MAX_SETTINGS: u64 = 1 << 20;
/// Bytes read to find a JPEG header.
const HEADER_BYTES: u64 = 1 << 16;
/// Bytes of one optimisation-file entry.
pub const OPT_ENTRY: usize = 40;
/// Bytes scanned at a time when looking for restart markers.
const SCAN_CHUNK: u64 = 4 << 20;
/// Largest grid accepted.
const MAX_GRID: u32 = 64;

/// One JPEG file of the slide.
#[derive(Debug, Clone)]
pub struct VmsImage {
    /// The file.
    pub path: PathBuf,
    /// Its size in bytes.
    pub len: u64,
    /// Its header.
    pub layout: JpegLayout,
    /// Column and row on the image grid (0, 0 for the map).
    pub cell: (u32, u32),
    /// Position of its first pixel in the full-resolution image (0, 0 for the map).
    pub origin: (u64, u64),
}

/// Where a pyramid level's pixels come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelSource {
    /// The slide images.
    Slide,
    /// The map image.
    Map,
}

/// One pyramid level: a source decoded at `1 / 2^shift` size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmsLevel {
    /// Which images.
    pub source: LevelSource,
    /// DCT reduction: 0 = stored size, 1 = half, 2 = quarter, 3 = eighth.
    pub shift: u32,
    /// Size in pixels.
    pub size: (u64, u64),
    /// Downsampling relative to level 0.
    pub downsample: u64,
}

/// An opened VMS slide.
#[derive(Debug, Clone)]
pub struct VmsSlide {
    /// The `.vms` file.
    pub path: PathBuf,
    /// Its folder.
    pub dir: PathBuf,
    /// The settings.
    pub ini: Ini,
    /// Grid columns and rows (`NoJpegColumns`, `NoJpegRows`).
    pub grid: (u32, u32),
    /// The slide images in raster order of the grid.
    pub images: Vec<VmsImage>,
    /// The map image (`MapFile`), when present and readable.
    pub map: Option<VmsImage>,
    /// Downsampling of the map relative to level 0, when it is a power of two.
    pub map_downsample: Option<u64>,
    /// The pyramid.
    pub levels: Vec<VmsLevel>,
    /// Full-resolution size.
    pub size: (u64, u64),
    /// MCU-row start offsets per slide image from the optimisation file (may be short).
    pub opt_rows: Vec<Vec<u64>>,
    /// Problems found while opening (for `check`).
    pub problems: Vec<String>,
}

fn bad(detail: impl Into<String>) -> Error {
    Error::corrupt(FORMAT_ID, detail.into())
}

/// The cell named by an `ImageFile(…)` key: `(x,y)` or `(z,x,y)`, with z the focal layer.
pub fn parse_cell(key: &str) -> Option<(u32, u32, u32)> {
    let inner = key.strip_prefix("ImageFile(")?.strip_suffix(')')?;
    let v: Vec<u32> = inner
        .split(',')
        .map(|t| t.trim().parse().ok())
        .collect::<Option<Vec<u32>>>()?;
    match v.as_slice() {
        [x, y] => Some((0, *x, *y)),
        [z, x, y] => Some((*z, *x, *y)),
        _ => None,
    }
}

fn open_image(fs: &Fs, path: &Path, cell: (u32, u32)) -> Result<VmsImage> {
    let f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = f.size().map_err(|e| Error::io(path, e))?;
    let mut head = vec![0u8; len.min(HEADER_BYTES) as usize];
    f.read_exact_at(0, &mut head)
        .map_err(|e| Error::io(path, e))?;
    let layout = JpegLayout::parse(&head)?;
    Ok(VmsImage {
        path: path.to_path_buf(),
        len,
        layout,
        cell,
        origin: (0, 0),
    })
}

impl VmsSlide {
    /// The settings section.
    pub fn settings(&self) -> Option<&IniSection> {
        self.ini.section(VMS_SECTION)
    }

    /// A setting.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.settings()?.get(key)
    }

    /// Open the slide whose settings file is `path`.
    pub fn open(fs: &Fs, path: &Path) -> Result<VmsSlide> {
        let meta = fs.metadata(path).map_err(|e| Error::io(path, e))?;
        if meta.len() > MAX_SETTINGS {
            return Err(bad(format!("settings file of {} bytes", meta.len())));
        }
        let raw = fs.read(path).map_err(|e| Error::io(path, e))?;
        let ini = Ini::parse(&decode_text(&raw));
        if ini.section(VMU_SECTION).is_some() {
            return Err(Error::unsupported(
                FORMAT_ID,
                "a VMU slide (uncompressed NGR images)",
                "Only VMS slides (JPEG images) are read; no public VMU file has been available to validate NGR decoding.",
            ));
        }
        let sec = ini
            .section(VMS_SECTION)
            .ok_or_else(|| bad(format!("no [{VMS_SECTION}] section")))?;
        let layers = sec.i64("NoLayers").unwrap_or(1);
        if layers != 1 {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("a VMS slide with {layers} focal layers"),
                "Only single-layer slides are read (NoLayers = 1); no public multi-layer sample exists yet.",
            ));
        }
        let cols = sec.i64("NoJpegColumns").unwrap_or(1);
        let rows = sec.i64("NoJpegRows").unwrap_or(1);
        if !(1..=i64::from(MAX_GRID)).contains(&cols) || !(1..=i64::from(MAX_GRID)).contains(&rows)
        {
            return Err(bad(format!("image grid of {cols} x {rows}")));
        }
        let (cols, rows) = (cols as u32, rows as u32);
        let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let mut names: Vec<Option<String>> = vec![None; (cols * rows) as usize];
        for (k, v) in &sec.entries {
            let cell = if k == "ImageFile" {
                Some((0, 0, 0))
            } else {
                parse_cell(k)
            };
            if let Some((z, x, y)) = cell {
                if z != 0 {
                    continue;
                }
                if x >= cols || y >= rows {
                    return Err(bad(format!("{k} lies outside the {cols} x {rows} grid")));
                }
                names[(y * cols + x) as usize] = Some(v.clone());
            }
        }
        let mut images = Vec::with_capacity(names.len());
        for (i, n) in names.iter().enumerate() {
            let (x, y) = (i as u32 % cols, i as u32 / cols);
            let n = n
                .as_ref()
                .ok_or_else(|| bad(format!("no ImageFile for grid cell ({x},{y})")))?;
            images.push(open_image(fs, &dir.join(n), (x, y))?);
        }
        // Columns share widths, rows share heights.
        let widths: Vec<u32> = (0..cols).map(|x| images[x as usize].layout.width).collect();
        let heights: Vec<u32> = (0..rows)
            .map(|y| images[(y * cols) as usize].layout.height)
            .collect();
        let width_of = |x: u32| widths[x as usize];
        let height_of = |y: u32| heights[y as usize];
        let mut problems = Vec::new();
        for im in &images {
            let (x, y) = im.cell;
            if im.layout.width != width_of(x) || im.layout.height != height_of(y) {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!(
                        "image ({x},{y}) is {} x {}, its column is {} wide and its row {} high",
                        im.layout.width,
                        im.layout.height,
                        width_of(x),
                        height_of(y)
                    ),
                    "Only grids whose columns share a width and rows share a height are read.",
                ));
            }
            if im.layout.restart == 0 {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "a slide image without restart intervals",
                    "Slide images are read by restart interval; this one has none.",
                ));
            }
            if im.layout.mcu_w != 8 || im.layout.mcu_h != 8 {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "a slide image with chroma subsampling",
                    "Only JPEG images without chroma subsampling (8 × 8 MCUs) are validated in VMS slides.",
                ));
            }
        }
        let mut ox = 0u64;
        let mut col_x = Vec::new();
        for x in 0..cols {
            col_x.push(ox);
            ox += u64::from(width_of(x));
        }
        let mut oy = 0u64;
        let mut row_y = Vec::new();
        for y in 0..rows {
            row_y.push(oy);
            oy += u64::from(height_of(y));
        }
        for im in &mut images {
            im.origin = (col_x[im.cell.0 as usize], row_y[im.cell.1 as usize]);
        }
        let size = (ox, oy);
        // The map: a power-of-two reduction of the whole slide.
        let mut map = None;
        let mut map_downsample = None;
        if let Some(n) = sec.get("MapFile") {
            match open_image(fs, &dir.join(n), (0, 0)) {
                Ok(m) => {
                    let (mw, mh) = (u64::from(m.layout.width), u64::from(m.layout.height));
                    let ds = (1..=6)
                        .map(|k| 1u64 << k)
                        .find(|d| mw * d == size.0 && mh * d == size.1);
                    if ds.is_none() {
                        problems.push(format!(
                            "the map ({mw} x {mh}) is not a power-of-two reduction of the slide ({} x {}); it is not used as a level",
                            size.0, size.1
                        ));
                    }
                    if m.layout.mcu_w != 8 || m.layout.mcu_h != 8 {
                        problems.push("the map uses chroma subsampling; it is not used as a level".into());
                    } else {
                        map_downsample = ds;
                    }
                    map = Some(m);
                }
                Err(e) => problems.push(format!("map file {n}: {e}")),
            }
        }
        let mut levels = Vec::new();
        let main_shifts = match map_downsample {
            Some(d) => d.trailing_zeros().min(4),
            None => 4,
        };
        for s in 0..main_shifts {
            let w: u64 = (0..cols).map(|x| u64::from(width_of(x)).div_ceil(1 << s)).sum();
            let h: u64 = (0..rows).map(|y| u64::from(height_of(y)).div_ceil(1 << s)).sum();
            levels.push(VmsLevel {
                source: LevelSource::Slide,
                shift: s,
                size: (w, h),
                downsample: 1 << s,
            });
        }
        if let (Some(_), Some(d)) = (&map, map_downsample) {
            // Sizes as OpenSlide gives them: level 0 divided by the downsampling, rounded down
            // (the reduced decode is cropped to it).
            for s in 0..4u32 {
                let ds = d << s;
                let (w, h) = (size.0 / ds, size.1 / ds);
                if w == 0 || h == 0 {
                    break;
                }
                levels.push(VmsLevel {
                    source: LevelSource::Map,
                    shift: s,
                    size: (w, h),
                    downsample: ds,
                });
            }
        }
        let mut slide = VmsSlide {
            path: path.to_path_buf(),
            dir,
            ini,
            grid: (cols, rows),
            images,
            map,
            map_downsample,
            levels,
            size,
            opt_rows: Vec::new(),
            problems,
        };
        slide.read_optimisation(fs);
        Ok(slide)
    }

    /// Read the optimisation file: one 40-byte entry per MCU row, the images one after the
    /// other in raster order of the grid, the last row of the last image missing. Rows that do
    /// not fit (wrong first offset, not increasing, past the file) are dropped and found by
    /// scanning instead.
    fn read_optimisation(&mut self, fs: &Fs) {
        self.opt_rows = vec![Vec::new(); self.images.len()];
        let Some(name) = self.get("OptimisationFile").map(str::to_string) else {
            return;
        };
        let p = self.dir.join(&name);
        let Ok(raw) = fs.read(&p) else {
            self.problems
                .push(format!("optimisation file {name} is missing or unreadable"));
            return;
        };
        let entries: Vec<u64> = raw
            .chunks_exact(OPT_ENTRY)
            .map(|c| u64::from_le_bytes(c[..8].try_into().unwrap_or([0; 8])))
            .collect();
        let mut at = 0usize;
        for (k, im) in self.images.iter().enumerate() {
            let rows = im.layout.rows() as usize;
            let take = entries.get(at..(at + rows).min(entries.len())).unwrap_or(&[]);
            at += rows;
            let mut ok = Vec::with_capacity(take.len());
            for &v in take {
                let prev = ok.last().copied().unwrap_or(0);
                if (ok.is_empty() && v != im.layout.scan_start) || (!ok.is_empty() && v <= prev) || v >= im.len {
                    break;
                }
                ok.push(v);
            }
            if ok.len() < take.len() {
                self.problems.push(format!(
                    "optimisation file: {} of {} row offsets of image {k} are usable",
                    ok.len(),
                    take.len()
                ));
            }
            self.opt_rows[k] = ok;
        }
    }

    /// Start offsets of every MCU row of slide image `k` (or the map, `k == None`): the
    /// optimisation file's, completed by scanning for restart markers.
    pub fn row_index(&self, file: &SourceFile, k: Option<usize>) -> Result<Vec<u64>> {
        let im = match k {
            Some(i) => &self.images[i],
            None => self
                .map
                .as_ref()
                .ok_or_else(|| bad("the slide has no map image"))?,
        };
        let rows = im.layout.rows();
        let per_row = im.layout.intervals_per_row();
        let mut starts = k
            .and_then(|i| self.opt_rows.get(i).cloned())
            .unwrap_or_default();
        if starts.len() >= rows as usize {
            starts.truncate(rows as usize);
            return Ok(starts);
        }
        // Scan from the last known row (or the scan start) for the remaining ones: row r
        // starts after the (r · per_row)-th restart marker.
        if starts.is_empty() {
            starts.push(im.layout.scan_start);
        }
        let mut pos = starts[starts.len() - 1];
        let mut markers = 0u64;
        let mut prev_ff = false;
        while starts.len() < rows as usize && pos < im.len {
            let n = (im.len - pos).min(SCAN_CHUNK);
            let mut buf = vec![0u8; n as usize];
            file.read_exact_at(pos, &mut buf)
                .map_err(|e| Error::io(&im.path, e))?;
            for (j, &b) in buf.iter().enumerate() {
                if prev_ff && (0xD0..=0xD7).contains(&b) {
                    markers += 1;
                    if markers.is_multiple_of(u64::from(per_row.max(1))) {
                        starts.push(pos + j as u64 + 1);
                        if starts.len() == rows as usize {
                            break;
                        }
                    }
                }
                prev_ff = b == 0xFF;
            }
            pos += n;
        }
        if starts.len() < rows as usize {
            return Err(bad(format!(
                "{}: found {} of {rows} MCU rows (restart markers missing)",
                im.path.display(),
                starts.len()
            )));
        }
        Ok(starts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells() {
        assert_eq!(parse_cell("ImageFile(1,0)"), Some((0, 1, 0)));
        assert_eq!(parse_cell("ImageFile(2,1,3)"), Some((2, 1, 3)));
        assert_eq!(parse_cell("ImageFile"), None);
        assert_eq!(parse_cell("ImageFile(a,b)"), None);
    }
}
