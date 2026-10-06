//! Batch mode (several inputs, globs, data-set directories, JSON lines), `stats`, `check
//! --against` and `info --view full --sidecar`, on the synthetic files `self doctor
//! --write-fixtures` generates.

use std::path::PathBuf;

use crate::common::*;

#[test]
fn single_input_with_unicode_and_spaces_is_not_a_batch() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, tif, fcs) = ux_fixtures(tmp.path());
    for (cmd, file) in [
        (&["info", "--view", "format"][..], &tif),
        (&["info"], &tif),
        (&["info", "--view", "full"], &tif),
        (&["info", "--view", "explain"], &tif),
        (&["info", "--view", "structure"], &tif),
        (&["check"], &tif),
        (&["check", "--planes"], &tif),
        (&["info"], &fcs),
        (&["check"], &fcs),
    ] {
        let out = bin().args(cmd).arg("--json").arg(file).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{cmd:?}: {}", stderr(&out));
        let v = json(&out);
        assert_eq!(v["ok"], true, "{cmd:?}");
        assert!(
            v.get("path").is_none(),
            "{cmd:?}: single input has no envelope path"
        );
    }
}

#[test]
fn batch_jsonl_array_summary_and_worst_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, tif, fcs) = ux_fixtures(tmp.path());
    std::fs::write(dir.join("notes.txt"), b"not an instrument file\n").unwrap();
    std::fs::write(dir.join(".hidden.tif"), b"ignored").unwrap();
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::copy(&tif, sub.join("copy.tif")).unwrap();

    // A directory without --recursive: its files only (sorted, hidden skipped).
    let out = bin().args(["info", "--jsonl"]).arg(&dir).output().unwrap();
    assert_eq!(out.status.code(), Some(3), "worst code is unknown format");
    let lines = jsonl(&out);
    let paths: Vec<String> = lines
        .iter()
        .map(|l| l["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(lines.len(), 3, "{paths:?}");
    assert!(paths[0].ends_with("doctor.fcs") && paths[1].ends_with("doctor.tif"));
    assert!(paths[2].ends_with("notes.txt"));
    assert_eq!(lines[0]["ok"], true);
    assert_eq!(lines[0]["data"]["format"]["id"], "fcs");
    assert_eq!(lines[2]["ok"], false);
    assert_eq!(lines[2]["error"]["exit_code"], 3);

    // --recursive walks sub-directories; --skip-unknown leaves out the text file.
    let out = bin()
        .args([
            "info",
            "--view",
            "format",
            "-r",
            "--jsonl",
            "--skip-unknown",
        ])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let lines = jsonl(&out);
    assert_eq!(lines.len(), 3);
    assert!(lines[2]["path"].as_str().unwrap().ends_with("copy.tif"));

    // --json in batch mode: one array of envelopes.
    let out = bin()
        .args(["check", "--json"])
        .arg(&tif)
        .arg(&fcs)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v = json(&out);
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    assert!(arr.iter().all(|e| e["ok"] == true && e["path"].is_string()));

    // --fail-fast stops at the first failure.
    let out = bin()
        .args(["info", "--jsonl", "--fail-fast"])
        .arg(dir.join("notes.txt"))
        .arg(&tif)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(jsonl(&out).len(), 1);

    // Human output: per-file reports, a summary table, and errors on stderr.
    let out = bin()
        .args(["--color", "never", "info", "--view", "structure"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let s = stdout(&out);
    assert!(s.contains("==> ") && s.contains("doctor.tif"), "{s}");
    assert!(
        s.contains("status") && s.contains("unknown format (3)"),
        "{s}"
    );
    assert!(s.contains("3 inputs: 2 ok, 1 failed"), "{s}");
    assert!(stderr(&out).contains("notes.txt"));

    // --quiet: no summary, errors still reported on stderr.
    let out = bin().args(["-q", "check"]).arg(&dir).output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(out.stdout.is_empty());
    assert!(stderr(&out).contains("error"));
}

#[test]
fn batch_glob_pattern_and_missing_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, _, _) = ux_fixtures(tmp.path());
    // Quoted (unexpanded) pattern, as Windows shells pass it.
    let out = bin()
        .args(["info", "--view", "format", "--jsonl"])
        .arg(dir.join("*.tif"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let lines = jsonl(&out);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["data"]["format"], "tiff");
    let out = bin()
        .args(["info", "--view", "format", "--jsonl"])
        .arg(dir.join("*.nothing"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let out = bin()
        .args(["info", "--jsonl"])
        .arg(dir.join("doctor.tif"))
        .arg(dir.join("missing.czi"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(jsonl(&out).len(), 2);
}

#[test]
fn batch_treats_dataset_directories_as_one_item() {
    let tmp = tempfile::tempdir().unwrap();
    let exp = tiny_bruker(tmp.path());
    let out = bin()
        .args(["info", "--view", "format", "-r", "--jsonl"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let lines = jsonl(&out);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["data"]["format"], "bruker-nmr");
    // The experiment (or the study directory holding it) is one item, never walked into.
    let item = PathBuf::from(lines[0]["path"].as_str().unwrap());
    assert!(exp.starts_with(&item) && item != tmp.path(), "{item:?}");
}

#[test]
fn batch_export_mirrors_inputs_under_output_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, _, _) = ux_fixtures(tmp.path());
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::copy(dir.join("doctor.tif"), sub.join("copy.tif")).unwrap();
    let out_dir = tmp.path().join("out");
    let out = bin()
        .args(["export", "-r", "--jsonl", "--skip-unknown", "-o"])
        .arg(&out_dir)
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let lines = jsonl(&out);
    assert_eq!(lines.len(), 3);
    assert!(out_dir.join("doctor.ome.tiff").exists());
    assert!(out_dir.join("doctor.csv").exists());
    assert!(out_dir.join("sub/copy.ome.tiff").exists());
    // Export does not read standard input.
    let out = bin().args(["export", "-"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn stats_json_human_and_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let (_, tif, fcs) = ux_fixtures(tmp.path());
    let out = bin()
        .args(["stats", "--json", "--bins", "8"])
        .arg(&tif)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let v = json(&out);
    let d = &v["data"];
    assert_eq!(d["format"], "tiff");
    assert_eq!(d["planes"].as_array().unwrap().len(), 2);
    let s = &d["images"][0]["stats"];
    assert_eq!(s["count"], 256);
    assert_eq!(s["min"], 0.0);
    assert_eq!(s["max"], 1127.0);
    // Values: page 0 is 0..127, page 1 is 1000..1127.
    assert_eq!(s["mean"], 563.5);
    assert_eq!(s["exact"], true);
    assert_eq!(s["saturated_fraction"], 0.0);
    assert!((s["zero_fraction"].as_f64().unwrap() - 1.0 / 256.0).abs() < 1e-12);
    for p in ["p1", "p5", "p50", "p95", "p99"] {
        assert!(s["percentiles"][p].is_number(), "{p}");
    }
    let counts: u64 = s["histogram"]["counts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_u64().unwrap())
        .sum();
    assert_eq!(counts, 256);
    assert_eq!(s["histogram"]["counts"].as_array().unwrap().len(), 8);
    // Per-plane statistics of plane z=1 only.
    let out = bin()
        .args(["stats", "--json", "--select", "z=1", "--bins", "0"])
        .arg(&tif)
        .output()
        .unwrap();
    let v = json(&out);
    let planes = v["data"]["planes"].as_array().unwrap();
    assert_eq!(planes.len(), 1);
    assert_eq!(planes[0]["stats"]["min"], 1000.0);
    assert!(planes[0]["stats"].get("histogram").is_none());
    // Human output, log scale, no per-plane table.
    let out = bin()
        .args(["--color", "never", "stats", "--no-planes", "--scale", "log"])
        .arg(&tif)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let s = stdout(&out);
    assert!(
        s.contains("p50") && s.contains("saturated") && s.contains("image 0"),
        "{s}"
    );
    // Not an image file: unsupported (6) with a hint towards `trace`/`export --format csv`.
    let out = bin().args(["stats", "--json"]).arg(&fcs).output().unwrap();
    assert_eq!(out.status.code(), Some(6));
    // Too many bins.
    let out = bin()
        .args(["stats", "--bins", "100000"])
        .arg(&tif)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    // Batch mode and standard input.
    let out = bin()
        .args(["stats", "--jsonl", "--no-planes"])
        .arg(&tif)
        .arg(&fcs)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6));
    assert_eq!(jsonl(&out).len(), 2);
    let out = bin()
        .args(["stats", "--json", "-"])
        .stdin(std::fs::File::open(&tif).unwrap())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json(&out)["data"]["path"], "-");
}

#[test]
fn compare_export_roundtrip_changed_sample_and_tolerance() {
    let tmp = tempfile::tempdir().unwrap();
    let (dir, tif, fcs) = ux_fixtures(tmp.path());
    let ome = dir.join("rt.ome.tiff");
    let out = bin()
        .args(["export", "-o"])
        .arg(&ome)
        .arg(&tif)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    // Pixel data identical; metadata left out.
    let out = bin()
        .args(["check", "--json", "--no-metadata"])
        .arg(&tif)
        .arg("--against")
        .arg(&ome)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let v = json(&out);
    assert_eq!(v["data"]["identical"], true);
    assert_eq!(v["data"]["planes"]["planes"], 2);
    assert_eq!(v["data"]["planes"]["identical"], 2);
    assert_eq!(v["data"]["images"][0]["geometry_equal"], true);
    // A file compared with itself, metadata included.
    let out = bin()
        .args(["check"])
        .arg(&tif)
        .arg("--against")
        .arg(&tif)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).contains("identical"));
    // One sample changed by 1 (the last one: 1127 -> 1128).
    let mut bytes = std::fs::read(&tif).unwrap();
    let n = bytes.len();
    assert_eq!(u16::from_le_bytes([bytes[n - 2], bytes[n - 1]]), 1127);
    bytes[n - 2] += 1;
    let changed = dir.join("changed.tif");
    std::fs::write(&changed, &bytes).unwrap();
    let out = bin()
        .args(["check", "--json"])
        .arg(&tif)
        .arg("--against")
        .arg(&changed)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    assert_eq!(v["ok"], true, "a difference is a result, not an error");
    assert_eq!(v["data"]["identical"], false);
    assert_eq!(v["data"]["planes"]["mismatched"], 1);
    let m = &v["data"]["planes"]["mismatches"][0];
    assert_eq!(m["z"], 1);
    assert_ne!(m["ours_xxh3"], m["theirs_xxh3"]);
    let out = bin()
        .args(["check", "--json", "--tolerance", "1"])
        .arg(&tif)
        .arg("--against")
        .arg(&changed)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let v = json(&out);
    assert_eq!(v["data"]["planes"]["within_tolerance"], 1);
    // Metadata diff: JSON pointers with ours/theirs.
    let out = bin()
        .args(["check", "--json", "--no-pixels"])
        .arg(&tif)
        .arg("--against")
        .arg(&fcs)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let v = json(&out);
    let diffs = v["data"]["metadata"]["differences"].as_array().unwrap();
    assert!(!diffs.is_empty());
    assert!(
        diffs
            .iter()
            .all(|d| d["pointer"].as_str().unwrap().starts_with('/'))
    );
    // Errors keep their own exit codes.
    let out = bin()
        .args(["check"])
        .arg(&tif)
        .arg("--against")
        .arg(dir.join("missing.tif"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
}

#[test]
fn full_info_sidecars_next_to_inputs_and_under_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(data.join("sub")).unwrap();
    let fcs = tiny_fcs(&data.join("sub"));
    std::fs::write(data.join("notes.txt"), b"not an instrument file").unwrap();
    let run = |extra: &[&str]| {
        bin()
            .args(["info", "--view", "full", "-r", "--skip-unknown", "--jsonl"])
            .args(extra)
            .arg(&data)
            .output()
            .unwrap()
    };
    let lines = |out: &std::process::Output| -> Vec<serde_json::Value> {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let before = std::fs::read(&fcs).unwrap();
    let out = run(&["--sidecar"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = lines(&out);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0]["data"]["status"], "written");
    let sidecar = data.join("sub/tiny.fcs.openreadout.json");
    let doc = read_json(&sidecar);
    assert_eq!(doc["ok"], true);
    assert_eq!(doc["data"]["file"]["format"]["id"], "fcs");
    assert_eq!(doc["sidecar"]["options"]["vendor"], true);
    assert_eq!(
        std::fs::read(&fcs).unwrap(),
        before,
        "the input is never modified"
    );
    // unchanged the second time; the sidecar itself is not an input
    let v = lines(&run(&["--sidecar"]));
    assert_eq!(v.len(), 1);
    assert_eq!(v[0]["data"]["status"], "unchanged");
    // other options rewrite it
    let v = lines(&run(&["--sidecar", "--no-vendor"]));
    assert_eq!(v[0]["data"]["status"], "written");
    assert!(read_json(&sidecar)["data"].get("vendor").is_none());
    // --sidecar=DIR mirrors the tree
    let out_dir = dir.path().join("meta");
    let out = run(&[&format!("--sidecar={}", out_dir.to_str().unwrap())]);
    assert!(out.status.success());
    assert!(out_dir.join("sub/tiny.fcs.openreadout.json").exists());
    // -o is the batch table, not the full view; standard input is refused
    let out = bin()
        .args(["info", "--view", "full", "-o", "x"])
        .arg(&fcs)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = bin()
        .args(["info", "--view", "full", "--sidecar", "-"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
