//! Write synthetic datasets to OME-Zarr and check the store layout, metadata and pixels.
#![allow(clippy::many_single_char_names)] // c, z, t, x, y are the axis names

use std::path::Path;

use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, FormatDescriptor, ImageInfo, LsEntry, PhysicalSize,
};
use openreadout_core::reader::PlaneIndex;
use openreadout_core::{Confidence, Dataset, PixelType, Plane, ProvenanceMap, Result};
use openreadout_ometiff::Codec;
use openreadout_omezarr::{ZarrExportOptions, export_ome_zarr};
use serde_json::Value;

/// Deterministic pixel value for (image, c, z, t, sample, x, y).
fn value(i: u32, c: u32, z: u32, t: u32, s: u32, x: u32, y: u32) -> u32 {
    (i * 7 + c * 1000 + z * 100 + t * 10 + s * 3 + x * 13 + y * 31) % 60_000
}

struct Fake {
    images: Vec<ImageInfo>,
}

impl Dataset for Fake {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: "fake.bin".into(),
            size_bytes: 1,
            format: FormatDescriptor {
                id: "fake".into(),
                name: "Fake".into(),
                vendor: "none".into(),
                extensions: vec![],
                family: "microscopy".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            images: self.images.clone(),
            tables: vec![],
            spectra: vec![],
            traces: Vec::new(),
            plane_count: self.images.iter().map(|i| i.plane_count).sum(),
            notes: vec![],
        })
    }
    fn vendor_metadata(&self) -> Result<Value> {
        Ok(serde_json::json!({"k": "v"}))
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::new()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![])
    }
    fn read_plane(&mut self, image: u32, ix: PlaneIndex) -> Result<Plane> {
        let im = &self.images[image as usize];
        let spp = im.samples_per_pixel;
        let mut data = Vec::new();
        for y in 0..im.size_y {
            for x in 0..im.size_x {
                for s in 0..spp {
                    let v = value(image, ix.c, ix.z, ix.t, s, x, y);
                    match im.pixel_type {
                        PixelType::Uint8 => data.push((v % 256) as u8),
                        PixelType::Uint16 => data.extend_from_slice(&(v as u16).to_le_bytes()),
                        PixelType::Float => data.extend_from_slice(&(v as f32 * 0.5).to_le_bytes()),
                        // uint32 images are blank, like an empty label image
                        PixelType::Uint32 => data.extend_from_slice(&0u32.to_le_bytes()),
                        _ => unreachable!(),
                    }
                }
            }
        }
        Ok(Plane {
            width: im.size_x,
            height: im.size_y,
            pixel_type: im.pixel_type,
            samples_per_pixel: spp,
            data,
        })
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("fake", "fake"))
    }
}

fn gray(index: u32, w: u32, h: u32, c: u32, z: u32, t: u32) -> ImageInfo {
    let mut im = ImageInfo::new(index, w, h, PixelType::Uint16);
    im.size_c = c;
    im.size_z = z;
    im.size_t = t;
    im.physical_size = PhysicalSize::micrometres(Some(0.5), Some(0.5), Some(2.0));
    im.channels = (0..c)
        .map(|i| ChannelInfo {
            index: i,
            name: Some(format!("ch{i}")),
            ..Default::default()
        })
        .collect();
    im.finish()
}

fn read_json(p: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

#[test]
fn single_image_at_root_with_pyramid() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("one.ome.zarr");
    let mut ds = Fake {
        images: vec![gray(0, 1100, 70, 2, 3, 2)],
    };
    let opts = {
        let mut zarr_export_options = ZarrExportOptions::default();
        zarr_export_options.chunk = 256;
        zarr_export_options
    };
    let r = export_ome_zarr(&mut ds, Path::new("fake.bin"), &out, &opts).unwrap();
    assert!(r.verified);
    assert_eq!(r.format, "ome-zarr");
    assert_eq!(r.images_written, 1);
    assert_eq!(r.planes_written, 12);
    // a single image stays at the root; its OME-XML sits in `OME/` as in collections
    assert!(r.ome_xml_bytes > 0);
    assert!(out.join("OME").join("METADATA.ome.xml").is_file());
    assert!(r.bytes_written > 0);

    let root = read_json(&out.join("zarr.json"));
    assert_eq!(root["node_type"], "group");
    let ome = &root["attributes"]["ome"];
    assert_eq!(ome["version"], "0.5");
    let ms = &ome["multiscales"][0];
    assert_eq!(ms["datasets"].as_array().unwrap().len(), 2);
    assert_eq!(ome["omero"]["channels"][1]["label"], "ch1");

    let a0 = read_json(&out.join("0").join("zarr.json"));
    assert_eq!(a0["shape"], serde_json::json!([2, 2, 3, 70, 1100]));
    assert_eq!(
        a0["chunk_grid"]["configuration"]["chunk_shape"],
        serde_json::json!([1, 1, 1, 70, 256])
    );
    assert_eq!(
        a0["dimension_names"],
        serde_json::json!(["t", "c", "z", "y", "x"])
    );
    assert_eq!(a0["data_type"], "uint16");
    assert!(a0.to_string().contains("gzip"));
    let a1 = read_json(&out.join("1").join("zarr.json"));
    assert_eq!(a1["shape"], serde_json::json!([2, 2, 3, 35, 550]));
    // no temp directory left behind
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains("partial"))
        .collect();
    assert!(leftovers.is_empty());

    // refusing to overwrite without the flag
    let e = export_ome_zarr(&mut ds, Path::new("fake.bin"), &out, &opts).unwrap_err();
    assert_eq!(e.exit_code(), 2);
    // with the flag it replaces the store
    let opts = {
        let mut zarr_export_options = ZarrExportOptions::default();
        zarr_export_options.overwrite = true;
        zarr_export_options.levels = Some(1);
        zarr_export_options.codec = Codec::None;
        zarr_export_options.select = vec!["c=1".into(), "t=1".into()];
        zarr_export_options
    };
    let r = export_ome_zarr(&mut ds, Path::new("fake.bin"), &out, &opts).unwrap();
    assert_eq!(r.planes_written, 3);
    assert!(!out.join("1").exists());
    let root = read_json(&out.join("zarr.json"));
    assert_eq!(
        root["attributes"]["ome"]["omero"]["channels"][0]["label"],
        "ch1"
    );
}

#[test]
fn multi_image_collection_and_rgb() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("many.ome.zarr");
    let mut rgb = ImageInfo::new(1, 40, 30, PixelType::Uint8);
    rgb.samples_per_pixel = 3;
    let rgb = rgb.finish();
    let mut f = ImageInfo::new(2, 17, 9, PixelType::Float);
    f.size_t = 2;
    let f = f.finish();
    let mut ds = Fake {
        images: vec![gray(0, 64, 48, 2, 1, 1), rgb, f],
    };
    let opts = {
        let mut zarr_export_options = ZarrExportOptions::default();
        zarr_export_options.embed_vendor = true;
        zarr_export_options.levels = Some(3);
        zarr_export_options
    };
    let r = export_ome_zarr(&mut ds, Path::new("fake.bin"), &out, &opts).unwrap();
    assert!(r.verified);
    assert_eq!(r.images_written, 3);
    assert_eq!(r.planes_written, 2 + 1 + 2);
    assert!(r.ome_xml_bytes > 0);
    let root = read_json(&out.join("zarr.json"));
    assert_eq!(root["attributes"]["ome"]["bioformats2raw.layout"], 3);
    let series = read_json(&out.join("OME").join("zarr.json"));
    assert_eq!(
        series["attributes"]["ome"]["series"],
        serde_json::json!(["0", "1", "2"])
    );
    let xml = std::fs::read_to_string(out.join("OME").join("METADATA.ome.xml")).unwrap();
    assert_eq!(xml.matches("<MetadataOnly/>").count(), 3);
    assert!(!xml.contains("TiffData"));
    assert!(xml.contains("vendor-metadata-json"));
    // RGB becomes three channels
    let a = read_json(&out.join("1").join("0").join("zarr.json"));
    assert_eq!(a["shape"], serde_json::json!([1, 3, 1, 30, 40]));
    let g = read_json(&out.join("1").join("zarr.json"));
    assert_eq!(
        g["attributes"]["ome"]["omero"]["channels"][2]["color"],
        "0000FF"
    );
    // three explicit levels, smallest last
    let a = read_json(&out.join("2").join("2").join("zarr.json"));
    assert_eq!(a["shape"], serde_json::json!([2, 1, 1, 3, 5]));
    assert_eq!(a["data_type"], "float32");
}

#[test]
fn rejects_bad_options_and_protects_input() {
    let dir = tempfile::tempdir().unwrap();
    let mut ds = Fake {
        images: vec![gray(0, 8, 8, 1, 1, 1)],
    };
    let out = dir.path().join("x.ome.zarr");
    for opts in [
        {
            let mut zarr_export_options = ZarrExportOptions::default();
            zarr_export_options.codec = Codec::Lzw;
            zarr_export_options
        },
        {
            let mut zarr_export_options = ZarrExportOptions::default();
            zarr_export_options.chunk = 0;
            zarr_export_options
        },
        {
            let mut zarr_export_options = ZarrExportOptions::default();
            zarr_export_options.levels = Some(0);
            zarr_export_options
        },
        {
            let mut zarr_export_options = ZarrExportOptions::default();
            zarr_export_options.image = Some(5);
            zarr_export_options
        },
        {
            let mut zarr_export_options = ZarrExportOptions::default();
            zarr_export_options.select = vec!["c=9".into()];
            zarr_export_options
        },
    ] {
        let e = export_ome_zarr(&mut ds, Path::new("fake.bin"), &out, &opts).unwrap_err();
        assert_eq!(e.exit_code(), 2, "{opts:?}: {e}");
    }
    assert!(!out.exists());
    // an existing directory that is not a Zarr store is never replaced
    let plain = dir.path().join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("keep.txt"), b"x").unwrap();
    let opts = {
        let mut zarr_export_options = ZarrExportOptions::default();
        zarr_export_options.overwrite = true;
        zarr_export_options
    };
    let e = export_ome_zarr(&mut ds, Path::new("fake.bin"), &plain, &opts).unwrap_err();
    assert_eq!(e.exit_code(), 2);
    assert!(plain.join("keep.txt").exists());
    // nor a directory containing the input
    let input = plain.join("keep.txt");
    let e = export_ome_zarr(&mut ds, &input, &plain, &opts).unwrap_err();
    assert!(e.to_string().contains("contains the input"));
}

/// Every exported plane read back through the OME-Zarr reader (`openreadout-zarr`) hashes the
/// same as the source plane; interleaved RGB samples come back as one channel each.
fn assert_reads_back(store: &Path, ds: &mut Fake, levels: u32) {
    use openreadout_core::reader::FormatReader;
    let mut back = openreadout_zarr::ZarrReader.open(store).unwrap();
    let info = back.info().unwrap();
    assert_eq!(info.images.len(), ds.images.len());
    for (i, src) in ds.images.clone().iter().enumerate() {
        let im = &info.images[i];
        let spp = src.samples_per_pixel;
        assert_eq!(
            (im.size_x, im.size_y, im.size_z, im.size_c, im.size_t),
            (
                src.size_x,
                src.size_y,
                src.size_z,
                src.size_c * spp,
                src.size_t
            )
        );
        assert_eq!(im.pixel_type, src.pixel_type);
        assert_eq!(im.physical_size, src.physical_size);
        assert_eq!(im.pyramid_levels, levels);
        for c in 0..src.size_c {
            for z in 0..src.size_z {
                for t in 0..src.size_t {
                    let p = ds.read_plane(i as u32, PlaneIndex { c, z, t }).unwrap();
                    let bps = p.pixel_type.bytes_per_sample();
                    for s in 0..spp {
                        let got = back
                            .read_plane(
                                i as u32,
                                PlaneIndex {
                                    c: c * spp + s,
                                    z,
                                    t,
                                },
                            )
                            .unwrap();
                        let want: Vec<u8> = if spp == 1 {
                            p.data.clone()
                        } else {
                            let s = s as usize;
                            p.data
                                .chunks_exact(bps * spp as usize)
                                .flat_map(|px| px[s * bps..(s + 1) * bps].to_vec())
                                .collect()
                        };
                        assert_eq!(
                            got.xxh3_hex(),
                            format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&want)),
                            "image {i} c={c} s={s} z={z} t={t}"
                        );
                    }
                }
            }
        }
        if im.pyramid_levels > 1 {
            let l1 = back
                .read_plane_level(i as u32, PlaneIndex::default(), 1)
                .unwrap();
            assert_eq!(
                (l1.width, l1.height),
                (src.size_x.div_ceil(2), src.size_y.div_ceil(2))
            );
        }
    }
}

#[test]
fn exports_read_back_hash_identical_through_the_ome_zarr_reader() {
    let dir = tempfile::tempdir().unwrap();
    // one image at the store root, with a pyramid
    let mut ds = Fake {
        images: vec![gray(0, 70, 33, 2, 3, 2)],
    };
    let out = dir.path().join("one.ome.zarr");
    let opts = {
        let mut zarr_export_options = ZarrExportOptions::default();
        zarr_export_options.levels = Some(2);
        zarr_export_options.chunk = 32;
        zarr_export_options
    };
    export_ome_zarr(&mut ds, Path::new("fake.bin"), &out, &opts).unwrap();
    assert_reads_back(&out, &mut ds, 2);
    // a bioformats2raw collection: grayscale, RGB and float images, uncompressed
    let mut rgb = ImageInfo::new(1, 40, 30, PixelType::Uint8);
    rgb.samples_per_pixel = 3;
    let mut f = ImageInfo::new(2, 17, 9, PixelType::Float);
    f.size_t = 2;
    let mut ds = Fake {
        images: vec![gray(0, 64, 48, 2, 1, 1), rgb.finish(), f.finish()],
    };
    let out = dir.path().join("many.ome.zarr");
    let opts = {
        let mut zarr_export_options = ZarrExportOptions::default();
        zarr_export_options.codec = Codec::None;
        zarr_export_options.levels = Some(1);
        zarr_export_options
    };
    export_ome_zarr(&mut ds, Path::new("fake.bin"), &out, &opts).unwrap();
    assert_reads_back(&out, &mut ds, 1);
    let back = {
        use openreadout_core::reader::FormatReader;
        openreadout_zarr::ZarrReader.open(&out).unwrap()
    };
    let info = back.info().unwrap();
    assert!(
        info.format_version
            .unwrap()
            .starts_with("0.5 (NGFF, Zarr v3")
    );
    assert_eq!(info.images[0].channels[1].name.as_deref(), Some("ch1"));
}

#[test]
fn a_blank_image_does_not_make_the_export_look_unfinished() {
    use openreadout_core::reader::FormatReader;
    // Found by the October 2026 deep pass: an OME-Zarr label image with no labels exported
    // without chunk files, and the reader took the fresh store for an acquisition in
    // progress, so `planes` returned nothing for five minutes.
    let dir = tempfile::tempdir().unwrap();
    let blank = ImageInfo::new(1, 40, 30, PixelType::Uint32).finish();
    let mut ds = Fake {
        images: vec![gray(0, 64, 48, 1, 1, 1), blank],
    };
    let out = dir.path().join("blank.ome.zarr");
    let opts = {
        let mut zarr_export_options = ZarrExportOptions::default();
        zarr_export_options.levels = Some(1);
        zarr_export_options
    };
    export_ome_zarr(&mut ds, Path::new("fake.bin"), &out, &opts).unwrap();
    assert!(
        out.join("1").join("0").join("c").is_dir(),
        "blank chunks are stored"
    );
    let back = openreadout_zarr::ZarrReader.open(&out).unwrap();
    assert!(
        back.write_state().is_none(),
        "a finished export is not growing"
    );
    assert_reads_back(&out, &mut ds, 1);
}

#[test]
fn a_store_growing_image_by_image_keeps_its_finished_images() {
    use openreadout_core::reader::FormatReader;
    // A plate written well by well: the first image is finished, the second has no chunk
    // files yet. The finished image's planes are complete. Before the October 2026 deep
    // pass only planes of unfinished images were counted, so here none were.
    let dir = tempfile::tempdir().unwrap();
    let mut ds = Fake {
        images: vec![gray(0, 64, 48, 2, 1, 1), gray(1, 64, 48, 2, 1, 1)],
    };
    let out = dir.path().join("growing.ome.zarr");
    let opts = {
        let mut zarr_export_options = ZarrExportOptions::default();
        zarr_export_options.levels = Some(1);
        zarr_export_options
    };
    export_ome_zarr(&mut ds, Path::new("fake.bin"), &out, &opts).unwrap();
    std::fs::remove_dir_all(out.join("1").join("0").join("c")).unwrap();
    let back = openreadout_zarr::ZarrReader.open(&out).unwrap();
    let ws = back
        .write_state()
        .expect("the second image is not written yet");
    let mut complete: Vec<(u32, u32, u32, u32)> = ws
        .complete
        .iter()
        .map(|(i, p)| (*i, p.c, p.z, p.t))
        .collect();
    complete.sort_unstable();
    assert_eq!(complete, vec![(0, 0, 0, 0), (0, 1, 0, 0)]);
    assert_eq!(ws.expected_planes, Some(4));
}

#[test]
fn a_file_with_no_images_is_refused_with_a_reason() {
    // Found by the October 2026 deep pass on metadata-only VSI files: both image exports
    // said "selection matches no planes" (exit 2) with a hint about `--select`.
    let dir = tempfile::tempdir().unwrap();
    let mut ds = Fake { images: vec![] };
    let e = export_ome_zarr(
        &mut ds,
        Path::new("fake.bin"),
        &dir.path().join("none.ome.zarr"),
        &ZarrExportOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 6, "{e}");
    assert!(e.to_string().contains("no images"), "{e}");
    let e = openreadout_ometiff::export_ome_tiff(
        &mut ds,
        Path::new("fake.bin"),
        &dir.path().join("none.ome.tiff"),
        &openreadout_ometiff::ExportOptions::default(),
    )
    .unwrap_err();
    assert_eq!(e.exit_code(), 6, "{e}");
    assert!(e.hint().is_some_and(|h| h.contains("info FILE")), "{e}");
}
