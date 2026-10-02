//! Streaming OME-Zarr export: a synthetic source with its own pyramid is exported with its
//! levels copied, a region with mean levels, and read back with OpenReadout's Zarr reader.

use std::path::Path;

use openreadout_core::model::{CheckReport, FileInfo, FormatDescriptor, ImageInfo, LsEntry};
use openreadout_core::region::{Region, ResolutionLevel, crop};
use openreadout_core::{
    Confidence, Dataset, Error, FormatReader, PixelType, Plane, PlaneIndex, ProvenanceMap, Result,
};
use openreadout_ometiff::pyramid::downsample_2x;
use openreadout_omezarr::{PyramidMode, ZarrExportOptions, export_ome_zarr};

struct Pyr(ImageInfo);

fn plane(level: u32, c: u32) -> Plane {
    let (w, h) = [(300u32, 200u32), (100, 67), (34, 23)][level as usize];
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.push(((x * 3 + y * 5 + c * 40 + level * 90) % 251) as u8);
        }
    }
    Plane {
        width: w,
        height: h,
        pixel_type: PixelType::Uint8,
        samples_per_pixel: 1,
        data,
    }
}

impl Pyr {
    fn new() -> Self {
        let mut im = ImageInfo::new(0, 300, 200, PixelType::Uint8);
        im.size_c = 2;
        im.pyramid_levels = 3;
        im.physical_size.x = Some(0.25);
        im.physical_size.y = Some(0.25);
        // A 3x pyramid: copied levels keep their own scale.
        let mut l1 = ResolutionLevel::new(1, 100, 67, 300, 200);
        l1.downsample_x = 3.0;
        l1.downsample_y = 3.0;
        let mut l2 = ResolutionLevel::new(2, 34, 23, 300, 200);
        l2.downsample_x = 9.0;
        l2.downsample_y = 9.0;
        im.resolution_levels = vec![ResolutionLevel::new(0, 300, 200, 300, 200), l1, l2];
        Pyr(im.finish())
    }
}

impl Dataset for Pyr {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: "pyr".into(),
            size_bytes: 0,
            format: FormatDescriptor {
                id: "pyr".into(),
                name: "Synthetic pyramid".into(),
                vendor: String::new(),
                extensions: vec![],
                family: "microscopy".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            plane_count: 2,
            images: vec![self.0.clone()],
            tables: vec![],
            spectra: vec![],
            traces: vec![],
            notes: vec![],
        })
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::new()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![])
    }
    fn read_plane(&mut self, _: u32, idx: PlaneIndex) -> Result<Plane> {
        Ok(plane(0, idx.c))
    }
    fn read_plane_level(&mut self, _: u32, idx: PlaneIndex, level: u32) -> Result<Plane> {
        if level > 2 {
            return Err(Error::Usage("level".into()));
        }
        Ok(plane(level, idx.c))
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("pyr", "pyr"))
    }
}

fn open(p: &Path) -> Box<dyn Dataset> {
    openreadout_zarr::ZarrReader.open(p).unwrap()
}

#[test]
fn source_levels_are_copied_with_their_scale() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("s.ome.zarr");
    let mut o = ZarrExportOptions::default();
    o.chunk = 64;
    let r = export_ome_zarr(&mut Pyr::new(), Path::new("pyr"), &out, &o).unwrap();
    assert!(r.verified);
    assert_eq!(r.resolutions[0].pyramid, PyramidMode::Source);
    assert_eq!(r.resolutions[0].factors, vec![1.0, 3.0, 9.0]);
    let attrs: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("zarr.json")).unwrap()).unwrap();
    let ms = &attrs["attributes"]["ome"]["multiscales"][0];
    assert_eq!(ms["type"], "source");
    assert_eq!(
        ms["datasets"][2]["coordinateTransformations"][0]["scale"][4],
        0.25 * 9.0
    );
    let mut back = open(&out);
    let info = back.info().unwrap();
    assert_eq!(info.images[0].pyramid_levels, 3);
    for c in 0..2 {
        for l in 0..3 {
            let p = back
                .read_plane_level(0, PlaneIndex { c, z: 0, t: 0 }, l)
                .unwrap();
            assert_eq!(p.data, plane(l, c).data, "c={c} level {l}");
        }
    }
}

#[test]
fn a_region_with_mean_levels() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("r.ome.zarr");
    let mut o = ZarrExportOptions::default();
    o.chunk = 32;
    o.region = Some(Region::new(7, 11, 250, 150));
    o.levels = Some(3);
    let r = export_ome_zarr(&mut Pyr::new(), Path::new("pyr"), &out, &o).unwrap();
    assert!(r.verified);
    assert_eq!(r.resolutions[0].pyramid, PyramidMode::Mean);
    assert_eq!(
        r.resolutions[0].sizes,
        vec![[250, 150], [125, 75], [63, 38]]
    );
    let mut back = open(&out);
    for c in 0..2 {
        let mut want = crop(plane(0, c), Region::new(7, 11, 250, 150), "t").unwrap();
        for l in 0..3 {
            let p = back
                .read_plane_level(0, PlaneIndex { c, z: 0, t: 0 }, l)
                .unwrap();
            assert_eq!(p.data, want.data, "c={c} level {l}");
            want = downsample_2x(&want);
        }
    }
    // pyramid none: one level only (through the in-memory writer).
    let out2 = dir.path().join("n.ome.zarr");
    o.region = None;
    o.pyramid = PyramidMode::None;
    o.levels = None;
    let r = export_ome_zarr(&mut Pyr::new(), Path::new("pyr"), &out2, &o).unwrap();
    assert!(r.resolutions.is_empty());
    assert_eq!(open(&out2).info().unwrap().images[0].pyramid_levels, 1);
}
