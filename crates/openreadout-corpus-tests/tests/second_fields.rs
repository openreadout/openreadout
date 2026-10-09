//! Second opinions on normalized fields and key values, for every family (docs/assurance.md,
//! "differential cross-checks").
//!
//! `oracle/second_fields.py` runs a reader independent of OpenReadout — and, where one exists,
//! independent of the primary oracle too (pyopenms and SQLite beside pyteomics and timsrust,
//! neo beside pyabf, pynwb beside h5py, fcsparser beside FlowIO, Aston beside rainbow-api, a
//! vendor export beside a community reader, ...) — on every development file of a family and
//! writes `corpus/oracle/second/fields/<id>.json`: a list of checks. Each check names a field,
//! the OpenReadout command whose JSON output holds our value (`cmd`, run as
//! `openreadout <cmd[0]> <file> <cmd[1..]> --json`), where in that output (`ours`, a path, below)
//! and the second reader's value (`theirs`), with how to compare them (`cmp`). The harness is
//! family-generic: a new reader plugs in by emitting checks; nothing here knows formats.
//!
//! A `cmd` argument `@<path>` is replaced by the value at that path in our `info` output, so a
//! second reader can name the trace it read (`--trace @/traces/[file~=FID1A.ch]/index`).
//!
//! Paths into our output (`ours`) are JSON pointers into the envelope's `data`, with three
//! extensions: a segment `[key=value]` selects the first array element whose `key` equals
//! `value` (as text; `[key~=value]` ignores case), a segment `*` maps over every element of an
//! array (`{key=value}` or `{key!=value}` over the elements that match), and a final `#len`, `#sum`, `#min`, `#max`, `#first`, `#last` or `#set` (sorted distinct
//! values) reduces the value found. With `at` (indices), the check compares only those elements
//! of an array value.
//!
//! Comparisons (`cmp`): `exact` (JSON equality, numbers as f64), `num` (|ours − theirs| ≤
//! `abs` + `rel`·|theirs|, element by element for arrays), `text` (trimmed, whitespace collapsed,
//! case ignored), `time` (timestamps: instants when both carry a UTC offset, wall-clock times
//! otherwise, within `abs` seconds, default 1; `zone: true` also requires the same offset),
//! `wallclock` (the clock times as written, offsets ignored: a local time against a local time),
//! `time_mod_zone` (the same clock reading up to whole quarter hours: a second reader whose time
//! zone cannot be trusted), `time_or_utc` (a zoneless second reader that writes either the local
//! clock or UTC: the same wall clock, or ours in UTC),
//! `set` (the same distinct texts in any order), `prefix` (theirs starts with ours or ours with
//! theirs, as text), `loose` (letters and digits only, case ignored, `µ` as `u`: a second reader
//! that re-spells names, as Neo does).
//!
//! A difference fails the test until `corpus/oracle/second/adjudications.toml` has a `[[field]]`
//! entry whose `ids` and `fields` (glob patterns) cover it, saying which reader is right
//! (`openreadout`, `second`, `neither`) and why. A value only the second reader reports is a gap,
//! listed but not failed (unless the check sets `required`).
//!
//! With `SECOND_RESULTS=<file>` the test writes per-file results in the corpus-results format
//! that `cargo xtask assurance-audit refresh --results` reads (a file whose checks agree counts
//! as confirmed by an independent reader for the scopes compared; a difference adjudicated
//! against OpenReadout is a failure); with `SECOND_REPORT=<file>` one line per check for
//! `oracle/second_report.py`.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test second_fields -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR`, `CORPUS_ONLY=<substring>` (ids), `SECOND_FAMILY=<family>[,<family>...]`.
#![cfg(feature = "corpus")]
#![allow(
    clippy::too_many_lines,
    clippy::float_cmp,
    clippy::many_single_char_names
)] // comparison code: short names for dates, times and offsets

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;

/// Per file: failed, scopes confirmed, normalized fields confirmed / contradicted.
type FileResult = (bool, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>);

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    #[serde(default)]
    format: String,
    filename: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    tier: String,
}

#[derive(Deserialize)]
struct Record {
    id: String,
    #[serde(default)]
    format: String,
    #[serde(default)]
    family: String,
    #[serde(default)]
    readers: Vec<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    checks: Vec<Check>,
}

#[derive(Deserialize, Clone)]
struct Check {
    field: String,
    #[serde(default = "metadata")]
    scope: String,
    #[serde(default = "info_cmd")]
    cmd: Vec<String>,
    ours: String,
    theirs: Value,
    #[serde(default = "exact")]
    cmp: String,
    #[serde(default)]
    rel: f64,
    #[serde(default)]
    abs: Option<f64>,
    #[serde(default)]
    zone: bool,
    #[serde(default)]
    at: Option<Vec<usize>>,
    /// With `each`: `ours` is an array of records, `theirs` an object keyed by the text of each
    /// record's `by` value; each record's `each` value is compared with `theirs[key]`.
    #[serde(default)]
    by: Option<String>,
    #[serde(default)]
    each: Option<String>,
    #[serde(default)]
    required: bool,
    /// Index into `readers` (default 0).
    #[serde(default)]
    reader: usize,
}

fn metadata() -> String {
    "metadata".into()
}
fn info_cmd() -> Vec<String> {
    vec!["info".into()]
}
fn exact() -> String {
    "exact".into()
}

#[derive(Deserialize, Default)]
struct Adjudications {
    #[serde(default)]
    field: Vec<FieldAdjudication>,
}
#[derive(Deserialize)]
struct FieldAdjudication {
    /// Glob patterns over corpus ids.
    ids: Vec<String>,
    /// Glob patterns over field names (default: every field).
    #[serde(default)]
    fields: Vec<String>,
    right: String,
    why: String,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The CLI binary next to this test executable, (re)built first (as in `malformed.rs`).
fn cli_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
    let bin = profile_dir.join(format!("openreadout{}", std::env::consts::EXE_SUFFIX));
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(root()).args(["build", "-p", "openreadout"]);
    // `target/debug` is the dev profile; every other directory is named after its profile.
    match profile_dir.file_name().and_then(|n| n.to_str()) {
        Some("debug") | None => {}
        Some(profile) => {
            cmd.args(["--profile", profile]);
        }
    }
    assert!(
        cmd.status().expect("run cargo build").success(),
        "building openreadout failed"
    );
    assert!(bin.exists(), "CLI binary not found at {}", bin.display());
    bin
}

/// Resolve an `ours` path (see the module docs) against a value.
fn resolve(data: &Value, path: &str) -> Option<Value> {
    let (path, reduce) = match path.rsplit_once('#') {
        Some((p, r)) => (p, Some(r)),
        None => (path, None),
    };
    let mut cur: Vec<Value> = vec![data.clone()];
    let mut mapped = false;
    for seg in path.split('/').skip(1) {
        let seg = seg.replace("~1", "/").replace("~0", "~");
        let mut next = Vec::new();
        for v in cur {
            if seg == "*" {
                if let Value::Array(a) = v {
                    next.extend(a);
                }
                continue;
            }
            // `{key=value}` / `{key!=value}`: every element that matches (like `*`, filtered)
            if let Some(sel) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                let (key, want, negate) = if let Some((k, w)) = sel.split_once("!=") {
                    (k, w, true)
                } else if let Some((k, w)) = sel.split_once('=') {
                    (k, w, false)
                } else {
                    return None;
                };
                if let Value::Array(a) = v {
                    next.extend(a.into_iter().filter(|x| {
                        let t = field_at(x, key).map(as_text).unwrap_or_default();
                        (t == want) != negate
                    }));
                }
                continue;
            }
            if let Some(sel) = seg.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                // `=` equal, `~=` equal ignoring case, `$=` ends with, `*=` contains (both
                // ignoring case); the key may be a dotted path (`extra.file`)
                let (key, want, op) = ["~=", "$=", "*=", "="]
                    .iter()
                    .find_map(|op| sel.split_once(op).map(|(k, w)| (k, w, *op)))?;
                let field = |x: &Value| -> Option<String> { field_at(x, key).map(as_text) };
                let (lw, want) = (want.to_lowercase(), want);
                if let Value::Array(a) = v
                    && let Some(x) = a.into_iter().find(|x| {
                        field(x).is_some_and(|t| match op {
                            "=" => t == want,
                            "~=" => t.to_lowercase() == lw,
                            "$=" => t.to_lowercase().ends_with(&lw),
                            _ => t.to_lowercase().contains(&lw),
                        })
                    })
                {
                    next.push(x);
                }
                continue;
            }
            let got = match &v {
                Value::Object(m) => m.get(&seg).cloned(),
                Value::Array(a) => seg.parse::<usize>().ok().and_then(|i| a.get(i).cloned()),
                _ => None,
            };
            if let Some(g) = got {
                next.push(g);
            }
        }
        if seg == "*" || seg.starts_with('{') {
            mapped = true;
        }
        cur = next;
    }
    let value = if mapped {
        Value::Array(cur)
    } else {
        cur.into_iter().next()?
    };
    match reduce {
        None => Some(value),
        Some(r) => reduce_value(&value, r),
    }
}

fn reduce_value(v: &Value, how: &str) -> Option<Value> {
    let arr = match v {
        Value::Array(a) => a.clone(),
        Value::Object(m) if how == "len" => return Some(Value::from(m.len())),
        Value::String(s) if how == "len" => return Some(Value::from(s.chars().count())),
        _ => return None,
    };
    let nums = || arr.iter().filter_map(Value::as_f64);
    Some(match how {
        "len" => Value::from(arr.len()),
        "sum" => Value::from(nums().sum::<f64>()),
        "min" => Value::from(nums().fold(f64::INFINITY, f64::min)),
        "max" => Value::from(nums().fold(f64::NEG_INFINITY, f64::max)),
        "first" => arr.first()?.clone(),
        "last" => arr.last()?.clone(),
        "set" => {
            let s: BTreeSet<String> = arr.iter().map(as_text).collect();
            Value::Array(s.into_iter().map(Value::from).collect())
        }
        _ => return None,
    })
}

/// Text of a value; an integral number prints without a fraction (`3.0` as `3`), so records
/// keyed by a number match whether it was written as an integer or a float.
fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) => match n.as_f64() {
            #[allow(clippy::cast_possible_truncation)]
            Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// A dotted key (`extra.file`) inside an object or array (`3` indexes an array).
fn field_at<'a>(x: &'a Value, key: &str) -> Option<&'a Value> {
    let mut cur = x;
    for part in key.split('.') {
        cur = match cur {
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            other => other.get(part)?,
        };
    }
    Some(cur)
}

fn norm_text(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Letters and digits only, lower case, `µ`/`μ` as `u`: for names a second reader re-spells
/// (Neo drops the spaces of `IN 0` and writes `uA` for `µA`).
fn loose_text(s: &str) -> String {
    s.chars()
        .map(|c| if c == 'µ' || c == 'μ' { 'u' } else { c })
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Seconds since 1970 (as if UTC) and the offset in seconds when the text carries one.
fn parse_time(s: &str) -> Option<(f64, Option<i64>)> {
    let s = s.trim();
    let (date, rest) = s.split_at(s.find(['T', ' ']).unwrap_or(s.len()));
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let mo: i64 = d.next()?.parse().ok()?;
    let da: i64 = d.next()?.parse().ok()?;
    let rest = rest.trim_start_matches(['T', ' ']);
    let (clock, zone) = if let Some(c) = rest.strip_suffix('Z') {
        (c, Some(0))
    } else if let Some(i) = rest.rfind(['+', '-']).filter(|&i| i > 0) {
        let z = &rest[i..];
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let z = z[1..].replace(':', "");
        let (h, m) = (
            z.get(0..2)?.parse::<i64>().ok()?,
            z.get(2..4).unwrap_or("0").parse::<i64>().ok()?,
        );
        (&rest[..i], Some(sign * (h * 3600 + m * 60)))
    } else {
        (rest, None)
    };
    let mut c = clock.split(':');
    let h: f64 = c
        .next()
        .filter(|x| !x.is_empty())
        .unwrap_or("0")
        .parse()
        .ok()?;
    let mi: f64 = c.next().unwrap_or("0").parse().ok()?;
    let se: f64 = c.next().unwrap_or("0").parse().ok()?;
    // days from civil (Howard Hinnant)
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    #[allow(clippy::cast_precision_loss)]
    let t = days as f64 * 86400.0 + h * 3600.0 + mi * 60.0 + se;
    Some((t, zone))
}

fn num_close(a: f64, b: f64, rel: f64, abs: f64) -> bool {
    (a.is_nan() && b.is_nan()) || a == b || (a - b).abs() <= abs + rel * b.abs()
}

/// Compare one value; `Err` says how they differ.
fn compare(c: &Check, ours: &Value, theirs: &Value) -> Result<(), String> {
    if let (Value::Array(o), Value::Array(t)) = (ours, theirs)
        && c.cmp != "set"
    {
        if o.len() != t.len() {
            return Err(format!("{} values (ours) vs {}", o.len(), t.len()));
        }
        let mut bad = Vec::new();
        for (i, (a, b)) in o.iter().zip(t).enumerate() {
            if let Err(e) = compare(c, a, b) {
                bad.push(format!("[{i}] {e}"));
            }
        }
        return if bad.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "{} of {} differ: {}",
                bad.len(),
                t.len(),
                bad.into_iter().take(3).collect::<Vec<_>>().join("; ")
            ))
        };
    }
    let show = |v: &Value| {
        let s = v.to_string();
        if s.len() > 120 {
            format!("{}…", &s[..s.floor_char_boundary(120)])
        } else {
            s
        }
    };
    let ok = match c.cmp.as_str() {
        "exact" => json_eq(ours, theirs),
        "num" => match (as_num(ours), as_num(theirs)) {
            (Some(a), Some(b)) => num_close(a, b, c.rel, c.abs.unwrap_or(0.0)),
            _ => json_eq(ours, theirs),
        },
        "text" => norm_text(&as_text(ours)) == norm_text(&as_text(theirs)),
        "loose" => loose_text(&as_text(ours)) == loose_text(&as_text(theirs)),
        // one name inside the other, letters and digits only (a model code such as `STA449F3`
        // inside a product name such as `NETZSCH STA 449 F3 Jupiter`)
        "contains" => {
            let (a, b) = (loose_text(&as_text(ours)), loose_text(&as_text(theirs)));
            !a.is_empty() && !b.is_empty() && (a.contains(&b) || b.contains(&a))
        }
        "prefix" => {
            let (a, b) = (norm_text(&as_text(ours)), norm_text(&as_text(theirs)));
            !a.is_empty() && !b.is_empty() && (a.starts_with(&b) || b.starts_with(&a))
        }
        "set" => {
            let set = |v: &Value| -> BTreeSet<String> {
                match v {
                    Value::Array(a) => a.iter().map(|x| norm_text(&as_text(x))).collect(),
                    other => std::iter::once(norm_text(&as_text(other))).collect(),
                }
            };
            set(ours) == set(theirs)
        }
        // the same clock reading up to a time-zone offset (whole quarter hours): for a second
        // reader whose zone is unreliable (converters that write the local clock with a `Z`)
        "time_mod_zone" => match (parse_time(&as_text(ours)), parse_time(&as_text(theirs))) {
            (Some((a, _)), Some((b, _))) => {
                let d = (a - b).rem_euclid(900.0);
                d.min(900.0 - d) <= c.abs.unwrap_or(1.0) && (a - b).abs() <= 14.0 * 3600.0 + 1.0
            }
            _ => false,
        },
        // a second reader that writes a time without a zone, sometimes the local clock and
        // sometimes UTC (Bio-Formats' OME-XML `AcquisitionDate`): the same wall clock, or ours
        // converted to UTC
        "time_or_utc" => match (parse_time(&as_text(ours)), parse_time(&as_text(theirs))) {
            (Some((a, za)), Some((b, zb))) => {
                let tol = c.abs.unwrap_or(1.0);
                #[allow(clippy::cast_precision_loss)]
                match (za, zb) {
                    (Some(x), Some(y)) => ((a - x as f64) - (b - y as f64)).abs() <= tol,
                    (Some(x), None) => (a - b).abs() <= tol || ((a - x as f64) - b).abs() <= tol,
                    (None, Some(y)) => (a - b).abs() <= tol || (a - (b - y as f64)).abs() <= tol,
                    (None, None) => (a - b).abs() <= tol,
                }
            }
            _ => false,
        },
        "wallclock" => match (parse_time(&as_text(ours)), parse_time(&as_text(theirs))) {
            (Some((a, _)), Some((b, _))) => (a - b).abs() <= c.abs.unwrap_or(1.0),
            _ => false,
        },
        "time" => match (parse_time(&as_text(ours)), parse_time(&as_text(theirs))) {
            (Some((a, za)), Some((b, zb))) => {
                let tol = c.abs.unwrap_or(1.0);
                #[allow(clippy::cast_precision_loss)]
                let same = match (za, zb) {
                    (Some(x), Some(y)) => ((a - x as f64) - (b - y as f64)).abs() <= tol,
                    _ => (a - b).abs() <= tol,
                };
                same && (!c.zone || za == zb)
            }
            _ => false,
        },
        other => return Err(format!("unknown comparison {other}")),
    };
    if ok {
        Ok(())
    } else {
        Err(format!("{} (ours) vs {}", show(ours), show(theirs)))
    }
}

/// A number, or text that parses as one (vendor keywords are often kept as text).
fn as_num(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| json_eq(v, w)))
        }
        _ => a == b,
    }
}

#[derive(Clone)]
struct Outcome {
    id: String,
    format: String,
    family: String,
    field: String,
    scope: String,
    reader: String,
    /// `agree`, `differ`, `gap` (theirs only), `error` (our command failed).
    status: String,
    detail: String,
    ours: Value,
    theirs: Value,
    adjudication: Option<(String, String)>,
    /// The normalized experiment field the check compares (`acquisition.started_at`), when its
    /// path is one (`/experiment/...` of `info`).
    normalized: Option<String>,
}

/// `/experiment/acquisition/started_at` of `info` → `acquisition.started_at`.
fn normalized_field(cmd: &[String], ours: &str) -> Option<String> {
    (cmd.first().map(String::as_str) == Some("info"))
        .then(|| ours.strip_prefix("/experiment/"))
        .flatten()
        .filter(|p| !p.contains('#') && !p.contains('[') && !p.contains('{'))
        .map(|p| p.replace('/', "."))
}

fn glob_match(pattern: &str, text: &str) -> bool {
    glob::Pattern::new(pattern).is_ok_and(|p| p.matches(text))
}

fn run_cmd(bin: &Path, path: &Path, cmd: &[String]) -> Result<Value, String> {
    let mut args: Vec<String> = vec![cmd[0].clone(), path.to_string_lossy().into_owned()];
    args.extend(cmd[1..].iter().cloned());
    args.push("--json".into());
    let out = Command::new(bin)
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| {
        format!(
            "{}: not JSON ({e}): {}",
            cmd.join(" "),
            String::from_utf8_lossy(&out.stderr)
                .chars()
                .take(200)
                .collect::<String>()
        )
    })?;
    if v.get("ok") == Some(&Value::Bool(true)) {
        Ok(v.get("data").cloned().unwrap_or(Value::Null))
    } else {
        Err(format!(
            "{}: {}",
            cmd.join(" "),
            v.get("error").map(Value::to_string).unwrap_or_default()
        ))
    }
}

#[test]
fn second_opinion_fields_agree() {
    let root = root();
    let dir = root.join("corpus/oracle/second/fields");
    let Ok(read) = std::fs::read_dir(&dir) else {
        return;
    };
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    // An input and its depositor export can share an id: the input is the file compared.
    let entries: HashMap<&str, &Entry> = manifest
        .file
        .iter()
        .filter(|e| e.role == "input" || e.role.is_empty())
        .map(|e| (e.id.as_str(), e))
        .collect();
    let adjudications: Adjudications =
        std::fs::read_to_string(root.join("corpus/oracle/second/adjudications.toml"))
            .map(|t| toml::from_str(&t).unwrap())
            .unwrap_or_default();
    for a in &adjudications.field {
        assert!(
            ["openreadout", "second", "neither"].contains(&a.right.as_str()) && !a.why.is_empty(),
            "{:?}: a field adjudication needs right = openreadout|second|neither and a why",
            a.ids
        );
    }
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let family = std::env::var("SECOND_FAMILY").ok();
    let mut paths: Vec<PathBuf> = read
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    let mut records = Vec::new();
    let problems = Mutex::new(Vec::<String>::new());
    for p in &paths {
        let r: Record = match serde_json::from_str(&std::fs::read_to_string(p).unwrap()) {
            Ok(r) => r,
            Err(e) => {
                problems
                    .lock()
                    .unwrap()
                    .push(format!("{}: {e}", p.display()));
                continue;
            }
        };
        if only.as_ref().is_some_and(|o| !r.id.contains(o.as_str()))
            || family
                .as_ref()
                .is_some_and(|f| !f.split(',').any(|f| f == r.family))
            || r.error.is_some()
        {
            continue;
        }
        let Some(e) = entries.get(r.id.as_str()) else {
            problems
                .lock()
                .unwrap()
                .push(format!("{}: not in the manifest", r.id));
            continue;
        };
        assert!(
            e.tier != "heldout" && e.role != "heldout",
            "{}: second opinions run on development files only",
            r.id
        );
        let path = files_dir.join(&e.filename);
        if path.exists() {
            records.push((r, path));
        }
    }
    if records.is_empty() {
        return;
    }
    let bin = cli_binary();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    let outcomes: Vec<Outcome> = pool.install(|| {
        records
            .par_iter()
            .flat_map_iter(|(r, path)| {
                let mut cache: HashMap<Vec<String>, Result<Value, String>> = HashMap::new();
                let mut out = Vec::new();
                for c in &r.checks {
                    // An argument `@<path>` is replaced by the value at that path in our `info`
                    // (e.g. `--trace @/traces/[file~=FID1A.ch]/index`): the second reader names
                    // a trace by what it is, not by our index.
                    let mut cmd = c.cmd.clone();
                    let mut unresolved = None;
                    if cmd.iter().any(|a| a.starts_with('@')) {
                        let info_cmd = vec!["info".to_string()];
                        let info = cache
                            .entry(info_cmd.clone())
                            .or_insert_with(|| run_cmd(&bin, path, &info_cmd))
                            .clone();
                        for a in &mut cmd {
                            if let Some(p) = a.strip_prefix('@') {
                                match info.as_ref().ok().and_then(|d| resolve(d, p)) {
                                    Some(v) => *a = as_text(&v),
                                    None => unresolved = Some(format!("nothing at {p} in info")),
                                }
                            }
                        }
                    }
                    let got = match unresolved {
                        Some(e) => Err(e),
                        None => cache
                            .entry(cmd.clone())
                            .or_insert_with(|| run_cmd(&bin, path, &cmd))
                            .clone(),
                    };
                    let mut o = Outcome {
                        id: r.id.clone(),
                        format: if r.format.is_empty() {
                            entries
                                .get(r.id.as_str())
                                .map(|e| e.format.clone())
                                .unwrap_or_default()
                        } else {
                            r.format.clone()
                        },
                        family: r.family.clone(),
                        field: c.field.clone(),
                        scope: c.scope.clone(),
                        reader: r.readers.get(c.reader).cloned().unwrap_or_default(),
                        status: String::new(),
                        detail: String::new(),
                        ours: Value::Null,
                        theirs: c.theirs.clone(),
                        adjudication: None,
                        normalized: normalized_field(&c.cmd, &c.ours),
                    };
                    match got {
                        Err(e) => {
                            o.status = "error".into();
                            o.detail = e;
                        }
                        Ok(data) => match resolve(&data, &c.ours) {
                            None | Some(Value::Null) => {
                                o.status = "gap".into();
                                o.detail = format!("ours has nothing at {}", c.ours);
                            }
                            Some(mut v) => {
                                let mut theirs = c.theirs.clone();
                                if let (Some(by), Value::Array(a), Value::Object(t)) =
                                    (&c.by, &v, &c.theirs)
                                {
                                    let each = c.each.as_deref().unwrap_or("");
                                    let mine: HashMap<String, Value> = a
                                        .iter()
                                        .filter_map(|x| {
                                            let k = resolve(x, by)?;
                                            Some((
                                                as_text(&k),
                                                resolve(x, each).unwrap_or(Value::Null),
                                            ))
                                        })
                                        .collect();
                                    let keys: Vec<&String> = t.keys().collect();
                                    v = Value::Array(
                                        keys.iter()
                                            .map(|k| mine.get(*k).cloned().unwrap_or(Value::Null))
                                            .collect(),
                                    );
                                    theirs = Value::Array(t.values().cloned().collect());
                                } else if let (Some(at), Value::Array(a)) = (&c.at, &v) {
                                    v = Value::Array(
                                        at.iter()
                                            .map(|&i| a.get(i).cloned().unwrap_or(Value::Null))
                                            .collect(),
                                    );
                                    if !theirs.is_array() {
                                        theirs = Value::Array(vec![theirs]);
                                    }
                                }
                                match compare(c, &v, &theirs) {
                                    Ok(()) => o.status = "agree".into(),
                                    Err(d) => {
                                        o.status = "differ".into();
                                        o.detail = d;
                                    }
                                }
                                o.ours = v;
                            }
                        },
                    }
                    if o.status == "gap" && c.required {
                        o.status = "differ".into();
                    }
                    if o.status == "differ" || o.status == "error" {
                        o.adjudication = adjudications
                            .field
                            .iter()
                            .find(|a| {
                                a.ids.iter().any(|p| glob_match(p, &o.id))
                                    && (a.fields.is_empty()
                                        || a.fields.iter().any(|p| glob_match(p, &o.field)))
                            })
                            .map(|a| (a.right.clone(), a.why.clone()));
                    }
                    out.push(o);
                }
                out
            })
            .collect()
    });

    // Summary per family and status.
    let mut summary: BTreeMap<(String, String), [usize; 5]> = BTreeMap::new();
    let mut per_file: BTreeMap<(String, String), FileResult> = BTreeMap::new();
    for o in &outcomes {
        let s = summary
            .entry((o.family.clone(), o.format.clone()))
            .or_default();
        let fe = per_file
            .entry((o.id.clone(), o.format.clone()))
            .or_insert_with(|| (false, BTreeSet::new(), BTreeSet::new(), BTreeSet::new()));
        match (o.status.as_str(), &o.adjudication) {
            ("agree", _) => {
                s[0] += 1;
                fe.1.insert(o.scope.clone());
                fe.2.extend(o.normalized.clone());
            }
            ("differ" | "error", Some((right, _))) => {
                s[2] += 1;
                if right == "openreadout" {
                    // the file passes, but a normalized field is confirmed only by agreement: an
                    // adjudicated difference says the other reader is wrong, not that ours is right
                    fe.1.insert(o.scope.clone());
                } else {
                    fe.0 = true;
                    fe.3.extend(o.normalized.clone());
                }
            }
            ("differ" | "error", None) => {
                s[1] += 1;
                fe.0 = true;
                fe.3.extend(o.normalized.clone());
                problems.lock().unwrap().push(format!(
                    "{} [{}] {} ({}): {}",
                    o.id, o.format, o.field, o.reader, o.detail
                ));
            }
            _ => s[3] += 1,
        }
        s[4] += 1;
    }
    println!(
        "{:<22} {:<24} {:>7} {:>7} {:>11} {:>5}",
        "family", "format", "agree", "differ", "adjudicated", "gaps"
    );
    for ((fam, fmt), s) in &summary {
        println!(
            "{fam:<22} {fmt:<24} {:>7} {:>7} {:>11} {:>5}",
            s[0], s[1], s[2], s[3]
        );
    }
    for o in outcomes.iter().filter(|o| o.adjudication.is_some()) {
        let (right, _) = o.adjudication.as_ref().unwrap();
        println!(
            "adjudicated {} {} (right = {right}): {}",
            o.id, o.field, o.detail
        );
    }
    if let Ok(p) = std::env::var("SECOND_RESULTS") {
        let mut lines = String::new();
        for ((id, format), (failed, scopes, confirmed, contradicted)) in &per_file {
            if !failed && scopes.is_empty() {
                continue;
            }
            // `fields`: the normalized experiment fields an independent reader confirmed on this
            // file, named as assurance-audit tracks them (`experiment.acquisition.started_at`);
            // `fields_contradicted`: those it contradicted (the file's status is then FAIL)
            let fields: Vec<String> = confirmed
                .iter()
                .map(|f| format!("experiment.{f}"))
                .collect();
            let _ = writeln!(
                lines,
                "{}",
                serde_json::json!({"id": id, "format": format, "status": if *failed {"FAIL"} else {"pass"}, "independent": true, "compared": scopes, "fields": fields, "fields_contradicted": contradicted})
            );
        }
        std::fs::write(p, lines).unwrap();
    }
    if let Ok(p) = std::env::var("SECOND_REPORT") {
        let mut lines = String::new();
        for o in &outcomes {
            let clip = |v: &Value| {
                let s = v.to_string();
                if s.len() > 300 {
                    Value::from(format!("{}…", &s[..s.floor_char_boundary(300)]))
                } else {
                    v.clone()
                }
            };
            let _ = writeln!(
                lines,
                "{}",
                serde_json::json!({
                    "id": o.id, "format": o.format, "family": o.family, "field": o.field,
                    "scope": o.scope, "reader": o.reader, "status": o.status, "detail": o.detail,
                    "ours": clip(&o.ours), "theirs": clip(&o.theirs),
                    "normalized": o.normalized,
                    "right": o.adjudication.as_ref().map(|a| a.0.clone()),
                    "why": o.adjudication.as_ref().map(|a| a.1.clone()),
                })
            );
        }
        std::fs::write(p, lines).unwrap();
    }
    let problems = problems.into_inner().unwrap();
    assert!(
        problems.is_empty(),
        "{} unadjudicated differences:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn resolve_paths() {
    let v: Value = serde_json::json!({"traces": [{"name": "A", "unit": "mV", "n": 3}, {"name": "b", "unit": "pA", "n": 5}]});
    assert_eq!(resolve(&v, "/traces/1/unit"), Some(Value::from("pA")));
    assert_eq!(
        resolve(&v, "/traces/[name=A]/unit"),
        Some(Value::from("mV"))
    );
    assert_eq!(resolve(&v, "/traces/[name~=B]/n"), Some(Value::from(5)));
    let w: Value = serde_json::json!({"traces": [{"index": 0, "extra": {"stream": "LC Raw Data/Chromatogram Ch1"}}, {"index": 1, "extra": {"stream": "LC Raw Data/Chromatogram Ch5"}}]});
    assert_eq!(
        resolve(&w, "/traces/[extra.stream$=chromatogram ch5]/index"),
        Some(Value::from(1))
    );
    assert_eq!(
        resolve(
            &w,
            "/traces/[extra.stream*=raw data~1chromatogram ch1]/index"
        ),
        Some(Value::from(0))
    );
    assert_eq!(
        resolve(&v, "/traces/*/unit"),
        Some(serde_json::json!(["mV", "pA"]))
    );
    assert_eq!(resolve(&v, "/traces#len"), Some(Value::from(2)));
    assert_eq!(
        resolve(&v, "/traces/{unit!=mV}/n"),
        Some(serde_json::json!([5]))
    );
    assert_eq!(resolve(&v, "/traces/{unit=mV}#len"), Some(Value::from(1)));
    assert_eq!(resolve(&v, "/traces/*/n#sum"), Some(Value::from(8.0)));
    assert_eq!(resolve(&v, "/traces/[name=C]/n"), None);
    let t = |s: &str| parse_time(s).unwrap();
    assert_eq!(t("2009-07-02T17:36:38Z").0, t("2009-07-02 17:36:38").0);
    assert_eq!(t("2009-07-02T10:36:38-07:00").1, Some(-25200));
    let c = Check {
        field: String::new(),
        scope: metadata(),
        cmd: info_cmd(),
        ours: String::new(),
        theirs: Value::Null,
        cmp: "time".into(),
        rel: 0.0,
        abs: None,
        zone: false,
        at: None,
        by: None,
        each: None,
        required: false,
        reader: 0,
    };
    assert!(
        compare(
            &c,
            &Value::from("2009-07-02T17:36:38.361Z"),
            &Value::from("2009-07-02T10:36:38-07:00")
        )
        .is_ok()
    );
}
