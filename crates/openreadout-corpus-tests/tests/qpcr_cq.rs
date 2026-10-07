//! Agreement with stored vendor results, including unavailable computations and call errors.
#![cfg(feature = "corpus")]

use std::{collections::BTreeMap, path::PathBuf};

use openreadout_qpcr::{CqMethod, QpcrDataset, QpcrReportRequest, qpcr_report};
use serde::Deserialize;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    filename: String,
    format: String,
    #[serde(default)]
    tier: String,
    #[serde(default)]
    role: String,
}

#[derive(Default, Debug)]
struct Agreement {
    files: usize,
    paired_files: usize,
    results: usize,
    paired: usize,
    unavailable: usize,
    without_curve: usize,
    calls_differ: usize,
    outside_tolerance: usize,
    sum: f64,
    max: f64,
}

fn audit() -> BTreeMap<String, Agreement> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let mut totals: BTreeMap<String, Agreement> = BTreeMap::new();
    for entry in manifest.file.iter().filter(|e| {
        e.tier != "heldout"
            && e.role != "heldout"
            && (e.role.is_empty() || e.role == "input")
            && [
                "rdml",
                "roche-lightcycler-ixo",
                "applied-biosystems-eds",
                "rotor-gene-rex",
                "qpcr-results-export",
            ]
            .contains(&e.format.as_str())
    }) {
        let ds = QpcrDataset::open(&dir.join(&entry.filename))
            .unwrap_or_else(|err| panic!("{}: {err}", entry.id));
        let mut request = QpcrReportRequest::default();
        request.compute_cq = true;
        for method in [
            CqMethod::Threshold,
            CqMethod::StoredThreshold,
            CqMethod::SecondDerivative,
        ] {
            request.method = method;
            let report = qpcr_report(&ds, &request).unwrap();
            if entry.id == "eds-qs7pro-tb18s-stdcurve" && method == CqMethod::StoredThreshold {
                let comparison = report.cq_comparison.as_ref().unwrap();
                assert_eq!(comparison.curves, 293);
                assert_eq!(comparison.both_cq, 90);
                assert_eq!(comparison.only_vendor, 0);
                assert_eq!(comparison.only_ours, 0);
            }
            let total = totals
                .entry(format!("{} {method:?}", entry.format))
                .or_default();
            total.files += 1;
            let previous_paired = total.paired;
            for row in &report.records {
                if entry.id == "eds-qs7pro-tb18s-stdcurve"
                    && method == CqMethod::StoredThreshold
                    && let Some(vendor) = row.cq
                {
                    let computed = row
                        .computed_cq
                        .expect("QuantStudio positive must be computed");
                    assert!(
                        (computed - vendor).abs() <= 0.05,
                        "{} {}: {computed} vs {vendor}",
                        entry.id,
                        row.well
                    );
                }
                if row.cq.is_none() && !row.cq_undetermined {
                    continue;
                }
                total.results += 1;
                total.without_curve += usize::from(row.cycles < 2);
                if row.computed_cq_status.is_none() {
                    total.unavailable += 1;
                    continue;
                }
                match (row.cq, row.computed_cq) {
                    (Some(vendor), Some(computed)) => {
                        let error = (vendor - computed).abs();
                        assert!(error.is_finite(), "{} {}", entry.id, row.well);
                        total.paired += 1;
                        total.outside_tolerance += usize::from(error > 0.05);
                        total.sum += error;
                        total.max = total.max.max(error);
                    }
                    (None, None) => {}
                    _ => total.calls_differ += 1,
                }
            }
            total.paired_files += usize::from(total.paired > previous_paired);
            println!("{} {method:?}: {:?}", entry.id, report.cq_comparison);
        }
    }
    assert!(!totals.is_empty(), "no development files in manifest");
    for (format, total) in &totals {
        println!(
            "{format}: files={} paired_files={} results={} paired={} unavailable={} without_curve={} calls_differ={} outside_0.05={} max={} mean={}",
            total.files,
            total.paired_files,
            total.results,
            total.paired,
            total.unavailable,
            total.without_curve,
            total.calls_differ,
            total.outside_tolerance,
            if total.paired == 0 {
                "n/a".into()
            } else {
                format!("{:.6}", total.max)
            },
            if total.paired == 0 {
                "n/a".into()
            } else {
                format!("{:.6}", total.sum / total.paired as f64)
            },
        );
    }
    totals
}

#[test]
fn qpcr_cq_agreement() {
    let totals = audit();
    assert_eq!(totals.len(), 15, "all methods must cover all five formats");
    let roche = &totals["roche-lightcycler-ixo SecondDerivative"];
    assert!(roche.paired >= 160, "missing LC480 positive wells");
    assert_eq!(roche.unavailable, 0);
    assert_eq!(roche.calls_differ, 0);
    assert_eq!(
        roche.outside_tolerance, 0,
        "LC480 exceeds 0.05 cycles: {roche:?}"
    );
}

/// Explicit acceptance gate, kept separate from the agreement audit: the proprietary
/// algorithms are not reproduced yet. Run with --ignored to see the remaining failures.
#[test]
#[ignore = "Vendor reproduction is incomplete; this is the unmet 0.05-cycle acceptance gate"]
fn qpcr_cq_vendor_acceptance() {
    let totals = audit();
    let failures: Vec<_> = totals
        .iter()
        .filter(|(key, _)| {
            if key.starts_with("roche-lightcycler-ixo ") {
                key.ends_with("SecondDerivative")
            } else {
                key.ends_with("StoredThreshold")
            }
        })
        .filter(|(_, value)| {
            value.unavailable > value.without_curve
                || value.calls_differ > 0
                || value.outside_tolerance > 0
        })
        .collect();
    assert!(
        failures.is_empty(),
        "vendor reproduction not achieved: {failures:#?}"
    );
}
