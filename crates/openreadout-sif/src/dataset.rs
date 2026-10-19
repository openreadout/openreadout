//! `Dataset` implementation for Andor SIF: every data block (signal, background, …) is one
//! trace when its frames are single rows (spectra, one sweep per frame) and one image otherwise
//! (frames as time points, sub-images stacked in Y).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, LsEntry, SignalChannelInfo, Trace,
    TraceInfo,
};
use openreadout_core::pixel::PixelType;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::layout::{self, Block, SifLayout};
use crate::{FORMAT_ID, SifReader};

/// Values decoded per `read_trace` call at most.
pub const MAX_TRACE_READ: u64 = 1 << 24;
/// Bytes of a plane read at most.
pub const MAX_PLANE_BYTES: u64 = 1 << 31;

/// What a block becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exposed {
    /// Trace `n`: frames of one row.
    Trace(u32),
    /// Image `n`.
    Image(u32),
}

/// An opened SIF file.
#[derive(Debug)]
pub struct SifDataset {
    path: PathBuf,
    fs: Fs,
    handle: Option<SourceFile>,
    layout: SifLayout,
    exposed: Vec<Exposed>,
}

fn num(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

/// Unit of the calibrated frame axis (inferred from the axis name).
pub fn axis_unit(axis: &str) -> Option<&'static str> {
    match axis.trim().to_ascii_lowercase().as_str() {
        "wavelength" => Some("nm"),
        "raman shift" => Some("cm-1"),
        _ => None,
    }
}

fn iso(unix: i64) -> Option<String> {
    (unix > 0).then(|| {
        let s = openreadout_core::time::unix_to_iso8601(unix, 0);
        // whole seconds: drop the `.000`
        format!("{}Z", s.get(..19).unwrap_or(&s))
    })
}

impl SifDataset {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let fs = input.fs().clone();
        let f = fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let file_len = f.size().map_err(|e| Error::io(&path, e))?;
        let layout = layout::parse(&f, file_len)?;
        let (mut t, mut i) = (0u32, 0u32);
        let exposed = layout
            .blocks
            .iter()
            .map(|b| {
                if b.sub_images.len() == 1 && b.height() == 1 {
                    t += 1;
                    Exposed::Trace(t - 1)
                } else {
                    i += 1;
                    Exposed::Image(i - 1)
                }
            })
            .collect();
        Ok(SifDataset {
            path,
            fs,
            handle: Some(f),
            layout,
            exposed,
        })
    }

    /// The parsed file.
    pub fn layout(&self) -> &SifLayout {
        &self.layout
    }

    fn handle(&mut self) -> Result<&SourceFile> {
        if self.handle.is_none() {
            self.handle = Some(
                self.fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?,
            );
        }
        Ok(self.handle.as_ref().expect("just opened"))
    }

    fn block_of(&self, e: Exposed) -> Option<&Block> {
        self.exposed
            .iter()
            .position(|x| *x == e)
            .and_then(|k| self.layout.blocks.get(k))
    }

    fn extra(&self, b: &Block) -> BTreeMap<String, Value> {
        let mut e = BTreeMap::new();
        e.insert("block".into(), json!(b.name()));
        e.insert("sif_version".into(), json!(b.version));
        e.insert("detector".into(), json!(b.detector));
        e.insert(
            "detector_size".into(),
            json!([b.detector_size.0, b.detector_size.1]),
        );
        if b.temperature_c > -999.0 {
            e.insert("detector_temperature_c".into(), num(b.temperature_c));
        }
        e.insert("exposure_s".into(), num(b.exposure_s));
        e.insert("accumulations".into(), json!(b.accumulations));
        e.insert("accumulation_cycle_s".into(), num(b.accumulation_cycle_s));
        e.insert("kinetic_cycle_s".into(), num(b.kinetic_cycle_s));
        e.insert("readout_time_s".into(), num(b.readout_s));
        e.insert("frame_axis".into(), json!(b.frame_axis));
        e.insert("data_type".into(), json!(b.data_type));
        e.insert("image_axis".into(), json!(b.image_axis));
        e.insert(
            "sub_images".into(),
            json!(
                b.sub_images
                    .iter()
                    .map(|s| json!({"left": s.left, "top": s.top, "right": s.right, "bottom": s.bottom,
                                    "x_binning": s.x_binning, "y_binning": s.y_binning,
                                    "width": s.width, "height": s.height}))
                    .collect::<Vec<_>>()
            ),
        );
        if !b.uncalibrated() {
            e.insert(
                "calibration".into(),
                json!({"coefficients": b.calibration, "variable": "pixel number from 1",
                       "unit": axis_unit(&b.frame_axis)}),
            );
        }
        if b.frame_times_us.iter().any(|t| *t != 0) {
            e.insert(
                "frame_times_s".into(),
                json!(
                    b.frame_times_us
                        .iter()
                        .map(|t| *t as f64 * 1e-6)
                        .collect::<Vec<_>>()
                ),
            );
        }
        if !b.original_name.is_empty() {
            e.insert("original_name".into(), json!(b.original_name));
        }
        e
    }

    fn trace_info(&self, b: &Block, index: u32) -> TraceInfo {
        let mut extra = self.extra(b);
        if let Some(t) = iso(b.acquired_unix) {
            extra.insert("acquired_at".into(), json!(t));
        }
        let n = b.width();
        let mut channels = Vec::new();
        let unit = axis_unit(&b.frame_axis);
        if b.uncalibrated() {
            extra.insert(
                "axis".into(),
                json!({"quantity": "pixel number", "unit": null, "first": 1, "step": 1,
                       "last": n, "size": n}),
            );
        } else {
            let first = b.x_value(0).unwrap_or(f64::NAN);
            let last = b.x_value(n.saturating_sub(1)).unwrap_or(f64::NAN);
            extra.insert(
                "axis".into(),
                json!({"quantity": b.frame_axis, "unit": unit, "irregular": true, "channel": 0,
                       "first": num(first), "last": num(last), "size": n}),
            );
            channels.push(SignalChannelInfo {
                index: 0,
                name: b.frame_axis.clone(),
                unit: unit.map(str::to_string),
                dtype: "float64".into(),
                scale: 1.0,
                offset: 0.0,
                extra: BTreeMap::new(),
            });
        }
        channels.push(SignalChannelInfo {
            index: channels.len() as u32,
            name: if b.data_type.is_empty() {
                "counts".into()
            } else {
                b.data_type.clone()
            },
            unit: None,
            dtype: "float32".into(),
            scale: 1.0,
            offset: 0.0,
            extra: BTreeMap::new(),
        });
        if b.frames > 1 {
            extra.insert(
                "sweep_starts_s".into(),
                json!(
                    b.frame_times_us
                        .iter()
                        .map(|t| *t as f64 * 1e-6)
                        .collect::<Vec<_>>()
                ),
            );
        }
        TraceInfo {
            index,
            name: Some(b.name().into()),
            sample_rate_hz: 0.0,
            sample_count: n,
            sweep_count: u32::try_from(b.frames).unwrap_or(u32::MAX),
            channels,
            start_s: None,
            extra,
        }
    }

    fn image_info(&self, b: &Block, index: u32) -> ImageInfo {
        let mut im = ImageInfo::new(
            index,
            u32::try_from(b.width()).unwrap_or(u32::MAX),
            u32::try_from(b.height()).unwrap_or(u32::MAX),
            PixelType::Float,
        );
        im.name = Some(b.name().into());
        im.size_t = u32::try_from(b.frames).unwrap_or(u32::MAX);
        if b.frames > 1 && b.kinetic_cycle_s.is_finite() && b.kinetic_cycle_s > 0.0 {
            im.time_increment_s = Some(b.kinetic_cycle_s);
        }
        im.acquired_at = iso(b.acquired_unix);
        im.channels = vec![ChannelInfo {
            index: 0,
            name: (!b.data_type.is_empty()).then(|| b.data_type.clone()),
            exposure_ms: (b.exposure_s.is_finite() && b.exposure_s > 0.0)
                .then_some(b.exposure_s * 1e3),
            ..ChannelInfo::default()
        }];
        im.extra = self.extra(b);
        im.finish()
    }

    fn read_frame(&mut self, b: &Block, frame: u64, first: u64, count: u64) -> Result<Vec<u8>> {
        let at = b
            .data_offset
            .checked_add(frame.saturating_mul(b.frame_bytes()))
            .and_then(|o| o.checked_add(first * 4))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "sample offset overflows"))?;
        let len = count * 4;
        let file_len = self.layout.file_len;
        if at.saturating_add(len) > file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                at,
                format!(
                    "frame {frame} of the {} block runs past the end of the file ({file_len} bytes); the file is cut short",
                    b.name()
                ),
            ));
        }
        let mut buf = vec![
            0u8;
            usize::try_from(len)
                .map_err(|_| Error::Other("read larger than memory".into()))?
        ];
        self.handle()?
            .read_exact_at(at, &mut buf)
            .map_err(|e| Error::io(&self.path, e))?;
        Ok(buf)
    }
}

impl Dataset for SifDataset {
    fn info(&self) -> Result<FileInfo> {
        let l = &self.layout;
        let mut notes = Vec::new();
        let sig = &l.blocks[0];
        notes.push(format!(
            "Andor SIF version {}, {}: {} block(s) ({})",
            sig.version,
            sig.detector,
            l.blocks.len(),
            l.blocks
                .iter()
                .map(Block::name)
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if l.blocks.len() > 1 {
            notes.push("blocks after the signal are named by their position (reference, background, live, source), as Andor Solis orders them".into());
        }
        if !l.problems.is_empty() {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        let mut traces = Vec::new();
        let mut images = Vec::new();
        for (b, e) in l.blocks.iter().zip(&self.exposed) {
            match *e {
                Exposed::Trace(i) => traces.push(self.trace_info(b, i)),
                Exposed::Image(i) => images.push(self.image_info(b, i)),
            }
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: l.file_len,
            format: SifReader.descriptor(),
            format_version: Some(sig.version.to_string()),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces,
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let l = &self.layout;
        let blocks: Vec<Value> = l
            .blocks
            .iter()
            .map(|b| {
                json!({
                    "block": b.name(), "offset": b.start, "version": b.version,
                    "acquired_unix_s": b.acquired_unix, "detector": b.detector,
                    "detector_size": [b.detector_size.0, b.detector_size.1],
                    "temperature_c": num(b.temperature_c), "exposure_s": num(b.exposure_s),
                    "accumulation_cycle_s": num(b.accumulation_cycle_s),
                    "accumulated_cycle_s": num(b.accumulated_cycle_s),
                    "accumulations": b.accumulations, "kinetic_cycle_s": num(b.kinetic_cycle_s),
                    "readout_time_s": num(b.readout_s), "first_line_values": b.line3_rest,
                    "original_name": b.original_name, "user_text": b.user_text,
                    "calibration": b.calibration, "frame_calibrations": b.frame_calibrations,
                    "settings_lines": b.settings_lines, "frame_axis": b.frame_axis,
                    "data_type": b.data_type, "image_axis": b.image_axis, "area": b.area,
                    "frames": b.frames, "frame_pixels": b.frame_pixels,
                    "frame_times_us": b.frame_times_us, "data_offset": b.data_offset,
                })
            })
            .collect();
        let mut v = json!({"blocks": blocks});
        if let Some(x) = &l.xml {
            v["xml"] = openreadout_core::xmljson::xml_to_json(x).unwrap_or_else(|| json!(x));
        }
        Ok(v)
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::PriorArt),
            ("images[].size_x", Source::PriorArt),
            ("images[].size_y", Source::PriorArt),
            ("images[].size_t", Source::PriorArt),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].time_increment_s", Source::Inferred),
            ("images[].channels[].name", Source::PriorArt),
            ("images[].channels[].exposure_ms", Source::PriorArt),
            ("images[].name", Source::Inferred),
            ("traces[].name", Source::Inferred),
            ("traces[].sample_count", Source::PriorArt),
            ("traces[].sweep_count", Source::PriorArt),
            ("traces[].channels[].name", Source::PriorArt),
            ("traces[].channels[].unit", Source::Inferred),
            ("traces[].extra.axis", Source::PriorArt),
            ("traces[].extra.acquired_at", Source::PriorArt),
            ("traces[].extra.sweep_starts_s", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let l = &self.layout;
        let mut out = Vec::new();
        for (k, b) in l.blocks.iter().enumerate() {
            out.push(LsEntry {
                kind: "header".into(),
                name: format!("{} header", b.name()),
                offset: Some(b.start),
                size: Some(b.data_offset - b.start),
                image: None,
                details: json!({"version": b.version, "frames": b.frames}),
            });
            out.push(LsEntry {
                kind: "data".into(),
                name: format!("{} data", b.name()),
                offset: Some(b.data_offset),
                size: Some(b.frames * b.frame_bytes()),
                image: match self.exposed.get(k) {
                    Some(Exposed::Image(i)) => Some(*i),
                    _ => None,
                },
                details: json!({"width": b.width(), "height": b.height(), "sample": "float32"}),
            });
        }
        if let Some(x) = &l.xml {
            out.push(LsEntry {
                kind: "xml".into(),
                name: "XML trailer".into(),
                offset: Some(l.file_len - x.len() as u64),
                size: Some(x.len() as u64),
                image: None,
                details: Value::Null,
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let b = self
            .block_of(Exposed::Image(image))
            .cloned()
            .ok_or_else(|| Error::Usage(format!("image index {image} out of range")))?;
        if index.c != 0 || index.z != 0 || u64::from(index.t) >= b.frames {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range ({} frames)",
                index.c, index.z, index.t, b.frames
            )));
        }
        if b.frame_bytes() > MAX_PLANE_BYTES {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("frames of {} bytes", b.frame_bytes()),
                "The frame is larger than this reader decodes at once.",
            ));
        }
        let w = b.width();
        let mut data = Vec::with_capacity(usize::try_from(b.frame_bytes()).unwrap_or(0));
        let raw = self.read_frame(&b, u64::from(index.t), 0, b.frame_pixels)?;
        // sub-images narrower than the widest are padded with zeros on the right
        let mut pos = 0usize;
        for s in &b.sub_images {
            let row = (s.width * 4) as usize;
            for _ in 0..s.height {
                data.extend_from_slice(raw.get(pos..pos + row).unwrap_or_default());
                data.resize(data.len() + ((w - s.width) * 4) as usize, 0);
                pos += row;
            }
        }
        Ok(Plane {
            width: u32::try_from(w).unwrap_or(u32::MAX),
            height: u32::try_from(b.height()).unwrap_or(u32::MAX),
            pixel_type: PixelType::Float,
            samples_per_pixel: 1,
            data,
        })
    }

    #[allow(clippy::many_single_char_names)]
    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let b = self
            .block_of(Exposed::Trace(index))
            .cloned()
            .ok_or_else(|| Error::Usage(format!("trace index {index} out of range")))?;
        if u64::from(sweep) >= b.frames {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({} frames)",
                b.frames
            )));
        }
        let n = b.width();
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let count = max.min(n - first).min(MAX_TRACE_READ);
        let raw = self.read_frame(&b, u64::from(sweep), first, count)?;
        let y: Vec<f64> = raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect();
        let mut channels = Vec::new();
        if !b.uncalibrated() {
            // a frame's own calibration when the user text holds one per frame
            let per_frame = b.frame_calibrations.get(sweep as usize);
            channels.push(
                (first..first + count)
                    .map(|i| {
                        let p = (i + 1) as f64;
                        let c = per_frame.unwrap_or(&b.calibration);
                        c.iter().rev().fold(0.0, |acc, k| acc * p + k)
                    })
                    .collect(),
            );
        }
        channels.push(y);
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let l = &self.layout;
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("signature; every block's header: version, timing values, detector, file name, user text, axis names, image structure (frames × pixels per frame = total), sub-image areas and binning (their pixels = pixels per frame), one time stamp per frame");
        r.performed("data of every block inside the file; the four block flags after the signal; the XML trailer");
        for p in &l.problems {
            let code = if p.contains("cut short") || p.contains("past the end") {
                "truncated"
            } else {
                "structure"
            };
            r.push(Finding::error(code, p.clone()));
        }
        for b in &l.blocks {
            if b.data_end() > l.file_len {
                r.push(
                    Finding::error(
                        "truncated",
                        format!(
                            "the {} data needs {} bytes from {}; the file has {}",
                            b.name(),
                            b.frames * b.frame_bytes(),
                            b.data_offset,
                            l.file_len
                        ),
                    )
                    .at(b.data_offset),
                );
            }
            let back = b.frame_times_us.windows(2).filter(|w| w[1] < w[0]).count();
            if back > 0 {
                r.push(Finding::warning(
                    "frame_times",
                    format!(
                        "{back} frame time stamp(s) of the {} block go backwards",
                        b.name()
                    ),
                ));
            }
        }
        if l.trailing_bytes > 0 {
            r.push(Finding::warning(
                "trailing_bytes",
                format!(
                    "{} bytes after the last block are not XML and are not read",
                    l.trailing_bytes
                ),
            ));
        }
        Ok(r)
    }
}
