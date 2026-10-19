//! Assurance profile (`docs/assurance.md`): the variant features of a DeltaVision file and the
//! feature values the development corpus validates. The table between the GENERATED markers is
//! written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static DELTAVISION: AssuranceProfile = AssuranceProfile {
    format_id: "deltavision",
    observe,
    validated: DELTAVISION_VALIDATED,
    confidence: DELTAVISION_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    for s in a::extra_values(info, "section_order") {
        o.feature(K::Layout, format!("sections {s}"), &[Scope::Pixels]);
    }
    for b in a::extra_values(info, "byte_order") {
        o.feature(K::Layout, b, &[Scope::Metadata, Scope::Pixels]);
    }
    for w in a::extra_values(info, "written_by") {
        o.feature(K::Writer, w, &[Scope::Metadata, Scope::Pixels]);
    }
    for m in a::extra_values(info, "pixel_mode") {
        o.feature(K::Codec, format!("mode {m}"), &[Scope::Pixels]);
    }
    o
}

// BEGIN GENERATED deltavision (cargo xtask assurance-audit --write; do not edit)
const DELTAVISION_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const DELTAVISION_VALIDATED: &[Validated] = &[];
// END GENERATED deltavision
