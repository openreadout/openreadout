//! Readers of sampled-column files (EPR, X-ray diffraction, electrochemistry, thermal analysis,
//! rheology, particle sizing, Cary UV-Vis) against their ground truth (`oracle/series_oracle.py`), for every
//! entry (development and held-out) through one code path (`tests/corpus/`).
//!
//! The oracle lists, per trace, sampled rows `[i, x, y]` taken from an independent source (the
//! vendor software's export of the same measurement, or an independent reader run as a black
//! box): our sample `i` of the named channel must equal `y` within the tolerance, and our abscissa
//! at `i` (from `extra.axis`, or the axis channel) must equal `x`. Sample counts, sweep counts and
//! experiment facts (`facts`: a dotted path into the experiment model) are compared exactly or
//! within the stated tolerance, and table cells likewise.
#![allow(dead_code)] // each test binary that includes this module uses a part of it
#![allow(clippy::float_cmp, clippy::many_single_char_names)] // comparison code

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::FormatReader;
use openreadout_core::reader::Dataset;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &[
    "biologic-mpr",
    "biologic-mpt",
    "gamry-dta",
    "neware-nda",
    "neware-ndax",
    "arbin-res",
    "netzsch-ngb",
    "ta-universal-analysis",
    "ta-trios",
    "sartorius-octet-frd",
    "malvern-zetasizer-dts",
    "bruker-bes3t",
    "bruker-esp",
    "panalytical-xrdml",
    "bruker-raw",
    "bruker-brml",
    "rigaku-ras",
    "rigaku-rasx",
    "agilent-cary",
];

/// The outputs the comparison covers for a format (assurance evidence).
pub fn scopes(format: &str) -> Vec<&'static str> {
    match format {
        "bruker-bes3t"
        | "bruker-esp"
        | "panalytical-xrdml"
        | "bruker-raw"
        | "bruker-brml"
        | "rigaku-ras"
        | "rigaku-rasx"
        | "neware-nda"
        | "neware-ndax"
        | "netzsch-ngb"
        | "ta-universal-analysis"
        | "ta-trios"
        | "agilent-cary" => {
            vec!["metadata", "traces"]
        }
        "malvern-zetasizer-dts" => vec!["metadata", "tables"],
        _ => vec!["metadata", "tables", "traces"],
    }
}

fn open(format: &str, path: &Path) -> Result<Box<dyn Dataset>, String> {
    let r: Box<dyn FormatReader> = match format {
        "bruker-bes3t" => Box::new(openreadout_epr::Bes3tReader),
        "biologic-mpr" => Box::new(openreadout_echem::MprReader),
        "biologic-mpt" => Box::new(openreadout_echem::MptReader),
        "neware-nda" => Box::new(openreadout_echem::NdaReader),
        "neware-ndax" => Box::new(openreadout_echem::NdaxReader),
        "arbin-res" => Box::new(openreadout_echem::ArbinReader),
        "netzsch-ngb" => Box::new(openreadout_thermal::NgbReader),
        "ta-universal-analysis" => Box::new(openreadout_thermal::TaReader),
        "ta-trios" => Box::new(openreadout_thermal::TriosReader),
        "sartorius-octet-frd" => Box::new(openreadout_biophys::OctetReader),
        "malvern-zetasizer-dts" => Box::new(openreadout_biophys::ZetasizerReader),
        "gamry-dta" => Box::new(openreadout_echem::GamryReader),
        "bruker-esp" => Box::new(openreadout_epr::EspReader),
        "panalytical-xrdml" => Box::new(openreadout_xrd::XrdmlReader),
        "bruker-raw" => Box::new(openreadout_xrd::BrukerRawReader),
        "bruker-brml" => Box::new(openreadout_xrd::BrmlReader),
        "rigaku-ras" => Box::new(openreadout_xrd::RasReader),
        "rigaku-rasx" => Box::new(openreadout_xrd::RasxReader),
        "agilent-cary" => Box::new(openreadout_spectro::CaryReader),
        other => return Err(format!("no reader for {other}")),
    };
    r.open(path).map_err(|e| format!("open failed: {e}"))
}

fn close(a: f64, b: f64, abs: f64, rel: f64) -> bool {
    if a.is_nan() && b.is_nan() {
        return true;
    }
    (a - b).abs() <= abs.max(rel * a.abs().max(b.abs()))
}

/// A value at a dotted path of a JSON tree (`method.parameters.scans`); a Quantity yields its
/// `value`.
fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = v;
    for k in path.split('.') {
        cur = match k.parse::<usize>() {
            Ok(i) if cur.is_array() => cur.get(i)?,
            _ => cur.get(k)?,
        };
    }
    if cur.is_object()
        && let Some(x) = cur.get("value")
    {
        return Some(x);
    }
    Some(cur)
}

pub fn compare_file(
    format: &str,
    _id: &str,
    path: &Path,
    oracle: &Value,
) -> Result<String, String> {
    let mut ds = open(format, path)?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let mut problems = Vec::new();
    let mut done = Vec::new();
    let empty = Vec::new();

    // columns compared by their vendor label: (columns, rows, source)
    let mut labelled = (0usize, 0usize, String::new());
    if let Some(n) = oracle["trace_count"].as_u64()
        && n != info.traces.len() as u64
    {
        problems.push(format!("{} traces, the oracle has {n}", info.traces.len()));
    }
    for (k, o) in oracle["traces"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .enumerate()
    {
        let ti = o["trace"].as_u64().unwrap_or(0) as usize;
        let sweep = o["sweep"].as_u64().unwrap_or(0) as u32;
        let Some(t) = info.traces.get(ti) else {
            problems.push(format!("oracle trace {k}: no trace {ti}"));
            continue;
        };
        if let Some(n) = o["sweeps"].as_u64()
            && n != u64::from(t.sweep_count)
        {
            problems.push(format!(
                "trace {ti}: {} sweeps, the oracle has {n}",
                t.sweep_count
            ));
            continue;
        }
        let label = o["label"].as_str();
        let ch_name = label.or(o["channel"].as_str()).unwrap_or("intensity");
        let found = match label {
            Some(l) => t
                .channels
                .iter()
                .position(|c| c.extra.get("label").and_then(Value::as_str) == Some(l)),
            None => t.channels.iter().position(|c| c.name == ch_name),
        };
        let Some(ch) = found else {
            if o["optional"].as_bool() != Some(true) {
                problems.push(format!("trace {ti}: no channel `{ch_name}`"));
            }
            continue;
        };
        let tr = match ds.read_trace(ti as u32, sweep, 0, u64::MAX) {
            Ok(t) => t,
            Err(e) => {
                problems.push(format!("trace {ti} sweep {sweep}: {e}"));
                continue;
            }
        };
        let n = tr.channels.first().map_or(0, Vec::len);
        if let Some(want) = o["n"].as_u64()
            && want != n as u64
        {
            problems.push(format!(
                "trace {ti} sweep {sweep}: {n} samples, the oracle has {want}"
            ));
            continue;
        }
        if let Some(u) = o["unit"].as_str()
            && t.channels[ch].unit.as_deref() != Some(u)
        {
            problems.push(format!(
                "trace {ti} channel {ch_name}: unit {:?}, the oracle says {u}",
                t.channels[ch].unit
            ));
        }
        let axis = t.extra.get("axis");
        let x_of = |i: usize| -> Option<f64> {
            let a = axis?;
            if a.get("irregular").and_then(Value::as_bool) == Some(true) {
                let c = a.get("channel")?.as_u64()? as usize;
                tr.channels.get(c)?.get(i).copied()
            } else {
                Some(a.get("first")?.as_f64()? + a.get("step")?.as_f64()? * i as f64)
            }
        };
        let x_abs = o["x_tol"].as_f64().unwrap_or(1e-6);
        let y_abs = o["y_tol_abs"].as_f64().unwrap_or(0.0);
        let y_rel = o["y_tol_rel"].as_f64().unwrap_or(1e-9);
        let y_scale = o["y_scale"].as_f64().unwrap_or(1.0);
        let mut bad = Vec::new();
        let rows = o["samples"].as_array().unwrap_or(&empty);
        let interpolate = o["interpolate"].as_bool() == Some(true);
        let xs: Vec<f64> = if interpolate {
            (0..n).filter_map(x_of).collect()
        } else {
            Vec::new()
        };
        for r in rows {
            // an export resampled at its own times: our value linearly interpolated at its x
            if interpolate
                && r[0].is_null()
                && let (Some(x), Some(y)) = (r[1].as_f64(), r[2].as_f64())
            {
                let k = xs.partition_point(|v| *v < x);
                let ours = match (k.checked_sub(1).and_then(|j| xs.get(j)), xs.get(k)) {
                    (Some(&x0), Some(&x1)) if x1 > x0 => {
                        let y0 = tr.channels[ch][k - 1] * y_scale;
                        let y1 = tr.channels[ch][k] * y_scale;
                        y0 + (y1 - y0) * (x - x0) / (x1 - x0)
                    }
                    (_, Some(&x1)) if (x1 - x).abs() <= x_abs => tr.channels[ch][k] * y_scale,
                    _ => f64::NAN,
                };
                if !close(ours, y, y_abs, y_rel) {
                    bad.push(format!("x {x}: y {ours} (interpolated) vs {y}"));
                }
                continue;
            }
            // a row without an index is matched by its x: the sample nearest to it (exports that
            // thin the data out do not say which samples they kept)
            let i = if let Some(i) = r[0].as_u64() {
                i as usize
            } else if let Some(x) = r[1].as_f64() {
                let Some(i) = (0..n)
                    .filter_map(|i| x_of(i).map(|ox| (i, (ox - x).abs())))
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i)
                else {
                    bad.push(format!("x {x}: no sample"));
                    continue;
                };
                i
            } else {
                continue;
            };
            let ours_y = tr.channels[ch].get(i).copied().unwrap_or(f64::NAN) * y_scale;
            if let Some(y) = r[2].as_f64()
                && !close(ours_y, y, y_abs, y_rel)
            {
                bad.push(format!("sample {i}: y {ours_y} vs {y}"));
            }
            if let Some(x) = r[1].as_f64() {
                match x_of(i) {
                    Some(ox) if (ox - x).abs() <= x_abs => {}
                    ox => bad.push(format!("sample {i}: x {ox:?} vs {x}")),
                }
            }
        }
        if bad.is_empty() && label.is_some() {
            labelled.0 += 1;
            labelled.1 += rows.len();
            labelled.2 = o["source"].as_str().unwrap_or("oracle").to_string();
        } else if bad.is_empty() {
            done.push(format!(
                "trace {ti}/{sweep} {ch_name}: {} rows match ({})",
                rows.len(),
                o["source"].as_str().unwrap_or("oracle")
            ));
        } else {
            problems.push(format!(
                "trace {ti}/{sweep} {ch_name}: {} of {} rows differ, first: {}",
                bad.len(),
                rows.len(),
                bad[0]
            ));
        }
    }
    if labelled.0 > 0 {
        done.push(format!(
            "{} columns ({} sampled values) match {}",
            labelled.0, labelled.1, labelled.2
        ));
    }
    if let Some(facts) = oracle["facts"].as_array() {
        let exp = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
        let tree = serde_json::to_value(&exp).unwrap_or(Value::Null);
        let mut ok = 0;
        for f in facts {
            let p = f["path"].as_str().unwrap_or("");
            let want = &f["value"];
            let got = at(&tree, p);
            let good = match (got, want) {
                (Some(g), Value::Number(w)) => g.as_f64().is_some_and(|g| {
                    close(
                        g,
                        w.as_f64().unwrap_or(f64::NAN),
                        f["tol_abs"].as_f64().unwrap_or(0.0),
                        f["tol_rel"].as_f64().unwrap_or(1e-9),
                    )
                }),
                (Some(Value::String(g)), Value::String(w)) => {
                    if f["prefix"].as_bool() == Some(true) {
                        g.starts_with(w.as_str())
                    } else {
                        g.trim() == w.trim()
                    }
                }
                (Some(g), w) => g == w,
                (None, _) => false,
            };
            if good {
                ok += 1;
            } else {
                problems.push(format!("fact {p}: {got:?}, the oracle has {want}"));
            }
        }
        if ok > 0 {
            done.push(format!("{ok} facts match"));
        }
    }
    for o in oracle["tables"].as_array().unwrap_or(&empty) {
        let ti = o["table"].as_u64().unwrap_or(0) as u32;
        let Some(t) = info.tables.get(ti as usize) else {
            problems.push(format!("no table {ti}"));
            continue;
        };
        if let Some(n) = o["rows"].as_u64()
            && n != t.row_count
        {
            problems.push(format!(
                "table {ti}: {} rows, the oracle has {n}",
                t.row_count
            ));
            continue;
        }
        let tab = ds
            .read_table(ti, 0, u64::MAX)
            .map_err(|e| format!("table {ti}: {e}"))?;
        let mut bad = 0;
        let mut first: Option<String> = None;
        let cells = o["cells"].as_array().unwrap_or(&empty);
        // `"key": "<column>"`: the oracle lists some of our rows, each found by its value in that
        // column (an export of some records of a file), instead of row by row
        let rows: BTreeMap<u64, u64> = match o["key"].as_str() {
            Some(key) => {
                let Some(ki) = t.columns.iter().position(|x| x.name == key) else {
                    problems.push(format!("table {ti}: no key column `{key}`"));
                    continue;
                };
                let mut map = BTreeMap::new();
                for c in cells {
                    if let (Some(r), Some(k), Some(v)) =
                        (c[0].as_u64(), c[1].as_str(), c[2].as_f64())
                        && k == key
                    {
                        match tab.columns[ki].iter().position(|x| *x == v) {
                            Some(ours) => {
                                map.insert(r, ours as u64);
                            }
                            None => problems.push(format!("table {ti}: no row with {key} {v}")),
                        }
                    }
                }
                map
            }
            None => BTreeMap::new(),
        };
        let keyed = o["key"].is_string();
        let row_of = |r: u64| -> Option<u64> {
            if keyed {
                rows.get(&r).copied()
            } else {
                Some(r)
            }
        };
        for c in cells {
            // a text cell: our column's category at the row
            if let (Some(r), Some(col), Some(want)) = (c[0].as_u64(), c[1].as_str(), c[2].as_str())
            {
                let Some(r) = row_of(r) else {
                    bad += 1;
                    continue;
                };
                let got = t.columns.iter().position(|x| x.name == col).and_then(|ci| {
                    let k = tab.columns[ci].get(r as usize).copied()?;
                    let cats = t.columns[ci].extra.get("categories")?.as_array()?;
                    cats.get(k as usize)?.as_str().map(str::to_string)
                });
                let same = got.as_deref().map(str::trim) == Some(want.trim())
                    // times: within `time_tol_s` when the oracle's source rounds them
                    || o["time_tol_s"].as_f64().is_some_and(|tol| {
                        let t = openreadout_core::time::iso8601_to_unix;
                        match (got.as_deref().and_then(t), t(want)) {
                            (Some(a), Some(b)) => (a - b).abs() <= tol,
                            _ => false,
                        }
                    });
                if !same {
                    bad += 1;
                }
                continue;
            }
            let (Some(r), Some(col), Some(v)) = (c[0].as_u64(), c[1].as_str(), c[2].as_f64())
            else {
                continue;
            };
            let Some(r) = row_of(r) else {
                bad += 1;
                continue;
            };
            let Some(ci) = t.columns.iter().position(|x| x.name == col) else {
                bad += 1;
                continue;
            };
            let ours = tab.columns[ci].get(r as usize).copied().unwrap_or(f64::NAN);
            // a fourth element is the cell's own absolute tolerance (the source's rounding)
            let (tol_abs, tol_rel) = match c.get(3).and_then(Value::as_f64) {
                Some(t) => (t, 0.0),
                None => (
                    o["tol_abs"].as_f64().unwrap_or(0.0),
                    o["tol_rel"].as_f64().unwrap_or(1e-9),
                ),
            };
            if !close(ours, v, tol_abs, tol_rel) {
                bad += 1;
                first.get_or_insert_with(|| format!("row {r} {col}: {ours} vs {v}"));
            }
        }
        if bad > 0 {
            problems.push(format!(
                "table {ti}: {bad} of {} cells differ{}",
                cells.len(),
                first
                    .map(|f| format!(", first numeric: {f}"))
                    .unwrap_or_default()
            ));
        } else {
            done.push(format!("table {ti}: {} cells match", cells.len()));
        }
    }
    if done.is_empty() && problems.is_empty() {
        return Err("nothing compared".into());
    }
    if problems.is_empty() {
        Ok(done.join("; "))
    } else {
        Err(problems.join("; "))
    }
}
