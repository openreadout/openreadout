//! Assurance profile (`docs/assurance.md`): the variant features of a Hamamatsu DCIMG stream
//! and the feature values the development corpus validates. The table between the GENERATED
//! markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static DCIMG: AssuranceProfile = AssuranceProfile {
    format_id: "dcimg",
    observe,
    validated: DCIMG_VALIDATED,
    confidence: DCIMG_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    a::instrument_context(&mut o, info);
    for l in a::extra_values(info, "frame_layout") {
        o.feature(K::Layout, format!("frames {l}"), &[Scope::Pixels]);
    }
    for b in a::extra_values(info, "binning") {
        if b != "1" {
            o.feature(K::Acquisition, format!("binning {b}"), &[Scope::Pixels]);
        }
    }
    if info
        .images
        .iter()
        .any(|i| i.extra.contains_key("stored_pixels"))
    {
        o.calibration(
            "4-pixel row overwrite (stored pixels restored)",
            CalibrationStatus::Applied,
            &[Scope::Pixels],
            "the camera overwrites the first pixels of one row in every frame; the values it stored are put back on read, as the vendor software shows them",
        );
    }
    if a::has_note(info, "file appears truncated") {
        o.undecoded(
            "incomplete frames",
            &[],
            "frames past the end of the file are not returned; `check` counts them",
        );
    }
    o
}

// BEGIN GENERATED dcimg (cargo xtask assurance-audit --write; do not edit)
const DCIMG_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const DCIMG_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 15),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 14),
    a::row(K::FormatVersion, "0x1000000", 14, 2, 14),
    a::row(K::FormatVersion, "0x7", 1, 1, 1),
    a::row(K::Instrument, "C11440-22C", 3, 1, 3),
    a::row(K::Instrument, "C15440-20UP", 11, 1, 11),
    a::row(K::Layout, "frames framed", 14, 2, 14),
    a::row(K::Layout, "frames packed", 1, 1, 1),
    a::row(K::SampleLayout, "uint16", 15, 3, 15),
];
// END GENERATED dcimg
