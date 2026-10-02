//! The slide model of a MIRAX data set: settings, pyramid levels with their stored images,
//! filters, camera positions and non-hierarchical records. `docs/formats/mirax.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};

use super::index::{HierItem, IndexFile, NonHierItem};
use super::ini::{Ini, IniSection, decode_text};
use crate::image::ImageCodec;

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "mirax";

/// Largest `Slidedat.ini` / `Index.dat` read whole (the corpus files are under 1 MB).
const MAX_SETTINGS_BYTES: u64 = 64 << 20;
/// Largest grid (`IMAGENUMBER_X · IMAGENUMBER_Y`) accepted.
const MAX_GRID_CELLS: u64 = 1 << 26;
/// Largest camera position table (bytes, compressed or not).
const MAX_POSITION_BYTES: u64 = 256 << 20;
/// Largest stored image side accepted.
const MAX_IMAGE_SIDE: u32 = 1 << 14;

/// Names of the hierarchies and records the reader uses (`Slidedat.ini` values).
pub const ZOOM_HIERARCHY: &str = "Slide zoom level";
/// The filter hierarchy.
pub const FILTER_HIERARCHY: &str = "Slide filter level";

/// One pyramid level (`HIER_<zoom>_VAL_<k>`).
#[derive(Debug, Clone)]
pub struct ZoomLevel {
    /// Level number (0 = full resolution of this file).
    pub level: u32,
    /// Settings section of the level.
    pub section: String,
    /// `MICROMETER_PER_PIXEL_X/Y`.
    pub mpp: (Option<f64>, Option<f64>),
    /// `OVERLAP_X/Y`: nominal overlap of neighbouring camera photos, pixels of this level.
    pub overlap: (f64, f64),
    /// `IMAGE_CONCAT_FACTOR`.
    pub concat_factor: u32,
    /// Grid cells per image side at this level: 2^(sum of the concat factors up to here).
    pub step: u64,
    /// `IMAGE_FORMAT` as stored (`JPEG`, `PNG`, `BMP`).
    pub image_format: String,
    /// `IMAGE_FILL_COLOR_BGR` as stored.
    pub fill_color: Option<u32>,
    /// Stored image size (`DIGITIZER_WIDTH/HEIGHT`).
    pub image_size: (u32, u32),
    /// The level's images by grid index.
    pub images: HashMap<u32, HierItem>,
}

/// A filter of the filter hierarchy (fluorescence channels; three identical `Default`
/// entries in brightfield slides).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// `FILTER_NAME`.
    pub name: Option<String>,
    /// `COLOR_R/G/B` (display colour).
    pub color: Option<(u8, u8, u8)>,
    /// `STORING_CHANNEL_NUMBER`: which colour component of the stored images holds it.
    pub storing_channel: Option<u32>,
    /// `EXPOSURE_TIME` as stored (unit not recorded).
    pub exposure_raw: Option<f64>,
    /// `DIGITALGAIN`.
    pub digital_gain: Option<f64>,
}

/// One camera position record (9 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Camera {
    /// The flag byte (1: the camera position has stored images, in version ≥ 1.9 files).
    pub flag: u8,
    /// Level-0 pixel coordinates of the photo's top-left corner.
    pub x: i32,
    /// See `x`.
    pub y: i32,
}

/// A non-hierarchical record (`NONHIER_<i>_VAL_<j>`).
#[derive(Debug, Clone)]
pub struct NonHierRecord {
    /// `NONHIER_<i>_NAME`.
    pub layer: String,
    /// `NONHIER_<i>_VAL_<j>`.
    pub value: String,
    /// Its settings section, if any.
    pub section: Option<String>,
    /// Data items.
    pub items: Vec<NonHierItem>,
}

/// Where the camera positions came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSource {
    /// `VIMSLIDE_POSITION_BUFFER`/`default`, raw 9-byte records.
    PositionBuffer,
    /// `StitchingIntensityLayer`/`StitchingIntensityLevel`, zlib-compressed 9-byte records
    /// (version 2.2 and later).
    StitchingIntensity,
}

impl PositionSource {
    /// Name used in `info` and assurance.
    pub fn name(self) -> &'static str {
        match self {
            PositionSource::PositionBuffer => "position buffer",
            PositionSource::StitchingIntensity => "compressed position table",
        }
    }
}

/// An opened MIRAX slide.
#[derive(Debug, Clone)]
pub struct MiraxSlide {
    /// The `.mrxs` file.
    pub path: PathBuf,
    /// The data directory (the `.mrxs` name without extension).
    pub dir: PathBuf,
    /// `Slidedat.ini`.
    pub ini: Ini,
    /// `Index.dat`.
    pub index: IndexFile,
    /// `[DATAFILE] FILE_<n>` paths.
    pub data_files: Vec<PathBuf>,
    /// `IMAGENUMBER_X/Y`.
    pub grid: (u32, u32),
    /// `CameraImageDivisionsPerSide` (1 when absent).
    pub divisions: u32,
    /// Pyramid levels.
    pub levels: Vec<ZoomLevel>,
    /// Filters.
    pub filters: Vec<Filter>,
    /// Camera positions (row-major), with their source.
    pub cameras: Option<(Vec<Camera>, PositionSource)>,
    /// Non-hierarchical records.
    pub nonhier: Vec<NonHierRecord>,
    /// Problems found while opening that do not stop reading (for `check`).
    pub problems: Vec<String>,
}

fn read_limited(fs: &Fs, path: &Path, max: u64, what: &str) -> Result<Vec<u8>> {
    let len = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
    if len > max {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("{what} is {len} bytes, larger than any MIRAX {what} ({max} bytes)"),
        ));
    }
    fs.read(path).map_err(|e| Error::io(path, e))
}

fn section<'a>(ini: &'a Ini, name: &str) -> Option<&'a IniSection> {
    ini.section(name)
}

fn count(ini: &Ini, key: &str) -> Result<usize> {
    let v = ini.get("HIERARCHICAL", key).ok_or_else(|| {
        Error::corrupt(
            FORMAT_ID,
            format!("Slidedat.ini lacks [HIERARCHICAL] {key}"),
        )
    })?;
    let n: usize = v.trim().parse().map_err(|_| {
        Error::corrupt(
            FORMAT_ID,
            format!("[HIERARCHICAL] {key} = {v} is not a count"),
        )
    })?;
    if n > 4096 {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("[HIERARCHICAL] {key} = {n} is implausibly large"),
        ));
    }
    Ok(n)
}

/// The data directory of a `.mrxs` path (same name without the extension), matched without
/// regard to case when the exact name is missing.
pub fn data_dir(fs: &Fs, mrxs: &Path) -> Option<PathBuf> {
    let stem = mrxs.file_stem()?.to_string_lossy().to_string();
    let parent = mrxs
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let exact = parent.join(&stem);
    if fs.is_dir(&exact) {
        return Some(exact);
    }
    fs.find_in_dir(parent, &stem).filter(|p| fs.is_dir(p))
}

/// Parse a `SLIDE_CREATIONDATETIME` value, `DD/MM/YYYY hh:mm:ss`, to ISO 8601 (no zone).
pub fn creation_time(v: &str) -> Option<String> {
    let (date, time) = v.trim().split_once(' ').unwrap_or((v.trim(), "00:00:00"));
    let d: Vec<u32> = date
        .split('/')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let t: Vec<u32> = time
        .split(':')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let ([day, month, year], [hh, mm, ss]) = (d.as_slice(), t.as_slice()) else {
        return None;
    };
    let (day, month, year, hh, mm, ss) = (*day, *month, *year, *hh, *mm, *ss);
    if !(1..=31).contains(&day)
        || !(1..=12).contains(&month)
        || year < 1990
        || hh > 23
        || mm > 59
        || ss > 60
    {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hh:02}:{mm:02}:{ss:02}"
    ))
}

impl MiraxSlide {
    /// Open the `.mrxs` at `path`: settings, index, level tables, camera positions. Pixel data
    /// is not touched.
    pub fn open(fs: &Fs, path: &Path) -> Result<MiraxSlide> {
        let dir = data_dir(fs, path).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "the data directory {} (the .mrxs name without extension, holding Slidedat.ini, Index.dat and Data*.dat) is missing; a MIRAX slide must be copied together with it",
                    path.with_extension("").display()
                ),
            )
        })?;
        let ini_path = fs
            .find_in_dir(&dir, "Slidedat.ini")
            .unwrap_or_else(|| dir.join("Slidedat.ini"));
        let ini = Ini::parse(&decode_text(&read_limited(
            fs,
            &ini_path,
            MAX_SETTINGS_BYTES,
            "Slidedat.ini",
        )?));
        let slide_id = ini
            .get("GENERAL", "SLIDE_ID")
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "Slidedat.ini lacks [GENERAL] SLIDE_ID"))?;
        let index_name = ini.get("HIERARCHICAL", "INDEXFILE").unwrap_or("Index.dat");
        let index_path = fs
            .find_in_dir(&dir, index_name)
            .unwrap_or_else(|| dir.join(index_name));
        let index = IndexFile::parse(
            read_limited(fs, &index_path, MAX_SETTINGS_BYTES, "Index.dat")?,
            slide_id,
        )
        .map_err(|p| Error::corrupt(FORMAT_ID, format!("Index.dat byte {}: {}", p.at, p.what)))?;

        let g = section(&ini, "GENERAL");
        let gx = g.and_then(|s| s.i64("IMAGENUMBER_X")).unwrap_or(0);
        let gy = g.and_then(|s| s.i64("IMAGENUMBER_Y")).unwrap_or(0);
        if gx <= 0 || gy <= 0 || (gx as u64).saturating_mul(gy as u64) > MAX_GRID_CELLS {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("[GENERAL] IMAGENUMBER_X/Y = {gx} x {gy} is not a usable image grid"),
            ));
        }
        let grid = (gx as u32, gy as u32);
        let divisions = g
            .and_then(|s| s.i64("CameraImageDivisionsPerSide"))
            .unwrap_or(1);
        if !(1..=64).contains(&divisions) || gx % divisions != 0 || gy % divisions != 0 {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("CameraImageDivisionsPerSide = {divisions} with a {gx} x {gy} image grid"),
                "The image grid is expected to be a whole number of camera photos, each split into CameraImageDivisionsPerSide images per side.",
            ));
        }
        let divisions = divisions as u32;

        let file_count = ini
            .get("DATAFILE", "FILE_COUNT")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0)
            .min(1 << 16);
        let data_files = (0..file_count)
            .map(|i| {
                let name = ini
                    .get("DATAFILE", &format!("FILE_{i}"))
                    .unwrap_or_default()
                    .to_string();
                fs.find_in_dir(&dir, &name)
                    .unwrap_or_else(|| dir.join(name))
            })
            .collect();

        // Hierarchical records: HIER_0 values, then HIER_1 values, ...
        let hier_count = count(&ini, "HIER_COUNT")?;
        let mut record = 0usize;
        let mut levels = Vec::new();
        let mut filters = Vec::new();
        let mut problems = Vec::new();
        let mut zoom_seen = false;
        for h in 0..hier_count {
            let name = ini
                .get("HIERARCHICAL", &format!("HIER_{h}_NAME"))
                .unwrap_or_default()
                .to_string();
            let n = count(&ini, &format!("HIER_{h}_COUNT"))?;
            for v in 0..n {
                let sec_name = ini
                    .get("HIERARCHICAL", &format!("HIER_{h}_VAL_{v}_SECTION"))
                    .unwrap_or_default()
                    .to_string();
                let sec = section(&ini, &sec_name);
                if name == ZOOM_HIERARCHY && !zoom_seen {
                    let items = index.hier_items(record).map_err(|p| {
                        Error::corrupt(
                            FORMAT_ID,
                            format!("Index.dat byte {}: level {v}: {}", p.at, p.what),
                        )
                    })?;
                    levels.push(Self::zoom_level(
                        v as u32, sec_name, sec, items, grid, &levels,
                    )?);
                } else if name == FILTER_HIERARCHY {
                    filters.push(Filter {
                        name: sec.and_then(|s| s.get("FILTER_NAME")).map(str::to_string),
                        color: sec.and_then(|s| {
                            let c = |k: &str| s.i64(k).and_then(|v| u8::try_from(v).ok());
                            Some((c("COLOR_R")?, c("COLOR_G")?, c("COLOR_B")?))
                        }),
                        storing_channel: sec
                            .and_then(|s| s.i64("STORING_CHANNEL_NUMBER"))
                            .and_then(|v| u32::try_from(v).ok()),
                        exposure_raw: sec.and_then(|s| s.f64("EXPOSURE_TIME")),
                        digital_gain: sec.and_then(|s| s.f64("DIGITALGAIN")),
                    });
                }
                record += 1;
            }
            if name == ZOOM_HIERARCHY {
                zoom_seen = true;
            }
        }
        if levels.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("Slidedat.ini has no \"{ZOOM_HIERARCHY}\" hierarchy with levels"),
            ));
        }

        // Non-hierarchical records.
        let nonhier_count = count(&ini, "NONHIER_COUNT")?;
        let mut nonhier = Vec::new();
        let mut k = 0usize;
        for i in 0..nonhier_count {
            let layer = ini
                .get("HIERARCHICAL", &format!("NONHIER_{i}_NAME"))
                .unwrap_or_default()
                .to_string();
            let n = count(&ini, &format!("NONHIER_{i}_COUNT"))?;
            for j in 0..n {
                let value = ini
                    .get("HIERARCHICAL", &format!("NONHIER_{i}_VAL_{j}"))
                    .unwrap_or_default()
                    .to_string();
                let items = match index.nonhier_items(k) {
                    Ok(v) => v,
                    Err(p) => {
                        problems.push(format!(
                            "Index.dat byte {}: record {layer}/{value}: {}",
                            p.at, p.what
                        ));
                        Vec::new()
                    }
                };
                nonhier.push(NonHierRecord {
                    layer: layer.clone(),
                    value,
                    section: ini
                        .get("HIERARCHICAL", &format!("NONHIER_{i}_VAL_{j}_SECTION"))
                        .map(str::to_string),
                    items,
                });
                k += 1;
            }
        }

        let mut slide = MiraxSlide {
            path: path.to_path_buf(),
            dir,
            ini,
            index,
            data_files,
            grid,
            divisions,
            levels,
            filters,
            cameras: None,
            nonhier,
            problems,
        };
        slide.cameras = slide.read_cameras(fs)?;
        Ok(slide)
    }

    fn zoom_level(
        level: u32,
        section_name: String,
        sec: Option<&IniSection>,
        items: Vec<HierItem>,
        grid: (u32, u32),
        below: &[ZoomLevel],
    ) -> Result<ZoomLevel> {
        let f = |k: &str| sec.and_then(|s| s.f64(k));
        let concat = sec
            .and_then(|s| s.i64("IMAGE_CONCAT_FACTOR"))
            .unwrap_or(i64::from(level != 0));
        if !(0..=16).contains(&concat) {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("level {level}: IMAGE_CONCAT_FACTOR = {concat}"),
            ));
        }
        let shift = below.last().map_or(0, |l| l.step.trailing_zeros()) + concat as u32;
        if shift > 30 {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("level {level}: the pyramid is deeper than 2^30 grid cells per image"),
            ));
        }
        let step = 1u64 << shift;
        let w = sec.and_then(|s| s.i64("DIGITIZER_WIDTH")).unwrap_or(0);
        let h = sec.and_then(|s| s.i64("DIGITIZER_HEIGHT")).unwrap_or(0);
        if w <= 0 || h <= 0 || w > i64::from(MAX_IMAGE_SIDE) || h > i64::from(MAX_IMAGE_SIDE) {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("level {level}: DIGITIZER_WIDTH/HEIGHT = {w} x {h} is not an image size"),
            ));
        }
        let mut images = HashMap::with_capacity(items.len());
        let cells = u64::from(grid.0) * u64::from(grid.1);
        for it in items {
            if u64::from(it.image) < cells {
                images.insert(it.image, it);
            }
        }
        Ok(ZoomLevel {
            level,
            section: section_name,
            mpp: (f("MICROMETER_PER_PIXEL_X"), f("MICROMETER_PER_PIXEL_Y")),
            overlap: (f("OVERLAP_X").unwrap_or(0.0), f("OVERLAP_Y").unwrap_or(0.0)),
            concat_factor: concat as u32,
            step,
            image_format: sec
                .and_then(|s| s.get("IMAGE_FORMAT"))
                .unwrap_or("JPEG")
                .to_string(),
            fill_color: sec
                .and_then(|s| s.i64("IMAGE_FILL_COLOR_BGR"))
                .and_then(|v| u32::try_from(v).ok()),
            image_size: (w as u32, h as u32),
            images,
        })
    }

    /// The non-hierarchical record `layer`/`value`.
    pub fn record(&self, layer: &str, value: &str) -> Option<&NonHierRecord> {
        self.nonhier
            .iter()
            .find(|r| r.layer == layer && r.value == value)
    }

    /// Read the bytes of a data item.
    pub fn read_item(&self, fs: &Fs, file: u32, offset: u64, length: u64) -> Result<Vec<u8>> {
        let path = self.data_files.get(file as usize).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "Index.dat refers to data file {file}, but [DATAFILE] lists {}",
                    self.data_files.len()
                ),
            )
        })?;
        let len = usize::try_from(length)
            .ok()
            .filter(|&l| l <= (512 << 20))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("a data item of {length} bytes")))?;
        let f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let mut buf = vec![0u8; len];
        f.read_exact_at(offset, &mut buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                Error::corrupt(
                    FORMAT_ID,
                    format!(
                        "{}: {length} bytes at {offset} run past the end of the file",
                        path.display()
                    ),
                )
            } else {
                Error::io(path, e)
            }
        })?;
        Ok(buf)
    }

    fn read_cameras(&self, fs: &Fs) -> Result<Option<(Vec<Camera>, PositionSource)>> {
        let (rec, src) = if let Some(r) = self.record("VIMSLIDE_POSITION_BUFFER", "default") {
            (r, PositionSource::PositionBuffer)
        } else if let Some(r) = self.record("StitchingIntensityLayer", "StitchingIntensityLevel") {
            (r, PositionSource::StitchingIntensity)
        } else {
            return Ok(None);
        };
        let Some(it) = rec.items.first() else {
            return Ok(None);
        };
        if it.length > MAX_POSITION_BYTES {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("the camera position table is {} bytes", it.length),
            ));
        }
        let cams =
            u64::from(self.grid.0 / self.divisions) * u64::from(self.grid.1 / self.divisions);
        let expect = cams.saturating_mul(9);
        let raw = self.read_item(fs, it.file, it.offset, it.length)?;
        let raw = match src {
            PositionSource::PositionBuffer => raw,
            PositionSource::StitchingIntensity => {
                openreadout_codecs::zlib_decode(&raw, usize::try_from(expect).unwrap_or(0))
                    .map_err(|e| {
                        Error::corrupt(FORMAT_ID, format!("compressed camera position table: {e}"))
                    })?
            }
        };
        if raw.len() as u64 != expect {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "the camera position table holds {} bytes; {} camera positions of 9 bytes need {expect}",
                    raw.len(),
                    cams
                ),
            ));
        }
        let cameras = raw
            .as_chunks::<9>()
            .0
            .iter()
            .map(|c| Camera {
                flag: c[0],
                x: i32::from_le_bytes([c[1], c[2], c[3], c[4]]),
                y: i32::from_le_bytes([c[5], c[6], c[7], c[8]]),
            })
            .collect();
        Ok(Some((cameras, src)))
    }

    /// Camera photo columns and rows.
    pub fn camera_grid(&self) -> (u32, u32) {
        (self.grid.0 / self.divisions, self.grid.1 / self.divisions)
    }

    /// Whether the slide is a fluorescence slide (`SLIDE_TYPE` names fluorescence).
    pub fn is_fluorescence(&self) -> bool {
        self.ini
            .get("GENERAL", "SLIDE_TYPE")
            .is_some_and(|t| t.to_ascii_uppercase().contains("FLUORESCENCE"))
    }

    /// The codec of level `level`'s images, from `IMAGE_FORMAT`.
    pub fn codec(&self, level: usize) -> Option<ImageCodec> {
        match self
            .levels
            .get(level)?
            .image_format
            .to_ascii_uppercase()
            .as_str()
        {
            "JPEG" | "JPG" => Some(ImageCodec::Jpeg),
            "PNG" => Some(ImageCodec::Png),
            f if f.starts_with("BMP") => Some(ImageCodec::Bmp),
            _ => None,
        }
    }

    /// Level-0 size in pixels. With camera positions: `cameras · (photo − o) + o` per axis,
    /// photo = image size · divisions / level-0 step, o = round(level-0 `OVERLAP`) / level-0
    /// step (the size OpenSlide reports, so coordinates agree with OpenSlide-based tools).
    /// Without positions (pixels refused): the image grid, `IMAGENUMBER / step · image size`.
    pub fn level0_size(&self) -> (u32, u32) {
        let l0 = &self.levels[0];
        let s = l0.step as f64;
        let (cx, cy) = self.camera_grid();
        let axis = |cams: u32, img: u32, ov: f64, cells: u32| -> u32 {
            let v = if self.cameras.is_some() {
                let photo = f64::from(img) * f64::from(self.divisions) / s;
                let o = ov.round() / s;
                f64::from(cams) * (photo - o) + o
            } else {
                (f64::from(cells) / s).ceil() * f64::from(img)
            };
            if v.is_finite() && v >= 1.0 {
                v.min(f64::from(u32::MAX)).floor() as u32
            } else {
                1
            }
        };
        (
            axis(cx, l0.image_size.0, l0.overlap.0, self.grid.0),
            axis(cy, l0.image_size.1, l0.overlap.1, self.grid.1),
        )
    }

    /// Size of level `level`: level-0 size / 2^level, rounded down (at least 1).
    pub fn level_size(&self, level: u32) -> (u32, u32) {
        let (w, h) = self.level0_size();
        let f = 1u64 << level.min(31);
        (
            ((u64::from(w) / f).max(1)) as u32,
            ((u64::from(h) / f).max(1)) as u32,
        )
    }

    /// The settings value `[GENERAL] key`.
    pub fn general(&self, key: &str) -> Option<&str> {
        self.ini.get("GENERAL", key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_times() {
        assert_eq!(
            creation_time("29/12/2009 12:43:52").as_deref(),
            Some("2009-12-29T12:43:52")
        );
        assert_eq!(
            creation_time("10/03/2011 11:47:25").as_deref(),
            Some("2011-03-10T11:47:25")
        );
        assert!(creation_time("12/29/2009 12:43:52").is_none());
        assert!(creation_time("garbage").is_none());
    }
}
