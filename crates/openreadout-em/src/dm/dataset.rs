//! `Dataset` for DM3/DM4 files: images from `ImageList`, thumbnails as attachments.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::time::{filetime_to_iso8601, unix_to_iso8601};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Value, json};

use super::dm5::{self, Dm5Data};
use super::tags::{DmHeader, ParseIssue, Parser, Tag, TagGroup, TagValue};
use super::{DmReader, FORMAT_ID};
use crate::util::{Blob, num, swap_samples};

/// Image `DataType` codes (the value of `ImageData.DataType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageDataType {
    Int16,
    Float32,
    Complex8,
    PackedComplex,
    Uint8,
    Int32,
    Rgb,
    Int8,
    Uint16,
    Uint32,
    Float64,
    Complex16,
    Binary,
    Rgba,
    /// 27: the non-redundant half of the Fourier transform of a real image, complex64.
    PackedComplex8,
    /// 28: the same with complex128 values.
    PackedComplex16,
    Int64,
    Uint64,
    Other(i64),
}

impl ImageDataType {
    pub fn from_code(c: i64) -> ImageDataType {
        match c {
            1 => ImageDataType::Int16,
            2 => ImageDataType::Float32,
            3 => ImageDataType::Complex8,
            5 => ImageDataType::PackedComplex,
            6 => ImageDataType::Uint8,
            7 => ImageDataType::Int32,
            8 => ImageDataType::Rgb,
            9 => ImageDataType::Int8,
            10 => ImageDataType::Uint16,
            11 => ImageDataType::Uint32,
            12 => ImageDataType::Float64,
            13 => ImageDataType::Complex16,
            14 => ImageDataType::Binary,
            23 => ImageDataType::Rgba,
            27 => ImageDataType::PackedComplex8,
            28 => ImageDataType::PackedComplex16,
            35 => ImageDataType::Int64,
            36 => ImageDataType::Uint64,
            other => ImageDataType::Other(other),
        }
    }

    /// Stored bytes per element of the `Data` array.
    pub fn bytes_per_pixel(self) -> Option<u64> {
        Some(match self {
            ImageDataType::Uint8 | ImageDataType::Int8 | ImageDataType::Binary => 1,
            ImageDataType::Int16 | ImageDataType::Uint16 => 2,
            ImageDataType::Float32
            | ImageDataType::Int32
            | ImageDataType::Uint32
            | ImageDataType::PackedComplex
            | ImageDataType::Rgb
            | ImageDataType::Rgba => 4,
            ImageDataType::Float64
            | ImageDataType::Complex8
            | ImageDataType::PackedComplex8
            | ImageDataType::Int64
            | ImageDataType::Uint64 => 8,
            ImageDataType::Complex16 | ImageDataType::PackedComplex16 => 16,
            ImageDataType::Other(_) => return None,
        })
    }

    /// The sample type we return, when we decode this data type.
    pub fn pixel_type(self) -> Option<PixelType> {
        Some(match self {
            ImageDataType::Int16 => PixelType::Int16,
            ImageDataType::Float32 => PixelType::Float,
            ImageDataType::Uint8 | ImageDataType::Binary | ImageDataType::Rgba => PixelType::Uint8,
            ImageDataType::Int32 => PixelType::Int32,
            ImageDataType::Int8 => PixelType::Int8,
            ImageDataType::Uint16 => PixelType::Uint16,
            ImageDataType::Uint32 => PixelType::Uint32,
            ImageDataType::Float64 => PixelType::Double,
            ImageDataType::Complex8 | ImageDataType::PackedComplex8 => PixelType::ComplexFloat,
            ImageDataType::Complex16 | ImageDataType::PackedComplex16 => PixelType::ComplexDouble,
            ImageDataType::Int64 => PixelType::Int64,
            ImageDataType::Uint64 => PixelType::Uint64,
            ImageDataType::PackedComplex | ImageDataType::Rgb | ImageDataType::Other(_) => {
                return None;
            }
        })
    }

    /// Samples per pixel we return: 3 for RGBA (the alpha byte carries no data), else 1.
    pub fn samples_per_pixel(self) -> u32 {
        if self == ImageDataType::Rgba { 3 } else { 1 }
    }

    /// Bytes of one number inside an element, the unit of big-endian byte swapping.
    fn component_bytes(self) -> usize {
        match self {
            ImageDataType::Complex8 | ImageDataType::PackedComplex8 => 4,
            ImageDataType::Complex16 | ImageDataType::PackedComplex16 => 8,
            ImageDataType::Rgba => 1,
            other => other
                .bytes_per_pixel()
                .and_then(|b| usize::try_from(b).ok())
                .unwrap_or(1),
        }
    }

    /// Stores half of a Fourier transform (DataType 27, 28).
    pub fn packed_half_plane(self) -> bool {
        matches!(
            self,
            ImageDataType::PackedComplex8 | ImageDataType::PackedComplex16
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            ImageDataType::Int16 => "int16",
            ImageDataType::Float32 => "float32",
            ImageDataType::Complex8 => "complex64",
            ImageDataType::PackedComplex => "packed-complex",
            ImageDataType::Uint8 => "uint8",
            ImageDataType::Int32 => "int32",
            ImageDataType::Rgb => "rgb",
            ImageDataType::Int8 => "int8",
            ImageDataType::Uint16 => "uint16",
            ImageDataType::Uint32 => "uint32",
            ImageDataType::Float64 => "float64",
            ImageDataType::Complex16 => "complex128",
            ImageDataType::Binary => "binary",
            ImageDataType::Rgba => "rgba",
            ImageDataType::PackedComplex8 => "packed-complex64",
            ImageDataType::PackedComplex16 => "packed-complex128",
            ImageDataType::Int64 => "int64",
            ImageDataType::Uint64 => "uint64",
            ImageDataType::Other(_) => "unknown",
        }
    }
}

/// One axis calibration (`Calibrations.Dimension[i]`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Calibration {
    pub scale: Option<f64>,
    pub origin: Option<f64>,
    pub units: String,
}

/// One `ImageList` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct DmImage {
    /// Position in `ImageList`.
    pub list_index: usize,
    pub name: Option<String>,
    pub data_type: ImageDataType,
    /// `ImageData.Dimensions`, fastest axis first.
    pub dimensions: Vec<u64>,
    pub calibrations: Vec<Calibration>,
    /// Byte offset and length of the `ImageData.Data` array.
    pub data_offset: Option<u64>,
    pub data_length: u64,
    /// Listed in `Thumbnails` (`ImageIndex`).
    pub thumbnail: bool,
    /// `ImageTags.Meta Data.Format` ("Image", "Spectrum", "Spectrum image", ...).
    pub meta_format: Option<String>,
    /// `ImageTags.Meta Data.Signal` ("EELS", "X-ray", "CL", ...).
    pub meta_signal: Option<String>,
    /// `ImageTags.Meta Data.IsSequence`.
    pub is_sequence: bool,
    /// `ImageData.Calibrations.Brightness.Units`.
    pub intensity_units: Option<String>,
    /// DM5: stored bytes per pixel when they differ from the DataType's (an RGBA thumbnail
    /// stored as an HDF5 RGB image, 3 bytes per pixel).
    pub stored_pixel_bytes: Option<u64>,
}

/// Where each stored dimension of an image goes (indices into `DmImage::dimensions`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AxisMap {
    /// The dimensions returned as X and as Y.
    pub x: Option<usize>,
    pub y: Option<usize>,
    /// The spectral dimension returned as channels (C).
    pub spectral: Option<usize>,
    /// A single spectrum: the spectral dimension is X (an image one pixel high) and a trace.
    pub single_spectrum: Option<usize>,
    /// Further dimensions, fastest first, flattened into Z (when `stack_is_z`) or T.
    pub stack: Vec<usize>,
    pub stack_is_z: bool,
}

/// A calibrated value for a channel name: at most 6 decimals, no trailing zeros.
fn short_number(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

fn energy_unit(u: &str) -> bool {
    matches!(u.trim(), "eV" | "keV" | "meV")
}

impl DmImage {
    fn size(&self, k: usize) -> u64 {
        self.dimensions.get(k).copied().unwrap_or(1).max(1)
    }

    /// Dimensions of the stored `Data` array: a packed half plane holds X/2 + 1 columns.
    pub fn stored_dimensions(&self) -> Vec<u64> {
        let mut d: Vec<u64> = self.dimensions.iter().map(|v| (*v).max(1)).collect();
        if self.data_type.packed_half_plane()
            && let Some(x) = d.first_mut()
        {
            *x = *x / 2 + 1;
        }
        d
    }

    fn stored_size(&self, k: usize) -> u64 {
        self.stored_dimensions().get(k).copied().unwrap_or(1)
    }

    fn element_count(&self) -> Option<u64> {
        self.stored_dimensions()
            .iter()
            .try_fold(1u64, |acc, d| acc.checked_mul(*d))
    }

    /// Stored bytes of one element (pixel) of the `Data` array.
    fn element_bytes(&self) -> Option<u64> {
        self.stored_pixel_bytes
            .or_else(|| self.data_type.bytes_per_pixel())
    }

    fn expected_length(&self) -> Option<u64> {
        self.element_count()?.checked_mul(self.element_bytes()?)
    }

    fn units(&self, k: usize) -> &str {
        self.calibrations.get(k).map_or("", |c| c.units.as_str())
    }

    fn format_is(&self, what: &str) -> bool {
        self.meta_format
            .as_deref()
            .is_some_and(|f| f.trim().eq_ignore_ascii_case(what))
    }

    /// The axis mapping (docs/formats/dm.md, "Axes").
    pub fn axes(&self) -> AxisMap {
        let n = self.dimensions.len();
        let spectral = if self.format_is("spectrum image") {
            match n {
                3 => Some(2),
                2 => Some(0),
                _ => None,
            }
        } else if self.format_is("spectrum") {
            (n == 1 || n == 2).then_some(0)
        } else if n == 3 && energy_unit(self.units(2)) {
            Some(2)
        } else if (n == 1 || n == 2) && energy_unit(self.units(0)) {
            Some(0)
        } else {
            None
        };
        let rest: Vec<usize> = (0..n).filter(|k| Some(*k) != spectral).collect();
        let mut m = AxisMap::default();
        if let Some(s) = spectral
            && rest.iter().all(|&k| self.size(k) == 1)
        {
            // a single spectrum stays an image one pixel high (the other dimensions are all 1)
            m.single_spectrum = Some(s);
            m.x = Some(s);
            m.y = rest.first().copied();
            m.stack = rest.iter().skip(1).copied().collect();
            return m;
        }
        m.spectral = spectral;
        m.x = rest.first().copied();
        m.y = rest.get(1).copied();
        m.stack = rest.iter().skip(2).copied().collect();
        m.stack_is_z = spectral.is_none()
            && !self.is_sequence
            && n == 3
            && length_to_um(self.units(2)).is_some();
        m
    }

    /// X, Y, C, Z, T sizes of the returned image.
    fn geometry(&self) -> (u32, u32, u32, u32, u32) {
        let m = self.axes();
        let d = |k: Option<usize>| k.map_or(1, |k| self.stored_size(k));
        let clamp = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
        let stack: u64 = m
            .stack
            .iter()
            .map(|&k| self.stored_size(k))
            .fold(1u64, u64::saturating_mul);
        let (z, t) = if m.stack_is_z { (stack, 1) } else { (1, stack) };
        (
            clamp(d(m.x)),
            clamp(d(m.y)),
            clamp(d(m.spectral)),
            clamp(z),
            clamp(t),
        )
    }

    /// Element strides (in elements) of the stored dimensions, fastest first.
    fn strides(&self) -> Vec<u64> {
        let mut out = Vec::new();
        let mut acc = 1u64;
        for d in self.stored_dimensions() {
            out.push(acc);
            acc = acc.saturating_mul(d);
        }
        out
    }

    /// Calibrated value of index `i` along dimension `k`: (i - Origin) x Scale.
    fn axis_value(&self, k: usize, i: u64) -> Option<f64> {
        let c = self.calibrations.get(k)?;
        Some((i as f64 - c.origin.unwrap_or(0.0)) * c.scale?)
    }

    /// What a spectral axis measures, from `Meta Data.Signal` and its unit.
    fn spectral_quantity(&self, k: usize) -> &'static str {
        let signal = self
            .meta_signal
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        let unit = self.units(k).trim();
        if signal == "eels" {
            "energy loss"
        } else if energy_unit(unit) {
            "energy"
        } else if signal == "cl" || length_to_um(unit).is_some() {
            "wavelength"
        } else {
            "spectral axis"
        }
    }

    /// `{quantity, unit, first, step, size, dimension}` of a spectral dimension.
    fn spectral_axis_json(&self, k: usize) -> Value {
        let c = self.calibrations.get(k);
        json!({
            "quantity": self.spectral_quantity(k),
            "unit": self.units(k),
            "first": self.axis_value(k, 0).map(num),
            "step": c.and_then(|c| c.scale).map(num),
            "size": self.size(k),
            "dimension": k,
        })
    }
}

/// Micrometres per unit for length units; `None` for anything else (reciprocal space, eV, ...).
pub fn length_to_um(units: &str) -> Option<f64> {
    Some(match units.trim() {
        "nm" => 1e-3,
        "µm" | "μm" | "um" | "micron" => 1.0,
        "Å" | "A" | "angstrom" | "Angstrom" => 1e-4,
        "pm" => 1e-6,
        "mm" => 1e3,
        "m" => 1e6,
        _ => return None,
    })
}

/// Largest byte span read at once when gathering a strided plane.
const GATHER_WINDOW: u64 = 64 << 20;

/// An opened DM3/DM4 file.
#[derive(Debug)]
pub struct DmDataset {
    path: PathBuf,
    blob: Blob,
    header: DmHeader,
    root: TagGroup,
    images: Vec<DmImage>,
    /// Indices into `images` exposed as images (non-thumbnails first).
    exposed: Vec<usize>,
    issues: Vec<ParseIssue>,
    complete: bool,
    parse_end: u64,
    /// DM5 (HDF5) files: the HDF5 file and where each image's pixels are.
    dm5: Option<Dm5State>,
}

/// The HDF5 side of an opened DM5 file.
struct Dm5State {
    file: hdf5_pure::File,
    data: HashMap<usize, Dm5Data>,
    /// Pixels of chunked or compact datasets, read whole on first use (by list index).
    cache: HashMap<usize, Vec<u8>>,
}

impl std::fmt::Debug for Dm5State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dm5State")
            .field("data", &self.data)
            .finish_non_exhaustive()
    }
}

/// Largest chunked or compact DM5 `Data` dataset read into memory.
const DM5_MEMORY_LIMIT: u64 = 2 << 30;

fn h5err(e: impl std::fmt::Display) -> Error {
    Error::corrupt(FORMAT_ID, format!("DM5 (HDF5): {e}"))
}

fn as_f64(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn as_text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn scalar_list(g: Option<&TagGroup>) -> Vec<u64> {
    g.map(|g| {
        g.entries
            .iter()
            .filter_map(|(_, t)| match t {
                Tag::Data(TagValue::Json(v)) => v.as_u64(),
                _ => None,
            })
            .collect()
    })
    .unwrap_or_default()
}

fn read_images(root: &TagGroup) -> Vec<DmImage> {
    let thumbs: Vec<u64> = root
        .group("Thumbnails")
        .map(|g| {
            g.children()
                .iter()
                .filter_map(|t| t.value("ImageIndex").and_then(Value::as_u64))
                .collect()
        })
        .unwrap_or_default();
    let Some(list) = root.group("ImageList") else {
        return Vec::new();
    };
    list.children()
        .into_iter()
        .enumerate()
        .map(|(k, entry)| {
            let data = entry.group("ImageData");
            let dims = scalar_list(data.and_then(|d| d.group("Dimensions")));
            let calibrations = data
                .and_then(|d| d.path("Calibrations.Dimension"))
                .and_then(|t| match t {
                    Tag::Group(g) => Some(g),
                    Tag::Data(_) => None,
                })
                .map(|g| {
                    g.children()
                        .iter()
                        .map(|c| Calibration {
                            scale: as_f64(c.value("Scale")),
                            origin: as_f64(c.value("Origin")),
                            units: as_text(c.value("Units")).unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let (data_offset, data_length) = match data.and_then(|d| d.get("Data")) {
                Some(Tag::Data(TagValue::Array { offset, length, .. })) => (Some(*offset), *length),
                _ => (None, 0),
            };
            let meta = |p: &str| entry.value(&format!("ImageTags.Meta Data.{p}"));
            DmImage {
                meta_format: as_text(meta("Format")),
                meta_signal: as_text(meta("Signal")),
                is_sequence: meta("IsSequence").is_some_and(|v| {
                    v.as_bool().unwrap_or(false) || v.as_i64().is_some_and(|i| i != 0)
                }),
                intensity_units: as_text(
                    data.and_then(|d| d.value("Calibrations.Brightness.Units")),
                ),
                stored_pixel_bytes: None,
                list_index: k,
                name: as_text(entry.value("Name")),
                data_type: ImageDataType::from_code(
                    data.and_then(|d| d.value("DataType"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                ),
                dimensions: dims,
                calibrations,
                data_offset,
                data_length,
                thumbnail: thumbs.contains(&(k as u64)),
            }
        })
        .collect()
}

/// Entries exposed as images: every non-thumbnail, else every entry.
fn exposed_of(images: &[DmImage]) -> Vec<usize> {
    let exposed: Vec<usize> = (0..images.len())
        .filter(|&k| !images[k].thumbnail)
        .collect();
    if exposed.is_empty() {
        (0..images.len()).collect()
    } else {
        exposed
    }
}

impl DmDataset {
    /// Open a DM3/DM4 file: parses the whole tag directory (pixel arrays are skipped).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs): (&Path, &Fs) = (input.path(), input.fs());
        let mut blob = Blob::open(fs, path)?;
        let head = blob.read_upto(0, 1100)?;
        if crate::emd::looks_like_hdf5(&head) {
            return Self::open_dm5(input, blob);
        }
        let header = DmHeader::parse(&head).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                "file does not start with a DM3/DM4 header (version 3 or 4, byte-order word 0 or 1)",
            )
        })?;
        let (root, issues, complete, parse_end) = {
            let mut p = Parser::new(&mut blob, &header);
            let root = p.root();
            (root, p.issues, p.complete, p.pos)
        };
        if root.entries.is_empty() && !complete {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                header.header_len,
                "the root tag directory cannot be read (file truncated or not a DM file)",
            ));
        }
        let images = read_images(&root);
        let exposed = exposed_of(&images);
        Ok(DmDataset {
            path: path.to_path_buf(),
            blob,
            header,
            root,
            images,
            exposed,
            issues,
            complete,
            parse_end,
            dm5: None,
        })
    }

    /// Open a DM5 file: the HDF5 tree becomes the tag tree; pixels are HDF5 datasets.
    fn open_dm5(input: &Input, blob: Blob) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
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
        let file_len = fs.metadata(path).map_or(u64::MAX, |m| m.len());
        let (root, data, problems) = dm5::read_tree(&file, file_len);
        if root.group("ImageList").is_none() {
            return Err(Error::unsupported(
                FORMAT_ID,
                "HDF5 file without a DM5 ImageList group",
                "This HDF5 file is not a DigitalMicrograph DM5 file; `openreadout info` on it without forcing the format reads it as generic HDF5.",
            ));
        }
        let mut images = read_images(&root);
        for im in &mut images {
            if let Some(d) = data.get(&im.list_index) {
                let bytes = d
                    .shape
                    .iter()
                    .try_fold(d.element_bytes, |a, v| a.checked_mul(*v))
                    .unwrap_or(u64::MAX);
                im.data_length = bytes;
                im.data_offset = d.contiguous.map(|(a, _)| a);
                if let Some(n) = im.element_count().filter(|n| *n > 0)
                    && bytes % n == 0
                    && Some(bytes / n) != im.data_type.bytes_per_pixel()
                {
                    im.stored_pixel_bytes = Some(bytes / n);
                }
            }
        }
        let exposed = exposed_of(&images);
        Ok(DmDataset {
            path: path.to_path_buf(),
            blob,
            header: DmHeader {
                version: 5,
                root_length: 0,
                little_endian: true,
                header_len: 0,
            },
            root,
            images,
            exposed,
            issues: problems
                .into_iter()
                .map(|message| ParseIssue { offset: 0, message })
                .collect(),
            complete: true,
            parse_end: 0,
            dm5: Some(Dm5State {
                file,
                data,
                cache: HashMap::new(),
            }),
        })
    }

    /// `len` bytes of image `im`'s `Data` array from byte `at`, in stored byte order.
    fn data_bytes(&mut self, im: &DmImage, at: u64, len: u64) -> Result<Vec<u8>> {
        let end = at
            .checked_add(len)
            .filter(|e| *e <= im.data_length)
            .ok_or_else(|| {
                Error::corrupt(
                    FORMAT_ID,
                    format!(
                        "ImageData.Data holds {} bytes, fewer than the dimensions {:?} need",
                        im.data_length, im.dimensions
                    ),
                )
            })?;
        if let Some(off) = im.data_offset {
            return self.blob.read_at(FORMAT_ID, off + at, len);
        }
        let Some(state) = self.dm5.as_mut() else {
            return Err(Error::corrupt(
                FORMAT_ID,
                "the image has no ImageData.Data array",
            ));
        };
        if !state.cache.contains_key(&im.list_index) {
            let d = state.data.get(&im.list_index).ok_or_else(|| {
                Error::corrupt(FORMAT_ID, "the image has no ImageData/Data dataset")
            })?;
            if im.data_length > DM5_MEMORY_LIMIT {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!(
                        "a chunked DM5 image of {:.1} GiB",
                        im.data_length as f64 / f64::from(1u32 << 30)
                    ),
                    "Chunked or compressed DM5 images above 2 GiB are not read; save the image as DM4 in DigitalMicrograph.",
                ));
            }
            let raw = state
                .file
                .dataset(&d.path)
                .and_then(|ds| ds.read_raw())
                .map_err(h5err)?;
            state.cache.insert(im.list_index, raw);
        }
        let raw = &state.cache[&im.list_index];
        let (a, b) = (
            usize::try_from(at).unwrap_or(usize::MAX),
            usize::try_from(end).unwrap_or(usize::MAX),
        );
        raw.get(a..b).map(<[u8]>::to_vec).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, "the DM5 Data dataset is shorter than its shape")
        })
    }

    /// Stored numbers are big-endian (DM3/DM4 header byte order, DM5 dataset datatype).
    fn big_endian(&self, im: &DmImage) -> bool {
        match &self.dm5 {
            Some(s) => s.data.get(&im.list_index).is_some_and(|d| d.big_endian),
            None => !self.header.little_endian,
        }
    }

    /// The parsed tag tree.
    pub fn tags(&self) -> &TagGroup {
        &self.root
    }

    /// Every `ImageList` entry, thumbnails included.
    pub fn images(&self) -> &[DmImage] {
        &self.images
    }

    fn image_tags(&self, im: &DmImage) -> Option<&TagGroup> {
        self.root
            .group("ImageList")?
            .children()
            .get(im.list_index)
            .copied()?
            .group("ImageTags")
    }

    fn image_info(&self, index: u32, im: &DmImage) -> ImageInfo {
        let (sx, sy, sc, sz, st) = im.geometry();
        let axes = im.axes();
        let pt = im.data_type.pixel_type().unwrap_or(PixelType::Uint8);
        let mut info = ImageInfo::new(index, sx, sy, pt);
        info.samples_per_pixel = im.data_type.samples_per_pixel();
        info.size_c = sc;
        info.size_z = sz;
        info.size_t = st;
        info.name.clone_from(&im.name);
        let um = |k: Option<usize>| {
            let c = im.calibrations.get(k?)?;
            Some(c.scale? * length_to_um(&c.units)?).filter(|v| *v > 0.0)
        };
        let spatial = |k: Option<usize>| k.filter(|_| axes.single_spectrum.is_none());
        info.physical_size = PhysicalSize::micrometres(
            um(spatial(axes.x)),
            um(spatial(axes.y)),
            if axes.stack_is_z {
                um(axes.stack.first().copied())
            } else {
                None
            },
        );
        let tags = self.image_tags(im);
        let t = |p: &str| tags.and_then(|g| g.value(p));
        let exposure_s =
            as_f64(t("Acquisition.Parameters.High Level.Exposure (s)")).or_else(|| {
                as_f64(t("Acquisition.Frame.Sequence.Exposure Time (ns)")).map(|ns| ns * 1e-9)
            });
        let exposure_ms = exposure_s.filter(|s| *s > 0.0).map(|s| s * 1000.0);
        info.channels = match axes.spectral {
            Some(k) => (0..sc.min(1 << 16))
                .map(|c| ChannelInfo {
                    index: c,
                    name: Some(im.axis_value(k, u64::from(c)).map_or_else(
                        || format!("channel {c}"),
                        |v| {
                            format!("{} {}", short_number(v), im.units(k))
                                .trim_end()
                                .to_string()
                        },
                    )),
                    exposure_ms,
                    ..ChannelInfo::default()
                })
                .collect(),
            None => vec![ChannelInfo {
                index: 0,
                name: im.name.clone(),
                exposure_ms,
                ..ChannelInfo::default()
            }],
        };
        let instrument = InstrumentInfo {
            manufacturer: None,
            model: as_text(t("Microscope Info.Microscope"))
                .or_else(|| as_text(t("Microscope Info.Name"))),
            software: Some("Gatan DigitalMicrograph".into()),
            software_version: as_text(t("GMS Version.Created")),
            detector: as_text(t("Acquisition.Device.Name")),
        };
        info.instrument = Some(instrument);
        info.acquired_at = as_f64(t("DataBar.Acquisition Time (OS)"))
            .filter(|v| *v > 1e17 && *v < 3e18)
            .map(|v| filetime_to_iso8601(v as u64))
            .or_else(|| {
                as_f64(t(
                    "Acquisition.Frame.Sequence.Acquisition Start Time (epoch)",
                ))
                .filter(|ms| *ms > 0.0 && *ms < 2.5e14)
                .map(|ms| {
                    let ms = ms as i64;
                    unix_to_iso8601(ms.div_euclid(1000), ms.rem_euclid(1000) as u32)
                })
            });
        let ex = &mut info.extra;
        ex.insert("data_type".into(), Value::from(im.data_type.label()));
        ex.insert("dimensions".into(), json!(im.dimensions));
        ex.insert(
            "calibrations".into(),
            Value::Array(
                im.calibrations
                    .iter()
                    .map(|c| json!({"scale": c.scale.map(num), "origin": c.origin.map(num), "units": c.units}))
                    .collect(),
            ),
        );
        if let Some(k) = axes.spectral.or(axes.single_spectrum) {
            ex.insert("spectral_axis".into(), im.spectral_axis_json(k));
        }
        if im.dimensions.len() >= 3 && axes.spectral.is_none() {
            ex.insert(
                "third_axis".into(),
                Value::from(if axes.stack_is_z { "z" } else { "t" }),
            );
        }
        if im.dimensions.len() >= 4 && axes.spectral.is_none() {
            // 4-D data: frames of the first two dimensions over a grid of the others
            ex.insert(
                "frame_grid".into(),
                json!(axes.stack.iter().map(|&k| im.size(k)).collect::<Vec<_>>()),
            );
        }
        if let Some(f) = &im.meta_format {
            ex.insert("meta_format".into(), Value::from(f.clone()));
        }
        if let Some(sig) = &im.meta_signal {
            ex.insert("signal".into(), Value::from(sig.clone()));
        }
        if im.is_sequence {
            ex.insert("is_sequence".into(), Value::Bool(true));
        }
        if im.data_type.packed_half_plane() {
            ex.insert("packed_half_plane".into(), Value::Bool(true));
            ex.insert("full_size_x".into(), Value::from(im.size(0)));
        }
        if let Some(u) = &im.intensity_units {
            ex.insert("intensity_units".into(), Value::from(u.clone()));
        }
        ex.insert("image_list_index".into(), Value::from(im.list_index as u64));
        let mut micro = serde_json::Map::new();
        for (ours, path) in [
            ("voltage_v", "Microscope Info.Voltage"),
            (
                "indicated_magnification",
                "Microscope Info.Indicated Magnification",
            ),
            (
                "actual_magnification",
                "Microscope Info.Actual Magnification",
            ),
            ("operation_mode", "Microscope Info.Operation Mode"),
            ("illumination_mode", "Microscope Info.Illumination Mode"),
            ("imaging_mode", "Microscope Info.Imaging Mode"),
            ("stem_camera_length", "Microscope Info.STEM Camera Length"),
            ("cs_mm", "Microscope Info.Cs(mm)"),
            ("probe_current_na", "Microscope Info.Probe Current (nA)"),
            (
                "exposure_s",
                "Acquisition.Parameters.High Level.Exposure (s)",
            ),
            ("binning", "Acquisition.Parameters.High Level.Binning"),
            ("acquisition_date", "DataBar.Acquisition Date"),
            ("acquisition_time", "DataBar.Acquisition Time"),
        ] {
            if let Some(v) = t(path) {
                micro.insert(ours.into(), v.clone());
            }
        }
        if !micro.is_empty() {
            ex.insert("microscope".into(), Value::Object(micro));
        }
        if let Some(v) = as_f64(t("Microscope Info.Voltage")).filter(|v| *v > 0.0) {
            ex.insert("voltage_kv".into(), num(v / 1000.0));
        }
        info.finish()
    }

    /// Exposed images that are single spectra: (image index, `images` index), in image order.
    fn spectrum_images(&self) -> Vec<(u32, usize)> {
        self.exposed
            .iter()
            .enumerate()
            .filter(|(_, k)| {
                let im = &self.images[**k];
                im.axes().single_spectrum.is_some() && im.data_type.pixel_type().is_some()
            })
            .map(|(i, &k)| (i as u32, k))
            .collect()
    }

    fn trace_info(
        &self,
        index: u32,
        image: u32,
        im: &DmImage,
    ) -> openreadout_core::model::TraceInfo {
        let mut extra = BTreeMap::new();
        extra.insert("image".into(), Value::from(image));
        if let Some(k) = im.axes().single_spectrum {
            let mut axis = im.spectral_axis_json(k);
            if let (Some(obj), Some(last)) = (
                axis.as_object_mut(),
                im.axis_value(k, im.size(k).saturating_sub(1)),
            ) {
                obj.insert("last".into(), num(last));
            }
            extra.insert("axis".into(), axis);
        }
        if let Some(sig) = &im.meta_signal {
            extra.insert("signal".into(), Value::from(sig.clone()));
        }
        let (sx, ..) = im.geometry();
        openreadout_core::model::TraceInfo {
            index,
            name: im.name.clone().or_else(|| Some("spectrum".into())),
            sample_rate_hz: 0.0,
            sample_count: u64::from(sx),
            sweep_count: 1,
            channels: vec![openreadout_core::model::SignalChannelInfo {
                index: 0,
                name: "intensity".into(),
                unit: im.intensity_units.clone(),
                dtype: im.data_type.label().into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            }],
            start_s: None,
            extra,
        }
    }

    fn exposed_image(&self, image: u32) -> Result<&DmImage> {
        self.exposed
            .get(image as usize)
            .map(|&k| &self.images[k])
            .ok_or_else(|| {
                Error::Usage(format!(
                    "image index {image} out of range (0..{})",
                    self.exposed.len()
                ))
            })
    }
}

impl Dataset for DmDataset {
    fn info(&self) -> Result<FileInfo> {
        let images: Vec<ImageInfo> = self
            .exposed
            .iter()
            .enumerate()
            .map(|(i, &k)| self.image_info(i as u32, &self.images[k]))
            .collect();
        let mut notes = Vec::new();
        if !self.complete {
            notes.push("the tag directory could not be read to the end (truncated or damaged); run `check`".into());
        }
        let thumbs = self.images.iter().filter(|i| i.thumbnail).count();
        if thumbs > 0 && self.exposed.len() < self.images.len() {
            notes.push(format!(
                "{thumbs} thumbnail image(s) in ImageList are exposed as attachments, not images (`export --attachment`)"
            ));
        }
        for &k in &self.exposed {
            let im = &self.images[k];
            if im.data_type.pixel_type().is_none() {
                notes.push(format!(
                    "image '{}' has DataType {} ({}), which is described but not decoded; reading it exits 6",
                    im.name.as_deref().unwrap_or(""),
                    match im.data_type {
                        ImageDataType::Other(c) => c,
                        _ => 0,
                    },
                    im.data_type.label()
                ));
            }
            let axes = im.axes();
            let label = im.name.as_deref().unwrap_or("");
            if axes.single_spectrum.is_some() {
                notes.push(format!("'{label}' is a spectrum: exposed as an image one pixel high and as a trace (images[].extra.spectral_axis)"));
            } else if im.dimensions.len() == 1 {
                notes.push("1-D data is exposed as an image one pixel high".into());
            }
            if let Some(k) = axes.spectral {
                notes.push(format!(
                    "'{label}' is a spectrum image: its {} axis (stored dimension {k}) is returned as channels, one per bin (images[].extra.spectral_axis)",
                    im.spectral_quantity(k)
                ));
            }
            if im.dimensions.len() > 3 && axes.spectral.is_none() {
                notes.push(format!(
                    "{}-D data: frames of the first two dimensions, the others flattened into T (fastest first; images[].extra.frame_grid)",
                    im.dimensions.len()
                ));
            }
            if im.data_type.packed_half_plane() {
                notes.push(format!("'{label}' is a packed Fourier transform (DataType {}): the stored half plane is returned as it is ({} of {} columns), not mirrored into the full plane", im.data_type.label(), im.stored_dimensions().first().copied().unwrap_or(0), im.size(0)));
            }
            if im.data_type == ImageDataType::Rgba {
                notes.push(format!("'{label}' is an RGBA image: returned as interleaved R, G, B (the alpha byte carries no data)"));
            }
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        let traces = self
            .spectrum_images()
            .into_iter()
            .enumerate()
            .map(|(t, (i, k))| self.trace_info(t as u32, i, &self.images[k]))
            .collect();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.blob.len,
            format: DmReader.descriptor(),
            format_version: Some(format!("DM{}", self.header.version)),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces,
            plane_count,
            notes,
        })
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<openreadout_core::model::Trace> {
        let list = self.spectrum_images();
        let &(image, _) = list.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                list.len()
            ))
        })?;
        if sweep > 0 {
            return Err(Error::Usage(format!("trace {index} has one sweep")));
        }
        let p = self.read_plane(image, PlaneIndex::default())?;
        let values: Vec<f64> = p.pixel_type.samples_f64(&p.data).collect();
        let a = usize::try_from(first_sample)
            .unwrap_or(usize::MAX)
            .min(values.len());
        let b = a
            .saturating_add(usize::try_from(max_samples).unwrap_or(usize::MAX))
            .min(values.len());
        Ok(openreadout_core::model::Trace {
            trace: index,
            sweep: 0,
            first_sample: a as u64,
            channels: vec![values[a..b].to_vec()],
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(self.root.to_json())
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::PriorArt),
            ("images[].size_x", Source::PriorArt),
            ("images[].size_y", Source::PriorArt),
            ("images[].size_z", Source::Inferred),
            ("images[].size_t", Source::Inferred),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].physical_size", Source::PriorArt),
            ("images[].name", Source::PriorArt),
            ("images[].channels[].exposure_ms", Source::Inferred),
            ("images[].instrument", Source::Inferred),
            ("images[].acquired_at", Source::Inferred),
            ("images[].extra.microscope", Source::PriorArt),
            ("images[].extra.calibrations", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = vec![LsEntry {
            kind: "metadata".into(),
            name: "tag-directory".into(),
            offset: Some(self.header.header_len),
            size: Some(self.parse_end.saturating_sub(self.header.header_len)),
            image: None,
            details: json!({"version": self.header.version, "byte_order": if self.header.little_endian { "little-endian" } else { "big-endian" }, "root_length": self.header.root_length, "complete": self.complete}),
        }];
        for im in &self.images {
            let exposed = self
                .exposed
                .iter()
                .position(|&k| self.images[k].list_index == im.list_index);
            out.push(LsEntry {
                kind: if im.thumbnail && exposed.is_none() { "attachment" } else { "image" }.into(),
                name: im.name.clone().unwrap_or_else(|| format!("ImageList[{}]", im.list_index)),
                offset: im.data_offset,
                size: Some(im.data_length),
                image: exposed.map(|i| i as u32),
                details: json!({"image_list_index": im.list_index, "data_type": im.data_type.label(), "dimensions": im.dimensions, "thumbnail": im.thumbnail}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let im = self.exposed_image(image)?.clone();
        let Some(pixel_type) = im.data_type.pixel_type() else {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("image DataType '{}'", im.data_type.label()),
                "Old-style packed-complex (5) and RGB (8) DM images, and DataTypes outside the documented list, are not decoded: their byte layout is not established by any public file. Convert the image in DigitalMicrograph (to a real, complex or RGBA image), or export it as TIFF.",
            ));
        };
        let (sx, sy, sc, sz, st) = im.geometry();
        if index.c >= sc || index.z >= sz || index.t >= st {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image} (c<{sc}, z<{sz}, t<{st})",
                index.c, index.z, index.t
            )));
        }
        let short = || {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "ImageData.Data holds {} bytes, fewer than the dimensions {:?} need",
                    im.data_length, im.dimensions
                ),
            )
        };
        if im.expected_length().is_none_or(|e| e > im.data_length) {
            return Err(short());
        }
        let axes = im.axes();
        let strides = im.strides();
        let dims = im.stored_dimensions();
        let stride = |k: Option<usize>| k.map_or(0, |k| strides[k]);
        // element index of pixel (0, 0): the channel and the stack position
        let mut base = u64::from(index.c) * stride(axes.spectral);
        let mut s = u64::from(index.z.max(index.t));
        for &k in &axes.stack {
            base += (s % dims[k]) * strides[k];
            s /= dims[k];
        }
        let (xs, ys) = (stride(axes.x), stride(axes.y));
        let bpe = im
            .element_bytes()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "unknown element size"))?;
        let (w, h) = (u64::from(sx), u64::from(sy));
        let plane_bytes = openreadout_core::pixel::plane_bytes_checked(
            FORMAT_ID,
            sx,
            sy,
            usize::try_from(bpe).unwrap_or(usize::MAX),
        )? as u64;
        let mut raw: Vec<u8>;
        if xs == 1 && (h == 1 || ys == w) {
            raw = self.data_bytes(&im, base * bpe, plane_bytes)?;
        } else {
            // strided plane (a line-scan spectrum image read per channel): gather row by row
            raw = Vec::with_capacity(usize::try_from(plane_bytes).unwrap_or(0));
            let span = (w - 1) * xs + 1;
            for y in 0..h {
                let row = base + y * ys;
                if span * bpe <= GATHER_WINDOW {
                    let buf = self.data_bytes(&im, row * bpe, span * bpe)?;
                    for x in 0..w {
                        let a = usize::try_from(x * xs * bpe).map_err(|_| short())?;
                        raw.extend_from_slice(buf.get(a..a + bpe as usize).ok_or_else(short)?);
                    }
                } else {
                    for x in 0..w {
                        raw.extend(self.data_bytes(&im, (row + x * xs) * bpe, bpe)?);
                    }
                }
            }
        }
        if self.big_endian(&im) {
            swap_samples(&mut raw, im.data_type.component_bytes());
        }
        let data = match im.data_type {
            ImageDataType::Binary => raw.iter().map(|v| u8::from(*v != 0)).collect(),
            // R, G, B, A bytes: alpha dropped (DM5 may store the three colour bytes only)
            ImageDataType::Rgba if bpe == 4 => raw
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|c| [c[0], c[1], c[2]])
                .collect(),
            _ => raw,
        };
        Ok(Plane {
            width: sx,
            height: sy,
            pixel_type,
            samples_per_pixel: im.data_type.samples_per_pixel(),
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        let dm5 = self.dm5.is_some();
        if dm5 {
            r.performed("DM5: the HDF5 group tree and its attributes read as the DM tag tree");
            r.performed("every ImageList entry has an ImageData/Data dataset whose size matches Dimensions x DataType");
        } else {
            r.performed("header: version 3/4, byte-order word, root length = file length - header - 8 end bytes (or - 4: some writers count half of the end bytes)");
            r.performed(
                "tag directory parses to the end; DM4 tag lengths agree with their contents",
            );
            r.performed("every ImageList entry has a Data array inside the file whose size matches Dimensions x DataType");
        }
        r.performed("Thumbnails.ImageIndex names an ImageList entry");
        let len = self.blob.len;
        let expected = len.saturating_sub(self.header.header_len + 8);
        // Writers differ: the root length is the file length minus the header and the 8 end
        // bytes, or minus the header and 4 of them (rsciio's and Nion's test files, and DM3/DM4
        // files from several depositors: docs/formats/dm.md).
        let counts_half_end = self.header.root_length == expected.saturating_add(4)
            && len >= self.header.header_len + 8;
        if !dm5 && self.header.root_length != expected && !counts_half_end {
            let f = if self.header.root_length > expected {
                Finding::error(
                    "truncated",
                    format!(
                        "root length says {} bytes follow the header, the file has {expected}",
                        self.header.root_length
                    ),
                )
            } else {
                Finding::warning(
                    "root_length",
                    format!(
                        "root length {} differs from file length - header - 8 ({expected})",
                        self.header.root_length
                    ),
                )
            };
            r.push(f.at(4));
        }
        for i in &self.issues {
            let sev_err = !self.complete || dm5;
            let f = if sev_err {
                Finding::error("tag_directory", i.message.clone())
            } else {
                Finding::warning("tag_length", i.message.clone())
            };
            r.push(f.at(i.offset));
        }
        if self.complete && !dm5 {
            let tail = self.blob.read_upto(len.saturating_sub(8), 8)?;
            if self.parse_end + 8 != len || tail.iter().any(|&b| b != 0) {
                r.push(Finding::info("end_marker", format!("tag directory ends at {} and the file at {len}; expected 8 zero bytes after the directory", self.parse_end)));
            }
        }
        if self.images.is_empty() {
            r.push(Finding::warning(
                "no_images",
                "the file has no ImageList entries",
            ));
        }
        let n = self.images.len() as u64;
        if let Some(g) = self.root.group("Thumbnails") {
            for t in g.children() {
                if let Some(k) = t.value("ImageIndex").and_then(Value::as_u64)
                    && k >= n
                {
                    r.push(Finding::warning(
                        "bad_thumbnail_index",
                        format!("Thumbnails.ImageIndex {k} but ImageList has {n} entries"),
                    ));
                }
            }
        }
        for im in &self.images {
            let label = im
                .name
                .clone()
                .unwrap_or_else(|| format!("ImageList[{}]", im.list_index));
            let in_hdf5 = self
                .dm5
                .as_ref()
                .is_some_and(|s| s.data.contains_key(&im.list_index));
            let off = match im.data_offset {
                Some(o) => o,
                None if in_hdf5 => 0,
                None => {
                    r.push(Finding::error(
                        "missing_data",
                        format!("'{label}' has no ImageData.Data array"),
                    ));
                    continue;
                }
            };
            if im.data_offset.is_some() && off.saturating_add(im.data_length) > len {
                r.push(Finding::error("truncated", format!("'{label}': pixel data {}..{} runs past the end of the file ({len} bytes)", off, off + im.data_length)).at(off));
            }
            match im.expected_length() {
                Some(e) if e != im.data_length => r.push(
                    Finding::error(
                        "dimension_mismatch",
                        format!(
                            "'{label}': Dimensions {:?} x {} bytes = {e}, but Data holds {} bytes",
                            im.dimensions,
                            im.element_bytes().unwrap_or(0),
                            im.data_length
                        ),
                    )
                    .at(off),
                ),
                None => r.push(Finding::info(
                    "unknown_data_type",
                    format!(
                        "'{label}': DataType {} is not decoded",
                        im.data_type.label()
                    ),
                )),
                _ => {}
            }
        }
        Ok(r)
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        let mut out = Vec::new();
        for im in &self.images {
            if !im.thumbnail || self.exposed.contains(&im.list_index) {
                continue;
            }
            let mut extra = BTreeMap::new();
            extra.insert("width".into(), Value::from(im.size(0)));
            extra.insert("height".into(), Value::from(im.size(1)));
            extra.insert("data_type".into(), Value::from(im.data_type.label()));
            extra.insert("image_list_index".into(), Value::from(im.list_index as u64));
            out.push(AttachmentInfo {
                index: out.len() as u32,
                name: format!("thumbnail-{}", im.list_index),
                content_type: format!("DM thumbnail, raw {} pixels", im.data_type.label()),
                extension: "raw".into(),
                offset: im.data_offset,
                size: im.data_length,
                extra,
            });
        }
        Ok(out)
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let list = self.attachments()?;
        let a = list.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "attachment #{index} does not exist; `info --view structure` lists them"
            ))
        })?;
        let k = a
            .extra
            .get("image_list_index")
            .and_then(Value::as_u64)
            .and_then(|k| usize::try_from(k).ok())
            .unwrap_or(usize::MAX);
        let im = self
            .images
            .iter()
            .find(|i| i.list_index == k)
            .cloned()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "thumbnail entry vanished"))?;
        self.data_bytes(&im, 0, im.data_length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dm::tags::tests::{data, file, group};

    fn image_file(version: u32, dims: &[u64], dtype: u64, pixels: &[u8]) -> Vec<u8> {
        let elem = match dtype {
            2 => 6,  // f32
            10 => 4, // u16
            _ => 10, // octet
        };
        let bpp = ImageDataType::from_code(dtype as i64)
            .bytes_per_pixel()
            .unwrap();
        let count = pixels.len() as u64 / bpp;
        let dim_tags: Vec<Vec<u8>> = dims
            .iter()
            .map(|d| data(version, "", &[5], &(*d as u32).to_le_bytes()))
            .collect();
        let cal: Vec<Vec<u8>> = dims
            .iter()
            .map(|_| {
                let units: Vec<u8> = "nm".encode_utf16().flat_map(u16::to_le_bytes).collect();
                group(
                    version,
                    "",
                    &[
                        data(version, "Scale", &[6], &0.5f32.to_le_bytes()),
                        data(version, "Origin", &[6], &0f32.to_le_bytes()),
                        data(version, "Units", &[20, 4, 2], &units),
                    ],
                )
            })
            .collect();
        let entry = group(
            version,
            "",
            &[
                group(
                    version,
                    "ImageData",
                    &[
                        group(
                            version,
                            "Calibrations",
                            &[group(version, "Dimension", &cal)],
                        ),
                        data(version, "Data", &[20, elem, count], pixels),
                        data(version, "DataType", &[3], &(dtype as i32).to_le_bytes()),
                        group(version, "Dimensions", &dim_tags),
                    ],
                ),
                group(
                    version,
                    "ImageTags",
                    &[group(
                        version,
                        "Microscope Info",
                        &[data(version, "Voltage", &[7], &300_000f64.to_le_bytes())],
                    )],
                ),
            ],
        );
        file(version, &[group(version, "ImageList", &[entry])])
    }

    /// A DM4 file with one image: `units` per dimension (Origin 1, Scale 0.5) and an optional
    /// `Meta Data.Format`; the pixels are an octet array.
    fn tagged_file(
        dims: &[u64],
        units: &[&str],
        dtype: u64,
        format: Option<&str>,
        pixels: &[u8],
    ) -> Vec<u8> {
        let v = 4;
        let dim_tags: Vec<Vec<u8>> = dims
            .iter()
            .map(|d| data(v, "", &[5], &(*d as u32).to_le_bytes()))
            .collect();
        let cal: Vec<Vec<u8>> = units
            .iter()
            .map(|u| {
                let units: Vec<u8> = u.encode_utf16().flat_map(u16::to_le_bytes).collect();
                group(
                    v,
                    "",
                    &[
                        data(v, "Scale", &[6], &0.5f32.to_le_bytes()),
                        data(v, "Origin", &[6], &1f32.to_le_bytes()),
                        data(v, "Units", &[20, 4, units.len() as u64 / 2], &units),
                    ],
                )
            })
            .collect();
        let mut tags = Vec::new();
        if let Some(f) = format {
            let text: Vec<u8> = f.encode_utf16().flat_map(u16::to_le_bytes).collect();
            tags.push(group(
                v,
                "Meta Data",
                &[data(v, "Format", &[20, 4, text.len() as u64 / 2], &text)],
            ));
        }
        let entry = group(
            v,
            "",
            &[
                group(
                    v,
                    "ImageData",
                    &[
                        group(v, "Calibrations", &[group(v, "Dimension", &cal)]),
                        data(v, "Data", &[20, 10, pixels.len() as u64], pixels),
                        data(v, "DataType", &[3], &(dtype as i32).to_le_bytes()),
                        group(v, "Dimensions", &dim_tags),
                    ],
                ),
                group(v, "ImageTags", &tags),
            ],
        );
        file(v, &[group(v, "ImageList", &[entry])])
    }

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn complex_images_keep_real_imaginary_pairs() {
        let px = f32s(&[1.0, 0.5, 2.0, -1.0]);
        let (_d, mut ds) = open(&tagged_file(&[2, 1], &["nm", "nm"], 3, None, &px));
        let info = ds.info().unwrap();
        assert_eq!(info.images[0].pixel_type, PixelType::ComplexFloat);
        let p = ds.read_plane(0, PlaneIndex::default()).unwrap();
        assert_eq!(p.data, px);
        assert_eq!(p.width, 2);
    }

    #[test]
    fn rgba_images_drop_alpha() {
        let px = [1u8, 2, 3, 0, 4, 5, 6, 255];
        let (_d, mut ds) = open(&tagged_file(&[2, 1], &["", ""], 23, None, &px));
        let info = ds.info().unwrap();
        assert_eq!(info.images[0].samples_per_pixel, 3);
        let p = ds.read_plane(0, PlaneIndex::default()).unwrap();
        assert_eq!((p.samples_per_pixel, p.data), (3, vec![1, 2, 3, 4, 5, 6]));
    }

    #[test]
    fn spectrum_image_energy_is_channels() {
        // x = 2, y = 2, 3 energy bins: element (x, y, e) = x + 2y + 4e
        let px = f32s(&(0..12).map(|v| v as f32).collect::<Vec<_>>());
        let (_d, mut ds) = open(&tagged_file(
            &[2, 2, 3],
            &["nm", "nm", "eV"],
            2,
            Some("Spectrum image"),
            &px,
        ));
        let info = ds.info().unwrap();
        let im = &info.images[0];
        assert_eq!(
            (im.size_x, im.size_y, im.size_c, im.size_z, im.size_t),
            (2, 2, 3, 1, 1)
        );
        assert_eq!(im.physical_size.x, Some(0.5e-3));
        // channel 1 is at (1 - Origin) x Scale = 0 eV
        assert_eq!(im.channels[1].name.as_deref(), Some("0 eV"));
        let p = ds.read_plane(0, PlaneIndex { c: 2, z: 0, t: 0 }).unwrap();
        assert_eq!(p.data, f32s(&[8.0, 9.0, 10.0, 11.0]));
        assert!(info.traces.is_empty());
    }

    #[test]
    fn line_scan_spectrum_image_reads_strided_channels() {
        // stored [energy = 3, position = 2]: element (e, p) = e + 3p
        let px = f32s(&(0..6).map(|v| v as f32).collect::<Vec<_>>());
        let (_d, mut ds) = open(&tagged_file(
            &[3, 2],
            &["nm", "µm"],
            2,
            Some("Spectrum image"),
            &px,
        ));
        let info = ds.info().unwrap();
        let im = &info.images[0];
        assert_eq!((im.size_x, im.size_y, im.size_c), (2, 1, 3));
        assert_eq!(im.physical_size.x, Some(0.5));
        assert_eq!(im.extra["spectral_axis"]["quantity"], "wavelength");
        let p = ds.read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
        assert_eq!(p.data, f32s(&[1.0, 4.0]));
    }

    #[test]
    fn a_single_spectrum_is_also_a_trace() {
        let px = f32s(&[5.0, 7.0, 9.0, 4.0]);
        let (_d, mut ds) = open(&tagged_file(&[4], &["eV"], 2, Some("Spectrum"), &px));
        let info = ds.info().unwrap();
        assert_eq!((info.images[0].size_x, info.images[0].size_y), (4, 1));
        assert_eq!(info.images[0].physical_size.x, None);
        assert_eq!(info.traces.len(), 1);
        let axis = &info.traces[0].extra["axis"];
        assert_eq!(
            (axis["first"].as_f64(), axis["last"].as_f64()),
            (Some(-0.5), Some(1.0))
        );
        let t = ds.read_trace(0, 0, 0, 10).unwrap();
        assert_eq!(t.channels[0], vec![5.0, 7.0, 9.0, 4.0]);
    }

    #[test]
    fn packed_fourier_transforms_are_the_stored_half_plane() {
        // Dimensions 5 x 2 -> 3 stored complex columns x 2 rows
        let px = f32s(&(0..12).map(|v| v as f32).collect::<Vec<_>>());
        let (_d, mut ds) = open(&tagged_file(&[5, 2], &["", ""], 27, None, &px));
        let info = ds.info().unwrap();
        let im = &info.images[0];
        assert_eq!(
            (im.size_x, im.size_y, im.pixel_type),
            (3, 2, PixelType::ComplexFloat)
        );
        assert_eq!(im.extra["full_size_x"], 5);
        assert!(ds.check().unwrap().ok);
        assert_eq!(ds.read_plane(0, PlaneIndex::default()).unwrap().data, px);
    }

    #[test]
    fn undocumented_layouts_are_refused() {
        let (_d, mut ds) = open(&tagged_file(&[1, 1], &["", ""], 8, None, &[0, 1, 2, 3]));
        let e = ds.read_plane(0, PlaneIndex::default()).unwrap_err();
        assert_eq!(e.exit_code(), 6);
    }

    fn open(bytes: &[u8]) -> (tempfile::TempDir, DmDataset) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.dm4");
        std::fs::write(&p, bytes).unwrap();
        let ds = DmDataset::open(&p).unwrap();
        (dir, ds)
    }

    #[test]
    fn reads_a_2d_image_and_its_calibration() {
        for v in [3, 4] {
            let px: Vec<u8> = (0..6u16).flat_map(u16::to_le_bytes).collect();
            let (_d, mut ds) = open(&image_file(v, &[3, 2], 10, &px));
            let info = ds.info().unwrap();
            let im = &info.images[0];
            assert_eq!((im.size_x, im.size_y, im.size_z, im.size_t), (3, 2, 1, 1));
            assert_eq!(im.pixel_type, PixelType::Uint16);
            assert_eq!(im.physical_size.x, Some(0.5e-3));
            assert_eq!(im.extra["voltage_kv"], 300.0);
            let p = ds.read_plane(0, PlaneIndex::default()).unwrap();
            assert_eq!(p.data, px);
            let r = ds.check().unwrap();
            assert!(r.ok, "{:?}", r.findings);
        }
    }

    #[test]
    fn third_axis_with_length_units_is_z() {
        let px: Vec<u8> = (0..8u32).flat_map(|v| (v as f32).to_le_bytes()).collect();
        let (_d, mut ds) = open(&image_file(4, &[2, 2, 2], 2, &px));
        let info = ds.info().unwrap();
        assert_eq!(info.images[0].size_z, 2);
        let p = ds.read_plane(0, PlaneIndex { c: 0, z: 1, t: 0 }).unwrap();
        assert_eq!(p.data, px[16..].to_vec());
    }

    /// Writers set the root length to file length − header − 8 or − 4 (30 of the 89
    /// development files); only a larger one means the file is cut short.
    #[test]
    fn root_length_may_count_half_of_the_end_bytes() {
        let px: Vec<u8> = (0..6u16).flat_map(u16::to_le_bytes).collect();
        for v in [3u32, 4] {
            for (extra, ok) in [(0u64, true), (4, true), (5, false), (12, false)] {
                let mut b = image_file(v, &[3, 2], 10, &px);
                if v == 4 {
                    let n = u64::from_be_bytes(b[4..12].try_into().unwrap()) + extra;
                    b[4..12].copy_from_slice(&n.to_be_bytes());
                } else {
                    let n = u32::from_be_bytes(b[4..8].try_into().unwrap()) + extra as u32;
                    b[4..8].copy_from_slice(&n.to_be_bytes());
                }
                let (_d, mut ds) = open(&b);
                let r = ds.check().unwrap();
                assert_eq!(r.ok, ok, "DM{v}, root length + {extra}: {:?}", r.findings);
            }
        }
    }

    #[test]
    fn truncated_pixels_fail_check_and_read() {
        let px: Vec<u8> = (0..64u16).flat_map(u16::to_le_bytes).collect();
        let mut b = image_file(4, &[8, 8], 10, &px);
        // cut in the middle of the pixel array: the directory after it is lost too
        b.truncate(b.len() - 150);
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.dm4");
        std::fs::write(&p, &b).unwrap();
        let mut ds = DmDataset::open(&p).unwrap();
        let r = ds.check().unwrap();
        assert!(!r.ok);
    }
}
