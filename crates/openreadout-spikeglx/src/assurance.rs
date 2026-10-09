//! Assurance profile (`docs/assurance.md`): the variant features of a SpikeGLX `.meta`/`.bin`
//! pair and the feature values the development corpus validates. The table between the
//! GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static SPIKEGLX: AssuranceProfile = AssuranceProfile {
    format_id: "spikeglx",
    observe,
    validated: SPIKEGLX_VALIDATED,
    confidence: SPIKEGLX_CONFIDENCE,
    basis: Basis::VendorDocs,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        if let Some(v) = a::extra_str(&t.extra, "app_version") {
            // The .meta keys change with the SpikeGLX release; the year is a fair grouping.
            let year: String = v.chars().take(4).collect();
            o.feature(
                K::FormatVersion,
                format!("SpikeGLX {year}"),
                &[Scope::Metadata, Scope::Traces],
            );
        }
        if let Some(k) = a::extra_str(&t.extra, "stream_kind") {
            o.feature(K::Record, format!("{k} stream"), &[Scope::Traces]);
        }
        if let Some(p) = a::extra_text(&t.extra, "probe_type") {
            o.feature(K::Instrument, format!("probe type {p}"), &[Scope::Traces]);
        }
        if let Some(m) = a::extra_text(&t.extra, "max_int") {
            o.feature(K::SampleLayout, format!("max int {m}"), &[Scope::Traces]);
        }
    }
    if !info.traces.is_empty() {
        o.calibration(
            "ADC counts to µV / V (imAiRangeMax, gains)",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "imec AP/LF values in µV and NI/OneBox analog values in V, from the .meta ranges and per-channel gains; status and digital words stay raw",
        );
    }
    if let Some(n) = a::note_with(info, "(truncated or a stub)") {
        o.undecoded(
            "samples the .meta declares beyond the .bin",
            &[],
            n.to_string(),
        );
    }
    o
}

// BEGIN GENERATED spikeglx (cargo xtask assurance-audit --write; do not edit)
const SPIKEGLX_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const SPIKEGLX_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 12, 2, 18),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 15),
    a::row(K::FormatVersion, "SpikeGLX 2019", 4, 1, 4),
    a::row(K::FormatVersion, "SpikeGLX 2020", 5, 2, 6),
    a::row(K::FormatVersion, "SpikeGLX 2023", 9, 2, 9),
    a::row(K::Instrument, "probe type 0.0", 7, 2, 7),
    a::row(K::Instrument, "probe type 1030.0", 1, 1, 1),
    a::row(K::Instrument, "probe type 2013.0", 4, 1, 4),
    a::row(K::Instrument, "probe type 24.0", 2, 2, 3),
    a::row(K::Record, "imec stream", 14, 3, 15),
    a::row(K::Record, "nidq stream", 6, 1, 6),
    a::row(K::SampleLayout, "max int 2048.0", 4, 1, 4),
    a::row(K::SampleLayout, "max int 32768.0", 4, 1, 4),
    a::row(K::SampleLayout, "max int 512.0", 5, 2, 5),
    a::row(K::SampleLayout, "max int 8192.0", 2, 2, 3),
];
// END GENERATED spikeglx
