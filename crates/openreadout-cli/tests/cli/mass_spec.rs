//! Mass spectrometry: Thermo, Waters and Sciex runs, spectra and scan headers.

use crate::common::*;

#[test]
#[allow(clippy::many_single_char_names)] // same short names as the other corpus tests
fn thermo_raw_info_spectrum_export_and_truncation() {
    let Some(p) = corpus("mtbls20-caffeine-pos.raw") else {
        return;
    };
    let f = p.to_str().unwrap();
    let out = bin().args(["info", f, "--json"]).output().unwrap();
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["data"]["format"]["id"], "thermo-raw");
    assert_eq!(v["data"]["format_version"], "63");
    let run = &v["data"]["spectra"][0];
    assert_eq!(run["scan_count"], 141);
    assert_eq!(run["ms_levels"], serde_json::json!([1, 2]));
    assert_eq!(run["instrument"]["model"], "LTQ Orbitrap Discovery");

    let out = bin()
        .args(["spectra", f, "--scan", "2", "--centroid", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = &json(&out)["data"]["spectrum"];
    assert_eq!(s["ms_level"], 2);
    assert_eq!(
        s["scan_filter"],
        "FTMS + p ESI d Full ms2 84.08@cid20.00 [50.00-95.00]"
    );
    assert_eq!(s["mz"].as_array().unwrap().len(), 21);
    let out = bin()
        .args(["spectra", f, "--index", "999999", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));

    let out = bin().args(["check", f, "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));

    let dir = std::env::temp_dir().join(format!("openreadout-mzml-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let o = dir.join("caffeine.mzML");
    let out = bin()
        .args([
            "export",
            f,
            "--to",
            "mzml",
            "--centroid",
            "--overwrite",
            "-o",
        ])
        .arg(&o)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = json(&out);
    assert_eq!(v["data"]["verified"], true);
    assert_eq!(v["data"]["spectra_written"], 141);
    let text = std::fs::read_to_string(&o).unwrap();
    assert!(text.contains("<indexListOffset>") && text.contains("<fileChecksum>"));

    let t = truncated_copy(&p);
    let out = bin()
        .args(["check", t.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json(&out)["data"]["findings"][0]["code"], "truncated");
    let out = bin()
        .args(["spectra", t.to_str().unwrap(), "--index", "0", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json(&out)["error"]["code"], "corrupt_file");
}

#[test]
#[allow(clippy::many_single_char_names)] // same short names as the other corpus tests
fn waters_full_scan_spectrum_export_and_truncation() {
    let Some(p) = corpus("pxd059722-mth2-alicine-td-1-raw-zip/240429_MTH2_Alicine_TD_1_r0CENT.raw")
    else {
        return;
    };
    let f = p.to_str().unwrap();
    let out = bin().args(["info", f, "--json"]).output().unwrap();
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["data"]["format"]["id"], "waters-raw");
    let run = &v["data"]["spectra"][0];
    assert_eq!(run["scan_count"], 472);
    assert_eq!(run["ms_levels"], serde_json::json!([1, 2]));
    assert_eq!(run["instrument"]["model"], "SYNAPT-XS");
    // the first spectrum is the lock-spray reference scan at 0.035 min
    let out = bin()
        .args(["spectra", f, "--index", "0", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = &json(&out)["data"]["spectrum"];
    assert_eq!(s["native_id"], "function=3 process=0 scan=1");
    assert_eq!(s["extra"]["lock_mass_reference"], true);
    let out = bin()
        .args(["spectra", f, "--ms-level", "2", "--nth", "1", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = &json(&out)["data"]["spectrum"];
    assert!((s["precursor_mz"].as_f64().unwrap() - 1223.3878).abs() < 1e-3);
    let out = bin().args(["check", f, "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let dir = std::env::temp_dir().join(format!("openreadout-waters-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let o = dir.join("waters.mzML");
    let out = bin()
        .args(["export", f, "--to", "mzml", "--overwrite", "-o"])
        .arg(&o)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(json(&out)["data"]["spectra_written"], 472);
    let text = std::fs::read_to_string(&o).unwrap();
    assert!(text.contains("Waters nativeID format"));
    assert!(text.contains("id=\"function=1 process=0 scan=1\""));
    // a cut-off DAT file: check exits 4
    let cut = dir.join("cut.raw");
    std::fs::create_dir_all(&cut).unwrap();
    for e in std::fs::read_dir(&p).unwrap().flatten() {
        std::fs::copy(e.path(), cut.join(e.file_name())).unwrap();
    }
    let dat = std::fs::read(p.join("_FUNC001.DAT")).unwrap();
    std::fs::write(cut.join("_FUNC001.DAT"), &dat[..dat.len() / 2]).unwrap();
    let out = bin()
        .args(["check", cut.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
}

#[test]
#[allow(clippy::many_single_char_names)] // same short names as the other corpus tests
fn sciex_wiff_mrm_check_truncation_and_wiff2() {
    let dir = std::env::temp_dir().join(format!("openreadout-sciex-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // .wiff2 is refused with exit 6 whatever it holds
    let w2 = dir.join("run.wiff2");
    std::fs::write(&w2, b"encrypted").unwrap();
    let out = bin()
        .args(["info", w2.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    let Some(p) = corpus("mtbls6084-sl-st-blank2.wiff") else {
        return;
    };
    let f = p.to_str().unwrap();
    let out = bin().args(["info", f, "--json"]).output().unwrap();
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["data"]["format"]["id"], "sciex-wiff");
    let run = &v["data"]["spectra"][0];
    assert_eq!(run["scan_count"], 27036);
    assert_eq!(run["instrument"]["model"], "QTRAP 6500+");
    // Local acquisition clock placed in its zone from the file's UTC storage times.
    assert_eq!(run["extra"]["acquired_at"], "2021-09-23T16:03:25.000+02:00");
    let out = bin()
        .args(["spectra", f, "--index", "0", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let s = &json(&out)["data"]["spectrum"];
    assert_eq!(s["ms_level"], 2);
    assert!(s["precursor_mz"].as_f64().is_some());
    // the .wiff.scan companion opens the .wiff beside it
    let scan = p.with_file_name("mtbls6084-sl-st-blank2.wiff.scan");
    let out = bin()
        .args(["info", scan.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = bin().args(["check", f, "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    // a cut-off .wiff.scan: check exits 4
    std::fs::copy(&p, dir.join("cut.wiff")).unwrap();
    let data = std::fs::read(&scan).unwrap();
    std::fs::write(dir.join("cut.wiff.scan"), &data[..data.len() * 6 / 10]).unwrap();
    let out = bin()
        .args(["check", dir.join("cut.wiff").to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(
        json(&out)["data"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "truncated")
    );
}

#[test]
fn scans_list_headers_with_filters_pages_and_csv() {
    let Some(p) = corpus("mtbls20-caffeine-pos.raw") else {
        return;
    };
    let f = p.to_str().unwrap();
    let out = bin()
        .args(["spectra", f, "--ms-level", "2", "--limit", "3", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let d = &json(&out)["data"];
    assert_eq!(d["scan_count"], 141);
    assert_eq!(d["source"], "headers");
    let matched = d["matched"].as_u64().unwrap();
    assert!(matched > 3);
    assert_eq!(d["returned"], 3);
    assert_eq!(d["truncated"], true);
    assert_eq!(d["ms_level_counts"]["2"].as_u64(), Some(matched));
    let s = &d["scans"][0];
    assert_eq!(s["scan_number"], 2);
    assert_eq!(
        s["scan_filter"],
        "FTMS + p ESI d Full ms2 84.08@cid20.00 [50.00-95.00]"
    );
    assert_eq!(s["activation"], "CID");
    assert!((s["precursor_mz"].as_f64().unwrap() - 84.08).abs() < 0.01);
    assert!(s.get("mz").is_none());
    // counting only, and a precursor filter
    let out = bin()
        .args([
            "spectra",
            f,
            "--precursor",
            "84.08",
            "--tol",
            "0.01",
            "--count",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let d = &json(&out)["data"];
    assert_eq!(d["returned"], 0);
    assert!(d["matched"].as_u64().unwrap() >= 1);
    // CSV: a header and one row per MS2 scan
    let out = bin()
        .args(["spectra", f, "--ms-level", "2", "--csv"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let mut lines = text.lines();
    assert!(
        lines
            .next()
            .unwrap()
            .starts_with("index,scan_number,native_id,ms_level,rt_s")
    );
    assert_eq!(lines.count() as u64, matched);
    // bad filters are usage errors
    let out = bin()
        .args(["spectra", f, "--rt", "9-3", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// An mzML exported to mzML keeps its instrument (exact model, software and component terms)
/// and its chromatograms: `check --against` finds the two identical. Regression for the
/// 2026-10 website audit (generic term names came back, the TIC was dropped).
#[test]
fn mzml_to_mzml_keeps_instrument_terms_and_chromatograms() {
    let src = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus/whole_mzml/pyteomics-tiny-pwiz.mzML");
    let tmp = tempfile::tempdir().unwrap();
    let out_path = tmp.path().join("rt.mzML");
    let out = bin()
        .arg("export")
        .arg(&src)
        .args(["--to", "mzml", "-o"])
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let out = bin()
        .args(["info", "--json"])
        .arg(&out_path)
        .output()
        .unwrap();
    let v = json(&out);
    let inst = &v["data"]["spectra"][0]["instrument"];
    assert_eq!(inst["model"], "LCQ Deca");
    assert_eq!(inst["software"], "CompassXtract");
    assert_eq!(inst["detector"], "electron multiplier");
    assert_eq!(
        v["data"]["traces"][0]["extra"]["chromatogram_type"],
        "total ion current chromatogram"
    );
    let out = bin()
        .args(["check", "--json"])
        .arg(&src)
        .arg("--against")
        .arg(&out_path)
        .output()
        .unwrap();
    let v = json(&out);
    assert_eq!(
        v["data"]["identical"], true,
        "{:#}",
        v["data"]["metadata"]["differences"]
    );
}
