//! qPCR readers against `oracle/qpcr.py` ground truth, shared by `tests/qpcr.rs` (development
//! files, with detection and the RDML round trip) and the generic corpus comparison in
//! `tests/corpus/` (`corpus_matches_oracle` and `heldout_agreement_is_recorded`, through the
//! same code path for every entry).
//!
//! - RDML: rdmlpython (MIT): every reaction's sample, target, dye, task, Cq, "undetermined", Tm,
//!   exclusion, and the count and sum of its amplification and melt points;
//! - `.eds`: the vendor's own result files read with the Python standard library, and qslib
//!   (EUPL, black box) for the per-dye multicomponent signal;
//! - `.rex`: ElementTree sums of the raw readings.
#![allow(dead_code)] // each test binary that includes this module uses a part of it

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use openreadout_core::Dataset;
use openreadout_qpcr::{QpcrDataset, QpcrReportRequest, qpcr_report};
use serde_json::Value;

/// Manifest formats compared here.
pub const FORMATS: [&str; 4] = [
    "rdml",
    "applied-biosystems-eds",
    "rotor-gene-rex",
    "qpcr-results-export",
];

/// Stored RDML `cq` values the LightCycler 96 software shows no Cq for (calls other than
/// Positive), which the reader keeps as `cq_stored` while rdmlpython reads them as Cqs:
/// adjudicated per file (docs/provenance/qpcr.md, 2026-09-26: the software's own analyses list
/// no Cq for 8 of 8 such graphs, and their curves barely rise). A file with another count, or
/// a LightCycler 96 file not listed here, fails until its withheld values are looked at.
pub const LC96_WITHHELD: &[(&str, usize)] = &[
    ("rdml-lc96-bactxy", 303),
    ("lc96p-pendo-hmuy-hm-dip", 60),
    ("lc96p-pendo-hmuy-24h48h-r12", 60),
    ("lc96p-pendo-hmuy-24h48h-r34", 60),
];

/// Stored RDML `cq` values at or beyond the run's cycle count (the exporting software's "no
/// Cq"), which the reader reports as undetermined with the number in `cq_stored` while
/// rdmlpython reads them as Cqs: adjudicated per file (docs/provenance/qpcr.md, 2026-10-02: the
/// StepOne export's three no-template controls store 40.0 of 40 cycles). A file with another
/// count fails until its withheld values are looked at.
pub const CYCLE_COUNT_WITHHELD: &[(&str, usize)] = &[("rdml-stepone-std", 3)];

/// Compare one opened file with its oracle; differences go to `errs`, the summary is returned.
pub fn compare_dataset(
    format: &str,
    id: &str,
    ds: &mut QpcrDataset,
    oracle: &Value,
    errs: &mut Vec<String>,
) -> String {
    match format {
        "rotor-gene-rex" => check_rex(id, ds, oracle, errs),
        "rdml" => check_records(id, ds, oracle, true, errs),
        _ => {
            let mut d = check_records(id, ds, oracle, false, errs);
            d.push_str(&check_qslib(id, ds, oracle, errs));
            d
        }
    }
}

/// Open `path` and compare it with `oracle`: `Ok(summary)` or `Err(differences)`. An oracle
/// that holds nothing to compare is an error, never a pass.
pub fn compare_file(format: &str, id: &str, path: &Path, oracle: &Value) -> Result<String, String> {
    let has_records = oracle["records"].as_array().is_some_and(|r| !r.is_empty());
    let has_channels = oracle["channels"].as_array().is_some_and(|r| !r.is_empty());
    let has_signals = oracle["qslib_multicomponent"]["wells"]
        .as_object()
        .is_some_and(|w| !w.is_empty());
    let has_program = oracle["cycles"].is_u64();
    if !has_records && !has_channels && !has_signals && !has_program {
        return Err(
            "the qPCR oracle holds no records, channels, signals or program to compare".into(),
        );
    }
    let mut ds = QpcrDataset::open(path).map_err(|e| format!("open failed: {e}"))?;
    let mut errs = Vec::new();
    let detail = compare_dataset(format, id, &mut ds, oracle, &mut errs);
    if errs.is_empty() {
        Ok(detail)
    } else {
        let n = errs.len();
        errs.truncate(12);
        Err(format!("{n} differences: {}; {detail}", errs.join("; ")))
    }
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * a.abs().max(b.abs()).max(1.0)
}

/// Our vocabulary for a raw task / RDML sample type.
fn task(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "unkn" | "unknown" | "target" => "unknown".into(),
        "std" | "standard" => "standard".into(),
        "ntc" => "ntc".into(),
        "pos" => "positive".into(),
        "opt" => "optical calibrator".into(),
        o => o.replace('_', " "),
    }
}

/// Per (run, row, col, target): (n, sum) of a curve table's value column.
fn curve_sums(
    ds: &mut QpcrDataset,
    kind: &str,
    value: &str,
    with_run: bool,
) -> BTreeMap<(String, u32, u32, String), (u64, f64)> {
    let info = ds.info().unwrap();
    let Some(t) = info.tables.iter().find(|t| t.name.as_deref() == Some(kind)) else {
        return BTreeMap::new();
    };
    let col = |n: &str| t.columns.iter().position(|c| c.name == n).unwrap();
    let cats = |n: &str| -> Vec<String> {
        t.columns[col(n)].extra["categories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect()
    };
    let (runs, targets) = (cats("run"), cats("target"));
    let tab = ds.read_table(t.index, 0, t.row_count).unwrap();
    let (ri, rr, cc, ti, vi) = (
        col("run"),
        col("row"),
        col("col"),
        col("target"),
        col(value),
    );
    let mut out: BTreeMap<(String, u32, u32, String), (u64, f64)> = BTreeMap::new();
    for i in 0..tab.columns[0].len() {
        let name = |list: &Vec<String>, v: f64| {
            if v.is_finite() {
                list.get(v as usize).cloned().unwrap_or_default()
            } else {
                String::new()
            }
        };
        let key = (
            if with_run {
                name(&runs, tab.columns[ri][i])
            } else {
                String::new()
            },
            tab.columns[rr][i] as u32,
            tab.columns[cc][i] as u32,
            name(&targets, tab.columns[ti][i]),
        );
        let v = tab.columns[vi][i];
        let e = out.entry(key).or_default();
        e.0 += 1;
        if v.is_finite() {
            e.1 += v;
        }
    }
    out
}

/// Per (well, dye): (n, sum) of the multicomponent traces.
fn multicomponent(ds: &mut QpcrDataset) -> BTreeMap<(String, String), (u64, f64)> {
    let info = ds.info().unwrap();
    let mut out = BTreeMap::new();
    for t in &info.traces {
        if t.extra.get("kind").and_then(Value::as_str) != Some("multicomponent") {
            continue;
        }
        let dye = t.extra["dye"].as_str().unwrap().to_string();
        let tr = ds.read_trace(t.index, 0, 0, t.sample_count).unwrap();
        for (c, ch) in t.channels.iter().enumerate() {
            let well = ch.extra["well"].as_str().unwrap().to_string();
            let v = &tr.channels[c];
            let n = v.iter().filter(|x| x.is_finite()).count() as u64;
            out.insert(
                (well, dye.clone()),
                (n, v.iter().filter(|x| x.is_finite()).sum()),
            );
        }
    }
    out
}

fn f(v: &Value) -> Option<f64> {
    v.as_f64()
}

fn check_records(
    id: &str,
    ds: &mut QpcrDataset,
    oracle: &Value,
    rdml: bool,
    errs: &mut Vec<String>,
) -> String {
    let rep = qpcr_report(ds, &{
        let mut r = QpcrReportRequest::default();
        r.compute_cq = true;
        r.standard_curve = true;
        r
    })
    .unwrap();
    let ours: BTreeMap<(String, u32, u32, String), &openreadout_qpcr::QpcrAssayRecord> = rep
        .records
        .iter()
        .map(|r| {
            (
                (
                    if rdml { r.run.clone() } else { String::new() },
                    r.row,
                    r.col,
                    r.target.clone().unwrap_or_default(),
                ),
                r,
            )
        })
        .collect();
    // RDML samples renamed from a GUID id to their description (LightCycler 96): id → name
    let renamed: BTreeMap<String, String> = ds
        .info()
        .unwrap()
        .tables
        .first()
        .and_then(|t| t.extra.get("samples"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    Some((
                        s["id"].as_str()?.to_string(),
                        s["name"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let amp = curve_sums(ds, "amplification", "fluorescence", rdml);
    let corr = curve_sums(ds, "amplification", "corrected", rdml);
    let melt = curve_sums(ds, "melt", "fluorescence", rdml);
    let melt_t = curve_sums(ds, "melt", "temperature", rdml);
    let recs = oracle["records"].as_array().unwrap();
    let (mut n_cq, mut n_curves, mut n_melt, mut n_setup) = (0, 0, 0, 0);
    let (mut n_export, mut n_export_und, mut n_amp) = (0, 0, 0);
    let mut n_withheld = 0usize;
    let mut n_at_cycles = 0usize;
    let mut missing = 0;
    for o in recs {
        let run = if rdml {
            o["run"].as_str().unwrap_or_default().to_string()
        } else {
            String::new()
        };
        let key = (
            run.clone(),
            o["row"].as_u64().unwrap() as u32,
            o["col"].as_u64().unwrap() as u32,
            o["target"].as_str().unwrap_or_default().to_string(),
        );
        let Some(r) = ours.get(&key) else {
            missing += 1;
            if missing <= 5 {
                errs.push(format!("{id}: no record for {key:?}"));
            }
            continue;
        };
        let at = format!("{id} {} {}", r.well, key.3);
        // Cq and "undetermined"
        // LightCycler 96: the RDML cq of a reaction its software does not call positive is not
        // a Cq; we keep it as cq_stored (docs/provenance/qpcr.md, 2026-09-26)
        let lc96_not_called = r.flags.iter().any(|x| x.starts_with("lc96_"));
        let at_cycles = r.flags.iter().any(|x| x == "cq_at_cycle_count");
        match (f(&o["cq"]), r.cq) {
            (Some(a), Some(b)) if close(a, b, 1e-9) => n_cq += 1,
            (Some(a), None) if lc96_not_called && r.cq_stored == Some(a) => n_withheld += 1,
            (Some(a), None) if rdml && at_cycles && r.cq_stored == Some(a) => n_at_cycles += 1,
            (None, None) => {}
            (a, b) => errs.push(format!("{at}: cq oracle {a:?} ours {b:?}")),
        }
        let withheld = lc96_not_called || (rdml && at_cycles);
        if !withheld && o["cq_undetermined"].as_bool().unwrap_or(false) != r.cq_undetermined {
            errs.push(format!(
                "{at}: undetermined oracle {} ours {}",
                o["cq_undetermined"], r.cq_undetermined
            ));
        }
        let status = if r.cq.is_some() {
            "determined"
        } else if r.cq_undetermined {
            "undetermined"
        } else {
            "no result"
        };
        if r.cq_status != status {
            errs.push(format!("{at}: cq_status {} for {status}", r.cq_status));
        }
        if let Some(v) = o["cq_stored"].as_f64()
            && r.cq_stored != Some(v)
        {
            errs.push(format!("{at}: cq_stored oracle {v} ours {:?}", r.cq_stored));
        }
        // the vendor's own Results export of the run (oracle/qpcr.py --export)
        if o["export_undetermined"].as_bool() == Some(true) {
            n_export += 1;
            n_export_und += 1;
            if r.cq_status != "undetermined" {
                errs.push(format!(
                    "{at}: the vendor export says Undetermined, ours {} {:?}",
                    r.cq_status, r.cq
                ));
            }
        } else if let Some(v) = o["export_ct"].as_f64() {
            n_export += 1;
            // the export writes the Ct as float32
            if !r
                .cq
                .is_some_and(|c| (c - v).abs() <= 1e-4 * v.abs().max(1.0))
            {
                errs.push(format!("{at}: the vendor export's CT {v}, ours {:?}", r.cq));
            }
        }
        let amp_word = |raw: &str| match raw {
            "1" | "Amp" => Some("amplified"),
            "0" | "Inconclusive" => Some("inconclusive"),
            "-1" | "No Amp" => Some("not amplified"),
            _ => None,
        };
        for k in ["amp_status_raw", "export_amp_status"] {
            if let Some(want) = o[k].as_str().and_then(amp_word) {
                n_amp += 1;
                if r.amp_status.as_deref() != Some(want) {
                    errs.push(format!(
                        "{at}: amp status {want} ({k}), ours {:?}",
                        r.amp_status
                    ));
                }
            }
        }
        let tm: Vec<f64> = o["tm"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        if tm.len() != r.tm.len() || tm.iter().zip(&r.tm).any(|(a, b)| !close(*a, *b, 1e-9)) {
            errs.push(format!("{at}: tm oracle {tm:?} ours {:?}", r.tm));
        }
        if let Some(s) = o["sample"].as_str()
            && r.sample.as_deref() != Some(renamed.get(s).map_or(s, String::as_str))
        {
            errs.push(format!("{at}: sample oracle {s:?} ours {:?}", r.sample));
        }
        if let Some(t) = o["task_raw"].as_str() {
            n_setup += 1;
            if r.task.as_deref() != Some(task(t).as_str()) {
                errs.push(format!("{at}: task oracle {t:?} ours {:?}", r.task));
            }
        }
        if let Some(d) = o["dye"].as_str()
            && r.dye.as_deref() != Some(d)
        {
            errs.push(format!("{at}: dye oracle {d:?} ours {:?}", r.dye));
        }
        if rdml && o["excluded"].is_string() != r.excluded.is_some() {
            errs.push(format!(
                "{at}: excluded oracle {} ours {:?}",
                o["excluded"], r.excluded
            ));
        }
        let tkey = (run.clone(), key.1, key.2, key.3.clone());
        // curves: count and sum
        if let Some(n) = o["amp_n"].as_u64().filter(|n| *n > 0) {
            let (on, os) = amp.get(&tkey).copied().unwrap_or_default();
            if on != n || !close(os, o["amp_sum"].as_f64().unwrap(), 1e-9) {
                errs.push(format!(
                    "{at}: amplification n/sum oracle {n}/{} ours {on}/{os}",
                    o["amp_sum"]
                ));
            } else {
                n_curves += 1;
            }
        }
        if let Some(s) = o["corrected_sum"].as_f64() {
            let (_, os) = corr.get(&tkey).copied().unwrap_or_default();
            if !close(os, s, 1e-9) {
                errs.push(format!("{at}: ΔRn sum oracle {s} ours {os}"));
            }
        }
        if let Some(n) = o["melt_n"].as_u64().filter(|n| *n > 0) {
            let (on, os) = melt.get(&tkey).copied().unwrap_or_default();
            let (_, ot) = melt_t.get(&tkey).copied().unwrap_or_default();
            if on != n
                || !close(os, o["melt_sum"].as_f64().unwrap(), 1e-9)
                || !close(ot, o["melt_t_sum"].as_f64().unwrap(), 1e-9)
            {
                errs.push(format!(
                    "{at}: melt n/sum/temp oracle {n}/{}/{} ours {on}/{os}/{ot}",
                    o["melt_sum"], o["melt_t_sum"]
                ));
            } else {
                n_melt += 1;
            }
        }
        for (k, ours_v) in [
            ("vendor_delta_cq", r.vendor_delta_cq),
            ("vendor_rq", r.vendor_rq),
        ] {
            if let Some(v) = o[k].as_f64()
                && !ours_v.is_some_and(|x| close(x, v, 1e-9))
            {
                errs.push(format!("{at}: {k} oracle {v} ours {ours_v:?}"));
            }
        }
    }
    // records without a target count when the oracle's have none either (an export that
    // names no target: CFX without a Target column, a trimmed QuantStudio export)
    let untargeted = !recs.is_empty() && recs.iter().all(|o| o["target"].is_null());
    let ours_n = rep
        .records
        .iter()
        .filter(|r| r.target.is_some() || untargeted)
        .count();
    if recs.len() != ours_n && missing == 0 {
        errs.push(format!(
            "{id}: {} oracle records, {ours_n} of ours",
            recs.len(),
        ));
    }
    if let Some(c) = oracle["cycles"].as_u64()
        && rep.cycles != Some(c as u32)
    {
        errs.push(format!("{id}: cycles oracle {c} ours {:?}", rep.cycles));
    }
    if let Some(t) = oracle["acquisition_temperature_c"].as_f64()
        && !rep
            .acquisition_temperature_c
            .is_some_and(|x| close(x, t, 1e-9))
    {
        errs.push(format!(
            "{id}: read temperature oracle {t} ours {:?}",
            rep.acquisition_temperature_c
        ));
    }
    if let Some(sc) = oracle["standard_curves"].as_array() {
        for c in sc {
            let t = c["targetName"].as_str().unwrap();
            let Some(fit) = rep.standard_curves.iter().find(|f| f.target == t) else {
                errs.push(format!("{id}: no standard curve for {t}"));
                continue;
            };
            let (vs, ve) = (
                c["slope"].as_f64().unwrap(),
                c["efficiency"].as_f64().unwrap(),
            );
            // the vendor rounds its slope to 4 decimals
            if (fit.slope - vs).abs() > 5e-4 || (fit.efficiency_percent - ve).abs() > 0.05 {
                errs.push(format!(
                    "{id}: standard curve {t}: vendor slope {vs} eff {ve}, ours {} {}",
                    fit.slope, fit.efficiency_percent
                ));
            }
        }
    }
    let mut detail = format!(
        "{} records: {n_cq} Cq, {n_curves} curves, {n_melt} melt curves, {n_setup} tasks",
        recs.len()
    );
    let expected = LC96_WITHHELD
        .iter()
        .find(|(f, _)| *f == id)
        .map_or(0, |(_, n)| *n);
    if n_withheld != expected {
        errs.push(format!(
            "{id}: {n_withheld} stored Cqs withheld under LightCycler 96 calls other than Positive (the oracle reads them as Cqs), {expected} adjudicated (LC96_WITHHELD)"
        ));
    }
    let expected = CYCLE_COUNT_WITHHELD
        .iter()
        .find(|(f, _)| *f == id)
        .map_or(0, |(_, n)| *n);
    if n_at_cycles != expected {
        errs.push(format!(
            "{id}: {n_at_cycles} stored Cqs at or beyond the cycle count withheld (the oracle reads them as Cqs), {expected} adjudicated (CYCLE_COUNT_WITHHELD)"
        ));
    }
    if n_at_cycles > 0 {
        let _ = write!(
            detail,
            "; {n_at_cycles} stored Cqs at the cycle count withheld as undetermined (adjudicated)"
        );
    }
    if n_withheld > 0 {
        let _ = write!(
            detail,
            "; {n_withheld} stored Cqs withheld under LightCycler 96 calls other than Positive (adjudicated)"
        );
    }
    if n_export > 0 {
        let _ = write!(
            detail,
            "; {n_export} wells equal to the vendor's Results export ({n_export_und} Undetermined)"
        );
    }
    if n_amp > 0 {
        let _ = write!(detail, "; {n_amp} amp statuses");
    }
    if let Some(c) = oracle["cycles"].as_u64() {
        let _ = write!(detail, "; {c} cycles");
    }
    if let Some(c) = &rep.cq_comparison
        && c.both_cq > 0
    {
        let _ = write!(
            detail,
            "; our Cq vs vendor: {} curves, {} both, {} both undetermined, {}/{} only vendor/ours, median |d| {:.3}, max |d| {:.3}, r {:.5}",
            c.curves,
            c.both_cq,
            c.both_undetermined,
            c.only_vendor,
            c.only_ours,
            c.median_abs_difference.unwrap_or(f64::NAN),
            c.max_abs_difference.unwrap_or(f64::NAN),
            c.pearson_r.unwrap_or(f64::NAN)
        );
    }
    detail
}

fn check_qslib(id: &str, ds: &mut QpcrDataset, oracle: &Value, errs: &mut Vec<String>) -> String {
    let Some(wells) = oracle["qslib_multicomponent"]["wells"].as_object() else {
        return String::new();
    };
    let ours = multicomponent(ds);
    let mut n = 0;
    for (well, dyes) in wells {
        for (dye, v) in dyes.as_object().unwrap() {
            let (on, os) = ours
                .get(&(well.clone(), dye.clone()))
                .copied()
                .unwrap_or_default();
            let (qn, qs) = (v["n"].as_u64().unwrap(), v["sum"].as_f64().unwrap());
            if qn == 0 {
                continue;
            }
            if on != qn || !close(os, qs, 1e-9) {
                if errs.len() < 40 {
                    errs.push(format!(
                        "{id} {well} {dye}: multicomponent qslib {qn}/{qs} ours {on}/{os}"
                    ));
                }
            } else {
                n += 1;
            }
        }
    }
    format!("; {n} multicomponent signals equal to qslib's")
}

fn check_rex(id: &str, ds: &mut QpcrDataset, oracle: &Value, errs: &mut Vec<String>) -> String {
    let amp = curve_sums(ds, "amplification", "fluorescence", false);
    let melt = curve_sums(ds, "melt", "fluorescence", false);
    let mut n = 0;
    for ch in oracle["channels"].as_array().unwrap() {
        let name = ch["name"].as_str().unwrap();
        let table = if name.to_ascii_lowercase().starts_with("melt") {
            &melt
        } else if name.to_ascii_lowercase().starts_with("cycling") {
            &amp
        } else {
            continue;
        };
        for r in ch["readings"].as_array().unwrap() {
            let tube = r["tube"].as_u64().unwrap() as u32;
            let got = table
                .iter()
                .find(|((_, row, _, _), _)| *row == tube)
                .map(|(_, v)| *v);
            let (on, os) = got.unwrap_or_default();
            if on != r["n"].as_u64().unwrap() || !close(os, r["sum"].as_f64().unwrap(), 1e-9) {
                errs.push(format!(
                    "{id} tube {tube} {name}: oracle {}/{} ours {on}/{os}",
                    r["n"], r["sum"]
                ));
            } else {
                n += 1;
            }
        }
    }
    format!("{n} channel readings equal to ElementTree's")
}
