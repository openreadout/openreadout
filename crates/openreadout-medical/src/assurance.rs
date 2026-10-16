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
    validated: BRUKER_PARAVISION_VALIDATED,
    confidence: BRUKER_PARAVISION_CONFIDENCE,
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
    a::row(K::Acquisition, "CR", 2, 2, 2),
    a::row(K::Acquisition, "CT", 7, 4, 7),
    a::row(K::Acquisition, "DX", 1, 1, 1),
    a::row(K::Acquisition, "MR", 18, 4, 18),
    a::row(K::Acquisition, "NM", 4, 3, 4),
    a::row(K::Acquisition, "OT", 9, 2, 9),
    a::row(K::Acquisition, "PT", 1, 1, 1),
    a::row(K::Acquisition, "RF", 1, 1, 1),
    a::row(K::Acquisition, "RTDOSE", 2, 1, 2),
    a::row(K::Acquisition, "SEG", 5, 3, 5),
    a::row(K::Acquisition, "SM", 2, 1, 2),
    a::row(K::Acquisition, "US", 10, 3, 10),
    a::row(K::Codec, "Deflated Explicit VR Little Endian", 1, 1, 1),
    a::row(K::Codec, "Deflated Image Frame Compression", 1, 1, 1),
    a::row(K::Codec, "Explicit VR Big Endian", 3, 2, 3),
    a::row(K::Codec, "Explicit VR Little Endian", 28, 5, 28),
    a::row(K::Codec, "High-Throughput JPEG 2000 Image Compression", 1, 1, 1),
    a::row(K::Codec, "High-Throughput JPEG 2000 Image Compression (Lossless Only)", 1, 1, 1),
    a::row(K::Codec, "Implicit VR Little Endian", 5, 3, 5),
    a::row(K::Codec, "JPEG 2000 Image Compression", 5, 3, 5),
    a::row(K::Codec, "JPEG 2000 Image Compression (Lossless Only)", 4, 2, 4),
    a::row(K::Codec, "JPEG Baseline (Process 1)", 4, 3, 4),
    a::row(K::Codec, "JPEG Extended (Process 2 & 4)", 1, 1, 1),
    a::row(K::Codec, "JPEG Lossless, Non-Hierarchical, First-Order Prediction (Process 14 [Selection Value 1])", 3, 1, 3),
    a::row(K::Codec, "JPEG-LS Lossless Image Compression", 2, 2, 2),
    a::row(K::Codec, "JPEG-LS Lossy (Near-Lossless) Image Compression", 2, 1, 2),
    a::row(K::Codec, "RLE Lossless", 6, 2, 6),
    a::row(K::Instrument, "ADC_5146", 1, 1, 1),
    a::row(K::Instrument, "Aperio converted by com.pixelmed.convert.TIFFToDicom", 1, 1, 1),
    a::row(K::Instrument, "Aquilion PRIME", 1, 1, 1),
    a::row(K::Instrument, "Avanto", 2, 2, 2),
    a::row(K::Instrument, "CX50", 3, 2, 3),
    a::row(K::Instrument, "Cassette Holder Type 9840 500 70201", 1, 1, 1),
    a::row(K::Instrument, "Discovery STE", 1, 1, 1),
    a::row(K::Instrument, "EPIQ 7C", 1, 1, 1),
    a::row(K::Instrument, "Encore2", 1, 1, 1),
    a::row(K::Instrument, "Espree", 1, 1, 1),
    a::row(K::Instrument, "Fluorospot Compact FD", 1, 1, 1),
    a::row(K::Instrument, "GT450", 1, 1, 1),
    a::row(K::Instrument, "LOGIQ 700", 2, 2, 2),
    a::row(K::Instrument, "MILLENNIUM MG", 3, 2, 3),
    a::row(K::Instrument, "MRT50H1", 7, 1, 7),
    a::row(K::Instrument, "MyModel", 2, 1, 2),
    a::row(K::Instrument, "RHAPSODE", 1, 1, 1),
    a::row(K::Instrument, "Revolution XQi ADS_28.2", 1, 1, 1),
    a::row(K::Instrument, "SOMATOM Definition AS+", 1, 1, 1),
    a::row(K::Instrument, "SSD-4000", 1, 1, 1),
    a::row(K::Instrument, "Sensation 64", 1, 1, 1),
    a::row(K::Instrument, "Super Dooper Scanner", 1, 1, 1),
    a::row(K::Instrument, "T5", 2, 1, 2),
    a::row(K::Instrument, "Treatment Planning System name here", 2, 1, 2),
    a::row(K::Instrument, "Turbo", 2, 2, 2),
    a::row(K::Instrument, "com.pixelmed.convert.NRRDToDicom", 1, 1, 1),
    a::row(K::Instrument, "git@github.com:QIICR/dcmqi.git", 1, 1, 1),
    a::row(K::Instrument, "https://github.com/fedorov/dcmqi.git", 4, 2, 4),
    a::row(K::Layout, "cine", 4, 3, 4),
    a::row(K::Layout, "segmentation", 5, 3, 5),
    a::row(K::Layout, "single frame", 41, 5, 41),
    a::row(K::Layout, "slide pyramid", 2, 1, 2),
    a::row(K::Layout, "stack", 7, 2, 7),
    a::row(K::Layout, "volume", 9, 4, 9),
    a::row(K::Record, "CT Image Storage", 6, 4, 6),
    a::row(K::Record, "Computed Radiography Image Storage", 2, 2, 2),
    a::row(K::Record, "Digital X-Ray Image Storage - For Presentation", 1, 1, 1),
    a::row(K::Record, "Enhanced CT Image Storage", 1, 1, 1),
    a::row(K::Record, "Enhanced MR Image Storage", 5, 1, 5),
    a::row(K::Record, "MR Image Storage", 13, 4, 13),
    a::row(K::Record, "Multi-frame Grayscale Byte Secondary Capture Image Storage", 1, 1, 1),
    a::row(K::Record, "Nuclear Medicine Image Storage", 1, 1, 1),
    a::row(K::Record, "Parametric Map Storage", 2, 1, 2),
    a::row(K::Record, "Positron Emission Tomography Image Storage", 1, 1, 1),
    a::row(K::Record, "RT Dose Storage", 2, 1, 2),
    a::row(K::Record, "Secondary Capture Image Storage", 15, 2, 15),
    a::row(K::Record, "Segmentation Storage", 5, 3, 5),
    a::row(K::Record, "Ultrasound Image Storage", 6, 2, 6),
    a::row(K::Record, "Ultrasound Multi-frame Image Storage", 3, 2, 3),
    a::row(K::Record, "VL Whole Slide Microscopy Image Storage", 2, 1, 2),
    a::row(K::Record, "X-Ray Radiofluoroscopic Image Storage", 1, 1, 1),
    a::row(K::SampleLayout, "MONOCHROME1", 2, 2, 2),
    a::row(K::SampleLayout, "MONOCHROME2", 46, 4, 46),
    a::row(K::SampleLayout, "PALETTE COLOR", 4, 2, 4),
    a::row(K::SampleLayout, "RGB", 10, 3, 10),
    a::row(K::SampleLayout, "YBR_FULL_422", 4, 3, 4),
    a::row(K::SampleLayout, "YBR_ICT", 2, 2, 2),
    a::row(K::SampleLayout, "double", 1, 1, 1),
    a::row(K::SampleLayout, "float", 5, 3, 5),
    a::row(K::SampleLayout, "int16", 13, 4, 13),
    a::row(K::SampleLayout, "int32", 4, 3, 4),
    a::row(K::SampleLayout, "uint16", 19, 3, 19),
    a::row(K::SampleLayout, "uint32", 3, 2, 3),
    a::row(K::SampleLayout, "uint8", 22, 4, 22),
];
// END GENERATED dicom
// BEGIN GENERATED nifti (cargo xtask assurance-audit --write; do not edit)
const NIFTI_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const NIFTI_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "gzip", 5, 2, 5),
    a::row(K::FormatVersion, "NIfTI-1", 7, 2, 7),
    a::row(K::FormatVersion, "NIfTI-2", 1, 1, 1),
    a::row(K::Layout, "big-endian", 2, 1, 2),
    a::row(K::Layout, "little-endian", 6, 2, 6),
    a::row(K::SampleLayout, "float32", 1, 1, 1),
    a::row(K::SampleLayout, "int16", 6, 2, 6),
    a::row(K::SampleLayout, "uint8", 1, 1, 1),
];
// END GENERATED nifti
// BEGIN GENERATED bruker-paravision (cargo xtask assurance-audit --write; do not edit)
const BRUKER_PARAVISION_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const BRUKER_PARAVISION_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "Bruker:B1Map", 2, 1, 2),
    a::row(K::Acquisition, "Bruker:CSI", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:DESS", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:DtiEpi", 2, 1, 2),
    a::row(K::Acquisition, "Bruker:DtiSpiral", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:DtiStandard", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:EPI", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:EPSI", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:FAIR_EPI", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:FAIR_RARE", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:FISP", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:FLASH", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:FLOWMAP", 2, 1, 2),
    a::row(K::Acquisition, "Bruker:FcFLASH", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:FieldMap", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:IgFLASH", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:MDEFT", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:MGE", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:MSME", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:RARE", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:RAREVTR", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:RAREst", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:SPIRAL", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:SegFLASH", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:T1_EPI", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:T2S_EPI", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:T2_EPI", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:UTE", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:UTE3D", 1, 1, 1),
    a::row(K::Acquisition, "Bruker:ZTE", 1, 1, 1),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 4),
    a::row(K::FormatVersion, "ParaVision 7.0.0", 5, 1, 5),
    a::row(K::Layout, "FG_CARDIAC_MOVIE", 1, 1, 1),
    a::row(K::Layout, "FG_COMPLEX", 1, 1, 1),
    a::row(K::Layout, "FG_CYCLE", 1, 1, 1),
    a::row(K::Layout, "FG_DIFFUSION", 2, 1, 2),
    a::row(K::Layout, "FG_ECHO", 2, 1, 2),
    a::row(K::Layout, "FG_FLOW", 2, 1, 2),
    a::row(K::Layout, "FG_IRMODE", 1, 1, 1),
    a::row(K::Layout, "FG_MOVIE", 1, 1, 1),
    a::row(K::Layout, "FG_SLICE", 2, 1, 2),
    a::row(K::Layout, "frames", 1, 1, 1),
    a::row(K::SampleLayout, "_16BIT_SGN_INT", 3, 1, 3),
    a::row(K::SampleLayout, "_32BIT_SGN_INT", 2, 1, 2),
];
// END GENERATED bruker-paravision
