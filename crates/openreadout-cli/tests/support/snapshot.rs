//! Golden-output snapshots, shared by `tests/golden.rs` (the committed fixtures, run by
//! `cargo test`) and the corpus harness (`openreadout-corpus-tests/tests/snapshots.rs`, every
//! development-corpus file on disk).
//!
//! A snapshot record is one compact JSON line per input: the format that claimed it, the error
//! code when it was refused, a readable projection of `info` (geometry, pixel types, trace,
//! spectrum and table shapes), an xxh3-128 of the whole `info` output but its evidence-dependent assurance block and confidence (so any change anywhere is
//! caught), the `check --headers-only` verdict with its finding codes, and the assurance level,
//! fingerprint and refused outputs. Records are committed; a test that finds a difference writes
//! the new records (and the full `info` of each input) under `target/tmp/snapshots/` and fails,
//! and `cargo xtask snapshot review` / `accept` show and take the difference
//! (docs/maintaining.md § Regression snapshots).

#![allow(dead_code)] // each including test uses part of it

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// Largest number of images, traces, runs or tables listed in a projection.
const MAX_LISTED: usize = 12;

/// The compact projection of an `info` payload (`data` of `info --json`).
pub fn project(info: &Value) -> Value {
    let mut p = serde_json::Map::new();
    if let Some(v) = info.get("format_version").filter(|v| !v.is_null()) {
        p.insert("format_version".into(), v.clone());
    }
    let list = |key: &str, f: &dyn Fn(&Value) -> String| -> Option<Value> {
        let a = info.get(key)?.as_array()?;
        if a.is_empty() {
            return None;
        }
        let mut out: Vec<Value> = a
            .iter()
            .take(MAX_LISTED)
            .map(|x| Value::from(f(x)))
            .collect();
        if a.len() > MAX_LISTED {
            out.push(Value::from(format!("+{} more", a.len() - MAX_LISTED)));
        }
        Some(Value::Array(out))
    };
    let s = |v: &Value, k: &str| -> String {
        match v.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => "-".into(),
            Some(x) => x.to_string(),
        }
    };
    if let Some(v) = list("images", &|i| {
        format!(
            "{}x{}x{}x{}x{} {} spp{} levels{} planes{}",
            s(i, "size_x"),
            s(i, "size_y"),
            s(i, "size_z"),
            s(i, "size_c"),
            s(i, "size_t"),
            s(i, "pixel_type"),
            s(i, "samples_per_pixel"),
            s(i, "pyramid_levels"),
            s(i, "plane_count"),
        )
    }) {
        p.insert("images".into(), v);
    }
    if let Some(n) = info.get("images_total") {
        p.insert("images_total".into(), n.clone());
    }
    if let Some(v) = list("traces", &|t| {
        format!(
            "{} sweeps x {} ch x {} samples @ {} Hz",
            s(t, "sweep_count"),
            t.get("channels")
                .and_then(Value::as_array)
                .map_or(0, Vec::len),
            s(t, "sample_count"),
            s(t, "sample_rate_hz"),
        )
    }) {
        p.insert("traces".into(), v);
    }
    if let Some(v) = list("spectra", &|r| {
        format!(
            "{} scans ms_levels {}",
            s(r, "scan_count"),
            s(r, "ms_levels")
        )
    }) {
        p.insert("spectra".into(), v);
    }
    if let Some(v) = list("tables", &|t| {
        format!(
            "{} rows x {} columns",
            s(t, "row_count"),
            t.get("columns")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        )
    }) {
        p.insert("tables".into(), v);
    }
    if let Some(n) = info.get("plane_count") {
        p.insert("plane_count".into(), n.clone());
    }
    Value::Object(p)
}

/// `info` output made independent of where the files are: `path` and the `acquisition` state
/// (which depends on the files' modification times) removed, `prefixes` (the corpus or
/// repository directory) replaced by `<root>` in every string.
pub fn normalized(info: &Value, prefixes: &[String]) -> Value {
    fn walk(v: &mut Value, prefixes: &[String]) {
        match v {
            Value::String(s) => {
                let mut t = s.replace('\\', "/");
                for p in prefixes {
                    t = t.replace(p.as_str(), "<root>");
                }
                *s = t;
            }
            // 12 significant digits: the platform's libm (powf, exp, ...) may differ in the last
            // bits, which is not a change in what the reader returns.
            Value::Number(n) if n.is_f64() => {
                if let Some(r) = n
                    .as_f64()
                    .and_then(|x| format!("{x:.11e}").parse::<f64>().ok())
                    .and_then(serde_json::Number::from_f64)
                {
                    *n = r;
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, prefixes)),
            Value::Object(m) => m.values_mut().for_each(|x| walk(x, prefixes)),
            _ => {}
        }
    }
    let mut v = info.clone();
    if let Some(m) = v.as_object_mut() {
        m.remove("path");
        m.remove("acquisition");
        // The corpus evidence behind the assurance block and the reader's confidence change
        // with every evidence refresh, not with what the reader returns: the record keeps the
        // assurance level, fingerprint and refused outputs instead.
        m.remove("assurance");
        if let Some(f) = m.get_mut("format").and_then(Value::as_object_mut) {
            f.remove("confidence");
        }
    }
    walk(&mut v, prefixes);
    v
}

/// xxh3-128 of the canonical JSON text of `v` (object keys are sorted by serde_json's map).
pub fn digest(v: &Value) -> String {
    format!(
        "{:032x}",
        xxhash_rust::xxh3::xxh3_128(serde_json::to_string(v).unwrap_or_default().as_bytes())
    )
}

/// The directory prefixes to replace by `<root>`, `/`-separated.
pub fn prefixes(dirs: &[&Path]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in dirs {
        for p in [
            d.to_path_buf(),
            d.canonicalize().unwrap_or_else(|_| d.to_path_buf()),
        ] {
            let s = p.display().to_string().replace('\\', "/");
            let s = s.trim_end_matches('/').to_string() + "/";
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.len()));
    out
}

/// One snapshot record. `info` is the `data` of `info --json` (or `None` with the error code),
/// `check` the `data` of `check --headers-only --json` (or `None` with its error code).
pub fn record(
    id: &str,
    format: Option<&str>,
    info: Result<&Value, &str>,
    check: Result<&Value, &str>,
    prefixes: &[String],
) -> Value {
    let mut r = serde_json::Map::new();
    r.insert("id".into(), Value::from(id));
    r.insert("format".into(), format.map_or(Value::Null, Value::from));
    match info {
        Ok(i) => {
            let n = normalized(i, prefixes);
            r.insert("info".into(), project(&n));
            r.insert("info_xxh3".into(), Value::from(digest(&n)));
            if let Some(a) = i.get("assurance").filter(|a| a.is_object()) {
                r.insert(
                    "assurance".into(),
                    json!({
                        "level": a["level"],
                        "fingerprint": a["fingerprint"],
                        "strict_refuses": a.get("strict_refuses").cloned().unwrap_or(json!([])),
                    }),
                );
            }
        }
        Err(code) => {
            r.insert("error".into(), Value::from(code));
        }
    }
    match check {
        Ok(c) => {
            let codes: Vec<String> = c["findings"]
                .as_array()
                .map(|f| {
                    f.iter()
                        .map(|x| {
                            format!(
                                "{}:{}",
                                x["severity"].as_str().unwrap_or_default(),
                                x["code"].as_str().unwrap_or_default()
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            r.insert("check".into(), json!({"ok": c["ok"], "findings": codes}));
        }
        Err(code) => {
            r.insert("check".into(), json!({"error": code}));
        }
    }
    Value::Object(r)
}

/// Records by id from JSON Lines text.
pub fn parse(text: &str) -> BTreeMap<String, Value> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| Some((v["id"].as_str()?.to_string(), v)))
        .collect()
}

/// JSON Lines text of `records`, sorted by id.
pub fn render(records: &BTreeMap<String, Value>) -> String {
    let mut s = String::new();
    for v in records.values() {
        s.push_str(&serde_json::to_string(v).unwrap_or_default());
        s.push('\n');
    }
    s
}

/// Field-level differences between two records (`key: old -> new`), for the failure message.
#[allow(clippy::many_single_char_names)]
pub fn diff(old: &Value, new: &Value) -> Vec<String> {
    fn walk(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
        if a == b {
            return;
        }
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
                for k in keys {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(
                        &p,
                        x.get(k).unwrap_or(&Value::Null),
                        y.get(k).unwrap_or(&Value::Null),
                        out,
                    );
                }
            }
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                for (i, (p, q)) in x.iter().zip(y).enumerate() {
                    walk(&format!("{path}[{i}]"), p, q, out);
                }
            }
            _ => out.push(format!("{path}: {a} -> {b}")),
        }
    }
    let mut out = Vec::new();
    walk("", old, new, &mut out);
    out
}

/// Where fresh records and full outputs go: `target/tmp/snapshots/<kind>`.
pub fn out_dir(kind: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("snapshots")
        .join(kind)
}

/// Compare `fresh` with the committed records, write the fresh records under `out`, and return
/// one message per difference: a changed record, a new input, and with `complete` (every input
/// is always present, as for committed fixtures) a committed record whose input was not read.
/// With `SNAPSHOT_ACCEPT=1` the committed file is rewritten instead (without `complete`, records
/// of inputs absent this run are kept).
pub fn compare(
    committed_path: &Path,
    fresh: &BTreeMap<String, Value>,
    out: &Path,
    name: &str,
    complete: bool,
) -> Vec<String> {
    let committed = std::fs::read_to_string(committed_path)
        .map(|t| parse(&t))
        .unwrap_or_default();
    let _ = std::fs::create_dir_all(out);
    let _ = std::fs::write(out.join(format!("{name}.jsonl")), render(fresh));
    let accept = std::env::var("SNAPSHOT_ACCEPT").is_ok_and(|v| v == "1");
    let mut problems = Vec::new();
    for (id, new) in fresh {
        match committed.get(id) {
            Some(old) if old == new => {}
            Some(old) => {
                let d = diff(old, new);
                problems.push(format!("{id}: {}", d.join("; ")));
            }
            None => problems.push(format!("{id}: new input (no committed snapshot)")),
        }
    }
    if complete {
        for id in committed.keys().filter(|id| !fresh.contains_key(*id)) {
            problems.push(format!(
                "{id}: committed snapshot, but the input was not read"
            ));
        }
    }
    if accept && !problems.is_empty() {
        let mut merged = if complete {
            BTreeMap::new()
        } else {
            committed.clone()
        };
        for (id, v) in fresh {
            merged.insert(id.clone(), v.clone());
        }
        if let Some(p) = committed_path.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        std::fs::write(committed_path, render(&merged)).expect("write snapshots");
        println!(
            "SNAPSHOT_ACCEPT=1: rewrote {} ({} records changed or added)",
            committed_path.display(),
            problems.len()
        );
        return Vec::new();
    }
    problems
}
