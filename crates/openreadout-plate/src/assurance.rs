//! Assurance profile (`docs/assurance.md`): the variant features of a plate-reader export
//! (dialect, container and delimiter, software, read modes and types) and the feature values
//! the development corpus validates. The table between the GENERATED markers is written by
//! `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, FeatureKind as K, Observations, Scope, Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static PLATE: AssuranceProfile = AssuranceProfile {
    format_id: "plate",
    observe,
    validated: PLATE_VALIDATED,
    confidence: PLATE_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let both = [Scope::Metadata, Scope::Tables];
    for t in &info.tables {
        if let Some(d) = a::extra_str(&t.extra, "export") {
            o.feature(K::Dialect, d, &both);
        }
        if let Some(sw) = a::extra_str(&t.extra, "software") {
            o.context(K::Writer, sw);
            if let Some(v) =
                a::extra_str(&t.extra, "software_version").and_then(|v| a::version_prefix(v, 1))
            {
                o.context(K::WriterVersion, format!("{sw} {v}"));
            }
        }
        if let Some(f) = a::extra_str(&t.extra, "export_format") {
            o.feature(K::Layout, format!("export format {f}"), &both);
        }
        if let Some(rt) = a::extra_str(&t.extra, "read_type") {
            o.feature(K::Acquisition, format!("{rt} read"), &[Scope::Tables]);
        }
        for m in a::extra_values_in(&t.extra, "read_modes") {
            o.feature(K::Acquisition, m, &[Scope::Tables]);
        }
        if t.extra
            .get("date_order_assumed")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            let field = if t.extra.contains_key("acquired_at") || !t.extra.contains_key("saved_at")
            {
                "tables[].extra.acquired_at"
            } else {
                "tables[].extra.saved_at"
            };
            o.assumed(
                field,
                "the export writes dates without saying whether day or month comes first; the order was assumed",
            );
        }
        if t.extra.contains_key("reads_inferred_from_labels") {
            o.derived(
                format!("tables[{}].extra.reads[].wavelength_nm", t.index),
                "label keywords",
                "the export has no procedure: read wavelengths come from the block labels",
            );
        }
        // How each measured read's mode was established (`mode_basis`): a derived mode is an
        // inferred value whose rule the corpus validates; an undetermined one is not a value.
        let reads = t
            .extra
            .get("reads")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (i, r) in reads.iter().enumerate() {
            let field = format!("tables[{}].extra.reads[{i}].mode", t.index);
            let basis = r.get("mode_basis").and_then(serde_json::Value::as_str);
            let mode = r
                .get("mode")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let label = r
                .get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            match basis {
                Some("detector") => o.derived(
                    &field,
                    "detector code",
                    format!("{mode} from the detector the export records for read {label:?}"),
                ),
                Some("label") => o.derived(
                    &field,
                    "label keywords",
                    format!("{mode} from keywords of the read's label {label:?}"),
                ),
                Some("settings") => o.derived(
                    &field,
                    "read settings",
                    format!("{mode} from the read's settings (a measurement wavelength, no excitation or emission)"),
                ),
                Some("undetermined") => o.assumed(
                    &field,
                    format!("nothing in the export names the detection mode of read {label:?}; it is reported unknown"),
                ),
                _ => {}
            }
        }
        if let Some(i) = t
            .extra
            .get("instrument")
            .and_then(|i| i.get("model"))
            .and_then(serde_json::Value::as_str)
        {
            o.context(K::Instrument, i);
        }
    }
    o
}

/// What only the dataset knows: how the export was stored (a workbook, or text with its
/// delimiter), from the container description `info --view full` also shows.
pub(crate) fn internal(container: &serde_json::Value) -> Observations {
    let mut o = Observations::default();
    let kind = container
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let value = match container
        .get("delimiter")
        .and_then(serde_json::Value::as_str)
    {
        Some(d) => format!("{kind} ({d})"),
        None => kind.to_string(),
    };
    o.feature(K::Layout, format!("container {value}"), &[Scope::Tables]);
    o
}

// BEGIN GENERATED plate (cargo xtask assurance-audit --write; do not edit)
const PLATE_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const PLATE_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "absorbance", 28, 8, 32),
    a::row(K::Acquisition, "endpoint read", 38, 9, 44),
    a::row(K::Acquisition, "fluorescence", 15, 5, 17),
    a::row(K::Acquisition, "kinetic read", 10, 5, 11),
    a::row(K::Acquisition, "luminescence", 11, 6, 12),
    a::row(K::Acquisition, "spectrum read", 5, 4, 5),
    a::row(K::Acquisition, "unknown", 1, 0, 1),
    a::row(K::Derivation, "tables[].extra.reads[].mode by detector code", 7, 4, 7),
    a::row(K::Derivation, "tables[].extra.reads[].mode by label keywords", 1, 1, 10),
    a::row(K::Derivation, "tables[].extra.reads[].mode by read settings", 1, 1, 1),
    a::row(K::Derivation, "tables[].extra.reads[].wavelength_nm by label keywords", 0, 0, 2),
    a::row(K::Dialect, "bmg-mars", 9, 4, 9),
    a::row(K::Dialect, "bmg-smart-control", 1, 1, 1),
    a::row(K::Dialect, "envision", 7, 4, 7),
    a::row(K::Dialect, "gen5", 10, 1, 16),
    a::row(K::Dialect, "generic", 2, 0, 2),
    a::row(K::Dialect, "kaleido", 1, 1, 1),
    a::row(K::Dialect, "skanit", 2, 1, 2),
    a::row(K::Dialect, "softmax-pro", 14, 4, 15),
    a::row(K::Dialect, "tecan-i-control", 5, 4, 5),
    a::row(K::Dialect, "tecan-magellan", 2, 1, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 21, 1, 52),
    a::row(K::Field, "experiment.instrument.model", 14, 5, 46),
    a::row(K::Field, "tables[].extra.reads[].mode", 47, 14, 58),
    a::row(K::Instrument, "CLARIOstar", 6, 4, 6),
    a::row(K::Instrument, "Cytation3", 0, 0, 1),
    a::row(K::Instrument, "Cytation5", 0, 0, 2),
    a::row(K::Instrument, "EnVision", 7, 4, 7),
    a::row(K::Instrument, "Example Device", 1, 1, 1),
    a::row(K::Instrument, "Generic Reader", 1, 1, 1),
    a::row(K::Instrument, "PHERAstar", 3, 1, 3),
    a::row(K::Instrument, "SPECTRAmax 340PC", 1, 1, 1),
    a::row(K::Instrument, "SPECTRAmax M2e", 1, 1, 1),
    a::row(K::Instrument, "SPECTRAmax M5", 2, 1, 2),
    a::row(K::Instrument, "Spark", 2, 1, 2),
    a::row(K::Instrument, "SpectraMax M3", 6, 1, 6),
    a::row(K::Instrument, "SpectraMax M5", 0, 0, 1),
    a::row(K::Instrument, "Synergy H1", 4, 1, 6),
    a::row(K::Instrument, "Synergy HT", 0, 0, 1),
    a::row(K::Instrument, "ULTRA", 1, 1, 1),
    a::row(K::Instrument, "Varioskan LUX", 1, 1, 1),
    a::row(K::Instrument, "infinite 200Pro", 3, 3, 3),
    a::row(K::Layout, "container gen5-experiment", 0, 0, 6),
    a::row(K::Layout, "container softmax-pro-5-document", 4, 2, 4),
    a::row(K::Layout, "container softmax-pro-document", 6, 1, 7),
    a::row(K::Layout, "container text (comma)", 19, 7, 19),
    a::row(K::Layout, "container text (semicolon)", 2, 2, 2),
    a::row(K::Layout, "container text (tab)", 16, 3, 16),
    a::row(K::Layout, "container xlsx", 6, 2, 6),
    a::row(K::Layout, "export format PlateFormat", 3, 1, 3),
    a::row(K::Layout, "export format TimeFormat", 1, 1, 1),
    a::row(K::Writer, "EnVision Workstation", 7, 4, 7),
    a::row(K::Writer, "Gen5", 10, 1, 16),
    a::row(K::Writer, "Kaleido", 1, 1, 1),
    a::row(K::Writer, "MARS", 9, 4, 9),
    a::row(K::Writer, "Magellan", 2, 1, 2),
    a::row(K::Writer, "SMART Control", 1, 1, 1),
    a::row(K::Writer, "SkanIt", 2, 1, 2),
    a::row(K::Writer, "SoftMax Pro", 14, 4, 15),
    a::row(K::Writer, "i-control", 5, 4, 5),
    a::row(K::WriterVersion, "EnVision Workstation 1", 7, 4, 7),
    a::row(K::WriterVersion, "Gen5 2", 0, 0, 2),
    a::row(K::WriterVersion, "Gen5 3", 8, 1, 12),
    a::row(K::WriterVersion, "Kaleido 2", 1, 1, 1),
    a::row(K::WriterVersion, "SkanIt 7", 1, 1, 1),
    a::row(K::WriterVersion, "SoftMax Pro 5", 4, 2, 4),
    a::row(K::WriterVersion, "i-control 1", 2, 2, 2),
    a::row(K::WriterVersion, "i-control 2", 3, 2, 3),
];
// END GENERATED plate
