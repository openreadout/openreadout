//! Bio-Rad Image Lab `.scn`: a MIME multipart document of XML headers and raw 16-bit images.
//! Layout: `docs/formats/biorad-scn.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Measurement, MeasurementKind, Method, Origin,
    Quantity, Sample,
};
use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, FormatDescriptor, ImageInfo, LsEntry,
    PhysicalSize, Table, TableInfo,
};
use openreadout_core::pixel::{PixelType, Plane, plane_bytes_checked};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::SourceFile;
use openreadout_core::time::unix_to_iso8601;
use openreadout_core::vocab;
use openreadout_core::xml::child;
use openreadout_core::xmljson::xml_to_json;
use openreadout_core::{ColumnInfo, Error, Result};
use roxmltree::{Document, Node, ParsingOptions};
use serde_json::{Map, Value, json};

use crate::mime::{self, Bytes, Part};

pub(crate) const FORMAT_ID: &str = "biorad-scn";
/// Largest XML part parsed.
const MAX_XML: u64 = 16 << 20;

struct Src<'a> {
    file: &'a SourceFile,
    len: u64,
}

impl Bytes for Src<'_> {
    fn read(&self, offset: u64, len: u64) -> std::result::Result<Vec<u8>, String> {
        let end = offset.saturating_add(len).min(self.len);
        if offset >= end {
            return Ok(Vec::new());
        }
        let n = usize::try_from(end - offset).map_err(|_| "read too large".to_string())?;
        let mut buf = vec![0u8; n];
        self.file
            .read_exact_at(offset, &mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }
    fn len(&self) -> u64 {
        self.len
    }
}

/// One scan (image) of the file.
#[derive(Debug, Clone)]
struct Scan {
    name: Option<String>,
    width: u32,
    height: u32,
    /// Offset and length of the pixel data.
    data: (u64, u64),
    bytes_per_sample: u32,
    pixel_um: (Option<f64>, Option<f64>),
    channel: ChannelInfo,
    extra: BTreeMap<String, Value>,
    /// Acquisition facts: (our key, value, vendor field).
    facts: Vec<(&'static str, String, String)>,
}

/// An opened `.scn` file.
#[derive(Debug)]
pub struct ScnDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    file: SourceFile,
    file_len: u64,
    format_version: Option<String>,
    scans: Vec<ScanRef>,
    log: Vec<(f64, String, String)>,
    entries: Vec<LsEntry>,
    vendor: Value,
    findings: Vec<Finding>,
    item: BTreeMap<&'static str, (String, String)>,
}

/// `Scan` without the trait object noise, kept `Debug`.
#[derive(Debug, Clone)]
struct ScanRef(Scan);

fn xml_doc(text: &str) -> Option<Document<'_>> {
    let opts = ParsingOptions {
        allow_dtd: true, // every part starts `<!DOCTYPE XML>`; no entities are declared
        nodes_limit: 2_000_000,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, opts).ok()
}

fn text_of(n: Node<'_, '_>) -> Option<String> {
    let t: String = n.children().filter_map(|c| c.text()).collect();
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn text(n: Node<'_, '_>, name: &str) -> Option<String> {
    child(n, name).and_then(text_of)
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// XML text of a part, from its first `<` (parts start `<!DOCTYPE XML>`).
fn xml_text(src: &Src<'_>, p: &Part) -> std::result::Result<String, String> {
    if p.len > MAX_XML {
        return Err(format!("{}: XML part of {} bytes", p.description(), p.len));
    }
    let b = src.read(p.offset, p.len)?;
    let s = String::from_utf8_lossy(&b).into_owned();
    Ok(s)
}

/// XML without the `<!DOCTYPE XML>` line (roxmltree reads it, but a vendor tree does not need it).
fn strip_doctype(s: &str) -> &str {
    let t = s.trim_start_matches('\u{feff}').trim_start();
    if let Some(rest) = t.strip_prefix("<!DOCTYPE")
        && let Some(i) = rest.find('>')
    {
        return rest[i + 1..].trim_start();
    }
    t
}

impl ScnDataset {
    pub(crate) fn open(
        descriptor: FormatDescriptor,
        path: PathBuf,
        file: SourceFile,
        file_len: u64,
    ) -> Result<ScnDataset> {
        let src = Src {
            file: &file,
            len: file_len,
        };
        let (top, parts) = mime::parse(&src).map_err(|e| Error::corrupt(FORMAT_ID, e))?;
        let version = top
            .get("mime-version")
            .and_then(|v| v.split("Image Lab").nth(1))
            .map(|v| v.trim().trim_end_matches(')').trim().to_string())
            .filter(|v| !v.is_empty());
        let mut ds = ScnDataset {
            descriptor,
            path,
            file_len,
            format_version: version.clone().map(|v| format!("Image Lab {v}")),
            scans: Vec::new(),
            log: Vec::new(),
            entries: Vec::new(),
            vendor: Value::Null,
            findings: Vec::new(),
            item: BTreeMap::new(),
            file: file
                .try_clone()
                .map_err(|e| Error::io(std::path::Path::new(""), e))?,
        };
        let mut vendor = Map::new();
        vendor.insert(
            "mime".into(),
            json!(
                top.iter()
                    .map(|(k, v)| (k.clone(), json!(v)))
                    .collect::<Map<_, _>>()
            ),
        );
        let mut xml_parts = Map::new();
        for (i, p) in parts.iter().enumerate() {
            ds.entries.push(LsEntry {
                kind: if p.header("content-type").is_some_and(|c| c.starts_with("multipart/")) {
                    "multipart".into()
                } else if p.header("content-type").is_some_and(|c| c.contains("xml")) {
                    "xml".into()
                } else {
                    "data".into()
                },
                name: p.description().to_string(),
                offset: Some(p.offset),
                size: Some(p.len),
                image: None,
                details: json!({"part": i, "depth": p.depth, "content_type": p.header("content-type")}),
            });
        }
        // item header and protocol settings (top-level XML parts)
        for p in parts.iter().filter(|p| p.depth == 0) {
            let desc = p.description().to_string();
            if !p.header("content-type").is_some_and(|c| c.contains("xml")) {
                continue;
            }
            let x = match xml_text(&src, p) {
                Ok(x) => x,
                Err(e) => {
                    ds.findings.push(Finding::warning("unreadable_part", e));
                    continue;
                }
            };
            let Some(doc) = xml_doc(&x) else {
                ds.findings.push(Finding::warning(
                    "unreadable_part",
                    format!("{desc}: XML does not parse"),
                ));
                continue;
            };
            let root = doc.root_element();
            if desc == "ItemHeaderTag" {
                for (ours, theirs) in [
                    ("name", "name"),
                    ("user", "user"),
                    ("description", "description"),
                    ("scan_id", "scan_id"),
                    ("channel_count", "channel_count"),
                ] {
                    if let Some(v) = text(root, theirs) {
                        ds.item.insert(ours, (v, format!("ItemHeaderTag/{theirs}")));
                    }
                }
                if let Some(a) = child(root, "FileAuthoringInfo")
                    && let Some(v) = text(a, "version")
                {
                    ds.item.insert(
                        "software_version",
                        (v, "ItemHeaderTag/FileAuthoringInfo/version".into()),
                    );
                }
            }
            if desc == "ItemProtocolSettingsTag"
                && let Some(list) = child(root, "ListLogEntries")
            {
                for e in list.children().filter(roxmltree::Node::is_element) {
                    let t = text(e, "Timestamp")
                        .and_then(|s| num(&s))
                        .unwrap_or(f64::NAN);
                    let who = text(e, "FullName")
                        .or_else(|| text(e, "User"))
                        .unwrap_or_default();
                    let what = text(e, "Data").unwrap_or_default();
                    ds.log.push((t, who, what));
                }
            }
            xml_parts.insert(desc, xml_to_json(strip_doctype(&x)).unwrap_or(Value::Null));
        }
        // scans: every multipart part holding an ImageData and an ImageHeader
        for (i, p) in parts.iter().enumerate() {
            if !p
                .header("content-type")
                .is_some_and(|c| c.starts_with("multipart/"))
            {
                continue;
            }
            let kids: Vec<&Part> = parts.iter().filter(|k| k.parent == Some(i)).collect();
            let data = kids.iter().find(|k| k.description() == "ImageData");
            let header = kids.iter().find(|k| k.description() == "ImageHeader");
            let (Some(data), Some(header)) = (data, header) else {
                continue;
            };
            match scan(&src, p.description(), data, header, &mut xml_parts) {
                Ok(s) => ds.scans.push(ScanRef(s)),
                Err(e) => ds.findings.push(Finding::error(
                    "unreadable_scan",
                    format!("{}: {e}", p.description()),
                )),
            }
        }
        if ds.scans.is_empty() && ds.findings.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                "no image part (a ScanImageTag with ImageData and ImageHeader) in the file",
            ));
        }
        vendor.insert("parts".into(), Value::Object(xml_parts));
        ds.vendor = json!({ "image_lab": Value::Object(vendor) });
        Ok(ds)
    }

    fn scan(&self, image: u32) -> Result<&Scan> {
        self.scans.get(image as usize).map(|s| &s.0).ok_or_else(|| {
            Error::Usage(format!(
                "image {image} out of range (file has {} images)",
                self.scans.len()
            ))
        })
    }
}

/// Our names for `scan_attributes` entries (by element name), and their fact keys.
const ATTRIBUTES: &[(&str, &str)] = &[
    ("imager", "imager"),
    ("image_date", "image_date"),
    ("exposure_time", "exposure_time_s"),
    ("serial_number", "serial_number"),
    ("software_version", "imager_software_version"),
    ("application", "application"),
    ("excitation_source", "excitation_source"),
    ("emission_filter", "emission_filter"),
    ("binning", "binning"),
    ("flat_field", "flat_field"),
    ("program", "program"),
];

fn scan(
    src: &Src<'_>,
    tag: &str,
    data: &Part,
    header: &Part,
    xml_parts: &mut Map<String, Value>,
) -> std::result::Result<Scan, String> {
    let xml = xml_text(src, header)?;
    let doc = xml_doc(&xml).ok_or("ImageHeader: XML does not parse")?;
    let root = doc.root_element();
    xml_parts.insert(
        format!("{tag}/ImageHeader"),
        xml_to_json(strip_doctype(&xml)).unwrap_or(Value::Null),
    );
    let size = child(root, "size_pix").ok_or("ImageHeader has no size_pix")?;
    let dim = |a: &str| -> std::result::Result<u32, String> {
        size.attribute(a)
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|v| *v > 0)
            .ok_or_else(|| format!("size_pix @{a} is not a positive integer"))
    };
    let (w, h) = (dim("width")?, dim("height")?);
    let endian = text(root, "endian").unwrap_or_else(|| "little".into());
    if endian != "little" {
        return Err(format!(
            "{endian}-endian pixel data (only little-endian files have been seen)"
        ));
    }
    let px = u64::from(w) * u64::from(h);
    let bps = if data.len == px * 2 {
        2
    } else if data.len == px {
        1
    } else {
        return Err(format!(
            "{} bytes of image data for {w} × {h} pixels (neither 1 nor 2 bytes per pixel)",
            data.len
        ));
    };
    let mut extra = BTreeMap::new();
    let mut pixel_um = (None, None);
    if let Some(mm) = child(root, "size_mm")
        && mm.attribute("known") == Some("true")
        && let (Some(wm), Some(hm)) = (
            mm.attribute("width").and_then(num),
            mm.attribute("height").and_then(num),
        )
        && wm > 0.0
        && hm > 0.0
    {
        pixel_um = (
            Some(wm * 1000.0 / f64::from(w)),
            Some(hm * 1000.0 / f64::from(h)),
        );
        extra.insert("size_mm".into(), json!([wm, hm]));
    }
    if let Some(o) = child(root, "org_size_pix")
        && let (Some(ow), Some(oh)) = (
            o.attribute("width").and_then(|v| v.parse::<u32>().ok()),
            o.attribute("height").and_then(|v| v.parse::<u32>().ok()),
        )
        && (ow, oh) != (w, h)
    {
        extra.insert("original_size_px".into(), json!([ow, oh]));
    }
    if let Some(z) = child(root, "image").and_then(|i| i.attribute("zero_is")) {
        extra.insert("zero_is".into(), json!(z));
        extra.insert("display_inverted".into(), json!(z == "white"));
    }
    if let Some(s) = child(root, "scanner") {
        for a in ["data_ceiling", "max_value"] {
            if let Some(v) = s.attribute(a).and_then(num) {
                extra.insert(a.into(), json!(v));
            }
        }
    }
    if let Some(m) = child(root, "scaler").and_then(|s| child(s, "moldyn")) {
        extra.insert(
            "gel_scaling".into(),
            json!({"mode": m.attribute("mode").and_then(num), "real_scale": m.attribute("real_scale").and_then(num)}),
        );
    }
    if let Some(t) = text(root, "creation_date").and_then(|s| num(&s)) {
        extra.insert("creation_date".into(), json!(unix_to_iso8601(t as i64, 0)));
    }
    if let Some(h) = text(root, "history") {
        extra.insert("history".into(), json!(h));
    }
    let mut facts = Vec::new();
    let mut attrs = Map::new();
    for group in ["scan_attributes", "extended_scan_attributes"] {
        let Some(g) = child(root, group) else {
            continue;
        };
        for a in g.children().filter(roxmltree::Node::is_element) {
            let Some(v) = a
                .attribute("value")
                .map(str::trim)
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            let el = a.tag_name().name();
            attrs.insert(el.to_string(), json!(v));
            if let Some((_, ours)) = ATTRIBUTES.iter().find(|(e, _)| *e == el) {
                facts.push((
                    *ours,
                    v.to_string(),
                    format!("ImageHeader/{group}/{el}@value"),
                ));
            }
        }
    }
    extra.insert("scan_attributes".into(), Value::Object(attrs));
    let app = facts
        .iter()
        .find(|f| f.0 == "application")
        .map(|f| f.1.clone());
    let exposure = facts
        .iter()
        .find(|f| f.0 == "exposure_time_s")
        .and_then(|f| num(&f.1));
    for key in ["excitation_source", "emission_filter"] {
        if let Some(f) = facts.iter().find(|f| f.0 == key) {
            extra.insert(key.to_string(), json!(f.1));
        }
    }
    // an emission filter written `<centre>/<width> Filter` gives the emission band
    let band = facts
        .iter()
        .find(|f| f.0 == "emission_filter")
        .and_then(|f| emission_band(&f.1));
    let channel = ChannelInfo {
        index: 0,
        name: app.clone().or_else(|| Some("signal".into())),
        exposure_ms: exposure.map(|s| s * 1000.0),
        emission_nm: band.map(|b| b.0),
        emission_range_nm: band.map(|(c, w)| [c - w / 2.0, c + w / 2.0]),
        ..ChannelInfo::default()
    };
    Ok(Scan {
        name: text(root, "name"),
        width: w,
        height: h,
        data: (data.offset, data.len),
        bytes_per_sample: bps,
        pixel_um,
        channel,
        extra,
        facts,
    })
}

/// `835/50 Filter` → (835, 50) nm.
fn emission_band(filter: &str) -> Option<(f64, f64)> {
    let head = filter.split_whitespace().next()?;
    let (c, w) = head.split_once('/')?;
    let (c, w) = (num(c)?, num(w)?);
    (c > 100.0 && c < 2000.0 && w > 0.0 && w < 1000.0).then_some((c, w))
}

/// The technique an Image Lab application names: blots imaged by chemiluminescence or IRDye
/// fluorescence are western blots; stained gels are gel electrophoresis; anything else is left
/// unnamed.
fn technique_of(application: &str) -> Option<&'static str> {
    let a = application.to_ascii_lowercase();
    if a.contains("chemi") || a.contains("irdye") || a.contains("starbright") {
        Some("OBI:0000854")
    } else if [
        "gel",
        "coomassie",
        "stain",
        "sybr",
        "ethidium",
        "fam",
        "cy5",
        "cy3",
    ]
    .iter()
    .any(|k| a.contains(k))
    {
        Some("CHMO:0001021")
    } else {
        None
    }
}

impl Dataset for ScnDataset {
    fn info(&self) -> Result<FileInfo> {
        let images = self
            .scans
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let s = &s.0;
                let pt = if s.bytes_per_sample == 2 {
                    PixelType::Uint16
                } else {
                    PixelType::Uint8
                };
                let mut img = ImageInfo::new(i as u32, s.width, s.height, pt);
                img.name = s
                    .name
                    .clone()
                    .or_else(|| self.item.get("name").map(|v| v.0.clone()));
                img.physical_size = PhysicalSize::micrometres(s.pixel_um.0, s.pixel_um.1, None);
                img.channels = vec![s.channel.clone()];
                img.extra = s.extra.clone();
                img.finish()
            })
            .collect::<Vec<_>>();
        let mut tables = Vec::new();
        if !self.log.is_empty() {
            let mut uniq: Vec<&str> = Vec::new();
            for (_, who, what) in &self.log {
                for s in [who.as_str(), what.as_str()] {
                    if !uniq.contains(&s) {
                        uniq.push(s);
                    }
                }
            }
            let col =
                |i: u32, name: &str, dtype: &str, unit: Option<&str>, cats: bool| ColumnInfo {
                    index: i,
                    name: name.into(),
                    label: None,
                    dtype: dtype.into(),
                    unit: unit.map(str::to_string),
                    range: None,
                    extra: if cats {
                        BTreeMap::from([("categories".to_string(), json!(uniq))])
                    } else {
                        BTreeMap::new()
                    },
                };
            tables.push(TableInfo {
                index: 0,
                name: Some("log".into()),
                row_count: self.log.len() as u64,
                columns: vec![
                    col(0, "time", "float64", Some("s"), false),
                    col(1, "user", "uint32", None, true),
                    col(2, "entry", "uint32", None, true),
                ],
                extra: BTreeMap::from([(
                    "time_origin".to_string(),
                    json!("Unix time (seconds since 1970-01-01 UTC)"),
                )]),
            });
        }
        let mut notes = Vec::new();
        if self
            .scans
            .iter()
            .any(|s| s.0.extra.get("display_inverted") == Some(&json!(true)))
        {
            notes.push("pixel values are as stored (higher = more signal); Image Lab displays this image inverted (zero_is: white), dark bands on a light background".into());
        }
        if self
            .scans
            .iter()
            .any(|s| s.0.extra.contains_key("gel_scaling"))
        {
            notes.push("imported from a Molecular Dynamics .gel scan: values are the stored (square-root encoded) counts, not linearized".into());
        }
        if let Some(f) = self.findings.iter().find(|f| f.code == "unreadable_scan") {
            notes.push(format!("the file is damaged ({}); run `check`", f.message));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: self.descriptor.clone(),
            format_version: self.format_version.clone(),
            plane_count: images.iter().map(|i| i.plane_count).sum(),
            images,
            tables,
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(self.vendor.clone())
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in ["images", "physical_size", "channels"] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self.entries.clone())
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let s = self.scan(image)?.clone();
        if index != PlaneIndex::default() {
            return Err(Error::Usage(format!(
                "image {image} has one plane (c=0 z=0 t=0)"
            )));
        }
        let n = plane_bytes_checked(FORMAT_ID, s.width, s.height, s.bytes_per_sample as usize)?;
        if n as u64 != s.data.1 {
            return Err(Error::corrupt(FORMAT_ID, "image data size changed"));
        }
        let mut data = vec![0u8; n];
        self.file
            .read_exact_at(s.data.0, &mut data)
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(Plane {
            width: s.width,
            height: s.height,
            pixel_type: if s.bytes_per_sample == 2 {
                PixelType::Uint16
            } else {
                PixelType::Uint8
            },
            samples_per_pixel: 1,
            data,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        if index != 0 || self.log.is_empty() {
            return Err(Error::Usage(format!("table {index} out of range")));
        }
        let mut uniq: Vec<&str> = Vec::new();
        for (_, who, what) in &self.log {
            for s in [who.as_str(), what.as_str()] {
                if !uniq.contains(&s) {
                    uniq.push(s);
                }
            }
        }
        let n = self.log.len() as u64;
        let a = first_row.min(n) as usize;
        let b = first_row.saturating_add(max_rows).min(n) as usize;
        let code = |s: &str| uniq.iter().position(|u| *u == s).unwrap_or(0) as f64;
        let rows = &self.log[a..b];
        Ok(Table {
            table: 0,
            first_row: a as u64,
            columns: vec![
                rows.iter().map(|r| r.0).collect(),
                rows.iter().map(|r| code(&r.1)).collect(),
                rows.iter().map(|r| code(&r.2)).collect(),
            ],
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        r.performed("every image's pixel data read");
        for i in 0..self.scans.len() as u32 {
            if let Err(e) = self.read_plane(i, PlaneIndex::default()) {
                r.push(Finding::error(
                    "unreadable_plane",
                    format!("image {i}: {e}"),
                ));
            }
        }
        Ok(r)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("MIME parts walked (every declared length inside the file), XML headers parsed, image data sizes checked against width × height");
        for f in &self.findings {
            r.push(f.clone());
        }
        Ok(r)
    }

    fn experiment(&self) -> Option<Experiment> {
        let mut exp = Experiment::default();
        let origin = |from: &str| Origin {
            source: Source::Inferred,
            from: from.to_string(),
        };
        let first = self.scans.first().map(|s| &s.0);
        let fact = |key: &str| first.and_then(|s| s.facts.iter().find(|f| f.0 == key));
        let mut ins = ExperimentInstrument {
            vendor: Some("Bio-Rad".into()),
            software: Some("Image Lab".into()),
            ..ExperimentInstrument::default()
        };
        exp.provenance.insert(
            "instrument.vendor".into(),
            origin("biorad-scn (Image Lab file)"),
        );
        exp.provenance
            .insert("instrument.software".into(), origin("MIME-Version header"));
        if let Some(f) = fact("imager") {
            ins.model = Some(f.1.replace('™', "").trim().to_string());
            exp.provenance
                .insert("instrument.model".into(), origin(&f.2));
        }
        if let Some(f) = fact("serial_number") {
            ins.serial = Some(f.1.clone());
            exp.provenance
                .insert("instrument.serial".into(), origin(&f.2));
        }
        if let Some((v, from)) = self.item.get("software_version") {
            ins.software_version = Some(v.clone());
            exp.provenance
                .insert("instrument.software_version".into(), origin(from));
        }
        exp.instrument = Some(ins);
        if let Some((v, from)) = self.item.get("name") {
            exp.sample = Some(Sample {
                name: Some(v.clone()),
                source_field: Some(from.clone()),
                ..Sample::default()
            });
            exp.provenance.insert("sample.name".into(), origin(from));
        }
        let mut method = Method::default();
        for (key, param, unit) in [
            ("exposure_time_s", "exposure_time", Some("s")),
            ("application", "application", None),
            ("excitation_source", "excitation_source", None),
            ("emission_filter", "emission_filter", None),
            ("binning", "binning", None),
        ] {
            if let Some(f) = fact(key) {
                let q = match (unit, num(&f.1)) {
                    (Some(u), Some(v)) => Quantity::number(v, u),
                    _ => Quantity::plain(f.1.clone()),
                };
                method.parameters.insert(param.into(), q);
                exp.provenance
                    .insert(format!("method.parameters.{param}"), origin(&f.2));
            }
        }
        if method != Method::default() {
            exp.method = Some(method);
        }
        let mut acq = Acquisition::default();
        if let Some(f) = fact("image_date") {
            acq.started_at = Some(f.1.clone());
            exp.provenance
                .insert("acquisition.started_at".into(), origin(&f.2));
        }
        if let Some((v, from)) = self.item.get("user") {
            acq.operator = Some(v.clone());
            exp.provenance
                .insert("acquisition.operator".into(), origin(from));
        }
        if let Some((v, from)) = self.item.get("description") {
            acq.comment = Some(v.clone());
            exp.provenance
                .insert("acquisition.comment".into(), origin(from));
        }
        if acq != Acquisition::default() {
            exp.acquisition = Some(acq);
        }
        if let Some(s) = first {
            let app = s.channel.name.clone().unwrap_or_default();
            exp.measurements.push(Measurement {
                kind: MeasurementKind::Image,
                indices: (0..self.scans.len() as u32).collect(),
                what: format!(
                    "gel or blot image ({app}), {} × {} pixels, {}-bit",
                    s.width,
                    s.height,
                    s.bytes_per_sample * 8
                ),
                technique: technique_of(&app).and_then(vocab::term),
                terms: Vec::new(),
                parameters: BTreeMap::new(),
            });
        }
        crate::complete_provenance(&mut exp);
        (!exp.is_empty()).then_some(exp)
    }
}
