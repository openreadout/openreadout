//! Edge cases: every one a clean exit code, never a panic.

use std::path::PathBuf;

use crate::common::*;

/// A fresh scratch directory unique to this test.
fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("openreadout-edge-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn fixtures_are_readable() {
    for name in ["mini.czi", "mini.nd2", "mini.lif"] {
        let p = fixture(name);
        let out = run(&["check", "--planes", p.to_str().unwrap(), "--json"]);
        assert_eq!(out.status.code(), Some(0), "{name}");
        let out = run(&["check", p.to_str().unwrap(), "--json"]);
        assert_eq!(out.status.code(), Some(0), "{name}");
    }
}

#[test]
fn preview_rulers_hint_and_plain() {
    let d = scratch("preview-rulers");
    let czi = fixture("mini.czi");
    let png = d.join("look.png");
    let out = bin()
        .args(["preview", "--json", "-o"])
        .arg(&png)
        .arg(&czi)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let v = json(&out);
    let im = &v["data"]["image"];
    assert_eq!(im["axes"], true);
    assert_eq!(im["full_res_region"]["width"], 8);
    assert!(im["plot_area"]["x"].as_u64().unwrap() > 0);
    let hint = v["data"]["hint"].as_str().unwrap();
    assert!(
        hint.contains("--region") && hint.contains("rulers"),
        "{hint}"
    );
    // human output ends with the same hint
    let out = bin()
        .args(["--color", "never", "preview", "--overwrite", "-o"])
        .arg(&png)
        .arg(&czi)
        .output()
        .unwrap();
    assert!(
        stdout(&out).contains("zoom with --region"),
        "{}",
        stdout(&out)
    );
    // --plain: the bare 8x8 plane
    let out = bin()
        .args(["preview", "--plain", "--json", "--overwrite", "-o"])
        .arg(&png)
        .arg(&czi)
        .output()
        .unwrap();
    let v = json(&out);
    assert_eq!(v["data"]["image"]["axes"], false);
    assert_eq!(
        (v["data"]["width"].as_u64(), v["data"]["height"].as_u64()),
        (Some(8), Some(8))
    );
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn zero_byte_file_is_corrupt_exit_4() {
    let d = scratch("zero");
    for ext in ["czi", "nd2", "lif", "bin"] {
        let p = d.join(format!("empty.{ext}"));
        std::fs::write(&p, b"").unwrap();
        for cmd in [&["info"][..], &["check"], &["info", "--view", "structure"]] {
            let out = run(&[cmd, &[p.to_str().unwrap(), "--json"]].concat());
            assert_eq!(out.status.code(), Some(4), "{cmd:?} empty.{ext}");
            let v = json(&out);
            assert_eq!(v["error"]["code"], "corrupt_file");
            assert!(v["error"]["message"].as_str().unwrap().contains("empty"));
        }
    }
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn directory_that_is_no_data_set_is_a_batch_of_its_files() {
    // Directories can be data sets (e.g. Bruker NMR experiments); one no reader claims is
    // read as a batch of its files (one envelope each), even when its name looks like a file.
    let d = scratch("dir");
    let fake = d.join("looks-like-a-file.czi");
    std::fs::create_dir_all(&fake).unwrap();
    for cmd in [&["info"][..], &["check"], &["info", "--view", "format"]] {
        let out = run(&[cmd, &[fake.to_str().unwrap(), "--json"]].concat());
        assert_eq!(out.status.code(), Some(0), "{cmd:?} on an empty directory");
        assert_eq!(json(&out), serde_json::json!([]), "{cmd:?}");
    }
    std::fs::write(fake.join("notes.txt"), b"plain text\n").unwrap();
    let out = run(&["info", fake.to_str().unwrap(), "--json"]);
    assert!(matches!(out.status.code(), Some(0 | 3)), "{:?}", out.status);
    let v = json(&out);
    assert_eq!(v[0]["error"]["code"], "unknown_format");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn wrong_extension_is_detected_by_signature() {
    let d = scratch("ext");
    for (src, wrong, fmt) in [
        ("mini.czi", "really-a-czi.nd2", "czi"),
        ("mini.nd2", "really-an-nd2.lif", "nd2"),
        ("mini.lif", "really-a-lif.tif", "lif"),
    ] {
        let p = d.join(wrong);
        std::fs::copy(fixture(src), &p).unwrap();
        let out = run(&["info", p.to_str().unwrap(), "--json"]);
        assert_eq!(out.status.code(), Some(0), "{wrong}");
        assert_eq!(json(&out)["data"]["format"]["id"], fmt);
    }
    // A non-instrument file with an instrument extension is an unknown format, not a crash.
    let p = d.join("notes.czi");
    std::fs::write(&p, b"plain text, not a CZI\n").unwrap();
    assert_eq!(run(&["info", p.to_str().unwrap()]).status.code(), Some(3));
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn unicode_path_roundtrips() {
    let d = scratch("unicode");
    let sub = d.join("Mikroskopie – データ");
    std::fs::create_dir_all(&sub).unwrap();
    let p = sub.join("Zelle-ß-🔬.czi");
    std::fs::copy(fixture("mini.czi"), &p).unwrap();
    let out = run(&["info", p.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        json(&out)["data"]["path"]
            .as_str()
            .unwrap()
            .ends_with("Zelle-ß-🔬.czi")
    );
    let out = run(&["check", "--planes", p.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(0));
    std::fs::remove_dir_all(&d).ok();
}

#[cfg(unix)]
#[test]
fn symlinks_are_followed_and_dangling_ones_are_io_errors() {
    let d = scratch("symlink");
    let target = d.join("real.nd2");
    std::fs::copy(fixture("mini.nd2"), &target).unwrap();
    let link = d.join("link.nd2");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let out = run(&["info", link.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json(&out)["data"]["format"]["id"], "nd2");
    assert_eq!(
        run(&["check", link.to_str().unwrap()]).status.code(),
        Some(0)
    );
    let dangling = d.join("dangling.nd2");
    std::os::unix::fs::symlink(d.join("gone.nd2"), &dangling).unwrap();
    let out = run(&["info", dangling.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(5));
    std::fs::remove_dir_all(&d).ok();
}

#[cfg(unix)]
#[test]
fn read_only_and_unreadable_files() {
    use std::os::unix::fs::PermissionsExt;
    let d = scratch("perm");
    let p = d.join("ro.lif");
    std::fs::copy(fixture("mini.lif"), &p).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444)).unwrap();
    for cmd in [
        &["info"][..],
        &["check"],
        &["info", "--view", "structure"],
        &["check", "--planes"],
    ] {
        assert_eq!(
            run(&[cmd, &[p.to_str().unwrap()]].concat()).status.code(),
            Some(0),
            "{cmd:?} on a read-only file"
        );
    }
    // Export of a read-only input writes elsewhere and never touches the source.
    let out_path = d.join("ro.ome.tiff");
    let out = run(&[
        "export",
        p.to_str().unwrap(),
        "-o",
        out_path.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(0));
    let q = d.join("noperm.lif");
    std::fs::copy(fixture("mini.lif"), &q).unwrap();
    std::fs::set_permissions(&q, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Root can read anything; only assert when the file really is unreadable.
    if std::fs::File::open(&q).is_err() {
        let out = run(&["info", q.to_str().unwrap(), "--json"]);
        assert_eq!(out.status.code(), Some(5));
        assert_eq!(json(&out)["error"]["code"], "io");
    }
    std::fs::set_permissions(&q, std::fs::Permissions::from_mode(0o644)).ok();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).ok();
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn czi_update_pending_is_a_warning() {
    let d = scratch("pending");
    let mut data = std::fs::read(fixture("mini.czi")).unwrap();
    // FileHeader.update_pending: u32 at segment payload offset 68 (absolute 32 + 68).
    data[100..104].copy_from_slice(&1u32.to_le_bytes());
    let p = d.join("writing.czi");
    std::fs::write(&p, &data).unwrap();
    let out = run(&["check", p.to_str().unwrap(), "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "readable, so check passes with a warning"
    );
    let v = json(&out);
    let f = v["data"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "unfinished_write")
        .expect("unfinished_write finding");
    assert_eq!(f["severity"], "warning");
    let out = run(&["info", p.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(
        json(&out)["data"]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().contains("update_pending"))
    );
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn czi_empty_and_garbage_metadata_xml() {
    let d = scratch("xml");
    let base = std::fs::read(fixture("mini.czi")).unwrap();
    // ZISRAWMETADATA payload starts at 544 + 32; its first u32 is the XML size.
    let mut empty = base.clone();
    empty[576..580].copy_from_slice(&0u32.to_le_bytes());
    let p = d.join("empty-xml.czi");
    std::fs::write(&p, &empty).unwrap();
    for cmd in [
        &["info"][..],
        &["check"],
        &["check", "--planes"],
        &["info", "--view", "full"],
    ] {
        let out = run(&[cmd, &[p.to_str().unwrap(), "--json"]].concat());
        assert_eq!(out.status.code(), Some(0), "{cmd:?} with empty XML");
    }
    assert!(
        json(&run(&[
            "info",
            "--view",
            "full",
            p.to_str().unwrap(),
            "--json"
        ]))["data"]["vendor"]
            .is_null()
    );
    // Same length, but not XML: pixels and structure still readable; the vendor dump is
    // reported as corrupt rather than crashing.
    let mut garbage = base;
    let xml_len = u32::from_le_bytes(garbage[576..580].try_into().unwrap()) as usize;
    let start = 544 + 32 + 256;
    garbage[start..start + xml_len].fill(b'<');
    let p = d.join("garbage-xml.czi");
    std::fs::write(&p, &garbage).unwrap();
    assert_eq!(run(&["info", p.to_str().unwrap()]).status.code(), Some(0));
    assert_eq!(
        run(&["check", "--planes", p.to_str().unwrap()])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        run(&["info", "--view", "full", p.to_str().unwrap()])
            .status
            .code(),
        Some(4)
    );
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn export_refuses_absurd_plane_counts() {
    // uiSequenceCount = 4e9 with a single frame chunk (crafted by fuzz/craft.py).
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../openreadout-nd2/tests/fixtures/malformed/file-crafted-huge-frame-count.nd2");
    let d = scratch("planecount");
    assert_eq!(run(&["info", p.to_str().unwrap()]).status.code(), Some(0));
    for to in ["ome-tiff", "ome-zarr"] {
        let out = run(&[
            "export",
            p.to_str().unwrap(),
            "--to",
            to,
            "-o",
            d.join(format!("x.{to}")).to_str().unwrap(),
            "--json",
        ]);
        assert_eq!(out.status.code(), Some(6), "{to}");
        assert_eq!(json(&out)["error"]["code"], "unsupported_feature");
    }
    std::fs::remove_dir_all(&d).ok();
}
