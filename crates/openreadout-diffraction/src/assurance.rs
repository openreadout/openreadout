//! Assurance profiles (`docs/assurance.md`) of the diffraction readers: the variant features of
//! a CBF or SMV file and the feature values the development corpus validates. The tables
//! between the GENERATED markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static CBF: AssuranceProfile = AssuranceProfile {
    format_id: "cbf",
    observe: observe_cbf,
    validated: CBF_VALIDATED,
    confidence: CBF_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static SMV: AssuranceProfile = AssuranceProfile {
    format_id: "smv",
    observe: observe_smv,
    validated: SMV_VALIDATED,
    confidence: SMV_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe_cbf(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    a::instrument_context(&mut o, info);
    for im in &info.images {
        let cbf = im.extra.get("cbf");
        if let Some(c) = cbf.and_then(|v| v.get("compression")).and_then(|v| v.as_str()) {
            o.feature(K::Codec, c, &[Scope::Pixels]);
        }
        if let Some(c) = cbf
            .and_then(|v| v.get("header_convention"))
            .and_then(|v| v.as_str())
        {
            o.feature(K::Dialect, c, &[Scope::Metadata]);
        }
    }
    if info.images.len() > 1 {
        o.feature(K::Layout, "several binary sections", &[Scope::Pixels]);
    }
    o
}

fn observe_smv(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    a::instrument_context(&mut o, info);
    for im in &info.images {
        if let Some(c) = im
            .extra
            .get("smv")
            .and_then(|v| v.get("beam_center_convention"))
            .and_then(|v| v.as_str())
        {
            o.feature(K::Dialect, format!("beam centre {c}"), &[Scope::Metadata]);
        }
    }
    o
}

// BEGIN GENERATED cbf (cargo xtask assurance-audit --write; do not edit)
const CBF_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const CBF_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "byte_offset", 4, 3, 4),
    a::row(K::Dialect, "PILATUS_1.2", 4, 3, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 4),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 4),
    a::row(K::FormatVersion, "1.5", 4, 3, 4),
    a::row(K::Instrument, "PILATUS 2M", 1, 1, 1),
    a::row(K::Instrument, "PILATUS 6M Prosport+", 1, 1, 1),
    a::row(K::Instrument, "PILATUS 6M-F", 2, 1, 2),
    a::row(K::SampleLayout, "int32", 4, 3, 4),
];
// END GENERATED cbf

// BEGIN GENERATED smv (cargo xtask assurance-audit --write; do not edit)
const SMV_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const SMV_VALIDATED: &[Validated] = &[
    a::row(K::Dialect, "beam centre ADSC (mm, X slow, Y fast)", 2, 2, 2),
    a::row(K::Dialect, "beam centre pixels (fast, slow)", 1, 1, 1),
    a::row(K::SampleLayout, "uint16", 3, 3, 3),
];
// END GENERATED smv
