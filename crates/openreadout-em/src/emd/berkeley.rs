//! NCEM/Berkeley EMD (the open "Electron Microscopy Dataset" HDF5 convention, versions 0.2 and
//! 1.0): data groups marked by an `emd_group_type` attribute (1 in 0.2, `"array"` in 1.0) hold a
//! `data` array and one calibration vector per dimension (`dim1`…`dimN` in 0.2, `dim0`… in 1.0).
//! Layout, axis rule and vocabulary: `docs/formats/emd.md` ("Berkeley EMD").

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hdf5_pure::{AttrValue, DType, Datatype, DatatypeByteOrder};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo, LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::Input;
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use super::{EmdReader, FORMAT_ID};
use crate::util::{num, swap_samples};

/// Deepest group nesting searched for data groups, and most groups visited.
const MAX_DEPTH: usize = 16;
const MAX_GROUPS: usize = 20_000;

fn h5err(e: impl std::fmt::Display) -> Error {
    Error::corrupt(FORMAT_ID, format!("HDF5: {e}"))
}

/// One calibration vector (`dimK`) of a data group.
#[derive(Debug, Clone, PartialEq)]
pub struct EmdDim {
    /// `name` attribute (`x`, `y`, `Number`, `Rx`, ...).
    pub name: Option<String>,
    /// `units` attribute with brackets removed (`m`, `nm`, `1/nm`, ...).
    pub units: Option<String>,
    /// Value of the first element and step between elements, when the vector gives them.
    pub first: Option<f64>,
    pub step: Option<f64>,
    /// Stored length of the vector (the data dimension's size, or 2 for a start/step pair).
    pub length: u64,
}

/// One data group: an N-dimensional array with its calibrations.
#[derive(Debug, Clone, PartialEq)]
pub struct EmdArray {
    /// HDF5 path of the group.
    pub group: String,
    /// HDF5 path of the array dataset (`data`, or the one data-sized dataset of older
    /// py4DSTEM/Prismatic files).
    pub dataset: String,
    /// Dataset shape, slowest first.
    pub shape: Vec<u64>,
    pub pixel_type: Option<PixelType>,
    pub big_endian: bool,
    /// Calibration vectors in data-dimension order.
    pub dims: Vec<EmdDim>,
    /// Scalar attributes of the group (`record_by`, `signal_type`, ...).
    pub attrs: Map<String, Value>,
}

impl EmdArray {
    fn size(&self, k: usize) -> u64 {
        self.shape.get(k).copied().unwrap_or(1).max(1)
    }

    fn is_length(&self, k: usize) -> bool {
        self.dims
            .get(k)
            .and_then(|d| d.units.as_deref())
            .and_then(length_to_um)
            .is_some()
    }

    /// A 3-D array whose first two dimensions are lengths and whose last is not (a Prismatic
    /// virtual-detector stack: positions x positions x detector bins): the last dimension is
    /// channels, X the second and Y the first.
    pub fn last_is_channels(&self) -> bool {
        self.shape.len() == 3 && self.is_length(0) && self.is_length(1) && !self.is_length(2)
    }

    fn leading_is_z(&self) -> bool {
        self.shape.len() == 3 && !self.last_is_channels() && self.is_length(0)
    }

    /// X, Y, C, Z, T. X = the last dimension, Y = the one before; leading dimensions are
    /// flattened into T (Z for a 3-D array whose first calibration is a length), except
    /// [`last_is_channels`](Self::last_is_channels) arrays.
    fn geometry(&self) -> (u32, u32, u32, u32, u32) {
        let n = self.shape.len();
        let c = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
        if self.last_is_channels() {
            return (c(self.size(1)), c(self.size(0)), c(self.size(2)), 1, 1);
        }
        let x = if n >= 1 { self.size(n - 1) } else { 1 };
        let y = if n >= 2 { self.size(n - 2) } else { 1 };
        let lead: u64 = (0..n.saturating_sub(2))
            .map(|k| self.size(k))
            .fold(1, u64::saturating_mul);
        if self.leading_is_z() {
            (c(x), c(y), 1, c(lead), 1)
        } else {
            (c(x), c(y), 1, 1, c(lead))
        }
    }

    /// Data dimensions shown as X and Y.
    fn xy_dims(&self) -> (Option<usize>, Option<usize>) {
        let n = self.shape.len();
        if self.last_is_channels() {
            (Some(1), Some(0))
        } else {
            (n.checked_sub(1), n.checked_sub(2))
        }
    }

    /// Micrometres per pixel along dimension `k`, when its calibration is a plausible length.
    fn um(&self, k: usize) -> Option<f64> {
        self.length_step_um(k)
            .filter(|v| *v > 0.0 && *v < MAX_PIXEL_UM)
    }

    fn length_step_um(&self, k: usize) -> Option<f64> {
        let d = self.dims.get(k)?;
        Some(d.step?.abs() * length_to_um(d.units.as_deref()?)?)
    }

    /// Dimensions shown as X, Y or Z whose step is labelled a length but is 1 mm or more per
    /// pixel: not a pixel size (ncempy's conversions of TIA diffraction patterns label their
    /// reciprocal-metre steps `m`).
    fn implausible_lengths(&self) -> Vec<usize> {
        let (xd, yd) = self.xy_dims();
        let zd = self.leading_is_z().then_some(0);
        [xd, yd, zd]
            .into_iter()
            .flatten()
            .filter(|&k| self.length_step_um(k).is_some_and(|v| v >= MAX_PIXEL_UM))
            .collect()
    }
}

/// Steps of this many micrometres (1 mm) or more per pixel are not pixel sizes.
const MAX_PIXEL_UM: f64 = 1e3;

/// Micrometres per unit of a Berkeley EMD length unit (`m`, `nm`, `n_m`, `Å`, ...).
pub fn length_to_um(u: &str) -> Option<f64> {
    Some(match u.trim() {
        "m" => 1e6,
        "mm" => 1e3,
        "um" | "µm" | "μm" | "u_m" | "micrometer" => 1.0,
        "nm" | "n_m" | "nanometer" => 1e-3,
        "A" | "Å" | "angstrom" | "Angstrom" => 1e-4,
        "pm" | "p_m" => 1e-6,
        _ => return None,
    })
}

fn clean_units(s: &str) -> Option<String> {
    let t = s
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn attr_text(v: &AttrValue) -> Option<String> {
    match v {
        AttrValue::String(s) | AttrValue::AsciiString(s) => {
            Some(s.trim_end_matches('\0').to_string())
        }
        AttrValue::StringSized { value, .. } | AttrValue::AsciiStringSized { value, .. } => {
            Some(value.trim_end_matches('\0').to_string())
        }
        AttrValue::StringArray(a) | AttrValue::AsciiStringArray(a) if a.len() == 1 => {
            Some(a[0].trim_end_matches('\0').to_string())
        }
        AttrValue::U8Array(b) => std::str::from_utf8(b)
            .ok()
            .map(|s| s.trim_end_matches('\0').to_string()),
        _ => None,
    }
}

fn attr_json(v: &AttrValue) -> Value {
    if let Some(t) = attr_text(v) {
        return Value::from(t);
    }
    match v {
        AttrValue::F32(x) => num(f64::from(*x)),
        AttrValue::F64(x) => num(*x),
        AttrValue::I8(x) => json!(x),
        AttrValue::I16(x) => json!(x),
        AttrValue::I32(x) => json!(x),
        AttrValue::I64(x) => json!(x),
        AttrValue::U8(x) => json!(x),
        AttrValue::U16(x) => json!(x),
        AttrValue::U32(x) => json!(x),
        AttrValue::U64(x) => json!(x),
        AttrValue::F64Array(a) if a.len() == 1 => num(a[0]),
        AttrValue::F32Array(a) if a.len() == 1 => num(f64::from(a[0])),
        AttrValue::I64Array(a) if a.len() == 1 => json!(a[0]),
        AttrValue::I32Array(a) if a.len() == 1 => json!(a[0]),
        AttrValue::U32Array(a) if a.len() == 1 => json!(a[0]),
        _ => Value::Null,
    }
}

/// `emd_group_type` marks a data group: 1 (EMD 0.2) or `"array"` (EMD 1.0).
fn is_data_group(v: &AttrValue) -> bool {
    match attr_text(v) {
        Some(t) => t.trim() == "array" || t.trim() == "1",
        None => matches!(attr_json(v).as_i64(), Some(1)),
    }
}

fn big_endian(d: &Datatype) -> bool {
    match d {
        Datatype::FixedPoint { byte_order, .. } | Datatype::FloatingPoint { byte_order, .. } => {
            *byte_order == DatatypeByteOrder::BigEndian
        }
        Datatype::Compound { members, .. } => {
            members.first().is_some_and(|m| big_endian(&m.datatype))
        }
        _ => false,
    }
}

fn pixel_type(d: &DType) -> Option<PixelType> {
    Some(match d {
        DType::U8 => PixelType::Uint8,
        DType::U16 => PixelType::Uint16,
        DType::U32 => PixelType::Uint32,
        DType::U64 => PixelType::Uint64,
        DType::I8 => PixelType::Int8,
        DType::I16 => PixelType::Int16,
        DType::I32 => PixelType::Int32,
        DType::I64 => PixelType::Int64,
        DType::F32 => PixelType::Float,
        DType::F64 => PixelType::Double,
        // complex numbers as h5py writes them: a compound of two same-typed floats
        DType::Compound(f) if f.len() == 2 && f[0].1 == DType::F32 && f[1].1 == DType::F32 => {
            PixelType::ComplexFloat
        }
        DType::Compound(f) if f.len() == 2 && f[0].1 == DType::F64 && f[1].1 == DType::F64 => {
            PixelType::ComplexDouble
        }
        _ => return None,
    })
}

/// `dimK` → `K`.
fn dim_number(name: &str) -> Option<u32> {
    let d = name.strip_prefix("dim")?;
    (!d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        .then(|| d.parse().ok())
        .flatten()
}

/// Whether the root (or its first-level groups) carries EMD markers.
pub(crate) fn looks_like_berkeley(file: &hdf5_pure::File) -> bool {
    let root = file.root();
    if root
        .attrs()
        .is_ok_and(|a| a.contains_key("version_major") || a.contains_key("emd_group_type"))
    {
        return true;
    }
    root.groups().unwrap_or_default().iter().any(|g| {
        file.group(g)
            .and_then(|g| g.attrs())
            .is_ok_and(|a| a.contains_key("emd_group_type") || a.contains_key("version_major"))
    })
}

fn read_dim(file: &hdf5_pure::File, path: &str, data_len: u64) -> EmdDim {
    let ds = file.dataset(path).ok();
    let attrs = ds.as_ref().and_then(|d| d.attrs().ok()).unwrap_or_default();
    let values = ds
        .as_ref()
        .and_then(|d| d.read_f64().ok())
        .unwrap_or_default();
    let (first, step) = match values.as_slice() {
        [a, b, ..]
            if (values.len() as u64 == data_len || values.len() == 2) && (b - a).is_finite() =>
        {
            (Some(*a), Some(b - a))
        }
        [a] => (Some(*a), None),
        _ => (None, None),
    };
    EmdDim {
        name: attrs
            .get("name")
            .and_then(attr_text)
            .filter(|s| !s.is_empty()),
        units: attrs
            .get("units")
            .and_then(attr_text)
            .and_then(|u| clean_units(&u)),
        first,
        step,
        length: values.len() as u64,
    }
}

/// Every data group of the file, in path order.
pub(crate) fn find_arrays(file: &hdf5_pure::File) -> Vec<EmdArray> {
    let mut out = Vec::new();
    let mut stack: Vec<(String, usize)> = vec![(String::new(), 0)];
    let mut visited = 0usize;
    while let Some((path, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_GROUPS {
            break;
        }
        let g = if path.is_empty() {
            file.root()
        } else {
            match file.group(&path) {
                Ok(g) => g,
                Err(_) => continue,
            }
        };
        let attrs = g.attrs().unwrap_or_default();
        let sub = |n: &str| {
            if path.is_empty() {
                n.to_string()
            } else {
                format!("{path}/{n}")
            }
        };
        if attrs.get("emd_group_type").is_some_and(is_data_group) {
            let names = g.datasets().unwrap_or_default();
            let mut dim_names: Vec<(u32, String)> = names
                .iter()
                .filter_map(|n| dim_number(n).map(|k| (k, n.clone())))
                .collect();
            dim_names.sort();
            let data_name = if names.iter().any(|n| n == "data") {
                Some("data".to_string())
            } else {
                // py4DSTEM 0.x / Prismatic: `datacube`, `realslice`, `diffractionslice`, ...
                let cands: Vec<&String> = names
                    .iter()
                    .filter(|n| dim_number(n).is_none() && !n.starts_with("dim"))
                    .collect();
                (cands.len() == 1).then(|| cands[0].clone())
            };
            if let Some(dn) = data_name {
                let dpath = sub(&dn);
                if let Ok(ds) = file.dataset(&dpath) {
                    let shape = ds.shape().unwrap_or_default();
                    let dims = dim_names
                        .iter()
                        .enumerate()
                        .map(|(i, (_, n))| {
                            read_dim(file, &sub(n), shape.get(i).copied().unwrap_or(0))
                        })
                        .collect();
                    let mut a = Map::new();
                    // HDF5 attributes come back in a hash map; insert them sorted so the
                    // output does not change from run to run.
                    let sorted: BTreeMap<&String, &AttrValue> = attrs.iter().collect();
                    for (k, v) in sorted {
                        let j = attr_json(v);
                        if !j.is_null() {
                            a.insert(k.clone(), j);
                        }
                    }
                    out.push(EmdArray {
                        group: path.clone(),
                        dataset: dpath,
                        shape,
                        pixel_type: ds.dtype().ok().as_ref().and_then(pixel_type),
                        big_endian: ds.datatype().is_ok_and(|d| big_endian(&d)),
                        dims,
                        attrs: a,
                    });
                }
            }
            continue; // a data group's children are its own
        }
        if depth < MAX_DEPTH {
            let mut kids = g.groups().unwrap_or_default();
            kids.sort();
            for k in kids.into_iter().rev() {
                stack.push((sub(&k), depth + 1));
            }
        }
    }
    out.sort_by(|a, b| a.group.cmp(&b.group));
    out
}

/// Attributes of a top-level metadata group (`microscope`, `sample`, `user`, `comments`).
fn group_attrs(file: &hdf5_pure::File, name: &str) -> Map<String, Value> {
    let mut m = Map::new();
    if let Ok(a) = file.group(name).and_then(|g| g.attrs()) {
        // sorted: the attributes come back in a hash map, whose order changes per run
        let sorted: BTreeMap<String, AttrValue> = a.into_iter().collect();
        for (k, v) in sorted {
            let j = attr_json(&v);
            if !j.is_null() && j != "" {
                m.insert(k, j);
            }
        }
    }
    m
}

/// An opened Berkeley EMD file.
pub struct BerkeleyDataset {
    path: PathBuf,
    size: u64,
    file: hdf5_pure::File,
    arrays: Vec<EmdArray>,
    version: Option<String>,
    microscope: Map<String, Value>,
    other: BTreeMap<String, Map<String, Value>>,
}

impl std::fmt::Debug for BerkeleyDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BerkeleyDataset")
            .field("path", &self.path)
            .field("arrays", &self.arrays)
            .finish_non_exhaustive()
    }
}

impl BerkeleyDataset {
    /// Open a Berkeley EMD file already opened as HDF5.
    pub(crate) fn from_file(input: &Input, file: hdf5_pure::File) -> Result<Self> {
        let path = input.path();
        let size = input
            .fs()
            .metadata(path)
            .map_err(|e| Error::io(path, e))?
            .len();
        let arrays = find_arrays(&file);
        if arrays.is_empty() {
            return Err(Error::unsupported(
                FORMAT_ID,
                "a Berkeley EMD file without data groups",
                "No group carries emd_group_type = 1 (EMD 0.2) or \"array\" (EMD 1.0) with a data array; `openreadout info --view structure` lists the HDF5 tree.",
            ));
        }
        let root = file.root().attrs().unwrap_or_default();
        let major = root.get("version_major").map(attr_json);
        let minor = root.get("version_minor").map(attr_json);
        let version = match (major, minor) {
            (Some(a), Some(b)) => Some(format!(
                "{}.{}",
                a.as_str().map_or_else(|| a.to_string(), str::to_string),
                b.as_str().map_or_else(|| b.to_string(), str::to_string)
            )),
            _ => None,
        };
        let microscope = group_attrs(&file, "microscope");
        let mut other = BTreeMap::new();
        for g in ["sample", "user", "comments"] {
            let m = group_attrs(&file, g);
            if !m.is_empty() {
                other.insert(g.to_string(), m);
            }
        }
        Ok(BerkeleyDataset {
            path: path.to_path_buf(),
            size,
            file,
            arrays,
            version,
            microscope,
            other,
        })
    }

    /// The data groups found.
    pub fn arrays(&self) -> &[EmdArray] {
        &self.arrays
    }

    fn array(&self, image: u32) -> Result<&EmdArray> {
        self.arrays.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image {image} out of range (file has {} images)",
                self.arrays.len()
            ))
        })
    }

    #[allow(clippy::many_single_char_names)]
    fn image_info(&self, index: u32, a: &EmdArray) -> ImageInfo {
        let (sx, sy, sc, sz, st) = a.geometry();
        let mut im = ImageInfo::new(index, sx, sy, a.pixel_type.unwrap_or(PixelType::Uint8));
        im.size_c = sc;
        im.size_z = sz;
        im.size_t = st;
        im.name = Some(a.group.clone());
        let n = a.shape.len();
        let (xd, yd) = a.xy_dims();
        im.physical_size = PhysicalSize::micrometres(
            xd.and_then(|k| a.um(k)),
            yd.and_then(|k| a.um(k)),
            if a.leading_is_z() { a.um(0) } else { None },
        );
        if a.last_is_channels() {
            let d = a.dims.get(2);
            im.channels = (0..sc.min(1 << 16))
                .map(|k| openreadout_core::model::ChannelInfo {
                    index: k,
                    name: Some(match (d.and_then(|d| d.first), d.and_then(|d| d.step)) {
                        (Some(f), Some(s)) => format!(
                            "{} {}",
                            num(f + f64::from(k) * s),
                            d.and_then(|d| d.units.clone()).unwrap_or_default()
                        )
                        .trim_end()
                        .to_string(),
                        _ => format!("channel {k}"),
                    }),
                    ..openreadout_core::model::ChannelInfo::default()
                })
                .collect();
        }
        let voltage = self
            .microscope
            .get("AcceleratingVoltage")
            .and_then(Value::as_f64)
            .or_else(|| self.microscope.get("voltage").and_then(Value::as_f64));
        let model = self
            .microscope
            .get("Microscope")
            .or_else(|| self.microscope.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if model.is_some() {
            im.instrument = Some(InstrumentInfo {
                model,
                ..InstrumentInfo::default()
            });
        }
        let ex = &mut im.extra;
        ex.insert("emd_group".into(), Value::from(a.group.clone()));
        ex.insert("emd_dataset".into(), Value::from(a.dataset.clone()));
        ex.insert("shape".into(), json!(a.shape));
        ex.insert(
            "dims".into(),
            Value::Array(
                a.dims
                    .iter()
                    .map(|d| {
                        json!({"name": d.name, "units": d.units, "first": d.first.map(num),
                               "step": d.step.map(num), "length": d.length})
                    })
                    .collect(),
            ),
        );
        if n > 2 && !a.last_is_channels() {
            ex.insert("frame_grid".into(), json!(a.shape[..n - 2].to_vec()));
        }
        if !a.attrs.is_empty() {
            ex.insert("group_attributes".into(), Value::Object(a.attrs.clone()));
        }
        if !self.microscope.is_empty() {
            ex.insert("microscope".into(), Value::Object(self.microscope.clone()));
        }
        if let Some(v) = voltage.filter(|v| *v > 0.0) {
            ex.insert("voltage_kv".into(), num(v / 1000.0));
        }
        if a.pixel_type.is_none() {
            ex.insert("undecoded_type".into(), Value::Bool(true));
        }
        im.finish()
    }
}

impl Dataset for BerkeleyDataset {
    fn info(&self) -> Result<FileInfo> {
        let images: Vec<ImageInfo> = self
            .arrays
            .iter()
            .enumerate()
            .map(|(i, a)| self.image_info(i as u32, a))
            .collect();
        let mut notes = vec![format!(
            "Berkeley EMD{}: {} data group(s); X is each array's last dimension, Y the one before, leading dimensions are T (Z for a 3-D array whose first calibration is a length; a 3-D array of two lengths and a detector axis has the detector axis as channels)",
            self.version
                .as_deref()
                .map_or(String::new(), |v| format!(" {v}")),
            self.arrays.len()
        )];
        for a in &self.arrays {
            if a.pixel_type.is_none() {
                notes.push(format!(
                    "{}: the array's HDF5 type is not a number type; reading it exits 6",
                    a.dataset
                ));
            }
            let odd = a.implausible_lengths();
            if !odd.is_empty() {
                notes.push(format!(
                    "{}: the step of dimension(s) {} is labelled a length but is 1 mm or more per pixel, so it is not reported as a pixel size (a diffraction pattern converted from TIA carries reciprocal metres labelled `m`); the calibration stays in extra.dims",
                    a.group,
                    odd.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
                ));
            }
            if a.dims.len() != a.shape.len() {
                notes.push(format!(
                    "{}: {} calibration vectors for {} dimensions",
                    a.group,
                    a.dims.len(),
                    a.shape.len()
                ));
            }
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: EmdReader.descriptor(),
            format_version: Some(match &self.version {
                Some(v) => format!("Berkeley EMD {v}"),
                None => "Berkeley EMD".into(),
            }),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut m = Map::new();
        m.insert("microscope".into(), Value::Object(self.microscope.clone()));
        for (k, v) in &self.other {
            m.insert(k.clone(), Value::Object(v.clone()));
        }
        m.insert(
            "arrays".into(),
            Value::Array(
                self.arrays
                    .iter()
                    .map(|a| json!({"group": a.group, "dataset": a.dataset, "shape": a.shape, "attributes": a.attrs}))
                    .collect(),
            ),
        );
        Ok(Value::Object(m))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Spec),
            ("images[].size_x", Source::Inferred),
            ("images[].size_y", Source::Inferred),
            ("images[].size_t", Source::Inferred),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::Spec),
            ("images[].extra.dims", Source::Spec),
            ("images[].extra.microscope", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .arrays
            .iter()
            .enumerate()
            .map(|(i, a)| LsEntry {
                kind: "image".into(),
                name: a.dataset.clone(),
                offset: None,
                size: None,
                image: Some(i as u32),
                details: json!({"shape": a.shape, "dims": a.dims.len()}),
            })
            .collect())
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let a = self.array(image)?.clone();
        let Some(pt) = a.pixel_type else {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("{}: a non-numeric HDF5 array", a.dataset),
                "Only integer, float and complex (two-float compound) Berkeley EMD arrays are read.",
            ));
        };
        let (sx, sy, sc, sz, st) = a.geometry();
        if index.c >= sc || index.z >= sz || index.t >= st {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image} (c<{sc}, z<{sz}, t<{st})",
                index.c, index.z, index.t
            )));
        }
        let bps = pt.bytes_per_sample() as u64;
        let plane_bytes =
            openreadout_core::pixel::plane_bytes_checked(FORMAT_ID, sx, sy, pt.bytes_per_sample())?
                as u64;
        let ds = self.file.dataset(&a.dataset).map_err(h5err)?;
        let n = a.shape.len();
        let plane = u64::from(index.z.max(index.t));
        let mut data = if a.last_is_channels() {
            // (Y, X, C): gather channel c row by row
            let (cn, w) = (a.size(2), u64::from(sx));
            let mut out = Vec::with_capacity(usize::try_from(plane_bytes).unwrap_or(0));
            for y in 0..u64::from(sy) {
                let row = ds.read_raw_rows(y, 1).map_err(h5err)?;
                for x in 0..w {
                    let at =
                        usize::try_from((x * cn + u64::from(index.c)) * bps).unwrap_or(usize::MAX);
                    out.extend_from_slice(
                        row.get(at..at.saturating_add(bps as usize))
                            .ok_or_else(|| {
                                Error::corrupt(
                                    FORMAT_ID,
                                    format!("{}: array shorter than its shape", a.dataset),
                                )
                            })?,
                    );
                }
            }
            out
        } else if n <= 2 {
            ds.read_raw().map_err(h5err)?
        } else {
            // one row of the first dimension holds prod(shape[1..]) elements
            let per_row: u64 = a.shape[1..].iter().product::<u64>().max(1);
            let planes_per_row = per_row / (u64::from(sx) * u64::from(sy)).max(1);
            let row = plane / planes_per_row.max(1);
            let within = plane % planes_per_row.max(1);
            let buf = ds.read_raw_rows(row, 1).map_err(h5err)?;
            let at = usize::try_from(within * plane_bytes).unwrap_or(usize::MAX);
            buf.get(at..at.saturating_add(plane_bytes as usize))
                .map(<[u8]>::to_vec)
                .ok_or_else(|| {
                    Error::corrupt(
                        FORMAT_ID,
                        format!("{}: array shorter than its shape", a.dataset),
                    )
                })?
        };
        if data.len() as u64 != plane_bytes {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "{}: {} bytes where {plane_bytes} were expected",
                    a.dataset,
                    data.len()
                ),
            ));
        }
        if a.big_endian {
            let comp = if pt.is_complex() { bps / 2 } else { bps };
            swap_samples(&mut data, usize::try_from(comp).unwrap_or(1));
        }
        Ok(Plane {
            width: sx,
            height: sy,
            pixel_type: pt,
            samples_per_pixel: 1,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("Berkeley EMD: every data group has a numeric array and one calibration vector per dimension");
        for a in &self.arrays {
            if a.pixel_type.is_none() {
                r.push(Finding::warning(
                    "unsupported_type",
                    format!("{}: the array is not of a number type", a.dataset),
                ));
            }
            if a.dims.len() != a.shape.len() {
                r.push(Finding::warning(
                    "dims",
                    format!(
                        "{}: {} calibration vectors for {} dimensions",
                        a.group,
                        a.dims.len(),
                        a.shape.len()
                    ),
                ));
            }
            for (k, d) in a.dims.iter().enumerate() {
                let want = a.shape.get(k).copied().unwrap_or(0);
                if d.length != want && d.length != 2 {
                    r.push(Finding::info(
                        "dim_length",
                        format!(
                            "{}: dim vector {} holds {} values for a dimension of {want}",
                            a.group,
                            k + 1,
                            d.length
                        ),
                    ));
                }
            }
        }
        Ok(r)
    }
}

/// Open a file as Berkeley EMD (after the Velox reader found no Velox data).
pub(crate) fn open(input: &Input) -> Result<BerkeleyDataset> {
    let (path, fs): (&Path, _) = (input.path(), input.fs());
    let opened = if fs.is_local() {
        hdf5_pure::File::open_streaming(path)
    } else {
        let src = fs.source(path).map_err(|e| Error::io(path, e))?;
        let src = crate::util::H5Source::new(src).map_err(|e| Error::io(path, e))?;
        hdf5_pure::File::from_source(src)
    };
    let file = opened.map_err(|e| match e {
        hdf5_pure::Error::Io(io) => Error::io(path, io),
        other => h5err(other),
    })?;
    BerkeleyDataset::from_file(input, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_names_and_units() {
        assert_eq!(dim_number("dim1"), Some(1));
        assert_eq!(dim_number("dim0"), Some(0));
        assert_eq!(dim_number("dim3_time"), None);
        assert_eq!(dim_number("data"), None);
        assert_eq!(clean_units("[n_m]").as_deref(), Some("n_m"));
        assert_eq!(clean_units("[]"), None);
        assert_eq!(length_to_um("n_m"), Some(1e-3));
        assert_eq!(length_to_um("m"), Some(1e6));
        assert_eq!(length_to_um("A^-1"), None);
    }

    /// A Berkeley EMD 0.2 file whose data group and `microscope` group carry many attributes.
    fn many_attrs_file() -> hdf5_pure::File {
        let names: Vec<String> = (0..24)
            .map(|k| format!("attr_{:02}", (k * 7) % 24))
            .collect();
        let mut fb = hdf5_pure::FileBuilder::new();
        fb.set_attr("version_major", AttrValue::I64(0));
        fb.set_attr("version_minor", AttrValue::I64(2));
        let mut data = fb.create_group("data");
        let mut g = data.create_group("a");
        g.set_attr("emd_group_type", AttrValue::I64(1));
        for n in &names {
            g.set_attr(n, AttrValue::VarLenString(n.clone()));
        }
        g.create_dataset("data")
            .with_u16_data(&[0; 6])
            .with_shape(&[2, 3]);
        g.create_dataset("dim1").with_f64_data(&[0.0, 1.0]);
        g.create_dataset("dim2").with_f64_data(&[0.0, 1.0, 2.0]);
        data.add_group(g.finish());
        fb.add_group(data.finish());
        let mut m = fb.create_group("microscope");
        for n in &names {
            m.set_attr(n, AttrValue::F64(1.5));
        }
        fb.add_group(m.finish());
        hdf5_pure::File::from_bytes(fb.finish().unwrap()).unwrap()
    }

    /// HDF5 attributes come back in a hash map whose order changes with every map (and every
    /// process); the JSON built from them must not (it made `info` differ between runs).
    #[test]
    fn attributes_are_listed_in_a_stable_order() {
        let file = many_attrs_file();
        let keys = |m: &Map<String, Value>| m.keys().cloned().collect::<Vec<_>>();
        let first = find_arrays(&file);
        assert_eq!(first.len(), 1);
        let group = keys(&first[0].attrs);
        let microscope = keys(&group_attrs(&file, "microscope"));
        let mut sorted = group.clone();
        sorted.sort();
        assert_eq!(group, sorted);
        assert_eq!(microscope.len(), 24);
        assert!(microscope.windows(2).all(|w| w[0] < w[1]));
        for _ in 0..8 {
            assert_eq!(keys(&find_arrays(&file)[0].attrs), group);
            assert_eq!(keys(&group_attrs(&file, "microscope")), microscope);
        }
    }

    fn dim(units: &str, step: f64) -> EmdDim {
        EmdDim {
            name: None,
            units: Some(units.into()),
            first: Some(0.0),
            step: Some(step),
            length: 2048,
        }
    }

    /// `ncem-emd-pt-saed-d910mm-single`: a diffraction pattern whose `m` axes step 8.9e6 per
    /// pixel (reciprocal metres); not a pixel size.
    #[test]
    fn steps_of_a_millimetre_or_more_are_not_pixel_sizes() {
        let mut a = EmdArray {
            group: "data/im01_1.ser".into(),
            dataset: "data/im01_1.ser/data".into(),
            shape: vec![1, 2048, 2048],
            pixel_type: Some(PixelType::Int32),
            big_endian: false,
            dims: vec![
                dim("", 1.0),
                dim("m", 8_894_687.457),
                dim("m", 8_894_687.457),
            ],
            attrs: Map::new(),
        };
        assert_eq!(a.um(2), None);
        assert_eq!(a.um(1), None);
        assert_eq!(a.implausible_lengths(), vec![2, 1]);
        a.dims = vec![dim("", 1.0), dim("nm", 0.02), dim("nm", 0.02)];
        assert!((a.um(2).unwrap() - 2e-5).abs() < 1e-12);
        assert!(a.implausible_lengths().is_empty());
    }

    #[test]
    fn geometry_takes_the_last_two_dimensions() {
        let a = EmdArray {
            group: "g".into(),
            dataset: "g/data".into(),
            shape: vec![7, 1024, 512],
            pixel_type: Some(PixelType::Uint16),
            big_endian: false,
            dims: vec![],
            attrs: Map::new(),
        };
        assert_eq!(a.geometry(), (512, 1024, 1, 1, 7));
    }
}
