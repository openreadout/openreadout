//! Assurance profile (`docs/assurance.md`) of the TDMS reader: the variant features of a file and
//! the feature values the development corpus validates. The table between the GENERATED markers
//! is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static TDMS: AssuranceProfile = AssuranceProfile {
    format_id: "ni-tdms",
    observe,
    validated: NI_TDMS_VALIDATED,
    confidence: NI_TDMS_CONFIDENCE,
    basis: Basis::VendorDocs,
};

/// The TDMS version, the data layout (interleaved, big-endian, DAQmx raw buffers), the stored
/// value types and the scalings applied decide how values are decoded; the writer only
/// describes the file.
fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(t) = info.traces.first() {
        if let Some(v) = t.extra.get("tdms_versions").and_then(|v| v.as_array()) {
            for v in v.iter().filter_map(|v| v.as_str()) {
                o.feature(K::FormatVersion, format!("TDMS {v}"), &[Scope::Traces]);
            }
        }
        for l in a::extra_values_in(&t.extra, "layout") {
            o.feature(K::Layout, l, &[Scope::Traces]);
        }
        if let Some(w) = a::extra_str(&t.extra, "writer") {
            o.context(K::Writer, w);
        }
    }
    for t in &info.traces {
        for c in &t.channels {
            o.feature(K::SampleLayout, &c.dtype, &[Scope::Traces]);
            if let Some(s) = c.extra.get("scaling").and_then(|v| v.as_array()) {
                for k in s
                    .iter()
                    .filter_map(|k| k.get("type").and_then(|t| t.as_str()))
                {
                    o.feature(K::Codec, format!("{k} scale"), &[Scope::Traces]);
                }
            }
        }
    }
    for n in &info.notes {
        if n.contains("are not read") || n.contains("is not applied") {
            o.undecoded("channels", &[], n.clone());
        }
    }
    o
}

// BEGIN GENERATED ni-tdms (cargo xtask assurance-audit --write; do not edit)
const NI_TDMS_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const NI_TDMS_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "linear scale", 1, 1, 1),
    a::row(K::Codec, "polynomial scale", 1, 1, 1),
    a::row(K::FormatVersion, "TDMS 4712", 12, 3, 12),
    a::row(K::FormatVersion, "TDMS 4713", 11, 11, 11),
    a::row(K::Layout, "DAQmx raw data", 2, 2, 2),
    a::row(K::Layout, "interleaved", 4, 4, 4),
    a::row(K::SampleLayout, "float32", 5, 5, 5),
    a::row(K::SampleLayout, "float64", 11, 10, 11),
    a::row(K::SampleLayout, "int16", 3, 3, 3),
    a::row(K::SampleLayout, "int32", 4, 3, 4),
    a::row(K::SampleLayout, "int64", 1, 1, 1),
    a::row(K::SampleLayout, "int8", 6, 1, 6),
    a::row(K::SampleLayout, "timestamp", 1, 1, 1),
    a::row(K::SampleLayout, "uint16", 1, 1, 1),
    a::row(K::SampleLayout, "uint32", 1, 1, 1),
    a::row(K::SampleLayout, "uint64", 1, 1, 1),
    a::row(K::SampleLayout, "uint8", 1, 1, 1),
    a::row(K::Writer, "SignalExpress 2015", 1, 1, 1),
];
// END GENERATED ni-tdms
