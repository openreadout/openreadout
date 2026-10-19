//! Assurance profile (`docs/assurance.md`) of the ABIF reader: the variant features of a file
//! and the feature values the development corpus validates. The tables between the GENERATED
//! markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static ABIF: AssuranceProfile = AssuranceProfile {
    format_id: "abif",
    observe,
    validated: ABIF_VALIDATED,
    confidence: ABIF_CONFIDENCE,
    basis: Basis::VendorDocs,
};

/// What `info` shows of a file's variant: how many dye channels its traces hold. The reader adds
/// the rest (run kind, instrument, records, sizing).
fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        if a::extra_str(&t.extra, "kind") == Some("run_monitor") {
            continue;
        }
        let dyes = t
            .channels
            .iter()
            .filter(|c| c.extra.contains_key("dye_number"))
            .count();
        if dyes > 0 {
            o.feature(K::SampleLayout, format!("{dyes} dyes"), &[Scope::Traces]);
        }
    }
    o
}

// BEGIN GENERATED abif (cargo xtask assurance-audit --write; do not edit)
const ABIF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const ABIF_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "fragment_analysis", 14, 5, 14),
    a::row(K::Acquisition, "sanger", 13, 6, 13),
    a::row(K::FormatVersion, "ABIF 101", 16, 8, 16),
    a::row(K::FormatVersion, "ABIF 300", 11, 2, 11),
    a::row(K::Instrument, "310", 1, 1, 1),
    a::row(K::Instrument, "3130", 1, 1, 1),
    a::row(K::Instrument, "3130xl", 3, 3, 3),
    a::row(K::Instrument, "3500", 13, 4, 13),
    a::row(K::Instrument, "3730xl", 8, 4, 8),
    a::row(K::Layout, "Fragment Format 2.0", 14, 5, 14),
    a::row(K::Layout, "Sequencing Format 2.0", 2, 2, 2),
    a::row(K::Record, "analyzed trace", 24, 8, 24),
    a::row(K::Record, "base calls", 13, 6, 13),
    a::row(K::Record, "fragment size by Local Southern", 5, 5, 5),
    a::row(K::Record, "fragment size from the instrument", 9, 1, 9),
    a::row(K::Record, "instrument peak table", 9, 1, 9),
    a::row(K::Record, "raw trace", 27, 10, 27),
    a::row(K::Record, "run monitor trace", 26, 10, 26),
    a::row(K::Record, "size standard GS120LIZ", 1, 1, 1),
    a::row(K::Record, "size standard GS500ROX", 1, 1, 1),
    a::row(K::Record, "size standard GS600LIZ", 12, 3, 12),
    a::row(K::Record, "sized trace", 14, 5, 14),
    a::row(K::SampleLayout, "4 dyes", 14, 6, 14),
    a::row(K::SampleLayout, "5 dyes", 13, 4, 13),
    a::row(K::WriterVersion, "Data Collection 3.0", 9, 5, 9),
    a::row(K::WriterVersion, "Data Collection 3.0bc2", 1, 1, 1),
    a::row(K::WriterVersion, "Data Collection 3.1", 2, 2, 2),
    a::row(K::WriterVersion, "Data Collection 3500 Data Collection Version 1.0 - 3500 FC_090519180136-0700 Build Number: 837", 2, 2, 2),
    a::row(K::WriterVersion, "Data Collection 3500 Dx Series Data Collection Software 3 IVD v3.0.1 (2,169)", 10, 1, 10),
    a::row(K::WriterVersion, "Data Collection 3500 Series Data Collection Software 3 v3.3 (2,251)", 1, 1, 1),
    a::row(K::WriterVersion, "Data Collection ABI PRISM 310 Collection 3.1.0", 1, 1, 1),
];
// END GENERATED abif

pub(crate) static BIOANALYZER: AssuranceProfile = AssuranceProfile {
    format_id: "agilent-bioanalyzer-xml",
    observe: observe_chip,
    validated: AGILENT_BIOANALYZER_XML_VALIDATED,
    confidence: AGILENT_BIOANALYZER_XML_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static TAPESTATION: AssuranceProfile = AssuranceProfile {
    format_id: "agilent-tapestation",
    observe: observe_chip,
    validated: AGILENT_TAPESTATION_VALIDATED,
    confidence: AGILENT_TAPESTATION_CONFIDENCE,
    basis: Basis::PriorArt,
};

/// What `info` shows of a chip or tape export: the size unit of its lanes.
fn observe_chip(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    for t in &info.traces {
        if let Some(u) = t
            .extra
            .get("axis")
            .and_then(|a| a.get("unit"))
            .and_then(|u| u.as_str())
        {
            o.feature(K::SampleLayout, format!("sized in {u}"), &[Scope::Traces]);
        }
    }
    o
}

// BEGIN GENERATED agilent-bioanalyzer-xml (cargo xtask assurance-audit --write; do not edit)
const AGILENT_BIOANALYZER_XML_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const AGILENT_BIOANALYZER_XML_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "DNA 1000", 1, 1, 1),
    a::row(K::Acquisition, "DNA 1000 Series II", 1, 1, 1),
    a::row(K::Acquisition, "Eukaryote Total RNA Nano", 1, 1, 1),
    a::row(K::Acquisition, "High Sensitivity DNA Assay", 1, 1, 1),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 5),
    a::row(K::Record, "size by point-to-point standard curve", 4, 2, 5),
    a::row(K::SampleLayout, "sized in bp", 3, 2, 3),
    a::row(K::SampleLayout, "sized in nt", 1, 1, 1),
    a::row(K::WriterVersion, "B.01.02.SI136", 2, 1, 2),
    a::row(K::WriterVersion, "B.02.07.SI529", 1, 1, 1),
    a::row(K::WriterVersion, "B.02.08.SI648", 1, 1, 1),
];
// END GENERATED agilent-bioanalyzer-xml

// BEGIN GENERATED agilent-tapestation (cargo xtask assurance-audit --write; do not edit)
const AGILENT_TAPESTATION_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const AGILENT_TAPESTATION_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "D1000", 1, 1, 1),
    a::row(K::Acquisition, "D1000 High  Sensitivity", 1, 1, 1),
    a::row(K::Acquisition, "Genomic DNA", 3, 2, 3),
    a::row(K::Acquisition, "RNA", 2, 2, 2),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 7),
    a::row(K::Record, "size by ladder aligned on the markers", 4, 1, 7),
    a::row(K::SampleLayout, "sized in bp", 3, 1, 3),
    a::row(K::SampleLayout, "sized in nt", 1, 1, 1),
    a::row(K::WriterVersion, "2.1.17.8091", 1, 1, 1),
    a::row(K::WriterVersion, "3.2.0.37131", 6, 2, 6),
];
// END GENERATED agilent-tapestation
