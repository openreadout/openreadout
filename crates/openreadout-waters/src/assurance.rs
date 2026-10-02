//! Assurance profile (`docs/assurance.md`): the variant features of a Waters MassLynx `.raw`
//! directory and the feature values the development corpus validates. The table between the
//! GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static WATERS_RAW: AssuranceProfile = AssuranceProfile {
    format_id: "waters-raw",
    observe,
    validated: WATERS_RAW_VALIDATED,
    confidence: WATERS_RAW_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

/// The instrument generation a model string belongs to (`SYNAPTG2-Si` → Synapt); an unknown
/// model is its own generation.
pub(crate) fn instrument_generation(model: &str) -> String {
    let m = model.to_ascii_uppercase().replace([' ', '-', '_'], "");
    let g = if m.contains("SYNAPT") {
        "generation Synapt"
    } else if m.contains("TQ") {
        "generation Xevo TQ"
    } else if m.contains("QTOF") || m.contains("G2XS") || m.contains("G2S") {
        "generation Xevo QTOF"
    } else if m.contains("SQD") || m.contains("QDA") {
        "generation single quadrupole"
    } else {
        return model.to_string();
    };
    g.to_string()
}

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [
        Scope::Metadata,
        Scope::Spectra,
        Scope::Traces,
        Scope::Tables,
    ];
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &all);
    }
    // The instrument generation decides the function layouts (Synapt mobility, Xevo TQ MRM);
    // the exact model string only describes the file.
    for m in a::extra_values(info, "instrument") {
        o.feature(
            K::Instrument,
            instrument_generation(&m),
            &[Scope::Spectra, Scope::Tables],
        );
        o.context(K::Instrument, m);
    }
    for t in &info.tables {
        if let Some(k) = a::extra_str(&t.extra, "function_kind") {
            o.feature(K::Acquisition, k, &[Scope::Spectra, Scope::Tables]);
        }
        if let Some(b) = a::extra_text(&t.extra, "bytes_per_value") {
            o.feature(
                K::SampleLayout,
                format!("{b}-byte values"),
                &[Scope::Spectra, Scope::Tables],
            );
        }
    }
    for s in &info.spectra {
        if let Some(i) = &s.instrument
            && let Some(v) = &i.software_version
        {
            o.context(K::WriterVersion, format!("MassLynx {v}"));
        }
        if let Some(st) = a::extra_str(&s.extra, "stored_spectra") {
            o.feature(K::Acquisition, format!("stored {st}"), &[Scope::Spectra]);
        }
        if s.extra.contains_key("lock_mass") {
            o.calibration(
                "lock-mass correction",
                CalibrationStatus::Available,
                &[Scope::Spectra],
                "the acquisition recorded a lock-spray reference (extra.lock_mass); masses are returned as calibrated by the Cal Function, without the lock-mass drift correction MassLynx can apply in processing",
            );
        }
        o.calibration(
            "m/z calibration (Cal Function)",
            CalibrationStatus::Applied,
            &[Scope::Spectra],
            "each scanning function's stored masses are calibrated with its Cal Function line (unless the index says they already are)",
        );
    }
    for t in &info.traces {
        if a::extra_text(&t.extra, "analog_channel").is_some() {
            o.feature(K::Record, "analog channel", &[Scope::Traces]);
        }
    }
    o
}

// BEGIN GENERATED waters-raw (cargo xtask assurance-audit --write; do not edit)
const WATERS_RAW_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const WATERS_RAW_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "MOBILITY FAST DDA FUNCTION", 0, 0, 1),
    a::row(K::Acquisition, "MOBILITY MSMS FUNCTION", 0, 0, 1),
    a::row(K::Acquisition, "MOBILITY SURVEY FUNCTION", 0, 0, 1),
    a::row(K::Acquisition, "REFERENCE", 5, 2, 5),
    a::row(K::Acquisition, "TOF FAST DDA FUNCTION", 3, 1, 3),
    a::row(K::Acquisition, "TOF MS FUNCTION", 2, 2, 2),
    a::row(K::Acquisition, "TOF MSMS FUNCTION", 1, 1, 1),
    a::row(K::Acquisition, "TOF PARENT FUNCTION", 1, 1, 3),
    a::row(K::Acquisition, "TOF SURVEY FUNCTION", 4, 1, 4),
    a::row(K::Acquisition, "stored centroid", 5, 3, 5),
    a::row(K::Acquisition, "stored mixed", 6, 1, 10),
    a::row(K::Field, "experiment.acquisition.started_at", 16, 6, 16),
    a::row(K::Field, "experiment.instrument.model", 16, 6, 16),
    a::row(K::FormatVersion, "01.00", 18, 6, 18),
    a::row(K::Instrument, "ACQ-SQD", 1, 1, 1),
    a::row(K::Instrument, "JAA143 Synapt MS", 1, 1, 1),
    a::row(K::Instrument, "SYNAPT-G2", 1, 1, 1),
    a::row(K::Instrument, "SYNAPT-XS", 1, 1, 1),
    a::row(K::Instrument, "SYNAPTG2-Si", 7, 1, 7),
    a::row(K::Instrument, "XEVO-G2XSQTOF", 1, 1, 1),
    a::row(K::Instrument, "XEVO-TQMS", 1, 1, 1),
    a::row(K::Instrument, "XEVO-TQS", 1, 1, 1),
    a::row(K::Instrument, "XEVO-TQXS", 2, 2, 2),
    a::row(K::Instrument, "generation Synapt", 7, 3, 10),
    a::row(K::Instrument, "generation Xevo QTOF", 0, 0, 1),
    a::row(K::Instrument, "generation Xevo TQ", 4, 4, 4),
    a::row(K::Instrument, "generation single quadrupole", 1, 1, 1),
    a::row(K::Record, "analog channel", 0, 0, 3),
    a::row(K::SampleLayout, "12-byte values", 5, 3, 5),
    a::row(K::SampleLayout, "2-byte values", 2, 2, 2),
    a::row(K::SampleLayout, "4-byte values", 2, 2, 2),
    a::row(K::SampleLayout, "6-byte values", 2, 1, 2),
    a::row(K::SampleLayout, "8-byte values", 5, 1, 9),
    a::row(K::WriterVersion, "MassLynx 4.1", 3, 2, 3),
    a::row(K::WriterVersion, "MassLynx 4.1 SCN957", 3, 1, 3),
    a::row(K::WriterVersion, "MassLynx 4.2 SCN1028", 1, 1, 1),
    a::row(K::WriterVersion, "MassLynx 4.2 SCN983", 4, 1, 4),
];
// END GENERATED waters-raw
