//! NanoDrop and Qubit exports against allotropy's ASM and pandas (`oracle/benchtop_oracle.py`):
//! per sample in file order the sample name, the concentrations (in the oracle's unit), the
//! absorbances at 260 and 280 nm, the fluorescence, the A260/A280 and A260/A230 ratios, the
//! instrument model and serial; per spectrum its name, wavelength range, point count and 32
//! absorbance values.
#![allow(dead_code)] // each test binary that includes this module uses a part of it
// i, k, n, t, v, u are the natural names here, and both readers write the same decimal text, so
// equal values compare equal
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use std::path::Path;

use openreadout_core::FormatReader;
use serde_json::Value;

/// Format ids this module compares.
pub const FORMATS: &[&str] = &["nanodrop", "qubit"];

/// ng/µL per unit (the mass concentrations both sides write), or `None`.
fn per_ng_ul(unit: &str) -> Option<f64> {
    let u = unit
        .replace('μ', "µ")
        .replace("ul", "µL")
        .replace("uL", "µL")
        .replace("ml", "mL");
    Some(match u.as_str() {
        "pg/µL" => 1e-3,
        "ng/µL" | "µg/mL" | "ug/mL" => 1.0,
        "ng/mL" => 1e-3,
        "mg/mL" | "µg/µL" => 1e3,
        _ => return None,
    })
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// Our table as rows: column name → (number or text).
struct Ours {
    names: Vec<String>,
    units: Vec<Option<String>>,
    cats: Vec<Option<Vec<String>>>,
    cols: Vec<Vec<f64>>,
}

impl Ours {
    fn num(&self, name: &str, i: usize) -> Option<(f64, Option<&str>)> {
        let k = self.names.iter().position(|n| n == name)?;
        if self.cats[k].is_some() {
            return None;
        }
        Some((self.cols[k][i], self.units[k].as_deref()))
    }
    fn text(&self, name: &str, i: usize) -> Option<String> {
        let k = self.names.iter().position(|n| n == name)?;
        let c = self.cats[k].as_ref()?;
        c.get(self.cols[k][i] as usize).cloned()
    }
}

pub fn compare_file(_id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = match oracle["format"].as_str() {
        Some("qubit") => openreadout_benchtop::QubitReader.open(path),
        _ => openreadout_benchtop::NanoDropReader.open(path),
    }
    .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let t = info.tables.first().ok_or("no samples table")?;
    let data = ds
        .read_table(0, 0, u64::MAX)
        .map_err(|e| format!("read failed: {e}"))?;
    let ours = Ours {
        names: t.columns.iter().map(|c| c.name.clone()).collect(),
        units: t.columns.iter().map(|c| c.unit.clone()).collect(),
        cats: t
            .columns
            .iter()
            .map(|c| {
                c.extra
                    .get("categories")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|v| v.as_str().unwrap_or_default().to_string())
                            .collect()
                    })
            })
            .collect(),
        cols: data.columns,
    };
    let n = t.row_count as usize;
    let mut problems = Vec::new();
    let mut checked = 0usize;
    let empty = Vec::new();
    // measurement documents grouped by sample row: consecutive documents of one sample,
    // location and time
    let ms = oracle["measurements"].as_array().unwrap_or(&empty);
    let mut groups: Vec<Vec<&Value>> = Vec::new();
    for m in ms {
        let key = |m: &Value| {
            (
                m["sample"].clone(),
                m["location"].clone(),
                m["time"].clone(),
            )
        };
        match groups.last_mut() {
            Some(g) if key(g[0]) == key(m) && !g.iter().any(|x| same_wavelength(x, m)) => g.push(m),
            _ => groups.push(vec![m]),
        }
    }
    if !ms.is_empty() && groups.len() != n {
        problems.push(format!("{n} rows, allotropy {} samples", groups.len()));
    }
    let calc = oracle["calculated"].as_array().unwrap_or(&empty);
    for (i, g) in groups.iter().enumerate().take(n) {
        let sample = g[0]["sample"].as_str().unwrap_or_default();
        let names = [ours.text("sample", i), ours.text("sample_uid", i)];
        // allotropy writes `N/A` for a sample without a name
        let blank = |s: &str| s.trim().is_empty() || s == "N/A";
        if !names
            .iter()
            .flatten()
            .any(|s| s.trim() == sample.trim() || (blank(s) && blank(sample)))
        {
            problems.push(format!("row {i}: sample {names:?}, allotropy {sample:?}"));
            continue;
        }
        checked += 1;
        let values: Vec<(&str, f64, &str, Option<f64>)> = g
            .iter()
            .flat_map(|m| {
                let wl = m["values"].as_array().and_then(|v| {
                    v.iter()
                        .find(|x| x[0] == "detector wavelength setting")
                        .and_then(|x| x[1].as_f64())
                });
                m["values"].as_array().into_iter().flatten().map(move |x| {
                    (
                        x[0].as_str().unwrap_or_default(),
                        x[1].as_f64().unwrap_or(f64::NAN),
                        x[2].as_str().unwrap_or_default(),
                        wl,
                    )
                })
            })
            .collect();
        let mut conc = |key: &str, ours_name: &str, problems: &mut Vec<String>| {
            if let Some((_, v, u, _)) = values.iter().find(|x| x.0 == key) {
                match ours.num(ours_name, i) {
                    Some((o, Some(ou))) if o.is_finite() => {
                        let ok = per_ng_ul(u)
                            .zip(per_ng_ul(ou))
                            .is_some_and(|(a, b)| close(o * b, v * a));
                        if ok {
                            checked += 1;
                        } else {
                            problems
                                .push(format!("row {i} {ours_name}: {o} {ou}, allotropy {v} {u}"));
                        }
                    }
                    other => {
                        problems.push(format!("row {i} {ours_name}: {other:?}, allotropy {v} {u}"));
                    }
                }
            }
        };
        conc("mass concentration", "concentration", &mut problems);
        conc(
            "original sample concentration",
            "sample_concentration",
            &mut problems,
        );
        conc(
            "qubit tube concentration",
            "tube_concentration",
            &mut problems,
        );
        for (wl, col) in [(260.0, "a260"), (280.0, "a280")] {
            if let Some((_, v, _, _)) = values
                .iter()
                .find(|x| x.0 == "absorbance" && x.3 == Some(wl))
            {
                // a custom method stores pairs `wavelength_k`, `absorbance_k`
                let pair = (1..=4).find_map(|k| {
                    let w = ours.num(&format!("wavelength_{k}"), i)?.0;
                    (w == wl).then(|| ours.num(&format!("absorbance_{k}"), i))?
                });
                match ours.num(col, i).or(pair) {
                    Some((o, _)) if close(o, *v) => checked += 1,
                    other => problems.push(format!("row {i} {col}: {other:?}, allotropy {v}")),
                }
            }
        }
        if let Some((_, v, _, _)) = values.iter().find(|x| x.0 == "fluorescence") {
            let any = ["sample_rfu", "green_rfu", "far_red_rfu"]
                .iter()
                .filter_map(|c| ours.num(c, i))
                .any(|(o, _)| close(o, *v));
            if any {
                checked += 1;
            } else {
                problems.push(format!("row {i}: no RFU column holds allotropy's {v}"));
            }
        }
        let ids: Vec<&str> = g.iter().filter_map(|m| m["id"].as_str()).collect();
        for c in calc {
            let mine = c["sources"].as_array().is_some_and(|s| {
                s.iter()
                    .any(|x| x.as_str().is_some_and(|x| ids.contains(&x)))
            });
            if !mine {
                continue;
            }
            let name = c["name"].as_str().unwrap_or_default();
            let col = if name.contains("280") {
                "a260_a280"
            } else if name.contains("230") {
                "a260_a230"
            } else {
                continue;
            };
            let v = c["value"].as_f64().unwrap_or(f64::NAN);
            // a custom method's formula `260/280`
            let formula = (ours.text("formula_name_1", i).as_deref() == Some(name))
                .then(|| ours.num("formula_value_1", i))
                .flatten();
            match ours.num(col, i).or(formula) {
                Some((o, _)) if close(o, v) => checked += 1,
                other => problems.push(format!("row {i} {col}: {other:?}, allotropy {v}")),
            }
        }
    }
    // instrument
    if let Some(e) = ds.experiment() {
        let ins = e.instrument.unwrap_or_default();
        if let Some(m) = oracle["model"].as_str()
            && !ins
                .model
                .as_deref()
                .is_some_and(|o| o.eq_ignore_ascii_case(m))
        {
            problems.push(format!("model {:?}, allotropy {m:?}", ins.model));
        }
        if let Some(s) = oracle["serial"].as_str()
            && ins.serial.as_deref() != Some(s)
        {
            problems.push(format!("serial {:?}, allotropy {s:?}", ins.serial));
        }
    }
    // spectra
    let mut spectra = 0usize;
    if let Some(sp) = oracle["spectra"].as_array() {
        let tr = info.traces.first().ok_or("no spectra trace")?;
        if tr.sweep_count as usize != sp.len() {
            problems.push(format!("{} spectra, pandas {}", tr.sweep_count, sp.len()));
        }
        let axis = &tr.extra["axis"];
        for (k, s) in sp.iter().enumerate().take(tr.sweep_count as usize) {
            let trace = ds
                .read_trace(0, k as u32, 0, u64::MAX)
                .map_err(|e| format!("trace read failed: {e}"))?;
            let y = &trace.channels[0];
            if y.len() as u64 != s["points"].as_u64().unwrap_or(0) {
                problems.push(format!(
                    "spectrum {k}: {} points, pandas {}",
                    y.len(),
                    s["points"]
                ));
                continue;
            }
            let first = axis["first"].as_f64().unwrap_or(f64::NAN);
            let step = axis["step"].as_f64().unwrap_or(f64::NAN);
            for p in s["samples"].as_array().unwrap_or(&empty) {
                let i = p[0].as_u64().unwrap_or(0) as usize;
                let (x, v) = (
                    p[1].as_f64().unwrap_or(f64::NAN),
                    p[2].as_f64().unwrap_or(f64::NAN),
                );
                let ox = first + step * i as f64;
                if !(close(ox, x) && close(y[i], v)) {
                    problems.push(format!(
                        "spectrum {k} point {i}: ({ox}, {}) vs ({x}, {v})",
                        y[i]
                    ));
                    break;
                }
            }
            if let Some(name) = s["name"].as_str() {
                let ours_name = ours.text("sample", k);
                if ours_name.as_deref().map(str::trim) != Some(name.trim()) {
                    problems.push(format!(
                        "spectrum {k}: sample {ours_name:?}, pandas {name:?}"
                    ));
                }
            }
            spectra += 1;
        }
    }
    if problems.is_empty() {
        Ok(format!(
            "{n} rows; {checked} sample names and values equal allotropy's; {spectra} spectra equal pandas'"
        ))
    } else {
        Err(problems.join("; "))
    }
}

/// Two measurement documents at the same detector wavelength belong to different samples.
fn same_wavelength(a: &Value, b: &Value) -> bool {
    let wl = |m: &Value| {
        m["values"].as_array().and_then(|v| {
            v.iter()
                .find(|x| x[0] == "detector wavelength setting")
                .and_then(|x| x[1].as_f64())
        })
    };
    matches!((wl(a), wl(b)), (Some(x), Some(y)) if x == y)
}
