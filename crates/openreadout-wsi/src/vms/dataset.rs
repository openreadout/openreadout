//! `Dataset` for a VMS slide: one RGB pyramid (the slide images at full, half and quarter
//! size, then the map at full to eighth size), the macro photo, the map and the optimisation
//! file as attachments.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::region::{Region, ResolutionLevel, TileCache};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, PixelType, Plane, Result};
use rayon::prelude::*;
use serde_json::{Value, json};

use super::jpeg::split_intervals;
use super::slide::{LevelSource, VmsImage, VmsSlide};
use super::{FORMAT_ID, VmsReader};

/// Decoded intervals kept between region reads.
const TILE_CACHE_BYTES: usize = 256 << 20;
/// Largest map level decoded whole (and kept) instead of interval by interval.
const WHOLE_MAP_BYTES: u64 = 128 << 20;

/// A decoded interval: `width × height` RGB pixels.
type Tile = (u32, u32, Vec<u8>);

/// An opened VMS slide.
pub struct VmsDataset {
    path: PathBuf,
    fs: Fs,
    slide: VmsSlide,
    /// Row start offsets per slide image (index `images.len()` is the map), built on first use.
    rows: Vec<Option<Arc<Vec<u64>>>>,
    tiles: TileCache<(usize, usize, u32, u32), Tile>,
    whole_map: HashMap<u32, Arc<Tile>>,
}

impl std::fmt::Debug for VmsDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VmsDataset")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
enum AttachmentSource {
    File(PathBuf),
}

fn decode(data: &[u8], shift: u32, expect: (u32, u32)) -> Result<Tile> {
    let max = (expect.0 as usize) * (expect.1 as usize) * 3;
    let r = openreadout_codecs::jpeg_decode_reduced(data, shift, max.max(64) << (2 * shift))
        .map_err(|e| Error::corrupt(FORMAT_ID, format!("JPEG interval: {e}")))?;
    if r.channels != 3 || r.bits_per_sample != 8 {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("a JPEG with {} components of {} bits", r.channels, r.bits_per_sample),
            "Only 8-bit three-component JPEG images are known in VMS slides.",
        ));
    }
    if (r.width, r.height) != expect {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "a JPEG interval decodes to {} x {}, expected {} x {}",
                r.width, r.height, expect.0, expect.1
            ),
        ));
    }
    let mut d = r.data;
    d.truncate(expect.0 as usize * expect.1 as usize * 3);
    Ok((r.width, r.height, d))
}

impl VmsDataset {
    /// Open the `.vms` named by `input`.
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let slide = VmsSlide::open(fs, path)?;
        let n = slide.images.len() + 1;
        Ok(VmsDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            slide,
            rows: vec![None; n],
            tiles: TileCache::new(TILE_CACHE_BYTES),
            whole_map: HashMap::new(),
        })
    }

    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(&self.slide.dir)
            .unwrap_or(p)
            .display()
            .to_string()
    }

    fn image(&self, k: usize) -> Result<&VmsImage> {
        if k < self.slide.images.len() {
            Ok(&self.slide.images[k])
        } else {
            self.slide
                .map
                .as_ref()
                .ok_or_else(|| Error::corrupt(FORMAT_ID, "the slide has no map image"))
        }
    }

    fn open_file(&self, im: &VmsImage) -> Result<SourceFile> {
        self.fs.open(&im.path).map_err(|e| Error::io(&im.path, e))
    }

    fn row_index(&mut self, k: usize) -> Result<Arc<Vec<u64>>> {
        if let Some(Some(r)) = self.rows.get(k) {
            return Ok(r.clone());
        }
        let im = self.image(k)?.clone();
        let file = self.open_file(&im)?;
        let which = (k < self.slide.images.len()).then_some(k);
        let r = Arc::new(self.slide.row_index(&file, which)?);
        if let Some(slot) = self.rows.get_mut(k) {
            *slot = Some(r.clone());
        }
        Ok(r)
    }

    /// The entropy-coded bytes of the intervals of MCU row `row` of image `k`, checking that
    /// the row follows the restart marker its position requires.
    fn row_intervals(
        &mut self,
        k: usize,
        row: u32,
    ) -> Result<(Vec<u8>, Vec<std::ops::Range<usize>>)> {
        let starts = self.row_index(k)?;
        let im = self.image(k)?.clone();
        let r = row as usize;
        let start = *starts
            .get(r)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("row {row} has no offset")))?;
        let end = starts.get(r + 1).copied().unwrap_or(im.len);
        let lead = if r > 0 { 2 } else { 0 };
        if end <= start || end > im.len || start < lead {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("{}: row {row} spans bytes {start}..{end}", self.rel(&im.path)),
            ));
        }
        let file = self.open_file(&im)?;
        let mut buf = vec![0u8; (end - start + lead) as usize];
        file.read_exact_at(start - lead, &mut buf)
            .map_err(|e| Error::io(&im.path, e))?;
        let per_row = im.layout.intervals_per_row();
        if r > 0 {
            let want = 0xD0 + ((u64::from(row) * u64::from(per_row) - 1) % 8) as u8;
            if buf[0] != 0xFF || buf[1] != want {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    start,
                    format!(
                        "{}: MCU row {row} does not follow restart marker {:02X} (found {:02X} {:02X}); the optimisation file or the image is damaged",
                        self.rel(&im.path),
                        want,
                        buf[0],
                        buf[1]
                    ),
                ));
            }
        }
        let data = buf.split_off(lead as usize);
        let ranges = split_intervals(&data);
        if ranges.len() < per_row as usize {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                start,
                format!(
                    "{}: MCU row {row} holds {} restart intervals, expected {per_row}",
                    self.rel(&im.path),
                    ranges.len()
                ),
            ));
        }
        Ok((data, ranges))
    }

    fn level_origins(&self, shift: u32) -> (Vec<u64>, Vec<u64>) {
        let s = &self.slide;
        let (cols, rows) = s.grid;
        let mut xs = vec![0u64];
        for x in 0..cols {
            let w = u64::from(s.images[x as usize].layout.width).div_ceil(1 << shift);
            xs.push(xs[x as usize] + w);
        }
        let mut ys = vec![0u64];
        for y in 0..rows {
            let h = u64::from(s.images[(y * cols) as usize].layout.height).div_ceil(1 << shift);
            ys.push(ys[y as usize] + h);
        }
        (xs, ys)
    }

    fn whole_map(&mut self, shift: u32) -> Result<Arc<Tile>> {
        if let Some(t) = self.whole_map.get(&shift) {
            return Ok(t.clone());
        }
        let im = self.image(self.slide.images.len())?.clone();
        let data = self
            .fs
            .read(&im.path)
            .map_err(|e| Error::io(&im.path, e))?;
        let expect = (
            im.layout.width.div_ceil(1 << shift),
            im.layout.height.div_ceil(1 << shift),
        );
        let t = Arc::new(decode(&data, shift, expect)?);
        self.whole_map.insert(shift, t.clone());
        Ok(t)
    }

    #[allow(clippy::too_many_lines)]
    fn read_level_region(
        &mut self,
        index: PlaneIndex,
        level: u32,
        region: Option<Region>,
    ) -> Result<Plane> {
        if index.c != 0 || index.z != 0 || index.t != 0 {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} is out of range (C=1, Z=1, T=1)",
                index.c, index.z, index.t
            )));
        }
        let lv = *self.slide.levels.get(level as usize).ok_or_else(|| {
            Error::Usage(format!(
                "pyramid level {level} out of range (0..{})",
                self.slide.levels.len()
            ))
        })?;
        let (lw, lh) = (
            u32::try_from(lv.size.0).unwrap_or(u32::MAX),
            u32::try_from(lv.size.1).unwrap_or(u32::MAX),
        );
        let win = match region {
            Some(r) => {
                r.check_within(lw, lh, &format!("image 0 level {level}"))?;
                r
            }
            None => Region::full(lw, lh),
        };
        let n = openreadout_core::pixel::plane_bytes_checked(FORMAT_ID, win.width, win.height, 3)
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
        let mut data = vec![255u8; n];
        let row_bytes = win.width as usize * 3;
        let (wx0, wy0) = (u64::from(win.x), u64::from(win.y));
        let (wx1, wy1) = (wx0 + u64::from(win.width), wy0 + u64::from(win.height));
        let s = lv.shift;
        let paste = |data: &mut [u8], ox: u64, oy: u64, t: &Tile| {
            let (tw, th) = (u64::from(t.0), u64::from(t.1));
            let x0 = ox.max(wx0);
            let x1 = (ox + tw).min(wx1);
            let y0 = oy.max(wy0);
            let y1 = (oy + th).min(wy1);
            if x0 >= x1 || y0 >= y1 {
                return;
            }
            let cw = ((x1 - x0) * 3) as usize;
            for y in y0..y1 {
                let src = (((y - oy) * tw + (x0 - ox)) * 3) as usize;
                let dst = (y - wy0) as usize * row_bytes + ((x0 - wx0) * 3) as usize;
                data[dst..dst + cw].copy_from_slice(&t.2[src..src + cw]);
            }
        };
        if lv.source == LevelSource::Map && s > 0 && lv.size.0 * lv.size.1 * 3 <= WHOLE_MAP_BYTES {
            let t = self.whole_map(s)?;
            paste(&mut data, 0, 0, &t);
            return Ok(Plane {
                width: win.width,
                height: win.height,
                pixel_type: PixelType::Uint8,
                samples_per_pixel: 3,
                data,
            });
        }
        // Images and their origins at this level.
        let images: Vec<(usize, u64, u64)> = match lv.source {
            LevelSource::Map => vec![(self.slide.images.len(), 0, 0)],
            LevelSource::Slide => {
                let (xs, ys) = self.level_origins(s);
                self.slide
                    .images
                    .iter()
                    .enumerate()
                    .map(|(k, im)| (k, xs[im.cell.0 as usize], ys[im.cell.1 as usize]))
                    .collect()
            }
        };
        for (k, ox, oy) in images {
            let im = self.image(k)?.clone();
            let l = &im.layout;
            let (iw, ih) = (
                u64::from(l.width).div_ceil(1 << s),
                u64::from(l.height).div_ceil(1 << s),
            );
            if ox >= wx1 || oy >= wy1 || ox + iw <= wx0 || oy + ih <= wy0 {
                continue;
            }
            let tw = u64::from(l.interval_width()) >> s;
            let th = u64::from(l.mcu_h) >> s;
            if tw == 0 || th == 0 {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "an interval smaller than the reduction",
                    "Read a level that is not reduced below one pixel per interval.",
                ));
            }
            let r0 = (wy0.max(oy) - oy) / th;
            let r1 = (wy1.min(oy + ih) - 1 - oy) / th;
            let k0 = (wx0.max(ox) - ox) / tw;
            let k1 = (wx1.min(ox + iw) - 1 - ox) / tw;
            // Gather the intervals that are not cached, decode them in parallel.
            let mut jobs: Vec<(u32, u32, Vec<u8>, (u32, u32))> = Vec::new();
            let mut ready: Vec<((u32, u32), Arc<Tile>)> = Vec::new();
            for row in r0..=r1 {
                let row = row as u32;
                let mut missing = Vec::new();
                for kk in k0..=k1 {
                    let key = (level as usize, k, row, kk as u32);
                    match self.tiles.get(&key) {
                        Some(t) => ready.push(((row, kk as u32), t)),
                        None => missing.push(kk as u32),
                    }
                }
                if missing.is_empty() {
                    continue;
                }
                let (bytes, ranges) = self.row_intervals(k, row)?;
                for kk in missing {
                    let (fw, fh) = l.interval_size(row, kk);
                    let frame = l.frame(fw, fh, &bytes[ranges[kk as usize].clone()]);
                    jobs.push((row, kk, frame, (fw.div_ceil(1 << s), fh.div_ceil(1 << s))));
                }
            }
            let decoded: Vec<Result<((u32, u32), Tile)>> = jobs
                .par_iter()
                .map(|(row, kk, frame, expect)| decode(frame, s, *expect).map(|t| ((*row, *kk), t)))
                .collect();
            for r in decoded {
                let (pos, t) = r?;
                let t = Arc::new(t);
                let bytes = t.2.len();
                self.tiles.put((level as usize, k, pos.0, pos.1), t.clone(), bytes);
                ready.push((pos, t));
            }
            for ((row, kk), t) in ready {
                paste(
                    &mut data,
                    ox + u64::from(kk) * tw,
                    oy + u64::from(row) * th,
                    &t,
                );
            }
        }
        Ok(Plane {
            width: win.width,
            height: win.height,
            pixel_type: PixelType::Uint8,
            samples_per_pixel: 3,
            data,
        })
    }

    fn image_info(&self) -> ImageInfo {
        let s = &self.slide;
        let (w, h) = (
            u32::try_from(s.size.0).unwrap_or(u32::MAX),
            u32::try_from(s.size.1).unwrap_or(u32::MAX),
        );
        let mut info = ImageInfo::new(0, w, h, PixelType::Uint8);
        info.name = s.get("Reference").map(str::to_string);
        info.samples_per_pixel = 3;
        info.dimension_order = "XYCZT".into();
        info.pyramid_levels = s.levels.len() as u32;
        info.resolution_levels = s
            .levels
            .iter()
            .enumerate()
            .map(|(i, lv)| {
                let l = match lv.source {
                    LevelSource::Slide => &s.images[0].layout,
                    LevelSource::Map => &s.map.as_ref().unwrap_or(&s.images[0]).layout,
                };
                let mut r = ResolutionLevel::new(
                    i as u32,
                    u32::try_from(lv.size.0).unwrap_or(u32::MAX),
                    u32::try_from(lv.size.1).unwrap_or(u32::MAX),
                    w,
                    h,
                )
                .with_tile(l.interval_width() >> lv.shift, (l.mcu_h >> lv.shift).max(1));
                r.downsample_x = lv.downsample as f64;
                r.downsample_y = lv.downsample as f64;
                r
            })
            .collect();
        let nm = |k: &str| s.settings().and_then(|x| x.f64(k)).filter(|v| *v > 0.0);
        info.physical_size = PhysicalSize::micrometres(
            nm("PhysicalWidth").map(|v| v / 1000.0 / f64::from(w.max(1))),
            nm("PhysicalHeight").map(|v| v / 1000.0 / f64::from(h.max(1))),
            None,
        );
        info.channels = vec![ChannelInfo {
            index: 0,
            acquisition_mode: Some("Brightfield (RGB)".into()),
            ..ChannelInfo::default()
        }];
        if let Some(mag) = nm("SourceLens") {
            info.objective = Some(ObjectiveInfo {
                nominal_magnification: Some(mag),
                ..ObjectiveInfo::default()
            });
        }
        info.instrument = Some(InstrumentInfo {
            manufacturer: Some("Hamamatsu".into()),
            ..InstrumentInfo::default()
        });
        let mut m = serde_json::Map::new();
        m.insert("image_grid".into(), json!([s.grid.0, s.grid.1]));
        m.insert(
            "image_files".into(),
            json!(
                s.images
                    .iter()
                    .map(|im| json!({
                        "file": self.rel(&im.path),
                        "cell": [im.cell.0, im.cell.1],
                        "size": [im.layout.width, im.layout.height],
                        "restart_interval_px": [im.layout.interval_width(), im.layout.mcu_h],
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        if let Some(mp) = &s.map {
            m.insert("map_file".into(), json!(self.rel(&mp.path)));
            m.insert("map_size".into(), json!([mp.layout.width, mp.layout.height]));
        }
        m.insert("map_downsample".into(), json!(s.map_downsample));
        m.insert(
            "levels".into(),
            json!(
                s.levels
                    .iter()
                    .map(|lv| json!({
                        "source": match lv.source { LevelSource::Slide => "slide images", LevelSource::Map => "map" },
                        "dct_reduction": 1u32 << lv.shift,
                    }))
                    .collect::<Vec<_>>()
            ),
        );
        for (k, key) in [
            ("physical_width_nm", "PhysicalWidth"),
            ("physical_height_nm", "PhysicalHeight"),
            ("x_offset_from_slide_centre_nm", "XOffsetFromSlideCentre"),
            ("y_offset_from_slide_centre_nm", "YOffsetFromSlideCentre"),
            ("physical_macro_width_nm", "PhysicalMacroWidth"),
            ("physical_macro_height_nm", "PhysicalMacroHeight"),
        ] {
            if let Some(v) = s.settings().and_then(|x| x.f64(key)) {
                m.insert(k.into(), json!(v));
            }
        }
        m.insert(
            "optimisation_file".into(),
            json!(s.get("OptimisationFile").is_some()),
        );
        info.extra.insert("vms".into(), Value::Object(m));
        info.extra.insert("codec".into(), json!("jpeg"));
        info.finish()
    }

    fn attachment_list(&self) -> Vec<(AttachmentInfo, AttachmentSource)> {
        let mut out = Vec::new();
        let s = &self.slide;
        for (name, key, ct, ext) in [
            ("macro", "MacroImage", "JPG", "jpg"),
            ("map", "MapFile", "JPG", "jpg"),
            ("optimisation file", "OptimisationFile", "BIN", "opt"),
        ] {
            let Some(f) = s.get(key) else { continue };
            let p = s.dir.join(f);
            let Ok(m) = self.fs.metadata(&p) else { continue };
            let mut extra = BTreeMap::new();
            extra.insert("file".into(), json!(f));
            out.push((
                AttachmentInfo {
                    index: out.len() as u32,
                    name: name.into(),
                    content_type: ct.into(),
                    extension: ext.into(),
                    offset: Some(0),
                    size: m.len(),
                    extra,
                },
                AttachmentSource::File(p),
            ));
        }
        out
    }

    fn check_impl(&mut self, full: bool) -> CheckReport {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("settings: the grid, every ImageFile present with a baseline JPEG header with restart intervals, columns and rows of equal size");
        r.performed("map and optimisation file: present and consistent");
        for p in self.slide.problems.clone() {
            r.push(Finding::warning("map_or_optimisation", p));
        }
        for key in ["MacroImage", "MapFile", "OptimisationFile"] {
            if let Some(f) = self.slide.get(key)
                && !self.fs.is_file(&self.slide.dir.join(f))
            {
                r.push(Finding::warning("missing_file", format!("{key} {f} is missing")));
            }
        }
        if full {
            r.performed("every image: all MCU rows found, the first and last row follow their restart markers and their first and last intervals decode");
            let n = self.slide.images.len() + usize::from(self.slide.map.is_some());
            for k in 0..n {
                let Ok(im) = self.image(k).cloned() else { continue };
                let rows = im.layout.rows();
                for row in [0, rows.saturating_sub(1)] {
                    match self.row_intervals(k, row) {
                        Ok((bytes, ranges)) => {
                            let last = im.layout.intervals_per_row() - 1;
                            for kk in [0, last] {
                                let (fw, fh) = im.layout.interval_size(row, kk);
                                let f = im.layout.frame(fw, fh, &bytes[ranges[kk as usize].clone()]);
                                if let Err(e) = decode(&f, 0, (fw, fh)) {
                                    r.push(Finding::error("bad_interval", format!("{} row {row} interval {kk}: {e}", self.rel(&im.path))));
                                }
                            }
                        }
                        Err(e) => r.push(Finding::error("bad_rows", e.to_string())),
                    }
                }
            }
        }
        r
    }
}

impl Dataset for VmsDataset {
    fn member_files(&self) -> Vec<PathBuf> {
        let s = &self.slide;
        let mut out: Vec<PathBuf> = s.images.iter().map(|i| i.path.clone()).collect();
        for key in ["MapFile", "MacroImage", "OptimisationFile"] {
            if let Some(f) = s.get(key) {
                out.push(s.dir.join(f));
            }
        }
        out.retain(|p| self.fs.is_file(p));
        out
    }

    fn info(&self) -> Result<FileInfo> {
        let image = self.image_info();
        let plane_count = image.plane_count;
        let mut notes = vec![
            "a VMS slide: a grid of JPEG images read by restart interval; read a region with `--region X,Y,W,H` or a coarser level with `--level N` (level 0 is too large to read whole)".to_string(),
            "levels 1 and 2 are the slide images decoded at 1/2 and 1/4 size, the next levels the map at full to 1/8 size (reduced inverse DCT, as OpenSlide does); only level 0 and the map's full size are stored samples".to_string(),
        ];
        if !self.slide.problems.is_empty() {
            notes.push(format!(
                "{} problem(s) found while opening; run `check`",
                self.slide.problems.len()
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: VmsReader.descriptor(),
            format_version: None,
            images: vec![image],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({ "vms": self.slide.ini.to_json() }))
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
            "images[].extra.vms",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        p.insert("images[].name".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let s = &self.slide;
        let mut out = vec![LsEntry {
            kind: "metadata".into(),
            name: self.rel(&self.path),
            offset: None,
            size: None,
            image: None,
            details: json!({"keys": s.settings().map_or(0, |x| x.entries.len())}),
        }];
        for im in &s.images {
            out.push(LsEntry {
                kind: "file".into(),
                name: self.rel(&im.path),
                offset: Some(0),
                size: Some(im.len),
                image: Some(0),
                details: json!({"cell": [im.cell.0, im.cell.1], "size": [im.layout.width, im.layout.height],
                                "rows": im.layout.rows(), "intervals_per_row": im.layout.intervals_per_row()}),
            });
        }
        for (i, lv) in s.levels.iter().enumerate() {
            out.push(LsEntry {
                kind: "pyramid-level".into(),
                name: format!("level {i}"),
                offset: None,
                size: None,
                image: Some(0),
                details: json!({"level": i, "size_x": lv.size.0, "size_y": lv.size.1, "downsample": lv.downsample,
                                "source": match lv.source { LevelSource::Slide => "slide images", LevelSource::Map => "map" },
                                "dct_reduction": 1u32 << lv.shift}),
            });
        }
        for (a, _) in self.attachment_list() {
            out.push(LsEntry {
                kind: "attachment".into(),
                name: a.name.clone(),
                offset: None,
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
        let AttachmentSource::File(p) = src;
        self.fs.read(p).map_err(|e| Error::io(p, e))
    }
}
