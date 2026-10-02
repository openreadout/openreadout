//! JASCO `.jws` files against their ground truth (`oracle/jasco_oracle.py`), for every entry
//! (development and held-out) through one code path (`tests/corpus/`):
//!
//! - Spectra Manager's text/CSV export of the same measurement: the axis unit and y quantity, the
//!   point count, x at the export's rows within its printed precision (and its single-precision
//!   x accumulation, ≤ 0.005), y to the export's six significant digits; the footer's model,
//!   serial, FT-IR settings (accumulation, resolution, gain, aperture, scan speed, filter, light
//!   source, detector) and measurement time (the local time the export prints is the file's UTC
//!   time plus a whole number of hours);
//! - an export of another acquisition with the same name: its instrument facts only;
//! - jws2txt (MIT) as a black box for compound files: the channel count and names, the point
//!   count and the stored values at 64 points per channel.
#![allow(dead_code)] // each test binary that includes this module uses a part of it
#![allow(
    clippy::many_single_char_names,
    clippy::float_cmp,
    clippy::if_not_else,
    clippy::semicolon_if_nothing_returned
)] // comparison code: short names for x, y, t; exact equality where values are copied

use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["jasco-jws"];

/// jws2txt's channel names in our vocabulary.
fn jws2txt_name(n: &str) -> Option<&'static str> {
    Some(match n {
        "CD" => "circular_dichroism",
        "HT VOLTAGE" => "ht_voltage",
        "ABSORBANCE" => "absorbance",
        // jws2txt calls code 14 FLUORESCENCE; it is also the Raman CCD intensity
        "FLUORESCENCE" => "intensity",
        _ => return None,
    })
}

fn yunits_name(y: &str) -> &'static [&'static str] {
    match y {
        "TRANSMITTANCE" => &["transmittance"],
        "ABSORBANCE" => &["absorbance"],
        "REFLECTANCE" => &["reflectance"],
        "INTENSITY" => &[
            "intensity",
            "single_beam",
            "single_beam_reference",
            "single_beam_sample",
        ],
        _ => &[],
    }
}

/// The first number in `Auto (16)`, `0.5 cm-1`, `2,5 mm`.
fn leading_number(s: &str) -> Option<f64> {
    let s = s.split_once('(').map_or(s, |(_, r)| r);
    let t: String = s
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',' || *c == '-')
        .collect();
    t.replace(',', ".").parse().ok()
}

/// `05.01.2023 11:48` or `2024/12/03 12:57` -> (y, m, d, hh, mm).
fn export_datetime(s: &str) -> Option<(i64, i64, i64, i64, i64)> {
    let (date, time) = s.trim().split_once(' ')?;
    let (hh, mm) = time.split_once(':')?;
    let (hh, mm) = (hh.parse().ok()?, mm.get(..2)?.parse().ok()?);
    if let Some((y, rest)) = date.split_once('/') {
        let (m, d) = rest.split_once('/')?;
        return Some((y.parse().ok()?, m.parse().ok()?, d.parse().ok()?, hh, mm));
    }
    let mut it = date.split('.');
    let (d, m, y) = (it.next()?, it.next()?, it.next()?);
    Some((y.parse().ok()?, m.parse().ok()?, d.parse().ok()?, hh, mm))
}

/// Minutes since 1970 of a civil date-time (proleptic Gregorian).
fn minutes((y, m, d, hh, mm): (i64, i64, i64, i64, i64)) -> i64 {
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 1440 + hh * 60 + mm
}

#[allow(clippy::too_many_lines)]
pub fn compare_file(_id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = openreadout_spectro::JwsReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let exp = ds.experiment().ok_or("no experiment facts")?;
    let mut problems = Vec::new();
    let mut done = Vec::new();
    let empty = Vec::new();
    let axis = |t: usize, i: usize| -> f64 {
        let a = &info.traces[t].extra["axis"];
        let n = a["size"].as_f64().unwrap_or(0.0);
        match (a["first"].as_f64(), a["last"].as_f64()) {
            (Some(f), Some(l)) if n > 1.0 => f + (l - f) * i as f64 / (n - 1.0),
            (Some(f), _) => f,
            _ => f64::NAN,
        }
    };

    if let Some(e) = oracle["export"].as_object() {
        let hdr = &e["header"];
        let footer = &e["footer"];
        let other = e["other_acquisition"].as_bool().unwrap_or(false);
        let ins = exp.instrument.clone().unwrap_or_default();
        let model = footer["Model Name"].as_str().or(footer["機種名"].as_str());
        let serial = footer["Serial Number"]
            .as_str()
            .or(footer["シリアル番号"].as_str());
        if model.is_some() && ins.model.as_deref() != model {
            problems.push(format!("model {:?}, export says {model:?}", ins.model));
        } else if model.is_some() {
            super::covered("experiment.instrument.model");
        }
        if serial.is_some() && ins.serial.as_deref() != serial {
            problems.push(format!("serial {:?}, export says {serial:?}", ins.serial));
        }
        let param = |k: &str| {
            exp.method
                .as_ref()
                .and_then(|m| m.parameters.get(k))
                .map(|q| q.value.clone())
        };
        let mut settings = 0;
        for (key, ours) in [
            ("Accumulation", "accumulations"),
            ("Resolution", "resolution"),
            ("Gain", "gain"),
            ("Aperture", "aperture"),
            ("Scanning Speed", "scan_speed"),
            ("Filter", "filter"),
        ] {
            if let Some(v) = footer[key].as_str().and_then(leading_number)
                && info.traces[0].extra["axis"]["quantity"] == "wavenumber"
            {
                match param(ours).and_then(|q| q.as_f64()) {
                    Some(o) if (o - v).abs() <= 1e-6 * v.abs().max(1.0) => settings += 1,
                    o => problems.push(format!("{ours} {o:?}, export says {v}")),
                }
            }
        }
        for (key, ours) in [("Light Source", "light_source"), ("Detector", "detector")] {
            if let Some(v) = footer[key].as_str()
                && info.traces[0].extra["axis"]["quantity"] == "wavenumber"
            {
                match param(ours) {
                    Some(Value::String(o)) if o == v => settings += 1,
                    o => problems.push(format!("{ours} {o:?}, export says {v}")),
                }
            }
        }
        if settings > 0 {
            done.push(format!("{settings} FT-IR settings match the export"));
        }
        if !other {
            // measurement time: the export's local time = our UTC + whole hours
            if let Some(local) = footer["Measurement Date"]
                .as_str()
                .or(footer["測定日時"].as_str())
                .and_then(export_datetime)
            {
                let ours = exp
                    .acquisition
                    .as_ref()
                    .and_then(|a| a.started_at.as_deref())
                    .and_then(|s| {
                        let (d, t) = s.split_once('T')?;
                        let mut dp = d.split('-');
                        let (y, m, dd) = (
                            dp.next()?.parse().ok()?,
                            dp.next()?.parse().ok()?,
                            dp.next()?.parse().ok()?,
                        );
                        let hh = t.get(..2)?.parse().ok()?;
                        let mm = t.get(3..5)?.parse().ok()?;
                        Some(minutes((y, m, dd, hh, mm)))
                    });
                match ours {
                    Some(o) => {
                        let diff = minutes(local) - o;
                        if diff.rem_euclid(60) > 1 && diff.rem_euclid(60) < 59
                            || diff.abs() > 14 * 60 + 1
                        {
                            problems.push(format!(
                                "measurement time: export {local:?} is {diff} min from ours"
                            ));
                        } else {
                            super::covered("experiment.acquisition.started_at");
                            done.push(
                                "measurement time = export's local time − whole hours".into(),
                            );
                        }
                    }
                    None => problems.push("no measurement time; the export has one".into()),
                }
            }
            let t = &info.traces[0];
            let n = e["n"].as_u64().unwrap_or(0);
            if t.sample_count != n {
                problems.push(format!("{} points, export {n}", t.sample_count));
            }
            let want_x = match hdr["XUNITS"].as_str() {
                Some("1/CM") => Some("1/cm"),
                Some("NANOMETERS") => Some("nm"),
                _ => None,
            };
            if want_x.is_some() && t.extra["axis"]["unit"].as_str() != want_x {
                problems.push(format!(
                    "x unit {}, export {:?}",
                    t.extra["axis"]["unit"], hdr["XUNITS"]
                ));
            }
            if let Some(y) = hdr["YUNITS"].as_str()
                && !yunits_name(y).contains(&t.extra["y_quantity"].as_str().unwrap_or(""))
            {
                problems.push(format!("y quantity {}, export {y}", t.extra["y_quantity"]));
            }
            let tr = ds
                .read_trace(0, 0, 0, u64::MAX)
                .map_err(|e| format!("read failed: {e}"))?;
            let irregular = tr.channels.len() > 1;
            let ys = tr.channels.last().cloned().unwrap_or_default();
            let (mut bad_x, mut bad_y) = (0, 0);
            let samples = e["samples"].as_array().unwrap_or(&empty);
            for s in samples {
                let (Some(i), Some(x), Some(y)) = (s[0].as_u64(), s[1].as_f64(), s[2].as_f64())
                else {
                    continue;
                };
                let i = i as usize;
                let ox = if irregular {
                    tr.channels[0][i]
                } else {
                    axis(0, i)
                };
                let oy = ys.get(i).copied().unwrap_or(f64::NAN);
                if (ox - x).abs() > 0.005 {
                    bad_x += 1;
                }
                if (oy - y).abs() > 5e-6 * y.abs() + 1e-12 {
                    bad_y += 1;
                }
            }
            if bad_x + bad_y > 0 {
                problems.push(format!(
                    "{bad_x} x and {bad_y} y of {} export rows differ",
                    samples.len()
                ));
            } else if !samples.is_empty() {
                done.push(format!("{} export rows match (x, y)", samples.len()));
            }
        } else {
            done.push("export of another acquisition: instrument facts only".into());
        }
    }

    if let Some(j) = oracle["jws2txt"].as_object() {
        let chans = j["channels"].as_array().unwrap_or(&empty);
        if chans.len() != info.traces.len() {
            problems.push(format!(
                "{} traces, jws2txt {} channels",
                info.traces.len(),
                chans.len()
            ));
        } else {
            let mut ok = 0;
            for (k, c) in chans.iter().enumerate() {
                let t = &info.traces[k];
                if let Some(want) = c["name"].as_str().and_then(jws2txt_name)
                    && t.extra["y_quantity"].as_str() != Some(want)
                {
                    problems.push(format!(
                        "trace {k} is {}, jws2txt says {}",
                        t.extra["y_quantity"], c["name"]
                    ));
                }
                if c["n"].as_u64() != Some(t.sample_count) {
                    problems.push(format!(
                        "trace {k}: {} points, jws2txt {}",
                        t.sample_count, c["n"]
                    ));
                    continue;
                }
                let tr = ds
                    .read_trace(k as u32, 0, 0, u64::MAX)
                    .map_err(|e| format!("read failed: {e}"))?;
                let ys = tr.channels.last().cloned().unwrap_or_default();
                let bad = c["samples"]
                    .as_array()
                    .unwrap_or(&empty)
                    .iter()
                    .filter(|s| {
                        let (i, v) = (
                            s[0].as_u64().unwrap_or(0) as usize,
                            s[1].as_f64().unwrap_or(f64::NAN),
                        );
                        ys.get(i)
                            .is_none_or(|o| (o - v).abs() > 1e-9 * v.abs().max(1e-30))
                    })
                    .count();
                if bad > 0 {
                    problems.push(format!("trace {k}: {bad} values differ from jws2txt"));
                } else {
                    ok += 1;
                }
            }
            if ok == chans.len() {
                done.push(format!("{ok} channel(s) equal jws2txt's values"));
            }
        }
    }
    if done.is_empty() && problems.is_empty() {
        done.push(format!("{} trace(s) read", info.traces.len()));
    }
    if problems.is_empty() {
        Ok(done.join(", "))
    } else {
        Err(problems.join("; "))
    }
}
