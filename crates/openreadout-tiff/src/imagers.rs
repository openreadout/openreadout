//! Gel and blot imagers that save plain TIFF files with their settings in text tags: Cytiva
//! Amersham Imager and ImageQuant 800 (`Key = Value` lines in `ImageDescription`), ImageQuant LAS
//! 4000 (`Key=Value` lines), Thermo Fisher iBright (the `Software` tag only), and Uvitec Alliance
//! and Vilber FusionCapt (a `_GLP_BPL_` block in tag 1808). The settings go to the instrument,
//! the channel, `images[].extra.imager` and the experiment; stored values are never changed.
//! See `docs/formats/tiff.md` § Gel and blot imagers and `docs/provenance/tiff.md`.

use std::collections::BTreeMap;

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Measurement, MeasurementKind, Method, Origin,
    Quantity,
};
use openreadout_core::model::InstrumentInfo;
use openreadout_core::provenance::Source;
use openreadout_core::vocab;
use serde_json::{Map, Value, json};

use crate::container::Ifd;
use crate::dataset::{Flavor, TiffDataset, text_value, tiff_datetime};
use crate::tags;

/// Tag holding the Uvitec/Vilber settings block (stored with type RATIONAL; read as bytes).
pub(crate) const GLP_TAG: u16 = 1808;
/// First bytes of that block: a one-byte length and `_GLP_BPL_`.
const GLP_MAGIC: &[u8] = b"\t_GLP_BPL_";
/// Largest settings block read.
const MAX_GLP_BYTES: u64 = 1 << 20;

/// Which imager wrote a file, as page 0's tags tell (the Uvitec/Vilber block is recognized from
/// its bytes, [`TiffDataset::glp_settings`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImagerKind {
    /// Amersham Imager 600/680 and ImageQuant 800: `Key = Value` description.
    Amersham,
    /// ImageQuant LAS 4000: `Key=Value` description.
    Las,
    /// Thermo Fisher iBright: the `Software` tag only.
    IBright,
}

impl ImagerKind {
    pub(crate) fn flavor(self) -> Flavor {
        match self {
            ImagerKind::Amersham | ImagerKind::Las => Flavor::CytivaImager,
            ImagerKind::IBright => Flavor::IBright,
        }
    }
}

/// One method parameter for the experiment: our name, value, unit, where it came from.
#[derive(Debug, Clone)]
pub(crate) struct Param {
    pub(crate) name: &'static str,
    pub(crate) value: Value,
    pub(crate) unit: Option<&'static str>,
    pub(crate) from: String,
}

/// Settings of an imager or a live-cell imager, ready for the image and the experiment.
#[derive(Debug, Clone, Default)]
pub(crate) struct ImagerSettings {
    pub(crate) vendor: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) serial: Option<String>,
    pub(crate) software: Option<String>,
    pub(crate) software_version: Option<String>,
    pub(crate) detector: Option<String>,
    /// Acquisition time (local, ISO 8601); `None` clears a time taken from tag 306.
    pub(crate) acquired_at: Option<String>,
    /// Whether `acquired_at` replaces whatever the plain-TIFF build found.
    pub(crate) replaces_time: bool,
    pub(crate) exposure_s: Option<f64>,
    /// The channel name (the imaging mode or method).
    pub(crate) mode: Option<String>,
    pub(crate) params: Vec<Param>,
    /// `images[].extra.imager`.
    pub(crate) extra: Map<String, Value>,
    pub(crate) notes: Vec<String>,
    /// Where vendor, model, serial and time came from (for the experiment's provenance).
    pub(crate) origins: BTreeMap<&'static str, String>,
}

impl ImagerSettings {
    fn param(&mut self, name: &'static str, value: Value, unit: Option<&'static str>, from: &str) {
        if value.is_null() {
            return;
        }
        self.params.push(Param {
            name,
            value,
            unit,
            from: from.to_string(),
        });
    }
}

/// The imager of a file from page 0's tags, `None` for anything else (Molecular Dynamics GEL
/// scans included).
pub(crate) fn kind_of(page0: &Ifd) -> Option<ImagerKind> {
    // Typhoon and other Molecular Dynamics scans (`Amersham Typhoon` in `Software`) are read
    // through their MD GEL tags instead.
    if page0.field(crate::mdgel::MD_FILE_TAG).is_some() {
        return None;
    }
    let model = page0.text(tags::MODEL).unwrap_or_default().trim();
    let software = page0.text(tags::SOFTWARE).unwrap_or_default().trim();
    let desc = page0.text(tags::IMAGE_DESCRIPTION).unwrap_or_default();
    if model.starts_with("Amersham")
        || software.starts_with("Amersham")
        || desc.contains("\nInstrument = Amersham")
    {
        return Some(ImagerKind::Amersham);
    }
    if model.starts_with("ImageQuant LAS") {
        return Some(ImagerKind::Las);
    }
    if software.starts_with("iBright") {
        return Some(ImagerKind::IBright);
    }
    None
}

/// `Key = Value` or `Key=Value` lines (CR, LF or CRLF); lines without `=` are skipped.
pub(crate) fn key_values(desc: &str) -> Vec<(String, String)> {
    desc.split(['\r', '\n'])
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            let k = k.trim();
            (!k.is_empty() && k.len() <= 64).then(|| (k.to_string(), v.trim().to_string()))
        })
        .collect()
}

/// A leading decimal number (`-24.55 °C` → -24.55, `1.244 µm` → 1.244).
pub(crate) fn leading_number(s: &str) -> Option<f64> {
    let s = s.trim();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(s.len(), |(i, _)| i);
    s[..end].parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A duration in seconds from text such as `5.0 s`, `10sec`, `0.46 msec` or
/// `0 min 11 sec 200 msec`; `None` when no number carries a time unit.
pub(crate) fn duration_s(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let (mut i, mut total, mut any) = (0usize, 0f64, false);
    while i < b.len() {
        if !(b[i].is_ascii_digit() || b[i] == b'.') {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
            i += 1;
        }
        let n: f64 = s.get(start..i)?.parse().ok()?;
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        let ustart = i;
        while i < b.len() && b[i].is_ascii_alphabetic() {
            i += 1;
        }
        let factor = match s.get(ustart..i)?.to_ascii_lowercase().as_str() {
            "h" | "hr" | "hour" | "hours" => 3600.0,
            "min" | "mins" | "minute" | "minutes" => 60.0,
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "ms" | "msec" | "msecs" => 1e-3,
            _ => return None,
        };
        total += n * factor;
        any = true;
    }
    (any && total.is_finite()).then_some(total)
}

fn yes_no(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "yes" | "on" | "true" => Some(true),
        "no" | "off" | "false" => Some(false),
        _ => None,
    }
}

/// A version token of a software name: `… Ver.2.0.0` → `2.0.0`, `… V2.0.0` → `2.0.0`.
fn version_of(software: &str) -> Option<String> {
    software.split_whitespace().find_map(|w| {
        let v = w
            .strip_prefix("Ver.")
            .or_else(|| w.strip_prefix('V'))
            .or_else(|| w.strip_prefix('v'))?;
        (v.starts_with(|c: char| c.is_ascii_digit()) && v.contains('.')).then(|| v.to_string())
    })
}

/// Settings of a Cytiva Amersham Imager / ImageQuant 800 or ImageQuant LAS 4000 file.
pub(crate) fn cytiva(page0: &Ifd, kind: ImagerKind) -> ImagerSettings {
    let desc = page0.text(tags::IMAGE_DESCRIPTION).unwrap_or_default();
    let kv = key_values(desc);
    let get = |k: &str| {
        kv.iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty() && *v != "-")
    };
    let mut s = ImagerSettings {
        vendor: Some("Cytiva".into()),
        ..ImagerSettings::default()
    };
    s.origins
        .insert("vendor", "the Amersham and ImageQuant product names".into());
    s.model = get("Instrument")
        .map(str::to_string)
        .or_else(|| text_value(page0, tags::MODEL));
    s.origins.insert(
        "model",
        if get("Instrument").is_some() {
            "ImageDescription `Instrument`".into()
        } else {
            "TIFF tag Model".into()
        },
    );
    if let Some(v) = get("Serial number") {
        s.serial = Some(v.to_string());
        s.origins
            .insert("serial", "ImageDescription `Serial number`".into());
    }
    s.software = text_value(page0, tags::SOFTWARE).or_else(|| get("Software").map(str::to_string));
    s.software_version = s.software.as_deref().and_then(version_of);
    s.acquired_at = get("Date time").and_then(tiff_datetime);
    // A file re-saved by other software (Photoshop) has that save's time in tag 306.
    let resaved = s
        .software
        .as_deref()
        .is_some_and(|sw| !sw.starts_with("Amersham") && !sw.starts_with("ImageQuant"));
    if s.acquired_at.is_none() && !resaved {
        s.acquired_at = text_value(page0, tags::DATE_TIME).and_then(|d| tiff_datetime(&d));
    }
    s.replaces_time = s.acquired_at.is_some() || resaved;
    if resaved {
        if let Some(sw) = s.software.take() {
            s.extra.insert("saved_by".into(), json!(sw));
        }
        s.notes.push(format!(
            "re-saved by {}: no acquisition time is recorded (tag 306 is the time of that save)",
            s.extra
                .get("saved_by")
                .and_then(Value::as_str)
                .unwrap_or("other software")
        ));
    }
    s.origins
        .insert("acquired_at", "ImageDescription `Date time`".into());
    let exposure_text = get("Exposure time");
    s.exposure_s = exposure_text.and_then(duration_s);
    s.mode = get("Method")
        .or_else(|| get("Imaging mode"))
        .or_else(|| get("LED&Filter"))
        .map(str::to_string);
    let light = get("Light").or_else(|| get("Illumination"));
    let filter = get("Filter");
    let binning = get("Binning");
    let temperature = get("CCD Temperature")
        .or_else(|| get("Temperature"))
        .and_then(leading_number);
    let dark_offset = get("Dark offset").and_then(leading_number);
    let inverted = get("Invert").or_else(|| get("Inverted")).and_then(yes_no);
    let range = match (
        get("Range low").and_then(leading_number),
        get("Range high").and_then(leading_number),
    ) {
        (Some(a), Some(b)) => json!([a, b]),
        _ => Value::Null,
    };
    let e = &mut s.extra;
    for (k, v) in [
        ("mode", s.mode.as_deref()),
        ("light", light),
        ("filter", filter),
        ("binning", binning),
        ("exposure_setting", exposure_text),
        ("exposure_mode", get("Exposure mode")),
        ("time_zone", get("Time zone")),
        ("orientation", get("Orientation")),
        ("image_split", get("Image split")),
        ("od_conversion", get("OD Conversion coefficient")),
        ("iris", get("Iris")),
        ("lens", get("Lens type")),
        (
            "tray_position",
            get("Tray position").or_else(|| get("Position")),
        ),
    ] {
        if let Some(v) = v {
            e.insert(k.into(), json!(v));
        }
    }
    if let Some(t) = s.exposure_s {
        e.insert("exposure_s".into(), json!(t));
    }
    if let Some(t) = temperature {
        e.insert("ccd_temperature_c".into(), json!(t));
    }
    if let Some(d) = dark_offset {
        e.insert("dark_offset".into(), json!(d));
    }
    if let Some(i) = inverted {
        e.insert("inverted".into(), json!(i));
    }
    if !range.is_null() {
        e.insert("display_range".into(), range);
    }
    e.insert(
        "settings".into(),
        Value::Object(kv.iter().map(|(k, v)| (k.clone(), json!(v))).collect()),
    );
    let field = if kind == ImagerKind::Las {
        "ImageDescription (LAS 4000 `Key=Value`)"
    } else {
        "ImageDescription"
    };
    s.param(
        "exposure_time",
        s.exposure_s.map_or(Value::Null, |v| json!(v)),
        Some("s"),
        &format!("{field} `Exposure time`"),
    );
    if let Some(m) = s.mode.clone() {
        s.param("mode", json!(m), None, &format!("{field} `Method`"));
    }
    for (name, v, key) in [
        ("light", light, "Light"),
        ("filter", filter, "Filter"),
        ("binning", binning, "Binning"),
    ] {
        if let Some(v) = v {
            s.param(name, json!(v), None, &format!("{field} `{key}`"));
        }
    }
    if let Some(t) = temperature {
        s.param(
            "ccd_temperature",
            json!(t),
            Some("°C"),
            &format!("{field} `Temperature`"),
        );
    }
    if let Some(d) = dark_offset {
        s.notes.push(format!(
            "values include the imager's dark offset of {d} counts (`Dark offset` in the description); they are as stored, not offset-corrected"
        ));
    }
    if inverted == Some(true) {
        s.notes.push(
            "the imager software inverted the image before saving (`Invert = Yes`): values are as stored".into(),
        );
    }
    s
}

/// Settings of an iBright file: the product name only.
pub(crate) fn ibright(page0: &Ifd) -> ImagerSettings {
    let mut s = ImagerSettings {
        vendor: Some("Thermo Fisher Scientific".into()),
        model: Some("iBright".into()),
        software: text_value(page0, tags::SOFTWARE),
        ..ImagerSettings::default()
    };
    s.origins
        .insert("vendor", "TIFF tag Software (`iBright(TM) Imager`)".into());
    s.origins
        .insert("model", "TIFF tag Software (`iBright(TM) Imager`)".into());
    s.notes.push(
        "iBright export: the file names the imager only (no exposure, mode or channel)".into(),
    );
    s
}

/// The texts of a `_GLP_BPL_` block: one-byte length, then that many printable bytes (Latin-1),
/// at least four, from the first record only (a second `_GLP_BPL_` starts another image's).
pub(crate) fn glp_strings(block: &[u8]) -> Vec<String> {
    let body = block.get(GLP_MAGIC.len()..).unwrap_or_default();
    let end = body
        .windows(GLP_MAGIC.len())
        .position(|w| w == GLP_MAGIC)
        .unwrap_or(body.len());
    let body = &body[..end];
    let texty =
        |b: u8| b == b'\r' || b == b'\n' || b == b'\t' || (0x20..0x7f).contains(&b) || b >= 0xa0;
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < body.len() {
        let n = usize::from(body[i]);
        if n >= 4
            && let Some(t) = body.get(i + 1..i + 1 + n)
            && t.iter().all(|&b| texty(b))
            && t.iter().any(u8::is_ascii_alphanumeric)
        {
            out.push(t.iter().map(|&b| char::from(b)).collect::<String>());
            i += 1 + n;
            continue;
        }
        i += 1;
    }
    out
}

/// `Thursday, 13 February 2020` and `02:00:38` → `2020-02-13T02:00:38` (English month names;
/// other languages give `None`).
fn glp_datetime(date: &str, time: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let words: Vec<String> = date
        .split([' ', ','])
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let m = words
        .iter()
        .find_map(|w| MONTHS.iter().position(|n| n == w))?;
    let nums: Vec<u32> = words.iter().filter_map(|w| w.parse().ok()).collect();
    let (d, y) = match nums.as_slice() {
        [d, y] if *d <= 31 && *y >= 1990 => (*d, *y),
        [y, d] if *d <= 31 && *y >= 1990 => (*d, *y),
        _ => return None,
    };
    let t = time.trim();
    let ok = t.len() == 8 && t.as_bytes()[2] == b':' && t.as_bytes()[5] == b':';
    let t = if ok { t } else { "00:00:00" };
    Some(format!("{y:04}-{:02}-{d:02}T{t}", m + 1))
}

/// Settings from a Uvitec/Vilber `_GLP_BPL_` block; `None` when the bytes are not one.
pub(crate) fn glp(block: &[u8]) -> Option<ImagerSettings> {
    if !block.starts_with(GLP_MAGIC) {
        return None;
    }
    let st = glp_strings(block);
    let is_time = |t: &str| {
        let b = t.as_bytes();
        b.len() == 8 && b[2] == b':' && b[5] == b':'
    };
    let time_i = st.iter().position(|t| is_time(t));
    let date = time_i
        .and_then(|i| i.checked_sub(1))
        .map(|i| st[i].trim().to_string());
    let time = time_i.map(|i| st[i].clone());
    let path_i = st.iter().position(|t| {
        let l = t.to_ascii_lowercase();
        l.contains(":\\")
            && std::path::Path::new(&l)
                .extension()
                .is_some_and(|e| e == "tif" || e == "tiff")
    });
    let product = path_i.and_then(|i| st.get(i + 1)).map(|t| {
        t.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches("- SN")
            .trim()
            .to_string()
    });
    // `Alliance Q9 Mini 18.02` → model `Alliance Q9 Mini`, version `18.02`
    let (model, version) = match &product {
        Some(p) => match p.rsplit_once(' ') {
            Some((m, v)) if v.starts_with(|c: char| c.is_ascii_digit()) && v.contains('.') => {
                (Some(m.to_string()), Some(v.to_string()))
            }
            _ => (Some(p.clone()), None),
        },
        None => (None, None),
    };
    let exposure_i = st
        .iter()
        .position(|t| t.contains(" sec") && duration_s(t).is_some());
    let exposure_text = exposure_i.map(|i| st[i].trim().to_string());
    let serial = exposure_i.and_then(|i| {
        st[i + 1..]
            .iter()
            .find(|t| t.chars().all(|c| c.is_ascii_alphanumeric()))
            .cloned()
    });
    let protocol_i = st
        .iter()
        .position(|t| t.to_ascii_lowercase().ends_with(".prt"));
    let mode = protocol_i
        .and_then(|i| st.get(i + 1))
        .map(|t| {
            let t = t.trim();
            let t = match t.split_once(": ") {
                Some((n, rest)) if n.chars().all(|c| c.is_ascii_digit()) => rest,
                _ => t,
            };
            t.split(" (").next().unwrap_or(t).trim().to_string()
        })
        .filter(|t| t.starts_with(|c: char| c.is_alphabetic()));
    let temperature = st.iter().find_map(|t| {
        let first = t.split_whitespace().next()?;
        (first.contains('.') && !first.contains(':'))
            .then(|| leading_number(first))
            .flatten()
            .filter(|v| (-100.0..60.0).contains(v))
    });
    let all = format!(
        "{} {}",
        product.as_deref().unwrap_or_default(),
        st.join(" ")
    )
    .to_ascii_lowercase();
    let vendor = if all.contains("alliance") || all.contains("uvitec") {
        Some("Uvitec")
    } else if all.contains("fusion") || all.contains("vilber") {
        Some("Vilber")
    } else {
        None
    };
    let mut s = ImagerSettings {
        vendor: vendor.map(str::to_string),
        model,
        serial,
        software_version: version,
        exposure_s: exposure_text.as_deref().and_then(duration_s),
        mode,
        acquired_at: date
            .as_deref()
            .zip(time.as_deref())
            .and_then(|(d, t)| glp_datetime(d, t)),
        ..ImagerSettings::default()
    };
    s.replaces_time = s.acquired_at.is_some();
    s.origins.insert(
        "vendor",
        "the product name in tag 1808 (`Alliance`, `Fusion`)".into(),
    );
    s.origins.insert("model", "tag 1808, product name".into());
    s.origins.insert("serial", "tag 1808, serial number".into());
    s.origins
        .insert("acquired_at", "tag 1808, date and time".into());
    let e = &mut s.extra;
    for (k, v) in [
        ("product", product.as_deref()),
        ("date_text", date.as_deref()),
        ("time_text", time.as_deref()),
        ("exposure_setting", exposure_text.as_deref()),
        ("mode", s.mode.as_deref()),
        ("original_path", path_i.map(|i| st[i].as_str())),
        ("protocol", protocol_i.map(|i| st[i].as_str())),
    ] {
        if let Some(v) = v {
            e.insert(k.into(), json!(v));
        }
    }
    if let Some(t) = s.exposure_s {
        e.insert("exposure_s".into(), json!(t));
    }
    if let Some(t) = temperature {
        e.insert("ccd_temperature_c".into(), json!(t));
    }
    e.insert("texts".into(), json!(st));
    s.param(
        "exposure_time",
        s.exposure_s.map_or(Value::Null, |v| json!(v)),
        Some("s"),
        "tag 1808, exposure",
    );
    if let Some(m) = s.mode.clone() {
        s.param("mode", json!(m), None, "tag 1808, mode");
    }
    if let Some(t) = temperature {
        s.param(
            "ccd_temperature",
            json!(t),
            Some("°C"),
            "tag 1808, CCD temperature",
        );
    }
    if s.acquired_at.is_none() && date.is_some() {
        s.notes.push(format!(
            "the acquisition date is written in words in the PC's language ({}); it is in images[].extra.imager.date_text, not acquired_at",
            date.as_deref().unwrap_or_default()
        ));
    }
    Some(s)
}

/// The technique an imaging mode names: chemiluminescence and near-infrared dyes image western
/// blots, stains and gel documentation image electrophoresis gels; anything else is not named.
pub(crate) fn technique_of(mode: &str) -> Option<&'static str> {
    let a = mode.to_ascii_lowercase();
    if a.contains("chemi") || a.contains("ecl") || a.contains("irdye") {
        Some("OBI:0000854")
    } else if ["gel", "coomassie", "stain", "sybr", "ethidium"]
        .iter()
        .any(|k| a.contains(k))
    {
        Some("CHMO:0001021")
    } else {
        None
    }
}

impl ImagerSettings {
    /// The experiment facts that `info` does not carry: serial number, method parameters, the
    /// technique of the measurement.
    pub(crate) fn experiment(&self, images: &[openreadout_core::model::ImageInfo]) -> Experiment {
        let mut exp = Experiment::default();
        let origin = |from: &str| Origin {
            source: Source::Inferred,
            from: from.to_string(),
        };
        let mut ins = ExperimentInstrument::default();
        if let Some(v) = &self.vendor {
            ins.vendor = Some(v.clone());
            if let Some(f) = self.origins.get("vendor") {
                exp.provenance.insert("instrument.vendor".into(), origin(f));
            }
        }
        if let Some(v) = &self.model {
            ins.model = Some(v.clone());
            if let Some(f) = self.origins.get("model") {
                exp.provenance.insert("instrument.model".into(), origin(f));
            }
        }
        if let Some(v) = &self.serial {
            ins.serial = Some(v.clone());
            if let Some(f) = self.origins.get("serial") {
                exp.provenance.insert("instrument.serial".into(), origin(f));
            }
        }
        if ins != ExperimentInstrument::default() {
            exp.instrument = Some(ins);
        }
        if let Some(t) = &self.acquired_at {
            exp.acquisition = Some(Acquisition {
                started_at: Some(t.clone()),
                ..Acquisition::default()
            });
            if let Some(f) = self.origins.get("acquired_at") {
                exp.provenance
                    .insert("acquisition.started_at".into(), origin(f));
            }
        }
        let mut method = Method::default();
        for p in &self.params {
            let q = match (p.unit, p.value.as_f64()) {
                (Some(u), Some(v)) => Quantity::number(v, u),
                _ => Quantity::plain(p.value.clone()),
            };
            method.parameters.insert(p.name.into(), q);
            exp.provenance
                .insert(format!("method.parameters.{}", p.name), origin(&p.from));
        }
        // The TIFF family is microscopy, so the derived technique would be light microscopy:
        // a mode that names the technique replaces it.
        if let Some(term) = self.mode.as_deref().and_then(technique_of) {
            method.technique = vocab::term(term);
            exp.provenance.insert(
                "method.technique".into(),
                origin("the imaging mode the file records"),
            );
            if term == "OBI:0000854" {
                method.assay = vocab::term(term);
                exp.provenance.insert(
                    "method.assay".into(),
                    origin("the imaging mode the file records"),
                );
            }
        }
        if method != Method::default() {
            exp.method = Some(method);
        }
        if let (Some(mode), Some(first)) = (&self.mode, images.first())
            && let Some(term) = technique_of(mode)
        {
            exp.measurements.push(Measurement {
                kind: MeasurementKind::Image,
                indices: images.iter().map(|i| i.index).collect(),
                what: format!(
                    "gel or blot image ({mode}), {} × {} pixels, {}",
                    first.size_x,
                    first.size_y,
                    first.pixel_type.ome_name()
                ),
                technique: vocab::term(term),
                terms: Vec::new(),
                parameters: BTreeMap::new(),
            });
            exp.provenance.insert(
                "measurements[0]".into(),
                origin("the imaging mode the file records"),
            );
        }
        exp
    }
}

impl TiffDataset {
    /// The raw bytes of a field of page `page` of the main file, read again from the file (the
    /// container decodes RATIONAL and ASCII values; these tags need the bytes as stored).
    pub(crate) fn raw_field(&mut self, page: usize, tag: u16, limit: u64) -> Option<Vec<u8>> {
        let (t, src) = self.files.first_mut()?.opened.as_mut()?;
        let f = t.ifds.get(page)?.field(tag)?;
        let size = crate::container::type_size(f.field_type)?.checked_mul(f.count)?;
        let at = f.value_offset?;
        src.read_at(at, size.min(limit)).ok()
    }

    /// Uvitec/Vilber settings when page 0 carries a `_GLP_BPL_` block.
    pub(crate) fn glp_settings(&mut self, page0: &Ifd) -> Option<ImagerSettings> {
        page0.field(GLP_TAG)?;
        let block = self.raw_field(0, GLP_TAG, MAX_GLP_BYTES)?;
        glp(&block)
    }

    /// Lays imager settings over the images built from the pages: instrument, channel name and
    /// exposure, acquisition time, `extra.imager`, notes. Stored values are left as they are.
    pub(crate) fn apply_imager(&mut self, page0: &Ifd, s: ImagerSettings) {
        let white_is_zero = page0.uint(tags::PHOTOMETRIC) == Some(0);
        for series in &mut self.series {
            let info = &mut series.info;
            let tag_inst = info.instrument.take().unwrap_or_default();
            info.instrument = Some(InstrumentInfo {
                manufacturer: s.vendor.clone().or(tag_inst.manufacturer),
                model: s.model.clone().or(tag_inst.model),
                software: s.software.clone().or(if s.extra.contains_key("saved_by") {
                    None
                } else {
                    tag_inst.software
                }),
                software_version: s.software_version.clone().or(tag_inst.software_version),
                detector: s.detector.clone().or(tag_inst.detector),
            });
            if s.replaces_time {
                info.acquired_at.clone_from(&s.acquired_at);
            }
            if info.size_c == 1
                && let Some(ch) = info.channels.first_mut()
            {
                if s.mode.is_some() {
                    ch.name.clone_from(&s.mode);
                }
                if let Some(t) = s.exposure_s {
                    ch.exposure_ms = Some(t * 1000.0);
                }
            }
            if !s.extra.is_empty() {
                let mut e = s.extra.clone();
                if white_is_zero {
                    e.insert("white_is_zero".into(), json!(true));
                }
                info.extra.insert("imager".into(), Value::Object(e));
            }
        }
        if white_is_zero {
            self.notes.push("stored with PhotometricInterpretation WhiteIsZero: values are as stored, and viewers that follow the tag show high values dark (dark bands on a light background)".into());
        }
        self.notes.extend(s.notes.iter().cloned());
        self.set_provenance(&[
            ("images[].instrument", Source::Inferred),
            ("images[].channels[].name", Source::Inferred),
            ("images[].channels[].exposure_ms", Source::Inferred),
            ("images[].acquired_at", Source::Inferred),
            ("images[].extra.imager", Source::Inferred),
        ]);
        self.imager = Some(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(duration_s("5.0 s"), Some(5.0));
        assert_eq!(duration_s("10sec"), Some(10.0));
        assert_eq!(duration_s("0 min 11 sec 200 msec"), Some(11.2));
        assert!((duration_s("0.46 msec").unwrap() - 0.00046).abs() < 1e-12);
        assert_eq!(duration_s("Automatic"), None);
        assert_eq!(duration_s("RGB Auto"), None);
        assert_eq!(duration_s("12 parsecs"), None);
        assert_eq!(leading_number("-24.55 °C USB3"), Some(-24.55));
        assert_eq!(leading_number("x"), None);
    }

    #[test]
    fn amersham_lines() {
        let kv = key_values(
            "Comment = \r\nMethod = Chemiluminescence\r\nExposure time = 5.0 s\r\nDark offset = 1000\r\nno equals here",
        );
        assert_eq!(kv.len(), 4);
        assert_eq!(kv[2], ("Exposure time".into(), "5.0 s".into()));
        assert_eq!(
            version_of("Amersham Imager 680 operation software Ver.2.0.0").as_deref(),
            Some("2.0.0")
        );
        assert_eq!(
            version_of("Amersham ImageQuant 800 Control Software V2.0.0").as_deref(),
            Some("2.0.0")
        );
        assert_eq!(technique_of("Chemiluminescence"), Some("OBI:0000854"));
        assert_eq!(technique_of("Gel documentation"), Some("CHMO:0001021"));
        assert_eq!(technique_of("Colorimetric"), None);
    }

    fn pstr(out: &mut Vec<u8>, s: &str) {
        out.push(s.len() as u8);
        out.extend_from_slice(s.as_bytes());
    }

    #[test]
    fn glp_block() {
        let mut b = GLP_MAGIC.to_vec();
        b.extend_from_slice(&[7, 0, 0, 0, 1, 0, 0]);
        pstr(&mut b, "Thursday, 13 February 2020");
        pstr(&mut b, "02:00:38");
        pstr(&mut b, "C:\\Images\\blot.Tif");
        pstr(&mut b, "Alliance  Q9 Mini 18.02 - SN ");
        b.extend_from_slice(&[0, 1, 1, 0]);
        pstr(&mut b, "0 min 11 sec 200 msec (");
        b.extend_from_slice(&[0, 0, 0, 2, 0]);
        pstr(&mut b, "Q45191");
        b.extend_from_slice(&[3, 0, 0]);
        pstr(&mut b, "C:\\ProgramData\\UVITec\\Blue.prt");
        b.extend_from_slice(&[1, 0, 0]);
        pstr(&mut b, "7: Chemiluminescence (-20)");
        pstr(&mut b, "-24.55 \u{b0}C");
        b.extend_from_slice(GLP_MAGIC);
        pstr(&mut b, "Friday, 14 February 2020");
        let s = glp(&b).unwrap();
        assert_eq!(s.vendor.as_deref(), Some("Uvitec"));
        assert_eq!(s.model.as_deref(), Some("Alliance Q9 Mini"));
        assert_eq!(s.software_version.as_deref(), Some("18.02"));
        assert_eq!(s.serial.as_deref(), Some("Q45191"));
        assert_eq!(s.exposure_s, Some(11.2));
        assert_eq!(s.mode.as_deref(), Some("Chemiluminescence"));
        assert_eq!(s.acquired_at.as_deref(), Some("2020-02-13T02:00:38"));
        assert_eq!(s.extra["ccd_temperature_c"], json!(-24.55));
        // every truncation of the block parses without panicking
        for n in 0..b.len() {
            let _ = glp(&b[..n]);
        }
        assert!(glp(b"not a block").is_none());
        assert_eq!(glp_datetime("czwartek, 26 stycznia 2023", "13:08:13"), None);
    }
}
