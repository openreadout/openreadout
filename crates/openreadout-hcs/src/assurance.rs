//! Assurance profiles (`docs/assurance.md`) of the screening-plate readers (Opera/Operetta
//! Harmony and Columbus, ImageXpress/MetaXpress, CellVoyager): the variant features of a plate
//! and the feature values the development corpus validates. The tables between the GENERATED
//! markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static OPERA_HARMONY: AssuranceProfile = AssuranceProfile {
    format_id: "opera-harmony",
    observe,
    validated: OPERA_HARMONY_VALIDATED,
    confidence: OPERA_HARMONY_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static IMAGEXPRESS: AssuranceProfile = AssuranceProfile {
    format_id: "imagexpress",
    observe,
    validated: IMAGEXPRESS_VALIDATED,
    confidence: IMAGEXPRESS_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static CELLVOYAGER: AssuranceProfile = AssuranceProfile {
    format_id: "cellvoyager",
    observe,
    validated: CELLVOYAGER_VALIDATED,
    confidence: CELLVOYAGER_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    // The index generation decides how wells, fields, channels and plane files are described.
    let version = info.format_version.as_deref().unwrap_or("no index version");
    o.feature(K::FormatVersion, version, &[Scope::Metadata, Scope::Pixels]);
    // Every field image of a plate is alike: the first describes them.
    if let Some(im) = info.images.first() {
        o.feature(K::SampleLayout, im.pixel_type.ome_name(), &[Scope::Pixels]);
        if let Some(i) = &im.instrument {
            if let Some(sw) = &i.software {
                o.feature(K::Writer, a::writer_name_only(sw), &[Scope::Metadata]);
            }
            if let Some(m) = &i.model {
                o.context(K::Instrument, m);
            }
        }
    }
    if let Some(n) = a::note_with(info, "the sample type is assumed") {
        o.assumed("images[].pixel_type", n.to_string());
    }
    if let Some(n) = a::note_with(info, "not on disk (the copy is incomplete)") {
        o.undecoded("plane files missing from the copy", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "are not named by the index and are not read") {
        o.undecoded("unindexed plane files", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "tiles of one field are not stitched") {
        o.feature(K::Layout, "tiled fields", &[Scope::Pixels]);
        o.undecoded("partial field tiles", &[Scope::Pixels], n.to_string());
    }
    if let Some(n) = a::note_with(info, "planes that differ only in FlimID are not told apart") {
        o.undecoded("FLIM planes", &[Scope::Pixels], n.to_string());
    }
    if let Some(n) = a::note_with(info, "planes of another size fail to read") {
        o.undecoded("channels of different image sizes", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "records of type") {
        o.undecoded("non-image records", &[], n.to_string());
    }
    if a::has_note(
        info,
        "channel descriptions (name, pixel size, wavelengths, objective, exposure) read from the index's Maps",
    ) {
        o.feature(K::Layout, "channel maps", &[Scope::Metadata]);
        o.calibration(
            "flat-field profile",
            CalibrationStatus::Available,
            &[Scope::Pixels],
            "the index carries per-channel flat-field profiles (dump); planes are returned as acquired, as Harmony exports them, without flat-field correction",
        );
    }
    o
}

// BEGIN GENERATED opera-harmony (cargo xtask assurance-audit --write; do not edit)
const OPERA_HARMONY_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const OPERA_HARMONY_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 8, 7, 11),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 9),
    a::row(K::FormatVersion, "Columbus", 2, 1, 2),
    a::row(K::FormatVersion, "FLEX 1.8.1.0", 2, 2, 2),
    a::row(K::FormatVersion, "HarmonyV4", 2, 2, 2),
    a::row(K::FormatVersion, "HarmonyV5", 3, 3, 3),
    a::row(K::FormatVersion, "HarmonyV6", 1, 1, 1),
    a::row(K::FormatVersion, "HarmonyV7", 1, 1, 1),
    a::row(K::Instrument, "OPERA5013", 2, 2, 2),
    a::row(K::Instrument, "Operetta", 2, 2, 2),
    a::row(K::Instrument, "Phenix", 2, 2, 2),
    a::row(K::Instrument, "Sonata", 3, 3, 3),
    a::row(K::Layout, "channel maps", 5, 5, 5),
    a::row(K::SampleLayout, "uint16", 11, 9, 11),
    a::row(K::Writer, "Columbus", 2, 1, 2),
    a::row(K::Writer, "Harmony", 7, 7, 7),
    a::row(K::Writer, "Opera (FLEX)", 2, 2, 2),
];
// END GENERATED opera-harmony

// BEGIN GENERATED imagexpress (cargo xtask assurance-audit --write; do not edit)
const IMAGEXPRESS_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const IMAGEXPRESS_VALIDATED: &[Validated] = &[
    a::row(K::FormatVersion, "HTSInfoFile 1.0", 2, 2, 2),
    a::row(K::FormatVersion, "no index version", 1, 1, 1),
    a::row(K::SampleLayout, "uint16", 3, 3, 3),
    a::row(K::Writer, "MetaMorph", 2, 2, 2),
    a::row(K::Writer, "MetaXpress", 1, 1, 1),
];
// END GENERATED imagexpress

// BEGIN GENERATED cellvoyager (cargo xtask assurance-audit --write; do not edit)
const CELLVOYAGER_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const CELLVOYAGER_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 3),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 3),
    a::row(K::FormatVersion, "MeasurementDetail 1.0", 3, 3, 3),
    a::row(K::Instrument, "CV7000 FD", 1, 1, 1),
    a::row(K::Instrument, "CV8000 AZ01", 2, 2, 2),
    a::row(K::SampleLayout, "uint16", 3, 3, 3),
];
// END GENERATED cellvoyager
