//! Gamry Framework `.DTA` (the `EXPLAIN` text format). Notes: `docs/formats/gamry-dta.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Scope};
use openreadout_core::bytes::latin1;
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile, SeriesTrace};
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

pub(crate) const FORMAT_ID: &str = "gamry-dta";

/// True when a file starts like an `EXPLAIN` file.
pub(crate) fn looks_like(head: &[u8]) -> bool {
    let t = String::from_utf8_lossy(&head[..head.len().min(256)]);
    let mut lines = t.lines();
    lines.next().is_some_and(|l| l.trim() == "EXPLAIN")
        && lines.next().is_some_and(|l| l.starts_with("TAG\t"))
}

/// Our name and unit for a Gamry column (unit from the file's unit row when it has one).
fn column(name: &str) -> (&'static str, Option<&'static str>) {
    match name {
        "Pt" => ("point", None),
        "T" | "Time" => ("time", Some("s")),
        "Vf" => ("potential", Some("V")),
        "Im" => ("current", Some("A")),
        "Vu" => ("potential_uncompensated", Some("V")),
        "Sig" => ("signal", Some("V")),
        "Ach" => ("aux_channel", Some("V")),
        "IERange" => ("current_range", None),
        "Cycle" => ("cycle", None),
        "Freq" => ("frequency", Some("Hz")),
        "Zreal" => ("z_real", Some("Ω")),
        "Zimag" => ("z_imag", Some("Ω")),
        "Zsig" => ("z_signal", Some("V")),
        "Zmod" => ("z_modulus", Some("Ω")),
        "Zphz" => ("z_phase", Some("°")),
        "Idc" => ("current_dc", Some("A")),
        "Vdc" => ("potential_dc", Some("V")),
        "Vm" => ("potential_measured", Some("V")),
        "Temp" => ("temperature", Some("°C")),
        "Q" => ("charge", Some("C")),
        _ => ("", None),
    }
}

/// Technique terms for experiment tags.
fn technique(tag: &str) -> Option<&'static str> {
    match tag {
        "CV" | "RCV" => Some("CHMO:0000025"),
        "LSV" | "POLDYN" | "POTENTIODYNAMIC" => Some("CHMO:0000028"),
        "CHRONOA" => Some("CHMO:0000005"),
        "CHRONOP" => Some("CHMO:0000017"),
        "EISPOT" => Some("CHMO:0002937"),
        "EISGALV" => Some("CHMO:0000423"),
        "CORPOT" => Some("CHMO:0002933"),
        _ => None,
    }
}

/// A number in the file's locale (a decimal comma is accepted).
fn number(s: &str) -> Option<f64> {
    let s = s.trim();
    s.parse::<f64>()
        .ok()
        .or_else(|| s.replace(',', ".").parse::<f64>().ok())
}

struct Table {
    key: String,
    names: Vec<String>,
    units: Vec<String>,
    rows: Vec<Vec<String>>,
    declared: Option<usize>,
}

/// Parse a `.DTA` file.
pub(crate) fn parse(bytes: &[u8]) -> Result<SeriesFile> {
    let text: String = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => latin1(bytes),
    };
    let lines: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).collect();
    if lines.first().is_none_or(|l| l.trim() != "EXPLAIN") {
        return Err(Error::corrupt(
            FORMAT_ID,
            "the file does not start with EXPLAIN",
        ));
    }
    let mut header = Map::new();
    let mut tables: Vec<Table> = Vec::new();
    let mut findings = Vec::new();
    let mut k = 1usize;
    while k < lines.len() {
        let l = lines[k];
        if l.trim().is_empty() {
            k += 1;
            continue;
        }
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() >= 2 && f[1] == "TABLE" {
            let declared = f.get(2).and_then(|v| v.trim().parse::<usize>().ok());
            let names: Vec<String> = lines
                .get(k + 1)
                .map(|x| {
                    x.split('\t')
                        .skip(1)
                        .map(|s| s.trim().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let units: Vec<String> = lines
                .get(k + 2)
                .map(|x| {
                    x.split('\t')
                        .skip(1)
                        .map(|s| s.trim().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let mut rows = Vec::new();
            let mut j = k + 3;
            while j < lines.len() && lines[j].starts_with('\t') {
                rows.push(
                    lines[j]
                        .split('\t')
                        .skip(1)
                        .map(|s| s.trim().to_string())
                        .collect(),
                );
                j += 1;
            }
            if names.is_empty() {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("table {} has no column names", f[0]),
                ));
            }
            tables.push(Table {
                key: f[0].to_string(),
                names,
                units,
                rows,
                declared,
            });
            k = j;
            continue;
        }
        if f.len() >= 2 && f[1] == "NOTES" {
            let n = f
                .get(2)
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            let notes: Vec<&str> = lines.iter().skip(k + 1).take(n).map(|x| x.trim()).collect();
            header.insert(
                f[0].to_string(),
                json!({"type": "NOTES", "text": notes.join("\n")}),
            );
            k += 1 + n;
            continue;
        }
        if f.len() >= 3 {
            header.insert(
                f[0].to_string(),
                json!({"type": f[1], "values": f[2..].iter().map(|s| s.trim()).collect::<Vec<_>>()}),
            );
        } else if f.len() == 2 {
            header.insert(
                f[0].to_string(),
                json!({"type": "", "values": [f[1].trim()]}),
            );
        }
        k += 1;
    }
    if tables.is_empty() {
        return Err(Error::corrupt(FORMAT_ID, "no TABLE in the file"));
    }
    let first = |key: &str| -> Option<String> {
        header
            .get(key)
            .and_then(|v| v.get("values"))
            .and_then(|v| v.get(0))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let tag = first("TAG").unwrap_or_default();
    // group tables: CURVE, CURVE1..n share one trace (one sweep each)
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, t) in tables.iter().enumerate() {
        let base = if t.key.starts_with("CURVE") && t.key[5..].chars().all(|c| c.is_ascii_digit()) {
            "CURVE".to_string()
        } else {
            t.key.clone()
        };
        match groups
            .iter_mut()
            .find(|g| g.0 == base && tables[g.1[0]].names == t.names)
        {
            Some(g) => g.1.push(i),
            None => groups.push((base, vec![i])),
        }
    }
    let mut traces = Vec::new();
    let mut observations = openreadout_core::assurance::Observations::default();
    let mut entries = Vec::new();
    for (base, members) in &groups {
        let t0 = &tables[members[0]];
        // numeric columns: those whose every value parses (the `Over` flags text does not)
        let numeric: Vec<usize> = (0..t0.names.len())
            .filter(|&c| {
                members.iter().all(|&m| {
                    tables[m]
                        .rows
                        .iter()
                        .all(|r| r.get(c).is_some_and(|v| number(v).is_some()))
                })
            })
            .collect();
        let dropped: Vec<&str> = (0..t0.names.len())
            .filter(|c| !numeric.contains(c))
            .map(|c| t0.names[c].as_str())
            .collect();
        if !dropped.is_empty() {
            findings.push(Finding::info(
                "text_columns",
                format!("{base}: text columns {dropped:?} are kept out of the trace"),
            ));
        }
        let time_col = numeric
            .iter()
            .position(|&c| matches!(t0.names[c].as_str(), "T" | "Time"));
        let mut order: Vec<usize> = numeric.clone();
        if let Some(p) = time_col {
            let c = order.remove(p);
            order.insert(0, c);
        }
        let channels: Vec<SeriesChannel> = order
            .iter()
            .map(|&c| {
                let label = &t0.names[c];
                let (ours, unit) = column(label);
                let file_unit = t0.units.get(c).map_or("", String::as_str);
                let unit = match (file_unit, unit) {
                    ("" | "#" | "bits", u) => u.map(str::to_string),
                    ("V vs. Ref.", _) => Some("V".into()),
                    ("ohms" | "ohm", _) => Some("Ω".into()),
                    ("°" | "deg", _) => Some("°".into()),
                    (fu, _) => Some(fu.to_string()),
                };
                let name = if ours.is_empty() {
                    label.to_ascii_lowercase()
                } else {
                    ours.to_string()
                };
                let mut ch = SeriesChannel::new(name, unit.as_deref(), "float64");
                ch.extra.insert("label".into(), json!(label));
                if !file_unit.is_empty() {
                    ch.extra.insert("file_unit".into(), json!(file_unit));
                }
                ch
            })
            .collect();
        let mut sweeps = Vec::new();
        for &m in members {
            let t = &tables[m];
            if let Some(d) = t.declared
                && d != t.rows.len()
            {
                findings.push(Finding::warning(
                    "count_mismatch",
                    format!("{}: {} rows, the table declares {d}", t.key, t.rows.len()),
                ));
            }
            sweeps.push(
                order
                    .iter()
                    .map(|&c| {
                        t.rows
                            .iter()
                            .map(|r| r.get(c).and_then(|v| number(v)).unwrap_or(f64::NAN))
                            .collect()
                    })
                    .collect::<Vec<Vec<f64>>>(),
            );
            entries.push(LsEntry {
                kind: "block".into(),
                name: t.key.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({"rows": t.rows.len(), "columns": t.names}),
            });
        }
        let mut extra = BTreeMap::new();
        if time_col.is_some() {
            let size = sweeps
                .iter()
                .map(|s| s.first().map_or(0, Vec::len))
                .max()
                .unwrap_or(0);
            extra.insert(
                "axis".into(),
                json!({"quantity": "time", "unit": "s", "irregular": true, "channel": 0, "size": size}),
            );
        }
        extra.insert("kind".into(), json!("electrochemistry"));
        extra.insert(
            "tables".into(),
            json!(
                members
                    .iter()
                    .map(|&m| tables[m].key.clone())
                    .collect::<Vec<_>>()
            ),
        );
        observations.feature(
            FeatureKind::Record,
            format!("table {base}"),
            &[Scope::Traces],
        );
        traces.push(SeriesTrace {
            name: match base.as_str() {
                "CURVE" => format!("{tag} curves"),
                "ZCURVE" => "impedance".into(),
                "OCVCURVE" => "open circuit".into(),
                other => other.to_ascii_lowercase(),
            },
            channels,
            sweeps,
            sample_rate_hz: 0.0,
            start_s: None,
            extra,
        });
    }
    let mut facts = Facts::new();
    facts
        .set("instrument.vendor", "Gamry Instruments", "the file format")
        .instrument_kind("CHMO:0002427")
        .set("instrument.software", "Gamry Framework", "the file format");
    if let Some(p) = first("PSTAT") {
        facts.set("instrument.serial", &p, "PSTAT header line (potentiostat)");
    }
    if let Some(t) = first("TITLE") {
        facts.set("method.name", &t, "TITLE header line");
    }
    if let Some(n) = header
        .get("NOTES")
        .and_then(|v| v.get("text"))
        .and_then(Value::as_str)
    {
        facts.set("acquisition.comment", n, "NOTES header lines");
    }
    if let (Some(d), Some(t)) = (first("DATE"), first("TIME"))
        && let Some(iso) = us_date(&d, &t)
    {
        facts.set(
            "acquisition.started_at",
            &iso,
            "DATE and TIME header lines (month/day/year, local time)",
        );
    }
    if let Some(v) = first("AREA").and_then(|v| number(&v)) {
        facts.number("electrode_area", v, "cm²", "AREA header line");
    }
    if let Some(v) = first("SCANRATE").and_then(|v| number(&v)) {
        facts.number("scan_rate", v, "mV/s", "SCANRATE header line (mV/s)");
    }
    let term = technique(&tag);
    if let Some(t) = term {
        facts.technique(t, "TAG header line");
    }
    facts.plain("experiment", tag.as_str(), "TAG header line");
    for (i, t) in traces.iter().enumerate() {
        facts.measurement(
            MeasurementKind::Trace,
            vec![i as u32],
            format!(
                "{} ({} sweep{})",
                t.name,
                t.sweeps.len(),
                if t.sweeps.len() == 1 { "" } else { "s" }
            ),
            term,
        );
    }
    observations.feature(FeatureKind::Acquisition, format!("experiment {tag}"), &[]);
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::PriorArt);
    Ok(SeriesFile {
        format_version: Some(format!("EXPLAIN {tag}")),
        traces,
        tables: Vec::new(),
        experiment: Some(facts.build()),
        vendor: json!({"gamry": Value::Object(header)}),
        entries,
        findings,
        notes: Vec::new(),
        provenance,
        observations,
        checks: vec!["every table parsed; row counts checked against the declared counts".into()],
        members: Vec::new(),
    })
}

fn us_date(date: &str, time: &str) -> Option<String> {
    let parts: Vec<&str> = date.trim().split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let (month, day, year): (u32, u32, u32) = (
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1990 {
        return None;
    }
    let hms: Vec<u32> = time
        .trim()
        .split(':')
        .filter_map(|x| x.parse().ok())
        .collect();
    if hms.len() != 3 || hms[0] > 23 || hms[1] > 59 || hms[2] > 60 {
        return Some(format!("{year:04}-{month:02}-{day:02}"));
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        hms[0], hms[1], hms[2]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_and_sweeps() {
        let t = b"EXPLAIN\nTAG\tCV\nDATE\tLABEL\t3/6/2019\tDate\nTIME\tLABEL\t16:35:22\tTime\nCURVE1\tTABLE\n\tPt\tT\tVf\tIm\tOver\n\t#\ts\tV vs. Ref.\tA\tbits\n\t0\t0.1\t0.5\t1E-9\t....\n\t1\t0.2\t0.6\t2E-9\t....\nCURVE2\tTABLE\n\tPt\tT\tVf\tIm\tOver\n\t#\ts\tV vs. Ref.\tA\tbits\n\t2\t0.3\t0.7\t3E-9\t....\n";
        assert!(looks_like(t));
        let f = parse(t).unwrap();
        assert_eq!(f.traces.len(), 1);
        assert_eq!(f.traces[0].sweeps.len(), 2);
        assert_eq!(f.traces[0].channels[0].name, "time");
        assert_eq!(f.traces[0].sweeps[1][2], vec![0.7]);
        assert!(parse(b"EXPLAIN\nTAG\tCV\n").is_err());
    }
}
