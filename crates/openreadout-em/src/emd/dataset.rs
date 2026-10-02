//! `Dataset` for Velox EMD files (HDF5 through `hdf5-pure`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hdf5_pure::{DType, Datatype, DatatypeByteOrder};
use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::time::unix_to_iso8601;
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Value, json};

use super::spectra::{
    EventTable, VeloxSpectrum, VeloxStream, decode_stream, event_table, stream_histogram,
};
use super::{EmdReader, FORMAT_ID};
use crate::util::{num, swap_samples};

/// Bytes read per window when gathering one frame of a multi-frame image.
const WINDOW_BYTES: u64 = 64 << 20;

fn h5err(e: impl std::fmt::Display) -> Error {
    Error::corrupt(FORMAT_ID, format!("HDF5: {e}"))
}

/// One `Data/Image/<id>` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct VeloxImage {
    /// The group name (a 32-hex-digit id).
    pub id: String,
    /// Rows, columns and frames of the `Data` dataset.
    pub rows: u64,
    pub columns: u64,
    pub frames: u64,
    pub pixel_type: Option<PixelType>,
    /// Stored big-endian (swapped on read).
    pub big_endian: bool,
    /// The first frame's metadata document.
    pub metadata: Option<Value>,
    /// The label of the Velox display that shows this image (`Presentation/Displays/
    /// ImageDisplay`: `HAADF`, or the element of a quantified EDS map such as `Au`).
    pub display_label: Option<String>,
    /// A complex Fourier transform stored as its non-redundant half plane: the compound's
    /// first member name (`realFloatHalfEven`, `realFloatHalfOdd`).
    pub fft_half_plane: Option<String>,
}

/// One `Data/EelsSpectrumImage/<id>` entry: a STEM-EELS spectrum image stored as a
/// (columns, channels, rows) array (`docs/formats/emd.md`).
#[derive(Debug, Clone, PartialEq)]
pub struct EelsSpectrumImage {
    /// The group name (a 32-hex-digit id).
    pub id: String,
    /// Scan columns (axis 0), energy channels (axis 1), scan rows (axis 2).
    pub columns: u64,
    pub bins: u64,
    pub rows: u64,
    pub pixel_type: Option<PixelType>,
    /// Stored big-endian (swapped on read).
    pub big_endian: bool,
    /// Energy of channel 0 and channel width, eV (`AcquisitionMetadata` `offset`,
    /// `dispersion`); `None` when the scan lines disagree.
    pub offset_ev: Option<f64>,
    pub dispersion_ev: Option<f64>,
    /// Per-pixel exposure (s) and the counts-to-intensity mapping Velox records (not applied).
    pub exposure_s: Option<f64>,
    pub intensity_scale: Option<f64>,
    pub intensity_offset: Option<f64>,
    /// The scan lines' `AcquisitionMetadata` documents disagree on the energy calibration.
    pub calibration_varies: bool,
    /// `BinaryResult.Detector` of the spectrum image's metadata document.
    pub detector: Option<String>,
    /// Pixel size (µm) shared by the images of the same raster and detector.
    pub pixel_um: (Option<f64>, Option<f64>),
    /// The `Metadata` document.
    pub metadata: Option<Value>,
}

/// Largest EELS spectrum image kept in memory after the first plane read (larger ones are
/// read slab by slab for every plane).
const EELS_CACHE_BYTES: u64 = 2 << 30;

/// A spectrum image assembled from the event streams that share one raster.
#[derive(Debug, Clone)]
struct SpectrumImage {
    /// Indices into `streams`.
    streams: Vec<usize>,
    width: u32,
    height: u32,
    bins: u32,
    frames: u64,
    /// Energy of channel 0 and channel width, eV (shared by the detector segments).
    offset_ev: Option<f64>,
    dispersion_ev: Option<f64>,
    /// Pixel size in µm (from the detector spectra's `BinaryResult`).
    pixel_um: (Option<f64>, Option<f64>),
    acquired_at: Option<String>,
    /// The detector segments' names.
    detectors: Vec<String>,
}

/// An opened EMD file.
pub struct EmdDataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    file: hdf5_pure::File,
    images: Vec<VeloxImage>,
    spectra: Vec<VeloxSpectrum>,
    streams: Vec<VeloxStream>,
    spectrum_images: Vec<SpectrumImage>,
    /// STEM-EELS spectrum images (`Data/EelsSpectrumImage`).
    eels: Vec<EelsSpectrumImage>,
    /// Raw samples of an EELS spectrum image, kept after its first plane read.
    eels_cache: Vec<Option<std::sync::Arc<Vec<u8>>>>,
    /// Decoded events per spectrum image, kept after the first plane read.
    events: Vec<Option<std::sync::Arc<EventTable>>>,
    version: Option<Value>,
    other_data: Vec<String>,
    has_thumbnail: bool,
    /// Absolute end-of-file address from the superblock, and the file's actual length.
    eof_address: u64,
    file_len: u64,
}

impl std::fmt::Debug for EmdDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmdDataset")
            .field("path", &self.path)
            .field("images", &self.images)
            .finish_non_exhaustive()
    }
}

fn pixel_type(d: &DType) -> Option<PixelType> {
    Some(match d {
        DType::U8 => PixelType::Uint8,
        DType::U16 => PixelType::Uint16,
        DType::U32 => PixelType::Uint32,
        DType::I8 => PixelType::Int8,
        DType::I16 => PixelType::Int16,
        DType::I32 => PixelType::Int32,
        DType::F32 => PixelType::Float,
        DType::F64 => PixelType::Double,
        // Velox Fourier transforms: a compound of real and imaginary float32 members
        DType::Compound(f) if f.len() == 2 && f.iter().all(|m| m.1 == DType::F32) => {
            PixelType::ComplexFloat
        }
        DType::Compound(f) if f.len() == 2 && f.iter().all(|m| m.1 == DType::F64) => {
            PixelType::ComplexDouble
        }
        _ => return None,
    })
}

/// The member name of a Velox half-plane Fourier transform (`realFloatHalfEven`, ...).
fn half_plane_member(d: &DType) -> Option<String> {
    match d {
        DType::Compound(f) if f.len() == 2 && f[0].0.contains("Half") => Some(f[0].0.clone()),
        _ => None,
    }
}

fn big_endian(d: &Datatype) -> bool {
    matches!(
        d,
        Datatype::FixedPoint {
            byte_order: DatatypeByteOrder::BigEndian,
            ..
        } | Datatype::FloatingPoint {
            byte_order: DatatypeByteOrder::BigEndian,
            ..
        }
    )
}

/// The first column of a (bytes, frames) uint8 metadata dataset, as JSON.
fn metadata_json(raw: &[u8], frames: u64) -> Option<Value> {
    let step = usize::try_from(frames.max(1)).ok()?;
    let doc: Vec<u8> = raw
        .iter()
        .step_by(step)
        .copied()
        .take_while(|&b| b != 0)
        .collect();
    serde_json::from_slice(&doc).ok()
}

fn text_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::String(s) => s.trim().parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
    .filter(|x: &f64| x.is_finite())
}

fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn metres_to_um(v: Option<f64>, unit: Option<&Value>) -> Option<f64> {
    let u = unit.and_then(Value::as_str).unwrap_or("m");
    let f = match u {
        "m" => 1e6,
        "nm" => 1e-3,
        "µm" | "um" => 1.0,
        _ => return None,
    };
    v.filter(|x| *x > 0.0).map(|x| x * f)
}

/// `%XX` escapes of Velox's display strings decoded.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Image id -> label of the Velox display showing it (`Presentation/Displays/ImageDisplay`).
fn display_labels(file: &hdf5_pure::File) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let base = "Presentation/Displays/ImageDisplay";
    for k in file
        .group(base)
        .and_then(|g| g.datasets())
        .unwrap_or_default()
    {
        let Some(doc) = file
            .dataset(&format!("{base}/{k}"))
            .ok()
            .and_then(|d| d.read_string().ok())
            .and_then(|v| v.into_iter().next())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        else {
            continue;
        };
        let path = doc.get("dataPath").and_then(Value::as_str).unwrap_or("");
        let label = doc
            .pointer("/display/label")
            .and_then(Value::as_str)
            .map(unescape)
            .filter(|l| !l.trim().is_empty());
        if let (Some(id), Some(l)) = (path.strip_prefix("/Data/Image/"), label) {
            out.entry(id.to_string()).or_insert(l);
        }
    }
    out
}

/// A JSON value at `path` inside `doc`.
fn at<'a>(doc: Option<&'a Value>, path: &[&str]) -> Option<&'a Value> {
    let mut cur = doc?;
    for p in path {
        cur = cur.get(*p)?;
    }
    Some(cur)
}

/// The `Detectors` entry named `name` in a metadata document.
fn detector_entry<'a>(doc: Option<&'a Value>, name: &str) -> Option<&'a Value> {
    at(doc, &["Detectors"])?
        .as_object()?
        .values()
        .find(|d| d.get("DetectorName").and_then(Value::as_str) == Some(name))
}

/// `Data/Spectrum/<id>` entries. A sum spectrum (whose detector has no `Detectors` entry)
/// takes the calibration its segments share.
fn read_spectra(file: &hdf5_pure::File) -> Vec<VeloxSpectrum> {
    let ids = file
        .group("Data/Spectrum")
        .and_then(|g| g.groups())
        .unwrap_or_default();
    let mut out = Vec::new();
    for id in ids {
        let Ok(ds) = file.dataset(&format!("Data/Spectrum/{id}/Data")) else {
            continue;
        };
        let bins = ds
            .shape()
            .ok()
            .and_then(|s| s.first().copied())
            .unwrap_or(0);
        let metadata = file
            .dataset(&format!("Data/Spectrum/{id}/Metadata"))
            .ok()
            .and_then(|m| {
                let f = m.shape().ok()?.get(1).copied().unwrap_or(1);
                metadata_json(&m.read_raw().ok()?, f)
            });
        let detector = text(at(metadata.as_ref(), &["BinaryResult", "Detector"]));
        let entry = detector
            .as_deref()
            .and_then(|d| detector_entry(metadata.as_ref(), d));
        out.push(VeloxSpectrum {
            id,
            detector,
            bins,
            offset_ev: text_f64(entry.and_then(|e| e.get("OffsetEnergy"))),
            dispersion_ev: text_f64(entry.and_then(|e| e.get("Dispersion"))).filter(|d| *d > 0.0),
            metadata,
        });
    }
    // sum spectra: the calibration every calibrated segment shares
    let cal: Vec<(u64, u64)> = out
        .iter()
        .filter_map(|s| Some((s.offset_ev?.to_bits(), s.dispersion_ev?.to_bits())))
        .collect();
    if let Some(first) = cal.first()
        && cal.iter().all(|c| c == first)
    {
        for s in &mut out {
            if s.dispersion_ev.is_none() {
                s.offset_ev = Some(f64::from_bits(first.0));
                s.dispersion_ev = Some(f64::from_bits(first.1));
            }
        }
    }
    out
}

/// `Data/SpectrumStream/<id>` headers.
fn read_streams(file: &hdf5_pure::File) -> Vec<VeloxStream> {
    let ids = file
        .group("Data/SpectrumStream")
        .and_then(|g| g.groups())
        .unwrap_or_default();
    let mut out = Vec::new();
    for id in ids {
        let settings = file
            .dataset(&format!("Data/SpectrumStream/{id}/AcquisitionSettings"))
            .ok()
            .and_then(|d| d.read_string().ok())
            .and_then(|v| v.into_iter().next())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let st = settings.as_ref();
        // Only the encoding observed in the corpus is decoded.
        let encoding = text(at(st, &["StreamEncoding"])).or_else(|| text(at(st, &["encoding"])));
        if encoding.as_deref() != Some("uint16") {
            continue;
        }
        let Some(bins) = text_f64(at(st, &["bincount"]))
            .and_then(|b| u32::try_from(b as u64).ok())
            .filter(|b| (1..=65535).contains(b))
        else {
            continue;
        };
        let raster = match (
            text_f64(at(st, &["RasterScanDefinition", "Width"])),
            text_f64(at(st, &["RasterScanDefinition", "Height"])),
        ) {
            (Some(w), Some(h)) if w >= 1.0 && h >= 1.0 && w * h <= f64::from(u32::MAX) => {
                Some((w as u32, h as u32))
            }
            _ => None,
        };
        let values = file
            .dataset(&format!("Data/SpectrumStream/{id}/Data"))
            .and_then(|d| d.shape())
            .ok()
            .and_then(|s| s.first().copied())
            .unwrap_or(0);
        let frames = file
            .dataset(&format!("Data/SpectrumStream/{id}/FrameLocationTable"))
            .and_then(|d| d.shape())
            .ok()
            .and_then(|s| s.first().copied())
            .unwrap_or(1)
            .max(1);
        out.push(VeloxStream {
            id,
            bins,
            raster,
            frames,
            values,
        });
    }
    out
}

/// Streams with a raster make spectrum images: one per (raster, channels, frames).
fn assemble_spectrum_images(
    streams: &[VeloxStream],
    spectra: &[VeloxSpectrum],
) -> Vec<SpectrumImage> {
    let mut out: Vec<SpectrumImage> = Vec::new();
    for (i, s) in streams.iter().enumerate() {
        let Some((w, h)) = s.raster else { continue };
        if let Some(si) = out
            .iter_mut()
            .find(|si| (si.width, si.height, si.bins, si.frames) == (w, h, s.bins, s.frames))
        {
            si.streams.push(i);
            continue;
        }
        out.push(SpectrumImage {
            streams: vec![i],
            width: w,
            height: h,
            bins: s.bins,
            frames: s.frames,
            offset_ev: None,
            dispersion_ev: None,
            pixel_um: (None, None),
            acquired_at: None,
            detectors: Vec::new(),
        });
    }
    // calibration and pixel size from the detector spectra (shared by the segments)
    let segs: Vec<&VeloxSpectrum> = spectra
        .iter()
        .filter(|s| s.dispersion_ev.is_some())
        .collect();
    for si in &mut out {
        if let Some(first) = segs.first()
            && segs.iter().all(|s| {
                s.offset_ev == first.offset_ev
                    && s.dispersion_ev == first.dispersion_ev
                    && s.bins == u64::from(si.bins)
            })
        {
            si.offset_ev = first.offset_ev;
            si.dispersion_ev = first.dispersion_ev;
        }
        let md = spectra.iter().find_map(|s| s.metadata.as_ref());
        si.pixel_um = (
            metres_to_um(
                text_f64(at(md, &["BinaryResult", "PixelSize", "width"])),
                at(md, &["BinaryResult", "PixelUnitX"]),
            ),
            metres_to_um(
                text_f64(at(md, &["BinaryResult", "PixelSize", "height"])),
                at(md, &["BinaryResult", "PixelUnitY"]),
            ),
        );
        si.acquired_at = text_f64(at(
            md,
            &["Acquisition", "AcquisitionStartDatetime", "DateTime"],
        ))
        .filter(|s| *s > 0.0)
        .map(|s| unix_to_iso8601(s as i64, 0));
        si.detectors = spectra.iter().filter_map(|s| s.detector.clone()).collect();
    }
    out
}

/// `Data/EelsSpectrumImage/<id>` entries. The pixel size comes from the images of the same
/// raster whose detector is the spectrum image's, when they all agree.
fn read_eels(file: &hdf5_pure::File, images: &[VeloxImage]) -> Vec<EelsSpectrumImage> {
    let ids = file
        .group("Data/EelsSpectrumImage")
        .and_then(|g| g.groups())
        .unwrap_or_default();
    let mut out = Vec::new();
    for id in ids {
        let Ok(ds) = file.dataset(&format!("Data/EelsSpectrumImage/{id}/Data")) else {
            continue;
        };
        let Some([columns, bins, rows]) =
            ds.shape().ok().and_then(|s| <[u64; 3]>::try_from(s).ok())
        else {
            continue;
        };
        let pixel_type = ds.dtype().ok().as_ref().and_then(pixel_type);
        let be = ds.datatype().is_ok_and(|d| big_endian(&d));
        let metadata = file
            .dataset(&format!("Data/EelsSpectrumImage/{id}/Metadata"))
            .ok()
            .and_then(|m| {
                let f = m.shape().ok()?.get(1).copied().unwrap_or(1);
                metadata_json(&m.read_raw().ok()?, f)
            });
        // One `AcquisitionMetadata` document per column (scan line).
        let acq: Vec<Value> = file
            .dataset(&format!("Data/EelsSpectrumImage/{id}/AcquisitionMetadata"))
            .ok()
            .and_then(|m| {
                let n = m.shape().ok()?.get(1).copied().unwrap_or(1).max(1);
                let raw = m.read_raw().ok()?;
                let n = usize::try_from(n).ok()?;
                Some(
                    (0..n)
                        .filter_map(|j| metadata_json(raw.get(j..)?, n as u64))
                        .collect(),
                )
            })
            .unwrap_or_default();
        let field = |k: &str| -> Vec<Option<f64>> {
            acq.iter()
                .map(|d| text_f64(at(Some(d), &["Data", k])))
                .collect()
        };
        let same = |v: &[Option<f64>]| -> Option<f64> {
            let first = (*v.first()?)?;
            v.iter().all(|x| *x == Some(first)).then_some(first)
        };
        let (offs, disp) = (field("offset"), field("dispersion"));
        let (offset_ev, dispersion_ev) = (same(&offs), same(&disp));
        let calibration_varies =
            !acq.is_empty() && (offset_ev.is_none() || dispersion_ev.is_none());
        let md = metadata.as_ref();
        let detector = text(at(md, &["BinaryResult", "Detector"]));
        let sizes: Vec<(Option<f64>, Option<f64>)> = images
            .iter()
            .filter(|v| v.columns == columns && v.rows == rows)
            .filter(|v| {
                detector.is_some()
                    && text(at(v.metadata.as_ref(), &["BinaryResult", "Detector"])) == detector
            })
            .map(|v| {
                let m = v.metadata.as_ref();
                (
                    metres_to_um(
                        text_f64(at(m, &["BinaryResult", "PixelSize", "width"])),
                        at(m, &["BinaryResult", "PixelUnitX"]),
                    ),
                    metres_to_um(
                        text_f64(at(m, &["BinaryResult", "PixelSize", "height"])),
                        at(m, &["BinaryResult", "PixelUnitY"]),
                    ),
                )
            })
            .collect();
        let pixel_um = match sizes.first() {
            Some(first) if sizes.iter().all(|s| s == first) => *first,
            _ => (None, None),
        };
        out.push(EelsSpectrumImage {
            id,
            columns,
            bins,
            rows,
            pixel_type,
            big_endian: be,
            offset_ev,
            dispersion_ev,
            exposure_s: same(&field("exposureTime")),
            intensity_scale: same(&field("intensityScale")),
            intensity_offset: same(&field("intensityOffset")),
            calibration_varies,
            detector,
            pixel_um,
            metadata,
        });
    }
    out
}

impl EmdDataset {
    /// Open an EMD file: HDF5 structure and image metadata only.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source). Local files use
    /// `hdf5-pure`'s own streaming reader.
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
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
        let sb = file.superblock();
        let eof_address = sb.base_address.get().saturating_add(sb.eof_address);
        let file_len = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
        let truncated = eof_address > file_len;
        let version = file
            .dataset("Version")
            .ok()
            .and_then(|d| d.read_string().ok())
            .and_then(|v| v.into_iter().next())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let ids = match file.group("Data/Image") {
            Ok(g) => g.groups().map_err(h5err)?,
            Err(_) if truncated => {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!(
                        "file is {file_len} bytes but its HDF5 superblock says it ends at {eof_address}; the image group cannot be read (truncated)"
                    ),
                ));
            }
            Err(_) => Vec::new(),
        };
        let labels = display_labels(&file);
        let mut images = Vec::new();
        for id in ids {
            let ds = file
                .dataset(&format!("Data/Image/{id}/Data"))
                .map_err(h5err)?;
            let shape = ds.shape().map_err(h5err)?;
            let (rows, columns, frames) = match shape.as_slice() {
                [r, c] => (*r, *c, 1),
                [r, c, f] => (*r, *c, *f),
                other => {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        format!("Velox image with shape {other:?}"),
                        "Velox images are (rows, columns[, frames]); this dataset has another rank.",
                    ));
                }
            };
            let dt = ds.dtype().ok();
            let pt = dt.as_ref().and_then(pixel_type);
            let fft_half_plane = dt.as_ref().and_then(half_plane_member);
            let be = ds.datatype().is_ok_and(|d| big_endian(&d));
            let metadata = file
                .dataset(&format!("Data/Image/{id}/Metadata"))
                .ok()
                .and_then(|m| {
                    let f = m.shape().ok()?.get(1).copied().unwrap_or(1);
                    metadata_json(&m.read_raw().ok()?, f)
                });
            images.push(VeloxImage {
                display_label: labels.get(&id).cloned(),
                id,
                rows,
                columns,
                frames,
                pixel_type: pt,
                big_endian: be,
                metadata,
                fft_half_plane,
            });
        }
        let spectra = read_spectra(&file);
        let streams = read_streams(&file);
        let spectrum_images = assemble_spectrum_images(&streams, &spectra);
        let eels = read_eels(&file, &images);
        let mut other_data = Vec::new();
        if let Ok(g) = file.group("Data") {
            for kind in g.groups().unwrap_or_default() {
                if !matches!(
                    kind.as_str(),
                    "Image" | "Spectrum" | "SpectrumStream" | "EelsSpectrumImage"
                ) {
                    let n = file
                        .group(&format!("Data/{kind}"))
                        .and_then(|k| k.groups())
                        .map_or(0, |v| v.len());
                    other_data.push(format!("{kind} ({n})"));
                }
            }
        }
        if images.is_empty() && spectra.is_empty() && spectrum_images.is_empty() && eels.is_empty()
        {
            let berkeley = super::berkeley::looks_like_berkeley(&file);
            return Err(Error::unsupported(
                FORMAT_ID,
                if berkeley {
                    "NCEM/Berkeley EMD (emd_group_type groups)".to_string()
                } else {
                    format!(
                        "EMD file without Velox images (Data holds: {})",
                        other_data.join(", ")
                    )
                },
                "Only Thermo Fisher Velox EMD images (Data/Image), spectra (Data/Spectrum), EDS spectrum images (Data/SpectrumStream) and EELS spectrum images (Data/EelsSpectrumImage) are read. Export the data from Velox or your analysis software as TIFF or MRC, or read it with an HDF5 tool.",
            ));
        }
        let has_thumbnail = file.dataset("Thumbnail.jpg").is_ok();
        let n_si = spectrum_images.len();
        let n_eels = eels.len();
        Ok(EmdDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            file,
            images,
            spectra,
            streams,
            spectrum_images,
            eels,
            eels_cache: vec![None; n_eels],
            events: vec![None; n_si],
            version,
            other_data,
            has_thumbnail,
            eof_address,
            file_len,
        })
    }

    /// The Velox images found in `Data/Image`.
    pub fn images(&self) -> &[VeloxImage] {
        &self.images
    }

    /// The detector spectra found in `Data/Spectrum` (one trace each).
    pub fn spectra(&self) -> &[VeloxSpectrum] {
        &self.spectra
    }

    /// The X-ray event streams found in `Data/SpectrumStream`.
    pub fn streams(&self) -> &[VeloxStream] {
        &self.streams
    }

    fn si_info(index: u32, si: &SpectrumImage) -> ImageInfo {
        let mut im = ImageInfo::new(index, si.width, si.height, PixelType::Uint32);
        im.name = Some("EDS spectrum image".into());
        im.size_c = si.bins;
        im.physical_size = PhysicalSize::micrometres(si.pixel_um.0, si.pixel_um.1, None);
        let energy = |k: u32| Some((si.offset_ev? + f64::from(k) * si.dispersion_ev?) / 1000.0);
        im.channels = (0..si.bins)
            .map(|k| ChannelInfo {
                index: k,
                name: Some(
                    energy(k).map_or_else(|| format!("channel {k}"), |e| format!("{e:.3} keV")),
                ),
                ..ChannelInfo::default()
            })
            .collect();
        im.acquired_at.clone_from(&si.acquired_at);
        im.instrument = Some(InstrumentInfo {
            software: Some("Velox".into()),
            detector: si.detectors.first().map(|_| si.detectors.join(" + ")),
            ..InstrumentInfo::default()
        });
        let ex = &mut im.extra;
        ex.insert("spectrum_image".into(), Value::Bool(true));
        if let (Some(o), Some(d)) = (si.offset_ev, si.dispersion_ev) {
            ex.insert(
                "energy_axis".into(),
                json!({"quantity": "energy", "unit": "keV", "first": num(o / 1000.0),
                       "step": num(d / 1000.0), "size": si.bins}),
            );
        }
        ex.insert("frames_summed".into(), json!(si.frames));
        ex.insert("detectors_summed".into(), json!(si.detectors));
        ex.insert("sample_meaning".into(), json!("X-ray counts per pixel in one energy channel, summed over detector segments and frames"));
        im.finish()
    }

    /// The STEM-EELS spectrum images (`Data/EelsSpectrumImage`).
    pub fn eels_spectrum_images(&self) -> &[EelsSpectrumImage] {
        &self.eels
    }

    fn eels_info(index: u32, e: &EelsSpectrumImage) -> ImageInfo {
        let dim = |v: u64| u32::try_from(v).unwrap_or(u32::MAX);
        let mut im = ImageInfo::new(
            index,
            dim(e.columns),
            dim(e.rows),
            e.pixel_type.unwrap_or(PixelType::Uint16),
        );
        im.name = Some("EELS spectrum image".into());
        im.size_c = dim(e.bins);
        im.physical_size = PhysicalSize::micrometres(e.pixel_um.0, e.pixel_um.1, None);
        let energy = |k: u32| Some(e.offset_ev? + f64::from(k) * e.dispersion_ev?);
        im.channels = (0..im.size_c.min(1 << 16))
            .map(|k| ChannelInfo {
                index: k,
                name: Some(energy(k).map_or_else(|| format!("channel {k}"), |v| format!("{v} eV"))),
                ..ChannelInfo::default()
            })
            .collect();
        let md = e.metadata.as_ref();
        if md.is_some() {
            im.instrument = Some(InstrumentInfo {
                manufacturer: text(at(md, &["Instrument", "Manufacturer"])),
                model: text(at(md, &["Instrument", "InstrumentModel"]))
                    .or_else(|| text(at(md, &["Instrument", "InstrumentClass"]))),
                software: Some("Velox".into()),
                software_version: text(at(md, &["Instrument", "ControlSoftwareVersion"])),
                detector: e.detector.clone(),
            });
        }
        im.acquired_at = text_f64(at(
            md,
            &["Acquisition", "AcquisitionStartDatetime", "DateTime"],
        ))
        .filter(|s| *s > 0.0)
        .map(|s| unix_to_iso8601(s as i64, 0));
        let ex = &mut im.extra;
        ex.insert("velox_id".into(), Value::from(e.id.clone()));
        ex.insert("spectrum_image".into(), Value::Bool(true));
        if let (Some(o), Some(d)) = (e.offset_ev, e.dispersion_ev) {
            ex.insert(
                "energy_axis".into(),
                json!({"quantity": "energy loss", "unit": "eV", "first": num(o), "step": num(d), "size": e.bins}),
            );
        }
        if e.calibration_varies {
            ex.insert("energy_calibration_varies".into(), Value::Bool(true));
        }
        if let Some(t) = e.exposure_s {
            ex.insert("exposure_s".into(), num(t));
        }
        if let (Some(a), Some(b)) = (e.intensity_scale, e.intensity_offset) {
            ex.insert("intensity_scale".into(), num(a));
            ex.insert("intensity_offset".into(), num(b));
        }
        if let Some(kv) = text_f64(at(md, &["Optics", "AccelerationVoltage"])) {
            ex.insert("voltage_kv".into(), num(kv / 1000.0));
        }
        ex.insert("sample_meaning".into(), json!("EELS counts per pixel in one energy-loss channel, as stored (intensity_scale and intensity_offset not applied)"));
        if e.pixel_type.is_none() {
            ex.insert("unsupported_sample_type".into(), Value::Bool(true));
        }
        im.finish()
    }

    /// Energy channel `c` of EELS spectrum image `k` as a (rows, columns) plane.
    #[allow(clippy::many_single_char_names)] // k, e, c, x, n: image, entry, channel, column, count
    fn eels_plane(&mut self, k: usize, index: PlaneIndex) -> Result<Plane> {
        let e = self.eels[k].clone();
        let Some(pixel_type) = e.pixel_type else {
            return Err(Error::unsupported(
                FORMAT_ID,
                "EELS spectrum image sample type",
                "Only 8/16/32-bit integer and 32/64-bit float samples are decoded.",
            ));
        };
        if u64::from(index.c) >= e.bins || index.z > 0 || index.t > 0 {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for the EELS spectrum image (c<{}, z<1, t<1)",
                index.c, index.z, index.t, e.bins
            )));
        }
        let bps = pixel_type.bytes_per_sample();
        let too_big = || Error::corrupt(FORMAT_ID, "EELS spectrum image size overflows");
        let total = e
            .columns
            .checked_mul(e.bins)
            .and_then(|n| n.checked_mul(e.rows))
            .and_then(|n| n.checked_mul(bps as u64))
            .ok_or_else(too_big)?;
        let (cols, bins, rows) = (
            usize::try_from(e.columns).map_err(|_| too_big())?,
            usize::try_from(e.bins).map_err(|_| too_big())?,
            usize::try_from(e.rows).map_err(|_| too_big())?,
        );
        let c = index.c as usize;
        let mut out = vec![
            0u8;
            cols.checked_mul(rows)
                .and_then(|n| n.checked_mul(bps))
                .ok_or_else(too_big)?
        ];
        // out[y][x] = data[x][c][y]
        let mut gather = |slab: &[u8], x0: usize, n: usize| -> Result<()> {
            for dx in 0..n {
                let base = (dx * bins + c) * rows * bps;
                let src = slab.get(base..base + rows * bps).ok_or_else(|| {
                    Error::corrupt(FORMAT_ID, "EELS spectrum image is shorter than its shape")
                })?;
                for y in 0..rows {
                    let o = (y * cols + x0 + dx) * bps;
                    out[o..o + bps].copy_from_slice(&src[y * bps..(y + 1) * bps]);
                }
            }
            Ok(())
        };
        let ds = self
            .file
            .dataset(&format!("Data/EelsSpectrumImage/{}/Data", e.id))
            .map_err(h5err)?;
        if total <= EELS_CACHE_BYTES {
            let raw = if let Some(r) = &self.eels_cache[k] {
                r.clone()
            } else {
                let raw = ds.read_raw().map_err(h5err)?;
                if raw.len() as u64 != total {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "EELS spectrum image {} holds {} bytes, expected {total}",
                            e.id,
                            raw.len()
                        ),
                    ));
                }
                let r = std::sync::Arc::new(raw);
                self.eels_cache[k] = Some(r.clone());
                r
            };
            gather(&raw, 0, cols)?;
        } else {
            let col_bytes = (bins * rows * bps) as u64;
            let per = (WINDOW_BYTES / col_bytes.max(1)).max(1);
            let mut x = 0u64;
            while x < e.columns {
                let n = per.min(e.columns - x);
                let raw = ds.read_raw_rows(x, n).map_err(h5err)?;
                gather(&raw, x as usize, n as usize)?;
                x += n;
            }
        }
        if e.big_endian {
            swap_samples(&mut out, bps);
        }
        Ok(Plane {
            width: e.columns as u32,
            height: e.rows as u32,
            pixel_type,
            samples_per_pixel: 1,
            data: out,
        })
    }

    /// Decode (once) the events of spectrum image `k`.
    fn si_events(&mut self, k: usize) -> Result<std::sync::Arc<EventTable>> {
        if let Some(t) = &self.events[k] {
            return Ok(t.clone());
        }
        let si = self.spectrum_images[k].clone();
        let pixels = u64::from(si.width) * u64::from(si.height);
        let mut events = Vec::new();
        let mut per_stream = Vec::new();
        for &i in &si.streams {
            let s = &self.streams[i];
            let ds = self
                .file
                .dataset(&format!("Data/SpectrumStream/{}/Data", s.id))
                .map_err(h5err)?;
            let mut raw = ds.read_raw().map_err(h5err)?;
            if ds.datatype().is_ok_and(|d| big_endian(&d)) {
                swap_samples(&mut raw, 2);
            }
            let n = decode_stream(&raw, si.bins, pixels, si.frames, &mut events)
                .map_err(|e| Error::corrupt(FORMAT_ID, format!("event stream {}: {e}", s.id)))?;
            per_stream.push(n);
        }
        let t = std::sync::Arc::new(event_table(&events, si.bins, per_stream));
        self.events[k] = Some(t.clone());
        Ok(t)
    }

    #[allow(clippy::many_single_char_names)] // w, h, k, n, t, a, b: raster, image, counts
    fn si_plane(&mut self, k: usize, index: PlaneIndex) -> Result<Plane> {
        let (w, h, bins) = {
            let si = &self.spectrum_images[k];
            (si.width, si.height, si.bins)
        };
        if index.c >= bins || index.z > 0 || index.t > 0 {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for the spectrum image (c<{bins}, z<1, t<1)",
                index.c, index.z, index.t
            )));
        }
        let t = self.si_events(k)?;
        let n = usize::try_from(u64::from(w) * u64::from(h))
            .map_err(|_| Error::corrupt(FORMAT_ID, "spectrum image raster overflows"))?;
        let mut counts = vec![0u32; n];
        let (a, b) = (
            t.offsets[index.c as usize] as usize,
            t.offsets[index.c as usize + 1] as usize,
        );
        for &p in &t.pixels[a..b] {
            if let Some(c) = counts.get_mut(p as usize) {
                *c = c.saturating_add(1);
            }
        }
        Ok(Plane {
            width: w,
            height: h,
            pixel_type: PixelType::Uint32,
            samples_per_pixel: 1,
            data: counts.iter().flat_map(|c| c.to_le_bytes()).collect(),
        })
    }

    fn trace_info(index: u32, s: &VeloxSpectrum) -> openreadout_core::model::TraceInfo {
        let mut extra = BTreeMap::new();
        extra.insert("velox_id".into(), json!(s.id));
        if let (Some(first), Some(last)) = (s.energy_kev(0), s.energy_kev(s.bins.saturating_sub(1)))
        {
            extra.insert(
                "axis".into(),
                json!({"quantity": "energy", "unit": "keV", "first": num(first), "last": num(last),
                       "step": num(s.dispersion_ev.unwrap_or(0.0) / 1000.0), "size": s.bins}),
            );
        }
        openreadout_core::model::TraceInfo {
            index,
            name: Some(
                s.detector
                    .clone()
                    .map_or_else(|| "EDS spectrum".into(), |d| format!("EDS spectrum {d}")),
            ),
            sample_rate_hz: 0.0,
            sample_count: s.bins,
            sweep_count: 1,
            channels: vec![openreadout_core::model::SignalChannelInfo {
                index: 0,
                name: "counts".into(),
                unit: Some("counts".into()),
                dtype: "uint32".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            }],
            start_s: None,
            extra,
        }
    }

    fn spectrum_counts(&self, s: &VeloxSpectrum) -> Result<Vec<u64>> {
        let ds = self
            .file
            .dataset(&format!("Data/Spectrum/{}/Data", s.id))
            .map_err(h5err)?;
        let mut raw = ds.read_raw().map_err(h5err)?;
        let bps = match ds.dtype().map_err(h5err)? {
            DType::U32 | DType::I32 => 4,
            DType::U16 | DType::I16 => 2,
            DType::U64 | DType::I64 => 8,
            other => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("Velox spectrum samples of type {other:?}"),
                    "Only integer Velox spectra are decoded.",
                ));
            }
        };
        if ds.datatype().is_ok_and(|d| big_endian(&d)) {
            swap_samples(&mut raw, bps);
        }
        Ok(raw
            .chunks_exact(bps)
            .map(|c| match bps {
                2 => u64::from(u16::from_le_bytes([c[0], c[1]])),
                4 => u64::from(u32::from_le_bytes([c[0], c[1], c[2], c[3]])),
                _ => u64::from_le_bytes(c.try_into().unwrap_or([0; 8])),
            })
            .collect())
    }

    fn image(&self, index: u32) -> Result<&VeloxImage> {
        self.images.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {index} out of range (0..{})",
                self.images.len() + self.spectrum_images.len() + self.eels.len()
            ))
        })
    }

    fn image_info(index: u32, v: &VeloxImage) -> ImageInfo {
        let mut im = ImageInfo::new(
            index,
            v.columns as u32,
            v.rows as u32,
            v.pixel_type.unwrap_or(PixelType::Uint8),
        );
        im.size_t = u32::try_from(v.frames.max(1)).unwrap_or(u32::MAX);
        let md = v.metadata.as_ref();
        let get = |path: &[&str]| {
            let mut cur = md?;
            for p in path {
                cur = cur.get(*p)?;
            }
            Some(cur)
        };
        let detector = text(get(&["BinaryResult", "Detector"]));
        im.name = v.display_label.clone().or_else(|| detector.clone());
        if let Some(m) = &v.fft_half_plane {
            im.extra
                .insert("packed_half_plane".into(), Value::Bool(true));
            im.extra
                .insert("half_plane_member".into(), Value::from(m.clone()));
        }
        im.physical_size = PhysicalSize::micrometres(
            metres_to_um(
                text_f64(get(&["BinaryResult", "PixelSize", "width"])),
                get(&["BinaryResult", "PixelUnitX"]),
            ),
            metres_to_um(
                text_f64(get(&["BinaryResult", "PixelSize", "height"])),
                get(&["BinaryResult", "PixelUnitY"]),
            ),
            None,
        );
        im.channels = vec![ChannelInfo {
            index: 0,
            name: v.display_label.clone().or_else(|| detector.clone()),
            ..ChannelInfo::default()
        }];
        if md.is_some() {
            im.instrument = Some(InstrumentInfo {
                manufacturer: text(get(&["Instrument", "Manufacturer"])),
                model: text(get(&["Instrument", "InstrumentModel"]))
                    .or_else(|| text(get(&["Instrument", "InstrumentClass"]))),
                software: Some("Velox".into()),
                software_version: text(get(&["Instrument", "ControlSoftwareVersion"])),
                detector,
            });
        }
        im.acquired_at = text_f64(get(&[
            "Acquisition",
            "AcquisitionStartDatetime",
            "DateTime",
        ]))
        .filter(|s| *s > 0.0)
        .map(|s| unix_to_iso8601(s as i64, 0));
        let ex = &mut im.extra;
        ex.insert("velox_id".into(), Value::from(v.id.clone()));
        if let Some(kv) = text_f64(get(&["Optics", "AccelerationVoltage"])) {
            ex.insert("voltage_kv".into(), num(kv / 1000.0));
        }
        if let Some(m) = text_f64(get(&["Optics", "NominalMagnification"])) {
            ex.insert("magnification".into(), num(m));
        }
        if let Some(c) = text_f64(get(&["Optics", "CameraLength"])) {
            ex.insert("camera_length_m".into(), num(c));
        }
        if v.pixel_type.is_none() {
            ex.insert("unsupported_sample_type".into(), Value::Bool(true));
        }
        im.finish()
    }

    fn frame(&self, v: &VeloxImage, t: u64, bps: usize) -> Result<Vec<u8>> {
        let ds = self
            .file
            .dataset(&format!("Data/Image/{}/Data", v.id))
            .map_err(h5err)?;
        let plane = usize::try_from(v.rows * v.columns)
            .ok()
            .and_then(|n| n.checked_mul(bps))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "plane size overflows"))?;
        if v.frames <= 1 {
            let raw = ds.read_raw().map_err(h5err)?;
            if raw.len() != plane {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!("image {} holds {} bytes, expected {plane}", v.id, raw.len()),
                ));
            }
            return Ok(raw);
        }
        // (rows, columns, frames): frames are innermost, so gather every `frames`-th sample.
        let row_bytes = v.columns * v.frames * bps as u64;
        let per = (WINDOW_BYTES / row_bytes.max(1)).max(1);
        let mut out = Vec::with_capacity(plane);
        let mut r = 0;
        while r < v.rows {
            let n = per.min(v.rows - r);
            let raw = ds.read_raw_rows(r, n).map_err(h5err)?;
            let stride = usize::try_from(v.frames).unwrap_or(usize::MAX) * bps;
            let first = usize::try_from(t).unwrap_or(0) * bps;
            for px in raw.chunks_exact(stride) {
                out.extend_from_slice(&px[first..first + bps]);
            }
            r += n;
        }
        if out.len() != plane {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "image {} frame {t} holds {} bytes, expected {plane}",
                    v.id,
                    out.len()
                ),
            ));
        }
        Ok(out)
    }
}

impl Dataset for EmdDataset {
    fn info(&self) -> Result<FileInfo> {
        let mut images: Vec<ImageInfo> = self
            .images
            .iter()
            .enumerate()
            .map(|(i, v)| Self::image_info(i as u32, v))
            .collect();
        let n0 = images.len();
        for (k, si) in self.spectrum_images.iter().enumerate() {
            images.push(Self::si_info((n0 + k) as u32, si));
        }
        let n1 = images.len();
        for (k, e) in self.eels.iter().enumerate() {
            images.push(Self::eels_info((n1 + k) as u32, e));
        }
        let traces = self
            .spectra
            .iter()
            .enumerate()
            .map(|(i, s)| Self::trace_info(i as u32, s))
            .collect();
        let mut notes = Vec::new();
        if !self.spectrum_images.is_empty() {
            notes.push("EDS spectrum images are assembled from Velox's X-ray event streams: one channel per energy channel (images[].extra.energy_axis), counts summed over detector segments and frames; the first plane read decodes every event".into());
        }
        if !self.eels.is_empty() {
            notes.push("EELS spectrum images (Data/EelsSpectrumImage): one channel per energy-loss channel (images[].extra.energy_axis), counts as stored".into());
        }
        if self.eels.iter().any(|e| e.calibration_varies) {
            notes.push("an EELS spectrum image's scan lines record different energy calibrations: no energy axis is reported for it".into());
        }
        if !self.spectra.is_empty() {
            notes.push(format!(
                "{} EDS detector spectra are traces (energy axis in traces[].extra.axis)",
                self.spectra.len()
            ));
        }
        if !self.other_data.is_empty() {
            notes.push(format!(
                "Velox data other than images is listed but not decoded: {}",
                self.other_data.join(", ")
            ));
        }
        if self.eof_address > self.file_len {
            notes.push("file is shorter than its HDF5 end-of-file address: it appears truncated (run `check`)".into());
        }
        if self.images.iter().any(|v| v.frames > 1) {
            notes.push("multi-frame Velox images: frames are exposed as T".into());
        }
        if self.images.iter().any(|v| v.fft_half_plane.is_some()) {
            notes.push("complex Fourier transforms (a real and an imaginary float32 per pixel) are returned as Velox stores them: the non-redundant half plane (images[].extra.packed_half_plane), not mirrored into the full plane".into());
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: EmdReader.descriptor(),
            format_version: self.version.as_ref().map(|v| {
                format!(
                    "{} {}",
                    v.get("format").and_then(Value::as_str).unwrap_or("EMD"),
                    v.get("version").and_then(Value::as_str).unwrap_or("")
                )
                .trim()
                .to_string()
            }),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces,
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut images = serde_json::Map::new();
        for v in &self.images {
            images.insert(v.id.clone(), v.metadata.clone().unwrap_or(Value::Null));
        }
        Ok(json!({"Version": self.version, "Data/Image": images, "other_data": self.other_data}))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::Inferred),
            ("images[].size_x", Source::Inferred),
            ("images[].size_y", Source::Inferred),
            ("images[].size_t", Source::Inferred),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::Inferred),
            ("images[].name", Source::Inferred),
            ("images[].instrument", Source::Inferred),
            ("images[].acquired_at", Source::Inferred),
            ("images[].extra.voltage_kv", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (i, v) in self.images.iter().enumerate() {
            out.push(LsEntry {
                kind: "image".into(),
                name: format!("Data/Image/{}/Data", v.id),
                offset: None,
                size: v
                    .pixel_type
                    .map(|p| v.rows * v.columns * v.frames * p.bytes_per_sample() as u64),
                image: Some(i as u32),
                details: json!({"rows": v.rows, "columns": v.columns, "frames": v.frames, "metadata": v.metadata.is_some()}),
            });
        }
        for (i, sp) in self.spectra.iter().enumerate() {
            out.push(LsEntry {
                kind: "spectrum".into(),
                name: format!("Data/Spectrum/{}/Data", sp.id),
                offset: None,
                size: None,
                image: None,
                details: json!({"trace": i, "detector": sp.detector, "channels": sp.bins,
                                "offset_ev": sp.offset_ev, "dispersion_ev": sp.dispersion_ev}),
            });
        }
        for st in &self.streams {
            let si = self
                .spectrum_images
                .iter()
                .position(|si| si.streams.iter().any(|&k| self.streams[k].id == st.id))
                .map(|k| (self.images.len() + k) as u32);
            out.push(LsEntry {
                kind: "event-stream".into(),
                name: format!("Data/SpectrumStream/{}/Data", st.id),
                offset: None,
                size: Some(st.values * 2),
                image: si,
                details: json!({"channels": st.bins, "raster": st.raster.map(|(w, h)| [w, h]), "frames": st.frames, "values": st.values}),
            });
        }
        let n1 = self.images.len() + self.spectrum_images.len();
        for (k, e) in self.eels.iter().enumerate() {
            out.push(LsEntry {
                kind: "eels-spectrum-image".into(),
                name: format!("Data/EelsSpectrumImage/{}/Data", e.id),
                offset: None,
                size: e
                    .pixel_type
                    .map(|p| e.columns * e.bins * e.rows * p.bytes_per_sample() as u64),
                image: Some((n1 + k) as u32),
                details: json!({"columns": e.columns, "channels": e.bins, "rows": e.rows,
                                "offset_ev": e.offset_ev, "dispersion_ev": e.dispersion_ev}),
            });
        }
        for d in &self.other_data {
            out.push(LsEntry {
                kind: "dataset".into(),
                name: format!("Data/{d}"),
                offset: None,
                size: None,
                image: None,
                details: json!({"decoded": false}),
            });
        }
        if self.has_thumbnail {
            out.push(LsEntry {
                kind: "attachment".into(),
                name: "Thumbnail.jpg".into(),
                offset: None,
                size: None,
                image: None,
                details: Value::Null,
            });
        }
        Ok(out)
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<openreadout_core::model::Trace> {
        let s = self.spectra.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (file has {} traces)",
                self.spectra.len()
            ))
        })?;
        if sweep > 0 {
            return Err(Error::Usage(format!("trace {index} has one sweep")));
        }
        let counts = self.spectrum_counts(&s)?;
        let a = usize::try_from(first_sample)
            .unwrap_or(usize::MAX)
            .min(counts.len());
        let b = a
            .saturating_add(usize::try_from(max_samples).unwrap_or(usize::MAX))
            .min(counts.len());
        Ok(openreadout_core::model::Trace {
            trace: index,
            sweep: 0,
            first_sample: a as u64,
            channels: vec![counts[a..b].iter().map(|&c| c as f64).collect()],
        })
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        if let Some(k) = (image as usize).checked_sub(self.images.len())
            && k < self.spectrum_images.len()
        {
            return self.si_plane(k, index);
        }
        if let Some(k) =
            (image as usize).checked_sub(self.images.len() + self.spectrum_images.len())
            && k < self.eels.len()
        {
            return self.eels_plane(k, index);
        }
        let v = self.image(image)?.clone();
        let Some(pixel_type) = v.pixel_type else {
            return Err(Error::unsupported(
                FORMAT_ID,
                "Velox image sample type",
                "Only 8/16/32-bit integer and 32/64-bit float Velox images are decoded.",
            ));
        };
        if index.c > 0 || index.z > 0 || u64::from(index.t) >= v.frames.max(1) {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image} (c<1, z<1, t<{})",
                index.c,
                index.z,
                index.t,
                v.frames.max(1)
            )));
        }
        let bps = pixel_type.bytes_per_sample();
        let mut data = self.frame(&v, u64::from(index.t), bps)?;
        if v.big_endian {
            // complex values are two numbers, each swapped on its own
            swap_samples(
                &mut data,
                if pixel_type.is_complex() {
                    bps / 2
                } else {
                    bps
                },
            );
        }
        Ok(Plane {
            width: v.columns as u32,
            height: v.rows as u32,
            pixel_type,
            samples_per_pixel: 1,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("HDF5 superblock and object headers parse (hdf5-pure)");
        r.performed("every Data/Image entry has a 2-D or 3-D Data dataset of a supported sample type and a JSON Metadata document");
        r.performed("the last row of every image is readable (catches truncated storage)");
        r.performed("the file is at least as long as the superblock's end-of-file address");
        if self.eof_address > self.file_len {
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "file is {} bytes but the HDF5 superblock says it ends at {}",
                        self.file_len, self.eof_address
                    ),
                )
                .at(self.file_len),
            );
        } else if self.eof_address < self.file_len {
            r.push(Finding::info(
                "trailing_bytes",
                format!(
                    "{} bytes follow the HDF5 end-of-file address",
                    self.file_len - self.eof_address
                ),
            ));
        }
        if !self.streams.is_empty() {
            r.performed("every X-ray event stream decodes: channels below its channel count, one pixel end per raster pixel and frame; its event histogram equals a stored detector spectrum");
        }
        let spectra: Vec<Vec<u64>> = self
            .spectra
            .clone()
            .iter()
            .map(|sp| self.spectrum_counts(sp).unwrap_or_default())
            .collect();
        for st in self.streams.clone() {
            let raw = self
                .file
                .dataset(&format!("Data/SpectrumStream/{}/Data", st.id))
                .and_then(|d| d.read_raw());
            let raw = match raw {
                Ok(r) => r,
                Err(e) => {
                    r.push(Finding::error(
                        "truncated",
                        format!("event stream {}: cannot be read: {e}", st.id),
                    ));
                    continue;
                }
            };
            if let Some((w, h)) = st.raster {
                let pixels = u64::from(w) * u64::from(h);
                if let Err(e) = decode_stream(&raw, st.bins, pixels, st.frames, &mut Vec::new()) {
                    r.push(Finding::error(
                        "event_stream",
                        format!("event stream {}: {e}", st.id),
                    ));
                    continue;
                }
            }
            let hist = stream_histogram(&raw, st.bins);
            if !spectra.contains(&hist) {
                r.push(Finding::warning(
                    "stream_spectrum_mismatch",
                    format!(
                        "event stream {}: its {} events match none of the stored detector spectra",
                        st.id,
                        hist.iter().sum::<u64>()
                    ),
                ));
            }
        }
        if !self.eels.is_empty() {
            r.performed("every EELS spectrum image's last scan column is readable and its scan lines share one energy calibration");
        }
        for e in self.eels.clone() {
            if e.calibration_varies {
                r.push(Finding::warning(
                    "energy_calibration",
                    format!(
                        "EELS spectrum image {}: scan lines record different energy calibrations",
                        e.id
                    ),
                ));
            }
            let last = self
                .file
                .dataset(&format!("Data/EelsSpectrumImage/{}/Data", e.id))
                .and_then(|d| d.read_raw_rows(e.columns.saturating_sub(1), 1));
            match last {
                Ok(b) if !b.is_empty() => {}
                Ok(_) => r.push(Finding::error(
                    "truncated",
                    format!("EELS spectrum image {}: last column is empty", e.id),
                )),
                Err(err) => r.push(Finding::error(
                    "truncated",
                    format!(
                        "EELS spectrum image {}: last column cannot be read: {err}",
                        e.id
                    ),
                )),
            }
        }
        for v in self.images.clone() {
            if v.pixel_type.is_none() {
                r.push(Finding::info(
                    "unsupported_sample_type",
                    format!("image {}: sample type is not decoded", v.id),
                ));
            }
            if v.metadata.is_none() {
                r.push(Finding::warning(
                    "metadata",
                    format!("image {}: Metadata is missing or not JSON", v.id),
                ));
            }
            let last = self
                .file
                .dataset(&format!("Data/Image/{}/Data", v.id))
                .and_then(|d| d.read_raw_rows(v.rows.saturating_sub(1), 1));
            match last {
                Ok(b) if !b.is_empty() => {}
                Ok(_) => r.push(Finding::error(
                    "truncated",
                    format!("image {}: last row is empty", v.id),
                )),
                Err(e) => r.push(Finding::error(
                    "truncated",
                    format!("image {}: last row cannot be read: {e}", v.id),
                )),
            }
        }
        Ok(r)
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        if !self.has_thumbnail {
            return Ok(Vec::new());
        }
        let size = self
            .file
            .dataset("Thumbnail.jpg")
            .and_then(|d| d.shape())
            .ok()
            .and_then(|s| s.first().copied())
            .unwrap_or(0);
        Ok(vec![AttachmentInfo {
            index: 0,
            name: "Thumbnail.jpg".into(),
            content_type: "JPG".into(),
            extension: "jpg".into(),
            offset: None,
            size,
            extra: BTreeMap::new(),
        }])
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        if index != 0 || !self.has_thumbnail {
            return Err(Error::Usage(format!(
                "attachment #{index} does not exist; `info --view structure` lists them"
            )));
        }
        self.file
            .dataset("Thumbnail.jpg")
            .and_then(|d| d.read_raw())
            .map_err(h5err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_column_is_extracted() {
        // two frames interleaved: "{}" for frame 0, "[]" for frame 1
        let raw = [b'{', b'[', b'}', b']', 0, 0];
        assert_eq!(metadata_json(&raw, 2), Some(json!({})));
        assert_eq!(metres_to_um(Some(2e-10), Some(&json!("m"))), Some(2e-4));
        assert_eq!(metres_to_um(Some(2e-10), Some(&json!("1/m"))), None);
    }
}
