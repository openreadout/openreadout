//! WITec Project (`.wip`) and WITec Data (`.wid`) files: a tree of tagged records holding
//! spectra (single spectra, line scans, depth profiles and Raman maps), images derived from them,
//! video images and the measurement information text.
//!
//! Layout and vocabulary: `docs/formats/witec-project.md`; provenance:
//! `docs/provenance/witec-project.md`.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::bytes::windows1252;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::common::{
    Attachment, Facts, MapSpec, Parsed, RasterLayout, RasterSpec, Rows, SpectrumSet, SpectrumTable,
    Stored, XValues, num, read_at,
};

/// The four magic strings: project and data files, versions 0–5 and 6–7.
pub(crate) const MAGICS: [&[u8; 8]; 4] = [b"WIT_PRCT", b"WIT_DATA", b"WIT_PR06", b"WIT_DA06"];

const ID: &str = crate::WITEC_FORMAT_ID;
/// Deepest tag nesting accepted (real files nest about 10 deep).
const MAX_DEPTH: usize = 48;
/// Most tags accepted in one file.
const MAX_TAGS: usize = 4_000_000;
/// Longest tag name accepted.
const MAX_NAME: u32 = 4096;
/// Leaf values up to this size are read while parsing; larger ones (the data arrays) are read
/// when needed.
const EAGER: u64 = 1 << 20;
/// Planck constant × speed of light, eV·nm (energy of a photon of wavelength λ nm = HC / λ).
const HC_EV_NM: f64 = 1_239.841_93;

/// True when `head` starts with one of the magic strings.
pub(crate) fn looks_like_witec(head: &[u8]) -> bool {
    MAGICS.iter().any(|m| head.starts_with(*m))
}

/// A leaf value.
#[derive(Debug, Clone)]
enum Val {
    F64(Vec<f64>),
    F32(Vec<f32>),
    I64(Vec<i64>),
    I32(Vec<i32>),
    U8(Vec<u8>),
    Bool(Vec<bool>),
    Text(Vec<String>),
    /// Dates: year, month, day, hour, minute, second, millisecond.
    Date(Vec<[u16; 7]>),
    /// A leaf too large to read while parsing (read on demand from `start`..`end`).
    Large,
    /// A leaf whose type code is not one of the ten known ones.
    Unknown,
}

/// One tag of the tree.
#[derive(Debug, Clone)]
struct Tag {
    name: String,
    type_code: u32,
    start: u64,
    end: u64,
    children: Vec<Tag>,
    value: Option<Val>,
}

impl Tag {
    fn child(&self, name: &str) -> Option<&Tag> {
        self.children.iter().find(|c| c.name == name)
    }
    fn path(&self, names: &[&str]) -> Option<&Tag> {
        let mut t = self;
        for n in names {
            t = t.child(n)?;
        }
        Some(t)
    }
    fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
    fn i64s(&self) -> Option<Vec<i64>> {
        Some(match self.value.as_ref()? {
            Val::I32(v) => v.iter().map(|x| i64::from(*x)).collect(),
            Val::I64(v) => v.clone(),
            Val::U8(v) => v.iter().map(|x| i64::from(*x)).collect(),
            _ => return None,
        })
    }
    fn f64s(&self) -> Option<Vec<f64>> {
        Some(match self.value.as_ref()? {
            Val::F64(v) => v.clone(),
            Val::F32(v) => v.iter().map(|x| f64::from(*x)).collect(),
            // integers stored in number tags are small
            Val::I64(v) => v.iter().map(|x| *x as f64).collect(),
            Val::I32(v) => v.iter().map(|x| f64::from(*x)).collect(),
            _ => return None,
        })
    }
    fn bools(&self) -> Option<Vec<bool>> {
        match self.value.as_ref()? {
            Val::Bool(v) => Some(v.clone()),
            _ => None,
        }
    }
    fn text(&self) -> Option<String> {
        match self.value.as_ref()? {
            Val::Text(v) => Some(v.join("\n")),
            _ => None,
        }
    }
    fn get_i64(&self, name: &str) -> Option<i64> {
        self.child(name)?.i64s()?.first().copied()
    }
    fn get_f64(&self, name: &str) -> Option<f64> {
        self.child(name)?.f64s()?.first().copied()
    }
    fn get_bool(&self, name: &str) -> Option<bool> {
        self.child(name)?.bools()?.first().copied()
    }
    fn get_text(&self, name: &str) -> Option<String> {
        self.child(name)?.text()
    }
}

/// Reads the tag tree.
struct TreeReader<'a> {
    f: &'a SourceFile,
    path: &'a Path,
    len: u64,
    tags: usize,
}

impl TreeReader<'_> {
    fn bytes(&self, at: u64, n: u64) -> Result<Vec<u8>> {
        let end = at
            .checked_add(n)
            .filter(|e| *e <= self.len)
            .ok_or_else(|| {
                Error::corrupt_at(
                    ID,
                    at,
                    format!("{n} bytes at {at} run past the end of the file"),
                )
            })?;
        read_at(self.f, self.path, at, end - at, self.len)
    }

    /// The tags in `[from, to)`, each followed by the next.
    fn tags(&mut self, from: u64, to: u64, depth: usize) -> Result<Vec<Tag>> {
        if depth > MAX_DEPTH {
            return Err(Error::corrupt_at(ID, from, "tags nest too deeply"));
        }
        let mut out = Vec::new();
        let mut at = from;
        while at < to {
            let tag = self.tag(at, to, depth)?;
            at = tag.end;
            out.push(tag);
        }
        Ok(out)
    }

    fn tag(&mut self, at: u64, limit: u64, depth: usize) -> Result<Tag> {
        self.tags += 1;
        if self.tags > MAX_TAGS {
            return Err(Error::corrupt_at(
                ID,
                at,
                "more tags than any real file holds",
            ));
        }
        let b = self.bytes(at, 4)?;
        let name_len = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        if name_len > MAX_NAME {
            return Err(Error::corrupt_at(
                ID,
                at,
                format!("tag name length {name_len} is not plausible"),
            ));
        }
        let h = self.bytes(at + 4, u64::from(name_len) + 20)?;
        let nl = name_len as usize;
        let name = windows1252(&h[..nl]);
        let type_code = u32::from_le_bytes([h[nl], h[nl + 1], h[nl + 2], h[nl + 3]]);
        let start = u64::from_le_bytes(h[nl + 4..nl + 12].try_into().unwrap_or([0; 8]));
        let end = u64::from_le_bytes(h[nl + 12..nl + 20].try_into().unwrap_or([0; 8]));
        let header_end = at + 4 + u64::from(name_len) + 20;
        if start < header_end || end < start || end > limit {
            return Err(Error::corrupt_at(
                ID,
                at,
                format!(
                    "tag `{name}`: its data [{start}, {end}) does not follow its header at {header_end} inside [{at}, {limit})"
                ),
            ));
        }
        let mut tag = Tag {
            name,
            type_code,
            start,
            end,
            children: Vec::new(),
            value: None,
        };
        if type_code == 0 {
            tag.children = self.tags(start, end, depth + 1)?;
        } else {
            tag.value = Some(self.value(&tag)?);
        }
        Ok(tag)
    }

    fn value(&self, t: &Tag) -> Result<Val> {
        let n = t.len();
        let size = match t.type_code {
            2 | 4 => 8,
            3 | 5 => 4,
            6 => 14,
            7..=9 => 1,
            _ => return Ok(Val::Unknown),
        };
        if !n.is_multiple_of(size) {
            return Err(Error::corrupt_at(
                ID,
                t.start,
                format!(
                    "tag `{}`: {n} bytes is not a whole number of {size}-byte values",
                    t.name
                ),
            ));
        }
        if n > EAGER {
            return Ok(Val::Large);
        }
        let b = self.bytes(t.start, n)?;
        Ok(match t.type_code {
            2 => Val::F64(
                b.as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| f64::from_le_bytes(*c))
                    .collect(),
            ),
            3 => Val::F32(
                b.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect(),
            ),
            4 => Val::I64(
                b.as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| i64::from_le_bytes(*c))
                    .collect(),
            ),
            5 => Val::I32(
                b.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| i32::from_le_bytes(*c))
                    .collect(),
            ),
            6 => Val::Date(
                b.as_chunks::<14>()
                    .0
                    .iter()
                    .map(|c| {
                        let mut d = [0u16; 7];
                        for (k, v) in d.iter_mut().enumerate() {
                            *v = u16::from_le_bytes([c[2 * k], c[2 * k + 1]]);
                        }
                        d
                    })
                    .collect(),
            ),
            7 => Val::U8(b),
            8 => Val::Bool(b.iter().map(|x| *x != 0).collect()),
            _ => Val::Text(strings(&b, t)?),
        })
    }
}

/// A string array: (length, bytes) pairs.
fn strings(b: &[u8], t: &Tag) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let n = b
            .get(i..i + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
            .ok_or_else(|| {
                Error::corrupt_at(
                    ID,
                    t.start,
                    format!("tag `{}`: string length cut off", t.name),
                )
            })?;
        let s = b.get(i + 4..i + 4 + n).ok_or_else(|| {
            Error::corrupt_at(
                ID,
                t.start,
                format!("tag `{}`: string runs past its tag", t.name),
            )
        })?;
        out.push(windows1252(s));
        i += 4 + n;
    }
    Ok(out)
}

/// A value of the vendor tree: the tag's value, small arrays in full, large ones summarized.
fn tag_json(t: &Tag) -> Value {
    if t.type_code == 0 {
        let mut m = Map::new();
        for c in &t.children {
            let mut key = c.name.clone();
            let mut k = 2;
            while m.contains_key(&key) {
                key = format!("{} ({k})", c.name);
                k += 1;
            }
            m.insert(key, tag_json(c));
        }
        return Value::Object(m);
    }
    let short = |v: Vec<Value>| -> Value {
        match v.len() {
            1 => v.into_iter().next().unwrap_or(Value::Null),
            n if n <= 16 => Value::Array(v),
            n => json!({"values": n, "offset": t.start, "bytes": t.len()}),
        }
    };
    match t.value.as_ref() {
        Some(Val::F64(v)) => short(v.iter().map(|x| num(*x)).collect()),
        Some(Val::F32(v)) => short(v.iter().map(|x| num(f64::from(*x))).collect()),
        Some(Val::I64(v)) => short(v.iter().map(|x| json!(x)).collect()),
        Some(Val::I32(v)) => short(v.iter().map(|x| json!(x)).collect()),
        Some(Val::U8(v)) => short(v.iter().map(|x| json!(x)).collect()),
        Some(Val::Bool(v)) => short(v.iter().map(|x| json!(x)).collect()),
        Some(Val::Text(v)) => match v.len() {
            1 => json!(v[0]),
            _ => json!(v),
        },
        Some(Val::Date(v)) => short(v.iter().map(|d| json!(date_text(d))).collect()),
        Some(Val::Large) => json!({"type_code": t.type_code, "offset": t.start, "bytes": t.len()}),
        Some(Val::Unknown) | None => {
            json!({"unknown_type_code": t.type_code, "offset": t.start, "bytes": t.len()})
        }
    }
}

fn date_text(d: &[u16; 7]) -> String {
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
        d[0], d[1], d[2], d[3], d[4], d[5], d[6]
    )
}

/// One entry of the `Data` list.
struct Entry<'a> {
    class: String,
    id: i64,
    caption: String,
    tag: &'a Tag,
}

impl Entry<'_> {
    /// The class-specific record (the tag named like the class).
    fn body(&self) -> Option<&Tag> {
        self.tag.child(&self.class)
    }
}

/// What an axis is, in our words.
struct Axis {
    quantity: &'static str,
    unit: Option<String>,
    x: XValues,
    extra: BTreeMap<String, Value>,
}

/// Spectral unit table: (quantity, unit) per interpretation unit index.
fn spectral_unit(index: i64) -> Option<(&'static str, &'static str)> {
    Some(match index {
        0 => ("wavelength", "nm"),
        1 => ("wavelength", "µm"),
        2 => ("wavenumber", "1/cm"),
        3 => ("raman_shift", "1/cm"),
        4 => ("energy", "eV"),
        5 => ("energy", "meV"),
        6 => ("energy_shift", "eV"),
        7 => ("energy_shift", "meV"),
        _ => return None,
    })
}

/// A wavelength in nm expressed in spectral unit `index` (excitation `exc` nm for the relative
/// units).
fn from_nm(index: i64, nm: f64, exc: f64) -> f64 {
    match index {
        1 => nm * 1e-3,
        2 => 1e7 / nm,
        3 => 1e7 * (1.0 / exc - 1.0 / nm),
        4 => HC_EV_NM / nm,
        5 => HC_EV_NM * 1e3 / nm,
        6 => -HC_EV_NM * (1.0 / exc - 1.0 / nm),
        7 => -HC_EV_NM * 1e3 * (1.0 / exc - 1.0 / nm),
        _ => nm,
    }
}

/// The grating calibration (type 1): pixel `nc` sees `lambda_c` nm; `gamma` the included
/// angle, `delta` the detector tilt (rad); `m` the order, `d` the grooves per mm, `x` the pixel
/// width and `f` the focal length (mm).
struct Grating {
    nc: f64,
    lambda_c: f64,
    gamma: f64,
    delta: f64,
    m: f64,
    d: f64,
    x: f64,
    f: f64,
}

impl Grating {
    fn of(b: &Tag) -> Option<Self> {
        let g = |k: &str| b.get_f64(k);
        Some(Grating {
            nc: g("nC")?,
            lambda_c: g("LambdaC")?,
            gamma: g("Gamma")?,
            delta: g("Delta")?,
            m: g("m")?,
            d: g("d")?,
            x: g("x")?,
            f: g("f")?,
        })
    }

    /// Wavelength (nm) of pixel `i` by the grating equation.
    fn nm(&self, i: f64) -> f64 {
        let Grating {
            nc,
            lambda_c,
            gamma,
            delta,
            m,
            d,
            x,
            f,
        } = *self;
        let alpha = (lambda_c * m / d / (2.0 * (gamma / 2.0).cos())).asin() - gamma / 2.0;
        let lh = f * delta.cos();
        let hc = f * delta.sin();
        let hn = x * (nc - i) - hc;
        let beta_c = gamma + alpha;
        let beta_h = beta_c - delta;
        let beta_n = beta_h - hn.atan2(lh);
        d / m * (alpha.sin() + beta_n.sin())
    }

    fn to_json(&self) -> Value {
        json!({"kind": "grating", "center_pixel": num(self.nc), "center_wavelength_nm": num(self.lambda_c),
               "included_angle_rad": num(self.gamma), "detector_tilt_rad": num(self.delta),
               "diffraction_order": num(self.m), "grooves_per_mm": num(self.d),
               "pixel_width_mm": num(self.x), "focal_length_mm": num(self.f)})
    }
}

/// Evaluate `Σ c_k·x^k`.
fn poly(c: &[f64], x: f64) -> f64 {
    c.iter().rev().fold(0.0, |acc, k| acc * x + k)
}

/// Unit tables of the other interpretations: (quantity, [(unit, factor from the default
/// unit)]); values are multiplied by the factor.
fn linear_units(kind: &str) -> Option<(&'static str, &'static [(&'static str, f64)])> {
    const SPACE: &[(&str, f64)] = &[
        ("m", 1e-6),
        ("mm", 1e-3),
        ("µm", 1.0),
        ("nm", 1e3),
        ("Å", 1e4),
        ("pm", 1e6),
    ];
    const TIME: &[(&str, f64)] = &[
        ("h", 1.0 / 3600.0),
        ("min", 1.0 / 60.0),
        ("s", 1.0),
        ("ms", 1e3),
        ("µs", 1e6),
        ("ns", 1e9),
        ("ps", 1e12),
        ("fs", 1e15),
    ];
    const FREQUENCY: &[(&str, f64)] = &[
        ("µHz", 1e6),
        ("mHz", 1e3),
        ("Hz", 1.0),
        ("kHz", 1e-3),
        ("MHz", 1e-6),
        ("GHz", 1e-9),
        ("THz", 1e-12),
    ];
    const INVERSE_SPACE: &[(&str, f64)] = &[
        ("1/m", 1e6),
        ("1/mm", 1e3),
        ("1/µm", 1.0),
        ("1/nm", 1e-3),
        ("1/Å", 1e-4),
        ("1/pm", 1e-6),
    ];
    const PHASE: &[(&str, f64)] = &[
        ("rad", 1.0),
        ("mrad", 1e3),
        ("°", 180.0 / std::f64::consts::PI),
        ("grad", 200.0 / std::f64::consts::PI),
        ("mgrad", 2e5 / std::f64::consts::PI),
    ];
    Some(match kind {
        "TDSpaceInterpretation" => ("position", SPACE),
        "TDTimeInterpretation" => ("time", TIME),
        "TDFrequencyInterpretation" => ("frequency", FREQUENCY),
        "TDInverseSpaceInterpretation" => ("spatial_frequency", INVERSE_SPACE),
        "TDPhaseInterpretation" => ("phase", PHASE),
        _ => return None,
    })
}

/// The parsed file: entries by id, the root.
struct Project<'a> {
    entries: Vec<Entry<'a>>,
    by_id: BTreeMap<i64, usize>,
}

impl<'a> Project<'a> {
    fn new(root: &'a Tag) -> (Self, Vec<Finding>) {
        let mut findings = Vec::new();
        let mut entries = Vec::new();
        let mut by_id = BTreeMap::new();
        if let Some(data) = root.child("Data") {
            let declared = data.get_i64("NumberOfData");
            let mut i = 0i64;
            loop {
                let class = data.get_text(&format!("DataClassName {i}"));
                let tag = data.child(&format!("Data {i}"));
                let (Some(class), Some(tag)) = (class, tag) else {
                    break;
                };
                let td = tag.child("TData");
                let id = td.and_then(|t| t.get_i64("ID")).unwrap_or(-1);
                let caption = td.and_then(|t| t.get_text("Caption")).unwrap_or_default();
                if id >= 0 {
                    by_id.entry(id).or_insert(entries.len());
                }
                entries.push(Entry {
                    class,
                    id,
                    caption,
                    tag,
                });
                i += 1;
            }
            if let Some(n) = declared
                && n != i
            {
                findings.push(Finding::warning(
                    "data_count",
                    format!("the data list declares {n} entries, {i} were found"),
                ));
            }
        }
        (Project { entries, by_id }, findings)
    }

    fn by_id(&self, id: Option<i64>) -> Option<&Entry<'a>> {
        let id = id.filter(|i| *i > 0)?;
        self.entries.get(*self.by_id.get(&id)?)
    }

    /// The x axis of a graph with `n` points: its transformation and interpretation.
    fn axis(&self, graph: &Tag, n: u64) -> (Axis, Vec<String>) {
        let mut notes = Vec::new();
        let tr = self.by_id(graph.get_i64("XTransformationID"));
        let interp = self.by_id(graph.get_i64("XInterpretationID"));
        let idx: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let points = |why: Option<String>, notes: &mut Vec<String>| {
            if let Some(w) = why {
                notes.push(w);
            }
            Axis {
                quantity: "points",
                unit: None,
                x: XValues::Regular {
                    first: 0.0,
                    last: n.saturating_sub(1) as f64,
                },
                extra: BTreeMap::new(),
            }
        };
        let Some(tr) = tr else {
            return (points(None, &mut notes), notes);
        };
        let mut extra = BTreeMap::new();
        extra.insert("x_calibration_caption".into(), json!(tr.caption));
        let body = tr.body();
        match tr.class.as_str() {
            "TDSpectralTransformation" => {
                let Some(b) = body else {
                    return (
                        points(
                            Some("spectral calibration record missing".into()),
                            &mut notes,
                        ),
                        notes,
                    );
                };
                let kind = b.get_i64("SpectralTransformationType").unwrap_or(-1);
                let nm: Option<Vec<f64>> = match kind {
                    0 => b.child("Polynom").and_then(Tag::f64s).map(|c| {
                        let c: Vec<f64> = c.into_iter().take(3).collect();
                        idx.iter().map(|i| poly(&c, *i)).collect()
                    }),
                    1 => Grating::of(b).map(|g| {
                        extra.insert("x_calibration".into(), g.to_json());
                        idx.iter().map(|i| g.nm(*i)).collect()
                    }),
                    2 => {
                        let c = b.child("FreePolynom").and_then(Tag::f64s);
                        let order = b.get_i64("FreePolynomOrder");
                        let lo = b.get_f64("FreePolynomStartBin");
                        let hi = b.get_f64("FreePolynomStopBin");
                        match (c, order, lo, hi) {
                            (Some(c), Some(order), Some(lo), Some(hi))
                                if order >= 0 && !c.is_empty() =>
                            {
                                let k = usize::try_from(order).unwrap_or(0).min(c.len() - 1);
                                let c = &c[..=k];
                                let hi = hi.max(lo);
                                extra.insert(
                                    "x_calibration".into(),
                                    json!({"kind": "polynomial", "coefficients": c.iter().map(|v| num(*v)).collect::<Vec<_>>(),
                                           "first_pixel": num(lo), "last_pixel": num(hi)}),
                                );
                                let (vlo, vhi) = (poly(c, lo), poly(c, hi));
                                Some(
                                    idx.iter()
                                        .map(|i| {
                                            if *i <= lo {
                                                vlo
                                            } else if *i >= hi {
                                                vhi
                                            } else {
                                                poly(c, *i)
                                            }
                                        })
                                        .collect(),
                                )
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                let Some(nm) = nm else {
                    return (
                        points(
                            Some(format!(
                                "spectral calibration type {kind} not decoded: x is the pixel index"
                            )),
                            &mut notes,
                        ),
                        notes,
                    );
                };
                if kind == 0 {
                    extra.insert("x_calibration".into(), json!({"kind": "pixel polynomial"}));
                }
                let (unit_index, exc) = match interp {
                    Some(e) if e.class == "TDSpectralInterpretation" => (
                        e.tag
                            .path(&["TDInterpretation", "UnitIndex"])
                            .and_then(|t| t.i64s()?.first().copied())
                            .unwrap_or(0),
                        e.body().and_then(|b| b.get_f64("ExcitationWaveLength")),
                    ),
                    _ => (0, None),
                };
                if let Some(e) = exc {
                    extra.insert("excitation_wavelength_nm".into(), num(e));
                }
                let (mut q, mut u) = spectral_unit(unit_index).unwrap_or(("wavelength", "nm"));
                let mut ui = unit_index;
                if spectral_unit(unit_index).is_none() {
                    notes.push(format!(
                        "spectral unit {unit_index} is an arbitrary unit: x is in nm"
                    ));
                    ui = 0;
                }
                let needs_exc = matches!(ui, 3 | 6 | 7);
                let exc_v = exc.filter(|e| e.is_finite() && *e > 0.0);
                if needs_exc && exc_v.is_none() {
                    notes.push(
                        "relative spectral unit without an excitation wavelength: x is in nm"
                            .into(),
                    );
                    ui = 0;
                    q = "wavelength";
                    u = "nm";
                }
                let vals: Vec<f64> = nm
                    .iter()
                    .map(|v| from_nm(ui, *v, exc_v.unwrap_or(f64::NAN)))
                    .collect();
                extra.insert("x_unit_code".into(), json!(unit_index));
                (
                    Axis {
                        quantity: q,
                        unit: Some(u.to_string()),
                        x: XValues::Listed(vals),
                        extra,
                    },
                    notes,
                )
            }
            "TDLinearTransformation" | "TDLUTTransformation" => {
                let Some(b) = body else {
                    return (
                        points(Some("x calibration record missing".into()), &mut notes),
                        notes,
                    );
                };
                let std_unit = tr
                    .tag
                    .path(&["TDTransformation", "StandardUnit"])
                    .and_then(Tag::text)
                    .unwrap_or_default();
                let raw: Vec<f64> = if tr.class == "TDLinearTransformation" {
                    let g = |a: &str, b2: &str| b.get_f64(a).or_else(|| b.get_f64(b2));
                    match (
                        g("ModelOrigin_D", "ModelOrigin"),
                        g("WorldOrigin_D", "WorldOrigin"),
                        g("Scale_D", "Scale"),
                    ) {
                        (Some(mo), Some(wo), Some(sc)) => {
                            extra.insert("x_calibration".into(), json!({"kind": "linear", "origin_pixel": num(mo), "origin_value": num(wo), "step": num(sc)}));
                            idx.iter().map(|i| sc * (i - mo) + wo).collect()
                        }
                        _ => {
                            return (
                                points(
                                    Some(
                                        "linear x calibration incomplete: x is the pixel index"
                                            .into(),
                                    ),
                                    &mut notes,
                                ),
                                notes,
                            );
                        }
                    }
                } else {
                    let size = b.get_i64("LUTSize").and_then(|s| usize::try_from(s).ok());
                    let lut = b.child("LUT").and_then(Tag::f64s);
                    match (size, lut) {
                        (Some(s), Some(l)) if s > 0 && s <= l.len() => {
                            let l = &l[..s];
                            extra.insert(
                                "x_calibration".into(),
                                json!({"kind": "lookup table", "entries": s}),
                            );
                            idx.iter()
                                .map(|i| {
                                    let last = (s - 1) as f64;
                                    if *i <= 0.0 {
                                        l[0]
                                    } else if *i >= last {
                                        l[s - 1]
                                    } else {
                                        // 0 < i < s - 1
                                        let k = i.floor() as usize;
                                        let t = i - k as f64;
                                        l[k] + (l[k + 1] - l[k]) * t
                                    }
                                })
                                .collect()
                        }
                        _ => return (
                            points(
                                Some(
                                    "lookup-table x calibration incomplete: x is the pixel index"
                                        .into(),
                                ),
                                &mut notes,
                            ),
                            notes,
                        ),
                    }
                };
                let conv = interp.and_then(|e| {
                    let (q, table) = linear_units(&e.class)?;
                    let k = e
                        .tag
                        .path(&["TDInterpretation", "UnitIndex"])
                        .and_then(|t| t.i64s()?.first().copied())?;
                    let (u, f) = usize::try_from(k)
                        .ok()
                        .and_then(|k| table.get(k).copied())?;
                    Some((q, u, f))
                });
                let (q, u, vals) = match conv {
                    Some((q, u, f)) => (
                        q,
                        u.to_string(),
                        raw.iter().map(|v| v * f).collect::<Vec<_>>(),
                    ),
                    None => ("x", std_unit, raw),
                };
                let regular = tr.class == "TDLinearTransformation" && vals.len() > 1;
                let x = if regular {
                    XValues::Regular {
                        first: vals[0],
                        last: vals[vals.len() - 1],
                    }
                } else {
                    XValues::Listed(vals)
                };
                (
                    Axis {
                        quantity: q,
                        unit: (!u.is_empty()).then_some(u),
                        x,
                        extra,
                    },
                    notes,
                )
            }
            other => (
                points(
                    Some(format!(
                        "x calibration of class {other} not decoded: x is the pixel index"
                    )),
                    &mut notes,
                ),
                notes,
            ),
        }
    }

    /// Affine map of pixel `(x, y, 0)` to µm by the space transformation `id`: the matrix
    /// `rotation · scale` (both stored a column at a time) and the two origins.
    fn space(&self, id: Option<i64>) -> Option<Space> {
        let e = self.by_id(id)?;
        if e.class != "TDSpaceTransformation" {
            return None;
        }
        let v = e.body()?.child("ViewPort3D")?;
        let mo = v.child("ModelOrigin")?.f64s()?;
        let wo = v.child("WorldOrigin")?.f64s()?;
        let sc = v.child("Scale")?.f64s()?;
        let ro = v.child("Rotation")?.f64s()?;
        if mo.len() != 3 || wo.len() != 3 || sc.len() != 9 || ro.len() != 9 {
            return None;
        }
        let at = |m: &[f64], r: usize, c: usize| m[r + 3 * c];
        let mut a = [[0.0; 3]; 3];
        for (r, row) in a.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| at(&ro, r, k) * at(&sc, k, c)).sum();
            }
        }
        let unit_kind = e
            .tag
            .path(&["TDTransformation", "UnitKind"])
            .and_then(|t| t.i64s()?.first().copied())
            .unwrap_or(1);
        Some(Space {
            a,
            model: [mo[0], mo[1], mo[2]],
            world: [wo[0], wo[1], wo[2]],
            inverse: unit_kind == 6,
        })
    }
}

/// A space transformation.
#[derive(Debug, Clone, Copy)]
struct Space {
    a: [[f64; 3]; 3],
    model: [f64; 3],
    world: [f64; 3],
    /// Reciprocal space (1/µm) rather than µm.
    inverse: bool,
}

impl Space {
    fn at(&self, x: f64, y: f64) -> [f64; 3] {
        let p = [x - self.model[0], y - self.model[1], -self.model[2]];
        let mut out = self.world;
        for (r, o) in out.iter_mut().enumerate() {
            *o += (0..3).map(|c| self.a[r][c] * p[c]).sum::<f64>();
        }
        out
    }
    /// Size of one pixel along x and y (length of the first two matrix columns).
    fn pixel(&self) -> (f64, f64) {
        let col = |c: usize| {
            (0..3)
                .map(|r| self.a[r][c] * self.a[r][c])
                .sum::<f64>()
                .sqrt()
        };
        (col(0), col(1))
    }
}

/// The value type of a data array code.
fn stored_of(code: i64) -> Option<Stored> {
    Some(match code {
        1 => Stored::I64,
        2 => Stored::I32,
        3 => Stored::I16,
        4 => Stored::I8,
        5 => Stored::U32,
        6 => Stored::U16,
        7 => Stored::U8,
        8 => Stored::Bool,
        9 => Stored::F32,
        10 => Stored::F64,
        _ => return None,
    })
}

/// Plain text of an RTF document (the measurement information): paragraphs as lines, tabs kept,
/// `\'hh` escapes decoded as Windows-1252, formatting and the font and colour tables dropped.
pub(crate) fn rtf_text(rtf: &[u8]) -> String {
    let mut out = Vec::<u8>::new();
    let mut i = 0usize;
    // group depth at which a skipped destination (font table, …) started
    let mut depth = 0usize;
    let mut skip_from: Option<usize> = None;
    while i < rtf.len() {
        let c = rtf[i];
        match c {
            b'{' => {
                depth += 1;
                // destinations that hold no text
                let rest = &rtf[i + 1..];
                if skip_from.is_none()
                    && [
                        &b"\\fonttbl"[..],
                        b"\\colortbl",
                        b"\\stylesheet",
                        b"\\*",
                        b"\\info",
                    ]
                    .iter()
                    .any(|p| rest.starts_with(p))
                {
                    skip_from = Some(depth);
                }
                i += 1;
            }
            b'}' => {
                if skip_from == Some(depth) {
                    skip_from = None;
                }
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'\\' => {
                let next = rtf.get(i + 1).copied().unwrap_or(0);
                if next == b'\'' {
                    let h = rtf
                        .get(i + 2..i + 4)
                        .and_then(|h| std::str::from_utf8(h).ok())
                        .and_then(|h| u8::from_str_radix(h, 16).ok());
                    if skip_from.is_none()
                        && let Some(v) = h
                    {
                        out.push(v);
                    }
                    i += 4;
                } else if next.is_ascii_alphabetic() {
                    let s = i + 1;
                    let mut j = s;
                    while j < rtf.len() && rtf[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                    let word = &rtf[s..j];
                    if j < rtf.len() && (rtf[j] == b'-' || rtf[j].is_ascii_digit()) {
                        j += 1;
                        while j < rtf.len() && rtf[j].is_ascii_digit() {
                            j += 1;
                        }
                    }
                    if j < rtf.len() && rtf[j] == b' ' {
                        j += 1;
                    }
                    if skip_from.is_none() {
                        match word {
                            b"par" | b"line" => out.push(b'\n'),
                            b"tab" => out.push(b'\t'),
                            _ => {}
                        }
                    }
                    i = j;
                } else {
                    // escaped character (`\\`, `\{`, `\}`) or a control symbol
                    if skip_from.is_none() && matches!(next, b'\\' | b'{' | b'}') {
                        out.push(next);
                    }
                    i += 2;
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                if skip_from.is_none() {
                    out.push(c);
                }
                i += 1;
            }
        }
    }
    windows1252(&out)
}

/// An information text: section → key → value.
type Sections = BTreeMap<String, BTreeMap<String, String>>;

/// `section → key → value` of an information text (`Key:<tab>value` lines under `Section:`
/// headings).
fn info_sections(text: &str) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut section = String::from("General");
    for line in text.lines() {
        let l = line.trim_end();
        if l.trim().is_empty() {
            continue;
        }
        if let Some((k, v)) = l.split_once(":\t") {
            out.entry(section.clone())
                .or_default()
                .entry(k.trim().to_string())
                .or_insert_with(|| v.trim().to_string());
        } else if let Some(h) = l.trim().strip_suffix(':') {
            section = h.trim().to_string();
        }
    }
    out
}

/// A key of the information text, from any section.
fn info_get<'a>(
    info: &'a BTreeMap<String, BTreeMap<String, String>>,
    keys: &[&str],
) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| info.values().find_map(|s| s.get(*k)))
        .map(String::as_str)
        .filter(|v| !v.trim().is_empty())
}

/// A number of the information text (`0.05000`, `1,5` read as 1.5).
fn info_num(info: &BTreeMap<String, BTreeMap<String, String>>, keys: &[&str]) -> Option<f64> {
    info_get(info, keys)
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.replace(',', ".").parse().ok())
        .filter(|v: &f64| v.is_finite())
}

/// `Thursday, April 16, 2020` + `10:25:35 AM` → `2020-04-16T10:25:35` (local time; the file
/// holds no time zone).
fn info_datetime(date: &str, time: &str) -> Option<String> {
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
    let parts: Vec<&str> = date.split(',').map(str::trim).collect();
    let (md, year) = match parts.as_slice() {
        [_, md, y] => (*md, *y),
        [md, y] => (*md, *y),
        _ => return None,
    };
    let mut it = md.split_whitespace();
    let month = it.next()?.to_ascii_lowercase();
    let day: u32 = it.next()?.parse().ok()?;
    let m = MONTHS.iter().position(|x| *x == month)? + 1;
    let year: u32 = year.parse().ok()?;
    let mut tp = time.split_whitespace();
    let hms = tp.next()?;
    let ampm = tp.next().map(str::to_ascii_uppercase);
    let v: Vec<u32> = hms
        .split(':')
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    let (mut h, mi, s) = match v.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [h, m] => (*h, *m, 0),
        _ => return None,
    };
    match ampm.as_deref() {
        Some("PM") if h < 12 => h += 12,
        Some("AM") if h == 12 => h = 0,
        _ => {}
    }
    if !(1..=31).contains(&day) || h > 23 || mi > 59 || s > 60 || year < 1990 {
        return None;
    }
    Some(format!("{year:04}-{m:02}-{day:02}T{h:02}:{mi:02}:{s:02}"))
}

/// The stem of a measurement's caption: `Scan_000_Spec.Data 1_F (Sub BG)` → `Scan_000`,
/// `Scan LA--037--Spec.Data 1` → `Scan LA--037`, `Reduced<Image Scan 1 (Data)` →
/// `Reduced<Image Scan 1`, `Scan_000 Information` → `Scan_000`.
fn caption_stem(c: &str) -> &str {
    for pat in [
        "--Spec.Data",
        "_Spec.Data",
        " (Data)",
        "--Information",
        " Information",
        " (Information)",
    ] {
        if let Some(k) = c.find(pat) {
            return c[..k].trim();
        }
    }
    c.trim()
}

/// Information texts by data id: caption and sections.
type Infos = BTreeMap<i64, (String, Sections)>;

/// Opens a WITec project or data file.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    let head = read_at(f, path, 0, 8, file_len)?;
    let magic = head.as_slice();
    if !MAGICS.iter().any(|m| magic == m.as_slice()) {
        return Err(Error::corrupt(ID, "not a WITec file (no WIT_ magic)"));
    }
    let project = magic.starts_with(b"WIT_PR");
    let mut tr = TreeReader {
        f,
        path,
        len: file_len,
        tags: 0,
    };
    let tops = tr.tags(8, file_len, 0)?;
    let root = tops
        .iter()
        .find(|t| t.type_code == 0)
        .ok_or_else(|| Error::corrupt(ID, "no root tag"))?;
    let version = root.get_i64("Version");
    let mut parsed = Parsed::default();
    let (proj, findings) = Project::new(root);
    parsed.findings = findings;
    let kind = if project {
        "WITec Project"
    } else {
        "WITec Data"
    };
    parsed.format_version = version.map(|v| format!("{v}"));
    parsed.notes.push(format!(
        "{kind} file, format version {} ({})",
        version.map_or_else(|| "unknown".into(), |v| v.to_string()),
        String::from_utf8_lossy(magic)
    ));
    if !matches!(version, Some(0..=7)) {
        parsed.notes.push(format!(
            "format version {} is outside the versions 0-7 this reader knows",
            version.map_or_else(|| "missing".into(), |v| v.to_string())
        ));
    }
    let infos = information_texts(&proj, &mut parsed);
    let mut firsts = Firsts::default();
    let mut undecoded_classes: BTreeMap<String, usize> = BTreeMap::new();
    for e in &proj.entries {
        match e.class.as_str() {
            "TDGraph" => push_graph(&proj, e, &infos, &mut firsts, &mut parsed),
            "TDImage" => push_image(&proj, e, &mut parsed),
            "TDBitmap" => push_bitmap(f, path, file_len, &proj, e, &mut parsed)?,
            "TDText" => {}
            c if c.ends_with("Interpretation") || c.ends_with("Transformation") => {}
            other => *undecoded_classes.entry(other.to_string()).or_default() += 1,
        }
    }
    if !undecoded_classes.is_empty() {
        parsed.notes.push(format!(
            "records not decoded (display settings and analysis settings, no measured values): {}",
            undecoded_classes
                .iter()
                .map(|(k, v)| format!("{k} ×{v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    parsed.facts = project_facts(root, &firsts);

    let mut data_list = Vec::new();
    for e in &proj.entries {
        data_list.push(
            json!({"class": e.class, "id": e.id, "caption": e.caption, "record": tag_json(e.tag)}),
        );
        parsed.entries.push(LsEntry {
            kind: "data".into(),
            name: if e.caption.is_empty() {
                e.class.clone()
            } else {
                e.caption.clone()
            },
            offset: Some(e.tag.start),
            size: Some(e.tag.len()),
            image: None,
            details: json!({"class": e.class, "id": e.id}),
        });
    }
    let mut top = Map::new();
    for c in &root.children {
        if c.name != "Data" {
            top.insert(c.name.clone(), tag_json(c));
        }
    }
    parsed.vendor = json!({
        "magic": String::from_utf8_lossy(magic),
        "kind": kind,
        "version": version,
        "root": top,
        "data": data_list,
        "information": infos.iter().map(|(id, (c, s))| json!({"data_id": id, "caption": c, "sections": s})).collect::<Vec<_>>(),
    });
    for (k, v) in [
        ("traces[].sample_count", Source::PriorArt),
        ("traces[].sweep_count", Source::PriorArt),
        ("traces[].extra.axis", Source::PriorArt),
        ("traces[].extra.excitation_wavelength_nm", Source::PriorArt),
        ("traces[].extra.information", Source::Inferred),
        ("traces[].extra.integration_time_s", Source::Inferred),
        ("traces[].extra.accumulations", Source::Inferred),
        ("traces[].extra.grating", Source::Inferred),
        ("traces[].extra.objective", Source::Inferred),
        ("traces[].extra.acquired_at", Source::Inferred),
        ("tables[].columns", Source::PriorArt),
        ("images[].size_x", Source::PriorArt),
        ("images[].size_y", Source::PriorArt),
        ("images[].physical_size", Source::PriorArt),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
    Ok(parsed)
}

/// The text records (`TDText`): each is an attachment; the RTF ones are information texts.
fn information_texts(proj: &Project, parsed: &mut Parsed) -> Infos {
    let mut infos = Infos::new();
    for e in proj.entries.iter().filter(|e| e.class == "TDText") {
        let Some(data) = e.tag.path(&["TDStream", "StreamData"]) else {
            continue;
        };
        if let Some(Val::U8(b)) = data.value.as_ref()
            && b.starts_with(b"{\\rtf")
        {
            let text = rtf_text(b);
            infos.insert(e.id, (e.caption.clone(), info_sections(&text)));
        }
        parsed.attachments.push(Attachment {
            name: if e.caption.is_empty() {
                format!("text {}", e.id)
            } else {
                e.caption.clone()
            },
            content_type: "text/rtf".into(),
            extension: "rtf".into(),
            offset: data.start,
            size: data.len(),
            extra: BTreeMap::from([("data_id".to_string(), json!(e.id))]),
        });
    }
    infos
}

/// The information text of a measurement: the `… Information` text with the same caption
/// stem, else the one with the next id.
fn info_for<'a>(infos: &'a Infos, e: &Entry) -> Option<(i64, &'a Sections)> {
    let stem = caption_stem(&e.caption);
    let mut by_stem = infos.iter().filter(|(_, (c, _))| {
        (c.ends_with("Information")) && caption_stem(c) == stem && !stem.is_empty()
    });
    if let Some((id, (_, s))) = by_stem.next() {
        return Some((*id, s));
    }
    infos
        .get(&(e.id + 1))
        .filter(|(c, _)| c.ends_with("Information"))
        .map(|(_, s)| (e.id + 1, s))
}

/// The first information text and excitation wavelength met, for the file's facts.
#[derive(Default)]
struct Firsts<'a> {
    info: Option<&'a Sections>,
    excitation_nm: Option<f64>,
}

/// The size, value type and data array of a graph record, checked against each other.
struct GraphData<'a> {
    width: u32,
    height: u32,
    points: u64,
    stored: Stored,
    data: &'a Tag,
}

impl<'a> GraphData<'a> {
    fn of(e: &Entry, g: &'a Tag) -> std::result::Result<Self, Finding> {
        let (sx, sy, sg) = (
            g.get_i64("SizeX").unwrap_or(-1),
            g.get_i64("SizeY").unwrap_or(-1),
            g.get_i64("SizeGraph").unwrap_or(-1),
        );
        let data = g.path(&["GraphData", "Data"]);
        let stored = g
            .path(&["GraphData", "DataType"])
            .and_then(|t| t.i64s()?.first().copied());
        let (Ok(w), Ok(h), Ok(n)) = (u32::try_from(sx), u32::try_from(sy), u64::try_from(sg))
        else {
            return Err(Finding::error(
                "graph_size",
                format!(
                    "data {} ({}): sizes {sx} × {sy} × {sg} are not valid",
                    e.id, e.caption
                ),
            ));
        };
        let Some(stored_t) = stored.and_then(stored_of) else {
            return Err(Finding::error(
                "graph_value_type",
                format!(
                    "data {} ({}): value type {stored:?} not known",
                    e.id, e.caption
                ),
            ));
        };
        let Some(data) = data else {
            return Err(Finding::error(
                "graph_data_missing",
                format!("data {} ({}): no data array", e.id, e.caption),
            ));
        };
        let count = u64::from(w) * u64::from(h);
        let want = count
            .checked_mul(n)
            .and_then(|v| v.checked_mul(stored_t.size()));
        if want != Some(data.len()) || count == 0 || n == 0 || count > u64::from(u32::MAX) {
            return Err(Finding::error(
                "graph_data_size",
                format!(
                    "data {} ({}): {w} × {h} spectra of {n} {} values need {want:?} bytes, the array holds {}",
                    e.id,
                    e.caption,
                    stored_t.dtype(),
                    data.len()
                ),
            ));
        }
        Ok(GraphData {
            width: w,
            height: h,
            points: n,
            stored: stored_t,
            data,
        })
    }

    fn count(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// The offset of each spectrum, sweep k = x + w·y (along a scan line first); the stored
    /// spectrum index follows the `DataFieldInverted` flag.
    fn offsets(&self, inverted: bool) -> Vec<u64> {
        let (w, h) = (self.width, self.height);
        let row_bytes = self.points * self.stored.size();
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let s = if inverted {
                    u64::from(x) + u64::from(w) * u64::from(y)
                } else {
                    u64::from(y) + u64::from(h) * u64::from(x)
                };
                self.data.start + s * row_bytes
            })
            .collect()
    }
}

/// The scan lines of an image or map that were not completed (`LineValid` false).
fn invalid_lines(g: &Tag, height: u32) -> Vec<u32> {
    g.child("LineValid")
        .and_then(Tag::bools)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .filter(|(_, v)| !**v)
        .filter_map(|(k, _)| u32::try_from(k).ok())
        .filter(|k| *k < height)
        .collect()
}

/// A spectrum graph (`TDGraph`): a spectrum set, with its positions table and map when it
/// is a scan.
fn push_graph<'a>(
    proj: &Project,
    e: &Entry,
    infos: &'a Infos,
    firsts: &mut Firsts<'a>,
    parsed: &mut Parsed,
) {
    let Some(g) = e.body() else {
        parsed.findings.push(Finding::error(
            "graph_record_missing",
            format!("data {} ({}): graph record missing", e.id, e.caption),
        ));
        return;
    };
    let gd = match GraphData::of(e, g) {
        Ok(gd) => gd,
        Err(finding) => {
            parsed.findings.push(finding);
            return;
        }
    };
    let (w, h, n, count) = (gd.width, gd.height, gd.points, gd.count());
    let inverted = g.get_bool("DataFieldInverted").unwrap_or(false);
    let offsets = gd.offsets(inverted);
    let (axis, axis_notes) = proj.axis(g, n);
    for note in axis_notes {
        parsed
            .notes
            .push(format!("{} ({}): {note}", e.caption, e.id));
    }
    let y_unit = proj
        .by_id(g.get_i64("ZInterpretationID"))
        .filter(|z| z.class == "TDZInterpretation")
        .and_then(|z| z.body())
        .and_then(|b| b.get_text("UnitName"))
        .filter(|u| !u.trim().is_empty());
    let mut extra = axis.extra;
    extra.insert("data_id".into(), json!(e.id));
    extra.insert("size_x".into(), json!(w));
    extra.insert("size_y".into(), json!(h));
    extra.insert("value_type".into(), json!(gd.stored.dtype()));
    extra.insert(
        "storage_order".into(),
        json!(if inverted {
            "row-first"
        } else {
            "column-first"
        }),
    );
    let invalid = invalid_lines(g, h);
    if !invalid.is_empty() {
        extra.insert("incomplete_lines".into(), json!(invalid));
        let valid: Vec<bool> = (0..count)
            .map(|k| {
                let y = u32::try_from(k / u64::from(w)).unwrap_or(u32::MAX);
                !invalid.contains(&y)
            })
            .collect();
        parsed.row_valid.insert(parsed.sets.len(), valid);
        parsed.notes.push(format!(
            "{} ({}): {} scan line(s) were not completed; their spectra read as NaN",
            e.caption,
            e.id,
            invalid.len()
        ));
    }
    if let Some((tid, s)) = info_for(infos, e) {
        information_extra(tid, s, &mut extra);
        if firsts.info.is_none() {
            firsts.info = Some(s);
        }
    }
    if firsts.excitation_nm.is_none() {
        firsts.excitation_nm = extra
            .get("excitation_wavelength_nm")
            .and_then(Value::as_f64);
    }
    let set_index = parsed.sets.len();
    let data_id = || BTreeMap::from([("data_id".to_string(), json!(e.id))]);
    if let Some(sp) = proj.space(g.get_i64("SpaceTransformationID")) {
        if count <= 4_000_000 {
            parsed.tables.push(SpectrumTable {
                name: format!("{} positions", e.caption),
                trace: u32::try_from(set_index).unwrap_or(u32::MAX),
                columns: position_columns(&sp, w, count),
            });
        }
        if w > 1 && h > 1 {
            let (px, py) = sp.pixel();
            parsed.maps.push(MapSpec {
                name: e.caption.clone(),
                set: set_index,
                width: w,
                height: h,
                pixels: (0..count).map(|k| u32::try_from(k).ok()).collect(),
                pixel_um: if sp.inverse {
                    (None, None)
                } else {
                    (Some(px), Some(py))
                },
                extra: data_id(),
            });
        }
    } else if w > 1 && h > 1 {
        parsed.maps.push(MapSpec {
            name: e.caption.clone(),
            set: set_index,
            width: w,
            height: h,
            pixels: (0..count).map(|k| u32::try_from(k).ok()).collect(),
            pixel_um: (None, None),
            extra: data_id(),
        });
    }
    parsed.sets.push(SpectrumSet {
        name: if e.caption.is_empty() {
            format!("graph {}", e.id)
        } else {
            e.caption.clone()
        },
        x_quantity: axis.quantity,
        x_unit: axis.unit,
        x: axis.x,
        y_name: "intensity".into(),
        y_unit,
        points: n,
        count: u32::try_from(count).unwrap_or(u32::MAX),
        rows: Rows::Listed(offsets),
        stored: gd.stored,
        scale: 1.0,
        data_type: if axis.quantity == "raman_shift" {
            "RAMAN SPECTRUM"
        } else {
            "SPECTRUM"
        },
        extra,
    });
}

/// The acquisition settings of a measurement's information text (data id `tid`).
fn information_extra(tid: i64, s: &Sections, extra: &mut BTreeMap<String, Value>) {
    extra.insert("information_text".into(), json!(tid));
    extra.insert("information".into(), json!(s));
    for (k, keys) in [
        ("integration_time_s", &["Integration Time [s]"][..]),
        ("accumulations", &["Number Of Accumulations"][..]),
        ("center_wavelength_nm", &["Center Wavelength [nm]"][..]),
        ("objective_magnification", &["Objective Magnification"][..]),
        ("detector_temperature_c", &["Temperature [°C]"][..]),
    ] {
        if let Some(v) = info_num(s, keys) {
            extra.insert(k.into(), num(v));
        }
    }
    for (k, keys) in [
        ("grating", &["Grating"][..]),
        ("objective", &["Objective Name"][..]),
        ("configuration", &["Configuration"][..]),
        ("read_mode", &["ReadMode"][..]),
    ] {
        if let Some(v) = info_get(s, keys) {
            extra.insert(k.into(), json!(v));
        }
    }
    let when = info_get(s, &["Start Date", "Date"])
        .zip(info_get(s, &["Start Time", "Time"]))
        .and_then(|(d, t)| info_datetime(d, t));
    if let Some(t) = when {
        extra.insert("acquired_at".into(), json!(t));
    }
}

/// Pixel and stage positions of each spectrum of a scan `w` pixels wide.
fn position_columns(sp: &Space, w: u32, count: u64) -> Vec<(String, Option<String>, Vec<f64>)> {
    let mut cols: Vec<(String, Option<String>, Vec<f64>)> = vec![
        ("x_px".into(), None, Vec::new()),
        ("y_px".into(), None, Vec::new()),
    ];
    let u = if sp.inverse { "1/µm" } else { "µm" };
    for c in ["x", "y", "z"] {
        cols.push((
            format!("{c}_{}", if sp.inverse { "per_um" } else { "um" }),
            Some(u.into()),
            Vec::new(),
        ));
    }
    for k in 0..count {
        let x = (k % u64::from(w)) as f64;
        let y = (k / u64::from(w)) as f64;
        let p = sp.at(x, y);
        cols[0].2.push(x);
        cols[1].2.push(y);
        for c in 0..3 {
            cols[2 + c].2.push(p[c]);
        }
    }
    cols
}

/// The pixel size of a space transformation, when it is in µm.
fn pixel_um(sp: Option<Space>) -> (Option<f64>, Option<f64>) {
    sp.filter(|s| !s.inverse).map_or((None, None), |s| {
        let (a, b) = s.pixel();
        (Some(a), Some(b))
    })
}

/// A measured image (`TDImage`) as a raster.
fn push_image(proj: &Project, e: &Entry, parsed: &mut Parsed) {
    let Some(g) = e.body() else { return };
    let (sx, sy) = (
        g.get_i64("SizeX").unwrap_or(-1),
        g.get_i64("SizeY").unwrap_or(-1),
    );
    let data = g.path(&["ImageData", "Data"]);
    let stored = g
        .path(&["ImageData", "DataType"])
        .and_then(|t| t.i64s()?.first().copied())
        .and_then(stored_of);
    let (Ok(w), Ok(h), Some(data), Some(st)) = (u32::try_from(sx), u32::try_from(sy), data, stored)
    else {
        parsed.findings.push(Finding::error(
            "image_record",
            format!(
                "data {} ({}): image size, value type or data missing",
                e.id, e.caption
            ),
        ));
        return;
    };
    let want = u64::from(w) * u64::from(h) * st.size();
    if want != data.len() || w == 0 || h == 0 {
        parsed.findings.push(Finding::error(
            "image_data_size",
            format!(
                "data {} ({}): {w} × {h} {} values need {want} bytes, the array holds {}",
                e.id,
                e.caption,
                st.dtype(),
                data.len()
            ),
        ));
        return;
    }
    let inverted = g.get_bool("ImageDataIsInverted").unwrap_or(false);
    let invalid_rows = invalid_lines(g, h);
    let sp = proj.space(g.get_i64("PositionTransformationID"));
    let mut extra = BTreeMap::new();
    extra.insert("data_id".into(), json!(e.id));
    extra.insert("kind".into(), json!("image"));
    extra.insert(
        "storage_order".into(),
        json!(if inverted {
            "row-first"
        } else {
            "column-first"
        }),
    );
    if let Some(v) = g.get_f64("Average") {
        extra.insert("stored_mean".into(), num(v));
    }
    if let Some(v) = g.get_f64("Deviation") {
        extra.insert("stored_deviation".into(), num(v));
    }
    if let Some(z) = proj
        .by_id(g.get_i64("ZInterpretationID"))
        .and_then(|z| z.body())
        .and_then(|b| b.get_text("UnitName"))
    {
        extra.insert("value_unit".into(), json!(z));
    }
    if !invalid_rows.is_empty() {
        extra.insert("incomplete_lines".into(), json!(invalid_rows));
    }
    if let Some(sp) = sp {
        let o = sp.at(0.0, 0.0);
        extra.insert("origin_um".into(), json!([num(o[0]), num(o[1]), num(o[2])]));
    }
    parsed.rasters.push(RasterSpec {
        name: if e.caption.is_empty() {
            format!("image {}", e.id)
        } else {
            e.caption.clone()
        },
        width: w,
        height: h,
        stored: st,
        components: 1,
        layout: if inverted {
            RasterLayout::RowFirst
        } else {
            RasterLayout::ColumnFirst
        },
        offset: data.start,
        invalid_rows,
        pixel_um: pixel_um(sp),
        extra,
    });
}

/// A video image (`TDBitmap`): a BMP stream (versions 0-5; also an attachment) or an RGBX
/// array.
fn push_bitmap(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    proj: &Project,
    e: &Entry,
    parsed: &mut Parsed,
) -> Result<()> {
    let g = e.body();
    let sp = g.and_then(|g| proj.space(g.get_i64("SpaceTransformationID")));
    let mut extra = BTreeMap::new();
    extra.insert("data_id".into(), json!(e.id));
    extra.insert("kind".into(), json!("video image"));
    let name = if e.caption.is_empty() {
        format!("bitmap {}", e.id)
    } else {
        e.caption.clone()
    };
    let pixel_um = pixel_um(sp);
    if let Some(sp) = sp {
        let o = sp.at(0.0, 0.0);
        extra.insert("origin_um".into(), json!([num(o[0]), num(o[1]), num(o[2])]));
    }
    if let Some(stream) = e.tag.path(&["TDStream", "StreamData"]) {
        parsed.attachments.push(Attachment {
            name: name.clone(),
            content_type: "image/bmp".into(),
            extension: "bmp".into(),
            offset: stream.start,
            size: stream.len(),
            extra: BTreeMap::from([("data_id".to_string(), json!(e.id))]),
        });
        let hdr = read_at(f, path, stream.start, stream.len().min(54), file_len)?;
        let u32_at = |k: usize| {
            hdr.get(k..k + 4)
                .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        };
        let i32_at = |k: usize| {
            hdr.get(k..k + 4)
                .map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        };
        let bpp = hdr.get(28..30).map(|s| u16::from_le_bytes([s[0], s[1]]));
        match (
            hdr.starts_with(b"BM"),
            u32_at(10),
            i32_at(18),
            i32_at(22),
            bpp,
            u32_at(30),
        ) {
            (true, Some(off), Some(bw), Some(bh), Some(24), Some(0)) if bw > 0 && bh > 0 => {
                let r = RasterSpec {
                    name,
                    width: bw.unsigned_abs(),
                    height: bh.unsigned_abs(),
                    stored: Stored::U8,
                    components: 3,
                    layout: RasterLayout::Bmp24BottomUp,
                    offset: stream.start + u64::from(off),
                    invalid_rows: Vec::new(),
                    pixel_um,
                    extra,
                };
                if r.stored_bytes()
                    .and_then(|n| n.checked_add(u64::from(off)))
                    .is_some_and(|end| end <= stream.len())
                {
                    parsed.rasters.push(r);
                } else {
                    parsed.findings.push(Finding::error(
                        "bitmap_size",
                        format!("data {}: the BMP is shorter than its header says", e.id),
                    ));
                }
            }
            _ => parsed.notes.push(format!(
                "{} ({}): bitmap is not an uncompressed 24-bit BMP; it is an attachment only",
                e.caption, e.id
            )),
        }
        return Ok(());
    }
    let Some(g) = g else { return Ok(()) };
    let (sx, sy) = (
        g.get_i64("SizeX").unwrap_or(-1),
        g.get_i64("SizeY").unwrap_or(-1),
    );
    let data = g.path(&["BitmapData", "Data"]);
    let (Ok(w), Ok(h), Some(data)) = (u32::try_from(sx), u32::try_from(sy), data) else {
        parsed.findings.push(Finding::error(
            "bitmap_record",
            format!("data {} ({}): bitmap size or data missing", e.id, e.caption),
        ));
        return Ok(());
    };
    if u64::from(w) * u64::from(h) * 4 != data.len() || w == 0 || h == 0 {
        parsed.findings.push(Finding::error(
            "bitmap_data_size",
            format!(
                "data {} ({}): {w} × {h} pixels need {} bytes, the array holds {}",
                e.id,
                e.caption,
                u64::from(w) * u64::from(h) * 4,
                data.len()
            ),
        ));
        return Ok(());
    }
    parsed.rasters.push(RasterSpec {
        name,
        width: w,
        height: h,
        stored: Stored::U8,
        components: 3,
        layout: RasterLayout::RgbxRowFirst,
        offset: data.start,
        invalid_rows: Vec::new(),
        pixel_um,
        extra,
    });
    Ok(())
}

/// The file's experiment facts: the application and system, the first excitation wavelength
/// and the first information text.
fn project_facts(root: &Tag, firsts: &Firsts) -> Facts {
    let first_child = |path: &[&str]| {
        root.path(path)
            .and_then(|t| t.children.first())
            .map(|t| t.name.clone())
            .unwrap_or_default()
    };
    let app = first_child(&["SystemInformation", "ApplicationVersions"]);
    let system_id = first_child(&["SystemInformation", "SystemID"]);
    let mut facts = Facts::default();
    Facts::text(&mut facts.vendor, "WITec", "format");
    let (sw, swv) = split_app(&app);
    Facts::text(
        &mut facts.software,
        &sw,
        "SystemInformation ApplicationVersions",
    );
    Facts::text(
        &mut facts.software_version,
        &swv,
        "SystemInformation ApplicationVersions",
    );
    Facts::text(&mut facts.serial, &system_id, "SystemInformation SystemID");
    if let Some(e) = firsts.excitation_nm {
        facts.number(
            "laser_wavelength",
            e,
            Some("nm"),
            "spectral interpretation excitation wavelength",
            Source::PriorArt,
        );
    }
    if let Some(s) = firsts.info {
        information_facts(s, &mut facts);
    }
    facts
}

/// Facts from the first information text.
fn information_facts(s: &Sections, facts: &mut Facts) {
    let inf = Source::Inferred;
    if let Some(v) = info_get(s, &["System ID"]) {
        Facts::text(&mut facts.serial, v, "information text System ID");
    }
    if let Some(v) = info_get(s, &["User Name"]) {
        Facts::text(&mut facts.operator, v, "information text User Name");
    }
    if let Some(v) = info_get(s, &["Sample Name"]) {
        Facts::text(&mut facts.sample_name, v, "information text Sample Name");
    }
    if let Some(t) = info_get(s, &["Start Date", "Date"])
        .zip(info_get(s, &["Start Time", "Time"]))
        .and_then(|(d, t)| info_datetime(d, t))
    {
        Facts::text(
            &mut facts.started_at,
            &t,
            "information text Start Date/Start Time (local time)",
        );
    }
    if let Some(v) = info_num(s, &["Excitation Wavelength [nm]"]) {
        facts.number(
            "laser_wavelength",
            v,
            Some("nm"),
            "information text Excitation Wavelength [nm]",
            inf,
        );
    }
    for (k, keys, unit) in [
        ("integration_time", &["Integration Time [s]"][..], Some("s")),
        ("accumulations", &["Number Of Accumulations"][..], None),
        (
            "center_wavelength",
            &["Center Wavelength [nm]"][..],
            Some("nm"),
        ),
        (
            "objective_magnification",
            &["Objective Magnification"][..],
            None,
        ),
        ("points_per_line", &["Points per Line"][..], None),
        ("lines_per_image", &["Lines per Image"][..], None),
        ("scan_width", &["Scan Width [µm]"][..], Some("µm")),
        ("scan_height", &["Scan Height [µm]"][..], Some("µm")),
    ] {
        if let Some(v) = info_num(s, keys) {
            facts.number(k, v, unit, &format!("information text {}", keys[0]), inf);
        }
    }
    for (k, keys) in [
        ("grating", &["Grating"][..]),
        ("objective", &["Objective Name"][..]),
        ("configuration", &["Configuration"][..]),
    ] {
        if let Some(v) = info_get(s, keys) {
            facts.word(k, v, &format!("information text {}", keys[0]), inf);
        }
    }
}

/// `Control FIVE 5.1.12.68 (Plus Version)` → (`WITec Control FIVE`, `5.1.12.68`);
/// `WITec Control 1.60.3.3` → (`WITec Control`, `1.60.3.3`).
fn split_app(app: &str) -> (String, String) {
    let words: Vec<&str> = app.split_whitespace().collect();
    let Some(k) = words
        .iter()
        .position(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.'))
    else {
        return (app.trim().to_string(), String::new());
    };
    let name = words[..k].join(" ");
    let name = if name.is_empty() || name.starts_with("WITec") {
        name
    } else {
        format!("WITec {name}")
    };
    (name, words[k].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tag record: name, type, then its data at the following offset.
    fn tag(at: u64, name: &str, ty: u32, data: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&u32::try_from(name.len()).unwrap().to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.extend_from_slice(&ty.to_le_bytes());
        let start = at + b.len() as u64 + 16;
        b.extend_from_slice(&start.to_le_bytes());
        b.extend_from_slice(&(start + data.len() as u64).to_le_bytes());
        b.extend_from_slice(data);
        b
    }

    #[test]
    fn grating_equation_is_monotonic_and_centred() {
        // parameters of a 600 g/mm grating in the corpus (axt_standard.wip)
        let g = Grating {
            nc: 800.0,
            lambda_c: 595.455_566,
            gamma: 0.432_980_84,
            delta: -0.003_697_43,
            m: 1.0,
            d: 1_666.666_626,
            x: 0.016,
            f: 295.113_8,
        };
        let at = |i: f64| g.nm(i);
        assert!((at(800.0) - 595.455_566).abs() < 1e-6, "{}", at(800.0));
        assert!(at(0.0) < at(1599.0));
        // Raman shift of the centre pixel for 532.2 nm excitation
        let rs = from_nm(3, at(800.0), 532.2);
        assert!((rs - 1996.06).abs() < 0.1, "{rs}");
    }

    #[test]
    fn rtf_and_information() {
        let rtf = b"{\\rtf1\\ansi{\\fonttbl{\\f0 Arial;}}\\f0 General:\\par Start Time:\\tab 10:25:35 AM\\par Start Date:\\tab Thursday, April 16, 2020\\par Temperature [\\'b0C]:\\tab -59\\par}";
        let t = rtf_text(rtf);
        assert!(!t.contains("Arial"), "{t}");
        let s = info_sections(&t);
        assert_eq!(info_num(&s, &["Temperature [°C]"]), Some(-59.0));
        let when = info_datetime(
            info_get(&s, &["Start Date"]).unwrap(),
            info_get(&s, &["Start Time"]).unwrap(),
        );
        assert_eq!(when.as_deref(), Some("2020-04-16T10:25:35"));
        assert_eq!(
            info_datetime("Monday, June 16, 2014", "4:45:30 PM").as_deref(),
            Some("2014-06-16T16:45:30")
        );
        assert_eq!(info_datetime("nonsense", "4:45:30 PM"), None);
    }

    #[test]
    fn captions_and_versions() {
        assert_eq!(caption_stem("Scan_000_Spec.Data 1_F (Sub BG)"), "Scan_000");
        assert_eq!(caption_stem("Scan LA--037--Spec.Data 1"), "Scan LA--037");
        assert_eq!(caption_stem("Scan LA--037--Information"), "Scan LA--037");
        assert_eq!(
            caption_stem("Reduced<Image Scan 1 (Data)"),
            "Reduced<Image Scan 1"
        );
        assert_eq!(
            caption_stem("Single Spectrum_000 Information"),
            "Single Spectrum_000"
        );
        assert_eq!(
            split_app("Control FIVE 5.1.12.68 (Plus Version)"),
            ("WITec Control FIVE".into(), "5.1.12.68".into())
        );
        assert_eq!(
            split_app("WITec Control 1.60.3.3"),
            ("WITec Control".into(), "1.60.3.3".into())
        );
    }

    #[test]
    fn tag_tree_and_bounds() {
        let v = tag(8, "Version", 5, &7i32.to_le_bytes());
        let mut file = b"WIT_PR06".to_vec();
        let root_hdr_len = 4 + "WITec Project".len() as u64 + 20;
        let children_start = 8 + root_hdr_len;
        let v = {
            let _ = v;
            tag(children_start, "Version", 5, &7i32.to_le_bytes())
        };
        let mut root = Vec::new();
        root.extend_from_slice(&13u32.to_le_bytes());
        root.extend_from_slice(b"WITec Project");
        root.extend_from_slice(&0u32.to_le_bytes());
        root.extend_from_slice(&children_start.to_le_bytes());
        root.extend_from_slice(&(children_start + v.len() as u64).to_le_bytes());
        file.extend_from_slice(&root);
        file.extend_from_slice(&v);
        let src = openreadout_core::source::MemSource::new("t.wip", file.clone());
        let sf = SourceFile::new(std::sync::Arc::new(src));
        let p = parse(&sf, Path::new("t.wip"), file.len() as u64).unwrap();
        assert_eq!(p.format_version.as_deref(), Some("7"));
        assert!(p.sets.is_empty());
        // an end offset past the parent is refused, not followed
        let mut bad = file.clone();
        let n = bad.len();
        let e = u64::from_le_bytes(bad[n - 4 - 8..n - 4].try_into().unwrap()) + 100;
        bad[n - 4 - 8..n - 4].copy_from_slice(&e.to_le_bytes());
        let src = openreadout_core::source::MemSource::new("t.wip", bad.clone());
        let sf = SourceFile::new(std::sync::Arc::new(src));
        assert!(parse(&sf, Path::new("t.wip"), bad.len() as u64).is_err());
        // truncated anywhere: an error, never a panic
        for cut in 0..file.len() {
            let src = openreadout_core::source::MemSource::new("t.wip", file[..cut].to_vec());
            let sf = SourceFile::new(std::sync::Arc::new(src));
            let _ = parse(&sf, Path::new("t.wip"), cut as u64);
        }
    }
}
