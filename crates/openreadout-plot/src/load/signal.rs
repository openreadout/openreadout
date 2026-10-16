//! Signals: traces and sweeps (with spikes), chromatograms, mass spectra and f–I curves.

use std::path::Path;

use openreadout_batch::analyze::{AnalyzeArgs, AnalyzeKind, AnalyzeOutput};
use openreadout_core::model::TraceInfo;
use openreadout_core::{Dataset, Result, ScanFilter, SpectrumView};
use serde_json::{Map, Value, json};

use super::{Ctx, top_peaks, usage};
use crate::model::{Callout, Geom, Quantity, Series, Source, Weight};
use crate::spec::{MarkKind, SeriesSpec, Show};

/// Most samples read per channel and sweep.
const MAX_SAMPLES: usize = 4_000_000;
/// Most sweeps drawn from one file when every sweep is asked for.
const MAX_SWEEPS: usize = 64;

fn analyze(ctx: &Ctx<'_>, file: &str, kind: AnalyzeKind, options: Value) -> Result<AnalyzeOutput> {
    let options = match options {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let args = AnalyzeArgs {
        file: file.to_string(),
        kind,
        options,
        strict: None,
    };
    Ok(openreadout_batch::analyze::run(ctx.reg, &args)?.0)
}

fn trace_list(info: &openreadout_core::FileInfo) -> String {
    info.traces
        .iter()
        .take(12)
        .map(|t| match &t.name {
            Some(n) => format!("{} ({n})", t.index),
            None => t.index.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn str_of<'a>(v: &'a serde_json::Map<String, Value>, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// Traces whose series are named by channel (a dye or a base), not by the trace: the dye
/// channels of an electropherogram or Sanger trace.
fn named_by_channel(t: &TraceInfo) -> bool {
    matches!(trace_kind(t), "electropherogram" | "sanger_trace")
}

fn trace_kind(t: &TraceInfo) -> &str {
    t.extra.get("kind").and_then(Value::as_str).unwrap_or("")
}

/// The x values of samples `0..n` of a sweep, and what they are.
fn abscissa(
    ds: &mut dyn Dataset,
    t: &TraceInfo,
    sweep: u32,
    n: usize,
) -> Result<(Quantity, Vec<f64>, Option<usize>)> {
    if let Some(a) = t.extra.get("axis").and_then(Value::as_object) {
        let num = |k: &str| a.get(k).and_then(Value::as_f64);
        let q = Quantity::new(str_of(a, "quantity").unwrap_or("x"), str_of(a, "unit"));
        if a.get("irregular").and_then(Value::as_bool) == Some(true)
            && let Some(c) = a.get("channel").and_then(Value::as_u64)
        {
            let c = c as usize;
            let v = openreadout_core::trace::read_channels(
                ds,
                t.index,
                sweep,
                0,
                n as u64,
                &[c as u32],
            )?;
            return Ok((q, v.into_iter().next().unwrap_or_default(), Some(c)));
        }
        let first = num("first");
        let step = num("step").or_else(|| {
            let (f, l) = (num("first")?, num("last")?);
            (n > 1).then(|| (l - f) / (n - 1) as f64)
        });
        if let (Some(f), Some(s)) = (first, step) {
            let q = if q
                .unit
                .as_deref()
                .and_then(openreadout_core::trace::seconds_per_unit)
                .is_some()
            {
                // a time axis in minutes (chromatograms): seconds, the panel picks the unit
                let k = q
                    .unit
                    .as_deref()
                    .and_then(openreadout_core::trace::seconds_per_unit)
                    .unwrap_or(1.0);
                let name = if q.name == "x" {
                    "time".to_string()
                } else {
                    q.name.clone()
                };
                let name = if name.contains("retention") {
                    "retention time".into()
                } else if name == "time" {
                    name
                } else {
                    "time".into()
                };
                return Ok((
                    Quantity::new(name, Some("s")),
                    (0..n).map(|i| (f + s * i as f64) * k).collect(),
                    None,
                ));
            } else {
                q
            };
            return Ok((q, (0..n).map(|i| f + s * i as f64).collect(), None));
        }
    }
    if t.sample_rate_hz > 0.0 && t.sample_rate_hz.is_finite() {
        let t0 = openreadout_core::trace::sweep_origin_s(t);
        let r = t.sample_rate_hz;
        return Ok((
            Quantity::new("time", Some("s")),
            (0..n).map(|i| t0 + i as f64 / r).collect(),
            None,
        ));
    }
    Ok((
        Quantity::new("sample", None),
        (0..n).map(|i| i as f64).collect(),
        None,
    ))
}

/// A channel's quantity: its name, or what its unit measures when the name is only an input
/// number (`IN 0`, `AI2`, `ADC1`).
fn signal_name(name: &str, unit: Option<&str>) -> String {
    let n = name.trim();
    let generic = {
        let letters: String = n.chars().take_while(char::is_ascii_alphabetic).collect();
        let rest = n[letters.len()..].trim();
        matches!(
            letters.to_ascii_uppercase().as_str(),
            "IN" | "AI" | "ADC" | "CH" | "CHANNEL"
        ) && !rest.is_empty()
            && rest
                .chars()
                .all(|c| c.is_ascii_digit() || c == ' ' || c == '_')
    };
    if !generic {
        return n.to_string();
    }
    match unit.map(str::to_ascii_lowercase).as_deref() {
        Some("mv" | "v" | "uv" | "µv") => "voltage".into(),
        Some("pa" | "na" | "a" | "ua" | "µa") => "current".into(),
        _ => n.to_string(),
    }
}

fn reversed_axis(q: &Quantity) -> bool {
    let u = q.unit.as_deref().unwrap_or("").to_ascii_lowercase();
    let n = q.name.to_ascii_lowercase();
    u.contains("ppm") || u.contains("cm-1") || u == "1/cm" || n.contains("wavenumber")
}

/// A short label for a position on an axis.
pub(crate) fn fmt_at(q: &Quantity, v: f64) -> String {
    let n = q.name.as_str();
    let s = if n == "m/z" {
        trim(format!("{v:.4}"))
    } else if q.is_time() {
        return format!("{} min", trim(format!("{:.2}", v / 60.0)));
    } else if q.unit.as_deref().is_some_and(|u| u.contains("ppm")) {
        trim(format!("{v:.2}"))
    } else if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        trim(format!("{v:.3}"))
    };
    match &q.unit {
        Some(u) if n != "m/z" && !u.contains("ppm") && !u.contains("cm-1") => format!("{s} {u}"),
        _ => s,
    }
}

fn trim(s: String) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// Traces, sweeps and qPCR curves.
pub(super) fn trace(
    ctx: &mut Ctx<'_>,
    spec: &SeriesSpec,
    file: &str,
    show: Show,
) -> Result<Vec<Series>> {
    let (_, ds, info) = ctx.open(file)?;
    let (mut ds, info): (Box<dyn Dataset>, _) = if spec.process {
        let p = openreadout_signal::nmr::ProcessedNmrDataset::new(
            ds,
            openreadout_signal::nmr::ProcessOptions::default(),
        )?;
        let i = p.info()?;
        (Box::new(p), i)
    } else {
        (ds, info)
    };
    if info.traces.is_empty() {
        return Err(usage(format!(
            "{file}: no traces (signals or 1-D spectra) to plot; mass spectra use show=chromatogram or spectrum, plates show=wells"
        )));
    }
    let ti = match (spec.trace, show) {
        (Some(t), _) => t,
        (None, Show::Amplification) => pick_kind(
            &info,
            &["amplification baseline-corrected", "amplification"],
        )
        .ok_or_else(|| {
            usage(format!(
                "{file}: no amplification curves; traces: {}",
                trace_list(&info)
            ))
        })?,
        (None, Show::Melt) => pick_kind(&info, &["melt derivative", "melt"]).ok_or_else(|| {
            usage(format!(
                "{file}: no melt curves; traces: {}",
                trace_list(&info)
            ))
        })?,
        // a processed spectrum (a ppm axis) before the FID it came from
        _ => {
            info.traces
                .iter()
                .find(|t| {
                    t.extra
                        .get("axis")
                        .and_then(|a| a.get("unit"))
                        .and_then(Value::as_str)
                        .is_some_and(|u| u.contains("ppm"))
                })
                .unwrap_or(&info.traces[0])
                .index
        }
    };
    let t = info
        .traces
        .iter()
        .find(|t| t.index == ti)
        .cloned()
        .ok_or_else(|| {
            usage(format!(
                "{file}: no trace {ti}; it has traces {}",
                trace_list(&info)
            ))
        })?;
    let overlay = t.extra.get("plot").and_then(Value::as_str) == Some("overlay");
    let sweeps: Vec<u32> = match &spec.sweep {
        Some(s) => s.to_vec(),
        None if show == Show::Sweeps => (0..t.sweep_count.min(MAX_SWEEPS as u32)).collect(),
        None => vec![0],
    };
    if let Some(&bad) = sweeps.iter().find(|&&s| s >= t.sweep_count.max(1)) {
        return Err(usage(format!(
            "{file}: trace {ti} has {} sweep(s) (0–{}); sweep {bad} is out of range",
            t.sweep_count,
            t.sweep_count.saturating_sub(1)
        )));
    }
    // channels: one per qPCR well for overlays, else the one asked for
    let mut channels: Vec<u32> = Vec::new();
    if overlay && spec.channel.is_none() {
        let wells = spec
            .wells
            .as_deref()
            .map(openreadout_assay::layout::parse_wells)
            .transpose()
            .map_err(|m| usage(format!("wells: {m}")))?;
        for c in &t.channels {
            let well = c.extra.get("well").and_then(Value::as_str);
            let target = c.extra.get("target").and_then(Value::as_str);
            if let Some(w) = &wells
                && !well
                    .and_then(openreadout_assay::layout::parse_well)
                    .is_some_and(|p| w.contains(&p))
            {
                continue;
            }
            if let Some(want) = &spec.target
                && !target.is_some_and(|x| x.eq_ignore_ascii_case(want))
            {
                continue;
            }
            channels.push(c.index);
        }
        if channels.is_empty() {
            let targets: std::collections::BTreeSet<&str> = t
                .channels
                .iter()
                .filter_map(|c| c.extra.get("target").and_then(Value::as_str))
                .collect();
            return Err(usage(format!(
                "{file}: no curves match wells/target; targets: {}",
                targets.into_iter().collect::<Vec<_>>().join(", ")
            )));
        }
    } else {
        let c = spec.channel.unwrap_or_else(|| {
            if show == Show::Sweeps || spec.spikes {
                t.channels
                    .iter()
                    .find(|c| {
                        c.unit
                            .as_deref()
                            .is_some_and(|u| u.eq_ignore_ascii_case("mv"))
                    })
                    .map_or(0, |c| c.index)
            } else {
                // the first signal: an irregular abscissa stored as a channel (a sized
                // electropherogram's bp, a Raman shift list) is the x axis, not a signal
                let axis = t
                    .extra
                    .get("axis")
                    .filter(|a| a.get("irregular").and_then(Value::as_bool) == Some(true))
                    .and_then(|a| a.get("channel"))
                    .and_then(Value::as_u64);
                t.channels
                    .iter()
                    .find(|c| Some(u64::from(c.index)) != axis)
                    .map_or(0, |c| c.index)
            }
        });
        if !t.channels.iter().any(|ch| ch.index == c) {
            return Err(usage(format!(
                "{file}: trace {ti} has channels {}",
                t.channels
                    .iter()
                    .map(|c| format!("{} ({})", c.index, c.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        channels.push(c);
    }

    // per-sweep stimulus and spikes from the patch-clamp analysis
    let mut stim: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
    let mut spikes: std::collections::HashMap<u32, Vec<(f64, f64)>> =
        std::collections::HashMap::new();
    if (show == Show::Sweeps || spec.spikes) && !overlay {
        let mut o = json!({"trace": ti, "sweeps": sweeps, "max_spikes": 1_000_000});
        if let Some(c) = spec.channel {
            o["channel"] = json!(c);
        }
        match analyze(ctx, file, AnalyzeKind::EphysFeatures, o) {
            Ok(AnalyzeOutput::EphysFeatures(r)) => {
                for s in &r.sweeps {
                    if let Some(p) = s.stimulus_pa {
                        stim.insert(s.sweep, p);
                    }
                }
                for sp in &r.spikes {
                    spikes
                        .entry(sp.sweep)
                        .or_default()
                        .push((sp.peak_time_s, sp.peak_mv));
                }
            }
            Ok(_) => {}
            Err(e) if spec.spikes => return Err(e),
            Err(_) => {}
        }
    }

    let n_total = openreadout_core::trace::sweep_samples(&t, 0) as usize;
    let mut out = Vec::new();
    for &sw in &sweeps {
        let n_sw = (openreadout_core::trace::sweep_samples(&t, sw) as usize)
            .max(usize::from(n_total != 0));
        let n_read = n_sw.min(MAX_SAMPLES);
        ctx.spend(n_read.saturating_mul(channels.len().max(1)))?;
        if n_sw > MAX_SAMPLES {
            ctx.warnings.push(format!(
                "{}: sweep {sw} has {n_sw} samples; drew the first {MAX_SAMPLES} (set x_range to choose a window)",
                super::file_label(file)
            ));
        }
        let (xq, xs, axis_channel) = abscissa(ds.as_mut(), &t, sw, n_read)?;
        // the window asked for
        let (i0, i1) = match spec.x_range {
            Some([a, b]) => {
                let (lo, hi) = (a.min(b), a.max(b));
                // x_range is in the axis' units; time axes are given in the unit the trace stores
                let k = if xq.is_time() { time_factor(&t) } else { 1.0 };
                let idx: Vec<usize> = xs
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| **x / k >= lo && **x / k <= hi)
                    .map(|(i, _)| i)
                    .collect();
                match (idx.first(), idx.last()) {
                    (Some(&f), Some(&l)) => (f, l + 1),
                    _ => {
                        return Err(usage(format!(
                            "{file}: x_range [{lo}, {hi}] holds no samples; the axis runs {} to {}",
                            xs.first().map_or(0.0, |v| v / k),
                            xs.last().map_or(0.0, |v| v / k)
                        )));
                    }
                }
            }
            None => (0, n_read),
        };
        let data = openreadout_core::trace::read_channels(
            ds.as_mut(),
            ti,
            sw,
            i0 as u64,
            (i1 - i0) as u64,
            &channels,
        )?;
        for (k, &c) in channels.iter().enumerate() {
            if axis_channel == Some(c as usize) {
                continue;
            }
            let Some(ch) = t.channels.iter().find(|ch| ch.index == c) else {
                continue;
            };
            let ys = &data[k];
            let pts: Vec<(f64, f64)> = xs[i0..i0 + ys.len().min(i1 - i0)]
                .iter()
                .copied()
                .zip(ys.iter().copied())
                .collect();
            let yq = if overlay {
                Quantity::new(
                    t.extra.get("quantity").and_then(Value::as_str).unwrap_or(
                        if trace_kind(&t).contains("derivative") {
                            "−dF/dT"
                        } else {
                            "fluorescence"
                        },
                    ),
                    ch.unit.as_deref(),
                )
            } else {
                Quantity::new(
                    signal_name(&ch.name, ch.unit.as_deref()),
                    ch.unit.as_deref(),
                )
            };
            let name = if overlay {
                ch.name.clone()
            } else if sweeps.len() > 1 || show == Show::Sweeps {
                match stim.get(&sw) {
                    Some(p) => format!("{:+} pA", p.round()),
                    None => format!("sweep {sw}"),
                }
            } else if let Some(n) = t.name.as_ref().filter(|_| !named_by_channel(&t)) {
                n.clone()
            } else {
                ch.name.clone()
            };
            let mut selector = format!("trace {ti}");
            if t.sweep_count > 1 {
                selector.push_str(&format!(" sweep {sw}"));
            }
            selector.push_str(&format!(" channel {c} ({})", ch.name));
            if spec.process {
                selector.push_str(", processed from the FID");
            }
            let mut s = Series::new(
                name,
                Geom::Line(pts),
                MarkKind::Line,
                Source::file(file, selector),
            )
            .axes(xq.clone(), yq);
            s.x_reverse = reversed_axis(&xq);
            if overlay {
                let well = ch.extra.get("well").and_then(Value::as_str);
                s.class = ch
                    .extra
                    .get("target")
                    .and_then(Value::as_str)
                    .or_else(|| ch.extra.get("sample").and_then(Value::as_str))
                    .map(str::to_string);
                s.facet = well.map(str::to_string);
                s.well = well.and_then(openreadout_assay::layout::parse_well);
                s.weight = Weight::Many;
            } else if sweeps.len() > 1 {
                s.facet = Some(match stim.get(&sw) {
                    Some(p) => format!("sweep {sw} · {:+} pA", p.round()),
                    None => format!("sweep {sw}"),
                });
            }
            if let Some(sp) = spikes.get(&sw) {
                s.callouts = sp
                    .iter()
                    .map(|&(x, y)| Callout {
                        x,
                        y,
                        text: String::new(),
                        auto: false,
                    })
                    .collect();
                if !s.name.contains("spike") {
                    s.name = format!(
                        "{} · {} spike{}",
                        s.name,
                        sp.len(),
                        if sp.len() == 1 { "" } else { "s" }
                    );
                }
            } else if spec.spikes {
                s.name = format!("{} · 0 spikes", s.name);
            }
            s.peaks = spec.peaks.unwrap_or(0) as usize;
            if let Some(n) = spec.peaks
                && let Geom::Line(p) = &s.geom
            {
                let span = p.last().map_or(0.0, |l| l.0) - p.first().map_or(0.0, |f| f.0);
                s.callouts = top_peaks(p, n as usize, span.abs() / 40.0)
                    .into_iter()
                    .map(|(x, y)| Callout {
                        x,
                        y,
                        text: fmt_at(&s.x, x),
                        auto: true,
                    })
                    .collect();
            }
            out.push(s);
        }
    }
    if (show == Show::Sweeps || spec.spikes) && spikes.is_empty() && !stim.is_empty() {
        ctx.notes.push(
            "no action potentials crossed the detection threshold in the sweeps drawn".into(),
        );
    }
    Ok(out)
}

/// Factor from the trace's stored time unit to seconds (1 when the axis is not in minutes).
fn time_factor(t: &TraceInfo) -> f64 {
    t.extra
        .get("axis")
        .and_then(|a| a.get("unit"))
        .and_then(Value::as_str)
        .and_then(openreadout_core::trace::seconds_per_unit)
        .unwrap_or(1.0)
}

fn pick_kind(info: &openreadout_core::FileInfo, kinds: &[&str]) -> Option<u32> {
    kinds.iter().find_map(|k| {
        info.traces
            .iter()
            .find(|t| trace_kind(t) == *k)
            .map(|t| t.index)
    })
}

/// Chromatograms: TIC, BPC, XICs or detector traces.
pub(super) fn chromatogram(
    ctx: &mut Ctx<'_>,
    spec: &SeriesSpec,
    file: &str,
) -> Result<Vec<Series>> {
    let mut o = json!({"max_points": 20000});
    let mzs = spec
        .mz
        .as_ref()
        .map(crate::spec::OneOrMany::to_vec)
        .unwrap_or_default();
    if !mzs.is_empty() {
        o["mz"] = json!(mzs);
        if let Some(p) = spec.ppm {
            o["ppm"] = json!(p);
        }
    } else if let Some(t) = spec.trace {
        o["traces"] = json!([t]);
        if let Some(c) = spec.channel {
            o["channel"] = json!(c);
        }
    } else {
        match spec
            .chromatogram
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("tic") => o["tic"] = json!(true),
            Some("bpc") => o["bpc"] = json!(true),
            Some(other) => {
                return Err(usage(format!(
                    "chromatogram `{other}`: use tic or bpc (or mz for extracted-ion chromatograms, trace for a stored or detector trace)"
                )));
            }
        }
    }
    if let Some([a, b]) = spec.x_range {
        o["rt_range"] = json!([a.min(b), a.max(b)]);
    }
    let AnalyzeOutput::Chromatogram(c) = analyze(ctx, file, AnalyzeKind::Chromatogram, o)? else {
        return Err(usage("chromatogram analysis returned something else"));
    };
    ctx.notes.extend(c.notes.iter().cloned());
    let mut out = Vec::new();
    for ch in &c.chromatograms {
        if ch.kind == "tic"
            && let Some(a) = ch.apex_rt_min
        {
            ctx.apex.insert(file.to_string(), a);
        }
        let pts: Vec<(f64, f64)> = ch
            .rt_min
            .iter()
            .zip(&ch.intensity)
            .map(|(&t, &v)| (t * 60.0, v))
            .collect();
        let xq = Quantity::new("retention time", Some("s"));
        let yq = Quantity::new("intensity", ch.intensity_unit.as_deref());
        let mut s = Series::new(
            ch.label.clone(),
            Geom::Line(pts),
            MarkKind::Line,
            Source::file(file, ch.label.clone()),
        )
        .axes(xq, yq);
        if ch.decimated {
            s.source
                .selector
                .push_str(" (most intense point per slice, 20000 slices)");
        }
        s.peaks = spec.peaks.unwrap_or(0) as usize;
        if let Some(n) = spec.peaks {
            if n == 1 {
                if let (Some(t), Some(v)) = (ch.apex_rt_min, ch.apex_intensity) {
                    s.callouts.push(Callout {
                        x: t * 60.0,
                        y: v,
                        text: fmt_at(&s.x, t * 60.0),
                        auto: true,
                    });
                }
            } else if let Geom::Line(p) = &s.geom {
                let span = p.last().map_or(0.0, |l| l.0) - p.first().map_or(0.0, |f| f.0);
                s.callouts = top_peaks(p, n as usize, span.abs() / 40.0)
                    .into_iter()
                    .map(|(x, y)| Callout {
                        x,
                        y,
                        text: fmt_at(&s.x, x),
                        auto: true,
                    })
                    .collect();
            }
        }
        out.push(s);
    }
    if out.is_empty() {
        return Err(usage(format!("{file}: no chromatogram could be extracted")));
    }
    Ok(out)
}

/// The TIC apex of `file`, in minutes.
fn apex(ctx: &mut Ctx<'_>, file: &str) -> Result<f64> {
    if let Some(&a) = ctx.apex.get(file) {
        return Ok(a);
    }
    let AnalyzeOutput::Chromatogram(c) = analyze(
        ctx,
        file,
        AnalyzeKind::Chromatogram,
        json!({"tic": true, "max_points": 10}),
    )?
    else {
        return Err(usage("chromatogram analysis returned something else"));
    };
    let a = c
        .chromatograms
        .first()
        .and_then(|c| c.apex_rt_min)
        .ok_or_else(|| usage(format!("{file}: the TIC has no apex")))?;
    ctx.apex.insert(file.to_string(), a);
    Ok(a)
}

/// One mass spectrum, by scan number or retention time.
pub(super) fn spectrum(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let rt_min: Option<f64> = match &spec.rt {
        None => None,
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) if s.eq_ignore_ascii_case("apex") => Some(apex(ctx, file)?),
        Some(Value::String(s)) => Some(
            s.trim()
                .parse::<f64>()
                .map_err(|_| usage(format!("rt `{s}`: give minutes (a number) or \"apex\"")))?,
        ),
        Some(other) => return Err(usage(format!("rt {other}: give minutes or \"apex\""))),
    };
    let (format, mut ds, info) = ctx.open(file)?;
    if info.spectra.is_empty() {
        return Err(usage(format!(
            "{file}: no mass spectra; IR/Raman/NMR spectra are traces (show=trace)"
        )));
    }
    let scan = match (spec.scan, rt_min) {
        (Some(s), _) => s,
        (None, Some(rt)) => {
            let mut best: Option<(f64, u64)> = None;
            for w in [0.5, 5.0, 1e6] {
                let f = ScanFilter {
                    ms_level: Some(1),
                    rt_min_s: Some((rt - w) * 60.0),
                    rt_max_s: Some((rt + w) * 60.0),
                    ..ScanFilter::default()
                };
                let list = openreadout_core::scans::scan_list(
                    ds.as_mut(),
                    file,
                    &format,
                    0,
                    &f,
                    0,
                    100_000,
                )?;
                for h in &list.scans {
                    if let Some(t) = h.rt_s {
                        let d = (t / 60.0 - rt).abs();
                        if best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, h.scan_number));
                        }
                    }
                }
                if best.is_some() {
                    break;
                }
            }
            best.map(|b| b.1)
                .ok_or_else(|| usage(format!("{file}: no MS1 scan near {rt} min")))?
        }
        (None, None) => {
            return Err(usage("spectrum: give rt (minutes, or \"apex\") or scan"));
        }
    };
    let sp =
        openreadout_core::reader::spectrum_by_scan(ds.as_mut(), 0, scan, SpectrumView::Primary)?;
    let pts: Vec<(f64, f64)> = sp
        .mz
        .iter()
        .copied()
        .zip(sp.intensity.iter().map(|&v| f64::from(v)))
        .collect();
    let rt_txt = sp
        .rt_s
        .map(|t| format!(" · {} min", trim(format!("{:.2}", t / 60.0))))
        .unwrap_or_default();
    let mut name = format!("scan {}{rt_txt}", sp.scan_number);
    if sp.ms_level > 1
        && let Some(p) = sp.precursor_mz
    {
        name.push_str(&format!(
            " · MS{} of {}",
            sp.ms_level,
            trim(format!("{p:.4}"))
        ));
    }
    let mark = if sp.centroided {
        MarkKind::Sticks
    } else {
        MarkKind::Line
    };
    let mut s = Series::new(
        name,
        Geom::Line(pts),
        mark,
        Source::file(
            file,
            format!(
                "scan {} (MS{}{})",
                sp.scan_number,
                sp.ms_level,
                if sp.centroided {
                    ", centroid"
                } else {
                    ", profile"
                }
            ),
        ),
    )
    .axes(Quantity::new("m/z", None), Quantity::new("intensity", None));
    s.peaks = spec.peaks.unwrap_or(0) as usize;
    if let Some(n) = spec.peaks
        && let Geom::Line(p) = &s.geom
    {
        let span = p.last().map_or(0.0, |l| l.0) - p.first().map_or(0.0, |f| f.0);
        let base = p.iter().map(|q| q.1).fold(0.0f64, f64::max);
        let picked = if sp.centroided {
            let mut v: Vec<(f64, f64)> = p
                .iter()
                .copied()
                .filter(|q| q.1.is_finite() && q.1 >= 0.02 * base)
                .collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            let mut out: Vec<(f64, f64)> = Vec::new();
            for c in v {
                if out.iter().all(|o| (o.0 - c.0).abs() >= span / 80.0) {
                    out.push(c);
                    if out.len() == n as usize {
                        break;
                    }
                }
            }
            out
        } else {
            top_peaks(p, n as usize, span.abs() / 80.0)
        };
        s.callouts = picked
            .into_iter()
            .map(|(x, y)| Callout {
                x,
                y,
                text: fmt_at(&s.x, x),
                auto: true,
            })
            .collect();
    }
    // the m/z range that holds the signal: every point above 1 % of the base peak
    if let Geom::Line(p) = &s.geom {
        let base = p.iter().map(|q| q.1).fold(0.0f64, f64::max);
        let strong: Vec<f64> = p
            .iter()
            .filter(|q| q.1 >= 0.01 * base)
            .map(|q| q.0)
            .collect();
        if let (Some(&a), Some(&b)) = (
            strong.iter().min_by(|a, b| a.total_cmp(b)),
            strong.iter().max_by(|a, b| a.total_cmp(b)),
        ) {
            let pad = ((b - a) * 0.08).max(5.0);
            s.x_focus = Some((a - pad, b + pad));
        }
    }
    Ok(vec![s])
}

/// The f–I curve of a current-clamp recording.
pub(super) fn fi(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let mut o = json!({"trace": spec.trace.unwrap_or(0), "max_spikes": 0});
    if let Some(c) = spec.channel {
        o["channel"] = json!(c);
    }
    if let Some(s) = &spec.sweep {
        o["sweeps"] = json!(s.to_vec());
    }
    let AnalyzeOutput::EphysFeatures(r) = analyze(ctx, file, AnalyzeKind::EphysFeatures, o)? else {
        return Err(usage("ephys analysis returned something else"));
    };
    if r.cell.fi_curve.is_empty() {
        return Err(usage(format!(
            "{file}: no f–I curve (clamp mode {}, {} sweeps with a depolarising current step found)",
            r.clamp_mode,
            r.sweeps
                .iter()
                .filter(|s| s.stimulus_pa.is_some_and(|p| p > 0.0))
                .count()
        )));
    }
    let rate = match spec.y.as_deref() {
        None | Some("spike_count" | "count" | "spikes") => false,
        Some("rate_hz" | "rate" | "firing_rate_hz") => true,
        Some(other) => {
            return Err(usage(format!(
                "f–I y `{other}`: spike_count (default) or rate_hz"
            )));
        }
    };
    let pts: Vec<(f64, f64)> = r
        .cell
        .fi_curve
        .iter()
        .map(|p| {
            (
                p.current_pa,
                if rate {
                    p.rate_hz
                } else {
                    p.spike_count as f64
                },
            )
        })
        .collect();
    let stim_s = r
        .sweeps
        .iter()
        .find_map(|s| Some(s.stimulus_end_s? - s.stimulus_start_s?))
        .filter(|d| *d > 0.0);
    let yq = if rate {
        Quantity::new("firing rate", Some("Hz"))
    } else {
        Quantity::new(
            match stim_s {
                Some(d) => format!("spikes in the {} s step", trim(format!("{d:.2}"))),
                None => "spikes in the step".to_string(),
            },
            None,
        )
    };
    let mut notes = Vec::new();
    if let Some(rb) = r.cell.rheobase_pa {
        notes.push(format!("rheobase {} pA", trim(format!("{rb:.1}"))));
    }
    if let Some(sl) = r.cell.fi_slope_hz_per_pa {
        notes.push(format!("f–I slope {} Hz/pA", trim(format!("{sl:.3}"))));
    }
    if !notes.is_empty() {
        ctx.notes
            .push(format!("{}: {}", super::file_label(file), notes.join(", ")));
    }
    let s = Series::new(
        "f–I",
        Geom::Points {
            pts,
            err: Vec::new(),
        },
        MarkKind::LinePoints,
        Source::file(
            file,
            format!(
                "ephys features, trace {}, channel {} ({}), f–I over depolarising steps",
                r.trace, r.channel.index, r.channel.name
            ),
        ),
    )
    .axes(Quantity::new("injected current", Some("pA")), yq);
    let _ = Path::new(file);
    Ok(vec![s])
}
