//! DICOM whole-slide microscopy (VL Whole Slide Microscopy Image IOD, PS3.3 A.32.8 and
//! C.8.12.4): each pyramid level is one or more instances of tiled frames placed in a total
//! pixel matrix, TILED_FULL (row-major frame order) or TILED_SPARSE (per-frame positions).
//! Label, overview and thumbnail instances become images of their own.

use std::collections::{BTreeMap, HashMap};

use super::element::{Lookup, functional};
use super::file::DicomFile;
use super::pixels::PixelDesc;
use super::tags as t;

/// One tile: the frame of a file that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TileRef {
    pub(crate) file: usize,
    pub(crate) frame: u32,
}

/// One pyramid level.
#[derive(Debug, Clone)]
pub(crate) struct Level {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) tile_w: u32,
    pub(crate) tile_h: u32,
    /// Tiles by (column, row) of the tile grid.
    pub(crate) tiles: HashMap<(u32, u32), TileRef>,
    /// Pixel spacing (between rows, between columns) in mm.
    pub(crate) spacing: Option<[f64; 2]>,
    pub(crate) files: Vec<usize>,
    pub(crate) organization: String,
}

impl Level {
    pub(crate) fn grid(&self) -> (u32, u32) {
        (
            self.width.div_ceil(self.tile_w.max(1)),
            self.height.div_ceil(self.tile_h.max(1)),
        )
    }
}

/// A slide: its pyramid, level 0 first.
#[derive(Debug, Clone)]
pub(crate) struct Slide {
    pub(crate) levels: Vec<Level>,
    pub(crate) desc: PixelDesc,
    pub(crate) first_file: usize,
    pub(crate) notes: Vec<String>,
}

/// Value 3 of Image Type for WSI instances: VOLUME, LABEL, OVERVIEW or THUMBNAIL.
pub(crate) fn flavour(f: &DicomFile) -> String {
    f.ds.get(t::IMAGE_TYPE)
        .map(super::parse::Element::strings)
        .and_then(|v| v.get(2).cloned())
        .unwrap_or_else(|| "VOLUME".into())
        .to_ascii_uppercase()
}

fn spacing_of(f: &DicomFile) -> Option<[f64; 2]> {
    let shared = f.ds.first(t::SHARED_FUNCTIONAL_GROUPS_SEQUENCE);
    let s = functional(None, shared, t::PIXEL_MEASURES_SEQUENCE)
        .map(|i| i.numbers(t::PIXEL_SPACING))
        .unwrap_or_default();
    if s.len() >= 2 && s[0] > 0.0 && s[1] > 0.0 {
        return Some([s[0], s[1]]);
    }
    // ImagedVolumeWidth/Height (mm) over the total pixel matrix
    let w = f.ds.number(t::IMAGED_VOLUME_WIDTH)?;
    let h = f.ds.number(t::IMAGED_VOLUME_HEIGHT)?;
    let cols = f.ds.number(t::TOTAL_PIXEL_MATRIX_COLUMNS)?;
    let rows = f.ds.number(t::TOTAL_PIXEL_MATRIX_ROWS)?;
    (w > 0.0 && h > 0.0 && cols > 0.0 && rows > 0.0).then(|| [h / rows, w / cols])
}

/// Place the frames of one instance in its level's tile grid.
fn place(f: &DicomFile, fi: usize, desc: &PixelDesc, level: &mut Level, notes: &mut Vec<String>) {
    let (gx, gy) = level.grid();
    let org =
        f.ds.text(t::DIMENSION_ORGANIZATION_TYPE)
            .unwrap_or_default()
            .to_ascii_uppercase();
    let per_frame = f.ds.items(t::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE);
    let sparse = org == "TILED_SPARSE" || (org != "TILED_FULL" && !per_frame.is_empty());
    level.organization = if sparse { "TILED_SPARSE" } else { "TILED_FULL" }.into();
    let per_plane = u64::from(gx) * u64::from(gy);
    let mut extra = 0u32;
    for frame in 0..desc.frames {
        let (tx, ty) = if sparse {
            let Some(p) = per_frame
                .get(frame as usize)
                .and_then(|i| i.first(t::PLANE_POSITION_SLIDE_SEQUENCE))
            else {
                extra += 1;
                continue;
            };
            let col = p
                .number(t::COLUMN_POSITION_IN_TOTAL_IMAGE_PIXEL_MATRIX)
                .unwrap_or(1.0);
            let row = p
                .number(t::ROW_POSITION_IN_TOTAL_IMAGE_PIXEL_MATRIX)
                .unwrap_or(1.0);
            if col < 1.0 || row < 1.0 {
                extra += 1;
                continue;
            }
            (
                ((col - 1.0) / f64::from(level.tile_w)) as u32,
                ((row - 1.0) / f64::from(level.tile_h)) as u32,
            )
        } else {
            let i = u64::from(frame);
            if i >= per_plane {
                // further focal planes or optical paths
                extra += 1;
                continue;
            }
            ((i % u64::from(gx)) as u32, (i / u64::from(gx)) as u32)
        };
        if tx >= gx || ty >= gy {
            extra += 1;
            continue;
        }
        // the first frame at a position wins (further focal planes repeat positions)
        if level.tiles.contains_key(&(tx, ty)) {
            extra += 1;
            continue;
        }
        level.tiles.insert((tx, ty), TileRef { file: fi, frame });
    }
    if extra > 0 {
        notes.push(format!(
            "{}: {extra} frames are not placed in the {}x{} level (further focal planes or optical paths, or positions outside it); the first focal plane and optical path are read",
            f.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            level.width,
            level.height
        ));
    }
}

/// Build the slides of the WSI instances among `files` (indices `wsi`): VOLUME instances
/// grouped by series and by total pixel matrix size into levels, largest first.
pub(crate) fn slides(files: &[(DicomFile, PixelDesc)], wsi: &[usize]) -> Vec<Slide> {
    let mut by_series: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for &i in wsi {
        let f = &files[i].0;
        if flavour(f) != "VOLUME" {
            continue;
        }
        let key = f.ds.text(t::SERIES_INSTANCE_UID).unwrap_or_default();
        by_series.entry(key).or_default().push(i);
    }
    let mut out = Vec::new();
    for (_, members) in by_series {
        let mut levels: Vec<Level> = Vec::new();
        let mut notes = Vec::new();
        for &i in &members {
            let (f, desc) = &files[i];
            let w =
                f.ds.uint(t::TOTAL_PIXEL_MATRIX_COLUMNS)
                    .unwrap_or(desc.cols);
            let h = f.ds.uint(t::TOTAL_PIXEL_MATRIX_ROWS).unwrap_or(desc.rows);
            let pos = levels.iter().position(|l| (l.width, l.height) == (w, h));
            let li = if let Some(p) = pos {
                p
            } else {
                levels.push(Level {
                    width: w,
                    height: h,
                    tile_w: desc.cols,
                    tile_h: desc.rows,
                    tiles: HashMap::new(),
                    spacing: spacing_of(f),
                    files: Vec::new(),
                    organization: String::new(),
                });
                levels.len() - 1
            };
            let level = &mut levels[li];
            if (level.tile_w, level.tile_h) != (desc.cols, desc.rows) {
                notes.push(format!(
                    "{}: tiles of {}x{} in a level of {}x{} tiles; not read",
                    f.path.display(),
                    desc.cols,
                    desc.rows,
                    level.tile_w,
                    level.tile_h
                ));
                continue;
            }
            level.files.push(i);
            place(f, i, desc, level, &mut notes);
        }
        levels.sort_by(|a, b| {
            (u64::from(b.width) * u64::from(b.height))
                .cmp(&(u64::from(a.width) * u64::from(a.height)))
        });
        let Some(first) = levels.first().and_then(|l| l.files.first()).copied() else {
            continue;
        };
        let mut desc = files[first].1.clone();
        desc.frames = 1;
        out.push(Slide {
            levels,
            desc,
            first_file: first,
            notes,
        });
    }
    out
}
