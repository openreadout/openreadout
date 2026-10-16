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
const CBF_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const CBF_VALIDATED: &[Validated] = &[
];
// END GENERATED cbf

// BEGIN GENERATED smv (cargo xtask assurance-audit --write; do not edit)
const SMV_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const SMV_VALIDATED: &[Validated] = &[
];
// END GENERATED smv
