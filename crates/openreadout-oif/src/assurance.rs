//! Assurance profiles (`docs/assurance.md`) of the Olympus FluoView OIB and OIF readers: the
//! variant features of a file and the feature values the development corpus validates. The
//! tables between the GENERATED markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static OIB: AssuranceProfile = AssuranceProfile {
    format_id: "oib",
    observe,
    validated: OIB_VALIDATED,
    confidence: OIB_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static OIF: AssuranceProfile = AssuranceProfile {
    format_id: "oif",
    observe,
    validated: OIF_VALIDATED,
    confidence: OIF_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    a::writer_context(&mut o, info);
    a::instrument_context(&mut o, info);
    for im in &info.images {
        if let Some(c) = a::extra_str(&im.extra, "compression") {
            o.feature(K::Codec, c, &[Scope::Pixels]);
        }
        if let Some(m) = a::extra_str(&im.extra, "scan_mode") {
            // XY, XYZ, XYT, XT (line scan), XYL (lambda): which axes the plane files carry.
            o.feature(
                K::Acquisition,
                format!("scan {m}"),
                &[Scope::Pixels, Scope::Metadata],
            );
        }
        if a::extra_str(&im.extra, "kind") == Some("reference") {
            o.feature(K::Layout, "reference_image", &[Scope::Pixels]);
        }
    }
    if let Some(n) = a::note_with(info, "whose names carry no axis indices") {
        o.undecoded("plane files without axis indices", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "missing planes read as errors") {
        o.undecoded("missing plane files", &[], n.to_string());
    }
    if a::note_with(info, "are exposed as separate images (inferred)").is_some() {
        o.feature(K::Layout, "separate_axis_groups", &[Scope::Pixels]);
        o.assumed(
            "images",
            "plane files with different axis letters are grouped into separate images (inferred)",
        );
    }
    o
}

// BEGIN GENERATED oib (cargo xtask assurance-audit --write; do not edit)
const OIB_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const OIB_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "scan XY", 3, 2, 3),
    a::row(K::Acquisition, "scan XYL", 1, 1, 1),
    a::row(K::Acquisition, "scan XYT", 1, 1, 1),
    a::row(K::Acquisition, "scan XYZ", 2, 2, 2),
    a::row(K::Codec, "lzw", 2, 1, 2),
    a::row(K::Codec, "none", 5, 4, 5),
    a::row(K::Field, "experiment.acquisition.started_at", 7, 5, 7),
    a::row(K::Field, "experiment.instrument.model", 7, 5, 7),
    a::row(K::FormatVersion, "1.2.6.0", 7, 5, 7),
    a::row(K::Instrument, "FLUOVIEW FV1000", 7, 5, 7),
    a::row(K::SampleLayout, "uint16", 7, 5, 7),
    a::row(K::Writer, "FluoView", 7, 5, 7),
    a::row(K::WriterVersion, "FluoView 4.2", 7, 5, 7),
];
// END GENERATED oib

// BEGIN GENERATED oif (cargo xtask assurance-audit --write; do not edit)
const OIF_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const OIF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "scan XT", 1, 1, 1),
    a::row(K::Acquisition, "scan XY", 1, 1, 1),
    a::row(K::Codec, "none", 2, 1, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 2, 1, 2),
    a::row(K::Field, "experiment.instrument.model", 2, 1, 2),
    a::row(K::FormatVersion, "1.2.6.0", 2, 1, 2),
    a::row(K::Instrument, "FLUOVIEW FV1000", 2, 1, 2),
    a::row(K::Layout, "reference_image", 1, 1, 1),
    a::row(K::SampleLayout, "uint16", 2, 1, 2),
    a::row(K::Writer, "FluoView", 2, 1, 2),
    a::row(K::WriterVersion, "FluoView 4.2", 2, 1, 2),
];
// END GENERATED oif
