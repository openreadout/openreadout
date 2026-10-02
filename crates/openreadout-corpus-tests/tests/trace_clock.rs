//! Cross-reader invariants of trace time bases, over every corpus trace:
//!
//! - a regular time abscissa (`extra.axis` with quantity `time`/`retention_time` and a time
//!   unit) agrees with `start_s` and `sample_rate_hz`: `start_s` = `axis.first` and 1 /
//!   `sample_rate_hz` = |`axis.step`| (in seconds); a falling time axis leaves `start_s` unset;
//! - `trace` places its window on that clock: `start_s` = the trace's time origin +
//!   `first_sample` / rate (single sweep), `argmax_time_s` = origin + `argmax` / rate, and
//!   `argmax_axis_value` = `axis.first + argmax * axis.step`; irregular traces take both from
//!   their `time` channel.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test trace_clock -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]
#![allow(clippy::many_single_char_names)] // t/s/n/f/k: trace, slice, count, first, factor

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::TraceInfo;
use openreadout_core::trace::{TraceRequest, seconds_per_unit, slice_trace, sweep_origin_s};
use openreadout_core::{Error, Registry};
use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    format: String,
    filename: String,
    #[serde(default)]
    role: String,
}

/// Formats whose files hold traces.
const TRACE_FORMATS: &[&str] = &[
    "abf",
    "atf",
    "agilent-masshunter",
    "andi-chrom",
    "blackrock",
    "bruker-nmr",
    "bruker-tdf",
    "chemstation",
    "intan",
    "jcamp-dx",
    "jeol-jdf",
    "winwcp",
    "magritek-spinsolve",
    "mzml",
    "mzxml",
    "neuralynx",
    "nwb",
    "plexon",
    "sciex-wiff",
    "shimadzu",
    "spikeglx",
    "thermo-raw",
    "varian-nmr",
    "waters-raw",
];
/// Traces checked per file (files with hundreds of channels or MRM transitions repeat the
/// same clock).
const PER_FILE: usize = 12;
/// Window length read for the slice checks.
const WINDOW: u64 = 2_000;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_abf::AbfReader))
        .with(Box::new(openreadout_abf::AtfReader))
        .with(Box::new(openreadout_neuralynx::NeuralynxReader))
        .with(Box::new(openreadout_blackrock::BlackrockReader))
        .with(Box::new(openreadout_spikeglx::SpikeGlxReader))
        .with(Box::new(openreadout_intan::IntanReader))
        .with(Box::new(openreadout_plexon::PlexonReader))
        .with(Box::new(openreadout_ephys::HekaReader))
        .with(Box::new(openreadout_ephys::Spike2Reader))
        .with(Box::new(openreadout_ephys::OpenEphysReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_chrom::ShimadzuReader))
        .with(Box::new(openreadout_chrom::ChromeleonReader))
        .with(Box::new(openreadout_chrom::EmpowerArwReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
        .with(Box::new(openreadout_bruker_tims::BrukerTimsReader))
        .with(Box::new(openreadout_hdf5::NwbReader))
        .with(Box::new(openreadout_nmr::BrukerReader))
        .with(Box::new(openreadout_nmr::JcampReader))
        .with(Box::new(openreadout_nmr::VarianReader))
        .with(Box::new(openreadout_nmr::JeolReader))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

/// The regular time axis of a trace in seconds: (first, step).
fn time_axis(t: &TraceInfo) -> Option<(f64, f64)> {
    let a = t.extra.get("axis")?;
    if a.get("irregular").and_then(serde_json::Value::as_bool) == Some(true) {
        return None;
    }
    let q = a.get("quantity")?.as_str()?;
    if q != "time" && q != "retention_time" {
        return None;
    }
    let k = seconds_per_unit(a.get("unit")?.as_str()?)?;
    Some((a.get("first")?.as_f64()? * k, a.get("step")?.as_f64()? * k))
}

fn regular_axis(t: &TraceInfo) -> Option<(f64, f64)> {
    let a = t.extra.get("axis")?;
    Some((a.get("first")?.as_f64()?, a.get("step")?.as_f64()?))
}

/// Problems with one trace; `Ok(checked slices)`.
fn check_trace(
    ds: &mut dyn openreadout_core::Dataset,
    info: &openreadout_core::FileInfo,
    t: &TraceInfo,
) -> Result<u32, String> {
    let mut bad = Vec::new();
    let rate = t.sample_rate_hz;
    if let Some((first, step)) = time_axis(t) {
        if step > 0.0 {
            match t.start_s {
                Some(s) if close(s, first) => {}
                other => bad.push(format!("start_s {other:?} != axis.first {first} s")),
            }
            if !(rate > 0.0 && close(1.0 / rate, step)) {
                bad.push(format!(
                    "1/sample_rate_hz {} != axis.step {step} s",
                    1.0 / rate
                ));
            }
        } else if t.start_s.is_some() && t.sweep_count <= 1 {
            bad.push("a falling time axis must leave start_s unset".into());
        }
    }
    let n = openreadout_core::trace::sweep_samples(t, 0);
    if n == 0 || t.channels.is_empty() || t.extra.contains_key("undecodable") {
        return if bad.is_empty() {
            Ok(0)
        } else {
            Err(bad.join("; "))
        };
    }
    let origin = sweep_origin_s(t);
    let time_ch = t
        .channels
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case("time"))
        .filter(|_| rate <= 0.0)
        .and_then(|k| {
            let f = t.channels[k].unit.as_deref().and_then(seconds_per_unit)?;
            Some((k, f))
        });
    let mut slices = 0;
    for first in [0, n / 2] {
        let mut r = TraceRequest::default();
        r.trace = t.index;
        r.first_sample = first;
        r.count = Some(WINDOW.min(n - first));
        r.max_samples = WINDOW;
        let s = match slice_trace(ds, info, &r) {
            Ok(s) => s,
            Err(Error::Unsupported { .. }) => break,
            Err(e) => {
                bad.push(format!("trace at {first}: {e}"));
                break;
            }
        };
        slices += 1;
        let data = s
            .channels
            .iter()
            .find(|c| Some(c.index as usize) != time_ch.map(|x| x.0));
        if rate > 0.0 {
            let want = origin + first as f64 / rate;
            if !close(s.start_s, want) {
                bad.push(format!("slice at {first}: start_s {} != {want}", s.start_s));
            }
            if let (Some((f, st)), true) = (time_axis(t), t.sweep_count <= 1)
                && st > 0.0
                && !close(s.start_s, f + st * first as f64)
            {
                bad.push(format!(
                    "slice at {first}: start_s {} is off the axis",
                    s.start_s
                ));
            }
            if let Some(c) = data
                && let (Some(i), Some(tm)) = (c.stats.argmax, c.stats.argmax_time_s)
                && !close(tm, origin + i as f64 / rate)
            {
                bad.push(format!(
                    "slice at {first}: argmax_time_s {tm} != sample {i}"
                ));
            }
        } else if let Some((k, f)) = time_ch {
            let t0 = s.channels.iter().find(|c| c.index as usize == k);
            if let Some(v) = t0.and_then(|c| c.samples.first())
                && !close(s.start_s, v * f)
            {
                bad.push(format!(
                    "slice at {first}: start_s {} != time channel {v}",
                    s.start_s
                ));
            }
            if let (Some(c), Some(tc)) = (data, t0)
                && let (Some(i), Some(tm)) = (c.stats.argmax, c.stats.argmax_time_s)
                && let Some(v) = tc.samples.get((i - first) as usize)
                && !close(tm, v * f)
            {
                bad.push(format!(
                    "slice at {first}: argmax_time_s {tm} != time channel {v}"
                ));
            }
        }
        if let (Some((f, st)), Some(c)) = (regular_axis(t), data)
            && let (Some(i), Some(x)) = (c.stats.argmax, c.stats.argmax_axis_value)
            && !close(x, f + st * i as f64)
        {
            bad.push(format!(
                "slice at {first}: argmax_axis_value {x} != axis at {i}"
            ));
        }
        if let (Some(c), Some((_, st))) = (data, time_axis(t))
            && st > 0.0
            && t.sweep_count <= 1
            && let (Some(tm), Some(x)) = (c.stats.argmax_time_s, c.stats.argmax_axis_value)
        {
            let k = t.extra["axis"]["unit"]
                .as_str()
                .and_then(seconds_per_unit)
                .unwrap_or(1.0);
            if !close(tm, x * k) {
                bad.push(format!(
                    "slice at {first}: argmax_time_s {tm} != argmax_axis_value {x} × {k}"
                ));
            }
        }
    }
    if bad.is_empty() {
        Ok(slices)
    } else {
        Err(bad.join("; "))
    }
}

#[test]
fn trace_time_bases_agree() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = registry();
    let mut failures = Vec::new();
    let mut per_format: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
    for e in manifest.file.iter().filter(|e| {
        (e.role.is_empty() || e.role == "input") && TRACE_FORMATS.contains(&e.format.as_str())
    }) {
        if only.as_ref().is_some_and(|o| !e.id.contains(o.as_str())) {
            continue;
        }
        let path = files_dir.join(&e.filename);
        if !path.exists() {
            continue;
        }
        let (_, mut ds) = match reg.open(&path) {
            Ok(x) => x,
            Err(Error::UnknownFormat { .. } | Error::Unsupported { .. }) => continue,
            Err(err) => {
                failures.push(format!("{}: open: {err}", e.id));
                continue;
            }
        };
        let info = match ds.info() {
            Ok(i) => i,
            Err(err) => {
                failures.push(format!("{}: info: {err}", e.id));
                continue;
            }
        };
        let stat = per_format.entry(e.format.clone()).or_default();
        stat.0 += 1;
        for t in info.traces.iter().take(PER_FILE) {
            stat.1 += 1;
            match check_trace(ds.as_mut(), &info, t) {
                Ok(n) => stat.2 += n,
                Err(m) => failures.push(format!("{} trace {}: {m}", e.id, t.index)),
            }
        }
    }
    for (f, (files, traces, slices)) in &per_format {
        println!("{f:<20} {files:>3} files {traces:>4} traces {slices:>5} slices");
    }
    assert!(
        failures.is_empty(),
        "{} trace clock failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
