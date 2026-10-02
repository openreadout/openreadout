//! OME-XML (schema 2016-06) document builder for the `ImageDescription` tag.
//! Spec: <https://www.openmicroscopy.org/Schemas/OME/2016-06> (open standard).
//!
//! Which normalized field becomes which OME element is listed in
//! `book/src/guides/metadata.md` (§ OME-XML export). In short: `Experiment`,
//! `Experimenter`, one `Instrument` per distinct optical setup (`Microscope`, light
//! sources, `Detector`s, `Objective`), then per image `AcquisitionDate`, the references,
//! `ObjectiveSettings`, `StageLabel`, `Pixels` (channels with light-source and detector
//! settings, `TiffData`, one `Plane` per written plane) and `ROIRef`s; `ROI`s close the document.

use std::collections::HashMap;

use openreadout_core::PixelType;
use openreadout_core::acquisition_mode;
use openreadout_core::model::{ChannelInfo, FileInfo, ImageInfo, ObjectiveInfo};
use openreadout_tiff::ome::NORMALIZED_NS;
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use serde_json::{Map, Value};

/// XML namespace of the OME 2016-06 schema.
pub const OME_NS: &str = "http://www.openmicroscopy.org/Schemas/OME/2016-06";
/// Location of the OME 2016-06 XML Schema document.
pub const OME_XSD: &str = "http://www.openmicroscopy.org/Schemas/OME/2016-06/ome.xsd";

/// A plane's (c, z, t).
type Czt = (u32, u32, u32);

/// Which planes of an image are written and where their IFDs start.
#[derive(Debug, Clone)]
pub struct WrittenImage<'a> {
    /// The image's normalized metadata.
    pub info: &'a ImageInfo,
    /// Zero-based IFD index of the first plane of this image.
    pub first_ifd: u32,
    /// Selected (c, z, t) in the order the IFDs were written, in the exported image's own
    /// index space. Empty for metadata-only documents describing whole images.
    pub planes: Vec<(u32, u32, u32)>,
    /// Effective sizes after selection.
    pub size_c: u32,
    /// Number of Z planes written.
    pub size_z: u32,
    /// Number of time points written.
    pub size_t: u32,
    /// Map from selected index → original index, per axis (for channel metadata).
    pub c_map: Vec<u32>,
    /// The source image's (c, z, t) for each entry of `planes`; empty means "same as `planes`".
    /// Used to look up per-plane metadata and to scale physical steps of strided selections.
    pub source_planes: Vec<(u32, u32, u32)>,
    /// Per-frame records of this image from `Dataset::frames`, in the record vocabulary of
    /// `book/src/guides/metadata.md` (`t`, `z`, optional `c`, `time_ms`, `exposure_ms`,
    /// `stage_x_um`, …). They become `Plane` attributes. May be empty.
    pub frames: Vec<Value>,
}

impl<'a> WrittenImage<'a> {
    /// Every plane of `info`, in XYCZT order, without per-frame records.
    pub fn whole(info: &'a ImageInfo) -> Self {
        WrittenImage {
            info,
            first_ifd: 0,
            planes: Vec::new(),
            size_c: info.size_c,
            size_z: info.size_z,
            size_t: info.size_t,
            c_map: (0..info.size_c).collect(),
            source_planes: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// (exported index, source index) of every plane the document describes.
    fn plane_pairs(&self) -> Vec<(Czt, Czt)> {
        if !self.planes.is_empty() {
            return self
                .planes
                .iter()
                .enumerate()
                .map(|(n, &p)| (p, self.source_planes.get(n).copied().unwrap_or(p)))
                .collect();
        }
        // Metadata-only description of a whole image (possibly with a channel subset).
        if self.size_z != self.info.size_z || self.size_t != self.info.size_t {
            return Vec::new();
        }
        let mut out = Vec::new();
        for t in 0..self.size_t {
            for z in 0..self.size_z {
                for (ci, &c) in self.c_map.iter().enumerate() {
                    out.push(((ci as u32, z, t), (c, z, t)));
                }
            }
        }
        out
    }
}

fn ome_pixel_type(p: PixelType) -> &'static str {
    p.ome_name()
}

/// OME-XML physical sizes are `PositiveFloat`; readers sometimes report 0 for an axis of
/// length 1, which would make the document invalid, so such values are left out.
fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite() && *x > 0.0)
}

fn fmt_f64(v: f64) -> String {
    // Shortest round-trip representation, no exponent surprises for XSD `float`.
    let s = format!("{v}");
    if s.contains('e') {
        format!("{v:.9}")
    } else {
        s
    }
}

// ---------- small writer helpers ----------

type W = Writer<Vec<u8>>;
type R = Result<(), String>;

fn start(w: &mut W, el: BytesStart<'_>) -> R {
    w.write_event(Event::Start(el)).map_err(|e| e.to_string())
}
fn empty(w: &mut W, el: BytesStart<'_>) -> R {
    w.write_event(Event::Empty(el)).map_err(|e| e.to_string())
}
fn end(w: &mut W, name: &str) -> R {
    w.write_event(Event::End(BytesEnd::new(name)))
        .map_err(|e| e.to_string())
}
/// `s` without the characters XML 1.0 does not allow (C0 controls other than tab, newline and
/// carriage return; U+FFFE, U+FFFF): vendor metadata can contain them, and a document holding
/// one does not parse.
fn xml_chars(s: &str) -> std::borrow::Cow<'_, str> {
    let bad = |c: char| {
        (c < ' ' && !matches!(c, '\t' | '\n' | '\r')) || matches!(c, '\u{FFFE}' | '\u{FFFF}')
    };
    if s.contains(bad) {
        s.replace(bad, "").into()
    } else {
        s.into()
    }
}
fn text_el(w: &mut W, name: &str, text: &str) -> R {
    start(w, BytesStart::new(name))?;
    w.write_event(Event::Text(BytesText::new(&xml_chars(text))))
        .map_err(|e| e.to_string())?;
    end(w, name)
}
fn attr(el: &mut BytesStart<'_>, k: &str, v: impl AsRef<str>) {
    el.push_attribute((k, xml_chars(v.as_ref()).as_ref()));
}
fn attr_f(el: &mut BytesStart<'_>, k: &str, v: f64) {
    el.push_attribute((k, fmt_f64(v).as_str()));
}

// ---------- per-plane metadata ----------

/// Per-plane acquisition metadata written as `Plane` attributes.
#[derive(Debug, Clone, Default, PartialEq)]
struct PlaneMeta {
    delta_t_s: Option<f64>,
    exposure_s: Option<f64>,
    position_um: [Option<f64>; 3],
}

impl PlaneMeta {
    fn is_empty(&self) -> bool {
        self.delta_t_s.is_none()
            && self.exposure_s.is_none()
            && self.position_um.iter().all(Option::is_none)
    }
}

fn idx(r: &Map<String, Value>, k: &str) -> Option<u32> {
    r.get(k)
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
}

fn num(r: &Map<String, Value>, k: &str) -> Option<f64> {
    r.get(k).and_then(Value::as_f64).filter(|v| v.is_finite())
}

/// Per-frame records indexed by (c, z, t); a record without `c` covers every channel of its
/// (z, t) (channels acquired together in one frame, as in ND2).
struct FrameIndex<'a> {
    by_czt: HashMap<(u32, u32, u32), &'a Map<String, Value>>,
    by_zt: HashMap<(u32, u32), &'a Map<String, Value>>,
}

impl<'a> FrameIndex<'a> {
    fn new(records: &'a [Value]) -> Self {
        let mut by_czt = HashMap::new();
        let mut by_zt = HashMap::new();
        for r in records.iter().filter_map(Value::as_object) {
            let z = idx(r, "z").unwrap_or(0);
            let t = idx(r, "t").unwrap_or(0);
            match idx(r, "c") {
                Some(c) => {
                    by_czt.entry((c, z, t)).or_insert(r);
                }
                None => {
                    by_zt.entry((z, t)).or_insert(r);
                }
            }
        }
        FrameIndex { by_czt, by_zt }
    }

    fn get(&self, c: u32, z: u32, t: u32) -> Option<&'a Map<String, Value>> {
        self.by_czt
            .get(&(c, z, t))
            .or_else(|| self.by_zt.get(&(z, t)))
            .copied()
    }
}

/// Metadata of source plane (c, t) of `im` from its record (if any), falling back to the
/// channel's exposure and to `time_increment_s × t`.
fn plane_meta(im: &ImageInfo, rec: Option<&Map<String, Value>>, c: u32, t: u32) -> PlaneMeta {
    let ch = im.channels.iter().find(|x| x.index == c);
    let delta_t_s = rec
        .and_then(|r| num(r, "delta_t_s").or_else(|| num(r, "time_ms").map(|m| m / 1000.0)))
        .or_else(|| positive(im.time_increment_s).map(|i| i * f64::from(t)));
    let exposure_ms = rec
        .and_then(|r| r.get("exposure_ms_per_channel"))
        .and_then(|a| a.get(c as usize))
        .and_then(Value::as_f64)
        .or_else(|| ch.and_then(|c| c.exposure_ms))
        .or_else(|| {
            rec.filter(|r| r.contains_key("c") || im.size_c == 1)
                .and_then(|r| num(r, "exposure_ms"))
        })
        .filter(|e| e.is_finite() && *e >= 0.0);
    let position_um =
        ["stage_x_um", "stage_y_um", "stage_z_um"].map(|k| rec.and_then(|r| num(r, k)));
    PlaneMeta {
        delta_t_s: delta_t_s.filter(|d| d.is_finite()),
        exposure_s: exposure_ms.map(|e| e / 1000.0),
        position_um,
    }
}

/// Step between the sorted distinct `values` when evenly spaced (1 for a single value).
fn regular_step(mut values: Vec<u32>) -> Option<u32> {
    values.sort_unstable();
    values.dedup();
    let step = match values.as_slice() {
        [] | [_] => return Some(1),
        [a, b, ..] => b - a,
    };
    values
        .windows(2)
        .all(|p| p[1] - p[0] == step)
        .then_some(step)
}

// ---------- instrument ----------

/// A light source inferred from a channel's excitation wavelength.
#[derive(Debug, Clone, PartialEq)]
struct LightSource {
    wavelength_nm: f64,
    laser: bool,
}

/// Everything an `Instrument` element holds. Images with equal specs share one instrument.
#[derive(Debug, Clone, Default, PartialEq)]
struct InstrumentSpec {
    manufacturer: Option<String>,
    model: Option<String>,
    light_sources: Vec<LightSource>,
    detectors: Vec<String>,
    objective: Option<ObjectiveInfo>,
    /// Detection bands `[start, end]` nm, one emission `Filter` each.
    filters: Vec<[f64; 2]>,
}

impl InstrumentSpec {
    fn is_empty(&self) -> bool {
        self.manufacturer.is_none()
            && self.model.is_none()
            && self.light_sources.is_empty()
            && self.detectors.is_empty()
            && self.objective.is_none()
            && self.filters.is_empty()
    }
}

/// Per-channel settings from `extra.channel_settings[]` (shared vocabulary: `index`,
/// `detector`, `binning`, `gain`).
#[derive(Debug, Clone, Default)]
struct ChannelLinks {
    light_source: Option<usize>,
    wavelength_nm: Option<f64>,
    detector: Option<usize>,
    binning: Option<&'static str>,
    gain: Option<f64>,
    emission_filter: Option<usize>,
}

/// A channel's detection band `[start, end]` in nm (`emission_band_*_nm`, else
/// `emission_range_nm`), when both edges are positive and ordered.
fn channel_band(ch: &ChannelInfo) -> Option<[f64; 2]> {
    let [a, b] = match (ch.emission_band_start_nm, ch.emission_band_end_nm) {
        (Some(a), Some(b)) => [a, b],
        _ => ch.emission_range_nm?,
    };
    let (a, b) = (a.min(b), a.max(b));
    (a.is_finite() && b.is_finite() && a > 0.0).then_some([a, b])
}

fn is_laser_mode(ch: &ChannelInfo) -> bool {
    let m = ch
        .acquisition_mode
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    [
        "laser",
        "confocal",
        "multiphoton",
        "multi-photon",
        "two-photon",
        "tirf",
        "sted",
    ]
    .iter()
    .any(|k| m.contains(k))
        && !m.contains("spinning")
}

fn channel_setting(im: &ImageInfo, c: u32) -> Option<&Map<String, Value>> {
    im.extra
        .get("channel_settings")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(Value::as_object)
        .find(|s| idx(s, "index") == Some(c))
}

fn ome_binning(s: &str) -> Option<&'static str> {
    Some(match s.trim() {
        "1x1" => "1x1",
        "2x2" => "2x2",
        "4x4" => "4x4",
        "8x8" => "8x8",
        _ => return None,
    })
}

/// The instrument this image was acquired on, and how each written channel links to it.
fn instrument_for(wi: &WrittenImage<'_>) -> (InstrumentSpec, Vec<ChannelLinks>) {
    let im = wi.info;
    let mut spec = InstrumentSpec {
        objective: im.objective.clone(),
        ..InstrumentSpec::default()
    };
    if let Some(ins) = &im.instrument {
        spec.manufacturer.clone_from(&ins.manufacturer);
        spec.model.clone_from(&ins.model);
        // The detector named on `instrument` comes first: readers take the first `Detector`
        // as the instrument's detector.
        if let Some(d) = ins.detector.clone().filter(|d| !d.trim().is_empty()) {
            spec.detectors.push(d);
        }
    }
    let mut links = Vec::new();
    for &orig in &wi.c_map {
        let mut l = ChannelLinks::default();
        let ch = im.channels.iter().find(|c| c.index == orig);
        if let Some(ch) = ch
            && let Some(wl) = positive(ch.excitation_nm)
        {
            let src = LightSource {
                wavelength_nm: wl,
                laser: is_laser_mode(ch),
            };
            let k = spec
                .light_sources
                .iter()
                .position(|s| *s == src)
                .unwrap_or_else(|| {
                    spec.light_sources.push(src);
                    spec.light_sources.len() - 1
                });
            l.light_source = Some(k);
            l.wavelength_nm = Some(wl);
        }
        if let Some(band) = ch.and_then(channel_band) {
            let k = spec
                .filters
                .iter()
                .position(|f| f.map(f64::to_bits) == band.map(f64::to_bits))
                .unwrap_or_else(|| {
                    spec.filters.push(band);
                    spec.filters.len() - 1
                });
            l.emission_filter = Some(k);
        }
        let setting = channel_setting(im, orig);
        let det = setting
            .and_then(|s| s.get("detector"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| im.instrument.as_ref().and_then(|i| i.detector.clone()))
            .filter(|d| !d.trim().is_empty());
        if let Some(d) = det {
            let k = spec
                .detectors
                .iter()
                .position(|x| *x == d)
                .unwrap_or_else(|| {
                    spec.detectors.push(d);
                    spec.detectors.len() - 1
                });
            l.detector = Some(k);
            l.binning = setting
                .and_then(|s| s.get("binning"))
                .and_then(Value::as_str)
                .and_then(ome_binning);
            l.gain = setting.and_then(|s| num(s, "gain"));
        }
        links.push(l);
    }
    (spec, links)
}

fn ome_immersion(v: &str) -> &'static str {
    match v.to_ascii_lowercase().as_str() {
        "oil" => "Oil",
        "water" | "water dipping" | "dipping" => "Water",
        "air" | "dry" => "Air",
        "glycerol" | "glycerin" | "glycerine" => "Glycerol",
        "multi" => "Multi",
        _ => "Other",
    }
}

/// `ObjectiveSettings/@Medium` has no `Multi`.
fn ome_medium(v: &str) -> &'static str {
    match ome_immersion(v) {
        "Multi" => "Other",
        m => m,
    }
}

fn write_instrument(w: &mut W, id: usize, spec: &InstrumentSpec) -> R {
    let mut inst = BytesStart::new("Instrument");
    attr(&mut inst, "ID", format!("Instrument:{id}"));
    start(w, inst)?;
    if spec.manufacturer.is_some() || spec.model.is_some() {
        let mut m = BytesStart::new("Microscope");
        if let Some(v) = &spec.manufacturer {
            attr(&mut m, "Manufacturer", v);
        }
        if let Some(v) = &spec.model {
            attr(&mut m, "Model", v);
        }
        empty(w, m)?;
    }
    for (k, s) in spec.light_sources.iter().enumerate() {
        let mut e = BytesStart::new(if s.laser {
            "Laser"
        } else {
            "GenericExcitationSource"
        });
        attr(&mut e, "ID", format!("LightSource:{id}:{k}"));
        if s.laser {
            attr_f(&mut e, "Wavelength", s.wavelength_nm);
            attr(&mut e, "WavelengthUnit", "nm");
        }
        empty(w, e)?;
    }
    for (k, d) in spec.detectors.iter().enumerate() {
        let mut det = BytesStart::new("Detector");
        attr(&mut det, "ID", format!("Detector:{id}:{k}"));
        attr(&mut det, "Model", d);
        empty(w, det)?;
    }
    if let Some(o) = &spec.objective {
        let mut obj = BytesStart::new("Objective");
        attr(&mut obj, "ID", format!("Objective:{id}:0"));
        if let Some(v) = &o.model {
            attr(&mut obj, "Model", v);
        }
        if let Some(v) = &o.immersion {
            attr(&mut obj, "Immersion", ome_immersion(v));
        }
        if let Some(v) = o.lens_na.filter(|v| v.is_finite()) {
            attr_f(&mut obj, "LensNA", v);
        }
        if let Some(v) = o.nominal_magnification.filter(|v| v.is_finite()) {
            attr_f(&mut obj, "NominalMagnification", v);
        }
        empty(w, obj)?;
    }
    // Detection bands as emission filters: the pass band is what the channel's detector saw
    // (a filter or a spectral detector window). No `Type`: which of the two is not known.
    for (k, [a, b]) in spec.filters.iter().enumerate() {
        let mut f = BytesStart::new("Filter");
        attr(&mut f, "ID", format!("Filter:{id}:{k}"));
        start(w, f)?;
        let mut r = BytesStart::new("TransmittanceRange");
        attr_f(&mut r, "CutIn", *a);
        attr(&mut r, "CutInUnit", "nm");
        attr_f(&mut r, "CutOut", *b);
        attr(&mut r, "CutOutUnit", "nm");
        empty(w, r)?;
        end(w, "Filter")?;
    }
    end(w, "Instrument")
}

// ---------- experimenter / experiment ----------

/// `Experimenter` attributes from `extra.experimenter`: a string (a user name) or an object
/// with `user_name`, `first_name`, `middle_name`, `last_name`, `email`, `institution`.
fn experimenter_attrs(v: &Value) -> Vec<(&'static str, String)> {
    let s = |x: &Value| {
        x.as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    match v {
        Value::String(_) => s(v).map(|u| vec![("UserName", u)]).unwrap_or_default(),
        Value::Object(o) => [
            ("first_name", "FirstName"),
            ("middle_name", "MiddleName"),
            ("last_name", "LastName"),
            ("email", "Email"),
            ("institution", "Institution"),
            ("user_name", "UserName"),
        ]
        .iter()
        .filter_map(|(k, a)| o.get(*k).and_then(s).map(|x| (*a, x)))
        .collect(),
        _ => Vec::new(),
    }
}

const EXPERIMENT_TYPES: &[&str] = &[
    "FP",
    "FRET",
    "TimeLapse",
    "FourDPlus",
    "Screen",
    "Immunocytochemistry",
    "Immunofluorescence",
    "FISH",
    "Electrophysiology",
    "IonImaging",
    "Colocalization",
    "PGIDocumentation",
    "FluorescenceLifetime",
    "SpectralImaging",
    "Photobleaching",
    "SPIM",
    "Other",
];

/// (`Type`, `Description`) from `extra.experiment`: a string (a description) or an object
/// with `type` (one of the OME experiment types, case and punctuation ignored) and `description`.
fn experiment_parts(v: &Value) -> (Option<&'static str>, Option<String>) {
    let norm = |s: &str| {
        s.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    match v {
        Value::String(d) if !d.trim().is_empty() => (None, Some(d.trim().to_string())),
        Value::Object(o) => {
            let ty = o.get("type").and_then(Value::as_str).and_then(|t| {
                EXPERIMENT_TYPES
                    .iter()
                    .find(|e| norm(e) == norm(t))
                    .copied()
            });
            let d = o
                .get("description")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            (ty, d)
        }
        _ => (None, None),
    }
}

// ---------- ROIs ----------

fn f64s(v: Option<&Value>) -> Vec<Option<f64>> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// One OME shape from a normalized ROI (`extra.rois[]`: `shape`, `color`, `keyframes[0]`
/// with `center`, `box_size`, `base_points`). Coordinates are copied as stored.
#[allow(clippy::many_single_char_names)]
fn roi_shape(roi: &Map<String, Value>) -> Option<(&'static str, Vec<(&'static str, String)>)> {
    let kf = roi
        .get("keyframes")
        .and_then(Value::as_array)
        .and_then(|k| k.first())?;
    let center = f64s(kf.get("center"));
    let size = f64s(kf.get("box_size"));
    let pts: Vec<(f64, f64)> = kf
        .get("base_points")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let p = f64s(Some(p));
                    Some(((*p.first()?)?, (*p.get(1)?)?))
                })
                .collect()
        })
        .unwrap_or_default();
    let (cx, cy) = (
        center.first().copied().flatten(),
        center.get(1).copied().flatten(),
    );
    let (sw, sh) = (
        size.first().copied().flatten(),
        size.get(1).copied().flatten(),
    );
    let f = fmt_f64;
    let points = || {
        pts.iter()
            .map(|(x, y)| format!("{},{}", f(*x), f(*y)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let shape = roi.get("shape").and_then(Value::as_str).unwrap_or("any");
    Some(match (shape, cx, cy, sw, sh) {
        ("rectangle" | "square", Some(x), Some(y), Some(w), Some(h)) => (
            "Rectangle",
            vec![
                ("X", f(x - w / 2.0)),
                ("Y", f(y - h / 2.0)),
                ("Width", f(w)),
                ("Height", f(h)),
            ],
        ),
        ("ellipse" | "circle", Some(x), Some(y), Some(w), Some(h)) => (
            "Ellipse",
            vec![
                ("X", f(x)),
                ("Y", f(y)),
                ("RadiusX", f(w / 2.0)),
                ("RadiusY", f(h / 2.0)),
            ],
        ),
        ("line", ..) if pts.len() >= 2 => (
            "Line",
            vec![
                ("X1", f(pts[0].0)),
                ("Y1", f(pts[0].1)),
                ("X2", f(pts[1].0)),
                ("Y2", f(pts[1].1)),
            ],
        ),
        ("polyline", ..) if pts.len() >= 2 => ("Polyline", vec![("Points", points())]),
        ("polygon" | "bezier" | "spiral" | "ring" | "raster" | "any", ..) if pts.len() >= 3 => {
            ("Polygon", vec![("Points", points())])
        }
        (_, Some(x), Some(y), ..) => ("Point", vec![("X", f(x)), ("Y", f(y))]),
        _ => return None,
    })
}

/// ROIs of one exported image that have geometry, as (`ROI` ID, name, shape).
type RoiOut = (
    String,
    Option<String>,
    Option<i32>,
    &'static str,
    Vec<(&'static str, String)>,
);

fn rois_of(im: &ImageInfo, image_id: usize) -> Vec<RoiOut> {
    let Some(rois) = im.extra.get("rois").and_then(Value::as_array) else {
        return Vec::new();
    };
    rois.iter()
        .filter_map(Value::as_object)
        .filter_map(roi_shape_with_meta)
        .enumerate()
        .map(|(k, (name, color, el, attrs))| {
            (format!("ROI:{image_id}:{k}"), name, color, el, attrs)
        })
        .collect()
}

#[allow(clippy::type_complexity)]
fn roi_shape_with_meta(
    roi: &Map<String, Value>,
) -> Option<(
    Option<String>,
    Option<i32>,
    &'static str,
    Vec<(&'static str, String)>,
)> {
    let (el, attrs) = roi_shape(roi)?;
    let name = roi
        .get("label")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            roi.get("id")
                .and_then(Value::as_i64)
                .map(|i| format!("ROI {i}"))
        });
    let color = roi.get("color").and_then(Value::as_str).and_then(ome_color);
    Some((name, color, el, attrs))
}

// ---------- the document ----------

/// Build the OME-XML document. `creator` goes into the `Creator` attribute.
pub fn build_ome_xml(
    file: &FileInfo,
    images: &[WrittenImage<'_>],
    creator: &str,
    vendor_json: Option<&str>,
) -> Result<String, String> {
    build(file, images, creator, vendor_json, true)
}

/// Build an OME-XML document that describes the images but not where their pixels live:
/// each `Pixels` element carries `<MetadataOnly/>` instead of `TiffData`. This is the form
/// an OME-Zarr collection stores as `OME/METADATA.ome.xml`. `first_ifd` is ignored.
pub fn build_ome_xml_metadata_only(
    file: &FileInfo,
    images: &[WrittenImage<'_>],
    creator: &str,
    vendor_json: Option<&str>,
) -> Result<String, String> {
    build(file, images, creator, vendor_json, false)
}

#[allow(clippy::too_many_lines)]
fn build(
    file: &FileInfo,
    images: &[WrittenImage<'_>],
    creator: &str,
    vendor_json: Option<&str>,
    tiff_data: bool,
) -> Result<String, String> {
    let mut w = Writer::new_with_indent(Vec::new(), b' ', 1);
    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .map_err(|x| x.to_string())?;
    let mut ome = BytesStart::new("OME");
    attr(&mut ome, "xmlns", OME_NS);
    attr(
        &mut ome,
        "xmlns:xsi",
        "http://www.w3.org/2001/XMLSchema-instance",
    );
    attr(
        &mut ome,
        "xsi:schemaLocation",
        format!("{OME_NS} {OME_XSD}"),
    );
    attr(&mut ome, "UUID", format!("urn:uuid:{}", pseudo_uuid(file)));
    attr(&mut ome, "Creator", creator);
    start(&mut w, ome)?;

    // Experiments and experimenters, deduplicated by content.
    let mut experiments: Vec<(Option<&'static str>, Option<String>)> = Vec::new();
    let mut experimenters: Vec<Vec<(&'static str, String)>> = Vec::new();
    let mut image_experiment = Vec::new();
    let mut image_experimenter = Vec::new();
    for wi in images {
        let ex = wi
            .info
            .extra
            .get("experiment")
            .map(experiment_parts)
            .filter(|(t, d)| t.is_some() || d.is_some());
        image_experiment.push(ex.map(|e| {
            experiments.iter().position(|x| *x == e).unwrap_or_else(|| {
                experiments.push(e);
                experiments.len() - 1
            })
        }));
        let who = wi
            .info
            .extra
            .get("experimenter")
            .map(experimenter_attrs)
            .filter(|a| !a.is_empty());
        image_experimenter.push(who.map(|a| {
            experimenters
                .iter()
                .position(|x| *x == a)
                .unwrap_or_else(|| {
                    experimenters.push(a);
                    experimenters.len() - 1
                })
        }));
    }
    // An experiment names its experimenter when every image of it agrees on one.
    for (k, (ty, desc)) in experiments.iter().enumerate() {
        let mut e = BytesStart::new("Experiment");
        attr(&mut e, "ID", format!("Experiment:{k}"));
        if let Some(t) = ty {
            attr(&mut e, "Type", t);
        }
        let who: Vec<usize> = image_experiment
            .iter()
            .zip(&image_experimenter)
            .filter(|(e, _)| **e == Some(k))
            .filter_map(|(_, p)| *p)
            .collect();
        let owner = who.first().copied().filter(|f| who.iter().all(|x| x == f));
        if desc.is_none() && owner.is_none() {
            empty(&mut w, e)?;
            continue;
        }
        start(&mut w, e)?;
        if let Some(d) = desc {
            text_el(&mut w, "Description", d)?;
        }
        if let Some(p) = owner {
            let mut r = BytesStart::new("ExperimenterRef");
            attr(&mut r, "ID", format!("Experimenter:{p}"));
            empty(&mut w, r)?;
        }
        end(&mut w, "Experiment")?;
    }
    for (k, attrs) in experimenters.iter().enumerate() {
        let mut e = BytesStart::new("Experimenter");
        attr(&mut e, "ID", format!("Experimenter:{k}"));
        for (a, v) in attrs {
            attr(&mut e, a, v);
        }
        empty(&mut w, e)?;
    }

    // Instruments, one per distinct optical setup.
    let mut instruments: Vec<InstrumentSpec> = Vec::new();
    let mut image_instrument = Vec::new();
    let mut image_links = Vec::new();
    for wi in images {
        let (spec, links) = instrument_for(wi);
        image_instrument.push((!spec.is_empty()).then(|| {
            instruments
                .iter()
                .position(|s| *s == spec)
                .unwrap_or_else(|| {
                    instruments.push(spec);
                    instruments.len() - 1
                })
        }));
        image_links.push(links);
    }
    for (k, spec) in instruments.iter().enumerate() {
        write_instrument(&mut w, k, spec)?;
    }

    let mut all_rois: Vec<RoiOut> = Vec::new();
    // Normalized fields OME cannot carry exactly: (annotation id, key/value pairs).
    let mut notes: Vec<(String, Vec<(&'static str, String)>)> = Vec::new();
    for (idx, wi) in images.iter().enumerate() {
        let im = wi.info;
        let mut image = BytesStart::new("Image");
        attr(&mut image, "ID", format!("Image:{idx}"));
        if let Some(n) = &im.name {
            attr(&mut image, "Name", n);
        }
        start(&mut w, image)?;
        let acquired = im.acquired_at.as_deref().and_then(xsd_datetime);
        if let Some(t) = &acquired {
            text_el(&mut w, "AcquisitionDate", t)?;
        }
        let image_note = image_normalized(im, acquired.as_deref());
        if let Some(p) = image_experimenter[idx] {
            let mut r = BytesStart::new("ExperimenterRef");
            attr(&mut r, "ID", format!("Experimenter:{p}"));
            empty(&mut w, r)?;
        }
        if let Some(e) = image_experiment[idx] {
            let mut r = BytesStart::new("ExperimentRef");
            attr(&mut r, "ID", format!("Experiment:{e}"));
            empty(&mut w, r)?;
        }
        let inst = image_instrument[idx];
        if let Some(k) = inst {
            let mut r = BytesStart::new("InstrumentRef");
            attr(&mut r, "ID", format!("Instrument:{k}"));
            empty(&mut w, r)?;
            if let Some(o) = &im.objective {
                let mut r = BytesStart::new("ObjectiveSettings");
                attr(&mut r, "ID", format!("Objective:{k}:0"));
                if let Some(m) = o.immersion.as_deref() {
                    attr(&mut r, "Medium", ome_medium(m));
                }
                if let Some(n) = im
                    .extra
                    .get("refractive_index")
                    .and_then(Value::as_f64)
                    .filter(|n| n.is_finite() && *n > 0.0)
                {
                    attr_f(&mut r, "RefractiveIndex", n);
                }
                empty(&mut w, r)?;
            }
        }
        let pairs = wi.plane_pairs();
        let index = FrameIndex::new(&wi.frames);
        if let Some(pos) = stage_position(im, &wi.frames) {
            let mut s = BytesStart::new("StageLabel");
            attr(
                &mut s,
                "Name",
                im.name
                    .clone()
                    .unwrap_or_else(|| format!("Position {}", im.index)),
            );
            for (k, v) in ["X", "Y", "Z"].iter().zip(pos) {
                if let Some(v) = v {
                    // nm, not µm: the only OME symbol for micrometres is non-ASCII, and
                    // Bio-Formats rejects its character reference in a TIFF description.
                    attr_f(&mut s, k, v * 1000.0);
                    attr(&mut s, &format!("{k}Unit"), "nm");
                }
            }
            empty(&mut w, s)?;
        }

        // Physical steps of strided selections: every 2nd Z slice is twice as far apart.
        let z_step = regular_step(pairs.iter().map(|p| p.1.1).collect());
        let t_step = regular_step(pairs.iter().map(|p| p.1.2).collect());
        let mut px = BytesStart::new("Pixels");
        attr(&mut px, "ID", format!("Pixels:{idx}"));
        attr(&mut px, "DimensionOrder", "XYCZT");
        attr(&mut px, "Type", ome_pixel_type(im.pixel_type));
        attr(&mut px, "SizeX", im.size_x.to_string());
        attr(&mut px, "SizeY", im.size_y.to_string());
        attr(&mut px, "SizeZ", wi.size_z.to_string());
        attr(
            &mut px,
            "SizeC",
            (wi.size_c * im.samples_per_pixel).to_string(),
        );
        attr(&mut px, "SizeT", wi.size_t.to_string());
        attr(
            &mut px,
            "Interleaved",
            if im.samples_per_pixel > 1 {
                "true"
            } else {
                "false"
            },
        );
        attr(&mut px, "BigEndian", "false");
        if let Some(v) = positive(im.physical_size.x) {
            attr_f(&mut px, "PhysicalSizeX", v);
        }
        if let Some(v) = positive(im.physical_size.y) {
            attr_f(&mut px, "PhysicalSizeY", v);
        }
        if let (Some(v), Some(k)) = (positive(im.physical_size.z), z_step) {
            attr_f(&mut px, "PhysicalSizeZ", v * f64::from(k));
        }
        if let (Some(v), Some(k)) = (positive(im.time_increment_s), t_step) {
            attr_f(&mut px, "TimeIncrement", v * f64::from(k));
            attr(&mut px, "TimeIncrementUnit", "s");
        }
        start(&mut w, px)?;
        let links = &image_links[idx];
        for (ci, &orig) in wi.c_map.iter().enumerate() {
            let ch = im.channels.iter().find(|c| c.index == orig);
            let mut c = BytesStart::new("Channel");
            attr(&mut c, "ID", format!("Channel:{idx}:{ci}"));
            if let Some(n) = ch.and_then(|c| c.name.as_deref()) {
                attr(&mut c, "Name", n);
            }
            attr(&mut c, "SamplesPerPixel", im.samples_per_pixel.to_string());
            let mode = ch.and_then(|c| c.acquisition_mode.as_deref());
            let (acq, contrast) = mode.map(acquisition_mode::to_ome).unwrap_or_default();
            if let Some(v) = acq {
                attr(&mut c, "AcquisitionMode", v);
            }
            if let Some(v) = contrast {
                attr(&mut c, "ContrastMethod", v);
            }
            // A label the enumerations do not carry is kept verbatim in an annotation.
            let channel_note = mode.filter(|m| !acquisition_mode::round_trips(m)).map(|m| {
                let id = format!("Annotation:Channel:{idx}:{ci}");
                notes.push((id.clone(), vec![("acquisition_mode", m.trim().to_string())]));
                id
            });
            if let Some(v) = positive(ch.and_then(|c| c.excitation_nm)) {
                attr_f(&mut c, "ExcitationWavelength", v);
                attr(&mut c, "ExcitationWavelengthUnit", "nm");
            }
            // Only a recorded emission wavelength: a detection band goes to an emission filter.
            if let Some(v) = positive(ch.and_then(|c| c.emission_nm)) {
                attr_f(&mut c, "EmissionWavelength", v);
                attr(&mut c, "EmissionWavelengthUnit", "nm");
            }
            if let Some(v) = ch.and_then(|c| c.fluorophore.as_deref()) {
                attr(&mut c, "Fluor", v);
            }
            if let Some(v) = ch.and_then(|c| c.color.as_deref()).and_then(ome_color) {
                attr(&mut c, "Color", v.to_string());
            }
            let link = links.get(ci).cloned().unwrap_or_default();
            let settings = inst.is_some()
                && (link.light_source.is_some()
                    || link.detector.is_some()
                    || link.emission_filter.is_some());
            if !settings && channel_note.is_none() {
                empty(&mut w, c)?;
                continue;
            }
            let k = inst.unwrap_or(0);
            start(&mut w, c)?;
            if let Some(s) = link.light_source {
                let mut e = BytesStart::new("LightSourceSettings");
                attr(&mut e, "ID", format!("LightSource:{k}:{s}"));
                if let Some(wl) = link.wavelength_nm {
                    attr_f(&mut e, "Wavelength", wl);
                    attr(&mut e, "WavelengthUnit", "nm");
                }
                empty(&mut w, e)?;
            }
            if let Some(d) = link.detector {
                let mut e = BytesStart::new("DetectorSettings");
                attr(&mut e, "ID", format!("Detector:{k}:{d}"));
                if let Some(g) = link.gain {
                    attr_f(&mut e, "Gain", g);
                }
                if let Some(b) = link.binning {
                    attr(&mut e, "Binning", b);
                }
                empty(&mut w, e)?;
            }
            if let Some(id) = &channel_note {
                let mut r = BytesStart::new("AnnotationRef");
                attr(&mut r, "ID", id);
                empty(&mut w, r)?;
            }
            if let Some(f) = link.emission_filter.filter(|_| settings) {
                start(&mut w, BytesStart::new("LightPath"))?;
                let mut r = BytesStart::new("EmissionFilterRef");
                attr(&mut r, "ID", format!("Filter:{k}:{f}"));
                empty(&mut w, r)?;
                end(&mut w, "LightPath")?;
            }
            end(&mut w, "Channel")?;
        }
        if !tiff_data {
            empty(&mut w, BytesStart::new("MetadataOnly"))?;
        }
        // One TiffData per plane, in write order.
        for (n, &(c, z, t)) in wi.planes.iter().enumerate().filter(|_| tiff_data) {
            let mut td = BytesStart::new("TiffData");
            attr(&mut td, "IFD", (wi.first_ifd as usize + n).to_string());
            attr(&mut td, "FirstC", c.to_string());
            attr(&mut td, "FirstZ", z.to_string());
            attr(&mut td, "FirstT", t.to_string());
            attr(&mut td, "PlaneCount", "1");
            empty(&mut w, td)?;
        }
        // One Plane per written plane when anything is known about the planes.
        let metas: Vec<PlaneMeta> = pairs
            .iter()
            .map(|&(_, (c, z, t))| plane_meta(im, index.get(c, z, t), c, t))
            .collect();
        if metas.iter().any(|m| !m.is_empty()) {
            for (&((c, z, t), _), m) in pairs.iter().zip(&metas) {
                let mut p = BytesStart::new("Plane");
                attr(&mut p, "TheZ", z.to_string());
                attr(&mut p, "TheT", t.to_string());
                attr(&mut p, "TheC", c.to_string());
                if let Some(v) = m.delta_t_s {
                    attr_f(&mut p, "DeltaT", v);
                    attr(&mut p, "DeltaTUnit", "s");
                }
                if let Some(v) = m.exposure_s {
                    attr_f(&mut p, "ExposureTime", v);
                    attr(&mut p, "ExposureTimeUnit", "s");
                }
                for (k, v) in ["PositionX", "PositionY", "PositionZ"]
                    .iter()
                    .zip(m.position_um)
                {
                    if let Some(v) = v {
                        attr_f(&mut p, k, v * 1000.0);
                        attr(&mut p, &format!("{k}Unit"), "nm");
                    }
                }
                empty(&mut w, p)?;
            }
        }
        end(&mut w, "Pixels")?;
        let rois = rois_of(im, idx);
        for r in &rois {
            let mut e = BytesStart::new("ROIRef");
            attr(&mut e, "ID", &r.0);
            empty(&mut w, e)?;
        }
        all_rois.extend(rois);
        if !image_note.is_empty() {
            let id = format!("Annotation:Image:{idx}");
            let mut r = BytesStart::new("AnnotationRef");
            attr(&mut r, "ID", &id);
            empty(&mut w, r)?;
            notes.push((id, image_note));
        }
        end(&mut w, "Image")?;
    }

    // Structured annotations: the normalized info and, optionally, the vendor block, as XML annotations.
    if images.iter().any(|i| !i.info.extra.is_empty()) || vendor_json.is_some() || !notes.is_empty()
    {
        start(&mut w, BytesStart::new("StructuredAnnotations"))?;
        for (id, pairs) in &notes {
            let mut ann = BytesStart::new("MapAnnotation");
            attr(&mut ann, "ID", id);
            attr(&mut ann, "Namespace", NORMALIZED_NS);
            start(&mut w, ann)?;
            start(&mut w, BytesStart::new("Value"))?;
            for (k, v) in pairs {
                let mut m = BytesStart::new("M");
                attr(&mut m, "K", *k);
                start(&mut w, m)?;
                w.write_event(Event::Text(BytesText::new(&xml_chars(v))))
                    .map_err(|x| x.to_string())?;
                end(&mut w, "M")?;
            }
            end(&mut w, "Value")?;
            end(&mut w, "MapAnnotation")?;
        }
        let mut ann = BytesStart::new("CommentAnnotation");
        attr(&mut ann, "ID", "Annotation:0");
        attr(&mut ann, "Namespace", "openreadout.dev/source");
        start(&mut w, ann)?;
        let summary = format!(
            "Converted by {creator} from {} ({}, {} bytes).",
            file.path, file.format.name, file.size_bytes
        );
        text_el(&mut w, "Value", &summary)?;
        end(&mut w, "CommentAnnotation")?;
        if let Some(v) = vendor_json {
            let mut ann = BytesStart::new("CommentAnnotation");
            attr(&mut ann, "ID", "Annotation:1");
            attr(
                &mut ann,
                "Namespace",
                "openreadout.dev/vendor-metadata-json",
            );
            start(&mut w, ann)?;
            text_el(&mut w, "Value", v)?;
            end(&mut w, "CommentAnnotation")?;
        }
        end(&mut w, "StructuredAnnotations")?;
    }

    for (id, name, color, el, attrs) in &all_rois {
        let mut roi = BytesStart::new("ROI");
        attr(&mut roi, "ID", id);
        if let Some(n) = name {
            attr(&mut roi, "Name", n);
        }
        start(&mut w, roi)?;
        start(&mut w, BytesStart::new("Union"))?;
        let mut s = BytesStart::new(*el);
        attr(&mut s, "ID", format!("Shape:{}:0", &id["ROI:".len()..]));
        if let Some(c) = color {
            attr(&mut s, "StrokeColor", c.to_string());
        }
        for (a, v) in attrs {
            attr(&mut s, a, v);
        }
        empty(&mut w, s)?;
        end(&mut w, "Union")?;
        text_el(
            &mut w,
            "Description",
            "Geometry copied from the source file's first keyframe as stored; units not verified.",
        )?;
        end(&mut w, "ROI")?;
    }
    end(&mut w, "OME")?;
    let s = String::from_utf8(w.into_inner()).map_err(|x| x.to_string())?;
    Ok(ascii_only(&s))
}

/// Stage position of an image in µm: `extra.stage_position_um` {x, y, z}, else the first
/// per-frame record's `stage_x_um`/`stage_y_um`/`stage_z_um`.
fn stage_position(im: &ImageInfo, frames: &[Value]) -> Option<[Option<f64>; 3]> {
    let from = |o: &Map<String, Value>, keys: [&str; 3]| keys.map(|k| num(o, k));
    let p = im
        .extra
        .get("stage_position_um")
        .and_then(Value::as_object)
        .map(|o| from(o, ["x", "y", "z"]))
        .filter(|p| p.iter().any(Option::is_some))
        .or_else(|| {
            frames
                .first()
                .and_then(Value::as_object)
                .map(|o| from(o, ["stage_x_um", "stage_y_um", "stage_z_um"]))
        })?;
    p.iter().any(Option::is_some).then_some(p)
}

/// TIFF `ImageDescription` is an ASCII field; XML numeric character references keep the
/// document valid while staying 7-bit (names with accents, …). Units avoid them: Bio-Formats
/// 8.5 rejects `&#181;m`, so lengths are written without a unit (the schema default, µm) or in
/// nm.
pub fn ascii_only(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    for ch in xml.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            out.push_str(&format!("&#{};", ch as u32));
        }
    }
    out
}

/// `#RRGGBB` → OME signed 32-bit RGBA colour with alpha 255.
fn ome_color(hex: &str) -> Option<i32> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    let rgba = (v << 8) | 0xFF;
    Some(rgba as i32)
}

/// Image-level normalized fields the OME model cannot carry exactly (`NORMALIZED_NS` keys):
/// the acquisition time when `AcquisitionDate` had to be shortened or left out, the software,
/// and an immersion outside the OME enumeration.
fn image_normalized(im: &ImageInfo, acquired: Option<&str>) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    if let Some(t) = im
        .acquired_at
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        && acquired != Some(t)
    {
        v.push(("acquired_at", t.to_string()));
    }
    if let Some(i) = &im.instrument {
        for (k, x) in [
            ("instrument.software", &i.software),
            ("instrument.software_version", &i.software_version),
        ] {
            if let Some(x) = x.as_deref().map(str::trim).filter(|x| !x.is_empty()) {
                v.push((k, x.to_string()));
            }
        }
    }
    if let Some(m) = im
        .objective
        .as_ref()
        .and_then(|o| o.immersion.as_deref())
        .map(str::trim)
        .filter(|m| !m.is_empty() && ome_immersion(m) != *m)
    {
        v.push(("objective.immersion", m.to_string()));
    }
    v
}

/// An ISO-8601 timestamp as `xsd:dateTime`, fraction cut to milliseconds, zone designator
/// (`Z` or `±hh:mm`) kept; `None` when it is not ISO-8601.
fn xsd_datetime(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let ok = s.len() >= 19
        && s.is_char_boundary(19)
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
            .iter()
            .all(|&i| b[i].is_ascii_digit());
    if !ok {
        return None;
    }
    let (base, rest) = s.split_at(19);
    let (frac, zone) = match rest.strip_prefix('.') {
        Some(r) => {
            let n = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            (&r[..n], &r[n..])
        }
        None => ("", rest),
    };
    let zone_ok = zone.is_empty()
        || zone == "Z"
        || (zone.len() == 6
            && (zone.starts_with('+') || zone.starts_with('-'))
            && zone.as_bytes()[3] == b':');
    if !zone_ok {
        return None;
    }
    let frac = &frac[..frac.len().min(3)];
    Some(if frac.is_empty() {
        format!("{base}{zone}")
    } else {
        format!("{base}.{frac}{zone}")
    })
}

/// A stable, content-derived UUID-shaped identifier (deterministic output, no randomness).
fn pseudo_uuid(file: &FileInfo) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in file.path.bytes().chain(file.size_bytes.to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let a = h;
    let b = h.rotate_left(29) ^ 0x9E37_79B9_7F4A_7C15;
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        (a & 0xfff) as u16,
        ((b >> 48) as u16 & 0x3fff) | 0x8000,
        b & 0xffff_ffff_ffff
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::model::{FormatDescriptor, InstrumentInfo, PhysicalSize};
    use openreadout_core::{Confidence, PixelType};
    use serde_json::json;

    #[test]
    fn characters_xml_forbids_are_dropped() {
        assert_eq!(xml_chars("a\0b\u{1}c\td\ne\u{FFFF}"), "abc\td\ne");
        assert!(matches!(xml_chars("plain"), std::borrow::Cow::Borrowed(_)));
    }

    fn file(images: Vec<ImageInfo>) -> FileInfo {
        FileInfo {
            path: "x".into(),
            size_bytes: 1,
            format: FormatDescriptor {
                id: "x".into(),
                name: "X".into(),
                vendor: String::new(),
                extensions: vec![],
                family: "microscopy".into(),
                can_read: true,
                can_write: false,
                confidence: Confidence::Low,
                known_gaps: vec![],
            },
            format_version: None,
            plane_count: images.iter().map(|i| i.plane_count).sum(),
            images,
            tables: vec![],
            spectra: vec![],
            traces: vec![],
            notes: vec![],
        }
    }

    #[test]
    fn metadata_only_and_positive_sizes() {
        let mut im = ImageInfo::new(0, 8, 8, PixelType::Uint8);
        im.physical_size = PhysicalSize {
            x: Some(0.5),
            y: Some(0.5),
            z: Some(0.0),
            unit: "µm".into(),
        };
        let im = im.finish();
        let file = file(vec![im.clone()]);
        let w = WrittenImage {
            planes: vec![(0, 0, 0)],
            ..WrittenImage::whole(&im)
        };
        let tiff = build_ome_xml(&file, std::slice::from_ref(&w), "t", None).unwrap();
        assert!(tiff.contains("<TiffData"));
        assert!(!tiff.contains("MetadataOnly"));
        assert!(tiff.contains("PhysicalSizeX=\"0.5\""));
        assert!(!tiff.contains("PhysicalSizeZ"));
        assert!(
            !tiff.contains("<Plane"),
            "nothing known about planes: {tiff}"
        );
        let meta = build_ome_xml_metadata_only(&file, &[w], "t", None).unwrap();
        assert!(meta.contains("<MetadataOnly/>"));
        assert!(!meta.contains("TiffData"));
    }

    #[test]
    fn color_packs_rgba() {
        assert_eq!(ome_color("#FF0000"), Some(0xFF00_00FFu32 as i32));
        assert_eq!(ome_color("#00FF00"), Some(0x00FF_00FF));
    }

    #[test]
    fn datetimes() {
        assert_eq!(
            xsd_datetime("2017-06-06T09:15:06.9801234Z").as_deref(),
            Some("2017-06-06T09:15:06.980Z")
        );
        assert_eq!(
            xsd_datetime("2017-06-06T09:15:06").as_deref(),
            Some("2017-06-06T09:15:06")
        );
        assert_eq!(
            xsd_datetime("2017-06-06T09:15:06+02:00").as_deref(),
            Some("2017-06-06T09:15:06+02:00")
        );
        assert_eq!(xsd_datetime("06/06/2017 11:15:06"), None);
    }

    #[test]
    fn strides() {
        assert_eq!(regular_step(vec![0, 2, 4]), Some(2));
        assert_eq!(regular_step(vec![3]), Some(1));
        assert_eq!(regular_step(vec![0, 1, 3]), None);
    }

    fn attr_of(doc: &roxmltree::Document<'_>, el: &str, n: usize, a: &str) -> Option<String> {
        doc.descendants()
            .filter(|e| e.tag_name().name() == el)
            .nth(n)
            .and_then(|e| e.attribute(a))
            .map(str::to_string)
    }

    /// A document with every element the builder can emit; key attributes are asserted.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn full_document() {
        let mut im = ImageInfo::new(0, 16, 8, PixelType::Uint16);
        im.name = Some("Well A1".into());
        im.size_c = 2;
        im.size_z = 3;
        im.size_t = 2;
        im.physical_size = PhysicalSize::micrometres(Some(0.2), Some(0.2), Some(1.5));
        im.time_increment_s = Some(10.0);
        im.acquired_at = Some("2024-03-01T10:20:30.1234567Z".into());
        im.channels = vec![
            ChannelInfo {
                index: 0,
                name: Some("GFP".into()),
                fluorophore: Some("EGFP".into()),
                excitation_nm: Some(488.0),
                emission_nm: Some(509.0),
                color: Some("#00FF00".into()),
                acquisition_mode: Some("Laser Scanning Confocal Fluorescence".into()),
                exposure_ms: Some(20.0),
                ..ChannelInfo::default()
            },
            ChannelInfo {
                index: 1,
                name: Some("mCherry".into()),
                excitation_nm: Some(561.0),
                emission_range_nm: Some([580.0, 640.0]),
                color: Some("#FF0000".into()),
                acquisition_mode: Some("Widefield Fluorescence".into()),
                ..ChannelInfo::default()
            },
        ];
        im.objective = Some(ObjectiveInfo {
            model: Some("Plan Apo 60x".into()),
            nominal_magnification: Some(60.0),
            lens_na: Some(1.4),
            immersion: Some("oil".into()),
        });
        im.instrument = Some(InstrumentInfo {
            manufacturer: Some("Acme".into()),
            model: Some("Scope 9".into()),
            detector: Some("Camera A".into()),
            software: Some("ZEN".into()),
            software_version: Some("3.1".into()),
        });
        im.extra.insert("refractive_index".into(), json!(1.515));
        im.extra.insert(
            "stage_position_um".into(),
            json!({"x": 100.5, "y": -20.0, "z": 3000.0}),
        );
        im.extra.insert(
            "channel_settings".into(),
            json!([{"index": 0, "detector": "PMT 1", "gain": 650.0},
                   {"index": 1, "detector": "Camera A", "binning": "2x2"}]),
        );
        im.extra.insert(
            "experimenter".into(),
            json!({"first_name": "Ada", "last_name": "Lovelace", "email": "ada@example.org"}),
        );
        im.extra.insert(
            "experiment".into(),
            json!({"type": "time-lapse", "description": "Drug response"}),
        );
        im.extra.insert(
            "rois".into(),
            json!([
                {"id": 1, "label": "cell", "shape": "rectangle", "color": "#FFFF00",
                 "keyframes": [{"time_ms": 0.0, "center": [8.0, 4.0, 0.0], "box_size": [4.0, 2.0, 0.0], "base_points": []}]},
                {"id": 2, "shape": "polygon",
                 "keyframes": [{"center": [0.0, 0.0, 0.0], "base_points": [[1.0, 1.0], [3.0, 1.0], [2.0, 3.0]]}]},
                {"id": 3, "shape": "ellipse", "keyframes": []}
            ]),
        );
        let im = im.finish();
        // Records cover all channels of a (z, t); plane (z=1, t=1) has none and falls back.
        let frames: Vec<Value> = (0..2u32)
            .flat_map(|t| (0..3u32).map(move |z| (t, z)))
            .filter(|&(t, z)| !(t == 1 && z == 1))
            .map(|(t, z)| {
                json!({"frame": t * 3 + z, "t": t, "z": z,
                       "time_ms": f64::from(t) * 10_000.0 + f64::from(z) * 100.0,
                       "stage_x_um": 100.5, "stage_y_um": -20.0,
                       "stage_z_um": 3000.0 + f64::from(z) * 1.5,
                       "exposure_ms": 99.0})
            })
            .collect();
        // Export every other Z slice (0 and 2) of both channels and both time points.
        let mut planes = Vec::new();
        let mut source = Vec::new();
        for t in 0..2 {
            for (zi, z) in [0u32, 2].iter().enumerate() {
                for c in 0..2 {
                    planes.push((c, zi as u32, t));
                    source.push((c, *z, t));
                }
            }
        }
        let w = WrittenImage {
            info: &im,
            first_ifd: 0,
            planes,
            size_c: 2,
            size_z: 2,
            size_t: 2,
            c_map: vec![0, 1],
            source_planes: source,
            frames,
        };
        let f = file(vec![im.clone()]);
        let xml = build_ome_xml(&f, std::slice::from_ref(&w), "test", None).unwrap();
        assert!(xml.is_ascii());
        let doc = roxmltree::Document::parse(&xml).unwrap();
        let names: Vec<&str> = doc
            .root_element()
            .children()
            .filter(roxmltree::Node::is_element)
            .map(|n| n.tag_name().name())
            .collect();
        assert_eq!(
            names,
            [
                "Experiment",
                "Experimenter",
                "Instrument",
                "Image",
                "StructuredAnnotations",
                "ROI",
                "ROI"
            ]
        );
        // Experiment and experimenter
        assert_eq!(
            attr_of(&doc, "Experiment", 0, "Type").as_deref(),
            Some("TimeLapse")
        );
        assert_eq!(
            attr_of(&doc, "Experimenter", 0, "LastName").as_deref(),
            Some("Lovelace")
        );
        assert_eq!(
            attr_of(&doc, "ExperimenterRef", 0, "ID").as_deref(),
            Some("Experimenter:0")
        );
        assert_eq!(
            attr_of(&doc, "ExperimentRef", 0, "ID").as_deref(),
            Some("Experiment:0")
        );
        // Instrument: laser for the confocal channel, generic source for widefield, two
        // detectors (the instrument's own first).
        assert_eq!(
            attr_of(&doc, "Microscope", 0, "Model").as_deref(),
            Some("Scope 9")
        );
        assert_eq!(
            attr_of(&doc, "Laser", 0, "Wavelength").as_deref(),
            Some("488")
        );
        assert_eq!(
            attr_of(&doc, "GenericExcitationSource", 0, "ID").as_deref(),
            Some("LightSource:0:1")
        );
        assert_eq!(
            attr_of(&doc, "Detector", 0, "Model").as_deref(),
            Some("Camera A")
        );
        assert_eq!(
            attr_of(&doc, "Detector", 1, "Model").as_deref(),
            Some("PMT 1")
        );
        assert_eq!(
            attr_of(&doc, "Objective", 0, "Immersion").as_deref(),
            Some("Oil")
        );
        assert_eq!(
            attr_of(&doc, "Objective", 0, "LensNA").as_deref(),
            Some("1.4")
        );
        assert_eq!(
            attr_of(&doc, "ObjectiveSettings", 0, "Medium").as_deref(),
            Some("Oil")
        );
        assert_eq!(
            attr_of(&doc, "ObjectiveSettings", 0, "RefractiveIndex").as_deref(),
            Some("1.515")
        );
        assert_eq!(
            attr_of(&doc, "AcquisitionDate", 0, "x"),
            None,
            "AcquisitionDate has text, not attributes"
        );
        let date = doc
            .descendants()
            .find(|n| n.tag_name().name() == "AcquisitionDate")
            .and_then(|n| n.text());
        assert_eq!(date, Some("2024-03-01T10:20:30.123Z"));
        // StageLabel
        assert_eq!(
            attr_of(&doc, "StageLabel", 0, "Name").as_deref(),
            Some("Well A1")
        );
        assert_eq!(
            attr_of(&doc, "StageLabel", 0, "X").as_deref(),
            Some("100500")
        );
        assert_eq!(
            attr_of(&doc, "StageLabel", 0, "ZUnit").as_deref(),
            Some("nm")
        );
        // Pixels: every other Z slice doubles the Z step; T is complete.
        assert_eq!(
            attr_of(&doc, "Pixels", 0, "PhysicalSizeZ").as_deref(),
            Some("3")
        );
        assert_eq!(
            attr_of(&doc, "Pixels", 0, "TimeIncrement").as_deref(),
            Some("10")
        );
        // Channel settings
        assert_eq!(
            attr_of(&doc, "LightSourceSettings", 0, "Wavelength").as_deref(),
            Some("488")
        );
        assert_eq!(
            attr_of(&doc, "DetectorSettings", 0, "Gain").as_deref(),
            Some("650")
        );
        assert_eq!(
            attr_of(&doc, "DetectorSettings", 1, "Binning").as_deref(),
            Some("2x2")
        );
        assert_eq!(
            attr_of(&doc, "DetectorSettings", 1, "ID").as_deref(),
            Some("Detector:0:0")
        );
        // A detection band is an emission filter, not an invented emission wavelength.
        assert_eq!(attr_of(&doc, "Channel", 1, "EmissionWavelength"), None);
        assert_eq!(
            attr_of(&doc, "TransmittanceRange", 0, "CutIn").as_deref(),
            Some("580")
        );
        assert_eq!(
            attr_of(&doc, "TransmittanceRange", 0, "CutOut").as_deref(),
            Some("640")
        );
        assert_eq!(
            attr_of(&doc, "EmissionFilterRef", 0, "ID").as_deref(),
            Some("Filter:0:0")
        );
        // Acquisition modes as OME enumerations, the fluorescence as contrast method.
        assert_eq!(
            attr_of(&doc, "Channel", 0, "AcquisitionMode").as_deref(),
            Some("LaserScanningConfocalMicroscopy")
        );
        assert_eq!(
            attr_of(&doc, "Channel", 1, "AcquisitionMode").as_deref(),
            Some("WideField")
        );
        assert_eq!(
            attr_of(&doc, "Channel", 1, "ContrastMethod").as_deref(),
            Some("Fluorescence")
        );
        // Software and the full-precision time go to the image's normalized annotation.
        assert_eq!(
            attr_of(&doc, "MapAnnotation", 0, "Namespace").as_deref(),
            Some(NORMALIZED_NS)
        );
        let pairs: Vec<(String, String)> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "M")
            .map(|n| {
                (
                    n.attribute("K").unwrap_or_default().to_string(),
                    n.text().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            pairs,
            [
                ("acquired_at".into(), "2024-03-01T10:20:30.1234567Z".into()),
                ("instrument.software".into(), "ZEN".into()),
                ("instrument.software_version".into(), "3.1".into()),
            ]
        );
        // Planes: one per written plane, in write order, with indices in the exported space.
        let planes: Vec<roxmltree::Node<'_, '_>> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "Plane")
            .collect();
        assert_eq!(planes.len(), 8);
        let p = |n: usize, a: &str| planes[n].attribute(a).map(str::to_string);
        // plane 2 = (c0, exported z1 = source z2, t0)
        assert_eq!(p(2, "TheZ").as_deref(), Some("1"));
        assert_eq!(p(2, "DeltaT").as_deref(), Some("0.2"));
        assert_eq!(p(2, "PositionZ").as_deref(), Some("3003000"));
        assert_eq!(p(2, "PositionZUnit").as_deref(), Some("nm"));
        assert_eq!(
            p(2, "ExposureTime").as_deref(),
            Some("0.02"),
            "channel exposure wins"
        );
        // channel 1 has no exposure of its own and records cover several channels: none.
        assert_eq!(p(3, "ExposureTime"), None);
        // plane 6 = (c0, source z2, t1): record time 10.2 s
        assert_eq!(p(6, "DeltaT").as_deref(), Some("10.2"));
        assert_eq!(p(6, "TheT").as_deref(), Some("1"));
        // ROIs: rectangle (corner from centre) and polygon; the ellipse without keyframes is dropped.
        assert_eq!(attr_of(&doc, "ROIRef", 0, "ID").as_deref(), Some("ROI:0:0"));
        assert_eq!(attr_of(&doc, "Rectangle", 0, "X").as_deref(), Some("6"));
        assert_eq!(
            attr_of(&doc, "Rectangle", 0, "Height").as_deref(),
            Some("2")
        );
        assert_eq!(attr_of(&doc, "ROI", 0, "Name").as_deref(), Some("cell"));
        assert_eq!(
            attr_of(&doc, "Polygon", 0, "Points").as_deref(),
            Some("1,1 3,1 2,3")
        );
        assert_eq!(attr_of(&doc, "Shape", 0, "ID"), None);
        assert_eq!(
            attr_of(&doc, "Polygon", 0, "ID").as_deref(),
            Some("Shape:0:1:0")
        );

        // Metadata-only form of the whole image: planes enumerated, fallback DeltaT = t × 10 s.
        let mut whole = WrittenImage::whole(&im);
        whole.frames = Vec::new();
        let meta = build_ome_xml_metadata_only(&f, &[whole], "test", None).unwrap();
        let doc = roxmltree::Document::parse(&meta).unwrap();
        let planes: Vec<_> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "Plane")
            .collect();
        assert_eq!(planes.len(), 12);
        assert_eq!(planes[11].attribute("DeltaT"), Some("10"));
        assert_eq!(planes[11].attribute("PositionX"), None);
    }

    #[test]
    fn shared_instrument_is_deduplicated() {
        let mut a = ImageInfo::new(0, 4, 4, PixelType::Uint8);
        a.objective = Some(ObjectiveInfo {
            nominal_magnification: Some(10.0),
            ..ObjectiveInfo::default()
        });
        let a = a.finish();
        let mut b = a.clone();
        b.index = 1;
        let mut c = a.clone();
        c.index = 2;
        c.objective = Some(ObjectiveInfo {
            nominal_magnification: Some(40.0),
            ..ObjectiveInfo::default()
        });
        let f = file(vec![a.clone(), b.clone(), c.clone()]);
        let ws = [
            WrittenImage::whole(&a),
            WrittenImage::whole(&b),
            WrittenImage::whole(&c),
        ];
        let xml = build_ome_xml_metadata_only(&f, &ws, "t", None).unwrap();
        let doc = roxmltree::Document::parse(&xml).unwrap();
        assert_eq!(
            doc.descendants()
                .filter(|n| n.tag_name().name() == "Instrument")
                .count(),
            2
        );
        assert_eq!(
            attr_of(&doc, "InstrumentRef", 1, "ID").as_deref(),
            Some("Instrument:0")
        );
        assert_eq!(
            attr_of(&doc, "InstrumentRef", 2, "ID").as_deref(),
            Some("Instrument:1")
        );
        assert_eq!(
            attr_of(&doc, "ObjectiveSettings", 2, "ID").as_deref(),
            Some("Objective:1:0")
        );
    }
}
