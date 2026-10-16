//! Assurance profiles (`docs/assurance.md`) of the NanoDrop and Qubit export readers: the export
//! layouts are the variant features. The tables between the GENERATED markers are written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
#[allow(unused_imports)] // `a::row` appears once the generated tables have rows
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static NANODROP: AssuranceProfile = AssuranceProfile {
    format_id: "nanodrop",
    observe,
    validated: NANODROP_VALIDATED,
    confidence: NANODROP_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static QUBIT: AssuranceProfile = AssuranceProfile {
    format_id: "qubit",
    observe,
    validated: QUBIT_VALIDATED,
    confidence: QUBIT_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::Dialect,
            v,
            &[Scope::Metadata, Scope::Tables, Scope::Traces],
        );
    }
    o
}

// BEGIN GENERATED nanodrop (cargo xtask assurance-audit --write; do not edit)
const NANODROP_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const NANODROP_VALIDATED: &[Validated] = &[];
// END GENERATED nanodrop

// BEGIN GENERATED qubit (cargo xtask assurance-audit --write; do not edit)
const QUBIT_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const QUBIT_VALIDATED: &[Validated] = &[];
// END GENERATED qubit
