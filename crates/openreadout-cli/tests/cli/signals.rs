//! Signals and spectra: NMR (Bruker, Varian, JEOL, JCAMP-DX), chromatography, electron
//! microscopy, HDF5/NWB, and the trace exports (Parquet, Arrow, JCAMP-DX, NWB).

use std::path::PathBuf;

use crate::common::*;

/// A VnmrJ `.fid` directory: one block of 4 float32 values (2 complex points), procpar with sw.
fn tiny_varian(root: &std::path::Path) -> PathBuf {
    let d = root.join("PROTON_01.fid");
    std::fs::create_dir_all(&d).unwrap();
    let mut fid = Vec::new();
    for v in [1i32, 1, 4, 4, 16, 44] {
        fid.extend_from_slice(&v.to_be_bytes());
    }
    fid.extend_from_slice(&0i16.to_be_bytes());
    fid.extend_from_slice(&0xc9i16.to_be_bytes());
    fid.extend_from_slice(&1i32.to_be_bytes());
    let mut block = vec![0u8; 28];
    block[5] = 1; // block number 1
    fid.extend_from_slice(&block);
    for v in [1.5f32, -2.0, 3.0, 4.25] {
        fid.extend_from_slice(&v.to_be_bytes());
    }
    std::fs::write(d.join("fid"), fid).unwrap();
    std::fs::write(
        d.join("procpar"),
        "sw 1 1 1e9 0 0 2 1 11 1 64\n1 1000 \n0 \nnp 7 1 1e9 0 0 2 1 11 1 64\n1 4 \n0 \ntn 2 2 4 0 0 2 1 8 1 64\n1 \"H1\"\n0 \n",
    )
    .unwrap();
    d
}

/// A version-30 ChemStation `.D` directory with one three-sample DAD signal (raw 100, 102, 98;
/// scale 0.5; 0–400 ms).
fn tiny_chemstation(dir: &std::path::Path) -> PathBuf {
    let d = dir.join("T.D");
    std::fs::create_dir_all(&d).unwrap();
    let mut h = vec![0u8; 1024];
    h[..3].copy_from_slice(b"\x0230");
    let put = |h: &mut Vec<u8>, off: usize, s: &str| {
        h[off] = s.len() as u8;
        h[off + 1..off + 1 + s.len()].copy_from_slice(s.as_bytes());
    };
    put(&mut h, 0x04, "LC DATA FILE");
    put(&mut h, 0x18, "Sample 7");
    put(&mut h, 0xB2, "18-Nov-10, 15:48:06");
    put(&mut h, 0x244, "mAU");
    put(&mut h, 0x254, "DAD1 A, Sig=254,4 Ref=off");
    h[0x108..0x10C].copy_from_slice(&3u32.to_be_bytes());
    h[0x11A..0x11E].copy_from_slice(&0i32.to_be_bytes());
    h[0x11E..0x122].copy_from_slice(&400i32.to_be_bytes());
    h[0x284..0x28C].copy_from_slice(&0.5f64.to_be_bytes());
    h.extend([0x10, 3, 0x80, 0x00, 0, 0, 0, 100, 0, 2, 0xFF, 0xFC, 0, 0]);
    std::fs::write(d.join("DAD1A.ch"), &h).unwrap();
    std::fs::write(d.join("RUN.LOG"), b"run log text\r\n").unwrap();
    d
}

/// Electron-microscopy corpus files: info, check, truncation (exit 4) and OME-TIFF export with
/// physical sizes in µm.
#[test]
fn em_formats_info_check_truncation_and_export() {
    let cases = [
        ("mrcfile-emd-3197.map", "mrc", 11.4e-4),
        ("zenodo8190744-bto-atomic.dm3", "dm", 0.024_414_062_5e-3),
        (
            "zenodo13821437-Fig2c-part1.ser",
            "ser",
            1.170_694_147_693_483_3e-2,
        ),
        (
            "zenodo20040988-0050-STEM-15.4nm.emd",
            "emd",
            3.004_444_660_801_416_7e-5,
        ),
    ];
    let dir = tmp_dir("em");
    for (name, id, px) in cases {
        let Some(p) = corpus(name) else { continue };
        let out = bin()
            .args(["info", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = json(&out);
        assert_eq!(v["data"]["format"]["id"], id, "{name}");
        assert_eq!(v["data"]["format"]["family"], "electron-microscopy");
        let got = v["data"]["images"][0]["physical_size"]["x"]
            .as_f64()
            .unwrap();
        assert!(
            (got - px).abs() < 1e-9 * px.max(1e-12) + 1e-15,
            "{name}: {got} != {px}"
        );
        let out = bin()
            .args(["check", p.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let t = truncated_copy(&p);
        let out = bin()
            .args(["check", t.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(4),
            "truncated {name} must exit 4: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let tiff = dir.join(format!("{name}.ome.tiff"));
        let out = bin()
            .args([
                "export",
                p.to_str().unwrap(),
                "-o",
                tiff.to_str().unwrap(),
                "--overwrite",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(json(&out)["data"]["verified"], true);
        let bytes = std::fs::read(&tiff).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains("PhysicalSizeX="),
            "{name}: no PhysicalSizeX in OME-XML"
        );
        assert!(
            !text.contains("&#181;"),
            "{name}: µm is the schema default and stays unwritten (Bio-Formats rejects &#181;)"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// An .emi opens the series files next to it; a missing .ser is exit 6 with a hint.
#[test]
fn ser_emi_sidecar() {
    let Some(emi) = corpus("zenodo17463176-Fig_b1.emi") else {
        return;
    };
    let out = bin()
        .args(["info", emi.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    if corpus("zenodo17463176-Fig_b1_1.ser").is_some() {
        assert!(out.status.success());
        let v = json(&out);
        assert_eq!(v["data"]["images"][0]["extra"]["voltage_kv"], 200.0);
    }
    let dir = tmp_dir("emi");
    let lone = dir.join("Lone.emi");
    std::fs::copy(&emi, &lone).unwrap();
    let out = bin()
        .args(["info", lone.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert!(
        json(&out)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains(".ser")
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bruker_directory_info_trace_export_and_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let exp = tiny_bruker(dir.path());
    let out = bin()
        .args(["info", "--view", "format", "--json"])
        .arg(&exp)
        .output()
        .unwrap();
    assert_eq!(json(&out)["data"]["format"], "bruker-nmr");
    let out = bin().args(["info", "--json"]).arg(&exp).output().unwrap();
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["data"]["format_version"], "TopSpin 3.6.2");
    assert_eq!(v["data"]["traces"].as_array().unwrap().len(), 2);
    assert_eq!(v["data"]["traces"][0]["sample_count"], 4);
    assert_eq!(v["data"]["traces"][1]["extra"]["axis"]["unit"], "ppm");
    // human info names the traces
    let out = bin().arg("info").arg(&exp).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("trace [1] pdata/1"));
    // trace: scaled by 2^NC, one channel, a window starting at sample 1
    let out = bin()
        .args([
            "trace",
            "--json",
            "--channel",
            "1",
            "--first-sample",
            "1",
            "--count",
            "2",
        ])
        .arg(&exp)
        .output()
        .unwrap();
    let v = json(&out);
    assert_eq!(v["data"]["channels"][0]["name"], "imag");
    assert_eq!(
        v["data"]["channels"][0]["samples"],
        serde_json::json!([3.0, 5.0])
    );
    assert_eq!(v["data"]["channels"][0]["stats"]["max"], 5.0);
    assert_eq!(v["data"]["start_s"], 0.0005);
    assert_eq!(v["data"]["truncated"], false);
    // the processed spectrum (trace 1) carries its ppm axis
    let out = bin()
        .args(["trace", "--json", "--trace", "1"])
        .arg(&exp)
        .output()
        .unwrap();
    let v = json(&out);
    assert_eq!(
        v["data"]["channels"][0]["samples"],
        serde_json::json!([5.0, 6.0, 7.0, 8.0])
    );
    assert_eq!(v["data"]["axis"]["first"], 200.0);
    assert_eq!(v["data"]["axis"]["step"], -1.0);
    let out = bin()
        .args(["trace", "--json", "--trace", "5"])
        .arg(&exp)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // ls lists the files
    let out = bin()
        .args(["info", "--view", "structure", "--json"])
        .arg(&exp)
        .output()
        .unwrap();
    let names: Vec<String> = json(&out)["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"fid".into()) && names.contains(&"pdata/1/procs".into()));
    // export --format csv (default for traces), verified
    let csv = dir.path().join("fid.csv");
    let out = bin()
        .args(["export", "--json", "-o"])
        .arg(&csv)
        .arg(&exp)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(json(&out)["data"]["verified"], true);
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with("time_s,real,imag\n0,1,-1\n"), "{text}");
    // the processed spectrum exports with its ppm axis
    let csv1 = dir.path().join("spec.csv");
    let out = bin()
        .args(["export", "--json", "--trace", "1", "-o"])
        .arg(&csv1)
        .arg(&exp)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        std::fs::read_to_string(&csv1).unwrap(),
        "chemical_shift_ppm,real\n200,5\n199,6\n198,7\n197,8\n"
    );
    // the study directory is not an experiment: usage error naming it
    let out = bin()
        .args(["info", "--json"])
        .arg(exp.parent().unwrap())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // truncate the fid: check exits 4
    std::fs::write(exp.join("fid"), [0u8; 12]).unwrap();
    let out = bin().args(["check", "--json"]).arg(&exp).output().unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(
        json(&out)["data"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "truncated")
    );
    let out = bin().args(["trace", "--json"]).arg(&exp).output().unwrap();
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn varian_directory_info_trace_and_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let d = tiny_varian(dir.path());
    let out = bin()
        .args(["info", "--view", "format", "--json"])
        .arg(&d)
        .output()
        .unwrap();
    assert_eq!(json(&out)["data"]["format"], "varian-nmr");
    let out = bin().args(["info", "--json"]).arg(&d).output().unwrap();
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["data"]["traces"][0]["sample_rate_hz"], 1000.0);
    assert_eq!(v["data"]["traces"][0]["extra"]["nucleus"], "1H");
    let out = bin()
        .args(["trace", "--json", "--channel", "1"])
        .arg(d.join("fid"))
        .output()
        .unwrap();
    assert_eq!(
        json(&out)["data"]["channels"][0]["samples"],
        serde_json::json!([-2.0, 4.25])
    );
    assert!(
        bin()
            .args(["check"])
            .arg(&d)
            .output()
            .unwrap()
            .status
            .success()
    );
    // the folder holding the .fid directory: usage error naming it
    let out = bin()
        .args(["info", "--json"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // cut the last value: check exits 4
    let bytes = std::fs::read(d.join("fid")).unwrap();
    std::fs::write(d.join("fid"), &bytes[..bytes.len() - 4]).unwrap();
    let out = bin().args(["check", "--json"]).arg(&d).output().unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json(&out)["data"]["findings"][0]["code"], "truncated");
}

#[test]
fn jeol_jdf_info_and_truncation() {
    let Some(p) = corpus("nmrxiv-s1243/E sinica-higher concentration-500 MHz.jdf") else {
        return;
    };
    let out = bin().args(["info", "--json"]).arg(&p).output().unwrap();
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["data"]["format"]["id"], "jeol-jdf");
    assert_eq!(v["data"]["traces"][0]["sample_count"], 35000);
    assert_eq!(v["data"]["traces"][0]["extra"]["nucleus"], "1H");
    assert!(
        bin()
            .args(["check"])
            .arg(&p)
            .output()
            .unwrap()
            .status
            .success()
    );
    let t = truncated_copy(&p);
    let out = bin().args(["check", "--json"]).arg(&t).output().unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(
        json(&out)["data"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["code"] == "truncated")
    );
    let out = bin().args(["trace", "--json"]).arg(&t).output().unwrap();
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn jcamp_info_trace_and_csv() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("peaks.jdx");
    std::fs::write(
        &p,
        "##TITLE= peaks\n##JCAMP-DX= 5.00\n##DATA TYPE= MASS SPECTRUM\n##DATA CLASS= PEAK TABLE\n##XUNITS= M/Z\n##YUNITS= RELATIVE ABUNDANCE\n##NPOINTS= 3\n##PEAK TABLE= (XY..XY)\n50, 5.84\n51, 9.55; 52,4.19\n##END=\n",
    )
    .unwrap();
    let out = bin().args(["info", "--json"]).arg(&p).output().unwrap();
    let v = json(&out);
    assert_eq!(v["data"]["format"]["id"], "jcamp-dx");
    assert_eq!(v["data"]["traces"][0]["extra"]["kind"], "peak_table");
    let out = bin().args(["trace", "--json"]).arg(&p).output().unwrap();
    let v = json(&out);
    assert_eq!(
        v["data"]["channels"][0]["samples"],
        serde_json::json!([50.0, 51.0, 52.0])
    );
    assert_eq!(
        v["data"]["channels"][1]["samples"],
        serde_json::json!([5.84, 9.55, 4.19])
    );
    let csv = dir.path().join("peaks.csv");
    let out = bin()
        .args(["export", "--format", "csv", "-o"])
        .arg(&csv)
        .arg(&p)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        std::fs::read_to_string(&csv).unwrap(),
        "sample,x (M/Z),y (RELATIVE ABUNDANCE)\n0,50,5.84\n1,51,9.55\n2,52,4.19\n"
    );
    // image export of a spectrum is refused with a hint (exit 6)
    let out = bin()
        .args(["export", "--json", "--format", "ome-tiff", "-o"])
        .arg(dir.path().join("x.ome.tiff"))
        .arg(&p)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
}

#[test]
#[allow(clippy::many_single_char_names)]
fn chemstation_directory_info_trace_export_and_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let d = tiny_chemstation(dir.path());
    let out = bin()
        .args(["info", "--view", "format", "--json"])
        .arg(&d)
        .output()
        .unwrap();
    assert_eq!(json(&out)["data"]["format"], "chemstation");
    let out = bin().args(["info", "--json"]).arg(&d).output().unwrap();
    assert!(out.status.success());
    let v = json(&out);
    let t = &v["data"]["traces"][0];
    assert_eq!(t["sample_count"], 3);
    assert_eq!(t["sample_rate_hz"], 5.0);
    assert_eq!(t["channels"][0]["unit"], "mAU");
    assert_eq!(t["extra"]["sample_name"], "Sample 7");
    assert_eq!(t["extra"]["acquired_at"], "2010-11-18T15:48:06");
    assert_eq!(t["extra"]["wavelength_nm"], 254.0);
    let out = bin().args(["trace", "--json"]).arg(&d).output().unwrap();
    let v = json(&out);
    assert_eq!(
        v["data"]["channels"][0]["samples"],
        serde_json::json!([50.0, 51.0, 49.0])
    );
    // the single file opens too
    let out = bin()
        .args(["trace", "--json"])
        .arg(d.join("DAD1A.ch"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = bin()
        .args(["info", "--view", "structure", "--json"])
        .arg(&d)
        .output()
        .unwrap();
    let kinds: Vec<String> = json(&out)["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect();
    assert!(kinds.contains(&"signal".into()) && kinds.contains(&"file".into()));
    let csv = dir.path().join("t.csv");
    let out = bin()
        .args(["export", "--json", "--format", "csv", "-o"])
        .arg(&csv)
        .arg(&d)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(json(&out)["data"]["verified"], true);
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with("retention_time_min,"), "{text}");
    assert!(text.contains(",50\n") && text.contains(",49\n"), "{text}");
    let out = bin().args(["check", "--json"]).arg(&d).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    // cut the body inside a value: check exits 4
    let f = d.join("DAD1A.ch");
    let b = std::fs::read(&f).unwrap();
    std::fs::write(&f, &b[..b.len() - 5]).unwrap();
    let out = bin().args(["check", "--json"]).arg(&d).output().unwrap();
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
fn chromatography_corpus_info_check_and_truncation() {
    // (corpus file, format id, expects traces)
    for (name, id, traces) in [
        ("entab-test_179_fid.ch", "chemstation", true),
        ("entab-carotenoid_extract.d", "chemstation", true),
        ("cheminfo-agilent-hplc.cdf", "andi-chrom", true),
        ("zenodo7729413-SLA_8.cdf", "andi-chrom", true),
        (
            "mtbls3555-bv-ix-alpha-raw-zip/BV IX alpha QM - Lizzy SS-06-17-2021.raw",
            "waters-raw",
            true,
        ),
        ("zenodo17868549-gp070190p-hplc.lcd", "shimadzu", true),
    ] {
        let Some(p) = corpus(name) else { continue };
        let out = bin().args(["info", "--json"]).arg(&p).output().unwrap();
        assert!(out.status.success(), "{name}");
        let v = json(&out);
        assert_eq!(v["data"]["format"]["id"], id, "{name}");
        assert_eq!(
            v["data"]["traces"]
                .as_array()
                .is_some_and(|t| !t.is_empty()),
            traces,
            "{name}"
        );
        let out = bin().args(["check", "--json"]).arg(&p).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{name}");
        if p.is_file() {
            let t = truncated_copy(&p);
            let out = bin().args(["check", "--json"]).arg(&t).output().unwrap();
            assert_eq!(out.status.code(), Some(4), "{name} truncated");
        }
    }
    // an ANDI/MS spectrum through the spectrum command
    if let Some(p) = corpus("zenodo7729413-SLA_8.cdf") {
        let out = bin()
            .args(["spectrum", "--json", "--scan", "3000"])
            .arg(&p)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = json(&out);
        assert!(
            v["data"]["spectrum"]["mz"]
                .as_array()
                .is_some_and(|m| !m.is_empty())
        );
    }
}

#[test]
fn hdf5_family_detection_and_nwb_csv_export() {
    let fx = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../openreadout-hdf5/tests/fixtures");
    for (f, id) in [
        ("synthetic-imaris.ims", "ims"),
        ("synthetic-timeseries.nwb", "nwb"),
        ("synthetic-generic.h5", "hdf5"),
    ] {
        let out = bin()
            .args([
                "info",
                "--view",
                "format",
                fx.join(f).to_str().unwrap(),
                "--json",
            ])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(json(&out)["data"]["format"], id, "{f}");
    }
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("voltage.csv");
    let out = bin()
        .args([
            "export",
            fx.join("synthetic-timeseries.nwb").to_str().unwrap(),
            "--format",
            "csv",
            "--trace",
            // trace 0 is the fixture's ElectricalSeries, 1 the temperature series
            "2",
            "-o",
            csv.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let text = std::fs::read_to_string(&csv).unwrap();
    assert_eq!(text.lines().count(), 1001, "header + 1000 samples");
    // the Imaris fixture's planes are all listed
    assert_eq!(plane_hashes(&fx.join("synthetic-imaris.ims")).len(), 20);
}

#[test]
fn export_parquet_arrow_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let fcs = tiny_fcs(dir.path());
    // default name next to the input, verified
    let out = bin()
        .args(["export", "--json", "--format", "parquet"])
        .arg(&fcs)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["data"]["format"], "parquet");
    assert_eq!(v["data"]["kind"], "table");
    assert_eq!(v["data"]["compression"], "snappy");
    assert_eq!(v["data"]["verified"], true);
    let parquet = dir.path().join("tiny.parquet");
    assert!(parquet.exists());
    assert_eq!(&std::fs::read(&parquet).unwrap()[..4], b"PAR1");
    // Arrow IPC with LZ4 and a row range
    let arrow = dir.path().join("cut.arrow");
    let out = bin()
        .args([
            "export",
            "--json",
            "--format",
            "arrow",
            "--compression",
            "lz4",
            "--rows",
            "1-2",
            "-o",
        ])
        .arg(&arrow)
        .arg(&fcs)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["data"]["rows_written"], 2);
    assert_eq!(v["data"]["first_row"], 1);
    assert_eq!(&std::fs::read(&arrow).unwrap()[..6], b"ARROW1");
    // existing output, snappy for Arrow, deflate for Parquet, --trace on a table file: usage (2)
    for args in [
        vec!["--format", "parquet"],
        vec![
            "--format",
            "arrow",
            "--compression",
            "snappy",
            "--overwrite",
        ],
        vec![
            "--format",
            "parquet",
            "--compression",
            "deflate",
            "--overwrite",
        ],
        vec!["--format", "parquet", "--trace", "0", "--overwrite"],
        vec![
            "--format",
            "parquet",
            "--table",
            "0",
            "--spectra",
            "--overwrite",
        ],
    ] {
        let out = bin()
            .arg("export")
            .args(&args)
            .arg("-o")
            .arg(&parquet)
            .arg(&fcs)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
    // an image file has no tables: unsupported (6)
    let out = bin()
        .args(["self", "doctor", "--write-fixtures"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = bin()
        .args(["export", "--json", "--format", "parquet"])
        .arg(dir.path().join("doctor.tif"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    // jcamp and nwb of a table file: unsupported (6)
    for to in ["jcamp", "nwb"] {
        let out = bin()
            .args(["export", "--json", "--format", to])
            .arg(&fcs)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(6), "{to}");
    }
}

#[test]
fn export_jcamp_of_a_bruker_fid_and_spectrum() {
    let dir = tempfile::tempdir().unwrap();
    let exp = tiny_bruker(dir.path());
    let fid = dir.path().join("fid.jdx");
    let out = bin()
        .args(["export", "--json", "--format", "jcamp", "-o"])
        .arg(&fid)
        .arg(&exp)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["data"]["format"], "jcamp-dx");
    assert_eq!(v["data"]["exact"], true);
    assert_eq!(v["data"]["verified"], true);
    // the written file reads back through our JCAMP-DX reader, value for value
    let a = json(&bin().args(["trace", "--json"]).arg(&exp).output().unwrap());
    let b = json(&bin().args(["trace", "--json"]).arg(&fid).output().unwrap());
    let n = a["data"]["channels"].as_array().unwrap().len();
    assert!(n > 0);
    for c in 0..n {
        assert_eq!(
            a["data"]["channels"][c]["samples"], b["data"]["channels"][c]["samples"],
            "channel {c}"
        );
    }
    // the processed spectrum, AFFN
    let spec = dir.path().join("spec.jdx");
    let out = bin()
        .args([
            "export",
            "--json",
            "--format",
            "jcamp",
            "--trace",
            "1",
            "--compression",
            "none",
            "-o",
        ])
        .arg(&spec)
        .arg(&exp)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(json(&out)["data"]["encoding"], "affn");
    let text = std::fs::read_to_string(&spec).unwrap();
    assert!(text.contains("##DATA TYPE= NMR SPECTRUM"));
}

#[test]
fn export_nwb_of_an_abf_file() {
    let Some(abf) = corpus("pyabf-171116sh-0011.abf") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let nwb = dir.path().join("cell.nwb");
    let out = bin()
        .args(["export", "--json", "--format", "nwb", "--sweep", "2", "-o"])
        .arg(&nwb)
        .arg(&abf)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["data"]["nwb_version"], "2.7.0");
    // the recorded trace and the command waveform synthesized from the epoch table
    let series = v["data"]["series"].as_array().unwrap();
    assert_eq!(series.len(), 2);
    assert_eq!(
        (series[0]["trace"].as_u64(), series[1]["trace"].as_u64()),
        (Some(0), Some(1))
    );
    // our NWB reader sees the series with the sweep's samples
    let a = json(
        &bin()
            .args(["trace", "--json", "--sweep", "2"])
            .arg(&abf)
            .output()
            .unwrap(),
    );
    let b = json(&bin().args(["trace", "--json"]).arg(&nwb).output().unwrap());
    assert_eq!(b["data"]["format"], "nwb");
    assert_eq!(
        a["data"]["channels"][0]["samples"],
        b["data"]["channels"][0]["samples"]
    );
}
