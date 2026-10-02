//! `link`: which files measured the same sample (or are the same acquisition stored twice), across
//! instruments and formats, with the evidence for every link.
//!
//! Every data set is opened for its headers (as `info` does; of a mass-spectrometry run also its
//! first, middle and last spectrum). Evidence between two data sets:
//!
//! | kind | when | confidence |
//! | --- | --- | --- |
//! | `same_barcode` | the same plate or tube barcode | high |
//! | `same_plate_well` | the same plate barcode and the same well | high |
//! | `same_sample_id` | the same recorded sample id (not a software default) | high |
//! | `conversion_source` | one file names the other as its source (an mzML's `sourceFile`), by file stem or sample id | high |
//! | `same_acquisition` | the same start time (to the millisecond when both record fractions, else the second; 2 s with one instrument serial), allowing a whole-hour time-zone shift, the same run length and instrument, and no different wells | high; medium when the length or instrument is unknown |
//! | `same_run_signature` | the same instrument serial, number of scans and run length to 2 ms | medium |
//! | `same_scan_series` | the same number of spectra, and the first, middle and last spectrum agree in MS level, retention time (to 0.011 s: converters print few digits) and stored total ion current (to 1e-5) — a conversion that records no start time (mzXML); not when both record different start times or different instruments | high |
//! | `same_sample_name` | the same sample name, where no id is recorded | medium |
//! | `same_stem` | the same file name stem in two different formats | medium |
//! | `sample_id_in_name` | one file's sample id is a whole token of the other's file name | medium (ids of 4+ characters with letters and digits), else low |
//!
//! Two data sets with only a well in common are never linked (wells repeat on every plate); a
//! sample id that is a well or tray position (`A1`, `R:A1`, `P2-D1`) or a generic word and a counter
//! (`ID 1`, `Sample_03`) and a source file name as generic as `data` are weak.
//! Identifiers shared by many data sets (more than `max_shared`) or that name controls (`blank`,
//! `QC`, `standard`, `wash`, …) count as low confidence: repeated injections of a pooled QC did
//! measure the same sample, but they are not what "the same sample" usually means. Links at or
//! above `min_confidence` form groups (connected components); a group whose members record two
//! different sample ids is flagged as a conflict and its confidence set to low.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use openreadout_core::Result;
use openreadout_core::experiment::looks_like_sample_id;
use serde::Serialize;

use crate::measure::{Item, Measure};
use crate::run::{BatchRequest, InputReport, run};
use crate::table::{Row, Value};

/// How sure a link is.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Circumstantial: a shared control name, a short id inside a file name.
    Low,
    /// Likely: the same sample name or file stem, a start time without a run length.
    Medium,
    /// Recorded identifiers agree (barcode, sample id, the conversion's source file, the same
    /// acquisition).
    High,
}

impl Confidence {
    /// Parse `low`, `medium`, `high`.
    pub fn parse(s: &str) -> Option<Confidence> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(Confidence::Low),
            "medium" => Some(Confidence::Medium),
            "high" => Some(Confidence::High),
            _ => None,
        }
    }
}

/// What to link.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LinkOptions {
    /// Inputs (directories are walked recursively by default).
    pub request: BatchRequest,
    /// Links below this do not join groups (they are still listed with `all_links`).
    pub min_confidence: Confidence,
    /// An identifier shared by more data sets than this is low confidence.
    pub max_shared: usize,
}

impl Default for LinkOptions {
    fn default() -> Self {
        LinkOptions {
            request: BatchRequest {
                recursive: true,
                ..BatchRequest::default()
            },
            min_confidence: Confidence::Medium,
            max_shared: 12,
        }
    }
}

/// One piece of evidence.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Evidence {
    /// Kind (`same_sample_id`, `same_acquisition`, …; see the module documentation).
    pub kind: String,
    /// The values compared, and where they came from.
    pub detail: String,
    /// Confidence of this evidence alone.
    pub confidence: Confidence,
}

/// A link between two data sets.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct Link {
    /// One data set.
    pub a: String,
    /// The other.
    pub b: String,
    /// The strongest evidence's confidence.
    pub confidence: Confidence,
    /// All evidence found.
    pub evidence: Vec<Evidence>,
}

/// A member of a group.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct LinkMember {
    /// The data set.
    pub path: String,
    /// Format id.
    pub format: Option<String>,
    /// Recorded sample id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_id: Option<String>,
    /// Recorded well.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub well: Option<String>,
    /// Recorded barcode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub barcode: Option<String>,
    /// Acquisition start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
}

/// Data sets that measured the same sample.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct LinkGroup {
    /// Group number (from 1, in the order of the first member).
    pub group: u32,
    /// The weakest link that holds the group together (low when the group has a conflict).
    pub confidence: Confidence,
    /// The sample id the members share, when they do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// Members in input order.
    pub members: Vec<LinkMember>,
    /// The links, with evidence.
    pub links: Vec<Link>,
    /// Disagreements inside the group (different sample ids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
}

/// An identifier shared by many data sets or naming a control.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SharedIdentifier {
    /// Field (`sample_id`, `sample_name`, `barcode`).
    pub field: String,
    /// Value.
    pub value: String,
    /// Data sets recording it.
    pub datasets: u64,
    /// Why it was treated as weak.
    pub reason: String,
}

/// Output of `link`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct LinkOutput {
    /// How the inputs became data sets.
    pub inputs: InputReport,
    /// Minimum confidence of links that form groups.
    pub min_confidence: Confidence,
    /// Groups of two or more data sets.
    pub groups: Vec<LinkGroup>,
    /// Data sets in no group.
    pub unlinked: u64,
    /// Some of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unlinked_examples: Vec<String>,
    /// Links below `min_confidence` (not grouped), strongest first (first 50).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weak_links: Vec<Link>,
    /// Identifiers treated as weak evidence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_identifiers: Vec<SharedIdentifier>,
    /// Things to know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// The headers `link` reads (a headers-only measure).
#[derive(Debug, Clone, Copy, Default)]
struct Probe;

fn source_files(info: &openreadout_core::model::FileInfo) -> Vec<String> {
    let mut out = Vec::new();
    let extras = info
        .spectra
        .iter()
        .map(|s| &s.extra)
        .chain(info.traces.iter().map(|t| &t.extra))
        .chain(info.tables.iter().map(|t| &t.extra))
        .chain(info.images.iter().map(|i| &i.extra));
    for x in extras {
        if let Some(list) = x.get("source_files").and_then(|v| v.as_array()) {
            for f in list {
                for k in ["name", "location"] {
                    if let Some(s) = f.get(k).and_then(|v| v.as_str()) {
                        out.push(s.to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

impl Measure for Probe {
    fn id(&self) -> &'static str {
        "link"
    }
    fn grain(&self) -> Vec<String> {
        Vec::new()
    }
    fn headers_only(&self) -> bool {
        true
    }
    fn fingerprint(&self) -> String {
        "link".into()
    }
    fn rows(&self, it: &mut Item<'_>) -> Result<Vec<Row>> {
        let e = openreadout_core::experiment::of_dataset(&*it.dataset, it.info);
        let ins = e.instrument.unwrap_or_default();
        let acq = e.acquisition.unwrap_or_default();
        let mut r = Row::new();
        r.set("instrument_model", Value::text_opt(ins.model.as_deref()));
        r.set("instrument_serial", Value::text_opt(ins.serial.as_deref()));
        r.set("duration_s", Value::float_opt(acq.duration_s));
        let scans: u64 = it.info.spectra.iter().map(|s| s.scan_count).sum();
        if scans > 0 {
            r.set("scans", scans);
        }
        let sf = source_files(it.info);
        if !sf.is_empty() {
            r.set("source_files", sf.join("\n"));
        }
        if let Some(fp) = scan_fingerprint(it) {
            r.set("scan_fingerprint", fp);
        }
        Ok(vec![r])
    }
}

/// The first, middle and last spectrum of the first run as `level:rt_s:tic` (tic empty when
/// not stored), `;`-separated: three spectra read, a few milliseconds per file.
fn scan_fingerprint(it: &mut Item<'_>) -> Option<String> {
    let n = it.info.spectra.first()?.scan_count;
    if n == 0 {
        return None;
    }
    let mut at = vec![0, n / 2, n - 1];
    at.dedup();
    let mut parts = Vec::with_capacity(at.len());
    for i in at {
        let sp = it.dataset.read_spectrum(0, i).ok()?;
        let rt = sp.rt_s.filter(|t| t.is_finite())?;
        parts.push(format!(
            "{}:{}:{}",
            sp.ms_level,
            rt,
            sp.total_ion_current
                .filter(|t| t.is_finite())
                .map(|t| t.to_string())
                .unwrap_or_default()
        ));
    }
    Some(parts.join(";"))
}

/// A parsed fingerprint entry: (MS level, retention time s, stored TIC).
type ScanPoint = (u32, f64, Option<f64>);

fn parse_fingerprint(s: &str) -> Vec<ScanPoint> {
    s.split(';')
        .filter_map(|p| {
            let mut it = p.split(':');
            let level = it.next()?.parse().ok()?;
            let rt = it.next()?.parse().ok()?;
            let tic = it.next().and_then(|t| t.parse().ok());
            Some((level, rt, tic))
        })
        .collect()
}

/// Do two fingerprints describe the same spectra? Retention times within 0.011 s (mzXML prints
/// `PT3601.98S`), stored TICs within 1e-5 relative; every point must carry a TIC on both sides.
fn same_scans(a: &[ScanPoint], b: &[ScanPoint]) -> bool {
    !a.is_empty()
        && a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.0 == y.0
                && (x.1 - y.1).abs() <= 0.011 + 1e-6 * x.1.abs()
                && match (x.2, y.2) {
                    (Some(p), Some(q)) => (p - q).abs() <= 1e-5 * p.abs().max(q.abs()).max(1.0),
                    _ => false,
                }
        })
}

const CONTROL_WORDS: &[&str] = &[
    "blank",
    "blanc",
    "qc",
    "pool",
    "pooled",
    "std",
    "standard",
    "wash",
    "solvent",
    "cal",
    "calibrant",
    "calibration",
    "sst",
    "control",
    "ctrl",
    "empty",
    "test",
    "sample",
];

fn is_control_name(v: &str) -> bool {
    let letters: String = v
        .to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphabetic)
        .collect();
    CONTROL_WORDS.contains(&letters.as_str())
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

fn tokens(s: &str) -> BTreeSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn stem(p: &str) -> String {
    let name = p.rsplit(['/', '\\']).next().unwrap_or(p);
    let mut s = name;
    // strip up to two extensions (.ome.tiff, .mzml.gz, .d)
    for _ in 0..2 {
        match s.rsplit_once('.') {
            Some((h, ext)) if !h.is_empty() && ext.len() <= 6 => s = h,
            _ => break,
        }
    }
    s.to_lowercase()
}

/// File and folder names too generic to identify an acquisition (`data`, `raw`, …).
const GENERIC_STEMS: &[&str] = &[
    "data", "sample", "samples", "test", "raw", "file", "files", "run", "runs", "export", "result",
    "results", "analysis", "acqdata", "untitled", "default", "image", "images",
];

/// A plate well or autosampler position (`A1`, `R:A1`, `P2-D1`, `1:B,3`), not a sample id.
fn looks_like_position(v: &str) -> bool {
    let alnum = v.chars().filter(char::is_ascii_alphanumeric).count();
    alnum <= 6
        && v.split([':', '-', '_', ',', ' ', '/'])
            .any(|t| crate::well::parse(t).is_some())
}

/// A generic word and a counter (`ID 1`, `Sample_03`, `run-12`, `Inj 2`): what acquisition
/// software fills in by default, repeated by every study, not a sample identity.
fn looks_like_counter(v: &str) -> bool {
    let n = norm(v);
    let word: String = n.chars().take_while(char::is_ascii_alphabetic).collect();
    let rest = n[word.len()..].trim_start_matches([' ', '_', '-', '#', '.']);
    matches!(
        word.as_str(),
        "id" | "sample" | "run" | "inj" | "injection" | "vial" | "file" | "test" | "no"
    ) && !rest.is_empty()
        && rest.len() <= 4
        && rest.bytes().all(|b| b.is_ascii_digit())
}

/// Does an ISO-8601 timestamp record fractions of a second (`…T09:49:02.553Z`)?
fn has_fraction(t: &str) -> bool {
    t.split_once('T').is_some_and(|(_, time)| {
        time.split(['Z', '+', '-'])
            .next()
            .is_some_and(|x| x.contains('.'))
    })
}

/// Seconds since the epoch, when the timestamp has a time of day.
fn epoch_s(t: &str) -> Option<f64> {
    openreadout_index::tables::micros_from_iso(t).map(|us| us as f64 / 1e6)
}

struct Node {
    path: String,
    format: Option<String>,
    id: Option<String>,
    name: Option<String>,
    well: Option<String>,
    barcode: Option<String>,
    started: Option<String>,
    duration: Option<f64>,
    scans: Option<u64>,
    model: Option<String>,
    serial: Option<String>,
    sources: Vec<String>,
    stem: String,
    fingerprint: Vec<ScanPoint>,
}

/// Union-find root of `x`.
fn find(p: &mut [usize], mut x: usize) -> usize {
    while p[x] != x {
        p[x] = p[p[x]];
        x = p[x];
    }
    x
}

fn add(ev: &mut Vec<Evidence>, kind: &str, detail: String, c: Confidence) {
    ev.push(Evidence {
        kind: kind.into(),
        detail,
        confidence: c,
    });
}

/// Find the links between the data sets of `opts.request` (see the module documentation).
pub fn link(reg: &openreadout_core::Registry, opts: &LinkOptions) -> Result<LinkOutput> {
    let res = run(reg, &Probe, &opts.request, None, None)?;
    let t = &res.table;
    let mut nodes: Vec<Node> = Vec::new();
    for (r, &di) in res.row_dataset.iter().enumerate() {
        let d = &res.datasets[di];
        if !t.get(r, "error").is_null() {
            continue;
        }
        let text = |n: &str| Some(t.get(r, n).text()).filter(|s| !s.is_empty());
        let path = d.path.display().to_string();
        nodes.push(Node {
            stem: stem(&path),
            path,
            format: d.format.clone(),
            id: d.keys.id.clone().filter(|v| looks_like_sample_id(v)),
            name: d.keys.name.clone().filter(|v| looks_like_sample_id(v)),
            well: d.keys.well.clone(),
            barcode: d.keys.barcode.clone().filter(|b| b.trim().len() >= 4),
            started: d.keys.started_at.clone(),
            duration: t.get(r, "duration_s").as_f64(),
            scans: t.get(r, "scans").as_f64().map(|x| x as u64),
            model: text("instrument_model"),
            serial: text("instrument_serial"),
            sources: text("source_files")
                .map(|s| s.lines().map(str::to_string).collect())
                .unwrap_or_default(),
            fingerprint: text("scan_fingerprint")
                .map(|s| parse_fingerprint(&s))
                .unwrap_or_default(),
        });
    }
    // how often each identifier occurs
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    for n in &nodes {
        for (f, v) in [
            ("sample_id", &n.id),
            ("sample_name", &n.name),
            ("barcode", &n.barcode),
        ] {
            if let Some(v) = v {
                *counts.entry((f.into(), norm(v))).or_default() += 1;
            }
        }
    }
    let mut shared: BTreeMap<(String, String), SharedIdentifier> = BTreeMap::new();
    let weak = |f: &str, v: &str, shared: &mut BTreeMap<(String, String), SharedIdentifier>| {
        let c = counts.get(&(f.to_string(), norm(v))).copied().unwrap_or(0);
        let reason = if is_control_name(v) {
            Some("names a control or blank")
        } else if GENERIC_STEMS.contains(&norm(v).as_str()) {
            Some("is a generic name (data, sample, test, …)")
        } else if looks_like_position(v) {
            Some("is a plate well or tray position, which every plate or tray repeats")
        } else if looks_like_counter(v) {
            Some("is a generic word and a counter (`ID 1`), as acquisition software fills in")
        } else if c > opts.max_shared {
            Some("shared by many data sets")
        } else {
            None
        };
        if let Some(r) = reason {
            shared
                .entry((f.to_string(), norm(v)))
                .or_insert_with(|| SharedIdentifier {
                    field: f.into(),
                    value: v.to_string(),
                    datasets: c as u64,
                    reason: r.into(),
                });
            true
        } else {
            false
        }
    };
    let mut links: Vec<(usize, usize, Link)> = Vec::new();
    for i in 0..nodes.len() {
        for j in i + 1..nodes.len() {
            let (a, b) = (&nodes[i], &nodes[j]);
            let mut ev = Vec::new();
            if let (Some(x), Some(y)) = (&a.barcode, &b.barcode)
                && norm(x) == norm(y)
            {
                if let (Some(wa), Some(wb)) = (&a.well, &b.well) {
                    let (pa, pb) = (crate::well::parse(wa), crate::well::parse(wb));
                    if pa.is_some() && pa == pb {
                        add(
                            &mut ev,
                            "same_plate_well",
                            format!("barcode `{x}`, well {wa}"),
                            Confidence::High,
                        );
                    }
                    // same plate, different wells: different samples; no link
                } else {
                    let c = if weak("barcode", x, &mut shared) {
                        Confidence::Low
                    } else {
                        Confidence::High
                    };
                    add(&mut ev, "same_barcode", format!("barcode `{x}`"), c);
                }
            }
            if let (Some(x), Some(y)) = (&a.id, &b.id)
                && norm(x) == norm(y)
            {
                let c = if weak("sample_id", x, &mut shared) {
                    Confidence::Low
                } else {
                    Confidence::High
                };
                add(&mut ev, "same_sample_id", format!("sample id `{x}`"), c);
            } else if let (Some(x), Some(y)) = (
                a.name.as_ref().or(a.id.as_ref()),
                b.name.as_ref().or(b.id.as_ref()),
            ) && norm(x) == norm(y)
                && !(a.id.is_some() && b.id.is_some())
            {
                let c = if weak("sample_name", x, &mut shared) {
                    Confidence::Low
                } else {
                    Confidence::Medium
                };
                add(&mut ev, "same_sample_name", format!("sample name `{x}`"), c);
            }
            // a conversion names its source
            for (conv, orig) in [(a, b), (b, a)] {
                for s in &conv.sources {
                    // any component of the recorded name or location (`…/13047CHQ_0001_A1.d/AcqData`)
                    let parts: Vec<String> = s
                        .split(['/', '\\'])
                        .filter(|c| !c.is_empty() && !c.ends_with(':'))
                        .map(stem)
                        .collect();
                    let id_hit = |st: &String| {
                        orig.id.as_ref().is_some_and(|id| {
                            !is_control_name(id)
                                && !GENERIC_STEMS.contains(&norm(id).as_str())
                                && (*st == norm(id) || stem(&norm(id)) == *st)
                        })
                    };
                    // the file name may match the other's stem or id; a folder only its id
                    let hit = parts.last().is_some_and(|st| {
                        st.len() >= 4 && !GENERIC_STEMS.contains(&st.as_str()) && *st == orig.stem
                    }) || parts.iter().any(|st| st.len() >= 3 && id_hit(st));
                    if hit {
                        add(
                            &mut ev,
                            "conversion_source",
                            format!(
                                "{} names its source file `{s}`",
                                conv.format.as_deref().unwrap_or("file")
                            ),
                            Confidence::High,
                        );
                        break;
                    }
                }
            }
            // the same acquisition
            let same_serial = match (&a.serial, &b.serial) {
                (Some(x), Some(y)) => Some(norm(x) == norm(y)),
                _ => None,
            };
            let same_ins = same_serial.or_else(|| match (&a.model, &b.model) {
                (Some(x), Some(y)) if norm(x) == norm(y) => Some(true),
                _ => None,
            });
            if let (Some(sa), Some(sb)) = (&a.started, &b.started)
                && let (Some(ta), Some(tb)) = (epoch_s(sa), epoch_s(sb))
            {
                let d = (ta - tb).abs();
                let hours = (d / 3600.0).round();
                // one instrument (same serial) cannot start two runs 2 s apart; conversions
                // often drop the fraction of a second
                // both with fractions of a second: equal to the millisecond (images of different
                // sites of one plate run can start within the same second); one rounded to the
                // second: within a second, two with the same instrument serial (converters drop
                // the fraction)
                let slack = if has_fraction(sa) && has_fraction(sb) {
                    0.0015
                } else if same_serial == Some(true) {
                    2.0
                } else {
                    1.0
                };
                let other_wells = match (&a.well, &b.well) {
                    (Some(x), Some(y)) => crate::well::parse(x) != crate::well::parse(y),
                    _ => false,
                };
                if hours <= 14.0 && (d - hours * 3600.0).abs() < slack && !other_wells {
                    let same_len = match (a.duration, b.duration) {
                        (Some(x), Some(y)) => Some((x - y).abs() <= (0.01 * x.abs()).max(1.0)),
                        _ => None,
                    };
                    if same_len != Some(false) && same_ins != Some(false) {
                        let c = if same_len == Some(true) && same_ins == Some(true) {
                            Confidence::High
                        } else {
                            Confidence::Medium
                        };
                        add(
                            &mut ev,
                            "same_acquisition",
                            format!(
                                "started {sa} and {sb}{}{}",
                                if hours > 0.0 {
                                    format!(" ({hours:.0} h apart: a time-zone difference)")
                                } else {
                                    String::new()
                                },
                                if same_len == Some(true) {
                                    ", same run length"
                                } else {
                                    ""
                                }
                            ),
                            c,
                        );
                    }
                }
            }
            // the same run by its signature: one instrument serial, the same number of scans and
            // the same run length to 2 ms (start times missing or rewritten by a converter).
            // Medium only: repeated injections of a fixed-cycle method can come close.
            if same_serial == Some(true)
                && let (Some(na), Some(nb)) = (a.scans, b.scans)
                && na == nb
                && na > 0
                && let (Some(x), Some(y)) = (a.duration, b.duration)
                && (x - y).abs() <= 0.002
                && !ev.iter().any(|e| e.kind == "same_acquisition")
            {
                add(
                    &mut ev,
                    "same_run_signature",
                    format!(
                        "instrument serial `{}`, {na} scans and {x:.3} s in both",
                        a.serial.as_deref().unwrap_or_default()
                    ),
                    Confidence::Medium,
                );
            }
            // the same spectra: a conversion without a start time (mzXML) of a raw file. Not when
            // both record start times that differ, or different instruments.
            let other_start = match (&a.started, &b.started) {
                (Some(x), Some(y)) => match (epoch_s(x), epoch_s(y)) {
                    (Some(tx), Some(ty)) => {
                        let d = (tx - ty).abs();
                        (d - (d / 3600.0).round() * 3600.0).abs() > 2.0
                    }
                    _ => false,
                },
                _ => false,
            };
            if let (Some(na), Some(nb)) = (a.scans, b.scans)
                && na == nb
                && !other_start
                && same_ins != Some(false)
                && same_scans(&a.fingerprint, &b.fingerprint)
            {
                let rts: Vec<String> = a
                    .fingerprint
                    .iter()
                    .map(|p| format!("{:.2} s", p.1))
                    .collect();
                add(
                    &mut ev,
                    "same_scan_series",
                    format!(
                        "{na} spectra in both; the spectra at {} agree in MS level, retention time and stored total ion current",
                        rts.join(", ")
                    ),
                    Confidence::High,
                );
            }
            if a.stem == b.stem && a.format != b.format && a.stem.len() >= 3 {
                add(
                    &mut ev,
                    "same_stem",
                    format!(
                        "file name `{}` in {} and {}",
                        a.stem,
                        a.format.as_deref().unwrap_or("?"),
                        b.format.as_deref().unwrap_or("?")
                    ),
                    Confidence::Medium,
                );
            }
            for (x, y) in [(a, b), (b, a)] {
                if let Some(id) = &x.id {
                    let idn = norm(id);
                    let ok = idn.len() >= 3
                        && tokens(&y.stem).contains(&idn)
                        && y.stem != idn
                        && !is_control_name(id);
                    if ok {
                        let strong = idn.len() >= 4
                            && idn.chars().any(|c| c.is_ascii_digit())
                            && idn.chars().any(|c| c.is_ascii_alphabetic());
                        add(
                            &mut ev,
                            "sample_id_in_name",
                            format!("sample id `{id}` is part of the file name `{}`", y.stem),
                            if strong {
                                Confidence::Medium
                            } else {
                                Confidence::Low
                            },
                        );
                    }
                }
            }
            if let Some(c) = ev.iter().map(|e| e.confidence).max() {
                links.push((
                    i,
                    j,
                    Link {
                        a: a.path.clone(),
                        b: b.path.clone(),
                        confidence: c,
                        evidence: ev,
                    },
                ));
            }
        }
    }
    // groups
    let mut parent: Vec<usize> = (0..nodes.len()).collect();
    for (i, j, l) in &links {
        if l.confidence >= opts.min_confidence {
            let (a, b) = (find(&mut parent, *i), find(&mut parent, *j));
            if a != b {
                parent[b.max(a)] = a.min(b);
            }
        }
    }
    let mut comps: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..nodes.len() {
        let r = find(&mut parent, i);
        comps.entry(r).or_default().push(i);
    }
    let mut groups = Vec::new();
    let mut unlinked = Vec::new();
    for members in comps.values() {
        if members.len() < 2 {
            unlinked.push(nodes[members[0]].path.clone());
            continue;
        }
        let set: BTreeSet<usize> = members.iter().copied().collect();
        let glinks: Vec<Link> = links
            .iter()
            .filter(|(i, j, l)| {
                set.contains(i) && set.contains(j) && l.confidence >= opts.min_confidence
            })
            .map(|(_, _, l)| l.clone())
            .collect();
        let ids: BTreeSet<String> = members
            .iter()
            .filter_map(|&m| nodes[m].id.as_deref().map(norm))
            .collect();
        let mut conflicts = Vec::new();
        if ids.len() > 1 {
            conflicts.push(format!(
                "members record different sample ids: {}",
                ids.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
        // the weakest link on the strongest spanning tree = the minimum over a max-spanning tree
        let mut conf = Confidence::High;
        {
            let mut p: Vec<usize> = (0..nodes.len()).collect();
            let mut sorted: Vec<&(usize, usize, Link)> = links
                .iter()
                .filter(|(i, j, l)| {
                    set.contains(i) && set.contains(j) && l.confidence >= opts.min_confidence
                })
                .collect();
            sorted.sort_by_key(|x| std::cmp::Reverse(x.2.confidence));
            for (i, j, l) in sorted {
                let (a, b) = (find(&mut p, *i), find(&mut p, *j));
                if a != b {
                    p[b] = a;
                    conf = conf.min(l.confidence);
                }
            }
        }
        if !conflicts.is_empty() {
            conf = Confidence::Low;
        }
        groups.push(LinkGroup {
            group: groups.len() as u32 + 1,
            confidence: conf,
            sample: (ids.len() == 1)
                .then(|| members.iter().find_map(|&m| nodes[m].id.clone()))
                .flatten(),
            members: members
                .iter()
                .map(|&m| {
                    let n = &nodes[m];
                    LinkMember {
                        path: n.path.clone(),
                        format: n.format.clone(),
                        sample_id: n.id.clone().or_else(|| n.name.clone()),
                        well: n.well.clone(),
                        barcode: n.barcode.clone(),
                        started_at: n.started.clone(),
                    }
                })
                .collect(),
            links: glinks,
            conflicts,
        });
    }
    let mut weak_links: Vec<Link> = links
        .iter()
        .filter(|(_, _, l)| l.confidence < opts.min_confidence)
        .map(|(_, _, l)| l.clone())
        .collect();
    weak_links.sort_by_key(|x| std::cmp::Reverse(x.confidence));
    weak_links.truncate(50);
    let mut notes = Vec::new();
    let failed = res.inputs.failed;
    if failed > 0 {
        notes.push(format!(
            "{failed} data set(s) could not be read and are not linked"
        ));
    }
    if nodes.len() > 2000 {
        notes.push(format!(
            "{} data sets compared pairwise; narrow the inputs for speed",
            nodes.len()
        ));
    }
    Ok(LinkOutput {
        inputs: res.inputs,
        min_confidence: opts.min_confidence,
        groups,
        unlinked: unlinked.len() as u64,
        unlinked_examples: unlinked.into_iter().take(20).collect(),
        weak_links,
        shared_identifiers: shared.into_values().collect(),
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_tokens_and_controls() {
        assert_eq!(stem("/a/B1_S2.ome.tiff"), "b1_s2");
        assert_eq!(stem("x/Run 7.d"), "run 7");
        assert_eq!(stem("C:\\data\\Blanc04.RAW"), "blanc04");
        assert!(tokens("plate1_S12_A01").contains("s12"));
        assert!(is_control_name("QC"));
        assert!(is_control_name("Blank_03"));
        assert!(!is_control_name("S-12"));
        assert!(has_fraction("2021-03-17T00:49:35.078Z"));
        assert!(looks_like_position("R:A1"));
        assert!(looks_like_position("P2-D1"));
        assert!(looks_like_position("A01"));
        assert!(!looks_like_position("13047CHQ_0001_A1"));
        assert!(!looks_like_position("KM1-17"));
        assert!(looks_like_counter("ID 1"));
        assert!(looks_like_counter("Sample_03"));
        assert!(!looks_like_counter("ID"));
        assert!(!looks_like_counter("KM1-17"));
        assert!(!looks_like_counter("13047CHQ_0001_A1"));
        assert!(!has_fraction("2021-03-17T00:49:35Z"));
        assert!(!has_fraction("2021-03-17T00:49:35+02:00"));
    }

    #[test]
    fn scan_fingerprints() {
        // a raw file and its mzXML: times printed with fewer digits, the same stored TICs
        let raw = parse_fingerprint(
            "1:0.45840000000000003:9187505;2:1107.3199000000002:1515763.875;2:3601.9826:313397.78125",
        );
        let mzxml =
            parse_fingerprint("1:0.4584:9187505;2:1107.32:1515763.875;2:3601.98:313397.78125");
        assert_eq!(raw.len(), 3);
        assert!(same_scans(&raw, &mzxml));
        // another injection: same scan count and times, other intensities
        let other =
            parse_fingerprint("1:0.4584:9187000;2:1107.32:1515763.875;2:3601.98:313397.78125");
        assert!(!same_scans(&raw, &other));
        // no TIC stored: no evidence
        let bare = parse_fingerprint("1:0.4584:;2:1107.32:;2:3601.98:");
        assert!(!same_scans(&bare, &bare));
        assert!(!same_scans(&[], &[]));
        // a spectrum 0.05 s off is another spectrum
        let late =
            parse_fingerprint("1:0.5084:9187505;2:1107.32:1515763.875;2:3601.98:313397.78125");
        assert!(!same_scans(&raw, &late));
    }
}
