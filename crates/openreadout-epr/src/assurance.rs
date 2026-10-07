//! Assurance profiles (`docs/assurance.md`) of the EPR readers: the variant features of a data set
//! and the feature values the development corpus validates. The tables between the GENERATED
//! markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static BRUKER_BES3T: AssuranceProfile = AssuranceProfile {
    format_id: "bruker-bes3t",
    observe,
    validated: BRUKER_BES3T_VALIDATED,
    confidence: BRUKER_BES3T_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static BRUKER_ESP: AssuranceProfile = AssuranceProfile {
    format_id: "bruker-esp",
    observe,
    validated: BRUKER_ESP_VALIDATED,
    confidence: BRUKER_ESP_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    for t in &info.traces {
        if let Some(q) = t
            .extra
            .get("axis")
            .and_then(|a| a.get("quantity"))
            .and_then(|q| q.as_str())
        {
            let irregular = t
                .extra
                .get("axis")
                .and_then(|a| a.get("irregular"))
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            o.feature(
                K::Layout,
                format!("{q} axis{}", if irregular { " (axis file)" } else { "" }),
                &[Scope::Traces],
            );
        }
        if let Some(e) = a::extra_str(&t.extra, "experiment") {
            o.feature(K::Acquisition, e, &[]);
        }
        let dims = if t.sweep_count > 1 { "2D" } else { "1D" };
        let values = if t.channels.iter().any(|c| c.name == "imaginary") {
            "complex"
        } else {
            "real"
        };
        o.feature(K::Record, format!("{dims} {values}"), &[Scope::Traces]);
    }
    o
}

// BEGIN GENERATED bruker-bes3t (cargo xtask assurance-audit --write; do not edit)
const BRUKER_BES3T_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const BRUKER_BES3T_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "cw", 9, 8, 11),
    a::row(K::Acquisition, "cwimg", 1, 1, 1),
    a::row(K::Acquisition, "pls", 3, 3, 3),
    a::row(K::Field, "experiment.acquisition.started_at", 12, 10, 14),
    a::row(K::FormatVersion, "BES3T 1.2", 14, 11, 16),
    a::row(K::Layout, "magnetic_field axis", 9, 8, 11),
    a::row(K::Layout, "time axis", 4, 3, 4),
    a::row(K::Layout, "x axis", 1, 1, 1),
    a::row(K::Record, "1D complex", 2, 1, 2),
    a::row(K::Record, "1D real", 8, 8, 9),
    a::row(K::Record, "2D complex", 1, 1, 1),
    a::row(K::Record, "2D real", 3, 3, 4),
    a::row(K::SampleLayout, "big-endian float64", 11, 9, 13),
    a::row(K::SampleLayout, "big-endian float64 complex", 3, 2, 3),
];
// END GENERATED bruker-bes3t

// BEGIN GENERATED bruker-esp (cargo xtask assurance-audit --write; do not edit)
const BRUKER_ESP_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const BRUKER_ESP_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "field-sweep", 15, 5, 15),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 13),
    a::row(K::FormatVersion, "ESP", 5, 2, 5),
    a::row(K::FormatVersion, "WinEPR", 10, 5, 10),
    a::row(K::Layout, "magnetic_field axis", 15, 5, 15),
    a::row(K::Record, "1D real", 12, 4, 12),
    a::row(K::Record, "2D real", 3, 3, 3),
    a::row(K::Writer, "ESP cw", 5, 2, 5),
    a::row(K::Writer, "WinEPR", 10, 5, 10),
];
// END GENERATED bruker-esp
