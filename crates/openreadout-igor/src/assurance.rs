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
        let shape = if t.extra.get("dimensions").and_then(|d| d.as_array()).is_some_and(|d| d.len() == 2) {
            "trace from labelled columns"
        } else {
            "trace from one dimension"
        };
        o.feature(K::Layout, shape, &[Scope::Traces]);
    }
    for i in &info.images {
        let layers = i.extra.get("layers_as").and_then(|v| v.as_str()).unwrap_or("z");
        let rank = i.extra.get("dimensions").and_then(|d| d.as_array()).map_or(2, Vec::len);
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
const IGOR_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const IGOR_VALIDATED: &[Validated] = &[];
// END GENERATED igor
