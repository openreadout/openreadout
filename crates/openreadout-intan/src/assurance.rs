//! Assurance profile (`docs/assurance.md`): the variant features of an Intan `.rhd`/`.rhs`
//! recording (single file or one-file-per-signal/channel directory) and the feature values the
//! development corpus validates. The table between the GENERATED markers is written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static INTAN: AssuranceProfile = AssuranceProfile {
    format_id: "intan",
    observe,
    validated: INTAN_VALIDATED,
    confidence: INTAN_CONFIDENCE,
    basis: Basis::VendorDocs,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    for f in a::extra_values(info, "family") {
        o.feature(K::Instrument, f, &[Scope::Traces]);
    }
    for l in a::extra_values(info, "layout") {
        o.feature(K::Layout, l, &[Scope::Traces]);
    }
    if a::extra_values(info, "layout").is_empty() && !info.traces.is_empty() {
        o.feature(K::Layout, "traditional (one file)", &[Scope::Traces]);
    }
    for s in a::extra_values(info, "signal") {
        o.feature(K::Record, format!("{s} signal"), &[Scope::Traces]);
    }
    for b in a::extra_values(info, "board_mode") {
        o.context(K::Acquisition, format!("board mode {b}"));
    }
    if !info.traces.is_empty() {
        o.calibration(
            "ADC codes to volts / µV",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "each signal kind is scaled as Intan's reference readers scale it (amplifier 0.195 µV/bit, ...)",
        );
    }
    if let Some(n) = a::note_with(info, "bytes after the last whole data block (truncated)") {
        o.undecoded("partial last data block", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "channel file(s) named by the header are absent") {
        o.undecoded("missing channel files", &[Scope::Traces], n.to_string());
    }
    if let Some(n) = a::note_with(info, "time.dat holds") {
        o.undecoded("time.dat length mismatch", &[Scope::Traces], n.to_string());
    }
    o
}

// BEGIN GENERATED intan (cargo xtask assurance-audit --write; do not edit)
const INTAN_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const INTAN_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "board mode 0", 5, 2, 5),
    a::row(K::Acquisition, "board mode 13", 6, 2, 6),
    a::row(K::Acquisition, "board mode 14", 6, 1, 6),
    a::row(K::FormatVersion, "1.0", 2, 1, 2),
    a::row(K::FormatVersion, "1.5", 1, 1, 1),
    a::row(K::FormatVersion, "3.0", 2, 2, 2),
    a::row(K::FormatVersion, "3.3", 12, 2, 12),
    a::row(K::Instrument, "rhd2000", 11, 3, 11),
    a::row(K::Instrument, "rhs2000", 6, 1, 6),
    a::row(K::Layout, "one_file_per_channel", 4, 1, 4),
    a::row(K::Layout, "one_file_per_signal_type", 3, 1, 3),
    a::row(K::Layout, "traditional (one file)", 10, 3, 10),
    a::row(K::Record, "amplifier signal", 15, 3, 15),
    a::row(K::Record, "auxiliary signal", 7, 2, 7),
    a::row(K::Record, "board_adc signal", 9, 1, 9),
    a::row(K::Record, "board_dac signal", 4, 1, 4),
    a::row(K::Record, "dc_amplifier signal", 3, 1, 3),
    a::row(K::Record, "digital_in signal", 15, 3, 15),
    a::row(K::Record, "digital_out signal", 5, 1, 5),
    a::row(K::Record, "stimulation signal", 6, 1, 6),
    a::row(K::Record, "supply signal", 1, 1, 1),
];
// END GENERATED intan
