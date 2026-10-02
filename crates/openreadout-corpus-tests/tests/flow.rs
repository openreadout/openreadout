//! Flow analysis against FlowKit/flowutils ground truth (`oracle/flow.py` →
//! `corpus/oracle/flow/*.json`):
//!
//! - gating: every population count of every paired FCS file must equal FlowKit's exactly; the
//!   counts FlowJo stored in the workspace are compared and reported (not asserted: FlowJo
//!   differs from FlowKit by a few events on small populations);
//! - compensation and transforms: compensated values of the recorded rows, per-detector sums of
//!   all events, and every transform of the compensated values must agree within
//!   `|ours − theirs| <= TOLERANCE × max(|theirs|, 1)`.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test flow -- --nocapture`
//! (`OPENREADOUT_CORPUS_DIR` overrides `corpus/files`).
#![cfg(feature = "corpus")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_corpus_tests::oracle_json;
use openreadout_fcs::analysis::{
    CompensationChoice, GateRequest, TableOptions, gate, parse_transform_spec, processed_rows,
};
use serde::Deserialize;

/// Relative tolerance (absolute below 1) for compensated and transformed values.
const TOLERANCE: f64 = 1e-9;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    filename: String,
}

#[derive(Deserialize)]
struct GatingOracle {
    gating: String,
    samples: Vec<GatingSample>,
}
#[derive(Deserialize)]
struct GatingSample {
    fcs: String,
    sample: Option<String>,
    event_count: u64,
    populations: Vec<OraclePopulation>,
}
#[derive(Deserialize)]
struct OraclePopulation {
    path: String,
    count: u64,
    #[serde(default)]
    flowjo_count: Option<u64>,
}

#[derive(Deserialize)]
struct CompOracle {
    fcs: String,
    compensation: CompSource,
    event_count: u64,
    rows: Vec<usize>,
    detectors: Vec<String>,
    compensated: BTreeMap<String, Vec<Option<f64>>>,
    column_sums: BTreeMap<String, Option<f64>>,
    transforms: Vec<OracleTransform>,
}
#[derive(Deserialize)]
struct CompSource {
    source: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    sample: Option<String>,
}
#[derive(Deserialize)]
struct OracleTransform {
    spec: String,
    values: BTreeMap<String, Vec<Option<f64>>>,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn filenames() -> BTreeMap<String, String> {
    let m: Manifest =
        toml::from_str(&std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap())
            .unwrap();
    m.file.into_iter().map(|e| (e.id, e.filename)).collect()
}

fn oracles(suffix_comp: bool) -> Vec<PathBuf> {
    // `<name>.json`, or `<name>.json.gz` over 1 MiB (logical `.json` paths, sorted)
    oracle_json::files(&root().join("corpus/oracle/flow"), ".json")
        .into_iter()
        .filter(|p| {
            let n = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            n.ends_with(".comp.json") == suffix_comp
                // Event-filter counts (fcs_filter.rs) share the directory but are not gating oracles.
                && n != "filter-counts.json"
        })
        .collect()
}

/// Error measure: absolute below 1, relative above.
fn err(ours: f64, theirs: Option<f64>) -> f64 {
    match theirs {
        None => {
            if ours.is_finite() {
                f64::INFINITY
            } else {
                0.0
            }
        }
        Some(t) => (ours - t).abs() / t.abs().max(1.0),
    }
}

#[test]
fn population_counts_match_flowkit() {
    let names = filenames();
    let dir = files_dir();
    let mut checked = 0;
    let mut failures = Vec::new();
    let (mut fj_total, mut fj_equal, mut fj_max_abs) = (0usize, 0usize, 0u64);
    for p in oracles(false) {
        let o: GatingOracle =
            serde_json::from_str(&oracle_json::read_to_string(&p).unwrap()).unwrap();
        let gating = dir.join(&names[&o.gating]);
        if !gating.exists() {
            continue;
        }
        for s in &o.samples {
            let fcs = dir.join(&names[&s.fcs]);
            if !fcs.exists() {
                continue;
            }
            let mut req = GateRequest::new(&gating, Some(fcs));
            req.sample = s.sample.clone();
            let out = match gate(&req) {
                Ok(o) => o,
                Err(e) => {
                    failures.push(format!("{} / {}: {e}", o.gating, s.fcs));
                    continue;
                }
            };
            let ours: BTreeMap<&str, Option<u64>> = out
                .populations
                .iter()
                .map(|r| (r.path.as_str(), r.count))
                .collect();
            let mut bad = Vec::new();
            for pop in &s.populations {
                match ours.get(pop.path.as_str()) {
                    Some(Some(c)) if *c == pop.count => {}
                    other => bad.push(format!(
                        "{}: ours {other:?}, FlowKit {}",
                        pop.path, pop.count
                    )),
                }
                if let (Some(fj), Some(Some(c))) = (pop.flowjo_count, ours.get(pop.path.as_str())) {
                    fj_total += 1;
                    if fj == *c {
                        fj_equal += 1;
                    }
                    fj_max_abs = fj_max_abs.max(fj.abs_diff(*c));
                }
            }
            if out.event_count != Some(s.event_count) {
                bad.push(format!(
                    "event count ours {:?}, FlowKit {}",
                    out.event_count, s.event_count
                ));
            }
            println!(
                "{:<6} {:<22} {:<16} {} populations",
                if bad.is_empty() { "pass" } else { "FAIL" },
                o.gating,
                s.fcs,
                s.populations.len()
            );
            checked += s.populations.len();
            if !bad.is_empty() {
                failures.push(format!(
                    "{} / {}:\n    {}",
                    o.gating,
                    s.fcs,
                    bad.join("\n    ")
                ));
            }
        }
    }
    println!(
        "{checked} population counts equal to FlowKit; FlowJo stored counts: {fj_equal}/{fj_total} equal, largest difference {fj_max_abs} events"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn compensation_and_transforms_match_flowkit() {
    let names = filenames();
    let dir = files_dir();
    let mut failures = Vec::new();
    let (mut worst_comp, mut worst_sum, mut worst_xf) = (0.0f64, 0.0f64, 0.0f64);
    let mut compared = 0usize;
    for p in oracles(true) {
        let o: CompOracle =
            serde_json::from_str(&oracle_json::read_to_string(&p).unwrap()).unwrap();
        let fcs = dir.join(&names[&o.fcs]);
        if !fcs.exists() {
            continue;
        }
        let mut opts = TableOptions::default();
        if o.compensation.source == "fcs" {
            opts.compensation = Some(CompensationChoice::File);
        } else {
            let w = o.compensation.workspace.as_deref().unwrap_or_default();
            opts.gating_file = Some(dir.join(&names[w]));
            opts.sample = o.compensation.sample.clone();
            opts.compensation = Some(CompensationChoice::GatingFile);
        }
        let rows = match processed_rows(&fcs, 0, 0, o.event_count, &opts) {
            Ok(r) => r,
            Err(e) => {
                failures.push(format!("{}: {e}", o.fcs));
                continue;
            }
        };
        let mut file_worst = (0.0f64, 0.0f64, 0.0f64);
        for d in &o.detectors {
            let Some(c) = rows.names.iter().position(|n| n == d) else {
                failures.push(format!("{}: no column {d}", o.fcs));
                continue;
            };
            let col = &rows.columns[c];
            for (k, &r) in o.rows.iter().enumerate() {
                let e = err(col[r], o.compensated[d][k]);
                file_worst.0 = file_worst.0.max(e);
                compared += 1;
            }
            let sum: f64 = col.iter().sum();
            file_worst.1 = file_worst
                .1
                .max(err(sum, o.column_sums[d]) / (o.event_count as f64).max(1.0));
            for t in &o.transforms {
                let Some(theirs) = t.values.get(d) else {
                    continue;
                };
                let prepared = parse_transform_spec(&t.spec).unwrap().prepare().unwrap();
                for (k, &r) in o.rows.iter().enumerate() {
                    let e = err(prepared.apply(col[r]), theirs[k]);
                    if e > TOLERANCE && failures.len() < 40 {
                        failures.push(format!(
                            "{} {d} {} row {r}: ours {} FlowKit {:?}",
                            o.fcs,
                            t.spec,
                            prepared.apply(col[r]),
                            theirs[k]
                        ));
                    }
                    file_worst.2 = file_worst.2.max(e);
                    compared += 1;
                }
            }
        }
        let ok =
            file_worst.0 <= TOLERANCE && file_worst.1 <= TOLERANCE && file_worst.2 <= TOLERANCE;
        println!(
            "{:<6} {:<30} compensated {:.1e}  sums {:.1e}  transforms {:.1e}",
            if ok { "pass" } else { "FAIL" },
            o.fcs,
            file_worst.0,
            file_worst.1,
            file_worst.2
        );
        if !ok {
            failures.push(format!("{}: worst errors {file_worst:?}", o.fcs));
        }
        worst_comp = worst_comp.max(file_worst.0);
        worst_sum = worst_sum.max(file_worst.1);
        worst_xf = worst_xf.max(file_worst.2);
    }
    println!(
        "{compared} values compared; worst error: compensated {worst_comp:.2e}, column sums {worst_sum:.2e} (per event), transforms {worst_xf:.2e} (tolerance {TOLERANCE:.0e})"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
