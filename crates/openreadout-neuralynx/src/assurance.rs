//! Assurance profile (`docs/assurance.md`): the variant features of a Neuralynx file or
//! recording directory and the feature values the development corpus validates. The table
//! between the GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static NEURALYNX: AssuranceProfile = AssuranceProfile {
    format_id: "neuralynx",
    observe,
    validated: NEURALYNX_VALIDATED,
    confidence: NEURALYNX_CONFIDENCE,
    basis: Basis::VendorDocs,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [Scope::Metadata, Scope::Traces, Scope::Tables];
    let versions = a::extra_values(info, "file_version");
    if versions.is_empty() {
        // Older Cheetah headers carry no -FileVersion.
        o.feature(K::FormatVersion, "no -FileVersion", &all);
    }
    for v in versions {
        o.feature(K::FormatVersion, v, &all);
    }
    for k in a::extra_values(info, "file_kind") {
        let scope = if k == "ncs" {
            Scope::Traces
        } else {
            Scope::Tables
        };
        o.feature(K::Record, k, &[scope]);
    }
    for app in a::extra_values(info, "application") {
        o.context(K::Writer, &app);
        for v in a::extra_values(info, "application_version") {
            if let Some(v) = a::version_prefix(&v, 1) {
                o.context(K::WriterVersion, format!("{app} {v}"));
            }
        }
    }
    for h in a::extra_values(info, "hardware_subsystem") {
        o.context(K::Instrument, h);
    }
    for s in a::extra_values(info, "segmented_by") {
        o.feature(K::Layout, format!("segmented by {s}"), &[Scope::Traces]);
    }
    if !info.traces.is_empty() || !info.tables.is_empty() {
        o.calibration(
            "ADC counts to µV (ADBitVolts)",
            CalibrationStatus::Applied,
            &[Scope::Traces, Scope::Tables],
            "values in µV = raw × ADBitVolts × 1e6; the sign is not flipped for -InputInverted, as neo reads them",
        );
    }
    o
}

// BEGIN GENERATED neuralynx (cargo xtask assurance-audit --write; do not edit)
const NEURALYNX_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const NEURALYNX_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 20, 3, 22),
    a::row(K::FormatVersion, "3.1.0", 1, 1, 1),
    a::row(K::FormatVersion, "3.2", 3, 2, 3),
    a::row(K::FormatVersion, "3.3.0", 4, 3, 4),
    a::row(K::FormatVersion, "3.4", 7, 2, 7),
    a::row(K::FormatVersion, "no -FileVersion", 8, 1, 8),
    a::row(K::Instrument, "DigitalLynx", 1, 1, 1),
    a::row(K::Instrument, "DigitalLynxSX", 6, 2, 6),
    a::row(K::Layout, "segmented by every record timestamp", 8, 1, 8),
    a::row(K::Layout, "segmented by first and last record (one gap-free run; `check` reads every record)", 6, 2, 6),
    a::row(K::Record, "ncs", 13, 2, 13),
    a::row(K::Record, "nev", 6, 3, 6),
    a::row(K::Record, "nse", 2, 1, 2),
    a::row(K::Record, "ntt", 3, 2, 3),
    a::row(K::Record, "nvt", 1, 1, 1),
    a::row(K::Writer, "Cheetah", 20, 3, 20),
    a::row(K::WriterVersion, "Cheetah 4", 1, 1, 1),
    a::row(K::WriterVersion, "Cheetah 5", 12, 2, 12),
    a::row(K::WriterVersion, "Cheetah 6", 7, 2, 7),
];
// END GENERATED neuralynx
