//! Sartorius (ForteBio) Octet biolayer-interferometry result files (`.frd`, one per biosensor):
//! XML with the experiment, the sensor and every assay step with its time and wavelength-shift
//! values (base64 of little-endian float32). Layout: `docs/formats/sartorius-octet.md`.

use std::collections::BTreeMap;

use base64::Engine as _;
use openreadout_core::assurance::{FeatureKind, Observations, Scope};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{
    Facts, SeriesChannel, SeriesColumn, SeriesFile, SeriesTable, SeriesTrace,
};
use openreadout_core::xml::child;
use openreadout_core::{Error, Result};
use roxmltree::Node;
use serde_json::{Map, Value, json};

pub(crate) const FORMAT_ID: &str = "sartorius-octet-frd";

/// Largest file read (the largest seen is 650 kB).
pub(crate) const MAX_BYTES: u64 = 1 << 30;

fn text<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(n, name)
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn num(n: Node<'_, '_>, name: &str) -> Option<f64> {
    text(n, name).and_then(|s| s.parse().ok())
}

/// A value where the file uses -1 for "not set".
fn set(v: Option<f64>) -> f64 {
    match v {
        Some(x) if x.is_finite() && (x + 1.0).abs() > f64::EPSILON => x,
        _ => f64::NAN,
    }
}

/// True when the start of a file is an Octet result document.
pub(crate) fn looks_like(head: &[u8]) -> bool {
    let s = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    s.contains("<ExperimentResults") && s.contains("<ExperimentInfo")
}

/// The float32 values of a base64 data element; checks its `Points` attribute.
fn values(n: Node<'_, '_>, what: &str, step: usize) -> Result<Vec<f64>> {
    let t: String = n
        .text()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(t.as_bytes())
        .map_err(|e| {
            Error::corrupt(FORMAT_ID, format!("step {step}: {what} is not base64: {e}"))
        })?;
    if bytes.len() % 4 != 0 {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "step {step}: {what} holds {} bytes, not float32 values",
                bytes.len()
            ),
        ));
    }
    let v: Vec<f64> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f64::from(f32::from_le_bytes(*c)))
        .collect();
    if let Some(p) = n.attribute("Points").and_then(|p| p.parse::<usize>().ok())
        && p != v.len()
    {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("step {step}: {what} says {p} points and holds {}", v.len()),
        ));
    }
    Ok(v)
}

/// Parse an Octet `.frd` document.
pub(crate) fn parse(xml: &str) -> Result<SeriesFile> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|e| Error::corrupt(FORMAT_ID, format!("the XML does not parse: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "ExperimentResults" {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "root element `{}`, not `ExperimentResults`",
                root.tag_name().name()
            ),
        ));
    }
    let info = child(root, "ExperimentInfo")
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no `ExperimentInfo`"))?;
    let kinetics = child(root, "KineticsData").ok_or_else(|| Error::Unsupported {
        format: FORMAT_ID,
        feature: format!(
            "an Octet result without kinetics data (experiment type {})",
            text(info, "ExperimentType").unwrap_or("?")
        ),
        hint: Some("only kinetics experiments (KineticsData) are read; export the data from the Octet analysis software".into()),
    })?;
    let mut time = Vec::new();
    let mut response = Vec::new();
    let mut step_of = Vec::new();
    let mut rows: Vec<BTreeMap<&'static str, Value>> = Vec::new();
    let mut findings = Vec::new();
    for (k, st) in kinetics
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "Step")
        .enumerate()
    {
        let step = k + 1;
        let x = child(st, "AssayXData")
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("step {step}: no AssayXData")))?;
        let y = child(st, "AssayYData")
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("step {step}: no AssayYData")))?;
        let xv = values(x, "AssayXData", step)?;
        let yv = values(y, "AssayYData", step)?;
        if xv.len() != yv.len() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("step {step}: {} times and {} responses", xv.len(), yv.len()),
            ));
        }
        if let (Some(prev), Some(first)) = (time.last(), xv.first())
            && first < prev
        {
            findings.push(Finding::warning(
                "time_goes_back",
                format!("step {step} starts at {first} s, before the previous step ended"),
            ));
        }
        let n = xv.len();
        let common = child(st, "CommonData");
        let cd = |name: &str| {
            common
                .and_then(|common| text(common, name))
                .unwrap_or("")
                .to_string()
        };
        let cn = |name: &str| set(common.and_then(|common| num(common, name)));
        let mut row: BTreeMap<&'static str, Value> = BTreeMap::new();
        row.insert("step", json!(step));
        row.insert("name", json!(text(st, "StepName").unwrap_or("")));
        row.insert("type", json!(text(st, "StepType").unwrap_or("")));
        row.insert("status", json!(text(st, "StepStatus").unwrap_or("")));
        row.insert("sample_id", json!(cd("SampleID")));
        row.insert("well_type", json!(cd("WellType")));
        row.insert("sample_row", json!(cd("SampleRow")));
        row.insert("sample_location", json!(cn("SampleLocation")));
        row.insert("concentration", json!(cn("Concentration")));
        row.insert("concentration_unit", json!(cd("ConcentrationUnits")));
        row.insert("molar_concentration", json!(cn("MolarConcentration")));
        row.insert("molar_concentration_unit", json!(cd("MolarConcUnits")));
        row.insert("molecular_weight", json!(cn("MolecularWeight")));
        row.insert("temperature", json!(cn("Temperature")));
        row.insert("start", json!(xv.first().copied().unwrap_or(f64::NAN)));
        row.insert("assay_time", json!(cn("AssayTime")));
        row.insert("actual_time", json!(set(num(st, "ActualTime"))));
        row.insert("cycle_time", json!(set(num(st, "CycleTime"))));
        row.insert("flow_rate", json!(set(num(st, "FlowRate"))));
        row.insert("points", json!(n));
        rows.push(row);
        time.extend(xv);
        response.extend(yv);
        step_of.extend(std::iter::repeat_n(step as f64, n));
    }
    if rows.is_empty() {
        return Err(Error::corrupt(FORMAT_ID, "no assay steps"));
    }
    let n = time.len();
    let mut ch_t = SeriesChannel::new("time", Some("s"), "float32");
    ch_t.extra.insert("label".into(), json!("AssayXData"));
    let mut ch_r = SeriesChannel::new("response", Some("nm"), "float32");
    ch_r.extra.insert("label".into(), json!("AssayYData"));
    let ch_s = SeriesChannel::new("step", None, "int32");
    let mut extra = BTreeMap::new();
    extra.insert(
        "axis".into(),
        json!({"quantity": "time", "unit": "s", "irregular": true, "channel": 0, "size": n}),
    );
    extra.insert("kind".into(), json!("sensorgram"));
    extra.insert("steps_table".into(), json!(0));
    let sensor = text(info, "SensorName").unwrap_or("").to_string();
    let trace = SeriesTrace {
        name: if sensor.is_empty() {
            "sensorgram".into()
        } else {
            format!("sensor {sensor}")
        },
        channels: vec![ch_t, ch_r, ch_s],
        sweeps: vec![vec![time, response, step_of]],
        sample_rate_hz: 0.0,
        start_s: None,
        extra,
    };
    // steps table
    let col_num = |key: &'static str, name: &str, unit: Option<&str>| {
        SeriesColumn::numbers(
            name,
            unit,
            rows.iter()
                .map(|r| r.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN))
                .collect(),
        )
    };
    let col_text = |key: &'static str, name: &str| {
        let t: Vec<String> = rows
            .iter()
            .map(|r| r.get(key).and_then(Value::as_str).unwrap_or("").to_string())
            .collect();
        SeriesColumn::texts(name, &t)
    };
    let table = SeriesTable {
        name: "steps".into(),
        columns: vec![
            col_num("step", "step", None),
            col_text("name", "name"),
            col_text("type", "type"),
            col_text("status", "status"),
            col_text("sample_id", "sample_id"),
            col_text("well_type", "well_type"),
            col_text("sample_row", "sample_row"),
            col_num("sample_location", "sample_location", None),
            col_num("concentration", "concentration", None),
            col_text("concentration_unit", "concentration_unit"),
            col_num("molar_concentration", "molar_concentration", None),
            col_text("molar_concentration_unit", "molar_concentration_unit"),
            col_num("molecular_weight", "molecular_weight", None),
            col_num("temperature", "temperature", Some("°C")),
            col_num("start", "start", Some("s")),
            col_num("assay_time", "assay_time", Some("s")),
            col_num("actual_time", "actual_time", Some("s")),
            col_num("cycle_time", "cycle_time", Some("s")),
            col_num("flow_rate", "flow_rate", Some("rpm")),
            col_num("points", "points", None),
        ],
        extra: BTreeMap::new(),
    };
    // facts
    let mut facts = Facts::new();
    facts.set(
        "instrument.vendor",
        "Sartorius (ForteBio)",
        "the file format",
    );
    if let Some(v) = text(info, "InstrumentType") {
        facts.set("instrument.model", v, "`ExperimentInfo/InstrumentType`");
    }
    if let Some(v) = text(info, "InstrumentSerial") {
        facts.set("instrument.serial", v, "`ExperimentInfo/InstrumentSerial`");
    }
    if let Some(v) = text(info, "WritingSW") {
        facts.set(
            "instrument.software_version",
            v,
            "`ExperimentInfo/WritingSW`",
        );
    }
    if let Some(v) = text(info, "UserName") {
        facts.set("acquisition.operator", v, "`ExperimentInfo/UserName`");
    }
    if let Some(v) = text(info, "StartDateTime") {
        facts.set(
            "acquisition.started_at",
            v,
            "`ExperimentInfo/StartDateTime` (instrument PC local time)",
        );
    }
    if let Some(v) = text(info, "ExpDescription") {
        facts.set("acquisition.comment", v, "`ExperimentInfo/ExpDescription`");
    }
    let method = [
        text(info, "ExperimentType"),
        text(info, "ExperimentSubType"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    if !method.is_empty() {
        facts.set(
            "method.name",
            &method,
            "`ExperimentType` and `ExperimentSubType`",
        );
    }
    for (key, name) in [
        ("SensorName", "sensor"),
        ("SensorType", "sensor_type"),
        ("SensorRole", "sensor_role"),
        ("PlateName", "plate_name"),
    ] {
        if let Some(v) = text(info, key) {
            facts.plain(name, json!(v), &format!("`ExperimentInfo/{key}`"));
        }
    }
    let duration = trace.sweeps[0][0]
        .last()
        .zip(trace.sweeps[0][0].first())
        .map_or(0.0, |(l, f)| l - f);
    if duration > 0.0 {
        facts.duration(duration, "the first and last time values");
    }
    facts.measurement(
        MeasurementKind::Trace,
        vec![0],
        format!(
            "biolayer interferometry sensorgram of sensor {sensor}: {} steps, {n} points",
            rows.len()
        ),
        None,
    );
    let mut observations = Observations::default();
    observations.feature(
        FeatureKind::FormatVersion,
        format!("RTD {}", text(info, "RTDVersion").unwrap_or("?")),
        &[Scope::Metadata, Scope::Traces, Scope::Tables],
    );
    if let Some(sw) = text(info, "WritingSW") {
        let (w, v) = sw.split_once(' ').unwrap_or((sw, ""));
        observations.context(FeatureKind::Writer, w);
        if let Some(v) = openreadout_core::assurance::version_prefix(v, 2) {
            observations.context(FeatureKind::WriterVersion, format!("{w} {v}"));
        }
    }
    if let Some(v) = text(info, "InstrumentType") {
        observations.context(FeatureKind::Instrument, v);
    }
    let mut vendor = Map::new();
    for c in info.children().filter(Node::is_element) {
        if let Some(t) = c.text().map(str::trim).filter(|t| !t.is_empty()) {
            vendor.insert(c.tag_name().name().to_string(), json!(t));
        }
    }
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::PriorArt);
    provenance.insert("tables".into(), Source::Inferred);
    let entries = rows
        .iter()
        .map(|r| LsEntry {
            kind: "step".into(),
            name: format!("step {} {}", r["step"], r["name"].as_str().unwrap_or("")),
            offset: None,
            size: None,
            image: None,
            details: json!({"type": r["type"], "points": r["points"], "sample_id": r["sample_id"]}),
        })
        .collect();
    Ok(SeriesFile {
        format_version: text(info, "RTDVersion").map(str::to_string),
        traces: vec![trace],
        tables: vec![table],
        experiment: Some(facts.build()),
        vendor: json!({"octet": vendor}),
        entries,
        findings,
        notes: vec![
            "one biosensor: the wavelength shift (nm) against time (s) through every assay step as stored; table 0 lists the steps (sample, well type, concentration, times); kinetic fits are not in the file".into(),
        ],
        provenance,
        observations,
        checks: vec![
            "every step's time and response arrays decode to the stated number of points".into(),
        ],
        members: Vec::new(),
    })
}
