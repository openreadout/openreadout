//! Assurance profile (`docs/assurance.md`): the variant features of an FCS file (version,
//! data type and byte order, acquisition platform) and the feature values the development
//! corpus validates, with the state of compensation and scaling. The table between the
//! GENERATED markers is written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static FCS: AssuranceProfile = AssuranceProfile {
    format_id: "fcs",
    observe,
    validated: FCS_VALIDATED,
    confidence: FCS_CONFIDENCE,
    basis: Basis::OpenSpec,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Tables]);
    }
    for t in &info.tables {
        if let Some(d) = a::extra_str(&t.extra, "datatype") {
            o.feature(K::SampleLayout, format!("$DATATYPE {d}"), &[Scope::Tables]);
        }
        if let Some(b) = a::extra_str(&t.extra, "byte_order") {
            o.feature(K::SampleLayout, b, &[Scope::Tables]);
        }
        if let Some(m) = a::extra_str(&t.extra, "mode") {
            o.feature(K::Layout, format!("$MODE {m}"), &[Scope::Tables]);
        }
        // Acquisition software decides keyword conventions ($SPILLOVER vs SPILL, $PnB packing).
        if let Some(f) = t
            .extra
            .get("platform")
            .and_then(|p| p.get("family"))
            .and_then(serde_json::Value::as_str)
        {
            o.feature(K::Writer, f, &[Scope::Metadata, Scope::Tables]);
        } else if let Some(sw) = a::extra_str(&t.extra, "software") {
            o.context(K::Writer, a::writer_name_only(sw));
        }
        if let Some(m) = t
            .extra
            .get("instrument")
            .and_then(|i| i.get("model"))
            .and_then(serde_json::Value::as_str)
        {
            o.context(K::Instrument, m);
        }
        if t.extra.contains_key("spillover") {
            o.calibration(
                "spillover compensation",
                CalibrationStatus::Available,
                &[Scope::Tables],
                "the file carries a spillover matrix; `table --compensate` applies it, raw values are returned otherwise (as fcsparser and FlowIO do)",
            );
        }
    }
    if info.tables.len() > 1 {
        o.feature(K::Layout, "chained data sets ($NEXTDATA)", &[Scope::Tables]);
    }
    if !info.tables.is_empty() {
        o.calibration(
            "$PnE/$PnG/$TIMESTEP scaling",
            CalibrationStatus::Available,
            &[Scope::Tables],
            "values are raw channel values; `table --transform` scales them, as FlowJo shows them",
        );
    }
    if let Some(n) = a::note_with(info, "histogram-mode data sets ($MODE C/U)") {
        o.undecoded("histogram-mode data sets", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded("damaged segments", &[Scope::Tables], n.to_string());
    }
    o
}

// BEGIN GENERATED fcs (cargo xtask assurance-audit --write; do not edit)
const FCS_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const FCS_VALIDATED: &[Validated] = &[
    a::row(K::FormatVersion, "2.0", 8, 3, 8),
    a::row(K::FormatVersion, "3.0", 22, 9, 22),
    a::row(K::FormatVersion, "3.1", 14, 5, 14),
    a::row(K::FormatVersion, "3.2", 1, 1, 1),
    a::row(K::Instrument, "4486521 Attune NxT Acoustic Focusing Cytometer (Lasers: BRVY)", 1, 1, 1),
    a::row(K::Instrument, "Aurora", 1, 1, 1),
    a::row(K::Instrument, "BD Accuri C6 Plus", 1, 1, 1),
    a::row(K::Instrument, "Cube_15", 1, 1, 1),
    a::row(K::Instrument, "Cytek xP5: NCSU CORE  xP5 Facscan", 1, 1, 1),
    a::row(K::Instrument, "CytoFLEX", 1, 1, 1),
    a::row(K::Instrument, "Cytomics FC 500", 1, 1, 1),
    a::row(K::Instrument, "DVSSCIENCES-FLUIDIGM-CYTOF-7.0.8493", 1, 1, 1),
    a::row(K::Instrument, "FACSAriaII", 1, 1, 1),
    a::row(K::Instrument, "FACSCalibur", 3, 3, 3),
    a::row(K::Instrument, "FACSCantoII", 2, 1, 2),
    a::row(K::Instrument, "FACSDiscover S8", 1, 1, 1),
    a::row(K::Instrument, "FACScan", 1, 1, 1),
    a::row(K::Instrument, "Guava Muse, Viacount 1.8", 1, 1, 1),
    a::row(K::Instrument, "LSRFortessa", 2, 1, 2),
    a::row(K::Instrument, "LSRII", 9, 3, 9),
    a::row(K::Instrument, "MACSQuant", 5, 1, 5),
    a::row(K::Instrument, "MACSQuant VYB,2.5.1345.9863", 1, 1, 1),
    a::row(K::Instrument, "Main Aria (FACSAria)", 1, 1, 1),
    a::row(K::Instrument, "Navios", 2, 2, 2),
    a::row(K::Instrument, "S1400EXi", 3, 1, 3),
    a::row(K::Instrument, "S3", 1, 1, 1),
    a::row(K::Instrument, "SA3800", 1, 1, 1),
    a::row(K::Layout, "$MODE L", 45, 12, 45),
    a::row(K::Layout, "chained data sets ($NEXTDATA)", 2, 2, 2),
    a::row(K::SampleLayout, "$DATATYPE F", 32, 12, 32),
    a::row(K::SampleLayout, "$DATATYPE I", 13, 3, 13),
    a::row(K::SampleLayout, "big-endian", 19, 8, 19),
    a::row(K::SampleLayout, "little-endian", 26, 7, 26),
    a::row(K::Writer, "CELLQuestª", 3, 2, 3),
    a::row(K::Writer, "CellQuest Proª", 1, 1, 1),
    a::row(K::Writer, "FlowJoCollectorsEdition", 1, 1, 1),
    a::row(K::Writer, "LYSYS", 1, 1, 1),
    a::row(K::Writer, "bd-facsdiva", 12, 6, 12),
    a::row(K::Writer, "bd-spectral", 1, 1, 1),
    a::row(K::Writer, "beckman-cytoflex", 1, 1, 1),
    a::row(K::Writer, "cytek-spectral", 1, 1, 1),
    a::row(K::Writer, "mass-cytometry", 1, 1, 1),
    a::row(K::Writer, "sony-spectral", 1, 1, 1),
];
// END GENERATED fcs
