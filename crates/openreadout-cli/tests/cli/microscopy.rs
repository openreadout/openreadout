//! Microscopy: CZI, ND2, LIF, TIFF and OME-TIFF/OME-Zarr reads, truncation and exports.

use std::path::{Path, PathBuf};

use crate::common::*;

/// One plane's hash via `planes --image --select`, or the error code.
#[allow(clippy::many_single_char_names)]
fn nd2_plane(p: &std::path::Path, image: u32, c: u32, z: u32, t: u32) -> Result<String, String> {
    let out = bin()
        .args([
            "planes",
            p.to_str().unwrap(),
            "--json",
            "--image",
            &image.to_string(),
            "--select",
            &format!("c={c}"),
            "--select",
            &format!("z={z}"),
            "--select",
            &format!("t={t}"),
        ])
        .output()
        .unwrap();
    let v = json(&out);
    if out.status.success() {
        Ok(v["data"]["planes"][0]["xxh3"].as_str().unwrap().to_string())
    } else {
        assert_eq!(out.status.code(), Some(4), "{v}");
        Err(v["error"]["code"].as_str().unwrap().to_string())
    }
}

/// The OME-XML document embedded in an OME-TIFF (stored uncompressed in the first IFD).
fn embedded_ome_xml(p: &std::path::Path) -> String {
    let b = std::fs::read(p).unwrap();
    let find = |needle: &[u8]| b.windows(needle.len()).position(|w| w == needle);
    let start = find(b"<OME").expect("OME-XML start");
    let end = find(b"</OME>").expect("OME-XML end") + "</OME>".len();
    String::from_utf8(b[start..end].to_vec()).unwrap()
}

/// Attribute `name` of an XML start tag.
fn xml_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let i = tag.find(&key)? + key.len();
    tag[i..].split('"').next()
}

fn explain_json(p: &std::path::Path) -> serde_json::Value {
    let out = bin()
        .args(["info", "--view", "explain", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = json(&out);
    assert_eq!(v["ok"], true);
    let d = v["data"].clone();
    for k in ["summary", "paragraphs", "suggested_commands", "caveats"] {
        assert!(d.get(k).is_some(), "explain JSON lacks {k}");
    }
    d
}

fn all_text(d: &serde_json::Value) -> String {
    let mut s = d["summary"].as_str().unwrap().to_string();
    for p in d["paragraphs"].as_array().unwrap() {
        s.push('\n');
        s.push_str(p.as_str().unwrap());
    }
    s
}

fn export_zarr(
    src: &std::path::Path,
    out: &std::path::Path,
    extra: &[&str],
) -> std::process::Output {
    let mut args = vec![
        "export",
        src.to_str().unwrap(),
        "--format",
        "ome-zarr",
        "-o",
        out.to_str().unwrap(),
        "--json",
    ];
    args.extend_from_slice(extra);
    bin().args(args).output().unwrap()
}

fn lif_fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../openreadout-lif/tests/fixtures")
        .join(rel)
}

/// `export x -o x.ome.tiff` then `info x.ome.tiff` must reproduce the geometry, pixel type,
/// channel names and physical sizes, and every plane must hash identically.
fn assert_ome_tiff_roundtrip(src: &std::path::Path) {
    let dir = std::env::temp_dir().join(format!(
        "openreadout-rt-{}-{}",
        std::process::id(),
        src.file_stem().unwrap().to_string_lossy()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("rt.ome.tiff");
    let out = bin()
        .args([
            "export",
            src.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
            "--overwrite",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let info_of = |p: &std::path::Path| {
        let out = bin()
            .args(["info", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        json(&out)["data"].clone()
    };
    let a = info_of(src);
    let b = info_of(&out_path);
    assert_eq!(b["format"]["id"], "tiff");
    assert!(
        b["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("ome-tiff")),
        "{}",
        b["notes"]
    );
    let (ia, ib) = (
        a["images"].as_array().unwrap(),
        b["images"].as_array().unwrap(),
    );
    assert_eq!(ia.len(), ib.len(), "image count");
    for (x, y) in ia.iter().zip(ib) {
        for k in [
            "size_x",
            "size_y",
            "size_z",
            "size_c",
            "size_t",
            "pixel_type",
            "samples_per_pixel",
        ] {
            assert_eq!(x[k], y[k], "{k} of image {}", x["index"]);
        }
        let names = |im: &serde_json::Value| -> Vec<serde_json::Value> {
            im["channels"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["name"].clone())
                .collect()
        };
        assert_eq!(names(x), names(y), "channel names of image {}", x["index"]);
        for ax in ["x", "y", "z"] {
            match (
                x["physical_size"][ax].as_f64(),
                y["physical_size"][ax].as_f64(),
            ) {
                (Some(p), Some(q)) => {
                    assert!((p - q).abs() <= 1e-9 * p.abs().max(1.0), "{ax}: {p} vs {q}");
                }
                (None, None) => {}
                (p, q) => panic!("physical_size.{ax}: {p:?} vs {q:?}"),
            }
        }
    }
    let hashes = |p: &std::path::Path| -> Vec<(u64, u64, u64, u64, String)> {
        let out = bin()
            .args(["planes", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        json(&out)["data"]["planes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| {
                (
                    q["image"].as_u64().unwrap(),
                    q["c"].as_u64().unwrap(),
                    q["z"].as_u64().unwrap(),
                    q["t"].as_u64().unwrap(),
                    q["xxh3"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    assert_eq!(
        hashes(src),
        hashes(&out_path),
        "plane hashes after round trip"
    );
    let out = bin()
        .args(["check", out_path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn lif_info_check_and_truncation() {
    let Some(p) = corpus("ome-michael-PR2729-frameOrderCombinedScanTypes.lif") else {
        return;
    };
    let out = bin()
        .args(["info", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["data"]["format"]["id"], "lif");
    // one 2x2 tile scan of 64x64 tiles, stitched from stage positions
    assert_eq!(v["data"]["images"].as_array().unwrap().len(), 1);
    assert_eq!(v["data"]["images"][0]["size_x"], 97);
    assert_eq!(v["data"]["images"][0]["mosaic"]["tile_count"], 4);
    assert_eq!(v["data"]["images"][0]["mosaic"]["stitched_on_read"], true);
    let out = bin()
        .args(["check", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json(&out)["data"]["ok"], true);
    let t = truncated_copy(&p);
    let out = bin()
        .args(["check", t.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4), "truncated LIF must fail check");
    let v = json(&out);
    assert_eq!(v["data"]["ok"], false);
    let codes: Vec<&str> = v["data"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(
        codes
            .iter()
            .any(|c| *c == "truncated" || *c == "missing_planes"),
        "{codes:?}"
    );
    // reading a missing plane is a clean error, not a panic
    let out = bin()
        .args(["planes", t.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(json(&out)["error"]["code"], "corrupt_file");
}

#[test]
fn czi_truncation_is_detected() {
    let Some(p) = corpus("zenodo7015307-T-1-CH-2.czi") else {
        return;
    };
    let t = truncated_copy(&p);
    let out = bin()
        .args(["check", t.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    let v = json(&out);
    let codes: Vec<&str> = v["data"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap())
        .collect();
    assert!(!codes.is_empty(), "no findings on a truncated CZI");
}

#[test]
fn nd2_truncation_is_detected() {
    let Some(p) = corpus("aics-ND2-dims-t3c2y32x32.nd2") else {
        return;
    };
    let t = truncated_copy(&p);
    let out = bin()
        .args(["check", t.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
}

/// A frame chunk cut in the middle must make that plane a `corrupt_file` error (exit 4) while
/// frames written before it still read back identical to the intact file.
#[test]
fn nd2_truncated_frame_is_an_error_not_stale_pixels() {
    let Some(p) = corpus("aics-ND2-dims-p4z5t3c2y32x32.nd2") else {
        return;
    };
    let data = std::fs::read(&p).unwrap();
    // Locate frame 30 (T(3) x P(4) x Z(5): t=1, position 2, z=0) through `ls`.
    let out = bin()
        .args(["info", "--view", "structure", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let v = json(&out);
    let off = v["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "ImageDataSeq|30!")
        .and_then(|e| e["offset"].as_u64())
        .unwrap() as usize;
    let name_len = u32::from_le_bytes(data[off + 4..off + 8].try_into().unwrap()) as usize;
    let cut = off + 16 + name_len + 8 + 100; // inside the pixel block
    let dir = std::env::temp_dir().join(format!("openreadout-nd2cut-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let t = dir.join("cut.nd2");
    std::fs::write(&t, &data[..cut]).unwrap();
    // frame 30 is incomplete
    assert_eq!(nd2_plane(&t, 2, 0, 0, 1), Err("corrupt_file".into()));
    assert_eq!(nd2_plane(&t, 2, 1, 0, 1), Err("corrupt_file".into()));
    // frame 29 (t=1, position 1, z=4) is complete and identical
    assert_eq!(nd2_plane(&t, 1, 1, 4, 1), nd2_plane(&p, 1, 1, 4, 1));
    // frames never written (t=1, position 3, z=4 is frame 39 of 31 written) are errors too
    assert_eq!(nd2_plane(&t, 3, 0, 4, 1), Err("corrupt_file".into()));
    let out = bin()
        .args(["check", t.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn nd2_legacy_jpeg2000_reads_and_detects_truncation() {
    let Some(p) = corpus("aics-ND2-aryeh-but3-cont200-1.nd2") else {
        return;
    };
    let out = bin()
        .args(["info", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json(&out);
    let images = v["data"]["images"].as_array().unwrap();
    assert_eq!(images.len(), 5, "five XY positions");
    assert_eq!(images[0]["size_c"], 2);
    assert_eq!(images[0]["pixel_type"], "uint16");
    assert_eq!(images[0]["channels"][0]["name"], "20phase");
    let out = bin()
        .args(["check", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", json(&out));
    assert!(nd2_plane(&p, 4, 1, 0, 0).is_ok());
    let t = truncated_copy(&p);
    let out = bin()
        .args(["check", t.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4), "{}", json(&out));
}

/// `acquired_at` is ISO-8601 (from the Julian day, UTC with `Z`; or the unambiguous local-time
/// text, no offset) and never the free text, which stays in `extra.acquired_at_text`.
#[test]
fn nd2_acquired_at_is_iso8601() {
    let iso = |s: &str| {
        let b = s.as_bytes();
        s.len() >= 19
            && b[4] == b'-'
            && b[7] == b'-'
            && b[10] == b'T'
            && b[13] == b':'
            && b[16] == b':'
            && s[..4].chars().all(|c| c.is_ascii_digit())
    };
    let mut seen = 0;
    for (name, want, source) in [
        (
            "aics-ND2-dims-c2y32x32.nd2",
            Some("2021-09-28T13:34:47.695Z"),
            Some("julian_day_utc"),
        ),
        (
            "ome-karl-sample-image.nd2",
            Some("2017-06-06T09:15:06.980Z"),
            Some("julian_day_utc"),
        ),
        // uninitialized Julian day and an ambiguous `11/3/2009` text: no date rather than a guess
        ("ome-jonas-control002.nd2", None, None),
        (
            "aics-ND2-aryeh-but3-cont200-1.nd2",
            Some("2007-03-08T10:08:51.421Z"),
            Some("julian_day_utc"),
        ),
        (
            "aics-ND2-maxime-BF007.nd2",
            Some("2017-07-19T20:44:22.872Z"),
            Some("julian_day_utc"),
        ),
    ] {
        let Some(p) = corpus(name) else { continue };
        seen += 1;
        let out = bin()
            .args(["info", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        let v = json(&out);
        let img = &v["data"]["images"][0];
        let got = img["acquired_at"].as_str();
        assert_eq!(got, want, "{name}");
        if let Some(g) = got {
            assert!(iso(g), "{name}: {g}");
        }
        assert_eq!(
            img["extra"]["acquired_at_source"].as_str(),
            source,
            "{name}"
        );
    }
    if seen > 0 {
        let p = corpus("aics-ND2-dims-c2y32x32.nd2").unwrap();
        let out = bin()
            .args(["info", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        let v = json(&out);
        assert_eq!(
            v["data"]["images"][0]["extra"]["acquired_at_text"],
            "9/28/2021  9:34:47 AM"
        );
    }
}

#[test]
fn nd2_dump_embeds_frame_records() {
    let Some(p) = corpus("aics-ND2-dims-p2z5t3-2c4y32x32.nd2") else {
        return;
    };
    let out = bin()
        .args(["info", "--view", "full", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json(&out);
    let img = &v["data"]["file"]["images"][1];
    let frames = img["extra"]["frames"].as_array().unwrap();
    assert_eq!(img["extra"]["frame_records_total"], 25);
    assert_eq!(frames.len(), 25);
    // T(5: NE periods 3 + 2) x P(2) x Z(5): position 1, t=0, z=0 is frame 5
    assert_eq!(frames[0]["frame"], 5);
    assert_eq!(frames[0]["period"], 0);
    assert_eq!(frames[24]["period"], 1);
    assert!(frames[0]["time_ms"].as_f64().unwrap() > 0.0);
    assert!(
        frames[0]["acquired_at"]
            .as_str()
            .unwrap()
            .starts_with("2021-09-28T")
    );
    assert_eq!(img["extra"]["time_periods"].as_array().unwrap().len(), 2);
    assert_eq!(img["channels"][0]["emission_nm"], 535.0);
    assert_eq!(img["channels"][0]["color"], "#5BFF00");
    assert!(
        img["acquired_at"]
            .as_str()
            .unwrap()
            .starts_with("2021-09-28T13:55")
    );
}

#[test]
fn export_roundtrip_matches_planes() {
    let Some(p) = corpus("aics-ND2-dims-t3c2y32x32.nd2") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("openreadout-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("t.ome.tiff");
    let out = bin()
        .args([
            "export",
            p.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
            "--overwrite",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["data"]["verified"], true);
    assert_eq!(v["data"]["planes_written"], 6);
    // refusing to overwrite without the flag is a usage error
    let out = bin()
        .args([
            "export",
            p.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_nd2_writes_plane_elements() {
    let Some(p) = corpus("aics-ND2-dims-p1z5t3c2y32x32.nd2") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("openreadout-planes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("p.ome.tiff");
    let out = bin()
        .args([
            "export",
            p.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
            "--overwrite",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let xml = embedded_ome_xml(&out_path);
    let planes: Vec<&str> = xml
        .match_indices("<Plane ")
        .map(|(i, _)| &xml[i..i + xml[i..].find("/>").unwrap()])
        .collect();
    // z5 × t3 × c2, one Plane per written plane, every one timed, exposed and positioned.
    assert_eq!(planes.len(), 30, "{xml}");
    let mut seen = std::collections::BTreeSet::new();
    let mut last_t_delta = [f64::MIN; 3];
    for pl in &planes {
        let (c, z, t) = (
            xml_attr(pl, "TheC").unwrap(),
            xml_attr(pl, "TheZ").unwrap(),
            xml_attr(pl, "TheT").unwrap(),
        );
        seen.insert((c.to_string(), z.to_string(), t.to_string()));
        let dt: f64 = xml_attr(pl, "DeltaT").unwrap().parse().unwrap();
        let t: usize = t.parse().unwrap();
        assert!((0.0..3600.0).contains(&dt), "{pl}");
        last_t_delta[t] = last_t_delta[t].max(dt);
        assert_eq!(xml_attr(pl, "DeltaTUnit"), Some("s"));
        assert!(xml_attr(pl, "ExposureTime").is_some(), "{pl}");
        for k in ["PositionX", "PositionY", "PositionZ"] {
            assert!(xml_attr(pl, k).unwrap().parse::<f64>().is_ok(), "{pl}");
        }
        assert_eq!(xml_attr(pl, "PositionZUnit"), Some("nm"));
    }
    assert_eq!(seen.len(), 30, "every (c, z, t) exactly once");
    assert!(last_t_delta[0] < last_t_delta[1] && last_t_delta[1] < last_t_delta[2]);
    // Planes follow TiffData inside Pixels; StageLabel precedes Pixels.
    assert!(xml.find("<Plane ").unwrap() > xml.rfind("<TiffData").unwrap());
    assert!(xml.find("<StageLabel").unwrap() < xml.find("<Pixels").unwrap());
    // Selecting every other Z slice doubles PhysicalSizeZ.
    let out = bin()
        .args([
            "export",
            p.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
            "--overwrite",
            "--select",
            "z=0,2,4",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let sel = embedded_ome_xml(&out_path);
    let full_z: f64 = xml_attr(&xml, "PhysicalSizeZ").unwrap().parse().unwrap();
    let sel_z: f64 = xml_attr(&sel, "PhysicalSizeZ").unwrap().parse().unwrap();
    assert!((sel_z - 2.0 * full_z).abs() < 1e-9, "{full_z} {sel_z}");
    assert_eq!(sel.matches("<Plane ").count(), 18);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn explain_nd2_czi_lif_fcs() {
    if let Some(p) = corpus("ome-karl-sample-image.nd2") {
        let d = explain_json(&p);
        let t = all_text(&d);
        assert!(
            d["summary"]
                .as_str()
                .unwrap()
                .starts_with("A Nikon ND2 microscopy file"),
            "{t}"
        );
        assert!(
            t.contains("5-channel fluorescence z-stack (21 slices) of 1019 × 1019 pixels"),
            "{t}"
        );
        assert!(
            t.contains("21 focal planes 5 µm apart (a depth of 100 µm)"),
            "{t}"
        );
        assert!(
            t.contains("Plan Apo λ 20x objective (20× magnification, numerical aperture 0.75)"),
            "{t}"
        );
        assert!(
            t.contains("Acquisition started 2017-06-06 09:15:06 (UTC)"),
            "{t}"
        );
        assert!(t.contains("a record for each of its 21 frames"), "{t}");
        let cmds: Vec<&str> = d["suggested_commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap())
            .collect();
        assert!(cmds.iter().any(|c| c.contains("--select z=10")), "{cmds:?}");
        assert!(
            cmds.iter().any(|c| c.contains("--max-frames -1")),
            "{cmds:?}"
        );
        // The human form carries the same narrative.
        let out = bin()
            .args(["info", "--view", "explain", p.to_str().unwrap()])
            .output()
            .unwrap();
        let human = String::from_utf8_lossy(&out.stdout);
        assert!(
            human.contains("Next steps:") && human.contains("5-channel fluorescence"),
            "{human}"
        );
    }
    if let Some(p) = corpus("openslide-zeiss-5-jxr.czi") {
        let t = all_text(&explain_json(&p));
        assert!(t.contains("colour (RGB) image"), "{t}");
        assert!(t.contains("mosaic of 27 tiles"), "{t}");
        assert!(t.contains("4 lower-resolution copies"), "{t}");
        assert!(t.contains("Axioscan 7"), "{t}");
    }
    if let Some(p) = corpus("zenodo6606445-Project007.lif") {
        let t = all_text(&explain_json(&p));
        assert!(t.contains("holding 4 images"), "{t}");
        assert!(t.contains("glycerol immersion"), "{t}");
        assert!(
            t.contains("mosaic of 4 tiles of 512 × 512 pixels, stitched"),
            "{t}"
        );
    }
    if let Some(p) = corpus("fcsparser-facs-diva.fcs") {
        let d = explain_json(&p);
        let t = all_text(&d);
        assert!(t.contains("83,411 events"), "{t}");
        assert!(t.contains("FITC-A (CD20)"), "{t}");
        assert!(
            t.contains("An 8 × 8 compensation (spillover) matrix is stored"),
            "{t}"
        );
        let caveats = d["caveats"].to_string();
        assert!(caveats.contains("no time zone"), "{caveats}");
        // The analysis command leads (explain's next steps); the CSV export is still offered.
        assert!(
            d["suggested_commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c.as_str().unwrap().contains("--format csv")),
            "{}",
            d["suggested_commands"]
        );
    }
}

#[test]
fn explain_unknown_file_is_an_error_envelope() {
    let dir = std::env::temp_dir().join(format!("openreadout-explain-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("notes.txt");
    std::fs::write(&p, b"hello").unwrap();
    let out = bin()
        .args(["info", "--view", "explain", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json(&out)["ok"], false);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_ome_zarr_nd2_multiposition_rgb() {
    let Some(p) = corpus("aics-ND2-dims-rgb-t3p2c2z3x64y64.nd2") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("openreadout-zarr-nd2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("t.ome.zarr");
    let out = export_zarr(&p, &out_path, &["--overwrite"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = json(&out);
    assert_eq!(v["data"]["format"], "ome-zarr");
    assert_eq!(v["data"]["verified"], true);
    assert_eq!(v["data"]["images_written"], 2);
    assert_eq!(v["data"]["planes_written"], 36);
    assert_eq!(v["data"]["codec"], "deflate");
    // two positions -> bioformats2raw collection
    let root = read_json(&out_path.join("zarr.json"));
    assert_eq!(root["attributes"]["ome"]["version"], "0.5");
    assert_eq!(root["attributes"]["ome"]["bioformats2raw.layout"], 3);
    let series = read_json(&out_path.join("OME/zarr.json"));
    assert_eq!(
        series["attributes"]["ome"]["series"],
        serde_json::json!(["0", "1"])
    );
    assert!(out_path.join("OME/METADATA.ome.xml").is_file());
    // RGB samples become channels: t=3, c=2*3, z=3
    let a = read_json(&out_path.join("1/0/zarr.json"));
    assert_eq!(a["shape"], serde_json::json!([3, 6, 3, 32, 32]));
    assert_eq!(
        a["dimension_names"],
        serde_json::json!(["t", "c", "z", "y", "x"])
    );
    let g = read_json(&out_path.join("0/zarr.json"));
    let ms = &g["attributes"]["ome"]["multiscales"][0];
    assert_eq!(ms["axes"][4]["unit"], "micrometer");
    assert_eq!(
        g["attributes"]["ome"]["omero"]["channels"]
            .as_array()
            .unwrap()
            .len(),
        6
    );
    // an existing store is not replaced without --overwrite
    let out = export_zarr(&p, &out_path, &[]);
    assert_eq!(out.status.code(), Some(2));
    // lzw is an OME-TIFF-only codec
    let out = export_zarr(&p, &dir.join("lzw.ome.zarr"), &["--compression", "lzw"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!dir.join("lzw.ome.zarr").exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_ome_zarr_czi_scene_selection_and_pyramid() {
    let Some(p) = corpus("aics-s-3-t-1-c-3-z-5.czi") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("openreadout-zarr-czi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("scene1.ome.zarr");
    let out = export_zarr(
        &p,
        &out_path,
        &[
            "--overwrite",
            "--image",
            "1",
            "--select",
            "c=0,2",
            "--select",
            "z=1-2",
            "--levels",
            "2",
            "--chunk-size",
            "128",
            "--compression",
            "none",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = json(&out);
    assert_eq!(v["data"]["verified"], true);
    assert_eq!(v["data"]["images_written"], 1);
    assert_eq!(v["data"]["planes_written"], 4);
    assert_eq!(v["data"]["codec"], "none");
    // a single image is written at the root
    let root = read_json(&out_path.join("zarr.json"));
    let ome = &root["attributes"]["ome"];
    assert!(ome.get("bioformats2raw.layout").is_none());
    assert_eq!(
        ome["multiscales"][0]["datasets"].as_array().unwrap().len(),
        2
    );
    let labels: Vec<&str> = ome["omero"]["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["EGFP", "Bright"]);
    let a0 = read_json(&out_path.join("0/zarr.json"));
    assert_eq!(a0["shape"], serde_json::json!([1, 2, 2, 325, 475]));
    assert_eq!(
        a0["chunk_grid"]["configuration"]["chunk_shape"],
        serde_json::json!([1, 1, 1, 128, 128])
    );
    let a1 = read_json(&out_path.join("1/zarr.json"));
    assert_eq!(a1["shape"], serde_json::json!([1, 2, 2, 163, 238]));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn export_ome_zarr_lif_multi_image() {
    let Some(p) = corpus("ome-michael-PR2729-frameOrderCombinedScanTypes.lif") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("openreadout-zarr-lif-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("l.ome.zarr");
    let out = export_zarr(&p, &out_path, &["--overwrite"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = json(&out);
    assert_eq!(v["data"]["verified"], true);
    // the file's 2x2 tile scan is one stitched 97x97 image (it used to be four 64x64 images)
    assert_eq!(v["data"]["images_written"], 1);
    assert_eq!(v["data"]["planes_written"], 12);
    let a = read_json(&out_path.join("0/zarr.json"));
    assert_eq!(a["shape"], serde_json::json!([2, 2, 3, 97, 97]));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn lif_stitched_tile_scan_export_roundtrip() {
    // Synthetic 3-tile scan (FlipX) stitched to 14x5; listed per tile; exported, read back, verified.
    let p = lif_fixture("synthetic-dims.lif");
    let out = bin()
        .args(["info", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let v = json(&out);
    let tiles = &v["data"]["images"][5];
    assert_eq!(
        (tiles["size_x"].as_u64(), tiles["size_y"].as_u64()),
        (Some(14), Some(5))
    );
    assert_eq!(tiles["extra"]["tiles"][0]["x_px"], 8);
    let out = bin()
        .args(["info", "--view", "structure", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let ls = json(&out);
    let tile_entries = ls["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "tile")
        .count();
    assert_eq!(tile_entries, 3, "per-tile access is listed by ls");
    // export only the tile scan (image 5): 2 Z planes of the stitched 14x5 mosaic
    let dir = std::env::temp_dir().join(format!("openreadout-lifexp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("tiles.ome.tiff");
    let out = bin()
        .args([
            "export",
            p.to_str().unwrap(),
            "--image",
            "5",
            "-o",
            out_path.to_str().unwrap(),
            "--overwrite",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = json(&out);
    assert_eq!(v["data"]["verified"], true);
    assert_eq!(v["data"]["planes_written"], 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn lif_corpus_mosaic_matches_lasx_merge_and_exports() {
    // Real tile scan: our stitched aics-tiled planes equal LAS X's own merge (aics-merged-tiles).
    let hashes = |p: &PathBuf| -> Vec<String> {
        let out = bin()
            .args(["planes", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        json(&out)["data"]["planes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["xxh3"].as_str().unwrap().to_string())
            .collect()
    };
    if let (Some(tiled), Some(merged)) = (corpus("aics-tiled.lif"), corpus("aics-merged-tiles.lif"))
    {
        assert_eq!(hashes(&tiled), hashes(&merged));
    }
    let Some(small) = corpus("ome-michael-PR2729-frameOrderCombinedScanTypes.lif") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("openreadout-lifmic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out_path = dir.join("michael.ome.tiff");
    let out = bin()
        .args([
            "export",
            small.to_str().unwrap(),
            "-o",
            out_path.to_str().unwrap(),
            "--overwrite",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["data"]["verified"], true);
    assert_eq!(v["data"]["planes_written"], 12);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn lif_flim_is_unsupported_exit_6_and_containers_read() {
    let p = lif_fixture("synthetic-dims.lif");
    let out = bin()
        .args(["planes", p.to_str().unwrap(), "--image", "7", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    let v = json(&out);
    assert_eq!(v["error"]["code"], "unsupported_feature");
    assert!(v["error"]["hint"].as_str().unwrap().contains("Fast Flim"));
    // XLEF experiment folder (synthetic) reads through its XLIF/LOF references
    let x = lif_fixture("xlef/Experiment.xlef");
    let out = bin()
        .args(["info", "--view", "format", x.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(json(&out)["data"]["confidence"], "definite");
    let out = bin()
        .args(["planes", x.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(json(&out)["data"]["planes"].as_array().unwrap().len(), 4);
    let out = bin()
        .args(["check", x.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn czi_ls_lists_attachments_and_extract_writes_them() {
    let Some(p) = corpus("zenodo7015307-T-3-CH-2.czi") else {
        return;
    };
    let out = bin()
        .args(["info", "--view", "structure", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json(&out);
    let atts: Vec<&serde_json::Value> = v["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "attachment")
        .collect();
    let names: Vec<&str> = atts.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Thumbnail (JPG)"), "{names:?}");
    assert!(names.contains(&"TimeStamps (CZTIMS)"), "{names:?}");
    let dir = tmp_dir("extract");
    let dst = dir.join("thumb.jpg");
    let out = bin()
        .args([
            "export",
            p.to_str().unwrap(),
            "--attachment",
            "thumbnail",
            "-o",
            dst.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = json(&out);
    assert_eq!(v["data"]["verified"], true);
    assert_eq!(v["data"]["attachment"]["content_type"], "JPG");
    let bytes = std::fs::read(&dst).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xD8], "a JPEG starts with SOI");
    assert_eq!(v["data"]["bytes_written"], bytes.len());
    // refusing to overwrite, and unknown names, are usage errors listing what exists
    let again = bin()
        .args([
            "export",
            p.to_str().unwrap(),
            "--attachment",
            "Thumbnail",
            "-o",
            dst.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(2));
    let bad = bin()
        .args([
            "export",
            p.to_str().unwrap(),
            "--attachment",
            "Nope",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(
        json(&bad)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("TimeStamps")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn czi_dump_has_per_plane_frames_and_info_has_time_stamps() {
    let Some(p) = corpus("zenodo7015307-T-3-CH-2.czi") else {
        return;
    };
    let out = bin()
        .args(["info", "--view", "full", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json(&out);
    let extra = &v["data"]["file"]["images"][0]["extra"];
    let frames = extra["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 6);
    assert_eq!(extra["frame_records_total"], 6);
    // ordered t, then z, then c: records 0, 2, 4 are channel 0 at t = 0, 1, 2
    let t: Vec<f64> = frames
        .iter()
        .filter(|f| f["c"] == 0)
        .map(|f| f["time_ms"].as_f64().unwrap())
        .collect();
    assert!(t[0].abs() < 1e-9, "{t:?}");
    assert!(t[1] > 300.0 && t[2] > t[1], "{t:?}");
    assert!(
        frames[0]["acquired_at"]
            .as_str()
            .unwrap()
            .starts_with("2022-08-22T")
    );
    assert_eq!(frames[0]["stage_x_um"], 49500.0);
    assert_eq!(frames[0]["exposure_ms"], 150.0);
    assert_eq!(extra["time_stamps_s"].as_array().unwrap().len(), 3);
    assert_eq!(extra["events"][0]["description"], "IncubationRecording");
    assert_eq!(extra["scene"]["well"]["name"], "D6");
    // info stays header-only: no frame records there
    let out = bin()
        .args(["info", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        json(&out)["data"]["images"][0]["extra"]
            .get("frames")
            .is_none()
    );
}

#[test]
fn czi_planes_level_reads_pyramid() {
    let Some(p) = corpus("zenodo7015307-S-2-2x2-CH-1.czi") else {
        return;
    };
    let info = json(
        &bin()
            .args(["info", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap(),
    );
    let im = &info["data"]["images"][0];
    assert_eq!(im["pyramid_levels"], 2);
    let lvl = &im["extra"]["pyramid"][0];
    let out = bin()
        .args([
            "planes",
            p.to_str().unwrap(),
            "--image",
            "0",
            "--level",
            "1",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json(&out);
    let pl = &v["data"]["planes"][0];
    assert_eq!(pl["level"], 1);
    assert_eq!(pl["width"], lvl["size_x"]);
    assert_eq!(pl["height"], lvl["size_y"]);
    // level 0 is the default and omits the field
    let v0 = json(
        &bin()
            .args(["planes", p.to_str().unwrap(), "--image", "0", "--json"])
            .output()
            .unwrap(),
    );
    assert!(v0["data"]["planes"][0].get("level").is_none());
    let out = bin()
        .args(["planes", p.to_str().unwrap(), "--level", "7"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn czi_multifile_parts_are_followed_and_missing_parts_reported() {
    let (Some(master), Some(p1), Some(p2)) = (
        corpus("synthetic-multifile.czi"),
        corpus("synthetic-multifile (1).czi"),
        corpus("synthetic-multifile (2).czi"),
    ) else {
        return;
    };
    let out = bin()
        .args(["planes", master.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(json(&out)["data"]["planes"].as_array().unwrap().len(), 3);
    // opening a following part points at the master
    let out = bin()
        .args(["info", p1.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert!(
        json(&out)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("synthetic-multifile.czi")
    );
    // a copy without part 2: check fails with missing_part, reading T=2 is a clean error
    let dir = tmp_dir("multifile");
    std::fs::copy(&master, dir.join("synthetic-multifile.czi")).unwrap();
    std::fs::copy(&p1, dir.join("synthetic-multifile (1).czi")).unwrap();
    let _ = p2;
    let m = dir.join("synthetic-multifile.czi");
    let out = bin()
        .args(["check", m.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    let codes: Vec<String> = json(&out)["data"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["code"].as_str().unwrap().to_string())
        .collect();
    assert!(codes.iter().any(|c| c == "missing_part"), "{codes:?}");
    let out = bin()
        .args(["planes", m.to_str().unwrap(), "--select", "t=0-1"])
        .output()
        .unwrap();
    assert!(out.status.success());
    // the missing part is a missing file (io, exit 5) naming it, not a corrupt master
    let out = bin()
        .args(["planes", m.to_str().unwrap(), "--select", "t=2", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let v = json(&out);
    assert_eq!(v["error"]["code"], "io");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("synthetic-multifile (2).czi"),
        "{v}"
    );
    // info still works and says a part is missing
    let out = bin()
        .args(["info", m.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let notes = json(&out)["data"]["notes"].to_string();
    assert!(notes.contains("missing"), "{notes}");
    std::fs::remove_dir_all(&dir).ok();
}

/// Line scans pack a whole Z or T stack into one subblock (Z size 25, or one subblock per T
/// with Y = 1); `check` must count the planes a subblock's ranges cover, not its start index.
#[test]
fn czi_check_counts_planes_packed_in_one_subblock() {
    for name in [
        "zenodo10577621-LineScan-T3500.czi",
        "zenodo10577621-LineScan-Z200.czi",
        "zenodo10577621-LineScan-T80-Z25.czi",
        "zenodo10577621-Channel-ZStack-LineScan-Bidirectional-Averaging.czi",
    ] {
        let Some(p) = corpus(name) else {
            continue;
        };
        let out = bin()
            .args(["check", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        let v = json(&out);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{name}: {}",
            v["data"]["findings"]
        );
        assert!(
            !v["data"]["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|f| f["code"] == "missing_planes"),
            "{name}"
        );
    }
}

#[test]
fn czi_jpeg_and_resolution_protocol_fixtures() {
    if let Some(p) = corpus("synthetic-gray8-jpeg.czi") {
        let out = bin()
            .args(["planes", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(json(&out)["data"]["planes"].as_array().unwrap().len(), 12);
    }
    if let Some(p) = corpus("synthetic-gray16-jpeg12.czi") {
        let out = bin()
            .args(["planes", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(6), "12-bit DCT JPEG is unsupported");
    }
    if let Some(p) = corpus("synthetic-jxr-mismatch-bgr48-as-bgr24.czi") {
        let out = bin()
            .args(["check", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0));
        let v = json(&out);
        let f = v["data"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["code"] == "resolution_protocol")
            .expect("resolution_protocol warning");
        assert_eq!(f["severity"], "warning");
        let planes = json(
            &bin()
                .args(["planes", p.to_str().unwrap(), "--json"])
                .output()
                .unwrap(),
        );
        assert_eq!(planes["data"]["planes"][0]["pixel_type"], "uint8");
    }
}

#[test]
fn ome_tiff_roundtrip_czi() {
    if let Some(p) = corpus("zenodo7015307-S-2-T-3-Z-5-CH-1.czi") {
        assert_ome_tiff_roundtrip(&p);
    }
}

#[test]
fn ome_tiff_roundtrip_nd2_rgb_multiposition() {
    if let Some(p) = corpus("aics-ND2-dims-rgb-t3p2c2z3x64y64.nd2") {
        assert_ome_tiff_roundtrip(&p);
    }
}

#[test]
fn ome_tiff_roundtrip_lif() {
    if let Some(p) = corpus("ome-michael-PR2729-frameOrderCombinedScanTypes.lif") {
        assert_ome_tiff_roundtrip(&p);
    }
}

#[test]
fn tiff_truncation_is_detected() {
    // An export we make ourselves (no corpus needed beyond one source file), then a corpus TIFF.
    let mut sources = Vec::new();
    if let Some(src) = corpus("aics-ND2-dims-t3c2y32x32.nd2") {
        let dir = std::env::temp_dir().join(format!("openreadout-ttrunc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out_path = dir.join("full.ome.tiff");
        let out = bin()
            .args([
                "export",
                src.to_str().unwrap(),
                "-o",
                out_path.to_str().unwrap(),
                "--overwrite",
                "--compression",
                "none",
            ])
            .output()
            .unwrap();
        assert!(out.status.success());
        sources.push(out_path);
    }
    if let Some(p) = corpus("ome-artificial-multi-channel-z-series.ome.tiff") {
        sources.push(p);
    }
    for src in sources {
        let out = bin()
            .args(["check", src.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "intact {} must pass: {}",
            src.display(),
            String::from_utf8_lossy(&out.stdout)
        );
        let t = truncated_copy(&src);
        let out = bin()
            .args(["check", t.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(4),
            "truncated TIFF must fail check: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let v = json(&out);
        let codes: Vec<String> = v["data"]["findings"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|f| f["code"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            v["ok"] == false
                || codes
                    .iter()
                    .any(|c| c == "truncated" || c == "missing_planes" || c == "bad_ifd_offset"),
            "{codes:?}"
        );
        // reading a plane that is gone is a clean corrupt-file error, not a panic
        let out = bin()
            .args(["planes", t.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(json(&out)["error"]["code"], "corrupt_file");
    }
}

#[test]
fn ome_tiff_multifile_set_resolves_siblings() {
    let Some(p) = corpus("tubhiswt-2D/tubhiswt_C0.ome.tif") else {
        return;
    };
    let out = bin()
        .args(["info", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let v = json(&out);
    let im = &v["data"]["images"][0];
    assert_eq!(im["size_c"], 2, "both channel files are part of the set");
    assert_eq!(im["extra"]["files"].as_array().unwrap().len(), 2);
    let out = bin()
        .args(["check", p.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Corpus files whose OME-TIFF export `compare` finds identical, metadata included (default
/// ignores only): a Leica λ scan (detection bands, confocal mode, objective, LAS X version), a
/// LAS AF LIF, a multi-scene CZI (7-digit timestamps, ZEN version) and an ND2 (brightfield
/// channels, NIS-Elements, no image name). Regression for the 2026-09-25 round-trip audit.
#[test]
fn ome_tiff_round_trip_keeps_metadata_corpus() {
    let tmp = tempfile::tempdir().unwrap();
    for name in [
        "zenodo14976703-Convalaria-LambdaScan.lif",
        "bsst749-4ii-bodipy-ctl.lif",
        "aics-s-3-t-1-c-3-z-5.czi",
        "ome-aryeh-b16-14-12.nd2",
        "aics-ND2-dims-t3c2y32x32.nd2",
    ] {
        let Some(p) = corpus(name) else { continue };
        let ome = tmp.path().join(format!("{name}.ome.tiff"));
        let out = bin()
            .arg("export")
            .arg(&p)
            .arg("-o")
            .arg(&ome)
            .output()
            .unwrap();
        assert!(out.status.success(), "{name}: {}", stdout(&out));
        let out = bin()
            .args(["compare", "--json"])
            .arg(&p)
            .arg(&ome)
            .output()
            .unwrap();
        let v = json(&out);
        assert_eq!(
            v["data"]["metadata"]["difference_count"], 0,
            "{name}: {:#}",
            v["data"]["metadata"]["differences"]
        );
        assert_eq!(v["data"]["identical"], true, "{name}");
        assert_eq!(out.status.code(), Some(0), "{name}");
    }
}

/// The fixtures' OME-Zarr exports compare identical, metadata included: the objective,
/// instrument and acquisition mode travel in `OME/METADATA.ome.xml`, and neither a name nor a
/// display colour is invented. An export made with `--select` compares identical with
/// `compare --select` (same selection). Regression for the 2026-10 website audit.
#[test]
fn ome_zarr_round_trip_and_selected_exports_compare_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let check = |src: &Path, other: &Path, select: &[&str]| {
        let mut cmd = bin();
        cmd.args(["compare", "--json"]).arg(src).arg(other);
        for s in select {
            cmd.args(["--select", s]);
        }
        let out = cmd.output().unwrap();
        let v = json(&out);
        assert_eq!(
            v["data"]["metadata"]["difference_count"],
            0,
            "{}: {:#}",
            other.display(),
            v["data"]["metadata"]["differences"]
        );
        assert_eq!(v["data"]["identical"], true, "{}", other.display());
        assert_eq!(out.status.code(), Some(0));
        v
    };
    for name in ["mini.nd2", "mini.lif", "mini.czi"] {
        let src = fixture(name);
        let zarr = tmp.path().join(format!("{name}.ome.zarr"));
        let out = bin()
            .arg("export")
            .arg(&src)
            .args(["--format", "ome-zarr", "-o"])
            .arg(&zarr)
            .output()
            .unwrap();
        assert!(out.status.success(), "{name}: {}", stderr(&out));
        assert!(zarr.join("OME").join("METADATA.ome.xml").is_file());
        check(&src, &zarr, &[]);
    }
    // unnamed channels stay unnamed (the writer's `omero` label is for display only)
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../openreadout-zarr/tests/fixtures/ngff05-v3-yx-zstd-stored.zip");
    let zarr = tmp.path().join("unnamed.ome.zarr");
    let out = bin()
        .arg("export")
        .arg(&src)
        .args(["--format", "ome-zarr", "-o"])
        .arg(&zarr)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let out = bin()
        .args(["compare", "--json"])
        .arg(&src)
        .arg(&zarr)
        .output()
        .unwrap();
    let v = json(&out);
    assert_eq!(v["data"]["images"][0]["channel_names_equal"], true, "{v:#}");

    let src = fixture("mini.lif");
    for to in ["ome-zarr", "ome-tiff"] {
        let out_path = tmp.path().join(format!("z1.{to}"));
        let out = bin()
            .arg("export")
            .arg(&src)
            .args(["--format", to, "--select", "z=1", "-o"])
            .arg(&out_path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{to}: {}", stderr(&out));
        let v = check(&src, &out_path, &["z=1"]);
        assert_eq!(v["data"]["images"][0]["selected"], true);
        assert_eq!(v["data"]["planes"]["planes"], 1);
        // without the selection the export is (correctly) a different file
        let out = bin()
            .args(["compare", "--json"])
            .arg(&src)
            .arg(&out_path)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1));
    }
}

#[test]
fn ome_zarr_input_zip_store_and_export_round_trips() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../openreadout-zarr/tests/fixtures/ngff04-v2-tczyx-blosc-lz4.zip");
    let out = bin()
        .args(["info", src.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let v = json(&out);
    assert_eq!(v["data"]["format"]["id"], "ome-zarr");
    assert_eq!(v["data"]["images"][0]["size_c"], 2);
    let all = plane_hashes(&src);
    // the label image `labels/cells` is exposed after the image (image 1, 6 planes)
    assert_eq!(all.len(), 18);
    assert_eq!(v["data"]["images"][1]["extra"]["label"], true);
    let image0 = |v: Vec<(u64, u64, u64, u64, String)>| -> Vec<_> {
        v.into_iter().filter(|p| p.0 == 0).collect()
    };
    let original = image0(all);
    assert_eq!(original.len(), 12);
    // OME-Zarr in, OME-Zarr out, OME-Zarr in again: every plane identical
    let dir = tempfile::tempdir().unwrap();
    let dst = dir.path().join("again.ome.zarr");
    let out = bin()
        .args([
            "export",
            src.to_str().unwrap(),
            "--format",
            "ome-zarr",
            "-o",
            dst.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(image0(plane_hashes(&dst)), original);
    // and through OME-TIFF
    let tif = dir.path().join("again.ome.tiff");
    let out = bin()
        .args(["export", src.to_str().unwrap(), "-o", tif.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(image0(plane_hashes(&tif)), original);
    // `ls` lists the label image
    let out = bin()
        .args([
            "info",
            "--view",
            "structure",
            src.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(json(&out).to_string().contains("labels/cells"));
}
