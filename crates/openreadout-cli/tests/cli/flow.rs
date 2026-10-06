//! Flow cytometry: FCS info, tables, CSV export and gating.

use std::path::PathBuf;

use crate::common::*;

#[test]
fn fcs_info_ls_check_and_csv_export() {
    let dir = std::env::temp_dir().join(format!("openreadout-fcs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = tiny_fcs(&dir);
    let ps = p.to_str().unwrap();
    let out = bin().args(["info", ps, "--json"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let v = json(&out);
    assert_eq!(v["data"]["format"]["id"], "fcs");
    assert_eq!(v["data"]["tables"][0]["row_count"], 4);
    assert_eq!(v["data"]["tables"][0]["columns"][0]["name"], "FSC-A");
    let out = bin()
        .args(["info", "--view", "structure", ps, "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let names: Vec<String> = json(&out)["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["HEADER", "TEXT", "DATA"]);
    let out = bin().args(["check", ps, "--json"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = bin()
        .args(["info", "--view", "full", "--vendor", ps, "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        json(&out)["data"]["vendor"]["data_sets"][0]["text"]["$P1S"],
        "Forward, area"
    );

    // CSV export: --format is inferred for tabular files; labels line is quoted; masks applied
    let csv = dir.join("tiny.csv");
    let out = bin()
        .args([
            "export",
            ps,
            "-o",
            csv.to_str().unwrap(),
            "--labels",
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
    assert_eq!(v["data"]["rows_written"], 4);
    let text = std::fs::read_to_string(&csv).unwrap();
    assert_eq!(
        text,
        "FSC-A,SSC-A\n\"Forward, area\",\n1,2\n1023,65535\n7,9\n100,200\n"
    );
    // a row range, and refusing to overwrite without --overwrite
    let out = bin()
        .args([
            "export",
            ps,
            "--format",
            "csv",
            "-o",
            csv.to_str().unwrap(),
            "--rows",
            "1-2",
            "--overwrite",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        std::fs::read_to_string(&csv).unwrap(),
        "FSC-A,SSC-A\n1023,65535\n7,9\n"
    );
    let out = bin()
        .args(["export", ps, "--format", "csv", "-o", csv.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // OME-TIFF of a table is an unsupported feature (6); a truncated FCS fails check with 4
    let out = bin()
        .args([
            "export",
            ps,
            "--format",
            "ome-tiff",
            "-o",
            dir.join("x.ome.tiff").to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    let bytes = std::fs::read(&p).unwrap();
    let t = dir.join("cut.fcs");
    std::fs::write(&t, &bytes[..bytes.len() - 3]).unwrap();
    let out = bin()
        .args(["check", t.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    let out = bin()
        .args([
            "export",
            t.to_str().unwrap(),
            "-o",
            dir.join("cut.csv").to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(
        !dir.join("cut.csv").exists(),
        "failed export must not leave a file"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fcs_corpus_export_matches_table() {
    let Some(p) = corpus("fcsparser-guava-muse.fcs") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("openreadout-fcsx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let csv = dir.join("g.csv");
    let out = bin()
        .args([
            "export",
            p.to_str().unwrap(),
            "--table",
            "3",
            "-o",
            csv.to_str().unwrap(),
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
    assert_eq!(v["data"]["rows_written"], 50037);
    assert_eq!(v["data"]["columns_written"], 10);
    let lines = std::fs::read_to_string(&csv).unwrap().lines().count();
    assert_eq!(lines, 50038);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn gate_and_table_on_flowkit_files() {
    let (Some(fcs), Some(wsp), Some(gml_fcs), Some(gml)) = (
        corpus("flowkit-diamond.fcs"),
        corpus("flowkit-diamond-quad.wsp"),
        corpus("flowkit-gml-events.fcs"),
        corpus("flowkit-gml-all.xml"),
    ) else {
        return;
    };
    let out = bin()
        .args([
            "analyze",
            "gate",
            fcs.to_str().unwrap(),
            "--workspace",
            wsp.to_str().unwrap(),
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
    let pops = v["data"]["populations"].as_array().unwrap();
    assert_eq!(pops.len(), 4);
    // FlowJo stored these counts; ours must agree.
    for p in pops {
        assert_eq!(p["count"], p["stored_count"], "{p}");
    }
    let out = bin()
        .args([
            "analyze",
            "gate",
            gml_fcs.to_str().unwrap(),
            "--gatingml",
            gml.to_str().unwrap(),
            "--population",
            "Range1",
            "--json",
        ])
        .output()
        .unwrap();
    let v = json(&out);
    assert_eq!(v["data"]["populations"][0]["path"], "/Range1");
    assert_eq!(v["data"]["populations"][0]["count"], 440);
    // table: processing is recorded; the membership column is appended.
    let out = bin()
        .args([
            "table",
            gml_fcs.to_str().unwrap(),
            "--gatingml",
            gml.to_str().unwrap(),
            "--population",
            "Range1",
            "--transform",
            "logicle",
            "--max-rows",
            "5",
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
    assert_eq!(
        v["data"]["columns"].as_array().unwrap().last().unwrap(),
        "gate:/Range1"
    );
    assert_eq!(v["data"]["rows"].as_array().unwrap().len(), 5);
    assert!(
        !v["data"]["processing"]["transforms"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn table_processing_needs_an_fcs_file() {
    let lif = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini.lif");
    let out = bin()
        .args(["table", lif.to_str().unwrap(), "--compensate", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = bin().args(["analyze", "gate", "--json"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
