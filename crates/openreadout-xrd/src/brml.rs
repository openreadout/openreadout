//! Bruker DIFFRAC.SUITE `.brml`: a zip of XML documents. Notes: `docs/formats/xrd.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Scope};
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile, json_num};
use openreadout_core::xml::{child, children};
use openreadout_core::zip::ZipIndex;
use openreadout_core::{Error, Result};
use roxmltree::{Document, Node, ParsingOptions};
use serde_json::{Value, json};

use crate::common::{FileParts, Scan, axis_of, finish};

pub(crate) const FORMAT_ID: &str = "bruker-brml";

/// Largest XML member read.
const MAX_MEMBER: u64 = 512 << 20;

/// True when a zip's member names are those of a BRML file.
pub(crate) fn is_brml<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    let mut coll = false;
    let mut raw = false;
    for n in names {
        let l = n.to_ascii_lowercase();
        coll |= l == "experimentcollection.xml";
        raw |= l.contains("rawdata") && has_ext(&l, "xml");
    }
    coll && raw
}

/// True when a member name has the extension `ext` (any case).
pub(crate) fn has_ext(name: &str, ext: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn txt(n: Node<'_, '_>) -> String {
    n.text().unwrap_or("").trim().to_string()
}

fn child_txt(n: Node<'_, '_>, name: &str) -> Option<String> {
    child(n, name).map(txt).filter(|s| !s.is_empty())
}

fn child_num(n: Node<'_, '_>, name: &str) -> Option<f64> {
    child_txt(n, name).and_then(|s| s.parse().ok())
}

/// A descendant's `Value` attribute as a number (`<Voltage Unit="kV" Value="40" />`).
fn value_of(root: Node<'_, '_>, name: &str) -> Option<f64> {
    root.descendants()
        .find(|d| d.is_element() && d.tag_name().name() == name)
        .and_then(|d| d.attribute("Value"))
        .and_then(|v| v.parse().ok())
}

fn parse_xml(bytes: &[u8], what: &str) -> Result<String> {
    let s = std::str::from_utf8(bytes)
        .map_err(|e| Error::corrupt(FORMAT_ID, format!("{what} is not UTF-8: {e}")))?;
    Ok(s.trim_start_matches('\u{feff}').to_string())
}

fn doc<'a>(text: &'a str, what: &str) -> Result<Document<'a>> {
    Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            ..ParsingOptions::default()
        },
    )
    .map_err(|e| Error::corrupt(FORMAT_ID, format!("{what}: XML: {e}")))
}

/// Parse a BRML zip.
pub(crate) fn parse(zip: &ZipIndex) -> Result<SeriesFile> {
    let mut facts = Facts::new();
    facts
        .set("instrument.vendor", "Bruker", "the file format")
        .instrument_kind("CHMO:0002105");
    let mut observations = openreadout_core::assurance::Observations::default();
    let mut findings = Vec::new();
    let mut entries = Vec::new();
    let mut vendor = serde_json::Map::new();
    // the raw-data members, from DataContainer.xml when it lists them
    let mut members: Vec<String> = Vec::new();
    let container_name = zip
        .members
        .iter()
        .map(|m| m.name.clone())
        .find(|n| n.to_ascii_lowercase().ends_with("datacontainer.xml"));
    if let Some(cn) = &container_name
        && let Some(b) = zip.read_named(cn)?
    {
        let t = parse_xml(&b, cn)?;
        let d = doc(&t, cn)?;
        let r = d.root_element();
        if let Some(l) = child(r, "RawDataReferenceList") {
            members = children(l, "string").map(txt).collect();
        }
        if let Some(v) = child_txt(r, "CreatingVersion") {
            facts.set("instrument.software", "DIFFRAC.SUITE", "BRML container");
            facts.set(
                "instrument.software_version",
                &v,
                "DataContainer.xml CreatingVersion",
            );
            observations.feature(FeatureKind::WriterVersion, format!("DIFFRAC {v}"), &[]);
            vendor.insert("creating_version".into(), json!(v));
        }
        if let Some(i) = r
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "InstrumentDescription")
        {
            if let Some(v) = child_txt(i, "DeviceTypeDesc") {
                facts.set(
                    "instrument.model",
                    &v,
                    "DataContainer.xml InstrumentDescription/DeviceTypeDesc",
                );
            }
            if let Some(v) = child_txt(i, "SerialNo") {
                facts.set(
                    "instrument.serial",
                    &v,
                    "DataContainer.xml InstrumentDescription/SerialNo",
                );
            }
            vendor.insert(
                "instrument".into(),
                json!({"name": child_txt(i, "InstrumentName"), "device": child_txt(i, "DeviceTypeDesc"),
                       "serial": child_txt(i, "SerialNo"), "ics_version": child_txt(i, "IcsVersion")}),
            );
        }
        if let Some(m) = child(r, "MeasurementInfo") {
            for (attr, path) in [
                ("UserName", "acquisition.operator"),
                ("SampleName", "sample.name"),
                ("Comment", "acquisition.comment"),
            ] {
                if let Some(v) = m.attribute(attr) {
                    facts.set(
                        path,
                        v,
                        &format!("DataContainer.xml MeasurementInfo@{attr}"),
                    );
                }
            }
            vendor.insert(
                "measurement_info".into(),
                Value::Object(
                    m.attributes()
                        .map(|a| (a.name().to_string(), json!(a.value())))
                        .collect(),
                ),
            );
        }
    }
    if members.is_empty() {
        members = zip
            .members
            .iter()
            .map(|m| m.name.clone())
            .filter(|n| {
                let l = n.to_ascii_lowercase();
                l.contains("rawdata") && has_ext(&l, "xml")
            })
            .collect();
        members.sort();
    }
    let mut scans = Vec::new();
    for mname in &members {
        let Some(raw) = zip.read_named(mname)? else {
            findings.push(Finding::error(
                "missing_member",
                format!("the container lists {mname}, which is not in the file"),
            ));
            continue;
        };
        if raw.len() as u64 > MAX_MEMBER {
            return Err(Error::unsupported(
                FORMAT_ID,
                "a raw-data member over 512 MiB",
                "Such BRML files are not read.",
            ));
        }
        entries.push(LsEntry {
            kind: "member".into(),
            name: mname.clone(),
            offset: None,
            size: Some(raw.len() as u64),
            image: None,
            details: Value::Null,
        });
        let xml = parse_xml(&raw, mname)?;
        let document = doc(&xml, mname)?;
        let root = document.root_element();
        if let Some(v) = child_txt(root, "TimeStampStarted") {
            facts.set("acquisition.started_at", &v, "RawData TimeStampStarted");
        }
        if let Some(v) = child_txt(root, "TimeStampFinished") {
            facts.set("acquisition.ended_at", &v, "RawData TimeStampFinished");
        }
        if let Some(fi) = child(root, "FixedInformation") {
            for (tag, name, unit) in [
                ("WaveLengthAlpha1", "wavelength_kalpha1", Some("Å")),
                ("WaveLengthAlpha2", "wavelength_kalpha2", Some("Å")),
                ("WaveLengthBeta", "wavelength_kbeta", Some("Å")),
                ("WaveLengthRatio", "kalpha2_kalpha1_ratio", None),
                ("Voltage", "tube_voltage", Some("kV")),
                ("Current", "tube_current", Some("mA")),
                ("Radius", "goniometer_radius", Some("mm")),
            ] {
                if let Some(v) = value_of(fi, tag) {
                    let from = format!("RawData FixedInformation {tag}");
                    match unit {
                        Some(u) => facts.number(name, v, u, &from),
                        None => facts.plain(name, v, &from),
                    };
                }
            }
            if let Some(m) = fi
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "TubeMaterial")
            {
                facts.plain("anode", txt(m), "RawData FixedInformation TubeMaterial");
            }
        }
        let Some(routes) = child(root, "DataRoutes") else {
            findings.push(Finding::warning(
                "no_data",
                format!("{mname} holds no data routes"),
            ));
            continue;
        };
        for route in children(routes, "DataRoute") {
            let flag = route.attribute("RouteFlag").unwrap_or("").to_string();
            let si = child(route, "ScanInformation");
            let scan_name = si
                .and_then(|s| {
                    s.attribute("VisibleName")
                        .or_else(|| s.attribute("ScanName"))
                })
                .unwrap_or("")
                .to_string();
            let time_per_step = si.and_then(|s| child_num(s, "TimePerStep"));
            let axes_info: Vec<Node> = si
                .and_then(|s| child(s, "ScanAxes"))
                .map(|a| children(a, "ScanAxisInfo").collect())
                .unwrap_or_default();
            let Some(views) = child(route, "DataViews") else {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("{mname}: a data route without DataViews"),
                ));
            };
            // columns: (name, unit, start)
            let mut time_col = None;
            let mut absorption_col = None;
            let mut axis_cols: Vec<(String, usize)> = Vec::new();
            let mut counts_col = None;
            let mut counts_name = String::new();
            for v in children(views, "RawDataView") {
                let start: usize = v
                    .attribute("Start")
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| Error::corrupt(FORMAT_ID, "a RawDataView without Start"))?;
                let len: usize = v
                    .attribute("Length")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(1);
                let ty = v
                    .attributes()
                    .find(|a| a.name() == "type")
                    .map(|a| a.value().to_string())
                    .unwrap_or_default();
                if ty.ends_with("FixedRawDataView") {
                    match v.attribute("LogicName") {
                        Some("MeasuredTime") => time_col = Some(start),
                        Some("AbsorptionFactor") => absorption_col = Some(start),
                        _ => {}
                    }
                } else if ty.ends_with("VaryingRawDataView") {
                    if let Some(var) = child(v, "Varying") {
                        for (k, f) in children(var, "FieldDefinitions").enumerate() {
                            let name = f
                                .attribute("AxisId")
                                .or_else(|| f.attribute("FieldName"))
                                .unwrap_or("")
                                .to_string();
                            if k < len {
                                axis_cols.push((name, start + k));
                            }
                        }
                    }
                } else if ty.ends_with("RecordedRawDataView") {
                    if len != 1 {
                        return Err(Error::unsupported(
                            FORMAT_ID,
                            format!("a recorded data view {len} values wide (2D detector frames)"),
                            "Only 0D/1D (integrated) counts per step are read.",
                        ));
                    }
                    if counts_col.is_none() {
                        counts_col = Some(start);
                        counts_name = child(v, "Recording")
                            .and_then(|r| r.attribute("VisibleName"))
                            .unwrap_or("")
                            .to_string();
                    }
                }
            }
            let Some(cc) = counts_col else {
                findings.push(Finding::info(
                    "no_counts",
                    format!("{mname}: a `{flag}` data route without recorded counts is left out"),
                ));
                continue;
            };
            // the scanned axis: the first ScanAxisInfo whose values vary
            let primary = axes_info
                .iter()
                .find(|a| {
                    a.attribute("UIVisibility") == Some("Primary")
                        || child_num(**a, "Increment").is_some_and(|i| i != 0.0)
                })
                .or_else(|| axes_info.first())
                .and_then(|a| a.attribute("AxisId"))
                .unwrap_or("TwoTheta")
                .to_string();
            let Some(&(_, xcol)) = axis_cols.iter().find(|(n, _)| *n == primary) else {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("a scan whose axis `{primary}` has no data column"),
                    "The scanned axis could not be established.",
                ));
            };
            let mut x = Vec::new();
            let mut y = Vec::new();
            let mut tcol = Vec::new();
            let mut acol = Vec::new();
            for dnode in children(route, "Datum") {
                let text = txt(dnode);
                let fields: Vec<f64> = text
                    .split(',')
                    .map(|v| v.trim().parse::<f64>())
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|_| {
                        Error::corrupt(
                            FORMAT_ID,
                            format!("{mname}: a Datum value is not a number: `{text}`"),
                        )
                    })?;
                let get = |c: usize| {
                    fields.get(c).copied().ok_or_else(|| {
                        Error::corrupt(
                            FORMAT_ID,
                            format!(
                                "{mname}: a Datum row has {} values, column {c} is named",
                                fields.len()
                            ),
                        )
                    })
                };
                x.push(get(xcol)?);
                y.push(get(cc)?);
                if let Some(c) = time_col {
                    tcol.push(get(c)?);
                }
                if let Some(c) = absorption_col {
                    acol.push(get(c)?);
                }
            }
            let ax = axis_of(
                match primary.as_str() {
                    "TwoTheta" => "2Theta",
                    other => other,
                },
                axes_info
                    .iter()
                    .find(|a| a.attribute("AxisId") == Some(primary.as_str()))
                    .and_then(|a| a.attribute("Unit"))
                    .unwrap_or("°"),
            );
            let mut extra_channels = Vec::new();
            if !tcol.is_empty() {
                extra_channels.push((
                    SeriesChannel::new("counting_time", Some("s"), "float64"),
                    tcol,
                ));
            }
            if !acol.is_empty() {
                if acol.iter().any(|a| (*a - 1.0).abs() > 1e-12) {
                    findings.push(Finding::info(
                        "absorber",
                        "absorption factors other than 1: counts are returned as stored"
                            .to_string(),
                    ));
                    observations.feature(FeatureKind::Record, "absorber factors", &[Scope::Traces]);
                }
                extra_channels.push((
                    SeriesChannel::new("absorption_factor", None, "float64"),
                    acol,
                ));
            }
            let mut extra = BTreeMap::new();
            extra.insert("route".into(), json!(flag));
            extra.insert("scan_type".into(), json!(scan_name));
            extra.insert("member".into(), json!(mname));
            if let Some(t) = time_per_step {
                extra.insert("time_per_step_s".into(), json_num(t));
            }
            if !counts_name.is_empty() {
                extra.insert("detector".into(), json!(counts_name));
            }
            observations.feature(
                FeatureKind::Layout,
                format!("{} scan ({flag})", ax.label),
                &[Scope::Traces],
            );
            scans.push(Scan {
                name: if flag == "Measured" || flag.is_empty() {
                    String::new()
                } else {
                    format!("{} ({flag})", ax.label)
                },
                axis: ax,
                abscissa: x,
                listed: true,
                intensity: y,
                intensity_unit: Some("counts".into()),
                extra_channels,
                extra,
            });
        }
    }
    if scans.is_empty() {
        return Err(Error::corrupt(FORMAT_ID, "no data route with counts"));
    }
    if let Some(t) = scans[0]
        .extra
        .get("time_per_step_s")
        .and_then(Value::as_f64)
    {
        facts.number("step_time", t, "s", "ScanInformation TimePerStep");
    }
    Ok(finish(
        scans,
        facts,
        FileParts {
            format_version: None,
            vendor: json!({ "brml": Value::Object(vendor) }),
            entries,
            findings,
            observations,
            check: "every data route's Datum rows parsed against its DataViews",
        },
    ))
}
