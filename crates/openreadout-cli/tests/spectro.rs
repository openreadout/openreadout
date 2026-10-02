//! Vibrational spectroscopy through the binary: a synthetic WiRE map (info, trace statistics at
//! the band position, CSV and JCAMP-DX export of one spectrum, OME-TIFF export and preview of the
//! map, a truncated file) and an OMNIC series refused with exit code 6.
#![allow(clippy::many_single_char_names)] // byte layouts built field by field

use std::path::Path;
use std::process::{Command, Output};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openreadout"))
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

fn run(args: &[&str], file: &Path) -> Output {
    bin().args(args).arg(file).output().unwrap()
}

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn block(name: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = name.to_vec();
    b.extend_from_slice(&0i32.to_le_bytes());
    b.extend_from_slice(&((body.len() + 16) as u64).to_le_bytes());
    b.extend_from_slice(body);
    b
}

/// A 2 × 2 WiRE map (layout in `docs/formats/renishaw-wdf.md`), 3 points per spectrum; spectrum
/// k holds 100k + p at point p, so the strongest band is the last point.
fn wdf_map() -> Vec<u8> {
    let (points, count) = (3u32, 4u64);
    let mut h = vec![0u8; 496];
    let o = |a: usize| a - 16;
    h[o(0x3c)..o(0x40)].copy_from_slice(&points.to_le_bytes());
    h[o(0x40)..o(0x48)].copy_from_slice(&count.to_le_bytes());
    h[o(0x48)..o(0x50)].copy_from_slice(&count.to_le_bytes());
    h[o(0x50)..o(0x54)].copy_from_slice(&2u32.to_le_bytes());
    h[o(0x58)..o(0x5c)].copy_from_slice(&points.to_le_bytes());
    h[o(0x5c)..o(0x60)].copy_from_slice(&2u32.to_le_bytes());
    h[o(0x60)..o(0x64)].copy_from_slice(b"WiRE");
    h[o(0x84)..o(0x88)].copy_from_slice(&3u32.to_le_bytes());
    h[o(0x98)..o(0x9c)].copy_from_slice(&6u32.to_le_bytes());
    h[o(0x9c)..o(0xa0)].copy_from_slice(&(1e7f32 / 785.0).to_le_bytes());
    h[o(0xf0)..o(0xf0) + 6].copy_from_slice(b"my map");
    let mut f = b"WDF1".to_vec();
    f.extend_from_slice(&1i32.to_le_bytes());
    f.extend_from_slice(&512u64.to_le_bytes());
    f.extend_from_slice(&h);
    let data: Vec<f32> = (0..4)
        .flat_map(|k| (0..3).map(move |p| (100 * k + p) as f32))
        .collect();
    f.extend(block(*b"DATA", &floats(&data)));
    let mut x = 1u32.to_le_bytes().to_vec();
    x.extend_from_slice(&1u32.to_le_bytes());
    x.extend(floats(&[1300.0, 1301.5, 1302.5]));
    f.extend(block(*b"XLST", &x));
    let xs = [10.0f64, 12.0, 10.0, 12.0];
    let ys = [20.0f64, 20.0, 22.0, 22.0];
    let mut org = 2u32.to_le_bytes().to_vec();
    for (ty, name, vals) in [(0x8000_0003u32, b"X", xs), (0x8000_0004, b"Y", ys)] {
        org.extend_from_slice(&ty.to_le_bytes());
        org.extend_from_slice(&5u32.to_le_bytes());
        let mut n = name.to_vec();
        n.resize(16, 0);
        org.extend_from_slice(&n);
        for v in vals {
            org.extend_from_slice(&v.to_le_bytes());
        }
    }
    f.extend(block(*b"ORGN", &org));
    let mut m = 2u32.to_le_bytes().to_vec();
    m.extend_from_slice(&0u32.to_le_bytes());
    for v in [10.0f32, 20.0, 0.0, 2.0, 2.0, 1.0] {
        m.extend_from_slice(&v.to_le_bytes());
    }
    for v in [2u32, 2, 1, 0] {
        m.extend_from_slice(&v.to_le_bytes());
    }
    f.extend(block(*b"WMAP", &m));
    f
}

#[test]
fn raman_map_info_trace_export_preview() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("map.wdf");
    std::fs::write(&f, wdf_map()).unwrap();

    let out = run(&["info", "--json"], &f);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = json(&out);
    let d = &v["data"];
    assert_eq!(d["format"]["id"], "renishaw-wdf");
    assert_eq!(d["traces"][0]["sweep_count"], 4);
    assert_eq!(d["traces"][0]["extra"]["axis"]["quantity"], "raman_shift");
    let wl = d["traces"][0]["extra"]["laser_wavelength_nm"]
        .as_f64()
        .unwrap();
    assert!((wl - 785.0).abs() < 1e-3, "{wl}");
    assert_eq!(d["images"][0]["size_c"], 3);
    assert_eq!(d["images"][0]["size_x"], 2);

    // the strongest band of spectrum 3 and where it is
    let out = run(&["trace", "--json", "--sweep", "3", "--channel", "1"], &f);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let s = &json(&out)["data"]["channels"][0]["stats"];
    assert_eq!(s["max"], 302.0);
    assert_eq!(s["argmax_axis_value"], 1302.5);

    // CSV of one spectrum of the map (the file also has a per-spectrum table)
    let csv = dir.path().join("s2.csv");
    let out = bin()
        .args(["export", "--to", "csv", "--sweep", "2", "--json", "-o"])
        .arg(&csv)
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let text = std::fs::read_to_string(&csv).unwrap();
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows.len(), 4, "{text}");
    assert!(rows[1].ends_with(",1300,200"), "{text}");
    // without --sweep/--trace the table is exported
    let tab = dir.path().join("t.csv");
    let out = bin()
        .args(["export", "--to", "csv", "--json", "-o"])
        .arg(&tab)
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(std::fs::read_to_string(&tab).unwrap().lines().count(), 5);

    // JCAMP-DX with the irregular Raman-shift axis as XY pairs
    let jdx = dir.path().join("s1.jdx");
    let out = bin()
        .args(["export", "--to", "jcamp", "--sweep", "1", "--json", "-o"])
        .arg(&jdx)
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let text = std::fs::read_to_string(&jdx).unwrap();
    assert!(text.contains("##DATA TYPE= RAMAN SPECTRUM"), "{text}");
    assert!(text.contains("##XYPOINTS= (XY..XY)"), "{text}");
    assert!(text.contains("1300,100"), "{text}");

    // the map as OME-TIFF and a preview of one band
    let tif = dir.path().join("map.ome.tif");
    let out = bin()
        .args(["export", "--to", "ome-tiff", "--json", "-o"])
        .arg(&tif)
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(json(&out)["data"]["planes_written"], 3);
    let png = dir.path().join("band.png");
    let out = bin()
        .args(["preview", "--select", "c=2", "--json", "-o"])
        .arg(&png)
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(std::fs::read(&png).unwrap().starts_with(b"\x89PNG"));

    let out = run(&["check", "--json"], &f);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // truncated in the data block: `info` (headers) says so, `check` fails with exit 4, and
    // reading a lost spectrum is a clean error with a hint, not a panic
    let cut = dir.path().join("cut.wdf");
    std::fs::write(&cut, &wdf_map()[..540]).unwrap();
    let out = run(&["info", "--json"], &cut);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let notes = json(&out)["data"]["notes"].to_string();
    assert!(notes.contains("damaged"), "{notes}");
    let out = run(&["check", "--json"], &cut);
    assert_eq!(
        out.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = run(&["trace", "--json", "--sweep", "3"], &cut);
    assert_eq!(
        out.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(json(&out)["error"]["hint"].is_string());
}

#[test]
fn omnic_series_without_a_series_record_is_corrupt() {
    // `.srs` series are read (tests/spectro.rs in the corpus tests); one whose key table has no
    // series record is a clean corrupt-file error, not a refusal
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("run.srs");
    let mut b = vec![0u8; 400];
    b[..18].copy_from_slice(b"Spectral Exte File");
    std::fs::write(&f, b).unwrap();
    let out = run(&["info", "--json"], &f);
    assert_eq!(out.status.code(), Some(4));
    let v = json(&out);
    assert_eq!(v["error"]["code"], "corrupt_file");
    assert!(v["error"]["hint"].is_string());
}
