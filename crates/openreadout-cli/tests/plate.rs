//! Plate-reader exports through the binary: info, check, CSV and ASM export, `.pda` refusal.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openreadout"))
}

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\n{}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

const GEN5_EXPORT: &str = "Software Version\t3.15.15\r\n\r\nPlate Number\tPlate 1\r\nDate\t6/12/2024\r\nTime\t5:47:49 PM\r\nReader Type:\tSynergy H1\r\nReader Serial Number:\t12345678\r\n\r\nProcedure Details\r\n\r\nPlate Type\t96 WELL PLATE (Use plate lid)\r\nRead\tod\r\n\tAbsorbance Endpoint\r\n\tFull Plate\r\n\tWavelengths:  600\r\n\r\nResults\r\n\t1\t2\t3\r\nA\t0.052\tOVRFLW\t0.051\tod:600\r\nB\t0.124\t0.047\t0.049\tod:600\r\n";

#[test]
fn plate_reader_export_info_check_csv_asm() {
    let dir = std::env::temp_dir().join(format!("openreadout-plate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("reads.txt");
    std::fs::write(&f, GEN5_EXPORT).unwrap();
    let out = bin().args(["info", "--json"]).arg(&f).output().unwrap();
    assert!(out.status.success());
    let v = json(&out);
    let t = &v["data"]["tables"][0];
    assert_eq!(v["data"]["format"]["id"], "plate");
    assert_eq!(t["row_count"], 6);
    assert_eq!(t["extra"]["export"], "gen5");
    assert_eq!(t["extra"]["instrument"]["model"], "Synergy H1");
    assert_eq!(t["extra"]["reads"][0]["wavelength_nm"], 600.0);
    assert_eq!(t["extra"]["non_numeric"][0]["text"], "OVRFLW");
    // check: a partial plate and a non-numeric cell are findings, not errors
    let out = bin().args(["check", "--json"]).arg(&f).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let codes: Vec<String> = json(&out)["data"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["code"].as_str().unwrap().to_string())
        .collect();
    assert!(codes.iter().any(|c| c == "non_numeric_value"), "{codes:?}");
    assert!(codes.iter().any(|c| c == "partial_plate"), "{codes:?}");
    // CSV: well names, NaN for the overflow cell, read back and verified
    let csv = dir.join("reads.csv");
    let out = bin()
        .args(["export", "--format", "csv", "--json", "-o"])
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
    assert!(
        text.starts_with("well,row,col,read,wavelength_nm,time_s,value\nA1,1,1,1,600,NaN,0.052\nA2,1,2,1,600,NaN,NaN\n"),
        "{text}"
    );
    // ASM plate-reader JSON
    let asm = dir.join("reads.asm.json");
    let out = bin()
        .args(["export", "--format", "asm", "--json", "-o"])
        .arg(&asm)
        .arg(&f)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let r = json(&out);
    assert_eq!(r["data"]["verified"], true);
    assert_eq!(r["data"]["measurements"], 6);
    assert_eq!(r["data"]["values"], 5);
    assert_eq!(r["data"]["errors"], 1);
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&asm).unwrap()).unwrap();
    assert_eq!(
        doc["$asm.manifest"],
        "http://purl.allotrope.org/manifests/plate-reader/REC/2025/03/plate-reader.manifest"
    );
    // a SoftMax Pro binary document is detected and refused (exit 6)
    let pda = dir.join("x.pda");
    std::fs::write(&pda, b"\x00\x01binary").unwrap();
    let out = bin().args(["info", "--json"]).arg(&pda).output().unwrap();
    assert_eq!(out.status.code(), Some(6));
    std::fs::remove_dir_all(&dir).ok();
}
