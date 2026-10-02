//! Rulers on a synthetic pyramid: the reported mapping takes PNG pixels back to the
//! full-resolution pixels they show, whatever level and downsample were drawn; thumbnails
//! respect their read budget.

#![allow(clippy::float_cmp, clippy::many_single_char_names)]

use openreadout_core::model::{CheckReport, FileInfo, FormatDescriptor, ImageInfo, LsEntry};
use openreadout_core::pixel::{PixelType, Plane};
use openreadout_core::provenance::{Confidence, ProvenanceMap};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::{Error, Region, Result};
use openreadout_preview::{
    Encoding, PreviewRequest, Rendered, finish, finish_within, render, thumbnail,
};

/// Level 0 size.
const W: u32 = 4000;
const H: u32 = 3000;
/// Level scales: level 1 is 4× smaller, level 2 16×.
const SCALES: [u32; 3] = [1, 4, 16];
/// A bright square at full resolution, on black.
const SQUARE: (u32, u32, u32) = (2200, 1400, 80);

/// Records every read's level and decoded size.
#[derive(Default)]
struct Pyramid {
    reads: Vec<(u32, usize)>,
    pixel_size: Option<f64>,
}

fn info(pixel_size: Option<f64>) -> FileInfo {
    let mut im = ImageInfo::new(0, W, H, PixelType::Uint8);
    im.pyramid_levels = 3;
    im.physical_size =
        openreadout_core::model::PhysicalSize::micrometres(pixel_size, pixel_size, None);
    im.resolution_levels = SCALES
        .iter()
        .enumerate()
        .map(|(l, k)| {
            openreadout_core::ResolutionLevel::new(l as u32, W.div_ceil(*k), H.div_ceil(*k), W, H)
        })
        .collect();
    let im = im.finish();
    FileInfo {
        path: "pyramid".into(),
        size_bytes: 0,
        format: FormatDescriptor {
            id: "synth".into(),
            name: "Synthetic".into(),
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

impl Dataset for Pyramid {
    fn info(&self) -> Result<FileInfo> {
        Ok(info(self.pixel_size))
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
    fn read_plane_level(&mut self, _: u32, _: PlaneIndex, level: u32) -> Result<Plane> {
        let k = *SCALES
            .get(level as usize)
            .ok_or_else(|| Error::Usage("no such level".into()))?;
        let (w, h) = (W.div_ceil(k), H.div_ceil(k));
        let (sx, sy, n) = SQUARE;
        let mut data = vec![0u8; w as usize * h as usize];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x * k, y * k);
                if (sx..sx + n).contains(&fx) && (sy..sy + n).contains(&fy) {
                    data[(y * w + x) as usize] = 255;
                }
            }
        }
        self.reads.push((level, data.len()));
        Ok(Plane {
            width: w,
            height: h,
            pixel_type: PixelType::Uint8,
            samples_per_pixel: 1,
            data,
        })
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("pyramid", "synth"))
    }
}

/// Bounding box of the bright pixels inside the plot area, in PNG pixels.
fn bright_box(r: &Rendered) -> (u32, u32, u32, u32) {
    let im = r.output.image.as_ref().unwrap();
    let pa = im.plot_area;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in pa.y..pa.y + pa.height {
        for x in pa.x..pa.x + pa.width {
            let i = (y as usize * r.canvas.width as usize + x as usize) * 3;
            if r.canvas.rgb[i] > 100 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    (x0, y0, x1, y1)
}

#[test]
fn region_on_a_downsampled_level_maps_back_to_full_resolution() {
    let info = info(Some(0.25));
    let mut req = PreviewRequest::default();
    req.max_size = 400;
    req.region = Some(Region::new(1000, 800, 2400, 1600));
    req.contrast = openreadout_preview::Contrast::Raw;
    let mut ds = Pyramid::default();
    let r = render(&mut ds, &info, &req).unwrap();
    let im = r.output.image.as_ref().unwrap();
    assert_eq!(im.level, 1, "the region spans 600 px at level 1");
    assert_eq!(im.region, Some(Region::new(250, 200, 600, 400)));
    assert_eq!(im.downsample, 2);
    assert!(im.axes);
    assert_eq!((im.source_origin.x, im.source_origin.y), (1000.0, 800.0));
    assert_eq!((im.source_per_pixel, im.source_per_pixel_y), (8.0, 8.0));
    assert_eq!(im.full_res_region, Region::new(1000, 800, 2400, 1600));
    assert_eq!((im.plot_area.width, im.plot_area.height), (300, 200));
    assert!(r.canvas.width.max(r.canvas.height) <= 400);
    // the bright square, mapped back through the reported transform
    let (x0, y0, x1, y1) = bright_box(&r);
    let to_src = |px: u32, py: u32| {
        (
            im.source_origin.x + f64::from(px - im.plot_area.x) * im.source_per_pixel,
            im.source_origin.y + f64::from(py - im.plot_area.y) * im.source_per_pixel_y,
        )
    };
    let (sx0, sy0) = to_src(x0, y0);
    let (sx1, sy1) = to_src(x1 + 1, y1 + 1);
    let (qx, qy, n) = SQUARE;
    let tol = im.source_per_pixel;
    for (got, want) in [(sx0, qx), (sy0, qy), (sx1, qx + n), (sy1, qy + n)] {
        assert!((got - f64::from(want)).abs() <= tol, "{got} vs {want}");
    }
    // 0.25 µm/px × 8 = 2 µm per drawn px, 300 px: ≤ 120 µm → 100 µm, 50 px
    let b = im.scale_bar.as_ref().unwrap();
    assert_eq!((b.length, b.unit.as_str(), b.pixels), (100.0, "µm", 50));
    // the same request gives the same bytes; halving to a byte budget keeps the mapping
    let (_, png) = finish(&r, Encoding::Png, 90).unwrap();
    let r2 = render(&mut Pyramid::default(), &info, &req).unwrap();
    assert_eq!(finish(&r2, Encoding::Png, 90).unwrap().1, png);
    let (small, _) = finish_within(&r, Encoding::Png, 90, 300).unwrap();
    let si = small.image.as_ref().unwrap();
    let halvings = (si.source_per_pixel / im.source_per_pixel).log2().round() as u32;
    assert!(halvings >= 1, "{:?}", small.notes);
    // the square's left column, halved `halvings` times, still maps to x = 2200
    let c = (x0 >> halvings) - si.plot_area.x;
    let back = si.source_origin.x + f64::from(c) * si.source_per_pixel;
    assert!(
        (back - f64::from(qx)).abs() <= si.source_per_pixel,
        "{back}"
    );
}

#[test]
fn whole_image_overview_and_plain_mode() {
    let info = info(None);
    let mut req = PreviewRequest::default();
    req.max_size = 512;
    let r = render(&mut Pyramid::default(), &info, &req).unwrap();
    let im = r.output.image.as_ref().unwrap();
    // the plot area (512 minus margins) is served by level 1 (1000 px), not level 2 (250 px)
    assert_eq!(im.level, 1);
    assert!(im.scale_bar.is_none());
    assert_eq!(im.full_res_region, Region::new(0, 0, W, H));
    let spp = im.source_per_pixel;
    assert!((spp - 4.0 * f64::from(im.downsample)).abs() < 1e-9);
    let (x0, _, _, _) = bright_box(&r);
    let sx = im.source_origin.x + f64::from(x0 - im.plot_area.x) * spp;
    assert!((sx - f64::from(SQUARE.0)).abs() <= spp, "{sx}");
    // plain: no frame, the plane itself, mapping still reported
    req.axes = false;
    let p = render(&mut Pyramid::default(), &info, &req).unwrap();
    let pi = p.output.image.as_ref().unwrap();
    assert!(!pi.axes);
    assert_eq!(
        pi.plot_area,
        Region::new(0, 0, p.canvas.width, p.canvas.height)
    );
    assert_eq!(
        p.canvas.width, 500,
        "plain fills max_size from level 1 (1000 px / 2)"
    );
    // grid lines change the data area only
    req.axes = true;
    req.grid = true;
    let g = render(&mut Pyramid::default(), &info, &req).unwrap();
    assert!(g.output.image.as_ref().unwrap().grid);
    assert_eq!(
        (g.canvas.width, g.canvas.height),
        (r.canvas.width, r.canvas.height)
    );
    assert_ne!(g.canvas.rgb, r.canvas.rgb);
}

#[test]
fn thumbnail_reads_a_small_level_within_budget() {
    let info = info(Some(1.0));
    let mut ds = Pyramid::default();
    let t = thumbnail(&mut ds, &info, 384, 32 << 20).unwrap();
    assert!(t.canvas.width.max(t.canvas.height) <= 384);
    let im = t.output.image.as_ref().unwrap();
    assert!(im.axes && im.scale_bar.is_some());
    // level 1 (1000 × 750) is the smallest that covers the plot area; level 0 is never read
    assert_eq!(ds.reads, vec![(1, 750_000)]);
    // over budget: refused before any read
    let mut ds = Pyramid::default();
    let e = thumbnail(&mut ds, &info, 384, 100_000).unwrap_err();
    assert_eq!(e.exit_code(), 6, "{e}");
    assert!(ds.reads.is_empty());
}
