//! Assurance profile (`docs/assurance.md`): the variant features of a Plexon PLX or PL2 file
//! and the feature values the development corpus validates. The table between the GENERATED
//! markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static PLEXON: AssuranceProfile = AssuranceProfile {
    format_id: "plexon",
    observe,
    validated: PLEXON_VALIDATED,
    confidence: PLEXON_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            v,
            &[Scope::Metadata, Scope::Traces, Scope::Tables],
        );
    }
    let apps = a::extra_values(info, "application");
    for app in &apps {
        o.context(K::Writer, a::writer_name_only(app));
    }
    if apps.is_empty() && info.format_version.as_deref() == Some("PL2") {
        // offline-written PL2 (e.g. a sorter's merged output) names no application
        o.context(K::Writer, "unnamed (offline)");
    }
    // which record kinds the file holds: continuous (analog) records are confirmed only for
    // internal consistency in PL2, spike and event records against Neo on a paired PLX
    if !info.traces.is_empty() {
        let container = if info.format_version.as_deref() == Some("PL2") {
            "PL2"
        } else {
            "PLX"
        };
        o.feature(
            K::Record,
            format!("{container} continuous"),
            &[Scope::Traces],
        );
    }
    if !info.traces.is_empty() {
        o.calibration(
            "ADC counts to volts",
            CalibrationStatus::Applied,
            &[Scope::Traces, Scope::Tables],
            "continuous samples and waveforms are scaled with the header's gains and maximum voltages",
        );
    }
    if let Some(n) = a::note_with(info, "continuous channel header(s) without data blocks") {
        o.undecoded("empty continuous channels", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded(
            "damaged data blocks",
            &[Scope::Traces, Scope::Tables],
            n.to_string(),
        );
    }
    o
}

// BEGIN GENERATED plexon (cargo xtask assurance-audit --write; do not edit)
const PLEXON_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const PLEXON_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 4, 1, 6),
    a::row(K::FormatVersion, "PL2", 0, 0, 2),
    a::row(K::FormatVersion, "PLX 101", 2, 1, 2),
    a::row(K::FormatVersion, "PLX 106", 2, 1, 2),
    a::row(K::Record, "PL2 continuous", 0, 0, 2),
    a::row(K::Record, "PLX continuous", 2, 1, 2),
    a::row(K::Writer, "OmniPlex", 1, 1, 3),
];
// END GENERATED plexon
