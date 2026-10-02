//! Assurance profiles (`docs/assurance.md`) of the electron-microscopy readers (MRC, Gatan DM,
//! TIA SER, Velox EMD): the variant features of a file and the feature values the development
//! corpus validates. The tables between the GENERATED markers are written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static MRC: AssuranceProfile = AssuranceProfile {
    format_id: "mrc",
    observe: observe_mrc,
    validated: MRC_VALIDATED,
    confidence: MRC_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static DM: AssuranceProfile = AssuranceProfile {
    format_id: "dm",
    observe: observe_dm,
    validated: DM_VALIDATED,
    confidence: DM_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static SER: AssuranceProfile = AssuranceProfile {
    format_id: "ser",
    observe: observe_ser,
    validated: SER_VALIDATED,
    confidence: SER_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static EMD: AssuranceProfile = AssuranceProfile {
    format_id: "emd",
    observe: observe_emd,
    validated: EMD_VALIDATED,
    confidence: EMD_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn truncated(o: &mut Observations, info: &FileInfo) {
    if info
        .notes
        .iter()
        .any(|n| n.contains("appears truncated") || n.contains("could not be read to the end"))
    {
        o.undecoded(
            "truncated data",
            &[Scope::Pixels],
            "the file ends before the data its header describes; `check` gives the extent",
        );
    }
}

fn observe_mrc(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let version = info
        .format_version
        .as_deref()
        .unwrap_or("pre-2014 (no NVERSION)");
    o.feature(K::FormatVersion, version, &[Scope::Metadata, Scope::Pixels]);
    for im in &info.images {
        o.feature(K::SampleLayout, im.pixel_type.ome_name(), &[Scope::Pixels]);
        if let Some(m) = a::extra_text(&im.extra, "mode") {
            o.feature(K::SampleLayout, format!("mode {m}"), &[Scope::Pixels]);
        }
        if let Some(l) = a::extra_str(&im.extra, "layout") {
            o.feature(K::Layout, l, &[Scope::Pixels, Scope::Metadata]);
        }
        if let Some(t) = im
            .extra
            .get("extended_header")
            .and_then(|e| e.get("type"))
            .and_then(serde_json::Value::as_str)
            .filter(|t| !t.is_empty())
        {
            o.feature(
                K::Record,
                format!("extended header {t}"),
                &[Scope::Metadata],
            );
        }
    }
    a::writer_context(&mut o, info);
    if a::has_note(info, "MAPC/MAPR/MAPS = ") {
        o.feature(K::Layout, "permuted axes", &[Scope::Pixels]);
    }
    if a::has_note(info, "gzip-compressed: ") {
        o.feature(K::Codec, "gzip", &[Scope::Pixels, Scope::Metadata]);
    }
    if a::has_note(info, "mode 0 bytes are unsigned") {
        o.feature(K::SampleLayout, "unsigned mode 0 (IMOD)", &[Scope::Pixels]);
    }
    if let Some(n) = a::note_with(info, "pixel size is exactly 1 Å") {
        o.assumed("images[].physical_size", n.to_string());
    }
    truncated(&mut o, info);
    o
}

fn observe_dm(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Pixels]);
    }
    for im in &info.images {
        if let Some(d) = a::extra_str(&im.extra, "data_type") {
            o.feature(K::SampleLayout, d, &[Scope::Pixels]);
        }
        if let Some(dims) = im
            .extra
            .get("dimensions")
            .and_then(serde_json::Value::as_array)
        {
            o.feature(K::Layout, format!("{}-D", dims.len()), &[Scope::Pixels]);
        }
        if let Some(m) = im
            .extra
            .get("microscope")
            .and_then(|m| m.get("operation_mode"))
            .and_then(serde_json::Value::as_str)
        {
            o.context(K::Acquisition, m);
        }
    }
    a::writer_context(&mut o, info);
    a::instrument_context(&mut o, info);
    for n in info
        .notes
        .iter()
        .filter(|n| n.contains("described but not decoded"))
    {
        o.undecoded("unsupported DataType", &[], n.clone());
    }
    if let Some(n) = a::note_with(info, "axes after the second are flattened into T") {
        o.assumed("images[].size_t", n.to_string());
    }
    truncated(&mut o, info);
    o
}

fn observe_ser(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Pixels]);
    }
    for im in &info.images {
        if let Some(d) = a::extra_str(&im.extra, "element_data_type") {
            o.feature(K::SampleLayout, d, &[Scope::Pixels]);
        }
        if let Some(k) = a::extra_str(&im.extra, "element_kind") {
            o.feature(K::Layout, format!("{k} elements"), &[Scope::Pixels]);
        }
    }
    a::writer_context(&mut o, info);
    a::instrument_context(&mut o, info);
    if a::has_note(info, "no .emi sidecar found") {
        o.feature(K::Layout, "without .emi", &[Scope::Metadata]);
    } else {
        o.feature(K::Layout, "with .emi", &[Scope::Metadata]);
    }
    if let Some(n) = a::note_with(info, "taken as metres")
        && info.images.iter().any(|i| i.physical_size.x.is_some())
    {
        o.assumed("images[].physical_size", n.to_string());
    }
    if let Some(n) = a::note_with(info, "complex element data is described but not decoded") {
        o.undecoded("complex elements", &[], n.to_string());
    }
    o
}

fn observe_emd(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Pixels]);
    }
    for im in &info.images {
        o.feature(K::SampleLayout, im.pixel_type.ome_name(), &[Scope::Pixels]);
        if let Some(d) = im.instrument.as_ref().and_then(|i| i.detector.as_deref()) {
            o.context(K::Record, format!("detector {d}"));
        }
    }
    a::writer_context(&mut o, info);
    a::instrument_context(&mut o, info);
    if a::has_note(info, "multi-frame Velox images") {
        o.feature(K::Layout, "multi-frame", &[Scope::Pixels]);
    }
    if a::has_note(info, "EDS spectrum images are assembled") {
        o.feature(K::Layout, "eds spectrum image", &[Scope::Pixels]);
        o.feature(K::Codec, "event stream uint16", &[Scope::Pixels]);
    }
    if !info.traces.is_empty() {
        o.feature(K::Record, "eds spectra", &[Scope::Traces]);
    }
    if a::has_note(info, "EELS spectrum images (Data/EelsSpectrumImage)") {
        o.feature(
            K::Layout,
            "eels spectrum image",
            &[Scope::Pixels, Scope::Metadata],
        );
    }
    if let Some(n) = a::note_with(
        info,
        "Velox data other than images is listed but not decoded",
    ) {
        o.undecoded(
            "Velox data other than images and EDS spectra",
            &[],
            n.to_string(),
        );
    }
    truncated(&mut o, info);
    o
}

// BEGIN GENERATED mrc (cargo xtask assurance-audit --write; do not edit)
const MRC_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const MRC_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "gzip", 4, 2, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 2),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 2),
    a::row(K::FormatVersion, "20140", 2, 1, 2),
    a::row(K::FormatVersion, "pre-2014 (no NVERSION)", 11, 7, 11),
    a::row(K::Layout, "image-stack", 6, 5, 6),
    a::row(K::Layout, "permuted axes", 2, 1, 2),
    a::row(K::Layout, "volume", 7, 3, 7),
    a::row(K::Record, "extended header FEI1", 1, 1, 1),
    a::row(K::Record, "extended header FEI2", 1, 1, 1),
    a::row(K::SampleLayout, "float", 9, 6, 9),
    a::row(K::SampleLayout, "int16", 1, 1, 1),
    a::row(K::SampleLayout, "int8", 1, 1, 1),
    a::row(K::SampleLayout, "mode 0", 1, 1, 1),
    a::row(K::SampleLayout, "mode 1", 1, 1, 1),
    a::row(K::SampleLayout, "mode 2", 9, 6, 9),
    a::row(K::SampleLayout, "mode 6", 2, 1, 2),
    a::row(K::SampleLayout, "uint16", 2, 1, 2),
    a::row(K::Writer, "EPU", 1, 1, 1),
    a::row(K::Writer, "Fei EPU", 1, 1, 1),
    a::row(K::WriterVersion, "EPU 2.9", 1, 1, 1),
    a::row(K::WriterVersion, "Fei EPU 1.2", 1, 1, 1),
];
// END GENERATED mrc

// BEGIN GENERATED dm (cargo xtask assurance-audit --write; do not edit)
const DM_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const DM_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "DIFFRACTION", 3, 3, 3),
    a::row(K::Acquisition, "GIF SCANNING", 2, 2, 2),
    a::row(K::Acquisition, "IMAGING", 3, 3, 3),
    a::row(K::Acquisition, "SCANNING", 12, 5, 12),
    a::row(K::Acquisition, "STEM", 2, 1, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 10),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 20),
    a::row(K::FormatVersion, "DM3", 40, 8, 40),
    a::row(K::FormatVersion, "DM4", 41, 10, 41),
    a::row(K::FormatVersion, "DM5", 8, 2, 8),
    a::row(K::Instrument, "FEI Tecnai", 3, 2, 3),
    a::row(K::Instrument, "FEI Tecnai Remote", 6, 1, 6),
    a::row(K::Instrument, "FEI Tecnai Remote TCPIP", 2, 2, 2),
    a::row(K::Instrument, "JEOL COM", 3, 3, 3),
    a::row(K::Instrument, "NCEM TEAM 0.5", 1, 1, 1),
    a::row(K::Instrument, "TitanX", 2, 2, 2),
    a::row(K::Instrument, "Unknown", 2, 2, 2),
    a::row(K::Instrument, "Zeiss SEM COM", 3, 1, 3),
    a::row(K::Layout, "1-D", 13, 4, 13),
    a::row(K::Layout, "2-D", 49, 13, 49),
    a::row(K::Layout, "3-D", 29, 8, 29),
    a::row(K::SampleLayout, "binary", 2, 1, 2),
    a::row(K::SampleLayout, "complex128", 4, 1, 4),
    a::row(K::SampleLayout, "complex64", 4, 1, 4),
    a::row(K::SampleLayout, "float32", 49, 11, 49),
    a::row(K::SampleLayout, "float64", 2, 1, 2),
    a::row(K::SampleLayout, "int16", 4, 2, 4),
    a::row(K::SampleLayout, "int32", 5, 3, 5),
    a::row(K::SampleLayout, "int8", 2, 1, 2),
    a::row(K::SampleLayout, "packed-complex64", 1, 1, 1),
    a::row(K::SampleLayout, "rgba", 4, 1, 4),
    a::row(K::SampleLayout, "uint16", 6, 3, 6),
    a::row(K::SampleLayout, "uint32", 6, 2, 6),
    a::row(K::SampleLayout, "uint8", 2, 1, 2),
    a::row(K::Writer, "Gatan DigitalMicrograph", 89, 15, 89),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 2.31", 6, 3, 6),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 2.32", 4, 2, 4),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 3.0", 2, 1, 2),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 3.20", 2, 1, 2),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 3.21", 1, 1, 1),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 3.43", 1, 1, 1),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 3.44", 1, 1, 1),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 3.50", 1, 1, 1),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 3.51", 1, 1, 1),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 3.53", 2, 2, 2),
    a::row(K::WriterVersion, "Gatan DigitalMicrograph 3.60", 1, 1, 1),
];
// END GENERATED dm

// BEGIN GENERATED ser (cargo xtask assurance-audit --write; do not edit)
const SER_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const SER_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.instrument.model", 0, 0, 18),
    a::row(K::FormatVersion, "0x0210", 15, 3, 15),
    a::row(K::FormatVersion, "0x0220", 6, 2, 6),
    a::row(K::Instrument, "Microscope TalosF200X 200 kV D6308 XTwin", 4, 1, 4),
    a::row(K::Instrument, "Microscope Tecnai 200 kV D2267 SuperTwin", 3, 1, 3),
    a::row(K::Instrument, "Microscope Tecnai 300 kV D248 SuperTwin", 5, 1, 5),
    a::row(K::Instrument, "Microscope TecnaiOsiris 200 kV D675 AnalyticalTwin", 2, 1, 2),
    a::row(K::Instrument, "Microscope Titan 300 kV D3165 SuperTwin", 1, 1, 1),
    a::row(K::Instrument, "Microscope TitanCubed 300 kV D3128 UltraTwin", 2, 2, 2),
    a::row(K::Instrument, "TitanCubed D3147 S-Twin", 1, 1, 1),
    a::row(K::Layout, "1d elements", 9, 1, 9),
    a::row(K::Layout, "2d elements", 12, 4, 12),
    a::row(K::Layout, "with .emi", 19, 3, 19),
    a::row(K::Layout, "without .emi", 2, 2, 2),
    a::row(K::SampleLayout, "float32", 1, 1, 1),
    a::row(K::SampleLayout, "int32", 8, 1, 8),
    a::row(K::SampleLayout, "uint16", 9, 4, 9),
    a::row(K::SampleLayout, "uint32", 3, 1, 3),
    a::row(K::Writer, "TIA / ES Vision", 19, 3, 19),
];
// END GENERATED ser

// BEGIN GENERATED emd (cargo xtask assurance-audit --write; do not edit)
const EMD_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const EMD_VALIDATED: &[Validated] = &[
    a::row(K::Codec, "event stream uint16", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 17),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 17),
    a::row(K::FormatVersion, "Berkeley EMD", 4, 1, 4),
    a::row(K::FormatVersion, "Berkeley EMD 0.2", 5, 2, 5),
    a::row(K::FormatVersion, "Berkeley EMD null.null", 4, 1, 4),
    a::row(K::FormatVersion, "Velox 10", 5, 3, 5),
    a::row(K::FormatVersion, "Velox 11", 4, 3, 4),
    a::row(K::FormatVersion, "Velox 7", 2, 1, 2),
    a::row(K::FormatVersion, "Velox 8", 5, 2, 5),
    a::row(K::FormatVersion, "Velox 9", 3, 1, 3),
    a::row(K::Instrument, "Spectra", 9, 7, 9),
    a::row(K::Instrument, "Titan", 8, 3, 8),
    a::row(K::Layout, "eds spectrum image", 1, 1, 1),
    a::row(K::Layout, "eels spectrum image", 1, 1, 1),
    a::row(K::Layout, "multi-frame", 2, 2, 2),
    a::row(K::Record, "detector BF", 1, 1, 1),
    a::row(K::Record, "detector BF-S", 1, 1, 1),
    a::row(K::Record, "detector BM-Ceta", 5, 3, 5),
    a::row(K::Record, "detector DF-S", 1, 1, 1),
    a::row(K::Record, "detector DF2", 1, 1, 1),
    a::row(K::Record, "detector DF4", 2, 2, 2),
    a::row(K::Record, "detector EELS Strip Detector", 1, 1, 1),
    a::row(K::Record, "detector HAADF", 10, 8, 10),
    a::row(K::Record, "detector SuperXG2", 1, 1, 1),
    a::row(K::Record, "detector SuperXG23 + SuperXG21 + SuperXG24 + SuperXG22", 1, 1, 1),
    a::row(K::Record, "eds spectra", 3, 2, 3),
    a::row(K::SampleLayout, "complex", 4, 1, 4),
    a::row(K::SampleLayout, "double", 1, 1, 1),
    a::row(K::SampleLayout, "float", 7, 3, 7),
    a::row(K::SampleLayout, "int16", 4, 3, 4),
    a::row(K::SampleLayout, "int32", 5, 2, 5),
    a::row(K::SampleLayout, "int64", 2, 2, 2),
    a::row(K::SampleLayout, "uint16", 13, 9, 13),
    a::row(K::SampleLayout, "uint32", 1, 1, 1),
    a::row(K::Writer, "Velox", 17, 9, 17),
    a::row(K::WriterVersion, "Velox 2.11", 2, 1, 2),
    a::row(K::WriterVersion, "Velox 2.15", 3, 2, 3),
    a::row(K::WriterVersion, "Velox 2.9", 3, 1, 3),
    a::row(K::WriterVersion, "Velox 3.12", 2, 1, 2),
    a::row(K::WriterVersion, "Velox 3.17", 5, 4, 5),
    a::row(K::WriterVersion, "Velox 3.21", 1, 1, 1),
    a::row(K::WriterVersion, "Velox 3.24", 1, 1, 1),
];
// END GENERATED emd
