//! Assurance profiles (`docs/assurance.md`) of the readers in this crate: the variant features of
//! a file and the feature values the development corpus validates. The tables between the
//! GENERATED markers are written by `cargo xtask assurance-audit --write`.

use openreadout_core::FileInfo;
use openreadout_core::assurance::{
    self as a, AssuranceProfile, Basis, CalibrationStatus, FeatureKind as K, Observations, Scope,
    Validated,
};
use openreadout_core::provenance::Confidence;

pub(crate) static HEKA: AssuranceProfile = AssuranceProfile {
    format_id: "heka-patchmaster",
    observe: observe_heka,
    validated: HEKA_PATCHMASTER_VALIDATED,
    confidence: HEKA_PATCHMASTER_CONFIDENCE,
    basis: Basis::VendorDocs,
};

pub(crate) static SPIKE2: AssuranceProfile = AssuranceProfile {
    format_id: "ced-spike2",
    observe: observe_spike2,
    validated: CED_SPIKE2_VALIDATED,
    confidence: CED_SPIKE2_CONFIDENCE,
    basis: Basis::PriorArt,
};

pub(crate) static WINWCP: AssuranceProfile = AssuranceProfile {
    format_id: "winwcp",
    observe: observe_winwcp,
    validated: WINWCP_VALIDATED,
    confidence: WINWCP_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe_winwcp(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    for v in a::extra_values(info, "application_version") {
        o.context(K::WriterVersion, v);
    }
    for t in &info.traces {
        o.feature(
            K::Layout,
            format!("{} channels", t.channels.len()),
            &[Scope::Traces],
        );
    }
    if !info.traces.is_empty() {
        o.calibration(
            "ADC counts to channel units",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "int16 samples × VMax / ADCMAX / YG per record and channel; the zero level YZ is not subtracted",
        );
    }
    o
}

fn observe_spike2(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            v,
            &[Scope::Metadata, Scope::Traces, Scope::Tables],
        );
    }
    for v in a::extra_values(info, "application_version") {
        o.context(K::WriterVersion, v);
    }
    let mut kinds: Vec<String> = Vec::new();
    for t in &info.traces {
        if let Some(k) = a::extra_str(&t.extra, "kind") {
            let k = k.to_string();
            if !kinds.contains(&k) {
                kinds.push(k);
            }
        }
    }
    for k in kinds {
        o.feature(K::Record, k, &[Scope::Traces]);
    }
    for t in &info.tables {
        if let Some(serde_json::Value::Array(chans)) = t.extra.get("channels") {
            let mut seen: Vec<&str> = Vec::new();
            for c in chans {
                if let Some(k) = c.get("kind").and_then(serde_json::Value::as_str)
                    && !seen.contains(&k)
                    && c.get("items")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                        > 0
                {
                    seen.push(k);
                }
            }
            for k in seen {
                o.feature(K::Record, k, &[Scope::Tables]);
            }
        }
    }
    if info
        .traces
        .iter()
        .any(|t| t.channels.iter().any(|c| c.dtype == "int16"))
    {
        o.calibration(
            "ADC counts to channel units",
            CalibrationStatus::Applied,
            &[Scope::Traces, Scope::Tables],
            "int16 samples are scaled with each channel's scale / 6553.6 and offset, as Spike2 displays them",
        );
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded(
            "damaged blocks",
            &[Scope::Traces, Scope::Tables],
            n.to_string(),
        );
    }
    o
}

pub(crate) static OPEN_EPHYS: AssuranceProfile = AssuranceProfile {
    format_id: "open-ephys",
    observe: observe_open_ephys,
    validated: OPEN_EPHYS_VALIDATED,
    confidence: OPEN_EPHYS_CONFIDENCE,
    basis: Basis::VendorDocs,
};

fn observe_open_ephys(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        // `binary (GUI 0.6.0)` -> layout binary, version 0.6; `Open Ephys format 0.4`
        if let Some(g) = v
            .strip_prefix("binary (GUI ")
            .map(|s| s.trim_end_matches(')'))
        {
            o.feature(
                K::Layout,
                "binary",
                &[Scope::Metadata, Scope::Traces, Scope::Tables],
            );
            if let Some(p) = a::version_prefix(g, 2) {
                o.feature(
                    K::FormatVersion,
                    format!("GUI {p}"),
                    &[Scope::Metadata, Scope::Traces, Scope::Tables],
                );
            }
        } else {
            o.feature(
                K::Layout,
                "legacy",
                &[Scope::Metadata, Scope::Traces, Scope::Tables],
            );
            o.feature(
                K::FormatVersion,
                v,
                &[Scope::Metadata, Scope::Traces, Scope::Tables],
            );
        }
    }
    if info.traces.iter().any(|t| t.sweep_count > 1) {
        o.context(K::Layout, "several sweeps");
    }
    if info
        .traces
        .iter()
        .flat_map(|t| &t.channels)
        .any(|c| c.extra.get("unit_assumed") == Some(&serde_json::Value::Bool(true)))
    {
        o.assumed(
            "channel units",
            "structure.oebin gives no unit for some channels; µV (neural) or V (ADC) as the Open Ephys docs say",
        );
    }
    if let Some(n) = a::note_with(info, "are not read: records") {
        o.undecoded("irregular channel files", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "binary event stream(s)") {
        o.undecoded("binary event streams", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "spike group(s)") {
        o.undecoded("spike groups", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded("missing streams", &[], n.to_string());
    }
    o
}

/// `v2x90.2, 22-Nov-2016` → `2x90.2`; `1.2.0 [Build 1469]` → `1.2.0`: the program version
/// without its date or build number.
fn heka_version(v: &str) -> String {
    let head = v.split([',', '[']).next().unwrap_or(v).trim();
    head.trim_start_matches(['v', 'V']).to_string()
}

fn observe_heka(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(
            K::FormatVersion,
            heka_version(v),
            &[Scope::Metadata, Scope::Traces],
        );
    }
    for v in a::extra_values(info, "pulsed_tree_version") {
        o.feature(
            K::Layout,
            format!("pulsed tree {v}"),
            &[Scope::Metadata, Scope::Traces],
        );
    }
    let mut dtypes: Vec<&str> = Vec::new();
    for t in &info.traces {
        for c in &t.channels {
            if !dtypes.contains(&c.dtype.as_str()) {
                dtypes.push(c.dtype.as_str());
            }
        }
        if let Some(m) = t
            .channels
            .first()
            .and_then(|c| a::extra_str(&c.extra, "recording_mode"))
        {
            o.context(K::Acquisition, m);
        }
    }
    for d in dtypes {
        o.feature(K::SampleLayout, d, &[Scope::Traces]);
    }
    for (key, what) in [("virtual", "virtual traces"), ("leak", "leak traces")] {
        if info
            .traces
            .iter()
            .flat_map(|t| &t.channels)
            .any(|c| c.extra.get(key) == Some(&serde_json::Value::Bool(true)))
        {
            o.feature(K::Record, what, &[Scope::Traces]);
        }
    }
    if !info.traces.is_empty() {
        o.calibration(
            "ADC counts to amperes and volts",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "integer samples are multiplied by each trace record's data scaler, as PatchMaster displays them",
        );
        o.assumed(
            "zero subtraction",
            "values are not zero-subtracted; PatchMaster subtracts each channel's `zero_offset` only when its zero-subtraction option is on",
        );
    }
    if a::note_with(info, "parts of a series").is_some() {
        o.context(K::Layout, "series split by sample grid");
    }
    if let Some(n) = a::note_with(info, "are not read: features never validated") {
        o.undecoded("unvalidated traces", &[Scope::Traces], n.to_string());
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded("damaged trace records", &[Scope::Traces], n.to_string());
    }
    o
}

// BEGIN GENERATED heka-patchmaster (cargo xtask assurance-audit --write; do not edit)
const HEKA_PATCHMASTER_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const HEKA_PATCHMASTER_VALIDATED: &[Validated] = &[
    a::row(K::Acquisition, "current-clamp", 3, 3, 3),
    a::row(K::Acquisition, "no-mode", 1, 1, 1),
    a::row(K::Acquisition, "whole-cell", 4, 3, 4),
    a::row(K::FormatVersion, "2.11", 1, 1, 1),
    a::row(K::FormatVersion, "2x60", 2, 1, 2),
    a::row(K::FormatVersion, "2x65", 1, 1, 1),
    a::row(K::FormatVersion, "2x90.2", 1, 1, 1),
    a::row(K::FormatVersion, "2x90.3", 1, 1, 1),
    a::row(K::Layout, "pulsed tree 1000", 1, 1, 1),
    a::row(K::Layout, "pulsed tree 9", 5, 4, 5),
    a::row(K::Layout, "series split by sample grid", 1, 1, 1),
    a::row(K::Record, "virtual traces", 1, 1, 1),
    a::row(K::SampleLayout, "float32", 1, 1, 1),
    a::row(K::SampleLayout, "int16", 6, 5, 6),
];
// END GENERATED heka-patchmaster

// BEGIN GENERATED ced-spike2 (cargo xtask assurance-audit --write; do not edit)
const CED_SPIKE2_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const CED_SPIKE2_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 16),
    a::row(K::FormatVersion, "3", 2, 2, 2),
    a::row(K::FormatVersion, "4", 1, 1, 1),
    a::row(K::FormatVersion, "5", 3, 1, 3),
    a::row(K::FormatVersion, "6", 2, 2, 2),
    a::row(K::FormatVersion, "7", 3, 3, 3),
    a::row(K::FormatVersion, "9", 1, 1, 1),
    a::row(K::FormatVersion, "smrx 0.1", 1, 1, 1),
    a::row(K::FormatVersion, "smrx 1.1", 3, 1, 4),
    a::row(K::Record, "adc", 15, 8, 16),
    a::row(K::Record, "adc-mark", 3, 1, 3),
    a::row(K::Record, "event-falling", 1, 1, 1),
    a::row(K::Record, "event-level", 1, 1, 1),
    a::row(K::Record, "event-rising", 6, 4, 6),
    a::row(K::Record, "marker", 2, 1, 2),
    a::row(K::Record, "text-mark", 3, 1, 3),
    a::row(K::WriterVersion, "00000000", 1, 1, 1),
    a::row(K::WriterVersion, "S2050123", 1, 1, 1),
    a::row(K::WriterVersion, "S2050210", 1, 1, 1),
    a::row(K::WriterVersion, "S2061895", 1, 1, 1),
    a::row(K::WriterVersion, "S2071033", 1, 1, 1),
    a::row(K::WriterVersion, "S2071431", 1, 1, 1),
    a::row(K::WriterVersion, "S2071635", 1, 1, 1),
    a::row(K::WriterVersion, "S2072307", 1, 1, 1),
    a::row(K::WriterVersion, "S2083226", 1, 1, 1),
    a::row(K::WriterVersion, "S2083331", 1, 1, 1),
    a::row(K::WriterVersion, "S2090130", 2, 1, 2),
    a::row(K::WriterVersion, "S2091349", 0, 0, 1),
    a::row(K::WriterVersion, "S2103724", 1, 1, 1),
];
// END GENERATED ced-spike2

// BEGIN GENERATED open-ephys (cargo xtask assurance-audit --write; do not edit)
const OPEN_EPHYS_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const OPEN_EPHYS_VALIDATED: &[Validated] = &[
    a::row(K::FormatVersion, "GUI 0.4", 3, 1, 3),
    a::row(K::FormatVersion, "GUI 0.5", 2, 1, 2),
    a::row(K::FormatVersion, "GUI 0.6", 5, 1, 5),
    a::row(K::FormatVersion, "GUI 1.0", 1, 1, 1),
    a::row(K::FormatVersion, "Open Ephys format 0.4", 5, 3, 5),
    a::row(K::FormatVersion, "Open Ephys format 0.6", 1, 1, 1),
    a::row(K::Layout, "binary", 11, 1, 11),
    a::row(K::Layout, "legacy", 6, 3, 6),
    a::row(K::Layout, "several sweeps", 2, 1, 2),
];
// END GENERATED open-ephys

// BEGIN GENERATED winwcp (cargo xtask assurance-audit --write; do not edit)
const WINWCP_CONFIDENCE: Confidence = Confidence::Medium;
#[rustfmt::skip]
const WINWCP_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 2),
    a::row(K::FormatVersion, "8", 1, 1, 1),
    a::row(K::FormatVersion, "9", 3, 3, 3),
    a::row(K::Layout, "2 channels", 4, 3, 4),
    a::row(K::WriterVersion, "V5.3.7", 1, 1, 1),
    a::row(K::WriterVersion, "V5.8.1", 1, 1, 1),
];
// END GENERATED winwcp

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(heka_version("v2x90.2, 22-Nov-2016"), "2x90.2");
        assert_eq!(heka_version("v2x65, 19-Dec-2011"), "2x65");
        assert_eq!(heka_version("1.2.0 [Build 1469]"), "1.2.0");
    }
}
