//! Agilent Seahorse XF assay result files (`.asyr`, Wave): gzip-compressed XML holding the plate
//! map, the injections, the executed protocol and every plate reading of the O2 and pH sensors.
//! Layout: `docs/formats/agilent-seahorse.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Measurement, MeasurementKind, Method, Origin,
    Quantity,
};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::xml::child;
use openreadout_core::{ColumnInfo, Error, Result};
use roxmltree::Node;

use crate::seahorse_rates::{self as rates, OxygenModel, SG_FIRST, SG_SECOND, SG_SMOOTH};
use serde_json::{Map, Value, json};

pub(crate) const FORMAT_ID: &str = "agilent-seahorse-asyr";
/// Largest decompressed document read (the largest seen is 17 MB).
pub(crate) const MAX_XML: u64 = 1 << 30;
/// The arrays every plate reading holds per analyte, one value per well.
const ARRAYS: [&str; 6] = [
    "LedOnEmissionValues",
    "LedOffEmissionValues",
    "LedOnReferenceValues",
    "LedOffReferenceValues",
    "LedUseValues",
    "CorrectedEmissionValues",
];

fn text<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(n, name)
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn num(n: Node<'_, '_>, name: &str) -> Option<f64> {
    text(n, name).and_then(|s| s.parse().ok())
}

fn elements<'a, 'i>(n: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(Node::is_element)
}

/// ISO-8601 duration (`PT38M4.66S`, `P1DT2H`) in seconds.
fn duration_s(s: &str) -> Option<f64> {
    let s = s.trim().strip_prefix('P')?;
    let (date, time) = s.split_once('T').unwrap_or((s, ""));
    let mut total = 0.0;
    let mut add = |part: &str, units: &[(char, f64)]| -> Option<()> {
        let mut num = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' {
                num.push(c);
            } else {
                let f = units.iter().find(|(u, _)| *u == c)?.1;
                total += num.parse::<f64>().ok()? * f;
                num.clear();
            }
        }
        num.is_empty().then_some(())
    };
    add(date, &[('D', 86_400.0)])?;
    add(time, &[('H', 3600.0), ('M', 60.0), ('S', 1.0)])?;
    Some(total)
}

/// Seconds between two ISO-8601 times with offsets (`2023-11-21T11:25:33.12+08:00`).
fn seconds_between(a: &str, b: &str) -> Option<f64> {
    let t = |s: &str| openreadout_core::time::iso8601_to_unix(s);
    Some(t(b)? - t(a)?)
}

fn well_name(row: usize, col: usize) -> String {
    let mut r = String::new();
    let mut k = row;
    loop {
        r.insert(0, char::from(b'A' + (k % 26) as u8));
        if k < 26 {
            break;
        }
        k = k / 26 - 1;
    }
    format!("{r}{}", col + 1)
}

#[derive(Debug, Clone)]
struct Well {
    name: String,
    row: usize,
    col: usize,
    group: Option<String>,
    background: bool,
    flagged: bool,
    buffer_factor: Option<f64>,
    buffer_capacity: Option<f64>,
}

#[derive(Debug, Clone)]
struct Injection {
    group: String,
    port: String,
    reagent: String,
    concentration: Option<f64>,
    unit: String,
    volume_ul: Option<f64>,
}

#[derive(Debug, Clone)]
struct Command {
    instruction: String,
    command: String,
    start: String,
    end: String,
    status: String,
}

/// One analyte's values at one plate reading.
#[derive(Debug, Clone, Default)]
struct AnalyteTick {
    valid: bool,
    well_temperature: Option<f64>,
    corrected: Vec<f64>,
}

#[derive(Debug, Clone, Default)]
struct Tick {
    time_s: f64,
    tray_temperature: Option<f64>,
    environment_temperature: Option<f64>,
    analytes: BTreeMap<String, AnalyteTick>,
}

/// Everything read from an `.asyr` file.
/// The constants the O2/pH levels and the rates of one assay need (all read from the file,
/// except `kvol`, see `docs/formats/agilent-seahorse.md`).
#[derive(Debug, Clone)]
pub(crate) struct RateSetup {
    model: OxygenModel,
    /// pH gain equation's linear terms (C3, C4).
    ph_gain: (f64, f64),
    ph_calibration: f64,
    /// Calibration emission of the pH sensor per well (plate order).
    ph_emission_cal: Vec<f64>,
    /// Readings skipped at the start of a measurement by the ECAR line fit.
    ecar_offset: usize,
    /// Wells whose emission is the O2 reference and whose ECAR is subtracted.
    background: Vec<usize>,
    /// Measurement volume (µL) of the proton efflux rate.
    plate_volume: f64,
    /// Whether PER is computed (`PPRTechnique` seen in the validated exports).
    per: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Assay {
    scalars: Vec<(String, String)>,
    rows: usize,
    cols: usize,
    wells: Vec<Well>,
    injections: Vec<Injection>,
    ticks: Vec<Tick>,
    analytes: Vec<String>,
    /// (first tick, last tick, valid) per measurement.
    spans: Vec<(usize, usize, bool)>,
    commands: Vec<Command>,
    vendor: Map<String, Value>,
    findings: Vec<Finding>,
    /// What the rate calculation needs, or why rates are not computed for this file.
    rates: std::result::Result<RateSetup, String>,
}

impl Default for Assay {
    fn default() -> Self {
        Assay {
            scalars: Vec::new(),
            rows: 0,
            cols: 0,
            wells: Vec::new(),
            injections: Vec::new(),
            ticks: Vec::new(),
            analytes: Vec::new(),
            spans: Vec::new(),
            commands: Vec::new(),
            vendor: Map::new(),
            findings: Vec::new(),
            rates: Err("the rate settings were not read".into()),
        }
    }
}

impl Assay {
    fn scalar(&self, k: &str) -> Option<&str> {
        self.scalars
            .iter()
            .find(|(a, _)| a == k)
            .map(|(_, v)| v.as_str())
    }
}

/// A small element as JSON (text leaves; repeated names become arrays), for the vendor tree.
fn xml_json(n: Node<'_, '_>, depth: usize) -> Value {
    let kids: Vec<Node<'_, '_>> = elements(n).collect();
    if kids.is_empty() || depth == 0 {
        return n.text().map_or(Value::Null, |t| json!(t.trim()));
    }
    let mut m = Map::new();
    for k in kids {
        let v = xml_json(k, depth - 1);
        match m.get_mut(k.tag_name().name()) {
            Some(Value::Array(a)) => a.push(v),
            Some(old) => {
                let prev = old.take();
                *old = json!([prev, v]);
            }
            None => {
                m.insert(k.tag_name().name().to_string(), v);
            }
        }
    }
    Value::Object(m)
}

/// True when decompressed `head` bytes are the start of a Seahorse assay document.
pub(crate) fn looks_like(xml_head: &[u8]) -> bool {
    let s = String::from_utf8_lossy(xml_head);
    s.contains("<XfeAssay")
}

pub(crate) fn parse(xml: &str) -> Result<Assay> {
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|e| Error::corrupt(FORMAT_ID, format!("the assay XML does not parse: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "XfeAssay" {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("root element `{}`, not `XfeAssay`", root.tag_name().name()),
        ));
    }
    let mut a = Assay::default();
    for c in elements(root) {
        if elements(c).next().is_none()
            && let Some(t) = c.text().map(str::trim).filter(|t| !t.is_empty())
            && !matches!(
                c.tag_name().name(),
                "SessionData" | "Notes" | "AnalyticalNotes"
            )
        {
            a.scalars
                .push((c.tag_name().name().to_string(), t.to_string()));
        }
    }
    // plate map
    let plate =
        child(root, "Plate").ok_or_else(|| Error::corrupt(FORMAT_ID, "no `Plate` element"))?;
    a.rows = num(plate, "RowCount").unwrap_or(0.0) as usize;
    a.cols = num(plate, "ColumnCount").unwrap_or(0.0) as usize;
    let wells_el =
        child(plate, "Wells").ok_or_else(|| Error::corrupt(FORMAT_ID, "no `Plate/Wells`"))?;
    let mut seen_groups: Vec<String> = Vec::new();
    for w in elements(wells_el) {
        let row = num(w, "RowIndex").unwrap_or(-1.0);
        let col = num(w, "ColumnIndex").unwrap_or(-1.0);
        if !(row >= 0.0 && col >= 0.0) {
            return Err(Error::corrupt(
                FORMAT_ID,
                "a well without its row and column",
            ));
        }
        let (row, col) = (row as usize, col as usize);
        let g = child(w, "ParentGroup");
        let group = g.and_then(|g| text(g, "GroupName")).map(str::to_string);
        let background = g.and_then(|g| text(g, "IsBackground")) == Some("true");
        if let (Some(g), Some(name)) = (g, &group)
            && !seen_groups.contains(name)
        {
            seen_groups.push(name.clone());
            for p in child(g, "InjectionCondition")
                .and_then(|c| child(c, "Injections"))
                .into_iter()
                .flat_map(elements)
            {
                let port = child(p, "PortContents")
                    .and_then(|c| text(c, "PortLocation"))
                    .or_else(|| text(p, "PortLocation"))
                    .or_else(|| {
                        p.descendants()
                            .find(|d| d.tag_name().name() == "PortLocation")
                            .and_then(|d| d.text())
                    })
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let find = |name: &str| {
                    p.descendants()
                        .find(|d| d.is_element() && d.tag_name().name() == name)
                        .and_then(|d| d.text())
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                };
                let reagent = find("ReagentName").unwrap_or("").to_string();
                if reagent.is_empty() && port.is_empty() {
                    continue;
                }
                a.injections.push(Injection {
                    group: name.clone(),
                    port,
                    reagent,
                    concentration: find("PortConcentration").and_then(|t| t.parse().ok()),
                    unit: find("PortConcentrationUnit").unwrap_or("").to_string(),
                    volume_ul: find("Volume").and_then(|t| t.parse().ok()),
                });
            }
        }
        a.wells.push(Well {
            name: well_name(row, col),
            row,
            col,
            group,
            background,
            flagged: text(w, "Flag") == Some("true"),
            buffer_factor: num(w, "BufferFactor"),
            buffer_capacity: num(w, "BufferCapacity"),
        });
    }
    let n = a.wells.len();
    if n == 0 || a.rows * a.cols != n {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!("{n} wells on a {} × {} plate", a.rows, a.cols),
        ));
    }
    // values follow the Wells list; every file seen lists them row by row
    let row_major = a
        .wells
        .iter()
        .enumerate()
        .all(|(i, w)| w.row == i / a.cols && w.col == i % a.cols);
    if !row_major {
        return Err(Error::unsupported(
            FORMAT_ID,
            "a plate whose wells are not listed row by row",
            "Every file seen lists wells row by row, which is how the per-well values are matched to wells; this one does not, so its values are not assigned. Please share it.",
        ));
    }
    let ads = child(root, "AssayDataSet")
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no `AssayDataSet` (the plate readings)"))?;
    let ticks_el = child(ads, "PlateTickDataSets")
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "no `PlateTickDataSets`"))?;
    let mut bad_ticks = 0usize;
    for (k, t) in elements(ticks_el).enumerate() {
        let time_s = text(t, "TimeStamp").and_then(duration_s);
        let Some(time_s) = time_s else {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("plate reading {k}: no readable `TimeStamp`"),
            ));
        };
        let mut tick = Tick {
            time_s,
            tray_temperature: num(t, "TrayTemperature"),
            environment_temperature: num(t, "EnvironmentalTemperature"),
            analytes: BTreeMap::new(),
        };
        for item in child(t, "AnalyteDataSetsByAnalyteName")
            .into_iter()
            .flat_map(elements)
        {
            let Some(key) = child(item, "Key").and_then(|k| text(k, "string")) else {
                continue;
            };
            let Some(ds) = child(item, "Value").and_then(|v| child(v, "AnalyteDataSet")) else {
                continue;
            };
            let mut ok = true;
            for arr in ARRAYS {
                if child(ds, arr).map_or(0, |c| elements(c).count()) != n {
                    ok = false;
                }
            }
            let corrected: Vec<f64> = child(ds, "CorrectedEmissionValues")
                .into_iter()
                .flat_map(elements)
                .map(|v| {
                    v.text()
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(f64::NAN)
                })
                .collect();
            if !ok || corrected.len() != n {
                bad_ticks += 1;
                continue;
            }
            if !a.analytes.contains(&key.to_string()) {
                a.analytes.push(key.to_string());
            }
            tick.analytes.insert(
                key.to_string(),
                AnalyteTick {
                    valid: text(ds, "IsValid") != Some("false"),
                    well_temperature: num(ds, "WellTemperature"),
                    corrected,
                },
            );
        }
        a.ticks.push(tick);
    }
    if bad_ticks > 0 {
        a.findings.push(Finding::error(
            "unreadable_readings",
            format!("{bad_ticks} analyte readings do not hold one value per well for every array; they are left out"),
        ));
    }
    if a.ticks.is_empty() {
        return Err(Error::corrupt(FORMAT_ID, "no plate readings"));
    }
    if a.ticks.windows(2).any(|w| w[1].time_s < w[0].time_s) {
        a.findings.push(Finding::warning(
            "time_goes_back",
            "plate reading times do not increase",
        ));
    }
    for s in child(ads, "RateSpans").into_iter().flat_map(elements) {
        if let (Some(first), Some(last)) = (num(s, "StartTickIndex"), num(s, "EndTickIndex")) {
            a.spans.push((
                first as usize,
                last as usize,
                text(s, "ValideRate") != Some("false"),
            ));
        }
    }
    for c in child(ads, "CommandHistory").into_iter().flat_map(elements) {
        a.commands.push(Command {
            instruction: text(c, "InstructionName").unwrap_or("").to_string(),
            command: text(c, "CommandName").unwrap_or("").to_string(),
            start: text(c, "StartTime").unwrap_or("").to_string(),
            end: text(c, "EndTime").unwrap_or("").to_string(),
            status: text(c, "CompletionStatus").unwrap_or("").to_string(),
        });
    }
    // measurements: contiguous spans that cover every reading; one per Measure command
    let nt = a.ticks.len();
    let contiguous = !a.spans.is_empty()
        && a.spans[0].0 == 0
        && a.spans.last().is_some_and(|s| s.1 + 1 == nt)
        && a.spans.windows(2).all(|w| w[1].0 == w[0].1 + 1)
        && a.spans.iter().all(|s| s.0 <= s.1);
    if !contiguous {
        a.findings.push(Finding::warning(
            "measurement_spans",
            "the measurement spans do not tile the plate readings as in every file seen",
        ));
        a.spans.retain(|s| s.0 <= s.1 && s.1 < nt);
    }
    let measures = a.commands.iter().filter(|c| c.command == "Measure").count();
    if measures != a.spans.len() {
        a.findings.push(Finding::warning(
            "measurement_count",
            format!(
                "{} measurement spans, {measures} Measure commands in the protocol",
                a.spans.len()
            ),
        ));
    }
    // vendor tree: settings and hardware, not the per-well arrays
    for k in ["Cartridge", "Groups", "Protocol", "AssayStrategies"] {
        if let Some(c) = child(root, k) {
            a.vendor.insert(k.to_string(), xml_json(c, 6));
        }
    }
    for k in [
        "O2DataModifiers",
        "pHDataModifiers",
        "EnvironmentDataModifiers",
        "CalibrationStartTemperature",
        "CalibrationEndTemperature",
        "CalibrationNumPoints",
    ] {
        if let Some(c) = child(ads, k) {
            a.vendor.insert(k.to_string(), xml_json(c, 4));
        }
    }
    if let Some(p) = Some(plate) {
        let mut m = Map::new();
        for k in [
            "Barcode",
            "RowCount",
            "ColumnCount",
            "Serial",
            "Lot",
            "Orientation",
            "Type",
        ] {
            if let Some(v) = text(p, k) {
                m.insert(k.to_string(), json!(v));
            }
        }
        a.vendor.insert("Plate".into(), Value::Object(m));
    }
    a.rates = rate_setup(root, ads, &a);
    Ok(a)
}

/// The plate constants (TauAC, TauAW, TauW, TauC, TauP, ChamberVolume, PlateVolume) of the only
/// plate whose rates were compared with Wave's (standard XFe96 plates, Wave 2.6.1).
const VALIDATED_PLATE: [f64; 7] = [746.0, 0.0, 296.0, 246.0, 60.9, 9.15, 2.28];

fn rm<'a>(n: Option<Node<'a, '_>>, k: &str) -> &'a str {
    n.and_then(|n| text(n, k)).unwrap_or("")
}

fn kernel_ok(n: Node<'_, '_>, name: &str, k: &[f64; 7]) -> bool {
    let vals: Vec<f64> = child(n, name)
        .into_iter()
        .flat_map(elements)
        .filter_map(|v| v.text().and_then(|t| t.trim().parse().ok()))
        .collect();
    !vals.is_empty()
        && vals.len().is_multiple_of(7)
        && vals
            .chunks(7)
            .all(|c| c.iter().zip(k).all(|(a, b)| (a - b).abs() < 1e-12))
}

/// What the O2/pH levels and rates need, or why they are not computed (`Err`, a sentence).
fn rate_setup(
    root: Node<'_, '_>,
    ads: Node<'_, '_>,
    a: &Assay,
) -> std::result::Result<RateSetup, String> {
    let o2 =
        child(ads, "O2DataModifiers").ok_or("no O2 calculation settings (`O2DataModifiers`)")?;
    let ph =
        child(ads, "pHDataModifiers").ok_or("no pH calculation settings (`pHDataModifiers`)")?;
    let method = text(o2, "O2CalculationMethod").unwrap_or("");
    if method != "AKOS" {
        return Err(format!(
            "the O2 calculation method is `{method}`; only AKOS (the published compartment model) was compared with Wave"
        ));
    }
    for k in ["DoKsvLeakCorrection", "DoKsvTempCorrection"] {
        if text(o2, k) != Some("false") {
            return Err(format!(
                "`{k}` is on; only files without it were compared with Wave"
            ));
        }
    }
    let need = |n: Node<'_, '_>, k: &str| -> std::result::Result<f64, String> {
        num(n, k)
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("no `{k}` in the rate settings"))
    };
    let plate = child(o2, "Plate").ok_or("no plate constants (`O2DataModifiers/Plate`)")?;
    let model = OxygenModel {
        f_zero: need(o2, "FO")?,
        ksv: need(o2, "Ksv")?,
        ambient_mmhg: need(o2, "CO")?,
        ambient_mm: need(o2, "COb")?,
        tau_ac: need(plate, "TauAC")?,
        tau_aw: need(plate, "TauAW")?,
        tau_w: need(plate, "TauW")?,
        tau_c: need(plate, "TauC")?,
        tau_p: need(plate, "TauP")?,
        chamber_ul: need(plate, "ChamberVolume")?,
    };
    let plate_volume = need(o2, "PlateVolume")?;
    let constants = [
        model.tau_ac,
        model.tau_aw,
        model.tau_w,
        model.tau_c,
        model.tau_p,
        model.chamber_ul,
        plate_volume,
    ];
    if a.wells.len() != 96
        || constants
            .iter()
            .zip(VALIDATED_PLATE)
            .any(|(x, v)| (x - v).abs() > 1e-9)
    {
        return Err(format!(
            "a {}-well plate with the compartment constants {constants:?} (τAC, τAW, τW, τC, τP, chamber and plate volume); rates were compared with Wave only for standard 96-well plates {VALIDATED_PLATE:?}",
            a.wells.len()
        ));
    }
    if !(model.ksv > 0.0 && model.f_zero > 0.0 && model.ambient_mmhg > 0.0) {
        return Err("Stern-Volmer constants that are not positive".into());
    }
    for (name, k) in [
        ("SGA1", &SG_FIRST),
        ("SGB1", &SG_FIRST),
        ("SGA2", &SG_SECOND),
        ("SGB2", &SG_FIRST),
        ("Smoothing", &SG_SMOOTH),
    ] {
        if !kernel_ok(o2, name, k) {
            return Err(format!(
                "the filter `{name}` is not the published 7-point Savitzky-Golay kernel"
            ));
        }
    }
    let o2_rate = child(o2, "RateModifier");
    let ph_rate = child(ph, "RateModifier");
    if (rm(o2_rate, "Technique"), rm(o2_rate, "Offset")) != ("LineFit", "1")
        || (rm(ph_rate, "Technique"), rm(ph_rate, "Offset")) != ("LineFit", "3")
    {
        return Err(format!(
            "rate settings O2 {}/{} and pH {}/{} (technique/offset); only LineFit/1 and LineFit/3 were compared with Wave",
            rm(o2_rate, "Technique"),
            rm(o2_rate, "Offset"),
            rm(ph_rate, "Technique"),
            rm(ph_rate, "Offset")
        ));
    }
    if text(ph, "DoTemperatureGainCorrection") != Some("false") {
        return Err("the pH temperature gain correction is on; only files without it were compared with Wave".into());
    }
    // the pH sensor's calibration: gain equation and per-well emission at the calibration pH
    let cal = child(ads, "AnalyteCalibrationsByAnalyteName")
        .into_iter()
        .flat_map(elements)
        .find(|i| child(*i, "Key").and_then(|k| text(k, "string")) == Some("pH"))
        .and_then(|i| child(i, "Value"))
        .and_then(|v| child(v, "AnalyteCalibration"))
        .ok_or("no pH calibration (`AnalyteCalibrationsByAnalyteName`)")?;
    let gain = child(cal, "GainEquation").ok_or("no pH gain equation")?;
    let (c1, c2, c3, c4) = (
        need(gain, "C1")?,
        need(gain, "C2")?,
        need(gain, "C3")?,
        need(gain, "C4")?,
    );
    if c1 != 0.0 || c2 != 0.0 {
        return Err(format!(
            "a pH gain equation with higher terms (C1 {c1}, C2 {c2}); only linear ones were compared with Wave"
        ));
    }
    let mut ph_emission_cal = Vec::with_capacity(a.wells.len());
    for row in child(cal, "CalibrationEmissionValues")
        .into_iter()
        .flat_map(elements)
    {
        for v in elements(row) {
            ph_emission_cal.push(
                v.text()
                    .and_then(|t| t.trim().parse::<f64>().ok())
                    .unwrap_or(f64::NAN),
            );
        }
    }
    if ph_emission_cal.len() != a.wells.len() || ph_emission_cal.iter().any(|v| !v.is_finite()) {
        return Err(format!(
            "{} pH calibration emissions for {} wells",
            ph_emission_cal.len(),
            a.wells.len()
        ));
    }
    let ph_calibration = num(root, "DefaultCalibrationPH")
        .filter(|v| v.is_finite())
        .ok_or("no calibration pH (`DefaultCalibrationPH`)")?;
    let background: Vec<usize> = a
        .wells
        .iter()
        .enumerate()
        .filter(|(_, w)| w.background)
        .map(|(i, _)| i)
        .collect();
    if background.is_empty() {
        return Err("no background wells, which are the O2 reference".into());
    }
    if a.wells.iter().any(|w| w.background && w.flagged) {
        return Err("a flagged background well; how Wave treats it was never compared".into());
    }
    if a.ticks.iter().any(|t| {
        ["O2", "pH"]
            .iter()
            .any(|an| t.analytes.get(*an).is_none_or(|x| !x.valid))
    }) {
        return Err("a reading without valid O2 and pH values".into());
    }
    if a.spans.is_empty() || a.spans.iter().any(|s| s.1 < s.0 + 2 * rates::OCR_EDGE) {
        return Err("a measurement with fewer than seven readings".into());
    }
    Ok(RateSetup {
        model,
        ph_gain: (c3, c4),
        ph_calibration,
        ph_emission_cal,
        ecar_offset: 3,
        background,
        plate_volume,
        per: text(ph, "PPRTechnique") == Some("BcFix") && text(ph, "PPROffset") == Some("1"),
    })
}

/// kVol of the proton efflux rate for standard 96-well plates: not stored in `.asyr` files;
/// the value a Wave export prints for the same plate (assumed, `docs/formats/agilent-seahorse.md`).
pub(crate) const KVOL_96: f64 = 1.6;

/// Levels and rates computed from the emissions (`seahorse_rates`).
#[derive(Debug, Clone)]
struct Derived {
    /// O2 level (mmHg) per reading and well.
    oxygen: Vec<Vec<f64>>,
    /// pH per reading and well.
    ph: Vec<Vec<f64>>,
    /// Per measurement: the rate time (min from the first reading).
    time_min: Vec<f64>,
    /// Per measurement and well.
    ocr: Vec<Vec<f64>>,
    ecar: Vec<Vec<f64>>,
    per: Vec<Vec<f64>>,
}

fn derive(a: &Assay, s: &RateSetup) -> Derived {
    let nw = a.wells.len();
    let series = |an: &str| -> Vec<Vec<f64>> {
        a.ticks
            .iter()
            .map(|t| {
                t.analytes
                    .get(an)
                    .map_or_else(|| vec![f64::NAN; nw], |x| x.corrected.clone())
            })
            .collect()
    };
    let oxygen: Vec<Vec<f64>> = series("O2")
        .iter()
        .map(|e| rates::oxygen_levels(e, &s.background, &s.model))
        .collect();
    let ph: Vec<Vec<f64>> = series("pH")
        .iter()
        .map(|e| {
            e.iter()
                .zip(&s.ph_emission_cal)
                .map(|(f, cal)| {
                    rates::ph_level(*f, *cal, s.ph_gain.0, s.ph_gain.1, s.ph_calibration)
                })
                .collect()
        })
        .collect();
    let times: Vec<f64> = a.ticks.iter().map(|t| t.time_s).collect();
    let spans: Vec<(usize, usize)> = a.spans.iter().map(|x| (x.0, x.1)).collect();
    let t0 = times.first().copied().unwrap_or(f64::NAN);
    let time_min = spans
        .iter()
        .map(|&(f, l)| {
            times
                .get(f + (l - f) / 2)
                .map_or(f64::NAN, |t| (t - t0) / 60.0)
        })
        .collect();
    let column = |m: &Vec<Vec<f64>>, w: usize| -> Vec<f64> {
        m.iter()
            .map(|r| r.get(w).copied().unwrap_or(f64::NAN))
            .collect()
    };
    let nm = spans.len();
    let mut ocr = vec![vec![f64::NAN; nw]; nm];
    let mut raw_ecar = vec![vec![f64::NAN; nw]; nm];
    for w in 0..nw {
        let lv = column(&oxygen, w);
        for (k, v) in rates::oxygen_consumption(&times, &lv, &spans, &s.model)
            .into_iter()
            .enumerate()
        {
            ocr[k][w] = v;
        }
        let p = column(&ph, w);
        for (k, &sp) in spans.iter().enumerate() {
            raw_ecar[k][w] = rates::acidification(&times, &p, sp, s.ecar_offset);
        }
    }
    let mut ecar = raw_ecar.clone();
    let mut per = vec![vec![f64::NAN; nw]; nm];
    for k in 0..nm {
        let bg: f64 =
            s.background.iter().map(|&i| raw_ecar[k][i]).sum::<f64>() / s.background.len() as f64;
        let valid = a.spans.get(k).is_some_and(|x| x.2);
        for (w, well) in a.wells.iter().enumerate() {
            if well.background || !valid {
                ocr[k][w] = f64::NAN;
                ecar[k][w] = f64::NAN;
                continue;
            }
            ecar[k][w] = raw_ecar[k][w] - bg;
            if s.per
                && let Some(bf) = well.buffer_factor.filter(|b| *b > 0.0)
            {
                per[k][w] = raw_ecar[k][w] * bf * s.plate_volume * KVOL_96;
            }
        }
    }
    Derived {
        oxygen,
        ph,
        time_min,
        ocr,
        ecar,
        per,
    }
}

/// An opened `.asyr` file.
#[derive(Debug)]
pub struct SeahorseDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    size: u64,
    a: Assay,
    derived: Option<Derived>,
}

type NamedColumn = (String, Option<String>, Vec<f64>, Option<Vec<String>>);

fn categories(values: &[&str]) -> (Vec<f64>, Vec<String>) {
    let mut labels: Vec<String> = Vec::new();
    let codes = values
        .iter()
        .map(|v| {
            let i = labels.iter().position(|l| l == v).unwrap_or_else(|| {
                labels.push((*v).to_string());
                labels.len() - 1
            });
            i as f64
        })
        .collect();
    (codes, labels)
}

fn cat(name: &str, values: &[&str]) -> NamedColumn {
    let (c, l) = categories(values);
    (name.to_string(), None, c, Some(l))
}

fn nums(name: &str, unit: Option<&str>, v: Vec<f64>) -> NamedColumn {
    (name.to_string(), unit.map(str::to_string), v, None)
}

impl SeahorseDataset {
    pub(crate) fn new(descriptor: FormatDescriptor, path: PathBuf, size: u64, a: Assay) -> Self {
        let derived = a.rates.as_ref().ok().map(|s| derive(&a, s));
        SeahorseDataset {
            descriptor,
            path,
            size,
            a,
            derived,
        }
    }

    fn tables(&self) -> Vec<(&'static str, Vec<NamedColumn>)> {
        let a = &self.a;
        let w = &a.wells;
        let first_cmd = a
            .commands
            .first()
            .map(|c| c.start.clone())
            .unwrap_or_default();
        let measure_names: Vec<&str> = a
            .commands
            .iter()
            .filter(|c| c.command == "Measure")
            .map(|c| c.instruction.as_str())
            .collect();
        let mut out = vec![
            (
                "wells",
                vec![
                    cat(
                        "well",
                        &w.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
                    ),
                    nums("row", None, w.iter().map(|x| x.row as f64 + 1.0).collect()),
                    nums(
                        "column",
                        None,
                        w.iter().map(|x| x.col as f64 + 1.0).collect(),
                    ),
                    cat(
                        "group",
                        &w.iter()
                            .map(|x| x.group.as_deref().unwrap_or(""))
                            .collect::<Vec<_>>(),
                    ),
                    nums(
                        "background",
                        None,
                        w.iter()
                            .map(|x| f64::from(u8::from(x.background)))
                            .collect(),
                    ),
                    nums(
                        "flagged",
                        None,
                        w.iter().map(|x| f64::from(u8::from(x.flagged))).collect(),
                    ),
                    nums(
                        "buffer_factor",
                        None,
                        w.iter()
                            .map(|x| x.buffer_factor.unwrap_or(f64::NAN))
                            .collect(),
                    ),
                    nums(
                        "buffer_capacity",
                        None,
                        w.iter()
                            .map(|x| x.buffer_capacity.unwrap_or(f64::NAN))
                            .collect(),
                    ),
                ],
            ),
            (
                "measurements",
                vec![
                    nums(
                        "measurement",
                        None,
                        (1..=a.spans.len()).map(|k| k as f64).collect(),
                    ),
                    nums(
                        "first_reading",
                        None,
                        a.spans.iter().map(|s| s.0 as f64).collect(),
                    ),
                    nums(
                        "last_reading",
                        None,
                        a.spans.iter().map(|s| s.1 as f64).collect(),
                    ),
                    nums(
                        "start",
                        Some("s"),
                        a.spans
                            .iter()
                            .map(|s| a.ticks.get(s.0).map_or(f64::NAN, |t| t.time_s))
                            .collect(),
                    ),
                    nums(
                        "end",
                        Some("s"),
                        a.spans
                            .iter()
                            .map(|s| a.ticks.get(s.1).map_or(f64::NAN, |t| t.time_s))
                            .collect(),
                    ),
                    nums(
                        "valid",
                        None,
                        a.spans.iter().map(|s| f64::from(u8::from(s.2))).collect(),
                    ),
                    cat(
                        "step",
                        &(0..a.spans.len())
                            .map(|k| measure_names.get(k).copied().unwrap_or(""))
                            .collect::<Vec<_>>(),
                    ),
                ],
            ),
            (
                "injections",
                vec![
                    cat(
                        "group",
                        &a.injections
                            .iter()
                            .map(|i| i.group.as_str())
                            .collect::<Vec<_>>(),
                    ),
                    cat(
                        "port",
                        &a.injections
                            .iter()
                            .map(|i| i.port.as_str())
                            .collect::<Vec<_>>(),
                    ),
                    cat(
                        "reagent",
                        &a.injections
                            .iter()
                            .map(|i| i.reagent.as_str())
                            .collect::<Vec<_>>(),
                    ),
                    nums(
                        "concentration",
                        None,
                        a.injections
                            .iter()
                            .map(|i| i.concentration.unwrap_or(f64::NAN))
                            .collect(),
                    ),
                    cat(
                        "unit",
                        &a.injections
                            .iter()
                            .map(|i| i.unit.as_str())
                            .collect::<Vec<_>>(),
                    ),
                    nums(
                        "volume",
                        Some("µL"),
                        a.injections
                            .iter()
                            .map(|i| i.volume_ul.unwrap_or(f64::NAN))
                            .collect(),
                    ),
                ],
            ),
            (
                "protocol",
                vec![
                    nums(
                        "step",
                        None,
                        (1..=a.commands.len()).map(|k| k as f64).collect(),
                    ),
                    cat(
                        "instruction",
                        &a.commands
                            .iter()
                            .map(|c| c.instruction.as_str())
                            .collect::<Vec<_>>(),
                    ),
                    cat(
                        "command",
                        &a.commands
                            .iter()
                            .map(|c| c.command.as_str())
                            .collect::<Vec<_>>(),
                    ),
                    nums(
                        "start",
                        Some("s"),
                        a.commands
                            .iter()
                            .map(|c| seconds_between(&first_cmd, &c.start).unwrap_or(f64::NAN))
                            .collect(),
                    ),
                    nums(
                        "duration",
                        Some("s"),
                        a.commands
                            .iter()
                            .map(|c| seconds_between(&c.start, &c.end).unwrap_or(f64::NAN))
                            .collect(),
                    ),
                    cat(
                        "status",
                        &a.commands
                            .iter()
                            .map(|c| c.status.as_str())
                            .collect::<Vec<_>>(),
                    ),
                ],
            ),
        ];
        if let Some(d) = &self.derived {
            let nw = a.wells.len();
            let rows = d.time_min.len() * nw;
            let pick = |m: &Vec<Vec<f64>>| -> Vec<f64> {
                (0..rows)
                    .map(|r| {
                        m.get(r / nw)
                            .and_then(|x| x.get(r % nw))
                            .copied()
                            .unwrap_or(f64::NAN)
                    })
                    .collect()
            };
            out.push((
                "rates",
                vec![
                    nums(
                        "measurement",
                        None,
                        (0..rows).map(|r| (r / nw + 1) as f64).collect(),
                    ),
                    cat(
                        "well",
                        &(0..rows)
                            .map(|r| w[r % nw].name.as_str())
                            .collect::<Vec<_>>(),
                    ),
                    cat(
                        "group",
                        &(0..rows)
                            .map(|r| w[r % nw].group.as_deref().unwrap_or(""))
                            .collect::<Vec<_>>(),
                    ),
                    nums(
                        "time",
                        Some("min"),
                        (0..rows)
                            .map(|r| d.time_min.get(r / nw).copied().unwrap_or(f64::NAN))
                            .collect(),
                    ),
                    nums("ocr", Some("pmol/min"), pick(&d.ocr)),
                    nums("ecar", Some("mpH/min"), pick(&d.ecar)),
                    nums("per", Some("pmol/min"), pick(&d.per)),
                ],
            ));
        }
        out
    }

    /// Trace k: 0..analytes = corrected emission per analyte; then temperatures; then, when
    /// the rates are computed, the O2 level and the pH.
    fn trace_count(&self) -> usize {
        self.a.analytes.len() + 1 + if self.derived.is_some() { 2 } else { 0 }
    }
}

/// What the rates table says about how its values were computed.
fn rates_extra(a: &Assay) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert(
        "method".into(),
        json!("Gerencser et al. 2009 compartment-model correction (Wave's AKOS): mean corrected OCR(t) over each measurement without its first and last three readings; ECAR = -(pH slope over the readings from the fourth on - the background wells' mean slope); PER = uncorrected ECAR x buffer factor x plate volume x kVol"),
    );
    m.insert(
        "agreement_with_wave".into(),
        json!({"ocr": "within max(0.2 pmol/min, 0.5 %)", "ecar": "within 0.001 mpH/min", "per": "as ECAR (kVol assumed)"}),
    );
    if let Ok(s) = &a.rates {
        m.insert(
            "background_wells".into(),
            json!(
                s.background
                    .iter()
                    .filter_map(|&i| a.wells.get(i).map(|w| w.name.clone()))
                    .collect::<Vec<_>>()
            ),
        );
        m.insert("kvol".into(), json!(KVOL_96));
        m.insert(
            "per_computed".into(),
            json!(
                s.per
                    && a.wells
                        .iter()
                        .any(|w| !w.background && w.buffer_factor.is_some_and(|b| b > 0.0))
            ),
        );
        if let Some(v) = a.scalar("SWVersion") {
            m.insert("software_version".into(), json!(v));
        }
        m.insert("plate_volume_ul".into(), json!(s.plate_volume));
        if !s.per {
            m.insert(
                "per_note".into(),
                json!("PER not computed: a proton-efflux setting never compared with Wave"),
            );
        }
    }
    m.insert(
        "note".into(),
        json!("one row per measurement and well; background wells and measurements marked not valid have no rates (Wave prints 0); PER needs a buffer factor > 0"),
    );
    m
}

fn channel(index: u32, name: &str, unit: Option<&str>) -> SignalChannelInfo {
    SignalChannelInfo {
        index,
        name: name.into(),
        unit: unit.map(str::to_string),
        dtype: "float64".into(),
        scale: 1.0,
        offset: 0.0,
        extra: BTreeMap::new(),
    }
}

impl Dataset for SeahorseDataset {
    fn info(&self) -> Result<FileInfo> {
        let a = &self.a;
        let axis = json!({"quantity": "time", "unit": "s", "irregular": true, "channel": 0});
        let mut traces = Vec::new();
        for (k, an) in a.analytes.iter().enumerate() {
            let mut channels = vec![channel(0, "time", Some("s"))];
            for (i, w) in a.wells.iter().enumerate() {
                channels.push(channel(i as u32 + 1, &w.name, None));
            }
            let mut extra = BTreeMap::new();
            extra.insert("kind".into(), json!("corrected_emission"));
            extra.insert("analyte".into(), json!(an));
            extra.insert("axis".into(), axis.clone());
            extra.insert("measurement_table".into(), json!(1));
            extra.insert(
                "note".into(),
                json!("the sensor's corrected emission per well at every plate reading, as stored"),
            );
            traces.push(TraceInfo {
                index: k as u32,
                name: Some(format!("{an} corrected emission")),
                sample_rate_hz: 0.0,
                sample_count: a.ticks.len() as u64,
                sweep_count: 1,
                channels,
                start_s: None,
                extra,
            });
        }
        let mut channels = vec![
            channel(0, "time", Some("s")),
            channel(1, "tray_temperature", Some("°C")),
            channel(2, "environment_temperature", Some("°C")),
        ];
        for an in &a.analytes {
            channels.push(channel(
                channels.len() as u32,
                &format!("{an}_well_temperature"),
                Some("°C"),
            ));
        }
        traces.push(TraceInfo {
            index: a.analytes.len() as u32,
            name: Some("temperatures".into()),
            sample_rate_hz: 0.0,
            sample_count: a.ticks.len() as u64,
            sweep_count: 1,
            channels,
            start_s: None,
            extra: BTreeMap::from([
                ("kind".to_string(), json!("temperature")),
                ("axis".to_string(), axis.clone()),
            ]),
        });
        if self.derived.is_some() {
            for (k, (name, analyte, unit, how)) in [
                (
                    "O2 level",
                    "O2",
                    "mmHg",
                    "CO + (FO/Ksv)(1/F - 1/F_background): the Stern-Volmer level against the mean emission of the background wells at each reading (Wave's O2 level)",
                ),
                (
                    "pH level",
                    "pH",
                    "pH",
                    "calibration pH + (F - F_calibration)/(1000 (C3 F_calibration + C4)) per well (Wave's pH)",
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let mut channels = vec![channel(0, "time", Some("s"))];
                for (i, w) in a.wells.iter().enumerate() {
                    channels.push(channel(i as u32 + 1, &w.name, Some(unit)));
                }
                traces.push(TraceInfo {
                    index: (a.analytes.len() + 1 + k) as u32,
                    name: Some(name.into()),
                    sample_rate_hz: 0.0,
                    sample_count: a.ticks.len() as u64,
                    sweep_count: 1,
                    channels,
                    start_s: None,
                    extra: BTreeMap::from([
                        ("kind".to_string(), json!("level")),
                        ("analyte".to_string(), json!(analyte)),
                        ("axis".to_string(), axis.clone()),
                        ("measurement_table".to_string(), json!(1)),
                        ("computed".to_string(), json!(how)),
                    ]),
                });
            }
        }
        let tables = self
            .tables()
            .into_iter()
            .enumerate()
            .map(|(i, (name, cols))| TableInfo {
                index: i as u32,
                name: Some(name.into()),
                row_count: cols.first().map_or(0, |c| c.2.len() as u64),
                columns: cols
                    .iter()
                    .enumerate()
                    .map(|(k, (n, unit, _, labels))| ColumnInfo {
                        index: k as u32,
                        name: n.clone(),
                        label: None,
                        dtype: if labels.is_some() {
                            "uint32"
                        } else {
                            "float64"
                        }
                        .into(),
                        unit: unit.clone(),
                        range: None,
                        extra: labels
                            .as_ref()
                            .map(|l| BTreeMap::from([("categories".to_string(), json!(l))]))
                            .unwrap_or_default(),
                    })
                    .collect(),
                extra: if name == "rates" {
                    rates_extra(&self.a)
                } else {
                    BTreeMap::new()
                },
            })
            .collect();
        let rates_note = match &self.a.rates {
            Ok(_) => "OCR, ECAR and PER (table 4 `rates`) and the O2 and pH levels (traces 3-4) are computed from the stored emissions with the published compartment model and the file's constants; they agree with Wave's within max(0.2 pmol/min, 0.5 %) (OCR) and 0.001 mpH/min (ECAR) on every assay compared (docs/formats/agilent-seahorse.md)".to_string(),
            Err(why) => format!("OCR, ECAR and PER are not stored in .asyr files and are not computed for this one: {why}; export them from Wave"),
        };
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: self.descriptor.clone(),
            format_version: a.scalar("VersionStamp").map(str::to_string),
            plane_count: 0,
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            notes: vec![
                "per-well sensor emissions at every plate reading (traces 0-1), the plate map, measurements, injections and executed protocol (tables)".into(),
                rates_note,
            ],
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut m = self.a.vendor.clone();
        let scalars: Map<String, Value> = self
            .a
            .scalars
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        m.insert("assay".into(), Value::Object(scalars));
        Ok(json!({ "seahorse": Value::Object(m) }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        p.insert("traces".into(), Source::Inferred);
        p.insert("tables".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .a
            .spans
            .iter()
            .enumerate()
            .map(|(k, s)| LsEntry {
                kind: "measurement".into(),
                name: format!("measurement {}", k + 1),
                offset: None,
                size: None,
                image: None,
                details: json!({"first_reading": s.0, "last_reading": s.1}),
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "a Seahorse assay holds sensor readings (traces) and tables, not images".into(),
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        r.performed("every reading checked for non-finite values and the sensors' validity flags");
        let invalid: usize = self
            .a
            .ticks
            .iter()
            .flat_map(|t| t.analytes.values())
            .filter(|x| !x.valid)
            .count();
        if invalid > 0 {
            r.push(Finding::warning(
                "invalid_readings",
                format!("{invalid} analyte readings are marked not valid by the instrument"),
            ));
        }
        let bad: usize = self
            .a
            .ticks
            .iter()
            .flat_map(|t| t.analytes.values())
            .map(|x| x.corrected.iter().filter(|v| !v.is_finite()).count())
            .sum();
        if bad > 0 {
            r.push(Finding::warning(
                "non_finite_values",
                format!("{bad} corrected emission values are not numbers"),
            ));
        }
        Ok(r)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("assay XML parsed; per-well arrays counted against the plate; measurement spans against the protocol");
        for f in &self.a.findings {
            r.push(f.clone());
        }
        Ok(r)
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if sweep != 0 || index as usize >= self.trace_count() {
            return Err(Error::Usage(format!(
                "trace {index} sweep {sweep} out of range ({} traces, one sweep each)",
                self.trace_count()
            )));
        }
        let a = &self.a;
        let n = a.ticks.len() as u64;
        let lo = first.min(n) as usize;
        let hi = first.saturating_add(max).min(n) as usize;
        let ticks = &a.ticks[lo..hi];
        let mut channels = vec![ticks.iter().map(|t| t.time_s).collect::<Vec<f64>>()];
        let level = (index as usize)
            .checked_sub(a.analytes.len() + 1)
            .and_then(|k| {
                self.derived
                    .as_ref()
                    .map(|d| if k == 0 { &d.oxygen } else { &d.ph })
            });
        if let Some(m) = level {
            for w in 0..a.wells.len() {
                channels.push(
                    m.get(lo..hi)
                        .unwrap_or_default()
                        .iter()
                        .map(|r| r.get(w).copied().unwrap_or(f64::NAN))
                        .collect(),
                );
            }
        } else if let Some(an) = a.analytes.get(index as usize) {
            for w in 0..a.wells.len() {
                channels.push(
                    ticks
                        .iter()
                        .map(|t| {
                            t.analytes
                                .get(an)
                                .and_then(|x| x.corrected.get(w))
                                .copied()
                                .unwrap_or(f64::NAN)
                        })
                        .collect(),
                );
            }
        } else {
            channels.push(
                ticks
                    .iter()
                    .map(|t| t.tray_temperature.unwrap_or(f64::NAN))
                    .collect(),
            );
            channels.push(
                ticks
                    .iter()
                    .map(|t| t.environment_temperature.unwrap_or(f64::NAN))
                    .collect(),
            );
            for an in &a.analytes {
                channels.push(
                    ticks
                        .iter()
                        .map(|t| {
                            t.analytes
                                .get(an)
                                .and_then(|x| x.well_temperature)
                                .unwrap_or(f64::NAN)
                        })
                        .collect(),
                );
            }
        }
        Ok(Trace {
            trace: index,
            sweep: 0,
            first_sample: lo as u64,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let tables = self.tables();
        let (_, cols) = tables.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {index} out of range ({} tables)",
                tables.len()
            ))
        })?;
        Ok(Table {
            table: index,
            first_row,
            columns: cols
                .iter()
                .map(|(_, _, v, _)| {
                    let n = v.len() as u64;
                    let lo = first_row.min(n) as usize;
                    let hi = first_row.saturating_add(max_rows).min(n) as usize;
                    v[lo..hi].to_vec()
                })
                .collect(),
        })
    }

    fn experiment(&self) -> Option<Experiment> {
        let a = &self.a;
        let mut exp = Experiment::default();
        let origin = |from: &str| Origin {
            source: Source::Inferred,
            from: from.into(),
        };
        let mut ins = ExperimentInstrument {
            vendor: Some("Agilent (Seahorse)".into()),
            ..ExperimentInstrument::default()
        };
        if let Some(s) = a.scalar("InstrumentSerialNumber") {
            ins.serial = Some(s.to_string());
            exp.provenance
                .insert("instrument.serial".into(), origin("InstrumentSerialNumber"));
        }
        if let Some(v) = a.scalar("SWVersion") {
            ins.software_version = Some(v.to_string());
            exp.provenance
                .insert("instrument.software_version".into(), origin("SWVersion"));
        }
        exp.instrument = Some(ins);
        let mut method = Method {
            name: a.scalar("Name").map(str::to_string),
            ..Method::default()
        };
        if method.name.is_some() {
            exp.provenance.insert("method.name".into(), origin("Name"));
        }
        method
            .parameters
            .insert("wells".into(), Quantity::plain(a.wells.len() as u32));
        method
            .parameters
            .insert("measurements".into(), Quantity::plain(a.spans.len() as u32));
        exp.provenance
            .insert("method.parameters.wells".into(), origin("Plate/Wells"));
        exp.provenance.insert(
            "method.parameters.measurements".into(),
            origin("AssayDataSet/RateSpans"),
        );
        if let Some(v) = a.scalar("WellVolume").and_then(|s| s.parse::<f64>().ok()) {
            method
                .parameters
                .insert("well_volume".into(), Quantity::number(v, "µL"));
            exp.provenance
                .insert("method.parameters.well_volume".into(), origin("WellVolume"));
        }
        exp.method = Some(method);
        let first = a.commands.first().map(|c| c.start.clone());
        let last = a.commands.last().map(|c| c.end.clone());
        exp.acquisition = Some(Acquisition {
            started_at: first.clone().filter(|s| !s.is_empty()),
            ended_at: last.clone().filter(|s| !s.is_empty()),
            operator: a.scalar("LastRunBy").map(str::to_string),
            duration_s: match (&first, &last) {
                (Some(f), Some(l)) => seconds_between(f, l),
                _ => None,
            },
            comment: None,
            saved_at: None,
        });
        let acq = exp.acquisition.clone().unwrap_or_default();
        for (key, set, from) in [
            (
                "acquisition.started_at",
                acq.started_at.is_some(),
                "CommandHistory: first command's StartTime",
            ),
            (
                "acquisition.ended_at",
                acq.ended_at.is_some(),
                "CommandHistory: last command's EndTime",
            ),
            ("acquisition.operator", acq.operator.is_some(), "LastRunBy"),
            (
                "acquisition.duration_s",
                acq.duration_s.is_some(),
                "CommandHistory: first start to last end",
            ),
        ] {
            if set {
                exp.provenance.insert(key.into(), origin(from));
            }
        }
        exp.measurements.push(Measurement {
            kind: MeasurementKind::Trace,
            indices: (0..a.analytes.len() as u32).collect(),
            what: format!(
                "Seahorse XF sensor readings ({}) in {} wells: {} readings, {} measurements",
                a.analytes.join(", "),
                a.wells.len(),
                a.ticks.len(),
                a.spans.len()
            ),
            technique: None,
            terms: Vec::new(),
            parameters: BTreeMap::new(),
        });
        crate::complete_provenance(&mut exp);
        Some(exp)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]
    use super::*;

    #[test]
    fn durations_and_wells() {
        assert_eq!(duration_s("PT38M4.5S"), Some(2284.5));
        assert_eq!(duration_s("PT2H15M26S"), Some(8126.0));
        assert_eq!(duration_s("P1DT1S"), Some(86_401.0));
        assert_eq!(duration_s("PT5X"), None);
        assert_eq!(duration_s("38M"), None);
        assert_eq!(well_name(0, 0), "A1");
        assert_eq!(well_name(7, 11), "H12");
        assert_eq!(well_name(26, 0), "AA1");
    }
}
