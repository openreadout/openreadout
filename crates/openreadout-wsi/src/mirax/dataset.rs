//! `Dataset` for a MIRAX slide: one pyramidal image (RGB for brightfield, one channel per
//! filter for fluorescence), associated images and records as attachments.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::bytes::find;
use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, MosaicInfo, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::region::{Region, ResolutionLevel, TileCache};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, PixelType, Plane, Result};
use rayon::prelude::*;
use serde_json::{Value, json};

use super::render::{LevelLayout, compose, valid_cameras};
use super::slide::{FORMAT_ID, MiraxSlide, creation_time};
use crate::MiraxReader;
use crate::image::{ImageCodec, RgbImage, decode_rgb, header_size};

/// Decoded images kept between region reads.
const IMAGE_CACHE_BYTES: usize = 256 << 20;
/// Rows composed at a time.
const BAND_ROWS: u32 = 512;

/// Associated images OpenSlide names label, macro and thumbnail, and our attachment names.
const ASSOCIATED: [(&str, &str); 3] = [
    ("ScanDataLayer_SlideBarcode", "label"),
    ("ScanDataLayer_SlideThumbnail", "macro"),
    ("ScanDataLayer_SlidePreview", "thumbnail"),
];

/// An opened MIRAX slide.
pub struct MiraxDataset {
    path: PathBuf,
    fs: Fs,
    slide: MiraxSlide,
    layouts: Vec<Option<Arc<LevelLayout>>>,
    cache: TileCache<(u32, u32), RgbImage>,
    /// Channel k is decoded colour component `components[k]` (fluorescence), or the RGB
    /// triple (brightfield, empty).
    components: Vec<usize>,
}

impl std::fmt::Debug for MiraxDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiraxDataset")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

fn rgb_hex(c: (u8, u8, u8)) -> String {
    format!("#{:02X}{:02X}{:02X}", c.0, c.1, c.2)
}

/// `IMAGE_FILL_COLOR_BGR` as R, G, B: the value is read as `0xBBGGRR` (red in the low byte).
pub fn fill_rgb(v: u32) -> [u8; 3] {
    [
        (v & 0xFF) as u8,
        ((v >> 8) & 0xFF) as u8,
        ((v >> 16) & 0xFF) as u8,
    ]
}

impl MiraxDataset {
    /// Open the `.mrxs` named by `input`.
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let slide = MiraxSlide::open(fs, path)?;
        let components = if slide.is_fluorescence() {
            let mut seen = Vec::new();
            for (k, f) in slide.filters.iter().enumerate() {
                let Some(s) = f
                    .storing_channel
                    .filter(|s| *s < 3 && !seen.contains(&(2 - *s as usize)))
                else {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        format!(
                            "fluorescence filter {k} ({}) stored in channel {:?}",
                            f.name.clone().unwrap_or_default(),
                            f.storing_channel
                        ),
                        "Only up to three filters stored in distinct colour components (STORING_CHANNEL_NUMBER 0, 1, 2) are known.",
                    ));
                };
                seen.push(2 - s as usize);
            }
            if seen.is_empty() {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "fluorescence slide without filters",
                    "The filter hierarchy (\"Slide filter level\") names the channels; none was found.",
                ));
            }
            seen
        } else {
            Vec::new()
        };
        let n = slide.levels.len();
        Ok(MiraxDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            slide,
            layouts: vec![None; n],
            cache: TileCache::new(IMAGE_CACHE_BYTES),
            components,
        })
    }

    fn rel(&self, p: &Path) -> String {
        let base = self.path.parent().unwrap_or(Path::new(""));
        p.strip_prefix(base).unwrap_or(p).display().to_string()
    }

    fn layout(&mut self, level: usize) -> Result<Arc<LevelLayout>> {
        if let Some(Some(l)) = self.layouts.get(level) {
            return Ok(l.clone());
        }
        let l = Arc::new(LevelLayout::build(&self.slide, level)?);
        if let Some(slot) = self.layouts.get_mut(level) {
            *slot = Some(l.clone());
        }
        Ok(l)
    }

    fn decode(slide: &MiraxSlide, fs: &Fs, level: usize, image: u32) -> Result<RgbImage> {
        let lv = &slide.levels[level];
        let it = lv.images.get(&image).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("level {level} has no image {image}"))
        })?;
        let bytes = slide.read_item(fs, it.file, it.offset, it.length)?;
        let (w, h) = lv.image_size;
        let img = decode_rgb(FORMAT_ID, &bytes, u64::from(w) * u64::from(h))?;
        if (img.width, img.height) != (w, h) {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "level {level} image {image} decodes to {} x {}, the level's images are {w} x {h}",
                    img.width, img.height
                ),
            ));
        }
        Ok(img)
    }

    fn channel_count(&self) -> u32 {
        if self.components.is_empty() {
            1
        } else {
            self.components.len() as u32
        }
    }

    #[allow(clippy::many_single_char_names)]
    fn read_level_region(
        &mut self,
        index: PlaneIndex,
        level: u32,
        region: Option<Region>,
    ) -> Result<Plane> {
        if index.z != 0 || index.t != 0 || index.c >= self.channel_count() {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} is out of range (C={}, Z=1, T=1)",
                index.c,
                index.z,
                index.t,
                self.channel_count()
            )));
        }
        let lvl = level as usize;
        if lvl >= self.slide.levels.len() {
            return Err(Error::Usage(format!(
                "pyramid level {level} out of range (0..{})",
                self.slide.levels.len()
            )));
        }
        let (w, h) = self.slide.level_size(level);
        let rgb_out = self.components.is_empty();
        let spp = if rgb_out { 3 } else { 1 };
        let win = match region {
            Some(r) => {
                r.check_within(w, h, &format!("image 0 level {level}"))?;
                r
            }
            None => Region::full(w, h),
        };
        let n = openreadout_core::pixel::plane_bytes_checked(
            FORMAT_ID,
            win.width,
            win.height,
            spp as usize,
        )
        .map_err(|_| {
            Error::unsupported(
                FORMAT_ID,
                format!("a {} x {} plane in memory", win.width, win.height),
                format!(
                    "Read a region of it (`--region X,Y,W,H`) or a downsampled pyramid level (`--level N`, N up to {}); level sizes are in `info` → images[].resolution_levels.",
                    self.slide.levels.len() - 1
                ),
            )
        })?;
        let layout = self.layout(lvl)?;
        let bg = self.slide.levels[lvl]
            .fill_color
            .map_or([255, 255, 255], fill_rgb);
        let comp = (!rgb_out).then(|| self.components[index.c as usize]);
        let mut data = vec![0u8; n];
        let row_bytes = win.width as usize * spp as usize;
        // Compose in bands of rows, so the working buffers stay small for whole levels.
        let mut y = 0u32;
        while y < win.height {
            let bh = BAND_ROWS.min(win.height - y);
            let band = Region::new(win.x, win.y + y, win.width, bh);
            let rgb = self.compose_band(&layout, lvl, band, bg)?;
            let dst = &mut data[y as usize * row_bytes..(y + bh) as usize * row_bytes];
            match comp {
                None => dst.copy_from_slice(&rgb),
                Some(c) => {
                    for (d, p) in dst.iter_mut().zip(rgb.as_chunks::<3>().0) {
                        *d = p[c];
                    }
                }
            }
            y += bh;
        }
        Ok(Plane {
            width: win.width,
            height: win.height,
            pixel_type: PixelType::Uint8,
            samples_per_pixel: spp,
            data,
        })
    }

    /// Compose one band of a level: decode the images it needs (in parallel, cached), then
    /// place them.
    fn compose_band(
        &mut self,
        layout: &LevelLayout,
        lvl: usize,
        band: Region,
        bg: [u8; 3],
    ) -> Result<Vec<u8>> {
        let level = lvl as u32;
        let subtiles = layout.candidates(band);
        let mut need: Vec<u32> = subtiles.iter().map(|t| t.image).collect();
        need.sort_unstable();
        need.dedup();
        need.retain(|i| self.cache.get(&(level, *i)).is_none());
        let (slide, fs) = (&self.slide, &self.fs);
        let fresh: Vec<Result<(u32, RgbImage)>> = need
            .into_par_iter()
            .map(|i| Self::decode(slide, fs, lvl, i).map(|d| (i, d)))
            .collect();
        let mut local: BTreeMap<u32, Arc<RgbImage>> = BTreeMap::new();
        for r in fresh {
            let (i, d) = r?;
            let d = Arc::new(d);
            let n = d.data.len();
            self.cache.put((level, i), d.clone(), n);
            local.insert(i, d);
        }
        let (slide, fs, cache) = (&self.slide, &self.fs, &mut self.cache);
        compose(&subtiles, band, bg, |i| {
            if let Some(d) = local.get(&i) {
                return Ok(d.clone());
            }
            if let Some(d) = cache.get(&(level, i)) {
                return Ok(d);
            }
            // Evicted by this band's own decodes (a band needing more than the cache holds).
            Self::decode(slide, fs, lvl, i).map(Arc::new)
        })
    }

    #[allow(clippy::many_single_char_names)]
    fn image_info(&self) -> ImageInfo {
        let s = &self.slide;
        let (w, h) = s.level0_size();
        let mut info = ImageInfo::new(0, w, h, PixelType::Uint8);
        info.name = s.general("SLIDE_NAME").map(str::to_string);
        info.size_c = self.channel_count();
        info.samples_per_pixel = if self.components.is_empty() { 3 } else { 1 };
        info.dimension_order = "XYCZT".into();
        info.pyramid_levels = s.levels.len() as u32;
        let s0 = s.levels[0].step;
        info.resolution_levels = s
            .levels
            .iter()
            .map(|lv| {
                let (lw, lh) = s.level_size(lv.level);
                let mut r = ResolutionLevel::new(lv.level, lw, lh, w, h)
                    .with_tile(lv.image_size.0, lv.image_size.1);
                let f = (lv.step / s0.max(1)) as f64;
                r.downsample_x = f;
                r.downsample_y = f;
                r
            })
            .collect();
        let l0 = &s.levels[0];
        info.physical_size = PhysicalSize::micrometres(
            l0.mpp.0.filter(|v| *v > 0.0),
            l0.mpp.1.filter(|v| *v > 0.0),
            None,
        );
        let fluor = !self.components.is_empty();
        info.channels = if fluor {
            s.filters
                .iter()
                .enumerate()
                .map(|(k, f)| ChannelInfo {
                    index: k as u32,
                    name: f.name.clone(),
                    color: f.color.map(rgb_hex),
                    acquisition_mode: Some("Fluorescence".into()),
                    ..ChannelInfo::default()
                })
                .collect()
        } else {
            vec![ChannelInfo {
                index: 0,
                acquisition_mode: Some("Brightfield (RGB)".into()),
                ..ChannelInfo::default()
            }]
        };
        let mag = s
            .general("OBJECTIVE_MAGNIFICATION")
            .and_then(super::ini::parse_f64)
            .filter(|v| *v > 0.0);
        let objective_name = s
            .general("OBJECTIVE_NAME")
            .map(str::to_string)
            .filter(|n| !n.is_empty());
        if mag.is_some() || objective_name.is_some() {
            info.objective = Some(ObjectiveInfo {
                model: objective_name,
                nominal_magnification: mag,
                lens_na: None,
                immersion: None,
            });
        }
        let nh = s.ini.section("NONHIERLAYER_0_SECTION");
        info.instrument = Some(InstrumentInfo {
            manufacturer: Some("3DHISTECH".into()),
            model: None,
            software: None,
            software_version: nh
                .and_then(|n| n.get("SCANNER_SOFTWARE_VERSION"))
                .map(|v| v.replace(',', ".")),
            detector: s.general("CAMERA_TYPE").map(str::to_string),
        });
        info.acquired_at = s.general("SLIDE_CREATIONDATETIME").and_then(creation_time);
        let l0_images = l0.images.len() as u32;
        info.mosaic = Some(MosaicInfo {
            tile_count: l0_images,
            tile_width: Some(l0.image_size.0),
            tile_height: Some(l0.image_size.1),
            stitched_on_read: true,
        });
        let (cx, cy) = s.camera_grid();
        let valid = valid_cameras(s);
        let mut m = serde_json::Map::new();
        m.insert("slide_id".into(), json!(s.general("SLIDE_ID")));
        m.insert("slide_version".into(), json!(s.general("SLIDE_VERSION")));
        m.insert(
            "current_slide_version".into(),
            json!(s.general("CURRENT_SLIDE_VERSION")),
        );
        m.insert("index_version".into(), json!(s.index.version));
        m.insert("slide_type".into(), json!(s.general("SLIDE_TYPE")));
        m.insert("image_grid".into(), json!([s.grid.0, s.grid.1]));
        m.insert("camera_divisions".into(), json!(s.divisions));
        m.insert("camera_grid".into(), json!([cx, cy]));
        m.insert(
            "camera_positions".into(),
            json!(s.cameras.as_ref().map(|(_, src)| src.name())),
        );
        m.insert(
            "cameras_with_images".into(),
            json!(valid.iter().filter(|v| **v).count()),
        );
        m.insert(
            "levels".into(),
            json!(
                s.levels
                    .iter()
                    .map(|lv| json!({
                        "level": lv.level,
                        "grid_step": lv.step,
                        "concat_factor": lv.concat_factor,
                        "image_format": lv.image_format,
                        "image_size": [lv.image_size.0, lv.image_size.1],
                        "images": lv.images.len(),
                        "overlap_px": [lv.overlap.0, lv.overlap.1],
                        "micrometre_per_pixel": [lv.mpp.0, lv.mpp.1],
                        "fill_color_bgr": lv.fill_color,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        if fluor {
            m.insert(
                "filters".into(),
                json!(
                    s.filters
                        .iter()
                        .map(|f| json!({
                            "name": f.name,
                            "storing_channel": f.storing_channel,
                            "exposure_time_raw": f.exposure_raw,
                            "digital_gain": f.digital_gain,
                        }))
                        .collect::<Vec<_>>()
                ),
            );
        }
        if let Some(n) = nh {
            for (k, key) in [
                ("scanner_hardware_version", "SCANNER_HARDWARE_VERSION"),
                ("scanner_software_version", "SCANNER_SOFTWARE_VERSION"),
                ("scanning_time_s", "SCANNING_TIME_IN_SEC"),
                ("scanned_fields", "SCANNED_FOV_COUNT"),
            ] {
                if let Some(v) = n.get(key) {
                    m.insert(k.into(), json!(v));
                }
            }
        }
        info.extra.insert("mirax".into(), Value::Object(m));
        info.extra.insert(
            "codec".into(),
            json!(s.codec(0).map_or_else(
                || s.levels[0].image_format.to_ascii_lowercase(),
                |c| c.name().to_string()
            )),
        );
        info.finish()
    }

    /// Attachments: the associated images first (label, macro, thumbnail), then every other
    /// non-hierarchical record with data, then the `.mrxs` preview.
    fn attachment_list(&self) -> Vec<(AttachmentInfo, AttachmentSource)> {
        let mut out = Vec::new();
        let mut push = |name: String,
                        src: AttachmentSource,
                        size: u64,
                        offset: Option<u64>,
                        extra: BTreeMap<String, Value>,
                        kind: &str| {
            let (ct, ext) = match kind {
                "jpeg" => ("JPG", "jpg"),
                "png" => ("PNG", "png"),
                "bmp" => ("BMP", "bmp"),
                "xml" => ("XML", "xml"),
                _ => ("BIN", "bin"),
            };
            let index = out.len() as u32;
            out.push((
                AttachmentInfo {
                    index,
                    name,
                    content_type: ct.into(),
                    extension: ext.into(),
                    offset,
                    size,
                    extra,
                },
                src,
            ));
        };
        let kind_of = |fmt: Option<&str>| match fmt.map(str::to_ascii_uppercase).as_deref() {
            Some("JPEG" | "JPG") => "jpeg",
            Some("PNG") => "png",
            Some("BMP") => "bmp",
            _ => "bin",
        };
        let mut done = Vec::new();
        for (value, name) in ASSOCIATED {
            if let Some(r) = self.slide.record("Scan data layer", value)
                && let Some(it) = r.items.first()
            {
                let sec = r.section.as_deref().and_then(|s| self.slide.ini.section(s));
                let fmt = sec.and_then(|s| {
                    s.entries
                        .iter()
                        .find(|(k, _)| k.ends_with("_IMAGE_TYPE"))
                        .map(|(_, v)| v.as_str())
                });
                let mut extra = BTreeMap::new();
                extra.insert("record".into(), json!(value));
                if let Some(s) = sec {
                    for (k, v) in &s.entries {
                        if k.ends_with("_IMAGE_WIDTH") {
                            extra.insert("width".into(), json!(v.parse::<u32>().ok()));
                        } else if k.ends_with("_IMAGE_HEIGHT") {
                            extra.insert("height".into(), json!(v.parse::<u32>().ok()));
                        }
                    }
                }
                push(
                    name.into(),
                    AttachmentSource::Item(*it),
                    it.length,
                    Some(it.offset),
                    extra,
                    kind_of(fmt),
                );
                done.push(value);
            }
        }
        for r in &self.slide.nonhier {
            if done.contains(&r.value.as_str())
                || (r.layer == "VIMSLIDE_POSITION_BUFFER" && r.value == "default")
            {
                continue;
            }
            for (k, it) in r.items.iter().enumerate() {
                let sec = r.section.as_deref().and_then(|s| self.slide.ini.section(s));
                let fmt = sec.and_then(|s| {
                    s.entries
                        .iter()
                        .find(|(k, _)| k.ends_with("_IMAGE_TYPE"))
                        .map(|(_, v)| v.as_str())
                });
                let kind = if fmt.is_some() {
                    kind_of(fmt)
                } else if r.value.contains("XML") {
                    "xml"
                } else {
                    "bin"
                };
                let name = if r.items.len() > 1 {
                    format!("{}/{} {k}", r.layer, r.value)
                } else {
                    format!("{}/{}", r.layer, r.value)
                };
                let mut extra = BTreeMap::new();
                extra.insert("record".into(), json!(r.value));
                extra.insert("layer".into(), json!(r.layer));
                push(
                    name,
                    AttachmentSource::Item(*it),
                    it.length,
                    Some(it.offset),
                    extra,
                    kind,
                );
            }
        }
        if let Ok(m) = self.fs.metadata(&self.path)
            && m.len() > 0
        {
            push(
                "mrxs preview".into(),
                AttachmentSource::MrxsFile,
                m.len(),
                Some(0),
                BTreeMap::new(),
                "jpeg",
            );
        }
        out
    }
}

#[derive(Debug, Clone, Copy)]
enum AttachmentSource {
    Item(super::index::NonHierItem),
    MrxsFile,
}

impl Dataset for MiraxDataset {
    fn member_files(&self) -> Vec<PathBuf> {
        let mut out = vec![self.slide.dir.join("Slidedat.ini")];
        if let Some(n) = self.slide.ini.get("HIERARCHICAL", "INDEXFILE") {
            out.push(self.slide.dir.join(n));
        }
        out.extend(self.slide.data_files.iter().cloned());
        out.retain(|p| self.fs.is_file(p));
        out
    }

    fn info(&self) -> Result<FileInfo> {
        let image = self.image_info();
        let plane_count = image.plane_count;
        let s = &self.slide;
        let mut notes = vec![
            "a MIRAX slide: pyramid levels of camera photos placed at their recorded positions; read a region with `--region X,Y,W,H` or a coarser level with `--level N` (level 0 of a whole slide is too large to read whole)".to_string(),
        ];
        if s.cameras.is_some() {
            notes.push("camera photos overlap: each stored image is placed at its camera's recorded position (fractional at levels above 0, resampled bilinearly as OpenSlide does); where photos overlap, the later one in raster order is shown".into());
        } else {
            notes.push("no camera position table (a slide exported with overlaps removed): the pixels are not read (exit 6) until such a slide is validated; the size is the image grid".into());
        }
        if !self.components.is_empty() {
            notes.push(format!(
                "fluorescence slide: {} filters stored as the colour components of each image (STORING_CHANNEL_NUMBER k is component {}); returned as one channel per filter",
                self.components.len(),
                "2 - k (B, G, R order)"
            ));
        }
        if let Some(c) = s.levels[0].fill_color
            && let [r, g, b] = fill_rgb(c)
            && !(r == g && g == b)
        {
            notes.push(format!("the fill colour {c} (IMAGE_FILL_COLOR_BGR) is not grey; it is read as 0xBBGGRR, an order no validated file settles"));
        }
        if !s.problems.is_empty() {
            notes.push(format!(
                "{} problem(s) found while opening; run `check`",
                s.problems.len()
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: MiraxReader.descriptor(),
            format_version: s.general("CURRENT_SLIDE_VERSION").map(str::to_string),
            images: vec![image],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut out = serde_json::Map::new();
        out.insert("slidedat".into(), self.slide.ini.to_json());
        let mut xml = serde_json::Map::new();
        for r in &self.slide.nonhier {
            if !r.value.contains("XML") {
                continue;
            }
            if let Some(it) = r.items.first()
                && it.length <= (1 << 20)
                && let Ok(b) = self
                    .slide
                    .read_item(&self.fs, it.file, it.offset, it.length)
            {
                xml.insert(r.value.clone(), json!(String::from_utf8_lossy(&b)));
            }
        }
        out.insert("xml_records".into(), Value::Object(xml));
        Ok(Value::Object(out))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "images[].size_x",
            "images[].size_y",
            "images[].pixel_type",
            "images[].samples_per_pixel",
            "images[].pyramid_levels",
            "images[].resolution_levels",
            "images[].physical_size",
            "images[].objective",
            "images[].instrument",
            "images[].channels[].name",
            "images[].channels[].color",
            "images[].mosaic",
            "images[].extra.mirax",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        for k in ["images[].size_c", "images[].acquired_at"] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let s = &self.slide;
        let mut out = vec![
            LsEntry {
                kind: "metadata".into(),
                name: self.rel(&s.dir.join("Slidedat.ini")),
                offset: None,
                size: None,
                image: None,
                details: json!({"sections": s.ini.sections.len()}),
            },
            LsEntry {
                kind: "index".into(),
                name: self.rel(
                    &s.dir.join(
                        s.ini
                            .get("HIERARCHICAL", "INDEXFILE")
                            .unwrap_or("Index.dat"),
                    ),
                ),
                offset: Some(0),
                size: Some(s.index.len() as u64),
                image: None,
                details: json!({"version": s.index.version, "hierarchical_root": s.index.hier_root, "nonhierarchical_root": s.index.nonhier_root}),
            },
        ];
        for lv in &s.levels {
            let (w, h) = s.level_size(lv.level);
            out.push(LsEntry {
                kind: "pyramid-level".into(),
                name: format!("level {}", lv.level),
                offset: None,
                size: Some(lv.images.values().map(|i| i.length).sum()),
                image: Some(0),
                details: json!({"level": lv.level, "size_x": w, "size_y": h, "images": lv.images.len(),
                                "grid_step": lv.step, "image_format": lv.image_format,
                                "image_size": [lv.image_size.0, lv.image_size.1]}),
            });
        }
        for f in &s.data_files {
            out.push(LsEntry {
                kind: "file".into(),
                name: self.rel(f),
                offset: None,
                size: self.fs.metadata(f).ok().map(|m| m.len()),
                image: None,
                details: json!({"read": true}),
            });
        }
        for (a, _) in self.attachment_list() {
            out.push(LsEntry {
                kind: "attachment".into(),
                name: a.name.clone(),
                offset: a.offset,
                size: Some(a.size),
                image: None,
                details: json!({"content_type": a.content_type}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        self.read_plane_level(image, index, 0)
    }

    fn read_plane_level(&mut self, image: u32, index: PlaneIndex, level: u32) -> Result<Plane> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image {image} does not exist (1 image)"
            )));
        }
        self.read_level_region(index, level, None)
    }

    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image {image} does not exist (1 image)"
            )));
        }
        self.read_level_region(index, level, Some(region))
    }

    fn check(&mut self) -> Result<CheckReport> {
        Ok(self.check_impl(true))
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        Ok(self.check_impl(false))
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        Ok(self.attachment_list().into_iter().map(|(a, _)| a).collect())
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let list = self.attachment_list();
        let (_, src) = list.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "attachment {index} does not exist ({} attachments)",
                list.len()
            ))
        })?;
        match src {
            AttachmentSource::Item(it) => self
                .slide
                .read_item(&self.fs, it.file, it.offset, it.length),
            AttachmentSource::MrxsFile => self
                .fs
                .read(&self.path)
                .map_err(|e| Error::io(&self.path, e)),
        }
    }
}

impl MiraxDataset {
    fn check_impl(&mut self, full: bool) -> CheckReport {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        let s = &self.slide;
        r.performed("Slidedat.ini: image grid, camera divisions, hierarchies and data files");
        r.performed("Index.dat: slide id, root tables, data-page lists (bounds, loops)");
        for p in &s.problems {
            r.push(Finding::error("bad_index", p.clone()));
        }
        r.performed("data files: present, header names the slide id, every level image and record lies inside its file");
        let mut lens = Vec::with_capacity(s.data_files.len());
        for (k, f) in s.data_files.iter().enumerate() {
            if let Ok(m) = self.fs.metadata(f) {
                lens.push(Some(m.len()));
            } else {
                lens.push(None);
                r.push(Finding::error(
                    "missing_file",
                    format!("data file {k} ({}) is missing", self.rel(f)),
                ));
            }
            if let Ok(fh) = self.fs.open(f) {
                let mut head = vec![0u8; 64];
                if fh.read_exact_at(0, &mut head).is_ok() {
                    let id = s.general("SLIDE_ID").unwrap_or_default();
                    let ascii = head.get(5..5 + id.len()) == Some(id.as_bytes());
                    let utf16: Vec<u8> = id.bytes().take(id.len() / 2).collect();
                    let wide = find(&head, &utf16).is_some();
                    if !ascii && !wide {
                        r.push(Finding::warning(
                            "slide_id_mismatch",
                            format!("{}: the header does not name slide {id}", self.rel(f)),
                        ));
                    }
                }
            }
        }
        let mut outside = 0usize;
        let mut bad_file = 0usize;
        for lv in &s.levels {
            for it in lv.images.values() {
                match lens.get(it.file as usize).copied().flatten() {
                    Some(len) if it.offset.saturating_add(it.length) <= len => {}
                    Some(_) => {
                        outside += 1;
                        if outside <= 3 {
                            r.push(
                                Finding::error("truncated", format!("level {} image {}: {} bytes at {} run past the end of data file {}", lv.level, it.image, it.length, it.offset, it.file))
                                    .at(it.offset),
                            );
                        }
                    }
                    None => bad_file += 1,
                }
            }
        }
        if outside > 3 {
            r.push(Finding::error(
                "truncated",
                format!("{outside} level images run past the end of their data file"),
            ));
        }
        if bad_file > 0 {
            r.push(Finding::error(
                "missing_file",
                format!("{bad_file} level images are in data files that are missing or not listed"),
            ));
        }
        for (lvl, lv) in s.levels.iter().enumerate() {
            if s.codec(lvl).is_none() {
                r.push(Finding::warning(
                    "unsupported_codec",
                    format!(
                        "level {lvl}: IMAGE_FORMAT {} is not JPEG, PNG or BMP",
                        lv.image_format
                    ),
                ));
            }
            let misplaced = lv
                .images
                .keys()
                .filter(|&&i| {
                    let (gx, gy) = (u64::from(i % s.grid.0), u64::from(i / s.grid.0));
                    gx % lv.step != 0 || gy % lv.step != 0
                })
                .count();
            if misplaced > 0 {
                r.push(Finding::warning(
                    "off_grid_images",
                    format!(
                        "level {lvl}: {misplaced} images are not on the level's {}-cell grid",
                        lv.step
                    ),
                ));
            }
        }
        r.performed("camera positions: table size matches the camera grid; every stored image's camera has a position");
        if s.cameras.is_some() {
            let valid = valid_cameras(s);
            let covered = valid.iter().filter(|v| **v).count();
            if covered == 0 {
                r.push(Finding::error(
                    "no_positions",
                    "no camera with stored images has a usable position",
                ));
            }
            if let Ok(l) = LevelLayout::build(s, 0)
                && l.unplaced > 0
            {
                r.push(Finding::warning(
                    "unplaced_images",
                    format!(
                        "{} level-0 images belong to cameras flagged as empty; they are not drawn",
                        l.unplaced
                    ),
                ));
            }
        }
        r.performed("associated images and records lie inside their data files");
        for rec in &s.nonhier {
            for it in &rec.items {
                if let Some(Some(len)) = lens.get(it.file as usize)
                    && it.offset.saturating_add(it.length) > *len
                {
                    r.push(
                        Finding::error(
                            "truncated",
                            format!(
                                "record {}/{} runs past the end of data file {}",
                                rec.layer, rec.value, it.file
                            ),
                        )
                        .at(it.offset),
                    );
                }
            }
        }
        if full {
            r.performed("every level image: its header declares the level's image size; the first image of each level decodes");
            let mut wrong = 0usize;
            for (lvl, lv) in s.levels.iter().enumerate() {
                let mut first = true;
                let mut keys: Vec<&u32> = lv.images.keys().collect();
                keys.sort_unstable();
                for k in keys {
                    let it = lv.images[k];
                    if lens
                        .get(it.file as usize)
                        .copied()
                        .flatten()
                        .is_none_or(|l| it.offset.saturating_add(it.length) > l)
                    {
                        continue;
                    }
                    let n = it.length.min(4096);
                    let Ok(head) = s.read_item(&self.fs, it.file, it.offset, n) else {
                        continue;
                    };
                    let sniff = ImageCodec::sniff(&head);
                    let size = header_size(&head);
                    if sniff.is_none() || size.is_some_and(|sz| sz != lv.image_size) {
                        wrong += 1;
                        if wrong <= 3 {
                            r.push(Finding::error("bad_image", format!("level {lvl} image {k}: header declares {size:?}, the level's images are {:?}", lv.image_size)).at(it.offset));
                        }
                        continue;
                    }
                    if first {
                        first = false;
                        if let Err(e) = Self::decode(s, &self.fs, lvl, *k) {
                            r.push(
                                Finding::error("bad_image", format!("level {lvl} image {k}: {e}"))
                                    .at(it.offset),
                            );
                        }
                    }
                }
            }
            if wrong > 3 {
                r.push(Finding::error(
                    "bad_image",
                    format!("{wrong} level images have a header that does not match their level"),
                ));
            }
        }
        r
    }
}
