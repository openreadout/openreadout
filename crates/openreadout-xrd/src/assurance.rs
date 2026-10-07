//! Assurance profiles (`docs/assurance.md`) of the diffraction readers: the variant features of a
//! file and the feature values the development corpus validates. The tables between the
//! GENERATED markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static PANALYTICAL_XRDML: AssuranceProfile = AssuranceProfile {
    format_id: "panalytical-xrdml",
    observe,
    validated: PANALYTICAL_XRDML_VALIDATED,
    confidence: PANALYTICAL_XRDML_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static BRUKER_RAW: AssuranceProfile = AssuranceProfile {
    format_id: "bruker-raw",
    observe,
    validated: BRUKER_RAW_VALIDATED,
    confidence: BRUKER_RAW_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static BRUKER_BRML: AssuranceProfile = AssuranceProfile {
    format_id: "bruker-brml",
    observe,
    validated: BRUKER_BRML_VALIDATED,
    confidence: BRUKER_BRML_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static RIGAKU_RAS: AssuranceProfile = AssuranceProfile {
    format_id: "rigaku-ras",
    observe,
    validated: RIGAKU_RAS_VALIDATED,
    confidence: RIGAKU_RAS_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static RIGAKU_RASX: AssuranceProfile = AssuranceProfile {
    format_id: "rigaku-rasx",
    observe,
    validated: RIGAKU_RASX_VALIDATED,
    confidence: RIGAKU_RASX_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        if let Some(q) = t
            .extra
            .get("axis")
            .and_then(|x| x.get("quantity"))
            .and_then(|q| q.as_str())
        {
            o.feature(K::Record, format!("{q} axis"), &[Scope::Traces]);
        }
        if let Some(u) = t
            .channels
            .iter()
            .find(|c| c.name == "intensity")
            .and_then(|c| c.unit.as_deref())
        {
            o.feature(
                K::SampleLayout,
                format!("intensity in {u}"),
                &[Scope::Traces],
            );
        }
    }
    let _ = a::extra_str;
    o
}

// BEGIN GENERATED panalytical-xrdml (cargo xtask assurance-audit --write; do not edit)
const PANALYTICAL_XRDML_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const PANALYTICAL_XRDML_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "measurement Repeated scan", 2, 2, 2),
    a::row(K::Acquisition, "measurement Scan", 5, 5, 5),
    a::row(K::Field, "experiment.acquisition.started_at", 7, 7, 7),
    a::row(K::Field, "experiment.instrument.model", 7, 7, 7),
    a::row(K::FormatVersion, "1.3", 1, 1, 1),
    a::row(K::FormatVersion, "1.5", 2, 2, 2),
    a::row(K::FormatVersion, "1.6", 2, 2, 2),
    a::row(K::FormatVersion, "2.1", 1, 1, 1),
    a::row(K::FormatVersion, "2.3", 1, 1, 1),
    a::row(K::Layout, "2Theta-Omega scan", 1, 1, 1),
    a::row(K::Layout, "Gonio scan", 4, 4, 6),
    a::row(K::Record, "two_theta axis", 5, 5, 7),
    a::row(K::SampleLayout, "intensity in counts", 5, 5, 7),
    a::row(K::WriterVersion, "Data Collector 4.1", 1, 1, 1),
    a::row(K::WriterVersion, "Data Collector 4.4a", 1, 1, 1),
    a::row(K::WriterVersion, "Data Collector 5.4", 2, 2, 2),
    a::row(K::WriterVersion, "Data Collector 6.1b", 1, 1, 1),
    a::row(K::WriterVersion, "Data Collector 7.5b", 1, 1, 1),
    a::row(K::WriterVersion, "X'Pert Data Collector 2.2f", 1, 1, 1),
];
// END GENERATED panalytical-xrdml

// BEGIN GENERATED bruker-raw (cargo xtask assurance-audit --write; do not edit)
const BRUKER_RAW_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const BRUKER_RAW_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 11),
    a::row(K::FormatVersion, "RAW1.01", 4, 4, 4),
    a::row(K::FormatVersion, "RAW4.00", 7, 7, 7),
    a::row(K::Layout, "2Theta scan", 11, 11, 11),
    a::row(K::Record, "two_theta axis", 11, 11, 11),
    a::row(K::SampleLayout, "8-byte records", 1, 1, 1),
    a::row(K::SampleLayout, "intensity in counts", 11, 11, 11),
];
// END GENERATED bruker-raw

// BEGIN GENERATED bruker-brml (cargo xtask assurance-audit --write; do not edit)
const BRUKER_BRML_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const BRUKER_BRML_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 4, 4, 4),
    a::row(K::Field, "experiment.instrument.model", 3, 3, 3),
    a::row(K::Layout, "2Theta scan (Measured)", 4, 4, 4),
    a::row(K::Record, "absorber factors", 1, 1, 1),
    a::row(K::Record, "two_theta axis", 4, 4, 4),
    a::row(K::SampleLayout, "intensity in counts", 4, 4, 4),
    a::row(K::WriterVersion, "DIFFRAC 6.5.0.0", 1, 1, 1),
    a::row(K::WriterVersion, "DIFFRAC 8.6.1.0", 1, 1, 1),
    a::row(K::WriterVersion, "DIFFRAC 8.6.3.0", 1, 1, 1),
    a::row(K::WriterVersion, "DIFFRAC 8.7.3.0", 1, 1, 1),
];
// END GENERATED bruker-brml

// BEGIN GENERATED rigaku-ras (cargo xtask assurance-audit --write; do not edit)
const RIGAKU_RAS_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const RIGAKU_RAS_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 4, 4, 5),
    a::row(K::Field, "experiment.instrument.model", 1, 1, 2),
    a::row(K::FormatVersion, "RAS 1", 3, 3, 3),
    a::row(K::FormatVersion, "RAS 1.0000000000", 2, 2, 2),
    a::row(K::Layout, "2θ/θ scan", 1, 1, 1),
    a::row(K::Layout, "Theta/2-Theta scan", 1, 1, 1),
    a::row(K::Layout, "TwoThetaTheta scan", 3, 3, 3),
    a::row(K::Record, "angle axis", 1, 1, 1),
    a::row(K::Record, "two_theta axis", 4, 4, 4),
    a::row(K::SampleLayout, "intensity in counts", 5, 5, 5),
];
// END GENERATED rigaku-ras

// BEGIN GENERATED rigaku-rasx (cargo xtask assurance-audit --write; do not edit)
const RIGAKU_RASX_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const RIGAKU_RASX_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 2, 2, 8),
    a::row(K::Field, "experiment.instrument.model", 2, 2, 8),
    a::row(K::Layout, "TwoTheta scan", 2, 1, 2),
    a::row(K::Layout, "TwoThetaOmega scan", 1, 1, 1),
    a::row(K::Layout, "TwoThetaTheta scan", 5, 5, 5),
    a::row(K::Record, "two_theta axis", 8, 5, 8),
    a::row(K::SampleLayout, "intensity in counts", 8, 5, 8),
];
// END GENERATED rigaku-rasx
