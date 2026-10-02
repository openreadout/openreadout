//! Reads the SYNTHETIC fixtures in `tests/fixtures/` (written by `make_fixtures.py`, cross-read by
//! liffile into `tests/fixtures/oracle/*.json`) and compares geometry, pixel types, tile offsets
//! and per-plane hashes. These files are not from a microscope; see the provenance log.

use std::path::{Path, PathBuf};

use openreadout_core::Error;
use openreadout_core::model::Severity;
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_lif::LifReader;
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn open(rel: &str) -> Box<dyn Dataset> {
    LifReader
        .open(&fixtures().join(rel))
        .unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// Compare one fixture against its liffile oracle; returns the number of planes compared.
fn compare(rel: &str, oracle: &str) -> usize {
    let o: Value = serde_json::from_str(
        &std::fs::read_to_string(fixtures().join("oracle").join(oracle)).unwrap(),
    )
    .unwrap();
    let mut ds = open(rel);
    let info = ds.info().unwrap();
    let images = o["images"].as_array().unwrap();
    assert_eq!(info.images.len(), images.len(), "{rel}: image count");
    let mut planes = 0;
    for oi in images {
        let idx = u32::try_from(oi["index"].as_u64().unwrap()).unwrap();
        let im = &info.images[idx as usize];
        let geo = (im.size_x, im.size_y, im.size_z, im.size_c, im.size_t);
        let ogeo = (
            oi["size_x"].as_u64().unwrap() as u32,
            oi["size_y"].as_u64().unwrap() as u32,
            oi["size_z"].as_u64().unwrap() as u32,
            oi["size_c"].as_u64().unwrap() as u32,
            oi["size_t"].as_u64().unwrap() as u32,
        );
        assert_eq!(geo, ogeo, "{rel} image {idx} ({:?}) geometry", im.name);
        assert_eq!(
            im.pixel_type.ome_name(),
            oi["pixel_type"],
            "{rel} image {idx} pixel type"
        );
        if oi["flim"].as_bool() == Some(true) {
            let r = ds.read_plane(idx, PlaneIndex::default());
            assert!(
                matches!(r, Err(Error::Unsupported { .. })),
                "{rel} image {idx}: FLIM must be unsupported"
            );
            continue;
        }
        if let Some(m) = oi.get("mosaic").filter(|m| !m.is_null()) {
            let ours: Vec<(u64, u64)> = im.extra["tiles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| (t["x_px"].as_u64().unwrap(), t["y_px"].as_u64().unwrap()))
                .collect();
            let theirs: Vec<(u64, u64)> = m["tiles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| (t["x_px"].as_u64().unwrap(), t["y_px"].as_u64().unwrap()))
                .collect();
            assert_eq!(ours, theirs, "{rel} image {idx}: tile offsets");
        }
        for p in oi["planes"].as_array().unwrap() {
            let pi = PlaneIndex {
                c: p["c"].as_u64().unwrap() as u32,
                z: p["z"].as_u64().unwrap() as u32,
                t: p["t"].as_u64().unwrap() as u32,
            };
            let plane = ds.read_plane(idx, pi).unwrap();
            assert_eq!(
                plane.xxh3_hex(),
                p["xxh3"],
                "{rel} image {idx} plane {pi:?}"
            );
            planes += 1;
        }
    }
    planes
}

#[test]
fn synthetic_lif_axes_match_liffile() {
    // lambda x 2 channels, rotation (3 images), XT/T slices folded into T, FlipX tile scan,
    // half floats, FLIM element
    assert_eq!(
        compare("synthetic-dims.lif", "synthetic-dims.json"),
        6 + 2 * 3 + 12 + 2 + 1
    );
    let info = open("synthetic-dims.lif").info().unwrap();
    let names: Vec<String> = info.images[0]
        .channels
        .iter()
        .map(|c| c.name.clone().unwrap())
        .collect();
    assert_eq!(names[..3], ["Green 500 nm", "Green 520 nm", "Green 540 nm"]);
    assert_eq!(info.images[0].channels[4].emission_nm, Some(520.0));
    assert_eq!(
        info.images[1].name.as_deref(),
        Some("synthetic-dims.lif/Rotation [rotation 0]")
    );
    assert_eq!(info.images[3].extra["split"][0]["index"], 2);
    let tiles = &info.images[5];
    assert_eq!((tiles.size_x, tiles.size_y), (14, 5));
    assert!(tiles.mosaic.as_ref().unwrap().stitched_on_read);
    assert_eq!(info.images[6].extra["stored_pixel_type"], "float16");
    assert_eq!(info.images[7].extra["flim"]["histogram_bins"], 528);
}

#[test]
fn lof_files_match_liffile() {
    assert_eq!(compare("lof/Single.lof", "Single.json"), 2);
    assert_eq!(compare("lof/Legacy.lof", "Legacy.json"), 1);
    let info = open("lof/Legacy.lof").info().unwrap();
    assert_eq!(info.images[0].name.as_deref(), Some("Legacy"));
}

#[test]
fn xml_containers_match_liffile() {
    assert_eq!(compare("xlef/Series001.xlif", "Series001.json"), 2);
    // two LOF frames make up one memory block (Z 0 and Z 1)
    assert_eq!(
        compare("xlef/Collection/Collection.xlcf", "Collection.json"),
        2
    );
    assert_eq!(compare("xlef/Experiment.xlef", "Experiment.json"), 4);
    let info = open("xlef/Experiment.xlef").info().unwrap();
    let names: Vec<&str> = info
        .images
        .iter()
        .map(|i| i.name.as_deref().unwrap())
        .collect();
    assert_eq!(names, ["Series001", "Collection/Series002"]);
    let mut ds = open("xlef/Experiment.xlef");
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
}

#[test]
fn xllf_is_followed_like_a_collection() {
    let mut ds = open("xlef/Folder.xllf");
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 1);
    assert_eq!(info.images[0].name.as_deref(), Some("Folder/Series001"));
    assert!(ds.read_plane(0, PlaneIndex::default()).is_ok());
}

#[test]
fn broken_experiment_reports_cleanly() {
    let mut ds = open("xlef-broken/Broken.xlef");
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 1);
    let e = ds.read_plane(0, PlaneIndex::default()).unwrap_err();
    assert!(matches!(e, Error::Unsupported { .. }), "{e}");
    let r = ds.check().unwrap();
    assert!(!r.ok);
    let codes: Vec<(&str, Severity)> = r
        .findings
        .iter()
        .map(|f| (f.code.as_str(), f.severity))
        .collect();
    assert!(
        codes.contains(&("missing_reference", Severity::Error)),
        "{codes:?}"
    );
    assert!(
        codes.iter().any(|c| c.0 == "unsupported_frames"),
        "{codes:?}"
    );
}

#[test]
fn detection_by_content() {
    let sniff = |rel: &str| {
        let p = fixtures().join(rel);
        let head = std::fs::read(&p).unwrap();
        LifReader.sniff(&head[..head.len().min(65536)], &p)
    };
    for rel in [
        "synthetic-dims.lif",
        "lof/Single.lof",
        "xlef/Experiment.xlef",
        "xlef/Series001.xlif",
        "xlef/Collection/Series002.xlif",
        "xlef/Collection/Collection.xlcf",
        "xlef/Folder.xllf",
    ] {
        let d = sniff(rel).unwrap_or_else(|| panic!("{rel} not detected"));
        assert_eq!(
            d.confidence,
            openreadout_core::model::DetectConfidence::Definite,
            "{rel}"
        );
    }
}

#[test]
fn truncated_lof_and_garbage_are_clean_errors() {
    let dir = std::env::temp_dir().join(format!("lif-fixture-trunc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = std::fs::read(fixtures().join("lof/Single.lof")).unwrap();
    for cut in [10, 60, 80, src.len() - 7] {
        let p = dir.join("t.lof");
        std::fs::write(&p, &src[..cut]).unwrap();
        assert!(LifReader.open(&p).is_err(), "cut {cut}");
    }
    let p = dir.join("g.xlef");
    std::fs::write(&p, b"<LMSDataContainerHeader><Element Name=\"x\"><Children><Reference File=\"x.xlef\"/></Children></Element></LMSDataContainerHeader>").unwrap();
    let mut ds = LifReader.open(&p).unwrap();
    assert_eq!(ds.info().unwrap().images.len(), 0);
    assert!(
        ds.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "missing_reference")
    );
    std::fs::remove_dir_all(&dir).ok();
}
