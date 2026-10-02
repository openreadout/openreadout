//! MetaMorph conventions: STK files (UIC1–UIC4 private tags), MetaSeries TIFFs (`<MetaData>`
//! XML in `ImageDescription`) and `.nd` series files. Layouts derived from corpus files and
//! the public documentation of tifffile (BSD-3-Clause); see `docs/formats/tiff.md`
//! § MetaMorph and `docs/provenance/tiff.md`.

use openreadout_core::bytes::{latin1_field, le_u16, le_u32};
use openreadout_core::model::{ChannelInfo, ImageInfo, ObjectiveInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::FORMAT_ID;
use crate::container::{ByteOrder, ByteSource, FieldValue, Ifd};
use crate::dataset::{PlaneSrc, Series, TiffDataset, instrument_from_tags, resolution_um};
use crate::decode::PageLayout;

/// Private tag holding (id, value) pairs of image-wide settings.
pub const STK_TAG_SETTINGS: u16 = 33628;
/// Private tag holding six u32 per plane: Z distance (rational), creation and modification
/// day and time.
pub const STK_TAG_PLANES: u16 = 33629;
/// Private tag holding one rational per plane: the wavelength.
pub const STK_TAG_WAVELENGTHS: u16 = 33630;
/// Private tag holding tagged per-plane arrays (stage positions, stage labels, ...).
pub const STK_TAG_PLANE_ARRAYS: u16 = 33631;

/// Largest plane count accepted from the plane tag.
const MAX_STK_PLANES: u64 = 1 << 20;
/// Largest per-plane array block read from the plane-array tag.
const MAX_ARRAY_BYTES: u64 = 16 << 20;

/// Parsed STK metadata. Per-plane vectors have one entry per plane (or are empty).
#[derive(Debug, Clone, Default)]
pub struct StkInfo {
    /// Number of planes stored back to back after page 0's data.
    pub plane_count: u32,
    /// Distance to the next plane, per plane (calibration units).
    pub z_distance: Vec<f64>,
    /// Creation time per plane as (MetaMorph day number, milliseconds since midnight).
    pub created: Vec<(u32, u32)>,
    /// Wavelength per plane (nm, 0 = none).
    pub wavelengths_nm: Vec<f64>,
    /// Spatial calibration switched on.
    pub calibrated: bool,
    /// Pixel width and height in `calibration_units`.
    pub x_calibration: Option<f64>,
    /// See `x_calibration`.
    pub y_calibration: Option<f64>,
    /// Calibration unit text (`um`, `nm`, `pixel`, ...).
    pub calibration_units: Option<String>,
    /// Image name.
    pub name: Option<String>,
    /// Stage X/Y per plane (signed rationals, stage units).
    pub stage_x: Vec<f64>,
    /// See `stage_x`.
    pub stage_y: Vec<f64>,
    /// Stage label per plane.
    pub stage_labels: Vec<String>,
    /// Absolute focus position per plane.
    pub absolute_z: Vec<f64>,
    /// Per-plane description texts (NUL-separated in `ImageDescription`).
    pub plane_texts: Vec<String>,
    /// Exposure in ms from the first plane text (`Exposure: 40 ms`).
    pub exposure_ms: Option<f64>,
}

/// Whether page 0 carries the STK plane tag (the marker of an STK file).
pub fn is_stk(ifd: &Ifd) -> bool {
    ifd.field(STK_TAG_PLANES).is_some() && ifd.field(STK_TAG_SETTINGS).is_some()
}

fn rational(num: u32, den: u32) -> Option<f64> {
    (den != 0).then(|| f64::from(num) / f64::from(den))
}

fn signed_rational(num: u32, den: u32) -> Option<f64> {
    (den != 0).then(|| f64::from(num as i32) / f64::from(den))
}

/// Parse the UIC tags of page 0 (little-endian files only; STK is always little-endian). Needs
/// UIC1; without UIC2 the page is taken as a single plane.
pub fn parse_stk(ifd: &Ifd, src: &mut ByteSource) -> Option<StkInfo> {
    if src.order != ByteOrder::Little {
        return None;
    }
    // UIC1 is required; without UIC2 (single-plane files of some MetaMorph versions) the
    // file is one plane.
    ifd.field(STK_TAG_SETTINGS)?;
    let planes = ifd.field(STK_TAG_PLANES);
    let n = planes.map_or(1, |p| p.count);
    if n == 0 || n > MAX_STK_PLANES {
        return None;
    }
    let mut info = StkInfo {
        plane_count: n as u32,
        ..StkInfo::default()
    };
    if let Some(off) = planes.and_then(|p| p.value_offset)
        && let Ok(b) = src.read_at(off, 24 * n)
    {
        for c in b.as_chunks::<24>().0 {
            let v: Vec<u32> = (0..6).filter_map(|k| le_u32(c, 4 * k)).collect();
            info.z_distance.push(rational(v[0], v[1]).unwrap_or(0.0));
            info.created.push((v[2], v[3]));
        }
    }
    if let Some(f) = ifd.field(STK_TAG_WAVELENGTHS)
        && let FieldValue::Float(w) = &f.value
        && w.len() as u64 == n
    {
        info.wavelengths_nm.clone_from(w);
    }
    if let Some(f) = ifd.field(STK_TAG_SETTINGS)
        && let Some(off) = f.value_offset
        && f.count <= 4096
        && let Ok(b) = src.read_at(off, 8 * f.count)
    {
        for pair in b.as_chunks::<8>().0 {
            let (Some(id), Some(v)) = (le_u32(pair, 0), le_u32(pair, 4)) else {
                continue;
            };
            settings_entry(&mut info, id, v, src);
        }
    }
    if let Some(f) = ifd.field(STK_TAG_PLANE_ARRAYS)
        && let Some(off) = f.value_offset
    {
        plane_arrays(&mut info, off, n, src);
    }
    if let Some(FieldValue::Ascii(d)) = ifd.field(crate::tags::IMAGE_DESCRIPTION).map(|f| &f.value)
    {
        info.plane_texts = d
            .split('\0')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        info.exposure_ms = info.plane_texts.first().and_then(|t| exposure_ms(t));
    }
    Some(info)
}

/// One (id, value) pair of the settings tag. Ids 4/5 point at a rational, 6/7 at a
/// length-prefixed text; 3 is the calibration switch.
fn settings_entry(info: &mut StkInfo, id: u32, v: u32, src: &mut ByteSource) {
    let at = u64::from(v);
    match id {
        3 => info.calibrated = v != 0,
        4 | 5 if at >= 8 => {
            if let Ok(b) = src.read_at(at, 8)
                && let (Some(num), Some(den)) = (le_u32(&b, 0), le_u32(&b, 4))
            {
                let r = rational(num, den).filter(|r| r.is_finite() && *r > 0.0);
                if id == 4 {
                    info.x_calibration = r;
                } else {
                    info.y_calibration = r;
                }
            }
        }
        6 | 7 if at >= 8 => {
            if let Ok(h) = src.read_at(at, 4)
                && let Some(len) = le_u32(&h, 0)
                && len > 0
                && len < 1024
                && let Ok(t) = src.read_at(at + 4, u64::from(len))
            {
                let text = latin1_field(&t);
                if !text.is_empty() {
                    if id == 6 {
                        info.calibration_units = Some(text);
                    } else {
                        info.name = Some(text);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Text up to the first NUL, trimmed (Latin-1).
/// The plane-array tag: u16 id + data, repeated until id 0 or an id whose size is unknown.
fn plane_arrays(info: &mut StkInfo, off: u64, n: u64, src: &mut ByteSource) {
    let avail = src.len.saturating_sub(off).min(MAX_ARRAY_BYTES);
    let Ok(b) = src.read_at(off, avail) else {
        return;
    };
    let n = n as usize;
    let mut p = 0usize;
    while let Some(id) = le_u16(&b, p) {
        p += 2;
        match id {
            // (x num, x den, y num, y den) per plane, signed numerators
            28 => {
                let Some(block) = b.get(p..p + 16 * n) else {
                    return;
                };
                for c in block.as_chunks::<16>().0 {
                    let v: Vec<u32> = (0..4).filter_map(|k| le_u32(c, 4 * k)).collect();
                    info.stage_x
                        .push(signed_rational(v[0], v[1]).unwrap_or(0.0));
                    info.stage_y
                        .push(signed_rational(v[2], v[3]).unwrap_or(0.0));
                }
                p += 16 * n;
            }
            // camera chip offsets, same shape: skipped
            29 => p += 16 * n,
            // absolute Z per plane (signed rational)
            40 => {
                let Some(block) = b.get(p..p + 8 * n) else {
                    return;
                };
                info.absolute_z = block
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| {
                        signed_rational(le_u32(c, 0).unwrap_or(0), le_u32(c, 4).unwrap_or(0))
                            .unwrap_or(0.0)
                    })
                    .collect();
                p += 8 * n;
            }
            41 => p += 4 * n,
            // per plane: u32 length + text
            37 => {
                let mut labels = Vec::with_capacity(n);
                for _ in 0..n {
                    let Some(len) = le_u32(&b, p) else { return };
                    let len = len as usize;
                    let Some(t) = b.get(p + 4..p + 4 + len) else {
                        return;
                    };
                    labels.push(latin1_field(t));
                    p += 4 + len;
                }
                info.stage_labels = labels;
            }
            _ => return,
        }
    }
}

/// `Exposure: 40 ms` (or `s`/`µs`) → milliseconds.
pub fn exposure_ms(text: &str) -> Option<f64> {
    let line = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("Exposure:"))?
        .trim();
    let (num, unit) = line.split_at(
        line.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == ','))
            .unwrap_or(line.len()),
    );
    let v: f64 = num.replace(',', ".").parse().ok()?;
    let f = match unit.trim() {
        "ms" | "msec" => 1.0,
        "s" | "sec" => 1000.0,
        "us" | "µs" | "usec" => 1e-3,
        _ => return None,
    };
    Some(v * f)
}

/// MetaMorph day number (Julian day number − 1) and milliseconds since midnight → ISO-8601
/// local time with milliseconds (no zone: MetaMorph records the acquisition PC's clock).
pub fn metamorph_time(day: u32, ms: u32) -> Option<String> {
    // Julian day number 2440588 is 1970-01-01.
    let days = i64::from(day) + 1 - 2_440_588;
    if !(-719_162..=2_932_896).contains(&days) || ms >= 86_400_000 {
        return None;
    }
    let iso =
        openreadout_core::time::unix_to_iso8601(days * 86_400 + i64::from(ms / 1000), ms % 1000);
    Some(iso.trim_end_matches('Z').to_string())
}

/// Seconds since the MetaMorph epoch (for differences between plane times).
pub fn metamorph_seconds(day: u32, ms: u32) -> f64 {
    f64::from(day) * 86_400.0 + f64::from(ms) / 1000.0
}

/// Spatial calibration unit → µm factor (`None` for `pixel` and unknown units).
pub fn unit_um(unit: &str) -> Option<f64> {
    match unit.trim().to_ascii_lowercase().as_str() {
        "um" | "µm" | "μm" | "micron" | "microns" | "micrometer" => Some(1.0),
        "nm" | "nanometer" => Some(1e-3),
        "mm" | "millimeter" => Some(1e3),
        "cm" => Some(1e4),
        "m" => Some(1e6),
        _ => None,
    }
}

impl StkInfo {
    /// Pixel size (x, y) in µm when calibration is on and the unit is a length.
    pub fn pixel_size_um(&self) -> (Option<f64>, Option<f64>) {
        if !self.calibrated {
            return (None, None);
        }
        let f = self.calibration_units.as_deref().and_then(unit_um);
        (
            f.and_then(|f| self.x_calibration.map(|x| x * f)),
            f.and_then(|f| self.y_calibration.map(|y| y * f)),
        )
    }

    /// The Z step in µm: the plane distance when every plane has the same non-zero one.
    pub fn z_step_um(&self) -> Option<f64> {
        let first = *self.z_distance.first()?;
        let f = self
            .calibration_units
            .as_deref()
            .and_then(unit_um)
            .unwrap_or(1.0);
        (first > 0.0 && self.z_distance.iter().all(|d| (d - first).abs() < 1e-9))
            .then_some(first * f)
    }

    /// How the planes stack: `Z` when every plane has a non-zero Z distance, else `T` when the
    /// creation times differ, else `Z` (an unlabelled stack).
    pub fn plane_axis(&self) -> char {
        if self.plane_count <= 1 {
            return 'Z';
        }
        if !self.z_distance.is_empty() && self.z_distance.iter().all(|d| *d != 0.0) {
            return 'Z';
        }
        let times: Vec<f64> = self
            .created
            .iter()
            .map(|&(d, ms)| metamorph_seconds(d, ms))
            .collect();
        if times.len() > 1 && times.windows(2).all(|w| (w[1] - w[0]).abs() > 1e-6) {
            return 'T';
        }
        'Z'
    }

    /// Selected values for `extra` and `info --view full` (our vocabulary).
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("plane_count".into(), json!(self.plane_count));
        m.insert("calibrated".into(), json!(self.calibrated));
        if let Some(u) = &self.calibration_units {
            m.insert("calibration_units".into(), json!(u));
        }
        if let Some(x) = self.x_calibration {
            m.insert("x_calibration".into(), json!(x));
        }
        if let Some(y) = self.y_calibration {
            m.insert("y_calibration".into(), json!(y));
        }
        if let Some(n) = &self.name {
            m.insert("name".into(), json!(n));
        }
        if let Some(first) = self.wavelengths_nm.first() {
            m.insert("wavelength_nm".into(), json!(first));
        }
        if let Some(e) = self.exposure_ms {
            m.insert("exposure_ms".into(), json!(e));
        }
        if let Some(t) = self.plane_texts.first() {
            m.insert("plane_text".into(), json!(t));
        }
        Value::Object(m)
    }
}

// ------------------------------------------------------------------ MetaSeries

/// One plane's MetaSeries `<MetaData>` description.
#[derive(Debug, Clone, Default)]
pub struct MetaSeriesPlane {
    /// Every `prop` (and `custom-prop`) as id → typed value, by section (`root`, `plane`, `set`).
    pub props: Map<String, Value>,
    /// Pixel size (x, y) in µm from `spatial-calibration-x/y` when calibration is on.
    pub pixel_size_um: (Option<f64>, Option<f64>),
    /// `image-name`.
    pub image_name: Option<String>,
    /// `acquisition-time-local` as ISO-8601 local time.
    pub acquired_local: Option<String>,
    /// `_IllumSetting_`: the illumination setting (MetaMorph's name for the wavelength).
    pub illumination: Option<String>,
    /// `_MagSetting_`: the objective setting.
    pub objective: Option<String>,
    /// `_MagNA_`.
    pub objective_na: Option<f64>,
    /// Exposure in ms from the `Description` text.
    pub exposure_ms: Option<f64>,
    /// `stage-position-x`, `stage-position-y`, `z-position`.
    pub stage_um: [Option<f64>; 3],
    /// `wavelength` (nm; 0 = none).
    pub wavelength_nm: Option<f64>,
    /// `ApplicationName` and `ApplicationVersion`.
    pub application: Option<String>,
    /// See `application`.
    pub application_version: Option<String>,
    /// `number-of-planes` of `SetInfo`.
    pub plane_count: Option<u32>,
}

/// `None` unless the description is a `<MetaData>` document.
pub fn parse_metaseries(desc: &str) -> Option<MetaSeriesPlane> {
    let d = desc.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if !d.starts_with("<MetaData>") {
        return None;
    }
    let doc = roxmltree::Document::parse(d).ok()?;
    let mut sections = Map::new();
    for sec in std::iter::once(doc.root_element()).chain(
        doc.root_element()
            .children()
            .filter(roxmltree::Node::is_element),
    ) {
        let mut props = Map::new();
        for p in sec
            .children()
            .filter(|n| n.is_element() && matches!(n.tag_name().name(), "prop" | "custom-prop"))
        {
            let (Some(id), Some(v)) = (p.attribute("id"), p.attribute("value")) else {
                continue;
            };
            let val = match p.attribute("type").unwrap_or("") {
                "int" => v.parse::<i64>().map_or_else(|_| json!(v), Value::from),
                "float" => v
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .map_or_else(|| json!(v), Value::from),
                "bool" => json!(v == "on" || v == "true"),
                _ => json!(v),
            };
            props.insert(id.to_string(), val);
        }
        let key = match sec.tag_name().name() {
            "MetaData" => "root",
            "PlaneInfo" => "plane",
            "SetInfo" => "set",
            other => other,
        };
        if !props.is_empty() {
            sections.insert(key.to_string(), Value::Object(props));
        }
    }
    let get = |sec: &str, id: &str| sections.get(sec).and_then(|s| s.get(id));
    let text = |sec: &str, id: &str| {
        get(sec, id)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let num = |sec: &str, id: &str| get(sec, id).and_then(Value::as_f64);
    let calibrated =
        get("plane", "spatial-calibration-state").and_then(Value::as_bool) == Some(true);
    let f = text("plane", "spatial-calibration-units").and_then(|u| unit_um(&u));
    let cal = |id: &str| {
        if calibrated {
            f.and_then(|f| num("plane", id).map(|v| v * f))
                .filter(|v| *v > 0.0)
        } else {
            None
        }
    };
    let description =
        text("root", "Description").map(|d| d.replace("&#13;&#10;", "\n").replace("\r\n", "\n"));
    Some(MetaSeriesPlane {
        pixel_size_um: (cal("spatial-calibration-x"), cal("spatial-calibration-y")),
        image_name: text("plane", "image-name"),
        acquired_local: text("plane", "acquisition-time-local").and_then(|t| metaseries_time(&t)),
        illumination: text("plane", "_IllumSetting_"),
        objective: text("plane", "_MagSetting_"),
        objective_na: num("plane", "_MagNA_").filter(|v| *v > 0.0),
        exposure_ms: description.as_deref().and_then(exposure_ms),
        stage_um: [
            num("plane", "stage-position-x"),
            num("plane", "stage-position-y"),
            num("plane", "z-position"),
        ],
        wavelength_nm: num("plane", "wavelength").filter(|v| *v > 0.0),
        application: text("root", "ApplicationName"),
        application_version: text("root", "ApplicationVersion"),
        plane_count: num("set", "number-of-planes").map(|v| v as u32),
        props: sections,
    })
}

/// Nominal magnification from an objective setting name that starts with it (`20x EC
/// Plan-neofluar (Air)`, `40XOil`, `100xAndor`).
pub fn magnification(objective: &str) -> Option<f64> {
    let t = objective.trim();
    let digits = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (num, rest) = t.split_at(digits);
    (!num.is_empty() && rest.starts_with(['x', 'X']))
        .then(|| num.parse::<f64>().ok())
        .flatten()
        .filter(|m| *m > 0.0)
}

/// `20240816 09:49:42.978` → `2024-08-16T09:49:42.978`.
pub fn metaseries_time(t: &str) -> Option<String> {
    let t = t.trim();
    let (d, rest) = t.split_once(' ')?;
    if d.len() != 8 || !d.bytes().all(|b| b.is_ascii_digit()) || rest.len() < 8 {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}",
        &d[0..4],
        &d[4..6],
        &d[6..8],
        rest.trim()
    ))
}

// ------------------------------------------------------------------ .nd series files

/// A parsed `.nd` file.
#[derive(Debug, Clone, Default)]
pub struct NdFile {
    /// Every key with its value (quotes removed), in file order.
    pub keys: Map<String, Value>,
    /// `NDInfoFile` version text.
    pub version: Option<String>,
    /// `Description` (may span several lines).
    pub description: Option<String>,
    /// `StartTime1` as ISO-8601 local time.
    pub start_time: Option<String>,
    /// Time points (1 without a time lapse).
    pub time_points: u32,
    /// Whether file names carry `_t<k>`.
    pub timelapse: bool,
    /// Stage position names (empty without stages).
    pub stages: Vec<String>,
    /// Wavelength names (empty without wavelengths).
    pub waves: Vec<String>,
    /// Per wavelength: acquired as a Z series.
    pub wave_do_z: Vec<bool>,
    /// Whether the wavelength name follows `_w<i>` in file names.
    pub wave_in_file_name: bool,
    /// Z planes (1 without a Z series).
    pub z_steps: u32,
    /// `ZStepSize` (µm).
    pub z_step_um: Option<f64>,
}

/// Largest count accepted from an `.nd` key (stages, wavelengths, time points, Z steps).
const MAX_ND_COUNT: u32 = 1 << 20;

/// `None` unless the text starts with the `"NDInfoFile"` key.
pub fn parse_nd(text: &str) -> Option<NdFile> {
    let text = text.trim_start_matches('\u{feff}');
    if !text.trim_start().starts_with("\"NDInfoFile\"") {
        return None;
    }
    let mut keys = Map::new();
    let mut last: Option<String> = None;
    for line in text.lines() {
        let l = line.trim_end_matches('\r');
        if let Some(rest) = l.strip_prefix('"')
            && let Some((k, v)) = rest.split_once("\",")
        {
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(v);
            keys.insert(k.to_string(), json!(v));
            last = Some(k.to_string());
            if k == "EndFile" {
                break;
            }
        } else if l.trim() == "\"EndFile\"" {
            break;
        } else if let Some(k) = &last
            && let Some(Value::String(prev)) = keys.get_mut(k)
        {
            // continuation of a multi-line value (Description)
            if !prev.is_empty() {
                prev.push('\n');
            }
            prev.push_str(l.trim_end());
        }
    }
    let s = |k: &str| keys.get(k).and_then(Value::as_str).map(str::trim);
    let flag = |k: &str| s(k).is_some_and(|v| v.eq_ignore_ascii_case("TRUE"));
    let count = |k: &str| {
        s(k).and_then(|v| v.parse::<u32>().ok())
            .map(|v| v.min(MAX_ND_COUNT))
    };
    let timelapse = flag("DoTimelapse");
    let time_points = if timelapse {
        count("NTimePoints").unwrap_or(1).max(1)
    } else {
        1
    };
    let stages = if flag("DoStage") {
        let n = count("NStagePositions").unwrap_or(0);
        (1..=n)
            .map(|i| s(&format!("Stage{i}")).unwrap_or("").to_string())
            .collect()
    } else {
        Vec::new()
    };
    let (waves, wave_do_z) = if flag("DoWave") {
        let n = count("NWavelengths").unwrap_or(0);
        (
            (1..=n)
                .map(|i| s(&format!("WaveName{i}")).unwrap_or("").to_string())
                .collect(),
            (1..=n)
                .map(|i| s(&format!("WaveDoZ{i}")).is_none_or(|v| v.eq_ignore_ascii_case("TRUE")))
                .collect(),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let z_series = flag("DoZSeries");
    let z_steps = if z_series {
        count("NZSteps").unwrap_or(1).max(1)
    } else {
        1
    };
    let z_step_um = s("ZStepSize")
        .and_then(|v| v.replace(',', ".").parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0);
    Some(NdFile {
        version: s("NDInfoFile").map(|v| v.trim_start_matches("Version").trim().to_string()),
        description: s("Description")
            .filter(|d| !d.is_empty())
            .map(str::to_string),
        start_time: s("StartTime1").and_then(metaseries_time),
        time_points,
        timelapse,
        stages,
        waves,
        wave_do_z,
        wave_in_file_name: flag("WaveInFileName"),
        z_steps,
        z_step_um,
        keys,
    })
}

impl NdFile {
    /// Number of images (stage positions, at least 1).
    pub fn image_count(&self) -> usize {
        self.stages.len().max(1)
    }

    /// Number of channels (wavelengths, at least 1).
    pub fn channel_count(&self) -> usize {
        self.waves.len().max(1)
    }

    /// File name stem (no extension) of the file holding stage `s`, wavelength `w` and time
    /// point `t` (all zero-based) for a data set whose `.nd` stem is `stem`.
    pub fn file_stem(&self, stem: &str, s: usize, w: usize, t: u32) -> String {
        let mut name = stem.to_string();
        if !self.waves.is_empty() {
            name.push_str(&format!("_w{}", w + 1));
            if self.wave_in_file_name {
                name.push_str(&self.waves[w]);
            }
        }
        if !self.stages.is_empty() {
            name.push_str(&format!("_s{}", s + 1));
        }
        if self.timelapse {
            name.push_str(&format!("_t{}", t + 1));
        }
        name
    }
}

impl TiffDataset {
    /// MetaMorph STK: planes back to back after page 0's first strip; UIC tags for geometry,
    /// calibration and per-plane times.
    pub(crate) fn build_stk(&mut self, page0: &Ifd, stk: &StkInfo) -> Result<()> {
        let order = self.main().header.byte_order;
        let lay = PageLayout::from_ifd(page0, order)?;
        let pt = lay.pixel_type()?;
        let count = stk.plane_count.max(1);
        let contiguous = lay.compression == 1
            && lay.planar != 2
            && lay
                .offsets
                .windows(2)
                .zip(&lay.byte_counts)
                .all(|(w, &c)| w[0].saturating_add(c) == w[1]);
        if count > 1 && !contiguous {
            return Err(Error::unsupported(
                FORMAT_ID,
                "an STK file whose first plane is compressed or not stored in one block",
                "Only uncompressed STK stacks are read (the planes after the first have no strip table); please share a sample file.",
            ));
        }
        let axis = stk.plane_axis();
        let mut info = ImageInfo::new(0, lay.width, lay.height, pt);
        info.samples_per_pixel = if lay.planar == 2 {
            1
        } else {
            u32::from(lay.samples_per_pixel)
        };
        if axis == 'T' {
            info.size_t = count;
        } else {
            info.size_z = count;
        }
        let (mut px, mut py) = stk.pixel_size_um();
        if px.is_none() && py.is_none() {
            (px, py) = resolution_um(page0);
        }
        let pz = if axis == 'Z' && count > 1 {
            stk.z_step_um()
        } else {
            None
        };
        info.physical_size = PhysicalSize::micrometres(px, py, pz);
        info.name.clone_from(&stk.name);
        info.acquired_at = stk
            .created
            .first()
            .and_then(|&(d, ms)| metamorph_time(d, ms));
        if axis == 'T'
            && let (Some(a), Some(b)) = (stk.created.first(), stk.created.last())
            && count > 1
        {
            let dt =
                (metamorph_seconds(b.0, b.1) - metamorph_seconds(a.0, a.1)) / f64::from(count - 1);
            if dt > 0.0 {
                info.time_increment_s = Some(dt);
            }
        }
        info.channels = vec![ChannelInfo {
            index: 0,
            exposure_ms: stk.exposure_ms,
            ..ChannelInfo::default()
        }];
        info.instrument = instrument_from_tags(page0);
        info.extra.insert("metamorph".into(), stk.to_json());
        if let (Some(sx), Some(sy)) = (stk.stage_x.first(), stk.stage_y.first()) {
            info.extra.insert(
                "stage_position_um".into(),
                json!({"x": sx, "y": sy, "z": stk.absolute_z.first()}),
            );
        }
        if let Some(label) = stk.stage_labels.first().filter(|s| !s.is_empty()) {
            info.extra.insert("stage_label".into(), json!(label));
        }
        let planes = (0..u64::from(count))
            .map(|i| {
                Some(if count == 1 {
                    PlaneSrc::Page {
                        file: 0,
                        page: 0,
                        sample: None,
                    }
                } else {
                    PlaneSrc::Contiguous { file: 0, index: i }
                })
            })
            .collect();
        if count > 1 {
            self.notes.push(format!(
                "MetaMorph STK: {count} planes stored back to back after the first page's data, exposed as {}",
                if axis == 'T' { "T" } else { "Z" }
            ));
        }
        self.series.push(Series {
            info: info.finish(),
            planes,
            levels: Vec::new(),
        });
        self.set_provenance(&[
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_z", Source::PriorArt),
            ("images[].size_t", Source::PriorArt),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::PriorArt),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].time_increment_s", Source::PriorArt),
            ("images[].name", Source::PriorArt),
            ("images[].instrument", Source::Spec),
            ("images[].channels[].exposure_ms", Source::Inferred),
            ("images[].extra.metamorph", Source::PriorArt),
            ("images[].extra.stage_position_um", Source::Inferred),
            ("images[].extra.stage_label", Source::PriorArt),
        ]);
        Ok(())
    }

    /// MetaSeries TIFF: the plain page grouping plus the `<MetaData>` values of page 0.
    pub(crate) fn apply_metaseries(&mut self, ms: &MetaSeriesPlane) {
        let Some(s) = self.series.first_mut() else {
            return;
        };
        let info = &mut s.info;
        if ms.pixel_size_um.0.is_some() || ms.pixel_size_um.1.is_some() {
            info.physical_size =
                PhysicalSize::micrometres(ms.pixel_size_um.0, ms.pixel_size_um.1, None);
        }
        if let Some(n) = &ms.image_name {
            info.name = Some(n.clone());
        }
        if let Some(t) = &ms.acquired_local {
            info.acquired_at = Some(t.clone());
        }
        if let Some(c) = info.channels.first_mut() {
            c.name = ms.illumination.clone().or_else(|| ms.image_name.clone());
            c.exposure_ms = ms.exposure_ms;
        }
        if ms.objective.is_some() || ms.objective_na.is_some() {
            info.objective = Some(ObjectiveInfo {
                model: ms.objective.clone(),
                nominal_magnification: ms.objective.as_deref().and_then(magnification),
                lens_na: ms.objective_na,
                ..ObjectiveInfo::default()
            });
        }
        if ms.application.is_some() {
            let mut inst = info.instrument.clone().unwrap_or_default();
            inst.software.clone_from(&ms.application);
            inst.software_version.clone_from(&ms.application_version);
            info.instrument = Some(inst);
        }
        if let [Some(x), Some(y), z] = ms.stage_um {
            info.extra
                .insert("stage_position_um".into(), json!({"x": x, "y": y, "z": z}));
        }
        let mut keep = Map::new();
        if let Some(Value::Object(plane)) = ms.props.get("plane") {
            for k in [
                "stage-label",
                "camera-binning-x",
                "camera-binning-y",
                "wavelength",
                "acquisition-time-local",
            ] {
                if let Some(v) = plane.get(k) {
                    keep.insert(k.into(), v.clone());
                }
            }
        }
        info.extra.insert("metaseries".into(), Value::Object(keep));
        self.set_provenance(&[
            ("images[].physical_size", Source::PriorArt),
            ("images[].name", Source::PriorArt),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].channels[].name", Source::Inferred),
            ("images[].channels[].exposure_ms", Source::Inferred),
            ("images[].objective", Source::PriorArt),
            ("images[].instrument", Source::PriorArt),
            ("images[].extra.stage_position_um", Source::Inferred),
            ("images[].extra.metaseries", Source::PriorArt),
        ]);
    }
}

impl TiffDataset {
    /// STK per-plane records for `info --view full` (`frames`).
    pub(crate) fn stk_frames(&self, limit: Option<usize>) -> (u64, Vec<Value>) {
        let (Some(stk), Some(s)) = (&self.stk, self.series.first()) else {
            return (0, Vec::new());
        };
        let axis_t = s.info.size_t > 1;
        let n = stk.plane_count as usize;
        let t0 = stk.created.first().map(|&(d, ms)| metamorph_seconds(d, ms));
        let out = (0..n)
            .take(limit.unwrap_or(usize::MAX))
            .map(|i| {
                let mut r = Map::new();
                r.insert(if axis_t { "t" } else { "z" }.into(), json!(i));
                if let (Some(&(d, ms)), Some(t0)) = (stk.created.get(i), t0) {
                    let dt = metamorph_seconds(d, ms) - t0;
                    r.insert("delta_t_s".into(), json!((dt * 1000.0).round() / 1000.0));
                    if let Some(t) = metamorph_time(d, ms) {
                        r.insert("time_local".into(), json!(t));
                    }
                }
                if let (Some(x), Some(y)) = (stk.stage_x.get(i), stk.stage_y.get(i)) {
                    r.insert("stage_x_um".into(), json!(x));
                    r.insert("stage_y_um".into(), json!(y));
                }
                if let Some(z) = stk.absolute_z.get(i) {
                    r.insert("stage_z_um".into(), json!(z));
                }
                if let Some(w) = stk.wavelengths_nm.get(i).filter(|w| **w > 0.0) {
                    r.insert("wavelength_nm".into(), json!(w));
                }
                Value::Object(r)
            })
            .collect();
        (n as u64, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nd_parse() {
        let nd = parse_nd("\"NDInfoFile\", Version 2.0\r\n\"Description\", \r\n40xOil\r\nline 2\r\n\r\n\"StartTime1\", 20211116 18:32:52\r\n\"DoTimelapse\", TRUE\r\n\"NTimePoints\", 31\r\n\"DoStage\", TRUE\r\n\"NStagePositions\", 2\r\n\"Stage1\", \"Position_1\"\r\n\"Stage2\", \"Position_2\"\r\n\"DoWave\", TRUE\r\n\"NWavelengths\", 2\r\n\"WaveName1\", \"FRET\"\r\n\"WaveDoZ1\", FALSE\r\n\"WaveName2\", \"CFP\"\r\n\"WaveDoZ2\", FALSE\r\n\"DoZSeries\", TRUE\r\n\"NZSteps\", 11\r\n\"ZStepSize\", 0,50\r\n\"WaveInFileName\", TRUE\r\n\"NEvents\", 0\r\n\"EndFile\"\r\n").unwrap();
        assert_eq!(nd.version.as_deref(), Some("2.0"));
        assert_eq!(nd.description.as_deref(), Some("40xOil\nline 2"));
        assert_eq!(nd.start_time.as_deref(), Some("2021-11-16T18:32:52"));
        assert_eq!((nd.time_points, nd.z_steps), (31, 11));
        assert_eq!(nd.z_step_um, Some(0.5));
        assert_eq!(nd.stages, vec!["Position_1", "Position_2"]);
        assert_eq!(nd.waves, vec!["FRET", "CFP"]);
        assert_eq!(nd.wave_do_z, vec![false, false]);
        assert_eq!(nd.file_stem("Dish2", 1, 0, 30), "Dish2_w1FRET_s2_t31");
        assert!(parse_nd("\"Other\", 1").is_none());
    }

    #[test]
    fn times_and_exposure() {
        // tifffile's documented example: day 2451576, 54362783 ms = 2000-02-02 15:06:02.783
        assert_eq!(
            metamorph_time(2_451_576, 54_362_783).as_deref(),
            Some("2000-02-02T15:06:02.783")
        );
        assert_eq!(exposure_ms("Exposure: 40 ms\r\nBinning: 1"), Some(40.0));
        assert_eq!(magnification("20x EC Plan-neofluar (Air)"), Some(20.0));
        assert_eq!(magnification("40XOil"), Some(40.0));
        assert_eq!(magnification("Pixel"), None);
        assert_eq!(exposure_ms("Exposure: 0,5 s"), Some(500.0));
        assert_eq!(
            metaseries_time("20240816 09:49:42.978").as_deref(),
            Some("2024-08-16T09:49:42.978")
        );
    }

    #[test]
    fn metaseries() {
        let m = parse_metaseries("<MetaData>\n<prop id=\"Description\" type=\"string\" value=\"Exposure: 50 ms&#13;&#10;Binning: 2 x 2\"/>\n<prop id=\"ApplicationName\" type=\"string\" value=\"MetaMorph\"/>\n<PlaneInfo>\n<prop id=\"spatial-calibration-state\" type=\"bool\" value=\"on\"/>\n<prop id=\"spatial-calibration-x\" type=\"float\" value=\"0.65\"/>\n<prop id=\"spatial-calibration-y\" type=\"float\" value=\"0.65\"/>\n<prop id=\"spatial-calibration-units\" type=\"string\" value=\"um\"/>\n<prop id=\"image-name\" type=\"string\" value=\"Brighfield\"/>\n<prop id=\"acquisition-time-local\" type=\"time\" value=\"20240816 09:49:42.978\"/>\n</PlaneInfo>\n<SetInfo>\n<prop id=\"number-of-planes\" type=\"int\" value=\"1\"/>\n</SetInfo>\n</MetaData>").unwrap();
        assert_eq!(m.pixel_size_um, (Some(0.65), Some(0.65)));
        assert_eq!(m.exposure_ms, Some(50.0));
        assert_eq!(m.image_name.as_deref(), Some("Brighfield"));
        assert_eq!(m.acquired_local.as_deref(), Some("2024-08-16T09:49:42.978"));
        assert_eq!(m.plane_count, Some(1));
        assert_eq!(m.application.as_deref(), Some("MetaMorph"));
    }
}
