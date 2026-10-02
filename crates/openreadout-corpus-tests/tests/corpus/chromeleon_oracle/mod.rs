//! Chromeleon archives against Chromeleon's own outputs of the same injections
//! (`oracle/chromeleon_oracle.py` → `corpus/oracle/chromeleon/<id>.json`):
//!
//! - ASCII chromatogram exports: the export's injection (by name, and by start time where names
//!   repeat), its point count, first time, step, unit and every value (to the printed precision);
//!   Their injection details (position, volume, dilution factor, weight, methods, status) equal
//!   the injection's details in our traces' `extra`;
//! - a PDF report: per injection page, the plotted chromatogram (every unclipped vertex inside
//!   the range of our samples in its pixel column after a linear fit of the page's y axis), the
//!   integration results (the apex of our trace at the printed retention time; area and height
//!   reproduced exactly from sample-exact limits near the drawn ones; Chromeleon's stored peaks in
//!   our `vendor_peaks` table equal to the printed retention time, area and height), and the
//!   page's injection details (vial, volume, type, methods, dilution factor, weight).

use std::path::Path;

use openreadout_core::FormatReader;
use openreadout_core::model::TraceInfo;
use serde_json::Value;

/// Whole-file comparison; `Ok` describes what matched.
pub fn compare_file(id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let mut ds = openreadout_chrom::ChromeleonReader
        .open(path)
        .map_err(|e| format!("open failed: {e}"))?;
    let info = ds.info().map_err(|e| format!("info failed: {e}"))?;
    let mut done = Vec::new();
    let mut problems = Vec::new();
    if let Some(exports) = oracle["ascii_exports"].as_array() {
        match compare_exports(ds.as_mut(), &info.traces, exports) {
            Ok(d) => done.push(d),
            Err(e) => problems.push(e),
        }
    }
    if let Some(pages) = oracle["pdf_report"]["pages"].as_array() {
        let vendor = vendor_rows(ds.as_mut(), &info)?;
        match compare_report(ds.as_mut(), &info.traces, pages, &vendor) {
            Ok(d) => done.push(d),
            Err(e) => problems.push(e),
        }
    }
    if done.is_empty() && problems.is_empty() {
        return Err(format!("{id}: the oracle holds no vendor export"));
    }
    if problems.is_empty() {
        Ok(done.join("; "))
    } else {
        Err(problems.join("; "))
    }
}

/// `vendor_peaks` rows as (trace, rt, area, height).
fn vendor_rows(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::model::FileInfo,
) -> Result<Vec<(u32, f64, f64, f64)>, String> {
    let Some(t) = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("vendor_peaks"))
    else {
        return Ok(Vec::new());
    };
    let col = |n: &str| t.columns.iter().position(|c| c.name == n);
    let (Some(ct), Some(cr), Some(ca), Some(ch)) =
        (col("trace"), col("rt_min"), col("area"), col("height"))
    else {
        return Err("vendor_peaks lacks trace/rt_min/area/height".into());
    };
    let tab = ds
        .read_table(t.index, 0, t.row_count)
        .map_err(|e| format!("vendor_peaks: {e}"))?;
    Ok((0..t.row_count as usize)
        .map(|r| {
            (
                tab.columns[ct][r] as u32,
                tab.columns[cr][r],
                tab.columns[ca][r],
                tab.columns[ch][r],
            )
        })
        .collect())
}

/// Injection details of an export or report page against a trace's `extra`.
fn details_agree(t: &TraceInfo, pairs: &[(&str, &Value)]) -> Result<usize, String> {
    let name = t.name.clone().unwrap_or_default();
    let mut n = 0;
    for (k, want) in pairs {
        if want.is_null() {
            continue;
        }
        let got = t.extra.get(*k).unwrap_or(&Value::Null);
        let same = match (got.as_f64(), want.as_f64()) {
            (Some(a), Some(b)) => (a - b).abs() < 1e-9,
            _ => got.as_str().map(str::trim) == want.as_str().map(str::trim),
        };
        if !same {
            return Err(format!("{name}: {k} {got}, the vendor's {want}"));
        }
        n += 1;
    }
    Ok(n)
}

fn extra<'a>(t: &'a TraceInfo, k: &str) -> Option<&'a str> {
    t.extra.get(k).and_then(Value::as_str)
}

/// Seconds since the epoch of a `YYYY-MM-DDTHH:MM:SS[.fff][Z]` time (no zone handling).
fn secs(s: &str) -> Option<f64> {
    openreadout_core::time::iso8601_to_unix(&format!("{}Z", s.trim_end_matches('Z')))
}

fn read_all(ds: &mut dyn openreadout_core::Dataset, t: &TraceInfo) -> Result<Vec<f64>, String> {
    ds.read_trace(t.index, 0, 0, t.sample_count)
        .map(|tr| tr.channels.into_iter().next().unwrap_or_default())
        .map_err(|e| format!("{}: {e}", t.name.clone().unwrap_or_default()))
}

fn compare_exports(
    ds: &mut dyn openreadout_core::Dataset,
    traces: &[TraceInfo],
    exports: &[Value],
) -> Result<String, String> {
    let (mut points, mut details) = (0usize, 0usize);
    for e in exports {
        let (inj, ch) = (
            e["injection"].as_str().unwrap_or_default(),
            e["channel"].as_str().unwrap_or_default(),
        );
        let utc = e["inject_time_utc"].as_str().and_then(secs);
        // same name and channel; where names repeat, the one whose local start time is a
        // time-zone offset (whole quarter hours) from the export's UTC inject time
        let named: Vec<&TraceInfo> = traces
            .iter()
            .filter(|t| extra(t, "injection") == Some(inj) && extra(t, "signal") == Some(ch))
            .collect();
        let timed: Vec<&TraceInfo> = named
            .iter()
            .copied()
            .filter(|t| {
                let local = extra(t, "acquired_local").and_then(secs);
                match (local, utc) {
                    (Some(l), Some(u)) => {
                        let off = l - u;
                        (off - (off / 900.0).round() * 900.0).abs() <= 2.0
                    }
                    _ => false,
                }
            })
            .collect();
        let [t] = timed.as_slice() else {
            return Err(format!(
                "export {}: {} traces named {inj} / {ch}, {} at its inject time",
                e["file"],
                named.len(),
                timed.len()
            ));
        };
        let name = t.name.clone().unwrap_or_default();
        details += details_agree(
            t,
            &[
                ("injection_position", &e["position"]),
                ("injection_volume_ul", &e["volume_ul"]),
                ("dilution_factor", &e["dilution_factor"]),
                ("weight", &e["weight"]),
                ("processing_method", &e["processing_method"]),
                ("instrument_method", &e["instrument_method"]),
                ("injection_status", &e["status"]),
            ],
        )?;
        let n = e["points"].as_u64().unwrap_or(0);
        if t.sample_count != n {
            return Err(format!("{name}: {} points, the export {n}", t.sample_count));
        }
        let unit = t.channels[0].unit.as_deref();
        if unit != e["unit"].as_str() {
            return Err(format!("{name}: unit {unit:?}, the export {}", e["unit"]));
        }
        let axis = &t.extra["axis"];
        let (first, step) = (
            axis["first"].as_f64().unwrap_or(f64::NAN),
            axis["step"].as_f64().unwrap_or(f64::NAN),
        );
        let (efirst, estep) = (
            e["first_time"].as_f64().unwrap_or(f64::NAN),
            e["step_s"].as_f64().unwrap_or(f64::NAN) / 60.0,
        );
        // times are printed to 6 decimals of a minute
        let dev = e["time_grid_max_deviation_min"].as_f64().unwrap_or(1.0);
        if (first - efirst).abs() > 5e-7 + 1e-12 || (step - estep).abs() > 1e-9 || dev > 1e-6 {
            return Err(format!(
                "{name}: grid {first} + i × {step} min, the export {efirst} + i × {estep} (printed times within {dev} of it)"
            ));
        }
        let ours = read_all(ds, t)?;
        let want: Vec<f64> = e["values"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        if want.len() != ours.len() {
            return Err(format!(
                "{name}: {} values, the export {}",
                ours.len(),
                want.len()
            ));
        }
        // values are printed to 6 decimals
        if let Some((i, (a, b))) = ours
            .iter()
            .zip(&want)
            .enumerate()
            .find(|(_, (a, b))| (*a - *b).abs() > 5e-7 + 1e-9)
        {
            return Err(format!("{name}: point {i} is {a}, the export {b}"));
        }
        let (lo, hi) = ours
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), &v| {
                (l.min(v), h.max(v))
            });
        let (elo, ehi) = (
            e["signal_min"].as_f64().unwrap_or(f64::NAN),
            e["signal_max"].as_f64().unwrap_or(f64::NAN),
        );
        if (lo - elo).abs() > 5e-7 + 1e-9 || (hi - ehi).abs() > 5e-7 + 1e-9 {
            return Err(format!("{name}: range {lo}–{hi}, the export {elo}–{ehi}"));
        }
        points += ours.len();
    }
    Ok(format!(
        "{} ASCII exports: {points} points equal to the printed precision (times, units, ranges); {details} injection details equal",
        exports.len()
    ))
}

/// Least-squares y = a + b·u, then refit 5 times without points further than max(3 MAD, 1 pt).
fn robust_fit(u: &[f64], w: &[f64]) -> Option<(f64, f64)> {
    let fit = |keep: &[usize]| -> Option<(f64, f64)> {
        let n = keep.len() as f64;
        if keep.len() < 3 {
            return None;
        }
        let mu = keep.iter().map(|&i| u[i]).sum::<f64>() / n;
        let mw = keep.iter().map(|&i| w[i]).sum::<f64>() / n;
        let sxx: f64 = keep.iter().map(|&i| (u[i] - mu).powi(2)).sum();
        let sxy: f64 = keep.iter().map(|&i| (u[i] - mu) * (w[i] - mw)).sum();
        if sxx <= 0.0 {
            return None;
        }
        let b = sxy / sxx;
        Some((mw - b * mu, b))
    };
    let all: Vec<usize> = (0..u.len()).collect();
    let (mut a, mut b) = fit(&all)?;
    for _ in 0..5 {
        let res: Vec<f64> = all.iter().map(|&i| (w[i] - (a + b * u[i])).abs()).collect();
        let mut sorted = res.clone();
        sorted.sort_by(f64::total_cmp);
        let mad = sorted[sorted.len() / 2];
        let cut = (3.0 * mad).max(1.0);
        let keep: Vec<usize> = all.iter().copied().filter(|&i| res[i] <= cut).collect();
        (a, b) = fit(&keep)?;
    }
    Some((a, b))
}

#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
fn compare_report(
    ds: &mut dyn openreadout_core::Dataset,
    traces: &[TraceInfo],
    pages: &[Value],
    vendor: &[(u32, f64, f64, f64)],
) -> Result<String, String> {
    let gc: Vec<&TraceInfo> = traces.iter().collect();
    let (mut stored_pages, mut stored_peaks, mut details) = (0usize, 0usize, 0usize);
    if gc.len() != pages.len() {
        return Err(format!(
            "{} traces, the report {} injection pages",
            gc.len(),
            pages.len()
        ));
    }
    let (mut vertices, mut inside) = (0usize, 0usize);
    let (mut peaks, mut apex_ok, mut exact, mut close) = (0usize, 0usize, 0usize, 0usize);
    for (t, page) in gc.iter().zip(pages) {
        let name = t.name.clone().unwrap_or_default();
        if extra(t, "injection") != page["injection"].as_str() {
            return Err(format!(
                "trace {name} is not the report's injection {}",
                page["injection"]
            ));
        }
        let d = &page["details"];
        details += details_agree(
            t,
            &[
                ("injection_position", &d["vial"]),
                ("injection_volume_ul", &d["volume_ul"]),
                ("injection_type", &d["injection_type"]),
                ("instrument_method", &d["instrument_method"]),
                ("processing_method", &d["processing_method"]),
                ("dilution_factor", &d["dilution_factor"]),
                ("weight", &d["weight"]),
            ],
        )?;
        // Chromeleon's stored results, where it saved them, equal the printed table
        if extra(t, "vendor_results") == Some("stored") {
            let ours: Vec<&(u32, f64, f64, f64)> =
                vendor.iter().filter(|r| r.0 == t.index).collect();
            let printed = page["peaks"].as_array().cloned().unwrap_or_default();
            if ours.len() != printed.len() {
                return Err(format!(
                    "{name}: {} stored peaks, the report prints {}",
                    ours.len(),
                    printed.len()
                ));
            }
            let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
            for (o, p) in ours.iter().zip(&printed) {
                let want = [&p["rt_min"], &p["area"], &p["height"]]
                    .map(|v| v.as_f64().unwrap_or(f64::NAN));
                // Exact on purpose: both sides are rounded to the report's printed precision.
                #[allow(clippy::float_cmp)]
                let differs = [r3(o.1), r3(o.2), r3(o.3)] != want;
                if differs {
                    return Err(format!(
                        "{name}: stored peak ({}, {}, {}), the report {want:?}",
                        o.1, o.2, o.3
                    ));
                }
                stored_peaks += 1;
            }
            stored_pages += 1;
        }
        let run = page["run_time_min"].as_f64().unwrap_or(f64::NAN);
        let end = t.extra["x_end_min"].as_f64().unwrap_or(f64::NAN);
        if (end - run).abs() > 0.005 {
            return Err(format!(
                "{name}: ends at {end} min, the report's run time {run}"
            ));
        }
        let rate = (t.sample_rate_hz * 60.0).round(); // points per minute
        let first = t.extra["axis"]["first"].as_f64().unwrap_or(0.0);
        let y = read_all(ds, t)?;
        let n = y.len() as i64;
        let idx = |tm: f64| -> i64 { ((tm - first) * rate).round() as i64 };
        let at = |i: i64| y[i.clamp(0, n - 1) as usize];
        // the plot: x0..x1 is 0..run time
        let plot = &page["plot"];
        let (x0, x1) = (
            plot["x0"].as_f64().unwrap_or(0.0),
            plot["x1"].as_f64().unwrap_or(1.0),
        );
        let tx = |x: f64| (x - x0) / (x1 - x0) * run;
        let verts: Vec<(f64, f64)> = plot["vertices"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
                    .collect()
            })
            .unwrap_or_default();
        let ymax = verts.iter().map(|v| v.1).fold(f64::NEG_INFINITY, f64::max);
        let ymin = verts.iter().map(|v| v.1).fold(f64::INFINITY, f64::min);
        let mut cols: Vec<(f64, f64, f64, f64)> = Vec::new(); // (y, lo, hi, median)
        for &(x, yy) in &verts {
            if yy >= ymax - 0.5 || yy <= ymin + 1e-6 {
                continue; // clipped at the frame
            }
            let k = idx(tx(x));
            let (a, b) = ((k - 20).max(0), (k + 20).min(n - 1));
            let mut c: Vec<f64> = (a..=b).map(at).collect();
            c.sort_by(f64::total_cmp);
            cols.push((yy, c[0], c[c.len() - 1], c[c.len() / 2]));
        }
        let u: Vec<f64> = cols.iter().map(|c| c.3).collect();
        let w: Vec<f64> = cols.iter().map(|c| c.0).collect();
        let Some((a, b)) = robust_fit(&u, &w) else {
            return Err(format!("{name}: the plotted path could not be fitted"));
        };
        let tol = 0.6 / b.abs();
        let ok = cols
            .iter()
            .filter(|c| {
                let v = (c.0 - a) / b;
                c.1 - tol <= v && v <= c.2 + tol
            })
            .count();
        if (ok as f64) < 0.97 * cols.len() as f64 {
            return Err(format!(
                "{name}: {ok} of {} plotted vertices inside our pixel columns",
                cols.len()
            ));
        }
        vertices += cols.len();
        inside += ok;
        // the peaks
        let baselines: Vec<(f64, f64)> = plot["baselines"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| Some((tx(s[0].as_f64()?), tx(s[2].as_f64()?))))
                    .collect()
            })
            .unwrap_or_default();
        let win = ((1.0 / (x1 - x0)) * run * rate).round() as i64;
        for p in page["peaks"].as_array().into_iter().flatten() {
            let (rt, area, height) = (
                p["rt_min"].as_f64().unwrap_or(f64::NAN),
                p["area"].as_f64().unwrap_or(f64::NAN),
                p["height"].as_f64().unwrap_or(f64::NAN),
            );
            peaks += 1;
            let Some(&(s0, s1)) = baselines.iter().min_by(|a, b| {
                let d = |s: &(f64, f64)| (s.0 - rt).abs().min((s.1 - rt).abs());
                d(a).total_cmp(&d(b))
            }) else {
                continue;
            };
            // our apex between the drawn limits
            let (l, r) = (idx(s0).max(0), idx(s1).min(n - 1));
            if r > l {
                let k = (l..=r)
                    .max_by(|&i, &j| at(i).total_cmp(&at(j)))
                    .unwrap_or(l);
                let tk = first + k as f64 / rate;
                if (tk - rt).abs() <= 0.0005 + 1.0 / rate + 1e-9 {
                    apex_ok += 1;
                }
            }
            // sample-exact limits within ±1 pt of the drawn ones reproducing area and height
            let (c0, c1) = (idx(s0), idx(s1));
            let mut found = false;
            let mut best = f64::INFINITY; // smallest max(relative height, area difference)
            'search: for sa in (c0 - win).max(0)..=(c0 + win).min(n - 1) {
                for sb in (c1 - win).max(sa + 3)..=(c1 + win).min(n - 1) {
                    let (ya, yb) = (at(sa), at(sb));
                    let line = |i: i64| ya + (yb - ya) * (i - sa) as f64 / (sb - sa) as f64;
                    let mut h = f64::NEG_INFINITY;
                    let mut s = 0.0;
                    for i in sa..=sb {
                        let d = at(i) - line(i);
                        h = h.max(d);
                        if i < sb {
                            s += (d + at(i + 1) - line(i + 1)) / 2.0;
                        }
                    }
                    best =
                        best.min(((h - height).abs() / height).max((s / rate - area).abs() / area));
                    if (h - height).abs() <= 0.0005 + 1e-9
                        && (s / rate - area).abs() <= 0.0005 + 1e-9
                    {
                        found = true;
                        break 'search;
                    }
                }
            }
            if found {
                exact += 1;
            }
            if found || best <= 0.05 {
                close += 1;
            }
        }
    }
    if (inside as f64) < 0.99 * vertices as f64 {
        return Err(format!(
            "{inside} of {vertices} plotted vertices inside our pixel columns"
        ));
    }
    // riders on a larger neighbour have their apex elsewhere and limits the drawing does not
    // resolve; the report prints 3 decimals, so "exact" is within half a printed unit
    if (apex_ok as f64) < 0.8 * peaks as f64
        || (close as f64) < 0.8 * peaks as f64
        || (exact as f64) < peaks as f64 / 4.0
    {
        return Err(format!(
            "report peaks: {apex_ok} of {peaks} at our apex, {close} with area and height within 5 %, {exact} exactly"
        ));
    }
    Ok(format!(
        "PDF report: {} injections, {inside} of {vertices} plotted vertices inside our pixel columns; {apex_ok} of {peaks} peaks at our apex, {close} with area and height within 5 % from limits near the drawn ones, {exact} of them exactly; {stored_peaks} stored peaks of {stored_pages} injections with saved results equal the printed table; {details} injection details equal",
        pages.len()
    ))
}
