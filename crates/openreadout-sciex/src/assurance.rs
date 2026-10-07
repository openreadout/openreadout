//! Assurance profile (`docs/assurance.md`): the variant features of a Sciex `.wiff`/`.wiff.scan`
//! pair and the feature values the development corpus validates. The table between the
//! GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static SCIEX_WIFF: AssuranceProfile = AssuranceProfile {
    format_id: "sciex-wiff",
    observe,
    validated: SCIEX_WIFF_VALIDATED,
    confidence: SCIEX_WIFF_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

/// The instrument generation a model belongs to (`TripleTOF 5600+` → TripleTOF); an unknown
/// model is its own generation.
pub(crate) fn instrument_generation(model: &str) -> String {
    let m = model.to_ascii_lowercase().replace(' ', "");
    let g = if m.contains("tripletof") {
        "generation TripleTOF"
    } else if m.contains("zenotof") {
        "generation ZenoTOF"
    } else if m.contains("qtrap") {
        "generation QTRAP"
    } else if m.contains("qstar") {
        "generation QSTAR"
    } else if m.contains("x500") {
        "generation X500"
    } else {
        return model.to_string();
    };
    g.to_string()
}

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    // `.wiff` records no container version: the writer generation stands in for it.
    for s in &info.spectra {
        if let Some(i) = &s.instrument {
            if let Some(sw) = &i.software {
                o.feature(K::Writer, sw, &[Scope::Metadata, Scope::Spectra]);
                if let Some(v) = i
                    .software_version
                    .as_deref()
                    .and_then(|v| a::version_prefix(v, 2))
                {
                    o.context(K::WriterVersion, format!("{sw} {v}"));
                }
            }
            // The instrument generation decides the scan layouts (TOF histograms of QSTAR and
            // TripleTOF, MRM cycles of QTRAP); the exact model only describes the file.
            if let Some(m) = &i.model {
                o.feature(K::Instrument, instrument_generation(m), &[Scope::Spectra]);
                o.context(K::Instrument, m);
            }
        }
        if let Some(st) = a::extra_str(&s.extra, "stored_spectra") {
            o.feature(K::Acquisition, format!("stored {st}"), &[Scope::Spectra]);
        }
        if let Some(ex) = s
            .extra
            .get("experiments")
            .and_then(serde_json::Value::as_array)
        {
            for e in ex {
                if let Some(t) = e.get("scan_type").and_then(serde_json::Value::as_str) {
                    o.feature(K::Acquisition, format!("scan type {t}"), &[Scope::Spectra]);
                    if t.contains("TOF") {
                        o.calibration(
                            "time-to-digital bins to m/z (TOFCalibrationData)",
                            CalibrationStatus::Applied,
                            &[Scope::Spectra],
                            "each scan's TOF calibration converts histogram bins to m/z",
                        );
                    }
                }
            }
        }
    }
    if let Some(n) = a::note_with(info, "periods in the method") {
        o.feature(K::Layout, "several periods", &[Scope::Spectra]);
        o.assumed("spectra[].scans experiment assignment", n.to_string());
    }
    if let Some(n) = a::note_with(info, "are listed but their scans are not decoded") {
        o.undecoded("experiments of unsupported scan types", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "mass ranges parsed") {
        // MRM transitions come from the mass ranges (their Q1/Q3 are the spectra's values); a
        // TOF experiment's ranges carry only its scan window and collision energy.
        let tof_only = info.spectra.iter().all(|s| {
            s.extra
                .get("experiments")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|ex| {
                    ex.iter().all(|e| {
                        e.get("scan_type")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|t| t.contains("TOF"))
                    })
                })
        });
        let scope: &[Scope] = if tof_only { &[] } else { &[Scope::Spectra] };
        o.undecoded("mass ranges", scope, n.to_string());
    }
    if a::has_note(info, "single-file layout") {
        o.feature(K::Layout, "single file (Scan stream)", &[Scope::Spectra]);
    }
    if let Some(n) = a::note_with(info, "beside the .wiff: metadata and the scan index only") {
        o.feature(K::Layout, "without .wiff.scan", &[Scope::Metadata]);
        o.undecoded("spectra in the missing .wiff.scan", &[], n.to_string());
    }
    o
}

// BEGIN GENERATED sciex-wiff (cargo xtask assurance-audit --write; do not edit)
const SCIEX_WIFF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const SCIEX_WIFF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "scan type MRM", 0, 0, 3),
    a::row(K::Acquisition, "scan type Q1 scan", 1, 1, 1),
    a::row(K::Acquisition, "scan type TOF MS", 3, 3, 5),
    a::row(K::Acquisition, "scan type TOF product ion", 3, 3, 5),
    a::row(K::Acquisition, "scan type enhanced MS", 1, 1, 2),
    a::row(K::Acquisition, "scan type enhanced product ion", 1, 1, 2),
    a::row(K::Acquisition, "scan type neutral loss", 2, 1, 2),
    a::row(K::Acquisition, "scan type precursor ion", 1, 1, 1),
    a::row(K::Acquisition, "stored SRM", 0, 0, 3),
    a::row(K::Acquisition, "stored profile", 7, 6, 10),
    a::row(K::Field, "experiment.acquisition.started_at", 6, 5, 13),
    a::row(K::Field, "experiment.instrument.model", 2, 2, 13),
    a::row(K::Instrument, "4000 Q TRAP", 1, 1, 1),
    a::row(K::Instrument, "API 2000", 1, 1, 1),
    a::row(K::Instrument, "QStar XL", 1, 1, 1),
    a::row(K::Instrument, "QTRAP 5500", 1, 1, 1),
    a::row(K::Instrument, "QTRAP 6500", 2, 2, 3),
    a::row(K::Instrument, "QTRAP 6500+", 2, 2, 2),
    a::row(K::Instrument, "TripleTOF 5600+", 1, 1, 1),
    a::row(K::Instrument, "TripleTOF 6600", 1, 1, 2),
    a::row(K::Instrument, "ZenoTOF 7600+ system", 1, 1, 1),
    a::row(K::Instrument, "generation QSTAR", 1, 1, 1),
    a::row(K::Instrument, "generation QTRAP", 3, 2, 7),
    a::row(K::Instrument, "generation TripleTOF", 2, 2, 3),
    a::row(K::Instrument, "generation ZenoTOF", 0, 0, 1),
    a::row(K::Layout, "single file (Scan stream)", 1, 1, 1),
    a::row(K::Writer, "Analyst", 7, 5, 8),
    a::row(K::Writer, "Analyst QS", 1, 1, 1),
    a::row(K::Writer, "Analyst TF", 2, 2, 3),
    a::row(K::Writer, "SCIEX OS", 1, 1, 1),
    a::row(K::WriterVersion, "Analyst 1.4", 1, 1, 1),
    a::row(K::WriterVersion, "Analyst 1.5", 1, 1, 1),
    a::row(K::WriterVersion, "Analyst 1.6", 3, 2, 4),
    a::row(K::WriterVersion, "Analyst 1.7", 2, 2, 2),
    a::row(K::WriterVersion, "Analyst QS 1.1", 1, 1, 1),
    a::row(K::WriterVersion, "Analyst TF 1.7", 0, 0, 1),
    a::row(K::WriterVersion, "Analyst TF 1.8", 2, 2, 2),
    a::row(K::WriterVersion, "SCIEX OS 3.4", 1, 1, 1),
];
// END GENERATED sciex-wiff
