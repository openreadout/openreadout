//! Spectral imaging and spectrum-series files against their ground truth, for every entry
//! (development and held-out) through one code path (`tests/corpus/`):
//!
//! - WITec `.wip`/`.wid` (`oracle/witec_oracle.py`): witio (MIT-0, a port of wit_io) run as a
//!   black box — per graph the spectrum count, point count, whole spectra of up to six sweeps
//!   (xxh3-128 of the values as f64, NaN for blanked lines) and sampled rows with the calibrated
//!   x value (relative 1e-9); per image the size, the plane (xxh3-128 as f64, or of the RGB bytes)
//!   and sampled pixels; the vendor software's text export of a data file (x to the six digits it
//!   prints, every exported value of the chosen spectra).
#![allow(dead_code)] // each test binary that includes this module uses a part of it
#![allow(clippy::many_single_char_names, clippy::float_cmp)] // comparison code

use std::path::Path;

use openreadout_core::FormatReader;
use openreadout_core::pixel::{PixelType, Plane};
use openreadout_core::reader::{Dataset, PlaneIndex};
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["witec-project", "agilent-fpa", "perkinelmer-fsm"];

/// The outputs a comparison covers (assurance evidence).
pub fn scopes(oracle_text: &str) -> Vec<&'static str> {
    let o: Value = serde_json::from_str(oracle_text).unwrap_or_default();
    let mut v = vec!["metadata", "traces"];
    if o["images"].as_array().is_some_and(|a| !a.is_empty()) {
        v.push("pixels");
    }
    v
}

fn open(format: &str, path: &Path) -> Result<Box<dyn Dataset>, String> {
    let r: Box<dyn FormatReader> = match format {
        "witec-project" => Box::new(openreadout_spectro::WitecReader),
        "agilent-fpa" => Box::new(openreadout_spectro::AgilentFpaReader),
        "perkinelmer-fsm" => Box::new(openreadout_spectro::FsmReader),
        other => return Err(format!("no reader for {other}")),
    };
    r.open(path).map_err(|e| format!("open failed: {e}"))
}

fn h128(values: &[f64]) -> String {
    let mut b = Vec::with_capacity(values.len() * 8);
    for v in values {
        let v = if v.is_nan() { f64::NAN } else { *v };
        b.extend_from_slice(&v.to_le_bytes());
    }
    format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&b))
}

/// Samples of a plane as f64, row-major.
fn plane_f64(p: &Plane) -> Vec<f64> {
    let d = &p.data;
    match p.pixel_type {
        PixelType::Uint8 => d.iter().map(|v| f64::from(*v)).collect(),
        PixelType::Int8 => d
            .iter()
            .map(|v| f64::from(i8::from_le_bytes([*v])))
            .collect(),
        PixelType::Uint16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(u16::from_le_bytes(*c)))
            .collect(),
        PixelType::Int16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(i16::from_le_bytes(*c)))
            .collect(),
        PixelType::Uint32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(u32::from_le_bytes(*c)))
            .collect(),
        PixelType::Int32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(i32::from_le_bytes(*c)))
            .collect(),
        PixelType::Float => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(f32::from_le_bytes(*c)))
            .collect(),
        PixelType::Double => d
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes(*c))
            .collect(),
        _ => Vec::new(),
    }
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a.is_nan() && b.is_nan()) || (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-12)
}

/// x of sample `i` of a trace (the axis channel when irregular).
fn x_at(axis: Option<&Value>, channels: &[Vec<f64>], i: usize) -> Option<f64> {
    let a = axis?;
    if a.get("irregular").and_then(Value::as_bool) == Some(true) {
        let c = usize::try_from(a.get("channel")?.as_u64()?).ok()?;
        channels.get(c)?.get(i).copied()
    } else {
        Some(a.get("first")?.as_f64()? + a.get("step")?.as_f64()? * i as f64)
    }
}

#[allow(clippy::too_many_lines)] // traces, images, facts and the export, one after the other
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
    if let Some(n) = oracle["trace_count"].as_u64()
        && n != info.traces.len() as u64
    {
        problems.push(format!("{} traces, the oracle has {n}", info.traces.len()));
    }
    let (mut sweeps_ok, mut rows_ok) = (0usize, 0usize);
    for o in oracle["traces"].as_array().unwrap_or(&empty) {
        let ti = o["trace"].as_u64().unwrap_or(0) as usize;
        if let Some(e) = o["error"].as_str() {
            problems.push(format!("trace {ti}: oracle error {e}"));
            continue;
        }
        let Some(t) = info.traces.get(ti) else {
            problems.push(format!("no trace {ti}"));
            continue;
        };
        if let Some(n) = o["name"].as_str()
            && t.name.as_deref() != Some(n)
        {
            problems.push(format!(
                "trace {ti}: name {:?}, the oracle has {n:?}",
                t.name
            ));
        }
        if let Some(n) = o["sweeps"].as_u64()
            && n != u64::from(t.sweep_count)
        {
            problems.push(format!(
                "trace {ti}: {} spectra, the oracle has {n}",
                t.sweep_count
            ));
            continue;
        }
        if let Some(n) = o["n"].as_u64()
            && n != t.sample_count
        {
            problems.push(format!(
                "trace {ti}: {} points, the oracle has {n}",
                t.sample_count
            ));
            continue;
        }
        let axis = t.extra.get("axis");
        let hashes = o["sweep_hashes"].as_object().cloned().unwrap_or_default();
        for (k, want) in &hashes {
            let Ok(sweep) = k.parse::<u32>() else {
                continue;
            };
            let tr = match ds.read_trace(ti as u32, sweep, 0, u64::MAX) {
                Ok(t) => t,
                Err(e) => {
                    problems.push(format!("trace {ti} sweep {sweep}: {e}"));
                    continue;
                }
            };
            let y = tr.channels.last().cloned().unwrap_or_default();
            let got = h128(&y);
            if want.as_str() == Some(got.as_str()) {
                sweeps_ok += 1;
            } else {
                problems.push(format!(
                    "trace {ti} sweep {sweep}: values hash {got} != oracle {want}"
                ));
            }
            for r in o["rows"][k].as_array().unwrap_or(&empty) {
                let (Some(i), Some(x)) = (r[0].as_u64(), r[1].as_f64()) else {
                    continue;
                };
                let i = i as usize;
                let ours_x = x_at(axis, &tr.channels, i).unwrap_or(f64::NAN);
                let ours_y = y.get(i).copied().unwrap_or(f64::NAN);
                let want_y = r[2].as_f64().unwrap_or(f64::NAN);
                if !close(ours_x, x, 1e-9) {
                    problems.push(format!(
                        "trace {ti} sweep {sweep} point {i}: x {ours_x} != oracle {x}"
                    ));
                    break;
                }
                if !close(ours_y, want_y, 0.0) {
                    problems.push(format!(
                        "trace {ti} sweep {sweep} point {i}: y {ours_y} != oracle {want_y}"
                    ));
                    break;
                }
                rows_ok += 1;
            }
        }
    }
    if sweeps_ok > 0 {
        done.push(format!(
            "{sweeps_ok} spectra and {rows_ok} calibrated x values match"
        ));
    }
    let mut images_ok = 0;
    for o in oracle["images"].as_array().unwrap_or(&empty) {
        let k = o["image"].as_u64().unwrap_or(0) as usize;
        if let Some(e) = o["error"].as_str() {
            problems.push(format!("image {k}: oracle error {e}"));
            continue;
        }
        let Some(im) = info.images.get(k) else {
            problems.push(format!("no image {k} (the oracle has {})", o["name"]));
            continue;
        };
        if let Some(n) = o["name"].as_str()
            && im.name.as_deref() != Some(n)
        {
            problems.push(format!(
                "image {k}: name {:?}, the oracle has {n:?}",
                im.name
            ));
            continue;
        }
        if o["width"].as_u64() != Some(u64::from(im.size_x))
            || o["height"].as_u64() != Some(u64::from(im.size_y))
        {
            problems.push(format!(
                "image {k}: {} × {}, the oracle has {} × {}",
                im.size_x, im.size_y, o["width"], o["height"]
            ));
            continue;
        }
        let c = o["channel"].as_u64().unwrap_or(0) as u32;
        let plane = match ds.read_plane(k as u32, PlaneIndex { c, z: 0, t: 0 }) {
            Ok(p) => p,
            Err(e) => {
                problems.push(format!("image {k}: {e}"));
                continue;
            }
        };
        if let Some(want) = o["xxh3_rgb8"].as_str() {
            let got = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&plane.data));
            if got == want {
                images_ok += 1;
            } else {
                problems.push(format!("image {k}: RGB hash {got} != oracle {want}"));
            }
            continue;
        }
        let v = plane_f64(&plane);
        if let Some(want) = o["xxh3_f64"].as_str() {
            let got = h128(&v);
            if got == want {
                images_ok += 1;
            } else {
                let w = im.size_x as usize;
                let bad = o["samples"].as_array().unwrap_or(&empty).iter().find(|s| {
                    let (x, y) = (
                        s[0].as_u64().unwrap_or(0) as usize,
                        s[1].as_u64().unwrap_or(0) as usize,
                    );
                    let want = s[2].as_f64().unwrap_or(f64::NAN);
                    !close(v.get(x + w * y).copied().unwrap_or(f64::NAN), want, 0.0)
                });
                problems.push(format!(
                    "image {k}: plane hash {got} != oracle {want}; first differing sample {bad:?}"
                ));
            }
        }
    }
    if images_ok > 0 {
        done.push(format!("{images_ok} images match"));
    }
    if let Some(ex) = oracle.get("export").filter(|e| e.is_object()) {
        let ti = ex["trace"].as_u64().unwrap_or(0) as u32;
        let tol = ex["x_tol_rel"].as_f64().unwrap_or(5e-6);
        match info.traces.get(ti as usize) {
            None => problems.push(format!("export: no trace {ti}")),
            Some(t) => {
                if ex["points"].as_u64() != Some(t.sample_count) {
                    problems.push(format!(
                        "export: {} points, we have {}",
                        ex["points"], t.sample_count
                    ));
                }
                if ex["spectra"].as_u64() != Some(u64::from(t.sweep_count)) {
                    problems.push(format!(
                        "export: {} spectra, we have {}",
                        ex["spectra"], t.sweep_count
                    ));
                }
                let axis = t.extra.get("axis").cloned();
                let mut values = 0usize;
                for s in ex["sweeps"].as_array().unwrap_or(&empty) {
                    let sweep = s["sweep"].as_u64().unwrap_or(0) as u32;
                    let tr = match ds.read_trace(ti, sweep, 0, u64::MAX) {
                        Ok(t) => t,
                        Err(e) => {
                            problems.push(format!("export sweep {sweep}: {e}"));
                            continue;
                        }
                    };
                    let y = tr.channels.last().cloned().unwrap_or_default();
                    let all: Vec<f64> = s["all_y"]
                        .as_array()
                        .unwrap_or(&empty)
                        .iter()
                        .map(|v| v.as_f64().unwrap_or(f64::NAN))
                        .collect();
                    if let Some((i, (a, b))) = y
                        .iter()
                        .zip(&all)
                        .enumerate()
                        .find(|(_, (a, b))| !close(**a, **b, 5e-6))
                    {
                        problems.push(format!(
                            "export sweep {sweep} point {i}: y {a} != export {b}"
                        ));
                        continue;
                    }
                    values += all.len();
                    for r in s["rows"].as_array().unwrap_or(&empty) {
                        let (Some(i), Some(x)) = (r[0].as_u64(), r[1].as_f64()) else {
                            continue;
                        };
                        let mut ours =
                            x_at(axis.as_ref(), &tr.channels, i as usize).unwrap_or(f64::NAN);
                        // an export in nm of a Raman-shift axis: back to wavelength
                        if ex["x"].as_str() == Some("nm")
                            && axis.as_ref().and_then(|a| a["quantity"].as_str())
                                == Some("raman_shift")
                        {
                            let exc = t
                                .extra
                                .get("excitation_wavelength_nm")
                                .and_then(Value::as_f64)
                                .unwrap_or(f64::NAN);
                            ours = 1.0 / (1.0 / exc - ours * 1e-7);
                        }
                        if !close(ours, x, tol) {
                            problems.push(format!(
                                "export sweep {sweep} point {i}: x {ours} != export {x}"
                            ));
                            break;
                        }
                    }
                }
                if values > 0 {
                    done.push(format!(
                        "{values} exported values and their x match the WITec export"
                    ));
                }
            }
        }
    }
    if let Some(facts) = oracle["facts"].as_array() {
        let exp = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
        let tree = serde_json::to_value(&exp).unwrap_or(Value::Null);
        for f in facts {
            let p = f["path"].as_str().unwrap_or("");
            let mut cur = &tree;
            for k in p.split('.') {
                cur = &cur[k];
            }
            // a quantity compares by its value; numbers within 1e-9 relative
            if cur.get("value").is_some() {
                cur = &cur["value"];
            }
            let same = match (cur.as_f64(), f["value"].as_f64()) {
                (Some(a), Some(b)) => close(a, b, 1e-9),
                _ => cur == &f["value"],
            };
            if same {
                done.push(format!("fact {p}"));
            } else {
                problems.push(format!("fact {p}: {cur}, the oracle has {}", f["value"]));
            }
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
