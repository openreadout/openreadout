//! Assurance profiles (`docs/assurance.md`) of the NanoDrop and Qubit export readers: the export
//! layouts are the variant features. The tables between the GENERATED markers are written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
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
const NANODROP_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const NANODROP_VALIDATED: &[Validated] = &[
    a::row(K::Dialect, "nanodrop-8000", 5, 1, 5),
    a::row(K::Dialect, "nanodrop-eight", 4, 1, 4),
    a::row(K::Dialect, "nanodrop-one", 1, 1, 1),
    a::row(K::Dialect, "nanodrop-spectra", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 11),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 11),
];
// END GENERATED nanodrop

// BEGIN GENERATED qubit (cargo xtask assurance-audit --write; do not edit)
const QUBIT_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const QUBIT_VALIDATED: &[Validated] = &[
    a::row(K::Dialect, "qubit-4", 2, 1, 2),
    a::row(K::Dialect, "qubit-flex", 2, 1, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 4),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 4),
];
// END GENERATED qubit
