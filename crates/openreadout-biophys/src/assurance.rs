//! Assurance profiles (`docs/assurance.md`) of the biophysics readers (MicroCal ITC, Cytiva
//! Biacore, Agilent Seahorse XF): the variant features of a file and the feature values the
//! development corpus validates. The tables between the GENERATED markers are written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static MICROCAL_ITC: AssuranceProfile = AssuranceProfile {
    format_id: "microcal-itc",
    observe: observe_itc,
    validated: MICROCAL_ITC_VALIDATED,
    confidence: MICROCAL_ITC_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static CYTIVA_BIACORE_BLR: AssuranceProfile = AssuranceProfile {
    format_id: "cytiva-biacore-blr",
    observe: observe_biacore,
    validated: CYTIVA_BIACORE_BLR_VALIDATED,
    confidence: CYTIVA_BIACORE_BLR_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static CYTIVA_BIACORE_BME: AssuranceProfile = AssuranceProfile {
    format_id: "cytiva-biacore-bme",
    observe: observe_biacore,
    validated: CYTIVA_BIACORE_BME_VALIDATED,
    confidence: CYTIVA_BIACORE_BME_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static AGILENT_SEAHORSE_ASYR: AssuranceProfile = AssuranceProfile {
    format_id: "agilent-seahorse-asyr",
    observe: observe_seahorse,
    validated: AGILENT_SEAHORSE_ASYR_VALIDATED,
    confidence: AGILENT_SEAHORSE_ASYR_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

pub(crate) static SARTORIUS_OCTET_FRD: AssuranceProfile = AssuranceProfile {
    format_id: "sartorius-octet-frd",
    observe: observe_octet,
    validated: SARTORIUS_OCTET_FRD_VALIDATED,
    confidence: SARTORIUS_OCTET_FRD_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static MALVERN_ZETASIZER_DTS: AssuranceProfile = AssuranceProfile {
    format_id: "malvern-zetasizer-dts",
    observe: observe_none,
    validated: MALVERN_ZETASIZER_DTS_VALIDATED,
    confidence: MALVERN_ZETASIZER_DTS_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

/// The reader makes every observation (record kinds, schema, software version).
fn observe_none(_info: &FileInfo) -> Observations {
    Observations::default()
}

fn observe_octet(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            format!("RTD {v}"),
            &[Scope::Metadata, Scope::Traces, Scope::Tables],
        );
    }
    o
}

pub(crate) static GENEPIX_GPR: AssuranceProfile = AssuranceProfile {
    format_id: "genepix-gpr",
    observe: observe_gpr,
    validated: GENEPIX_GPR_VALIDATED,
    confidence: GENEPIX_GPR_CONFIDENCE,
    basis: Basis::ReverseEngineered,
};

fn observe_gpr(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Tables]);
    }
    for t in &info.tables {
        if let Some(w) = t
            .extra
            .get("wavelengths_nm")
            .and_then(serde_json::Value::as_array)
        {
            o.feature(
                K::Layout,
                format!("{} wavelengths", w.len()),
                &[Scope::Tables],
            );
        }
    }
    o
}

fn observe_itc(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [Scope::Metadata, Scope::Tables, Scope::Traces];
    if let Some(s) = &info.format_version {
        // `VPViewer2000 Ver: 1.4.8`, `ITC200 Ver: 1.26.1`, `MicroCalITC Ver: 1.29.32 Run time:…`
        let (name, ver) = s.split_once(" Ver:").unwrap_or((s.as_str(), ""));
        o.feature(K::Writer, name.trim(), &all);
        if let Some(v) = a::version_prefix(ver, 2) {
            o.context(K::WriterVersion, format!("{} {v}", name.trim()));
        }
    }
    for t in &info.traces {
        // 3, 7 or 9 data columns: which ones are named depends on it
        let columns = t.channels.len() + usize::from(t.sample_rate_hz > 0.0);
        o.feature(
            K::Layout,
            format!("{columns} data columns"),
            &[Scope::Traces],
        );
    }
    o
}

fn observe_biacore(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            format!("result file {v}"),
            &[Scope::Metadata, Scope::Tables, Scope::Traces],
        );
    }
    for t in &info.traces {
        let kind = if t.extra.get("reference_subtracted") == Some(&serde_json::Value::Bool(true)) {
            "reference-subtracted curve (XYData)"
        } else {
            "flow-cell curve (Segment)"
        };
        o.feature(K::Record, kind, &[Scope::Traces]);
        if t.sample_rate_hz > 0.0 {
            o.feature(
                K::Acquisition,
                format!("{} Hz", t.sample_rate_hz.round()),
                &[Scope::Traces],
            );
        }
    }
    if info
        .tables
        .iter()
        .any(|t| t.name.as_deref() == Some("report_points"))
    {
        o.feature(K::Record, "report point table", &[Scope::Tables]);
    }
    // evaluation files: the kinds of evaluation items that hold fits
    if let Some(t) = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("evaluation_items"))
        && let Some(classes) = t
            .columns
            .iter()
            .find(|c| c.name == "class")
            .and_then(|c| c.extra.get("categories"))
            .and_then(serde_json::Value::as_array)
    {
        for c in classes.iter().filter_map(serde_json::Value::as_str) {
            o.feature(K::Record, format!("evaluation item {c}"), &[Scope::Tables]);
        }
    }
    o
}

fn observe_seahorse(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            format!("assay version {v}"),
            &[Scope::Metadata, Scope::Tables, Scope::Traces],
        );
    }
    for t in &info.traces {
        if let Some(an) = a::extra_str(&t.extra, "analyte") {
            if a::extra_str(&t.extra, "kind") == Some("level") {
                continue;
            }
            o.feature(K::Record, format!("analyte {an}"), &[Scope::Traces]);
            let wells = t.channels.len().saturating_sub(1);
            o.feature(
                K::Layout,
                format!("{wells}-well plate"),
                &[Scope::Traces, Scope::Tables],
            );
        }
    }
    // rates: computed by the published method with the file's constants, compared with Wave's
    if let Some(t) = info
        .tables
        .iter()
        .find(|t| t.name.as_deref() == Some("rates"))
    {
        o.feature(
            K::Acquisition,
            "rates by the compartment model (standard 96-well plate)",
            &[Scope::Tables, Scope::Traces],
        );
        if let Some(v) =
            a::extra_str(&t.extra, "software_version").and_then(|v| a::version_prefix(v, 2))
        {
            o.feature(K::WriterVersion, format!("Wave {v}"), &[Scope::Tables]);
        }
        if t.extra.get("per_computed") == Some(&serde_json::Value::Bool(true)) {
            o.assumed(
                format!("tables[{}].extra.kvol", t.index),
                "kVol of the proton efflux rate is not stored in .asyr files; 1.6, what a Wave export prints for the same 96-well plate, is used",
            );
        }
    }
    o
}

// BEGIN GENERATED microcal-itc (cargo xtask assurance-audit --write; do not edit)
const MICROCAL_ITC_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const MICROCAL_ITC_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.instrument.model", 6, 2, 10),
    a::row(K::Layout, "3 data columns", 3, 1, 4),
    a::row(K::Layout, "7 data columns", 1, 1, 2),
    a::row(K::Layout, "9 data columns", 2, 1, 4),
    a::row(K::Writer, "ITC200", 2, 1, 4),
    a::row(K::Writer, "MicroCalITC", 0, 0, 1),
    a::row(K::Writer, "VPViewer2000", 4, 1, 5),
    a::row(K::WriterVersion, "ITC200 1.25", 0, 0, 1),
    a::row(K::WriterVersion, "ITC200 1.26", 2, 1, 3),
    a::row(K::WriterVersion, "MicroCalITC 1.29", 0, 0, 1),
    a::row(K::WriterVersion, "VPViewer2000 1.30", 1, 1, 1),
    a::row(K::WriterVersion, "VPViewer2000 1.4", 3, 1, 4),
];
// END GENERATED microcal-itc

// BEGIN GENERATED cytiva-biacore-blr (cargo xtask assurance-audit --write; do not edit)
const CYTIVA_BIACORE_BLR_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const CYTIVA_BIACORE_BLR_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "1 Hz", 3, 1, 3),
    a::row(K::Acquisition, "10 Hz", 6, 2, 6),
    a::row(K::Field, "experiment.acquisition.started_at", 9, 2, 9),
    a::row(K::Field, "experiment.instrument.model", 9, 2, 9),
    a::row(K::FormatVersion, "result file 12", 9, 2, 9),
    a::row(K::Record, "flow-cell curve (Segment)", 9, 2, 9),
    a::row(K::Record, "reference-subtracted curve (XYData)", 7, 2, 7),
    a::row(K::Record, "report point table", 9, 2, 9),
];
// END GENERATED cytiva-biacore-blr

// BEGIN GENERATED agilent-seahorse-asyr (cargo xtask assurance-audit --write; do not edit)
const AGILENT_SEAHORSE_ASYR_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const AGILENT_SEAHORSE_ASYR_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "rates by the compartment model (standard 96-well plate)", 3, 1, 3),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 9),
    a::row(K::FormatVersion, "assay version 4", 3, 1, 9),
    a::row(K::Layout, "24-well plate", 0, 0, 2),
    a::row(K::Layout, "96-well plate", 3, 1, 7),
    a::row(K::Record, "analyte O2", 3, 1, 9),
    a::row(K::Record, "analyte pH", 3, 1, 9),
    a::row(K::WriterVersion, "Wave 2.6", 3, 1, 3),
];
// END GENERATED agilent-seahorse-asyr

// BEGIN GENERATED genepix-gpr (cargo xtask assurance-audit --write; do not edit)
const GENEPIX_GPR_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const GENEPIX_GPR_VALIDATED: &[Validated] = &[
    a::row(K::FormatVersion, "ATF 1.0 (GenePix Results 1.4)", 4, 1, 4),
    a::row(K::FormatVersion, "ATF 1.0 (GenePix Results 3)", 1, 1, 1),
    a::row(K::Layout, "2 wavelengths", 1, 1, 1),
];
// END GENERATED genepix-gpr

// BEGIN GENERATED sartorius-octet-frd (cargo xtask assurance-audit --write; do not edit)
const SARTORIUS_OCTET_FRD_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const SARTORIUS_OCTET_FRD_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 6),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 6),
    a::row(K::FormatVersion, "RTD 2.0", 6, 2, 6),
    a::row(K::Instrument, "OctetRED384", 3, 1, 3),
    a::row(K::Instrument, "OctetRED96e", 3, 1, 3),
    a::row(K::Writer, "DataAcquisition.exe", 6, 2, 6),
    a::row(K::WriterVersion, "DataAcquisition.exe 11.1", 6, 2, 6),
];
// END GENERATED sartorius-octet-frd

// BEGIN GENERATED malvern-zetasizer-dts (cargo xtask assurance-audit --write; do not edit)
const MALVERN_ZETASIZER_DTS_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const MALVERN_ZETASIZER_DTS_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "size record", 9, 2, 14),
    a::row(K::Acquisition, "zeta record", 8, 4, 8),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 16),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 16),
    a::row(K::FormatVersion, "record schema 10", 0, 0, 1),
    a::row(K::FormatVersion, "record schema 12", 5, 1, 5),
    a::row(K::FormatVersion, "record schema 13", 9, 4, 13),
    a::row(K::Record, "size result block", 9, 2, 14),
    a::row(K::Record, "zeta result block", 8, 4, 8),
    a::row(K::WriterVersion, "Zetasizer 7.02", 0, 0, 1),
    a::row(K::WriterVersion, "Zetasizer 7.10", 6, 1, 6),
    a::row(K::WriterVersion, "Zetasizer 7.12", 4, 2, 7),
    a::row(K::WriterVersion, "Zetasizer 7.13", 1, 1, 3),
    a::row(K::WriterVersion, "Zetasizer 8.00", 0, 0, 1),
    a::row(K::WriterVersion, "Zetasizer 8.01", 0, 0, 1),
];
// END GENERATED malvern-zetasizer-dts

// BEGIN GENERATED cytiva-biacore-bme (cargo xtask assurance-audit --write; do not edit)
const CYTIVA_BIACORE_BME_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const CYTIVA_BIACORE_BME_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "1 Hz", 2, 2, 2),
    a::row(K::Acquisition, "10 Hz", 6, 4, 6),
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 8),
    a::row(K::Field, "experiment.instrument.model", 0, 0, 8),
    a::row(K::FormatVersion, "result file 4", 6, 4, 6),
    a::row(K::FormatVersion, "result file 5", 2, 2, 2),
    a::row(K::Record, "evaluation item AffinityScreen", 4, 4, 4),
    a::row(K::Record, "evaluation item ConcentrationAnalysis", 1, 1, 1),
    a::row(K::Record, "evaluation item KineticScreen", 1, 1, 1),
    a::row(K::Record, "evaluation item KineticsAffinity", 2, 1, 2),
    a::row(K::Record, "evaluation item Plot", 8, 5, 8),
    a::row(K::Record, "evaluation item ReportPointTable", 8, 5, 8),
    a::row(K::Record, "evaluation item Sensorgram", 8, 5, 8),
    a::row(K::Record, "flow-cell curve (Segment)", 8, 5, 8),
];
// END GENERATED cytiva-biacore-bme
