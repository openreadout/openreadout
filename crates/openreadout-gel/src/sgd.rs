//! Syngene GeneSys `.sgd` (G:Box and PXi imagers): a compound file with one storage per capture,
//! each holding the raw samples (`PixelData`), the capture settings (`Capture`) and the time and
//! user (`Description`). Layout: `docs/formats/syngene-sgd.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::cfb::{Cfb, CfbEntry};
use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Measurement, MeasurementKind, Method, Origin,
    Quantity,
};
use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, FormatDescriptor, ImageInfo, InstrumentInfo,
    LsEntry,
};
use openreadout_core::pixel::{PixelType, Plane, plane_bytes_checked};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::SourceFile;
use openreadout_core::time::unix_to_iso8601;
use openreadout_core::vocab;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

pub(crate) const FORMAT_ID: &str = "syngene-sgd";
/// Storage holding one storage per capture.
const ROOT: &str = "Detector/GeneSnap";
/// The list of capture storages.
const MULTI_DATA: &str = "Detector/MultiImage/MultiData";
/// Bytes of the `PixelData` header.
const PIXEL_HEADER: u64 = 26;
/// Largest settings stream read.
const MAX_META: u64 = 64 << 10;
/// Most captures listed.
const MAX_IMAGES: usize = 4096;

/// Settings from a `Capture` stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Capture {
    pub(crate) software: Option<String>,
    pub(crate) exposure_s: Option<f64>,
    pub(crate) light: Option<String>,
    pub(crate) camera: Option<String>,
    pub(crate) camera_label: Option<String>,
    pub(crate) imager: Option<String>,
    pub(crate) filter: Option<String>,
    pub(crate) setup: Option<String>,
    pub(crate) application: Option<String>,
}

/// One capture (image) of the file.
#[derive(Debug, Clone)]
struct Exposure {
    storage: String,
    width: u32,
    height: u32,
    bytes_per_sample: u32,
    pixel_stream: CfbEntry,
    capture: Capture,
    unix_time: Option<i64>,
    user: Option<String>,
}

/// An opened `.sgd` file.
#[derive(Debug)]
pub struct SgdDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    file: SourceFile,
    file_len: u64,
    cfb: Cfb,
    images: Vec<Exposure>,
    findings: Vec<Finding>,
}

/// A bounded little-endian cursor.
struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    /// A u32 length and that many bytes of text (Latin-1), trimmed (possibly empty).
    fn text(&mut self) -> Option<String> {
        let n = self.u32()?;
        if n > 4096 {
            return None;
        }
        let t: String = self
            .take(n as usize)?
            .iter()
            .map(|&c| char::from(c))
            .collect();
        let t = t.trim_matches(|c: char| c == '\0' || c.is_whitespace());
        Some(t.to_string())
    }
}

/// `None` for an empty text.
fn nonempty(t: String) -> Option<String> {
    (!t.is_empty()).then_some(t)
}

/// Texts written as a u32 length and printable ASCII starting with a letter, found anywhere in
/// `b` (the tail of a `Capture` stream mixes them with numbers).
fn scan_texts(b: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= b.len() {
        let n = u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        if (2..=128).contains(&n)
            && let Some(t) = b.get(i + 4..i + 4 + n)
            && t[0].is_ascii_alphabetic()
            && t.iter().all(|&c| (0x20..0x7f).contains(&c))
        {
            out.push(String::from_utf8_lossy(t).trim().to_string());
            i += 4 + n;
            continue;
        }
        i += 1;
    }
    out
}

/// Parses a `Capture` stream as far as it goes.
pub(crate) fn parse_capture(b: &[u8]) -> Capture {
    let mut c = Capture::default();
    let mut cur = Cur { b, i: 0 };
    if cur.take(11).is_none() {
        return c;
    }
    let Some(sw) = cur.text() else { return c };
    c.software = nonempty(sw);
    let Some(e) = cur.f64() else { return c };
    c.exposure_s = Some(e).filter(|v| v.is_finite() && *v > 0.0 && *v < 1e6);
    if cur.u16().is_none() {
        return c;
    }
    let mut texts: [Option<String>; 4] = Default::default();
    let mut complete = true;
    for slot in &mut texts {
        let Some(t) = cur.text() else {
            complete = false;
            break;
        };
        *slot = nonempty(t);
    }
    let [light, camera, label, imager] = texts;
    (c.light, c.camera, c.camera_label, c.imager) = (light, camera, label, imager);
    if !complete {
        return c;
    }
    if cur.u16().is_none() {
        return c;
    }
    let Some(f) = cur.text() else { return c };
    c.filter = nonempty(f);
    let tail = scan_texts(b.get(cur.i..).unwrap_or_default());
    c.setup = tail.iter().find(|t| t.contains(" : ")).cloned();
    c.application = tail.iter().rev().find(|t| !t.contains(" : ")).cloned();
    c
}

/// `Description`: u16, u32 Unix time, u16, u64 .NET ticks, then the user name.
pub(crate) fn parse_description(b: &[u8]) -> (Option<i64>, Option<String>) {
    let mut cur = Cur { b, i: 0 };
    let Some(_) = cur.u16() else {
        return (None, None);
    };
    let time = cur
        .u32()
        .map(i64::from)
        .filter(|t| (631_152_000..4_102_444_800).contains(t)); // 1990-2100
    let user = (|| {
        cur.u16()?;
        cur.i64()?;
        nonempty(cur.text()?)
    })();
    (time, user)
}

/// `MultiData`: u16, u32 count, then count × (u32 length + storage name).
pub(crate) fn parse_multi_data(b: &[u8]) -> Vec<String> {
    let mut cur = Cur { b, i: 0 };
    let (Some(_), Some(n)) = (cur.u16(), cur.u32()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for _ in 0..(n as usize).min(MAX_IMAGES) {
        match cur.text() {
            Some(t) if !t.is_empty() && !t.contains('/') => out.push(t),
            _ => break,
        }
    }
    out
}

/// `PixelData` header: (width, height, sample code); `Err` says why it is not usable.
pub(crate) fn parse_pixel_header(b: &[u8]) -> std::result::Result<(u32, u32, u32), String> {
    let mut cur = Cur { b, i: 0 };
    let bad = || "PixelData header shorter than 26 bytes".to_string();
    cur.u16().ok_or_else(bad)?;
    let w = cur.u32().ok_or_else(bad)?;
    let h = cur.u32().ok_or_else(bad)?;
    let ones = [
        cur.u32().ok_or_else(bad)?,
        cur.u32().ok_or_else(bad)?,
        cur.u32().ok_or_else(bad)?,
    ];
    let code = cur.u32().ok_or_else(bad)?;
    if w == 0 || h == 0 {
        return Err(format!("PixelData size {w} × {h}"));
    }
    if ones != [1, 1, 1] {
        return Err(format!(
            "PixelData header fields {ones:?} (every file seen has 1, 1, 1: planes or channels this reader does not know)"
        ));
    }
    Ok((w, h, code))
}

/// Storage names in natural order: `RawData`, `RawData0`, `RawData1`, … `RawData10`.
fn natural_key(s: &str) -> (String, u64) {
    let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (stem, num) = s.split_at(s.len() - digits);
    (stem.to_string(), num.parse::<u64>().map_or(0, |n| n + 1))
}

impl SgdDataset {
    pub(crate) fn open(
        descriptor: FormatDescriptor,
        path: PathBuf,
        mut file: SourceFile,
        file_len: u64,
    ) -> Result<SgdDataset> {
        let cfb = Cfb::open(&mut file, &path, FORMAT_ID)?;
        let read_small = |file: &mut SourceFile, name: &str| -> Option<Vec<u8>> {
            let e = cfb.stream(name)?.clone();
            cfb.read(file, &path, &e, MAX_META).ok()
        };
        let mut names = read_small(&mut file, MULTI_DATA)
            .map(|b| parse_multi_data(&b))
            .unwrap_or_default();
        // storages that hold pixels, listed or not
        let prefix = format!("{ROOT}/");
        let mut stored: Vec<String> = cfb
            .entries
            .iter()
            .filter(|e| e.is_stream)
            .filter_map(|e| {
                let rest = e.path.strip_prefix(&prefix)?;
                let (storage, leaf) = rest.split_once('/')?;
                (leaf == "PixelData").then(|| storage.to_string())
            })
            .collect();
        stored.sort_by_key(|s| natural_key(s));
        stored.dedup();
        let mut findings = Vec::new();
        for n in &names {
            if !stored.contains(n) {
                findings.push(Finding::error(
                    "missing_image",
                    format!("MultiData lists {n}, which has no PixelData stream"),
                ));
            }
        }
        names.retain(|n| stored.contains(n));
        for s in stored {
            if !names.contains(&s) {
                names.push(s);
            }
        }
        if names.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "no {ROOT}/<capture>/PixelData stream: not a GeneSys image file, or it holds no image"
                ),
            ));
        }
        let mut images = Vec::new();
        for n in names.into_iter().take(MAX_IMAGES) {
            let base = format!("{ROOT}/{n}");
            let Some(px) = cfb.stream(&format!("{base}/PixelData")).cloned() else {
                continue;
            };
            let head = match read_range(&cfb, &mut file, &path, &px, 0, PIXEL_HEADER.min(px.size)) {
                Ok(h) => h,
                Err(e) => {
                    findings.push(Finding::error("unreadable_image", format!("{n}: {e}")));
                    continue;
                }
            };
            let (w, h, code) = match parse_pixel_header(&head) {
                Ok(v) => v,
                Err(e) => {
                    findings.push(Finding::error("unreadable_image", format!("{n}: {e}")));
                    continue;
                }
            };
            let px_count = u64::from(w) * u64::from(h);
            let data = px.size.saturating_sub(PIXEL_HEADER);
            let bps = if px_count.checked_mul(2) == Some(data) {
                2
            } else if data == px_count {
                1
            } else {
                findings.push(Finding::error(
                    "unreadable_image",
                    format!(
                        "{n}: {data} bytes of samples for {w} × {h} pixels (neither 1 nor 2 bytes per pixel)"
                    ),
                ));
                continue;
            };
            if (bps == 1 && code != 2) || (bps == 2 && code != 4) {
                findings.push(Finding::warning(
                    "unexpected_sample_code",
                    format!(
                        "{n}: sample code {code} with {} bits per sample (files seen pair 2 with 8 bits, 4 with 16)",
                        bps * 8
                    ),
                ));
            }
            let capture = read_small(&mut file, &format!("{base}/Capture"))
                .map(|b| parse_capture(&b))
                .unwrap_or_default();
            let (unix_time, user) = read_small(&mut file, &format!("{base}/Description"))
                .map(|b| parse_description(&b))
                .unwrap_or_default();
            images.push(Exposure {
                storage: n,
                width: w,
                height: h,
                bytes_per_sample: bps,
                pixel_stream: px,
                capture,
                unix_time,
                user,
            });
        }
        if images.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "no capture of the file can be read: {}",
                    findings
                        .first()
                        .map_or("no PixelData stream", |f| f.message.as_str())
                ),
            ));
        }
        if let Some(first) = cfb.problems.first() {
            findings.push(Finding::warning(
                "container",
                format!(
                    "{} compound-file problem(s), the first: {first} (the file looks truncated or damaged)",
                    cfb.problems.len()
                ),
            ));
        }
        Ok(SgdDataset {
            descriptor,
            path,
            file,
            file_len,
            cfb,
            images,
            findings,
        })
    }

    fn image(&self, image: u32) -> Result<&Exposure> {
        self.images.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image {image} out of range (file has {} images)",
                self.images.len()
            ))
        })
    }
}

/// `len` bytes at `offset` of a stream, without reading a large stream whole.
fn read_range(
    cfb: &Cfb,
    file: &mut SourceFile,
    path: &Path,
    e: &CfbEntry,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>> {
    if let Some(sectors) = cfb.sectors(e)? {
        cfb.read_range(file, path, &sectors, e.size, offset, len)
    } else {
        {
            let end = offset
                .checked_add(len)
                .filter(|&x| x <= e.size)
                .ok_or_else(|| {
                    Error::corrupt(FORMAT_ID, format!("stream {} is too short", e.path))
                })?;
            let all = cfb.read(file, path, e, end)?;
            Ok(all
                .get(usize::try_from(offset).unwrap_or(usize::MAX)..)
                .unwrap_or_default()
                .to_vec())
        }
    }
}

/// The technique an application or light names: chemiluminescence (ECL) images western blots,
/// gel stains image electrophoresis gels; a marker or anything else is not named.
fn technique_of(text: &str) -> Option<&'static str> {
    let a = text.to_ascii_lowercase();
    if a.contains("ecl") || a.contains("chemi") {
        Some("OBI:0000854")
    } else if ["gel", "stain", "sybr", "ethidium", "coomassie"]
        .iter()
        .any(|k| a.contains(k))
    {
        Some("CHMO:0001021")
    } else {
        None
    }
}

/// `GeneSys Version: 1.6.1.0` → (`GeneSys`, `1.6.1.0`).
fn software_parts(s: &str) -> (String, Option<String>) {
    match s.split_once("Version:") {
        Some((name, v)) => (name.trim().to_string(), Some(v.trim().to_string())),
        None => (s.trim().to_string(), None),
    }
}

impl Exposure {
    fn pixel_type(&self) -> PixelType {
        if self.bytes_per_sample == 2 {
            PixelType::Uint16
        } else {
            PixelType::Uint8
        }
    }
    fn channel_name(&self) -> Option<String> {
        self.capture
            .application
            .clone()
            .or_else(|| self.capture.light.clone())
    }
}

impl Dataset for SgdDataset {
    fn info(&self) -> Result<FileInfo> {
        let images = self
            .images
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let c = &x.capture;
                let mut img = ImageInfo::new(i as u32, x.width, x.height, x.pixel_type());
                img.name = Some(x.storage.clone());
                img.channels = vec![ChannelInfo {
                    index: 0,
                    name: x.channel_name(),
                    exposure_ms: c.exposure_s.map(|s| s * 1000.0),
                    ..ChannelInfo::default()
                }];
                let (sw, version) = c
                    .software
                    .as_deref()
                    .map(software_parts)
                    .map_or((None, None), |(a, b)| (Some(a), b));
                img.instrument = Some(InstrumentInfo {
                    manufacturer: Some("Syngene".into()),
                    model: c.imager.clone(),
                    software: sw.or_else(|| Some("GeneSys".into())),
                    software_version: version,
                    detector: c.camera.clone(),
                });
                img.acquired_at = x.unix_time.map(|t| unix_to_iso8601(t, 0));
                let mut extra = BTreeMap::new();
                for (k, v) in [
                    ("light", &c.light),
                    ("filter", &c.filter),
                    ("camera_label", &c.camera_label),
                    ("setup", &c.setup),
                    ("application", &c.application),
                    ("user", &x.user),
                ] {
                    if let Some(v) = v {
                        extra.insert(k.to_string(), json!(v));
                    }
                }
                if let Some(t) = c.exposure_s {
                    extra.insert("exposure_s".into(), json!(t));
                }
                extra.insert("storage".into(), json!(format!("{ROOT}/{}", x.storage)));
                img.extra = extra;
                img.finish()
            })
            .collect::<Vec<_>>();
        let mut notes = Vec::new();
        if self.images.len() > 1 {
            notes.push(format!(
                "{} captures of the same field (GeneSys keeps every exposure of a series and the marker image): one image each",
                self.images.len()
            ));
        }
        if let Some(f) = self
            .findings
            .iter()
            .find(|f| f.code == "unreadable_image" || f.code == "missing_image")
        {
            notes.push(format!("the file is damaged ({}); run `check`", f.message));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: self.descriptor.clone(),
            format_version: self.images.iter().find_map(|x| {
                let (name, v) = software_parts(x.capture.software.as_deref()?);
                Some(format!("{name} {}", v?))
            }),
            plane_count: images.iter().map(|i| i.plane_count).sum(),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let captures: Vec<Value> = self
            .images
            .iter()
            .map(|x| {
                let c = &x.capture;
                json!({
                    "storage": x.storage,
                    "software": c.software,
                    "exposure_s": c.exposure_s,
                    "light": c.light,
                    "camera": c.camera,
                    "camera_label": c.camera_label,
                    "imager": c.imager,
                    "filter": c.filter,
                    "setup": c.setup,
                    "application": c.application,
                    "unix_time": x.unix_time,
                    "user": x.user,
                })
            })
            .collect();
        Ok(json!({ "genesys": { "captures": captures } }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "images",
            "images[].channels[].name",
            "images[].channels[].exposure_ms",
            "images[].instrument",
            "images[].acquired_at",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .cfb
            .entries
            .iter()
            .filter(|e| !e.path.is_empty())
            .map(|e| {
                let image = self
                    .images
                    .iter()
                    .position(|x| e.path.starts_with(&format!("{ROOT}/{}/", x.storage)))
                    .map(|i| i as u32);
                LsEntry {
                    kind: if e.is_stream { "stream" } else { "storage" }.into(),
                    name: e.path.clone(),
                    offset: None,
                    size: e.is_stream.then_some(e.size),
                    image,
                    details: Value::Object(Map::new()),
                }
            })
            .collect())
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let x = self.image(image)?.clone();
        if index != PlaneIndex::default() {
            return Err(Error::Usage(format!(
                "image {image} has one plane (c=0 z=0 t=0)"
            )));
        }
        let n = plane_bytes_checked(FORMAT_ID, x.width, x.height, x.bytes_per_sample as usize)?;
        let data = read_range(
            &self.cfb,
            &mut self.file,
            &self.path,
            &x.pixel_stream,
            PIXEL_HEADER,
            n as u64,
        )?;
        if data.len() != n {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("{}: PixelData is shorter than its header says", x.storage),
            ));
        }
        Ok(Plane {
            width: x.width,
            height: x.height,
            pixel_type: x.pixel_type(),
            samples_per_pixel: 1,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        r.performed("every capture's pixel data read through its sector chain");
        for i in 0..self.images.len() as u32 {
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
        r.performed("compound-file structure parsed, capture list compared with the PixelData streams, every PixelData header and stream size checked against width × height");
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
        let first = self.images.first()?;
        let c = &first.capture;
        let mut ins = ExperimentInstrument {
            vendor: Some("Syngene".into()),
            model: c.imager.clone(),
            ..ExperimentInstrument::default()
        };
        exp.provenance.insert(
            "instrument.vendor".into(),
            origin("syngene-sgd (GeneSys file)"),
        );
        if ins.model.is_some() {
            exp.provenance
                .insert("instrument.model".into(), origin("Capture stream, imager"));
        }
        if let Some((sw, v)) = c.software.as_deref().map(software_parts) {
            ins.software = Some(sw);
            ins.software_version = v;
            exp.provenance.insert(
                "instrument.software".into(),
                origin("Capture stream, software"),
            );
        }
        exp.instrument = Some(ins);
        let mut acq = Acquisition::default();
        if let Some(t) = self.images.iter().filter_map(|x| x.unix_time).min() {
            acq.started_at = Some(unix_to_iso8601(t, 0));
            exp.provenance.insert(
                "acquisition.started_at".into(),
                origin("Description stream, Unix time"),
            );
        }
        if let Some(t) = self.images.iter().filter_map(|x| x.unix_time).max()
            && self.images.len() > 1
        {
            acq.ended_at = Some(unix_to_iso8601(t, 0));
            exp.provenance.insert(
                "acquisition.ended_at".into(),
                origin("Description stream, Unix time"),
            );
        }
        if let Some(u) = &first.user {
            acq.operator = Some(u.clone());
            exp.provenance.insert(
                "acquisition.operator".into(),
                origin("Description stream, user"),
            );
        }
        if acq != Acquisition::default() {
            exp.acquisition = Some(acq);
        }
        let mut method = Method::default();
        let exposures: Vec<f64> = self
            .images
            .iter()
            .filter_map(|x| x.capture.exposure_s)
            .collect();
        match exposures.as_slice() {
            [] => {}
            [one] => {
                method
                    .parameters
                    .insert("exposure_time".into(), Quantity::number(*one, "s"));
            }
            many => {
                method
                    .parameters
                    .insert("exposure_times".into(), Quantity::plain(json!(many)));
            }
        }
        for (key, v) in [
            ("application", &c.application),
            ("light", &c.light),
            ("filter", &c.filter),
        ] {
            if let Some(v) = v {
                method
                    .parameters
                    .insert(key.into(), Quantity::plain(v.clone()));
            }
        }
        for k in method.parameters.keys() {
            exp.provenance
                .insert(format!("method.parameters.{k}"), origin("Capture stream"));
        }
        let technique = self
            .images
            .iter()
            .find_map(|x| x.channel_name().as_deref().and_then(technique_of));
        if let Some(t) = technique {
            method.technique = vocab::term(t);
            exp.provenance.insert(
                "method.technique".into(),
                origin("the application the Capture stream records"),
            );
        }
        if method != Method::default() {
            exp.method = Some(method);
        }
        exp.measurements.push(Measurement {
            kind: MeasurementKind::Image,
            indices: (0..self.images.len() as u32).collect(),
            what: format!(
                "gel or blot image{} ({}), {} × {} pixels",
                if self.images.len() > 1 { "s" } else { "" },
                self.images
                    .iter()
                    .filter_map(Exposure::channel_name)
                    .collect::<Vec<_>>()
                    .join(", "),
                first.width,
                first.height
            ),
            technique: technique.and_then(vocab::term),
            terms: Vec::new(),
            parameters: BTreeMap::new(),
        });
        crate::complete_provenance(&mut exp);
        Some(exp)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn text(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    pub(crate) fn capture_bytes() -> Vec<u8> {
        let mut b = vec![0x0f, 0];
        b.extend_from_slice(&[0; 8]);
        b.push(1);
        text(&mut b, "GeneSys Version: 1.6.1.0");
        b.extend_from_slice(&9.5f64.to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        text(&mut b, "No Light");
        text(&mut b, "Synoptics 6.0MP");
        text(&mut b, "ART-694 #321");
        text(&mut b, "G:Box mini 6");
        b.extend_from_slice(&[11, 0]);
        text(&mut b, "No Filter");
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0x40, 0x68, 0x40, 0xc2, 0, 0, 0]);
        text(&mut b, "ECL : No Light : No Filter");
        b.extend_from_slice(&1.5f64.to_le_bytes());
        text(&mut b, "ECL");
        b.extend_from_slice(&[0xff, 0, 0, 0]);
        b
    }

    #[test]
    fn capture_stream() {
        let b = capture_bytes();
        let c = parse_capture(&b);
        assert_eq!(c.software.as_deref(), Some("GeneSys Version: 1.6.1.0"));
        assert_eq!(c.exposure_s, Some(9.5));
        assert_eq!(c.light.as_deref(), Some("No Light"));
        assert_eq!(c.camera.as_deref(), Some("Synoptics 6.0MP"));
        assert_eq!(c.imager.as_deref(), Some("G:Box mini 6"));
        assert_eq!(c.filter.as_deref(), Some("No Filter"));
        assert_eq!(c.setup.as_deref(), Some("ECL : No Light : No Filter"));
        assert_eq!(c.application.as_deref(), Some("ECL"));
        for n in 0..b.len() {
            let _ = parse_capture(&b[..n]);
        }
        assert_eq!(
            software_parts("GeneSys Version: 1.6.1.0"),
            ("GeneSys".into(), Some("1.6.1.0".into()))
        );
    }

    #[test]
    fn small_streams() {
        let mut d = vec![5, 0];
        d.extend_from_slice(&0x6328_8da9u32.to_le_bytes());
        d.extend_from_slice(&[0, 0]);
        d.extend_from_slice(&0x08da_9a55_6b56_f075u64.to_le_bytes());
        text(&mut d, "harimm");
        assert_eq!(
            parse_description(&d),
            (Some(0x6328_8da9), Some("harimm".into()))
        );
        for n in 0..d.len() {
            let _ = parse_description(&d[..n]);
        }
        let mut m = vec![5, 0, 2, 0, 0, 0];
        text(&mut m, "RawData");
        text(&mut m, "RawData0");
        assert_eq!(parse_multi_data(&m), vec!["RawData", "RawData0"]);
        let mut h = vec![2, 0];
        for v in [3u32, 2, 1, 1, 1, 4] {
            h.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(parse_pixel_header(&h), Ok((3, 2, 4)));
        assert!(parse_pixel_header(&h[..20]).is_err());
        let mut names = vec!["RawData10", "RawData", "RawData2"];
        names.sort_by_key(|s| natural_key(s));
        assert_eq!(names, vec!["RawData", "RawData2", "RawData10"]);
        assert_eq!(technique_of("ECL"), Some("OBI:0000854"));
        assert_eq!(technique_of("Visible Marker"), None);
    }
}
