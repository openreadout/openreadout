//! Reads the SYNTHETIC OME-Zarr fixtures in `tests/fixtures/` (written by
//! `oracle/make_zarr_fixtures.py` with zarr-python and ome-zarr-py, cross-read by zarr-python
//! into `tests/fixtures/oracle/*.json` by `oracle/gen.py`) both as zip stores and unpacked as
//! directory stores, and compares geometry, pixel types, physical sizes and per-plane hashes of
//! every image and pyramid level.
#![allow(clippy::many_single_char_names)]

use std::path::{Path, PathBuf};

use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::zip::ZipIndex;
use openreadout_zarr::ZarrReader;
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

const NAMES: [&str; 7] = [
    "ngff04-bf2raw-dtypes",
    "ngff04-v2-tczyx-blosc-lz4",
    "ngff04-v2-zyx-float-codecs",
    "ngff05-v3-yx-zstd-stored",
    "ngff05-v3-czyx-sharded",
    "ngff04-v2-plate",
    "ngff04-bf2raw-collection",
];

/// Unpack a zip into `dir` (the store root at `dir`).
fn unpack(zip: &Path, dir: &Path) {
    let z = ZipIndex::open(&openreadout_core::source::Fs::local(), zip, "ome-zarr").unwrap();
    for m in &z.members {
        let p = dir.join(&m.name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, z.read(m).unwrap()).unwrap();
    }
}

fn close(a: Option<f64>, b: &Value) -> bool {
    match (a, b.as_f64()) {
        (None, None) => true,
        (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * y.abs().max(1.0),
        _ => false,
    }
}

/// Compare one opened store with its oracle; returns the number of planes compared.
fn compare(name: &str, ds: &mut Box<dyn Dataset>, o: &Value) -> usize {
    let info = ds.info().unwrap();
    let images = o["images"].as_array().unwrap();
    assert_eq!(info.images.len(), images.len(), "{name}: image count");
    let mut n = 0;
    for oi in images {
        let idx = oi["index"].as_u64().unwrap() as u32;
        let im = &info.images[idx as usize];
        let geo = (im.size_x, im.size_y, im.size_z, im.size_c, im.size_t);
        let g = |k: &str| oi[k].as_u64().unwrap() as u32;
        assert_eq!(
            geo,
            (
                g("size_x"),
                g("size_y"),
                g("size_z"),
                g("size_c"),
                g("size_t")
            ),
            "{name} image {idx}: geometry"
        );
        assert_eq!(
            im.pixel_type.ome_name(),
            oi["pixel_type"],
            "{name} image {idx}"
        );
        let ps = &oi["physical_size_um"];
        assert!(
            close(im.physical_size.x, &ps["x"]),
            "{name} image {idx}: x {:?} vs {}",
            im.physical_size.x,
            ps["x"]
        );
        assert!(close(im.physical_size.y, &ps["y"]), "{name} image {idx}: y");
        if !ps["z"].is_null() {
            assert!(close(im.physical_size.z, &ps["z"]), "{name} image {idx}: z");
        }
        if !oi["time_increment_s"].is_null() {
            assert!(
                close(im.time_increment_s, &oi["time_increment_s"]),
                "{name} image {idx}: dt"
            );
        }
        let names: Vec<Option<String>> = im.channels.iter().map(|c| c.name.clone()).collect();
        for (k, on) in oi["channel_names"].as_array().unwrap().iter().enumerate() {
            // bioformats2raw collections take names from the OME-XML, not omero
            if o["kind"] != "bioformats2raw" {
                assert_eq!(
                    names[k].as_deref(),
                    on.as_str(),
                    "{name} image {idx}: channel {k}"
                );
            }
        }
        assert_eq!(
            im.pyramid_levels as usize,
            oi["level_sizes"].as_array().unwrap().len(),
            "{name} image {idx}: levels"
        );
        for p in oi["planes"].as_array().unwrap() {
            let pi = PlaneIndex {
                c: p["c"].as_u64().unwrap() as u32,
                z: p["z"].as_u64().unwrap() as u32,
                t: p["t"].as_u64().unwrap() as u32,
            };
            let plane = ds.read_plane(idx, pi).unwrap();
            assert_eq!(plane.xxh3_hex(), p["xxh3"], "{name} image {idx} {pi:?}");
            n += 1;
        }
        for l in oi["levels"].as_array().unwrap() {
            let level = l["level"].as_u64().unwrap() as u32;
            for p in l["planes"].as_array().unwrap() {
                let plane = ds
                    .read_plane_level(idx, PlaneIndex::default(), level)
                    .unwrap();
                assert_eq!(
                    (u64::from(plane.width), u64::from(plane.height)),
                    (l["size_x"].as_u64().unwrap(), l["size_y"].as_u64().unwrap())
                );
                assert_eq!(
                    plane.xxh3_hex(),
                    p["xxh3"],
                    "{name} image {idx} level {level}"
                );
                n += 1;
            }
        }
    }
    n
}

#[test]
fn fixtures_match_zarr_python_as_zip_and_directory() {
    let mut total = 0;
    for name in NAMES {
        let zip = fixtures().join(format!("{name}.zip"));
        let o: Value = serde_json::from_str(
            &std::fs::read_to_string(fixtures().join("oracle").join(format!("{name}.json")))
                .unwrap(),
        )
        .unwrap();
        let head = std::fs::read(&zip).unwrap();
        let det = ZarrReader.sniff(&head[..head.len().min(65536)], &zip);
        assert!(det.is_some(), "{name}: zip store not detected");
        let mut ds = ZarrReader
            .open(&zip)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        total += compare(name, &mut ds, &o);
        let r = ds.check().unwrap();
        assert!(r.ok, "{name}: check {:?}", r.findings);

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(format!("{name}.ome.zarr"));
        unpack(&zip, &root);
        assert!(
            ZarrReader.sniff(&[], &root).is_some(),
            "{name}: directory not detected"
        );
        let mut ds = ZarrReader.open(&root).unwrap();
        total += compare(name, &mut ds, &o);
        assert!(ds.check().unwrap().ok);
    }
    assert!(total > 80, "{total} planes compared");
}

#[test]
fn metadata_details() {
    let open = |n: &str| {
        ZarrReader
            .open(&fixtures().join(format!("{n}.zip")))
            .unwrap()
    };
    // labels are listed, levels readable, NGFF version and the omero colours kept
    let ds = open("ngff04-v2-tczyx-blosc-lz4");
    let info = ds.info().unwrap();
    assert_eq!(
        info.format_version.as_deref(),
        Some("0.4 (NGFF, Zarr v2, zip store)")
    );
    let im = &info.images[0];
    assert_eq!(im.dimension_order, "XYZCT");
    assert_eq!(im.channels[0].color.as_deref(), Some("#0000FF"));
    assert_eq!(im.time_increment_s, Some(2.5));
    // the label image (listed twice in the store) is one image after the image, bit-shuffled
    assert_eq!(info.images.len(), 2);
    assert_eq!(info.images[1].name.as_deref(), Some("labels/cells"));
    assert_eq!(info.images[1].extra["label_of"], 0);
    assert!(info.notes.iter().any(|n| n.contains("label image")));
    let ls = ds.entries().unwrap();
    assert!(
        ls.iter()
            .any(|e| e.kind == "label" && e.name == "labels/cells")
    );
    // two levels of the image, one of the label image
    assert_eq!(ls.iter().filter(|e| e.kind == "pyramid-level").count(), 3);
    // nanometre units become micrometres
    let info = open("ngff04-v2-zyx-float-codecs").info().unwrap();
    assert!((info.images[0].physical_size.x.unwrap() - 0.11).abs() < 1e-12);
    assert!((info.images[0].physical_size.z.unwrap() - 2.0).abs() < 1e-12);
    // NGFF 0.5 on Zarr v3
    let info = open("ngff05-v3-czyx-sharded").info().unwrap();
    assert_eq!(
        info.format_version.as_deref(),
        Some("0.5 (NGFF, Zarr v3, zip store)")
    );
    assert_eq!(info.images[0].dimension_order, "XYZCT");
    // plate: one image per field, named by well and field
    let ds = open("ngff04-v2-plate");
    let info = ds.info().unwrap();
    let names: Vec<_> = info
        .images
        .iter()
        .map(|i| i.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["A/1/0", "A/1/1", "A/2/0", "A/2/1"]);
    assert_eq!(info.images[2].extra["column"], "2");
    assert!(ds.entries().unwrap().iter().any(|e| e.kind == "plate"));
    // bioformats2raw: names, channels, objective and date from OME/METADATA.ome.xml
    let ds = open("ngff04-bf2raw-collection");
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!(im.name.as_deref(), Some("series zero"));
    assert_eq!(im.channels[1].name.as_deref(), Some("mCherry"));
    assert_eq!(im.channels[0].fluorophore.as_deref(), Some("Hoechst 33342"));
    assert_eq!(im.channels[0].emission_nm, Some(461.0));
    assert_eq!(
        im.objective.as_ref().unwrap().nominal_magnification,
        Some(20.0)
    );
    assert_eq!(im.acquired_at.as_deref(), Some("2024-05-06T07:08:09"));
    assert_eq!(info.images[1].time_increment_s, Some(30.0));
    // data types and composed transforms: y = 2 * (0.5 * i + 10) + 1, x = 2 * (0.25 * j - 4) + 3
    let info = open("ngff04-bf2raw-dtypes").info().unwrap();
    let types: Vec<&str> = info
        .images
        .iter()
        .map(|i| i.pixel_type.ome_name())
        .collect();
    assert_eq!(
        types,
        [
            "int64",
            "uint64",
            "float",
            "uint8",
            "complex",
            "double-complex"
        ]
    );
    let im = &info.images[0];
    assert_eq!(im.physical_size.y, Some(1.0));
    assert_eq!(im.physical_size.x, Some(0.5));
    assert_eq!(im.extra["origin_um"]["y"], 21.0);
    assert_eq!(im.extra["origin_um"]["x"], -5.0);
}

#[test]
fn errors_are_clean() {
    let dir = tempfile::tempdir().unwrap();
    // a truncated zip store
    let bytes = std::fs::read(fixtures().join("ngff04-v2-plate.zip")).unwrap();
    let p = dir.path().join("cut.zip");
    std::fs::write(&p, &bytes[..bytes.len() / 2]).unwrap();
    let e = ZarrReader.open(&p).map(|_| ()).unwrap_err();
    assert_eq!(e.exit_code(), 4, "{e}");
    // a plain Zarr group without NGFF metadata
    let g = dir.path().join("plain.zarr");
    std::fs::create_dir_all(&g).unwrap();
    std::fs::write(g.join(".zgroup"), br#"{"zarr_format": 2}"#).unwrap();
    let e = ZarrReader.open(&g).map(|_| ()).unwrap_err();
    assert_eq!(e.exit_code(), 6, "{e}");
    // plane index out of range, level out of range
    let mut ds = ZarrReader
        .open(&fixtures().join("ngff05-v3-yx-zstd-stored.zip"))
        .unwrap();
    let e = ds
        .read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 })
        .unwrap_err();
    assert_eq!(e.exit_code(), 2);
    assert_eq!(
        ds.read_plane_level(0, PlaneIndex::default(), 3)
            .unwrap_err()
            .exit_code(),
        2
    );
    // a corrupted chunk is an error, never a panic; check reports it
    let src = dir.path().join("broken.zarr");
    unpack(&fixtures().join("ngff05-v3-yx-zstd-stored.zip"), &src);
    std::fs::write(src.join("0/c/0/0"), b"not zstd").unwrap();
    let mut ds = ZarrReader.open(&src).unwrap();
    assert_eq!(
        ds.read_plane(0, PlaneIndex::default())
            .unwrap_err()
            .exit_code(),
        4
    );
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(r.findings.iter().any(|f| f.code == "chunk_decode"));
    // a missing chunk reads as the fill value and is reported as info
    std::fs::remove_file(src.join("0/c/0/0")).unwrap();
    let mut ds = ZarrReader.open(&src).unwrap();
    assert!(ds.read_plane(0, PlaneIndex::default()).is_ok());
    let r = ds.check().unwrap();
    assert!(r.ok);
    assert!(r.findings.iter().any(|f| f.code == "missing_chunks"));
}
