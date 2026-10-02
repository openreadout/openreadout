//! Tiled, pyramidal OME-TIFF export of a synthetic pyramidal source: the source's own levels
//! (SubIFDs), 2 × 2 mean levels, a region and a downsampled level; read back with the `tiff`
//! crate (independent of our reader) and with OpenReadout's TIFF reader.

use std::path::Path;

use openreadout_core::model::{CheckReport, FileInfo, FormatDescriptor, ImageInfo, LsEntry};
use openreadout_core::region::{Region, ResolutionLevel, crop};
use openreadout_core::{
    Confidence, Dataset, Error, FormatReader, PixelType, Plane, PlaneIndex, ProvenanceMap, Result,
};
use openreadout_ometiff::pyramid::downsample_2x;
use openreadout_ometiff::{Codec, ExportOptions, PyramidMode, export_ome_tiff};

/// Two uint16 channels of 300 × 200 with two stored levels (150 × 100, 75 × 50) whose pixels are
/// deliberately *not* 2 × 2 means, so copied and computed levels can be told apart.
struct Pyr {
    info: ImageInfo,
}

fn value(level: u32, c: u32, x: u32, y: u32) -> u16 {
    ((x * 7 + y * 13 + c * 1000 + level * 20_000) % 65_000) as u16
}

impl Pyr {
    fn new() -> Self {
        let mut im = ImageInfo::new(0, 300, 200, PixelType::Uint16);
        im.size_c = 2;
        im.pyramid_levels = 3;
        im.physical_size.x = Some(0.5);
        im.physical_size.y = Some(0.5);
        im.resolution_levels = vec![
            ResolutionLevel::new(0, 300, 200, 300, 200),
            ResolutionLevel::new(1, 150, 100, 300, 200),
            ResolutionLevel::new(2, 75, 50, 300, 200),
        ];
        Pyr { info: im.finish() }
    }
    fn plane(level: u32, c: u32) -> Plane {
        let (w, h) = [(300, 200), (150, 100), (75, 50)][level as usize];
        let mut data = Vec::with_capacity(w as usize * h as usize * 2);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&value(level, c, x, y).to_le_bytes());
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
            images: vec![self.info.clone()],
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
        Ok(Pyr::plane(0, idx.c))
    }
    fn read_plane_level(&mut self, _: u32, idx: PlaneIndex, level: u32) -> Result<Plane> {
        if level > 2 {
            return Err(Error::Usage("level".into()));
        }
        Ok(Pyr::plane(level, idx.c))
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("pyr", "pyr"))
    }
}

fn read_back(path: &Path) -> Box<dyn Dataset> {
    openreadout_tiff::TiffReader
        .open_input(&openreadout_core::Input::local(path))
        .unwrap()
}

fn opts(pyramid: PyramidMode, tile: u32) -> ExportOptions {
    let mut o = ExportOptions::default();
    o.pyramid = pyramid;
    o.tile = tile;
    o.codec = Codec::Deflate;
    o
}

#[test]
fn source_pyramid_goes_to_subifds() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("p.ome.tiff");
    let mut ds = Pyr::new();
    // Auto: the source has a pyramid and the whole image is exported → its own levels.
    let r = export_ome_tiff(
        &mut ds,
        Path::new("pyr"),
        &out,
        &opts(PyramidMode::Auto, 64),
    )
    .unwrap();
    assert!(r.verified);
    assert_eq!(r.planes_written, 2);
    assert_eq!(r.resolutions[0].pyramid, PyramidMode::Source);
    assert_eq!(
        r.resolutions[0].sizes,
        vec![[300, 200], [150, 100], [75, 50]]
    );
    // Our reader: three levels, each equal to the source's.
    let mut back = read_back(&out);
    let info = back.info().unwrap();
    assert_eq!(info.images[0].pyramid_levels, 3);
    for c in 0..2 {
        for l in 0..3 {
            let p = back
                .read_plane_level(0, PlaneIndex { c, z: 0, t: 0 }, l)
                .unwrap();
            assert_eq!(p.data, Pyr::plane(l, c).data, "c={c} level {l}");
        }
    }
    // The `tiff` crate (independent decoder): the main chain holds the two full-resolution
    // planes, tiled 64 × 64.
    let f = std::fs::File::open(&out).unwrap();
    let mut dec = tiff::decoder::Decoder::new(std::io::BufReader::new(f)).unwrap();
    for c in 0..2 {
        if c > 0 {
            dec.next_image().unwrap();
        }
        assert_eq!(dec.dimensions().unwrap(), (300, 200));
        assert_eq!(dec.chunk_dimensions(), (64, 64));
        let tiff::decoder::DecodingResult::U16(v) = dec.read_image().unwrap() else {
            panic!("not u16");
        };
        let want: Vec<u16> = Pyr::plane(0, c)
            .data
            .chunks(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(v, want, "plane {c}");
    }
    assert!(
        !dec.more_images(),
        "levels are SubIFDs, not main-chain pages"
    );
}

#[test]
fn mean_pyramid_of_a_region_of_a_level() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("r.ome.tiff");
    let mut ds = Pyr::new();
    let mut o = opts(PyramidMode::Mean, 16);
    o.level = 1;
    o.region = Some(Region::new(10, 20, 130, 70));
    o.levels = Some(4);
    o.select = vec!["c=1".into()];
    let r = export_ome_tiff(&mut ds, Path::new("pyr"), &out, &o).unwrap();
    assert!(r.verified);
    assert_eq!(
        r.resolutions[0].sizes,
        vec![[130, 70], [65, 35], [33, 18], [17, 9]]
    );
    assert_eq!(r.resolutions[0].source_level, 1);
    let mut want = crop(Pyr::plane(1, 1), Region::new(10, 20, 130, 70), "t").unwrap();
    let mut back = read_back(&out);
    let info = back.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_c), (130, 70, 1));
    // Level 1 of the source: the pixel size doubles.
    assert_eq!(im.physical_size.x, Some(1.0));
    for l in 0..4 {
        let p = back.read_plane_level(0, PlaneIndex::default(), l).unwrap();
        assert_eq!(p.data, want.data, "level {l}");
        want = downsample_2x(&want);
    }
}

#[test]
fn bad_options_are_usage_errors() {
    let dir = tempfile::tempdir().unwrap();
    let mut ds = Pyr::new();
    let mut o = opts(PyramidMode::Mean, 20);
    let e = export_ome_tiff(&mut ds, Path::new("pyr"), &dir.path().join("a.tif"), &o).unwrap_err();
    assert_eq!(e.exit_code(), 2, "{e}"); // tile not a multiple of 16
    o.tile = 64;
    o.region = Some(Region::new(250, 0, 100, 10));
    let e = export_ome_tiff(&mut ds, Path::new("pyr"), &dir.path().join("b.tif"), &o).unwrap_err();
    assert_eq!(e.exit_code(), 2, "{e}"); // region past the image
    o.region = None;
    o.level = 3;
    let e = export_ome_tiff(&mut ds, Path::new("pyr"), &dir.path().join("c.tif"), &o).unwrap_err();
    assert_eq!(e.exit_code(), 2, "{e}");
    assert!(!dir.path().join("a.tif").exists() && !dir.path().join("c.tif").exists());
}
