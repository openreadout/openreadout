//! Field- and file-level metadata: units, labels, normalized-field provenance, the source format,
//! the instrument and a JSON copy of `info`.

use std::collections::HashMap;

use openreadout_core::experiment::InfoOutput;
use openreadout_core::model::FileInfo;
use openreadout_core::provenance::ProvenanceMap;
use serde_json::{Map, Value};

/// Field metadata key: the physical unit of the column's values (`s`, `mV`, `ppm`, ...).
pub const KEY_UNIT: &str = "unit";
/// Field metadata key: the column's descriptive label (FCS `$PnS`, ...).
pub const KEY_LABEL: &str = "label";
/// Field metadata key: the source dtype as `info` reports it (`uint16`, `float32`, ...).
pub const KEY_DTYPE: &str = "openreadout.dtype";
/// Field or file metadata key: provenance of the normalized fields (JSON `{path: source}`).
pub const KEY_PROVENANCE: &str = "openreadout.provenance";
/// Field metadata key: the column's or channel's format-specific metadata (JSON).
pub const KEY_EXTRA: &str = "openreadout.extra";
/// Field metadata key: `value = raw * scale + offset` of a trace channel (JSON `[scale, offset]`).
pub const KEY_SCALING: &str = "openreadout.scaling";
/// File metadata key: the `openreadout` version that wrote the file.
pub const KEY_VERSION: &str = "openreadout.version";
/// File metadata key: what the file holds (`table`, `trace`, `spectra` or `scans`).
pub const KEY_KIND: &str = "openreadout.kind";
/// File metadata key: format id of the source file (`fcs`, `abf`, `thermo-raw`, ...).
pub const KEY_SOURCE_FORMAT: &str = "openreadout.source_format";
/// File metadata key: file name of the source file.
pub const KEY_SOURCE_FILE: &str = "openreadout.source_file";
/// File metadata key: the instrument, as JSON, when the source records one.
pub const KEY_INSTRUMENT: &str = "openreadout.instrument";
/// File metadata key: the exported table, trace or run as `info` describes it (JSON).
pub const KEY_OBJECT: &str = "openreadout.object";
/// File metadata key: the whole `info` output of the source file, with its `experiment` (JSON).
pub const KEY_INFO: &str = "openreadout.info";

/// File metadata key: the experiment (sample, instrument, method, acquisition) as `info` derives
/// it (JSON), when anything is known.
pub const KEY_EXPERIMENT: &str = "openreadout.experiment";

/// Which part of `info` an export comes from, for provenance matching.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Part<'a> {
    /// `tables`, `traces` or `spectra`.
    pub root: &'a str,
    pub index: u32,
    /// The trace's `extra.kind`, matched by provenance keys such as `traces[kind=time_domain]`.
    pub kind: Option<&'a str>,
}

/// The part of a provenance key after `root[...]` when the brackets select this part (`[]`,
/// `[3]`, `[kind=time_domain]`); `None` when the key is about something else.
fn rest_of<'k>(key: &'k str, part: Part<'_>) -> Option<&'k str> {
    let after = key.strip_prefix(part.root)?;
    let after = after.strip_prefix('[')?;
    let close = after.find(']')?;
    let sel = &after[..close];
    let ok = sel.is_empty()
        || sel.parse::<u32>().is_ok_and(|i| i == part.index)
        || sel
            .strip_prefix("kind=")
            .is_some_and(|k| part.kind == Some(k));
    ok.then(|| &after[close + 1..])
}

/// Provenance entries of `part` whose remaining path starts with one of `prefixes`, as a JSON
/// object (`{"traces[].channels[].unit": "spec"}`); `None` when there are none.
pub(crate) fn provenance_json(
    prov: &ProvenanceMap,
    part: Part<'_>,
    prefixes: &[&str],
) -> Option<String> {
    let mut m = Map::new();
    for (k, v) in prov {
        if let Some(rest) = rest_of(k, part)
            && prefixes.iter().any(|p| rest.starts_with(p))
        {
            m.insert(k.clone(), serde_json::to_value(v).unwrap_or(Value::Null));
        }
    }
    (!m.is_empty()).then(|| Value::Object(m).to_string())
}

/// The instrument that recorded the exported part: the experiment's instrument, else the run's
/// or first image's `InstrumentInfo`, else an `instrument`/`cytometer` entry of the part's `extra`.
fn instrument(out: &InfoOutput, part: Part<'_>) -> Option<Value> {
    if let Some(i) = out.experiment.as_ref().and_then(|e| e.instrument.as_ref())
        && let Ok(v) = serde_json::to_value(i)
    {
        return Some(v);
    }
    let info: &FileInfo = &out.file;
    let from_extra = |extra: &std::collections::BTreeMap<String, Value>| {
        ["instrument", "cytometer", "spectrometer"]
            .iter()
            .find_map(|k| extra.get(*k).cloned())
    };
    match part.root {
        "spectra" => info
            .spectra
            .iter()
            .find(|s| s.index == part.index)
            .and_then(|s| {
                s.instrument
                    .as_ref()
                    .and_then(|i| serde_json::to_value(i).ok())
                    .or_else(|| from_extra(&s.extra))
            }),
        "tables" => info
            .tables
            .iter()
            .find(|t| t.index == part.index)
            .and_then(|t| from_extra(&t.extra)),
        "traces" => info
            .traces
            .iter()
            .find(|t| t.index == part.index)
            .and_then(|t| from_extra(&t.extra)),
        _ => None,
    }
    .or_else(|| {
        info.images
            .iter()
            .find_map(|i| i.instrument.as_ref())
            .and_then(|i| serde_json::to_value(i).ok())
    })
}

/// File-level metadata of an export.
pub(crate) fn file_metadata(
    info: &InfoOutput,
    prov: &ProvenanceMap,
    part: Part<'_>,
    kind: &str,
    object: &Value,
    source_file: &str,
) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert(KEY_VERSION.into(), env!("CARGO_PKG_VERSION").into());
    m.insert(KEY_KIND.into(), kind.into());
    m.insert(KEY_SOURCE_FORMAT.into(), info.format.id.clone());
    m.insert(KEY_SOURCE_FILE.into(), source_file.into());
    if let Some(i) = instrument(info, part) {
        m.insert(KEY_INSTRUMENT.into(), i.to_string());
    }
    m.insert(KEY_OBJECT.into(), object.to_string());
    m.insert(
        KEY_INFO.into(),
        serde_json::to_string(info).unwrap_or_default(),
    );
    if let Some(e) = &info.experiment {
        m.insert(
            KEY_EXPERIMENT.into(),
            serde_json::to_string(e).unwrap_or_default(),
        );
    }
    if !prov.is_empty() {
        m.insert(
            KEY_PROVENANCE.into(),
            serde_json::to_string(prov).unwrap_or_default(),
        );
    }
    m
}

/// Add `k = v` when `v` is present and not empty.
pub(crate) fn put(m: &mut HashMap<String, String>, k: &str, v: Option<String>) {
    if let Some(v) = v.filter(|v| !v.is_empty()) {
        m.insert(k.into(), v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::provenance::Source;

    #[test]
    fn provenance_keys_select_the_part() {
        let mut p = ProvenanceMap::new();
        p.insert("traces[].channels[].unit".into(), Source::Spec);
        p.insert(
            "traces[kind=time_domain].channels[].scale".into(),
            Source::Inferred,
        );
        p.insert(
            "traces[kind=processed].channels[].scale".into(),
            Source::Spec,
        );
        p.insert("traces[1].channels[].offset".into(), Source::Spec);
        p.insert("tables[].columns[].name".into(), Source::Spec);
        p.insert("traces[].sample_rate_hz".into(), Source::Spec);
        let part = Part {
            root: "traces",
            index: 0,
            kind: Some("time_domain"),
        };
        let v: serde_json::Value =
            serde_json::from_str(&provenance_json(&p, part, &[".channels"]).unwrap()).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "traces[].channels[].unit",
                "traces[kind=time_domain].channels[].scale"
            ]
        );
        assert!(provenance_json(&p, part, &[".nothing"]).is_none());
    }
}
