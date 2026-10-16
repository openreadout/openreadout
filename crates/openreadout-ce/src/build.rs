//! What an ABIF file holds, in OpenReadout's model: the dye channels as traces, the base calls
//! (Sanger) or the size-standard matches (fragment analysis) as tables, and the run as
//! experiment facts.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind as K, Observations, Scope};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::Finding;
use openreadout_core::provenance::Source;
use openreadout_core::series::{
    Facts, SeriesChannel, SeriesColumn, SeriesFile, SeriesTable, SeriesTrace, json_num,
};
use serde_json::{Value, json};

use crate::abif::{Abif, Item};
use crate::ladders::{self, Ladder};
use crate::sizing::{self, Sizing};

/// Most dye channels a file may hold (DATA 1–4 and 105–199).
const MAX_DYES: i32 = 99;

/// What kind of run the file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunKind {
    /// Base calls: a sequencing (`.ab1`) file.
    Sanger,
    /// No base calls: fragment analysis (`.fsa`, `.hid`).
    Fragment,
}

impl RunKind {
    pub(crate) fn id(self) -> &'static str {
        match self {
            RunKind::Sanger => "sanger",
            RunKind::Fragment => "fragment_analysis",
        }
    }
}

/// One dye channel: its stored number, the dye's name and wavelength, and the base it reports
/// (sequencing).
#[derive(Debug, Clone)]
struct Dye {
    /// 1-based dye number (DATA 1–4 raw, 105+ for the fifth and later).
    number: i32,
    name: Option<String>,
    wavelength_nm: Option<f64>,
    base: Option<char>,
    /// `DyeB` is `S`: the size-standard dye.
    standard: bool,
}

/// The raw-data tag number of dye `k` (1-based) and its analysed counterpart.
fn data_numbers(k: i32) -> (i32, i32) {
    if k <= 4 {
        (k, k + 8)
    } else {
        (100 + k, 200 + k)
    }
}

fn dyes(a: &Abif) -> Vec<Dye> {
    let order: Vec<char> = a
        .text("FWO_", 1)
        .map(|t| t.chars().filter(char::is_ascii_alphabetic).collect())
        .unwrap_or_default();
    let declared = a
        .number("Dye#", 1)
        .and_then(|v| (v >= 1.0 && v <= f64::from(MAX_DYES)).then_some(v as i32));
    let mut out = Vec::new();
    for k in 1..=declared.unwrap_or(MAX_DYES) {
        let (raw, analysed) = data_numbers(k);
        if a.get("DATA", raw).is_none() && a.get("DATA", analysed).is_none() {
            if declared.is_none() && k > 4 {
                break;
            }
            continue;
        }
        let n = usize::try_from(k - 1).unwrap_or(0);
        out.push(Dye {
            number: k,
            name: a.text("DyeN", k),
            wavelength_nm: a.number("DyeW", k).filter(|w| *w > 0.0),
            base: order.get(n).map(char::to_ascii_uppercase),
            standard: a
                .get("DyeB", k)
                .and_then(Item::text)
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("S")),
        });
    }
    out
}

/// A dye channel's name in a trace: the base for sequencing, else the dye's name.
fn channel_name(d: &Dye, kind: RunKind) -> String {
    match (kind, d.base, &d.name) {
        (RunKind::Sanger, Some(b), _) => b.to_string(),
        (_, _, Some(n)) => n.clone(),
        (_, Some(b), None) => b.to_string(),
        _ => format!("dye {}", d.number),
    }
}

fn dye_channel(d: &Dye, kind: RunKind, tag: &str) -> SeriesChannel {
    let mut c = SeriesChannel::new(channel_name(d, kind), Some("RFU"), "int16");
    c.extra.insert("dye_number".into(), json!(d.number));
    if let Some(n) = &d.name {
        c.extra.insert("dye".into(), json!(n));
    }
    if let Some(w) = d.wavelength_nm {
        c.extra.insert("wavelength_nm".into(), json_num(w));
    }
    if let Some(b) = d.base {
        c.extra.insert("base".into(), json!(b.to_string()));
    }
    if d.standard {
        c.extra.insert("size_standard".into(), json!(true));
    }
    c.extra.insert("stored_as".into(), json!(tag));
    c
}

/// The dye channels of one set (raw or analysed), equal in length; `None` when absent.
fn dye_set(
    a: &Abif,
    dyes: &[Dye],
    analysed: bool,
    kind: RunKind,
    findings: &mut Vec<Finding>,
) -> Option<(Vec<SeriesChannel>, Vec<Vec<f64>>)> {
    let mut channels = Vec::new();
    let mut values: Vec<Vec<f64>> = Vec::new();
    for d in dyes {
        let (raw, an) = data_numbers(d.number);
        let number = if analysed { an } else { raw };
        let Some(item) = a.get("DATA", number) else {
            continue;
        };
        let Some(v) = item.shorts() else {
            findings.push(Finding::warning(
                "unexpected_type",
                format!("DATA{number} is not an array of 16-bit integers; left out"),
            ));
            continue;
        };
        channels.push(dye_channel(d, kind, &format!("DATA{number}")));
        values.push(v);
    }
    if values.is_empty() {
        return None;
    }
    let n = values.iter().map(Vec::len).min().unwrap_or(0);
    if values.iter().any(|v| v.len() != n) {
        findings.push(Finding::warning(
            "channel_lengths_differ",
            format!(
                "the {} dye channels differ in length; cut to the shortest ({n} scans)",
                if analysed { "analysed" } else { "raw" }
            ),
        ));
        for v in &mut values {
            v.truncate(n);
        }
    }
    // sequencing: A, C, G, T, then anything else
    if kind == RunKind::Sanger {
        let rank = |c: &SeriesChannel| match c.name.as_str() {
            "A" => 0,
            "C" => 1,
            "G" => 2,
            "T" => 3,
            _ => 4,
        };
        let mut both: Vec<(SeriesChannel, Vec<f64>)> = channels.into_iter().zip(values).collect();
        both.sort_by_key(|(c, _)| rank(c));
        (channels, values) = both.into_iter().unzip();
    }
    Some((channels, values))
}

fn scan_axis(n: usize) -> Value {
    json!({"quantity": "scan", "first": 0, "step": 1, "last": n.saturating_sub(1), "size": n})
}

/// Base calls of one set (1 = edited, 2 = as called): bases, quality values, peak scans.
struct Calls {
    bases: Vec<String>,
    quality: Option<Vec<f64>>,
    scans: Option<Vec<f64>>,
}

fn calls(a: &Abif, number: i32) -> Option<Calls> {
    let item = a.get("PBAS", number)?;
    let text: String = item
        .data
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| char::from(b))
        .collect();
    let bases: Vec<String> = text.chars().map(|c| c.to_string()).collect();
    let quality = a
        .get("PCON", number)
        .map(|q| q.data.iter().map(|&b| f64::from(b)).collect::<Vec<f64>>());
    let scans = a.get("PLOC", number).and_then(Item::shorts);
    Some(Calls {
        bases,
        quality,
        scans,
    })
}

fn calls_table(c: &Calls, name: &str, findings: &mut Vec<Finding>) -> SeriesTable {
    let n = c.bases.len();
    let mut cols = vec![
        SeriesColumn::numbers("position", None, (1..=n).map(|i| i as f64).collect()),
        SeriesColumn::texts("base", &c.bases),
    ];
    let mut fit = |what: &str, v: &Option<Vec<f64>>| -> Option<Vec<f64>> {
        let v = v.as_ref()?;
        if v.len() != n {
            findings.push(Finding::warning(
                "call_lengths_differ",
                format!("{name}: {n} bases but {} {what}", v.len()),
            ));
        }
        let mut v = v.clone();
        v.resize(n, f64::NAN);
        Some(v)
    };
    if let Some(q) = fit("quality values", &c.quality) {
        cols.push(SeriesColumn::numbers("quality", None, q));
    }
    if let Some(s) = fit("peak positions", &c.scans) {
        cols.push(SeriesColumn::numbers("scan", None, s));
    }
    let mut extra = BTreeMap::new();
    extra.insert("kind".into(), json!("base_calls"));
    SeriesTable {
        name: name.into(),
        columns: cols,
        extra,
    }
}

/// `YYYY-MM-DDTHH:MM:SS` from a date and a time tag, and seconds since midnight.
fn when(a: &Abif, n: i32) -> Option<(String, String, f64)> {
    let d = a.get("RUND", n)?.date()?;
    let (t, secs) = a.get("RUNT", n)?.time()?;
    Some((format!("{d}T{t}"), d, secs))
}

/// Seconds between two date-times of the same or the next day.
fn span(a: &(String, String, f64), b: &(String, String, f64)) -> Option<f64> {
    let s = b.2 - a.2;
    if a.1 == b.1 && s >= 0.0 {
        Some(s)
    } else if a.1 < b.1 && s < 0.0 {
        Some(s + 86_400.0)
    } else {
        None
    }
}

/// Mean of the finite values.
fn mean(v: &[f64]) -> Option<f64> {
    let f: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    (!f.is_empty()).then(|| f.iter().sum::<f64>() / f.len() as f64)
}

/// The size standard of a fragment run: the dye marked `S`, else the dye named LIZ, ROX or
/// similar, else the last dye; the ladder named by `StdF`, else the best fit among the ladders
/// of that dye.
fn size_run(
    a: &Abif,
    dyes: &[Dye],
    raw: &(Vec<SeriesChannel>, Vec<Vec<f64>>),
    notes: &mut Vec<String>,
) -> Option<(usize, Sizing, &'static str)> {
    let names: Vec<String> = dyes
        .iter()
        .map(|d| d.name.clone().unwrap_or_default().to_ascii_uppercase())
        .collect();
    let k = dyes
        .iter()
        .position(|d| d.standard)
        .or_else(|| {
            names
                .iter()
                .position(|n| n.contains("LIZ") || n.contains("ROX") || n.contains("ORANGE"))
        })
        .or_else(|| dyes.len().checked_sub(1))?;
    let dye = &dyes[k];
    let col = raw
        .0
        .iter()
        .position(|c| c.extra.get("dye_number") == Some(&json!(dye.number)))?;
    let y = &raw.1[col];
    let nz = sizing::noise(y);
    let peaks = sizing::find_peaks(y, (8.0 * nz).max(30.0), 3);
    let named = a
        .text("StdF", 1)
        .and_then(|t| ladders::named(&t).map(|l| (l, t)));
    let (sizing, how) = if let Some((l, t)) = named {
        let s = sizing::size_with(l, &peaks);
        if s.is_none() {
            notes.push(format!(
                "the size standard the file names ({t}) did not match the peaks of dye {}; fragments are not sized",
                dye.name.as_deref().unwrap_or("?")
            ));
        }
        (s?, "named in the file")
    } else {
        let candidates: Vec<&'static Ladder> = ladders::for_dye(&names[k]);
        let s = sizing::best_of(&candidates, &peaks);
        if s.is_none() {
            notes.push(format!(
                "no size standard matched the peaks of dye {} (the file names none); fragments are not sized",
                dye.name.as_deref().unwrap_or("?")
            ));
        }
        (s?, "best fit (the file names no size standard)")
    };
    Some((col, sizing, how))
}

/// Build the model of a parsed file.
pub(crate) fn build(a: &Abif) -> SeriesFile {
    let mut findings = a.findings.clone();
    let mut notes = Vec::new();
    let mut obs = Observations::default();
    let kind = if a.get("PBAS", 2).is_some() || a.get("PBAS", 1).is_some() {
        RunKind::Sanger
    } else {
        RunKind::Fragment
    };
    let dyes = dyes(a);
    let mut traces: Vec<SeriesTrace> = Vec::new();
    let mut tables: Vec<SeriesTable> = Vec::new();
    let analysed = dye_set(a, &dyes, true, kind, &mut findings);
    let raw = dye_set(a, &dyes, false, kind, &mut findings);
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::Spec);
    provenance.insert("tables".into(), Source::Spec);

    // fragment analysis: the sized trace first
    let mut sizing_facts: Option<(Sizing, &'static str, String)> = None;
    if kind == RunKind::Fragment
        && let Some(r) = &raw
        && let Some((col, s, how)) = size_run(a, &dyes, r, &mut notes)
        && let Some((lo, hi)) = s.range()
    {
        let first = lo.ceil().max(0.0) as usize;
        let last = (hi.floor() as usize).min(r.1[0].len().saturating_sub(1));
        if last > first {
            let sizes: Vec<f64> = (first..=last)
                .map(|i| s.size_at(i as f64).unwrap_or(f64::NAN))
                .collect();
            // a sized axis must rise; Local Southern rises between matched fragments
            let rising = sizes.windows(2).all(|w| w[1] > w[0]);
            if rising {
                let mut channels = vec![SeriesChannel::new("size", Some("bp"), "float64")];
                let mut values = vec![sizes];
                for (c, v) in r.0.iter().zip(&r.1) {
                    channels.push(c.clone());
                    values.push(v[first..=last].to_vec());
                }
                let mut extra = BTreeMap::new();
                extra.insert(
                    "axis".into(),
                    json!({"quantity": "size", "unit": "bp", "irregular": true, "channel": 0, "size": last - first + 1}),
                );
                extra.insert("kind".into(), json!("electropherogram"));
                extra.insert("first_scan".into(), json!(first));
                extra.insert("size_standard".into(), json!(s.ladder.name));
                extra.insert("size_standard_channel".into(), json!(col + 1));
                extra.insert("sizing_method".into(), json!("local_southern"));
                traces.push(SeriesTrace {
                    name: "sized".into(),
                    channels,
                    sweeps: vec![values],
                    sample_rate_hz: 0.0,
                    start_s: None,
                    extra,
                });
                let dye = r.0[col].name.clone();
                sizing_facts = Some((s, how, dye));
                provenance.insert("traces[0].channels[0]".into(), Source::Inferred);
            } else {
                notes.push("the ladder fit does not rise steadily; fragments are not sized".into());
            }
        }
    }

    if let Some((channels, values)) = analysed {
        let n = values[0].len();
        let mut extra = BTreeMap::new();
        extra.insert("axis".into(), scan_axis(n));
        extra.insert(
            "kind".into(),
            json!(if kind == RunKind::Sanger {
                "sanger_trace"
            } else {
                "electropherogram"
            }),
        );
        extra.insert("processed".into(), json!(true));
        traces.push(SeriesTrace {
            name: "analyzed".into(),
            channels,
            sweeps: vec![values],
            sample_rate_hz: 0.0,
            start_s: None,
            extra,
        });
    }
    if let Some((channels, values)) = raw {
        let n = values[0].len();
        let mut extra = BTreeMap::new();
        extra.insert("axis".into(), scan_axis(n));
        extra.insert(
            "kind".into(),
            json!(if kind == RunKind::Sanger {
                "sanger_trace"
            } else {
                "electropherogram"
            }),
        );
        extra.insert("processed".into(), json!(false));
        // values above 32767 the instrument lists apart
        let over = (1..=8).filter(|k| a.get("OvrI", *k).is_some()).count();
        if over > 0 {
            extra.insert("over_range_lists".into(), json!(over));
            notes.push(format!(
                "{over} channels list values above 32767 apart (OvrI/OvrV); the raw trace holds them clipped as stored"
            ));
        }
        if let Some(s) = a.get("Satd", 1).and_then(Item::numbers) {
            extra.insert("saturated_scans".into(), json!(s.len()));
        }
        traces.push(SeriesTrace {
            name: "raw".into(),
            channels,
            sweeps: vec![values],
            sample_rate_hz: 0.0,
            start_s: None,
            extra,
        });
    }
    // run monitors: voltage (decavolts), current, laser power, temperature
    let monitors = [
        (5, "voltage", "V", 10.0),
        (6, "current", "mA", 1.0),
        (7, "laser_power", "mW", 1.0),
        (8, "temperature", "°C", 1.0),
    ];
    let mut mch = Vec::new();
    let mut mval: Vec<Vec<f64>> = Vec::new();
    for (num, name, unit, scale) in monitors {
        if let Some(v) = a.get("DATA", num).and_then(Item::shorts) {
            let mut c = SeriesChannel::new(name, Some(unit), "int16");
            c.extra
                .insert("stored_as".into(), json!(format!("DATA{num}")));
            mch.push(c);
            mval.push(v.into_iter().map(|x| x * scale).collect());
        }
    }
    if !mval.is_empty() {
        let n = mval.iter().map(Vec::len).min().unwrap_or(0);
        for v in &mut mval {
            v.truncate(n);
        }
        let mut extra = BTreeMap::new();
        extra.insert(
            "axis".into(),
            json!({"quantity": "sample", "first": 0, "step": 1, "last": n.saturating_sub(1), "size": n}),
        );
        extra.insert("kind".into(), json!("run_monitor"));
        traces.push(SeriesTrace {
            name: "run monitor".into(),
            channels: mch,
            sweeps: vec![mval],
            sample_rate_hz: 0.0,
            start_s: None,
            extra,
        });
    }

    // tables
    let mut call_summary: Option<(usize, Option<f64>, usize)> = None;
    if kind == RunKind::Sanger {
        let called = calls(a, 2).or_else(|| calls(a, 1));
        if let Some(c) = &called {
            tables.push(calls_table(c, "base calls", &mut findings));
            let q20 = c
                .quality
                .as_ref()
                .map_or(0, |q| q.iter().filter(|v| **v >= 20.0).count());
            call_summary = Some((c.bases.len(), c.quality.as_deref().and_then(mean), q20));
            let analysed_n = traces
                .iter()
                .find(|t| t.name == "analyzed")
                .map(|t| t.sweeps[0][0].len());
            if let (Some(s), Some(n)) = (&c.scans, analysed_n)
                && s.iter().any(|v| *v < 0.0 || *v >= n as f64)
            {
                findings.push(Finding::warning(
                    "call_outside_trace",
                    format!("base-call peak positions fall outside the analysed trace ({n} scans)"),
                ));
            }
        }
        if let (Some(e), Some(c)) = (calls(a, 1), &called)
            && a.get("PBAS", 2).is_some()
            && (e.bases != c.bases || e.quality != c.quality || e.scans != c.scans)
        {
            tables.push(calls_table(&e, "edited base calls", &mut findings));
            notes.push("the file holds edited base calls that differ from the basecaller's; table 0 holds the basecaller's, table 1 the edited ones".into());
        }
        if let Some(t) = traces
            .iter_mut()
            .find(|t| t.name == "analyzed" || t.name == "raw")
        {
            t.extra.insert("base_calls_table".into(), json!(0));
        }
    }
    if let Some((s, how, dye)) = &sizing_facts {
        let mut extra = BTreeMap::new();
        extra.insert("kind".into(), json!("size_standard"));
        extra.insert("size_standard".into(), json!(s.ladder.name));
        extra.insert("product".into(), json!(s.ladder.product));
        extra.insert("dye".into(), json!(dye));
        extra.insert("chosen".into(), json!(how));
        extra.insert("quality".into(), json_num(s.quality));
        if let Some(m) = s.mean_abs_residual {
            extra.insert("mean_abs_residual_bp".into(), json_num(m));
        }
        if !s.missing.is_empty() {
            extra.insert("missing_bp".into(), json!(s.missing));
        }
        let first = traces[0]
            .extra
            .get("first_scan")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        tables.push(SeriesTable {
            name: "size standard".into(),
            columns: vec![
                SeriesColumn::numbers(
                    "size",
                    Some("bp"),
                    s.matches.iter().map(|m| m.size).collect(),
                ),
                SeriesColumn::numbers("scan", None, s.matches.iter().map(|m| m.scan).collect()),
                SeriesColumn::numbers(
                    "sample",
                    None,
                    s.matches.iter().map(|m| m.scan - first).collect(),
                ),
                SeriesColumn::numbers(
                    "height",
                    Some("RFU"),
                    s.matches.iter().map(|m| m.height).collect(),
                ),
                SeriesColumn::numbers(
                    "residual",
                    Some("bp"),
                    s.matches
                        .iter()
                        .map(|m| m.residual.unwrap_or(f64::NAN))
                        .collect(),
                ),
            ],
            extra,
        });
        provenance.insert(format!("tables[{}]", tables.len() - 1), Source::Inferred);
        if let Some(t) = traces.first_mut() {
            t.extra
                .insert("size_standard_table".into(), json!(tables.len() - 1));
        }
        notes.push(format!(
            "fragments sized here against {} on dye {dye} ({how}): {} of {} fragments matched{}",
            s.ladder.product,
            s.matches.len(),
            s.ladder.sizes.len(),
            s.mean_abs_residual
                .map_or(String::new(), |m| format!(", mean residual {m:.2} bp"))
        ));
        obs.feature(
            K::Record,
            format!("size standard {}", s.ladder.name),
            &[Scope::Traces],
        );
        obs.feature(
            K::Derivation,
            "fragment size by Local Southern",
            &[Scope::Traces],
        );
    }

    // experiment
    let mut f = Facts::new();
    f.set_with(
        "instrument.vendor",
        "Applied Biosystems",
        "the file format",
        Source::Inferred,
    );
    let model = a
        .text("HCFG", 3)
        .or_else(|| a.text("MODL", 1))
        .map(|m| m.trim().to_string());
    if let Some(m) = &model {
        f.set_with("instrument.model", m, "HCFG3 / MODL1", Source::Spec);
    }
    if let Some(serial) = a.text("HCFG", 4).and_then(|t| {
        t.split(';')
            .find_map(|kv| kv.trim().strip_prefix("SerialNumber=").map(str::to_string))
    }) {
        f.set_with(
            "instrument.serial",
            &serial,
            "HCFG4 SerialNumber",
            Source::Spec,
        );
    }
    if let Some(v) = a.text("SVER", 1) {
        f.set_with(
            "instrument.software",
            "Data Collection",
            "SVER1",
            Source::Inferred,
        );
        f.set_with("instrument.software_version", &v, "SVER1", Source::Spec);
    }
    let sample = a.text("SMPL", 1).or_else(|| a.text("SpNm", 1));
    if let Some(s) = &sample {
        f.set_with("sample.name", s, "SMPL1 / SpNm1", Source::Spec);
    }
    if let Some(w) = a.text("TUBE", 1) {
        f.set_with("sample.well", &w, "TUBE1", Source::Spec);
    }
    if let Some(b) = a.text("CTID", 1) {
        f.set_with("sample.barcode", &b, "CTID1 (plate barcode)", Source::Spec);
    }
    if let Some(id) = a.text("LIMS", 1) {
        f.set_with("sample.id", &id, "LIMS1", Source::Spec);
    }
    if let Some(u) = a.text("User", 1) {
        f.set_with("acquisition.operator", &u, "User1", Source::Spec);
    }
    if let Some(c) = a.text("CMNT", 1) {
        f.set_with("acquisition.comment", &c, "CMNT1", Source::Spec);
    }
    let start = when(a, 1);
    let stop = when(a, 2);
    if let Some(s) = &start {
        f.set_with(
            "acquisition.started_at",
            &s.0,
            "RUND1 RUNT1 (local time)",
            Source::Spec,
        );
    }
    if let Some(s) = &stop {
        f.set_with(
            "acquisition.ended_at",
            &s.0,
            "RUND2 RUNT2 (local time)",
            Source::Spec,
        );
    }
    if let (Some(a0), Some(b0)) = (&start, &stop)
        && let Some(d) = span(a0, b0)
    {
        f.duration(d, "RUNT1 to RUNT2");
    }
    if let Some(m) = a.text("RMdN", 1).or_else(|| a.text("MODF", 1)) {
        f.set_with(
            "method.name",
            &m,
            "RMdN1 / MODF1 (run module)",
            Source::Spec,
        );
    }
    f.plain(
        "run_kind",
        kind.id(),
        "the tags present (base calls or not)",
    );
    if let Some(v) = a.text("RunN", 1) {
        f.plain("run_name", v, "RunN1");
    }
    if let Some(v) = a.text("RPrN", 1) {
        f.plain("run_protocol", v, "RPrN1");
    }
    if let Some(v) = a.text("GTyp", 1) {
        f.plain("polymer", v, "GTyp1");
    }
    if let Some(v) = a.text("DySN", 1) {
        f.plain("dye_set", v, "DySN1");
    }
    if let Some(v) = a.number("LANE", 1) {
        f.plain("capillary", v, "LANE1");
    }
    if let Some(v) = a.number("NLNE", 1) {
        f.plain("capillaries", v, "NLNE1");
    }
    if let Some(v) = a.number("LNTD", 1) {
        f.number("length_to_detector", v, "cm", "LNTD1");
    }
    if let Some(v) = a.number("EPVt", 1) {
        f.number("run_voltage", v, "V", "EPVt1");
    }
    if let Some(v) = a.number("InVt", 1) {
        f.number("injection_voltage", v, "V", "InVt1");
    }
    if let Some(v) = a.number("InSc", 1) {
        f.number("injection_time", v, "s", "InSc1");
    }
    if let Some(v) = a.number("Tmpr", 1) {
        f.number("run_temperature", v, "°C", "Tmpr1");
    }
    if let Some(v) = a.number("LsrP", 1) {
        f.number("laser_power", v, "µW", "LsrP1");
    }
    if let Some(v) = a.text("PDMF", 2).or_else(|| a.text("PDMF", 1)) {
        f.plain("mobility_file", v, "PDMF");
    }
    if let Some(v) = a.text("SPAC", 2) {
        f.plain("basecaller", v, "SPAC2");
    }
    if let Some(v) = a.text("SVER", 2) {
        f.plain("basecaller_version", v, "SVER2");
    }
    if let Some(v) = a.text("StdF", 1) {
        f.plain("size_standard_file", v, "StdF1");
    }
    if let Some(v) = a.text("PANL", 1) {
        f.plain("panel", v, "PANL1");
    }
    let dye_names: Vec<String> = dyes.iter().filter_map(|d| d.name.clone()).collect();
    if !dye_names.is_empty() {
        f.plain("dyes", dye_names.join(", "), "DyeN");
    }
    if let Some((n, q, q20)) = call_summary {
        f.plain("read_length", n as f64, "PBAS2 (bases called)");
        if let Some(q) = q {
            f.plain(
                "mean_quality",
                (q * 10.0).round() / 10.0,
                "PCON2 (quality values)",
            );
        }
        f.plain(
            "bases_q20",
            q20 as f64,
            "PCON2 (bases with quality 20 or more)",
        );
    }
    if let Some((s, how, dye)) = &sizing_facts {
        f.plain(
            "size_standard",
            s.ladder.product,
            &format!("sizing ({how})"),
        );
        f.plain("size_standard_dye", dye.clone(), "sizing");
    }
    let what = match kind {
        RunKind::Sanger => {
            f.technique(
                "OBI:0000695",
                "the file holds base calls (a sequencing run)",
            );
            f.instrument_kind("OBI:0400103");
            let n = call_summary.map_or(0, |c| c.0);
            format!("Sanger sequencing trace, {n} bases called")
        }
        RunKind::Fragment => {
            f.technique(
                "CHMO:0001024",
                "the file holds dye traces and no base calls (fragment analysis)",
            );
            f.instrument_kind("OBI:0001132");
            format!(
                "capillary electrophoresis fragment analysis, {} dyes",
                dyes.len()
            )
        }
    };
    f.measurement(
        MeasurementKind::Trace,
        (0..traces.len() as u32).collect(),
        what,
        Some(if kind == RunKind::Sanger {
            "OBI:0000695"
        } else {
            "CHMO:0001024"
        }),
    );

    // assurance observations
    obs.feature(K::Acquisition, kind.id(), &[Scope::Traces, Scope::Tables]);
    if let Some(m) = &model {
        obs.feature(K::Instrument, m, &[]);
    }
    if let Some(v) = a.text("SVER", 4) {
        obs.feature(K::Layout, v, &[Scope::Traces]);
    }
    if let Some(v) = a.text("SVER", 1) {
        obs.feature(K::WriterVersion, format!("Data Collection {v}"), &[]);
    }
    obs.feature(
        K::FormatVersion,
        format!("ABIF {}", a.version),
        &[Scope::Traces],
    );
    for t in &traces {
        obs.feature(K::Record, format!("{} trace", t.name), &[Scope::Traces]);
    }
    if !tables.is_empty() && kind == RunKind::Sanger {
        obs.feature(K::Record, "base calls".to_string(), &[Scope::Tables]);
    }

    let format_version = Some(format!("{}.{:02}", a.version / 100, a.version % 100));
    if traces.is_empty() {
        findings.push(Finding::warning(
            "no_traces",
            "the file holds no dye channels (no DATA 1–4 or 9–12)",
        ));
    }
    SeriesFile {
        format_version,
        traces,
        tables,
        experiment: Some(f.build()),
        vendor: a.vendor(),
        entries: a.entries.clone(),
        findings,
        notes,
        provenance,
        observations: obs,
        checks: vec![
            "every directory entry checked against the file length and its element size".into(),
        ],
        members: Vec::new(),
    }
}
