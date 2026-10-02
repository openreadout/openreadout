//! `Dataset` implementation for ND2 (chunk-based files and legacy JPEG 2000-based files).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{le_u32, le_u64};
use openreadout_core::limits::plane_len;
use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo, LsEntry, ObjectiveInfo,
    PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use crate::container::{ChunkHeader, Nd2File, read_at, read_chunk_header};
use crate::deinterleave;
use crate::frames::{
    CustomTag, base_record, custom_tags_from_lv, f64_values, legacy_record_fields, record_field,
    stage_z_tag, tag_values,
};
use crate::legacy::{LegacyFile, box_header, codestream_size};
use crate::lv::lv_decode;
use crate::meta::{
    Attributes, FrameLayout, Loop, LoopKind, PlaneDesc, acquisition_mode, color_hex,
    events_from_lv, jdn_to_iso8601, list_items, loops_from_lv, planes_from_lv, rois_from_lv,
    text_datetime_to_iso8601, unwrap_root,
};
use crate::variant::variant_decode;
use crate::{FORMAT_ID, LEGACY_SIGNATURE, Nd2Reader};

/// Records per image embedded in `info` for events and ROIs (the full lists are in
/// `info --view full`'s vendor tree).
const MAX_INFO_ITEMS: usize = 256;

/// Per-frame metadata pulled from the first frame's picture metadata.
#[derive(Debug, Clone, Default)]
pub struct FrameMeta {
    pub time_ms: Option<f64>,
    /// `dTimeAbsolute`: Julian day number of the experiment start.
    pub start_jdn: Option<f64>,
    pub stage_x: Option<f64>,
    pub stage_y: Option<f64>,
    pub stage_z: Option<f64>,
    pub calibration_um: Option<f64>,
    pub objective_name: Option<String>,
    pub objective_mag: Option<f64>,
    pub objective_na: Option<f64>,
    pub refractive_index: Option<f64>,
}

impl FrameMeta {
    fn from_picture_metadata(v: &Value) -> Self {
        let pm = unwrap_root(v);
        let fnum = |k: &str| pm.get(k).and_then(Value::as_f64);
        FrameMeta {
            time_ms: fnum("dTimeMSec"),
            start_jdn: fnum("dTimeAbsolute"),
            stage_x: fnum("dXPos"),
            stage_y: fnum("dYPos"),
            stage_z: fnum("dZPos"),
            calibration_um: fnum("dCalibration").filter(|c| *c > 0.0),
            objective_name: pm
                .get("wsObjectiveName")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty()),
            objective_mag: fnum("dObjectiveMag").filter(|m| *m > 0.0),
            objective_na: fnum("dObjectiveNA").filter(|n| *n > 0.0),
            refractive_index: fnum("dRefractIndex1").filter(|n| *n > 0.0),
        }
    }
}

/// Where the pixels live.
#[derive(Debug)]
enum Backend {
    /// Chunk-based file (NIS-Elements ≥ 3, `Ver2.x`/`Ver3.x`).
    Chunked(Nd2File),
    /// JPEG 2000 box file (NIS-Elements 2.x).
    Legacy(LegacyFile),
}

/// An opened ND2 file.
#[derive(Debug)]
pub struct Nd2Dataset {
    path: PathBuf,
    backend: Backend,
    attrs: Attributes,
    loops: Vec<Loop>,
    layout: FrameLayout,
    planes: Vec<PlaneDesc>,
    frame0: FrameMeta,
    /// Channel → (first interleaved sample index, samples per pixel).
    channel_samples: Vec<(u32, u32)>,
    text_info: BTreeMap<String, String>,
    events: Vec<Value>,
    rois: Vec<Value>,
    custom_tags: Vec<CustomTag>,
    camera_name: Option<String>,
    software_version: Option<String>,
    /// Metadata chunks decoded to JSON, by chunk name (for `info --view full`).
    vendor: BTreeMap<String, Value>,
    handle: Option<SourceFile>,
    /// Where the file is read from.
    fs: Fs,
    /// Frame number → offset of its `ImageDataSeq|N!` chunk, built on the first plane read.
    frame_chunks: Option<HashMap<u32, u64>>,
    /// The channels of the last multichannel frame read (split in one pass), each taken by the
    /// read that asks for it: reading the channels of a frame reads the frame once.
    split_cache: Option<(u32, Vec<Option<Vec<u8>>>)>,
}

/// LV or variant XML, whichever the payload is.
fn decode_any(raw: &[u8]) -> Option<Value> {
    let start = raw
        .iter()
        .position(|b| !b.is_ascii_whitespace() && *b != 0xEF && *b != 0xBB && *b != 0xBF);
    if start.is_some_and(|i| raw[i] == b'<') {
        variant_decode(raw).ok()
    } else {
        lv_decode(raw).ok()
    }
}

/// Most interleaved components per pixel accepted from the image attributes (real files have
/// a handful; the cap keeps a corrupt header from allocating billions of channel entries).
const MAX_COMPONENTS: u32 = 4096;

/// Swap the first and third sample of every 3-sample pixel in place (B,G,R ↔ R,G,B);
/// `bps` bytes per sample. A trailing partial pixel is left alone.
pub(crate) fn bgr_to_rgb(data: &mut [u8], bps: usize) {
    if bps == 0 {
        return;
    }
    for px in data.chunks_exact_mut(3 * bps) {
        let (b, rest) = px.split_at_mut(bps);
        b.swap_with_slice(&mut rest[bps..2 * bps]);
    }
}

/// Frame number → chunk offset of every `ImageDataSeq|N!` chunk in the chunk map.
fn frame_chunk_index(file: &Nd2File) -> HashMap<u32, u64> {
    file.map
        .iter()
        .filter_map(|e| {
            let n = e.name.strip_prefix("ImageDataSeq|")?.strip_suffix('!')?;
            Some((n.parse().ok()?, e.chunk_offset))
        })
        .collect()
}

/// The chunk header of frame chunk `name` at `off`, checked against that name: one positional
/// read of the magic, lengths and the name (with the terminating NUL NIS-Elements may write);
/// any other layout goes through the general two-read [`read_chunk_header`].
fn read_frame_header(
    f: &SourceFile,
    path: &Path,
    off: u64,
    file_len: u64,
    name: &str,
) -> Result<ChunkHeader> {
    let n = name.len();
    let want = (16 + n as u64 + 1).min(file_len.saturating_sub(off));
    if want >= 16 + n as u64 {
        let mut h = vec![0u8; usize::try_from(want).unwrap_or(0)];
        f.read_exact_at(off, &mut h)
            .map_err(|e| Error::io(path, e))?;
        let name_len = le_u32(&h, 4).unwrap_or(0) as usize;
        let exact = name_len == n || (name_len == n + 1 && h.get(16 + n) == Some(&0));
        if le_u32(&h, 0) == Some(crate::container::CHUNK_MAGIC)
            && exact
            && &h[16..16 + n] == name.as_bytes()
        {
            let data_len = le_u64(&h, 8).unwrap_or(0);
            return Ok(ChunkHeader {
                name: name.to_string(),
                name_len: name_len as u32,
                data_len,
                payload_offset: off + 16 + name_len as u64,
            });
        }
    }
    let hdr = read_chunk_header(&mut f.clone(), path, off, file_len)?;
    if hdr.name != name {
        return Err(Error::corrupt_at(
            FORMAT_ID,
            off,
            format!("chunk index says '{name}' but the chunk is '{}'", hdr.name),
        ));
    }
    Ok(hdr)
}

/// A plane from a channel's packed samples: 3-component (colour camera) planes are stored B,
/// G, R and returned R, G, B (docs/formats/nd2.md § RGB sample order).
fn finish_plane(mut data: Vec<u8>, w: u32, h: u32, pt: PixelType, spp: u32, bps: usize) -> Plane {
    if spp == 3 {
        bgr_to_rgb(&mut data, bps);
    }
    Plane {
        width: w,
        height: h,
        pixel_type: pt,
        samples_per_pixel: spp,
        data,
    }
}

fn channel_samples_for(planes: &[PlaneDesc], components: u32) -> Result<Vec<(u32, u32)>> {
    if components > MAX_COMPONENTS {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "image attributes declare {components} components per pixel (limit {MAX_COMPONENTS})"
            ),
        ));
    }
    let total: u64 = planes.iter().map(|p| u64::from(p.component_count)).sum();
    Ok(if !planes.is_empty() && total == u64::from(components) {
        let mut acc = 0;
        planes
            .iter()
            .map(|p| {
                let s = (acc, p.component_count);
                acc += p.component_count;
                s
            })
            .collect()
    } else {
        (0..components.max(1)).map(|c| (c, 1)).collect()
    })
}

impl Nd2Dataset {
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
        if len >= 12 && read_at(&mut f, path, 0, 12)? == LEGACY_SIGNATURE {
            return Self::open_legacy(fs, path, f);
        }
        Self::open_chunked(fs, path, f)
    }

    fn open_chunked(fs: &Fs, path: &Path, mut f: SourceFile) -> Result<Self> {
        let file = Nd2File::open_in(fs, path)?;
        let mut vendor = BTreeMap::new();
        // Version 3 files carry LV chunks (`…LV!`); version 2 files carry XML variants (`…!`).
        // Some files carry both; LV wins.
        let mut lv = |lv_name: &str, xml_name: &str| -> Option<Value> {
            if let Ok(raw) = file.read_chunk(&mut f, lv_name)
                && let Ok(v) = lv_decode(&raw)
            {
                vendor.insert(lv_name.to_string(), v.clone());
                return Some(v);
            }
            let raw = file.read_chunk(&mut f, xml_name).ok()?;
            let v = variant_decode(&raw).ok()?;
            vendor.insert(xml_name.to_string(), v.clone());
            Some(v)
        };
        let attrs_v = lv("ImageAttributesLV!", "ImageAttributes!")
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "no readable ImageAttributes chunk"))?;
        let mut attrs = Attributes::from_lv(&attrs_v)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "ImageAttributes lacks width/height"))?;
        if attrs.frame_count == 0 {
            // Interrupted acquisitions can leave uiSequenceCount at 0: count the frames written.
            attrs.frame_count = file
                .map
                .iter()
                .filter(|e| e.name.starts_with("ImageDataSeq|"))
                .count() as u32;
        }
        let exp = lv("ImageMetadataLV!", "ImageMetadata!");
        let loops = exp.as_ref().map(loops_from_lv).unwrap_or_default();
        let camera_name = exp
            .as_ref()
            .map(unwrap_root)
            .and_then(|e| e.get("wsCameraName"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let layout = FrameLayout::from_loops(&loops, attrs.frame_count);
        let seq0 = lv("ImageMetadataSeqLV|0!", "ImageMetadataSeq|0!");
        let planes = seq0.as_ref().map(planes_from_lv).unwrap_or_default();
        let mut frame0 = seq0
            .as_ref()
            .map(FrameMeta::from_picture_metadata)
            .unwrap_or_default();
        if let Some(s) = seq0
            .as_ref()
            .map(unwrap_root)
            .and_then(|pm| pm.get("sPicturePlanes"))
            .and_then(|p| p.get("sSampleSetting"))
            .map(list_items)
            .and_then(|l| l.into_iter().next())
            .and_then(|s| s.get("pObjectiveSetting"))
        {
            let g = |k: &str| s.get(k).and_then(Value::as_f64).filter(|v| *v > 0.0);
            frame0.objective_name = frame0.objective_name.or_else(|| {
                s.get("wsObjectiveName")
                    .and_then(Value::as_str)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string)
            });
            frame0.objective_mag = frame0.objective_mag.or_else(|| g("dObjectiveMag"));
            frame0.objective_na = frame0.objective_na.or_else(|| g("dObjectiveNA"));
            frame0.refractive_index = frame0.refractive_index.or_else(|| g("dRefractIndex"));
        }
        let calib =
            lv("ImageCalibrationLV|0!", "ImageCalibration|0!").map(|c| unwrap_root(&c).clone());
        let text_info = text_info_map(lv("ImageTextInfoLV!", "ImageTextInfo!").as_ref());
        if frame0.calibration_um.is_none() {
            frame0.calibration_um = calib
                .as_ref()
                .and_then(|c| c.get("dCalibration"))
                .and_then(Value::as_f64)
                .filter(|c| *c > 0.0);
        }
        if frame0.objective_name.is_none() {
            frame0.objective_name = calib
                .as_ref()
                .and_then(|c| c.get("sObjective"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty());
        }
        let mut decoded = |name: &str| -> Option<Value> {
            let raw = file.read_chunk(&mut f, name).ok()?;
            let v = decode_any(&raw)?;
            vendor.insert(name.to_string(), v.clone());
            Some(v)
        };
        let mut events = Vec::new();
        for name in [
            "ImageEventsLV!",
            "ImageEvents!",
            "CustomData|ExperimentEventsV1_0!",
        ] {
            if let Some(v) = decoded(name) {
                events.extend(events_from_lv(&v));
            }
        }
        let rois = decoded("CustomData|RoiMetadata_v1!")
            .map(|v| rois_from_lv(&v))
            .unwrap_or_default();
        let custom_tags = decoded("CustomDataVar|CustomDataV2_0!")
            .map(|v| custom_tags_from_lv(&v))
            .unwrap_or_default();
        let software_version = text_info
            .get("TextInfoItem_14")
            .cloned()
            .or_else(|| decoded("CustomDataVar|AppInfo_V1_0!").and_then(|v| app_version(&v)));
        let channel_samples = channel_samples_for(&planes, attrs.components)?;
        Ok(Nd2Dataset {
            path: path.to_path_buf(),
            backend: Backend::Chunked(file),
            attrs,
            loops,
            layout,
            planes,
            frame0,
            channel_samples,
            text_info,
            events,
            rois,
            custom_tags,
            camera_name,
            software_version,
            vendor,
            handle: None,
            fs: fs.clone(),
            frame_chunks: None,
            split_cache: None,
        })
    }

    fn open_legacy(fs: &Fs, path: &Path, mut f: SourceFile) -> Result<Self> {
        let lf = LegacyFile::open_in(fs, path)?;
        let mut vendor = BTreeMap::new();
        let mut xml = |tag: &str, n: usize| -> Option<Value> {
            let v = lf.xml(&mut f, tag, n)?;
            if n == 0 {
                vendor.insert(tag.to_string(), v.clone());
            }
            Some(v)
        };
        let artt = xml("ARTT", 0);
        let vimd0 = xml("VIMD", 0);
        let vcal0 = xml("VCAL", 0);
        let acal = xml("ACAL", 0);
        let tinf = xml("TINF", 0);
        let exp = xml("AIM1", 0).or_else(|| xml("AIMD", 0));
        let ieve = xml("IEVE", 0);
        let (h, w, nc, bpc) = lf.image_header(&mut f)?;
        let planes = vimd0.as_ref().map(planes_from_lv).unwrap_or_default();
        let components = vimd0
            .as_ref()
            .and_then(|v| v.get("sPicturePlanes"))
            .and_then(|p| p.get("uiCompCount"))
            .and_then(Value::as_u64)
            .map_or(u32::from(nc), |c| c as u32);
        let frame_count = lf.tagged("VCAL").len().max(lf.tagged("VIMD").len()) as u32;
        let significant = artt
            .as_ref()
            .and_then(|a| a.get("SignificantBits"))
            .and_then(Value::as_u64)
            .map_or(u32::from(bpc), |b| b as u32);
        let attrs = Attributes {
            width: w,
            height: h,
            row_bytes: 0,
            components,
            bits_in_memory: if bpc <= 8 { 8 } else { 16 },
            bits_significant: significant,
            frame_count: frame_count.max(1),
            compression: None,
            pixel_kind: Some(1),
            virtual_components: artt
                .as_ref()
                .and_then(|a| a.get("VirtualComponents"))
                .and_then(Value::as_u64)
                .map(|v| v as u32),
        };
        let loops = exp.as_ref().map(loops_from_lv).unwrap_or_default();
        let layout = FrameLayout::from_loops(&loops, attrs.frame_count);
        let mut frame0 = vimd0
            .as_ref()
            .map(FrameMeta::from_picture_metadata)
            .unwrap_or_default();
        let vcal = vcal0.as_ref();
        if frame0.calibration_um.is_none() {
            frame0.calibration_um = vcal
                .and_then(|c| c.get("dCalibration"))
                .and_then(Value::as_f64)
                .or_else(|| {
                    acal.as_ref()
                        .and_then(|a| a.get("CalibrationValues"))
                        .and_then(|c| c.get("Calibration_11"))
                        .and_then(Value::as_str)
                        .and_then(|s| s.parse::<f64>().ok())
                })
                .filter(|c| *c > 0.0);
        }
        if frame0.objective_name.is_none() {
            frame0.objective_name = vcal
                .and_then(|c| c.get("sObjective"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
        let mut text_info = BTreeMap::new();
        if let Some(t) = &tinf {
            for item in t.get("TextInfoItem").map(list_items).unwrap_or_default() {
                if let (Some(i), Some(text)) = (
                    item.get("Index").and_then(Value::as_str),
                    item.get("Text").and_then(Value::as_str),
                ) && !text.is_empty()
                {
                    text_info.insert(format!("TextInfoItem_{i}"), text.to_string());
                }
            }
        }
        let events = ieve.as_ref().map(events_from_lv).unwrap_or_default();
        let camera_name = planes.iter().find_map(|p| p.camera.detector.clone());
        let channel_samples = channel_samples_for(&planes, attrs.components)?;
        Ok(Nd2Dataset {
            path: path.to_path_buf(),
            backend: Backend::Legacy(lf),
            attrs,
            loops,
            layout,
            planes,
            frame0,
            channel_samples,
            text_info,
            events,
            rois: Vec::new(),
            custom_tags: Vec::new(),
            camera_name,
            software_version: None,
            vendor,
            handle: None,
            fs: fs.clone(),
            frame_chunks: None,
            split_cache: None,
        })
    }

    fn pixel_type(&self) -> Result<PixelType> {
        Ok(
            match (
                self.attrs.pixel_kind.unwrap_or(1),
                self.attrs.bits_in_memory,
            ) {
                (1, 8) => PixelType::Uint8,
                (1, 16) => PixelType::Uint16,
                (1, 32) => PixelType::Uint32,
                (2, 32) => PixelType::Float,
                (2, 64) => PixelType::Double,
                (k, b) => {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        format!("pixel kind {k} with {b} bits"),
                        "Only 8/16/32-bit unsigned and 32/64-bit float frames are supported.",
                    ));
                }
            },
        )
    }

    fn handle(&mut self) -> Result<&mut SourceFile> {
        if self.handle.is_none() {
            self.handle = Some(
                self.fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?,
            );
        }
        Ok(self.handle.as_mut().expect("just opened"))
    }

    fn is_legacy(&self) -> bool {
        matches!(self.backend, Backend::Legacy(_))
    }

    fn file_len(&self) -> u64 {
        match &self.backend {
            Backend::Chunked(f) => f.file_len,
            Backend::Legacy(l) => l.file_len,
        }
    }

    fn problems(&self) -> &[(Option<u64>, String)] {
        match &self.backend {
            Backend::Chunked(f) => &f.problems,
            Backend::Legacy(l) => &l.problems,
        }
    }

    fn rescued(&self) -> bool {
        match &self.backend {
            Backend::Chunked(f) => f.rescued,
            Backend::Legacy(l) => l.rescued,
        }
    }

    /// Frames the file actually addresses: the layout, capped at the frames written.
    fn addressable_frames(&self) -> u32 {
        let n = self.layout.frames().min(u64::from(self.attrs.frame_count));
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    /// Time increment: a time loop's period, or the common period of an NE-time loop's valid
    /// periods (`None` when they differ; see `extra.time_periods`).
    fn time_increment_ms(&self) -> Option<f64> {
        let lp = self
            .loops
            .iter()
            .find(|l| matches!(l.kind, LoopKind::TimeLoop | LoopKind::NeTimeLoop))?;
        if lp.kind == LoopKind::NeTimeLoop && lp.periods.len() > 1 {
            let first = lp.periods[0].period_ms?;
            if lp
                .periods
                .iter()
                .all(|p| p.period_ms.is_some_and(|q| (q - first).abs() < 1e-9))
            {
                return Some(first).filter(|p| *p > 0.0);
            }
            return None;
        }
        lp.period_ms.filter(|p| *p > 0.0)
    }

    fn loops_json(&self) -> Value {
        Value::Array(
            self.loops
                .iter()
                .map(|l| {
                    let mut o = Map::new();
                    o.insert("kind".into(), Value::String(l.kind.name()));
                    o.insert("count".into(), Value::from(l.count));
                    if let Some(d) = l.declared_count.filter(|d| *d != l.count) {
                        o.insert("declared_count".into(), Value::from(d));
                    }
                    if l.invalid_items > 0 {
                        o.insert("invalid_items".into(), Value::from(l.invalid_items));
                    }
                    if let Some(r) = l.repeat_count.filter(|r| *r > 1) {
                        o.insert("repeat_count".into(), Value::from(r));
                    }
                    if let Some(p) = l.period_ms.filter(|_| l.kind == LoopKind::TimeLoop) {
                        o.insert("period_ms".into(), json!(p));
                    }
                    if let Some(z) = l.z_step_um {
                        o.insert("z_step_um".into(), json!(z));
                    }
                    Value::Object(o)
                })
                .collect(),
        )
    }

    fn channel_settings_json(&self) -> Value {
        Value::Array(
            self.planes
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let mut o = Map::new();
                    o.insert("index".into(), Value::from(i as u32));
                    o.insert(
                        "modality".into(),
                        Value::Array(
                            p.modality
                                .iter()
                                .map(|m| Value::String((*m).into()))
                                .collect(),
                        ),
                    );
                    if !p.filters.is_empty() {
                        o.insert("filters".into(), json!(p.filters));
                    }
                    let c = &p.camera;
                    for (k, v) in [
                        ("detector", c.detector.as_ref().map(|d| json!(d))),
                        ("exposure_ms", c.exposure_ms.map(|v| json!(v))),
                        ("binning", c.binning.as_ref().map(|b| json!(b))),
                        ("gain", c.gain.map(|v| json!(v))),
                        ("em_gain", c.em_gain.map(|v| json!(v))),
                    ] {
                        if let Some(v) = v {
                            o.insert(k.into(), v);
                        }
                    }
                    if !c.settings_text.is_empty() {
                        o.insert("camera_settings".into(), json!(c.settings_text));
                    }
                    Value::Object(o)
                })
                .collect(),
        )
    }

    fn image_info(&self, p: u32) -> Result<ImageInfo> {
        let pt = self.pixel_type()?;
        let mut info = ImageInfo::new(p, self.attrs.width, self.attrs.height, pt);
        let xy = self
            .loops
            .iter()
            .find(|l| l.kind == LoopKind::XyPositionLoop);
        info.name = if self.layout.p_count > 1 {
            let name = xy
                .and_then(|l| l.positions.get(p as usize))
                .and_then(|q| q.pos_name.clone());
            Some(name.unwrap_or_else(|| format!("Position {p}")))
        } else {
            None
        };
        info.size_z = self.layout.z_count;
        info.size_t = self.layout.t_count;
        info.size_c = self.channel_samples.len() as u32;
        info.samples_per_pixel = self.channel_samples.iter().map(|s| s.1).max().unwrap_or(1);
        info.dimension_order = "XYCZT".into();
        let z_step = self
            .loops
            .iter()
            .find(|l| l.kind == LoopKind::ZStackLoop)
            .and_then(|l| l.z_step_um)
            .filter(|s| *s > 0.0);
        info.physical_size = PhysicalSize::micrometres(
            self.frame0.calibration_um,
            self.frame0.calibration_um,
            z_step,
        );
        info.time_increment_s = self.time_increment_ms().map(|ms| ms / 1000.0);
        info.channels = self
            .channel_samples
            .iter()
            .enumerate()
            .map(|(i, (_, spp))| {
                let pd = self.planes.get(i);
                ChannelInfo {
                    index: i as u32,
                    name: pd.and_then(|p| p.description.clone()),
                    fluorophore: pd.and_then(|p| p.fluorophore.clone()),
                    // RGB planes carry no wavelengths.
                    excitation_nm: pd.and_then(|p| p.excitation_nm).filter(|_| *spp == 1),
                    emission_nm: pd.and_then(|p| p.emission_nm).filter(|_| *spp == 1),
                    emission_range_nm: pd.and_then(|p| p.emission_band_nm).filter(|_| *spp == 1),
                    color: pd.and_then(|p| p.color_abgr).map(color_hex),
                    acquisition_mode: pd.and_then(|p| acquisition_mode(&p.modality)),
                    exposure_ms: pd.and_then(|p| p.camera.exposure_ms),

                    ..ChannelInfo::default()
                }
            })
            .collect();
        // The optics text (`TextInfoItem_13`) and the description's `Numerical Aperture:` line
        // stand in when the frame metadata names no objective (`ome-jonas-control002`).
        let objective_name = self.frame0.objective_name.clone().or_else(|| {
            self.text_info
                .get("TextInfoItem_13")
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        });
        let objective_na = self.frame0.objective_na.or_else(|| {
            self.text_info
                .get("TextInfoItem_5")
                .and_then(|t| na_from_description(t))
        });
        if objective_name.is_some() || objective_na.is_some() {
            info.objective = Some(ObjectiveInfo {
                nominal_magnification: self
                    .frame0
                    .objective_mag
                    .or_else(|| objective_name.as_deref().and_then(magnification_from_name)),
                model: objective_name,
                lens_na: objective_na,
                immersion: None,
            });
        }
        if let Some(n) = self
            .frame0
            .refractive_index
            .filter(|n| n.is_finite() && *n > 0.0)
        {
            info.extra.insert("refractive_index".into(), json!(n));
        }
        let detector = self
            .camera_name
            .clone()
            .or_else(|| self.planes.iter().find_map(|p| p.camera.detector.clone()));
        info.instrument = Some(InstrumentInfo {
            manufacturer: Some("Nikon".into()),
            model: None,
            software: Some("NIS-Elements".into()),
            software_version: self.software_version.clone(),
            detector,
        });
        // Julian day (UTC) first; the local-time text only when it is unambiguous.
        let text = self.text_info.get("TextInfoItem_9");
        let (acquired, source) = match self.frame0.start_jdn.and_then(jdn_to_iso8601) {
            Some(iso) => (Some(iso), "julian_day_utc"),
            None => match text.and_then(|t| text_datetime_to_iso8601(t)) {
                Some(iso) => (Some(iso), "text_local_time"),
                None => (None, ""),
            },
        };
        if acquired.is_some() {
            info.extra
                .insert("acquired_at_source".into(), Value::String(source.into()));
        }
        info.acquired_at = acquired;
        if let Some(t) = text {
            info.extra
                .insert("acquired_at_text".into(), Value::String(t.clone()));
        }
        info.extra.insert(
            "first_frame".into(),
            json!({"time_ms": self.frame0.time_ms, "stage_x": self.frame0.stage_x, "stage_y": self.frame0.stage_y, "stage_z": self.frame0.stage_z}),
        );
        if let Some(q) = xy.and_then(|l| l.positions.get(p as usize)) {
            info.extra.insert(
                "stage_position_um".into(),
                json!({"x": q.pos_x, "y": q.pos_y, "z": q.pos_z}),
            );
        }
        if info.samples_per_pixel == 3 {
            // Planes are returned in `sample_order`; the file stores `stored_sample_order`.
            let stored = if self.is_legacy() { "RGB" } else { "BGR" };
            info.extra
                .insert("sample_order".into(), Value::String("RGB".into()));
            info.extra
                .insert("stored_sample_order".into(), Value::String(stored.into()));
        }
        info.extra.insert("loops".into(), self.loops_json());
        if let Some(ne) = self
            .loops
            .iter()
            .find(|l| l.kind == LoopKind::NeTimeLoop && l.periods.len() > 1)
        {
            let mut first = 0u32;
            let periods: Vec<Value> = ne
                .periods
                .iter()
                .map(|pd| {
                    let v = json!({
                        "first_t": first,
                        "count": pd.count,
                        "period_ms": pd.period_ms,
                        "time_increment_s": pd.period_ms.filter(|p| *p > 0.0).map(|ms| ms / 1000.0),
                        "duration_ms": pd.duration_ms,
                    });
                    first += pd.count;
                    v
                })
                .collect();
            info.extra
                .insert("time_periods".into(), Value::Array(periods));
        }
        if !self.planes.is_empty() {
            info.extra
                .insert("channel_settings".into(), self.channel_settings_json());
        }
        if !self.events.is_empty() {
            info.extra.insert(
                "events".into(),
                Value::Array(self.events.iter().take(MAX_INFO_ITEMS).cloned().collect()),
            );
            info.extra
                .insert("event_count".into(), Value::from(self.events.len()));
        }
        let rois: Vec<Value> = self
            .rois
            .iter()
            .filter(|r| {
                r.get("position_index")
                    .and_then(Value::as_u64)
                    .is_none_or(|q| q == u64::from(p))
            })
            .take(MAX_INFO_ITEMS)
            .cloned()
            .collect();
        if !rois.is_empty() {
            info.extra.insert("rois".into(), Value::Array(rois));
        }
        if let Some(d) = self
            .text_info
            .get("TextInfoItem_5")
            .and_then(|t| t.lines().find(|l| l.starts_with("Dimensions:")))
        {
            info.extra.insert(
                "dimensions_text".into(),
                Value::String(d.trim().to_string()),
            );
        }
        info.extra.insert(
            "bits_significant".into(),
            Value::from(self.attrs.bits_significant),
        );
        if self.is_legacy() {
            info.extra
                .insert("container".into(), Value::String("legacy_jpeg2000".into()));
        }
        Ok(info.finish())
    }

    fn read_plane_chunked(&mut self, frame: u32, c: u32, pt: PixelType) -> Result<Plane> {
        let compression = self.attrs.compression.unwrap_or(2);
        if compression != 2 && compression != 0 {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("lossy-compressed frames (eCompression {compression})"),
                "Only uncompressed (2) and lossless zlib (0) frames are decoded; no public sample of a lossy ND2 was available to derive the codec from (see docs/formats/nd2.md). Export the file uncompressed or as TIFF from NIS-Elements.",
            ));
        }
        let name = format!("ImageDataSeq|{frame}!");
        let (first, spp) = self.channel_samples[c as usize];
        let (w, h, comps, row_bytes) = (
            self.attrs.width,
            self.attrs.height,
            self.attrs.components,
            self.attrs.row_bytes,
        );
        let bps = pt.bytes_per_sample();
        let Backend::Chunked(file) = &self.backend else {
            unreachable!("chunked read on a legacy file")
        };
        let index = self
            .frame_chunks
            .get_or_insert_with(|| frame_chunk_index(file));
        let chunk_offset = index.get(&frame).copied().ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("frame chunk {name} missing (file truncated or frames not written)"),
            )
        })?;
        // A multichannel frame split by an earlier read of another of its channels.
        if let Some((f, parts)) = self.split_cache.as_mut()
            && *f == frame
            && let Some(data) = parts.get_mut(c as usize).and_then(Option::take)
        {
            return Ok(finish_plane(data, w, h, pt, spp, bps));
        }
        let file_len = self.file_len();
        let path = self.path.clone();
        let f = self.handle()?;
        let hdr = read_frame_header(f, &path, chunk_offset, file_len, &name)?;
        if hdr.payload_offset.saturating_add(hdr.data_len) > file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                hdr.payload_offset,
                format!(
                    "frame chunk {name} declares {} bytes but the file ends {} bytes into it (truncated)",
                    hdr.data_len,
                    file_len.saturating_sub(hdr.payload_offset)
                ),
            ));
        }
        let stride = if row_bytes == 0 {
            w as usize * comps as usize * bps
        } else {
            row_bytes as usize
        };
        let px_in = comps as usize * bps;
        if stride < w as usize * px_in {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("row stride {stride} is smaller than {w} pixels × {px_in} bytes"),
            ));
        }
        // Geometry checks before any allocation: frame and output plane sizes are capped
        // relative to the file (a zlib frame cannot inflate to gigabytes from a few KB).
        let frame_bytes = plane_len(FORMAT_ID, stride as u64, u64::from(h), 1, 1, file_len)?;
        let out_len = plane_len(
            FORMAT_ID,
            u64::from(w),
            u64::from(h),
            u64::from(spp),
            bps as u64,
            file_len,
        )?;
        let need = 8 + frame_bytes;
        let raw = if compression == 0 {
            // Lossless: the pixel block after the 8-byte timestamp is a zlib stream.
            let packed = read_at(
                f,
                &path,
                hdr.payload_offset + 8,
                usize::try_from(hdr.data_len.saturating_sub(8)).unwrap_or(usize::MAX),
            )?;
            let out = openreadout_codecs::zlib_decode(&packed, frame_bytes)
                .map_err(|e| Error::corrupt_at(FORMAT_ID, hdr.payload_offset, e.to_string()))?;
            if out.len() < frame_bytes {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    hdr.payload_offset,
                    format!(
                        "frame {frame} decompressed to {} bytes, geometry needs {}",
                        out.len(),
                        frame_bytes
                    ),
                ));
            }
            out
        } else {
            if hdr.data_len < need as u64 {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    hdr.payload_offset,
                    format!(
                        "frame {frame} holds {} bytes, geometry needs {need}",
                        hdr.data_len
                    ),
                ));
            }
            let mut raw = vec![0u8; frame_bytes];
            f.read_exact_at(hdr.payload_offset + 8, &mut raw)
                .map_err(|e| Error::io(&path, e))?;
            raw
        };
        let px_out = spp as usize * bps;
        let row_out = w as usize * px_out;
        let (wu, hu) = (w as usize, h as usize);
        let data = if out_len == 0 {
            Vec::new()
        } else if px_in == px_out && stride == row_out {
            // One channel covering every component and unpadded rows: the frame is the plane.
            let mut raw = raw;
            raw.truncate(out_len);
            raw
        } else {
            // `raw` holds at least `stride * h` bytes (checked above). The channels partition
            // the pixel (`channel_samples`): split them all in one pass and keep the others for
            // the next reads of this frame.
            let spans: Vec<deinterleave::Span> = self
                .channel_samples
                .iter()
                .map(|&(first, n)| (first as usize * bps, n as usize * bps))
                .collect();
            if spans.iter().all(|&(o, n)| o + n <= px_in) && self.channel_samples.len() > 1 {
                let mut parts: Vec<Option<Vec<u8>>> =
                    deinterleave::split_channels(&raw, stride, wu, hu, px_in, &spans)
                        .into_iter()
                        .map(Some)
                        .collect();
                let data = parts
                    .get_mut(c as usize)
                    .and_then(Option::take)
                    .unwrap_or_default();
                self.split_cache = Some((frame, parts));
                data
            } else {
                let off = first as usize * bps;
                deinterleave::gather_channel(&raw, stride, wu, hu, px_in, (off, px_out))
            }
        };
        Ok(finish_plane(data, w, h, pt, spp, bps))
    }

    /// Legacy frames: one JPEG 2000 codestream per picture plane per frame, frame-major.
    #[allow(clippy::many_single_char_names)] // w/h/c/p/t/z are the axis names
    fn read_plane_legacy(&mut self, frame: u32, c: u32, pt: PixelType) -> Result<Plane> {
        let (first, spp) = self.channel_samples[c as usize];
        let per_frame = if self.planes.is_empty() || self.channel_samples.len() != self.planes.len()
        {
            1
        } else {
            self.planes.len()
        };
        let (cs_index, take_component) = if per_frame == 1 {
            (
                frame as usize,
                (self.channel_samples.len() > 1).then_some(first),
            )
        } else {
            (frame as usize * per_frame + c as usize, None)
        };
        let (w, h) = (self.attrs.width, self.attrs.height);
        let Backend::Legacy(lf) = &self.backend else {
            unreachable!("legacy read on a chunked file")
        };
        let b = lf.codestreams().get(cs_index).map(|b| (*b).clone()).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "codestream {cs_index} (frame {frame}, channel {c}) missing (file truncated or frame not written)"
                ),
            )
        })?;
        let path = self.path.clone();
        let file_len = self.file_len();
        let f = self.handle()?;
        let (payload, len, ty) = box_header(f, &path, b.box_offset, file_len)?;
        if ty != "jp2c" {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                b.box_offset,
                format!("expected a jp2c box, found '{ty}'"),
            ));
        }
        let cs = read_at(
            f,
            &path,
            payload,
            usize::try_from(len).unwrap_or(usize::MAX),
        )?;
        match codestream_size(&cs) {
            Some((cw, ch, _)) if cw == w && ch == h => {}
            other => {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    payload,
                    format!("codestream {cs_index} declares {other:?}, the image is {w}×{h}"),
                ));
            }
        }
        let r = openreadout_codecs::jpeg2000_decode(&cs)
            .map_err(|e| Error::corrupt_at(FORMAT_ID, payload, e.to_string()))?;
        let bps_out = pt.bytes_per_sample();
        let bps_in = (r.bits_per_sample / 8) as usize;
        if bps_in > bps_out {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                payload,
                format!(
                    "codestream holds {}-bit samples, the file declares {}-bit",
                    r.bits_per_sample,
                    8 * bps_out
                ),
            ));
        }
        let nc = r.channels as usize;
        let (c0, n_out) = match take_component {
            Some(k) => (k as usize, spp as usize),
            None => (0, nc),
        };
        if c0 + n_out > nc || n_out != spp as usize {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                payload,
                format!("codestream has {nc} components, channel {c} needs {spp} from {c0}"),
            ));
        }
        let pixels = w as usize * h as usize;
        let mut data = Vec::with_capacity(pixels * n_out * bps_out);
        for px in 0..pixels {
            for k in 0..n_out {
                let s = (px * nc + c0 + k) * bps_in;
                let v: u16 = if bps_in == 2 {
                    u16::from_le_bytes([r.data[s], r.data[s + 1]])
                } else {
                    u16::from(r.data[s])
                };
                if bps_out == 2 {
                    data.extend_from_slice(&v.to_le_bytes());
                } else {
                    data.push(v as u8);
                }
            }
        }
        Ok(Plane {
            width: w,
            height: h,
            pixel_type: pt,
            samples_per_pixel: spp,
            data,
        })
    }

    /// Frames of image (XY position) `image`. The frame count is declared by the file, so
    /// it is also capped by what the file could physically hold: every frame is a chunk or
    /// box with a header of at least 16 bytes. Without the cap a few-KB file declaring 2^32
    /// frames made `frames` loop for minutes and allocate gigabytes.
    fn frames_of_image(&self, image: u32) -> Vec<u32> {
        let physical = u32::try_from(self.file_len() / 16).unwrap_or(u32::MAX);
        (0..self.addressable_frames().min(physical))
            .filter(|f| self.layout.coords(*f).0 == image)
            .collect()
    }

    fn read_named(&self, f: &mut SourceFile, name: &str) -> Option<Vec<u8>> {
        match &self.backend {
            Backend::Chunked(file) => file.read_chunk(f, name).ok(),
            Backend::Legacy(_) => None,
        }
    }
}

fn text_info_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.map(|v| unwrap_root(v).clone())
        .and_then(|v| v.as_object().cloned())
        .map(|o| {
            o.into_iter()
                .filter_map(|(k, v)| {
                    v.as_str()
                        .filter(|s| !s.is_empty())
                        .map(|s| (k, s.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Application version from `CustomDataVar|AppInfo_V1_0!` (`…/VersionString` style keys).
fn app_version(v: &Value) -> Option<String> {
    fn find(v: &Value, depth: usize) -> Option<String> {
        if depth > 4 {
            return None;
        }
        let o = v.as_object()?;
        for (k, val) in o {
            if k.to_ascii_lowercase().contains("version")
                && let Some(s) = val.as_str().filter(|s| !s.trim().is_empty())
            {
                return Some(s.trim().to_string());
            }
        }
        o.values().find_map(|c| find(c, depth + 1))
    }
    find(v, 0)
}

/// "Plan Fluor 10x Ph1 DLL" → 10.0; "Plan Fluor 20xC ELWD ADL" → 20.0 (a word that starts with
/// the number and an `x`).
fn magnification_from_name(s: &str) -> Option<f64> {
    s.split_whitespace().find_map(|w| {
        let digits = w
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(w.len());
        let (n, rest) = w.split_at(digits);
        (!n.is_empty() && rest.starts_with(['x', 'X']))
            .then(|| n.parse::<f64>().ok())
            .flatten()
            .filter(|m| *m > 0.0)
    })
}

/// The `Numerical Aperture: 1.4` (or `.45`) line of the description text (`TextInfoItem_5`).
fn na_from_description(text: &str) -> Option<f64> {
    text.lines().find_map(|l| {
        l.trim()
            .strip_prefix("Numerical Aperture:")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|na| *na > 0.0 && *na < 2.0)
    })
}

impl Nd2Dataset {
    /// Growing-file evidence (see [`Dataset::write_state`]): NIS-Elements writes the chunk map
    /// last, so a chunked file without a usable map whose chunks were recovered by scanning,
    /// with no damage before the last chunk, is an unfinished write. Complete frames are frame
    /// chunks whose payload lies inside the file and holds a full frame.
    fn growing_state(&self) -> Option<openreadout_core::live::WriteState> {
        let Backend::Chunked(file) = &self.backend else {
            return None;
        };
        if !file.rescued {
            return None;
        }
        let mut ws = openreadout_core::live::WriteState::new();
        ws.missing.push("chunk map".into());
        ws.evidence.push(
            "no chunk map at the end of the file (NIS-Elements writes it last); chunks recovered by scanning".into(),
        );
        let file_len = file.file_len;
        let mut f = self.fs.open(&self.path).ok()?;
        let compressed = matches!(self.attrs.compression, Some(0 | 1));
        let bps = self.pixel_type().map_or(2, PixelType::bytes_per_sample) as u64;
        let need = 8 + if self.attrs.row_bytes == 0 {
            u64::from(self.attrs.width) * u64::from(self.attrs.components) * bps
        } else {
            u64::from(self.attrs.row_bytes)
        } * u64::from(self.attrs.height);
        let mut frames: Vec<(u32, u64)> = file
            .map
            .iter()
            .filter_map(|e| {
                let n = e.name.strip_prefix("ImageDataSeq|")?.strip_suffix('!')?;
                Some((n.parse::<u32>().ok()?, e.chunk_offset))
            })
            .collect();
        frames.sort_by_key(|(_, off)| *off);
        let mut end_of_complete = 0u64;
        let mut unit = None;
        let mut complete_frames = Vec::new();
        for (i, (frame, off)) in frames.iter().enumerate() {
            match read_chunk_header(&mut f, &self.path, *off, file_len) {
                Ok(h)
                    if h.payload_offset.saturating_add(h.data_len) <= file_len
                        && (compressed || h.data_len >= need) =>
                {
                    complete_frames.push(*frame);
                    end_of_complete = end_of_complete.max(h.payload_offset + h.data_len);
                    unit = Some(h.payload_offset + h.data_len - off);
                }
                Ok(_) | Err(_) if i + 1 == frames.len() => {
                    ws.evidence
                        .push(format!("frame chunk {frame} is still being written"));
                }
                Ok(_) | Err(_) => {
                    // A damaged frame before the last one is not an unfinished write.
                    ws.append_consistent = false;
                    ws.evidence.push(format!(
                        "frame chunk {frame} is damaged before the end of the file"
                    ));
                }
            }
        }
        // Chunks after the last complete frame (a frame in flight, or the closing metadata
        // chunks) are the tail.
        ws.tail_bytes = file_len.saturating_sub(
            end_of_complete.max(file.map.iter().map(|e| e.chunk_offset).min().unwrap_or(0)),
        );
        ws.unit_bytes = unit;
        let nc = self.channel_samples.len().max(1) as u32;
        complete_frames.sort_unstable();
        for fr in complete_frames {
            if fr >= self.addressable_frames() {
                continue;
            }
            let (p, t, z) = self.layout.coords(fr);
            for c in 0..nc {
                ws.complete.push((p, PlaneIndex { c, z, t }));
            }
        }
        let declared: u64 = self
            .loops
            .iter()
            .map(|l| u64::from(l.count.max(1)))
            .product();
        if !self.loops.is_empty() {
            ws.expected_planes = Some(declared.saturating_mul(u64::from(nc)));
        }
        Some(ws)
    }
}

impl Dataset for Nd2Dataset {
    fn write_state(&self) -> Option<openreadout_core::live::WriteState> {
        self.growing_state()
    }
    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        crate::assurance::internal(self.attrs.compression, self.is_legacy())
    }

    fn info(&self) -> Result<FileInfo> {
        let mut images = Vec::new();
        for p in 0..self.layout.p_count {
            images.push(self.image_info(p)?);
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        let mut notes = Vec::new();
        if images.iter().any(|i| {
            i.extra.get("acquired_at_source").and_then(Value::as_str) == Some("text_local_time")
        }) {
            notes.push("acquired_at is local clock time from the date text (no valid Julian day); the file records no time zone".into());
        }
        if self.rescued() {
            notes.push("container index was rebuilt by scanning the file; run `check`".into());
        }
        if let Some(n) = &self.layout.layout_note {
            notes.push(n.clone());
        }
        if let Some(k) = self
            .loops
            .iter()
            .find(|l| matches!(l.kind, LoopKind::Other(_) | LoopKind::CustomLoop))
        {
            notes.push(format!(
                "experiment contains a {} ({} steps); it is folded into T (see extra.loops)",
                k.kind.name(),
                k.count
            ));
        }
        if self.attrs.compression == Some(1) {
            notes.push("frames are lossy-compressed (eCompression 1); metadata is readable, pixels are not decoded".into());
        }
        if self.is_legacy() {
            notes.push(
                "legacy JPEG 2000-based ND2 (NIS-Elements 2.x); frames are decoded from JPEG 2000 codestreams".into(),
            );
        }
        for (_, p) in self.problems() {
            notes.push(format!("structure: {p}"));
        }
        let version = match &self.backend {
            Backend::Chunked(f) => Some(f.version.clone()),
            Backend::Legacy(_) => Some("legacy-jp2".into()),
        };
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len(),
            format: Nd2Reader.descriptor(),
            format_version: version,
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
        for (k, v) in &self.vendor {
            m.insert(k.clone(), v.clone());
        }
        match &self.backend {
            Backend::Chunked(file) => {
                let mut f = self
                    .fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?;
                for e in &file.map {
                    if e.name.starts_with("CustomDataVar|")
                        && !m.contains_key(&e.name)
                        && let Some(v) = file
                            .read_chunk(&mut f, &e.name)
                            .ok()
                            .and_then(|r| decode_any(&r))
                    {
                        m.insert(e.name.clone(), v);
                    }
                }
                m.insert("chunks".into(), Value::Array(file.map.iter().map(|e| json!({"name": e.name, "offset": e.chunk_offset, "length": e.chunk_data_len})).collect()));
            }
            Backend::Legacy(lf) => {
                m.insert(
                    "boxes".into(),
                    Value::Array(
                        lf.boxes
                            .iter()
                            .map(|b| json!({"type": b.box_type, "tag": b.tag, "offset": b.box_offset}))
                            .collect(),
                    ),
                );
            }
        }
        Ok(Value::Object(m))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::PriorArt),
            ("images[].size_x", Source::PriorArt),
            ("images[].size_y", Source::PriorArt),
            ("images[].size_z", Source::Inferred),
            ("images[].size_c", Source::Inferred),
            ("images[].size_t", Source::Inferred),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].physical_size", Source::Inferred),
            ("images[].time_increment_s", Source::Inferred),
            ("images[].channels[]", Source::Inferred),
            ("images[].channels[].excitation_nm", Source::PriorArt),
            ("images[].channels[].emission_nm", Source::PriorArt),
            ("images[].channels[].color", Source::PriorArt),
            ("images[].channels[].acquisition_mode", Source::PriorArt),
            ("images[].objective", Source::Inferred),
            ("images[].instrument", Source::Inferred),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].extra.loops", Source::PriorArt),
            ("images[].extra.time_periods", Source::PriorArt),
            ("images[].extra.channel_settings", Source::Inferred),
            ("images[].extra.refractive_index", Source::PriorArt),
            ("images[].extra.events", Source::PriorArt),
            ("images[].extra.rois", Source::PriorArt),
            ("images[].extra.frames", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for p in 0..self.layout.p_count {
            out.push(LsEntry {
                kind: "image".into(),
                name: format!("position {p}"),
                offset: None,
                size: None,
                image: Some(p),
                details: json!({"frames": self.layout.t_count * self.layout.z_count}),
            });
        }
        match &self.backend {
            Backend::Chunked(file) => {
                for e in &file.map {
                    let kind = if e.name.starts_with("ImageDataSeq|") {
                        "frame"
                    } else if e.name.ends_with("LV!") || e.name.contains("LV|") {
                        "metadata"
                    } else if e.name.starts_with("CustomData") {
                        "custom-data"
                    } else {
                        "chunk"
                    };
                    let image = if kind == "frame" {
                        e.name
                            .trim_start_matches("ImageDataSeq|")
                            .trim_end_matches('!')
                            .parse::<u32>()
                            .ok()
                            .map(|fi| self.layout.coords(fi).0)
                    } else {
                        None
                    };
                    out.push(LsEntry {
                        kind: kind.into(),
                        name: e.name.clone(),
                        offset: Some(e.chunk_offset),
                        size: Some(e.chunk_data_len),
                        image,
                        details: Value::Null,
                    });
                }
            }
            Backend::Legacy(lf) => {
                let per_frame = self.planes.len().max(1) as u64;
                let mut seen: BTreeMap<&str, u64> = BTreeMap::new();
                for b in &lf.boxes {
                    let n = seen.entry(b.tag.as_str()).or_default();
                    let (kind, image, details) = if b.box_type == "jp2c" {
                        let frame = u32::try_from(*n / per_frame).unwrap_or(u32::MAX);
                        (
                            "frame",
                            Some(self.layout.coords(frame).0),
                            json!({"frame": frame, "plane": *n % per_frame, "codec": "jpeg2000"}),
                        )
                    } else if b.box_type == "xml " {
                        ("metadata", None, Value::Null)
                    } else {
                        ("box", None, Value::Null)
                    };
                    out.push(LsEntry {
                        kind: kind.into(),
                        name: format!("{}|{}|{}", b.box_type.trim_end(), b.tag, n),
                        offset: Some(b.box_offset),
                        size: None,
                        image,
                        details,
                    });
                    *n += 1;
                }
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        let pt = self.pixel_type()?;
        if image >= self.layout.p_count {
            return Err(Error::Usage(format!(
                "image index {image} out of range (0..{})",
                self.layout.p_count
            )));
        }
        let nc = self.channel_samples.len() as u32;
        if idx.c >= nc || idx.z >= self.layout.z_count || idx.t >= self.layout.t_count {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range (c<{nc}, z<{}, t<{})",
                idx.c, idx.z, idx.t, self.layout.z_count, self.layout.t_count
            )));
        }
        let frame = self.layout.frame_index(image, idx.t, idx.z);
        if frame >= self.attrs.frame_count {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "frame {frame} was never written: the file holds {} frames (interrupted acquisition)",
                    self.attrs.frame_count
                ),
            ));
        }
        if self.is_legacy() {
            self.read_plane_legacy(frame, idx.c, pt)
        } else {
            self.read_plane_chunked(frame, idx.c, pt)
        }
    }

    #[allow(clippy::many_single_char_names)] // w/h/c/p/t/z are the axis names
    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        let frames = self.frames_of_image(image);
        let total = frames.len() as u64;
        let take: Vec<u32> = frames
            .into_iter()
            .take(limit.unwrap_or(usize::MAX))
            .collect();
        if take.is_empty() {
            return Ok((total, Vec::new()));
        }
        let mut f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        let ne = self
            .loops
            .iter()
            .find(|l| l.kind == LoopKind::NeTimeLoop && l.periods.len() > 1);
        let period_of = |t: u32| -> Option<usize> {
            let ne = ne?;
            let mut acc = 0u32;
            ne.periods.iter().position(|p| {
                acc += p.count;
                t < acc
            })
        };
        let mut out = Vec::with_capacity(take.len());
        match &self.backend {
            Backend::Chunked(_) => {
                let times = self
                    .read_named(&mut f, "CustomData|AcqTimesCache!")
                    .map(|r| f64_values(&r))
                    .unwrap_or_default();
                let z_tag = stage_z_tag(&self.custom_tags).map(str::to_string);
                let columns: Vec<(&CustomTag, Vec<Value>)> = self
                    .custom_tags
                    .iter()
                    .filter_map(|t| {
                        self.read_named(&mut f, &format!("CustomData|{}!", t.tag_id))
                            .map(|raw| (t, tag_values(t, &raw)))
                    })
                    .collect();
                let xy = self
                    .loops
                    .iter()
                    .find(|l| l.kind == LoopKind::XyPositionLoop);
                for fr in take {
                    let (p, t, z) = self.layout.coords(fr);
                    let time = times.get(fr as usize).copied().or(if fr == 0 {
                        self.frame0.time_ms
                    } else {
                        None
                    });
                    let mut r = base_record(fr, t, z, time, self.frame0.start_jdn);
                    if let Some(k) = period_of(t) {
                        r.insert("period".into(), Value::from(k as u32));
                    }
                    let mut tags = Map::new();
                    for (tag, vals) in &columns {
                        let Some(v) = vals.get(fr as usize) else {
                            continue;
                        };
                        let field = if z_tag.as_deref() == Some(tag.tag_id.as_str()) {
                            Some("stage_z_um")
                        } else {
                            record_field(&tag.tag_id).filter(|f| *f != "stage_z_um")
                        };
                        match field {
                            Some(name) => {
                                r.insert(name.into(), v.clone());
                            }
                            None => {
                                tags.insert(tag.tag_id.clone(), v.clone());
                            }
                        }
                    }
                    if !r.contains_key("stage_x_um")
                        && let Some(q) = xy.and_then(|l| l.positions.get(p as usize))
                    {
                        r.insert("stage_x_um".into(), json!(q.pos_x));
                        r.insert("stage_y_um".into(), json!(q.pos_y));
                    }
                    if !tags.is_empty() {
                        r.insert("tags".into(), Value::Object(tags));
                    }
                    out.push(Value::Object(r));
                }
            }
            Backend::Legacy(lf) => {
                for fr in take {
                    let (_, t, z) = self.layout.coords(fr);
                    let vimd = lf.xml(&mut f, "VIMD", fr as usize);
                    let time = vimd
                        .as_ref()
                        .and_then(|v| v.get("dTimeMSec"))
                        .and_then(Value::as_f64);
                    let mut r = base_record(fr, t, z, time, self.frame0.start_jdn);
                    if let Some(k) = period_of(t) {
                        r.insert("period".into(), Value::from(k as u32));
                    }
                    if let Some(v) = &vimd {
                        legacy_record_fields(&mut r, v);
                    }
                    out.push(Value::Object(r));
                }
            }
        }
        Ok((total, out))
    }

    fn check(&mut self) -> Result<CheckReport> {
        if self.is_legacy() {
            return self.check_legacy();
        }
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("file signature chunk and version string");
        r.performed(
            "chunk map located from the trailing offset and parsed (or rebuilt by scanning)",
        );
        r.performed("every chunk in the map has a valid header with the same name at that offset");
        r.performed("every frame chunk declared by uiSequenceCount exists and holds a full frame");
        r.performed("loop-tree frame count equals uiSequenceCount");
        if self.rescued() {
            // The chunk map is the last thing written: without a usable one the file was cut
            // short (interrupted acquisition or copy), even when every frame survived.
            r.push(Finding::error(
                "truncated",
                "the chunk map that ends every ND2 is missing or unusable; the file was cut short (metadata written after the frames may be lost); chunks were recovered by scanning",
            ));
        }
        for (off, p) in self.problems() {
            let mut f = Finding::warning("structure", p.clone());
            if let Some(o) = off {
                f = f.at(*o);
            }
            r.push(f);
        }
        let Backend::Chunked(file) = &self.backend else {
            unreachable!()
        };
        let map = file.map.clone();
        let file_len = self.file_len();
        let (h, stride, comps, bps) = (
            self.attrs.height,
            self.attrs.row_bytes,
            self.attrs.components,
            self.pixel_type()
                .map_or(2, openreadout_core::PixelType::bytes_per_sample),
        );
        let need = 8
            + (if stride == 0 {
                self.attrs.width as usize * comps as usize * bps
            } else {
                stride as usize
            }) * h as usize;
        let expected_frames = self.attrs.frame_count;
        let compressed = matches!(self.attrs.compression, Some(0 | 1));
        let path = self.path.clone();
        let layout_note = self.layout.layout_note.clone();
        let f = self.handle()?;
        let mut frames_ok = 0u32;
        for e in &map {
            match read_chunk_header(f, &path, e.chunk_offset, file_len) {
                Ok(hd) => {
                    if hd.name != e.name {
                        r.push(
                            Finding::error(
                                "bad_chunk",
                                format!(
                                    "map says '{}' but chunk at offset is '{}'",
                                    e.name, hd.name
                                ),
                            )
                            .at(e.chunk_offset),
                        );
                    } else if hd.payload_offset + hd.data_len > file_len {
                        r.push(
                            Finding::error(
                                "truncated",
                                format!("chunk '{}' extends past end of file", e.name),
                            )
                            .at(e.chunk_offset),
                        );
                    } else if e.name.starts_with("ImageDataSeq|") {
                        if compressed {
                            frames_ok += 1; // compressed: size varies; decoded lazily
                        } else if (hd.data_len as usize) < need {
                            r.push(
                                Finding::error(
                                    "missing_pixels",
                                    format!(
                                        "frame chunk '{}' is {} bytes, needs {need}",
                                        e.name, hd.data_len
                                    ),
                                )
                                .at(e.chunk_offset),
                            );
                        } else {
                            frames_ok += 1;
                        }
                    }
                }
                Err(err) => r.push(
                    Finding::error("bad_chunk", format!("chunk '{}': {err}", e.name))
                        .at(e.chunk_offset),
                ),
            }
        }
        if frames_ok < expected_frames {
            r.push(Finding::error(
                "missing_planes",
                format!("{frames_ok} of {expected_frames} frames present"),
            ));
        }
        if let Some(n) = layout_note {
            r.push(Finding::warning("loop_mismatch", n));
        }
        r.push(Finding::info(
            "frames_checked",
            format!("{frames_ok} frame chunks verified"),
        ));
        Ok(r)
    }
}

impl Nd2Dataset {
    #[allow(clippy::many_single_char_names)] // w/h/c/p/t/z are the axis names
    fn check_legacy(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed(
            "JPEG 2000 signature box and the box map at the end of the file (or a box walk)",
        );
        r.performed("every box in the map has a valid header of the recorded type inside the file");
        r.performed("every XML metadata box parses");
        r.performed(
            "every frame has one codestream per picture plane, each starting with SOC+SIZ and declaring the image size",
        );
        r.performed("loop-tree frame count equals the number of per-frame metadata boxes");
        for (off, p) in self.problems() {
            let mut f = Finding::warning("structure", p.clone());
            if let Some(o) = off {
                f = f.at(*o);
            }
            r.push(f);
        }
        let Backend::Legacy(lf) = &self.backend else {
            unreachable!()
        };
        if lf.box_map_offset.is_none() {
            // The box map is the last thing written: without it the file was cut short.
            r.push(Finding::error(
                "truncated",
                "the box map that ends every legacy ND2 is missing; the file was cut short (metadata written after the frames is lost)",
            ));
        }
        let boxes = lf.boxes.clone();
        let (w, h) = (self.attrs.width, self.attrs.height);
        let per_frame = self.planes.len().max(1) as u64;
        let needed = u64::from(self.addressable_frames()) * per_frame;
        let file_len = self.file_len();
        let path = self.path.clone();
        let layout_note = self.layout.layout_note.clone();
        let f = self.handle()?;
        let mut streams_ok = 0u64;
        for b in &boxes {
            match box_header(f, &path, b.box_offset, file_len) {
                Ok((payload, len, ty)) => {
                    if ty != b.box_type {
                        r.push(
                            Finding::error(
                                "bad_box",
                                format!("map says '{}' but the box is '{ty}'", b.box_type),
                            )
                            .at(b.box_offset),
                        );
                    } else if ty == "jp2c" {
                        let head = read_at(f, &path, payload, len.min(64) as usize)?;
                        match codestream_size(&head) {
                            Some((cw, ch, _)) if cw == w && ch == h => streams_ok += 1,
                            other => r.push(
                                Finding::error(
                                    "bad_codestream",
                                    format!("codestream declares {other:?}, image is {w}×{h}"),
                                )
                                .at(b.box_offset),
                            ),
                        }
                    } else if ty == "xml " {
                        let raw = read_at(f, &path, payload, usize::try_from(len).unwrap_or(0))?;
                        if let Err(e) = crate::variant::legacy_xml_decode(&raw) {
                            r.push(
                                Finding::error(
                                    "bad_metadata",
                                    format!("XML box '{}' does not parse: {e}", b.tag),
                                )
                                .at(b.box_offset),
                            );
                        }
                    }
                }
                Err(e) => r.push(
                    Finding::error("bad_box", format!("box '{}': {e}", b.tag)).at(b.box_offset),
                ),
            }
        }
        if streams_ok < needed {
            r.push(Finding::error(
                "missing_planes",
                format!("{streams_ok} of {needed} frame codestreams present"),
            ));
        }
        if let Some(n) = layout_note {
            r.push(Finding::warning("loop_mismatch", n));
        }
        r.push(Finding::info(
            "frames_checked",
            format!("{streams_ok} codestreams verified"),
        ));
        Ok(r)
    }
}

#[cfg(test)]
mod objective_tests {
    use super::{magnification_from_name, na_from_description};

    #[test]
    fn objective_magnification_from_names() {
        assert_eq!(
            magnification_from_name("Plan Fluor 10x Ph1 DLL"),
            Some(10.0)
        );
        assert_eq!(
            magnification_from_name("Plan Fluor 20xC ELWD ADL"),
            Some(20.0)
        );
        assert_eq!(magnification_from_name("Plan Apo \u{3bb} 20x"), Some(20.0));
        assert_eq!(
            magnification_from_name("Plan Apochromat 60x WI/C"),
            Some(60.0)
        );
        assert_eq!(magnification_from_name("Ph1 DLL"), None);
        assert_eq!(magnification_from_name("x"), None);
    }

    #[test]
    fn numerical_aperture_from_description() {
        let t = "Metadata:\r\nDimensions: T'(71) x Z(9)\r\nNumerical Aperture: 1.4\r\nRefractive Index: 1.515";
        assert_eq!(na_from_description(t), Some(1.4));
        assert_eq!(na_from_description("Numerical Aperture: .45"), Some(0.45));
        assert_eq!(na_from_description("Numerical Aperture: N/A"), None);
        assert_eq!(na_from_description("Dimensions: T(3)"), None);
    }

    #[test]
    fn bgr_to_rgb_swaps_first_and_third_sample() {
        let mut b = vec![1u8, 2, 3, 4, 5, 6, 7];
        super::bgr_to_rgb(&mut b, 1);
        assert_eq!(b, [3, 2, 1, 6, 5, 4, 7]);
        // 16-bit samples move as whole samples
        let mut w = vec![0x01, 0x10, 0x02, 0x20, 0x03, 0x30];
        super::bgr_to_rgb(&mut w, 2);
        assert_eq!(w, [0x03, 0x30, 0x02, 0x20, 0x01, 0x10]);
        super::bgr_to_rgb(&mut [], 0);
    }
}
