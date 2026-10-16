//! Assurance profile (`docs/assurance.md`) of the ABIF reader: the variant features of a file
//! and the feature values the development corpus validates. The tables between the GENERATED
//! markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static ABIF: AssuranceProfile = AssuranceProfile {
    format_id: "abif",
    observe,
    validated: ABIF_VALIDATED,
    confidence: ABIF_CONFIDENCE,
    basis: Basis::VendorDocs,
};

/// What `info` shows of a file's variant: how many dye channels its traces hold. The reader adds
/// the rest (run kind, instrument, records, sizing).
fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        if a::extra_str(&t.extra, "kind") == Some("run_monitor") {
            continue;
        }
        let dyes = t
            .channels
            .iter()
            .filter(|c| c.extra.contains_key("dye_number"))
            .count();
        if dyes > 0 {
            o.feature(K::SampleLayout, format!("{dyes} dyes"), &[Scope::Traces]);
        }
    }
    o
}

// BEGIN GENERATED abif (cargo xtask assurance-audit --write; do not edit)
const ABIF_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const ABIF_VALIDATED: &[Validated] = &[
];
// END GENERATED abif
