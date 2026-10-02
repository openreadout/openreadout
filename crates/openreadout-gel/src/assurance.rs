//! Assurance profile (`docs/assurance.md`) of the Bio-Rad Image Lab `.scn` reader: the variant
//! features of a scan file and the feature values the development corpus validates. The table
//! between the GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static BIORAD_SCN: AssuranceProfile = AssuranceProfile {
    format_id: "biorad-scn",
    observe,
    validated: BIORAD_SCN_VALIDATED,
    confidence: BIORAD_SCN_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = info
        .format_version
        .as_deref()
        .and_then(|v| a::version_prefix(v, 2))
    {
        o.feature(
            K::WriterVersion,
            format!("Image Lab {v}"),
            &[Scope::Metadata, Scope::Pixels],
        );
    }
    for im in &info.images {
        o.feature(K::SampleLayout, im.pixel_type.ome_name(), &[Scope::Pixels]);
        for c in &im.channels {
            if let Some(n) = &c.name {
                o.context(K::Acquisition, n);
            }
        }
    }
    if let Some(n) = a::note_with(info, "imported from a Molecular Dynamics .gel scan") {
        o.calibration(
            "Molecular Dynamics square-root encoding",
            CalibrationStatus::NotApplied,
            &[Scope::Pixels],
            n.to_string(),
        );
    }
    if let Some(n) = a::note_with(info, "the file is damaged (") {
        o.undecoded("damaged parts", &[Scope::Pixels], n.to_string());
    }
    o
}

// BEGIN GENERATED biorad-scn (cargo xtask assurance-audit --write; do not edit)
const BIORAD_SCN_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const BIORAD_SCN_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "Chemi", 1, 1, 1),
    a::row(K::Acquisition, "Chemi+Marker", 1, 1, 1),
    a::row(K::Acquisition, "Chemiluminescence", 5, 5, 5),
    a::row(K::Acquisition, "Colorimetric", 1, 1, 1),
    a::row(K::Acquisition, "Coomassie Blue", 1, 1, 1),
    a::row(K::Acquisition, "Ethidium Bromide", 1, 1, 1),
    a::row(K::Acquisition, "IRDye 800CW", 1, 1, 1),
    a::row(K::Acquisition, "signal", 2, 2, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 12, 10, 12),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 12),
    a::row(K::SampleLayout, "uint16", 13, 11, 13),
    a::row(K::WriterVersion, "Image Lab 3.0", 4, 2, 4),
    a::row(K::WriterVersion, "Image Lab 4.1", 1, 1, 1),
    a::row(K::WriterVersion, "Image Lab 5.2", 3, 3, 3),
    a::row(K::WriterVersion, "Image Lab 6.0", 4, 4, 4),
    a::row(K::WriterVersion, "Image Lab 6.1", 1, 1, 1),
];
// END GENERATED biorad-scn
