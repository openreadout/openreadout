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

/// Plate data the reader found but did not read. A read, table or plate section the export
/// holds and the reader refused is left out (the other reads do not depend on it). An export
/// that yields no values at all, or a block without values, has its data in a layout the
/// reader does not decode, so its tables are unvalidated whatever else is known about the file.
pub(crate) fn left_out(ex: &crate::model::Export) -> Observations {
    const REFUSALS: &[&str] = &[
        "multiple_reads_without_mean",
        "plate_not_decoded",
        "read_not_decoded",
        "table_axis_not_decoded",
        "unsupported_read_type",
    ];
    let mut o = Observations::default();
    let findings = ex
        .findings
        .iter()
        .chain(ex.blocks.iter().flat_map(|b| &b.findings));
    for f in findings.filter(|f| REFUSALS.contains(&f.code.as_str())) {
        o.undecoded(
            format!("plate data refused ({})", f.code),
            &[],
            f.message.clone(),
        );
    }
    // A worksheet that is an export of its own and was not read: the tables read from the
    // other sheets do not depend on it.
    for (name, kind) in &ex.unread_sheets {
        if let Some(k) = kind {
            o.undecoded(
                format!("worksheet not read ({name})"),
                &[],
                format!("the worksheet looks like a separate `{}` export", k.id()),
            );
        }
    }
    if ex.blocks.iter().all(|b| b.obs.is_empty()) {
        o.undecoded(
            "plate values",
            &[Scope::Tables],
            "the export was recognised but no plate values were read from it",
        );
    } else {
        for b in ex.blocks.iter().filter(|b| b.obs.is_empty()) {
            o.undecoded(
                format!("plate block without values ({})", b.name),
                &[Scope::Tables],
                "the block was found but none of its values were read",
            );
        }
    }
    o
}

// BEGIN GENERATED plate (cargo xtask assurance-audit --write; do not edit)
const PLATE_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const PLATE_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "absorbance", 39, 15, 40),
    a::row(K::Acquisition, "endpoint read", 47, 14, 52),
    a::row(K::Acquisition, "fluorescence", 21, 9, 26),
    a::row(K::Acquisition, "kinetic read", 17, 11, 19),
    a::row(K::Acquisition, "luminescence", 11, 6, 12),
    a::row(K::Acquisition, "spectrum read", 5, 4, 5),
    a::row(K::Acquisition, "unknown", 1, 0, 3),
    a::row(K::Derivation, "tables[].extra.reads[].mode by detector code", 7, 4, 7),
    a::row(K::Derivation, "tables[].extra.reads[].mode by label keywords", 1, 1, 13),
    a::row(K::Derivation, "tables[].extra.reads[].mode by read settings", 1, 1, 1),
    a::row(K::Derivation, "tables[].extra.reads[].wavelength_nm by label keywords", 0, 0, 2),
    a::row(K::Dialect, "bmg-mars", 11, 6, 11),
    a::row(K::Dialect, "bmg-smart-control", 1, 1, 3),
    a::row(K::Dialect, "envision", 7, 4, 7),
    a::row(K::Dialect, "gen5", 18, 6, 20),
    a::row(K::Dialect, "generic", 2, 0, 2),
    a::row(K::Dialect, "kaleido", 1, 1, 1),
    a::row(K::Dialect, "skanit", 2, 1, 2),
    a::row(K::Dialect, "softmax-pro", 16, 5, 17),
    a::row(K::Dialect, "tecan-i-control", 9, 6, 11),
    a::row(K::Dialect, "tecan-magellan", 2, 1, 2),
    a::row(K::Field, "experiment.acquisition.started_at", 21, 1, 62),
    a::row(K::Field, "experiment.instrument.model", 19, 8, 61),
    a::row(K::Field, "tables[].extra.reads[].mode", 56, 20, 74),
    a::row(K::Instrument, "CLARIOstar", 7, 5, 9),
    a::row(K::Instrument, "Cytation3", 0, 0, 1),
    a::row(K::Instrument, "Cytation5", 2, 1, 2),
    a::row(K::Instrument, "EnVision", 7, 4, 7),
    a::row(K::Instrument, "Example Device", 1, 1, 1),
    a::row(K::Instrument, "Generic Reader", 1, 1, 1),
    a::row(K::Instrument, "PHERAstar", 3, 1, 3),
    a::row(K::Instrument, "SPECTRAmax 340PC", 1, 1, 1),
    a::row(K::Instrument, "SPECTRAmax M2e", 1, 1, 1),
    a::row(K::Instrument, "SPECTRAmax M5", 2, 1, 2),
    a::row(K::Instrument, "Spark", 2, 1, 4),
    a::row(K::Instrument, "SpectraMax ABS", 2, 1, 2),
    a::row(K::Instrument, "SpectraMax M3", 6, 1, 6),
    a::row(K::Instrument, "SpectraMax M5", 0, 0, 1),
    a::row(K::Instrument, "Synergy H1", 8, 3, 9),
    a::row(K::Instrument, "Synergy HT", 1, 1, 1),
    a::row(K::Instrument, "Synergy HTX", 1, 1, 1),
    a::row(K::Instrument, "ULTRA", 1, 1, 1),
    a::row(K::Instrument, "Varioskan LUX", 1, 1, 1),
    a::row(K::Instrument, "infinite 200Pro", 7, 5, 7),
    a::row(K::Layout, "container gen5-experiment", 7, 4, 9),
    a::row(K::Layout, "container softmax-pro-5-document", 4, 2, 4),
    a::row(K::Layout, "container softmax-pro-document", 8, 2, 9),
    a::row(K::Layout, "container text (comma)", 21, 9, 21),
    a::row(K::Layout, "container text (semicolon)", 2, 2, 2),
    a::row(K::Layout, "container text (tab)", 16, 3, 16),
    a::row(K::Layout, "container xlsx", 11, 5, 15),
    a::row(K::Layout, "export format PlateFormat", 3, 1, 3),
    a::row(K::Layout, "export format TimeFormat", 1, 1, 1),
    a::row(K::Writer, "EnVision Workstation", 7, 4, 7),
    a::row(K::Writer, "Gen5", 18, 6, 20),
    a::row(K::Writer, "Kaleido", 1, 1, 1),
    a::row(K::Writer, "MARS", 11, 6, 11),
    a::row(K::Writer, "Magellan", 2, 1, 2),
    a::row(K::Writer, "SMART Control", 1, 1, 3),
    a::row(K::Writer, "SkanIt", 2, 1, 2),
    a::row(K::Writer, "SoftMax Pro", 16, 5, 17),
    a::row(K::Writer, "i-control", 9, 6, 11),
    a::row(K::WriterVersion, "EnVision Workstation 1", 7, 4, 7),
    a::row(K::WriterVersion, "Gen5 1", 3, 1, 3),
    a::row(K::WriterVersion, "Gen5 2", 1, 1, 2),
    a::row(K::WriterVersion, "Gen5 3", 12, 4, 13),
    a::row(K::WriterVersion, "Kaleido 2", 1, 1, 1),
    a::row(K::WriterVersion, "SkanIt 7", 1, 1, 1),
    a::row(K::WriterVersion, "SoftMax Pro 5", 4, 2, 4),
    a::row(K::WriterVersion, "i-control 1", 3, 3, 3),
    a::row(K::WriterVersion, "i-control 2", 6, 3, 8),
];
// END GENERATED plate

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::text_book;

    #[test]
    fn an_export_without_values_is_unvalidated_for_tables() {
        // A table view whose second header line the reader does not read: no values at all.
        let t = "User: U,Path: C:\\BMG\\CLARIOstar\\U\\Data,Test run no.: 1\nTest name: x,Date: 3/06/2026,Time: 11:57:23 AM\nAbsorbance\n\nWell,Content,Raw Data (600),Raw Data (600)\n,Unknown axis,1,2\nA01,Sample X1,0.1,0.2\n";
        let ex = crate::vendors::bmg::parse(&text_book(t.as_bytes()), false);
        assert!(ex.blocks.iter().all(|b| b.obs.is_empty()));
        let o = left_out(&ex);
        assert!(o.undecoded.iter().any(|u| u.scope == [Scope::Tables]));
        assert!(o.undecoded.iter().any(|u| u.scope.is_empty()));
    }
}
