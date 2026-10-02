//! The model every spectroscopy reader fills, and the one `Dataset` that serves it.
//!
//! A reader parses a file's headers into [`Parsed`]: *spectrum sets* (one trace each: spectra
//! that share an x axis and a y quantity, one sweep per spectrum), per-spectrum tables
//! (positions, times), maps (spectra on a grid, one image with one channel per spectral point),
//! attachments (a white-light image), experiment facts and the vendor tree. Spectrum values are
//! read lazily from the file through the byte source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes;
use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Method, Origin, Quantity, Sample,
};
use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, FormatDescriptor, ImageInfo,
    LsEntry, PhysicalSize, Severity, SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::pixel::{PixelType, Plane, plane_bytes_checked};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{ColumnInfo, Error, Result};
use serde_json::{Value, json};

/// Bytes read per request while gathering map planes or long spectra.
const CHUNK: u64 = 8 << 20;

/// How spectrum values are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stored {
    /// Little-endian IEEE float32.
    F32,
    /// Little-endian IEEE float64.
    F64,
    /// Little-endian int32.
    I32,
    /// Little-endian int16.
    I16,
    /// int32 stored as two little-endian 16-bit words, the most significant word first
    /// (Galactic's old 0x4D SPC format).
    I32WordSwapped,
    /// Little-endian uint16 (WITec camera counts).
    U16,
    /// Little-endian uint32.
    U32,
    /// Little-endian int64 (widened to f64: exact up to 2^53).
    I64,
    /// uint8.
    U8,
    /// int8.
    I8,
    /// One byte, 0 = false, anything else = true (1.0).
    Bool,
}

impl Stored {
    pub(crate) fn size(self) -> u64 {
        match self {
            Stored::F32 | Stored::I32 | Stored::I32WordSwapped | Stored::U32 => 4,
            Stored::F64 | Stored::I64 => 8,
            Stored::I16 | Stored::U16 => 2,
            Stored::U8 | Stored::I8 | Stored::Bool => 1,
        }
    }
    pub(crate) fn dtype(self) -> &'static str {
        match self {
            Stored::F32 => "float32",
            Stored::F64 => "float64",
            Stored::I32 | Stored::I32WordSwapped => "int32",
            Stored::I16 => "int16",
            Stored::U16 => "uint16",
            Stored::U32 => "uint32",
            Stored::I64 => "int64",
            Stored::U8 => "uint8",
            Stored::I8 => "int8",
            Stored::Bool => "bool",
        }
    }
    pub(crate) fn decode(self, b: &[u8]) -> f64 {
        match self {
            Stored::F32 => b.get(..4).map_or(f64::NAN, |s| {
                f64::from(f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
            }),
            Stored::I32 => b.get(..4).map_or(f64::NAN, |s| {
                f64::from(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
            }),
            Stored::U32 => b.get(..4).map_or(f64::NAN, |s| {
                f64::from(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
            }),
            Stored::F64 => b.get(..8).map_or(f64::NAN, |s| {
                f64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
            }),
            // exact up to 2^53
            Stored::I64 => b.get(..8).map_or(f64::NAN, |s| {
                i64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]) as f64
            }),
            Stored::I16 => b
                .get(..2)
                .map_or(f64::NAN, |s| f64::from(i16::from_le_bytes([s[0], s[1]]))),
            Stored::U16 => b
                .get(..2)
                .map_or(f64::NAN, |s| f64::from(u16::from_le_bytes([s[0], s[1]]))),
            Stored::U8 => b.first().map_or(f64::NAN, |v| f64::from(*v)),
            Stored::I8 => b
                .first()
                .map_or(f64::NAN, |v| f64::from(i8::from_le_bytes([*v]))),
            Stored::Bool => b.first().map_or(f64::NAN, |v| f64::from(u8::from(*v != 0))),
            Stored::I32WordSwapped => b.get(..4).map_or(f64::NAN, |s| {
                f64::from(i32::from_le_bytes([s[2], s[3], s[0], s[1]]))
            }),
        }
    }
}

/// Where the spectra of a set start in the file.
#[derive(Debug, Clone)]
pub(crate) enum Rows {
    /// Spectrum `k` starts at `first + k * stride`.
    Strided { first: u64, stride: u64 },
    /// One offset per spectrum.
    Listed(Vec<u64>),
    /// One offset per spectrum, its points `point_stride` bytes apart (band-sequential files:
    /// every pixel's point 0, then every pixel's point 1, …).
    Interleaved { starts: Vec<u64>, point_stride: u64 },
}

/// The x values of a set.
#[derive(Debug, Clone)]
pub(crate) enum XValues {
    /// Evenly spaced from `first` (point 0) to `last` (point n − 1).
    Regular { first: f64, last: f64 },
    /// One value per point (e.g. a grating spectrometer's Raman-shift calibration).
    Listed(Vec<f64>),
}

/// Spectra sharing one x axis and y quantity: one trace, one sweep per spectrum.
#[derive(Debug, Clone)]
pub(crate) struct SpectrumSet {
    /// Trace name (e.g. `absorbance`, `sample single channel`).
    pub(crate) name: String,
    /// Our x quantity: `wavenumber`, `raman_shift`, `wavelength`, `points`, `time`, `x`.
    pub(crate) x_quantity: &'static str,
    /// x unit (`1/cm`, `nm`, `µm`, `s`), when it has one.
    pub(crate) x_unit: Option<String>,
    pub(crate) x: XValues,
    /// The y channel name: our y quantity (`absorbance`, `transmittance`, `intensity`, ...).
    pub(crate) y_name: String,
    /// The y unit, when it has one (`%`, `counts`, `V`).
    pub(crate) y_unit: Option<String>,
    /// Points per spectrum.
    pub(crate) points: u64,
    /// Spectra in the set.
    pub(crate) count: u32,
    pub(crate) rows: Rows,
    pub(crate) stored: Stored,
    /// Value = stored × `scale`.
    pub(crate) scale: f64,
    /// JCAMP-DX `##DATA TYPE=` of the set (`INFRARED SPECTRUM`, `RAMAN SPECTRUM`, ...).
    pub(crate) data_type: &'static str,
    /// Trace `extra` in our vocabulary (besides `axis`, `data_type`, `y_quantity`).
    pub(crate) extra: BTreeMap<String, Value>,
}

impl SpectrumSet {
    /// Byte distance between consecutive points of a spectrum.
    fn point_stride(&self) -> u64 {
        match &self.rows {
            Rows::Interleaved { point_stride, .. } => *point_stride,
            _ => self.stored.size(),
        }
    }
    /// Bytes from the first point of a spectrum to the end of its last one.
    fn row_bytes(&self) -> u64 {
        self.points
            .saturating_sub(1)
            .saturating_mul(self.point_stride())
            .saturating_add(if self.points > 0 {
                self.stored.size()
            } else {
                0
            })
    }
    /// File offset of spectrum `k`.
    fn row_offset(&self, k: u32) -> Option<u64> {
        match &self.rows {
            Rows::Strided { first, stride } => first.checked_add(stride.checked_mul(u64::from(k))?),
            Rows::Listed(v) | Rows::Interleaved { starts: v, .. } => v.get(k as usize).copied(),
        }
    }
    fn irregular(&self) -> bool {
        matches!(self.x, XValues::Listed(_))
    }
    /// x value of point `i`.
    pub(crate) fn x_at(&self, i: u64) -> f64 {
        match &self.x {
            XValues::Regular { first, last } => {
                if self.points > 1 {
                    first + (last - first) * i as f64 / (self.points - 1) as f64
                } else {
                    *first
                }
            }
            XValues::Listed(v) => usize::try_from(i)
                .ok()
                .and_then(|i| v.get(i))
                .copied()
                .unwrap_or(f64::NAN),
        }
    }
}

/// A per-spectrum table (positions, times, titles as codes) belonging to a set.
#[derive(Debug, Clone)]
pub(crate) struct SpectrumTable {
    pub(crate) name: String,
    /// The set (trace index) the rows describe, row `k` = sweep `k`.
    pub(crate) trace: u32,
    /// `(name, unit, values)`.
    pub(crate) columns: Vec<(String, Option<String>, Vec<f64>)>,
}

/// Spectra on a regular grid, exposed as one image with a channel per spectral point.
#[derive(Debug, Clone)]
pub(crate) struct MapSpec {
    pub(crate) name: String,
    /// The set the spectra belong to.
    pub(crate) set: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Row-major `width × height`: the spectrum at each pixel, if any.
    pub(crate) pixels: Vec<Option<u32>>,
    /// Pixel size in µm.
    pub(crate) pixel_um: (Option<f64>, Option<f64>),
    pub(crate) extra: BTreeMap<String, Value>,
}

/// An embedded file (a white-light image).
#[derive(Debug, Clone)]
pub(crate) struct Attachment {
    pub(crate) name: String,
    pub(crate) content_type: String,
    pub(crate) extension: String,
    pub(crate) offset: u64,
    pub(crate) size: u64,
    pub(crate) extra: BTreeMap<String, Value>,
}

/// A JPEG attachment also exposed as an 8-bit image (a white-light picture of the sample).
#[derive(Debug, Clone)]
pub(crate) struct PhotoSpec {
    pub(crate) name: String,
    /// Index into `Parsed::attachments`.
    pub(crate) attachment: usize,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// 1 gray, 3 RGB.
    pub(crate) components: u32,
    /// Pixel size in µm, when the file records the field of view.
    pub(crate) pixel_um: (Option<f64>, Option<f64>),
    pub(crate) extra: BTreeMap<String, Value>,
}

/// How the pixels of a stored raster are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RasterLayout {
    /// One value per pixel, a row at a time (`x + width·y`).
    RowFirst,
    /// One value per pixel, a column at a time (`y + height·x`; WITec images without the
    /// inverted flag).
    ColumnFirst,
    /// Four bytes per pixel (red, green, blue, unused), a row at a time.
    RgbxRowFirst,
    /// A Windows BMP pixel array: rows of blue, green, red bytes (24-bit) padded to four bytes,
    /// the bottom row first.
    Bmp24BottomUp,
}

/// An image stored as a raster in the file (a WITec scalar image or video image).
#[derive(Debug, Clone)]
pub(crate) struct RasterSpec {
    pub(crate) name: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Value type of one sample (`U8` for colour rasters).
    pub(crate) stored: Stored,
    /// 1 (scalar) or 3 (RGB).
    pub(crate) components: u32,
    pub(crate) layout: RasterLayout,
    /// File offset of the first byte of the pixel array.
    pub(crate) offset: u64,
    /// Rows (`y`) to blank with NaN (float images only): lines the instrument did not complete.
    pub(crate) invalid_rows: Vec<u32>,
    /// Pixel size in µm, when the file records it.
    pub(crate) pixel_um: (Option<f64>, Option<f64>),
    pub(crate) extra: BTreeMap<String, Value>,
}

impl RasterSpec {
    /// The pixel type a plane of this raster is returned as.
    pub(crate) fn pixel_type(&self) -> PixelType {
        match self.stored {
            Stored::U8 | Stored::Bool => PixelType::Uint8,
            Stored::I8 => PixelType::Int8,
            Stored::U16 => PixelType::Uint16,
            Stored::I16 => PixelType::Int16,
            Stored::I32 | Stored::I32WordSwapped => PixelType::Int32,
            Stored::U32 => PixelType::Uint32,
            Stored::F32 => PixelType::Float,
            Stored::F64 | Stored::I64 => PixelType::Double,
        }
    }
    /// Bytes of the stored pixel array.
    pub(crate) fn stored_bytes(&self) -> Option<u64> {
        let (w, h) = (u64::from(self.width), u64::from(self.height));
        match self.layout {
            RasterLayout::RowFirst | RasterLayout::ColumnFirst => {
                w.checked_mul(h)?.checked_mul(self.stored.size())
            }
            RasterLayout::RgbxRowFirst => w.checked_mul(h)?.checked_mul(4),
            RasterLayout::Bmp24BottomUp => {
                w.checked_mul(3)?.div_ceil(4).checked_mul(4)?.checked_mul(h)
            }
        }
    }
}

/// Experiment facts, each with the vendor field it came from.
#[derive(Debug, Clone, Default)]
pub(crate) struct Facts {
    pub(crate) sample_name: Option<(String, String)>,
    pub(crate) sample_id: Option<(String, String)>,
    pub(crate) operator: Option<(String, String)>,
    pub(crate) vendor: Option<(String, String)>,
    pub(crate) model: Option<(String, String)>,
    pub(crate) serial: Option<(String, String)>,
    pub(crate) software: Option<(String, String)>,
    pub(crate) software_version: Option<(String, String)>,
    pub(crate) started_at: Option<(String, String)>,
    pub(crate) comment: Option<(String, String)>,
    pub(crate) method_name: Option<(String, String)>,
    /// Method parameters: our name → (quantity, vendor field, source).
    pub(crate) parameters: BTreeMap<String, (Quantity, String, Source)>,
}

impl Facts {
    /// Set a text fact when `value` is not blank.
    pub(crate) fn text(slot: &mut Option<(String, String)>, value: &str, from: &str) {
        let v = value.trim();
        if !v.is_empty() && slot.is_none() {
            *slot = Some((v.to_string(), from.to_string()));
        }
    }
    /// A numeric method parameter.
    pub(crate) fn number(&mut self, name: &str, v: f64, unit: Option<&str>, from: &str, s: Source) {
        if !v.is_finite() || self.parameters.contains_key(name) {
            return;
        }
        let q = match unit {
            Some(u) => Quantity::number(v, u),
            None => Quantity::plain(if v.fract() == 0.0 && v.abs() < 1e15 {
                json!(v as i64)
            } else {
                json!(v)
            }),
        };
        self.parameters.insert(name.into(), (q, from.into(), s));
    }
    /// A text method parameter.
    pub(crate) fn word(&mut self, name: &str, v: &str, from: &str, s: Source) {
        let v = v.trim();
        if v.is_empty() || self.parameters.contains_key(name) {
            return;
        }
        self.parameters
            .insert(name.into(), (Quantity::plain(v), from.into(), s));
    }
}

/// Everything a reader learned from the headers.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub(crate) format_version: Option<String>,
    pub(crate) sets: Vec<SpectrumSet>,
    pub(crate) tables: Vec<SpectrumTable>,
    pub(crate) maps: Vec<MapSpec>,
    /// Images after the maps: JPEG attachments decoded on read.
    pub(crate) photos: Vec<PhotoSpec>,
    /// Images after the photos: rasters stored uncompressed in the file.
    pub(crate) rasters: Vec<RasterSpec>,
    /// Spectra of a set that hold no measurement (a scan line the instrument did not complete):
    /// `false` at their sweep index. They read as NaN.
    pub(crate) row_valid: BTreeMap<usize, Vec<bool>>,
    pub(crate) attachments: Vec<Attachment>,
    pub(crate) facts: Facts,
    pub(crate) vendor: Value,
    pub(crate) entries: Vec<LsEntry>,
    /// Problems found while parsing (reported by `check`).
    pub(crate) findings: Vec<Finding>,
    pub(crate) notes: Vec<String>,
    pub(crate) provenance: ProvenanceMap,
    /// Per-spectrum scale factors of a set whose spectra are scaled differently (Galactic SPC
    /// multifiles with fixed-point values: one exponent per subfile), overriding `scale`.
    pub(crate) row_scales: BTreeMap<usize, Vec<f64>>,
}

/// An opened spectroscopy file.
#[derive(Debug)]
pub struct SpectroDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    file: SourceFile,
    file_len: u64,
    /// Where spectrum values are read from when they live in a container stream (JASCO's
    /// compound-file `Y-Data`) rather than at file offsets: the stream's bytes and length.
    values: Option<(SourceFile, u64)>,
    parsed: Parsed,
}

/// Round-trip-safe JSON number (non-finite → null).
pub(crate) fn num(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

impl SpectroDataset {
    /// The parser's notes, plus one line when parsing found the file damaged (`info` reads
    /// headers only; the problems themselves are `check` findings).
    fn info_notes(&self) -> Vec<String> {
        let mut notes = self.parsed.notes.clone();
        let errors: Vec<&Finding> = self
            .parsed
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect();
        if let Some(first) = errors.first() {
            let more = match errors.len() {
                1 => String::new(),
                n => format!(" and {} more", n - 1),
            };
            notes.push(format!(
                "the file is damaged ({}{more}); some spectra may be unreadable: run `check` for the list",
                first.message
            ));
        }
        notes
    }

    /// Wrap a parse result.
    pub(crate) fn new(
        descriptor: FormatDescriptor,
        input: &Input,
        file: SourceFile,
        file_len: u64,
        parsed: Parsed,
    ) -> Self {
        SpectroDataset {
            descriptor,
            path: input.path().to_path_buf(),
            file,
            file_len,
            values: None,
            parsed,
        }
    }

    /// Read spectrum values from `src` (`len` bytes) instead of the file: row offsets are then
    /// offsets into `src`.
    pub(crate) fn with_values_source(mut self, src: SourceFile, len: u64) -> Self {
        self.values = Some((src, len));
        self
    }

    fn format_id(&self) -> &'static str {
        // the descriptor holds an owned id; the format ids are static
        match self.descriptor.id.as_str() {
            crate::OPUS_FORMAT_ID => crate::OPUS_FORMAT_ID,
            crate::OMNIC_FORMAT_ID => crate::OMNIC_FORMAT_ID,
            crate::WDF_FORMAT_ID => crate::WDF_FORMAT_ID,
            crate::JWS_FORMAT_ID => crate::JWS_FORMAT_ID,
            crate::SPC_FORMAT_ID => crate::SPC_FORMAT_ID,
            crate::WITEC_FORMAT_ID => crate::WITEC_FORMAT_ID,
            crate::AGILENT_FPA_FORMAT_ID => crate::AGILENT_FPA_FORMAT_ID,
            crate::FSM_FORMAT_ID => crate::FSM_FORMAT_ID,
            crate::CARY_FORMAT_ID => crate::CARY_FORMAT_ID,
            _ => crate::PESP_FORMAT_ID,
        }
    }

    fn set(&self, index: u32) -> Result<&SpectrumSet> {
        self.parsed.sets.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                self.parsed.sets.len()
            ))
        })
    }

    /// What spectrum rows are read from, for messages.
    fn values_what(&self) -> &'static str {
        if self.values.is_some() {
            "data stream"
        } else {
            "file"
        }
    }

    /// Length of what spectrum rows are read from (the file, or the values stream).
    fn values_len(&self) -> u64 {
        self.values.as_ref().map_or(self.file_len, |(_, n)| *n)
    }

    fn read_bytes(&self, offset: u64, len: u64) -> Result<Vec<u8>> {
        let limit = self.values_len();
        let end = offset
            .checked_add(len)
            .filter(|e| *e <= limit)
            .ok_or_else(|| {
                Error::corrupt_at(
                    self.format_id(),
                    offset,
                    format!(
                        "{len} bytes at offset {offset} run past the end of the {} ({limit} bytes)",
                        self.values_what()
                    ),
                )
            })?;
        let n = usize::try_from(end - offset)
            .map_err(|_| Error::Other("read larger than memory".into()))?;
        let mut buf = vec![0u8; n];
        self.values
            .as_ref()
            .map_or(&self.file, |(f, _)| f)
            .read_exact_at(offset, &mut buf)
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(buf)
    }

    fn trace_info(&self, index: u32, s: &SpectrumSet) -> TraceInfo {
        let mut extra = s.extra.clone();
        let n = s.points;
        let axis = match &s.x {
            XValues::Regular { first, last } => {
                let step = if n > 1 {
                    (last - first) / (n - 1) as f64
                } else {
                    0.0
                };
                json!({"quantity": s.x_quantity, "unit": s.x_unit, "first": num(*first),
                       "last": num(*last), "step": num(step), "size": n})
            }
            XValues::Listed(v) => {
                json!({"quantity": s.x_quantity, "unit": s.x_unit, "irregular": true,
                       "channel": 0, "first": num(v.first().copied().unwrap_or(f64::NAN)),
                       "last": num(v.last().copied().unwrap_or(f64::NAN)), "size": n})
            }
        };
        extra.insert("axis".into(), axis);
        extra.insert("data_type".into(), json!(s.data_type));
        extra.insert("y_quantity".into(), json!(s.y_name));
        if let Some(t) = self.parsed.tables.iter().position(|t| t.trace == index) {
            extra.insert("spectra_table".into(), json!(t));
        }
        if let Some(m) = self
            .parsed
            .maps
            .iter()
            .position(|m| m.set == index as usize)
        {
            extra.insert("map_image".into(), json!(m));
        }
        let mut channels = Vec::new();
        if s.irregular() {
            channels.push(SignalChannelInfo {
                index: 0,
                name: s.x_quantity.to_string(),
                unit: s.x_unit.clone(),
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            });
        }
        channels.push(SignalChannelInfo {
            index: channels.len() as u32,
            name: s.y_name.clone(),
            unit: s.y_unit.clone(),
            dtype: s.stored.dtype().into(),
            scale: s.scale,
            offset: 0.0,
            extra: BTreeMap::new(),
        });
        TraceInfo {
            index,
            name: Some(s.name.clone()),
            sample_rate_hz: 0.0,
            sample_count: n,
            sweep_count: s.count,
            channels,
            start_s: None,
            extra,
        }
    }

    fn image_info(&self, index: u32, m: &MapSpec) -> ImageInfo {
        let s = &self.parsed.sets[m.set];
        let mut img = ImageInfo::new(index, m.width, m.height, PixelType::Float);
        img.name = Some(m.name.clone());
        img.size_c = u32::try_from(s.points).unwrap_or(u32::MAX);
        img.physical_size = PhysicalSize::micrometres(m.pixel_um.0, m.pixel_um.1, None);
        let unit = s.x_unit.as_deref().unwrap_or("");
        img.channels = (0..s.points.min(u64::from(u32::MAX)))
            .map(|i| ChannelInfo {
                index: i as u32,
                name: Some(format!("{} {unit}", short(s.x_at(i))).trim().to_string()),
                ..ChannelInfo::default()
            })
            .collect();
        let mut extra = m.extra.clone();
        extra.insert("trace".into(), json!(m.set));
        extra.insert(
            "band_axis".into(),
            json!({"quantity": s.x_quantity, "unit": s.x_unit, "first": num(s.x_at(0)),
                   "last": num(s.x_at(s.points.saturating_sub(1))), "size": s.points,
                   "irregular": s.irregular()}),
        );
        extra.insert("y_quantity".into(), json!(s.y_name));
        if let Some(u) = &s.y_unit {
            extra.insert("y_unit".into(), json!(u));
        }
        img.extra = extra;
        img.finish()
    }

    /// True when spectrum `k` of set `set` holds no measurement (it reads as NaN).
    fn row_is_blank(&self, set: usize, k: u32) -> bool {
        self.parsed
            .row_valid
            .get(&set)
            .and_then(|v| v.get(k as usize))
            .is_some_and(|ok| !ok)
    }

    /// Scale of spectrum `k` of set `set`.
    fn row_scale(&self, set: usize, s: &SpectrumSet, k: u32) -> f64 {
        self.parsed
            .row_scales
            .get(&set)
            .and_then(|v| v.get(k as usize))
            .copied()
            .unwrap_or(s.scale)
    }

    /// Values of spectrum `sweep` of set `s` (index `set`), points `[first, first + n)`, scaled.
    fn read_values(
        &self,
        set: usize,
        s: &SpectrumSet,
        sweep: u32,
        first: u64,
        n: u64,
    ) -> Result<Vec<f64>> {
        if self.row_is_blank(set, sweep) {
            return Ok(vec![f64::NAN; usize::try_from(n).unwrap_or(0)]);
        }
        let step = s.point_stride();
        let off = s
            .row_offset(sweep)
            .and_then(|o| o.checked_add(first.checked_mul(step)?))
            .ok_or_else(|| Error::corrupt(self.format_id(), "spectrum offset overflows"))?;
        let w = s.stored.size();
        let scale = self.row_scale(set, s, sweep);
        if step == w {
            let bytes = self.read_bytes(off, n.saturating_mul(w))?;
            return Ok(bytes
                .chunks_exact(w as usize)
                .map(|c| s.stored.decode(c) * scale)
                .collect());
        }
        // Points `step` bytes apart: read windows of at most CHUNK bytes, each holding as many
        // consecutive points as fit.
        let per = (CHUNK / step.max(1)).max(1);
        let mut out = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
        let mut i = 0u64;
        while i < n {
            let m = per.min(n - i);
            let at =
                off.checked_add(i.checked_mul(step).ok_or_else(|| {
                    Error::corrupt(self.format_id(), "spectrum offset overflows")
                })?)
                .ok_or_else(|| Error::corrupt(self.format_id(), "spectrum offset overflows"))?;
            let span = (m - 1).saturating_mul(step).saturating_add(w);
            let bytes = self.read_bytes(at, span)?;
            for k in 0..m {
                let b = usize::try_from(k * step).unwrap_or(usize::MAX);
                out.push(
                    bytes
                        .get(b..b + w as usize)
                        .map_or(f64::NAN, |c| s.stored.decode(c))
                        * scale,
                );
            }
            i += m;
        }
        Ok(out)
    }
}

impl SpectroDataset {
    /// The one plane of a stored raster, rows top to bottom, samples as `r.pixel_type()`.
    fn read_raster(&self, r: &RasterSpec) -> Result<Plane> {
        let len = r
            .stored_bytes()
            .ok_or_else(|| Error::corrupt(self.format_id(), "image size overflows"))?;
        let pt = r.pixel_type();
        let bpp = pt.bytes_per_sample() * r.components as usize;
        let n = plane_bytes_checked(self.format_id(), r.width, r.height, bpp)?;
        let src = self.read_bytes(r.offset, len)?;
        let (w, h) = (r.width as usize, r.height as usize);
        let mut data = vec![0u8; n];
        match r.layout {
            RasterLayout::RgbxRowFirst => {
                for (px, s) in data
                    .as_chunks_mut::<3>()
                    .0
                    .iter_mut()
                    .zip(src.as_chunks::<4>().0)
                {
                    px.copy_from_slice(&s[..3]);
                }
            }
            RasterLayout::Bmp24BottomUp => {
                let stride = (w * 3).div_ceil(4) * 4;
                for y in 0..h {
                    let from = (h - 1 - y) * stride;
                    let row = src.get(from..from + w * 3).ok_or_else(|| {
                        Error::corrupt(
                            self.format_id(),
                            format!("{}: bitmap rows truncated", r.name),
                        )
                    })?;
                    for (x, bgr) in row.as_chunks::<3>().0.iter().enumerate() {
                        let o = (y * w + x) * 3;
                        data[o] = bgr[2];
                        data[o + 1] = bgr[1];
                        data[o + 2] = bgr[0];
                    }
                }
            }
            RasterLayout::RowFirst | RasterLayout::ColumnFirst => {
                let sz = r.stored.size() as usize;
                let out = pt.bytes_per_sample();
                let blank: std::collections::BTreeSet<u32> =
                    r.invalid_rows.iter().copied().collect();
                for y in 0..h {
                    for x in 0..w {
                        let k = if r.layout == RasterLayout::RowFirst {
                            x + w * y
                        } else {
                            y + h * x
                        };
                        let b = src.get(k * sz..k * sz + sz).ok_or_else(|| {
                            Error::corrupt(
                                self.format_id(),
                                format!("{}: image data truncated", r.name),
                            )
                        })?;
                        let o = (y * w + x) * out;
                        let dst = &mut data[o..o + out];
                        // an f32 plane of f32 values: the narrowing casts are exact
                        match r.stored {
                            Stored::Bool => dst[0] = u8::from(b[0] != 0),
                            Stored::I64 => {
                                dst.copy_from_slice(&r.stored.decode(b).to_le_bytes());
                            }
                            Stored::F32 if blank.contains(&(y as u32)) => {
                                dst.copy_from_slice(&f32::NAN.to_le_bytes());
                            }
                            Stored::F64 if blank.contains(&(y as u32)) => {
                                dst.copy_from_slice(&f64::NAN.to_le_bytes());
                            }
                            Stored::I32WordSwapped => {
                                dst.copy_from_slice(&(r.stored.decode(b) as i32).to_le_bytes());
                            }
                            _ => dst.copy_from_slice(b),
                        }
                    }
                }
            }
        }
        Ok(Plane {
            width: r.width,
            height: r.height,
            pixel_type: pt,
            samples_per_pixel: r.components,
            data,
        })
    }
}

/// A short label for a band position (at most 6 significant digits).
fn short(v: f64) -> String {
    if !v.is_finite() {
        return "-".into();
    }
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl Dataset for SpectroDataset {
    fn info(&self) -> Result<FileInfo> {
        let traces = self
            .parsed
            .sets
            .iter()
            .enumerate()
            .map(|(i, s)| self.trace_info(i as u32, s))
            .collect();
        let tables = self
            .parsed
            .tables
            .iter()
            .enumerate()
            .map(|(i, t)| TableInfo {
                index: i as u32,
                name: Some(t.name.clone()),
                row_count: t.columns.first().map_or(0, |c| c.2.len() as u64),
                columns: t
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(k, (name, unit, _))| ColumnInfo {
                        index: k as u32,
                        name: name.clone(),
                        label: None,
                        dtype: "float64".into(),
                        unit: unit.clone(),
                        range: None,
                        extra: BTreeMap::new(),
                    })
                    .collect(),
                extra: BTreeMap::from([("trace".to_string(), json!(t.trace))]),
            })
            .collect();
        let mut images: Vec<ImageInfo> = self
            .parsed
            .maps
            .iter()
            .enumerate()
            .map(|(i, m)| self.image_info(i as u32, m))
            .collect();
        for p in &self.parsed.photos {
            let mut img = ImageInfo::new(images.len() as u32, p.width, p.height, PixelType::Uint8);
            img.name = Some(p.name.clone());
            img.samples_per_pixel = p.components;
            img.physical_size = PhysicalSize::micrometres(p.pixel_um.0, p.pixel_um.1, None);
            img.channels = vec![ChannelInfo {
                index: 0,
                name: Some(if p.components == 3 { "RGB" } else { "gray" }.into()),
                ..ChannelInfo::default()
            }];
            img.extra = p.extra.clone();
            images.push(img.finish());
        }
        for r in &self.parsed.rasters {
            let mut img = ImageInfo::new(images.len() as u32, r.width, r.height, r.pixel_type());
            img.name = Some(r.name.clone());
            img.samples_per_pixel = r.components;
            img.physical_size = PhysicalSize::micrometres(r.pixel_um.0, r.pixel_um.1, None);
            img.channels = vec![ChannelInfo {
                index: 0,
                name: Some(if r.components == 3 { "RGB" } else { "value" }.into()),
                ..ChannelInfo::default()
            }];
            img.extra = r.extra.clone();
            images.push(img.finish());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: self.descriptor.clone(),
            format_version: self.parsed.format_version.clone(),
            plane_count: images.iter().map(|i| i.plane_count).sum(),
            images,
            tables,
            spectra: Vec::new(),
            traces,
            notes: self.info_notes(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(self.parsed.vendor.clone())
    }

    fn provenance(&self) -> ProvenanceMap {
        self.parsed.provenance.clone()
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut e = self.parsed.entries.clone();
        for (i, a) in self.parsed.attachments.iter().enumerate() {
            e.push(LsEntry {
                kind: "attachment".into(),
                name: a.name.clone(),
                offset: Some(a.offset),
                size: Some(a.size),
                image: None,
                details: json!({"attachment": i, "content_type": a.content_type}),
            });
        }
        Ok(e)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let nmaps = self.parsed.maps.len();
        if let Some(r) = (image as usize)
            .checked_sub(nmaps + self.parsed.photos.len())
            .and_then(|k| self.parsed.rasters.get(k))
            .cloned()
        {
            if index != PlaneIndex::default() {
                return Err(Error::Usage(format!(
                    "image {image} ({}) has one plane (c=0 z=0 t=0)",
                    r.name
                )));
            }
            return self.read_raster(&r);
        }
        if let Some(p) = (image as usize)
            .checked_sub(nmaps)
            .and_then(|k| self.parsed.photos.get(k))
            .cloned()
        {
            if index != PlaneIndex::default() {
                return Err(Error::Usage(format!(
                    "image {image} ({}) has one plane (c=0 z=0 t=0)",
                    p.name
                )));
            }
            let a = self
                .parsed
                .attachments
                .get(p.attachment)
                .cloned()
                .ok_or_else(|| Error::Other("photo without its attachment".into()))?;
            let bytes = self.read_bytes(a.offset, a.size)?;
            let expect = (p.width as usize)
                .saturating_mul(p.height as usize)
                .saturating_mul(p.components as usize);
            let r =
                openreadout_codecs::jpeg_decode_limited(&bytes, expect.max(1)).map_err(|e| {
                    Error::corrupt(
                        self.format_id(),
                        format!("{}: JPEG does not decode: {e}", p.name),
                    )
                })?;
            if r.width != p.width
                || r.height != p.height
                || r.channels != p.components
                || r.bits_per_sample != 8
            {
                return Err(Error::corrupt(
                    self.format_id(),
                    format!(
                        "{}: the JPEG decodes to {}x{}x{} at {} bits, its header says {}x{}x{}",
                        p.name,
                        r.width,
                        r.height,
                        r.channels,
                        r.bits_per_sample,
                        p.width,
                        p.height,
                        p.components
                    ),
                ));
            }
            return Ok(Plane {
                width: r.width,
                height: r.height,
                pixel_type: PixelType::Uint8,
                samples_per_pixel: r.channels,
                data: r.data,
            });
        }
        let m = self.parsed.maps.get(image as usize).ok_or_else(|| {
            Error::Usage(
                if self.parsed.maps.is_empty()
                    && self.parsed.photos.is_empty()
                    && self.parsed.rasters.is_empty()
                {
                    "this file holds spectra (traces), not images; use `trace`".into()
                } else {
                    format!(
                        "image {image} out of range (file has {} images)",
                        nmaps + self.parsed.photos.len() + self.parsed.rasters.len()
                    )
                },
            )
        })?;
        let s = &self.parsed.sets[m.set];
        if index.z != 0 || index.t != 0 || u64::from(index.c) >= s.points {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range (map has {} channels = spectral points, 1 z, 1 t)",
                index.c, index.z, index.t, s.points
            )));
        }
        let n = plane_bytes_checked(self.format_id(), m.width, m.height, 4)?;
        let mut data = vec![0u8; n];
        let nan = f32::NAN.to_le_bytes();
        for px in data.as_chunks_mut::<4>().0 {
            *px = nan;
        }
        // Gather point `c` of every spectrum, reading contiguous runs of rows in chunks.
        let w = s.stored.size();
        let step = s.point_stride();
        let mut want: Vec<(u64, usize)> = Vec::with_capacity(m.pixels.len());
        for (p, k) in m.pixels.iter().enumerate() {
            if let Some(k) = k
                && !self.row_is_blank(m.set, *k)
                && let Some(off) = s.row_offset(*k)
            {
                want.push((off + u64::from(index.c) * step, p));
            }
        }
        want.sort_unstable();
        let mut i = 0;
        while i < want.len() {
            let start = want[i].0;
            let mut j = i;
            while j + 1 < want.len() && want[j + 1].0 + w - start <= CHUNK {
                j += 1;
            }
            let len = want[j].0 + w - start;
            let buf = self.read_bytes(start, len)?;
            for &(off, p) in &want[i..=j] {
                let r = (off - start) as usize;
                let scale = m.pixels[p].map_or(s.scale, |k| self.row_scale(m.set, s, k));
                let v = s.stored.decode(&buf[r..r + w as usize]) * scale;
                data[p * 4..p * 4 + 4].copy_from_slice(&(v as f32).to_le_bytes());
            }
            i = j + 1;
        }
        Ok(Plane {
            width: m.width,
            height: m.height,
            pixel_type: PixelType::Float,
            samples_per_pixel: 1,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        r.performed("every spectrum read and its values checked for non-finite numbers");
        for (t, s) in self.parsed.sets.iter().enumerate() {
            let mut bad = 0u64;
            let mut unreadable = None;
            for k in 0..s.count {
                if self.row_is_blank(t, k) {
                    continue;
                }
                match self.read_values(t, s, k, 0, s.points) {
                    Ok(v) => bad += v.iter().filter(|x| !x.is_finite()).count() as u64,
                    Err(e) => {
                        unreadable = Some((k, e));
                        break;
                    }
                }
            }
            if let Some((k, e)) = unreadable {
                r.push(Finding::error(
                    "unreadable_spectrum",
                    format!("trace {t} spectrum {k}: {e}"),
                ));
            } else if bad > 0 {
                r.push(Finding::warning(
                    "non_finite_values",
                    format!("trace {t} ({}) holds {bad} NaN or infinite values", s.name),
                ));
            }
        }
        Ok(r)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("block directory and headers parsed");
        r.performed("every spectrum's byte range lies inside the file");
        for f in &self.parsed.findings {
            r.push(f.clone());
        }
        for (t, s) in self.parsed.sets.iter().enumerate() {
            for k in 0..s.count {
                let end = s.row_offset(k).and_then(|o| o.checked_add(s.row_bytes()));
                if end.is_none_or(|e| e > self.values_len()) {
                    r.push(Finding::error(
                        "truncated",
                        format!(
                            "trace {t} ({}): spectrum {k} ends past the end of the {} ({} bytes)",
                            s.name,
                            self.values_what(),
                            self.values_len()
                        ),
                    ));
                    break;
                }
            }
        }
        for ras in &self.parsed.rasters {
            if ras
                .stored_bytes()
                .and_then(|n| ras.offset.checked_add(n))
                .is_none_or(|e| e > self.file_len)
            {
                r.push(Finding::error(
                    "truncated",
                    format!("image {} ends past the end of the file", ras.name),
                ));
            }
        }
        for a in &self.parsed.attachments {
            if a.offset.saturating_add(a.size) > self.file_len {
                r.push(Finding::error(
                    "truncated",
                    format!("attachment {} ends past the end of the file", a.name),
                ));
            }
        }
        Ok(r)
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let s = self.set(index)?;
        if sweep >= s.count {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (trace {index} has {} spectra)",
                s.count
            )));
        }
        let first = first_sample.min(s.points);
        let n = max_samples.min(s.points - first);
        let y = self.read_values(index as usize, s, sweep, first, n)?;
        let channels = if s.irregular() {
            let x = (first..first + n).map(|i| s.x_at(i)).collect();
            vec![x, y]
        } else {
            vec![y]
        };
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let t = self.parsed.tables.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {index} out of range (file has {} tables)",
                self.parsed.tables.len()
            ))
        })?;
        let rows = t.columns.first().map_or(0, |c| c.2.len() as u64);
        let a = first_row.min(rows) as usize;
        let b = first_row.saturating_add(max_rows).min(rows) as usize;
        Ok(Table {
            table: index,
            first_row: a as u64,
            columns: t.columns.iter().map(|c| c.2[a..b].to_vec()).collect(),
        })
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        Ok(self
            .parsed
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| AttachmentInfo {
                index: i as u32,
                name: a.name.clone(),
                content_type: a.content_type.clone(),
                extension: a.extension.clone(),
                offset: Some(a.offset),
                size: a.size,
                extra: a.extra.clone(),
            })
            .collect())
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let a = self.parsed.attachments.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "attachment {index} out of range (file has {})",
                self.parsed.attachments.len()
            ))
        })?;
        self.read_bytes(a.offset, a.size)
    }

    fn experiment(&self) -> Option<Experiment> {
        experiment_of(&self.parsed.facts, self.format_id())
    }
}

fn origin(s: Source, from: &str) -> Origin {
    Origin {
        source: s,
        from: from.to_string(),
    }
}

/// The experiment facts a reader collected, with their origins.
fn experiment_of(f: &Facts, format: &str) -> Option<Experiment> {
    let mut e = Experiment::default();
    let src = Source::Inferred;
    let mut s = Sample::default();
    if let Some((v, from)) = &f.sample_id {
        s.id = Some(v.clone());
        s.source_field = Some(from.clone());
        e.provenance.insert("sample.id".into(), origin(src, from));
    }
    if let Some((v, from)) = &f.sample_name {
        if s.id.is_none() {
            s.id = Some(v.clone());
            s.source_field = Some(from.clone());
            e.provenance.insert("sample.id".into(), origin(src, from));
        } else if s.id.as_deref() != Some(v.as_str()) {
            s.name = Some(v.clone());
            e.provenance.insert("sample.name".into(), origin(src, from));
        }
    }
    if s != Sample::default() {
        e.sample = Some(s);
    }
    let mut ins = ExperimentInstrument::default();
    for (slot, key, fact) in [
        (&mut ins.vendor, "instrument.vendor", &f.vendor),
        (&mut ins.model, "instrument.model", &f.model),
        (&mut ins.serial, "instrument.serial", &f.serial),
        (&mut ins.software, "instrument.software", &f.software),
        (
            &mut ins.software_version,
            "instrument.software_version",
            &f.software_version,
        ),
    ] {
        if let Some((v, from)) = fact {
            *slot = Some(v.clone());
            e.provenance.insert(key.into(), origin(src, from));
        }
    }
    if ins != ExperimentInstrument::default() {
        e.instrument = Some(ins);
    }
    let mut m = Method::default();
    if let Some((v, from)) = &f.method_name {
        m.name = Some(v.clone());
        e.provenance.insert("method.name".into(), origin(src, from));
    }
    for (k, (q, from, s)) in &f.parameters {
        m.parameters.insert(k.clone(), q.clone());
        e.provenance
            .insert(format!("method.parameters.{k}"), origin(*s, from));
    }
    if m != Method::default() {
        e.method = Some(m);
    }
    let mut a = Acquisition::default();
    if let Some((v, from)) = &f.started_at {
        a.started_at = Some(v.clone());
        e.provenance
            .insert("acquisition.started_at".into(), origin(src, from));
    }
    if let Some((v, from)) = &f.operator {
        a.operator = Some(v.clone());
        e.provenance
            .insert("acquisition.operator".into(), origin(src, from));
    }
    if let Some((v, from)) = &f.comment {
        a.comment = Some(v.clone());
        e.provenance
            .insert("acquisition.comment".into(), origin(src, from));
    }
    if a != Acquisition::default() {
        e.acquisition = Some(a);
    }
    let _ = format;
    (!e.is_empty()).then_some(e)
}

/// The file's length and a handle, or an I/O error naming the path.
pub(crate) fn open_source(input: &Input) -> Result<(SourceFile, u64)> {
    let f = input.open()?;
    let len = f.size().map_err(|e| Error::io(input.path(), e))?;
    Ok((f, len))
}

/// Read `len` bytes at `offset` (fewer at the end of the file).
pub(crate) fn read_at(
    f: &SourceFile,
    path: &Path,
    offset: u64,
    len: u64,
    file_len: u64,
) -> Result<Vec<u8>> {
    let end = offset.saturating_add(len).min(file_len);
    if offset >= end {
        return Ok(Vec::new());
    }
    let n = usize::try_from(end - offset)
        .map_err(|_| Error::Other("read larger than memory".into()))?;
    let mut buf = vec![0u8; n];
    f.read_exact_at(offset, &mut buf)
        .map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// Little-endian readers over a byte slice that return `None` past its end.
pub(crate) trait Le {
    fn u8_at(&self, at: usize) -> Option<u8>;
    fn u16_at(&self, at: usize) -> Option<u16>;
    fn i16_at(&self, at: usize) -> Option<i16>;
    fn u32_at(&self, at: usize) -> Option<u32>;
    fn i32_at(&self, at: usize) -> Option<i32>;
    fn u64_at(&self, at: usize) -> Option<u64>;
    fn i64_at(&self, at: usize) -> Option<i64>;
    fn f32_at(&self, at: usize) -> Option<f32>;
    fn f64_at(&self, at: usize) -> Option<f64>;
    fn bytes_at(&self, at: usize, n: usize) -> Option<&[u8]>;
}

impl Le for [u8] {
    fn u8_at(&self, at: usize) -> Option<u8> {
        self.get(at).copied()
    }
    fn u16_at(&self, at: usize) -> Option<u16> {
        bytes::le_u16(self, at)
    }
    fn i16_at(&self, at: usize) -> Option<i16> {
        bytes::le_i16(self, at)
    }
    fn u32_at(&self, at: usize) -> Option<u32> {
        bytes::le_u32(self, at)
    }
    fn i32_at(&self, at: usize) -> Option<i32> {
        bytes::le_i32(self, at)
    }
    fn u64_at(&self, at: usize) -> Option<u64> {
        bytes::le_u64(self, at)
    }
    fn i64_at(&self, at: usize) -> Option<i64> {
        bytes::le_i64(self, at)
    }
    fn f32_at(&self, at: usize) -> Option<f32> {
        bytes::le_f32(self, at)
    }
    fn f64_at(&self, at: usize) -> Option<f64> {
        bytes::le_f64(self, at)
    }
    fn bytes_at(&self, at: usize, n: usize) -> Option<&[u8]> {
        self.get(at..at.checked_add(n)?)
    }
}

/// Text from a fixed field: cut at the first NUL, UTF-8 when valid, else Latin-1; trimmed.
pub(crate) fn text_field(b: &[u8]) -> String {
    let b = bytes::until_nul(b);
    match std::str::from_utf8(b) {
        Ok(s) => s.trim().to_string(),
        Err(_) => bytes::latin1(b).trim().to_string(),
    }
}

/// Wavelength in nm of a laser given as a wavenumber in cm⁻¹ (10⁷ / ν).
pub(crate) fn nm_of_wavenumber(v: f64) -> Option<f64> {
    (v.is_finite() && v > 0.0).then(|| 1e7 / v)
}
