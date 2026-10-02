//! Yokogawa CellVoyager measurements (CV7000, CV8000, CQ1): `MeasurementData.mlf` (one record
//! per image) with `MeasurementDetail.mrf` (plate and channel geometry), the measurement setting
//! `.mes` (channel names, filters, lasers, objective) and the plate files `.wpi`/`.wpp`.
//! Layout and vocabulary: `docs/formats/cellvoyager.md`; provenance:
//! `docs/provenance/cellvoyager.md`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use openreadout_core::model::{ChannelInfo, InstrumentInfo, ObjectiveInfo};
use openreadout_core::{Error, Fs, Result};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde_json::{Map, Value, json};

use crate::model::{
    HcsPlate, MAX_PLATE_COLUMNS, MAX_PLATE_ROWS, PlaneFile, PlaneSlot, PlateBuilder, RawPlane,
    number,
};

/// Format id.
pub const CELLVOYAGER_FORMAT_ID: &str = "cellvoyager";

/// Name of the per-image record file.
pub const MEASUREMENT_DATA: &str = "MeasurementData.mlf";
/// Name of the measurement detail file.
pub const MEASUREMENT_DETAIL: &str = "MeasurementDetail.mrf";

/// Largest metadata file (`.mrf`, `.mes`, `.wpi`, `.wpp`) read into memory.
const MAX_SIDE_FILE: u64 = 64 << 20;

/// Does this text look like a CellVoyager measurement file (`.mlf`, `.mrf`, `.wpi`, `.mes`)?
pub fn looks_like_cellvoyager(head: &[u8]) -> bool {
    let s = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    s.contains("yokogawa.co.jp/BTS")
        && (s.contains("MeasurementData")
            || s.contains("MeasurementDetail")
            || s.contains("WellPlate")
            || s.contains("MeasurementSetting"))
}

/// The `MeasurementData.mlf` of a measurement folder.
pub fn find_index(fs: &Fs, dir: &Path) -> Option<PathBuf> {
    let p = dir.join(MEASUREMENT_DATA);
    fs.is_file(&p).then_some(p)
}

fn read_small(fs: &Fs, path: &Path) -> Option<String> {
    let m = fs.metadata(path).ok()?;
    if !m.is_file() || m.len() > MAX_SIDE_FILE {
        return None;
    }
    let b = fs.read(path).ok()?;
    let s = String::from_utf8_lossy(&b).into_owned();
    Some(s.trim_start_matches('\u{feff}').to_string())
}

fn attrs_of(n: roxmltree::Node<'_, '_>) -> BTreeMap<String, String> {
    n.attributes()
        .map(|a| (a.name().to_string(), a.value().to_string()))
        .collect()
}

fn to_json(m: &BTreeMap<String, String>) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

/// `MeasurementDetail.mrf`.
#[derive(Debug, Default)]
struct Detail {
    root: BTreeMap<String, String>,
    plate: BTreeMap<String, String>,
    channels: Vec<BTreeMap<String, String>>,
}

fn parse_detail(text: &str) -> Option<Detail> {
    let doc = roxmltree::Document::parse(text).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "MeasurementDetail" {
        return None;
    }
    let mut d = Detail {
        root: attrs_of(root),
        ..Detail::default()
    };
    for c in root.children().filter(roxmltree::Node::is_element) {
        match c.tag_name().name() {
            "MeasurementSamplePlate" => d.plate = attrs_of(c),
            "MeasurementChannel" => d.channels.push(attrs_of(c)),
            _ => {}
        }
    }
    Some(d)
}

/// `.mes` measurement setting: channels and light sources.
#[derive(Debug, Default)]
struct Setting {
    channels: Vec<(BTreeMap<String, String>, Vec<String>)>,
    lights: BTreeMap<String, BTreeMap<String, String>>,
    timelines: Vec<BTreeMap<String, String>>,
}

fn parse_setting(text: &str) -> Option<Setting> {
    let doc = roxmltree::Document::parse(text).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "MeasurementSetting" {
        return None;
    }
    let mut s = Setting::default();
    for n in root.descendants().filter(roxmltree::Node::is_element) {
        match n.tag_name().name() {
            "Channel"
                if n.parent()
                    .is_some_and(|p| p.tag_name().name() == "ChannelList") =>
            {
                let lights = n
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "LightSourceName")
                    .filter_map(|c| c.text().map(|t| t.trim().to_string()))
                    .collect();
                s.channels.push((attrs_of(n), lights));
            }
            "LightSource" => {
                let a = attrs_of(n);
                if let Some(name) = a.get("Name").cloned() {
                    s.lights.insert(name, a);
                }
            }
            "Timeline" => s.timelines.push(attrs_of(n)),
            _ => {}
        }
    }
    Some(s)
}

fn plate_file(text: &str, root_tag: &str) -> Option<BTreeMap<String, String>> {
    let doc = roxmltree::Document::parse(text).ok()?;
    let r = doc.root_element();
    (r.tag_name().name() == root_tag).then(|| attrs_of(r))
}

/// One `MeasurementRecord`.
#[derive(Debug, Default, Clone)]
struct Rec {
    kind: Option<String>,
    time: Option<String>,
    column: Option<u32>,
    row: Option<u32>,
    timepoint: Option<u32>,
    field: Option<u32>,
    z: Option<u32>,
    ch: Option<u32>,
    x: Option<f64>,
    y: Option<f64>,
    zpos: Option<f64>,
    action: Option<String>,
    z_processing: Option<String>,
    tile: Option<u32>,
    file: String,
}

fn local(name: &str) -> &str {
    match name.rfind(':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn rec_from(e: &BytesStart<'_>) -> Rec {
    let mut r = Rec::default();
    for a in e.attributes().with_checks(false).flatten() {
        let v = a.value.to_string();
        let v = v.trim();
        let u = || v.parse::<u32>().ok();
        match local(a.key.as_ref()) {
            "Type" => r.kind = Some(v.to_string()),
            "Time" => r.time = Some(v.to_string()),
            "Column" => r.column = u(),
            "Row" => r.row = u(),
            "TimePoint" => r.timepoint = u(),
            "FieldIndex" => r.field = u(),
            "ZIndex" => r.z = u(),
            "Ch" => r.ch = u(),
            "X" => r.x = number(v),
            "Y" => r.y = number(v),
            "Z" => r.zpos = number(v),
            "Action" => r.action = Some(v.to_string()),
            "ZImageProcessing" => r.z_processing = Some(v.to_string()),
            "PartialTileIndex" => r.tile = u(),
            _ => {}
        }
    }
    r
}

/// Stream the records of `MeasurementData.mlf`.
fn parse_records(fs: &Fs, path: &Path) -> Result<(Vec<Rec>, u64)> {
    let file = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = file.metadata().map_or(0, |m| m.len());
    let mut reader = Reader::from_reader(BufReader::with_capacity(1 << 16, file));
    reader.config_mut().trim_text(false);
    reader.config_mut().check_end_names = false;
    let mut out = Vec::new();
    let mut cur: Option<Rec> = None;
    let mut buf = Vec::new();
    let mut root_ok = false;
    loop {
        let pos = reader.buffer_position();
        let ev = reader.read_event_into(&mut buf).map_err(|e| {
            Error::corrupt_at(
                CELLVOYAGER_FORMAT_ID,
                pos,
                format!("MeasurementData.mlf XML: {e}"),
            )
        })?;
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let name = local(e.name().as_ref()).to_string();
                if !root_ok {
                    if name != "MeasurementData" {
                        return Err(Error::corrupt(
                            CELLVOYAGER_FORMAT_ID,
                            format!(
                                "{} is not a CellVoyager measurement record file (root <{}>)",
                                path.display(),
                                name
                            ),
                        ));
                    }
                    root_ok = true;
                } else if name == "MeasurementRecord" {
                    if let Some(done) = cur.take() {
                        out.push(done); // an empty record element (`<r .../>`)
                    }
                    cur = Some(rec_from(&e));
                    if out.len() as u64 > crate::model::MAX_PLATE_PLANES {
                        return Err(Error::unsupported(
                            CELLVOYAGER_FORMAT_ID,
                            "a measurement with more than 20 million records",
                            "Plates this large are not supported; report the file.",
                        ));
                    }
                }
            }
            Event::Text(t) => {
                if let Some(r) = cur.as_mut() {
                    let s: &str = t.as_ref();
                    r.file.push_str(s);
                }
            }
            Event::GeneralRef(g) => {
                if let Some(r) = cur.as_mut() {
                    let n: &str = g.as_ref();
                    r.file.push_str(match n {
                        "amp" => "&",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Event::End(e) => {
                if local(e.name().as_ref()) == "MeasurementRecord"
                    && let Some(mut done) = cur.take()
                {
                    done.file = done.file.trim().to_string();
                    out.push(done);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if let Some(done) = cur.take() {
        out.push(done);
    }
    if !root_ok {
        return Err(Error::corrupt(
            CELLVOYAGER_FORMAT_ID,
            format!("{} holds no XML element", path.display()),
        ));
    }
    Ok((out, len))
}

/// `BP445/45` → (445, [422.5, 467.5]).
fn band(filter: &str) -> Option<(f64, [f64; 2])> {
    let f = filter.trim();
    let rest = f.strip_prefix("BP").or_else(|| f.strip_prefix("bp"))?;
    let (c, w) = rest.split_once('/')?;
    let (c, w) = (number(c)?, number(w)?);
    (c > 0.0 && w > 0.0).then_some((c, [c - w / 2.0, c + w / 2.0]))
}

/// `#AARRGGBB` → `#RRGGBB`.
fn colour(v: &str) -> Option<String> {
    let h = v.trim().strip_prefix('#')?;
    match h.len() {
        8 if h.chars().all(|c| c.is_ascii_hexdigit()) => {
            Some(format!("#{}", h[2..].to_ascii_uppercase()))
        }
        6 if h.chars().all(|c| c.is_ascii_hexdigit()) => {
            Some(format!("#{}", h.to_ascii_uppercase()))
        }
        _ => None,
    }
}

fn stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map_or_else(|| name.to_string(), |s| s.to_string_lossy().into_owned())
}

/// Parse a CellVoyager measurement from its `MeasurementData.mlf`.
pub fn parse(fs: &Fs, mlf: &Path) -> Result<HcsPlate> {
    let dir = mlf.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut sidecars = Vec::new();
    let mut side_bytes = 0u64;
    let mut read = |name: &str| -> Option<String> {
        let p = dir.join(name);
        let t = read_small(fs, &p)?;
        side_bytes += t.len() as u64;
        sidecars.push(p);
        Some(t)
    };
    let detail = read(MEASUREMENT_DETAIL).and_then(|t| parse_detail(&t));
    let setting_name = detail
        .as_ref()
        .and_then(|d| d.root.get("MeasurementSettingFileName").cloned());
    let setting = setting_name
        .as_deref()
        .and_then(&mut read)
        .and_then(|t| parse_setting(&t));
    let wpi_name = detail
        .as_ref()
        .and_then(|d| d.plate.get("WellPlateFileName").cloned());
    let wpi = wpi_name
        .as_deref()
        .and_then(&mut read)
        .and_then(|t| plate_file(&t, "WellPlate"));
    let wpp_name = detail
        .as_ref()
        .and_then(|d| d.plate.get("WellPlateProductFileName").cloned());
    let wpp = wpp_name
        .as_deref()
        .and_then(&mut read)
        .and_then(|t| plate_file(&t, "WellPlateProduct"));
    let (recs, mlf_len) = parse_records(fs, mlf)?;

    let mut plate = HcsPlate {
        format_id: CELLVOYAGER_FORMAT_ID,
        source: mlf.to_path_buf(),
        root: dir.clone(),
        sidecars,
        index_bytes: mlf_len + side_bytes,
        ..HcsPlate::default()
    };
    if detail.is_none() {
        plate.notes.push(format!(
            "{MEASUREMENT_DETAIL} is missing or unreadable: plate geometry, pixel size and channel list come from the records alone"
        ));
    }
    let d = detail.unwrap_or_default();
    let dr = |k: &str| d.root.get(k).filter(|v| !v.trim().is_empty()).cloned();
    plate.format_version = dr("Version").map(|v| format!("MeasurementDetail {v}"));
    plate.operator = dr("OperatorName");
    plate.started_at = dr("BeginTime");
    plate.ended_at = dr("EndTime");
    plate.method = setting_name.as_deref().map(stem);
    plate.description = dr("Application");
    plate.id = d
        .plate
        .get("Name")
        .or_else(|| wpi.as_ref().and_then(|w| w.get("Name")))
        .filter(|v| !v.trim().is_empty())
        .cloned();
    plate.name = dr("Title").filter(|t| Some(t) != plate.id.as_ref());
    plate.plate_type = wpp.as_ref().and_then(|w| {
        let name = w.get("Name")?;
        Some(match w.get("Manufacturer") {
            Some(m) if !m.is_empty() => format!("{m} {name}"),
            _ => name.clone(),
        })
    });
    plate.instrument = InstrumentInfo {
        manufacturer: Some("Yokogawa".into()),
        model: dr("TargetSystem"),
        software: None,
        software_version: dr("ReleaseNumber"),
        detector: None,
    };

    // channels: the .mrf list, then any channel only the records name
    let mut ch_ids: Vec<u32> = d
        .channels
        .iter()
        .filter_map(|c| c.get("Ch").and_then(|v| v.trim().parse().ok()))
        .collect();
    let mut from_records: BTreeSet<u32> = BTreeSet::new();
    for r in &recs {
        if let Some(c) = r.ch
            && !ch_ids.contains(&c)
        {
            from_records.insert(c);
        }
    }
    if !from_records.is_empty() {
        if !ch_ids.is_empty() {
            plate.notes.push(format!(
                "the records name channel(s) {from_records:?} that {MEASUREMENT_DETAIL} does not list"
            ));
        }
        ch_ids.extend(from_records);
    }
    if ch_ids.is_empty() {
        return Err(Error::corrupt(
            CELLVOYAGER_FORMAT_ID,
            format!("{} records no channel", mlf.display()),
        ));
    }
    let ch_index: HashMap<u32, usize> = ch_ids.iter().enumerate().map(|(i, &c)| (c, i)).collect();
    let settings: HashMap<u32, &(BTreeMap<String, String>, Vec<String>)> = setting
        .as_ref()
        .map(|s| {
            s.channels
                .iter()
                .filter_map(|c| Some((c.0.get("Ch")?.trim().parse().ok()?, c)))
                .collect()
        })
        .unwrap_or_default();
    let targets: Vec<Option<String>> = ch_ids
        .iter()
        .map(|c| {
            settings
                .get(c)
                .and_then(|s| s.0.get("Target"))
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
        })
        .collect();
    let distinct_targets = {
        let set: BTreeSet<&String> = targets.iter().flatten().collect();
        set.len() == ch_ids.len()
    };
    let actions: BTreeMap<u32, BTreeSet<String>> = recs.iter().fold(BTreeMap::new(), |mut m, r| {
        if let (Some(c), Some(a)) = (r.ch, &r.action) {
            m.entry(c).or_default().insert(a.clone());
        }
        m
    });
    let z_processing: BTreeMap<u32, BTreeSet<String>> =
        recs.iter().fold(BTreeMap::new(), |mut m, r| {
            if let (Some(c), Some(a)) = (r.ch, &r.z_processing) {
                m.entry(c).or_default().insert(a.clone());
            }
            m
        });
    for (i, &c) in ch_ids.iter().enumerate() {
        let geo = d
            .channels
            .iter()
            .find(|m| m.get("Ch").and_then(|v| v.trim().parse::<u32>().ok()) == Some(c));
        let set = settings.get(&c);
        let sa = |k: &str| {
            set.and_then(|s| s.0.get(k))
                .filter(|v| !v.trim().is_empty())
        };
        let filter = sa("Acquisition").cloned();
        let bp = filter.as_deref().and_then(band);
        let lights: Vec<String> = set.map(|s| s.1.clone()).unwrap_or_default();
        let excitation = if lights.len() == 1 {
            setting
                .as_ref()
                .and_then(|s| s.lights.get(&lights[0]))
                .and_then(|l| l.get("WaveLength"))
                .and_then(|w| number(w))
                .filter(|w| *w > 0.0)
        } else {
            None
        };
        let name = if distinct_targets {
            targets[i].clone()
        } else {
            filter.clone().or_else(|| targets[i].clone())
        }
        .or_else(|| Some(format!("Ch{c}")));
        plate.channels.push(ChannelInfo {
            index: i as u32,
            name,
            fluorophore: sa("Fluorophore").cloned(),
            excitation_nm: excitation,
            emission_nm: bp.map(|b| b.0),
            emission_range_nm: bp.map(|b| b.1),
            color: sa("Color").and_then(|v| colour(v)),
            acquisition_mode: sa("Kind").cloned(),
            exposure_ms: sa("ExposureTime")
                .and_then(|v| number(v))
                .filter(|v| *v > 0.0),
            ..ChannelInfo::default()
        });
        let mut ex = BTreeMap::new();
        ex.insert("channel_number".into(), json!(c));
        if let Some(t) = &targets[i] {
            ex.insert("target".into(), json!(t));
        }
        if let Some(f) = &filter {
            ex.insert("emission_filter".into(), json!(f));
        }
        if !lights.is_empty() {
            ex.insert("light_sources".into(), json!(lights));
        }
        for (k, key) in [
            ("method", "Method"),
            ("objective", "Objective"),
            ("binning", "Binning"),
            ("camera_type", "CameraType"),
            ("pinhole_diameter", "PinholeDiameter"),
        ] {
            if let Some(v) = sa(key) {
                ex.insert(k.into(), json!(v));
            }
        }
        if let Some(g) = geo {
            for (k, key) in [
                ("camera_number", "CameraNumber"),
                ("input_bit_depth", "InputBitDepth"),
                ("input_level", "InputLevel"),
                ("shading_correction_source", "ShadingCorrectionSource"),
                ("filter_wheel_position", "FilterWheelPosition"),
                ("filter_position", "FilterPosition"),
            ] {
                if let Some(v) = g.get(key).filter(|v| !v.is_empty()) {
                    ex.insert(k.into(), json!(v));
                }
            }
        }
        if let Some(a) = actions.get(&c) {
            ex.insert("actions".into(), json!(a));
        }
        if let Some(a) = z_processing.get(&c) {
            ex.insert("z_image_processing".into(), json!(a));
        }
        plate.channel_extra.push(ex);
    }
    // geometry from the first channel of the .mrf
    if let Some(g) = d.channels.first() {
        let n = |k: &str| g.get(k).and_then(|v| v.trim().parse::<u32>().ok());
        plate.size_x = n("HorizontalPixels").unwrap_or(0);
        plate.size_y = n("VerticalPixels").unwrap_or(0);
        let f = |k: &str| g.get(k).and_then(|v| number(v)).filter(|v| *v > 0.0);
        plate.pixel_size_um = [f("HorizontalPixelDimension"), f("VerticalPixelDimension")];
        if n("InputBitDepth").is_some_and(|b| b <= 16 && b > 8) {
            plate.pixel_type = Some(openreadout_core::PixelType::Uint16);
            plate.pixel_source = Some(format!("{MEASUREMENT_DETAIL} InputBitDepth"));
        }
        let differing = d.channels.iter().any(|c| {
            c.get("HorizontalPixels") != g.get("HorizontalPixels")
                || c.get("VerticalPixels") != g.get("VerticalPixels")
        });
        if differing {
            plate.notes.push(
                "channels record different image sizes; the first channel's size is used and planes of another size fail to read".into(),
            );
        }
    }
    let mag = settings
        .values()
        .find_map(|s| s.0.get("Magnification"))
        .and_then(|v| number(v));
    let objective = settings
        .values()
        .find_map(|s| s.0.get("Objective"))
        .cloned();
    if mag.is_some() || objective.is_some() {
        plate.objective = Some(ObjectiveInfo {
            model: objective,
            nominal_magnification: mag.filter(|m| *m > 0.0),
            lens_na: None,
            immersion: None,
        });
    }
    let rows = d
        .root
        .get("RowCount")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .or_else(|| {
            wpi.as_ref()
                .and_then(|w| w.get("Rows")?.trim().parse().ok())
        });
    let cols = d
        .root
        .get("ColumnCount")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .or_else(|| {
            wpi.as_ref()
                .and_then(|w| w.get("Columns")?.trim().parse().ok())
        });
    let max_row = recs.iter().filter_map(|r| r.row).max().unwrap_or(0);
    let max_col = recs.iter().filter_map(|r| r.column).max().unwrap_or(0);
    let (rows, cols) = match (rows, cols) {
        (Some(r), Some(c)) if r >= max_row && c >= max_col && r > 0 && c > 0 => (r, c),
        _ => openreadout_core::plate::standard_dimensions(max_row, max_col),
    };
    if rows > MAX_PLATE_ROWS || cols > MAX_PLATE_COLUMNS {
        return Err(Error::corrupt(
            CELLVOYAGER_FORMAT_ID,
            format!("plate geometry {rows} x {cols} is implausible"),
        ));
    }
    plate.rows = rows;
    plate.columns = cols;

    let mut b = PlateBuilder::new(CELLVOYAGER_FORMAT_ID);
    let mut other_types: BTreeMap<String, u64> = BTreeMap::new();
    let mut skipped = 0u64;
    let mut tiles = BTreeSet::new();
    for r in &recs {
        let kind = r.kind.as_deref().unwrap_or("IMG");
        if kind != "IMG" {
            *other_types.entry(kind.to_string()).or_default() += 1;
            continue;
        }
        let (Some(row), Some(col), Some(ch)) = (r.row, r.column, r.ch) else {
            skipped += 1;
            continue;
        };
        if row == 0 || col == 0 {
            skipped += 1;
            continue;
        }
        if let Some(t) = r.tile {
            tiles.insert(t);
        }
        let slot = if r.file.is_empty() {
            PlaneSlot::NotRecorded
        } else {
            PlaneSlot::File(PlaneFile {
                name: r.file.replace('\\', "/"),
                page: 0,
                acquired_at: r.time.clone(),
                position_um: [r.x, r.y, r.zpos],
                exposure_ms: None,
            })
        };
        b.push(RawPlane {
            row: row - 1,
            column: col - 1,
            field: r.field.unwrap_or(1),
            channel: ch_index[&ch],
            z: r.z.unwrap_or(1),
            t: r.timepoint.unwrap_or(1),
            slot,
            field_position_um: [r.x, r.y],
        })?;
    }
    if b.is_empty() {
        return Err(Error::corrupt(
            CELLVOYAGER_FORMAT_ID,
            format!("{} holds no image record", mlf.display()),
        ));
    }
    if skipped > 0 {
        plate.notes.push(format!(
            "{skipped} image records lack a row, column or channel and are left out"
        ));
    }
    for (k, n) in &other_types {
        plate.notes.push(format!(
            "{n} records of type {k} are not images and are left out"
        ));
    }
    if tiles.len() > 1 {
        plate.notes.push(format!(
            "fields are tiled ({} partial tiles); tiles of one field are not stitched and the first record of each plane is used",
            tiles.len()
        ));
    }
    b.finish(&mut plate)?;
    // Z step: the smallest positive Z difference between consecutive Z indices of one field
    // and channel.
    if plate.size_z > 1 {
        let mut zs: BTreeMap<(u32, u32, u32, u32), BTreeMap<u32, f64>> = BTreeMap::new();
        for r in recs.iter().take(100_000) {
            if let (Some(row), Some(col), Some(f), Some(c), Some(z), Some(p)) =
                (r.row, r.column, r.field, r.ch, r.z, r.zpos)
            {
                zs.entry((row, col, f, c)).or_default().insert(z, p);
            }
        }
        plate.z_step_um = zs
            .values()
            .filter(|m| m.len() > 1)
            .flat_map(|m| {
                let v: Vec<f64> = m.values().copied().collect();
                v.windows(2)
                    .map(|w| (w[1] - w[0]).abs())
                    .collect::<Vec<_>>()
            })
            .filter(|d| *d > 0.0)
            .min_by(f64::total_cmp);
    }
    let mut ex = BTreeMap::new();
    for (k, key) in [
        ("title", "Title"),
        ("field_count", "FieldCount"),
        ("z_count", "ZCount"),
        ("time_point_count", "TimePointCount"),
        ("status", "Status"),
    ] {
        if let Some(v) = d.root.get(key).filter(|v| !v.is_empty()) {
            ex.insert(k.into(), json!(v));
        }
    }
    if let Some(w) = &wpp {
        for (k, key) in [
            ("plate_product_id", "ProductID"),
            ("column_pitch_mm", "ColumnPitch"),
            ("row_pitch_mm", "RowPitch"),
            ("well_shape", "WellShape"),
            ("bottom_material", "BottomMaterial"),
        ] {
            if let Some(v) = w.get(key).filter(|v| !v.is_empty()) {
                ex.insert(k.into(), json!(v));
            }
        }
    }
    ex.insert("records".into(), json!(recs.len()));
    plate.plate_extra = ex;
    let mut vendor = Map::new();
    vendor.insert("MeasurementDetail".into(), to_json(&d.root));
    vendor.insert("MeasurementSamplePlate".into(), to_json(&d.plate));
    vendor.insert(
        "MeasurementChannel".into(),
        Value::Array(d.channels.iter().map(to_json).collect()),
    );
    if let Some(s) = &setting {
        vendor.insert(
            "MeasurementSetting.Channel".into(),
            Value::Array(
                s.channels
                    .iter()
                    .map(|(a, l)| {
                        let mut v = to_json(a);
                        if let Value::Object(o) = &mut v {
                            o.insert("LightSourceName".into(), json!(l));
                        }
                        v
                    })
                    .collect(),
            ),
        );
        vendor.insert(
            "MeasurementSetting.LightSource".into(),
            Value::Array(s.lights.values().map(to_json).collect()),
        );
        vendor.insert(
            "MeasurementSetting.Timeline".into(),
            Value::Array(s.timelines.iter().map(to_json).collect()),
        );
    }
    if let Some(w) = &wpi {
        vendor.insert("WellPlate".into(), to_json(w));
    }
    if let Some(w) = &wpp {
        vendor.insert("WellPlateProduct".into(), to_json(w));
    }
    vendor.insert("MeasurementRecord".into(), json!(recs.len()));
    plate.vendor = Value::Object(vendor);
    Ok(plate)
}

/// CellVoyager plane file names: `<plate>_<well>_T<t>F<f>L<l>A<a>Z<z>C<c>.tif`.
pub fn is_plane_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    let Some(stem) = n.strip_suffix(".TIF").or_else(|| n.strip_suffix(".TIFF")) else {
        return false;
    };
    let Some((_, tail)) = stem.rsplit_once('_') else {
        return false;
    };
    tail.starts_with('T') && tail.contains('F') && tail.contains('Z') && tail.contains('C')
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::source::{MemFs, MemSource};
    use std::sync::Arc;

    const MRF: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<bts:MeasurementDetail bts:Version="1.0" bts:OperatorName="op" bts:Title="T" bts:BeginTime="2021-06-14T09:13:26+01:00" bts:EndTime="2021-06-14T12:22:21+01:00" bts:MeasurementSettingFileName="S.mes" bts:ColumnCount="24" bts:RowCount="16" bts:TargetSystem="CV8000" bts:ReleaseNumber="R2" xmlns:bts="http://www.yokogawa.co.jp/BTS/BTSSchema/1.0">
  <bts:MeasurementSamplePlate bts:Name="PL1" bts:WellPlateFileName="PL1.wpi" bts:WellPlateProductFileName="x.wpp" />
  <bts:MeasurementChannel bts:Ch="1" bts:HorizontalPixelDimension="0.65" bts:VerticalPixelDimension="0.65" bts:InputBitDepth="16" bts:HorizontalPixels="4" bts:VerticalPixels="2" />
  <bts:MeasurementChannel bts:Ch="2" bts:HorizontalPixelDimension="0.65" bts:VerticalPixelDimension="0.65" bts:InputBitDepth="16" bts:HorizontalPixels="4" bts:VerticalPixels="2" />
</bts:MeasurementDetail>"#;
    const MES: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<bts:MeasurementSetting xmlns:bts="http://www.yokogawa.co.jp/BTS/BTSSchema/1.0">
  <bts:LightSourceList><bts:LightSource bts:Name="405nm" bts:WaveLength="405" /></bts:LightSourceList>
  <bts:ChannelList>
    <bts:Channel bts:Ch="1" bts:Target="DNA" bts:Objective="20x" bts:Magnification="20" bts:Acquisition="BP445/45" bts:ExposureTime="50" bts:Color="#FF002FFF" bts:Kind="ConfocalFluorescence"><bts:LightSourceName>405nm</bts:LightSourceName></bts:Channel>
    <bts:Channel bts:Ch="2" bts:Target="BF" bts:Acquisition="BP525/50" bts:ExposureTime="20" bts:Kind="Brightfield" />
  </bts:ChannelList>
</bts:MeasurementSetting>"##;
    const MLF: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<bts:MeasurementData bts:Version="1.0" xmlns:bts="http://www.yokogawa.co.jp/BTS/BTSSchema/1.0">
<bts:MeasurementRecord bts:Type="IMG" bts:Time="2021-06-14T09:13:30.1+01:00" bts:Column="1" bts:Row="1" bts:TimePoint="1" bts:FieldIndex="1" bts:ZIndex="1" bts:X="-1.5" bts:Y="2" bts:Z="-8.5" bts:Ch="1">PL1_A01_T0001F001L01A01Z01C01.tif</bts:MeasurementRecord>
<bts:MeasurementRecord bts:Type="IMG" bts:Time="2021-06-14T09:13:31+01:00" bts:Column="1" bts:Row="1" bts:TimePoint="1" bts:FieldIndex="1" bts:ZIndex="1" bts:X="-1.5" bts:Y="2" bts:Z="-15.5" bts:Ch="2">PL1_A01_T0001F001L01A02Z01C02.tif</bts:MeasurementRecord>
<bts:MeasurementRecord bts:Type="IMG" bts:Time="2021-06-14T09:13:32+01:00" bts:Column="1" bts:Row="1" bts:TimePoint="1" bts:FieldIndex="1" bts:ZIndex="2" bts:X="-1.5" bts:Y="2" bts:Z="-10.5" bts:Ch="2">PL1_A01_T0001F001L01A02Z02C02.tif</bts:MeasurementRecord>
<bts:MeasurementRecord bts:Type="ERR" bts:Column="1" bts:Row="1" />
</bts:MeasurementData>"#;

    fn fs() -> Fs {
        let f = |t: &str| -> Arc<dyn openreadout_core::ByteSource> {
            Arc::new(MemSource::new("x", t.as_bytes().to_vec()))
        };
        Fs::new(Arc::new(
            MemFs::new()
                .with("m/MeasurementData.mlf", f(MLF))
                .with("m/MeasurementDetail.mrf", f(MRF))
                .with("m/S.mes", f(MES)),
        ))
    }

    #[test]
    fn parses_records_detail_and_setting() {
        let p = parse(&fs(), Path::new("m/MeasurementData.mlf")).unwrap();
        assert_eq!(p.id.as_deref(), Some("PL1"));
        assert_eq!(p.name.as_deref(), Some("T"));
        assert_eq!(p.method.as_deref(), Some("S"));
        assert_eq!((p.rows, p.columns), (16, 24));
        assert_eq!((p.size_x, p.size_y, p.size_z), (4, 2, 2));
        assert_eq!(p.channels[0].name.as_deref(), Some("DNA"));
        assert_eq!(p.channels[0].excitation_nm, Some(405.0));
        assert_eq!(p.channels[0].emission_range_nm, Some([422.5, 467.5]));
        assert_eq!(p.channels[0].color.as_deref(), Some("#002FFF"));
        assert_eq!(p.channels[1].excitation_nm, None);
        assert_eq!(p.z_step_um, Some(5.0));
        let f = &p.fields[0];
        // channel 1 has no Z 2: never acquired
        assert_eq!(f.planes[p.slot(0, 1, 0)], PlaneSlot::NotAcquired);
        assert_eq!(
            f.planes[p.slot(1, 1, 0)].file_name(),
            Some("PL1_A01_T0001F001L01A02Z02C02.tif")
        );
        assert!(p.notes.iter().any(|n| n.contains("type ERR")));
    }

    #[test]
    fn helpers() {
        assert_eq!(band("BP600/37"), Some((600.0, [581.5, 618.5])));
        assert_eq!(band("LP500"), None);
        assert_eq!(colour("#FFff1b00").as_deref(), Some("#FF1B00"));
        assert!(is_plane_name("1053601756_A01_T0001F001L01A01Z01C01.tif"));
        assert!(!is_plane_name("DC_DCAM#1_CAM3.tif"));
    }

    #[test]
    fn malformed_records_are_clean_errors() {
        for cut in [5, 60, 200, MLF.len() / 2] {
            let f = Fs::new(Arc::new(MemFs::new().with(
                "m/MeasurementData.mlf",
                Arc::new(MemSource::new("x", MLF.as_bytes()[..cut].to_vec())),
            )));
            let _ = parse(&f, Path::new("m/MeasurementData.mlf"));
        }
    }
}
