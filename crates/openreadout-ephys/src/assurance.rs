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

pub(crate) static EDF: AssuranceProfile = AssuranceProfile {
    format_id: "edf",
    observe: observe_edf,
    validated: EDF_VALIDATED,
    confidence: EDF_CONFIDENCE,
    basis: Basis::OpenSpec,
};

fn observe_edf(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [Scope::Metadata, Scope::Traces, Scope::Tables];
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &all);
    }
    for t in &info.traces {
        if let Some(c) = t.channels.first() {
            o.feature(K::SampleLayout, &c.dtype, &[Scope::Traces]);
        }
        if t.sweep_count > 1 {
            o.feature(K::Layout, "discontinuous records", &[Scope::Traces]);
        }
    }
    if info.traces.len() > 1 {
        o.feature(K::Layout, "several sample rates", &[Scope::Traces]);
    }
    if info
        .tables
        .iter()
        .any(|t| t.name.as_deref() == Some("annotations"))
    {
        o.feature(K::Record, "annotations", &[Scope::Tables]);
    }
    if !info.traces.is_empty() {
        o.calibration(
            "digital to physical values",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "digital × (physical max − physical min) / (digital max − digital min) + offset, per signal",
        );
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded(
            "cut-short files",
            &[Scope::Traces, Scope::Tables],
            n.to_string(),
        );
    }
    o
}

pub(crate) static BRAINVISION: AssuranceProfile = AssuranceProfile {
    format_id: "brainvision",
    observe: observe_brainvision,
    validated: BRAINVISION_VALIDATED,
    confidence: BRAINVISION_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe_brainvision(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &[Scope::Metadata]);
    }
    for t in &info.traces {
        if let Some(f) = a::extra_str(&t.extra, "binary_format") {
            o.feature(K::SampleLayout, f, &[Scope::Traces]);
        }
        if let Some(f) = a::extra_str(&t.extra, "orientation") {
            o.feature(K::Layout, f, &[Scope::Traces]);
        }
        if t.sweep_count > 1 {
            o.feature(K::Layout, "marker-based segments", &[Scope::Traces]);
        }
        if let Some(w) = a::extra_str(&t.extra, "application") {
            o.context(K::Writer, w);
        }
        o.calibration(
            "stored values to channel units",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "every stored sample × the channel's resolution",
        );
    }
    if info
        .tables
        .iter()
        .any(|t| t.name.as_deref() == Some("markers"))
    {
        o.feature(K::Record, "markers", &[Scope::Tables]);
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded(
            "cut-short files",
            &[Scope::Traces, Scope::Tables],
            n.to_string(),
        );
    }
    o
}

pub(crate) static EEGLAB: AssuranceProfile = AssuranceProfile {
    format_id: "eeglab",
    observe: observe_eeglab,
    validated: EEGLAB_VALIDATED,
    confidence: EEGLAB_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe_eeglab(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [Scope::Metadata, Scope::Traces, Scope::Tables];
    if let Some(v) = &info.format_version {
        o.feature(K::FormatVersion, v, &all);
    }
    if a::note_with(info, ", compressed)").is_some() {
        o.feature(K::Codec, "zlib (MAT 7)", &all);
    }
    for t in &info.traces {
        if let Some(s) = a::extra_str(&t.extra, "sample_storage") {
            let dtype = t.channels.first().map_or("", |c| c.dtype.as_str());
            o.feature(K::SampleLayout, format!("{s} {dtype}"), &[Scope::Traces]);
        }
        if t.sweep_count > 1 {
            o.feature(K::Layout, "epochs", &[Scope::Traces]);
        }
        if let Some(w) = a::extra_str(&t.extra, "mat_writer") {
            o.context(K::Writer, w);
        }
        if let Some(v) = a::extra_str(&t.extra, "application_version") {
            o.context(K::WriterVersion, v);
        }
    }
    if info
        .tables
        .iter()
        .any(|t| t.name.as_deref() == Some("events"))
    {
        o.feature(K::Record, "events", &[Scope::Tables]);
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded(
            "cut-short files",
            &[Scope::Traces, Scope::Tables],
            n.to_string(),
        );
    }
    o
}

pub(crate) static TDT: AssuranceProfile = AssuranceProfile {
    format_id: "tdt-tank",
    observe: observe_tdt,
    validated: TDT_TANK_VALIDATED,
    confidence: TDT_TANK_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe_tdt(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    let all = [Scope::Metadata, Scope::Traces, Scope::Tables];
    if let Some(v) = &info.format_version {
        // `Synapse 41743P`, from the `.tin`; the reader does not branch on it
        o.context(K::WriterVersion, v);
    }
    if a::note_with(info, "TDT tank").is_some() {
        o.feature(K::Layout, "tank", &all);
    } else {
        o.feature(K::Layout, "block", &all);
    }
    for t in &info.traces {
        o.feature(K::Record, "stream", &[Scope::Traces]);
        if let Some(f) = a::extra_str(&t.extra, "data_format") {
            o.feature(K::SampleLayout, f, &[Scope::Traces]);
        }
    }
    for t in &info.tables {
        match t.name.as_deref() {
            Some("spikes") => o.feature(K::Record, "snippet", &[Scope::Tables]),
            Some("epocs") => {
                o.feature(K::Record, "strobe", &[Scope::Tables]);
                if t.extra
                    .get("paired_offsets")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
                    > 0
                {
                    o.feature(K::Record, "strobe-off", &[Scope::Tables]);
                }
            }
            _ => {}
        }
    }
    if let Some(n) = a::note_with(info, "is not read") {
        o.undecoded("unread stores", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "not decoded") {
        o.undecoded("scalar records", &[Scope::Tables], n.to_string());
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded(
            "data past the end of the .tev",
            &[Scope::Traces, Scope::Tables],
            n.to_string(),
        );
    }
    o
}

pub(crate) static AXONA: AssuranceProfile = AssuranceProfile {
    format_id: "axona-dacqusb",
    observe: observe_axona,
    validated: AXONA_DACQUSB_VALIDATED,
    confidence: AXONA_DACQUSB_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe_axona(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.context(K::WriterVersion, v);
    }
    if !info.traces.is_empty() {
        o.feature(K::Layout, "raw packets", &[Scope::Traces]);
        o.calibration(
            "ADC counts to µV",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "int16 samples × 1000 × ADC_fullscale_mv / (gain_ch × 32768)",
        );
    }
    for t in &info.tables {
        if t.name.as_deref() == Some("spikes") {
            o.feature(K::Record, "tetrode spikes", &[Scope::Tables]);
        }
    }
    if let Some(n) = a::note_with(info, "not read:") {
        o.undecoded("EEG, position and other session files", &[], n.to_string());
    }
    if let Some(n) = a::note_with(info, "structural problems found") {
        o.undecoded(
            "cut-short files",
            &[Scope::Traces, Scope::Tables],
            n.to_string(),
        );
    }
    o
}

pub(crate) static MCS_RAW: AssuranceProfile = AssuranceProfile {
    format_id: "mcs-raw",
    observe: observe_mcs_raw,
    validated: MCS_RAW_VALIDATED,
    confidence: MCS_RAW_CONFIDENCE,
    basis: Basis::PriorArt,
};

fn observe_mcs_raw(info: &FileInfo) -> Observations {
    let mut o = Observations::default();
    if let Some(v) = &info.format_version {
        o.feature(K::WriterVersion, v, &[Scope::Metadata, Scope::Traces]);
    }
    if !info.traces.is_empty() {
        o.calibration(
            "ADC counts to channel units",
            CalibrationStatus::Applied,
            &[Scope::Traces],
            "uint16 samples × El gain − El gain × ADC zero",
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
    a::row(K::Acquisition, "current-clamp", 7, 4, 7),
    a::row(K::Acquisition, "no-mode", 1, 1, 1),
    a::row(K::Acquisition, "whole-cell", 6, 4, 6),
    a::row(K::FormatVersion, "2.11", 1, 1, 1),
    a::row(K::FormatVersion, "2x60", 2, 1, 2),
    a::row(K::FormatVersion, "2x65", 2, 2, 2),
    a::row(K::FormatVersion, "2x90.2", 2, 2, 2),
    a::row(K::FormatVersion, "2x90.3", 3, 2, 3),
    a::row(K::Layout, "pulsed tree 1000", 3, 2, 3),
    a::row(K::Layout, "pulsed tree 9", 7, 5, 7),
    a::row(K::Layout, "series split by sample grid", 1, 1, 1),
    a::row(K::Record, "virtual traces", 1, 1, 1),
    a::row(K::SampleLayout, "float32", 1, 1, 1),
    a::row(K::SampleLayout, "int16", 10, 6, 10),
];
// END GENERATED heka-patchmaster

// BEGIN GENERATED ced-spike2 (cargo xtask assurance-audit --write; do not edit)
const CED_SPIKE2_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const CED_SPIKE2_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 23),
    a::row(K::FormatVersion, "3", 2, 2, 2),
    a::row(K::FormatVersion, "4", 1, 1, 1),
    a::row(K::FormatVersion, "5", 6, 2, 6),
    a::row(K::FormatVersion, "6", 4, 3, 4),
    a::row(K::FormatVersion, "7", 4, 4, 4),
    a::row(K::FormatVersion, "9", 1, 1, 1),
    a::row(K::FormatVersion, "smrx 0.1", 1, 1, 1),
    a::row(K::FormatVersion, "smrx 1.1", 3, 1, 5),
    a::row(K::Record, "adc", 21, 11, 23),
    a::row(K::Record, "adc-mark", 3, 1, 3),
    a::row(K::Record, "event-falling", 1, 1, 1),
    a::row(K::Record, "event-level", 1, 1, 1),
    a::row(K::Record, "event-rising", 6, 4, 6),
    a::row(K::Record, "marker", 6, 3, 7),
    a::row(K::Record, "text-mark", 5, 3, 6),
    a::row(K::WriterVersion, "00000000", 1, 1, 1),
    a::row(K::WriterVersion, "S2050123", 1, 1, 1),
    a::row(K::WriterVersion, "S2050210", 1, 1, 1),
    a::row(K::WriterVersion, "S2051454", 3, 1, 3),
    a::row(K::WriterVersion, "S2061895", 1, 1, 1),
    a::row(K::WriterVersion, "S2071033", 1, 1, 1),
    a::row(K::WriterVersion, "S2071345", 2, 1, 2),
    a::row(K::WriterVersion, "S2071431", 1, 1, 1),
    a::row(K::WriterVersion, "S2071635", 1, 1, 1),
    a::row(K::WriterVersion, "S2072307", 1, 1, 1),
    a::row(K::WriterVersion, "S2083226", 1, 1, 1),
    a::row(K::WriterVersion, "S2083331", 1, 1, 1),
    a::row(K::WriterVersion, "S2090130", 2, 1, 2),
    a::row(K::WriterVersion, "S2090302", 1, 1, 2),
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
    a::row(K::FormatVersion, "Open Ephys format 0.6", 2, 2, 2),
    a::row(K::Layout, "binary", 11, 1, 11),
    a::row(K::Layout, "legacy", 7, 4, 7),
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

// BEGIN GENERATED axona-dacqusb (cargo xtask assurance-audit --write; do not edit)
const AXONA_DACQUSB_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const AXONA_DACQUSB_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 3),
    a::row(K::Layout, "raw packets", 2, 1, 2),
    a::row(K::Record, "tetrode spikes", 2, 1, 2),
    a::row(K::WriterVersion, "1.2.2.16", 3, 1, 3),
];
// END GENERATED axona-dacqusb

// BEGIN GENERATED mcs-raw (cargo xtask assurance-audit --write; do not edit)
const MCS_RAW_CONFIDENCE: Confidence = Confidence::Low;
#[rustfmt::skip]
const MCS_RAW_VALIDATED: &[Validated] = &[
    a::row(K::WriterVersion, "2.6.10", 1, 1, 1),
];
// END GENERATED mcs-raw

// BEGIN GENERATED tdt-tank (cargo xtask assurance-audit --write; do not edit)
const TDT_TANK_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const TDT_TANK_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 8),
    a::row(K::Layout, "block", 5, 1, 5),
    a::row(K::Layout, "tank", 6, 6, 6),
    a::row(K::Record, "snippet", 2, 2, 2),
    a::row(K::Record, "stream", 8, 6, 8),
    a::row(K::Record, "strobe", 9, 6, 9),
    a::row(K::Record, "strobe-off", 5, 4, 5),
    a::row(K::SampleLayout, "float32", 6, 5, 6),
    a::row(K::SampleLayout, "int16", 2, 2, 2),
    a::row(K::WriterVersion, "Synapse 39174P", 1, 1, 1),
    a::row(K::WriterVersion, "Synapse 41743P", 1, 1, 1),
    a::row(K::WriterVersion, "Synapse 44132P", 1, 1, 1),
    a::row(K::WriterVersion, "Synapse 47410P", 1, 1, 1),
    a::row(K::WriterVersion, "Synapse 49313", 1, 1, 1),
    a::row(K::WriterVersion, "Synapse 50105", 1, 1, 1),
];
// END GENERATED tdt-tank

// BEGIN GENERATED edf (cargo xtask assurance-audit --write; do not edit)
const EDF_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const EDF_VALIDATED: &[Validated] = &[
    a::row(K::FormatVersion, "BDF", 2, 2, 2),
    a::row(K::FormatVersion, "BDF+C", 3, 3, 3),
    a::row(K::FormatVersion, "EDF", 5, 5, 5),
    a::row(K::FormatVersion, "EDF+C", 16, 10, 16),
    a::row(K::FormatVersion, "EDF+D", 1, 1, 1),
    a::row(K::Layout, "several sample rates", 4, 4, 4),
    a::row(K::Record, "annotations", 13, 9, 14),
    a::row(K::SampleLayout, "int16", 21, 13, 21),
    a::row(K::SampleLayout, "int24", 5, 5, 5),
];
// END GENERATED edf

// BEGIN GENERATED brainvision (cargo xtask assurance-audit --write; do not edit)
const BRAINVISION_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const BRAINVISION_VALIDATED: &[Validated] = &[
    a::row(K::Field, "experiment.acquisition.started_at", 0, 0, 8),
    a::row(K::FormatVersion, "1", 9, 7, 9),
    a::row(K::FormatVersion, "2", 4, 2, 4),
    a::row(K::Layout, "MULTIPLEXED", 12, 7, 12),
    a::row(K::Layout, "VECTORIZED", 1, 1, 1),
    a::row(K::Layout, "marker-based segments", 4, 1, 4),
    a::row(K::Record, "markers", 11, 7, 11),
    a::row(K::SampleLayout, "IEEE_FLOAT_32", 9, 6, 9),
    a::row(K::SampleLayout, "INT_16", 3, 3, 3),
    a::row(K::SampleLayout, "INT_32", 1, 1, 1),
    a::row(K::Writer, "BrainVision Analyzer", 6, 2, 6),
    a::row(K::Writer, "BrainVision Recorder", 2, 2, 2),
    a::row(K::Writer, "FieldTrip", 2, 2, 2),
    a::row(K::Writer, "pybv", 3, 3, 3),
];
// END GENERATED brainvision

// BEGIN GENERATED eeglab (cargo xtask assurance-audit --write; do not edit)
const EEGLAB_CONFIDENCE: Confidence = Confidence::High;
#[rustfmt::skip]
const EEGLAB_VALIDATED: &[Validated] = &[
    a::row(K::FormatVersion, "MAT 5.0", 11, 9, 11),
    a::row(K::FormatVersion, "MAT 7.3", 2, 2, 2),
    a::row(K::Layout, "epochs", 4, 4, 4),
    a::row(K::Record, "events", 10, 8, 10),
    a::row(K::SampleLayout, "embedded float32", 6, 6, 6),
    a::row(K::SampleLayout, "embedded float64", 1, 1, 1),
    a::row(K::SampleLayout, "fdt float32", 6, 5, 6),
    a::row(K::Writer, "MATLAB MACA64", 2, 2, 2),
    a::row(K::Writer, "MATLAB PCWIN64", 8, 8, 8),
    a::row(K::Writer, "SciPy", 3, 1, 3),
    a::row(K::WriterVersion, "14.1.2", 1, 1, 1),
    a::row(K::WriterVersion, "2020.0", 1, 1, 1),
    a::row(K::WriterVersion, "2022.0", 1, 1, 1),
    a::row(K::WriterVersion, "2022.1", 1, 1, 1),
    a::row(K::WriterVersion, "2023.1", 1, 1, 1),
    a::row(K::WriterVersion, "2024.0", 1, 1, 1),
    a::row(K::WriterVersion, "2024.2", 1, 1, 1),
    a::row(K::WriterVersion, "2025.0.0", 2, 2, 2),
    a::row(K::WriterVersion, "2025.1.0", 1, 1, 1),
    a::row(K::WriterVersion, "synthetic (not written by EEGLAB)", 2, 1, 2),
];
// END GENERATED eeglab

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
