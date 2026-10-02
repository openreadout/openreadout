//! Assurance profile (`docs/assurance.md`): the variant features of a Blackrock NSx/NEV file
//! or session and the feature values the development corpus validates. The table between the
//! GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static BLACKROCK: AssuranceProfile = AssuranceProfile {
    format_id: "blackrock",
    observe,
    validated: BLACKROCK_VALIDATED,
    confidence: BLACKROCK_CONFIDENCE,
    basis: Basis::VendorDocs,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        if let Some(s) = a::extra_str(&t.extra, "spec") {
            o.feature(
                K::FormatVersion,
                format!("NSx {s}"),
                &[Scope::Metadata, Scope::Traces],
            );
        }
        if t.extra.get("ptp").and_then(serde_json::Value::as_bool) == Some(true) {
            o.feature(K::Layout, "PTP timestamps", &[Scope::Traces]);
        }
        if t.sweep_count > 1 {
            o.feature(K::Layout, "paused (several data packets)", &[Scope::Traces]);
        }
    }
    for t in &info.tables {
        if let Some(v) = a::extra_str(&t.extra, "file_version") {
            o.feature(
                K::FormatVersion,
                format!("NEV {v}"),
                &[Scope::Metadata, Scope::Tables],
            );
        }
        if let Some(s) = a::extra_str(&t.extra, "signature") {
            o.feature(K::Record, s, &[Scope::Tables]);
        }
        if let Some(p) = a::extra_text(&t.extra, "packet_size") {
            o.feature(
                K::SampleLayout,
                format!("{p}-byte packets"),
                &[Scope::Tables],
            );
        }
        if let Some(app) = a::extra_str(&t.extra, "application") {
            o.context(K::Writer, a::writer_name_only(app));
        }
    }
    if !info.traces.is_empty() {
        o.calibration(
            "digital units to µV",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "samples are scaled with each channel's analog and digital ranges from the extended headers",
        );
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded("damaged data packets", &[Scope::Traces], n.to_string());
    }
    o
}

// BEGIN GENERATED blackrock (cargo xtask assurance-audit --write; do not edit)
const BLACKROCK_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const BLACKROCK_VALIDATED: &[Validated] = &[
    a::row(K::FormatVersion, "NEV 2.1", 3, 2, 3),
    a::row(K::FormatVersion, "NEV 2.3", 3, 2, 3),
    a::row(K::FormatVersion, "NEV 3.0", 2, 1, 2),
    a::row(K::FormatVersion, "NSx 2.1", 2, 1, 3),
    a::row(K::FormatVersion, "NSx 2.2/2.3", 3, 1, 3),
    a::row(K::FormatVersion, "NSx 3.0", 3, 1, 3),
    a::row(K::Layout, "PTP timestamps", 1, 1, 1),
    a::row(K::Layout, "paused (several data packets)", 2, 1, 2),
    a::row(K::Record, "BREVENTS", 2, 1, 2),
    a::row(K::Record, "NEURALEV", 6, 3, 6),
    a::row(K::SampleLayout, "104-byte packets", 5, 3, 5),
    a::row(K::SampleLayout, "108-byte packets", 2, 1, 2),
    a::row(K::SampleLayout, "84-byte packets", 1, 1, 1),
    a::row(K::Writer, "File Dialog", 8, 3, 8),
];
// END GENERATED blackrock
