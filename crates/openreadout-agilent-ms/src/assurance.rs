//! Assurance profile (`docs/assurance.md`): the variant features of an Agilent MassHunter `.d`
//! directory and the feature values the development corpus validates. The table between the
//! GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static AGILENT_MASSHUNTER: AssuranceProfile = AssuranceProfile {
    format_id: "agilent-masshunter",
    observe,
    validated: AGILENT_MASSHUNTER_VALIDATED,
    confidence: AGILENT_MASSHUNTER_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

/// The instrument series an Agilent model number belongs to (`G6545B` → 6500 Q-TOF); an unknown
/// model is its own series.
pub(crate) fn instrument_generation(model: &str) -> String {
    let m = model.trim().to_ascii_uppercase();
    let digits: String = m
        .strip_prefix('G')
        .unwrap_or("")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let g = match digits.get(..3) {
        Some("656") => "generation 6560 ion-mobility Q-TOF",
        Some("654" | "655") => "generation 6500 Q-TOF",
        Some("652" | "653") => "generation 6500 Q-TOF",
        Some("622" | "623") => "generation 6200 TOF",
        Some("641" | "643" | "646" | "647" | "649") => "generation 6400 triple quadrupole",
        _ if m.contains("TANDEMQUADRUPOLE") => "generation 6400 triple quadrupole",
        _ => return model.to_string(),
    };
    g.to_string()
}

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            v,
            &[Scope::Metadata, Scope::Spectra, Scope::Traces],
        );
    }
    for s in &info.spectra {
        if let Some(i) = &s.instrument {
            // The instrument series decides the stored spectra (TOF and Q-TOF profiles, QQQ
            // transitions, mobility frames); the exact model only describes the file.
            if let Some(m) = &i.model {
                o.feature(K::Instrument, instrument_generation(m), &[Scope::Spectra]);
                o.context(K::Instrument, m);
            }
            if let Some(v) = &i.software_version {
                o.context(K::WriterVersion, v);
            }
        }
        if let Some(d) = a::extra_str(&s.extra, "ms_device") {
            o.feature(K::Acquisition, format!("device {d}"), &[Scope::Spectra]);
            if d.contains("TOF") {
                o.calibration(
                    "time-of-flight to m/z (MSMassCal.bin, DefaultMassCal.xml)",
                    CalibrationStatus::Applied,
                    &[Scope::Spectra],
                    "flight-time centroids are converted to m/z with each scan's calibration, as MassHunter reports them",
                );
            }
        }
        if let Some(st) = a::extra_str(&s.extra, "stored_spectra") {
            o.feature(K::Acquisition, format!("stored {st}"), &[Scope::Spectra]);
        }
        if s.extra.contains_key("ion_mobility") {
            o.feature(
                K::Layout,
                "ion-mobility frames (IMSFrame.bin)",
                &[Scope::Spectra, Scope::Traces],
            );
        }
        for t in a::extra_values_in(&s.extra, "scan_types") {
            o.feature(K::Acquisition, format!("scan type {t}"), &[Scope::Spectra]);
        }
    }
    for t in &info.traces {
        if let Some(k) = a::extra_str(&t.extra, "kind") {
            o.feature(K::Record, k, &[Scope::Traces]);
        }
        if let Some(d) = a::extra_str(&t.extra, "device") {
            o.context(K::Record, format!("device {d}"));
        }
    }
    o
}

// BEGIN GENERATED agilent-masshunter (cargo xtask assurance-audit --write; do not edit)
const AGILENT_MASSHUNTER_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const AGILENT_MASSHUNTER_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "device IM-QTOF", 3, 1, 3),
    a::row(K::Acquisition, "device QTOF", 6, 6, 6),
    a::row(K::Acquisition, "device TOF", 1, 1, 1),
    a::row(K::Acquisition, "device TandemQuadrupole", 9, 3, 10),
    a::row(K::Acquisition, "scan type 1", 12, 7, 12),
    a::row(K::Acquisition, "scan type 2", 1, 1, 1),
    a::row(K::Acquisition, "scan type 2048", 1, 1, 1),
    a::row(K::Acquisition, "scan type 256", 5, 3, 6),
    a::row(K::Acquisition, "scan type 512", 7, 4, 7),
    a::row(K::Acquisition, "stored centroid", 12, 7, 13),
    a::row(K::Acquisition, "stored profile", 4, 2, 4),
    a::row(K::Acquisition, "stored profile+centroid", 3, 2, 3),
    a::row(K::Field, "experiment.acquisition.started_at", 17, 7, 20),
    a::row(K::Field, "experiment.instrument.model", 6, 4, 20),
    a::row(K::FormatVersion, "MSScan layout 4", 1, 1, 1),
    a::row(K::FormatVersion, "MSScan layout 5", 10, 4, 10),
    a::row(K::FormatVersion, "MSScan layout 6", 9, 7, 9),
    a::row(K::Instrument, "G6220A", 1, 1, 1),
    a::row(K::Instrument, "G6410A", 9, 4, 9),
    a::row(K::Instrument, "G6540B", 1, 1, 1),
    a::row(K::Instrument, "G6545A", 1, 1, 1),
    a::row(K::Instrument, "G6545B", 1, 1, 1),
    a::row(K::Instrument, "G6546A", 2, 2, 2),
    a::row(K::Instrument, "G6550B", 1, 1, 1),
    a::row(K::Instrument, "G6560sim", 3, 1, 3),
    a::row(K::Instrument, "TandemQuadrupole", 1, 1, 1),
    a::row(K::Instrument, "generation 6200 TOF", 1, 1, 1),
    a::row(K::Instrument, "generation 6400 triple quadrupole", 9, 3, 10),
    a::row(K::Instrument, "generation 6500 Q-TOF", 6, 6, 6),
    a::row(K::Instrument, "generation 6560 ion-mobility Q-TOF", 3, 1, 3),
    a::row(K::Layout, "ion-mobility frames (IMSFrame.bin)", 3, 1, 3),
    a::row(K::Record, "base peak", 6, 4, 20),
    a::row(K::Record, "detector signal", 0, 0, 2),
    a::row(K::Record, "device BinPump", 5, 5, 5),
    a::row(K::Record, "device DAD", 3, 3, 3),
    a::row(K::Record, "device HiP-ALS", 6, 6, 6),
    a::row(K::Record, "device IsoPump", 2, 2, 2),
    a::row(K::Record, "device QuatPump", 1, 1, 1),
    a::row(K::Record, "device TCC", 14, 9, 14),
    a::row(K::Record, "device TandemQuadrupole", 1, 1, 1),
    a::row(K::Record, "instrument reading", 6, 4, 15),
    a::row(K::Record, "total ion current", 6, 4, 20),
    a::row(K::WriterVersion, "6200 series TOF/6500 series Q-TOF 10.1 (48.0)", 3, 3, 3),
    a::row(K::WriterVersion, "6200 series TOF/6500 series Q-TOF B.05.01 (B5125)", 1, 1, 1),
    a::row(K::WriterVersion, "6200 series TOF/6500 series Q-TOF B.06.00 (B6000)", 3, 1, 3),
    a::row(K::WriterVersion, "6200 series TOF/6500 series Q-TOF B.08.00 (B8058.0)", 1, 1, 1),
    a::row(K::WriterVersion, "6200 series TOF/6500 series Q-TOF B.09.00 (B9044.1 SP1)", 1, 1, 1),
    a::row(K::WriterVersion, "6400 Series Triple Quadrupole B.08.02 (B8260.0)", 1, 1, 1),
];
// END GENERATED agilent-masshunter
