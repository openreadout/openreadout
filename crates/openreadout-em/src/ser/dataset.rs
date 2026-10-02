//! `Dataset` for TIA series files: one image per `.ser` (elements as T, or as rows for 1-D
//! elements), metadata from the `.emi` sidecar when present.

use std::path::{Path, PathBuf};

use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo, LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::time::unix_to_iso8601;
use openreadout_core::xmljson::xml_to_json;
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use super::emi::{EmiInfo, series_of_in, sidecar_of_in};
use super::file::{
    ELEMENTS_1D, ELEMENTS_2D, ElementHeader, SerHeader, TAG_TIME, TAG_TIME_POSITION, read_element,
    read_header, read_tag,
};
use super::{FORMAT_ID, SerReader};
use crate::util::{Blob, ext_lower, num};

/// Element calibration deltas below this are taken to be metres (inferred; see provenance).
const MAX_METRE_DELTA: f64 = 1e-3;

/// How a series is returned (`docs/formats/ser.md`, "What we expose").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerLayout {
    /// 2-D elements: each element is a plane (T), in element order.
    Frames,
    /// 1-D elements that fill the scan dimensions: X = dimension 0, Y = dimension 1, one
    /// channel per spectrum bin.
    Scan,
    /// 1-D elements otherwise: one row per element.
    Rows,
}

/// One `.ser` file.
#[derive(Debug)]
pub struct SerSeries {
    pub path: PathBuf,
    blob: Blob,
    pub header: SerHeader,
    /// Header of the first valid element (geometry of the exposed image).
    pub first: Option<ElementHeader>,
}

impl SerSeries {
    fn open(fs: &Fs, path: &Path) -> Result<SerSeries> {
        let mut blob = Blob::open(fs, path)?;
        let header = read_header(&mut blob)?;
        let first = if header.valid_elements > 0 {
            let off = header.data_offsets.first().copied().unwrap_or(0);
            Some(read_element(&mut blob, header.data_type_id, off)?)
        } else {
            None
        };
        Ok(SerSeries {
            path: path.to_path_buf(),
            blob,
            header,
            first,
        })
    }

    fn valid(&self) -> u32 {
        self.header.valid_elements.min(self.header.total_elements)
    }

    /// How this series is returned.
    pub fn layout(&self) -> SerLayout {
        if self.header.data_type_id != ELEMENTS_1D {
            return SerLayout::Frames;
        }
        let dims = &self.header.dimensions;
        let product: u64 = dims.iter().map(|d| u64::from(d.size)).product();
        if (1..=2).contains(&dims.len())
            && self.valid() > 1
            && self.valid() == self.header.total_elements
            && product == u64::from(self.header.total_elements)
        {
            SerLayout::Scan
        } else {
            SerLayout::Rows
        }
    }

    /// Scan size (dimension 0, dimension 1) of a [`SerLayout::Scan`] series.
    fn scan_size(&self) -> (u32, u32) {
        let d = &self.header.dimensions;
        (
            d.first().map_or(1, |d| d.size.max(1)),
            d.get(1).map_or(1, |d| d.size.max(1)),
        )
    }
}

/// Energy (eV) of bin `i` of a 1-D element: offset + (i - element) x delta (inferred unit).
fn bin_energy(c: &super::file::AxisCalibration, i: u64) -> f64 {
    c.offset + (i as f64 - f64::from(c.element)) * c.delta
}

/// A calibrated value for a channel name: at most 6 decimals, no trailing zeros.
fn short_number(v: f64) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// Micrometres of a scan dimension's step when its unit is metres and the step is a
/// calibration: TIA writes a step of exactly 1 m on positions that were never calibrated
/// (point spectra at arbitrary positions), so steps of 1 mm or more are not pixel sizes.
fn scan_um(d: Option<&super::file::SerDimension>) -> Option<f64> {
    let d = d?;
    (d.units.trim().eq_ignore_ascii_case("meters") || d.units.trim() == "m")
        .then(|| d.calibration_delta.abs())
        .and_then(metres_to_um)
}

/// An opened `.ser` (one series) or `.emi` (all `<stem>_<n>.ser` series of the acquisition).
#[derive(Debug)]
pub struct SerDataset {
    path: PathBuf,
    /// Where the series files and the `.emi` are read from.
    fs: Fs,
    series: Vec<SerSeries>,
    emi: Option<EmiInfo>,
    opened_emi: bool,
}

fn metres_to_um(delta: f64) -> Option<f64> {
    (delta > 0.0 && delta < MAX_METRE_DELTA).then_some(delta * 1e6)
}

impl SerDataset {
    /// Open a `.ser` file, or an `.emi` file (whose series files must sit next to it).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let opened_emi = ext_lower(path) == "emi";
        let (series_paths, emi_path) = if opened_emi {
            let s = series_of_in(fs, path);
            if s.is_empty() {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "an .emi file without its .ser series files",
                    "The pixels of a TIA acquisition live in <name>_1.ser, <name>_2.ser, ... next to the .emi; copy them into the same directory, or open a .ser directly.",
                ));
            }
            (s, Some(path.to_path_buf()))
        } else {
            (vec![path.to_path_buf()], sidecar_of_in(fs, path))
        };
        let series = series_paths
            .iter()
            .map(|p| SerSeries::open(fs, p))
            .collect::<Result<Vec<_>>>()?;
        let emi = emi_path.as_deref().and_then(|p| EmiInfo::read_in(fs, p));
        Ok(SerDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            series,
            emi,
            opened_emi,
        })
    }

    fn series(&self, image: u32) -> Result<&SerSeries> {
        self.series.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {image} out of range (0..{})",
                self.series.len()
            ))
        })
    }

    /// (size_x, size_y, size_c, size_t) of a series' image.
    fn geometry(s: &SerSeries) -> (u32, u32, u32, u32) {
        let Some(f) = &s.first else {
            return (0, 0, 1, 0);
        };
        match s.layout() {
            SerLayout::Frames => (f.size_x, f.size_y, 1, s.valid().max(1)),
            SerLayout::Rows => (f.size_x, s.valid().max(1), 1, 1),
            SerLayout::Scan => {
                let (w, h) = s.scan_size();
                (w, h, f.size_x.max(1), 1)
            }
        }
    }

    fn image_info(&self, index: u32, s: &SerSeries) -> Result<ImageInfo> {
        let f = s.first.as_ref().ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("{} has no valid elements", s.path.display()),
            )
        })?;
        let pt = f
            .data_type
            .pixel_type()
            .unwrap_or(openreadout_core::PixelType::Uint8);
        let (sx, sy, sc, st) = Self::geometry(s);
        let layout = s.layout();
        let mut im = ImageInfo::new(index, sx, sy, pt);
        im.size_c = sc;
        im.size_t = st;
        im.name = s.path.file_stem().map(|n| n.to_string_lossy().to_string());
        let px = metres_to_um(f.calibration_x.delta);
        let py = f.calibration_y.and_then(|c| metres_to_um(c.delta));
        im.physical_size = match layout {
            SerLayout::Frames => PhysicalSize::micrometres(px, py, None),
            SerLayout::Scan => PhysicalSize::micrometres(
                scan_um(s.header.dimensions.first()),
                if sy > 1 {
                    scan_um(s.header.dimensions.get(1))
                } else {
                    None
                },
                None,
            ),
            SerLayout::Rows => PhysicalSize::micrometres(None, None, None),
        };
        im.channels = if layout == SerLayout::Scan {
            (0..sc.min(1 << 16))
                .map(|k| ChannelInfo {
                    index: k,
                    name: Some(format!(
                        "{} eV",
                        short_number(bin_energy(&f.calibration_x, u64::from(k)))
                    )),
                    ..ChannelInfo::default()
                })
                .collect()
        } else {
            vec![ChannelInfo {
                index: 0,
                ..ChannelInfo::default()
            }]
        };
        let ex = &mut im.extra;
        ex.insert(
            "layout".into(),
            Value::from(match layout {
                SerLayout::Frames => "frames",
                SerLayout::Scan => "scan",
                SerLayout::Rows => "rows",
            }),
        );
        if s.header.data_type_id == ELEMENTS_1D {
            let c = &f.calibration_x;
            ex.insert(
                "spectral_axis".into(),
                json!({"quantity": "energy", "unit": "eV", "first": num(bin_energy(c, 0)),
                       "step": num(c.delta), "size": f.size_x}),
            );
        } else if s.header.dimensions.len() > 1 {
            ex.insert(
                "frame_grid".into(),
                json!(
                    s.header
                        .dimensions
                        .iter()
                        .map(|d| d.size)
                        .collect::<Vec<_>>()
                ),
            );
        }
        ex.insert(
            "series_version".into(),
            Value::from(format!("0x{:04x}", s.header.version)),
        );
        ex.insert(
            "element_kind".into(),
            Value::from(if s.header.data_type_id == ELEMENTS_1D {
                "1d"
            } else {
                "2d"
            }),
        );
        ex.insert("element_data_type".into(), Value::from(f.data_type.label()));
        ex.insert(
            "total_elements".into(),
            Value::from(s.header.total_elements),
        );
        ex.insert(
            "valid_elements".into(),
            Value::from(s.header.valid_elements),
        );
        ex.insert(
            "scan_dimensions".into(),
            Value::Array(
                s.header
                    .dimensions
                    .iter()
                    .map(|d| json!({"size": d.size, "calibration_offset": num(d.calibration_offset), "calibration_delta": num(d.calibration_delta), "calibration_element": d.calibration_element, "description": d.description, "units": d.units}))
                    .collect(),
            ),
        );
        let cal = |c: &super::file::AxisCalibration| json!({"offset": num(c.offset), "delta": num(c.delta), "element": c.element});
        ex.insert(
            "element_calibration".into(),
            json!({"x": cal(&f.calibration_x), "y": f.calibration_y.as_ref().map(cal)}),
        );
        if let Some(e) = &self.emi {
            let summary = e.summary();
            if let Some(v) = summary["accelerating_voltage_v"].as_f64() {
                ex.insert("voltage_kv".into(), num(v / 1000.0));
            }
            let desc = e.description();
            let get = |label: &str| {
                desc.iter()
                    .find(|(l, _, _)| l == label)
                    .map(|(_, v, _)| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            };
            if let Some(m) = get("Magnification").and_then(|v| v.parse::<f64>().ok()) {
                ex.insert("magnification".into(), num(m));
            }
            if let Some(m) = get("Mode") {
                ex.insert("mode".into(), Value::from(m));
            }
            im.instrument = Some(InstrumentInfo {
                manufacturer: None,
                model: get("Microscope"),
                software: Some("TIA / ES Vision".into()),
                software_version: None,
                detector: None,
            });
            ex.insert("emi".into(), summary);
        }
        Ok(im.finish())
    }

    /// Indices of the series of 1-D elements (one trace each).
    fn spectrum_series(&self) -> Vec<usize> {
        (0..self.series.len())
            .filter(|&k| {
                self.series[k].header.data_type_id == ELEMENTS_1D && self.series[k].first.is_some()
            })
            .collect()
    }

    fn trace_infos(&self) -> Vec<openreadout_core::model::TraceInfo> {
        self.spectrum_series()
            .into_iter()
            .enumerate()
            .filter_map(|(t, k)| {
                let s = &self.series[k];
                let f = s.first?;
                let c = &f.calibration_x;
                let mut extra = std::collections::BTreeMap::new();
                extra.insert("image".into(), Value::from(k as u64));
                extra.insert(
                    "axis".into(),
                    json!({"quantity": "energy", "unit": "eV", "first": num(bin_energy(c, 0)),
                           "last": num(bin_energy(c, u64::from(f.size_x.saturating_sub(1)))),
                           "step": num(c.delta), "size": f.size_x}),
                );
                Some(openreadout_core::model::TraceInfo {
                    index: t as u32,
                    name: s.path.file_stem().map(|n| n.to_string_lossy().to_string()),
                    sample_rate_hz: 0.0,
                    sample_count: u64::from(f.size_x),
                    sweep_count: s.valid(),
                    channels: vec![openreadout_core::model::SignalChannelInfo {
                        index: 0,
                        name: "counts".into(),
                        unit: None,
                        dtype: f.data_type.label().into(),
                        scale: 1.0,
                        offset: 0.0,
                        extra: std::collections::BTreeMap::new(),
                    }],
                    start_s: None,
                    extra,
                })
            })
            .collect()
    }

    fn tag_record(&mut self, image: u32, element: u64) -> Option<Value> {
        let s = self.series.get_mut(image as usize)?;
        let off = *s.header.tag_offsets.get(usize::try_from(element).ok()?)?;
        if off == 0 {
            return None;
        }
        let t = read_tag(&mut s.blob, off)?;
        let mut m = serde_json::Map::new();
        m.insert("element".into(), Value::from(element));
        m.insert("frame".into(), Value::from(element));
        if s.header.data_type_id == ELEMENTS_2D {
            m.insert("t".into(), Value::from(element));
        }
        m.insert("time_unix_s".into(), Value::from(t.time));
        m.insert(
            "acquired_at".into(),
            Value::from(unix_to_iso8601(i64::from(t.time), 0)),
        );
        if let Some((x, y)) = t.position {
            m.insert("position_x".into(), num(x));
            m.insert("position_y".into(), num(y));
        }
        Some(Value::Object(m))
    }
}

impl Dataset for SerDataset {
    fn member_files(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.series.iter().map(|s| s.path.clone()).collect();
        out.extend(self.emi.iter().map(|e| e.path.clone()));
        out.retain(|p| *p != self.path && self.fs.is_file(p));
        out.dedup();
        out
    }
    fn info(&self) -> Result<FileInfo> {
        let mut images = Vec::with_capacity(self.series.len());
        for (i, s) in self.series.iter().enumerate() {
            let mut im = self.image_info(i as u32, s)?;
            // Acquisition time of the first element (seconds since 1970).
            if let Some(&off) = s.header.tag_offsets.first()
                && off > 0
            {
                let mut blob = Blob::open(&self.fs, &s.path)?;
                if let Some(t) = read_tag(&mut blob, off)
                    && t.time > 0
                {
                    im.acquired_at = Some(unix_to_iso8601(i64::from(t.time), 0));
                }
            }
            images.push(im);
        }
        let mut notes = Vec::new();
        if self.emi.is_none() {
            notes.push("no .emi sidecar found next to the .ser (or it holds no <ObjectInfo>): microscope metadata is unavailable".into());
        }
        if self.series.iter().any(|s| s.layout() == SerLayout::Rows) {
            notes.push(
                "1-D elements (spectra) outside a complete scan are exposed as an image with one row per element".into(),
            );
        }
        if self.series.iter().any(|s| s.layout() == SerLayout::Scan) {
            notes.push("spectrum scans (1-D elements filling the scan dimensions) are images of the scan with one channel per spectrum bin (X = scan dimension 0, Y = dimension 1)".into());
        }
        if self
            .series
            .iter()
            .any(|s| s.header.data_type_id == ELEMENTS_1D)
        {
            notes.push("every series of spectra is also a trace whose sweeps are its elements; energies are offset + (bin - element) x delta in eV (the unit is not stored in the .ser; inferred)".into());
        }
        if self
            .series
            .iter()
            .any(|s| s.first.is_some_and(|f| f.data_type.pixel_type().is_none()))
        {
            notes.push(
                "element data of an unknown type is described but not decoded; reading planes exits 6".into(),
            );
        }
        notes.push("pixel sizes come from element calibration deltas below 1 mm, taken as metres (the .ser does not store units)".into());
        if self.series.iter().any(|s| {
            s.layout() == SerLayout::Scan
                && s.header.dimensions.iter().take(2).any(|d| {
                    d.calibration_delta.abs() >= MAX_METRE_DELTA
                        && (d.units.trim().eq_ignore_ascii_case("meters") || d.units.trim() == "m")
                })
        }) {
            notes.push("scan steps of 1 mm or more (TIA writes 1 m on uncalibrated positions, e.g. point spectra) are not pixel sizes; the step stays in extra.scan_dimensions".into());
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: SerReader.descriptor(),
            format_version: self
                .series
                .first()
                .map(|s| format!("0x{:04x}", s.header.version)),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: self.trace_infos(),
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
        let image = self
            .spectrum_series()
            .get(index as usize)
            .copied()
            .ok_or_else(|| Error::Usage(format!("trace {index} out of range")))?;
        let s = &mut self.series[image];
        if sweep >= s.valid() {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (trace {index} has {} sweeps)",
                s.valid()
            )));
        }
        let off = s.header.data_offsets[sweep as usize];
        let e = read_element(&mut s.blob, ELEMENTS_1D, off)?;
        let pt = e.data_type.pixel_type().ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("{} element data", e.data_type.label()),
                "Only integer, float and complex TIA elements are decoded.",
            )
        })?;
        let len = e
            .data_len()
            .ok_or_else(|| Error::corrupt_at(FORMAT_ID, off, "element size overflows"))?;
        let raw = s.blob.read_at(FORMAT_ID, e.data_offset, len)?;
        let values: Vec<f64> = pt.samples_f64(&raw).collect();
        let a = usize::try_from(first_sample)
            .unwrap_or(usize::MAX)
            .min(values.len());
        let b = a
            .saturating_add(usize::try_from(max_samples).unwrap_or(usize::MAX))
            .min(values.len());
        Ok(openreadout_core::model::Trace {
            trace: index,
            sweep,
            first_sample: a as u64,
            channels: vec![values[a..b].to_vec()],
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let series: Vec<Value> = self
            .series
            .iter()
            .map(|s| {
                let h = &s.header;
                json!({
                    "file": s.path.file_name().map(|n| n.to_string_lossy().to_string()),
                    "SeriesVersion": h.version, "DataTypeID": h.data_type_id, "TagTypeID": h.tag_type_id,
                    "TotalNumberElements": h.total_elements, "ValidNumberElements": h.valid_elements,
                    "OffsetArrayOffset": h.offset_array_offset,
                    "Dimensions": h.dimensions.iter().map(|d| json!({"DimensionSize": d.size, "CalibrationOffset": num(d.calibration_offset), "CalibrationDelta": num(d.calibration_delta), "CalibrationElement": d.calibration_element, "Description": d.description, "Units": d.units})).collect::<Vec<_>>(),
                })
            })
            .collect();
        let mut out = json!({ "series": series });
        if let Some(e) = &self.emi {
            out["emi"] = xml_to_json(&e.xml).unwrap_or(Value::String(e.xml.clone()));
        }
        Ok(out)
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::VendorImpl),
            ("images[].size_x", Source::VendorImpl),
            ("images[].size_y", Source::VendorImpl),
            ("images[].size_t", Source::Inferred),
            ("images[].pixel_type", Source::VendorImpl),
            ("images[].physical_size", Source::Inferred),
            ("images[].acquired_at", Source::VendorImpl),
            ("images[].instrument", Source::Inferred),
            ("images[].extra.emi", Source::Inferred),
            ("images[].extra.voltage_kv", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        if let Some(e) = &self.emi {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: e.path.display().to_string(),
                offset: Some(e.offset),
                size: Some(e.xml.len() as u64),
                image: None,
                details: json!({"element": "ObjectInfo", "well_formed": e.well_formed()}),
            });
        }
        for (i, s) in self.series.iter().enumerate() {
            let h = &s.header;
            out.push(LsEntry {
                kind: "container".into(),
                name: s.path.display().to_string(),
                offset: Some(h.offset_array_offset),
                size: Some(u64::from(h.total_elements) * 2 * h.offset_width()),
                image: Some(i as u32),
                details: json!({"version": format!("0x{:04x}", h.version), "elements": h.valid_elements, "total_elements": h.total_elements}),
            });
            for k in 0..u64::from(s.valid()).min(4096) {
                let off = h.data_offsets.get(k as usize).copied();
                out.push(LsEntry {
                    kind: "plane".into(),
                    name: format!(
                        "{} element {k}",
                        s.path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    ),
                    offset: off,
                    size: s.first.and_then(|f| f.data_len()),
                    image: Some(i as u32),
                    details: json!({"tag_offset": h.tag_offsets.get(k as usize)}),
                });
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let s = self.series(image)?;
        let first = s
            .first
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "the series has no valid elements"))?;
        let Some(pixel_type) = first.data_type.pixel_type() else {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("{} element data", first.data_type.label()),
                "Element data of this type is not decoded; export it from TIA as a real-valued image.",
            ));
        };
        let (sx, sy, sc, st) = Self::geometry(s);
        if index.c >= sc || index.z > 0 || index.t >= st {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image} (c<{sc}, z<1, t<{st})",
                index.c, index.z, index.t
            )));
        }
        let kind = s.header.data_type_id;
        let layout = s.layout();
        let elements: Vec<u64> = match layout {
            SerLayout::Rows => s
                .header
                .data_offsets
                .iter()
                .take(sy as usize)
                .copied()
                .collect(),
            SerLayout::Scan => s
                .header
                .data_offsets
                .iter()
                .take(s.valid() as usize)
                .copied()
                .collect(),
            SerLayout::Frames => vec![s.header.data_offsets[index.t as usize]],
        };
        let bps = pixel_type.bytes_per_sample() as u64;
        let s = &mut self.series[image as usize];
        let mut data = Vec::new();
        for off in elements {
            let e = read_element(&mut s.blob, kind, off)?;
            if (e.size_x, e.size_y, e.data_type) != (first.size_x, first.size_y, first.data_type) {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "series whose elements differ in size or type",
                    "Each element of this series has its own geometry; this reader exposes uniform series only.",
                ));
            }
            if layout == SerLayout::Scan {
                // one bin of every spectrum, in element order (X fastest)
                data.extend(s.blob.read_at(
                    FORMAT_ID,
                    e.data_offset + u64::from(index.c) * bps,
                    bps,
                )?);
                continue;
            }
            let len = e
                .data_len()
                .ok_or_else(|| Error::corrupt_at(FORMAT_ID, off, "element size overflows"))?;
            data.extend(s.blob.read_at(FORMAT_ID, e.data_offset, len)?);
        }
        if kind == ELEMENTS_2D {
            // 2-D elements are stored bottom row first; return rows top to bottom (see ser.md).
            let row = sx as usize * pixel_type.bytes_per_sample();
            if row > 0 {
                data = data.chunks_exact(row).rev().flatten().copied().collect();
            }
        }
        Ok(Plane {
            width: sx,
            height: sy,
            pixel_type,
            samples_per_pixel: 1,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("header: byte order 'II', series id 0x0197, version 0x0210/0x0220, data and tag type ids");
        r.performed("valid <= total elements; total = product of the dimension sizes");
        r.performed("every valid element's header and data, and every tag, lie inside the file; elements share one geometry");
        r.performed(".emi sidecar present and its <ObjectInfo> XML well-formed");
        for s in &mut self.series {
            let h = s.header.clone();
            let name = s
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !matches!(h.version, 0x0210 | 0x0220) {
                r.push(
                    Finding::warning(
                        "series_version",
                        format!(
                            "{name}: series version 0x{:04x} is not 0x0210 or 0x0220",
                            h.version
                        ),
                    )
                    .at(4),
                );
            }
            if !matches!(h.data_type_id, ELEMENTS_1D | ELEMENTS_2D) {
                r.push(
                    Finding::error(
                        "data_type_id",
                        format!("{name}: data type id 0x{:04x}", h.data_type_id),
                    )
                    .at(6),
                );
                continue;
            }
            if !matches!(h.tag_type_id, TAG_TIME | TAG_TIME_POSITION) {
                r.push(
                    Finding::warning(
                        "tag_type_id",
                        format!("{name}: tag type id 0x{:04x}", h.tag_type_id),
                    )
                    .at(10),
                );
            }
            if h.valid_elements > h.total_elements {
                r.push(
                    Finding::error(
                        "element_count",
                        format!(
                            "{name}: {} valid elements of {} total",
                            h.valid_elements, h.total_elements
                        ),
                    )
                    .at(18),
                );
            } else if h.valid_elements < h.total_elements {
                r.push(Finding::warning("incomplete_series", format!("{name}: only {} of {} elements were written (acquisition stopped early?)", h.valid_elements, h.total_elements)).at(18));
            }
            let product: u64 = h.dimensions.iter().map(|d| u64::from(d.size)).product();
            if !h.dimensions.is_empty() && product != u64::from(h.total_elements) {
                r.push(Finding::warning(
                    "dimension_product",
                    format!(
                        "{name}: dimension sizes multiply to {product}, total elements is {}",
                        h.total_elements
                    ),
                ));
            }
            let first = s.first;
            let len = s.blob.len;
            let valid = s.valid();
            for k in 0..valid as usize {
                let off = h.data_offsets[k];
                match read_element(&mut s.blob, h.data_type_id, off) {
                    Ok(e) => {
                        let end = e.data_len().map(|l| e.data_offset.saturating_add(l));
                        if end.is_none_or(|end| end > len) {
                            r.push(Finding::error("truncated", format!("{name}: element {k} data runs past the end of the file ({len} bytes)")).at(off));
                        }
                        if let Some(f) = first
                            && (e.size_x, e.size_y, e.data_type)
                                != (f.size_x, f.size_y, f.data_type)
                        {
                            r.push(
                                Finding::warning(
                                    "mixed_elements",
                                    format!(
                                        "{name}: element {k} is {}x{} {}, element 0 is {}x{} {}",
                                        e.size_x,
                                        e.size_y,
                                        e.data_type.label(),
                                        f.size_x,
                                        f.size_y,
                                        f.data_type.label()
                                    ),
                                )
                                .at(off),
                            );
                        }
                    }
                    Err(e) => {
                        r.push(
                            Finding::error("truncated", format!("{name}: element {k}: {e}"))
                                .at(off),
                        );
                        break;
                    }
                }
                let t = h.tag_offsets[k];
                if t != 0 && t.saturating_add(8) > len {
                    r.push(
                        Finding::error(
                            "truncated",
                            format!("{name}: tag of element {k} lies past the end of the file"),
                        )
                        .at(t),
                    );
                }
            }
        }
        match &self.emi {
            None if !self.opened_emi => r.push(Finding::info(
                "no_emi",
                "no .emi sidecar with <ObjectInfo> next to the .ser",
            )),
            None => r.push(Finding::warning(
                "emi_unreadable",
                "the .emi holds no <ObjectInfo> document",
            )),
            Some(e) if !e.well_formed() => r.push(
                Finding::warning(
                    "emi_xml",
                    "the .emi <ObjectInfo> document is not well-formed XML",
                )
                .at(e.offset),
            ),
            Some(_) => {}
        }
        Ok(r)
    }

    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        let Some(s) = self.series.get(image as usize) else {
            return Ok((0, Vec::new()));
        };
        let total = u64::from(s.valid());
        let take = limit.map_or(total, |l| (l as u64).min(total));
        // `frames` takes &self: read tags through a fresh handle.
        let mut ds = SerDataset {
            path: self.path.clone(),
            fs: self.fs.clone(),
            series: vec![SerSeries::open(&self.fs, &s.path)?],
            emi: None,
            opened_emi: false,
        };
        let recs: Vec<Value> = (0..take).filter_map(|k| ds.tag_record(0, k)).collect();
        if recs.is_empty() {
            return Ok((0, Vec::new()));
        }
        Ok((total, recs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ser::file::tests::series;

    fn scan_dim(delta: f64, units: &str) -> super::super::file::SerDimension {
        super::super::file::SerDimension {
            size: 2,
            calibration_offset: 0.0,
            calibration_delta: delta,
            calibration_element: 0,
            description: "Position".into(),
            units: units.into(),
        }
    }

    /// `rsciio-tia-16x16-2_point-spectra-2x1024_1.ser`: two point spectra whose position
    /// dimension steps 1 m (uncalibrated); a real scan steps nanometres.
    #[test]
    fn a_one_metre_scan_step_is_not_a_pixel_size() {
        assert_eq!(scan_um(Some(&scan_dim(1.0, "meters"))), None);
        assert_eq!(scan_um(Some(&scan_dim(-2e-9, "meters"))), Some(2e-3));
        assert_eq!(scan_um(Some(&scan_dim(2e-9, "Number"))), None);
    }

    #[test]
    fn a_series_is_one_image_with_elements_as_t() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Scan_1.ser");
        std::fs::write(&p, series(3, 4, 2, 2e-10)).unwrap();
        let mut ds = SerDataset::open(&p).unwrap();
        let info = ds.info().unwrap();
        let im = &info.images[0];
        assert_eq!((im.size_x, im.size_y, im.size_t), (4, 2, 3));
        assert_eq!(im.physical_size.x, Some(2e-4));
        assert_eq!(im.acquired_at.as_deref(), Some("2020-09-13T12:26:40.000Z"));
        let plane = ds.read_plane(0, PlaneIndex { c: 0, z: 0, t: 2 }).unwrap();
        // stored bottom row first: the returned first row is the stored second row
        assert_eq!(&plane.data[..2], &2004u16.to_le_bytes());
        assert_eq!(&plane.data[8..10], &2000u16.to_le_bytes());
        let (n, recs) = ds.frames(0, None).unwrap();
        assert_eq!(n, 3);
        assert_eq!(recs[1]["time_unix_s"], 1_600_000_001);
        assert!(ds.check().unwrap().ok);
    }

    /// A series of `dims` (fastest first, metres, delta 2 nm) 1-D int32 elements of `bins`
    /// values `k * 1000 + bin`, calibration offset -20, delta 0.5, element 0; `valid` written.
    fn spectra(dims: &[u32], bins: u32, valid: u32, data_type: u16) -> Vec<u8> {
        let n: u32 = dims.iter().product();
        let mut b = Vec::new();
        b.extend_from_slice(&crate::ser::file::BYTE_ORDER_LE.to_le_bytes());
        b.extend_from_slice(&crate::ser::file::SERIES_ID.to_le_bytes());
        b.extend_from_slice(&0x0220u16.to_le_bytes());
        b.extend_from_slice(&ELEMENTS_1D.to_le_bytes());
        b.extend_from_slice(&TAG_TIME.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
        b.extend_from_slice(&valid.to_le_bytes());
        let oao_at = b.len();
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&(dims.len() as u32).to_le_bytes());
        for d in dims {
            b.extend_from_slice(&d.to_le_bytes());
            b.extend_from_slice(&0f64.to_le_bytes());
            b.extend_from_slice(&2e-9f64.to_le_bytes());
            b.extend_from_slice(&0i32.to_le_bytes());
            b.extend_from_slice(&8u32.to_le_bytes());
            b.extend_from_slice(b"Position");
            b.extend_from_slice(&6u32.to_le_bytes());
            b.extend_from_slice(b"meters");
        }
        let oao = b.len() as u64;
        b[oao_at..oao_at + 8].copy_from_slice(&oao.to_le_bytes());
        let arrays = b.len();
        b.resize(arrays + 16 * n as usize, 0);
        let mut offs = Vec::new();
        for k in 0..valid {
            offs.push(b.len() as u64);
            b.extend_from_slice(&(-20f64).to_le_bytes());
            b.extend_from_slice(&0.5f64.to_le_bytes());
            b.extend_from_slice(&0i32.to_le_bytes());
            b.extend_from_slice(&data_type.to_le_bytes());
            b.extend_from_slice(&bins.to_le_bytes());
            for i in 0..bins {
                let v = (k * 1000 + i) as i32;
                if data_type == 9 {
                    b.extend_from_slice(&(v as f32).to_le_bytes());
                    b.extend_from_slice(&1f32.to_le_bytes());
                } else {
                    b.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        for (k, o) in offs.iter().enumerate() {
            b[arrays + 8 * k..arrays + 8 * k + 8].copy_from_slice(&o.to_le_bytes());
        }
        b
    }

    fn open_bytes(bytes: &[u8]) -> (tempfile::TempDir, SerDataset) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("SI_1.ser");
        std::fs::write(&p, bytes).unwrap();
        let ds = SerDataset::open(&p).unwrap();
        (dir, ds)
    }

    #[test]
    fn a_spectrum_scan_is_an_image_of_the_scan_with_bins_as_channels() {
        let (_d, mut ds) = open_bytes(&spectra(&[3, 2], 4, 6, 6));
        let info = ds.info().unwrap();
        let im = &info.images[0];
        assert_eq!((im.size_x, im.size_y, im.size_c, im.size_t), (3, 2, 4, 1));
        assert_eq!(im.physical_size.x, Some(2e-3));
        assert_eq!(im.channels[1].name.as_deref(), Some("-19.5 eV"));
        // channel 2 of elements 0..5, X fastest
        let p = ds.read_plane(0, PlaneIndex { c: 2, z: 0, t: 0 }).unwrap();
        let want: Vec<u8> = (0..6)
            .flat_map(|k: i32| (k * 1000 + 2).to_le_bytes())
            .collect();
        assert_eq!(p.data, want);
        assert_eq!(info.traces.len(), 1);
        assert_eq!(info.traces[0].sweep_count, 6);
        let t = ds.read_trace(0, 4, 0, 10).unwrap();
        assert_eq!(t.channels[0], vec![4000.0, 4001.0, 4002.0, 4003.0]);
    }

    #[test]
    fn an_incomplete_scan_keeps_one_row_per_element() {
        let (_d, ds) = open_bytes(&spectra(&[3, 2], 4, 5, 6));
        let im = &ds.info().unwrap().images[0];
        assert_eq!((im.size_x, im.size_y, im.size_c), (4, 5, 1));
        assert_eq!(im.extra["layout"], "rows");
    }

    #[test]
    fn complex_spectra_are_decoded() {
        let (_d, mut ds) = open_bytes(&spectra(&[2], 3, 2, 9));
        let info = ds.info().unwrap();
        assert_eq!(
            info.images[0].pixel_type,
            openreadout_core::PixelType::ComplexFloat
        );
        let p = ds.read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
        let want: Vec<u8> = [1f32, 1.0, 1001.0, 1.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(p.data, want);
    }

    #[test]
    fn truncated_series_fails_check() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Scan_1.ser");
        let mut b = series(3, 4, 2, 2e-10);
        b.truncate(b.len() - 30);
        std::fs::write(&p, b).unwrap();
        let mut ds = SerDataset::open(&p).unwrap();
        let r = ds.check().unwrap();
        assert!(!r.ok);
        assert!(r.findings.iter().any(|f| f.code == "truncated"));
    }
}
