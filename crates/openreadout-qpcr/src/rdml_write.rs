//! `export --format rdml`: any readable qPCR file as RDML 1.3 (`rdml_data.xml` in a zip), written
//! to a temporary file, read back and compared, then renamed into place.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};

use crate::dataset::QpcrDataset;
use crate::model::{Assay, Program, QpcrData, Reaction};
use crate::xml::escape;
use openreadout_core::zip::{ZipIndex, ZipWriter};

/// Version of RDML written.
pub const RDML_WRITE_VERSION: &str = "1.3";

/// What `export --format rdml` wrote.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct RdmlExportReport {
    /// The input file.
    pub input: String,
    /// The RDML file written.
    pub output: String,
    /// Always `rdml`.
    pub format: String,
    /// RDML schema version written (`1.3`).
    pub rdml_version: String,
    /// Runs (plates) written.
    pub runs: u32,
    /// Reactions (`react` elements).
    pub reactions: u64,
    /// Data elements (one per reaction × target).
    pub data_elements: u64,
    /// Cq values written.
    pub cq_values: u64,
    /// Amplification data points (`adp`).
    pub amplification_points: u64,
    /// Melt data points (`mdp`).
    pub melt_points: u64,
    /// Samples, targets, dyes defined.
    pub samples: u32,
    /// Targets defined.
    pub targets: u32,
    /// Dyes defined.
    pub dyes: u32,
    /// Size of the file.
    pub bytes_written: u64,
    /// The file was read back and every reaction, Cq and data point matched.
    pub verified: bool,
    /// What could not be carried over and why.
    pub notes: Vec<String>,
}

fn f(v: f64) -> String {
    // Rust prints the shortest text that reads back to the same f64.
    format!("{v}")
}

fn el(out: &mut String, tag: &str, v: &str) {
    let _ = write!(out, "<{tag}>{}</{tag}>", escape(v));
}

fn cq_method(m: Option<&str>) -> Option<&'static str> {
    let m = m?.to_ascii_lowercase();
    Some(if m.contains("second derivative") {
        "second derivative maximum"
    } else if m.contains("manual") {
        "manual threshold and baseline settings"
    } else if m.contains("automated threshold") || m.contains("auto") {
        "automated threshold and baseline settings"
    } else {
        "other"
    })
}

fn sample_type(task: Option<&str>) -> &'static str {
    match task {
        Some("ntc") => "ntc",
        Some("nac") => "nac",
        Some("standard") => "std",
        Some("ntp") => "ntp",
        Some("nrt") => "nrt",
        Some("positive") => "pos",
        Some("optical calibrator") => "opt",
        _ => "unkn",
    }
}

fn quantity_unit(u: Option<&str>) -> &'static str {
    match u.map(str::trim) {
        Some("cop") => "cop",
        Some("fold") => "fold",
        Some("dil") => "dil",
        Some("ng") => "ng",
        Some("nMol") => "nMol",
        _ => "other",
    }
}

/// One RDML sample: the name, and the type/quantity per target.
#[derive(Debug, Clone, PartialEq)]
struct SampleKey {
    base: String,
    per_target: Vec<(String, &'static str, Option<String>)>,
}

/// Build the sample id of every (run, reaction) and the sample definitions.
fn samples(d: &QpcrData) -> (Vec<(String, SampleKey)>, BTreeMap<(usize, usize), String>) {
    let mut defs: Vec<(String, SampleKey)> = Vec::new();
    let mut ids = BTreeMap::new();
    for (ri, run) in d.runs.iter().enumerate() {
        for (xi, rx) in run.reactions.iter().enumerate() {
            let first_task = rx.assays.first().and_then(|a| a.task.as_deref());
            let base = rx.sample.clone().unwrap_or_else(|| match first_task {
                Some("ntc") => "NTC".into(),
                Some("standard") => format!(
                    "standard {}",
                    rx.assays
                        .first()
                        .and_then(|a| a.quantity)
                        .map_or_else(String::new, f)
                )
                .trim()
                .to_string(),
                _ => "unnamed".into(),
            });
            let mut per_target: Vec<(String, &'static str, Option<String>)> = rx
                .assays
                .iter()
                .filter_map(|a| {
                    a.target.as_ref().map(|t| {
                        (
                            t.clone(),
                            sample_type(a.task.as_deref()),
                            a.quantity.filter(|q| q.is_finite() && *q > 0.0).map(f),
                        )
                    })
                })
                .collect();
            per_target.sort();
            per_target.dedup_by(|a, b| a.0 == b.0);
            let key = SampleKey {
                base: base.clone(),
                per_target,
            };
            // Same name, same types and quantities for the targets both measure → same sample.
            let compatible = |k: &SampleKey| {
                k.base == key.base
                    && key.per_target.iter().all(|(t, ty, q)| {
                        k.per_target
                            .iter()
                            .find(|x| &x.0 == t)
                            .is_none_or(|x| x.1 == *ty && &x.2 == q)
                    })
            };
            let id = if let Some((id, k)) = defs.iter_mut().find(|(_, k)| compatible(k)) {
                for pt in &key.per_target {
                    if !k.per_target.iter().any(|x| x.0 == pt.0) {
                        k.per_target.push(pt.clone());
                    }
                }
                id.clone()
            } else {
                let n = defs.iter().filter(|(_, k)| k.base == base).count();
                let id = if n == 0 {
                    base.clone()
                } else {
                    format!("{base} ({})", n + 1)
                };
                defs.push((id.clone(), key));
                id
            };
            ids.insert((ri, xi), id);
        }
    }
    (defs, ids)
}

/// RDML steps of a program (stages flattened; cycling stages close with a loop).
fn program_steps(p: &Program, notes: &mut Vec<String>) -> String {
    let mut out = String::new();
    let mut nr = 0u32;
    let mut prev_temp: Option<f64> = None;
    let mut clipped = 0;
    let dur = |h: Option<f64>, clipped: &mut u32| -> String {
        let s = h.unwrap_or(0.0).round();
        if s < 1.0 {
            *clipped += 1;
            "1".into()
        } else {
            format!("{}", s as u64)
        }
    };
    for st in &p.stages {
        let first = nr + 1;
        for s in &st.steps {
            nr += 1;
            let _ = write!(out, "<step><nr>{nr}</nr>");
            match s.kind.as_str() {
                "gradient" => {
                    let hi = s.high_temperature_c.or(s.temperature_c).unwrap_or(95.0);
                    let lo = s.low_temperature_c.or(prev_temp).unwrap_or(hi);
                    let _ = write!(
                        out,
                        "<gradient><highTemperature>{}</highTemperature><lowTemperature>{}</lowTemperature><duration>{}</duration>",
                        f(hi),
                        f(lo),
                        dur(s.hold_s, &mut clipped)
                    );
                    if s.measure.is_some() {
                        let m = if s.measure.as_deref() == Some("melt") {
                            "meltcurve"
                        } else {
                            "real time"
                        };
                        el(&mut out, "measure", m);
                    }
                    out.push_str("</gradient>");
                }
                "loop" => {
                    let _ = write!(
                        out,
                        "<loop><goto>{}</goto><repeat>{}</repeat></loop>",
                        s.goto.unwrap_or(1).max(1),
                        s.repeat.unwrap_or(1).max(1)
                    );
                }
                "pause" => {
                    let _ = write!(
                        out,
                        "<pause><temperature>{}</temperature></pause>",
                        f(s.temperature_c.unwrap_or(4.0))
                    );
                }
                "lid open" => out.push_str("<lidOpen/>"),
                _ if s.measure.as_deref() == Some("melt") => {
                    // a melt read during the ramp to this temperature
                    let hi = s.temperature_c.unwrap_or(95.0);
                    let lo = prev_temp.unwrap_or(hi).min(hi);
                    let _ = write!(
                        out,
                        "<gradient><highTemperature>{}</highTemperature><lowTemperature>{}</lowTemperature><duration>{}</duration><measure>meltcurve</measure>",
                        f(hi),
                        f(lo),
                        dur(s.hold_s, &mut clipped)
                    );
                    if let Some(r) = s.ramp_c_per_s {
                        el(&mut out, "ramp", &f(r));
                    }
                    out.push_str("</gradient>");
                }
                _ => {
                    let _ = write!(
                        out,
                        "<temperature><temperature>{}</temperature><duration>{}</duration>",
                        f(s.temperature_c.unwrap_or(0.0)),
                        dur(s.hold_s, &mut clipped)
                    );
                    if s.measure.as_deref() == Some("real time") {
                        el(&mut out, "measure", "real time");
                    }
                    if let Some(r) = s.ramp_c_per_s {
                        el(&mut out, "ramp", &f(r));
                    }
                    out.push_str("</temperature>");
                }
            }
            out.push_str("</step>");
            if s.temperature_c.is_some() {
                prev_temp = s.temperature_c;
            }
        }
        if st.repeats > 1 && nr >= first {
            nr += 1;
            let _ = write!(
                out,
                "<step><nr>{nr}</nr><loop><goto>{first}</goto><repeat>{}</repeat></loop></step>",
                st.repeats - 1
            );
        }
    }
    if clipped > 0 {
        notes.push(format!(
            "{clipped} program steps without a hold time were written with a duration of 1 s (RDML requires a positive duration)"
        ));
    }
    out
}

/// Serialize a dataset's model as an RDML 1.3 document.
fn rdml_xml(d: &QpcrData, notes: &mut Vec<String>) -> String {
    let mut x = String::with_capacity(1 << 20);
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    let _ = write!(
        x,
        "<rdml xmlns:rdml=\"http://www.rdml.org\" xmlns=\"http://www.rdml.org\" version=\"{RDML_WRITE_VERSION}\">"
    );
    if let Some(t) = d.created_at.as_deref().and_then(date_time) {
        el(&mut x, "dateMade", &t);
    }
    for p in &d.experimenters {
        let _ = write!(x, "<experimenter id=\"{}\">", escape(&p.id));
        el(&mut x, "firstName", p.first_name.as_deref().unwrap_or("-"));
        el(&mut x, "lastName", p.last_name.as_deref().unwrap_or("-"));
        if let Some(v) = &p.email {
            el(&mut x, "email", v);
        }
        if let Some(v) = &p.lab {
            el(&mut x, "labName", v);
        }
        x.push_str("</experimenter>");
    }
    // dyes: declared, target reporters; targets without a dye get "unknown"
    let mut dyes = d.dye_names();
    let needs_unknown = d.targets.iter().any(|t| t.dye.is_none())
        || d.runs
            .iter()
            .flat_map(|r| &r.reactions)
            .flat_map(|x| &x.assays)
            .any(|a| a.target.as_ref().is_some_and(|t| d.target(t).is_none()) && a.dye.is_none());
    if needs_unknown && !dyes.iter().any(|x| x == "unknown") {
        dyes.push("unknown".into());
    }
    for y in &dyes {
        let _ = write!(x, "<dye id=\"{}\">", escape(y));
        if let Some(c) = d
            .dyes
            .iter()
            .find(|z| &z.name == y)
            .and_then(|z| z.chemistry.as_deref())
        {
            el(&mut x, "dyeChemistry", c);
        }
        x.push_str("</dye>");
    }
    let (sample_defs, sample_ids) = samples(d);
    for (id, k) in &sample_defs {
        let _ = write!(x, "<sample id=\"{}\">", escape(id));
        let src = d.sample(&k.base);
        if let Some(desc) = src.and_then(|s| s.description.as_deref()) {
            el(&mut x, "description", desc);
        }
        for (p, v) in src.map(|s| s.annotations.as_slice()).unwrap_or_default() {
            x.push_str("<annotation>");
            el(&mut x, "property", p);
            el(&mut x, "value", v);
            x.push_str("</annotation>");
        }
        let types: Vec<&str> = k.per_target.iter().map(|p| p.1).collect();
        let quantities: Vec<&Option<String>> = k.per_target.iter().map(|p| &p.2).collect();
        let unit = quantity_unit(src.and_then(|s| s.quantity_unit.as_deref()));
        if types.windows(2).all(|w| w[0] == w[1]) {
            el(
                &mut x,
                "type",
                types
                    .first()
                    .copied()
                    .unwrap_or(match src.and_then(|s| s.kind.as_deref()) {
                        Some(k) => sample_type(Some(k)),
                        None => "unkn",
                    }),
            );
        } else {
            for (t, ty, _) in &k.per_target {
                let _ = write!(x, "<type targetId=\"{}\">{ty}</type>", escape(t));
            }
        }
        if quantities.windows(2).all(|w| w[0] == w[1]) {
            if let Some(Some(q)) = quantities.first() {
                let _ = write!(
                    x,
                    "<quantity><value>{q}</value><unit>{unit}</unit></quantity>"
                );
            }
        } else {
            for (t, _, q) in &k.per_target {
                if let Some(q) = q {
                    let _ = write!(
                        x,
                        "<quantity targetId=\"{}\"><value>{q}</value><unit>{unit}</unit></quantity>",
                        escape(t)
                    );
                }
            }
        }
        x.push_str("</sample>");
    }
    // targets (declared, then any named only in results)
    let mut targets: Vec<(String, Option<String>)> = d
        .targets
        .iter()
        .map(|t| (t.name.clone(), t.dye.clone()))
        .collect();
    for a in d
        .runs
        .iter()
        .flat_map(|r| &r.reactions)
        .flat_map(|x| &x.assays)
    {
        if let Some(t) = &a.target
            && !targets.iter().any(|(n, _)| n == t)
        {
            targets.push((t.clone(), a.dye.clone()));
        }
    }
    for (name, dye) in &targets {
        let t = d.target(name);
        let _ = write!(x, "<target id=\"{}\">", escape(name));
        if let Some(desc) = t.and_then(|t| t.description.as_deref()) {
            el(&mut x, "description", desc);
        }
        let is_ref = t.and_then(|t| t.kind.as_deref()) == Some("reference")
            || d.reference_targets.iter().any(|r| r == name);
        el(&mut x, "type", if is_ref { "ref" } else { "toi" });
        if let Some(m) = t.and_then(|t| t.efficiency_method.as_deref()) {
            el(&mut x, "amplificationEfficiencyMethod", m);
        }
        if let Some(e) = t.and_then(|t| t.efficiency) {
            el(&mut x, "amplificationEfficiency", &f(e));
        }
        if let Some(e) = t.and_then(|t| t.efficiency_se) {
            el(&mut x, "amplificationEfficiencySE", &f(e));
        }
        if let Some(m) = t.and_then(|t| t.melting_temperature) {
            el(&mut x, "meltingTemperature", &f(m));
        }
        let _ = write!(
            x,
            "<dyeId id=\"{}\"/>",
            escape(dye.as_deref().unwrap_or("unknown"))
        );
        if let Some(t) = t
            && !t.sequences.is_empty()
        {
            x.push_str("<sequences>");
            for kind in [
                "forwardPrimer",
                "reversePrimer",
                "probe1",
                "probe2",
                "amplicon",
            ] {
                if let Some((_, s)) = t.sequences.iter().find(|(k, _)| k == kind) {
                    let _ = write!(x, "<{kind}><sequence>{}</sequence></{kind}>", escape(s));
                }
            }
            x.push_str("</sequences>");
        }
        x.push_str("</target>");
    }
    // programs: unique ids
    let mut program_ids: Vec<String> = Vec::new();
    for (i, p) in d.programs.iter().enumerate() {
        let mut id = if p.name.trim().is_empty() {
            format!("program {}", i + 1)
        } else {
            p.name.clone()
        };
        if program_ids.contains(&id) {
            id = format!("{id} ({})", i + 1);
        }
        let _ = write!(x, "<thermalCyclingConditions id=\"{}\">", escape(&id));
        if let Some(desc) = &p.description {
            el(&mut x, "description", desc);
        }
        if let Some(l) = p.lid_temperature_c {
            el(&mut x, "lidTemperature", &f(l));
        }
        x.push_str(&program_steps(p, notes));
        x.push_str("</thermalCyclingConditions>");
        program_ids.push(id);
    }
    // experiments → runs
    let mut experiments: Vec<(String, Vec<usize>)> = Vec::new();
    for (ri, r) in d.runs.iter().enumerate() {
        let e = r
            .experiment
            .clone()
            .or_else(|| d.name.clone())
            .unwrap_or_else(|| "experiment".into());
        match experiments.iter_mut().find(|(n, _)| *n == e) {
            Some((_, v)) => v.push(ri),
            None => experiments.push((e, vec![ri])),
        }
    }
    let mut dup_melt = 0u64;
    let mut dup_cycles = 0u64;
    let mut run_ids: Vec<String> = Vec::new();
    for (e, runs) in &experiments {
        let _ = write!(x, "<experiment id=\"{}\">", escape(e));
        if let Some(desc) = &d.description {
            el(&mut x, "description", desc);
        }
        for &ri in runs {
            let r = &d.runs[ri];
            let mut rid = if r.name.trim().is_empty() {
                format!("run {}", ri + 1)
            } else {
                r.name.clone()
            };
            if run_ids.contains(&rid) {
                rid = format!("{rid} ({})", ri + 1);
            }
            run_ids.push(rid.clone());
            let _ = write!(x, "<run id=\"{}\">", escape(&rid));
            if let Some(v) = &r.description {
                el(&mut x, "description", v);
            }
            if let Some(v) = r.instrument.as_ref().or(d.instrument.model.as_ref()) {
                el(&mut x, "instrument", v);
            }
            let sw = r.software.clone().or_else(|| d.instrument.software.clone());
            if let Some(name) = sw {
                x.push_str("<dataCollectionSoftware>");
                el(&mut x, "name", &name);
                el(
                    &mut x,
                    "version",
                    d.instrument
                        .software_version
                        .as_deref()
                        .unwrap_or("unknown"),
                );
                x.push_str("</dataCollectionSoftware>");
            }
            if let Some(v) = &r.background_method {
                el(&mut x, "backgroundDeterminationMethod", v);
            }
            if let Some(m) = cq_method(r.cq_method.as_deref()) {
                el(&mut x, "cqDetectionMethod", m);
            }
            if let Some(pid) = r.program.and_then(|p| program_ids.get(p)) {
                let _ = write!(x, "<thermalCyclingConditions id=\"{}\"/>", escape(pid));
            }
            let label = |l: &str| match l {
                "123" | "A1a1" => l.to_string(),
                _ => "ABC".to_string(),
            };
            let _ = write!(
                x,
                "<pcrFormat><rows>{}</rows><columns>{}</columns><rowLabel>{}</rowLabel><columnLabel>{}</columnLabel></pcrFormat>",
                r.rows.max(1),
                r.columns.max(1),
                label(&r.row_label),
                if r.column_label == "ABC" {
                    "ABC".to_string()
                } else {
                    label(&r.column_label)
                }
            );
            if let Some(t) = r.started_at.as_deref().and_then(date_time) {
                el(&mut x, "runDate", &t);
            }
            for (xi, rx) in r.reactions.iter().enumerate() {
                write_react(
                    &mut x,
                    r.columns.max(1),
                    rx,
                    &sample_ids[&(ri, xi)],
                    &mut dup_cycles,
                    &mut dup_melt,
                );
            }
            x.push_str("</run>");
        }
        x.push_str("</experiment>");
    }
    x.push_str("</rdml>\n");
    if dup_cycles > 0 {
        notes.push(format!("{dup_cycles} amplification points repeating a cycle number were left out (RDML cycle numbers are unique per curve)"));
    }
    if dup_melt > 0 {
        notes.push(format!("{dup_melt} melt points repeating a temperature were left out (RDML melt temperatures are unique per curve)"));
    }
    x
}

/// `2024-01-02T03:04:05...` → an xs:dateTime (date-only values get midnight).
fn date_time(s: &str) -> Option<String> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() >= 10 && b[4] == b'-' && b[7] == b'-' && b[..4].iter().all(u8::is_ascii_digit) {
        if b.len() == 10 {
            return Some(format!("{s}T00:00:00"));
        }
        if b.len() >= 19 && b[10] == b'T' {
            return Some(s.to_string());
        }
    }
    None
}

fn write_react(
    x: &mut String,
    columns: u32,
    rx: &Reaction,
    sample_id: &str,
    dup_cycles: &mut u64,
    dup_melt: &mut u64,
) {
    let id = u64::from(rx.row) * u64::from(columns) + u64::from(rx.column) + 1;
    let _ = write!(
        x,
        "<react id=\"{id}\"><sample id=\"{}\"/>",
        escape(sample_id)
    );
    let mut seen: Vec<&str> = Vec::new();
    for a in &rx.assays {
        let Some(t) = a.target.as_deref() else {
            continue;
        };
        if seen.contains(&t) {
            continue;
        }
        seen.push(t);
        write_data(x, rx, a, t, dup_cycles, dup_melt);
    }
    x.push_str("</react>");
}

fn write_data(
    x: &mut String,
    rx: &Reaction,
    a: &Assay,
    target: &str,
    dup_cycles: &mut u64,
    dup_melt: &mut u64,
) {
    let _ = write!(x, "<data><tar id=\"{}\"/>", escape(target));
    match (a.cq, a.cq_undetermined) {
        (Some(c), _) => el(x, "cq", &f(c)),
        (None, true) => el(x, "cq", "-1"),
        _ => {}
    }
    if let Some(v) = a.n0 {
        el(x, "N0", &f(v));
    }
    if let Some(v) = a.efficiency {
        el(x, "ampEff", &f(v));
    }
    if let Some(v) = a.efficiency_se {
        el(x, "ampEffSE", &f(v));
    }
    if let Some(tm) = a.tm.first() {
        el(x, "meltTemp", &f(*tm));
    }
    let excl = a
        .excluded
        .clone()
        .or_else(|| rx.omitted.then(|| "omitted".to_string()));
    if let Some(e) = excl {
        el(x, "excl", &e);
    }
    if let Some(n) = &a.note {
        el(x, "note", n);
    }
    if let Some(c) = &a.amplification {
        let mut used: Vec<u64> = Vec::new();
        for (cy, v) in c.cycles.iter().zip(&c.fluorescence) {
            if !(cy.is_finite() && v.is_finite()) {
                continue;
            }
            let key = cy.to_bits();
            if used.contains(&key) {
                *dup_cycles += 1;
                continue;
            }
            used.push(key);
            let _ = write!(
                x,
                "<adp><cyc>{}</cyc><fluor>{}</fluor></adp>",
                f(*cy),
                f(*v)
            );
        }
    }
    if let Some(m) = &a.melt {
        let mut used = std::collections::BTreeSet::new();
        for (t, v) in m.temperature.iter().zip(&m.fluorescence) {
            if !(t.is_finite() && v.is_finite()) {
                continue;
            }
            if !used.insert(t.to_bits()) {
                *dup_melt += 1;
                continue;
            }
            let _ = write!(x, "<mdp><tmp>{}</tmp><fluor>{}</fluor></mdp>", f(*t), f(*v));
        }
    }
    if let Some(b) = a.background {
        el(x, "bgFluor", &f(b));
        if let Some(s) = a.background_slope.filter(|s| *s != 0.0) {
            el(x, "bgFluorSlp", &f(s));
        }
    }
    if let Some(t) = a.threshold.filter(|_| a.threshold_used) {
        el(x, "quantFluor", &f(t));
    }
    x.push_str("</data>");
}

/// Counts that the read-back must reproduce.
#[derive(Debug, Default, PartialEq)]
struct Counts {
    runs: u32,
    reactions: u64,
    data: u64,
    cq: u64,
    adp: u64,
    mdp: u64,
    /// Every Cq, in order.
    cq_values: Vec<u64>,
}

fn counts(d: &QpcrData) -> Counts {
    let mut c = Counts {
        runs: d.runs.len() as u32,
        ..Counts::default()
    };
    for r in &d.runs {
        for rx in &r.reactions {
            c.reactions += 1;
            let mut seen: Vec<&str> = Vec::new();
            for a in &rx.assays {
                let Some(t) = a.target.as_deref() else {
                    continue;
                };
                if seen.contains(&t) {
                    continue;
                }
                seen.push(t);
                c.data += 1;
                if let Some(q) = a.cq {
                    c.cq += 1;
                    c.cq_values.push(q.to_bits());
                }
                if let Some(cv) = &a.amplification {
                    let mut used = std::collections::BTreeSet::new();
                    c.adp += cv
                        .cycles
                        .iter()
                        .zip(&cv.fluorescence)
                        .filter(|(cy, v)| {
                            cy.is_finite() && v.is_finite() && used.insert(cy.to_bits())
                        })
                        .count() as u64;
                }
                if let Some(m) = &a.melt {
                    let mut used = std::collections::BTreeSet::new();
                    c.mdp += m
                        .temperature
                        .iter()
                        .zip(&m.fluorescence)
                        .filter(|(t, v)| t.is_finite() && v.is_finite() && used.insert(t.to_bits()))
                        .count() as u64;
                }
            }
        }
    }
    c
}

/// Write `ds` as RDML 1.3 to `output`; the file is verified by reading it back before it is
/// renamed into place. `overwrite` replaces an existing output.
pub fn export_rdml(ds: &QpcrDataset, output: &Path, overwrite: bool) -> Result<RdmlExportReport> {
    if output == ds.path() {
        return Err(Error::Usage(
            "output path must differ from the input; raw files are never modified".into(),
        ));
    }
    if output.exists() && !overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let d = &ds.data;
    let mut notes = Vec::new();
    let xml = rdml_xml(d, &mut notes);
    let expected = counts(d);
    let dir = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        ".{}.tmp-{}",
        output.file_name().map_or_else(
            || "export.rdml".into(),
            |n| n.to_string_lossy().into_owned()
        ),
        std::process::id()
    ));
    let cleanup = |e: Error| {
        let _ = std::fs::remove_file(&tmp);
        e
    };
    {
        let file = std::fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let mut w = ZipWriter::new(std::io::BufWriter::new(file));
        w.add("rdml_data.xml", xml.as_bytes())
            .map_err(|e| cleanup(Error::io(&tmp, e)))?;
        let buf = w.finish().map_err(|e| cleanup(Error::io(&tmp, e)))?;
        let file = buf
            .into_inner()
            .map_err(|e| cleanup(Error::io(&tmp, e.into_error())))?;
        file.sync_all().map_err(|e| cleanup(Error::io(&tmp, e)))?;
    }
    // read back
    let verify = || -> Result<Counts> {
        let z = ZipIndex::open(&Fs::local(), &tmp, crate::RDML_FORMAT_ID)?;
        let text = z
            .read_text("rdml_data.xml")?
            .ok_or_else(|| Error::Other("written RDML archive has no rdml_data.xml".into()))?;
        Ok(counts(&crate::rdml::parse_rdml(&text)?))
    };
    let got = verify().map_err(cleanup)?;
    if got != expected {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Other(format!(
            "RDML read-back differs from the source (expected {expected:?}, read {got:?}); nothing was written"
        )));
    }
    let bytes = std::fs::metadata(&tmp)
        .map_err(|e| Error::io(&tmp, e))?
        .len();
    std::fs::rename(&tmp, output).map_err(|e| cleanup(Error::io(output, e)))?;
    Ok(RdmlExportReport {
        input: ds.path().display().to_string(),
        output: output.display().to_string(),
        format: "rdml".into(),
        rdml_version: RDML_WRITE_VERSION.into(),
        runs: expected.runs,
        reactions: expected.reactions,
        data_elements: expected.data,
        cq_values: expected.cq,
        amplification_points: expected.adp,
        melt_points: expected.mdp,
        samples: samples(d).0.len() as u32,
        targets: d.targets.len() as u32,
        dyes: d.dye_names().len() as u32,
        bytes_written: bytes,
        verified: true,
        notes,
    })
}

/// Default output path: the input with `.rdml` (`name.eds` → `name.rdml`; an `.rdml` input
/// gets `name.export.rdml`).
pub fn default_rdml_output(input: &Path) -> std::path::PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "export".into(), |s| s.to_string_lossy().into_owned());
    let is_rdml = input
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("rdml") || e.eq_ignore_ascii_case("rdm"));
    input.with_file_name(if is_rdml {
        format!("{stem}.export.rdml")
    } else {
        format!("{stem}.rdml")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(
            date_time("2024-01-02").as_deref(),
            Some("2024-01-02T00:00:00")
        );
        assert_eq!(
            date_time("2024-01-02T03:04:05.1Z").as_deref(),
            Some("2024-01-02T03:04:05.1Z")
        );
        assert_eq!(date_time("yesterday"), None);
        assert_eq!(cq_method(Some("LinRegPCR")), Some("other"));
        assert_eq!(sample_type(Some("standard")), "std");
        assert_eq!(
            default_rdml_output(Path::new("/a/x.eds")),
            Path::new("/a/x.rdml")
        );
        assert_eq!(
            default_rdml_output(Path::new("/a/x.rdml")),
            Path::new("/a/x.export.rdml")
        );
    }
}
