//! Assurance profile (`docs/assurance.md`): the variant features of an Andor SIF file and the
//! feature values the development corpus validates. The table between the GENERATED markers is
//! written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static ANDOR_SIF: AssuranceProfile = AssuranceProfile {
    format_id: "andor-sif",
    observe,
    validated: ANDOR_SIF_VALIDATED,
    confidence: ANDOR_SIF_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [Scope::Metadata, Scope::Traces, Scope::Pixels];
    // the SIF version decides which header lines exist and where the data starts
    for v in a::extra_values(info, "sif_version") {
        o.feature(K::FormatVersion, v, &all);
    }
    for d in a::extra_values(info, "detector") {
        o.context(K::Instrument, d);
    }
    for b in a::extra_values(info, "block") {
        o.feature(K::Record, format!("{b} block"), &all);
    }
    for t in &info.traces {
        let shape = if t.extra.contains_key("calibration") {
            "spectrum, calibrated axis"
        } else {
            "spectrum, pixel axis"
        };
        o.feature(K::Layout, shape, &[Scope::Traces]);
    }
    for i in &info.images {
        let subs = i
            .extra
            .get("sub_images")
            .and_then(|s| s.as_array())
            .map_or(1, Vec::len);
        let shape = if subs > 1 {
            "image, several sub-images"
        } else {
            "image"
        };
        o.feature(K::Layout, shape, &[Scope::Pixels]);
        let binned = i
            .extra
            .get("sub_images")
            .and_then(|s| s.as_array())
            .is_some_and(|s| {
                s.iter().any(|x| {
                    x.get("x_binning").and_then(serde_json::Value::as_i64) != Some(1)
                        || x.get("y_binning").and_then(serde_json::Value::as_i64) != Some(1)
                })
            });
        if binned {
            o.feature(K::Acquisition, "binned", &[Scope::Pixels]);
        }
    }
    o
}

// BEGIN GENERATED andor-sif (cargo xtask assurance-audit --write; do not edit)
const ANDOR_SIF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const ANDOR_SIF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "binned", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 5, 2, 20),
    a::row(K::FormatVersion, "65555", 1, 1, 1),
    a::row(K::FormatVersion, "65559", 1, 1, 1),
    a::row(K::FormatVersion, "65564", 5, 3, 5),
    a::row(K::FormatVersion, "65567", 14, 5, 14),
    a::row(K::Instrument, "DH334T-18F-03", 1, 1, 1),
    a::row(K::Instrument, "DH334T-18F-63", 1, 1, 1),
    a::row(K::Instrument, "DH734_18mm", 1, 1, 1),
    a::row(K::Instrument, "DU401_BVF", 1, 1, 1),
    a::row(K::Instrument, "DU420_BX2DD", 1, 1, 1),
    a::row(K::Instrument, "DU420_OE", 1, 1, 1),
    a::row(K::Instrument, "DU491_17", 1, 1, 1),
    a::row(K::Instrument, "DU888_BV", 2, 1, 2),
    a::row(K::Instrument, "DU897_BV", 2, 1, 2),
    a::row(K::Instrument, "DU920P_BX2DD", 4, 2, 4),
    a::row(K::Instrument, "DU934P_BR,DD", 1, 1, 1),
    a::row(K::Instrument, "DU940P", 3, 1, 3),
    a::row(K::Instrument, "DV420_BU2", 1, 1, 1),
    a::row(K::Instrument, "ZYLA-4.2P-USB3", 1, 1, 1),
    a::row(K::Layout, "image", 9, 4, 9),
    a::row(K::Layout, "spectrum, calibrated axis", 12, 4, 12),
    a::row(K::Record, "background block", 5, 3, 5),
    a::row(K::Record, "live block", 2, 1, 2),
    a::row(K::Record, "signal block", 21, 7, 21),
];
// END GENERATED andor-sif
