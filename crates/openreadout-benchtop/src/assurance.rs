//! Assurance profiles (`docs/assurance.md`) of the NanoDrop, Qubit and ddPCR export readers: the export
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

pub(crate) static DDPCR: AssuranceProfile = AssuranceProfile {
    format_id: "biorad-ddpcr",
    observe,
    validated: DDPCR_VALIDATED,
    confidence: DDPCR_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static QLP: AssuranceProfile = AssuranceProfile {
    format_id: "biorad-qlp",
    observe: observe_qlp,
    validated: QLP_VALIDATED,
    confidence: QLP_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

/// A `.qlp`: the QuantaSoft version that wrote it (major.minor) decides the layout.
fn observe_qlp(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        let short: String = v.split('.').take(2).collect::<Vec<_>>().join(".");
        o.feature(K::WriterVersion, short, &[Scope::Metadata, Scope::Tables]);
    }
    o.undecoded(
        "per-droplet third value and flags",
        &[],
        "each droplet record's third value per channel and the 8 flag bytes per droplet are not read",
    );
    o
}

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

// BEGIN GENERATED biorad-ddpcr (cargo xtask assurance-audit --write; do not edit)
const DDPCR_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const DDPCR_VALIDATED: &[Validated] = &[
];
// END GENERATED biorad-ddpcr

// BEGIN GENERATED biorad-qlp (cargo xtask assurance-audit --write; do not edit)
const QLP_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const QLP_VALIDATED: &[Validated] = &[
];
// END GENERATED biorad-qlp
