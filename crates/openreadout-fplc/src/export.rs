//! UNICORN 6/7 result exports (`.zip`): XML documents, nested zips of .NET-serialized XML and
//! one nested zip of float arrays per curve. Layout: `docs/formats/cytiva-unicorn.md`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use openreadout_core::bytes::le_u16;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::source::{Input, MemSource};
use openreadout_core::xml::{child, children};
use openreadout_core::xmljson::xml_to_json;
use openreadout_core::zip::{ZipIndex, ZipMember};
use openreadout_core::{Error, Result};
use roxmltree::{Document, Node, ParsingOptions};
use serde_json::{Map, Value, json};

use crate::model::{
    Curve, CurveKind, Event, EventList, Facts, PEAK_COLUMNS, Parsed, Peak, PeakTable, Points, num,
    wavelength_of,
};
use crate::nrbf;

pub(crate) const FORMAT_ID: &str = "cytiva-unicorn-zip";
/// Largest nested member (a curve's zip) inflated into memory.
const MAX_NESTED: u64 = 1 << 30;

/// Logical name of a member: its base name without a `Copy of ` prefix or `.zip` suffix
/// (a re-packed export keeps both), lower case.
pub(crate) fn logical(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let base = base.strip_prefix("Copy of ").unwrap_or(base);
    let lower = base.to_ascii_lowercase();
    lower
        .strip_suffix(".zip")
        .map_or(lower.clone(), str::to_string)
}

/// Does a zip's member list look like a UNICORN result export (a chromatogram document and a
/// result document)?
pub(crate) fn is_unicorn_export<'a>(names: impl Iterator<Item = &'a str>) -> bool {
    let mut chrom = false;
    let mut result = false;
    for n in names {
        let l = logical(n);
        if l.starts_with("chrom.") && l.ends_with(".xml") {
            chrom = true;
        }
        if l == "result.xml" {
            result = true;
        }
    }
    chrom && result
}

/// The outer archive with a lookup by logical member name.
#[derive(Debug, Clone)]
pub(crate) struct Export {
    pub(crate) zip: ZipIndex,
    by_logical: BTreeMap<String, usize>,
}

impl Export {
    pub(crate) fn open(input: &Input) -> Result<Export> {
        let zip = ZipIndex::open(input.fs(), input.path(), FORMAT_ID)?.with_max_member(MAX_NESTED);
        let mut by_logical = BTreeMap::new();
        for (i, m) in zip.members.iter().enumerate() {
            by_logical.entry(logical(&m.name)).or_insert(i);
        }
        Ok(Export { zip, by_logical })
    }

    pub(crate) fn member(&self, logical_name: &str) -> Option<&ZipMember> {
        self.by_logical
            .get(&logical_name.to_ascii_lowercase())
            .and_then(|&i| self.zip.members.get(i))
    }

    fn text(&self, logical_name: &str) -> Result<Option<String>> {
        match self.member(logical_name) {
            Some(m) => Ok(Some(openreadout_core::zip::text(&self.zip.read(m)?))),
            None => Ok(None),
        }
    }

    /// A nested zip (a `…Data` member or a curve), cut after its end record.
    pub(crate) fn nested(&self, logical_name: &str) -> Result<Option<ZipIndex>> {
        let Some(m) = self.member(logical_name) else {
            return Ok(None);
        };
        let bytes = self.zip.read(m)?;
        if bytes.is_empty() {
            return Ok(None);
        }
        nested_zip(bytes, &m.name).map(Some)
    }

    /// The XML text of a nested `…Data` member (`Xml`: one .NET string).
    fn data_xml(&self, logical_name: &str) -> Result<Option<String>> {
        let Some(zip) = self.nested(logical_name)? else {
            return Ok(None);
        };
        let Some(raw) = zip.read_named("Xml")? else {
            return Ok(None);
        };
        let s = match nrbf::string(&raw) {
            Ok(s) => s,
            // not a .NET string record: plain XML (as a re-written export stores it) is read
            // from its first `<`; anything else is corrupt
            Err(e) => {
                let text = String::from_utf8_lossy(&raw);
                match text.find('<') {
                    Some(i) => text[i..].trim_end_matches(|c: char| c != '>').to_string(),
                    None => {
                        return Err(Error::corrupt(
                            FORMAT_ID,
                            format!("{logical_name}/Xml: {}", e.0),
                        ));
                    }
                }
            }
        };
        Ok((!s.trim().is_empty()).then_some(s))
    }
}

/// A zip held in memory whose end record may be followed by padding: cut after the last end
/// record (+ its comment) and index it.
pub(crate) fn nested_zip(mut bytes: Vec<u8>, name: &str) -> Result<ZipIndex> {
    const EOCD: &[u8] = b"PK\x05\x06";
    let at = bytes.windows(4).rposition(|w| w == EOCD).ok_or_else(|| {
        Error::corrupt(
            FORMAT_ID,
            format!("{name}: nested zip without an end-of-central-directory record"),
        )
    })?;
    let comment = le_u16(&bytes, at + 20).map_or(0, usize::from);
    let end = (at + 22 + comment).min(bytes.len());
    bytes.truncate(end);
    let src = Arc::new(MemSource::new(name.to_string(), bytes));
    ZipIndex::from_source(src, Path::new(name), FORMAT_ID).map(|z| z.with_max_member(MAX_NESTED))
}

/// Volumes and amplitudes of a curve's nested zip.
pub(crate) fn curve_points(
    z: &ZipIndex,
    name: &str,
    volume_grid: Option<(f64, f64)>,
) -> Result<(Vec<f64>, Vec<f64>)> {
    let arr = |member: &str| -> Result<Vec<f64>> {
        if let Some(t) = z.read_text(&format!("{member}DataType"))?
            && !t.trim().starts_with("System.Single[]")
        {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("curve `{name}` stored as `{}`", t.trim()),
                "Only curves stored as 32-bit floats (System.Single[]) are read; please report the file.",
            ));
        }
        let b = z.read_named(member)?.ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("curve `{name}`: no `{member}` member"))
        })?;
        nrbf::single_array(&b)
            .map_err(|e| Error::corrupt(FORMAT_ID, format!("curve `{name}` {member}: {}", e.0)))
    };
    let a = arr("CoordinateData.Amplitudes")?;
    let v = match volume_grid {
        Some((first, step)) if z.get("CoordinateData.Volumes").is_none() => {
            (0..a.len()).map(|i| first + step * i as f64).collect()
        }
        _ => arr("CoordinateData.Volumes")?,
    };
    Ok((v, a))
}

/// Samples of a curve's nested zip: both arrays inflated and their record headers checked (the
/// declared length must fill the member exactly), so `info` never reports a curve that does
/// not read. The floats themselves are not kept.
fn curve_samples(z: &ZipIndex, volume_grid: bool) -> std::result::Result<u64, String> {
    let n_of = |member: &str| -> std::result::Result<u64, String> {
        if let Ok(Some(t)) = z.read_text(&format!("{member}DataType"))
            && !t.trim().starts_with("System.Single[]")
        {
            return Err(format!(
                "`{member}` is stored as `{}`, not 32-bit floats",
                t.trim()
            ));
        }
        let b = z
            .read_named(member)
            .map_err(|e| format!("`{member}`: {e}"))?
            .ok_or_else(|| format!("no `{member}` member"))?;
        let (n, start) =
            nrbf::single_array_shape(&b).map_err(|e| format!("`{member}`: {}", e.0))?;
        if start + n * 4 + 1 != b.len() {
            return Err(format!(
                "`{member}`: {} bytes after the {n}-float array",
                b.len() - (start + n * 4 + 1)
            ));
        }
        Ok(n as u64)
    };
    let a = n_of("CoordinateData.Amplitudes")?;
    if volume_grid && z.get("CoordinateData.Volumes").is_none() {
        return Ok(a);
    }
    let v = n_of("CoordinateData.Volumes")?;
    if a != v {
        return Err(format!("{a} amplitudes but {v} volumes"));
    }
    Ok(a)
}

fn parse_xml<'a>(text: &'a str, what: &str) -> Result<Document<'a>> {
    let opts = ParsingOptions {
        allow_dtd: false,
        nodes_limit: 20_000_000,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, opts)
        .map_err(|e| Error::corrupt(FORMAT_ID, format!("{what}: XML does not parse: {e}")))
}

fn text(n: Node<'_, '_>, name: &str) -> Option<String> {
    let c = child(n, name)?;
    let t: String = c.children().filter_map(|x| x.text()).collect();
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn number(n: Node<'_, '_>, name: &str) -> Option<f64> {
    text(n, name).and_then(|t| num(&t))
}

/// `2024-02-14T18:13:47.412` + 60 minutes → `2024-02-14T18:13:47.412+01:00`.
fn with_offset(t: &str, minutes: Option<f64>) -> String {
    match minutes {
        Some(m) if m.is_finite() && m.fract() == 0.0 && m.abs() < 24.0 * 60.0 => {
            let m = m as i64;
            let sign = if m < 0 { '-' } else { '+' };
            format!("{t}{sign}{:02}:{:02}", m.abs() / 60, m.abs() % 60)
        }
        _ => t.to_string(),
    }
}

fn time_of(n: Node<'_, '_>, name: &str) -> Option<String> {
    let t = text(n, name)?;
    Some(with_offset(
        &t,
        number(n, &format!("{name}UtcOffsetMinutes")),
    ))
}

/// A metadata document (`…Data/Xml`): damage is a warning, never fatal (the curves do not
/// depend on it).
fn metadata(ex: &Export, logical_name: &str, findings: &mut Vec<Finding>) -> Option<String> {
    match ex.data_xml(logical_name) {
        Ok(x) => x,
        Err(e) => {
            findings.push(Finding::warning(
                "unreadable_metadata",
                format!("{logical_name}: {e}"),
            ));
            None
        }
    }
}

fn parse_meta<'a>(x: &'a str, what: &str, findings: &mut Vec<Finding>) -> Option<Document<'a>> {
    match parse_xml(x, what) {
        Ok(d) => Some(d),
        Err(e) => {
            findings.push(Finding::warning("unreadable_metadata", e.to_string()));
            None
        }
    }
}

/// Parse the export's documents; curve points stay in their members.
pub(crate) fn parse(ex: &Export) -> Result<Parsed> {
    let mut p = Parsed::default();
    for (i, m) in ex.zip.members.iter().enumerate() {
        p.entries.push(LsEntry {
            kind: member_kind(&logical(&m.name)).into(),
            name: m.name.clone(),
            offset: Some(m.local_offset),
            size: Some(m.size),
            image: None,
            details: json!({"member": i, "compressed_size": m.compressed_size, "method": m.method}),
        });
    }
    let mut vendor = Map::new();
    let mut facts = Facts::default();
    let mut provenance = ProvenanceMap::new();

    // Result.xml: names, system, creator, run information, method variables
    let result_text = ex.text("result.xml")?.ok_or_else(|| {
        Error::corrupt(
            FORMAT_ID,
            "no Result.xml member: not a complete UNICORN result export",
        )
    })?;
    let rdoc = parse_xml(&result_text, "Result.xml")?;
    let r = rdoc.root_element();
    let unicorn_version = r.attribute("UNICORNVersion").map(str::to_string);
    p.format_version = r
        .attribute("FormatVersion")
        .map(|v| format!("result format {v}"));
    if let Some(v) = &unicorn_version {
        Facts::text(&mut facts.software_version, v, "Result.xml @UNICORNVersion");
    }
    if let Some(v) = text(r, "CreatedBy") {
        Facts::text(&mut facts.operator, &v, "Result.xml CreatedBy");
    }
    let mut run_info = Map::new();
    for ri in children(r, "ResultRunInformation") {
        let kind = ri.attribute("RunInformationType").unwrap_or("").to_string();
        let raw: String = text(ri, "RunInformation")
            .unwrap_or_default()
            .split_whitespace()
            .collect();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(raw.as_bytes())
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        if let Some(d) = decoded {
            let v = if d.trim_start().starts_with('<') {
                xml_to_json(d.trim_start().trim_start_matches('\u{feff}'))
                    .unwrap_or(Value::String(d))
            } else {
                Value::String(d)
            };
            run_info.insert(kind, v);
        }
    }
    let mut variables = Map::new();
    if let Some(sc) = child(r, "ResultSearchCriterias") {
        for c in children(sc, "ResultSearchCriteria") {
            if text(c, "Name").as_deref() != Some("VariableValue") {
                continue;
            }
            let (Some(k), v) = (text(c, "Keyword1"), text(c, "Keyword2").unwrap_or_default())
            else {
                continue;
            };
            let unit = text(c, "ExtraDisplayInformation").unwrap_or_default();
            variables.insert(k.clone(), json!({"value": v, "unit": unit}));
            method_variable(&mut facts, &k, &v, &unit);
        }
    }
    let mut result = Map::new();
    for k in [
        "Name",
        "SystemName",
        "SystemTypeName",
        "BatchId",
        "FolderPath",
        "ResultState",
        "RunType",
        "CreatedBy",
        "LastModifiedBy",
    ] {
        if let Some(v) = text(r, k) {
            result.insert(k.into(), json!(v));
        }
    }
    for k in ["Created", "LastModified"] {
        if let Some(v) = time_of(r, k) {
            result.insert(k.into(), json!(v));
        }
    }
    if let Some(v) = &unicorn_version {
        result.insert("UNICORNVersion".into(), json!(v));
    }
    result.insert("RunInformation".into(), Value::Object(run_info));
    result.insert("Variables".into(), Value::Object(variables));
    vendor.insert("result".into(), Value::Object(result));
    if let Some(s) = text(r, "SystemName") {
        vendor.insert("system_name".into(), json!(s));
    }

    // instrument configuration: model and firmware
    if let Some(x) = metadata(ex, "instrumentconfigurationdata", &mut p.findings)
        && let Some(d) = parse_meta(&x, "InstrumentConfigurationData", &mut p.findings)
    {
        let root = d.root_element();
        if let Some(v) = text(root, "Description") {
            Facts::text(
                &mut facts.model,
                &v,
                "InstrumentConfigurationData Description",
            );
        }
        if let (Some(n), Some(v)) = (text(root, "FirmwareName"), text(root, "FirmwareVersion")) {
            Facts::text(
                &mut facts.firmware,
                &format!("{n} {v}"),
                "InstrumentConfigurationData Firmware",
            );
        }
        vendor.insert(
            "instrument_configuration".into(),
            xml_to_json(&x).unwrap_or(Value::Null),
        );
    }
    // column type
    if let Some(x) = metadata(ex, "columntypedata", &mut p.findings)
        && let Some(d) = parse_meta(&x, "ColumnTypeData", &mut p.findings)
    {
        if let Some(ct) = child(d.root_element(), "ColumnType") {
            if let Some(v) = text(ct, "Name") {
                facts.word("column", &v, "ColumnTypeData ColumnType/Name");
            }
            if let Some(v) = text(ct, "ArticleNumber") {
                facts.word(
                    "column_article_number",
                    &v,
                    "ColumnTypeData ColumnType/ArticleNumber",
                );
            }
            if let Some(v) = number(ct, "BedHeight")
                && text(ct, "BedHeightUnit").as_deref() == Some("cm")
            {
                facts.number(
                    "column_bed_height",
                    v,
                    "cm",
                    "ColumnTypeData ColumnType/BedHeight",
                );
            }
            if let Some(t) = text(ct, "TechniqueName")
                .or_else(|| child(ct, "Media").and_then(|m| text(m, "TechniqueName")))
                && facts.technique.is_none()
            {
                facts.technique = crate::res::technique_of(&t);
            }
        }
        vendor.insert(
            "column_types".into(),
            xml_to_json(&x).unwrap_or(Value::Null),
        );
    }
    // method: name and technique
    if let Some(x) = metadata(ex, "methoddata", &mut p.findings)
        && let Some(d) = parse_meta(&x, "MethodData", &mut p.findings)
    {
        let root = d.root_element();
        if let Some(t) = text(root, "TechniqueName") {
            facts.word("technique", &t, "MethodData TechniqueName");
            if let Some(id) = crate::res::technique_of(&t) {
                facts.technique = Some(id);
            }
        }
        vendor.insert("method".into(), xml_to_json(&x).unwrap_or(Value::Null));
        if let Some(desc) = text(root, "Description") {
            vendor.insert("method_description".into(), json!(desc));
        }
    }
    if let Some(x) = metadata(ex, "systemdata", &mut p.findings)
        && let Some(d) = parse_meta(&x, "SystemData", &mut p.findings)
        && let Some(sys) = child(d.root_element(), "System")
    {
        let mut s = Map::new();
        if let Some(v) = text(sys, "Name") {
            s.insert("Name".into(), json!(v));
        }
        if let Some(t) = sys.attribute("SystemType") {
            s.insert("SystemType".into(), json!(t));
        }
        if let Some(ic) = child(sys, "InstrumentConfiguration") {
            for a in ["Description", "Version"] {
                if let Some(v) = ic.attribute(a) {
                    s.insert(format!("InstrumentConfiguration{a}"), json!(v));
                }
            }
            if let Some(v) = ic.attribute("Description") {
                Facts::text(
                    &mut facts.model,
                    v,
                    "SystemData InstrumentConfiguration@Description",
                );
            }
        }
        vendor.insert("system".into(), Value::Object(s));
    }
    if let Some(t) = ex.text("evaluationlog.xml")? {
        vendor.insert(
            "evaluation_log".into(),
            xml_to_json(&t).unwrap_or(Value::Null),
        );
    }

    // chromatograms
    let mut chrom_names: Vec<String> = ex
        .by_logical
        .keys()
        .filter(|k| k.starts_with("chrom.") && k.ends_with(".xml"))
        .cloned()
        .collect();
    chrom_names.sort_by_key(|k| {
        k.trim_start_matches("chrom.")
            .trim_end_matches(".xml")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    let several = chrom_names.len() > 1;
    let mut chrom_meta = Vec::new();
    for cn in &chrom_names {
        let t = ex.text(cn)?.unwrap_or_default();
        let doc = parse_xml(&t, cn)?;
        let c = doc.root_element();
        let chrom_name =
            text(c, "ChromatogramName").unwrap_or_else(|| cn.trim_end_matches(".xml").to_string());
        let prefix = if several {
            format!("{chrom_name}: ")
        } else {
            String::new()
        };
        let time_unit = text(c, "TimeUnit").unwrap_or_else(|| "min".into());
        chrom_meta.push(json!({
            "name": chrom_name,
            "format_version": c.attribute("FormatVersion"),
            "unicorn_version": c.attribute("UNICORNVersion"),
            "created": time_of(c, "Created"),
            "time_unit": time_unit,
            "volume_unit": text(c, "VolumeUnit"),
        }));
        let first_curve = p.curves.len();
        let mut by_number: BTreeMap<String, u32> = BTreeMap::new();
        if let Some(curves) = child(c, "Curves") {
            for cv in children(curves, "Curve") {
                if let Some(curve) = zip_curve(ex, cv, &prefix, &mut p.findings, &mut facts) {
                    if let Some(num) = text(cv, "CurveNumber") {
                        by_number.insert(num, p.curves.len() as u32);
                    }
                    p.curves.push(curve);
                }
            }
        }
        if let Some(evs) = child(c, "EventCurves") {
            for ec in children(evs, "EventCurve") {
                let kind = ec.attribute("EventCurveType").unwrap_or("");
                let (name, label) = match kind {
                    "Fraction" => ("fractions".to_string(), "fraction"),
                    "Injection" => ("injections".to_string(), "injection"),
                    "Logbook" => ("logbook".to_string(), "text"),
                    other => (other.to_ascii_lowercase(), "text"),
                };
                let mut events = Vec::new();
                if let Some(list) = child(ec, "Events") {
                    for e in children(list, "Event") {
                        events.push(Event {
                            time_min: number(e, "EventTime").unwrap_or(f64::NAN),
                            volume_ml: number(e, "EventVolume").unwrap_or(f64::NAN),
                            text: text(e, "EventText").unwrap_or_default(),
                        });
                    }
                }
                p.events.push(EventList {
                    name: format!("{prefix}{name}"),
                    label,
                    events,
                });
            }
        }
        if let Some(pts) = child(c, "PeakTables") {
            for pt in children(pts, "PeakTable") {
                p.peak_tables
                    .push(peak_table(pt, &by_number, &p.curves, &prefix));
            }
        }
        let _ = first_curve;
    }
    vendor.insert("chromatograms".into(), json!(chrom_meta));

    // UNICORN shows volumes from the last injection
    p.zero_volume_ml = p
        .events
        .iter()
        .find(|l| l.name.ends_with("injections"))
        .and_then(|l| l.events.last())
        .map(|e| e.volume_ml)
        .filter(|v| v.is_finite());
    if let Some(z) = p.zero_volume_ml {
        for c in &mut p.curves {
            c.extra.insert("injection_volume_ml".into(), json!(z));
        }
    }
    if let Some(line) = p
        .events
        .iter()
        .find(|l| l.name.ends_with("logbook"))
        .and_then(|l| l.events.iter().find(|e| e.text.starts_with("Method Run")))
        && let Some((_, m)) = line.text.split_once("Method:")
    {
        Facts::text(
            &mut facts.method_name,
            m.trim(),
            "logbook `Method Run … Method: <name>`",
        );
    }
    if facts.method_name.is_none()
        && let Some(Value::String(d)) = vendor.get("method_description")
    {
        let d = d.clone();
        Facts::text(&mut facts.method_name, &d, "MethodData Description");
    }
    let duration_min = p
        .curves
        .iter()
        .filter(|c| c.original && c.interval_min > 0.0)
        .map(|c| c.start_min + c.interval_min * c.samples.saturating_sub(1) as f64)
        .fold(0.0_f64, f64::max);
    if duration_min > 0.0 {
        facts.number(
            "run_duration",
            duration_min,
            "min",
            "curves: start + interval × samples",
        );
    }
    for k in ["traces", "tables"] {
        provenance.insert(k.into(), Source::Inferred);
    }
    p.vendor = json!({ "unicorn_export": Value::Object(vendor) });
    p.facts = facts;
    p.provenance = provenance;
    Ok(p)
}

fn member_kind(l: &str) -> &'static str {
    if l.starts_with("chrom.") && l.ends_with(".xml") {
        "chromatogram"
    } else if l.starts_with("chrom.") {
        "curve"
    } else if l.ends_with(".xml") {
        "document"
    } else {
        "data"
    }
}

/// A method variable from Result.xml as an experiment parameter (the common ones only; all are
/// in the vendor tree).
fn method_variable(facts: &mut Facts, key: &str, value: &str, unit: &str) {
    let from = format!("Result.xml variable `{key}`");
    let n = num(value);
    match (key, unit, n) {
        ("Flow rate", "ml/min", Some(v)) => facts.number("flow_rate", v, "mL/min", &from),
        ("Column type", _, _) => facts.word("column", value, &from),
        ("Column volume", "ml", Some(v)) => facts.number("column_volume", v, "mL", &from),
        ("Sample_ID" | "Sample ID" | "Sample name", _, _) => {
            Facts::text(&mut facts.sample_id, value, &from);
        }
        ("Frac volume (Elution)" | "Fraction volume", "ml", Some(v)) => {
            facts.number("fraction_volume", v, "mL", &from);
        }
        (
            "UV1" | "UV2" | "UV3" | "Wavelength 1" | "Wavelength 2" | "Wavelength 3",
            "nm",
            Some(v),
        ) if v > 0.0 => {
            let k = format!("uv_wavelength_{}", key.chars().last().unwrap_or('1'));
            facts.number(&k, v, "nm", &from);
        }
        _ => {}
    }
}

/// One `<Curve>` element: metadata from the XML, the sample count from its nested zip.
fn zip_curve(
    ex: &Export,
    cv: Node<'_, '_>,
    prefix: &str,
    findings: &mut Vec<Finding>,
    facts: &mut Facts,
) -> Option<Curve> {
    let name = text(cv, "Name").unwrap_or_else(|| "curve".into());
    let full = format!("{prefix}{name}");
    let vendor_type = cv.attribute("CurveDataType").map(str::to_string);
    let unit = text(cv, "AmplitudeUnit");
    // the full-resolution point file (the only one in the exports seen)
    let file = child(cv, "CurvePoints").and_then(|pts| {
        let all: Vec<_> = children(pts, "CurvePoint").collect();
        all.iter()
            .find(|p| text(**p, "IsFullResolution").as_deref() == Some("true"))
            .or_else(|| all.first())
            .and_then(|p| text(*p, "BinaryCurvePointsFileName"))
    });
    let Some(file) = file else {
        findings.push(Finding::warning(
            "curve_without_points",
            format!("curve `{full}` names no point file"),
        ));
        return None;
    };
    let Some(member) = ex.member(&logical(&file)).map(|m| m.name.clone()) else {
        findings.push(Finding::error(
            "missing_member",
            format!("curve `{full}`: its points `{file}` are not in the archive"),
        ));
        return None;
    };
    let iso = text(cv, "IsoChroneType").unwrap_or_else(|| "Time".into());
    let dbp = number(cv, "DistanceBetweenPoints");
    let dsp = number(cv, "DistanceToStartPoint");
    let volume_grid = match (iso.as_str(), dbp, dsp) {
        ("Volume", Some(d), Some(s)) if d > 0.0 && s.is_finite() => Some((s, d)),
        _ => None,
    };
    let samples = match ex.nested(&logical(&file)) {
        Ok(Some(z)) => match curve_samples(&z, volume_grid.is_some()) {
            Ok(n) => n,
            Err(e) => {
                findings.push(Finding::error(
                    "unreadable_curve",
                    format!("curve `{full}`: {e}"),
                ));
                return None;
            }
        },
        Ok(None) => {
            findings.push(Finding::error(
                "unreadable_curve",
                format!("curve `{full}`: its point file `{file}` is empty"),
            ));
            return None;
        }
        Err(e) => {
            findings.push(Finding::error(
                "unreadable_curve",
                format!("curve `{full}`: {e}"),
            ));
            return None;
        }
    };
    let time_unit = text(cv, "TimeUnit").unwrap_or_else(|| "min".into());
    let to_min = match time_unit.as_str() {
        "min" => Some(1.0),
        "s" | "sec" => Some(1.0 / 60.0),
        "h" => Some(60.0),
        _ => None,
    };
    let original = text(cv, "IsOriginalData").as_deref() != Some("false");
    let mut extra = BTreeMap::new();
    extra.insert("member".into(), json!(member));
    if let Some(t) = &vendor_type {
        extra.insert("curve_type".into(), json!(t));
    }
    if let Some(n) = text(cv, "CurveNumber") {
        extra.insert(
            "curve_number".into(),
            json!(num(&n).map_or(json!(n), |v| json!(v))),
        );
    }
    extra.insert("isochrone".into(), json!(iso.to_ascii_lowercase()));
    if let Some(v) = number(cv, "ColumnVolume") {
        extra.insert("column_volume_ml".into(), json!(v));
        facts.number("column_volume", v, "mL", "Chrom.1.Xml Curve/ColumnVolume");
    }
    if let Some(v) = number(cv, "AmplitudePrecision") {
        extra.insert("display_decimals".into(), json!(v));
    }
    if let Some(uv) = child(cv, "CurveUVInfo") {
        if let Some(v) = number(uv, "UVPathLength") {
            extra.insert("uv_path_length".into(), json!(v));
        }
        if let Some(v) = number(uv, "NominalUVPathLength") {
            extra.insert("uv_nominal_path_length".into(), json!(v));
        }
        if let Some(v) = text(uv, "IsUVValuesNormalizedToNominalUVPathLength") {
            extra.insert("uv_normalized_to_nominal_path".into(), json!(v == "true"));
        }
    }
    if let Some(t) = time_of(cv, "MethodStartTime") {
        extra.insert("method_start".into(), json!(t));
        if facts.started_at.is_none() {
            facts.started_at = Some((
                t,
                "Chrom.1.Xml Curve/MethodStartTime".into(),
                Source::Inferred,
            ));
        }
    }
    let mut interval_min = 0.0;
    let mut start_min = 0.0;
    if iso == "Time" {
        match (dbp, dsp, to_min) {
            (Some(d), Some(s), Some(k)) if d > 0.0 && s.is_finite() => {
                interval_min = d * k;
                start_min = s * k;
            }
            _ => {
                findings.push(Finding::warning(
                    "no_time_axis",
                    format!(
                        "curve `{full}`: sampling interval {dbp:?} {time_unit} is not usable; its samples keep their volumes but have no times"
                    ),
                ));
            }
        }
    } else if let (Some(d), Some(s)) = (dbp, dsp) {
        extra.insert("volume_step_ml".into(), json!(d));
        extra.insert("volume_start_ml".into(), json!(s));
    }
    let unit = unit.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
    let kind = CurveKind::classify(&name, unit.as_deref().unwrap_or(""), vendor_type.as_deref());
    Some(Curve {
        wavelength_nm: if kind == CurveKind::Uv {
            wavelength_of(&name)
        } else {
            None
        },
        name: full,
        kind,
        unit,
        samples,
        start_min,
        interval_min,
        points: Points::Zip {
            member,
            volume_grid,
        },
        original,
        extra,
    })
}

fn peak_table(
    pt: Node<'_, '_>,
    by_number: &BTreeMap<String, u32>,
    curves: &[Curve],
    prefix: &str,
) -> PeakTable {
    let curve_number = child(pt, "DataCurve").and_then(|d| text(d, "CurveNumber"));
    let trace = curve_number
        .as_ref()
        .and_then(|n| by_number.get(n))
        .copied();
    let basis = text(pt, "CalculationRetention").unwrap_or_else(|| "Volume".into());
    let retention_unit = if basis.eq_ignore_ascii_case("time") {
        "min"
    } else {
        "ml"
    };
    let height_unit = trace
        .and_then(|t| curves.get(t as usize))
        .and_then(|c| c.unit.clone());
    let mut extra = BTreeMap::new();
    for (ours, theirs) in [
        ("detected_peaks", "NumberOfDetectedPeaks"),
        ("total_peak_area", "TotalPeakArea"),
        ("total_area_evaluated_peaks", "TotalPeakAreaEvaluatedPeaks"),
        ("ratio_peak_area_total_area", "RatioPeakAreaTotalArea"),
        ("max_peaks", "MaxNumberOfPeaks"),
        ("zero_at_injection", "ZeroAdjustedToInjectionNumber"),
        ("column_volume_ml", "ColumnVolume"),
    ] {
        if let Some(v) = number(pt, theirs) {
            extra.insert(ours.into(), json!(v));
        }
    }
    for (ours, theirs) in [
        ("algorithm", "ResolutionAlgorithm"),
        ("technique", "TechniqueName"),
    ] {
        if let Some(v) = text(pt, theirs) {
            extra.insert(ours.into(), json!(v));
        }
    }
    if let Some(t) = time_of(pt, "Created") {
        extra.insert("created".into(), json!(t));
    }
    if let Some(n) = &curve_number {
        extra.insert("curve_number".into(), json!(num(n)));
    }
    // heights, areas and endpoint heights are measured above this curve (zero without one)
    let baseline = child(pt, "BaseLine").and_then(|d| text(d, "CurveNumber"));
    match &baseline {
        Some(b) => {
            extra.insert("baseline_curve_number".into(), json!(num(b)));
            if let Some(t) = by_number.get(b) {
                extra.insert("baseline_trace".into(), json!(t));
            }
            extra.insert("height_reference".into(), json!("baseline"));
        }
        None => {
            extra.insert("height_reference".into(), json!("zero"));
        }
    }
    let mut peaks = Vec::new();
    if let Some(list) = child(pt, "Peaks") {
        for pk in children(list, "Peak") {
            let mut values = BTreeMap::new();
            for (ours, theirs, _) in PEAK_COLUMNS {
                values.insert(*ours, number(pk, theirs).unwrap_or(f64::NAN));
            }
            peaks.push(Peak {
                values,
                name: text(pk, "Name").unwrap_or_default(),
            });
        }
    }
    PeakTable {
        name: format!(
            "{prefix}{}",
            text(pt, "Name").unwrap_or_else(|| "peaks".into())
        ),
        trace,
        basis: basis.to_ascii_lowercase(),
        retention_unit: retention_unit.into(),
        height_unit,
        peaks,
        extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_names() {
        assert_eq!(logical("Chrom.1_1_True"), "chrom.1_1_true");
        assert_eq!(
            logical("Unicorn Results Sept 25, 2024/Copy of Chrom.1_10_True.zip"),
            "chrom.1_10_true"
        );
        assert_eq!(logical("Copy of Result.xml"), "result.xml");
        assert!(is_unicorn_export(
            ["Result.xml", "Chrom.1.Xml", "SystemData"].into_iter()
        ));
        assert!(!is_unicorn_export(["rdml_data.xml"].into_iter()));
    }

    #[test]
    fn offsets() {
        assert_eq!(
            with_offset("2024-02-14T18:13:47.412", Some(60.0)),
            "2024-02-14T18:13:47.412+01:00"
        );
        assert_eq!(
            with_offset("2023-03-10T16:30:07", Some(-300.0)),
            "2023-03-10T16:30:07-05:00"
        );
        assert_eq!(
            with_offset("2023-03-10T16:30:07", None),
            "2023-03-10T16:30:07"
        );
    }

    #[test]
    fn nested_zip_padding() {
        // a stored one-member zip followed by zeros past the 64 KiB end-record search window
        let z = openreadout_core::zip::zip_bytes(&[("a", b"hello")]).unwrap();
        let mut padded = z.clone();
        padded.extend(std::iter::repeat_n(0u8, 150_000));
        let idx = nested_zip(padded, "t").unwrap();
        assert_eq!(idx.read_named("a").unwrap().unwrap(), b"hello");
        assert!(nested_zip(vec![0u8; 100], "t").is_err());
    }
}
