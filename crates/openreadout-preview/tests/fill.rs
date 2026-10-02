//! Previews of a synthetic mosaic with gaps: the canvas no tile covers is 0 at full resolution,
//! while its pyramid level carries the fill slide scanners write past the scanned tiles
//! (65535, and 65535 blended with data at the tile edge), as ZEN's Axioscan RAC scans do. The
//! preview must draw the gaps as background at every level and take its contrast from the data.

#![allow(clippy::float_cmp)]

use openreadout_core::model::{CheckReport, FileInfo, FormatDescriptor, ImageInfo, LsEntry};
use openreadout_core::pixel::{PixelType, Plane};
use openreadout_core::provenance::{Confidence, ProvenanceMap};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::{Error, Result};
use openreadout_preview::{PreviewRequest, Rendered, render};

const W: u32 = 96;
const H: u32 = 64;
const FILL: u16 = u16::MAX;

/// Two 48 × 32 tiles on the diagonal (top-left and bottom-right); the other quadrants are
/// gaps. Values are 12-bit (100..=1080, a 3000 "cell" in each tile).
struct Mosaic {
    /// Record `component_bit_count` 12, as the CZI reader does for these scans.
    recorded_depth: bool,
    /// Blend the fill into the data along the tile edge at level 1.
    blend: bool,
}

fn covered(x: u32, y: u32) -> bool {
    (x < W / 2) == (y < H / 2)
}

fn level0(x: u32, y: u32) -> u16 {
    if !covered(x, y) {
        return 0;
    }
    let (tx, ty) = (x % 48, y % 32);
    if (tx as i32 - 24).pow(2) + (ty as i32 - 16).pow(2) < 16 {
        3000
    } else {
        100 + ((x + y) % 50) as u16 * 20
    }
}

/// Level 1 (48 × 32): 2 × 2 means of level 0 where it is covered; the top-right gap lies
/// inside a pyramid tile and holds the fill, its first column blended with the neighbouring
/// data; the bottom-left gap lies outside every pyramid tile (0).
fn level1(x: u32, y: u32, blend: bool) -> u16 {
    let (x0, y0) = (x * 2, y * 2);
    if covered(x0, y0) {
        let s: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
            .iter()
            .map(|&(dx, dy)| u32::from(level0(x0 + dx, y0 + dy)))
            .sum();
        return (s / 4) as u16;
    }
    if y0 < H / 2 {
        if blend && x == 24 { 30_000 } else { FILL }
    } else {
        0
    }
}

fn plane(w: u32, h: u32, f: impl Fn(u32, u32) -> u16) -> Plane {
    let mut data = Vec::with_capacity((w * h * 2) as usize);
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&f(x, y).to_le_bytes());
        }
    }
    Plane {
        width: w,
        height: h,
        pixel_type: PixelType::Uint16,
        samples_per_pixel: 1,
        data,
    }
}

impl Mosaic {
    fn file_info(&self) -> FileInfo {
        let mut im = ImageInfo::new(0, W, H, PixelType::Uint16);
        im.pyramid_levels = 2;
        im.extra.insert(
            "pyramid".into(),
            serde_json::json!([{"level": 1, "size_x": W / 2, "size_y": H / 2}]),
        );
        if self.recorded_depth {
            im.extra
                .insert("component_bit_count".into(), serde_json::json!(12));
        }
        let im = im.finish();
        FileInfo {
            path: "mosaic".into(),
            size_bytes: 0,
            format: FormatDescriptor {
                id: "synth".into(),
                name: "Synthetic mosaic".into(),
                vendor: "test".into(),
                extensions: vec![],
                family: "microscopy".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::High,
                known_gaps: vec![],
            },
            format_version: None,
            plane_count: im.plane_count,
            images: vec![im],
            tables: vec![],
            spectra: vec![],
            traces: vec![],
            notes: vec![],
        }
    }
}

impl Dataset for Mosaic {
    fn info(&self) -> Result<FileInfo> {
        Ok(self.file_info())
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::default()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![])
    }
    fn read_plane(&mut self, image: u32, i: PlaneIndex) -> Result<Plane> {
        self.read_plane_level(image, i, 0)
    }
    fn read_plane_level(&mut self, image: u32, i: PlaneIndex, level: u32) -> Result<Plane> {
        if image != 0 || i != PlaneIndex::default() {
            return Err(Error::Usage("out of range".into()));
        }
        match level {
            0 => Ok(plane(W, H, level0)),
            1 => Ok(plane(W / 2, H / 2, |x, y| level1(x, y, self.blend))),
            _ => Err(Error::Usage("no such level".into())),
        }
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("mosaic", "synth"))
    }
}

fn preview(recorded_depth: bool, blend: bool, level: u32) -> Rendered {
    let mut ds = Mosaic {
        recorded_depth,
        blend,
    };
    let info = ds.file_info();
    let mut req = PreviewRequest::default();
    req.axes = false;
    req.level = Some(level);
    render(&mut ds, &info, &req).unwrap()
}

/// Grey level drawn at level-`level` pixel `(x, y)` (no downsampling at these sizes).
fn grey(r: &Rendered, x: u32, y: u32) -> u8 {
    r.canvas.rgb[((y * r.canvas.width + x) * 3) as usize]
}

/// Mean grey level over the level-`level` pixels `f` selects.
fn mean(r: &Rendered, f: impl Fn(u32, u32) -> bool) -> f64 {
    let (mut s, mut n) = (0u64, 0u64);
    for y in 0..r.canvas.height {
        for x in 0..r.canvas.width {
            if f(x, y) {
                s += u64::from(grey(r, x, y));
                n += 1;
            }
        }
    }
    s as f64 / n as f64
}

fn display_range(r: &Rendered) -> (f64, f64) {
    let ch = &r.output.image.as_ref().unwrap().channels[0];
    (ch.display_min, ch.display_max)
}

#[test]
fn gaps_are_background_at_full_resolution() {
    let r = preview(true, true, 0);
    assert_eq!((r.canvas.width, r.canvas.height), (W, H));
    assert!(display_range(&r).1 <= 4095.0);
    assert_eq!(mean(&r, |x, y| !covered(x, y)), 0.0, "gaps are black");
    assert!(mean(&r, covered) > 30.0, "tiles are visible");
    assert!(r.output.notes.is_empty(), "{:?}", r.output.notes);
}

#[test]
fn pyramid_fill_is_background_when_the_depth_is_recorded() {
    let r = preview(true, true, 1);
    assert_eq!((r.canvas.width, r.canvas.height), (W / 2, H / 2));
    let (lo, hi) = display_range(&r);
    assert!(
        hi <= 4095.0,
        "white point {hi} from the tiles, not the fill"
    );
    assert!(lo < 200.0);
    // The fill (and its blended edge) is drawn like the level-0 gap: black.
    assert_eq!(mean(&r, |x, y| !covered(x * 2, y * 2)), 0.0);
    assert!(
        mean(&r, |x, y| covered(x * 2, y * 2)) > 30.0,
        "tiles are visible"
    );
    // The level-0 and level-1 pictures agree on what is covered.
    let full = preview(true, true, 0);
    for y in 0..H / 2 {
        for x in 0..W / 2 {
            assert_eq!(
                grey(&r, x, y) == 0,
                grey(&full, x * 2, y * 2) == 0 && grey(&full, x * 2 + 1, y * 2 + 1) == 0,
                "level 1 ({x}, {y})"
            );
        }
    }
    let notes = r.output.notes.join("\n");
    assert!(
        notes.contains("above the 12-bit range") && notes.contains("drawn as background"),
        "{notes}"
    );
}

#[test]
fn pyramid_fill_leaves_the_contrast_without_a_recorded_depth() {
    // (Blended edges cannot be told from data without a recorded depth: none here.)
    let r = preview(false, false, 1);
    let (_, hi) = display_range(&r);
    assert!(hi < 4095.0, "white point {hi} from the tiles, not the fill");
    // Without a recorded depth the fill cannot be told from clipping: drawn at the top.
    assert_eq!(grey(&r, 40, 4), 255);
    assert!(
        mean(&r, |x, y| covered(x * 2, y * 2)) > 30.0,
        "tiles are visible"
    );
    assert!(
        r.output
            .notes
            .iter()
            .any(|n| n.contains("the pixel type's maximum")),
        "{:?}",
        r.output.notes
    );
    // An explicit contrast rule is applied as asked.
    let mut ds = Mosaic {
        recorded_depth: false,
        blend: false,
    };
    let info = ds.file_info();
    let mut req = PreviewRequest::default();
    req.axes = false;
    req.level = Some(1);
    req.contrast = openreadout_preview::Contrast::MinMax;
    let m = render(&mut ds, &info, &req).unwrap();
    assert_eq!(display_range(&m).1, 65535.0);
}
