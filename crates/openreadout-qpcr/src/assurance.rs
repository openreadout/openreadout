//! Assurance profiles (`docs/assurance.md`) of the qPCR readers (RDML, Applied Biosystems
//! `.eds`, Bio-Rad `.pcrd`, Rotor-Gene `.rex`): the variant features of a run file and the
//! feature values the development corpus validates. The tables between the GENERATED markers
//! are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static RDML: AssuranceProfile = AssuranceProfile {
    format_id: "rdml",
    observe,
    validated: RDML_VALIDATED,
    confidence: RDML_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static APPLIED_BIOSYSTEMS_EDS: AssuranceProfile = AssuranceProfile {
    format_id: "applied-biosystems-eds",
    observe,
    validated: APPLIED_BIOSYSTEMS_EDS_VALIDATED,
    confidence: APPLIED_BIOSYSTEMS_EDS_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static BIO_RAD_PCRD: AssuranceProfile = AssuranceProfile {
    format_id: "bio-rad-pcrd",
    observe,
    validated: BIO_RAD_PCRD_VALIDATED,
    confidence: BIO_RAD_PCRD_CONFIDENCE,
    basis: Basis::OpenSpec,
};

pub(crate) static ROCHE_LIGHTCYCLER_IXO: AssuranceProfile = AssuranceProfile {
    format_id: "roche-lightcycler-ixo",
    observe,
    validated: ROCHE_LIGHTCYCLER_IXO_VALIDATED,
    confidence: ROCHE_LIGHTCYCLER_IXO_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static ROTOR_GENE_REX: AssuranceProfile = AssuranceProfile {
    format_id: "rotor-gene-rex",
    observe,
    validated: ROTOR_GENE_REX_VALIDATED,
    confidence: ROTOR_GENE_REX_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static QPCR_RESULTS_EXPORT: AssuranceProfile = AssuranceProfile {
    format_id: "qpcr-results-export",
    observe,
    validated: QPCR_RESULTS_EXPORT_VALIDATED,
    confidence: QPCR_RESULTS_EXPORT_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let both = [Scope::Metadata, Scope::Tables, Scope::Traces];
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &both);
    }
    for t in &info.tables {
        if let Some(d) = a::extra_str(&t.extra, "dialect") {
            o.feature(K::Dialect, d, &both);
        }
        if let Some(i) = t.extra.get("instrument") {
            if let Some(m) = i.get("model").and_then(serde_json::Value::as_str) {
                o.context(K::Instrument, m);
            }
            if let Some(s) = i.get("software").and_then(serde_json::Value::as_str) {
                o.context(K::Writer, a::writer_name_only(s));
            }
        }
        if let Some(e) = a::extra_str(&t.extra, "experiment_type") {
            o.context(K::Acquisition, e);
        }
        if let Some(c) = a::extra_str(&t.extra, "chemistry") {
            o.context(K::Acquisition, format!("chemistry {c}"));
        }
        // results exports: the container the table was read from
        if let Some(c) = a::extra_str(&t.extra, "export_container") {
            o.feature(K::Codec, c, &both);
        }
    }
    for t in &info.traces {
        if let Some(k) = a::extra_str(&t.extra, "kind") {
            o.feature(K::Record, k, &[Scope::Traces]);
        }
    }
    if a::has_note(info, "the file holds no analysis results") {
        o.feature(K::Layout, "not analysed (no Cq)", &[Scope::Tables]);
    }
    if a::has_note(info, "no fluorescence data in the file") {
        o.feature(K::Layout, "no fluorescence data", &[Scope::Traces]);
    }
    if let Some(n) = a::note_with(info, "raw optical images only, which are not decoded") {
        o.undecoded("raw optical images", &[], n.to_string());
    }
    if a::has_note(info, "genotyping (allelic discrimination) results") {
        o.feature(K::Acquisition, "genotyping", &[Scope::Tables]);
    }
    for n in &info.notes {
        // LightCycler 480: analyses other than absolute quantification are left out
        if n.contains("is listed in the vendor tree, not read") {
            o.undecoded("LightCycler 480 analysis", &[], n.clone());
        }
        // LightCycler 480: call codes other than 0 and 2 give no Cq
        if n.contains("no Cq is reported for them") {
            o.undecoded("LightCycler 480 call codes", &[Scope::Tables], n.clone());
        }
        if n.contains("the window the vendor used could not be recovered") {
            o.undecoded("automatic baseline window", &[], n.clone());
        }
    }
    if let Some(n) = a::note_with(info, "digital-PCR partition data, which is not read") {
        o.undecoded("digital-PCR partitions", &[], n.to_string());
    }
    for n in &info.notes {
        if n.contains("read as a percentage") {
            o.assumed("targets[].efficiency", n.clone());
        }
        if n.contains("could not be read: ") {
            o.undecoded("unreadable member", &[], n.clone());
        }
        if n.contains("give no plate position and were skipped") || n.contains("lies outside the") {
            o.undecoded("reactions without a plate position", &[], n.clone());
        }
    }
    o
}

// BEGIN GENERATED rdml (cargo xtask assurance-audit --write; do not edit)
const RDML_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const RDML_VALIDATED: &[Validated] = &[
    a::row(K::Dialect, "rdml", 9, 5, 9),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 1),
    a::row(K::FormatVersion, "1.0", 1, 1, 1),
    a::row(K::FormatVersion, "1.1", 5, 2, 5),
    a::row(K::FormatVersion, "1.2", 1, 1, 1),
    a::row(K::FormatVersion, "1.3", 2, 2, 2),
    a::row(K::Instrument, "LightCycler 96", 4, 2, 4),
    a::row(K::Record, "amplification", 7, 3, 7),
    a::row(K::Record, "melt", 6, 4, 6),
    a::row(K::Record, "melt derivative", 6, 4, 6),
    a::row(K::Writer, "LightCycler", 4, 2, 4),
];
// END GENERATED rdml

// BEGIN GENERATED applied-biosystems-eds (cargo xtask assurance-audit --write; do not edit)
const APPLIED_BIOSYSTEMS_EDS_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const APPLIED_BIOSYSTEMS_EDS_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "Comparative CT (ΔΔCT)", 1, 1, 1),
    a::row(K::Acquisition, "Comparative Cт (ΔΔCт)", 3, 3, 3),
    a::row(K::Acquisition, "Custom", 1, 1, 1),
    a::row(K::Acquisition, "Genotyping", 1, 1, 1),
    a::row(K::Acquisition, "Quantitation - Comparative Cт (ΔΔCт)", 4, 3, 4),
    a::row(K::Acquisition, "Quantitation - Standard Curve", 8, 3, 8),
    a::row(K::Acquisition, "Standard Curve", 3, 2, 3),
    a::row(K::Acquisition, "chemistry OTHER", 1, 1, 1),
    a::row(K::Acquisition, "chemistry SYBR_GREEN", 17, 10, 17),
    a::row(K::Acquisition, "chemistry TAQMAN", 3, 3, 3),
    a::row(K::Acquisition, "genotyping", 1, 1, 1),
    a::row(K::Dialect, "eds-7500", 11, 5, 11),
    a::row(K::Dialect, "eds-json", 3, 3, 3),
    a::row(K::Dialect, "eds-sds", 9, 8, 9),
    a::row(K::Field, "experiment.acquisition.started_at", 6, 5, 17),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 23),
    a::row(K::FormatVersion, "1.1.1", 12, 6, 12),
    a::row(K::FormatVersion, "1.3.0", 3, 3, 3),
    a::row(K::FormatVersion, "1.3.1", 2, 2, 2),
    a::row(K::FormatVersion, "1.3.2", 3, 3, 3),
    a::row(K::FormatVersion, "2.0.0", 3, 3, 3),
    a::row(K::Instrument, "QS7Flex", 1, 1, 1),
    a::row(K::Instrument, "QuantStudio 1", 1, 1, 1),
    a::row(K::Instrument, "QuantStudio 6 Pro", 1, 1, 1),
    a::row(K::Instrument, "QuantStudio 7 Pro", 1, 1, 1),
    a::row(K::Instrument, "appletini", 3, 3, 3),
    a::row(K::Instrument, "paragon", 3, 3, 3),
    a::row(K::Instrument, "sds7500", 2, 2, 2),
    a::row(K::Instrument, "sds7500fast", 1, 1, 1),
    a::row(K::Instrument, "spyder", 1, 1, 1),
    a::row(K::Instrument, "steponeplus", 9, 3, 9),
    a::row(K::Layout, "no fluorescence data", 1, 1, 1),
    a::row(K::Layout, "not analysed (no Cq)", 2, 2, 2),
    a::row(K::Record, "amplification", 21, 13, 21),
    a::row(K::Record, "amplification baseline-corrected", 18, 11, 18),
    a::row(K::Record, "melt", 15, 8, 15),
    a::row(K::Record, "melt derivative", 15, 8, 15),
    a::row(K::Record, "multicomponent", 22, 14, 22),
    a::row(K::Writer, "QuantStudio", 1, 1, 1),
    a::row(K::Writer, "QuantStudio Design & Analysis", 2, 2, 2),
    a::row(K::Writer, "QuantStudio™", 1, 1, 1),
    a::row(K::Writer, "QuantStudio™ Design & Analysis Software", 3, 3, 3),
    a::row(K::Writer, "QuantStudio™ Real-Time PCR Software", 4, 3, 4),
    a::row(K::Writer, "StepOne Software", 9, 3, 9),
];
// END GENERATED applied-biosystems-eds

// BEGIN GENERATED bio-rad-pcrd (cargo xtask assurance-audit --write; do not edit)
const BIO_RAD_PCRD_CONFIDENCE: Confidence = Confidence::Low;
const BIO_RAD_PCRD_VALIDATED: &[Validated] = &[];
// END GENERATED bio-rad-pcrd

// BEGIN GENERATED roche-lightcycler-ixo (cargo xtask assurance-audit --write; do not edit)
const ROCHE_LIGHTCYCLER_IXO_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const ROCHE_LIGHTCYCLER_IXO_VALIDATED: &[Validated] = &[
    a::row(K::Dialect, "ixo", 0, 0, 10),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 10),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 10),
    a::row(K::Instrument, "29892", 0, 0, 6),
    a::row(K::Instrument, "LightCycler 480 - LED lamp", 0, 0, 2),
    a::row(K::Instrument, "LightCycler 480 - Xenon lamp", 0, 0, 2),
    a::row(K::Record, "amplification", 0, 0, 10),
    a::row(K::Record, "melt", 0, 0, 4),
    a::row(K::Record, "melt derivative", 0, 0, 4),
    a::row(K::Writer, "LightCycler", 0, 0, 10),
];
// END GENERATED roche-lightcycler-ixo

// BEGIN GENERATED rotor-gene-rex (cargo xtask assurance-audit --write; do not edit)
const ROTOR_GENE_REX_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const ROTOR_GENE_REX_VALIDATED: &[Validated] = &[
    a::row(K::Dialect, "rex", 1, 1, 1),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 1),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 1),
    a::row(K::FormatVersion, "REX 3.15", 1, 1, 1),
    a::row(K::Record, "amplification", 1, 1, 1),
    a::row(K::Record, "melt", 1, 1, 1),
    a::row(K::Record, "melt derivative", 1, 1, 1),
    a::row(K::Writer, "Rotor-Gene Q Series Software", 1, 1, 1),
];
// END GENERATED rotor-gene-rex

// BEGIN GENERATED qpcr-results-export (cargo xtask assurance-audit --write; do not edit)
const QPCR_RESULTS_EXPORT_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const QPCR_RESULTS_EXPORT_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "Comparative Cт (ΔΔCт)", 2, 2, 2),
    a::row(K::Acquisition, "Standard Curve", 2, 2, 2),
    a::row(K::Acquisition, "chemistry SYBR_GREEN", 6, 6, 6),
    a::row(K::Acquisition, "chemistry TAQMAN", 1, 1, 1),
    a::row(K::Codec, "comma-delimited text", 2, 1, 2),
    a::row(K::Codec, "xls workbook", 7, 6, 7),
    a::row(K::Dialect, "applied-biosystems-export", 7, 6, 7),
    a::row(K::Dialect, "bio-rad-cfx-export", 2, 1, 2),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 7),
    a::row(K::Instrument, "QuantStudio 12K Flex", 1, 1, 1),
    a::row(K::Instrument, "QuantStudio(TM) 7 Flex System", 1, 1, 1),
    a::row(K::Instrument, "QuantStudio™ 3 System", 1, 1, 1),
    a::row(K::Instrument, "ViiA 7", 1, 1, 1),
    a::row(K::Instrument, "steponeplus", 3, 2, 3),
    a::row(K::Record, "amplification", 2, 2, 2),
    a::row(K::Record, "amplification baseline-corrected", 2, 2, 2),
    a::row(K::Record, "melt", 2, 2, 2),
    a::row(K::Record, "melt derivative", 2, 2, 2),
];
// END GENERATED qpcr-results-export
