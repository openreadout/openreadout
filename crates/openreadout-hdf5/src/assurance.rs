//! Assurance profiles (`docs/assurance.md`) of the HDF5-based readers (Imaris `.ims`, NWB and
//! generic HDF5): the variant features of a file and the feature values the development corpus
//! validates. The tables between the GENERATED markers are written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static IMS: AssuranceProfile = AssuranceProfile {
    format_id: "ims",
    observe: observe_ims,
    validated: IMS_VALIDATED,
    confidence: IMS_CONFIDENCE,
    basis: Basis::VendorDocs,
};

pub(crate) static NWB: AssuranceProfile = AssuranceProfile {
    format_id: "nwb",
    observe: observe_nwb,
    validated: NWB_VALIDATED,
    confidence: NWB_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static HDF5: AssuranceProfile = AssuranceProfile {
    format_id: "hdf5",
    observe: observe_hdf5,
    validated: HDF5_VALIDATED,
    confidence: HDF5_CONFIDENCE,
    basis: Basis::OpenSpec,
};

/// HDF5 filter ids as codec names (the registered filter numbers).
pub(crate) fn hdf5_filters(ids: &[u16]) -> Observations {
    let mut o = Observations::default();
    if ids.is_empty() {
        o.feature(K::Codec, "hdf5 unfiltered", &[Scope::Pixels]);
    }
    for id in ids {
        let name = match id {
            1 => "hdf5 deflate".to_string(),
            2 => "hdf5 shuffle".to_string(),
            3 => "hdf5 fletcher32".to_string(),
            4 => "hdf5 szip".to_string(),
            5 => "hdf5 nbit".to_string(),
            6 => "hdf5 scale-offset".to_string(),
            32001 => "hdf5 blosc".to_string(),
            32004 => "hdf5 lz4".to_string(),
            32015 => "hdf5 zstd".to_string(),
            n => format!("hdf5 filter {n}"),
        };
        o.feature(K::Codec, name, &[Scope::Pixels]);
    }
    o
}

fn observe_ims(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    a::image_basics(&mut o, info);
    a::writer_context(&mut o, info);
    for im in &info.images {
        if im.pyramid_levels > 1 {
            o.feature(K::Layout, "resolution_levels", &[Scope::Pixels]);
        }
        if let Some(m) = a::extra_str(&im.extra, "microscope_mode") {
            o.context(K::Acquisition, m);
        }
    }
    if let Some(n) = a::note_with(info, "data sets; only DataSet is read") {
        o.undecoded("additional data sets", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "Imaris groups listed, not decoded") {
        o.undecoded("Imaris groups (legacy Scene, events)", &[], n.to_string());
    }
    // Scene8 objects: statistics pivoted per category, record datasets as tables. The object
    // kind (group name without its number) decides which datasets and statistics exist.
    for t in &info.tables {
        let kind = t
            .extra
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let object = t
            .extra
            .get("object")
            .and_then(serde_json::Value::as_str)
            .map_or("", |o| o.trim_end_matches(|c: char| c.is_ascii_digit()));
        match kind {
            "statistics" => o.feature(
                K::Record,
                format!("scene statistics: {object}"),
                &[Scope::Tables],
            ),
            "records" => o.feature(
                K::Record,
                format!("scene records: {object}"),
                &[Scope::Tables],
            ),
            _ => {}
        }
    }
    if let Some(n) = a::note_with(info, "unknown length unit") {
        o.undecoded("length unit", &[], n.to_string());
    }
    o
}

fn observe_nwb(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            v,
            &[Scope::Metadata, Scope::Traces, Scope::Tables],
        );
    }
    for t in &info.traces {
        if let Some(n) = a::extra_str(&t.extra, "neurodata_type") {
            o.feature(K::Record, n, &[Scope::Traces]);
        }
        if let Some(b) = a::extra_str(&t.extra, "time_base") {
            o.feature(K::Layout, format!("time base {b}"), &[Scope::Traces]);
        }
    }
    for t in &info.tables {
        if let Some(n) = a::extra_str(&t.extra, "neurodata_type") {
            o.feature(K::Record, n, &[Scope::Tables]);
        }
    }
    if let Some(n) = a::note_with(
        info,
        "NWB objects listed by `info --view structure`, not decoded",
    ) {
        o.undecoded("NWB objects", &[], n.to_string());
    }
    o
}

fn observe_hdf5(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    // A generic HDF5 file has no convention to validate: every dataset is exposed as stored.
    o.feature(
        K::Dialect,
        "generic hdf5",
        &[Scope::Metadata, Scope::Tables, Scope::Traces],
    );
    if let Some(v) = &info.format_version {
        o.context(K::FormatVersion, v);
    }
    o
}

// BEGIN GENERATED ims (cargo xtask assurance-audit --write; do not edit)
const IMS_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const IMS_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "SpinningDiskConfocal", 5, 3, 5),
    a::row(K::Codec, "hdf5 deflate", 10, 8, 10),
    a::row(K::Codec, "hdf5 lz4", 2, 1, 2),
    a::row(K::Codec, "hdf5 shuffle", 1, 1, 1),
    a::row(K::Codec, "hdf5 unfiltered", 4, 3, 4),
    a::row(K::Field, "experiment.acquisition.started_at", 6, 3, 15),
    a::row(K::FormatVersion, "5.5.0", 16, 11, 16),
    a::row(K::Layout, "resolution_levels", 9, 7, 9),
    a::row(K::Record, "scene records: Cells", 1, 1, 1),
    a::row(K::Record, "scene records: Filaments", 3, 1, 3),
    a::row(K::Record, "scene records: ImageMasks", 1, 1, 1),
    a::row(K::Record, "scene records: MegaSurfaces", 1, 1, 1),
    a::row(K::Record, "scene records: Surfaces", 1, 1, 1),
    a::row(K::Record, "scene statistics: Cells", 1, 1, 1),
    a::row(K::Record, "scene statistics: Filaments", 3, 1, 3),
    a::row(K::Record, "scene statistics: MegaSurfaces", 1, 1, 1),
    a::row(K::Record, "scene statistics: Surfaces", 1, 1, 1),
    a::row(K::SampleLayout, "uint16", 11, 7, 11),
    a::row(K::SampleLayout, "uint8", 5, 5, 5),
    a::row(K::Writer, "Imaris", 16, 11, 16),
    a::row(K::WriterVersion, "Imaris 10.1", 1, 1, 1),
    a::row(K::WriterVersion, "Imaris 10.2", 2, 2, 2),
    a::row(K::WriterVersion, "Imaris 5.5", 5, 3, 5),
    a::row(K::WriterVersion, "Imaris 9.0", 1, 1, 1),
    a::row(K::WriterVersion, "Imaris 9.5", 2, 2, 2),
    a::row(K::WriterVersion, "Imaris 9.7", 1, 1, 1),
    a::row(K::WriterVersion, "Imaris 9.8", 4, 2, 4),
];
// END GENERATED ims

// BEGIN GENERATED nwb (cargo xtask assurance-audit --write; do not edit)
const NWB_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const NWB_VALIDATED: &[Validated] = &[
    a::row(K::FormatVersion, "2.0.2", 1, 1, 1),
    a::row(K::FormatVersion, "2.0b", 1, 1, 1),
    a::row(K::FormatVersion, "2.1.0", 1, 1, 1),
    a::row(K::FormatVersion, "2.2.4", 1, 1, 1),
    a::row(K::FormatVersion, "2.2.5", 3, 3, 3),
    a::row(K::FormatVersion, "2.3.0", 2, 2, 2),
    a::row(K::FormatVersion, "2.4.0", 1, 1, 1),
    a::row(K::FormatVersion, "2.6.0", 1, 1, 1),
    a::row(K::Layout, "time base starting_time + rate", 5, 5, 5),
    a::row(K::Layout, "time base timestamps", 2, 2, 2),
    a::row(K::Layout, "time base uniform timestamps", 1, 1, 1),
    a::row(K::Record, "CurrentClampSeries", 2, 2, 2),
    a::row(K::Record, "CurrentClampStimulusSeries", 2, 2, 2),
    a::row(K::Record, "DynamicTable", 4, 4, 4),
    a::row(K::Record, "ElectricalSeries", 1, 1, 1),
    a::row(K::Record, "IZeroClampSeries", 3, 3, 3),
    a::row(K::Record, "IntracellularElectrodesTable", 1, 1, 1),
    a::row(K::Record, "IntracellularRecordingsTable", 1, 1, 1),
    a::row(K::Record, "IntracellularResponsesTable", 1, 1, 1),
    a::row(K::Record, "IntracellularStimuliTable", 1, 1, 1),
    a::row(K::Record, "SequentialRecordingsTable", 1, 1, 1),
    a::row(K::Record, "SimultaneousRecordingsTable", 1, 1, 1),
    a::row(K::Record, "SpatialSeries", 1, 1, 1),
    a::row(K::Record, "SpikeEventSeries", 1, 1, 1),
    a::row(K::Record, "SweepTable", 3, 3, 3),
    a::row(K::Record, "TimeIntervals", 3, 3, 3),
    a::row(K::Record, "TimeSeries", 3, 3, 3),
    a::row(K::Record, "Units", 3, 3, 3),
    a::row(K::Record, "VectorData", 3, 3, 3),
];
// END GENERATED nwb

// BEGIN GENERATED hdf5 (cargo xtask assurance-audit --write; do not edit)
const HDF5_CONFIDENCE: Confidence = Confidence::Low;
const HDF5_VALIDATED: &[Validated] = &[];
// END GENERATED hdf5
