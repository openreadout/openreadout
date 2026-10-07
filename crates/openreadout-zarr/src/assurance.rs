//! Assurance profile (`docs/assurance.md`): the variant features of an OME-Zarr store and the
//! feature values the development corpus validates. The table between the GENERATED markers is
//! written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

use crate::dataset::ZarrImage;

pub(crate) static OME_ZARR: AssuranceProfile = AssuranceProfile {
    format_id: "ome-zarr",
    observe,
    validated: OME_ZARR_VALIDATED,
    confidence: OME_ZARR_CONFIDENCE,
    basis: Basis::OpenSpec,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        // "0.4 (NGFF, Zarr v2, zip store)": NGFF version, Zarr version, store kind.
        let (ngff, rest) = v.split_once(' ').unwrap_or((v, ""));
        o.feature(
            K::FormatVersion,
            format!("NGFF {ngff}"),
            &[Scope::Metadata, Scope::Pixels],
        );
        for part in rest.trim_matches(['(', ')']).split(", ") {
            if part.starts_with("Zarr") {
                o.feature(K::FormatVersion, part, &[Scope::Pixels]);
            } else if part.ends_with("store") {
                o.feature(K::Layout, part, &[Scope::Pixels]);
            }
        }
    }
    for im in &info.images {
        o.feature(K::SampleLayout, im.pixel_type.ome_name(), &[Scope::Pixels]);
        if let Some(d) = a::extra_str(&im.extra, "dtype") {
            o.feature(K::SampleLayout, format!("dtype {d}"), &[Scope::Pixels]);
        }
        if let Some(axes) = im.extra.get("axes").and_then(serde_json::Value::as_array) {
            let s: Vec<&str> = axes.iter().filter_map(serde_json::Value::as_str).collect();
            o.feature(K::Layout, format!("axes {}", s.join("")), &[Scope::Pixels]);
        }
        if im.extra.contains_key("well") {
            o.feature(K::Layout, "plate", &[Scope::Metadata]);
        }
    }
    if let Some(n) = a::note_with(info, "have a data type that is not read") {
        o.undecoded("arrays of unsupported data types", &[], n.to_string());
    }
    for n in info
        .notes
        .iter()
        .filter(|n| n.starts_with("image group ") && n.ends_with(" is missing"))
    {
        o.undecoded("missing image groups", &[], n.clone());
    }
    o
}

/// What only the dataset knows: the codec pipeline of every array.
pub(crate) fn internal(images: &[ZarrImage]) -> Observations {
    let mut o = Observations::default();
    for im in images {
        for l in &im.levels {
            if l.codecs.is_empty() {
                o.feature(K::Codec, "raw", &[Scope::Pixels]);
            }
            for c in &l.codecs {
                o.feature(K::Codec, c, &[Scope::Pixels]);
            }
        }
    }
    o
}

// BEGIN GENERATED ome-zarr (cargo xtask assurance-audit --write; do not edit)
const OME_ZARR_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const OME_ZARR_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "blosc/lz4", 7, 5, 7),
    a::row(K::Codec, "bytes", 1, 1, 1),
    a::row(K::Codec, "sharding_indexed", 2, 2, 2),
    a::row(K::Codec, "sharding_indexed/blosc", 2, 2, 2),
    a::row(K::Codec, "sharding_indexed/bytes", 2, 2, 2),
    a::row(K::Codec, "zstd", 2, 2, 2),
    a::row(K::FormatVersion, "NGFF 0.4", 8, 6, 8),
    a::row(K::FormatVersion, "NGFF 0.5", 2, 2, 2),
    a::row(K::FormatVersion, "Zarr v2", 8, 6, 8),
    a::row(K::FormatVersion, "Zarr v3", 2, 2, 2),
    a::row(K::Layout, "axes cyx", 1, 1, 1),
    a::row(K::Layout, "axes czyx", 7, 5, 7),
    a::row(K::Layout, "axes tczyx", 1, 1, 1),
    a::row(K::Layout, "axes yx", 1, 1, 1),
    a::row(K::Layout, "axes zyx", 4, 3, 4),
    a::row(K::Layout, "directory store", 2, 2, 2),
    a::row(K::Layout, "plate", 5, 3, 5),
    a::row(K::Layout, "zip store", 8, 6, 8),
    a::row(K::SampleLayout, "dtype <f4", 1, 1, 1),
    a::row(K::SampleLayout, "dtype <u2", 7, 5, 7),
    a::row(K::SampleLayout, "dtype <u4", 4, 3, 4),
    a::row(K::SampleLayout, "dtype int8", 1, 1, 1),
    a::row(K::SampleLayout, "dtype uint16", 1, 1, 1),
    a::row(K::SampleLayout, "dtype uint8", 1, 1, 1),
    a::row(K::SampleLayout, "dtype |i1", 1, 1, 1),
    a::row(K::SampleLayout, "float", 1, 1, 1),
    a::row(K::SampleLayout, "int8", 2, 2, 2),
    a::row(K::SampleLayout, "uint16", 8, 6, 8),
    a::row(K::SampleLayout, "uint32", 4, 3, 4),
    a::row(K::SampleLayout, "uint8", 1, 1, 1),
];
// END GENERATED ome-zarr
