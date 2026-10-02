//! `watch`, and `info`/`check`/`planes` on a file still being written, through the binary.
//! Synthetic OME-TIFFs are replayed with `openreadout_live::replay` (no corpus needed).
//! `watch_latency` measures how long a new plane takes to reach `watch`'s stdout.
#![allow(clippy::many_single_char_names)]

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use openreadout_live::replay::{Pattern, Replayer, plan};
use serde_json::Value;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_openreadout"))
}

fn run(args: &[&str]) -> (i32, String) {
    let o = bin().args(args).output().unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).to_string(),
    )
}

fn lines(out: &str) -> Vec<Value> {
    out.lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect()
}

fn kinds(ev: &[Value]) -> Vec<String> {
    ev.iter()
        .map(|e| e["data"]["event"].as_str().unwrap().to_string())
        .collect()
}

/// A replay of a synthetic OME-TIFF with `pages` pages into `dst`, stopped after `units` pages.
fn partial(dir: &Path, dst: &Path, pages: u32, units: u64) -> Replayer {
    let src = dir.join(format!("src-{pages}.ome.tif"));
    std::fs::write(&src, openreadout_live::synth::ome_tiff(32, 16, pages)).unwrap();
    let p = plan(&src, Pattern::OmeTiff, false).unwrap();
    let last = p.step_after_units(units).unwrap();
    let mut r = Replayer::new(p, dst).unwrap();
    r.run_through(last).unwrap();
    r
}

#[test]
fn growing_file_is_in_progress_not_corrupt() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("run.ome.tif");
    let mut r = partial(d.path(), &f, 5, 3);
    let fs = f.to_str().unwrap();
    let (code, out) = run(&["info", fs, "--json"]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    let a = &v["data"]["acquisition"];
    assert_eq!(a["state"], "in_progress", "{v}");
    assert_eq!(a["complete_planes"], 3);
    assert_eq!(a["missing"][0], "OME-XML");
    let (code, out) = run(&["check", fs, "--json"]);
    assert_eq!(code, 0, "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"]["ok"], true);
    assert_eq!(v["data"]["findings"][0]["code"], "acquisition_in_progress");
    let (code, out) = run(&["check", "--planes", fs, "--json"]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"]["planes"].as_array().unwrap().len(), 3);
    // `--live-window 0`: the same file is an interrupted acquisition.
    let (_, out) = run(&["info", fs, "--json", "--live-window", "0"]);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["data"]["acquisition"]["state"], "interrupted");
    // Finished: no acquisition block, OME metadata applies.
    r.finish().unwrap();
    let (code, out) = run(&["info", fs, "--json"]);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert!(v["data"].get("acquisition").is_none());
    assert_eq!(v["data"]["images"][0]["size_t"], 5);
}

#[test]
fn print_qc_rules_needs_no_directory() {
    let (code, out) = run(&["watch", "--print-qc-rules"]);
    assert_eq!(code, 0);
    assert!(out.contains('['), "expected TOML rules, got {out:?}");
    // Without `--print-qc-rules` a directory is still required (usage error).
    let (code, _) = run(&["watch"]);
    assert_eq!(code, 2);
}

#[test]
fn watch_once_reports_planes_and_state() {
    let d = tempfile::tempdir().unwrap();
    let inbox = d.path().join("inbox");
    std::fs::create_dir(&inbox).unwrap();
    let _growing = partial(d.path(), &inbox.join("growing.ome.tif"), 6, 2);
    let mut done = partial(d.path(), &inbox.join("done.ome.tif"), 3, 1);
    done.finish().unwrap();
    std::fs::write(inbox.join("notes.txt"), "not an instrument file").unwrap();
    let (code, out) = run(&["watch", inbox.to_str().unwrap(), "--once"]);
    assert_eq!(code, 0);
    let ev = lines(&out);
    assert!(ev.iter().all(|e| e["ok"] == true && e["path"].is_string()));
    let by = |name: &str| -> Vec<&Value> {
        ev.iter()
            .filter(|e| e["path"].as_str().unwrap().ends_with(name))
            .collect()
    };
    let g = by("growing.ome.tif");
    assert_eq!(
        kinds(&g.iter().map(|v| (*v).clone()).collect::<Vec<_>>()),
        ["dataset_new", "plane_new", "plane_new"]
    );
    assert_eq!(g[0]["data"]["state"], "in_progress");
    assert_eq!(g[0]["data"]["format"], "tiff");
    let k = kinds(
        &by("done.ome.tif")
            .iter()
            .map(|v| (*v).clone())
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        k,
        [
            "dataset_new",
            "plane_new",
            "plane_new",
            "plane_new",
            "dataset_complete"
        ]
    );
    assert!(by("notes.txt").is_empty(), "unknown formats are ignored");
    // A missing directory is an I/O error (exit 5).
    let (code, _) = run(&["watch", d.path().join("nope").to_str().unwrap(), "--once"]);
    assert_eq!(code, 5);
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        * 1000.0
}

/// Latency from a page hitting disk to its `plane_new` line reaching a reader of `watch`'s
/// stdout (poll interval 0.25 s, the default). Prints the numbers (`--nocapture`).
#[test]
fn watch_latency() {
    let d = tempfile::tempdir().unwrap();
    let inbox = d.path().join("inbox");
    std::fs::create_dir(&inbox).unwrap();
    let src = d.path().join("src.ome.tif");
    let pages = 12u32;
    std::fs::write(&src, openreadout_live::synth::ome_tiff(256, 256, pages)).unwrap();
    let mut child = bin()
        .args(["watch", inbox.to_str().unwrap(), "--timeout", "30"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<(f64, Value)>();
    let reader = std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines() {
            let Ok(l) = l else { break };
            let v: Value = serde_json::from_str(&l).unwrap();
            if tx.send((now_ms(), v)).is_err() {
                break;
            }
        }
    });
    std::thread::sleep(Duration::from_millis(400)); // let the watcher start
    let mut r = Replayer::new(
        plan(&src, Pattern::OmeTiff, false).unwrap(),
        &inbox.join("run.ome.tif"),
    )
    .unwrap();
    let mut written = Vec::new();
    while let Some(step) = r.step().unwrap().cloned() {
        if step.unit.is_some() {
            written.push(now_ms());
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen: Vec<Option<f64>> = vec![None; pages as usize];
    let mut complete = false;
    while Instant::now() < deadline && !complete {
        if let Ok((at, v)) = rx.recv_timeout(Duration::from_millis(200)) {
            let data = &v["data"];
            match data["event"].as_str() {
                Some("plane_new") => {
                    let i = data["index"].as_u64().unwrap() as usize;
                    if i < seen.len() && seen[i].is_none() {
                        seen[i] = Some(at);
                    }
                }
                Some("dataset_complete") => complete = true,
                _ => {}
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    assert!(complete, "dataset_complete after the replay");
    let mut lat: Vec<f64> = seen
        .iter()
        .zip(&written)
        .map(|(s, w)| s.expect("every plane reported") - w)
        .collect();
    lat.sort_by(f64::total_cmp);
    let p50 = lat[lat.len() / 2];
    let max = lat[lat.len() - 1];
    println!(
        "watch latency over {} planes (poll 0.25 s): min {:.0} ms, median {p50:.0} ms, max {max:.0} ms",
        lat.len(),
        lat[0]
    );
    // The target is 1 s; allow slack for loaded CI machines.
    assert!(max < 2000.0, "max latency {max:.0} ms");
}
