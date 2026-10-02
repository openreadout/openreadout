//! Assurance profile (`docs/assurance.md`): the variant features of a Zeiss ZVI file and the
//! feature values the development corpus validates. The table between the GENERATED markers is
//! written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static ZVI: AssuranceProfile = AssuranceProfile {
    format_id: "zvi",
    observe,
    validated: ZVI_VALIDATED,
    confidence: ZVI_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    // ZVI records no container version: the per-item pixel format is the variant.
    o.feature(K::FormatVersion, "ole2", &[Scope::Metadata, Scope::Pixels]);
    a::image_basics(&mut o, info);
    a::writer_context(&mut o, info);
    a::instrument_context(&mut o, info);
    for pf in a::extra_values(info, "pixel_format") {
        o.feature(
            K::SampleLayout,
            format!("pixel_format {pf}"),
            &[Scope::Pixels],
        );
    }
    if info.images.len() > 1 {
        o.feature(K::Layout, "multiple_images", &[Scope::Pixels]);
    }
    if let Some(n) = a::note_with(
        info,
        "each combination is exposed as a separate image (inferred",
    ) {
        o.feature(K::Layout, "position_tags", &[Scope::Pixels]);
        o.assumed("images", n.to_string());
    }
    if let Some(n) = a::note_with(info, "holds no raw image header") {
        o.undecoded("items without an image header", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "scale unit code") {
        o.undecoded("unknown scale unit", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "missing planes read as errors") {
        o.undecoded("missing items", &[], n.to_string());
    }
    o
}

// BEGIN GENERATED zvi (cargo xtask assurance-audit --write; do not edit)
const ZVI_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const ZVI_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 10, 10, 13),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 12),
    a::row(K::FormatVersion, "ole2", 15, 12, 16),
    a::row(K::Instrument, "Axio Imager Z1", 1, 1, 1),
    a::row(K::Instrument, "Axio Imager.A2", 1, 1, 1),
    a::row(K::Instrument, "Axio Imager.M1", 2, 2, 2),
    a::row(K::Instrument, "Axio Imager.Z1", 1, 1, 1),
    a::row(K::Instrument, "Axio Imager.Z2", 1, 1, 1),
    a::row(K::Instrument, "Axio Observer.Z1", 3, 3, 3),
    a::row(K::Instrument, "Axioplan 2 imaging e", 1, 1, 1),
    a::row(K::Instrument, "Axioskop 2", 1, 1, 1),
    a::row(K::Instrument, "Axiovert 200 M", 1, 1, 1),
    a::row(K::SampleLayout, "pixel_format 4", 13, 10, 14),
    a::row(K::SampleLayout, "pixel_format 8", 2, 2, 2),
    a::row(K::SampleLayout, "uint16", 13, 10, 14),
    a::row(K::SampleLayout, "uint16x3", 2, 2, 2),
    a::row(K::Writer, "AxioVision", 12, 12, 13),
];
// END GENERATED zvi
