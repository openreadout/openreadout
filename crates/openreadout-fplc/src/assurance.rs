//! Assurance profiles (`docs/assurance.md`) of the ÄKTA/UNICORN readers (`.res` results and
//! UNICORN 6/7 result exports): the variant features of a run and the feature values the
//! development corpus validates. The tables between the GENERATED markers are written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static CYTIVA_UNICORN_RES: AssuranceProfile = AssuranceProfile {
    format_id: "cytiva-unicorn-res",
    observe,
    validated: CYTIVA_UNICORN_RES_VALIDATED,
    confidence: CYTIVA_UNICORN_RES_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static CYTIVA_UNICORN_ZIP: AssuranceProfile = AssuranceProfile {
    format_id: "cytiva-unicorn-zip",
    observe,
    validated: CYTIVA_UNICORN_ZIP_VALIDATED,
    confidence: CYTIVA_UNICORN_ZIP_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [Scope::Metadata, Scope::Tables, Scope::Traces];
    if let Some(v) = info
        .format_version
        .as_deref()
        .and_then(|v| a::version_prefix(v, 1))
    {
        o.feature(K::FormatVersion, format!("UNICORN {v}"), &all);
    }
    for t in &info.traces {
        if let Some(k) = a::extra_str(&t.extra, "kind") {
            o.feature(K::Record, format!("curve {k}"), &[Scope::Traces]);
        }
        // curves stored on a volume grid have no time base
        if !t.extra.contains_key("interval_min") {
            o.feature(K::Layout, "volume-grid curve", &[Scope::Traces]);
        }
    }
    // `check` could not read a curve: the curves it lists, if any, hold no confirmed values
    if let Some(note) = a::note_with(info, "the file is damaged (") {
        o.undecoded("curves", &[Scope::Traces], note);
    }
    for t in &info.tables {
        if a::extra_str(&t.extra, "kind") == Some("vendor_peaks") {
            o.feature(K::Record, "UNICORN peak table", &[Scope::Tables]);
        } else if let Some(n) = &t.name {
            o.feature(K::Record, format!("events {n}"), &[Scope::Tables]);
        }
    }
    o
}

// BEGIN GENERATED cytiva-unicorn-res (cargo xtask assurance-audit --write; do not edit)
const CYTIVA_UNICORN_RES_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const CYTIVA_UNICORN_RES_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 2),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 2),
    a::row(K::FormatVersion, "UNICORN 3", 2, 2, 2),
    a::row(K::Record, "curve concentration_b", 2, 2, 2),
    a::row(K::Record, "curve conductivity", 2, 2, 2),
    a::row(K::Record, "curve conductivity_percent", 1, 1, 1),
    a::row(K::Record, "curve flow", 1, 1, 1),
    a::row(K::Record, "curve ph", 1, 1, 1),
    a::row(K::Record, "curve pressure", 2, 2, 2),
    a::row(K::Record, "curve temperature", 2, 2, 2),
    a::row(K::Record, "curve uv", 2, 2, 2),
    a::row(K::Record, "events fractions", 2, 2, 2),
    a::row(K::Record, "events injections", 1, 1, 1),
    a::row(K::Record, "events logbook", 2, 2, 2),
];
// END GENERATED cytiva-unicorn-res

// BEGIN GENERATED cytiva-unicorn-zip (cargo xtask assurance-audit --write; do not edit)
const CYTIVA_UNICORN_ZIP_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const CYTIVA_UNICORN_ZIP_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 10, 4, 10),
    a::row(K::Field, "experiment.instrument.model", 11, 4, 11),
    a::row(K::FormatVersion, "UNICORN 100", 1, 1, 1),
    a::row(K::FormatVersion, "UNICORN 5", 9, 3, 9),
    a::row(K::Layout, "volume-grid curve", 2, 2, 2),
    a::row(K::Record, "UNICORN peak table", 3, 3, 3),
    a::row(K::Record, "curve concentration_b", 9, 3, 9),
    a::row(K::Record, "curve conductivity", 9, 3, 9),
    a::row(K::Record, "curve conductivity_percent", 9, 3, 9),
    a::row(K::Record, "curve flow", 10, 4, 10),
    a::row(K::Record, "curve other", 10, 4, 10),
    a::row(K::Record, "curve ph", 5, 2, 5),
    a::row(K::Record, "curve pressure", 9, 3, 9),
    a::row(K::Record, "curve temperature", 10, 4, 10),
    a::row(K::Record, "curve uv", 10, 4, 10),
    a::row(K::Record, "events fractions", 8, 4, 8),
    a::row(K::Record, "events injections", 11, 4, 11),
    a::row(K::Record, "events logbook", 11, 4, 11),
];
// END GENERATED cytiva-unicorn-zip

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_curves_leave_the_traces_unvalidated() {
        let mut info: FileInfo = serde_json::from_value(serde_json::json!({
            "path": "run.zip",
            "size_bytes": 0,
            "format": {
                "id": "cytiva-unicorn-zip", "name": "", "vendor": "", "extensions": [],
                "family": "chromatography", "can_read": true, "can_write": false,
                "confidence": "medium", "known_gaps": []
            },
            "images": [],
            "plane_count": 0
        }))
        .expect("a minimal FileInfo");
        assert!(observe(&info).undecoded.is_empty());
        info.notes.push(
            "the file is damaged (curve `UV`: `CoordinateData.Amplitudes`: no serialization header); run `check` for the list".into(),
        );
        let o = observe(&info);
        assert_eq!(o.undecoded.len(), 1);
        assert_eq!(o.undecoded[0].scope, vec![Scope::Traces]);
    }
}
