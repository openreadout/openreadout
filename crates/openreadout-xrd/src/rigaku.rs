//! Rigaku SmartLab Studio / PDXL `.ras` (text) and `.rasx` (zip). Notes: `docs/formats/xrd.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Scope};
use openreadout_core::bytes::latin1;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile};
use openreadout_core::zip::ZipIndex;
use openreadout_core::{Error, Result};
use roxmltree::{Document, ParsingOptions};
use serde_json::{Map, Value, json};

use crate::common::{FileParts, Scan, axis_of, finish};

pub(crate) const RAS_FORMAT_ID: &str = "rigaku-ras";
pub(crate) const RASX_FORMAT_ID: &str = "rigaku-rasx";

/// True when the start of a file is a RAS file.
pub(crate) fn looks_like_ras(head: &[u8]) -> bool {
    head.starts_with(b"*RAS_DATA_START")
}

/// True when a zip's member names are those of a RASX file.
pub(crate) fn is_rasx<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    let mut profile = false;
    let mut cond = false;
    for n in names {
        let l = n.to_ascii_lowercase();
        profile |=
            l.contains("profile") && crate::brml::has_ext(&l, "txt") && l.starts_with("data");
        cond |= l.contains("mesurementconditions") || l.contains("measurementconditions");
    }
    profile && cond
}

/// One scan's header keys (RAS `*KEY "value"`, or the RASX XML flattened to the same names).
type Header = BTreeMap<String, String>;

/// `MM/DD/YYYY HH:MM:SS` (RAS) as ISO-8601 local time; ISO strings are kept.
fn rigaku_time(v: &str) -> Option<String> {
    let v = v.trim();
    if v.len() >= 19 && v.as_bytes().get(4) == Some(&b'-') {
        return Some(v.to_string());
    }
    let (date, time) = v.split_once(' ')?;
    let parts: Vec<&str> = date.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let (month, day, mut year): (u32, u32, u32) = (
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    );
    if parts[2].len() <= 2 {
        year += 2000; // SmartLab writes `07/16/25`: month/day/two-digit year
    }
    let hms: Vec<u32> = time.split(':').filter_map(|x| x.parse().ok()).collect();
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hms.len() != 3 || year < 1900 {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        hms[0], hms[1], hms[2]
    ))
}

fn num(h: &Header, k: &str) -> Option<f64> {
    h.get(k).and_then(|v| v.trim().parse::<f64>().ok())
}

/// Facts shared by RAS and RASX, from the first scan's header.
fn header_facts(f: &mut Facts, h: &Header, container: &str) {
    let from = |k: &str| format!("{container} {k}");
    for (k, path) in [
        ("FILE_OPERATOR", "acquisition.operator"),
        ("FILE_SAMPLE", "sample.name"),
        ("FILE_COMMENT", "acquisition.comment"),
        ("FILE_SYSTEM_NAME", "instrument.model"),
    ] {
        if let Some(v) = h.get(k) {
            f.set(path, v, &from(k));
        }
    }
    if let Some(s) = h.get("MEAS_SCAN_START_TIME").and_then(|v| rigaku_time(v)) {
        f.set(
            "acquisition.started_at",
            &s,
            &from("MEAS_SCAN_START_TIME (local time)"),
        );
    }
    if let Some(s) = h.get("MEAS_SCAN_END_TIME").and_then(|v| rigaku_time(v)) {
        f.set(
            "acquisition.ended_at",
            &s,
            &from("MEAS_SCAN_END_TIME (local time)"),
        );
    }
    if let Some(v) = h.get("HW_XG_TARGET_NAME") {
        f.plain("anode", v.as_str(), &from("HW_XG_TARGET_NAME"));
    }
    for (k, name) in [
        ("HW_XG_WAVE_LENGTH_ALPHA1", "wavelength_kalpha1"),
        ("HW_XG_WAVE_LENGTH_ALPHA2", "wavelength_kalpha2"),
        ("HW_XG_WAVE_LENGTH_BETA", "wavelength_kbeta"),
    ] {
        if let Some(v) = num(h, k)
            && h.get("HW_XG_WAVE_LENGTH_UNIT")
                .is_none_or(|u| u.eq_ignore_ascii_case("angstrom"))
        {
            f.number(name, v, "Å", &from(k));
        }
    }
    if let Some(v) = num(h, "MEAS_COND_XG_VOLTAGE") {
        f.number("tube_voltage", v, "kV", &from("MEAS_COND_XG_VOLTAGE"));
    }
    if let Some(v) = num(h, "MEAS_COND_XG_CURRENT") {
        f.number("tube_current", v, "mA", &from("MEAS_COND_XG_CURRENT"));
    }
    if let Some(v) = h.get("HW_COUNTER_SELECT_NAME") {
        f.plain("detector", v.as_str(), &from("HW_COUNTER_SELECT_NAME"));
    }
}

/// A scan from its header and `(x, intensity, attenuation)` rows.
fn scan(
    h: &Header,
    rows: Vec<[f64; 3]>,
    k: usize,
    findings: &mut Vec<Finding>,
    observations: &mut openreadout_core::assurance::Observations,
) -> Scan {
    // Japanese-locale files write θ in Shift-JIS (0x83 0xC6), read here as Latin-1
    let axis_name = h
        .get("MEAS_SCAN_AXIS_X")
        .cloned()
        .unwrap_or_else(|| "2Theta".into())
        .replace("\u{83}\u{c6}", "θ");
    // a coupled scan (`2θ/θ`, `2theta/omega`, `TwoThetaTheta`) is sampled on 2θ
    let first = axis_name
        .split('/')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let axis_key = match axis_name.as_str() {
        "TwoThetaTheta" | "TwoTheta" | "TwoThetaOmega" | "TwoThetaChiPhi" => "2Theta",
        _ if matches!(first.as_str(), "2theta" | "2θ" | "2-theta" | "twotheta") => "2Theta",
        other => other,
    };
    let unit = h.get("MEAS_SCAN_UNIT_X").map_or("deg", String::as_str);
    let unit = if unit.starts_with("deg") { "deg" } else { unit };
    let axis = axis_of(axis_key, unit);
    let y_unit = h
        .get("MEAS_SCAN_UNIT_Y")
        .cloned()
        .unwrap_or_else(|| "counts".into());
    let att: Vec<f64> = rows.iter().map(|r| r[2]).collect();
    if att.iter().any(|a| (*a - 1.0).abs() > 1e-12) {
        findings.push(Finding::warning(
            "attenuation",
            format!(
                "scan {}: attenuation factors other than 1; intensities are returned as stored (not multiplied by them)",
                k + 1
            ),
        ));
        observations.feature(FeatureKind::Record, "attenuated points", &[Scope::Traces]);
    }
    observations.feature(
        FeatureKind::Layout,
        format!("{axis_name} scan"),
        &[Scope::Traces],
    );
    let mut extra = BTreeMap::new();
    extra.insert("scan_axis".into(), json!(axis_name));
    for (k2, ours) in [
        ("MEAS_SCAN_MODE", "scan_mode"),
        ("MEAS_SCAN_SPEED", "scan_speed"),
        ("MEAS_SCAN_SPEED_UNIT", "scan_speed_unit"),
        ("MEAS_SCAN_STEP", "scan_step"),
    ] {
        if let Some(v) = h.get(k2) {
            extra.insert(ours.into(), json!(v));
        }
    }
    Scan {
        name: String::new(),
        axis,
        abscissa: rows.iter().map(|r| r[0]).collect(),
        listed: false,
        intensity: rows.iter().map(|r| r[1]).collect(),
        intensity_unit: Some(y_unit),
        extra_channels: vec![(
            SeriesChannel::new("attenuation_factor", None, "float64"),
            att,
        )],
        extra,
    }
}

fn row(line: &str, what: &str) -> Result<Option<[f64; 3]>> {
    let fields: Vec<&str> = line
        .trim_start_matches('\u{feff}')
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect();
    if fields.is_empty() {
        return Ok(None);
    }
    let value = |i: usize| -> Result<f64> {
        fields
            .get(i)
            .ok_or_else(|| {
                Error::corrupt(
                    what_id(what),
                    format!("{what}: a data row with {} values: `{line}`", fields.len()),
                )
            })?
            .parse::<f64>()
            .map_err(|_| {
                Error::corrupt(
                    what_id(what),
                    format!("{what}: a value is not a number: `{line}`"),
                )
            })
    };
    let x = value(0)?;
    let y = value(1)?;
    let a = if fields.len() > 2 { value(2)? } else { 1.0 };
    Ok(Some([x, y, a]))
}

fn what_id(what: &str) -> &'static str {
    if what.starts_with("RASX") {
        RASX_FORMAT_ID
    } else {
        RAS_FORMAT_ID
    }
}

/// Parse a RAS file.
pub(crate) fn parse_ras(bytes: &[u8]) -> Result<SeriesFile> {
    let text: String = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => latin1(bytes),
    };
    let mut scans = Vec::new();
    let mut headers: Vec<Header> = Vec::new();
    let mut findings = Vec::new();
    let mut observations = openreadout_core::assurance::Observations::default();
    let mut h = Header::new();
    let mut rows: Vec<[f64; 3]> = Vec::new();
    let mut state = 0u8; // 0 outside, 1 header, 2 data
    for line in text.lines() {
        let l = line.trim_end_matches('\r');
        match l.trim() {
            "*RAS_HEADER_START" => {
                h = Header::new();
                state = 1;
                continue;
            }
            "*RAS_HEADER_END" => {
                state = 0;
                continue;
            }
            "*RAS_INT_START" => {
                rows.clear();
                state = 2;
                continue;
            }
            "*RAS_INT_END" => {
                let k = scans.len();
                if let Some(n) = num(&h, "MEAS_DATA_COUNT")
                    && (n - rows.len() as f64).abs() > 0.5
                {
                    findings.push(Finding::warning(
                        "count_mismatch",
                        format!(
                            "scan {}: {} data rows, MEAS_DATA_COUNT says {n}",
                            k + 1,
                            rows.len()
                        ),
                    ));
                }
                scans.push(scan(
                    &h,
                    std::mem::take(&mut rows),
                    k,
                    &mut findings,
                    &mut observations,
                ));
                headers.push(h.clone());
                state = 0;
                continue;
            }
            _ => {}
        }
        match state {
            1 => {
                if let Some(rest) = l.strip_prefix('*') {
                    let (k, v) = rest.split_once(' ').unwrap_or((rest, ""));
                    let v = v.trim();
                    let v = v
                        .strip_prefix('"')
                        .and_then(|x| x.strip_suffix('"'))
                        .unwrap_or(v);
                    h.insert(k.trim().to_string(), v.to_string());
                }
            }
            2 => {
                if let Some(r) = row(l, "RAS")? {
                    rows.push(r);
                }
            }
            _ => {}
        }
    }
    if state == 2 {
        return Err(Error::corrupt(
            RAS_FORMAT_ID,
            "the file ends inside a data block (no *RAS_INT_END)",
        ));
    }
    if scans.is_empty() {
        return Err(Error::corrupt(
            RAS_FORMAT_ID,
            "no *RAS_INT_START … *RAS_INT_END data block",
        ));
    }
    let mut facts = Facts::new();
    facts
        .set("instrument.vendor", "Rigaku", "the file format")
        .instrument_kind("CHMO:0002105");
    header_facts(&mut facts, &headers[0], "RAS header");
    if let Some(v) = headers[0].get("FILE_VERSION") {
        observations.feature(
            FeatureKind::FormatVersion,
            format!("RAS {v}"),
            &[Scope::Traces, Scope::Metadata],
        );
    }
    let entries = headers
        .iter()
        .enumerate()
        .map(|(k, h)| LsEntry {
            kind: "block".into(),
            name: format!("scan {}", k + 1),
            offset: None,
            size: None,
            image: None,
            details: json!({"keys": h.len()}),
        })
        .collect();
    let vendor = json!({"ras": headers.iter().map(|h| json!(h)).collect::<Vec<Value>>()});
    Ok(finish(
        scans,
        facts,
        FileParts {
            format_version: headers[0].get("FILE_VERSION").map(|v| format!("RAS {v}")),
            vendor,
            entries,
            findings,
            observations,
            check: "every data block parsed; row counts checked against MEAS_DATA_COUNT",
        },
    ))
}

/// Flatten a RASX conditions document into RAS-style keys.
fn rasx_header(xml: &str) -> Result<Header> {
    let d = Document::parse_with_options(
        xml.trim_start_matches('\u{feff}'),
        ParsingOptions {
            allow_dtd: false,
            ..ParsingOptions::default()
        },
    )
    .map_err(|e| Error::corrupt(RASX_FORMAT_ID, format!("measurement conditions: XML: {e}")))?;
    let mut h = Header::new();
    let r = d.root_element();
    let find = |parent: &str, name: &str| -> Option<String> {
        r.descendants()
            .find(|n| {
                n.is_element()
                    && n.tag_name().name() == name
                    && n.parent_element()
                        .is_some_and(|p| p.tag_name().name() == parent)
            })
            .and_then(|n| n.text())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    };
    for (parent, name, key) in [
        ("GeneralInformation", "Operator", "FILE_OPERATOR"),
        ("GeneralInformation", "SampleName", "FILE_SAMPLE"),
        ("GeneralInformation", "Comment", "FILE_COMMENT"),
        ("GeneralInformation", "SystemName", "FILE_SYSTEM_NAME"),
        ("GeneralInformation", "Version", "FILE_VERSION"),
        ("XrayGenerator", "TargetName", "HW_XG_TARGET_NAME"),
        (
            "XrayGenerator",
            "WavelengthKalpha1",
            "HW_XG_WAVE_LENGTH_ALPHA1",
        ),
        (
            "XrayGenerator",
            "WavelengthKalpha2",
            "HW_XG_WAVE_LENGTH_ALPHA2",
        ),
        ("XrayGenerator", "WavelengthKbeta", "HW_XG_WAVE_LENGTH_BETA"),
        ("XrayGenerator", "Voltage", "MEAS_COND_XG_VOLTAGE"),
        ("XrayGenerator", "Current", "MEAS_COND_XG_CURRENT"),
        ("ScanInformation", "AxisName", "MEAS_SCAN_AXIS_X"),
        ("ScanInformation", "Mode", "MEAS_SCAN_MODE"),
        ("ScanInformation", "Start", "MEAS_SCAN_START"),
        ("ScanInformation", "Stop", "MEAS_SCAN_STOP"),
        ("ScanInformation", "Step", "MEAS_SCAN_STEP"),
        ("ScanInformation", "Speed", "MEAS_SCAN_SPEED"),
        ("ScanInformation", "SpeedUnit", "MEAS_SCAN_SPEED_UNIT"),
        ("ScanInformation", "PositionUnit", "MEAS_SCAN_UNIT_X"),
        ("ScanInformation", "IntensityUnit", "MEAS_SCAN_UNIT_Y"),
        ("ScanInformation", "StartTime", "MEAS_SCAN_START_TIME"),
        ("ScanInformation", "EndTime", "MEAS_SCAN_END_TIME"),
    ] {
        if let Some(v) = find(parent, name) {
            h.insert(key.into(), v);
        }
    }
    // the RAS header as key/value pairs (`*KEY`, value): fills what the elements do not give
    for pair in r
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "Pair")
    {
        let mut it = pair
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "string");
        if let (Some(k), Some(v)) = (it.next(), it.next()) {
            let key = k
                .text()
                .unwrap_or("")
                .trim()
                .trim_start_matches('*')
                .to_string();
            let val = v.text().unwrap_or("").trim().to_string();
            if !key.is_empty() && !val.is_empty() {
                h.entry(key).or_insert(val);
            }
        }
    }
    // the selected detector
    if let Some(det) = r
        .descendants()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "Category"
                && n.attribute("Name") == Some("Detector")
        })
        .and_then(|n| n.attribute("SelectedUnit"))
    {
        h.insert("HW_COUNTER_SELECT_NAME".into(), det.to_string());
    }
    Ok(h)
}

/// The number a member name ends with (`Data12` → 12); `u64::MAX` when it ends with none.
fn trailing_number(s: &str) -> u64 {
    let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    s[s.len() - digits..].parse().unwrap_or(u64::MAX)
}

/// Parse a RASX zip.
pub(crate) fn parse_rasx(zip: &ZipIndex) -> Result<SeriesFile> {
    let mut profiles: Vec<String> = zip
        .members
        .iter()
        .map(|m| m.name.clone())
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            l.starts_with("data") && l.contains("profile") && crate::brml::has_ext(&l, "txt")
        })
        .collect();
    // `Data2` before `Data10`: by the folder's number, then the profile's, then the name
    profiles.sort_by_key(|n| {
        let (dir, file) = n.rsplit_once('/').unwrap_or(("", n.as_str()));
        (trailing_number(dir), trailing_number(file.trim_end_matches(".txt")), n.clone())
    });
    let mut scans = Vec::new();
    let mut headers = Vec::new();
    let mut findings = Vec::new();
    let mut observations = openreadout_core::assurance::Observations::default();
    let mut entries = Vec::new();
    for (k, p) in profiles.iter().enumerate() {
        // `Data0/Profile0.txt` pairs with `Data0/MesurementConditions0.xml`
        let (dir, file) = p.rsplit_once('/').unwrap_or(("", p.as_str()));
        let idx = file
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .trim_end_matches(".txt");
        let cond_name = zip.members.iter().map(|m| m.name.clone()).find(|n| {
            let (d, f) = n.rsplit_once('/').unwrap_or(("", n.as_str()));
            let fl = f.to_ascii_lowercase();
            d == dir
                && (fl.starts_with("mesurementconditions")
                    || fl.starts_with("measurementconditions"))
                && fl.trim_end_matches(".xml").ends_with(idx)
        });
        let h = if let Some(c) = &cond_name {
            match zip.read_text(c)? {
                Some(x) => rasx_header(&x)?,
                None => Header::new(),
            }
        } else {
            findings.push(Finding::warning(
                "no_conditions",
                format!(
                    "{p} has no measurement conditions: axis and units are assumed (2θ, counts)"
                ),
            ));
            Header::new()
        };
        let text = zip
            .read_text(p)?
            .ok_or_else(|| Error::corrupt(RASX_FORMAT_ID, format!("{p} is not readable")))?;
        let mut rows = Vec::new();
        for l in text.lines() {
            if let Some(r) = row(l, "RASX profile")? {
                rows.push(r);
            }
        }
        entries.push(LsEntry {
            kind: "member".into(),
            name: p.clone(),
            offset: None,
            size: Some(text.len() as u64),
            image: None,
            details: json!({"rows": rows.len(), "conditions": cond_name}),
        });
        scans.push(scan(&h, rows, k, &mut findings, &mut observations));
        headers.push(h);
    }
    if scans.is_empty() {
        return Err(Error::corrupt(
            RASX_FORMAT_ID,
            "no Data*/Profile*.txt member",
        ));
    }
    let mut facts = Facts::new();
    facts
        .set("instrument.vendor", "Rigaku", "the file format")
        .instrument_kind("CHMO:0002105");
    header_facts(&mut facts, &headers[0], "RASX conditions");
    let mut vendor = Map::new();
    vendor.insert("rasx".into(), json!(headers));
    Ok(finish(
        scans,
        facts,
        FileParts {
            format_version: None,
            vendor: Value::Object(vendor),
            entries,
            findings,
            observations,
            check: "every profile parsed with its measurement conditions",
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasx_members_in_numeric_order() {
        assert_eq!(trailing_number("Data12"), 12);
        assert_eq!(trailing_number("Profile0"), 0);
        assert_eq!(trailing_number("Data"), u64::MAX);
        assert_eq!(trailing_number(""), u64::MAX);
        let mut v = vec!["Data10/Profile10.txt", "Data2/Profile2.txt", "Data1/Profile1.txt"];
        v.sort_by_key(|n| {
            let (dir, file) = n.rsplit_once('/').unwrap_or(("", n));
            (trailing_number(dir), trailing_number(file.trim_end_matches(".txt")))
        });
        assert_eq!(v[2], "Data10/Profile10.txt");
    }

    #[test]
    fn ras_blocks() {
        let t = b"*RAS_DATA_START\n*RAS_HEADER_START\n*MEAS_SCAN_AXIS_X \"TwoThetaTheta\"\n*MEAS_DATA_COUNT \"2\"\n*MEAS_SCAN_START_TIME \"12/12/2022 14:42:50\"\n*RAS_HEADER_END\n*RAS_INT_START\n10.0 5 1\n10.1 6 1\n*RAS_INT_END\n*RAS_DATA_END\n";
        let f = parse_ras(t).unwrap();
        assert_eq!(f.traces.len(), 1);
        assert_eq!(f.traces[0].sweeps[0][0], vec![5.0, 6.0]);
        assert!(parse_ras(b"*RAS_DATA_START\n*RAS_INT_START\n1 2 1\n").is_err());
        assert_eq!(
            rigaku_time("12/12/2022 14:42:50").as_deref(),
            Some("2022-12-12T14:42:50")
        );
    }
}
