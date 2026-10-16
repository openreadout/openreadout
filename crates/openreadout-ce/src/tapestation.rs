//! Agilent TapeStation exports (TapeStation Analysis software): the XML of results and metadata
//! and, next to it, `<name>_Electropherogram.csv` with one column of signal per lane. Layout:
//! `docs/formats/agilent-electrophoresis.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::assurance::{FeatureKind as K, Observations, Scope};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{Facts, SeriesColumn, SeriesFile, SeriesTable, json_num};
use openreadout_core::xml::{child, path};
use openreadout_core::{Error, Result};
use roxmltree::Node;
use serde_json::json;

use crate::chip::{self, Curve};

pub(crate) const FORMAT_ID: &str = "agilent-tapestation";

/// True when the start of a (decompressed) file is a TapeStation XML export.
pub(crate) fn looks_like(head: &[u8]) -> bool {
    let s = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    s.contains("<File>") && s.contains("<FileInformation>") && s.contains("<ScreenTapes")
}

/// The electropherogram CSV that belongs to an XML export: `X.xml` (or `X.xml.gz`) →
/// `X_Electropherogram.csv` (or `.csv.gz`). Candidates, plain first.
pub(crate) fn csv_candidates(xml: &Path) -> Vec<PathBuf> {
    let name = xml
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    let stem = if lower.ends_with(".xml.gz") {
        &name[..name.len() - 7]
    } else if lower.ends_with(".xml") {
        &name[..name.len() - 4]
    } else {
        &name
    };
    ["_Electropherogram.csv", "_Electropherogram.csv.gz"]
        .iter()
        .map(|s| xml.with_file_name(format!("{stem}{s}")))
        .collect()
}

fn text<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(n, name)
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "-")
}

fn num(n: Node<'_, '_>, name: &str) -> f64 {
    text(n, name)
        .and_then(|s| s.replace(',', "").parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}

fn kids<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// `18-Sep-2020 6:29 AM` → `2020-09-18T06:29:00`.
pub(crate) fn iso_date(s: &str) -> Option<String> {
    let mut parts = s.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    let ampm = parts.next().unwrap_or("");
    let mut d = date.split('-');
    let day: u32 = d.next()?.parse().ok()?;
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| {
        d.clone()
            .next()
            .is_some_and(|x| x.to_ascii_lowercase().starts_with(m))
    })? + 1;
    d.next()?;
    let year: u32 = d.next()?.parse().ok()?;
    let (h, m) = time.split_once(':')?;
    let (mut h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    match ampm.to_ascii_uppercase().as_str() {
        "PM" if h < 12 => h += 12,
        "AM" if h == 12 => h = 0,
        _ => {}
    }
    (day <= 31 && h < 24 && m < 60)
        .then(|| format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:00"))
}

/// The lanes of the CSV: (well, values in file order).
fn lanes(csv: &str) -> Result<Vec<(String, Vec<f64>)>> {
    let mut lines = csv.lines();
    let head = lines
        .next()
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "the electropherogram CSV is empty"))?;
    let wells: Vec<String> = head
        .split(',')
        .map(|h| h.split(':').next().unwrap_or("").trim().to_string())
        .collect();
    let mut cols: Vec<Vec<f64>> = vec![Vec::new(); wells.len()];
    for (k, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.split(',').collect();
        if cells.len() != wells.len() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "electropherogram row {}: {} cells for {} lanes",
                    k + 2,
                    cells.len(),
                    wells.len()
                ),
            ));
        }
        for (c, v) in cols.iter_mut().zip(cells) {
            c.push(v.trim().parse::<f64>().unwrap_or(f64::NAN));
        }
    }
    Ok(wells.into_iter().zip(cols).collect())
}

/// One sample of the XML.
struct Sample<'a, 'i> {
    node: Node<'a, 'i>,
    well: String,
    ladder: bool,
    tape: String,
    /// (size, run distance %, observation) of each peak.
    peaks: Vec<(f64, f64, String)>,
}

/// Run distance (%) of a lane's marker.
fn marker(s: &Sample<'_, '_>, which: &str) -> Option<f64> {
    s.peaks
        .iter()
        .find(|p| p.2.eq_ignore_ascii_case(which) && p.1.is_finite())
        .map(|p| p.1)
}

/// Parse a TapeStation export: the XML text and the CSV text (`None` when it is missing).
#[allow(clippy::too_many_lines)]
pub(crate) fn parse(xml: &str, csv: Option<&str>, csv_name: Option<&str>) -> Result<SeriesFile> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|e| Error::corrupt(FORMAT_ID, format!("the XML does not parse: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "File" || child(root, "Samples").is_none() {
        return Err(Error::corrupt(
            FORMAT_ID,
            "not a TapeStation export: no File/Samples element",
        ));
    }
    let info = child(root, "FileInformation");
    let assay = info
        .and_then(|i| text(i, "Assay"))
        .unwrap_or("")
        .to_string();
    let units = path(root, &["Assay", "Units"]);
    let size_unit = units
        .and_then(|u| text(u, "MolecularWeightUnit"))
        .unwrap_or("bp")
        .to_string();
    let conc_unit = units
        .and_then(|u| text(u, "ConcentrationUnit"))
        .map(|u| u.replace("µl", "µL"));
    let molarity_unit = units
        .and_then(|u| text(u, "MolarityUnit"))
        .map(|u| u.replace("nmol/l", "nmol/L").replace("pmol/l", "pmol/L"));
    let samples_node =
        child(root, "Samples").ok_or_else(|| Error::corrupt(FORMAT_ID, "no Samples element"))?;
    let samples: Vec<Sample> = kids(samples_node, "Sample")
        .map(|s| {
            let obs = text(s, "Observations").unwrap_or("");
            // the software marks the ladder in Observations ("Ladder"); not "Ladder run as sample"
            let ladder = obs
                .split(',')
                .any(|o| o.trim().eq_ignore_ascii_case("ladder"));
            let peaks = child(s, "Peaks")
                .into_iter()
                .flat_map(|p| kids(p, "Peak"))
                .map(|p| {
                    (
                        num(p, "Size"),
                        num(p, "RunDistance"),
                        text(p, "Observations").unwrap_or("").to_string(),
                    )
                })
                .collect();
            Sample {
                node: s,
                well: text(s, "WellNumber").unwrap_or("?").to_string(),
                ladder,
                tape: text(s, "ScreenTapeID").unwrap_or("").to_string(),
                peaks,
            }
        })
        .collect();
    let mut findings = Vec::new();
    let mut notes = Vec::new();
    // the CSV's lanes, by well
    let lanes = match csv {
        Some(c) => lanes(c)?,
        None => {
            notes.push(
                "the electropherogram CSV next to the XML (<name>_Electropherogram.csv) is missing: results only, no signal".into(),
            );
            Vec::new()
        }
    };
    let mut traces = Vec::new();
    for s in &samples {
        let Some((_, y)) = lanes.iter().find(|(w, _)| *w == s.well) else {
            if csv.is_some() {
                findings.push(Finding::warning(
                    "no_lane",
                    format!("well {}: no column in the electropherogram CSV", s.well),
                ));
            }
            continue;
        };
        let n = y.len();
        if n < 2 {
            continue;
        }
        let distance: Vec<f64> = (0..n).map(|i| (n - i) as f64 / n as f64 * 100.0).collect();
        // the ladder of this lane's tape (else the only ladder), aligned by the markers
        let ladder = samples
            .iter()
            .find(|l| l.ladder && l.tape == s.tape && !s.tape.is_empty())
            .or_else(|| samples.iter().find(|l| l.ladder));
        let Some(ladder) = ladder else {
            findings.push(Finding::warning(
                "no_ladder",
                format!("well {}: no ladder to size against; left out", s.well),
            ));
            continue;
        };
        let curve = Curve::new(ladder.peaks.iter().map(|p| (p.1, p.0)).collect());
        let (lo_l, up_l) = (
            marker(ladder, "Lower Marker"),
            marker(ladder, "Upper Marker"),
        );
        let (lo_s, up_s) = (marker(s, "Lower Marker"), marker(s, "Upper Marker"));
        let map: Box<dyn Fn(f64) -> f64> = match (lo_l, up_l, lo_s, up_s) {
            (Some(a), Some(b), Some(c), Some(d)) if (d - c).abs() > 1e-9 => {
                let k = (b - a) / (d - c);
                Box::new(move |x| a + (x - c) * k)
            }
            // only the lower marker: scale distances so it falls on the ladder's
            (Some(a), _, Some(c), _) if c.abs() > 1e-9 => Box::new(move |x| x * a / c),
            _ => Box::new(|x| x),
        };
        let aligned: Vec<f64> = distance.iter().map(|d| map(*d)).collect();
        // assays with an upper marker size linearly; with only the lower one (genomic DNA,
        // RNA, sizes over several decades) the logarithm of size fits the instrument
        let curve = if up_l.is_some() && up_s.is_some() {
            curve
        } else {
            curve.log_size()
        };
        let name = text(s.node, "Comment").unwrap_or("").to_string();
        let mut extra = BTreeMap::new();
        extra.insert("well".into(), json!(s.well));
        extra.insert("sample".into(), json!(name));
        extra.insert(
            "category".into(),
            json!(if s.ladder { "ladder" } else { "sample" }),
        );
        let rine = s
            .node
            .children()
            .find(|c| c.has_tag_name("RNA"))
            .map(|r| num(r, "RINe"))
            .filter(|v| v.is_finite());
        if let Some(r) = rine {
            extra.insert("rin".into(), json_num(r));
        }
        let din = num(s.node, "DIN");
        if din.is_finite() {
            extra.insert("din".into(), json_num(din));
        }
        extra.insert("ladder_well".into(), json!(ladder.well));
        let label = if name.is_empty() {
            format!("well {}", s.well)
        } else {
            format!("well {}: {name}", s.well)
        };
        match chip::sized_trace(
            label,
            &curve,
            &aligned,
            y,
            Some(("run_distance", "%", &distance)),
            &size_unit,
            ("signal", "FU"),
            extra,
        ) {
            Some(t) => traces.push(t),
            None => findings.push(Finding::warning(
                "not_sized",
                format!(
                    "well {}: the ladder gives no sizes for this lane; left out",
                    s.well
                ),
            )),
        }
    }
    // tables
    let col_text =
        |f: &dyn Fn(&Sample) -> String| -> Vec<String> { samples.iter().map(f).collect() };
    let col_num = |f: &dyn Fn(&Sample) -> f64| -> Vec<f64> { samples.iter().map(f).collect() };
    let rna = |s: &Sample, tag: &str| {
        s.node
            .children()
            .find(|c| c.has_tag_name("RNA"))
            .map_or(f64::NAN, |r| num(r, tag))
    };
    let mut sample_cols = vec![
        SeriesColumn::texts("well", &col_text(&|s| s.well.clone())),
        SeriesColumn::texts(
            "name",
            &col_text(&|s| text(s.node, "Comment").unwrap_or("").to_string()),
        ),
        SeriesColumn::texts(
            "kind",
            &col_text(&|s| {
                if s.ladder {
                    "ladder".into()
                } else {
                    "sample".into()
                }
            }),
        ),
        SeriesColumn::numbers(
            "concentration",
            conc_unit.as_deref(),
            col_num(&|s| num(s.node, "Concentration")),
        ),
    ];
    for (name, tag) in [
        ("rine", "RINe"),
        ("ratio_28s_18s_area", "Ratio28S18SArea"),
        ("ratio_28s_18s_height", "Ratio28S18SHeight"),
    ] {
        let v = col_num(&|s| rna(s, tag));
        if v.iter().any(|x| x.is_finite()) {
            sample_cols.push(SeriesColumn::numbers(name, None, v));
        }
    }
    let din = col_num(&|s| num(s.node, "DIN"));
    if din.iter().any(|x| x.is_finite()) {
        sample_cols.push(SeriesColumn::numbers("din", None, din));
    }
    sample_cols.push(SeriesColumn::texts(
        "observation",
        &col_text(&|s| text(s.node, "Observations").unwrap_or("").to_string()),
    ));
    sample_cols.push(SeriesColumn::texts(
        "alert",
        &col_text(&|s| text(s.node, "Alert").unwrap_or("").to_string()),
    ));
    sample_cols.push(SeriesColumn::texts(
        "screentape",
        &col_text(&|s| s.tape.clone()),
    ));
    let mut tables = vec![SeriesTable {
        name: "samples".into(),
        columns: sample_cols,
        extra: BTreeMap::from([("kind".into(), json!("samples"))]),
    }];
    // peaks and regions of every sample
    let mut p_rows: Vec<(String, Node)> = Vec::new();
    let mut r_rows: Vec<(String, Node)> = Vec::new();
    for s in &samples {
        for p in child(s.node, "Peaks")
            .into_iter()
            .flat_map(|p| kids(p, "Peak"))
        {
            p_rows.push((s.well.clone(), p));
        }
        for r in child(s.node, "Regions")
            .into_iter()
            .flat_map(|r| kids(r, "Region"))
        {
            r_rows.push((s.well.clone(), r));
        }
    }
    let unit = Some(size_unit.as_str());
    let pn = |tag: &str| -> Vec<f64> { p_rows.iter().map(|(_, n)| num(*n, tag)).collect() };
    tables.push(SeriesTable {
        name: "peaks".into(),
        columns: vec![
            SeriesColumn::texts(
                "well",
                &p_rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            ),
            SeriesColumn::numbers("size", unit, pn("Size")),
            SeriesColumn::numbers("run_distance", Some("%"), pn("RunDistance")),
            SeriesColumn::numbers("start_size", unit, pn("FromMW")),
            SeriesColumn::numbers("end_size", unit, pn("ToMW")),
            SeriesColumn::numbers("height", Some("FU"), pn("Height")),
            SeriesColumn::numbers("area", None, pn("Area")),
            SeriesColumn::numbers(
                "concentration",
                conc_unit.as_deref(),
                pn("CalibratedQuantity"),
            ),
            SeriesColumn::numbers("molarity", molarity_unit.as_deref(), pn("Molarity")),
            SeriesColumn::numbers("percent_of_total", Some("%"), pn("PercentOfTotal")),
            SeriesColumn::texts(
                "observation",
                &p_rows
                    .iter()
                    .map(|(_, n)| text(*n, "Observations").unwrap_or(""))
                    .collect::<Vec<_>>(),
            ),
        ],
        extra: BTreeMap::from([
            ("kind".into(), json!("peaks")),
            ("sized_by".into(), json!("instrument")),
        ]),
    });
    if !r_rows.is_empty() {
        let rn = |tag: &str| -> Vec<f64> { r_rows.iter().map(|(_, n)| num(*n, tag)).collect() };
        tables.push(SeriesTable {
            name: "regions".into(),
            columns: vec![
                SeriesColumn::texts(
                    "well",
                    &r_rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
                ),
                SeriesColumn::numbers("start_size", unit, rn("From")),
                SeriesColumn::numbers("end_size", unit, rn("To")),
                SeriesColumn::numbers("average_size", unit, rn("AverageSize")),
                SeriesColumn::numbers("concentration", conc_unit.as_deref(), rn("Concentration")),
                SeriesColumn::numbers("molarity", molarity_unit.as_deref(), rn("Molarity")),
                SeriesColumn::numbers("percent_of_total", Some("%"), rn("PercentOfTotal")),
            ],
            extra: BTreeMap::from([("kind".into(), json!("regions"))]),
        });
    }
    for t in &mut traces {
        t.extra.insert("samples_table".into(), json!(0));
        t.extra.insert("peaks_table".into(), json!(1));
        t.extra.insert("sized_by".into(), json!("openreadout"));
        t.extra.insert(
            "sizing_method".into(),
            json!("ladder point to point over run distance, aligned by the markers (logarithm of size with a lower marker only)"),
        );
    }
    // experiment
    let mut f = Facts::new();
    f.set_with(
        "instrument.vendor",
        "Agilent",
        "the file format",
        Source::Inferred,
    );
    f.set_with(
        "instrument.model",
        "TapeStation",
        "the file format",
        Source::Inferred,
    );
    let tape = path(root, &["ScreenTapes", "ScreenTape"]);
    let env = tape.and_then(|t| child(t, "Environment"));
    if let Some(e) = env {
        if let Some(v) = text(e, "InstrumentSerialNumber") {
            f.set_with(
                "instrument.serial",
                v,
                "Environment/InstrumentSerialNumber",
                Source::Spec,
            );
        }
        f.set_with(
            "instrument.software",
            "TapeStation Analysis",
            "the file format",
            Source::Inferred,
        );
        if let Some(v) = text(e, "AnalysisVersion") {
            f.set_with(
                "instrument.software_version",
                v,
                "Environment/AnalysisVersion",
                Source::Spec,
            );
        }
        if let Some(v) = text(e, "Experimenter") {
            f.set_with(
                "acquisition.operator",
                v,
                "Environment/Experimenter",
                Source::Spec,
            );
        }
        if let Some(v) = text(e, "ControllerVersion") {
            f.plain("controller_version", v, "Environment/ControllerVersion");
        }
    }
    if let Some(t) = tape {
        if let Some(d) = text(t, "TapeRunDate").and_then(iso_date) {
            f.set_with(
                "acquisition.started_at",
                &d,
                "ScreenTape/TapeRunDate (local time)",
                Source::Spec,
            );
        }
        if let Some(v) = text(t, "ScreenTapeID") {
            f.plain("screentape", v, "ScreenTape/ScreenTapeID");
        }
        let temp = num(t, "ElectrophoresisTemp");
        if temp.is_finite() {
            f.number(
                "run_temperature",
                temp,
                "°C",
                "ScreenTape/ElectrophoresisTemp",
            );
        }
        let secs = num(t, "ElectrophoresisTime");
        if secs.is_finite() {
            f.number("run_time", secs, "s", "ScreenTape/ElectrophoresisTime");
        }
    }
    if let Some(d) = info.and_then(|i| text(i, "RunEndDate")).and_then(iso_date) {
        f.set_with(
            "acquisition.ended_at",
            &d,
            "FileInformation/RunEndDate (local time)",
            Source::Spec,
        );
    }
    if !assay.is_empty() {
        f.set_with("method.name", &assay, "FileInformation/Assay", Source::Spec);
    }
    f.plain(
        "size_unit",
        size_unit.clone(),
        "Assay/Units/MolecularWeightUnit",
    );
    f.technique("CHMO:0001024", "the file format (tape electrophoresis)");
    f.instrument_kind("OBI:0001132");
    f.measurement(
        MeasurementKind::Trace,
        (0..traces.len() as u32).collect(),
        format!(
            "ScreenTape electrophoresis, {} lanes sized in {size_unit}",
            traces.len()
        ),
        Some("CHMO:0001024"),
    );
    let mut obs = Observations::default();
    if !assay.is_empty() {
        obs.feature(K::Acquisition, &assay, &[Scope::Traces, Scope::Tables]);
    }
    if let Some(v) = env.and_then(|e| text(e, "AnalysisVersion")) {
        obs.feature(K::WriterVersion, v, &[]);
    }
    obs.feature(
        K::Derivation,
        "size by ladder aligned on the markers",
        &[Scope::Traces],
    );
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::Inferred);
    provenance.insert("tables".into(), Source::PriorArt);
    if !traces.is_empty() {
        notes.push(format!(
            "{} lanes sized here against the ladder ({size_unit}, point to point over run distance after aligning the markers); RINe, DIN, concentrations and peaks are the instrument's",
            traces.len()
        ));
    }
    let mut entries = vec![LsEntry {
        kind: "xml".into(),
        name: "results and metadata".into(),
        offset: None,
        size: None,
        image: None,
        details: json!({"samples": samples.len()}),
    }];
    if let Some(n) = csv_name {
        entries.push(LsEntry {
            kind: "csv".into(),
            name: n.to_string(),
            offset: None,
            size: None,
            image: None,
            details: json!({"lanes": lanes.len()}),
        });
    }
    let flat = |n: Node<'_, '_>| {
        let mut m = serde_json::Map::new();
        for c in n
            .children()
            .filter(|c| c.is_element() && !c.children().any(|g| g.is_element()))
        {
            m.entry(c.tag_name().name().to_string())
                .or_insert_with(|| json!(c.text().unwrap_or("").trim()));
        }
        serde_json::Value::Object(m)
    };
    Ok(SeriesFile {
        format_version: None,
        traces,
        tables,
        experiment: Some(f.build()),
        vendor: json!({
            "FileInformation": info.map(flat),
            "ScreenTape": tape.map(flat),
            "Environment": env.map(flat),
            "Units": units.map(flat),
        }),
        entries,
        findings,
        notes,
        provenance,
        observations: obs,
        checks: vec!["every CSV row checked for one value per lane".into()],
        members: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_names() {
        assert_eq!(
            iso_date("18-Sep-2020 6:29 AM").as_deref(),
            Some("2020-09-18T06:29:00")
        );
        assert_eq!(
            iso_date("02-Oct-2020 11:59 PM").as_deref(),
            Some("2020-10-02T23:59:00")
        );
        assert_eq!(iso_date("nonsense"), None);
        let c = csv_candidates(Path::new("/d/Run 1.xml.gz"));
        assert_eq!(c[0], Path::new("/d/Run 1_Electropherogram.csv"));
    }

    #[test]
    fn a_small_export_sizes_its_lanes() {
        let xml = r#"<?xml version="1.0"?><File><FileInformation><Assay>D1000</Assay></FileInformation><ScreenTapes><ScreenTape><ScreenTapeID>T</ScreenTapeID></ScreenTape></ScreenTapes><Assay><Units><MolecularWeightUnit>bp</MolecularWeightUnit></Units></Assay><Samples>
<Sample><WellNumber>A1</WellNumber><Comment>Ladder</Comment><Observations>Ladder</Observations><ScreenTapeID>T</ScreenTapeID><Peaks>
<Peak><Size>25</Size><RunDistance>80</RunDistance><Observations>Lower Marker</Observations></Peak>
<Peak><Size>100</Size><RunDistance>50</RunDistance></Peak>
<Peak><Size>1500</Size><RunDistance>10</RunDistance><Observations>Upper Marker</Observations></Peak></Peaks></Sample>
<Sample><WellNumber>B1</WellNumber><Comment>s</Comment><ScreenTapeID>T</ScreenTapeID><Peaks>
<Peak><Size>25</Size><RunDistance>82</RunDistance><Observations>Lower Marker</Observations></Peak>
<Peak><Size>1500</Size><RunDistance>12</RunDistance><Observations>Upper Marker</Observations></Peak></Peaks></Sample></Samples></File>"#;
        let mut csv = String::from("A1: Ladder,B1: s\n");
        for i in 0..100 {
            csv.push_str(&format!("{},{}\n", i % 7, i % 5));
        }
        let f = parse(xml, Some(&csv), Some("x.csv")).unwrap();
        assert_eq!(f.traces.len(), 2);
        assert_eq!(f.tables[0].columns[0].name, "well");
        assert!(parse("<x/>", None, None).is_err());
        assert!(parse(xml, Some("A1,B1\n1\n"), None).is_err());
    }
}
