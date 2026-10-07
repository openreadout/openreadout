//! `Dataset` implementation: one image per stack that has an ETS file, tiles stitched per
//! pyramid level, metadata from the `.vsi` record tree.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, MosaicInfo, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::region::{PlacedTile, Region, ResolutionLevel, TileCache, paste_into_region};
use openreadout_core::source::{Fs, Input};
use openreadout_core::time::unix_to_iso8601;
use openreadout_core::{Error, PixelType, Plane, Result};
use rayon::prelude::*;
use serde_json::{Value, json};

use crate::ets::{EtsFile, Tile};
use crate::meta::{DimKind, StackMeta, VsiMeta, read_meta};
use crate::tiff::{PreviewInfo, preview_info, preview_jpeg};
use crate::tree::TagTree;
use crate::{FORMAT_ID, VsiReader};

/// Largest `.vsi` we load into memory (the record tree and preview are small).
const MAX_VSI_BYTES: u64 = 512 << 20;
/// Largest plane we assemble in memory; bigger levels must be read at a coarser level.
const MAX_PLANE_BYTES: u64 = 4 << 30;
/// Decoded tiles kept per image between region reads.
const REGION_TILE_CACHE_BYTES: usize = 64 << 20;
/// Decoded tile bytes held at once while a plane or region is assembled.
const DECODE_BATCH_BYTES: usize = 64 << 20;

/// Which of c, z, t a non-XY dimension feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    C,
    Z,
    T,
}

/// Where an image's level-0 size comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SizeSource {
    /// The ETS header's size list.
    EtsHeader,
    /// The `.vsi` layout rectangle (ETS headers of version 0x00030003 have no size list).
    Vsi,
    /// The extent of the stored level-0 tiles (a standalone ETS without a size list): may
    /// include background padding at the right and bottom.
    TileGrid,
}

impl SizeSource {
    fn name(self) -> &'static str {
        match self {
            SizeSource::EtsHeader => "ets_header",
            SizeSource::Vsi => "vsi_layout",
            SizeSource::TileGrid => "tile_grid",
        }
    }
}

/// One exposed image: a stack with its ETS file.
#[derive(Debug)]
struct StackImage {
    meta: Option<StackMeta>,
    ets: EtsFile,
    /// Other files in the stack directory (further `frame_t*.ets`, `blob_*` focus data).
    other_files: Vec<PathBuf>,
    /// Per non-XY coordinate: kind, group it feeds, size.
    dims: Vec<(DimKind, Group, u32)>,
    pixel_type: Option<PixelType>,
    spp: u32,
    level_sizes: Vec<(u32, u32)>,
    /// Level-0 pixel position of tile column 0, row 0 (`.vsi` record 2410).
    origin: (i64, i64),
    /// Where the level-0 size came from.
    size_source: SizeSource,
    /// Width and height the `.vsi` records for the stack (record 2053), when it does.
    vsi_size: Option<(u32, u32)>,
    /// `(level, non-XY indices)` → tile indices.
    tile_index: HashMap<(u32, Vec<u32>), Vec<usize>>,
    /// Decoded tiles kept between region reads, keyed by file offset.
    cache: TileCache<u64, Vec<u8>>,
}

impl StackImage {
    fn group_size(&self, g: Group) -> u32 {
        self.dims
            .iter()
            .filter(|d| d.1 == g)
            .map(|d| d.2.max(1))
            // Sizes come from tile coordinates in the file: saturate, never overflow.
            .fold(1u32, u32::saturating_mul)
    }

    /// Non-XY coordinates of plane (c, z, t).
    fn coords(&self, idx: PlaneIndex) -> Vec<u32> {
        let mut rest = [idx.c, idx.z, idx.t];
        self.dims
            .iter()
            .map(|(_, g, n)| {
                let slot = match g {
                    Group::C => 0,
                    Group::Z => 1,
                    Group::T => 2,
                };
                let n = (*n).max(1);
                let v = rest[slot] % n;
                rest[slot] /= n;
                v
            })
            .collect()
    }

    /// (c, z, t) of the plane whose non-XY coordinates are `coords`.
    fn plane_of(&self, coords: &[u32]) -> PlaneIndex {
        let mut out = [0u32; 3];
        let mut mul = [1u32; 3];
        for ((_, g, n), v) in self.dims.iter().zip(coords) {
            let slot = match g {
                Group::C => 0,
                Group::Z => 1,
                Group::T => 2,
            };
            out[slot] += v * mul[slot];
            mul[slot] *= (*n).max(1);
        }
        PlaneIndex {
            c: out[0],
            z: out[1],
            t: out[2],
        }
    }

    fn channel_dim(&self) -> Option<usize> {
        self.dims.iter().position(|d| d.0 == DimKind::Channel)
    }

    fn name(&self) -> String {
        self.meta
            .as_ref()
            .and_then(|m| m.name.clone())
            .unwrap_or_else(|| {
                self.ets
                    .path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map_or_else(String::new, |n| n.to_string_lossy().to_string())
            })
    }
}

/// An opened `.vsi` (with its ETS files) or a standalone `.ets`.
#[derive(Debug)]
pub struct VsiDataset {
    path: PathBuf,
    /// Where the `.vsi` and its `.ets` files are read from.
    fs: Fs,
    /// `vsi` or `ets`.
    kind: &'static str,
    tree: Option<TagTree>,
    meta: VsiMeta,
    preview: Option<PreviewInfo>,
    /// `_<stem>_` directory, when found.
    ets_dir: Option<PathBuf>,
    images: Vec<StackImage>,
    open_findings: Vec<Finding>,
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().to_string())
}

/// Entry of `dir` whose name equals `want`, case-insensitively.
fn find_ci(fs: &Fs, dir: &Path, want: &str) -> Option<PathBuf> {
    let direct = dir.join(want);
    if fs.exists(&direct) {
        return Some(direct);
    }
    fs.read_dir(dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(want))
        })
}

/// `frame_t*.ets` files of a stack directory (sorted), and every other file.
fn stack_files(fs: &Fs, dir: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut frames = Vec::new();
    let mut other = Vec::new();
    if let Ok(rd) = fs.read_dir(dir) {
        for e in rd.filter_map(std::result::Result::ok) {
            let p = e.path();
            let n = file_name(&p).to_ascii_lowercase();
            if n.starts_with("frame_t")
                && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ets"))
            {
                frames.push(p);
            } else if fs.is_file(&p) {
                other.push(p);
            }
        }
    }
    frames.sort();
    other.sort();
    (frames, other)
}

/// Most pyramid levels an image can have (u32 sizes halve to 1 within 33 levels).
const MAX_LEVELS: u32 = 33;

/// `origin` (the level-0 pixel position of tile column 0, row 0) at pyramid level `level`:
/// divided by 2^level, rounded to the nearest pixel with halves toward zero
/// (`docs/formats/vsi.md`, *Tile placement*).
fn level_origin(origin: (i64, i64), level: u32) -> (i64, i64) {
    let f = 1i64 << level.min(40);
    let div = |v: i64| {
        let q = v / f; // toward zero
        let r = v % f;
        if 2 * r.abs() > f { q + v.signum() } else { q }
    };
    (div(origin.0), div(origin.1))
}

/// Level 0 is `(w, h)`; level L halves level L - 1 (rounding up) and is clipped to the extent
/// of the tiles stored at that level (slides do not store empty margins; Bio-Formats reports the
/// same sizes where the tile grid starts at the image corner).
fn level_sizes(
    w: u32,
    h: u32,
    tiles: &[Tile],
    tw: u32,
    th: u32,
    origin: (i64, i64),
) -> Vec<(u32, u32)> {
    // Each level halves the previous one, so a u32-sized image has at most 33 levels; a tile
    // claiming level 2^32 - 1 must not make us build billions of them.
    let levels = tiles
        .iter()
        .map(Tile::level)
        .max()
        .map_or(1, |m| m.saturating_add(1))
        .min(MAX_LEVELS);
    let mut out = vec![(w, h)];
    for l in 1..levels {
        let (pw, ph) = *out.last().expect("level 0");
        let (mut nw, mut nh) = (pw.div_ceil(2).max(1), ph.div_ceil(2).max(1));
        let (ox, oy) = level_origin(origin, l);
        let at = tiles.iter().filter(|t| t.level() == l);
        // Right and bottom edges of the stored tiles, in level pixels.
        let right = at
            .clone()
            .map(|t| (t.column() + 1) * i64::from(tw) + ox)
            .max();
        let bottom = at.map(|t| (t.row() + 1) * i64::from(th) + oy).max();
        if let (Some(r), Some(b)) = (right, bottom)
            && r > 0
            && b > 0
        {
            nw = nw.min(u32::try_from(r).unwrap_or(u32::MAX)).max(1);
            nh = nh.min(u32::try_from(b).unwrap_or(u32::MAX)).max(1);
        }
        out.push((nw, nh));
    }
    out
}

fn build_image(meta: Option<StackMeta>, ets: EtsFile, other_files: Vec<PathBuf>) -> StackImage {
    let h = &ets.header;
    let extra = h.extra_dims();
    let dims = (0..extra)
        .map(|i| {
            let kind = meta
                .as_ref()
                .and_then(|m| m.dims.get(i))
                .map_or(DimKind::Unknown, |d| d.kind);
            let size = h.sizes.get(2 + i).copied().unwrap_or_else(|| {
                ets.tiles
                    .iter()
                    .filter_map(|t| t.extra().get(i).copied())
                    .max()
                    .map_or(1, |m| m.saturating_add(1))
            });
            let group = match kind {
                DimKind::Channel => Group::C,
                DimKind::Z => Group::Z,
                _ => Group::T,
            };
            (kind, group, size.max(1))
        })
        .collect();
    let (tw, th) = (i64::from(h.tile_width), i64::from(h.tile_height));
    let level0 = || ets.tiles.iter().filter(|t| t.level() == 0);
    let rect = meta
        .as_ref()
        .and_then(|m| m.image_rect)
        .and_then(|r| Some((u32::try_from(r[2]).ok()?, u32::try_from(r[3]).ok()?)));
    // Where tile (0, 0) starts: the .vsi says so; a standalone ETS whose tiles have negative
    // indices is placed from its smallest column and row.
    let origin = match meta.as_ref().and_then(|m| m.tile_origin) {
        Some(o) => o,
        None => (
            -level0().map(Tile::column).min().unwrap_or(0).min(0) * tw,
            -level0().map(Tile::row).min().unwrap_or(0).min(0) * th,
        ),
    };
    let ((w0, h0), size_source) = match (h.sizes.first(), h.sizes.get(1), rect) {
        (Some(w), Some(hh), _) if *w > 0 && *hh > 0 => ((*w, *hh), SizeSource::EtsHeader),
        (_, _, Some(r)) => (r, SizeSource::Vsi),
        _ => {
            let extent = |right: Option<i64>| {
                right
                    .and_then(|r| u32::try_from(r.max(1)).ok())
                    .unwrap_or(u32::MAX)
            };
            (
                (
                    extent(level0().map(|t| (t.column() + 1) * tw + origin.0).max()),
                    extent(level0().map(|t| (t.row() + 1) * th + origin.1).max()),
                ),
                SizeSource::TileGrid,
            )
        }
    };
    let mut tile_index: HashMap<(u32, Vec<u32>), Vec<usize>> = HashMap::new();
    for (i, t) in ets.tiles.iter().enumerate() {
        tile_index
            .entry((t.level(), t.extra().to_vec()))
            .or_default()
            .push(i);
    }
    StackImage {
        pixel_type: h.pixel_type(),
        spp: h.samples_per_pixel.max(1),
        level_sizes: level_sizes(w0, h0, &ets.tiles, h.tile_width, h.tile_height, origin),
        origin,
        size_source,
        vsi_size: rect,
        meta,
        dims,
        tile_index,
        cache: TileCache::new(REGION_TILE_CACHE_BYTES),
        ets,
        other_files,
    }
}

impl VsiDataset {
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let head = {
            use std::io::Read;
            let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
            let mut b = vec![0u8; 128];
            let n = f.read(&mut b).map_err(|e| Error::io(path, e))?;
            b.truncate(n);
            b
        };
        if crate::ets::looks_like_ets(&head) {
            return Self::open_ets(fs, path);
        }
        Self::open_vsi(fs, path)
    }

    fn open_ets(fs: &Fs, path: &Path) -> Result<Self> {
        let ets = EtsFile::open_in(fs, path)?;
        let img = build_image(None, ets, Vec::new());
        Ok(VsiDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            kind: "ets",
            tree: None,
            meta: VsiMeta::default(),
            preview: None,
            ets_dir: None,
            images: vec![img],
            open_findings: Vec::new(),
        })
    }

    fn open_vsi(fs: &Fs, path: &Path) -> Result<Self> {
        let len = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
        if len > MAX_VSI_BYTES {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(".vsi file of {len} bytes"),
                "A .vsi normally holds metadata and a small preview (≤ a few MB); pixel data lives in the _<name>_ directory of .ets files.",
            ));
        }
        let data = fs.read(path).map_err(|e| Error::io(path, e))?;
        if !crate::tiff::looks_like_vsi(&data) {
            return Err(Error::corrupt(
                FORMAT_ID,
                "not a cellSens VSI: expected a little-endian TIFF header followed by an 'IS' record set at byte 8",
            ));
        }
        let preview = preview_info(&data);
        let tree = TagTree::parse(data);
        let meta = read_meta(&tree);
        let mut findings = Vec::new();
        let stem = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().to_string());
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        let ets_dir = find_ci(fs, dir, &format!("_{stem}_")).filter(|p| fs.is_dir(p));
        let mut images = Vec::new();
        if let Some(ed) = &ets_dir {
            // stacks named in the tree first (in tree order), then directories the tree does not name
            let mut dirs: Vec<(u32, PathBuf)> = fs
                .read_dir(ed)
                .map(|rd| {
                    rd.filter_map(std::result::Result::ok)
                        .map(|e| e.path())
                        .filter(|p| fs.is_dir(p))
                        .filter_map(|p| {
                            let n = file_name(&p);
                            let id: u32 = n.strip_prefix("stack")?.parse().ok()?;
                            Some((id, p))
                        })
                        .collect()
                })
                .unwrap_or_default();
            dirs.sort();
            let mut ordered: Vec<(u32, PathBuf, Option<StackMeta>)> = Vec::new();
            for s in &meta.stacks {
                if let Some(pos) = dirs.iter().position(|(id, _)| *id == s.id) {
                    let (id, p) = dirs.remove(pos);
                    ordered.push((id, p, Some(s.clone())));
                }
            }
            for (id, p) in dirs {
                findings.push(Finding::warning(
                    "unnamed_stack",
                    format!("directory stack{id} is not described in the .vsi; exposed without names or calibration"),
                ));
                ordered.push((id, p, None));
            }
            for (id, p, m) in ordered {
                let (frames, other) = stack_files(fs, &p);
                let Some(first) = frames.first() else {
                    findings.push(Finding::warning(
                        "missing_ets",
                        format!("stack{id} has no frame_t*.ets file"),
                    ));
                    continue;
                };
                if frames.len() > 1 {
                    findings.push(Finding::warning(
                        "unexpected_ets_files",
                        format!(
                            "stack{id} holds {} frame_t*.ets files; only {} is read",
                            frames.len(),
                            file_name(first)
                        ),
                    ));
                }
                match EtsFile::open_in(fs, first) {
                    Ok(ets) => {
                        let mut other = other;
                        other.extend(frames.iter().skip(1).cloned());
                        images.push(build_image(m, ets, other));
                    }
                    Err(e) => findings.push(Finding::error(
                        "bad_ets",
                        format!("stack{id}/{}: {e}", file_name(first)),
                    )),
                }
            }
        }
        Ok(VsiDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            kind: "vsi",
            tree: Some(tree),
            meta,
            preview,
            ets_dir,
            images,
            open_findings: findings,
        })
    }

    fn image(&self, image: u32) -> Result<&StackImage> {
        self.images.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image {image} does not exist (file has {} image(s))",
                self.images.len()
            ))
        })
    }

    fn rel(&self, p: &Path) -> String {
        let base = self.path.parent().unwrap_or_else(|| Path::new(""));
        p.strip_prefix(base).unwrap_or(p).display().to_string()
    }

    #[allow(clippy::many_single_char_names)]
    fn image_info(&self, index: u32, s: &StackImage) -> Result<ImageInfo> {
        let pt = s.pixel_type.ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("ETS sample type {}", s.ets.header.sample_type),
                "Only 8-bit (2) and 16-bit (4) ETS samples are known.",
            )
        })?;
        let (w, h) = s.level_sizes[0];
        let mut info = ImageInfo::new(index, w, h, pt);
        info.name = Some(s.name());
        info.samples_per_pixel = s.spp;
        info.size_c = s.group_size(Group::C);
        info.size_z = s.group_size(Group::Z);
        info.size_t = s.group_size(Group::T);
        info.dimension_order = "XYCZT".into();
        info.pyramid_levels = s.level_sizes.len() as u32;
        let (tw, th) = (s.ets.header.tile_width, s.ets.header.tile_height);
        info.resolution_levels = s
            .level_sizes
            .iter()
            .enumerate()
            .map(|(l, &(lw, lh))| {
                // Level L halves level L - 1: its scale is 2^L even where it is clipped to the
                // tiles stored at it (then the size ratio is larger; docs/formats/vsi.md).
                let mut r = ResolutionLevel::new(l as u32, lw, lh, w, h).with_tile(tw, th);
                let f = 2f64.powi(i32::try_from(l).unwrap_or(i32::MAX).min(62));
                r.downsample_x = f;
                r.downsample_y = f;
                r
            })
            .collect();
        let m = s.meta.as_ref();
        let z_step = m
            .and_then(|m| m.dims.iter().find(|d| d.kind == DimKind::Z))
            .and_then(|d| d.z_step_um)
            .map(f64::abs)
            .filter(|v| *v > 0.0 && info.size_z > 1);
        info.physical_size = PhysicalSize::micrometres(
            m.and_then(|m| m.pixel_size_um)
                .map(|p| p.0)
                .filter(|v| *v > 0.0),
            m.and_then(|m| m.pixel_size_um)
                .map(|p| p.1)
                .filter(|v| *v > 0.0),
            z_step,
        );
        if info.size_t > 1
            && let Some(m) = m
        {
            let k0 = Self::plane_record(s, PlaneIndex::default());
            let k1 = Self::plane_record(s, PlaneIndex { c: 0, z: 0, t: 1 });
            if let (Some(Some(a)), Some(Some(b))) =
                (m.plane_times_ms.get(k0), m.plane_times_ms.get(k1))
                && b > a
            {
                info.time_increment_s = Some((b - a) / 1000.0);
            }
        }
        let chans = s
            .channel_dim()
            .and_then(|i| m.and_then(|m| m.dims.get(i)))
            .map(|d| d.channels.clone())
            .unwrap_or_default();
        info.channels = (0..info.size_c)
            .map(|k| {
                let c = chans.get(k as usize);
                ChannelInfo {
                    index: k,
                    // Stacks without a channel dimension name their one channel in the
                    // settings (`BF`, `Label Scan`).
                    name: c.and_then(|c| c.name.clone()).or_else(|| {
                        (chans.is_empty() && info.size_c == 1)
                            .then(|| m.and_then(|m| m.setting_name.clone()))
                            .flatten()
                    }),
                    emission_nm: c.and_then(|c| c.emission_nm),
                    excitation_nm: c.and_then(|c| c.excitation_nm),
                    acquisition_mode: (s.spp == 3).then(|| "Brightfield (RGB)".to_string()),
                    // A channel's own camera record, else the stack's.
                    exposure_ms: c
                        .and_then(|c| c.exposure_ms)
                        .or_else(|| m.and_then(|m| m.exposure_ms)),
                    ..ChannelInfo::default()
                }
            })
            .collect();
        if let Some(o) = m.and_then(|m| m.objective.as_ref()) {
            info.objective = Some(ObjectiveInfo {
                model: o.name.clone(),
                nominal_magnification: o.magnification,
                lens_na: o.numerical_aperture,
                immersion: None,
            });
        }
        if self.kind == "vsi" {
            info.instrument = Some(InstrumentInfo {
                manufacturer: Some("Olympus/Evident".into()),
                model: m.and_then(|m| m.microscope.clone()),
                software: self.meta.software.clone(),
                software_version: self.meta.software_version.clone(),
                detector: m
                    .and_then(|m| m.camera.clone())
                    .or_else(|| self.preview.as_ref().and_then(|p| p.model.clone())),
            });
        }
        info.acquired_at = m
            .and_then(|m| m.acquired_unix)
            .filter(|t| *t > 0)
            .map(|t| unix_to_iso8601(t, 0));
        let level0_tiles = s.ets.tiles.iter().filter(|t| t.level() == 0).count();
        let per_plane = level0_tiles / (info.size_c * info.size_z * info.size_t).max(1) as usize;
        if per_plane > 1 {
            info.mosaic = Some(MosaicInfo {
                tile_count: per_plane as u32,
                tile_width: Some(s.ets.header.tile_width),
                tile_height: Some(s.ets.header.tile_height),
                stitched_on_read: true,
            });
        }
        info.extra.insert("stack_id".into(), json!(m.map(|m| m.id)));
        info.extra
            .insert("ets_file".into(), json!(self.rel(&s.ets.path)));
        info.extra
            .insert("compression".into(), json!(s.ets.header.compression.name()));
        info.extra.insert(
            "tile_size".into(),
            json!([s.ets.header.tile_width, s.ets.header.tile_height]),
        );
        info.extra.insert(
            "pyramid".into(),
            json!(
                s.level_sizes
                    .iter()
                    .enumerate()
                    .map(|(l, (w, h))| json!({"level": l, "size_x": w, "size_y": h,
                "tiles": s.ets.tiles.iter().filter(|t| t.level() == l as u32).count()}))
                    .collect::<Vec<_>>()
            ),
        );
        info.extra.insert(
            "dimensions".into(),
            json!(
                s.dims
                    .iter()
                    .map(|(k, _, n)| json!({"kind": k.name(), "size": n}))
                    .collect::<Vec<_>>()
            ),
        );
        info.extra.insert(
            "background".into(),
            json!(format!("{:#08x}", s.ets.header.background)),
        );
        info.extra
            .insert("size_source".into(), json!(s.size_source.name()));
        if s.origin != (0, 0) {
            info.extra.insert(
                "tile_origin".into(),
                json!({"x": s.origin.0, "y": s.origin.1}),
            );
        }
        if let Some(ri) = m
            .and_then(|m| m.objective.as_ref())
            .and_then(|o| o.refractive_index)
        {
            info.extra
                .insert("objective_refractive_index".into(), json!(ri));
        }
        if let Some(p) = m.and_then(|m| m.stage_position_um) {
            info.extra
                .insert("stage_position_um".into(), json!({"x": p.0, "y": p.1}));
        }
        Ok(info.finish())
    }

    /// Index of the per-plane record of plane `idx` (first non-XY dimension fastest).
    fn plane_record(s: &StackImage, idx: PlaneIndex) -> usize {
        let coords = s.coords(idx);
        let mut k = 0usize;
        let mut mul = 1usize;
        for ((_, _, n), v) in s.dims.iter().zip(&coords) {
            k += *v as usize * mul;
            mul *= (*n).max(1) as usize;
        }
        k
    }

    /// Plane `index` of `image` at `level`, or only `region` of it (then only the tiles the
    /// region overlaps are decoded, in parallel, and kept in the image's tile cache).
    #[allow(clippy::many_single_char_names)]
    fn read_level_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Option<Region>,
    ) -> Result<Plane> {
        let s = self.image(image)?;
        let pt = s.pixel_type.ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("ETS sample type {}", s.ets.header.sample_type),
                "Only 8-bit (2) and 16-bit (4) ETS samples are known.",
            )
        })?;
        let (sc, sz, st) = (
            s.group_size(Group::C),
            s.group_size(Group::Z),
            s.group_size(Group::T),
        );
        if index.c >= sc || index.z >= sz || index.t >= st {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} is out of range (C={sc}, Z={sz}, T={st})",
                index.c, index.z, index.t
            )));
        }
        let (w, h) = *s.level_sizes.get(level as usize).ok_or_else(|| {
            Error::Usage(format!(
                "pyramid level {level} out of range for image {image} (0..{})",
                s.level_sizes.len()
            ))
        })?;
        let bps = pt.bytes_per_sample();
        let px = s.spp as usize * bps;
        let win = if let Some(r) = region {
            r.check_within(w, h, &format!("image {image} level {level}"))?;
            r
        } else {
            let total = u64::from(w)
                .saturating_mul(u64::from(h))
                .saturating_mul(px as u64);
            if total > MAX_PLANE_BYTES {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("a {w}x{h} plane ({total} bytes) in memory"),
                    format!(
                        "Read a region of it (`--region X,Y,W,H`) or a downsampled pyramid level (`--level N`, N up to {}); level sizes are in `info` → images[].resolution_levels.",
                        s.level_sizes.len() - 1
                    ),
                ));
            }
            Region::full(w, h)
        };
        let n = openreadout_core::pixel::plane_bytes_checked(FORMAT_ID, win.width, win.height, px)?;
        if let Some(why) = &s.ets.table_problem {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "{}: {why}; planes cannot be assembled reliably",
                    self.rel(&s.ets.path)
                ),
            ));
        }
        let all: &[usize] = s
            .tile_index
            .get(&(level, s.coords(index)))
            .map_or(&[], Vec::as_slice);
        if all.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "plane c={} z={} t={} level {level} has no stored tiles in {}",
                    index.c,
                    index.z,
                    index.t,
                    self.rel(&s.ets.path)
                ),
            ));
        }
        let (tw, th) = (s.ets.header.tile_width, s.ets.header.tile_height);
        let (ox, oy) = level_origin(s.origin, level);
        let tiles: Vec<Tile> = all
            .iter()
            .map(|&i| s.ets.tiles[i].clone())
            .filter(|t| {
                let x0 = t.column() * i64::from(tw) + ox;
                let y0 = t.row() * i64::from(th) + oy;
                x0 < i64::from(w)
                    && y0 < i64::from(h)
                    && win.overlap(x0, y0, u64::from(tw), u64::from(th)).is_some()
            })
            .collect();
        // Unstored tiles show the background colour (a whole pixel repeated; memcpy-doubling,
        // not a per-byte iterator, which took seconds for large windows).
        let fill = Self::background(s, bps);
        let mut data = if fill.iter().all(|&b| b == 0) {
            vec![0u8; n]
        } else {
            let mut d = fill.repeat(n / fill.len().max(1));
            d.resize(n, 0);
            d
        };
        let spp = s.spp;
        let cache = region.is_some();
        let s = &mut self.images[image as usize];
        let paste = |data: &mut Vec<u8>, t: &Tile, pix: &[u8]| {
            let x0 = t.column() * i64::from(tw) + ox;
            let y0 = t.row() * i64::from(th) + oy;
            // Clip the tile to the level (edge tiles overhang it), then to the window.
            let cw = (i64::from(w) - x0).min(i64::from(tw)).max(0) as u32;
            let ch = (i64::from(h) - y0).min(i64::from(th)).max(0) as u32;
            paste_into_region(
                data,
                win,
                &PlacedTile {
                    data: pix,
                    row_bytes: tw as usize * px,
                    x: x0,
                    y: y0,
                    width: cw,
                    height: ch,
                },
                px,
            );
        };
        // Cached tiles first, then the rest read in file order and decoded in parallel, a
        // batch at a time: decoding every tile of a whole-slide plane before pasting held the
        // plane twice (3.6 GB for a 1.9 GB plane).
        let mut rest: Vec<Tile> = Vec::with_capacity(tiles.len());
        for t in tiles {
            if cache && let Some(d) = s.cache.get(&t.offset) {
                paste(&mut data, &t, &d);
            } else {
                rest.push(t);
            }
        }
        let tile_bytes = (tw as usize)
            .saturating_mul(th as usize)
            .saturating_mul(px)
            .max(1);
        let batch = (DECODE_BATCH_BYTES / tile_bytes).max(rayon::current_num_threads().max(1));
        for group in rest.chunks(batch) {
            let mut raw: Vec<(&Tile, Vec<u8>)> = Vec::with_capacity(group.len());
            for t in group {
                raw.push((t, s.ets.tile_bytes(t)?));
            }
            let ets = &s.ets;
            let fresh: Vec<Result<(&Tile, Vec<u8>)>> = raw
                .into_par_iter()
                .map(|(t, b)| ets.decode_tile_bytes(t, b).map(|d| (t, d)))
                .collect();
            for r in fresh {
                let (t, d) = r?;
                paste(&mut data, t, &d);
                if cache {
                    let n = d.len();
                    s.cache.put(t.offset, std::sync::Arc::new(d), n);
                }
            }
        }
        Ok(Plane {
            width: win.width,
            height: win.height,
            pixel_type: pt,
            samples_per_pixel: spp,
            data,
        })
    }

    fn background(s: &StackImage, bps: usize) -> Vec<u8> {
        let b = s.ets.header.background.to_le_bytes();
        let spp = s.spp as usize;
        match (bps, spp) {
            (1, 1) => vec![b[0]],
            (1, _) => (0..spp).map(|i| b[i.min(3)]).collect(),
            (2, _) => (0..spp).flat_map(|_| [b[0], b[1]]).collect(),
            _ => vec![0; bps * spp],
        }
    }
}

impl Dataset for VsiDataset {
    fn member_files(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for s in &self.images {
            for p in std::iter::once(&s.ets.path).chain(&s.other_files) {
                if *p != self.path && self.fs.is_file(p) && !out.contains(p) {
                    out.push(p.clone());
                }
            }
        }
        out
    }
    fn info(&self) -> Result<FileInfo> {
        let mut images = Vec::with_capacity(self.images.len());
        for (i, s) in self.images.iter().enumerate() {
            images.push(self.image_info(i as u32, s)?);
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        let mut notes = Vec::new();
        if self
            .images
            .iter()
            .any(|s| s.size_source == SizeSource::TileGrid)
        {
            notes.push("an image size is recorded neither in the ETS header (version 0x00030003) nor in the .vsi: it is the extent of the stored tiles and may include background padding at the right and bottom (images[].extra.size_source = tile_grid)".into());
        }
        if self.kind == "vsi"
            && self.images.iter().any(|s| {
                s.dims
                    .iter()
                    .any(|d| matches!(d.0, DimKind::Other(_) | DimKind::Unknown) && d.2 > 1)
            })
        {
            notes.push("a non-XY dimension has a kind code not seen in validated files (only Z = 1, T = 2, channel = 4 are known): it is exposed as T; check images[].extra.dimensions before interpreting T".into());
        }
        if self.kind == "ets" {
            notes.push("standalone ETS tile file: names, calibration and dimension kinds are in the .vsi next to the _<name>_ directory; non-XY dimensions are exposed as T".into());
        } else if self.ets_dir.is_none() {
            notes.push("the _<name>_ directory with the .ets pixel files was not found next to the .vsi: only metadata is available".into());
        }
        if images.iter().any(|i| i.pyramid_levels > 1) {
            notes.push("whole-slide stacks are pyramids: read a downsampled level with `planes --level N` (level sizes in images[].extra.pyramid)".into());
        }
        let described_only: Vec<String> = self
            .meta
            .stacks
            .iter()
            .filter(|m| {
                !self
                    .images
                    .iter()
                    .any(|s| s.meta.as_ref().is_some_and(|x| x.id == m.id))
            })
            .map(|m| format!("{} (stack{})", m.name.clone().unwrap_or_default(), m.id))
            .collect();
        if !described_only.is_empty() && self.ets_dir.is_some() {
            notes.push(format!(
                "stacks described in the .vsi without pixel data on disk: {}",
                described_only.join(", ")
            ));
        }
        if !self.open_findings.is_empty() {
            notes.push(format!(
                "{} problem(s) found while opening; run `check`",
                self.open_findings.len()
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: VsiReader.descriptor(),
            format_version: self
                .images
                .first()
                .map(|s| format!("{:#010x}", s.ets.header.version)),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut out = serde_json::Map::new();
        if let Some(t) = &self.tree {
            out.insert("records".into(), t.to_json());
        }
        if let Some(p) = &self.preview {
            out.insert(
                "preview_tiff".into(),
                json!({"width": p.width, "height": p.height, "compression": p.compression, "make": p.make, "model": p.model}),
            );
        }
        out.insert(
            "ets_headers".into(),
            json!(self.images.iter().map(|s| {
                let h = &s.ets.header;
                json!({"file": self.rel(&s.ets.path), "version": h.version, "sample_type": h.sample_type,
                       "samples_per_pixel": h.samples_per_pixel, "color_space": h.color_space,
                       "compression": h.compression.name(), "quality": h.quality,
                       "tile_size": [h.tile_width, h.tile_height, h.tile_depth], "background": h.background,
                       "sizes": h.sizes, "coordinates_per_tile": h.coord_count, "tiles": h.tile_count})
            }).collect::<Vec<_>>()),
        );
        Ok(Value::Object(out))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "images[].size_x",
            "images[].size_y",
            "images[].size_z",
            "images[].size_c",
            "images[].size_t",
            "images[].pixel_type",
            "images[].samples_per_pixel",
            "images[].pyramid_levels",
            "images[].physical_size",
            "images[].time_increment_s",
            "images[].channels[].name",
            "images[].channels[].emission_nm",
            "images[].channels[].excitation_nm",
            "images[].objective",
            "images[].instrument",
            "images[].acquired_at",
            "images[].mosaic",
            "images[].extra.pyramid",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        if let Some(t) = &self.tree {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: "record tree".into(),
                offset: Some(crate::tree::TREE_OFFSET as u64),
                size: None,
                image: None,
                details: json!({"records": t.record_count, "problems": t.problems.len()}),
            });
        }
        for m in &self.meta.stacks {
            let img = self
                .images
                .iter()
                .position(|s| s.meta.as_ref().is_some_and(|x| x.id == m.id));
            out.push(LsEntry {
                kind: "stack".into(),
                name: format!("stack{}", m.id),
                offset: None,
                size: None,
                image: img.map(|i| i as u32),
                details: json!({"name": m.name, "dimension_sizes": m.dim_sizes, "has_pixels": img.is_some()}),
            });
        }
        for (i, s) in self.images.iter().enumerate() {
            let h = &s.ets.header;
            out.push(LsEntry {
                kind: "ets".into(),
                name: self.rel(&s.ets.path),
                offset: Some(0),
                size: Some(s.ets.file_len),
                image: Some(i as u32),
                details: json!({"sample_type": h.sample_type, "samples_per_pixel": h.samples_per_pixel,
                                "compression": h.compression.name(), "quality": h.quality,
                                "tile_size": [h.tile_width, h.tile_height], "sizes": h.sizes,
                                "coordinates_per_tile": h.coord_count, "tiles": h.tile_count}),
            });
            out.push(LsEntry {
                kind: "tile-table".into(),
                name: format!("{}: tile table", self.rel(&s.ets.path)),
                offset: Some(h.table_offset),
                size: Some(h.entry_size() * u64::from(h.tile_count)),
                image: Some(i as u32),
                details: json!({"entries": s.ets.tiles.len(), "entry_size": h.entry_size()}),
            });
            for (l, (w, hh)) in s.level_sizes.iter().enumerate() {
                let tiles: Vec<&Tile> = s
                    .ets
                    .tiles
                    .iter()
                    .filter(|t| t.level() == l as u32)
                    .collect();
                out.push(LsEntry {
                    kind: "pyramid-level".into(),
                    name: format!("level {l}"),
                    offset: tiles.iter().map(|t| t.offset).min(),
                    size: Some(tiles.iter().map(|t| u64::from(t.len)).sum()),
                    image: Some(i as u32),
                    details: json!({"level": l, "size_x": w, "size_y": hh, "tiles": tiles.len()}),
                });
            }
            for f in &s.other_files {
                out.push(LsEntry {
                    kind: "file".into(),
                    name: self.rel(f),
                    offset: None,
                    size: self.fs.metadata(f).ok().map(|m| m.len()),
                    image: Some(i as u32),
                    details: json!({"read": false}),
                });
            }
        }
        for a in self.attachments()? {
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
        self.read_level_region(image, index, level, None)
    }

    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        self.read_level_region(image, index, level, Some(region))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        if let Some(t) = &self.tree {
            r.performed("record tree: every record set has its signature, links point forward inside the file, record counts match");
            for (at, why) in &t.problems {
                r.push(Finding::error("bad_record_tree", why.clone()).at(*at as u64));
            }
            if self.meta.stacks.is_empty() {
                r.push(Finding::warning(
                    "no_stacks",
                    "the record tree describes no image stacks",
                ));
            }
            r.performed("the _<name>_ directory exists and every stack directory holds a readable frame_t*.ets");
            if self.ets_dir.is_none() && !self.meta.stacks.is_empty() {
                let stem = self
                    .path
                    .file_stem()
                    .map_or_else(String::new, |s| s.to_string_lossy().to_string());
                r.push(Finding::error("missing_ets_directory", format!("directory _{stem}_ with the pixel data (.ets files) is missing next to the .vsi")));
            }
        }
        for f in &self.open_findings {
            r.push(f.clone());
        }
        r.performed("ETS: headers, tile table inside the file, every tile's bytes inside the file, tile coordinates within the image and pyramid");
        r.performed("one tile per image decoded");
        r.performed("dimension sizes in the .vsi agree with the ETS header");
        let mut images = std::mem::take(&mut self.images);
        for (i, s) in images.iter_mut().enumerate() {
            let name = self.rel(&s.ets.path);
            let h = s.ets.header.clone();
            if let Some(why) = &s.ets.table_problem {
                r.push(Finding::error("truncated", format!("{name}: {why}")).at(h.table_offset));
            }
            if s.pixel_type.is_none() {
                r.push(Finding::error(
                    "unsupported_sample_type",
                    format!("{name}: ETS sample type {}", h.sample_type),
                ));
            }
            let mut beyond = 0usize;
            let mut outside = 0usize;
            // Tiles of the stored grid that fall just outside the image (the grid origin is not
            // a multiple of the tile size, so a column or row of tiles can lie entirely past an
            // edge): read as cropped away, reported once as info.
            let mut off_canvas = 0usize;
            for t in &s.ets.tiles {
                if t.offset.saturating_add(u64::from(t.len)) > s.ets.file_len {
                    beyond += 1;
                    if beyond == 1 {
                        r.push(
                            Finding::error(
                                "truncated",
                                format!("{name}: tile data runs past the end of the file"),
                            )
                            .at(t.offset),
                        );
                    }
                    continue;
                }
                let lvl = t.level() as usize;
                let ok_level = lvl < s.level_sizes.len();
                let (lw, lh) = s.level_sizes.get(lvl).copied().unwrap_or((0, 0));
                let (ox, oy) = level_origin(s.origin, t.level());
                let (x0, y0) = (
                    t.column() * i64::from(h.tile_width) + ox,
                    t.row() * i64::from(h.tile_height) + oy,
                );
                let (tw, th) = (i64::from(h.tile_width), i64::from(h.tile_height));
                let ok_xy = x0 < i64::from(lw) && y0 < i64::from(lh) && x0 + tw > 0 && y0 + th > 0;
                let near_xy = x0 < i64::from(lw) + tw
                    && y0 < i64::from(lh) + th
                    && x0 + 2 * tw > 0
                    && y0 + 2 * th > 0;
                let ok_extra = t.extra().iter().zip(&s.dims).all(|(v, d)| *v < d.2);
                if ok_level && ok_extra && !ok_xy && near_xy {
                    off_canvas += 1;
                } else if !(ok_level && ok_xy && ok_extra) {
                    outside += 1;
                    if outside <= 3 {
                        let mut at = vec![t.column(), t.row()];
                        at.extend(t.extra().iter().map(|&v| i64::from(v)));
                        at.push(i64::from(t.level()));
                        r.push(
                            Finding::error(
                                "tile_out_of_range",
                                format!(
                                    "{name}: tile at {at:?} (column, row, other indices, level) lies outside the image or its pyramid"
                                ),
                            )
                            .at(t.offset),
                        );
                    }
                }
            }
            if off_canvas > 0 {
                r.push(Finding::info(
                    "tiles_off_canvas",
                    format!(
                        "{name}: {off_canvas} stored tile(s) lie just past an edge of the image (within one tile; the tile grid starts at {:?}) and are cropped away",
                        s.origin
                    ),
                ));
            }
            if beyond > 1 {
                r.push(Finding::error(
                    "truncated",
                    format!("{name}: {beyond} tiles run past the end of the file"),
                ));
            }
            if let Some(m) = &s.meta {
                let ets_sizes: Vec<i64> = h.sizes.iter().skip(2).map(|v| i64::from(*v)).collect();
                let vsi_sizes: Vec<i64> = m.dim_sizes.iter().map(|v| i64::from(*v)).collect();
                // Version 0x00030003 ETS headers carry no size list at all.
                if !vsi_sizes.is_empty() && !h.sizes.is_empty() && vsi_sizes != ets_sizes {
                    r.push(Finding::warning("dimension_mismatch", format!("{name}: the .vsi declares non-XY sizes {vsi_sizes:?}, the ETS {ets_sizes:?}")));
                }
                if m.tile_origin.is_none()
                    && s.ets.tiles.iter().any(|t| t.column() < 0 || t.row() < 0)
                {
                    r.push(Finding::warning(
                        "tile_origin_missing",
                        format!("{name}: tiles have negative grid indices but the .vsi records no tile-grid origin (record 2410); the image is placed from the smallest column and row, which may shift it"),
                    ));
                }
                if let (Some((vw, vh)), SizeSource::EtsHeader) = (s.vsi_size, s.size_source)
                    && (vw, vh) != s.level_sizes[0]
                {
                    r.push(Finding::warning(
                        "dimension_mismatch",
                        format!(
                            "{name}: the .vsi records a {vw} x {vh} image, the ETS header {} x {} (the ETS header is used)",
                            s.level_sizes[0].0, s.level_sizes[0].1
                        ),
                    ));
                }
                for (k, _, n) in &s.dims {
                    if matches!(k, DimKind::Other(_) | DimKind::Unknown) && *n > 1 {
                        r.push(Finding::warning(
                            "unknown_dimension_kind",
                            format!(
                                "{name}: a dimension of size {n} has kind {}; it is exposed as T",
                                k.name()
                            ),
                        ));
                    }
                }
            }
            // Every plane has at least one tile at level 0. The plane count is declared (ETS
            // header sizes), so count the planes that do have tiles instead of visiting every
            // declared plane: a few-KB file may declare 2^32 of them.
            let planes = [Group::C, Group::Z, Group::T]
                .iter()
                .map(|g| u64::from(s.group_size(*g)))
                .fold(1u64, u64::saturating_mul);
            let present = s
                .tile_index
                .keys()
                .filter(|(level, v)| {
                    *level == 0
                        && v.len() == s.dims.len()
                        && v.iter().zip(&s.dims).all(|(x, d)| *x < d.2.max(1))
                })
                .count() as u64;
            let empty = planes.saturating_sub(present);
            if empty > 0 {
                r.push(Finding::error(
                    "missing_planes",
                    format!("{name}: {empty} of {planes} planes have no tiles at full resolution"),
                ));
            }
            if let Some(t) = s.ets.tiles.iter().find(|t| t.level() == 0).cloned()
                && s.pixel_type.is_some()
                && t.offset.saturating_add(u64::from(t.len)) <= s.ets.file_len
            {
                match s.ets.decode_tile(&t) {
                    Ok(_) => {}
                    Err(e @ Error::Unsupported { .. }) => r.push(
                        Finding::warning("unsupported_tile", format!("{name}: {e}")).at(t.offset),
                    ),
                    Err(e) => {
                        r.push(Finding::error("bad_tile", format!("{name}: {e}")).at(t.offset));
                    }
                }
            }
            let _ = i;
        }
        self.images = images;
        Ok(r)
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        let Some(p) = &self.preview else {
            return Ok(Vec::new());
        };
        if p.strip_len == 0 || p.compression != 7 {
            return Ok(Vec::new());
        }
        let size = self
            .tree
            .as_ref()
            .and_then(|t| preview_jpeg(&t.data, p))
            .map_or(0, |j| j.len() as u64);
        let mut extra = BTreeMap::new();
        extra.insert("width".into(), json!(p.width));
        extra.insert("height".into(), json!(p.height));
        Ok(vec![AttachmentInfo {
            index: 0,
            name: "preview".into(),
            content_type: "JPG".into(),
            extension: "jpg".into(),
            offset: Some(u64::from(p.strip_offset)),
            size,
            extra,
        }])
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        if index != 0 {
            return Err(Error::Usage(format!("attachment {index} does not exist")));
        }
        let (Some(t), Some(p)) = (&self.tree, &self.preview) else {
            return Err(Error::Usage("this file has no preview attachment".into()));
        };
        preview_jpeg(&t.data, p)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "preview strip lies outside the file"))
    }

    #[allow(clippy::many_single_char_names)]
    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        let s = self.image(image)?;
        let Some(m) = &s.meta else {
            return Ok((0, Vec::new()));
        };
        let planes =
            (s.group_size(Group::C) * s.group_size(Group::Z) * s.group_size(Group::T)) as usize;
        if m.plane_times_ms.len() != planes || planes <= 1 {
            return Ok((0, Vec::new()));
        }
        let total = planes as u64;
        let n = limit.unwrap_or(planes).min(planes);
        let out = (0..n)
            .map(|k| {
                let mut rest = k as u32;
                let coords: Vec<u32> = s
                    .dims
                    .iter()
                    .map(|(_, _, sz)| {
                        let v = rest % (*sz).max(1);
                        rest /= (*sz).max(1);
                        v
                    })
                    .collect();
                let p = s.plane_of(&coords);
                let mut f = json!({"index": k, "c": p.c, "z": p.z, "t": p.t, "time_ms": m.plane_times_ms[k]});
                if let Some(Some(z)) = m.plane_z_um.get(k) {
                    f["z_um"] = json!(z);
                }
                f
            })
            .collect();
        Ok((total, out))
    }
}
