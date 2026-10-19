//! Assurance profile (`docs/assurance.md`): the variant features of an Igor binary wave or
//! packed experiment and the feature values the development corpus validates. The table between
//! the GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static IGOR: AssuranceProfile = AssuranceProfile {
    format_id: "igor",
    observe,
    validated: IGOR_VALIDATED,
    confidence: IGOR_CONFIDENCE,
    basis: Basis::VendorDocs,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [Scope::Metadata, Scope::Traces, Scope::Pixels];
    let container = info.format_version.as_deref().unwrap_or("");
    // the binary-wave version decides every header offset; a packed experiment can mix versions
    for v in a::extra_values(info, "wave_version") {
        let value = if container.starts_with("pxp") {
            format!("pxp wave {v}")
        } else {
            format!("ibw {v}")
        };
        o.feature(K::FormatVersion, value, &all);
    }
    for v in a::extra_values(info, "application_version") {
        // Igor Pro 8.040 → `Igor Pro 8`: the record layout follows the major version
        let major = v.split('.').next().unwrap_or(&v).to_string();
        o.context(K::WriterVersion, format!("Igor Pro {major}"));
    }
    for t in a::extra_values(info, "wave_type") {
        o.feature(K::SampleLayout, t, &[Scope::Traces, Scope::Pixels]);
    }
    for b in a::extra_values(info, "byte_order") {
        o.feature(K::Layout, b, &all);
    }
    for t in &info.traces {
        let shape = if t
            .extra
            .get("dimensions")
            .and_then(|d| d.as_array())
            .is_some_and(|d| d.len() == 2)
        {
            "trace from labelled columns"
        } else {
            "trace from one dimension"
        };
        o.feature(K::Layout, shape, &[Scope::Traces]);
    }
    for i in &info.images {
        let layers = i
            .extra
            .get("layers_as")
            .and_then(|v| v.as_str())
            .unwrap_or("z");
        let rank = i
            .extra
            .get("dimensions")
            .and_then(|d| d.as_array())
            .map_or(2, Vec::len);
        let shape = match (rank, layers) {
            (2, _) => "image from two dimensions".to_string(),
            (_, l) => format!("image with layers as {l}"),
        };
        o.feature(K::Layout, shape, &[Scope::Pixels]);
    }
    if a::note_with(info, "bytes after the wave's last section").is_some() {
        o.undecoded(
            "data appended after the wave",
            &[],
            "instrument software (Asylum Research) appends a preview after the sections the format defines; it is not read",
        );
    }
    o
}

// BEGIN GENERATED igor (cargo xtask assurance-audit --write; do not edit)
const IGOR_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const IGOR_VALIDATED: &[Validated] = &[
    a::row(K::FormatVersion, "ibw 2", 2, 1, 2),
    a::row(K::FormatVersion, "ibw 5", 15, 12, 15),
    a::row(K::FormatVersion, "pxp wave 2", 1, 1, 1),
    a::row(K::FormatVersion, "pxp wave 5", 15, 13, 15),
    a::row(K::Layout, "big-endian", 1, 1, 1),
    a::row(K::Layout, "image from two dimensions", 6, 5, 6),
    a::row(K::Layout, "image with layers as channels", 4, 4, 4),
    a::row(K::Layout, "image with layers as z", 1, 1, 1),
    a::row(K::Layout, "little-endian", 32, 27, 32),
    a::row(K::Layout, "trace from labelled columns", 2, 2, 2),
    a::row(K::Layout, "trace from one dimension", 22, 18, 22),
    a::row(K::SampleLayout, "float32", 19, 18, 19),
    a::row(K::SampleLayout, "float64", 18, 16, 18),
    a::row(K::SampleLayout, "int8", 1, 1, 1),
    a::row(K::WriterVersion, "Igor Pro 6", 1, 1, 1),
    a::row(K::WriterVersion, "Igor Pro 7", 2, 2, 2),
    a::row(K::WriterVersion, "Igor Pro 8", 8, 7, 8),
    a::row(K::WriterVersion, "Igor Pro 9", 5, 4, 5),
];
// END GENERATED igor
