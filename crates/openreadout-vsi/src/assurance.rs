//! Assurance profile (`docs/assurance.md`): the variant features of an Olympus/Evident VSI file
//! (with its ETS stacks) and the feature values the development corpus validates. The table
//! between the GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static VSI: AssuranceProfile = AssuranceProfile {
    format_id: "vsi",
    observe,
    validated: VSI_VALIDATED,
    confidence: VSI_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    for im in &info.images {
        if let Some(c) = a::extra_str(&im.extra, "compression") {
            o.feature(K::Codec, c, &[Scope::Pixels]);
        }
        if im.pyramid_levels > 1 {
            o.feature(K::Layout, "pyramid", &[Scope::Pixels]);
        }
        // Where the image size came from, and a tile grid offset from the image corner.
        if let Some(src) = a::extra_str(&im.extra, "size_source") {
            o.feature(K::Layout, format!("size from {src}"), &[Scope::Pixels]);
            if src == "tile_grid" {
                o.assumed(
                    "images[].size_x",
                    "no image size is recorded: the extent of the stored tiles is used (may include background padding)",
                );
            }
        }
        if im.extra.contains_key("tile_origin") {
            o.feature(K::Layout, "tile grid offset", &[Scope::Pixels]);
        }
        if let Some(i) = &im.instrument
            && let Some(sw) = &i.software
        {
            // VS200 ASW and cellSens write different stack trees.
            o.feature(K::Writer, sw, &[Scope::Metadata, Scope::Pixels]);
            if let Some(v) = i
                .software_version
                .as_deref()
                .and_then(|v| a::version_prefix(v, 1))
            {
                o.context(K::WriterVersion, format!("{sw} {v}"));
            }
        }
        if let Some(dims) = im
            .extra
            .get("dimensions")
            .and_then(serde_json::Value::as_array)
        {
            for d in dims {
                if let Some(k) = d.get("kind").and_then(serde_json::Value::as_str)
                    && d.get("size")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                        > 1
                {
                    o.feature(K::Layout, format!("dimension {k}"), &[Scope::Pixels]);
                }
            }
        }
    }
    if a::has_note(info, "standalone ETS tile file") {
        o.feature(
            K::Layout,
            "standalone_ets",
            &[Scope::Metadata, Scope::Pixels],
        );
        o.assumed(
            "images[].size_t",
            "a standalone .ets carries no dimension kinds: non-XY dimensions are exposed as T",
        );
    }
    if a::has_note(
        info,
        "the _<name>_ directory with the .ets pixel files was not found",
    ) {
        o.feature(K::Layout, "metadata_only", &[Scope::Metadata]);
    }
    if let Some(n) = a::note_with(info, "stacks described in the .vsi without pixel data") {
        o.undecoded("stacks without pixel files", &[], n.to_string());
    }
    o
}

// BEGIN GENERATED vsi (cargo xtask assurance-audit --write; do not edit)
const VSI_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const VSI_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "jpeg", 7, 6, 7),
    a::row(K::Codec, "jpeg-lossless", 1, 1, 1),
    a::row(K::Codec, "jpeg2000", 1, 1, 1),
    a::row(K::Codec, "raw", 3, 2, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 12),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 12),
    a::row(K::FormatVersion, "0x00030003", 4, 3, 4),
    a::row(K::FormatVersion, "0x00030005", 1, 1, 1),
    a::row(K::FormatVersion, "0x00030006", 7, 6, 7),
    a::row(K::Layout, "dimension c", 3, 3, 3),
    a::row(K::Layout, "dimension t", 2, 2, 2),
    a::row(K::Layout, "dimension z", 4, 3, 4),
    a::row(K::Layout, "metadata_only", 0, 0, 16),
    a::row(K::Layout, "pyramid", 6, 5, 7),
    a::row(K::Layout, "size from ets_header", 8, 7, 8),
    a::row(K::Layout, "size from vsi_layout", 3, 2, 4),
    a::row(K::Layout, "tile grid offset", 1, 1, 2),
    a::row(K::SampleLayout, "uint16", 4, 3, 4),
    a::row(K::SampleLayout, "uint8", 1, 1, 2),
    a::row(K::SampleLayout, "uint8x3", 6, 5, 6),
    a::row(K::Writer, "OLYMPUS VS-ASW", 1, 1, 1),
    a::row(K::Writer, "OLYMPUS VS200 ASW", 2, 2, 2),
    a::row(K::Writer, "OLYMPUS cellSens Dimension", 6, 5, 6),
    a::row(K::Writer, "Stream Essentials", 1, 1, 1),
    a::row(K::Writer, "dotSlide", 2, 1, 2),
    a::row(K::WriterVersion, "OLYMPUS VS-ASW 2", 1, 1, 1),
    a::row(K::WriterVersion, "OLYMPUS VS200 ASW 3", 1, 1, 1),
    a::row(K::WriterVersion, "OLYMPUS VS200 ASW 4", 1, 1, 1),
    a::row(K::WriterVersion, "OLYMPUS cellSens Dimension 1", 1, 1, 1),
    a::row(K::WriterVersion, "OLYMPUS cellSens Dimension 3", 2, 2, 2),
    a::row(K::WriterVersion, "OLYMPUS cellSens Dimension 4", 3, 2, 3),
    a::row(K::WriterVersion, "Stream Essentials 1", 1, 1, 1),
    a::row(K::WriterVersion, "dotSlide 2", 2, 1, 2),
];
// END GENERATED vsi
