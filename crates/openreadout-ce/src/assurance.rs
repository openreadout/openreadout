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

pub(crate) static BIOANALYZER: AssuranceProfile = AssuranceProfile {
    format_id: "agilent-bioanalyzer-xml",
    observe: observe_chip,
    validated: BIOANALYZER_VALIDATED,
    confidence: BIOANALYZER_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static TAPESTATION: AssuranceProfile = AssuranceProfile {
    format_id: "agilent-tapestation",
    observe: observe_chip,
    validated: TAPESTATION_VALIDATED,
    confidence: TAPESTATION_CONFIDENCE,
    basis: Basis::PriorArt,
};

/// What `info` shows of a chip or tape export: the size unit of its lanes.
fn observe_chip(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        if let Some(u) = t
            .extra
            .get("axis")
            .and_then(|a| a.get("unit"))
            .and_then(|u| u.as_str())
        {
            o.feature(K::SampleLayout, format!("sized in {u}"), &[Scope::Traces]);
        }
    }
    o
}

// BEGIN GENERATED agilent-bioanalyzer-xml (cargo xtask assurance-audit --write; do not edit)
const BIOANALYZER_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const BIOANALYZER_VALIDATED: &[Validated] = &[
];
// END GENERATED agilent-bioanalyzer-xml

// BEGIN GENERATED agilent-tapestation (cargo xtask assurance-audit --write; do not edit)
const TAPESTATION_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const TAPESTATION_VALIDATED: &[Validated] = &[
];
// END GENERATED agilent-tapestation
