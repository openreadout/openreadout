//! Assurance profiles (`docs/assurance.md`) of the medical-imaging readers (DICOM, NIfTI /
//! Analyze, Bruker ParaVision): the variant features of a file and the feature values the
//! development corpus validates. The tables between the GENERATED markers are written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, Calibration, CalibrationStatus, FeatureKind as K,
    Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static DICOM: AssuranceProfile = AssuranceProfile {
    format_id: "dicom",
    observe: observe_dicom,
    validated: DICOM_VALIDATED,
    confidence: DICOM_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static NIFTI: AssuranceProfile = AssuranceProfile {
    format_id: "nifti",
    observe: observe_nifti,
    validated: NIFTI_VALIDATED,
    confidence: NIFTI_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static PARAVISION: AssuranceProfile = AssuranceProfile {
    format_id: "bruker-paravision",
    observe: observe_paravision,
    validated: PARAVISION_VALIDATED,
    confidence: PARAVISION_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn rescale(o: &mut Observations, info: &FileInfo, what: &str, detail: &str) {
    if info.images.iter().any(|i| i.extra.contains_key("rescale")) {
        o.calibrations.push(Calibration {
            name: what.into(),
            status: CalibrationStatus::Applied,
            scope: vec![Scope::Pixels],
            detail: detail.into(),
        });
    }
}

fn observe_dicom(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for im in &info.images {
        if let Some(ts) = im
            .extra
            .get("transfer_syntax")
            .and_then(|t| t.get("name"))
            .and_then(serde_json::Value::as_str)
        {
            o.feature(K::Codec, ts, &[Scope::Pixels]);
        }
        if let Some(m) = a::extra_str(&im.extra, "modality") {
            o.context(K::Acquisition, m);
        }
        if let Some(c) = a::extra_str(&im.extra, "sop_class") {
            o.feature(K::Record, c, &[Scope::Metadata, Scope::Pixels]);
        }
        if let Some(l) = a::extra_str(&im.extra, "arrangement") {
            o.feature(K::Layout, l, &[Scope::Pixels]);
        }
        if let Some(p) = a::extra_str(&im.extra, "photometric_interpretation") {
            o.feature(K::SampleLayout, p, &[Scope::Pixels]);
        }
        o.feature(K::SampleLayout, im.pixel_type.ome_name(), &[Scope::Pixels]);
    }
    a::instrument_context(&mut o, info);
    rescale(
        &mut o,
        info,
        "modality rescale (Rescale Slope and Intercept, or Modality LUT)",
        "applied to every plane: values are in output units (Hounsfield units for CT); extra.stored_pixel_type gives the stored type",
    );
    if let Some(n) = a::note_with(info, "slice spacing is irregular") {
        o.assumed("images[].physical_size.z", n.to_string());
    }
    if info
        .images
        .iter()
        .any(|i| a::extra_str(&i.extra, "z_step_from") == Some("SliceThickness"))
    {
        o.assumed(
            "images[].physical_size.z",
            "no slice positions or spacing: the Z step is the slice thickness",
        );
    }
    if a::has_note(info, "palette colour: ") {
        o.undecoded("segmented palette", &[Scope::Pixels], "palette indices are returned");
    }
    o
}

fn observe_nifti(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Pixels]);
    }
    for im in &info.images {
        if let Some(d) = a::extra_str(&im.extra, "datatype") {
            o.feature(K::SampleLayout, d, &[Scope::Pixels]);
        }
        if let Some(d) = a::extra_str(&im.extra, "byte_order") {
            o.feature(K::Layout, d, &[Scope::Pixels]);
        }
    }
    if a::has_note(info, "gzip-compressed") {
        o.feature(K::Codec, "gzip", &[Scope::Pixels, Scope::Metadata]);
    }
    rescale(
        &mut o,
        info,
        "scl_slope and scl_inter",
        "applied to every plane: values are float32 stored x scl_slope + scl_inter",
    );
    o
}

fn observe_paravision(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Pixels]);
    }
    for im in &info.images {
        if let Some(d) = a::extra_str(&im.extra, "word_type") {
            o.feature(K::SampleLayout, d, &[Scope::Pixels]);
        }
        for g in a::extra_values_in(&im.extra, "frame_groups") {
            o.feature(K::Layout, g, &[Scope::Pixels]);
        }
        if let Some(m) = a::extra_str(&im.extra, "method") {
            o.context(K::Acquisition, m);
        }
    }
    rescale(
        &mut o,
        info,
        "VisuCoreDataSlope and VisuCoreDataOffs",
        "applied to every frame: values are float32 stored x slope + offset",
    );
    o
}

// BEGIN GENERATED dicom (cargo xtask assurance-audit --write; do not edit)
const DICOM_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const DICOM_VALIDATED: &[Validated] = &[
];
// END GENERATED dicom
// BEGIN GENERATED nifti (cargo xtask assurance-audit --write; do not edit)
const NIFTI_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const NIFTI_VALIDATED: &[Validated] = &[
];
// END GENERATED nifti
// BEGIN GENERATED bruker-paravision (cargo xtask assurance-audit --write; do not edit)
const PARAVISION_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const PARAVISION_VALIDATED: &[Validated] = &[
];
// END GENERATED bruker-paravision
