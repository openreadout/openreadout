//! A region read off the rulers of a volume shown in a radiological view comes back as the same
//! part of the picture. The synthetic volume is stored the way a micro-CT NIfTI file was
//! (columns toward the feet, rows toward the back, slices toward the right), so its sagittal
//! view swaps the stored axes; it is not square, so a swap or flip done in one place and not
//! the other moves the region.

use openreadout_core::model::{CheckReport, FileInfo, FormatDescriptor, ImageInfo, LsEntry};
use openreadout_core::pixel::{PixelType, Plane};
use openreadout_core::provenance::{Confidence, ProvenanceMap};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::region::Region;
use openreadout_core::{Error, Result};
use openreadout_preview::{Axis, PreviewRequest, Rendered, render};

const NX: u32 = 60;
const NY: u32 = 40;
const NZ: u32 = 12;
/// The stored voxel (x, y) of the bright spot, through every slice.
const SPOT: (u32, u32) = (45, 10);

struct Volume;

fn value(x: u32, y: u32) -> u8 {
    let d = (x as i64 - i64::from(SPOT.0)).pow(2) + (y as i64 - i64::from(SPOT.1)).pow(2);
    if d <= 2 {
        250
    } else {
        10 + ((x * 7 + y * 3) % 20) as u8
    }
}

impl Volume {
    fn file_info() -> FileInfo {
        let mut im = ImageInfo::new(0, NX, NY, PixelType::Uint8);
        im.size_z = NZ;
        im.extra.insert(
            "orientation".into(),
            serde_json::json!({
                "plane": "sagittal",
                "columns_toward": "I",
                "rows_toward": "P",
                "slices_toward": "R"
            }),
        );
        let im = im.finish();
        FileInfo {
            path: "volume".into(),
            size_bytes: 0,
            format: FormatDescriptor {
                id: "synth".into(),
                name: "Synthetic volume".into(),
                vendor: "test".into(),
                extensions: vec![],
                family: "medical-imaging".into(),
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

impl Dataset for Volume {
    fn info(&self) -> Result<FileInfo> {
        Ok(Volume::file_info())
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
        if image != 0 || i.c != 0 || i.t != 0 || i.z >= NZ {
            return Err(Error::Usage("out of range".into()));
        }
        let mut data = Vec::with_capacity((NX * NY) as usize);
        for y in 0..NY {
            for x in 0..NX {
                data.push(value(x, y));
            }
        }
        Ok(Plane {
            width: NX,
            height: NY,
            pixel_type: PixelType::Uint8,
            samples_per_pixel: 1,
            data,
        })
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("volume", "synth"))
    }
}

fn preview(axes: bool, region: Option<Region>, max_size: u32) -> Rendered {
    let mut req = PreviewRequest::default();
    req.mip = Some(Axis::Z);
    req.axes = axes;
    req.region = region;
    req.max_size = max_size;
    render(&mut Volume, &Volume::file_info(), &req).unwrap()
}

/// The full-resolution coordinate of the brightest pixel of the plot area.
fn brightest(r: &Rendered) -> (f64, f64) {
    let im = r.output.image.as_ref().unwrap();
    let a = im.plot_area;
    let (mut best, mut at) = (0u8, (0, 0));
    for y in a.y..a.y + a.height {
        for x in a.x..a.x + a.width {
            let g = r.canvas.rgb[((y * r.canvas.width + x) * 3) as usize];
            if g > best {
                (best, at) = (g, (x, y));
            }
        }
    }
    (
        im.source_origin.x + (f64::from(at.0 - a.x) + 0.5) * im.source_per_pixel,
        im.source_origin.y + (f64::from(at.1 - a.y) + 0.5) * im.source_per_pixel_y,
    )
}

#[test]
fn a_region_read_off_the_rulers_shows_that_part_of_a_sagittal_view() {
    let full = preview(true, None, 400);
    let im = full.output.image.as_ref().unwrap();
    assert_eq!(im.view.as_deref(), Some("sagittal"));
    // the view shows the stored rows across and the stored columns down
    assert_eq!((im.source_width, im.source_height), (NY, NX));
    let (x, y) = brightest(&full);
    assert!(
        (x - f64::from(SPOT.1) - 0.5).abs() < 1.5 && (y - f64::from(SPOT.0) - 0.5).abs() < 1.5,
        "the spot is at ({x:.1}, {y:.1}) on the rulers"
    );
    // a region around what the rulers say, read back at one pixel per source pixel
    let r = Region {
        x: (x as u32).saturating_sub(8),
        y: (y as u32).saturating_sub(8),
        width: 16,
        height: 16,
    };
    let zoom = preview(false, Some(r), 16);
    let zim = zoom.output.image.as_ref().unwrap();
    assert_eq!(zim.full_res_region, r);
    let (zx, zy) = brightest(&zoom);
    assert!(
        (zx - x).abs() < 1.5 && (zy - y).abs() < 1.5,
        "the rulers put the spot at ({x:.1}, {y:.1}); the region shows it at ({zx:.1}, {zy:.1})"
    );
}

#[test]
fn a_title_is_drawn_without_axes() {
    let mut req = PreviewRequest::default();
    req.axes = false;
    req.max_size = 300;
    let bare = render(&mut Volume, &Volume::file_info(), &req).unwrap();
    assert_eq!(bare.output.image.as_ref().unwrap().plot_area.y, 0);
    req.title = Some("Normal".into());
    let titled = render(&mut Volume, &Volume::file_info(), &req).unwrap();
    let im = titled.output.image.as_ref().unwrap();
    // the picture moves down under the title, with no rulers and no scale bar
    assert!(im.plot_area.y > 0, "{:?}", im.plot_area);
    assert!(!im.axes && im.scale_bar.is_none());
    assert!(titled.figure.to_svg().contains("Normal"));
    assert!(
        titled.output.notes.iter().all(|n| !n.contains("title")),
        "{:?}",
        titled.output.notes
    );
}
