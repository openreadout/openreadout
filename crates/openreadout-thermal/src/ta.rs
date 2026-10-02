//! TA Instruments Q-series data files as Universal Analysis reads them (`.001`, `.002`, …): a
//! UTF-16 text header, a form-feed, the signal count, then float32 records ended by a sentinel
//! record. Notes: `docs/formats/ta-universal-analysis.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Observations, Scope};
use openreadout_core::bytes::utf16le;
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile, SeriesTrace};
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

pub(crate) const FORMAT_ID: &str = "ta-universal-analysis";

/// Largest header accepted (UTF-16 code units).
const MAX_HEADER: usize = 1 << 20;

/// True when a file starts like a TA data file: a UTF-16LE BOM and `CLOSED` or `OPEN`.
pub(crate) fn looks_like(head: &[u8]) -> bool {
    head.starts_with(&[0xff, 0xfe]) && {
        let s = utf16le(head.get(2..head.len().min(64)).unwrap_or(&[]));
        s.starts_with("CLOSED\r\n") || s.starts_with("OPEN\r\n")
    }
}

/// Our name for a signal label (`Heat Flow (mW)` → `heat_flow`, unit `mW`).
fn signal(label: &str) -> (String, Option<String>) {
    let (name, unit) = match label.rsplit_once(" (") {
        Some((n, u)) if u.ends_with(')') => (n.trim(), Some(u.trim_end_matches(')').trim())),
        _ => (label.trim(), None),
    };
    let ours = match name {
        "Time" => "time".to_string(),
        "Temperature" => "temperature".to_string(),
        "Heat Flow" => "heat_flow".to_string(),
        "Weight" => "weight".to_string(),
        _ => {
            let mut s = String::new();
            for c in name.chars() {
                if c.is_ascii_alphanumeric() {
                    s.push(c.to_ascii_lowercase());
                } else if !s.ends_with('_') && !s.is_empty() {
                    s.push('_');
                }
            }
            s.trim_end_matches('_').to_string()
        }
    };
    let unit = unit.map(|u| match u {
        "ml/min" => "mL/min".to_string(),
        "°C" | "C" => "°C".to_string(),
        other => other.to_string(),
    });
    (ours, unit)
}

/// Parse a TA data file.
pub(crate) fn parse(b: &[u8]) -> Result<SeriesFile> {
    if !looks_like(b) {
        return Err(Error::corrupt(
            FORMAT_ID,
            "no UTF-16 `CLOSED`/`OPEN` header",
        ));
    }
    // the header: UTF-16LE text up to a form feed
    let mut units = Vec::new();
    let mut at = 2usize;
    let mut end = None;
    while at + 1 < b.len() && units.len() < MAX_HEADER {
        let c = u16::from_le_bytes([b[at], b[at + 1]]);
        at += 2;
        if c == 0x000C {
            end = Some(at);
            break;
        }
        units.push(c);
    }
    let Some(text_end) = end else {
        return Err(Error::corrupt(
            FORMAT_ID,
            "the header does not end in a form feed (no data)",
        ));
    };
    let text = String::from_utf16_lossy(&units);
    let mut hdr: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let (k, v) = line.split_once([' ', '\t']).unwrap_or((line, ""));
        hdr.push((k.trim().to_string(), v.to_string()));
    }
    let get = |k: &str| {
        hdr.iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.trim().to_string())
    };
    let nsig: usize = get("Nsig")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no `Nsig` line"))?;
    if nsig == 0 || nsig > 64 {
        return Err(Error::corrupt(FORMAT_ID, format!("Nsig {nsig}")));
    }
    let mut labels = Vec::with_capacity(nsig);
    for k in 1..=nsig {
        labels.push(get(&format!("Sig{k}")).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("no `Sig{k}` line for {nsig} signals"))
        })?);
    }
    let stored = *b
        .get(text_end)
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "the file ends after its header"))?;
    if usize::from(stored) != nsig {
        return Err(Error::Unsupported {
            format: FORMAT_ID,
            feature: format!("a data block of {stored} signals after a header naming {nsig}"),
            hint: Some("export the run as text from Universal Analysis".into()),
        });
    }
    let data_start = text_end + 1;
    let rec = 4 * nsig;
    let mut findings = Vec::new();
    let mut columns: Vec<Vec<f64>> = vec![Vec::new(); nsig];
    let mut at = data_start;
    let mut ended = false;
    while at + rec <= b.len() {
        let r = &b[at..at + rec];
        let v: Vec<f64> = r
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(f32::from_le_bytes(*c)))
            .collect();
        #[allow(clippy::float_cmp)] // an exact sentinel value
        if v[0] == -100.0 {
            ended = true;
            break;
        }
        for (c, x) in columns.iter_mut().zip(v) {
            c.push(x);
        }
        at += rec;
    }
    if !ended {
        findings.push(Finding::warning(
            "no_end_record",
            "the records do not end in the end-of-data record: the run may be incomplete or the file cut",
        ));
    }
    let n = columns[0].len();
    if n == 0 {
        if !ended {
            return Err(Error::corrupt(
                FORMAT_ID,
                "no data records and no end-of-data record",
            ));
        }
        findings.push(Finding::warning(
            "no_records",
            "the run stored no records (it ended before the first reading)",
        ));
    }
    // channels: time first (minutes → s)
    let mut channels = Vec::new();
    let mut values = Vec::new();
    let mut time_idx = None;
    for (k, l) in labels.iter().enumerate() {
        let (name, _) = signal(l);
        if name == "time" && time_idx.is_none() {
            time_idx = Some(k);
        }
    }
    let Some(ti) = time_idx else {
        return Err(Error::Unsupported {
            format: FORMAT_ID,
            feature: format!("signals without a time signal: {labels:?}"),
            hint: Some("export the run as text from Universal Analysis".into()),
        });
    };
    let (_, tunit) = signal(&labels[ti]);
    let t_scale = match tunit.as_deref() {
        Some("min") => 60.0,
        Some("s" | "sec") => 1.0,
        other => {
            return Err(Error::Unsupported {
                format: FORMAT_ID,
                feature: format!("a time signal in {other:?}"),
                hint: None,
            });
        }
    };
    let mut order: Vec<usize> = (0..nsig).collect();
    order.sort_by_key(|&k| u8::from(k != ti));
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for k in order {
        let (name, unit) = signal(&labels[k]);
        let c = seen.entry(name.clone()).or_insert(0);
        *c += 1;
        let name = if *c > 1 { format!("{name}_{c}") } else { name };
        let (unit, vals) = if k == ti {
            (
                Some("s".to_string()),
                columns[k].iter().map(|x| x * t_scale).collect(),
            )
        } else {
            (unit, std::mem::take(&mut columns[k]))
        };
        let mut ch = SeriesChannel::new(name, unit.as_deref(), "float32");
        ch.extra.insert("label".into(), json!(labels[k]));
        channels.push(ch);
        values.push(vals);
    }
    let mut extra = BTreeMap::new();
    extra.insert(
        "axis".into(),
        json!({"quantity": "time", "unit": "s", "irregular": true, "channel": 0, "size": n}),
    );
    extra.insert("kind".into(), json!("thermal_analysis"));
    // facts
    let mut vendor = Map::new();
    for (k, v) in &hdr {
        if k.is_empty() {
            continue;
        }
        match vendor.get_mut(k) {
            Some(Value::Array(a)) => a.push(json!(v)),
            Some(prev) => {
                let p = prev.take();
                *prev = json!([p, v]);
            }
            None => {
                vendor.insert(k.clone(), json!(v));
            }
        }
    }
    let mut facts = Facts::new();
    facts.set("instrument.vendor", "TA Instruments", "the file format");
    let instrument = get("Instrument").unwrap_or_default();
    let (model, firmware) = match instrument.split_once(" V") {
        Some((m, f)) => (m.trim().to_string(), Some(format!("V{}", f.trim()))),
        None => (instrument.trim().to_string(), None),
    };
    if !model.is_empty() {
        facts.set("instrument.model", &model, "`Instrument` header line");
    }
    if let Some(f) = &firmware {
        facts.set(
            "instrument.software_version",
            f,
            "`Instrument` header line (the instrument's firmware)",
        );
    }
    if let Some(v) = get("InstSerial") {
        facts.set("instrument.serial", &v, "`InstSerial` header line");
    }
    if let Some(v) = get("Operator") {
        facts.set("acquisition.operator", &v, "`Operator` header line");
    }
    if let Some(v) = get("Sample") {
        facts.set("sample.name", &v, "`Sample` header line");
    }
    if let Some(v) = get("Comment") {
        facts.set("acquisition.comment", &v, "`Comment` header line");
    }
    if let (Some(d), Some(t)) = (get("Date"), get("Time"))
        && d.len() == 10
        && d.as_bytes()[4] == b'-'
    {
        facts.set(
            "acquisition.started_at",
            &format!("{d}T{t}"),
            "`Date` and `Time` header lines (instrument local time)",
        );
    }
    if let Some(sz) = hdr
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("Size"))
        .map(|(_, v)| v.clone())
    {
        let mut f = sz.split_whitespace();
        if let (Some(v), Some(u)) = (
            f.next().and_then(|x| x.trim().parse::<f64>().ok()),
            f.next(),
        ) && u.trim() == "mg"
            && v > 0.0
        {
            facts.number("sample_mass", v, "mg", "`Size` header line");
        }
    }
    let steps: Vec<String> = hdr
        .iter()
        .filter(|(k, _)| k == "OrgMethod")
        .map(|(_, v)| v.clone())
        .collect();
    if !steps.is_empty() {
        facts.plain("method_steps", json!(steps), "`OrgMethod` header lines");
    }
    let duration = values
        .first()
        .and_then(|t| t.last())
        .copied()
        .unwrap_or(0.0)
        - values
            .first()
            .and_then(|t| t.first())
            .copied()
            .unwrap_or(0.0);
    if duration > 0.0 {
        facts.duration(duration, "the first and last time values");
    }
    let term = if model.starts_with("DSC") {
        Some("CHMO:0000684")
    } else if model.starts_with("TGA") {
        Some("CHMO:0000690")
    } else if model.starts_with("SDT") {
        Some("CHMO:0000681")
    } else {
        None
    };
    if let Some(t) = term {
        facts.technique(t, "the instrument model");
    }
    facts.measurement(
        MeasurementKind::Trace,
        vec![0],
        format!(
            "{} run, {n} records, {nsig} signals",
            if model.is_empty() {
                "thermal analysis"
            } else {
                model.as_str()
            }
        ),
        term,
    );
    let mut observations = Observations::default();
    let version = get("Version").map_or_else(|| "unknown".to_string(), |v| format!("Version {v}"));
    observations.feature(
        FeatureKind::FormatVersion,
        &version,
        &[Scope::Metadata, Scope::Traces],
    );
    if !model.is_empty() {
        observations.feature(FeatureKind::Instrument, &model, &[]);
    }
    if let Some(m) = get("Module") {
        observations.feature(FeatureKind::Acquisition, &m, &[]);
    }
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::Inferred);
    let entries = vec![
        LsEntry {
            kind: "block".into(),
            name: "header".into(),
            offset: Some(0),
            size: Some(text_end as u64),
            image: None,
            details: Value::Null,
        },
        LsEntry {
            kind: "block".into(),
            name: "records".into(),
            offset: Some(data_start as u64),
            size: Some((n * rec) as u64),
            image: None,
            details: json!({"signals": nsig, "records": n}),
        },
    ];
    Ok(SeriesFile {
        format_version: Some(version),
        traces: vec![SeriesTrace {
            name: "run".into(),
            channels,
            sweeps: vec![values],
            sample_rate_hz: 0.0,
            start_s: None,
            extra,
        }],
        tables: Vec::new(),
        experiment: Some(facts.build()),
        vendor: json!({"ta": vendor}),
        entries,
        findings,
        notes: Vec::new(),
        provenance,
        observations,
        checks: vec![
            "header signal count equal to the data block's".into(),
            "records up to the end-of-data record".into(),
        ],
        members: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_names() {
        assert_eq!(
            signal("Heat Flow (mW)"),
            ("heat_flow".into(), Some("mW".into()))
        );
        assert_eq!(
            signal("Rev Cp (mJ/°C)"),
            ("rev_cp".into(), Some("mJ/°C".into()))
        );
        assert_eq!(
            signal("Deriv. Weight (%/min)"),
            ("deriv_weight".into(), Some("%/min".into()))
        );
        assert_eq!(signal("Sample Purge Flow (mL/min)").0, "sample_purge_flow");
    }
}
