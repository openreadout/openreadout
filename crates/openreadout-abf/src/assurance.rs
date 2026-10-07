//! Assurance profiles (`docs/assurance.md`) of the Axon ABF and ATF readers: the variant
//! features of a file and the feature values the development corpus validates. The tables
//! between the GENERATED markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static ABF: AssuranceProfile = AssuranceProfile {
    format_id: "abf",
    observe: observe_abf,
    validated: ABF_VALIDATED,
    confidence: ABF_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static ATF: AssuranceProfile = AssuranceProfile {
    format_id: "atf",
    observe: observe_atf,
    validated: ATF_VALIDATED,
    confidence: ATF_CONFIDENCE,
    basis: Basis::VendorDocs,
};

fn observe_abf(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    for t in &info.traces {
        if let Some(g) = a::extra_str(&t.extra, "generation") {
            o.feature(K::Layout, g, &[Scope::Metadata, Scope::Traces]);
        }
        if let Some(m) = a::extra_str(&t.extra, "acquisition_mode") {
            o.feature(K::Acquisition, m, &[Scope::Traces]);
        }
        if let Some(f) = a::extra_str(&t.extra, "sample_format") {
            o.feature(K::SampleLayout, f, &[Scope::Traces]);
        }
        if let Some(c) = a::extra_str(&t.extra, "creator") {
            o.context(K::Writer, a::writer_name_only(c));
            if let Some(v) =
                a::extra_str(&t.extra, "creator_version").and_then(|v| a::version_prefix(v, 1))
            {
                o.context(K::WriterVersion, format!("{} {v}", a::writer_name_only(c)));
            }
        }
        if a::extra_str(&t.extra, "sample_format") == Some("int16") {
            o.calibration(
                "ADC counts to physical units",
                CalibrationStatus::Applied,
                &[Scope::Traces],
                "int16 samples are scaled with the header's ADC range, resolution, gains and offsets, as Clampex displays them",
            );
        }
    }
    if info
        .traces
        .iter()
        .any(|t| t.extra.get("synthesized") == Some(&serde_json::Value::Bool(true)))
    {
        o.feature(
            K::Record,
            "command waveform (epoch table)",
            &[Scope::Traces],
        );
    }
    if let Some(n) = a::note_with(info, "command waveform: ") {
        o.undecoded("command waveform not synthesized", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded("damaged sections", &[Scope::Traces], n.to_string());
    }
    o
}

fn observe_atf(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    for t in &info.traces {
        if let Some(m) = a::extra_str(&t.extra, "acquisition_mode") {
            o.feature(K::Acquisition, m, &[Scope::Traces]);
        }
    }
    o
}

// BEGIN GENERATED abf (cargo xtask assurance-audit --write; do not edit)
const ABF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const ABF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "episodic", 62, 13, 62),
    a::row(K::Acquisition, "event-fixed-length", 2, 1, 2),
    a::row(K::Acquisition, "event-variable-length", 2, 2, 2),
    a::row(K::Acquisition, "gap-free", 15, 6, 15),
    a::row(K::Field, "experiment.acquisition.started_at", 34, 10, 80),
    a::row(K::FormatVersion, "1.30", 3, 2, 3),
    a::row(K::FormatVersion, "1.65", 1, 1, 1),
    a::row(K::FormatVersion, "1.83", 10, 3, 10),
    a::row(K::FormatVersion, "1.84", 6, 4, 6),
    a::row(K::FormatVersion, "2.0.0.0", 15, 4, 15),
    a::row(K::FormatVersion, "2.3.0.0", 4, 3, 4),
    a::row(K::FormatVersion, "2.4.0.0", 2, 1, 2),
    a::row(K::FormatVersion, "2.5.0.0", 2, 2, 2),
    a::row(K::FormatVersion, "2.6.0.0", 26, 7, 26),
    a::row(K::FormatVersion, "2.9.0.0", 12, 6, 12),
    a::row(K::Layout, "abf1", 20, 6, 20),
    a::row(K::Layout, "abf2", 61, 16, 61),
    a::row(K::Record, "command waveform (epoch table)", 36, 9, 36),
    a::row(K::SampleLayout, "float32", 12, 8, 12),
    a::row(K::SampleLayout, "int16", 69, 14, 69),
    a::row(K::Writer, "AXENGN", 3, 1, 3),
    a::row(K::Writer, "AxoScope", 2, 1, 2),
    a::row(K::Writer, "Clampex", 68, 17, 68),
    a::row(K::Writer, "Clampfit", 2, 1, 2),
    a::row(K::Writer, "FETCHEX", 2, 2, 2),
    a::row(K::Writer, "clampex", 3, 1, 3),
    a::row(K::WriterVersion, "AxoScope 10", 1, 1, 1),
    a::row(K::WriterVersion, "AxoScope 9", 1, 1, 1),
    a::row(K::WriterVersion, "Clampex 10", 50, 13, 50),
    a::row(K::WriterVersion, "Clampex 11", 10, 3, 10),
    a::row(K::WriterVersion, "Clampex 9", 8, 3, 8),
    a::row(K::WriterVersion, "Clampfit 10", 2, 1, 2),
    a::row(K::WriterVersion, "clampex 10", 2, 1, 2),
    a::row(K::WriterVersion, "clampex 9", 1, 1, 1),
];
// END GENERATED abf

// BEGIN GENERATED atf (cargo xtask assurance-audit --write; do not edit)
const ATF_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const ATF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "Episodic Stimulation", 5, 2, 5),
    a::row(K::Acquisition, "Fixed-Length Event-Driven", 4, 1, 4),
    a::row(K::Acquisition, "Gap Free", 1, 1, 1),
    a::row(K::Acquisition, "High-Speed Oscilloscope", 1, 1, 1),
    a::row(K::FormatVersion, "ATF 1.0", 11, 4, 11),
];
// END GENERATED atf
