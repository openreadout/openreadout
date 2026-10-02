//! `openreadout report`: the diagnostic bundle for new-variant issues is written to a local
//! file, holds the decode path and structure, and never the file's name, path or free text
//! unless asked (docs/maintaining.md).

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_openreadout"));
    c.env("OPENREADOUT_LIVE_WINDOW", "0");
    c
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn json(out: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON: {e}\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// Copy `src` into `dir` under a name that looks like a sample name, so leaks are visible.
fn copy_as(src: &Path, dir: &Path, name: &str) -> PathBuf {
    let dst = dir.join(name);
    std::fs::copy(src, &dst).unwrap();
    dst
}

#[test]
fn report_writes_a_bundle_without_the_name_or_path() {
    let tmp = tempfile::tempdir().unwrap();
    let input = copy_as(&fixture("mini.nd2"), tmp.path(), "JaneRoe_mouse7_liver.nd2");
    let out_path = tmp.path().join("bundle.json");
    let out = bin()
        .args(["check", "--report", "--json", "-o"])
        .arg(&out_path)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert_eq!(v["data"]["output"], out_path.display().to_string());
    let text = std::fs::read_to_string(&out_path).unwrap();
    let bundle: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        bundle, v["data"]["report"],
        "the file is exactly what --json shows"
    );
    for leak in ["JaneRoe", "mouse7", tmp.path().to_str().unwrap()] {
        assert!(!text.contains(leak), "bundle contains {leak:?}");
    }
    assert_eq!(bundle["detection"]["format"], "nd2");
    let stages: Vec<&str> = bundle["stages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["stage"].as_str().unwrap())
        .collect();
    assert_eq!(
        stages,
        [
            "detect",
            "open",
            "info",
            "vendor",
            "assurance",
            "ls",
            "check",
            "first_read"
        ]
    );
    assert!(
        bundle["assurance"]["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("nd2|")
    );
    assert!(bundle["structure"]["entries_total"].as_u64().unwrap() > 0);
    assert_eq!(bundle["input"]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(bundle["privacy"]["mode"], "structure_only");
    // a second run refuses to replace it
    let again = bin()
        .args(["check", "--report", "-o"])
        .arg(&out_path)
        .arg(&input)
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(2));
}

#[test]
fn unknown_files_are_reported_with_their_text_masked() {
    let tmp = tempfile::tempdir().unwrap();
    let input = tmp.path().join("assay_JaneRoe.xyz");
    std::fs::write(&input, "Operator: Jane Roe\nwell,value\nA1,0.5\n").unwrap();
    let out = bin()
        .args(["check", "--report", "--dry-run", "--json"])
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = json(&out);
    assert!(v["data"]["output"].is_null(), "dry run writes nothing");
    let r = &v["data"]["report"];
    let text = r.to_string();
    assert!(!text.contains("Jane"), "{text}");
    assert_eq!(r["stages"][0]["stage"], "detect");
    assert_eq!(r["stages"][0]["error"]["code"], "unknown_format");
    assert_eq!(r["stages"][0]["error"]["exit_code"], 3);
    assert_eq!(r["input"]["text_like"], true);
    assert!(
        r["input"]["signature_hex"]
            .as_str()
            .unwrap()
            .starts_with("__")
    );
    assert!(
        std::fs::read_dir(tmp.path()).unwrap().count() == 1,
        "nothing written"
    );
}

#[test]
fn include_text_and_hex_are_opt_in() {
    let tmp = tempfile::tempdir().unwrap();
    let input = copy_as(&fixture("mini.czi"), tmp.path(), "mini.czi");
    let plain = json(
        &bin()
            .args(["check", "--report", "--dry-run", "--json"])
            .arg(&input)
            .output()
            .unwrap(),
    );
    assert!(plain["data"]["report"]["samples"].is_null());
    let hex = json(
        &bin()
            .args([
                "check",
                "--report",
                "--dry-run",
                "--json",
                "--hex",
                "32",
                "--include-text",
            ])
            .arg(&input)
            .output()
            .unwrap(),
    );
    let r = &hex["data"]["report"];
    assert_eq!(r["privacy"]["mode"], "with_text");
    let samples = r["samples"].as_array().unwrap();
    assert_eq!(samples[0]["at"], "head");
    // `ZISRAWFILE`, unmasked
    assert!(
        samples[0]["hex"]
            .as_str()
            .unwrap()
            .starts_with("5a 49 53 52 41 57")
    );
}
