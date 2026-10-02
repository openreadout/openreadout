//! Assurance profile (`docs/assurance.md`): the variant features of an Olympus/Evident OIR file
//! and the feature values the development corpus validates. The table between the GENERATED
//! markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static OIR: AssuranceProfile = AssuranceProfile {
    format_id: "oir",
    observe,
    validated: OIR_VALIDATED,
    confidence: OIR_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    for im in &info.images {
        if let Some(i) = &im.instrument {
            if let Some(v) = i
                .software_version
                .as_deref()
                .and_then(|v| a::version_prefix(v, 1))
            {
                // FV31S (1.x) and FV4000 software (3.x) write different XML documents.
                o.feature(
                    K::WriterVersion,
                    format!("FLUOVIEW {v}"),
                    &[Scope::Metadata],
                );
            }
            if let Some(m) = &i.model {
                o.context(K::Instrument, m);
            }
        }
        if im.extra.contains_key("lambda") {
            o.feature(
                K::Acquisition,
                "lambda_scan",
                &[Scope::Metadata, Scope::Pixels],
            );
        }
        if a::extra_str(&im.extra, "kind") == Some("reference") {
            o.feature(K::Layout, "reference_image", &[Scope::Pixels]);
        }
        if im.mosaic.as_ref().is_some_and(|m| m.tile_count > 1) {
            o.feature(K::Layout, "tiles", &[Scope::Pixels]);
        }
        if let Some(c) = a::extra_str(&im.extra, "color_type") {
            o.feature(K::SampleLayout, format!("color_type {c}"), &[Scope::Pixels]);
        }
    }
    if a::has_note(info, "no pixel data is stored in this file") {
        o.feature(K::Layout, "metadata_only", &[Scope::Metadata]);
    }
    if info
        .notes
        .iter()
        .any(|n| n.contains("continuation file(s) read"))
    {
        o.feature(K::Layout, "continuation_files", &[Scope::Pixels]);
    }
    if let Some(n) = a::note_with(info, "carry axis letters other than t/l/z") {
        o.undecoded("chunks along unknown axes", &[], n.to_string());
    }
    if a::has_note(info, "file appears truncated") {
        o.undecoded(
            "truncated chunks",
            &[Scope::Pixels],
            "the file ends inside its data; `check` lists the planes affected",
        );
    }
    o
}

// BEGIN GENERATED oir (cargo xtask assurance-audit --write; do not edit)
const OIR_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const OIR_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "lambda_scan", 3, 1, 3),
    a::row(K::Field, "experiment.acquisition.started_at", 15, 8, 15),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 15),
    a::row(K::FormatVersion, "2.1.2.1", 4, 2, 4),
    a::row(K::FormatVersion, "2.1.2.3", 12, 6, 12),
    a::row(K::Instrument, "FV3000", 3, 3, 3),
    a::row(K::Instrument, "FV4000", 8, 4, 8),
    a::row(K::Instrument, "FV4000 (Modified)", 1, 1, 1),
    a::row(K::Instrument, "FluoView Software", 3, 1, 3),
    a::row(K::Layout, "continuation_files", 1, 1, 1),
    a::row(K::Layout, "metadata_only", 1, 1, 1),
    a::row(K::Layout, "reference_image", 13, 7, 13),
    a::row(K::SampleLayout, "color_type GlayScale", 15, 8, 15),
    a::row(K::SampleLayout, "uint16", 15, 8, 15),
    a::row(K::WriterVersion, "FLUOVIEW 1", 3, 1, 3),
    a::row(K::WriterVersion, "FLUOVIEW 2", 3, 3, 3),
    a::row(K::WriterVersion, "FLUOVIEW 3", 9, 4, 9),
];
// END GENERATED oir
