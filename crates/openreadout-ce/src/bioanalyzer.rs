//! Agilent 2100 Bioanalyzer XML exports (2100 Expert, File → Export → XML): the assay, the chip
//! and, per well, the processed electropherogram (base64 float32), the peak table, the standard
//! curve, the RIN and the total concentration. Layout: `docs/formats/agilent-electrophoresis.md`.

use std::collections::BTreeMap;

use base64::Engine as _;
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

pub(crate) const FORMAT_ID: &str = "agilent-bioanalyzer-xml";

/// Most samples a value array may hold (a run has a few thousand).
const MAX_SAMPLES: usize = 10_000_000;

/// True when the start of a (decompressed) file is a Bioanalyzer export.
pub(crate) fn looks_like(head: &[u8]) -> bool {
    let s = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    s.contains("<Chipset") && (s.contains("<Method") || s.contains("<Chips"))
}

fn text<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(n, name)
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn at<'a>(n: Node<'a, '_>, p: &[&str]) -> Option<&'a str> {
    path(n, p)
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn num(n: Node<'_, '_>, name: &str) -> f64 {
    text(n, name)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(f64::NAN)
}

fn kids<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// The float32 values of a base64 signal.
fn signal(n: Node<'_, '_>, count: usize, well: &str) -> Result<Vec<f64>> {
    let t: String = n
        .text()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(t.as_bytes())
        .map_err(|e| {
            Error::corrupt(
                FORMAT_ID,
                format!("well {well}: the signal is not base64: {e}"),
            )
        })?;
    if count > MAX_SAMPLES || bytes.len() / 4 < count {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "well {well}: {count} samples declared, {} stored",
                bytes.len() / 4
            ),
        ));
    }
    Ok(bytes
        .chunks_exact(4)
        .take(count)
        .map(|c| f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
        .collect())
}

/// The text children of an element, by name (nested elements are left out).
fn flat(n: Node<'_, '_>) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for c in n
        .children()
        .filter(|c| c.is_element() && !c.children().any(|g| g.is_element()))
    {
        m.entry(c.tag_name().name().to_string())
            .or_insert_with(|| json!(c.text().unwrap_or("").trim()));
    }
    serde_json::Value::Object(m)
}

/// One peak of a sample.
struct PeakRow {
    well: String,
    size: f64,
    time: f64,
    aligned: f64,
    height: f64,
    area: f64,
    concentration: f64,
    molarity: f64,
    percent: f64,
    observation: String,
}

/// Parse a Bioanalyzer XML export.
#[allow(clippy::too_many_lines)]
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
    if root.tag_name().name() != "Chipset" {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("root element `{}`, not `Chipset`", root.tag_name().name()),
        ));
    }
    let chip = path(root, &["Chips", "Chip"])
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no Chips/Chip element"))?;
    let header = child(chip, "AssayHeader");
    let assay_info = path(
        chip,
        &["AssayBody", "DAAssaySetpoints", "DAMAssayInfoMolecular"],
    );
    let size_unit = assay_info
        .and_then(|a| text(a, "SizeUnit"))
        .unwrap_or("bp")
        .to_string();
    let conc_unit = assay_info
        .and_then(|a| text(a, "ConcentrationUnit"))
        .map(|u| u.replace("µl", "µL"));
    let samples = path(chip, &["Files", "File", "Samples"])
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no Files/File/Samples element"))?;
    let mut findings = Vec::new();
    let mut traces = Vec::new();
    let mut entries = vec![LsEntry {
        kind: "chip".into(),
        name: at(chip, &["AssayHeader", "Title"])
            .unwrap_or("chip")
            .to_string(),
        offset: None,
        size: None,
        image: None,
        details: json!({"samples": kids(samples, "Sample").count()}),
    }];
    // the samples table
    let (mut wells, mut names, mut kinds, mut rins, mut concs, mut flags, mut has) =
        (vec![], vec![], vec![], vec![], vec![], vec![], vec![]);
    let mut peaks: Vec<PeakRow> = Vec::new();
    let mut ladder_well = None;
    for s in kids(samples, "Sample") {
        let well = text(s, "WellNumber").unwrap_or("?").to_string();
        let name = text(s, "Name").unwrap_or("").to_string();
        let category = text(s, "Category").unwrap_or("Sample").to_string();
        let with_data = text(s, "HasData") == Some("true");
        let results = child(s, "DAResultStructures");
        let rin = results
            .and_then(|r| path(r, &["DARRIN", "Channel", "RIN"]))
            .and_then(|n| n.text())
            .and_then(|t| t.trim().parse::<f64>().ok())
            .filter(|v| *v > 0.0);
        let total = results
            .and_then(|r| path(r, &["DARConcentration", "Channel", "TotalConcentration"]))
            .and_then(|n| n.text())
            .and_then(|t| t.trim().parse::<f64>().ok());
        wells.push(well.clone());
        names.push(name.clone());
        kinds.push(if category.eq_ignore_ascii_case("ladder") {
            "ladder".to_string()
        } else {
            "sample".to_string()
        });
        rins.push(rin.unwrap_or(f64::NAN));
        concs.push(total.unwrap_or(f64::NAN));
        flags.push(text(s, "ResultFlagCommonLabel").unwrap_or("").to_string());
        has.push(f64::from(u8::from(with_data)));
        if category.eq_ignore_ascii_case("ladder") {
            ladder_well = Some(well.clone());
        }
        if !with_data {
            continue;
        }
        // peaks of the first channel that has them
        let peak_nodes: Vec<Node> = results
            .and_then(|r| child(r, "DARIntegrator"))
            .into_iter()
            .flat_map(|i| kids(i, "Channel"))
            .find_map(|c| {
                let pm = child(c, "PeaksMolecular")?;
                let v: Vec<Node> = kids(pm, "PeakMolecular").collect();
                (!v.is_empty()).then_some(v)
            })
            .unwrap_or_default();
        for p in &peak_nodes {
            peaks.push(PeakRow {
                well: well.clone(),
                size: num(*p, "FragmentSize"),
                time: num(*p, "MigrationTime"),
                aligned: num(*p, "AlignedMigrationTime"),
                height: num(*p, "Height"),
                area: num(*p, "Area"),
                concentration: num(*p, "Concentration"),
                molarity: num(*p, "Molarity"),
                percent: num(*p, "PercentTotal"),
                observation: text(*p, "Observations").unwrap_or("").to_string(),
            });
        }
        // the standard curve: ladder lengths against aligned migration time
        let curve = Curve::new(
            results
                .and_then(|r| child(r, "DARStandardCurve"))
                .and_then(|c| child(c, "Channel"))
                .and_then(|c| child(c, "LadderPeaks"))
                .map(|lp| {
                    kids(lp, "LadderPeak")
                        .map(|p| (num(p, "MigrationTime"), num(p, "FragmentSize")))
                        .collect()
                })
                .unwrap_or_default(),
        );
        // the processed signal of the red channel
        let Some(sd) = child(s, "DASignals")
            .and_then(|d| child(d, "DetectorChannels"))
            .into_iter()
            .flat_map(|d| kids(d, "Channel"))
            .filter_map(|c| child(c, "SignalData"))
            .find(|sd| text(*sd, "ChannelID") == Some("RedFluorescence"))
        else {
            findings.push(Finding::warning(
                "no_signal",
                format!("well {well}: no RedFluorescence signal"),
            ));
            continue;
        };
        let count = num(sd, "NumberOfSamples");
        let Some(count) = (count.is_finite() && count >= 0.0).then_some(count as usize) else {
            findings.push(Finding::error(
                "bad_signal",
                format!("well {well}: no sample count"),
            ));
            continue;
        };
        let Some(raw) = child(sd, "ProcessedSignal") else {
            findings.push(Finding::error(
                "bad_signal",
                format!("well {well}: no ProcessedSignal"),
            ));
            continue;
        };
        let y = match signal(raw, count, &well) {
            Ok(v) => v,
            Err(e) => {
                findings.push(Finding::error("bad_signal", e.to_string()));
                continue;
            }
        };
        let (x0, dx) = (num(sd, "XStart"), num(sd, "XStep"));
        let (a0, da) = (num(sd, "XStartAligned"), num(sd, "XStepAligned"));
        let time: Vec<f64> = (0..y.len()).map(|i| x0 + dx * i as f64).collect();
        let aligned: Vec<f64> = (0..y.len()).map(|i| a0 + da * i as f64).collect();
        let mut extra = BTreeMap::new();
        extra.insert("well".into(), json!(well));
        extra.insert("sample".into(), json!(name));
        extra.insert("category".into(), json!(kinds.last()));
        if let Some(r) = rin {
            extra.insert("rin".into(), json_num(r));
        }
        extra.insert(
            "time_axis_s".into(),
            json!({"first": json_num(x0), "step": json_num(dx), "size": y.len()}),
        );
        let label = if name.is_empty() {
            format!("well {well}")
        } else {
            format!("well {well}: {name}")
        };
        match chip::sized_trace(
            label,
            &curve,
            &aligned,
            &y,
            Some(("time", "s", &time)),
            &size_unit,
            ("fluorescence", "FU"),
            extra,
        ) {
            Some(t) => traces.push(t),
            None => findings.push(Finding::warning(
                "not_sized",
                format!("well {well}: no standard curve to size the signal with; left out"),
            )),
        }
        entries.push(LsEntry {
            kind: "sample".into(),
            name: format!("well {well}"),
            offset: None,
            size: None,
            image: None,
            details: json!({"name": name, "samples": y.len(), "peaks": peak_nodes.len()}),
        });
    }
    let unit = Some(size_unit.as_str());
    let mut tables = vec![SeriesTable {
        name: "samples".into(),
        columns: vec![
            SeriesColumn::texts("well", &wells),
            SeriesColumn::texts("name", &names),
            SeriesColumn::texts("kind", &kinds),
            SeriesColumn::numbers("rin", None, rins.clone()),
            SeriesColumn::numbers("concentration", conc_unit.as_deref(), concs),
            SeriesColumn::texts("result", &flags),
            SeriesColumn::numbers("has_data", None, has),
        ],
        extra: BTreeMap::from([("kind".into(), json!("samples"))]),
    }];
    if rins.iter().all(|v| v.is_nan()) {
        tables[0].columns.remove(3);
    }
    let molarity_unit = match conc_unit.as_deref() {
        Some("ng/µL") => Some("nmol/L"),
        Some("pg/µL") => Some("pmol/L"),
        _ => None,
    };
    tables.push(SeriesTable {
        name: "peaks".into(),
        columns: vec![
            SeriesColumn::texts(
                "well",
                &peaks.iter().map(|p| p.well.as_str()).collect::<Vec<_>>(),
            ),
            SeriesColumn::numbers("size", unit, peaks.iter().map(|p| p.size).collect()),
            SeriesColumn::numbers("time", Some("s"), peaks.iter().map(|p| p.time).collect()),
            SeriesColumn::numbers(
                "aligned_time",
                Some("s"),
                peaks.iter().map(|p| p.aligned).collect(),
            ),
            SeriesColumn::numbers(
                "height",
                Some("FU"),
                peaks.iter().map(|p| p.height).collect(),
            ),
            SeriesColumn::numbers("area", None, peaks.iter().map(|p| p.area).collect()),
            SeriesColumn::numbers(
                "concentration",
                conc_unit.as_deref(),
                peaks.iter().map(|p| p.concentration).collect(),
            ),
            SeriesColumn::numbers(
                "molarity",
                molarity_unit,
                peaks.iter().map(|p| p.molarity).collect(),
            ),
            SeriesColumn::numbers(
                "percent_of_total",
                Some("%"),
                peaks.iter().map(|p| p.percent).collect(),
            ),
            SeriesColumn::texts(
                "observation",
                &peaks
                    .iter()
                    .map(|p| p.observation.as_str())
                    .collect::<Vec<_>>(),
            ),
        ],
        extra: BTreeMap::from([
            ("kind".into(), json!("peaks")),
            ("sized_by".into(), json!("instrument")),
        ]),
    });
    for t in &mut traces {
        t.extra.insert("peaks_table".into(), json!(1));
        t.extra.insert("samples_table".into(), json!(0));
        t.extra.insert("sized_by".into(), json!("instrument"));
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
        "2100 Bioanalyzer",
        "the file format",
        Source::Inferred,
    );
    if let Some(i) = child(chip, "Instrument") {
        if let Some(sn) = text(i, "SerialNumber") {
            f.set_with(
                "instrument.serial",
                sn,
                "Instrument/SerialNumber",
                Source::Spec,
            );
        }
        if let Some(t) = text(i, "InstrumentType") {
            f.plain("instrument_type", t, "Instrument/InstrumentType");
        }
        if let Some(t) = text(i, "FirmwareVersion") {
            f.plain("firmware", t, "Instrument/FirmwareVersion");
        }
    }
    if let Some(ci) = child(chip, "ChipInformation") {
        f.set_with(
            "instrument.software",
            "2100 Expert",
            "the file format",
            Source::Inferred,
        );
        if let Some(v) = text(ci, "CreationSoftware") {
            f.set_with(
                "instrument.software_version",
                v,
                "ChipInformation/CreationSoftware",
                Source::Spec,
            );
        }
        if let Some(v) = text(ci, "CreationDate") {
            f.set_with(
                "acquisition.started_at",
                v,
                "ChipInformation/CreationDate",
                Source::Spec,
            );
        }
        if let Some(v) = text(ci, "UserName") {
            f.set_with(
                "acquisition.operator",
                v,
                "ChipInformation/UserName",
                Source::Spec,
            );
        }
        if let Some(v) = text(ci, "Comment") {
            f.set_with(
                "acquisition.comment",
                v,
                "ChipInformation/Comment",
                Source::Spec,
            );
        }
        for (k, tag) in [
            ("chip_lot", "ChipLot"),
            ("reagent_kit_lot", "ReagentKitLot"),
            ("dye_lot", "DyeLot"),
        ] {
            if let Some(v) = text(ci, tag) {
                f.plain(k, v, &format!("ChipInformation/{tag}"));
            }
        }
    }
    if let Some(h) = header {
        if let Some(t) = text(h, "Title") {
            f.set_with("method.name", t, "AssayHeader/Title", Source::Spec);
        }
        if let Some(c) = text(h, "Class") {
            f.plain("assay_class", c, "AssayHeader/Class");
        }
    }
    f.plain(
        "size_unit",
        size_unit.clone(),
        "DAMAssayInfoMolecular/SizeUnit",
    );
    if let Some(w) = &ladder_well {
        f.plain(
            "ladder_well",
            w.clone(),
            "the sample whose Category is Ladder",
        );
    }
    let n_samples = wells.len();
    f.technique("CHMO:0001024", "the file format (chip electrophoresis)");
    f.instrument_kind("OBI:0001132");
    f.measurement(
        MeasurementKind::Trace,
        (0..traces.len() as u32).collect(),
        format!(
            "chip electrophoresis, {} wells sized in {size_unit}",
            traces.len()
        ),
        Some("CHMO:0001024"),
    );
    let mut obs = Observations::default();
    if let Some(t) = header.and_then(|h| text(h, "Title")) {
        obs.feature(K::Acquisition, t, &[Scope::Traces, Scope::Tables]);
    }
    if let Some(v) = child(chip, "ChipInformation").and_then(|c| text(c, "CreationSoftware")) {
        obs.feature(K::WriterVersion, v, &[]);
    }
    obs.feature(
        K::Derivation,
        "size by point-to-point standard curve",
        &[Scope::Traces],
    );
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::PriorArt);
    provenance.insert("tables".into(), Source::PriorArt);
    let mut notes = vec![format!(
        "{} of {n_samples} wells sized against the ladder ({size_unit}, point to point over aligned migration time, as the instrument sizes); RIN, concentrations and peaks are the instrument's",
        traces.len()
    )];
    if traces.is_empty() {
        notes.push("no well holds a sized signal".into());
    }
    let mut facts = f;
    if rins.iter().any(|v| v.is_finite()) {
        facts.remark("RIN values are in tables[0] (samples) and each trace's extra.rin");
    }
    Ok(SeriesFile {
        format_version: header.and_then(|h| text(h, "Version")).map(str::to_string),
        traces,
        tables,
        experiment: Some(facts.build()),
        vendor: json!({
            "AssayHeader": header.map(flat),
            "ChipInformation": child(chip, "ChipInformation").map(flat),
            "Instrument": child(chip, "Instrument").map(flat),
            "FileInformation": path(chip, &["Files", "File", "FileInformation"]).map(flat),
        }),
        entries,
        findings,
        notes,
        provenance,
        observations: obs,
        checks: vec!["every signal decoded and its length checked against NumberOfSamples".into()],
        members: Vec::new(),
    })
}
