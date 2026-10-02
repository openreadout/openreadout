//! PANalytical / Malvern Panalytical XRDML (`.xrdml`): the public XML schema of Data Collector.
//! Notes: `docs/formats/xrd.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Scope};
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile, json_num};
use openreadout_core::xml::{child, children};
use openreadout_core::{Error, Result};
use roxmltree::{Document, Node, ParsingOptions};
use serde_json::{Map, Value, json};

use crate::common::{FileParts, Scan, ScanAxis, axis_of, finish, parse_numbers};

pub(crate) const FORMAT_ID: &str = "panalytical-xrdml";

/// True when the start of a file is an XRDML document.
pub(crate) fn looks_like(head: &[u8]) -> bool {
    let n = head.len().min(4096);
    let t = String::from_utf8_lossy(&head[..n]);
    t.contains("<xrdMeasurements") && t.contains("xrdml.com/XRDMeasurement")
}

fn text(n: Node<'_, '_>) -> String {
    n.text().unwrap_or("").trim().to_string()
}

fn child_text(n: Node<'_, '_>, name: &str) -> Option<String> {
    child(n, name).map(text).filter(|s| !s.is_empty())
}

fn child_num(n: Node<'_, '_>, name: &str) -> Option<f64> {
    child_text(n, name).and_then(|s| s.parse::<f64>().ok())
}

/// An element and its descendants as JSON (attributes as `@name`, text as `#text`).
fn tree(n: Node<'_, '_>) -> Value {
    let mut o = Map::new();
    for a in n.attributes() {
        o.insert(format!("@{}", a.name()), json!(a.value()));
    }
    let kids: Vec<Node> = n.children().filter(Node::is_element).collect();
    if kids.is_empty() {
        let t = text(n);
        if o.is_empty() {
            return json!(t);
        }
        if !t.is_empty() {
            o.insert("#text".into(), json!(t));
        }
        return Value::Object(o);
    }
    for k in kids {
        let name = k.tag_name().name().to_string();
        let v = tree(k);
        match o.get_mut(&name) {
            Some(Value::Array(a)) => a.push(v),
            Some(prev) => {
                let p = prev.take();
                *prev = json!([p, v]);
            }
            None => {
                o.insert(name, v);
            }
        }
    }
    Value::Object(o)
}

/// A `positions` element of a scan.
#[derive(Debug, Clone)]
enum Positions {
    Common(f64),
    Range(f64, f64),
    Listed(Vec<f64>),
}

/// Parse an XRDML document.
pub(crate) fn parse(bytes: &[u8]) -> Result<SeriesFile> {
    let text_all = std::str::from_utf8(bytes)
        .map_err(|e| Error::corrupt(FORMAT_ID, format!("the document is not UTF-8: {e}")))?;
    let text_all = text_all.trim_start_matches('\u{feff}');
    let doc = Document::parse_with_options(
        text_all,
        ParsingOptions {
            allow_dtd: false,
            ..ParsingOptions::default()
        },
    )
    .map_err(|e| Error::corrupt(FORMAT_ID, format!("XML: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "xrdMeasurements" {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "root element is `{}`, not xrdMeasurements",
                root.tag_name().name()
            ),
        ));
    }
    let ns = root.tag_name().namespace().unwrap_or("").to_string();
    let version = ns
        .rsplit('/')
        .next()
        .filter(|v| v.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_string);
    let mut findings = Vec::new();
    let mut facts = Facts::new();
    facts
        .set(
            "instrument.vendor",
            "Malvern Panalytical",
            "the file format",
        )
        .instrument_kind("CHMO:0002105");
    // file comments: `Diffractometer system=EMPYREAN`, …
    let mut comments = Vec::new();
    if let Some(c) = child(root, "comment") {
        for e in children(c, "entry") {
            let t = text(e);
            if let Some(v) = t.strip_prefix("Diffractometer system=") {
                facts.set(
                    "instrument.model",
                    v,
                    "comment entry `Diffractometer system=`",
                );
            }
            comments.push(t);
        }
    }
    if let Some(s) = child(root, "sample") {
        if let Some(v) = child_text(s, "id") {
            facts.set("sample.id", &v, "sample/id");
        }
        if let Some(v) = child_text(s, "name") {
            facts.set("sample.name", &v, "sample/name");
        }
    }
    let mut scans: Vec<Scan> = Vec::new();
    let mut vendor_measurements = Vec::new();
    let mut observations = openreadout_core::assurance::Observations::default();
    for (mi, m) in children(root, "xrdMeasurement").enumerate() {
        let mtype = m.attribute("measurementType").unwrap_or("").to_string();
        let sample_mode = m.attribute("sampleMode").unwrap_or("").to_string();
        observations.feature(
            FeatureKind::Acquisition,
            format!("measurement {mtype}"),
            &[],
        );
        let mut wl = BTreeMap::new();
        if let Some(w) = child(m, "usedWavelength") {
            for (k, ours) in [
                ("kAlpha1", "kalpha1"),
                ("kAlpha2", "kalpha2"),
                ("kBeta", "kbeta"),
                ("ratioKAlpha2KAlpha1", "kalpha2_kalpha1_ratio"),
            ] {
                if let Some(v) = child_num(w, k) {
                    wl.insert(ours.to_string(), v);
                }
            }
            if let Some(i) = w.attribute("intended") {
                facts.plain("wavelength_intended", i, "usedWavelength@intended");
            }
        }
        if let Some(v) = wl.get("kalpha1") {
            facts.number("wavelength_kalpha1", *v, "Å", "usedWavelength/kAlpha1");
        }
        if let Some(v) = wl.get("kalpha2") {
            facts.number("wavelength_kalpha2", *v, "Å", "usedWavelength/kAlpha2");
        }
        if let Some(v) = wl.get("kalpha2_kalpha1_ratio") {
            facts.plain(
                "kalpha2_kalpha1_ratio",
                *v,
                "usedWavelength/ratioKAlpha2KAlpha1",
            );
        }
        if let Some(tube) = child(m, "incidentBeamPath").and_then(|p| child(p, "xRayTube")) {
            if let Some(v) = child_text(tube, "anodeMaterial") {
                facts.plain("anode", v, "xRayTube/anodeMaterial");
            }
            if let Some(v) = child_num(tube, "tension") {
                facts.number("tube_voltage", v, "kV", "xRayTube/tension");
            }
            if let Some(v) = child_num(tube, "current") {
                facts.number("tube_current", v, "mA", "xRayTube/current");
            }
        }
        if let Some(det) = child(m, "diffractedBeamPath").and_then(|p| child(p, "detector"))
            && let Some(n) = det.attribute("name")
        {
            facts.plain("detector", n, "diffractedBeamPath/detector@name");
        }
        vendor_measurements.push(json!({
            "measurement_type": mtype,
            "sample_mode": sample_mode,
            "wavelength": wl,
            "incident_beam_path": child(m, "incidentBeamPath").map(tree),
            "diffracted_beam_path": child(m, "diffractedBeamPath").map(tree),
            "comment": child(m, "comment").map(tree),
        }));
        for sc in children(m, "scan") {
            let axis_name = sc.attribute("scanAxis").unwrap_or("").to_string();
            let mode = sc.attribute("mode").unwrap_or("").to_string();
            let status = sc.attribute("status").unwrap_or("").to_string();
            let append = sc.attribute("appendNumber").unwrap_or("").to_string();
            if let Some(h) = child(sc, "header") {
                if let Some(v) = child_text(h, "startTimeStamp") {
                    facts.set("acquisition.started_at", &v, "scan/header/startTimeStamp");
                }
                if let Some(v) = child_text(h, "endTimeStamp") {
                    facts.set("acquisition.ended_at", &v, "scan/header/endTimeStamp");
                }
                if let Some(a) = child(h, "author").and_then(|a| child_text(a, "name")) {
                    facts.set("acquisition.operator", &a, "scan/header/author/name");
                }
                if let Some(src) = child(h, "source") {
                    if let Some(app) = child(src, "applicationSoftware") {
                        facts.set(
                            "instrument.software",
                            &text(app),
                            "scan/header/source/applicationSoftware",
                        );
                        if let Some(v) = app.attribute("version") {
                            facts.set(
                                "instrument.software_version",
                                v,
                                "applicationSoftware@version",
                            );
                            observations.feature(
                                FeatureKind::WriterVersion,
                                format!("{} {v}", text(app)),
                                &[],
                            );
                        }
                    }
                    if let Some(ic) = child(src, "instrumentControlSoftware") {
                        facts.set(
                            "instrument.model",
                            &text(ic),
                            "scan/header/source/instrumentControlSoftware",
                        );
                    }
                    if let Some(v) = child_text(src, "instrumentID") {
                        facts.set("instrument.serial", &v, "scan/header/source/instrumentID");
                    }
                }
            }
            let dp = child(sc, "dataPoints").ok_or_else(|| {
                Error::corrupt(
                    FORMAT_ID,
                    format!("scan {} has no dataPoints", scans.len() + 1),
                )
            })?;
            let intens_node = child(dp, "intensities").or_else(|| child(dp, "counts"));
            let Some(intens_node) = intens_node else {
                if child(dp, "frame").is_some() || child(dp, "frames").is_some() {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        "area-detector frames in an XRDML scan",
                        "Only 1D scans (an intensities or counts list) are read.",
                    ));
                }
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("scan {} has no intensities", scans.len() + 1),
                ));
            };
            let raw_counts = intens_node.tag_name().name() == "counts";
            let unit = intens_node
                .attribute("unit")
                .unwrap_or("counts")
                .to_string();
            let mut values = parse_numbers(&text(intens_node)).ok_or_else(|| {
                Error::corrupt(FORMAT_ID, "an intensity is not a number".to_string())
            })?;
            let n = values.len();
            if n == 0 {
                findings.push(Finding::warning(
                    "empty_scan",
                    format!("scan {} holds no points", scans.len() + 1),
                ));
            }
            let mut positions: Vec<(String, String, Positions)> = Vec::new();
            for p in children(dp, "positions") {
                let axis = p.attribute("axis").unwrap_or("").to_string();
                let unit = p.attribute("unit").unwrap_or("").to_string();
                let pos = if let Some(c) = child_num(p, "commonPosition") {
                    Positions::Common(c)
                } else if let (Some(a), Some(b)) =
                    (child_num(p, "startPosition"), child_num(p, "endPosition"))
                {
                    Positions::Range(a, b)
                } else if let Some(l) = child(p, "listPositions") {
                    let v = parse_numbers(&text(l)).ok_or_else(|| {
                        Error::corrupt(FORMAT_ID, format!("a {axis} position is not a number"))
                    })?;
                    if v.len() != n {
                        return Err(Error::corrupt(
                            FORMAT_ID,
                            format!("{} {axis} positions for {n} intensities", v.len()),
                        ));
                    }
                    Positions::Listed(v)
                } else {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("{axis} positions without a common, start/end or listed value"),
                    ));
                };
                positions.push((axis, unit, pos));
            }
            // the abscissa: the scan axis's own positions, else the first that varies
            let primary = match axis_name.as_str() {
                "Gonio" | "2Theta-Omega" | "2Theta" | "Omega-2Theta" | "2Theta-Phi"
                | "Phi-2Theta" => "2Theta",
                other => other,
            };
            let pick = positions
                .iter()
                .position(|(a, _, p)| a == primary && !matches!(p, Positions::Common(_)))
                .or_else(|| {
                    positions
                        .iter()
                        .position(|(_, _, p)| !matches!(p, Positions::Common(_)))
                });
            let Some(pi) = pick else {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("a {axis_name} scan whose positions do not vary"),
                    "Scans without a varying axis (static or time scans) are not read.",
                ));
            };
            let (ax_name, ax_unit, ax_pos) = positions[pi].clone();
            let ax: ScanAxis = axis_of(&ax_name, &ax_unit);
            let (abscissa, listed) = match &ax_pos {
                Positions::Range(a, b) => (
                    (0..n)
                        .map(|i| {
                            if n > 1 {
                                a + (b - a) * i as f64 / (n - 1) as f64
                            } else {
                                *a
                            }
                        })
                        .collect::<Vec<f64>>(),
                    false,
                ),
                Positions::Listed(v) => (v.clone(), true),
                Positions::Common(c) => (vec![*c; n], false),
            };
            let counting_common = child(dp, "commonCountingTime")
                .map(text)
                .and_then(|s| s.parse::<f64>().ok());
            let counting_list = match child(dp, "countingTimes") {
                Some(c) => {
                    let v = parse_numbers(&text(c)).ok_or_else(|| {
                        Error::corrupt(FORMAT_ID, "a counting time is not a number")
                    })?;
                    if v.len() != n {
                        return Err(Error::corrupt(
                            FORMAT_ID,
                            format!("{} counting times for {n} intensities", v.len()),
                        ));
                    }
                    Some(v)
                }
                None => None,
            };
            let atten_common = child(dp, "commonBeamAttenuationFactor")
                .map(text)
                .and_then(|s| s.parse::<f64>().ok());
            let atten_list = match child(dp, "beamAttenuationFactors") {
                Some(c) => {
                    let v = parse_numbers(&text(c)).ok_or_else(|| {
                        Error::corrupt(FORMAT_ID, "an attenuation factor is not a number")
                    })?;
                    if v.len() != n {
                        return Err(Error::corrupt(
                            FORMAT_ID,
                            format!("{} attenuation factors for {n} intensities", v.len()),
                        ));
                    }
                    Some(v)
                }
                None => None,
            };
            let divergence = child(dp, "commonDivergenceCorrection").is_some()
                || child(dp, "divergenceCorrections").is_some();
            let mut extra_channels = Vec::new();
            // schema 2.x: `counts` are "as received by the detector, not yet corrected for beam
            // attenuator or divergence factors"; schema 1.x `intensities` are already multiplied
            // by the attenuation factor. The attenuation factor is applied to raw counts; a
            // divergence correction (its arithmetic is not defined by the schema) is refused.
            let has_atten =
                atten_list.is_some() || atten_common.is_some_and(|a| (a - 1.0).abs() > 1e-12);
            if raw_counts && divergence {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "raw counts with divergence corrections",
                    "How divergence corrections combine with the counts is not defined by the XRDML schema; export the corrected intensities from HighScore or Data Collector.",
                ));
            }
            if raw_counts && has_atten {
                extra_channels.push((
                    SeriesChannel::new("counts", Some("counts"), "float64"),
                    values.clone(),
                ));
                for (i, v) in values.iter_mut().enumerate() {
                    let f = atten_list
                        .as_ref()
                        .and_then(|l| l.get(i).copied())
                        .or(atten_common)
                        .unwrap_or(1.0);
                    *v *= f;
                }
                observations
                    .calibrations
                    .push(openreadout_core::assurance::Calibration {
                    name: "beam attenuation".into(),
                    status: openreadout_core::assurance::CalibrationStatus::Applied,
                    scope: vec![Scope::Traces],
                    detail:
                        "raw counts multiplied by the beam attenuation factors (XRDML 2.x schema)"
                            .into(),
                });
                observations.feature(FeatureKind::Record, "attenuated counts", &[Scope::Traces]);
            }
            if let Some(v) = counting_list {
                extra_channels.push((SeriesChannel::new("counting_time", Some("s"), "float64"), v));
            }
            if let Some(v) = atten_list {
                extra_channels.push((SeriesChannel::new("attenuation_factor", None, "float64"), v));
            }
            let mut others = Map::new();
            for (k, (a, u, p)) in positions.iter().enumerate() {
                if k == pi {
                    continue;
                }
                let v = match p {
                    Positions::Common(c) => json!({"common": json_num(*c)}),
                    Positions::Range(s, e) => json!({"start": json_num(*s), "end": json_num(*e)}),
                    Positions::Listed(v) => {
                        let q = axis_of(a, u);
                        extra_channels.push((
                            SeriesChannel::new(q.quantity, q.unit.as_deref(), "float64"),
                            v.clone(),
                        ));
                        json!({"listed": true})
                    }
                };
                others.insert(a.clone(), json!({"unit": u, "positions": v}));
            }
            observations.feature(
                FeatureKind::Layout,
                format!(
                    "{} scan{}",
                    axis_name,
                    if listed { " (listed positions)" } else { "" }
                ),
                &[Scope::Traces],
            );
            let mut extra = BTreeMap::new();
            extra.insert("scan_axis".into(), json!(axis_name));
            extra.insert("scan_mode".into(), json!(mode));
            if !status.is_empty() {
                extra.insert("status".into(), json!(status));
            }
            if !append.is_empty() {
                extra.insert("append_number".into(), json!(append));
            }
            extra.insert("measurement".into(), json!(mi));
            if let Some(c) = counting_common {
                extra.insert("counting_time_s".into(), json_num(c));
            }
            if let Some(a) = atten_common {
                extra.insert("attenuation_factor".into(), json_num(a));
            }
            if !others.is_empty() {
                extra.insert("other_axes".into(), Value::Object(others));
            }
            if !wl.is_empty() {
                extra.insert("wavelength_angstrom".into(), json!(wl));
            }
            scans.push(Scan {
                name: String::new(),
                axis: ax,
                abscissa,
                listed,
                intensity: values,
                intensity_unit: Some(unit),
                extra_channels,
                extra,
            });
        }
    }
    if scans.is_empty() {
        return Err(Error::corrupt(FORMAT_ID, "the document holds no scan"));
    }
    if let Some(c) = scans
        .first()
        .and_then(|s| s.extra.get("counting_time_s"))
        .and_then(Value::as_f64)
    {
        facts.number("counting_time", c, "s", "dataPoints/commonCountingTime");
    }
    let entries = vec![LsEntry {
        kind: "metadata".into(),
        name: "xrdMeasurements".into(),
        offset: None,
        size: Some(bytes.len() as u64),
        image: None,
        details: json!({"namespace": ns, "scans": scans.len()}),
    }];
    let vendor = json!({"xrdml": {
        "namespace": ns,
        "status": root.attribute("status"),
        "comment": comments,
        "sample": child(root, "sample").map(tree),
        "measurements": vendor_measurements,
    }});
    if let Some(v) = &version {
        observations.feature(
            FeatureKind::FormatVersion,
            v,
            &[Scope::Traces, Scope::Metadata],
        );
    }
    Ok(finish(
        scans,
        facts,
        FileParts {
            format_version: version,
            vendor,
            entries,
            findings,
            observations,
            check: "every scan's positions and intensities parsed; list lengths checked against the intensities",
        },
    ))
}
