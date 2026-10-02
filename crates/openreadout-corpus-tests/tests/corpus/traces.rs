//! Sampled signals (electrophysiology) and spectra (NMR, JCAMP-DX).

use std::collections::BTreeMap;

use crate::oracle::{Oracle, OracleSweep, OracleTrace};

/// Differences between the oracle's per-channel `extra` values and ours (at most 5 reported).
pub(crate) fn channel_extra_problems(
    o: &OracleTrace,
    t: &openreadout_core::model::TraceInfo,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut offsets: BTreeMap<&str, f64> = BTreeMap::new();
    for (i, want) in o.channel_extra.iter().enumerate() {
        let Some(ch) = t.channels.get(i) else {
            out.push(format!(
                "trace {}: no channel {i} for oracle extra",
                o.index
            ));
            break;
        };
        for (k, w) in want {
            let got = ch.extra.get(k);
            let fine = match (got.and_then(serde_json::Value::as_f64), w.as_f64()) {
                (Some(ours), Some(theirs))
                    if o.channel_extra_offset_keys.iter().any(|x| x == k) =>
                {
                    let shift = *offsets.entry(k.as_str()).or_insert(ours - theirs);
                    (ours - theirs - shift).abs() <= 1e-6
                }
                (Some(ours), Some(theirs)) => (ours - theirs).abs() <= 1e-6 * theirs.abs().max(1.0),
                _ => got == Some(w),
            };
            if !fine && out.len() < 5 {
                out.push(format!(
                    "trace {} channel {i}: extra.{k} {got:?} != oracle {w}",
                    o.index
                ));
            }
        }
    }
    out
}

/// Oracle units are compared after folding `µ` to `u` (pyABF writes `u`) and treating `?` as absent.
pub(crate) fn norm_unit(u: &str) -> String {
    let u = u.trim().replace(['µ', 'μ'], "u");
    if u == "?" { String::new() } else { u }
}

/// Oracle names: `?`, empty or all-NUL mean "no name" (pyABF), which we report as `ch<i>`.
pub(crate) fn oracle_name_absent(n: &str) -> bool {
    let t = n.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    t.is_empty() || t == "?"
}

/// Compare traces: count, sweeps, channels (names, units), sample rate, and for every oracle
/// sweep x channel the exact sample count, the first samples (within 1e-9) and the xxh3-128 of
/// the samples as little-endian f64 (definition in oracle/gen.py).
pub(crate) fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// Compare a reader parameter (from the oracle's `parameters`) with a trace `extra` field.
pub(crate) fn same_value(ours: Option<&serde_json::Value>, theirs: &serde_json::Value) -> bool {
    match (ours, theirs) {
        (Some(a), b) if a.is_number() && b.is_number() => near(
            a.as_f64().unwrap_or(f64::NAN),
            b.as_f64().unwrap_or(f64::NAN),
        ),
        (Some(serde_json::Value::String(a)), serde_json::Value::String(b)) => a.trim() == b.trim(),
        // the oracle keeps empty strings; we omit empty values
        (None, serde_json::Value::String(b)) => b.trim().is_empty(),
        _ => false,
    }
}

/// Compare traces: count, name, samples, sweeps, channels, the xxh3-128 of every value (sweep by
/// sweep, channels interleaved, little-endian f64), the ppm axis of processed spectra and the
/// Bruker parameters the oracle read.
pub(crate) fn check_traces(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::model::FileInfo,
    oracle: &Oracle,
    problems: &mut Vec<String>,
    ok: &mut usize,
) {
    if info.traces.len() != oracle.traces.len() {
        problems.push(format!(
            "trace count {} != oracle {}",
            info.traces.len(),
            oracle.traces.len()
        ));
    }
    for o in &oracle.traces {
        let Some(t) = info.traces.iter().find(|t| t.index == o.index) else {
            problems.push(format!("trace {} missing", o.index));
            continue;
        };
        if let Some(e) = &o.error {
            problems.push(format!("trace {}: oracle error {e}", o.index));
            continue;
        }
        // An empty name in the oracle is "not recorded", which we omit
        // (book/src/guides/metadata.md § General rules).
        if let Some(n) = o.name.as_deref().map(str::trim).filter(|n| !n.is_empty())
            && t.name.as_deref().map(str::trim) != Some(n)
        {
            problems.push(format!(
                "trace {}: name {:?} != oracle {n:?}",
                o.index, t.name
            ));
        }
        let axis = t.extra.get("axis");
        for (key, want) in [("first", o.ppm_first), ("last", o.ppm_last)] {
            if let Some(w) = want {
                let got = axis
                    .and_then(|a| a.get(key))
                    .and_then(serde_json::Value::as_f64);
                if !got.is_some_and(|g| (g - w).abs() <= 1e-9 * w.abs().max(1.0)) {
                    problems.push(format!(
                        "trace {}: ppm axis {key} {got:?} != oracle {w}",
                        o.index
                    ));
                }
            }
        }
        if let Some(params) = &o.parameters {
            // Bruker parameter name → our extra key
            let map: &[(&str, &str, &str)] = &[
                ("acqus", "NUC1", "nucleus"),
                ("acqus", "SFO1", "spectrometer_frequency_mhz"),
                ("acqus", "TD", "time_domain_size"),
                ("acqus", "NS", "scans"),
                ("acqus", "PULPROG", "pulse_program"),
                ("acqus", "SOLVENT", "solvent"),
                ("acqus", "TE", "temperature_k"),
                ("acqus", "INSTRUM", "instrument"),
                ("acqus", "PROBHD", "probe"),
                ("acqus", "DECIM", "decimation"),
                ("acqus", "DSPFVS", "dsp_firmware"),
                ("acqus", "NC", "normalization_exponent"),
                ("procs", "NC_proc", "normalization_exponent"),
                ("procs", "SW_p", "spectral_width_hz"),
                ("procs", "SF", "spectrometer_frequency_mhz"),
                ("procs", "AXNUC", "nucleus"),
            ];
            for (file, name, ours) in map {
                if let Some(v) = params.get(file).and_then(|f| f.get(name))
                    && !same_value(t.extra.get(*ours), v)
                {
                    problems.push(format!(
                        "trace {}: extra.{ours} {:?} != oracle {file}.{name} {v}",
                        o.index,
                        t.extra.get(*ours)
                    ));
                }
            }
            // Varian/JEOL: the oracle names our `extra` keys directly (values it read with nmrglue)
            if let Some(extra) = params.get("extra").and_then(serde_json::Value::as_object) {
                for (ours, v) in extra {
                    if !same_value(t.extra.get(ours), v) {
                        problems.push(format!(
                            "trace {}: extra.{ours} {:?} != oracle {v}",
                            o.index,
                            t.extra.get(ours)
                        ));
                    }
                }
            }
        }
        if let Some(n) = o.sweep_count
            && t.sweep_count != n
        {
            problems.push(format!(
                "trace {}: sweeps {} != oracle {n}",
                o.index, t.sweep_count
            ));
        }
        let Some(channel_count) = o.channel_count else {
            continue; // metadata only
        };
        if t.channels.len() != channel_count {
            problems.push(format!(
                "trace {}: channels {} != oracle {channel_count}",
                o.index,
                t.channels.len(),
            ));
            continue;
        }
        for (i, n) in o.channel_names.iter().enumerate() {
            let ours = &t.channels[i].name;
            let fine = if oracle_name_absent(n) {
                ours == &format!("ch{i}")
            } else {
                ours == n.trim()
            };
            if !fine {
                problems.push(format!(
                    "trace {} channel {i}: name {ours:?} != oracle {n:?}",
                    o.index
                ));
            }
        }
        for (i, u) in o.channel_units.iter().enumerate() {
            let ours = norm_unit(t.channels[i].unit.as_deref().unwrap_or(""));
            if ours != norm_unit(u) {
                problems.push(format!(
                    "trace {} channel {i}: unit {ours:?} != oracle {u:?}",
                    o.index
                ));
            }
        }
        problems.extend(channel_extra_problems(o, t));
        if o.x_first_min.is_some() || o.x_last_min.is_some() {
            let axis = t.extra.get("axis");
            let first = axis
                .and_then(|a| a.get("first"))
                .and_then(serde_json::Value::as_f64);
            let step = axis
                .and_then(|a| a.get("step"))
                .and_then(serde_json::Value::as_f64);
            let n = t.sample_count.saturating_sub(1) as f64;
            let ours = [first, first.zip(step).map(|(f, s)| f + s * n)];
            for (k, (want, got)) in [o.x_first_min, o.x_last_min].iter().zip(ours).enumerate() {
                if let Some(w) = want
                    && !got.is_some_and(|g| (g - w).abs() <= 1e-6 * w.abs().max(1.0))
                {
                    problems.push(format!(
                        "trace {}: {} retention time {got:?} min != oracle {w}",
                        o.index,
                        if k == 0 { "first" } else { "last" }
                    ));
                }
            }
        }
        if let Some(rate) = o.sample_rate_hz {
            let tol = o
                .sample_rate_tolerance_hz
                .unwrap_or(1e-9 * rate.abs().max(1.0));
            if (t.sample_rate_hz - rate).abs() > tol {
                problems.push(format!(
                    "trace {}: sample rate {} != oracle {rate} (tolerance {tol})",
                    o.index, t.sample_rate_hz
                ));
            }
        }
        for sw in &o.sweeps {
            let tr = match read_sweep_paged(ds, o.index, sw) {
                Ok(tr) => tr,
                Err(e) => {
                    problems.push(format!(
                        "trace {} sweep {}: read failed: {e}",
                        o.index, sw.sweep
                    ));
                    break;
                }
            };
            for (c, oc) in sw.channels.iter().enumerate() {
                let Some(col) = tr.channels.get(c) else {
                    problems.push(format!(
                        "trace {} sweep {}: channel {c} missing",
                        o.index, sw.sweep
                    ));
                    continue;
                };
                if tr.total != sw.sample_count {
                    problems.push(format!(
                        "trace {} sweep {} channel {c}: {} samples != oracle {}",
                        o.index, sw.sweep, tr.total, sw.sample_count
                    ));
                    continue;
                }
                if let Some((k, (a, b))) = col
                    .iter()
                    .zip(&oc.first)
                    .enumerate()
                    .find(|(_, (a, b))| (**a - **b).abs() > 1e-9)
                {
                    problems.push(format!(
                        "trace {} sweep {} channel {c}: sample {k} = {a} != oracle {b}",
                        o.index, sw.sweep
                    ));
                    continue;
                }
                let hashed = sw
                    .hashed_samples
                    .map_or(col.len(), |h| (h as usize).min(col.len()));
                let mut bytes = Vec::with_capacity(hashed * 8);
                for v in &col[..hashed] {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
                let got = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
                if got == oc.xxh3 {
                    *ok += 1;
                } else {
                    problems.push(format!(
                        "trace {} sweep {} channel {c}: hash {got} != oracle {}",
                        o.index, sw.sweep, oc.xxh3
                    ));
                }
                if problems.len() > 12 {
                    problems.push("(more omitted)".into());
                    return;
                }
            }
        }
    }
}

/// A sweep read page by page (readers cap one read), keeping the first `hashed_samples` (else
/// all) samples of each channel and counting the rest.
pub(crate) struct PagedSweep {
    pub(crate) channels: Vec<Vec<f64>>,
    pub(crate) total: u64,
}

pub(crate) fn read_sweep_paged(
    ds: &mut dyn openreadout_core::Dataset,
    index: u32,
    sw: &OracleSweep,
) -> openreadout_core::Result<PagedSweep> {
    let keep = sw
        .hashed_samples
        .map_or(sw.sample_count, |h| h.min(sw.sample_count));
    let first = ds.read_trace(index, sw.sweep, 0, u64::MAX)?;
    let mut total = first.channels.first().map_or(0, |c| c.len() as u64);
    let mut channels = first.channels;
    for col in &mut channels {
        col.truncate(usize::try_from(keep).unwrap_or(usize::MAX));
    }
    // later pages only while the reader keeps returning samples and the oracle wants more
    while total > 0 && total < sw.sample_count {
        let page = ds.read_trace(index, sw.sweep, total, u64::MAX)?;
        let got = page.channels.first().map_or(0, |c| c.len() as u64);
        if got == 0 {
            break;
        }
        for (col, more) in channels.iter_mut().zip(page.channels) {
            let room = usize::try_from(keep)
                .unwrap_or(usize::MAX)
                .saturating_sub(col.len());
            col.extend(more.into_iter().take(room));
        }
        total += got;
    }
    Ok(PagedSweep { channels, total })
}
