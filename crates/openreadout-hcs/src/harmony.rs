//! Revvity/PerkinElmer Harmony exports (Opera Phenix, Operetta, Operetta CLS, Sonata):
//! `Images/Index.idx.xml` (also `Index.xml` / `Index.ref.xml`) naming one TIFF per plane.
//! Layout and vocabulary: `docs/formats/opera-harmony.md`; provenance:
//! `docs/provenance/opera-harmony.md`.
//!
//! The index is streamed (quick-xml) so a 384-well plate's 47 MB index is parsed in one pass
//! without building a tree; only the plate header, the channel descriptions and one small
//! record per plane are kept.

use std::collections::{BTreeMap, HashMap};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use openreadout_core::model::{ChannelInfo, InstrumentInfo, ObjectiveInfo};
use openreadout_core::{Error, Fs, Result};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde_json::{Map, Value, json};

use crate::model::{
    HcsPlate, MAX_PLATE_COLUMNS, MAX_PLATE_ROWS, PlaneFile, PlaneSlot, PlateBuilder, RawPlane,
    number, to_micrometres,
};

/// Format id.
pub const HARMONY_FORMAT_ID: &str = "opera-harmony";

/// File names of a Harmony (or Columbus) index, in the order a folder is searched.
pub const HARMONY_INDEX_NAMES: [&str; 4] = [
    "Index.idx.xml",
    "Index.xml",
    "Index.ref.xml",
    "ImageIndex.ColumbusIDX.xml",
];

/// Does this text look like a Harmony or Columbus index (root element and namespace)?
pub fn looks_like_harmony(head: &[u8]) -> bool {
    let s = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    s.contains("<EvaluationInputData")
        && (s.contains("PEHH")
            || s.contains("perkinelmer.com/Columbus")
            // Harmony 7 writes a GUID path: `43B2A954-…/HarmonyV7`
            || s.contains("/HarmonyV"))
}

/// The index file of a Harmony folder: the folder itself, or its `Images` sub-folder.
pub fn find_index(fs: &Fs, dir: &Path) -> Option<PathBuf> {
    for d in [dir.to_path_buf(), dir.join("Images")] {
        for n in HARMONY_INDEX_NAMES {
            let p = d.join(n);
            if fs.is_file(&p) {
                return Some(p);
            }
        }
    }
    None
}

/// One `Image` element of the index (a plane).
#[derive(Debug, Clone, Default)]
struct Rec {
    url: Option<String>,
    /// Columbus: `URL@BufferNo`, the page of a multi-page file.
    page: Option<u32>,
    /// Columbus: `ChannelColor` as `#RRGGBB`.
    color: Option<String>,
    state: Option<String>,
    row: Option<u32>,
    col: Option<u32>,
    field: Option<u32>,
    plane: Option<u32>,
    timepoint: Option<u32>,
    channel: Option<u32>,
    flim: Option<u32>,
    channel_name: Option<String>,
    image_type: Option<String>,
    acquisition_type: Option<String>,
    illumination_type: Option<String>,
    channel_type: Option<String>,
    res_x_um: Option<f64>,
    res_y_um: Option<f64>,
    size_x: Option<u32>,
    size_y: Option<u32>,
    binning: Option<String>,
    max_intensity: Option<f64>,
    camera: Option<String>,
    pos_um: [Option<f64>; 3],
    abs_z_um: Option<f64>,
    time_offset_s: Option<f64>,
    abs_time: Option<String>,
    excitation_nm: Option<f64>,
    emission_nm: Option<f64>,
    magnification: Option<f64>,
    na: Option<f64>,
    exposure_s: Option<f64>,
    orientation: Option<String>,
}

/// Plate header fields.
#[derive(Debug, Default)]
struct Header {
    namespace: Option<String>,
    version: Option<String>,
    user: Option<String>,
    instrument: Option<String>,
    plate_id: Option<String>,
    measurement_id: Option<String>,
    start: Option<String>,
    name: Option<String>,
    plate_type: Option<String>,
    rows: Option<u32>,
    columns: Option<u32>,
    wells: Vec<(u32, u32)>,
    flatfield: Vec<(String, String)>,
    maps: u32,
}

fn local(name: &str) -> &str {
    match name.rfind(':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

fn attr(e: &BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|a| local(a.key.as_ref()) == key)
        .map(|a| a.value.to_string())
}

fn unit_value(text: &str, unit: Option<&str>) -> Option<f64> {
    let v = number(text)?;
    match unit {
        None | Some("") => Some(v),
        Some(u) => to_micrometres(v, u),
    }
}

fn seconds(text: &str, unit: Option<&str>) -> Option<f64> {
    let v = number(text)?;
    match unit.unwrap_or("s") {
        "s" | "" => Some(v),
        "ms" => Some(v / 1000.0),
        "us" | "µs" => Some(v / 1e6),
        _ => None,
    }
}

fn int(text: &str) -> Option<u32> {
    text.trim().parse().ok()
}

/// Parser state while streaming the index.
#[derive(Default)]
struct State {
    hdr: Header,
    recs: Vec<Rec>,
    stack: Vec<String>,
    text: String,
    unit: Option<String>,
    cur: Option<Rec>,
    map_channel: Option<String>,
    /// The channel description of the `Maps/Map/Entry` being read (Harmony 6/7 keep it there
    /// instead of in every `Image`).
    map_rec: Option<Rec>,
    /// Channel descriptions from `Maps`, by ChannelID.
    map_channels: BTreeMap<u32, Rec>,
    root_seen: bool,
}

impl State {
    fn start(&mut self, e: &BytesStart<'_>, empty: bool, pos: u64, path: &Path) -> Result<()> {
        let name = local(e.name().as_ref()).to_string();
        if !self.root_seen {
            self.root_seen = true;
            if name != "EvaluationInputData" {
                return Err(Error::corrupt(
                    HARMONY_FORMAT_ID,
                    format!(
                        "{} is not a Harmony index (root element <{}>)",
                        path.display(),
                        name
                    ),
                ));
            }
            self.hdr.version = attr(e, "Version");
            self.hdr.namespace = e
                .attributes()
                .with_checks(false)
                .flatten()
                .find(|a| {
                    let k: &str = a.key.as_ref();
                    k == "xmlns"
                })
                .map(|a| a.value.to_string());
        }
        self.text.clear();
        self.unit = attr(e, "Unit");
        let parent = self.stack.last().map(String::as_str);
        match (parent, name.as_str()) {
            (Some("Images"), "Image") => self.cur = Some(Rec::default()),
            (Some("Image"), "URL") => {
                if let Some(r) = self.cur.as_mut() {
                    r.page = attr(e, "BufferNo").and_then(|b| b.trim().parse().ok());
                }
            }
            (Some("Plate"), "Well") => {
                if let Some(w) = attr(e, "id").as_deref().and_then(well_id) {
                    self.hdr.wells.push(w);
                }
            }
            (Some("Map"), "Entry") => {
                self.map_channel = attr(e, "ChannelID");
                self.map_rec = Some(Rec::default());
            }
            (Some("Maps"), "Map") => self.hdr.maps += 1,
            _ => {}
        }
        if self.stack.len() >= 32 {
            return Err(Error::corrupt_at(
                HARMONY_FORMAT_ID,
                pos,
                "index XML nested too deeply",
            ));
        }
        self.stack.push(name);
        if empty {
            // `<URL />` is an element with empty text: end it now.
            return self.end();
        }
        Ok(())
    }

    fn end(&mut self) -> Result<()> {
        let Some(name) = self.stack.pop() else {
            return Ok(());
        };
        let parent = self.stack.last().map(String::as_str);
        let v = self.text.trim().to_string();
        if let Some(r) = self.cur.as_mut() {
            if name.as_str() == "Image" && parent == Some("Images") {
                if let Some(done) = self.cur.take() {
                    self.recs.push(done);
                    if self.recs.len() as u64 > crate::model::MAX_PLATE_PLANES {
                        return Err(Error::unsupported(
                            HARMONY_FORMAT_ID,
                            "an index with more than 20 million planes",
                            "Plates this large are not supported; report the file.",
                        ));
                    }
                }
            } else if parent == Some("Image") {
                image_field(r, &name, &v, self.unit.as_deref());
            }
        } else if name.as_str() == "Entry" && parent == Some("Map") {
            // merge this Map's description of the channel with other Maps'
            if let (Some(done), Some(id)) = (
                self.map_rec.take(),
                self.map_channel.as_deref().and_then(int),
            ) {
                let slot = self.map_channels.entry(id).or_default();
                fill(slot, &done);
            }
        } else if parent == Some("Entry")
            && let Some(r) = self.map_rec.as_mut()
            && name.as_str() != "FlatfieldProfile"
        {
            image_field(r, &name, &v, self.unit.as_deref());
        } else {
            let h = &mut self.hdr;
            match (parent, name.as_str()) {
                (Some("EvaluationInputData"), "User") => h.user = some(v),
                (Some("EvaluationInputData"), "InstrumentType") => h.instrument = some(v),
                (Some("Plate"), "PlateID") => h.plate_id = some(v),
                (Some("Plate"), "MeasurementID") => h.measurement_id = some(v),
                (Some("Plate"), "MeasurementStartTime") => h.start = some(v),
                (Some("Plate"), "Name") => h.name = some(v),
                (Some("Plate"), "PlateTypeName") => h.plate_type = some(v),
                (Some("Plate"), "PlateRows") => h.rows = int(&v),
                (Some("Plate"), "PlateColumns") => h.columns = int(&v),
                (Some("Entry"), "FlatfieldProfile") => {
                    if let Some(c) = &self.map_channel {
                        h.flatfield.push((c.clone(), v));
                    }
                }
                _ => {}
            }
        }
        self.text.clear();
        Ok(())
    }
}

/// Parse a Harmony index read from `fs` at `path`.
pub fn parse(fs: &Fs, path: &Path) -> Result<HcsPlate> {
    let file = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = file.metadata().map_or(0, |m| m.len());
    let mut reader = Reader::from_reader(BufReader::with_capacity(1 << 16, file));
    {
        let c = reader.config_mut();
        c.trim_text(false);
        c.check_end_names = false;
    }
    let mut st = State::default();
    let mut buf = Vec::new();
    loop {
        let pos = reader.buffer_position();
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::corrupt_at(HARMONY_FORMAT_ID, pos, format!("index XML: {e}")))?;
        match ev {
            Event::Start(e) => st.start(&e, false, pos, path)?,
            Event::Empty(e) => st.start(&e, true, pos, path)?,
            Event::Text(t) => {
                let s: &str = t.as_ref();
                st.text.push_str(s);
            }
            Event::CData(t) => {
                let s: &str = t.as_ref();
                st.text.push_str(s);
            }
            Event::GeneralRef(r) => {
                let n: &str = r.as_ref();
                st.text.push_str(match n {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    _ => "",
                });
            }
            Event::End(_) => st.end()?,
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    if !st.root_seen {
        return Err(Error::corrupt(
            HARMONY_FORMAT_ID,
            format!("{} holds no XML element", path.display()),
        ));
    }
    build(path, len, st.hdr, st.recs, &st.map_channels)
}

/// Copy into `r` every channel field it lacks from `from` (a `Maps` entry).
fn fill(r: &mut Rec, from: &Rec) {
    macro_rules! take {
        ($($f:ident),*) => {$(
            if r.$f.is_none() {
                r.$f.clone_from(&from.$f);
            }
        )*};
    }
    take!(
        channel_name,
        image_type,
        acquisition_type,
        illumination_type,
        channel_type,
        res_x_um,
        res_y_um,
        size_x,
        size_y,
        binning,
        max_intensity,
        camera,
        excitation_nm,
        emission_nm,
        magnification,
        na,
        exposure_s,
        orientation
    );
}

fn some(v: String) -> Option<String> {
    (!v.is_empty()).then_some(v)
}

/// `RRCC` (two digits each) → zero-based (row, column).
fn well_id(id: &str) -> Option<(u32, u32)> {
    let id = id.trim();
    if id.len() != 4 || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let r: u32 = id[..2].parse().ok()?;
    let c: u32 = id[2..].parse().ok()?;
    (r >= 1 && c >= 1).then(|| (r - 1, c - 1))
}

fn image_field(r: &mut Rec, name: &str, v: &str, unit: Option<&str>) {
    let s = || some(v.to_string());
    match name {
        "URL" => r.url = Some(v.to_string()),
        "State" => r.state = s(),
        "Row" => r.row = int(v),
        "Col" => r.col = int(v),
        "FieldID" => r.field = int(v),
        "PlaneID" => r.plane = int(v),
        "TimepointID" => r.timepoint = int(v),
        "ChannelID" => r.channel = int(v),
        "FlimID" => r.flim = int(v),
        "ChannelName" => r.channel_name = s(),
        "ChannelColor" => {
            r.color = v
                .trim()
                .parse::<u32>()
                .ok()
                .map(|c| format!("#{:06X}", c & 0x00FF_FFFF));
        }
        "ImageType" => r.image_type = s(),
        "AcquisitionType" => r.acquisition_type = s(),
        "IlluminationType" => r.illumination_type = s(),
        "ChannelType" => r.channel_type = s(),
        "ImageResolutionX" => r.res_x_um = unit_value(v, unit),
        "ImageResolutionY" => r.res_y_um = unit_value(v, unit),
        "ImageSizeX" => r.size_x = int(v),
        "ImageSizeY" => r.size_y = int(v),
        "BinningX" => r.binning = s(),
        "MaxIntensity" => r.max_intensity = number(v),
        "CameraType" => r.camera = s(),
        "PositionX" => r.pos_um[0] = unit_value(v, unit),
        "PositionY" => r.pos_um[1] = unit_value(v, unit),
        "PositionZ" => r.pos_um[2] = unit_value(v, unit),
        "AbsPositionZ" => r.abs_z_um = unit_value(v, unit),
        "MeasurementTimeOffset" => r.time_offset_s = seconds(v, unit),
        "AbsTime" => r.abs_time = s(),
        "MainExcitationWavelength" => r.excitation_nm = number(v),
        "MainEmissionWavelength" => r.emission_nm = number(v),
        "ObjectiveMagnification" => r.magnification = number(v),
        "ObjectiveNA" => r.na = number(v),
        "ExposureTime" => r.exposure_s = seconds(v, unit),
        "OrientationMatrix" => r.orientation = s(),
        _ => {}
    }
}

/// Keep only a file name relative to the index folder: Harmony writes plain names; an absolute
/// path or URL keeps its last component.
fn relative_name(url: &str) -> String {
    let u = url.trim();
    if u.contains("://") || u.starts_with('/') || u.contains(":\\") {
        u.rsplit(['/', '\\']).next().unwrap_or(u).to_string()
    } else {
        u.replace('\\', "/")
    }
}

fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x > 0.0)
}

fn build(
    path: &Path,
    len: u64,
    hdr: Header,
    mut recs: Vec<Rec>,
    map_channels: &BTreeMap<u32, Rec>,
) -> Result<HcsPlate> {
    if recs.is_empty() {
        return Err(Error::corrupt(
            HARMONY_FORMAT_ID,
            format!(
                "{} lists no images (no Images/Image element)",
                path.display()
            ),
        ));
    }
    let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut plate = HcsPlate {
        format_id: HARMONY_FORMAT_ID,
        source: path.to_path_buf(),
        root,
        index_bytes: len,
        ..HcsPlate::default()
    };
    let generation = hdr
        .namespace
        .as_deref()
        .and_then(|n| n.rsplit('/').next())
        .filter(|g| g.starts_with("Harmony") || *g == "Columbus")
        .map(str::to_string);
    let columbus = generation.as_deref() == Some("Columbus");
    // Harmony 6/7: channel descriptions in `Maps`, not in each `Image`
    if !map_channels.is_empty() {
        for r in &mut recs {
            if let Some(m) = r.channel.and_then(|c| map_channels.get(&c)) {
                if r.exposure_s.is_none() {
                    r.exposure_s = m.exposure_s;
                }
                if r.size_x.is_none() {
                    r.size_x = m.size_x;
                    r.size_y = m.size_y;
                }
            }
        }
        plate.notes.push(format!(
            "channel descriptions (name, pixel size, wavelengths, objective, exposure) read from the index's Maps section ({} channels)",
            map_channels.len()
        ));
    }
    plate.format_version = generation.clone();
    plate.id = hdr.plate_id.clone();
    plate.name = hdr
        .name
        .clone()
        .filter(|n| Some(n) != hdr.plate_id.as_ref());
    plate.plate_type = hdr.plate_type.clone();
    plate.operator = hdr.user.clone();
    plate.started_at = hdr.start.clone();
    plate.declared_wells = hdr.wells.clone();

    // channels: every distinct ChannelID, described by its first record
    let mut chan_ids: Vec<u32> = recs.iter().filter_map(|r| r.channel).collect();
    chan_ids.sort_unstable();
    chan_ids.dedup();
    if chan_ids.is_empty() {
        chan_ids.push(0);
    }
    let chan_index: HashMap<u32, usize> =
        chan_ids.iter().enumerate().map(|(i, &c)| (c, i)).collect();
    let mut first: Vec<Option<&Rec>> = vec![None; chan_ids.len()];
    for r in &recs {
        let i = chan_index[&r.channel.unwrap_or(chan_ids[0])];
        if first[i].is_none() {
            first[i] = Some(r);
        }
    }
    let described: Vec<Rec> = first
        .iter()
        .map(|f| {
            let mut r = (*f).cloned().unwrap_or_default();
            if let Some(m) = r.channel.and_then(|c| map_channels.get(&c)) {
                fill(&mut r, m);
            }
            r
        })
        .collect();
    for (i, r) in described.iter().enumerate() {
        plate.channels.push(ChannelInfo {
            index: i as u32,
            name: r.channel_name.clone(),
            fluorophore: None,
            excitation_nm: positive(r.excitation_nm),
            emission_nm: positive(r.emission_nm),
            emission_range_nm: None,
            color: r.color.clone(),
            acquisition_mode: r.channel_type.clone(),
            exposure_ms: positive(r.exposure_s).map(|s| s * 1000.0),
            ..ChannelInfo::default()
        });
        let mut ex = BTreeMap::new();
        ex.insert("channel_id".into(), json!(chan_ids[i]));
        for (k, v) in [
            ("acquisition_type", &r.acquisition_type),
            ("illumination_type", &r.illumination_type),
            ("image_type", &r.image_type),
            ("binning", &r.binning),
            ("camera", &r.camera),
        ] {
            if let Some(v) = v {
                ex.insert(k.into(), json!(v));
            }
        }
        if let Some(m) = r.max_intensity {
            ex.insert("max_intensity".into(), json!(m));
        }
        plate.channel_extra.push(ex);
    }
    let r0 = described
        .iter()
        .find(|r| r.size_x.is_some())
        .cloned()
        .unwrap_or_default();
    plate.size_x = r0.size_x.unwrap_or(0);
    plate.size_y = r0.size_y.unwrap_or(0);
    plate.pixel_size_um = [positive(r0.res_x_um), positive(r0.res_y_um)];
    if r0.magnification.is_some() || r0.na.is_some() {
        plate.objective = Some(ObjectiveInfo {
            model: None,
            nominal_magnification: positive(r0.magnification),
            lens_na: positive(r0.na),
            immersion: None,
        });
    }
    plate.instrument = InstrumentInfo {
        manufacturer: hdr
            .namespace
            .as_deref()
            .filter(|n| n.contains("perkinelmer"))
            .map(|_| "PerkinElmer".to_string()),
        model: hdr.instrument.clone(),
        software: Some(if columbus { "Columbus" } else { "Harmony" }.into()),
        software_version: generation
            .as_deref()
            .and_then(|g| g.strip_prefix("HarmonyV"))
            .map(str::to_string),
        detector: r0.camera.clone(),
    };
    let rows = hdr.rows.unwrap_or(0);
    let cols = hdr.columns.unwrap_or(0);
    let max_row = recs.iter().filter_map(|r| r.row).max().unwrap_or(0);
    let max_col = recs.iter().filter_map(|r| r.col).max().unwrap_or(0);
    let (rows, cols) = if rows >= max_row && cols >= max_col && rows > 0 && cols > 0 {
        (rows, cols)
    } else {
        openreadout_core::plate::standard_dimensions(max_row, max_col)
    };
    if rows > MAX_PLATE_ROWS || cols > MAX_PLATE_COLUMNS {
        return Err(Error::corrupt(
            HARMONY_FORMAT_ID,
            format!("plate geometry {rows} x {cols} is implausible"),
        ));
    }
    plate.rows = rows;
    plate.columns = cols;

    let mut latest: Option<String> = None;
    let mut b = PlateBuilder::new(HARMONY_FORMAT_ID);
    let mut flims = std::collections::BTreeSet::new();
    let mut skipped = 0u64;
    for r in &recs {
        let (Some(row), Some(col)) = (r.row, r.col) else {
            skipped += 1;
            continue;
        };
        if row == 0 || col == 0 {
            skipped += 1;
            continue;
        }
        if let Some(f) = r.flim {
            flims.insert(f);
        }
        if let Some(t) = &r.abs_time
            && latest.as_ref().is_none_or(|l| crate::model::earlier(l, t))
        {
            latest = Some(t.clone());
        }
        let slot = match r.url.as_deref().map(str::trim) {
            None | Some("") => PlaneSlot::NotRecorded,
            Some(u) => PlaneSlot::File(PlaneFile {
                name: relative_name(u),
                page: r.page.unwrap_or(0),
                acquired_at: r.abs_time.clone(),
                position_um: [r.pos_um[0], r.pos_um[1], r.pos_um[2]],
                exposure_ms: r.exposure_s.map(|s| s * 1000.0),
            }),
        };
        b.push(RawPlane {
            row: row - 1,
            column: col - 1,
            field: r.field.unwrap_or(1),
            channel: chan_index[&r.channel.unwrap_or(chan_ids[0])],
            z: r.plane.unwrap_or(1),
            t: r.timepoint.unwrap_or(0),
            slot,
            field_position_um: [r.pos_um[0], r.pos_um[1]],
        })?;
    }
    if skipped > 0 {
        plate.notes.push(format!(
            "{skipped} Image records have no Row/Col and are left out"
        ));
    }
    if flims.len() > 1 {
        plate.notes.push(format!(
            "the index has {} FlimID values; planes that differ only in FlimID are not told apart (the first is used)",
            flims.len()
        ));
    }
    b.finish(&mut plate)?;
    plate.ended_at = latest;
    // Z step: PositionZ of the first two planes (PlaneID order) of the first record's field and
    // channel, whether or not their images were recorded
    if plate.size_z > 1
        && let Some(r0) = recs.first()
    {
        let key = (r0.row, r0.col, r0.field, r0.channel, r0.timepoint);
        let mut zs: Vec<(u32, f64)> = recs
            .iter()
            .filter(|r| (r.row, r.col, r.field, r.channel, r.timepoint) == key)
            .filter_map(|r| Some((r.plane?, r.pos_um[2]?)))
            .collect();
        zs.sort_by_key(|z| z.0);
        zs.dedup_by_key(|z| z.0);
        if let [a, b, ..] = zs.as_slice() {
            plate.z_step_um = positive(Some((b.1 - a.1).abs()));
        }
    }
    if plate.size_t > 1 {
        let mut offs: Vec<f64> = recs
            .iter()
            .filter(|r| r.field == plate.fields.first().map(|f| f.field))
            .filter_map(|r| r.time_offset_s)
            .collect();
        offs.sort_by(f64::total_cmp);
        offs.dedup();
        if offs.len() >= 2 {
            plate.time_increment_s = positive(Some(offs[1] - offs[0]));
        }
    }
    plate.plate_extra = {
        let mut m = BTreeMap::new();
        if let Some(v) = &hdr.measurement_id {
            m.insert("measurement_id".into(), json!(v));
        }
        m.insert("records".into(), json!(recs.len()));
        m
    };
    plate.vendor = vendor_json(&hdr, &described, !map_channels.is_empty());
    Ok(plate)
}

fn vendor_json(hdr: &Header, first: &[Rec], from_maps: bool) -> Value {
    let mut m = Map::new();
    let mut put = |k: &str, v: &Option<String>| {
        if let Some(v) = v {
            m.insert(k.into(), json!(v));
        }
    };
    put("xmlns", &hdr.namespace);
    put("Version", &hdr.version);
    put("User", &hdr.user);
    put("InstrumentType", &hdr.instrument);
    let mut plate = Map::new();
    for (k, v) in [
        ("PlateID", &hdr.plate_id),
        ("MeasurementID", &hdr.measurement_id),
        ("MeasurementStartTime", &hdr.start),
        ("Name", &hdr.name),
        ("PlateTypeName", &hdr.plate_type),
    ] {
        if let Some(v) = v {
            plate.insert(k.into(), json!(v));
        }
    }
    if let Some(r) = hdr.rows {
        plate.insert("PlateRows".into(), json!(r));
    }
    if let Some(c) = hdr.columns {
        plate.insert("PlateColumns".into(), json!(c));
    }
    plate.insert("Well".into(), json!(hdr.wells.len()));
    m.insert("Plate".into(), Value::Object(plate));
    let chans: Vec<Value> = first
        .iter()
        .map(|r| {
            json!({
                "ChannelID": r.channel, "ChannelName": r.channel_name, "ImageType": r.image_type,
                "AcquisitionType": r.acquisition_type, "IlluminationType": r.illumination_type,
                "ChannelType": r.channel_type, "ImageResolutionX_um": r.res_x_um,
                "ImageResolutionY_um": r.res_y_um, "ImageSizeX": r.size_x, "ImageSizeY": r.size_y,
                "BinningX": r.binning, "MaxIntensity": r.max_intensity, "CameraType": r.camera,
                "MainExcitationWavelength": r.excitation_nm, "MainEmissionWavelength": r.emission_nm,
                "ObjectiveMagnification": r.magnification, "ObjectiveNA": r.na,
                "ExposureTime_s": r.exposure_s, "OrientationMatrix": r.orientation,
            })
        })
        .collect();
    m.insert(
        if from_maps {
            "channels (first Image record of each ChannelID, completed from Maps)"
        } else {
            "channels (first Image record of each ChannelID)"
        }
        .into(),
        Value::Array(chans),
    );
    if !hdr.flatfield.is_empty() {
        let ff: Map<String, Value> = hdr
            .flatfield
            .iter()
            .map(|(c, v)| (c.clone(), json!(v)))
            .collect();
        m.insert("FlatfieldProfile".into(), Value::Object(ff));
    }
    m.insert("Map".into(), json!(hdr.maps));
    Value::Object(m)
}

/// Harmony plane file names: `r<RR>c<CC>f<FF>p<PP>-ch<C>sk<K>fk<F>fl<L>.tif[f]`.
pub fn is_plane_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    crate::model::has_extension(&n, &["tiff", "tif"])
        && n.starts_with('r')
        && n.contains("-ch")
        && n.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::source::MemFs;
    use openreadout_core::source::MemSource;
    use std::sync::Arc;

    const INDEX: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<EvaluationInputData Version="1" xmlns="http://www.perkinelmer.com/PEHH/HarmonyV5">
  <User>someone</User>
  <InstrumentType>Phenix</InstrumentType>
  <Plates><Plate><PlateID>P1</PlateID><Name>P1</Name><PlateTypeName>96 x</PlateTypeName>
    <PlateRows>8</PlateRows><PlateColumns>12</PlateColumns><Well id="0102" /></Plate></Plates>
  <Wells><Well><id>0102</id><Row>1</Row><Col>2</Col><Image id="x" /></Well></Wells>
  <Images>
    <Image Version="1"><id>a</id><State>Ok</State><URL>r01c02f01p01-ch1sk1fk1fl1.tiff</URL>
      <Row>1</Row><Col>2</Col><FieldID>1</FieldID><PlaneID>1</PlaneID><TimepointID>0</TimepointID>
      <ChannelID>1</ChannelID><ChannelName>DAPI &amp; co</ChannelName>
      <ImageResolutionX Unit="m">6.5E-07</ImageResolutionX><ImageResolutionY Unit="m">6.5E-07</ImageResolutionY>
      <ImageSizeX>4</ImageSizeX><ImageSizeY>2</ImageSizeY><PositionZ Unit="m">1E-06</PositionZ>
      <AbsTime>2021-01-01T10:00:00+00:00</AbsTime><ExposureTime Unit="s">0.02</ExposureTime>
      <MainExcitationWavelength Unit="nm">375</MainExcitationWavelength><MainEmissionWavelength Unit="nm">0</MainEmissionWavelength>
      <ObjectiveMagnification Unit="">20</ObjectiveMagnification><ObjectiveNA Unit="">1</ObjectiveNA></Image>
    <Image Version="1"><id>b</id><State>Ok</State><URL></URL>
      <Row>1</Row><Col>2</Col><FieldID>1</FieldID><PlaneID>2</PlaneID><TimepointID>0</TimepointID>
      <ChannelID>1</ChannelID><PositionZ Unit="m">3E-06</PositionZ></Image>
  </Images>
</EvaluationInputData>"#;

    fn fs_with(text: &str) -> Fs {
        Fs::new(Arc::new(MemFs::new().with(
            "p/Index.idx.xml",
            Arc::new(MemSource::new("i", text.as_bytes().to_vec())),
        )))
    }

    #[test]
    fn parses_a_small_index() {
        let fs = fs_with(INDEX);
        let p = parse(&fs, Path::new("p/Index.idx.xml")).unwrap();
        assert_eq!(p.format_version.as_deref(), Some("HarmonyV5"));
        assert_eq!(p.id.as_deref(), Some("P1"));
        assert_eq!(p.name, None);
        assert_eq!((p.rows, p.columns), (8, 12));
        assert_eq!(p.declared_wells, vec![(0, 1)]);
        assert_eq!(p.channels.len(), 1);
        assert_eq!(p.channels[0].name.as_deref(), Some("DAPI & co"));
        assert_eq!(p.channels[0].emission_nm, None);
        assert_eq!(p.channels[0].exposure_ms, Some(20.0));
        assert_eq!((p.size_x, p.size_y, p.size_z), (4, 2, 2));
        assert!((p.pixel_size_um[0].unwrap() - 0.65).abs() < 1e-12);
        assert!((p.z_step_um.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(p.fields.len(), 1);
        assert!(matches!(p.fields[0].planes[1], PlaneSlot::NotRecorded));
        assert_eq!(p.instrument.model.as_deref(), Some("Phenix"));
        assert_eq!(p.instrument.software_version.as_deref(), Some("5"));
    }

    #[test]
    fn rejects_other_xml_and_truncation() {
        let fs = fs_with("<?xml version=\"1.0\"?><Other/>");
        assert_eq!(
            parse(&fs, Path::new("p/Index.idx.xml"))
                .unwrap_err()
                .exit_code(),
            4
        );
        for cut in [10, 200, 600, INDEX.len() - 30] {
            let fs = fs_with(&INDEX[..cut]);
            let _ = parse(&fs, Path::new("p/Index.idx.xml")); // no panic
        }
    }

    /// Harmony 6/7: `Image` records without channel fields; the descriptions in `Maps` (not
    /// necessarily the first Map), and a GUID-path namespace (Harmony 7).
    const INDEX_V7: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<EvaluationInputData Version="2" xmlns="43B2A954-E3C3-47E1-B392-6635266B0DD3/HarmonyV7">
  <User>u</User><InstrumentType>Phenix</InstrumentType>
  <Plates><Plate><PlateID>P7</PlateID><PlateRows>16</PlateRows><PlateColumns>24</PlateColumns><Well id="0303" /></Plate></Plates>
  <Maps>
    <Map><Entry ChannelID="2"><FlatfieldProfile>{a}</FlatfieldProfile></Entry><Entry ChannelID="1"><FlatfieldProfile>{b}</FlatfieldProfile></Entry></Map>
    <Map><Entry ChannelID="1"><SkewcropParameters>x</SkewcropParameters></Entry></Map>
    <Map>
      <Entry ChannelID="1"><ChannelName>Hoechst 33342</ChannelName><ImageResolutionX Unit="m">5.9338E-07</ImageResolutionX><ImageResolutionY Unit="m">5.9338E-07</ImageResolutionY><ImageSizeX>4</ImageSizeX><ImageSizeY>2</ImageSizeY><MainExcitationWavelength Unit="nm">375</MainExcitationWavelength><MainEmissionWavelength Unit="nm">456</MainEmissionWavelength><ObjectiveMagnification Unit="">20</ObjectiveMagnification><ObjectiveNA Unit="">1</ObjectiveNA><ExposureTime Unit="s">0.04</ExposureTime></Entry>
      <Entry ChannelID="2"><ChannelName>Alexa 488</ChannelName><ImageResolutionX Unit="m">5.9338E-07</ImageResolutionX><ImageResolutionY Unit="m">5.9338E-07</ImageResolutionY><ImageSizeX>4</ImageSizeX><ImageSizeY>2</ImageSizeY><ExposureTime Unit="s">0.1</ExposureTime></Entry>
    </Map>
  </Maps>
  <Images>
    <Image Version="1"><id>a</id><State>Ok</State><URL>r03c03f01p01-ch1sk1fk1fl1.tiff</URL><Row>3</Row><Col>3</Col><FieldID>1</FieldID><PlaneID>1</PlaneID><TimepointID>0</TimepointID><ChannelID>1</ChannelID></Image>
    <Image Version="1"><id>b</id><State>Ok</State><URL>r03c03f01p01-ch2sk1fk1fl1.tiff</URL><Row>3</Row><Col>3</Col><FieldID>1</FieldID><PlaneID>1</PlaneID><TimepointID>0</TimepointID><ChannelID>2</ChannelID></Image>
  </Images>
</EvaluationInputData>"#;

    #[test]
    fn channel_descriptions_from_maps() {
        assert!(looks_like_harmony(INDEX_V7.as_bytes()));
        let fs = fs_with(INDEX_V7);
        let p = parse(&fs, Path::new("p/Index.idx.xml")).unwrap();
        assert_eq!(p.format_version.as_deref(), Some("HarmonyV7"));
        assert_eq!(p.instrument.software_version.as_deref(), Some("7"));
        let names: Vec<_> = p.channels.iter().map(|c| c.name.as_deref()).collect();
        assert_eq!(names, vec![Some("Hoechst 33342"), Some("Alexa 488")]);
        assert!((p.pixel_size_um[0].unwrap() - 0.59338).abs() < 1e-9);
        assert_eq!((p.size_x, p.size_y), (4, 2));
        assert_eq!(p.channels[0].excitation_nm, Some(375.0));
        assert_eq!(p.channels[1].exposure_ms, Some(100.0));
        assert_eq!(
            p.objective.as_ref().and_then(|o| o.nominal_magnification),
            Some(20.0)
        );
        assert!(p.notes.iter().any(|n| n.contains("Maps")));
    }

    #[test]
    fn well_ids_and_names() {
        assert_eq!(well_id("0307"), Some((2, 6)));
        assert_eq!(well_id("0007"), None);
        assert_eq!(well_id("30a7"), None);
        assert!(is_plane_name("r03c07f01p01-ch1sk1fk1fl1.tiff"));
        assert!(!is_plane_name("Index.idx.xml"));
        assert_eq!(relative_name("C:\\data\\r01.tiff"), "r01.tiff");
        assert_eq!(relative_name("sub\\r01.tiff"), "sub/r01.tiff");
    }
}
