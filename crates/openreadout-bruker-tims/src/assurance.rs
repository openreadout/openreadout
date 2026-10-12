//! Assurance profile (`docs/assurance.md`): the variant features of a Bruker timsTOF `.d`
//! directory (TDF/TSF) and the feature values the development corpus validates, with the
//! state of the m/z and mobility calibrations. The table between the GENERATED markers is
//! written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static BRUKER_TDF: AssuranceProfile = AssuranceProfile {
    format_id: "bruker-tdf",
    observe,
    validated: BRUKER_TDF_VALIDATED,
    confidence: BRUKER_TDF_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for s in &info.spectra {
        // `schema` is the SchemaType + version (`TDF 3.8`), `? ?.?` when GlobalMetadata omits it.
        if let Some(v) = a::extra_str(&s.extra, "schema") {
            o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Spectra]);
        }
        if let Some(k) = a::extra_str(&s.extra, "data_kind") {
            o.feature(K::Layout, k, &[Scope::Spectra]);
        }
        if let Some(c) = a::extra_text(&s.extra, "compression_type") {
            o.feature(
                K::Codec,
                format!("TimsCompressionType {c}"),
                &[Scope::Spectra],
            );
        }
        if let Some(ac) = a::extra_str(&s.extra, "acquisition") {
            o.feature(K::Acquisition, ac, &[Scope::Spectra]);
        }
        // The ion polarity is a variant of the 1/K0 decoding, not only a description of the
        // run: the mobility calibration of one polarity says nothing about the other's.
        let mobility = s.extra.contains_key("mobility_conversion");
        let polarities = a::extra_values_in(&s.extra, "polarities");
        if polarities.iter().any(|p| p == "negative")
            && a::extra_str(&s.extra, "mobility_conversion")
                .is_some_and(|c| c.starts_with("TimsCalibration"))
        {
            o.derived(
                "spectra[].inverse_reduced_mobility",
                "negative-ion voltage magnitude",
                "a negative-ion run stores its ramp voltages negative; the mobility model is applied to their magnitude",
            );
        }
        for p in polarities {
            let value = if mobility {
                format!("{p} ions with ion mobility")
            } else {
                format!("{p} ions")
            };
            o.feature(K::Acquisition, value, &[Scope::Spectra]);
        }
        if let Some(i) = &s.instrument {
            if let Some(m) = &i.model {
                o.context(K::Instrument, m);
            }
            if let Some(v) = i
                .software_version
                .as_deref()
                .and_then(|v| a::version_prefix(v, 1))
            {
                o.context(K::WriterVersion, format!("timsControl {v}"));
            }
        }
        // The reader applies MzCalibration / TimsCalibration per frame (`mz_conversion` /
        // `mobility_conversion` start with the model's name) or falls back to the acquisition-range
        // approximation. The model types are variant features: an unseen one is unvalidated.
        for (key, model_key, name) in [
            ("mz_conversion", "mz_calibration", "MzCalibration"),
            (
                "mobility_conversion",
                "mobility_calibration",
                "TimsCalibration",
            ),
        ] {
            let Some(c) = a::extra_str(&s.extra, key) else {
                continue;
            };
            let what = if name == "MzCalibration" {
                "m/z (MzCalibration)"
            } else {
                "ion mobility (TimsCalibration)"
            };
            if c.starts_with(name) {
                o.calibration(
                    what,
                    CalibrationStatus::Applied,
                    &[Scope::Spectra],
                    c.to_string(),
                );
                if let Some(t) = s
                    .extra
                    .get(model_key)
                    .and_then(|m| m.get("model_type"))
                    .and_then(serde_json::Value::as_i64)
                {
                    o.feature(K::Layout, format!("{name} model {t}"), &[Scope::Spectra]);
                }
            } else {
                o.calibration(
                    what,
                    CalibrationStatus::NotApplied,
                    &[Scope::Spectra],
                    format!(
                        "{c}: values differ from Bruker's calibrated values, typically by tens of ppm (the calibration table is in `info --view full`)"
                    ),
                );
            }
        }
        if s.extra
            .get("closed_properly")
            .and_then(serde_json::Value::as_bool)
            == Some(false)
        {
            o.feature(K::Layout, "not closed properly", &[Scope::Spectra]);
        }
        if s.extra
            .get("wal_commits_applied")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
            > 0
        {
            o.feature(
                K::Layout,
                "uncheckpointed WAL",
                &[Scope::Metadata, Scope::Spectra],
            );
        }
    }
    if let Some(n) = a::note_with(info, "frames of this compression type are not decoded") {
        o.undecoded(
            "frames of an unsupported compression type",
            &[],
            n.to_string(),
        );
    }
    if let Some(n) = a::note_with(info, "m/z values cannot be computed") {
        o.undecoded("m/z conversion parameters", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "PASEF selections name no known precursor") {
        o.undecoded("orphan PASEF selections", &[], n.to_string());
    }
    o
}

pub(crate) static BRUKER_BAF: AssuranceProfile = AssuranceProfile {
    format_id: "bruker-baf",
    observe: observe_baf,
    validated: BRUKER_BAF_VALIDATED,
    confidence: BRUKER_BAF_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe_baf(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for s in &info.spectra {
        if let Some(w) = a::extra_str(&s.extra, "writer") {
            let family = w.split('.').next().unwrap_or(w);
            o.feature(K::FormatVersion, family, &[Scope::Metadata, Scope::Spectra]);
        }
        if let Some(c) = a::extra_str(&s.extra, "profile_coding") {
            o.feature(K::Codec, format!("{c} profile"), &[Scope::Spectra]);
        }
        if let Some(c) = s.extra.get("centroid_element_code").and_then(serde_json::Value::as_u64) {
            o.feature(K::Layout, format!("centroid element code {c}"), &[Scope::Spectra]);
        }
        if let Some(c) = a::extra_str(&s.extra, "calibration") {
            o.feature(K::Layout, format!("calibration {c}"), &[Scope::Spectra]);
        }
        if let Some(src) = a::extra_str(&s.extra, "index_source") {
            o.feature(K::Layout, src, &[Scope::Spectra]);
        }
        for p in a::extra_values_in(&s.extra, "polarities") {
            o.feature(K::Acquisition, format!("{p} ions"), &[Scope::Spectra]);
        }
        if s.ms_levels.iter().any(|&l| l > 1) {
            o.feature(K::Acquisition, "MS/MS", &[Scope::Spectra]);
        }
        match a::extra_str(&s.extra, "mz_calibration_source") {
            Some("Calibrator.ami") => {
                o.calibration(
                    "m/z (Calibrator.ami recalibration)",
                    CalibrationStatus::Applied,
                    &[Scope::Spectra],
                    "the recalibration the vendor's processing software stored next to the run",
                );
                // The payload is found by scanning Calibrator.ami (its container is not
                // decoded): a variant that only spectra compared with vendor exports validate.
                o.feature(
                    K::Layout,
                    "m/z from a Calibrator.ami payload found by scanning",
                    &[Scope::Spectra],
                );
            }
            Some(_) => o.calibration(
                "m/z (acquisition calibration)",
                CalibrationStatus::Applied,
                &[Scope::Spectra],
                "each spectrum's calibration with temperature drift compensation",
            ),
            None => {}
        }
        if let Some(i) = &s.instrument {
            if let Some(m) = &i.model {
                o.context(K::Instrument, m);
            }
            if let Some(v) = i
                .software_version
                .as_deref()
                .and_then(|v| a::version_prefix(v, 1))
            {
                o.context(K::WriterVersion, format!("{} {v}", i.software.as_deref().unwrap_or("software")));
            }
        }
    }
    o
}

// BEGIN GENERATED bruker-baf (cargo xtask assurance-audit --write; do not edit)
const BRUKER_BAF_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const BRUKER_BAF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "MS/MS", 4, 4, 4),
    a::row(K::Acquisition, "negative ions", 3, 3, 3),
    a::row(K::Acquisition, "positive ions", 4, 4, 4),
    a::row(K::Codec, "float32 profile", 2, 2, 2),
    a::row(K::Codec, "packed profile", 5, 5, 5),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 7),
    a::row(K::FormatVersion, "AIDA4", 3, 3, 3),
    a::row(K::FormatVersion, "AIDA5", 2, 2, 2),
    a::row(K::FormatVersion, "AIDA6", 1, 1, 1),
    a::row(K::FormatVersion, "BCC4_3_72_0", 1, 1, 1),
    a::row(K::Instrument, "apex-Qe", 1, 1, 1),
    a::row(K::Instrument, "impact HD", 1, 1, 1),
    a::row(K::Instrument, "impact II", 1, 1, 1),
    a::row(K::Instrument, "maXis", 1, 1, 1),
    a::row(K::Instrument, "micrOTOF-Q", 1, 1, 1),
    a::row(K::Instrument, "solariX", 1, 1, 1),
    a::row(K::Instrument, "timsTOF Pro", 1, 1, 1),
    a::row(K::Layout, "calibration FT-ICR", 2, 2, 2),
    a::row(K::Layout, "calibration time of flight, kind 10", 1, 1, 1),
    a::row(K::Layout, "calibration time of flight, kind 3", 4, 4, 4),
    a::row(K::Layout, "centroid element code 1", 1, 1, 1),
    a::row(K::Layout, "centroid element code 2", 2, 2, 2),
    a::row(K::Layout, "centroid element code 3", 4, 4, 4),
    a::row(K::Layout, "m/z from a Calibrator.ami payload found by scanning", 5, 5, 5),
    a::row(K::Layout, "trailer index", 7, 7, 7),
    a::row(K::WriterVersion, "Bruker otofControl 4", 1, 1, 1),
    a::row(K::WriterVersion, "Bruker otofControl 5", 1, 1, 1),
    a::row(K::WriterVersion, "Bruker otofControl 6", 1, 1, 1),
    a::row(K::WriterVersion, "otofControl 3", 2, 2, 2),
];
// END GENERATED bruker-baf

// BEGIN GENERATED bruker-tdf (cargo xtask assurance-audit --write; do not edit)
const BRUKER_TDF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const BRUKER_TDF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "DDA-PASEF", 7, 6, 7),
    a::row(K::Acquisition, "DIA-PASEF", 3, 3, 3),
    a::row(K::Acquisition, "line spectra (TSF)", 2, 1, 2),
    a::row(K::Acquisition, "negative ions", 1, 1, 1),
    a::row(K::Acquisition, "negative ions with ion mobility", 2, 2, 2),
    a::row(K::Acquisition, "other", 1, 1, 1),
    a::row(K::Acquisition, "positive ions", 1, 1, 1),
    a::row(K::Acquisition, "positive ions with ion mobility", 9, 6, 9),
    a::row(K::Codec, "TimsCompressionType 1", 2, 1, 2),
    a::row(K::Codec, "TimsCompressionType 2", 9, 7, 9),
    a::row(K::Codec, "TimsCompressionType 3", 2, 1, 2),
    a::row(K::Derivation, "spectra[].inverse_reduced_mobility by negative-ion voltage magnitude", 0, 0, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 7, 3, 11),
    a::row(K::Field, "experiment.instrument.model", 7, 3, 11),
    a::row(K::FormatVersion, "? ?.?", 2, 1, 2),
    a::row(K::FormatVersion, "TDF 1.0", 1, 1, 1),
    a::row(K::FormatVersion, "TDF 2.0", 1, 1, 1),
    a::row(K::FormatVersion, "TDF 3.1", 1, 1, 1),
    a::row(K::FormatVersion, "TDF 3.3", 1, 1, 1),
    a::row(K::FormatVersion, "TDF 3.7", 2, 1, 2),
    a::row(K::FormatVersion, "TDF 3.8", 3, 3, 3),
    a::row(K::FormatVersion, "TSF 3.3", 2, 1, 2),
    a::row(K::Instrument, "<unknown>", 1, 1, 1),
    a::row(K::Instrument, "timsTOF", 1, 1, 1),
    a::row(K::Instrument, "timsTOF HT", 2, 2, 2),
    a::row(K::Instrument, "timsTOF Pro", 4, 3, 4),
    a::row(K::Instrument, "timsTOF Pro 2", 1, 1, 1),
    a::row(K::Instrument, "timsTOF fleX", 1, 1, 1),
    a::row(K::Instrument, "timsTOF fleX MALDI 2", 1, 1, 1),
    a::row(K::Layout, "MzCalibration model 1", 9, 5, 9),
    a::row(K::Layout, "MzCalibration model 2", 2, 2, 2),
    a::row(K::Layout, "TimsCalibration model 2", 9, 6, 9),
    a::row(K::Layout, "not closed properly", 1, 1, 1),
    a::row(K::Layout, "tdf", 11, 7, 11),
    a::row(K::Layout, "tsf", 2, 1, 2),
    a::row(K::Layout, "uncheckpointed WAL", 1, 1, 1),
    a::row(K::WriterVersion, "timsControl 3", 4, 2, 4),
    a::row(K::WriterVersion, "timsControl 5", 3, 2, 3),
    a::row(K::WriterVersion, "timsControl 6", 4, 4, 4),
];
// END GENERATED bruker-tdf
