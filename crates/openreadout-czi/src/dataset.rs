//! `Dataset` implementation for CZI: scenes, pyramid levels, plane assembly, decoding,
//! attachments, per-plane metadata, integrity checks.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use openreadout_core::limits::plane_len;
use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, MosaicInfo, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::region::{Region, ResolutionLevel, TileCache};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::time::iso8601_to_unix;
use openreadout_core::xmljson::xml_to_json;
use openreadout_core::{Error, PixelType, Plane, Result};
use rayon::prelude::*;
use serde_json::{Value, json};

use crate::attach::{
    EventRecord, MAX_INTERPRETED_ATTACHMENT, extension_for, parse_event_list, parse_time_stamps,
};
use crate::container::{
    ATTACHMENT_DATA_OFFSET, CompressionId, CziFile, DirectoryEntry, PixelTypeId,
    SEGMENT_HEADER_LEN, SegmentId, SubBlockHeader, part_path, read_at, read_subblock_header,
};
use crate::convert::conform;
use crate::mask::{ValidMask, parse_valid_mask};
use crate::xml::{ImageXml, SubBlockTags, argb_to_rgb, parse_image_xml, parse_subblock_tags};
use crate::{CziReader, FORMAT_ID};

/// Largest subblock `<METADATA>` block read for `info --view full` planes.
const MAX_SUBBLOCK_METADATA: u32 = 1 << 20;

/// Most decoded tile bytes held at once while one plane is composited in parallel.
const TILE_BATCH_BYTES: u64 = 256 << 20;

/// Decoded subblocks kept between region reads (neighbouring windows share subblocks).
const REGION_TILE_CACHE_BYTES: usize = 64 << 20;

/// Bounding box of subblocks, in the pixel coordinates of their level.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bounds {
    pub min_x: i64,
    pub min_y: i64,
    pub width: u32,
    pub height: u32,
}

/// One downsampled pyramid level of a scene (level 0, full resolution, is `Scene::level0`).
#[derive(Debug, Clone)]
pub struct Level {
    /// The level's downsampling factors (logical size / stored size of its subblocks, snapped
    /// to the power of two, else the integer, their rounding allows).
    pub scale_x: f64,
    pub scale_y: f64,
    /// Indices into `CziFile::entries` of this level's subblocks.
    pub entries: Vec<usize>,
    /// `width` × `height`: the level's size in its own pixels (the scene's level-0 size over the
    /// factors, rounded down); `min_x`/`min_y`: the scene's level-0 origin the level grid is
    /// anchored at.
    pub bounds: Bounds,
}

/// One exposed image (a scene, or the whole file when there is no S dimension).
#[derive(Debug, Clone)]
pub struct Scene {
    pub scene_index: i32,
    pub scene_name: Option<String>,
    /// Indices into `CziFile::entries` of full-resolution subblocks.
    pub level0: Vec<usize>,
    pub bounds: Bounds,
    pub size_z: u32,
    pub size_c: u32,
    pub size_t: u32,
    pub min_z: i32,
    pub min_c: i32,
    pub min_t: i32,
    pub pyramid_levels: u32,
    /// Downsampled levels, finest first (`levels[0]` is pyramid level 1).
    pub levels: Vec<Level>,
    pub pixel_type: PixelTypeId,
    /// Extents of dimensions other than X/Y/Z/C/T/S/M that exceed 1 across the whole scene.
    pub other_dims: BTreeMap<char, u32>,
    /// This image's coordinates on the dimensions other than X/Y/Z/C/T/S/M (H, I, R, V, B):
    /// a scene whose subblocks vary along them is one image per combination.
    pub extra_index: BTreeMap<char, i32>,
    /// The dimensions of `extra_index` that vary within the scene (named in the image name).
    pub extra_varying: BTreeSet<char>,
    /// Channels of the scene with no subblock at this image's `extra_index` (read as 0).
    pub absent_channels: Vec<u32>,
    pub tile_count: u32,
    /// Stored-to-logical ratio of the scene's full-resolution subblocks (1 normally; above 1
    /// for super-resolved renderings such as PALM, which are exposed at the stored size).
    pub pixel_scale: f64,
}

/// An opened CZI file.
#[derive(Debug)]
pub struct CziDataset {
    path: PathBuf,
    file: CziFile,
    xml: ImageXml,
    scenes: Vec<Scene>,
    /// Open handles per file part (0 = master).
    handles: BTreeMap<u32, SourceFile>,
    /// Where the master file and its parts are read from.
    fs: Fs,
    /// `TimeStamps` attachment values (seconds), if present and readable.
    time_stamps: Option<Vec<f64>>,
    /// `EventList` attachment records, if present and readable.
    events: Option<Vec<EventRecord>>,
    /// Decoded subblocks kept between region reads, keyed by (file part, file position).
    tiles: TileCache<(u32, u64), Decoded>,
    /// The JPEG coding processes met in the first JPEG subblock of each pixel type
    /// (`jpeg 12-bit`, `jpeg lossless`), for the assurance profile.
    jpeg_processes: BTreeSet<String>,
}

impl CziDataset {
    pub fn open(input: &Input) -> Result<Self> {
        let path = input.path();
        let file = CziFile::open_in(input.fs(), path)?;
        let xml = parse_image_xml(&file.xml).unwrap_or_default();
        let scenes = build_scenes(&file, &xml);
        let mut ds = CziDataset {
            path: path.to_path_buf(),
            file,
            xml,
            scenes,
            handles: BTreeMap::new(),
            fs: input.fs().clone(),
            time_stamps: None,
            events: None,
            tiles: TileCache::new(REGION_TILE_CACHE_BYTES),
            jpeg_processes: BTreeSet::new(),
        };
        ds.load_small_attachments();
        ds.probe_jpeg_processes();
        Ok(ds)
    }

    /// Read the frame header of the first JPEG subblock of each pixel type (its first 64 KiB
    /// at most). A stream whose header cannot be read is left to plane reads and `check`.
    fn probe_jpeg_processes(&mut self) {
        let mut seen = BTreeSet::new();
        let firsts: Vec<DirectoryEntry> = self
            .file
            .entries
            .iter()
            .filter(|e| e.compression == CompressionId::Jpeg && seen.insert(e.pixel_type.name()))
            .cloned()
            .collect();
        for e in firsts {
            let Ok(sb) = self.subblock_header(&e) else {
                continue;
            };
            let off = e
                .file_position
                .saturating_add(SEGMENT_HEADER_LEN + sb.header_len)
                .saturating_add(u64::from(sb.metadata_size));
            let Ok(head) = self.read_part(e.file_part, off, sb.data_size.min(64 << 10)) else {
                continue;
            };
            if let Ok(m) = openreadout_codecs::jpeg_markers(&head, None) {
                match (m.process, m.precision) {
                    (3, _) => {
                        self.jpeg_processes.insert("jpeg lossless".into());
                    }
                    (_, 12) => {
                        self.jpeg_processes.insert("jpeg 12-bit".into());
                    }
                    _ => {}
                }
            }
        }
    }

    /// Read and interpret the `TimeStamps` and `EventList` attachments (a few hundred bytes each).
    fn load_small_attachments(&mut self) {
        for i in 0..self.file.attachments.len() {
            let a = &self.file.attachments[i];
            let kind = a.content_file_type.to_ascii_uppercase();
            if !(kind == "CZTIMS" || kind == "CZEVL") || a.data_size > MAX_INTERPRETED_ATTACHMENT {
                continue;
            }
            let (pos, name) = (a.file_position, a.name.clone());
            let parsed = self.attachment_bytes(i).and_then(|b| {
                if kind == "CZTIMS" {
                    parse_time_stamps(&b).map(|t| self.time_stamps.get_or_insert(t).len())
                } else {
                    parse_event_list(&b).map(|e| self.events.get_or_insert(e).len())
                }
            });
            if let Err(e) = parsed {
                self.file
                    .problems
                    .push((Some(pos), format!("attachment `{name}`: {e}")));
            }
        }
    }

    fn attachment_bytes(&mut self, i: usize) -> Result<Vec<u8>> {
        let a = self.file.attachments.get(i).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "attachment index {i} out of range (0..{})",
                self.file.attachments.len()
            ))
        })?;
        self.read_part(
            a.file_part,
            a.file_position.saturating_add(ATTACHMENT_DATA_OFFSET),
            a.data_size,
        )
    }

    /// Open (and cache) the file holding `file_part`.
    fn handle(&mut self, file_part: u32) -> Result<(&mut SourceFile, PathBuf, u64)> {
        let (p, len) = self
            .file
            .part_file(file_part)
            .map(|(p, l)| (p.to_path_buf(), l))
            .ok_or_else(|| {
                // A missing following part is a missing file, not a damaged one: report it as
                // I/O "not found" on the path we looked for (exit 5), naming the part.
                let expected = self
                    .file
                    .parts
                    .iter()
                    .find(|p| p.file_part == file_part)
                    .map_or_else(|| part_path(&self.path, file_part), |p| p.path.clone());
                Error::io(
                    expected,
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "file part {file_part} of this multi-file CZI document is missing; \
                             copy it next to the master file (planes in other parts stay readable)"
                        ),
                    ),
                )
            })?;
        let f = match self.handles.entry(file_part) {
            std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(self.fs.open(&p).map_err(|e| Error::io(&p, e))?)
            }
        };
        Ok((f, p, len))
    }

    fn read_part(&mut self, file_part: u32, off: u64, len: u64) -> Result<Vec<u8>> {
        let (f, p, flen) = self.handle(file_part)?;
        if off.saturating_add(len) > flen {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                off,
                format!(
                    "a {len}-byte read at {off} runs past the end of `{}` (truncated file)",
                    p.display()
                ),
            ));
        }
        let len = usize::try_from(len)
            .map_err(|_| Error::corrupt_at(FORMAT_ID, off, "read length does not fit in memory"))?;
        read_at(f, &p, off, len)
    }

    fn subblock_header(&mut self, e: &DirectoryEntry) -> Result<SubBlockHeader> {
        let (f, p, _) = self.handle(e.file_part)?;
        Ok(read_subblock_header(f, &p, e.file_position)?.1)
    }

    fn pixel_type(pt: PixelTypeId) -> Result<(PixelType, u32)> {
        Ok(match pt {
            PixelTypeId::Gray8 => (PixelType::Uint8, 1),
            PixelTypeId::Gray16 => (PixelType::Uint16, 1),
            PixelTypeId::Gray32Float => (PixelType::Float, 1),
            PixelTypeId::Bgr24 => (PixelType::Uint8, 3),
            PixelTypeId::Bgr48 => (PixelType::Uint16, 3),
            PixelTypeId::Bgr96Float => (PixelType::Float, 3),
            PixelTypeId::Bgra32 => (PixelType::Uint8, 3),
            PixelTypeId::Gray32 => (PixelType::Int32, 1),
            PixelTypeId::Gray64 => (PixelType::Double, 1),
            other => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("pixel type {}", other.name()),
                    "Complex-valued and unknown pixel types are not decoded.",
                ));
            }
        })
    }

    fn scene(&self, image: u32) -> Result<&Scene> {
        self.scenes.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {image} out of range (0..{})",
                self.scenes.len()
            ))
        })
    }

    fn image_info(&self, index: u32, s: &Scene) -> Result<ImageInfo> {
        let (pt, spp) = Self::pixel_type(s.pixel_type)?;
        let mut info = ImageInfo::new(index, s.bounds.width, s.bounds.height, pt);
        info.samples_per_pixel = spp;
        info.name = s
            .scene_name
            .clone()
            .or_else(|| (self.scenes.len() > 1).then(|| format!("Scene {}", s.scene_index)));
        info.size_z = s.size_z;
        info.size_c = s.size_c;
        info.size_t = s.size_t;
        info.dimension_order = "XYCZT".into();
        // A super-resolved rendering's pixels are the logical pixel divided by its ratio.
        let scale = s.pixel_scale;
        info.physical_size = PhysicalSize::micrometres(
            self.xml.scaling.distance_x.map(|d| d / scale),
            self.xml.scaling.distance_y.map(|d| d / scale),
            self.xml.scaling.distance_z,
        );
        if (scale - 1.0).abs() > 1e-6 {
            info.extra.insert("rendering_scale".into(), json!(scale));
        }
        if !s.extra_varying.is_empty() {
            let coords: serde_json::Map<String, Value> = s
                .extra_varying
                .iter()
                .map(|d| {
                    (
                        d.to_string(),
                        json!(s.extra_index.get(d).copied().unwrap_or(0)),
                    )
                })
                .collect();
            info.extra
                .insert("dimension_index".into(), Value::Object(coords));
            if !s.absent_channels.is_empty() {
                info.extra
                    .insert("absent_channels".into(), json!(s.absent_channels));
            }
        }
        info.time_increment_s = self.xml.t_increment_s;
        // A corrupt directory can declare billions of channels; list at most MAX_LISTED_CHANNELS.
        info.channels = (0..s.size_c.min(MAX_LISTED_CHANNELS))
            .map(|c| {
                let x = self.xml.channels.get((s.min_c.max(0) as u32 + c) as usize);
                ChannelInfo {
                    index: c,
                    name: x.and_then(|x| x.channel_name.clone()),
                    fluorophore: x.and_then(|x| x.fluor.clone().or_else(|| x.dye_name.clone())),
                    excitation_nm: x.and_then(|x| x.excitation_nm),
                    emission_nm: x.and_then(|x| x.emission_nm),
                    emission_range_nm: x.and_then(|x| x.detection_range_nm),
                    color: x
                        .and_then(|x| x.color_argb.as_deref())
                        .and_then(argb_to_rgb),
                    // `AcquisitionMode` and `ContrastMethod` hold OME enumeration tokens:
                    // one readable label (`WideField` + `Fluorescence` → `Widefield Fluorescence`)
                    acquisition_mode: x.and_then(|x| {
                        openreadout_core::acquisition_mode::from_ome(
                            x.acquisition_mode.as_deref(),
                            x.contrast_method.as_deref(),
                        )
                    }),
                    exposure_ms: x.and_then(|x| x.exposure_ns).map(|ns| ns / 1e6),

                    ..ChannelInfo::default()
                }
            })
            .collect();
        let o = &self.xml.objective;
        if o.objective_name.is_some() || o.lens_na.is_some() || o.nominal_magnification.is_some() {
            info.objective = Some(ObjectiveInfo {
                model: o.objective_name.clone(),
                nominal_magnification: o.nominal_magnification,
                lens_na: o.lens_na,
                immersion: o.immersion.clone(),
            });
        }
        // The detectors this image's channels name (`DetectorSettings`), in channel order.
        let mut detectors: Vec<&str> = Vec::new();
        for c in 0..s.size_c.min(MAX_LISTED_CHANNELS) {
            if let Some(label) = self
                .xml
                .channels
                .get((s.min_c.max(0) as u32 + c) as usize)
                .and_then(|x| x.detector_id.as_deref())
                .and_then(|id| self.xml.detectors.get(id))
                && !detectors.contains(&label.as_str())
            {
                detectors.push(label);
            }
        }
        let detector = (!detectors.is_empty()).then(|| detectors.join(" + "));
        if self.xml.microscope_name.is_some()
            || self.xml.application_name.is_some()
            || detector.is_some()
        {
            info.instrument = Some(InstrumentInfo {
                manufacturer: Some("Carl Zeiss Microscopy".into()),
                model: self.xml.microscope_name.clone(),
                software: self.xml.application_name.clone(),
                software_version: self.xml.application_version.clone(),
                detector,
            });
        }
        info.acquired_at = self.xml.acquisition_time.clone();
        if let Some(u) = &self.xml.user_name {
            info.extra
                .insert("experimenter".into(), json!({ "user_name": u }));
        }
        if s.tile_count > 1 {
            let first = s.level0.first().and_then(|&i| self.file.entries.get(i));
            info.mosaic = Some(MosaicInfo {
                tile_count: s.tile_count,
                tile_width: first.and_then(|e| e.dim('X')).map(|d| d.size),
                tile_height: first.and_then(|e| e.dim('Y')).map(|d| d.size),
                stitched_on_read: true,
            });
        }
        info.pyramid_levels = s.pyramid_levels;
        info.resolution_levels = self.resolution_levels(s);
        let x = &mut info.extra;
        if !s.levels.is_empty() {
            x.insert(
                "pyramid".into(),
                Value::Array(
                    s.levels
                        .iter()
                        .enumerate()
                        .map(|(i, l)| {
                            json!({"level": i + 1, "size_x": l.bounds.width, "size_y": l.bounds.height,
                                   "downsample_x": l.scale_x, "downsample_y": l.scale_y, "subblocks": l.entries.len()})
                        })
                        .collect(),
                ),
            );
        }
        if !s.other_dims.is_empty() {
            x.insert(
                "other_dimensions".into(),
                Value::Array(
                    s.other_dims
                        .iter()
                        .map(|(d, n)| json!({"axis": d.to_string(), "count": n}))
                        .collect(),
                ),
            );
        }
        let comps: BTreeSet<String> = s
            .level0
            .iter()
            .filter_map(|&i| self.file.entries.get(i))
            .map(|e| e.compression.name())
            .collect();
        x.insert(
            "compression".into(),
            Value::Array(comps.into_iter().map(Value::String).collect()),
        );
        x.insert(
            "stored_pixel_type".into(),
            Value::String(s.pixel_type.name()),
        );
        x.insert(
            "origin_px".into(),
            json!({"x": s.bounds.min_x, "y": s.bounds.min_y}),
        );
        if let Some(b) = self.xml.component_bit_count {
            x.insert("component_bit_count".into(), Value::from(b));
        }
        if let Some(sx) = self.xml.scenes.get(&(s.scene_index.max(0) as u32)) {
            let mut scene = serde_json::Map::new();
            scene.insert("index".into(), json!(s.scene_index));
            if let (Some(cx), Some(cy)) = (sx.center_x_um, sx.center_y_um) {
                scene.insert("center_position_um".into(), json!({"x": cx, "y": cy}));
            }
            if let (Some(w), Some(h)) = (sx.contour_width_um, sx.contour_height_um) {
                scene.insert("contour_size_um".into(), json!({"width": w, "height": h}));
            }
            if sx.well_name.is_some() || sx.row_index.is_some() {
                scene.insert(
                    "well".into(),
                    json!({"name": sx.well_name, "id": sx.well_id, "row_index": sx.row_index, "column_index": sx.column_index}),
                );
            }
            x.insert("scene".into(), Value::Object(scene));
        }
        if let Some(ts) = &self.time_stamps {
            // Per-T times (seconds, relative clock) for the T range this image covers.
            let lo = s.min_t.max(0) as usize;
            let slice: Vec<f64> = ts
                .iter()
                .skip(lo)
                .take(s.size_t as usize)
                .copied()
                .collect();
            if !slice.is_empty() {
                x.insert("time_stamps_s".into(), json!(slice));
            }
        }
        if let Some(ev) = self.events.as_ref().filter(|e| !e.is_empty()) {
            x.insert(
                "events".into(),
                Value::Array(
                    ev.iter()
                        .map(|e| json!({"time_s": e.time_s, "kind": e.event_kind(), "description": e.description}))
                        .collect(),
                ),
            );
        }
        if let Some(e) = &self.xml.experiment {
            x.insert(
                "experiment".into(),
                json!({"version": e.experiment_version, "acquisition_blocks": e.block_count,
                       "active_setups": e.active_setups, "time_series_cycles": e.time_series_cycles,
                       "time_series_interval_s": e.time_series_interval_s}),
            );
        }
        if let Some(xpt) = &self.xml.pixel_type
            && !xpt.eq_ignore_ascii_case(&s.pixel_type.name().replace('_', ""))
        {
            x.insert("xml_pixel_type".into(), Value::String(xpt.clone()));
        }
        Ok(info.finish())
    }

    /// Level geometry for `info`: level 0 and every pyramid level, each with the stored size
    /// of its first subblock as the tile size (mosaics and pyramids only).
    fn resolution_levels(&self, s: &Scene) -> Vec<ResolutionLevel> {
        if s.levels.is_empty() && s.tile_count <= 1 {
            return Vec::new();
        }
        let tile = |idxs: &[usize]| {
            idxs.first()
                .and_then(|&i| self.file.entries.get(i))
                .map_or((0, 0), |e| {
                    (
                        e.dim('X').map_or(0, |d| d.stored_size),
                        e.dim('Y').map_or(0, |d| d.stored_size),
                    )
                })
        };
        let (w0, h0) = (s.bounds.width, s.bounds.height);
        let (tx, ty) = tile(&s.level0);
        let mut out = vec![ResolutionLevel::new(0, w0, h0, w0, h0).with_tile(tx, ty)];
        for (i, l) in s.levels.iter().enumerate() {
            let (tx, ty) = tile(&l.entries);
            let mut r = ResolutionLevel::new(i as u32 + 1, l.bounds.width, l.bounds.height, w0, h0)
                .with_tile(tx, ty);
            // The file's own factors (snapped), not the size ratio of the bounding boxes.
            r.downsample_x = l.scale_x;
            r.downsample_y = l.scale_y;
            out.push(r);
        }
        out
    }

    fn total_file_len(&self) -> u64 {
        self.file
            .parts
            .iter()
            .fold(self.file.file_len, |n, p| n.saturating_add(p.file_len))
    }

    /// Decode one subblock into samples of its declared pixel type, R,G,B order, alpha dropped.
    fn decode_entry(&mut self, entry: &DirectoryEntry) -> Result<Decoded> {
        let fetched = self.fetch_entry(entry)?;
        decode_fetched(entry, fetched)
    }

    /// The I/O half of [`Self::decode_entry`]: the subblock's compressed bytes and its mask.
    fn fetch_entry(&mut self, entry: &DirectoryEntry) -> Result<Fetched> {
        let sb = self.subblock_header(entry)?;
        let mask = self.valid_mask(entry, &sb);
        let data_off = entry
            .file_position
            .saturating_add(SEGMENT_HEADER_LEN + sb.header_len)
            .saturating_add(u64::from(sb.metadata_size));
        let sx = entry.dim('X').map_or(0, |d| d.stored_size);
        let sy = entry.dim('Y').map_or(0, |d| d.stored_size);
        let planes = ['Z', 'C', 'T']
            .iter()
            .try_fold(1u64, |n, &d| {
                n.checked_mul(entry.dim(d).map_or(1, |x| u64::from(x.size.max(1))))
            })
            .ok_or_else(|| Error::corrupt_at(FORMAT_ID, data_off, "subblock geometry overflows"))?;
        let expected = plane_len(
            FORMAT_ID,
            u64::from(sx),
            u64::from(sy),
            planes,
            u64::from(entry.pixel_type.bytes_per_pixel()),
            self.total_file_len(),
        )
        .map_err(|e| match e {
            Error::Corrupt { format, detail, .. } => Error::Corrupt {
                format,
                detail,
                offset: Some(data_off),
            },
            other => other,
        })?;
        let data = self.read_part(entry.file_part, data_off, sb.data_size)?;
        Ok(Fetched {
            expected,
            data,
            data_off,
            mask,
        })
    }
}

/// A subblock read from the file but not decoded yet.
struct Fetched {
    expected: usize,
    data: Vec<u8>,
    /// File offset of the data (for error messages).
    data_off: u64,
    mask: Option<ValidMask>,
}

/// The CPU half of [`CziDataset::decode_entry`]: decode fetched subblock bytes. Free of `self`,
/// so the tiles of one plane can be decoded on several threads.
fn decode_fetched(entry: &DirectoryEntry, fetched: Fetched) -> Result<Decoded> {
    let Fetched {
        data,
        data_off,
        mask,
        expected,
    } = fetched;
    let sx = entry.dim('X').map_or(0, |d| d.stored_size);
    let sy = entry.dim('Y').map_or(0, |d| d.stored_size);
    let pt = entry.pixel_type;
    let codec_err =
        |e: openreadout_codecs::CodecError| Error::corrupt_at(FORMAT_ID, data_off, e.to_string());
    let raw: Vec<u8> = match entry.compression {
        CompressionId::Uncompressed => {
            if data.len() < expected {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    data_off,
                    format!(
                        "subblock holds {} bytes, geometry needs {expected}",
                        data.len()
                    ),
                ));
            }
            data
        }
        CompressionId::Zstd0 => {
            openreadout_codecs::zstd_decode(&data, expected).map_err(codec_err)?
        }
        CompressionId::Zstd1 => {
            openreadout_codecs::zstd1_decode(&data, expected, pt.bytes_per_sample() as usize)
                .map_err(codec_err)?
        }
        CompressionId::Lzw => openreadout_codecs::lzw_decode(&data, expected).map_err(codec_err)?,
        CompressionId::Chunked => {
            openreadout_codecs::chunked_decode(&data, expected, pt.bytes_per_sample() as usize)
                .map_err(|e| match e {
                    openreadout_codecs::CodecError::Unsupported { detail, .. } => {
                        Error::unsupported(
                            FORMAT_ID,
                            format!("chunked subblock: {detail}"),
                            "Chunked subblocks (id 7) are decoded with zstd or LZ4 chunks and the optional HiLo split; this one uses something else.",
                        )
                    }
                    other => codec_err(other),
                })?
        }
        CompressionId::JpegXr | CompressionId::Jpeg => {
            let mut r = if entry.compression == CompressionId::Jpeg {
                openreadout_codecs::jpeg_decode_limited(
                    &data,
                    expected.saturating_mul(4).max(1 << 20),
                )
                .map_err(|e| jpeg_error(&e, data_off))?
            } else {
                openreadout_codecs::jpegxr_decode(&data).map_err(|e| match e {
                    openreadout_codecs::CodecError::Unsupported { detail, .. } => {
                        Error::unsupported(
                            FORMAT_ID,
                            format!("JPEG XR subblock: {detail}"),
                            "Grey, RGB and N-channel JPEG XR (8/16-bit, float) are decoded; alpha, CMYK and packed formats are not.",
                        )
                    }
                    other => codec_err(other),
                })?
            };
            if r.bgr && r.channels >= 3 {
                let bps = (r.bits_per_sample as usize).div_ceil(8);
                swap_rb(&mut r.data, bps, r.channels as usize);
                r.bgr = false;
            }
            // Resolution protocol: the directory entry's pixel type and size win.
            let c = conform(r, pt, sx, sy)?;
            return Ok(Decoded {
                data: c.data,
                width: sx,
                height: sy,
                adjustments: c.adjustments,
                mask,
            });
        }
        other => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("subblock compression {}", other.name()),
                "Only uncompressed, zstd0, zstd1 (with HiLo), chunked (id 7), LZW, JPEG and JPEG XR subblocks are decoded.",
            ));
        }
    };
    let out = match pt {
        PixelTypeId::Bgr24 | PixelTypeId::Bgr48 | PixelTypeId::Bgr96Float => {
            let mut raw = raw;
            swap_rb(&mut raw, pt.bytes_per_sample() as usize, 3);
            raw
        }
        PixelTypeId::Bgra32 => drop_alpha(&raw),
        _ => raw,
    };
    Ok(Decoded {
        data: out,
        width: sx,
        height: sy,
        adjustments: Vec::new(),
        mask,
    })
}

impl CziDataset {
    /// Plane `idx` of level `level`, or only `region` of it.
    fn read_level(
        &mut self,
        image: u32,
        idx: PlaneIndex,
        level: u32,
        region: Option<Region>,
    ) -> Result<Plane> {
        let s = self.scene(image)?.clone();
        Self::check_plane_index(&s, image, idx)?;
        if level == 0 {
            let entries: Vec<DirectoryEntry> = s
                .level0
                .iter()
                .map(|&i| self.file.entries[i].clone())
                .collect();
            let ratio = s.pixel_scale;
            let (mx, my) = (s.bounds.min_x, s.bounds.min_y);
            let place = move |e: &DirectoryEntry| {
                (
                    stored_start(e.dim('X').map_or(0, |d| d.start), ratio) - mx,
                    stored_start(e.dim('Y').map_or(0, |d| d.start), ratio) - my,
                )
            };
            return self.composite(
                &s,
                idx,
                entries,
                (s.bounds.width, s.bounds.height),
                &place,
                image,
                region,
            );
        }
        let l = s.levels.get(level as usize - 1).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "pyramid level {level} out of range for image {image} (0..{})",
                s.pyramid_levels
            ))
        })?;
        let entries: Vec<DirectoryEntry> = l
            .entries
            .iter()
            .map(|&i| self.file.entries[i].clone())
            .collect();
        let (fx, fy, x0, y0) = (l.scale_x, l.scale_y, l.bounds.min_x, l.bounds.min_y);
        let place = move |e: &DirectoryEntry| level_place(e, fx, fy, x0, y0);
        self.composite(
            &s,
            idx,
            entries,
            (l.bounds.width, l.bounds.height),
            &place,
            image,
            region,
        )
    }

    /// The subblock's valid-pixel mask, when its attachment carries one that is not all-valid.
    /// A malformed or unreadable mask is ignored (every pixel is then treated as valid).
    fn valid_mask(&mut self, e: &DirectoryEntry, sb: &SubBlockHeader) -> Option<ValidMask> {
        if sb.attachment_size < 32 || sb.attachment_size > MAX_SUBBLOCK_METADATA {
            return None;
        }
        let off = e
            .file_position
            .saturating_add(SEGMENT_HEADER_LEN + sb.header_len)
            .saturating_add(u64::from(sb.metadata_size))
            .saturating_add(sb.data_size);
        let att = self
            .read_part(e.file_part, off, u64::from(sb.attachment_size))
            .ok()?;
        parse_valid_mask(&att).filter(|m| !m.all_valid())
    }

    /// Composite the wanted plane from `entries`, placing each at `place(entry)` (top-left
    /// corner in plane pixels), or only the `region` of it (`None` = the whole `width` ×
    /// `height` plane). A region read decodes only the subblocks it overlaps and keeps them in
    /// the tile cache for the next read.
    fn composite(
        &mut self,
        s: &Scene,
        idx: PlaneIndex,
        mut entries: Vec<DirectoryEntry>,
        (width, height): (u32, u32),
        place: &dyn Fn(&DirectoryEntry) -> (i64, i64),
        image: u32,
        region: Option<Region>,
    ) -> Result<Plane> {
        let (pt, spp) = Self::pixel_type(s.pixel_type)?;
        let bpp = spp as usize * pt.bytes_per_sample();
        let want = |min: i32, i: u32| i32::try_from(i64::from(min) + i64::from(i)).ok();
        let (Some(want_z), Some(want_c), Some(want_t)) = (
            want(s.min_z, idx.z),
            want(s.min_c, idx.c),
            want(s.min_t, idx.t),
        ) else {
            return Err(Error::corrupt(
                FORMAT_ID,
                "plane index overflows the directory's coordinates",
            ));
        };
        entries.retain(|e| {
            e.covers('Z', want_z)
                && e.covers('C', want_c)
                && e.covers('T', want_t)
                && e.dimensions.iter().all(|d| {
                    "XYZCTSM".contains(d.dimension)
                        || d.start == s.extra_index.get(&d.dimension).copied().unwrap_or(0)
                })
        });
        // Overlapping mosaic tiles: later tiles overwrite earlier ones, so paste in tile (M)
        // order regardless of directory order to make the result deterministic.
        entries.sort_by_key(|e| (e.index('M'), e.file_position));
        // A channel absent at this image's H/I/R/V/B coordinates reads as 0 (`absent_channels`).
        if entries.is_empty() && !s.absent_channels.contains(&idx.c) {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "no subblock for image {image} c={} z={} t={}",
                    idx.c, idx.z, idx.t
                ),
            ));
        }
        let window = match region {
            Some(r) => {
                r.check_within(width, height, &format!("image {image}"))?;
                // Only the subblocks the window overlaps (their stored extent, placed).
                entries.retain(|e| {
                    let (tx, ty) = place(e);
                    let sx = e.dim('X').map_or(0, |d| d.stored_size);
                    let sy = e.dim('Y').map_or(0, |d| d.stored_size);
                    r.overlap(tx, ty, u64::from(sx), u64::from(sy)).is_some()
                });
                r
            }
            None => Region::full(width, height),
        };
        let cache = region.is_some();
        let (ox, oy) = (i64::from(window.x), i64::from(window.y));
        let place_in = move |e: &DirectoryEntry| {
            let (x, y) = place(e);
            (x - ox, y - oy)
        };
        // A stitched whole-slide scene can be tens of GB; refuse it before allocating.
        let total = if cache {
            openreadout_core::pixel::plane_bytes_checked(
                FORMAT_ID,
                window.width,
                window.height,
                bpp,
            )?
        } else {
            plane_len(
                FORMAT_ID,
                u64::from(width),
                u64::from(height),
                u64::from(spp),
                pt.bytes_per_sample() as u64,
                self.total_file_len(),
            )?
        };
        let (width, height) = (window.width, window.height);
        let mut plane = vec![0u8; total];
        // Tiles are read in order, decoded in batches on the `--threads` pool and pasted in
        // order, so the plane does not depend on the thread count; errors surface in the order
        // a sequential walk would meet them. A batch holds at most one decoded tile per thread
        // and at most `TILE_BATCH_BYTES` of them (never fewer than one tile).
        let tile_bytes = entries
            .iter()
            .map(|e| {
                let side = |d: char| e.dim(d).map_or(1, |x| u64::from(x.stored_size.max(1)));
                ['X', 'Y', 'Z', 'C', 'T'].iter().fold(bpp as u64, |n, &d| {
                    let count = if matches!(d, 'X' | 'Y') {
                        side(d)
                    } else {
                        e.dim(d).map_or(1, |x| u64::from(x.size.max(1)))
                    };
                    n.saturating_mul(count)
                })
            })
            .max()
            .unwrap_or(1);
        let batch = rayon::current_num_threads()
            .min(usize::try_from(TILE_BATCH_BYTES / tile_bytes.max(1)).unwrap_or(usize::MAX))
            .max(1);
        for chunk in entries.chunks(batch) {
            // Cached subblocks (region reads) are reused; the rest are read in order.
            let fetched: Vec<Result<Got>> = chunk
                .iter()
                .map(|e| {
                    if e.pixel_type != s.pixel_type {
                        return Err(Error::unsupported(
                            FORMAT_ID,
                            "mixed pixel types within one scene",
                            "Subblocks of one scene use different pixel types; not handled yet.",
                        ));
                    }
                    if cache && let Some(d) = self.tiles.get(&(e.file_part, e.file_position)) {
                        return Ok(Got::Cached(d));
                    }
                    self.fetch_entry(e).map(Got::Read)
                })
                .collect();
            let decode = |(e, f): (&DirectoryEntry, Result<Got>)| {
                f.and_then(|f| match f {
                    Got::Cached(d) => Ok((d, true)),
                    Got::Read(f) => decode_fetched(e, f).map(|d| (Arc::new(d), false)),
                })
            };
            let decoded: Vec<Result<(Arc<Decoded>, bool)>> = if chunk.len() > 1 {
                chunk.par_iter().zip(fetched).map(decode).collect()
            } else {
                chunk.iter().zip(fetched).map(decode).collect()
            };
            for (e, d) in chunk.iter().zip(decoded) {
                let (d, was_cached) = d?;
                Self::paste_decoded(
                    &mut plane,
                    e,
                    &d,
                    (want_z, want_c, want_t),
                    bpp,
                    width,
                    height,
                    &place_in,
                )?;
                if cache && !was_cached {
                    let n = d.data.len();
                    self.tiles.put((e.file_part, e.file_position), d, n);
                }
            }
        }
        Ok(Plane {
            width,
            height,
            pixel_type: pt,
            samples_per_pixel: spp,
            data: plane,
        })
    }

    /// Copy the wanted plane of a decoded subblock into `plane` at the subblock's place.
    fn paste_decoded(
        plane: &mut [u8],
        e: &DirectoryEntry,
        d: &Decoded,
        (want_z, want_c, want_t): (i32, i32, i32),
        bpp: usize,
        width: u32,
        height: u32,
        place: &dyn Fn(&DirectoryEntry) -> (i64, i64),
    ) -> Result<()> {
        let Decoded {
            data: block,
            width: tw,
            height: th,
            mask,
            ..
        } = d;
        let (tw, th) = (*tw, *th);
        // A subblock may hold several planes along one of Z/C/T (line scans store a whole
        // stack in one block). Locate the wanted plane inside it.
        let plane_bytes = (tw as usize)
            .saturating_mul(th as usize)
            .saturating_mul(bpp);
        let multi: Vec<(char, i32)> = [('Z', want_z), ('C', want_c), ('T', want_t)]
            .into_iter()
            .filter(|(d, _)| e.dim(*d).is_some_and(|x| x.size > 1))
            .collect();
        let offset = match multi.as_slice() {
            [] => 0,
            [(d, want)] => usize::try_from(i64::from(*want) - i64::from(e.index(*d)))
                .ok()
                .and_then(|k| k.checked_mul(plane_bytes))
                .unwrap_or(usize::MAX),
            _ => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    "subblocks spanning more than one of Z, C and T",
                    "This subblock packs a multi-dimensional stack; only single-axis stacks are unpacked.",
                ));
            }
        };
        let Some(tile) = offset
            .checked_add(plane_bytes)
            .and_then(|end| block.get(offset..end))
        else {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                e.file_position,
                "subblock is smaller than the planes it declares",
            ));
        };
        let (tx, ty) = place(e);
        paste_tile(
            plane,
            width,
            height,
            tile,
            tx,
            ty,
            tw,
            th,
            bpp,
            mask.as_ref(),
        );
        Ok(())
    }

    fn check_plane_index(s: &Scene, image: u32, idx: PlaneIndex) -> Result<()> {
        if idx.c >= s.size_c || idx.z >= s.size_z || idx.t >= s.size_t {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image} (c<{}, z<{}, t<{})",
                idx.c, idx.z, idx.t, s.size_c, s.size_z, s.size_t
            )));
        }
        Ok(())
    }
}

fn jpeg_error(e: &openreadout_codecs::CodecError, off: u64) -> Error {
    let msg = e.to_string();
    if msg.contains("not supported") || msg.contains("unsupported") || msg.contains("Unsupported") {
        Error::unsupported(
            FORMAT_ID,
            format!("JPEG subblock: {msg}"),
            "8-bit baseline/progressive, 12-bit sequential and lossless (up to 16-bit) JPEG are decoded; 12-bit progressive or chroma-subsampled and CMYK JPEG are not.",
        )
    } else {
        Error::corrupt_at(FORMAT_ID, off, msg)
    }
}

/// Swap the first and third sample of every pixel in place (B, G, R to R, G, B).
fn swap_rb(data: &mut [u8], bps: usize, channels: usize) {
    match (bps, channels) {
        // The common cases get fixed-size chunks, so the loop has no bounds checks.
        (1, 3) => data
            .as_chunks_mut::<3>()
            .0
            .iter_mut()
            .for_each(|p| p.swap(0, 2)),
        (2, 3) => data.as_chunks_mut::<6>().0.iter_mut().for_each(|p| {
            p.swap(0, 4);
            p.swap(1, 5);
        }),
        _ => {
            for p in data.chunks_exact_mut(channels * bps) {
                for k in 0..bps {
                    p.swap(k, 2 * bps + k);
                }
            }
        }
    }
}

fn drop_alpha(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() / 4 * 3);
    for p in raw.as_chunks::<4>().0 {
        out.extend_from_slice(&[p[2], p[1], p[0]]);
    }
    out
}

/// A subblock taken from the tile cache, or read from the file and still to be decoded.
enum Got {
    Cached(Arc<Decoded>),
    Read(Fetched),
}

/// One decoded subblock plane stack.
#[derive(Debug)]
struct Decoded {
    data: Vec<u8>,
    width: u32,
    height: u32,
    /// Resolution-protocol adjustments applied (empty when the bitmap matched its entry).
    adjustments: Vec<String>,
    mask: Option<ValidMask>,
}

/// Copy a decoded tile into the plane buffer at `(tx, ty)`, clipping to the plane. With a
/// valid-pixel mask, only valid pixels are copied (the rest keep what is underneath).
fn paste_tile(
    plane: &mut [u8],
    pw: u32,
    ph: u32,
    tile: &[u8],
    tx: i64,
    ty: i64,
    tw: u32,
    th: u32,
    bpp: usize,
    mask: Option<&ValidMask>,
) {
    if let Some(m) = mask {
        for y in 0..th {
            let py = ty + i64::from(y);
            if py < 0 || py >= i64::from(ph) {
                continue;
            }
            for x in 0..tw {
                let px = tx + i64::from(x);
                if px < 0 || px >= i64::from(pw) || !m.is_valid(x, y) {
                    continue;
                }
                let src = (y as usize * tw as usize + x as usize) * bpp;
                let dst = (py as usize * pw as usize + px as usize) * bpp;
                if let (Some(s), Some(d)) =
                    (tile.get(src..src + bpp), plane.get_mut(dst..dst + bpp))
                {
                    d.copy_from_slice(s);
                }
            }
        }
        return;
    }
    let row = tw as usize * bpp;
    let prow = pw as usize * bpp;
    // clip horizontally
    let x0 = tx.max(0);
    let x1 = (tx + i64::from(tw)).min(i64::from(pw));
    if x1 <= x0 {
        return;
    }
    let skip = ((x0 - tx) as usize) * bpp;
    let len = ((x1 - x0) as usize) * bpp;
    for y in 0..th as i64 {
        let py = ty + y;
        if py < 0 || py >= i64::from(ph) {
            continue;
        }
        let src = y as usize * row + skip;
        let dst = py as usize * prow + x0 as usize * bpp;
        if let (Some(s), Some(d)) = (tile.get(src..src + len), plane.get_mut(dst..dst + len)) {
            d.copy_from_slice(s);
        }
    }
}

/// Rounding slack of a pyramid subblock's stored size, in stored pixels: ZEN builds each level
/// from the one below, so `stored_size` is `size / f` give or take this much.
const STORED_SIZE_SLACK: f64 = 2.0;

/// One axis of a pyramid subblock's downsampling: `(ratio, lo, hi)`, its raw ratio
/// `size / stored_size` and the interval `[lo, hi]` the level's true factor lies in.
type AxisScale = (f64, f64, f64);

/// The downsampling factors a pyramid subblock is consistent with, per axis (X, Y): its raw
/// ratio `size / stored_size` and the interval `[size / (stored + slack), size / (stored -
/// slack)]` the level's true factor lies in. `None` for a subblock without X/Y extents.
fn level_scale_range(e: &DirectoryEntry) -> Option<[AxisScale; 2]> {
    let axis = |d: char| {
        let x = e.dim(d)?;
        if x.stored_size == 0 || x.size == 0 {
            return None;
        }
        let (size, stored) = (f64::from(x.size), f64::from(x.stored_size));
        let hi = if stored > STORED_SIZE_SLACK {
            size / (stored - STORED_SIZE_SLACK)
        } else {
            f64::INFINITY
        };
        Some((size / stored, size / (stored + STORED_SIZE_SLACK), hi))
    };
    Some([axis('X')?, axis('Y')?])
}

/// A level's factor on one axis from a subblock's `(ratio, lo, hi)`: the power of two inside
/// `[lo, hi]` nearest the ratio, else the nearest integer inside it, else the ratio itself.
fn snap_factor((ratio, lo, hi): AxisScale) -> f64 {
    let near = |a: f64, b: f64| {
        (a.ln() - ratio.ln())
            .abs()
            .total_cmp(&(b.ln() - ratio.ln()).abs())
    };
    let pow2 = (0..=40)
        .map(|k| 2f64.powi(k))
        .filter(|&p| p >= lo && p <= hi)
        .min_by(|&a, &b| near(a, b));
    if let Some(p) = pow2 {
        return p;
    }
    let (a, b) = (ratio.floor(), ratio.ceil());
    [a, b]
        .into_iter()
        .filter(|&n| n >= 1.0 && n >= lo && n <= hi)
        .min_by(|&x, &y| near(x, y))
        .unwrap_or(ratio)
}

/// Stored-pixel position of a pyramid subblock in a level with factors `fx`, `fy` whose grid
/// is anchored at the scene's level-0 origin `(x0, y0)`: `floor((start - origin) / f)`, the
/// placement pylibCZIrw's scaled reads show.
fn level_place(e: &DirectoryEntry, fx: f64, fy: f64, x0: i64, y0: i64) -> (i64, i64) {
    let p = |d: char, f: f64, o: i64| {
        let start = i64::from(e.dim(d).map_or(0, |x| x.start));
        let off = start.saturating_sub(o);
        if f >= 1.0 && f.fract() == 0.0 && f < 9.0e15 {
            off.div_euclid(f as i64)
        } else {
            (off as f64 / f).floor() as i64
        }
    };
    (p('X', fx, x0), p('Y', fy, y0))
}

/// Group a scene's pyramid subblocks into levels (see `docs/formats/czi.md`, "Pyramid
/// levels"). Levels are formed from the largest subblocks first; each subblock joins the
/// level whose factors lie inside its own rounding interval (the nearest, when several do)
/// or starts a new one. A level covers the scene's level-0 rectangle `bounds` divided by its
/// factors (rounded down), finest level first.
fn build_levels(all: &[DirectoryEntry], idxs: &[usize], bounds: Bounds) -> Vec<Level> {
    let mut cands: Vec<(usize, [AxisScale; 2], u64)> = idxs
        .iter()
        .filter_map(|&i| {
            let e = all.get(i)?;
            if e.pyramid_type == 0 {
                return None;
            }
            let r = level_scale_range(e)?;
            // Not downsampled on either axis: not a pyramid subblock after all.
            if r[0].0 <= 1.0 + 1e-9 && r[1].0 <= 1.0 + 1e-9 {
                return None;
            }
            let area = e.dim('X').map_or(0, |d| u64::from(d.stored_size))
                * e.dim('Y').map_or(0, |d| u64::from(d.stored_size));
            Some((i, r, area))
        })
        .collect();
    cands.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    let mut groups: Vec<(f64, f64, Vec<usize>)> = Vec::new();
    for (i, [x, y], _) in cands {
        let fits = |f: f64, (_, lo, hi): AxisScale| f >= lo && f <= hi;
        let dist = |fx: f64, fy: f64| (fx.ln() - x.0.ln()).abs() + (fy.ln() - y.0.ln()).abs();
        let best = groups
            .iter_mut()
            .filter(|g| fits(g.0, x) && fits(g.1, y))
            .min_by(|a, b| dist(a.0, a.1).total_cmp(&dist(b.0, b.1)));
        match best {
            Some(g) => g.2.push(i),
            None => groups.push((snap_factor(x), snap_factor(y), vec![i])),
        }
    }
    groups.sort_by(|a, b| (a.0 * a.1).total_cmp(&(b.0 * b.1)));
    groups
        .into_iter()
        .map(|(fx, fy, mut entries)| {
            entries.sort_unstable();
            let size = |w: u32, f: f64| ((f64::from(w) / f).floor() as u32).max(1);
            Level {
                scale_x: fx,
                scale_y: fy,
                entries,
                bounds: Bounds {
                    min_x: bounds.min_x,
                    min_y: bounds.min_y,
                    width: size(bounds.width, fx),
                    height: size(bounds.height, fy),
                },
            }
        })
        .collect()
}

/// Most channels `info` lists for one image (a corrupt directory can declare billions).
const MAX_LISTED_CHANNELS: u32 = 1 << 16;

fn build_scenes(file: &CziFile, xml: &ImageXml) -> Vec<Scene> {
    let mut by_scene: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (i, e) in file.entries.iter().enumerate() {
        by_scene.entry(e.index('S')).or_default().push(i);
    }
    let multi_scene = by_scene.len() > 1;
    let mut scenes = Vec::new();
    for (scene_index, all_idxs) in by_scene {
        let level0_all: Vec<usize> = all_idxs
            .iter()
            .copied()
            .filter(|&i| file.entries[i].is_level0())
            .collect();
        // Full-resolution subblocks stored at different ratios to their logical extent (a
        // super-resolved PALM rendering next to a widefield channel) cannot share one pixel
        // grid: one image per ratio, each with the channels (and pyramid subblocks) of its own.
        // Subblocks at different coordinates on H, I, R, V or B (SIM phases, light-sheet views
        // and illuminations) are different images: one per combination of those coordinates.
        let mut groups: BTreeMap<(ExtraKey, i64), Vec<usize>> = BTreeMap::new();
        for &i in &level0_all {
            let key = (file.entries[i].stored_ratio() * 1e6).round() as i64;
            groups
                .entry((extra_key(&file.entries[i]), key))
                .or_default()
                .push(i);
        }
        let ratios: BTreeSet<i64> = groups.keys().map(|(_, r)| *r).collect();
        let mut scene_other: BTreeMap<char, u32> = BTreeMap::new();
        {
            let mut ranges: BTreeMap<char, (i32, i32)> = BTreeMap::new();
            for d in level0_all
                .iter()
                .flat_map(|&i| file.entries[i].dimensions.iter())
                .filter(|d| !"XYZCTSM".contains(d.dimension))
            {
                let en = ranges.entry(d.dimension).or_insert((i32::MAX, i32::MIN));
                en.0 = en.0.min(d.start);
                en.1 = en.1.max(
                    d.start
                        .saturating_add((d.size.min(i32::MAX as u32) as i32) - 1),
                );
            }
            for (d, (lo, hi)) in ranges {
                if hi > lo {
                    scene_other.insert(
                        d,
                        (i64::from(hi) - i64::from(lo) + 1).clamp(1, i64::from(u32::MAX)) as u32,
                    );
                }
            }
        }
        let split = ratios.len() > 1;
        let extra_keys: BTreeSet<&ExtraKey> = groups.keys().map(|(k, _)| k).collect();
        let mut extra_varying = BTreeSet::new();
        for k in &extra_keys {
            for (d, v) in *k {
                if extra_keys
                    .iter()
                    .any(|o| o.iter().find(|(od, _)| od == d).map_or(0, |(_, ov)| *ov) != *v)
                {
                    extra_varying.insert(*d);
                }
            }
        }
        for ((extra, key), level0) in groups {
            let ratio = key as f64 / 1e6;
            let channels: BTreeSet<i32> =
                level0.iter().map(|&i| file.entries[i].index('C')).collect();
            let extra_index: BTreeMap<char, i32> = extra.iter().copied().collect();
            let same_extra = |i: usize| {
                extra_varying
                    .iter()
                    .all(|d| file.entries[i].index(*d) == extra_index.get(d).copied().unwrap_or(0))
            };
            let idxs: Vec<usize> = all_idxs
                .iter()
                .copied()
                .filter(|&i| !split || channels.contains(&file.entries[i].index('C')))
                .filter(|&i| same_extra(i))
                .collect();
            let refs: Vec<&DirectoryEntry> = level0.iter().map(|&i| &file.entries[i]).collect();
            if refs.is_empty() {
                continue;
            }
            let (mut min_x, mut min_y, mut max_x, mut max_y) =
                (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
            let (mut min_z, mut min_c, mut min_t) = (i32::MAX, i32::MAX, i32::MAX);
            let (mut max_z, mut max_c, mut max_t) = (i32::MIN, i32::MIN, i32::MIN);
            let mut tiles = BTreeSet::new();
            for e in &refs {
                if let Some(x) = e.dim('X') {
                    let x0 = stored_start(x.start, ratio);
                    min_x = min_x.min(x0);
                    max_x = max_x.max(x0.saturating_add(i64::from(x.stored_size)));
                }
                if let Some(y) = e.dim('Y') {
                    let y0 = stored_start(y.start, ratio);
                    min_y = min_y.min(y0);
                    max_y = max_y.max(y0.saturating_add(i64::from(y.stored_size)));
                }
                for (d, lo, hi) in [
                    ('Z', &mut min_z, &mut max_z),
                    ('C', &mut min_c, &mut max_c),
                    ('T', &mut min_t, &mut max_t),
                ] {
                    let v = e.index(d);
                    let n = e.dim(d).map_or(1, |x| x.size.max(1)).min(i32::MAX as u32) as i32;
                    *lo = (*lo).min(v);
                    *hi = (*hi).max(v.saturating_add(n - 1));
                }
                tiles.insert(e.index('M'));
            }
            // Images split along H/I/R/V/B share the scene's Z/C/T extents (an Airyscan or
            // multi-track acquisition stores some channels at H = 0 only); channels without a
            // subblock at this image's coordinates are listed and read as 0, as czifile does.
            let mut absent_channels = Vec::new();
            if !extra_varying.is_empty() {
                let present: BTreeSet<i32> = refs.iter().map(|e| e.index('C')).collect();
                for &i in &level0_all {
                    let e = &file.entries[i];
                    if (e.stored_ratio() * 1e6).round() as i64 != key {
                        continue;
                    }
                    for (d, lo, hi) in [
                        ('Z', &mut min_z, &mut max_z),
                        ('C', &mut min_c, &mut max_c),
                        ('T', &mut min_t, &mut max_t),
                    ] {
                        let v = e.index(d);
                        let n = e.dim(d).map_or(1, |x| x.size.max(1)).min(i32::MAX as u32) as i32;
                        *lo = (*lo).min(v);
                        *hi = (*hi).max(v.saturating_add(n - 1));
                    }
                }
                absent_channels = (min_c..=max_c)
                    .filter(|c| !present.contains(c))
                    .filter_map(|c| u32::try_from(i64::from(c) - i64::from(min_c)).ok())
                    .take(MAX_LISTED_CHANNELS as usize)
                    .collect();
            }
            let bounds = Bounds {
                min_x,
                min_y,
                width: max_x.saturating_sub(min_x).clamp(0, i64::from(u32::MAX)) as u32,
                height: max_y.saturating_sub(min_y).clamp(0, i64::from(u32::MAX)) as u32,
            };
            let levels = build_levels(&file.entries, &idxs, bounds);
            let span = |lo: i32, hi: i32| {
                (i64::from(hi) - i64::from(lo) + 1).clamp(1, i64::from(u32::MAX)) as u32
            };
            let mut base_name = xml.scene_names.get(&(scene_index.max(0) as u32)).cloned();
            if !extra_varying.is_empty() {
                let coords: Vec<String> = extra_varying
                    .iter()
                    .map(|d| format!("{d}={}", extra_index.get(d).copied().unwrap_or(0)))
                    .collect();
                let coords = coords.join(" ");
                base_name = Some(match (base_name, multi_scene) {
                    (Some(b), _) => format!("{b} {coords}"),
                    (None, true) => format!("Scene {scene_index} {coords}"),
                    (None, false) => coords,
                });
            }
            scenes.push(Scene {
                scene_index,
                scene_name: if split && (ratio - 1.0).abs() > 1e-6 {
                    Some(format!(
                        "{} (rendered at {ratio:.4}x)",
                        base_name
                            .clone()
                            .unwrap_or_else(|| format!("Scene {scene_index}"))
                    ))
                } else {
                    base_name
                },
                pixel_type: refs[0].pixel_type,
                level0,
                bounds,
                size_z: span(min_z, max_z),
                size_c: span(min_c, max_c),
                size_t: span(min_t, max_t),
                min_z,
                min_c,
                min_t,
                pyramid_levels: 1 + levels.len() as u32,
                levels,
                other_dims: scene_other.clone(),
                extra_index,
                extra_varying: extra_varying.clone(),
                absent_channels,
                tile_count: tiles.len() as u32,
                pixel_scale: Some(refs[0].stored_ratio())
                    .filter(|r| *r > 0.0)
                    .unwrap_or(1.0),
            });
        }
    }
    scenes
}

/// Coordinates on the dimensions other than X/Y/Z/C/T/S/M, sorted by dimension letter.
type ExtraKey = Vec<(char, i32)>;

/// A subblock's coordinates on the dimensions other than X/Y/Z/C/T/S/M, by dimension letter.
fn extra_key(e: &DirectoryEntry) -> ExtraKey {
    let mut k: ExtraKey = e
        .dimensions
        .iter()
        .filter(|d| !"XYZCTSM".contains(d.dimension))
        .map(|d| (d.dimension, d.start))
        .collect();
    k.sort_unstable();
    k.dedup_by_key(|(d, _)| *d);
    k
}

/// Where a subblock starting at logical coordinate `start` begins in the stored-size grid of
/// a scene rendered at `ratio` (stored / logical): `round(start × ratio)`.
fn stored_start(start: i32, ratio: f64) -> i64 {
    if (ratio - 1.0).abs() < 1e-9 {
        i64::from(start)
    } else {
        (f64::from(start) * ratio).round() as i64
    }
}

impl CziDataset {
    /// Growing-file evidence (see [`Dataset::write_state`]). A file whose header does not yet
    /// point at a subblock directory (position 0: not finalized) and whose segment chain is
    /// intact up to a last segment that may run past the end is an unfinished write. A header
    /// that points past the end of the file is a finished file cut short (a truncated copy),
    /// not a growing one.
    fn growing_state(&self) -> Option<openreadout_core::live::WriteState> {
        let hdr = &self.file.header;
        if hdr.directory_position != 0 {
            return None;
        }
        let mut ws = openreadout_core::live::WriteState::new();
        ws.missing.push("subblock directory".into());
        ws.evidence.push(
            "the file header does not point at a subblock directory yet (written when the acquisition is finalized); subblocks found by walking the segments".into(),
        );
        if hdr.metadata_position == 0 {
            ws.missing.push("metadata".into());
        }
        if hdr.attachment_directory_position == 0 {
            ws.missing.push("attachment directory".into());
        }
        for (_, p) in &self.file.problems {
            let benign = p.contains("extends past end of file")
                || p.contains("recovered")
                || p.contains("no usable subblock directory");
            if !benign {
                ws.append_consistent = false;
                ws.evidence.push(format!("damage before the end: {p}"));
            }
        }
        let file_len = self.file.file_len;
        if let Some((off, last)) = self.file.segments.last() {
            let end =
                (off + crate::container::SEGMENT_HEADER_LEN).saturating_add(last.allocated_size);
            ws.tail_bytes = if end > file_len {
                file_len - off
            } else {
                file_len - end
            };
        }
        let mut sizes: Vec<u64> = self
            .file
            .segments
            .iter()
            .filter(|(_, s)| s.segment_id == crate::container::SegmentId::SubBlock)
            .map(|(_, s)| s.allocated_size + crate::container::SEGMENT_HEADER_LEN)
            .collect();
        sizes.sort_unstable();
        ws.unit_bytes = sizes.get(sizes.len() / 2).copied();
        let mut seen = std::collections::HashSet::new();
        let mut order: Vec<(u64, u32, PlaneIndex)> = Vec::new();
        for (image, sc) in self.scenes.iter().enumerate() {
            for &i in &sc.level0 {
                let Some(entry) = self.file.entries.get(i) else {
                    continue;
                };
                let rel = |d: char, min: i32| u32::try_from(entry.index(d) - min).ok();
                let (Some(c), Some(z), Some(t)) =
                    (rel('C', sc.min_c), rel('Z', sc.min_z), rel('T', sc.min_t))
                else {
                    continue;
                };
                let idx = PlaneIndex { c, z, t };
                if seen.insert((image as u32, idx)) {
                    order.push((entry.file_position, image as u32, idx));
                }
            }
        }
        order.sort_by_key(|(pos, _, _)| *pos);
        ws.complete = order.into_iter().map(|(_, i, p)| (i, p)).collect();
        if self.scenes.iter().any(|s| s.tile_count > 1) {
            ws.evidence
                .push("mosaic: a plane counts as complete once one of its tiles is written".into());
        }
        Some(ws)
    }
}

impl Dataset for CziDataset {
    fn write_state(&self) -> Option<openreadout_core::live::WriteState> {
        self.growing_state()
    }
    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        crate::assurance::internal(
            self.scenes
                .iter()
                .flat_map(|s| s.level0.iter().filter_map(|&i| self.file.entries.get(i))),
            &self.jpeg_processes,
        )
    }

    fn member_files(&self) -> Vec<std::path::PathBuf> {
        self.file
            .parts
            .iter()
            .filter(|p| p.present)
            .map(|p| p.path.clone())
            .collect()
    }
    fn info(&self) -> Result<FileInfo> {
        let mut images = Vec::with_capacity(self.scenes.len());
        for (i, s) in self.scenes.iter().enumerate() {
            images.push(self.image_info(i as u32, s)?);
        }
        let plane_count = images
            .iter()
            .fold(0u64, |n, i| n.saturating_add(i.plane_count));
        let mut notes = Vec::new();
        if self.scenes.iter().any(|s| s.pyramid_levels > 1) {
            notes.push("pyramid levels are readable with `planes --level N`; export writes full resolution only".into());
        }
        if self.scenes.iter().any(|s| !s.other_dims.is_empty()) {
            notes.push("extra dimensions (H/I/R/V/B) vary: one image per combination of their coordinates (`images[].extra.dimension_index`)".into());
        }
        if !self.file.parts.is_empty() {
            let present = self.file.parts.iter().filter(|p| p.present).count();
            notes.push(format!(
                "multi-file document: {} following part(s) referenced, {present} found next to this file",
                self.file.parts.len()
            ));
        }
        if !self.file.attachments.is_empty() {
            notes.push(format!(
                "{} attachment(s): `info --view structure` lists them, `extract FILE NAME` writes one out",
                self.file.attachments.len()
            ));
        }
        for (_, p) in &self.file.problems {
            notes.push(format!("structure: {p}"));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file.file_len,
            format: CziReader.descriptor(),
            format_version: Some(format!(
                "{}.{}",
                self.file.header.version_major, self.file.header.version_minor
            )),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        if self.file.xml.is_empty() {
            return Ok(Value::Null);
        }
        xml_to_json(&self.file.xml)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "metadata XML does not parse"))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::PriorArt),
            ("images[].size_x", Source::Inferred),
            ("images[].size_y", Source::Inferred),
            ("images[].size_z", Source::PriorArt),
            ("images[].size_c", Source::PriorArt),
            ("images[].size_t", Source::PriorArt),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].samples_per_pixel", Source::PriorArt),
            ("images[].physical_size", Source::VendorImpl),
            ("images[].channels[]", Source::VendorImpl),
            ("images[].objective", Source::VendorImpl),
            ("images[].instrument", Source::VendorImpl),
            ("images[].acquired_at", Source::VendorImpl),
            ("images[].mosaic", Source::Inferred),
            ("images[].pyramid_levels", Source::PriorArt),
            ("images[].extra.pyramid", Source::PriorArt),
            ("images[].extra.compression", Source::PriorArt),
            ("images[].extra.scene", Source::Inferred),
            ("images[].extra.time_stamps_s", Source::PriorArt),
            ("images[].extra.events", Source::PriorArt),
            ("images[].extra.experiment", Source::Inferred),
            ("images[].extra.experimenter", Source::Inferred),
            ("images[].extra.frames[]", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (off, s) in &self.file.segments {
            let kind = match s.segment_id {
                // subblocks are listed per directory entry, attachments per attachment entry
                SegmentId::SubBlock | SegmentId::Attachment => continue,
                SegmentId::File => "file-header",
                SegmentId::Directory => "subblock-directory",
                SegmentId::Metadata => "metadata",
                SegmentId::AttachmentDirectory => "attachment-directory",
                SegmentId::Deleted => "deleted",
                SegmentId::Unknown => "unknown",
            };
            out.push(LsEntry {
                kind: kind.into(),
                name: s.segment_id.name().to_string(),
                offset: Some(*off),
                size: Some(s.used_size),
                image: None,
                details: json!({"allocated_size": s.allocated_size}),
            });
        }
        for p in &self.file.parts {
            out.push(LsEntry {
                kind: "file-part".into(),
                name: p.path.display().to_string(),
                offset: None,
                size: p.present.then_some(p.file_len),
                image: None,
                details: json!({"file_part": p.file_part, "present": p.present, "problem": p.part_problem}),
            });
        }
        for a in self.attachments()? {
            let e = &self.file.attachments[a.index as usize];
            let mut details = json!({"index": a.index, "content_type": a.content_type,
                "extract": format!("openreadout extract FILE {}", if a.name.is_empty() { format!("#{}", a.index) } else { a.name.clone() }),
                "file_part": e.file_part});
            for (k, v) in a.extra {
                details[k] = v;
            }
            out.push(LsEntry {
                kind: "attachment".into(),
                name: format!("{} ({})", a.name, a.content_type),
                offset: Some(e.file_position),
                size: Some(a.size),
                image: None,
                details,
            });
        }
        for (i, s) in self.scenes.iter().enumerate() {
            out.push(LsEntry {
                kind: "image".into(),
                name: s.scene_name.clone().unwrap_or_else(|| format!("scene {}", s.scene_index)),
                offset: None,
                size: None,
                image: Some(i as u32),
                details: json!({"scene_index": s.scene_index, "level0_subblocks": s.level0.len(), "tiles": s.tile_count, "pyramid_levels": s.pyramid_levels, "origin_px": {"x": s.bounds.min_x, "y": s.bounds.min_y}, "stored_pixel_type": s.pixel_type.name()}),
            });
            for (li, l) in s.levels.iter().enumerate() {
                out.push(LsEntry {
                    kind: "pyramid-level".into(),
                    name: format!("level {} ({}x{})", li + 1, l.bounds.width, l.bounds.height),
                    offset: None,
                    size: None,
                    image: Some(i as u32),
                    details: json!({"level": li + 1, "downsample_x": l.scale_x, "downsample_y": l.scale_y, "subblocks": l.entries.len(), "size_x": l.bounds.width, "size_y": l.bounds.height}),
                });
            }
        }
        for e in &self.file.entries {
            let scene = self
                .scenes
                .iter()
                .position(|s| s.scene_index == e.index('S'));
            let dims: serde_json::Map<String, Value> = e
                .dimensions
                .iter()
                .map(|d| {
                    (
                        d.dimension.to_string(),
                        json!({"start": d.start, "size": d.size, "stored": d.stored_size}),
                    )
                })
                .collect();
            out.push(LsEntry {
                kind: if e.is_level0() { "subblock".into() } else { "pyramid-subblock".into() },
                name: format!("{} {}", e.pixel_type.name(), e.dimensions.iter().filter(|d| "ZCTSM".contains(d.dimension)).map(|d| format!("{}{}", d.dimension, d.start)).collect::<Vec<_>>().join(" ")),
                offset: Some(e.file_position),
                size: None,
                image: scene.map(|i| i as u32),
                details: json!({"compression": e.compression.name(), "scale": e.scale(), "pyramid_type": e.pyramid_type, "file_part": e.file_part, "dims": dims}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        self.read_level(image, idx, 0, None)
    }

    fn read_plane_level(&mut self, image: u32, idx: PlaneIndex, level: u32) -> Result<Plane> {
        self.read_level(image, idx, level, None)
    }

    fn read_region(
        &mut self,
        image: u32,
        idx: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        self.read_level(image, idx, level, Some(region))
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        Ok(self
            .file
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let mut extra = BTreeMap::new();
                extra.insert(
                    "content_guid".into(),
                    Value::String(a.content_guid.iter().fold(String::new(), |mut acc, b| {
                        let _ = write!(acc, "{b:02x}");
                        acc
                    })),
                );
                if a.file_part > 0 {
                    extra.insert("file_part".into(), json!(a.file_part));
                }
                match a.content_file_type.to_ascii_uppercase().as_str() {
                    "CZTIMS" => {
                        if let Some(t) = &self.time_stamps {
                            extra.insert("time_stamp_count".into(), json!(t.len()));
                        }
                    }
                    "CZEVL" => {
                        if let Some(e) = &self.events {
                            extra.insert("event_count".into(), json!(e.len()));
                        }
                    }
                    _ => {}
                }
                AttachmentInfo {
                    index: i as u32,
                    name: a.name.clone(),
                    content_type: a.content_file_type.clone(),
                    extension: extension_for(&a.content_file_type).into(),
                    offset: Some(a.file_position.saturating_add(ATTACHMENT_DATA_OFFSET)),
                    size: a.data_size,
                    extra,
                }
            })
            .collect())
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        self.attachment_bytes(index as usize)
    }

    /// One record per plane of image `image`, in acquisition-like order (t, then z, then c):
    /// `{frame, c, z, t, acquired_at, time_ms, stage_x_um, stage_y_um, stage_z_um, exposure_ms,
    /// time_stamp_s}` (absent values omitted). Reads one subblock header + metadata per plane.
    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        let s = self.scene(image)?;
        let total = u64::from(s.size_c)
            .saturating_mul(u64::from(s.size_z))
            .saturating_mul(u64::from(s.size_t));
        // Without a limit, at most MAX_ITEM_COUNT records (a corrupt directory can declare
        // billions of planes).
        let want = limit.unwrap_or(openreadout_core::limits::MAX_ITEM_COUNT as usize);
        let mut handles: BTreeMap<u32, SourceFile> = BTreeMap::new();
        let mut cache: BTreeMap<(u32, u64), SubBlockTags> = BTreeMap::new();
        let mut rows: Vec<FrameRow> = Vec::new();
        'outer: for t in 0..s.size_t {
            for z in 0..s.size_z {
                for c in 0..s.size_c {
                    if rows.len() >= want {
                        break 'outer;
                    }
                    let at =
                        |min: i32, i: u32| min.saturating_add(i32::try_from(i).unwrap_or(i32::MAX));
                    let (wz, wc, wt) = (at(s.min_z, z), at(s.min_c, c), at(s.min_t, t));
                    let entry = s
                        .level0
                        .iter()
                        .map(|&i| &self.file.entries[i])
                        .filter(|e| e.covers('Z', wz) && e.covers('C', wc) && e.covers('T', wt))
                        .min_by_key(|e| (e.index('M'), e.file_position));
                    let tags = match entry {
                        Some(e) => cache
                            .entry((e.file_part, e.file_position))
                            .or_insert_with(|| self.read_tags(&mut handles, e).unwrap_or_default())
                            .clone(),
                        None => SubBlockTags::default(),
                    };
                    let secs = tags.acquisition_time.as_deref().and_then(iso8601_to_unix);
                    let stamp = self
                        .time_stamps
                        .as_ref()
                        .and_then(|ts| ts.get(usize::try_from(wt).ok()?).copied());
                    let exposure_ms = tags.exposure_ns.map(|ns| ns / 1e6).or_else(|| {
                        self.xml
                            .channels
                            .get(wc.max(0) as usize)
                            .and_then(|x| x.exposure_ns)
                            .map(|ns| ns / 1e6)
                    });
                    let mut rec = serde_json::Map::new();
                    rec.insert("frame".into(), json!(rows.len()));
                    rec.insert("c".into(), json!(c));
                    rec.insert("z".into(), json!(z));
                    rec.insert("t".into(), json!(t));
                    if let Some(v) = &tags.acquisition_time {
                        rec.insert("acquired_at".into(), json!(v));
                    }
                    for (k, v) in [
                        ("stage_x_um", tags.stage_x_um),
                        ("stage_y_um", tags.stage_y_um),
                        ("stage_z_um", tags.focus_um),
                        ("exposure_ms", exposure_ms),
                        ("time_stamp_s", stamp),
                    ] {
                        if let Some(v) = v {
                            rec.insert(k.into(), json!(v));
                        }
                    }
                    rows.push((secs, stamp, rec));
                }
            }
        }
        // time_ms: from subblock acquisition times when every record has one, else from the
        // TimeStamps attachment; relative to the earliest record of this image.
        // A NaN or infinite stored time is no time (ZEN writes NaN for frames it did not stamp).
        let rows: Vec<_> = rows
            .into_iter()
            .map(|(secs, stamp, rec)| {
                (
                    secs.filter(|v: &f64| v.is_finite()),
                    stamp.filter(|v: &f64| v.is_finite()),
                    rec,
                )
            })
            .collect();
        let all_times = rows.iter().all(|r| r.0.is_some());
        let first_time = rows.iter().filter_map(|r| r.0).reduce(f64::min);
        let first_stamp = rows.iter().filter_map(|r| r.1).reduce(f64::min);
        let out = rows
            .into_iter()
            .map(|(secs, stamp, mut rec)| {
                let rel = if all_times {
                    secs.zip(first_time).map(|(v, f)| v - f)
                } else {
                    stamp.zip(first_stamp).map(|(v, f)| v - f)
                };
                if let Some(d) = rel {
                    rec.insert("time_ms".into(), json!(round_us(d * 1e3)));
                }
                Value::Object(rec)
            })
            .collect();
        Ok((total, out))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed(
            "segment chain: ids known, allocated/used sizes consistent, chain ends inside the file",
        );
        r.performed("file header positions land on metadata, subblock-directory and attachment-directory segments");
        r.performed(
            "subblock directory parses; every entry's subblock header agrees with the directory",
        );
        r.performed(
            "every subblock's data fits inside its segment; uncompressed sizes match geometry",
        );
        r.performed("each scene has a subblock for every (c, z, t) it declares");
        r.performed("attachment entries point at attachment segments whose data fits the file; TimeStamps/EventList parse");
        if !self.file.parts.is_empty() {
            r.performed("multi-file document: every referenced file part is present next to the master, says it is that part, and names the master's GUID");
        }
        for (off, p) in &self.file.problems {
            let code = if p.contains("is missing: expected") {
                "missing_part"
            } else if p.contains("primary_file_guid") || p.contains("says it is file part") {
                "bad_part"
            } else if p.contains("truncated") {
                "truncated"
            } else if p.contains("update_pending") {
                "unfinished_write"
            } else {
                "structure"
            };
            let mut f = Finding::error(code, p.clone());
            if let Some(o) = off {
                f = f.at(*o);
            }
            if code == "unfinished_write" {
                f.severity = openreadout_core::model::Severity::Warning;
            }
            r.push(f);
        }
        // Per-subblock verification (headers only; no decoding).
        let entries = self.file.entries.clone();
        let mut checked = 0usize;
        for (i, e) in entries.iter().enumerate() {
            let Some((_, part_len)) = self.file.part_file(e.file_part) else {
                continue; // reported once as missing_part
            };
            if e.file_position.saturating_add(SEGMENT_HEADER_LEN) > part_len {
                r.push(
                    Finding::error(
                        "bad_offset",
                        format!(
                            "directory entry {i} points past end of file part {}",
                            e.file_part
                        ),
                    )
                    .at(e.file_position),
                );
                continue;
            }
            let res = self
                .handle(e.file_part)
                .and_then(|(f, p, _)| read_subblock_header(f, &p, e.file_position));
            match res {
                Ok((sh, sb)) => {
                    let need = sb
                        .header_len
                        .saturating_add(u64::from(sb.metadata_size))
                        .saturating_add(sb.data_size)
                        .saturating_add(u64::from(sb.attachment_size));
                    if need > sh.used_size {
                        r.push(Finding::error("subblock_overflow", format!("subblock {i}: header+metadata+data+attachment ({need}) exceeds used_size {}", sh.used_size)).at(e.file_position));
                    }
                    if e.file_position
                        .saturating_add(SEGMENT_HEADER_LEN)
                        .saturating_add(need)
                        > part_len
                    {
                        r.push(
                            Finding::error(
                                "truncated",
                                format!("subblock {i} data extends past end of file"),
                            )
                            .at(e.file_position),
                        );
                    }
                    if sb.entry.dimensions != e.dimensions
                        || sb.entry.pixel_type != e.pixel_type
                        || sb.entry.compression != e.compression
                    {
                        r.push(
                            Finding::warning(
                                "directory_mismatch",
                                format!("subblock {i}: in-file entry differs from the directory (the directory is used)"),
                            )
                            .at(e.file_position),
                        );
                    }
                    if e.compression == CompressionId::Uncompressed {
                        let sx = e.dim('X').map_or(0, |d| d.stored_size) as u64;
                        let sy = e.dim('Y').map_or(0, |d| d.stored_size) as u64;
                        let expect = sx
                            .saturating_mul(sy)
                            .saturating_mul(u64::from(e.pixel_type.bytes_per_pixel()));
                        if sb.data_size < expect {
                            r.push(
                                Finding::error(
                                    "missing_pixels",
                                    format!(
                                        "subblock {i}: {} bytes stored, geometry needs {expect}",
                                        sb.data_size
                                    ),
                                )
                                .at(e.file_position),
                            );
                        }
                    }
                    checked += 1;
                }
                Err(err) => r.push(
                    Finding::error("bad_subblock", format!("subblock {i}: {err}"))
                        .at(e.file_position),
                ),
            }
        }
        for (si, s) in self.scenes.iter().enumerate() {
            // Enumerating (z, c, t) is linear in the planes the subblocks declare; a corrupt
            // directory can declare billions, so refuse to enumerate beyond MAX_ITEM_COUNT.
            let declared = s
                .level0
                .iter()
                .filter_map(|&i| entries.get(i))
                .map(|e| {
                    ['Z', 'C', 'T']
                        .iter()
                        .map(|&d| e.dim(d).map_or(1, |x| u64::from(x.size.max(1))))
                        .fold(1u64, u64::saturating_mul)
                })
                .fold(0u64, u64::saturating_add);
            if declared > openreadout_core::limits::MAX_ITEM_COUNT {
                r.push(Finding::error(
                    "structure",
                    format!(
                        "image {si}: subblocks declare {declared} (z,c,t) planes; not enumerated"
                    ),
                ));
                continue;
            }
            let have: BTreeSet<(i32, i32, i32)> = s
                .level0
                .iter()
                .filter_map(|&i| entries.get(i))
                .flat_map(|e| {
                    let span = |d: char| {
                        let lo = e.index(d);
                        let n = e.dim(d).map_or(1, |x| x.size.max(1)).min(i32::MAX as u32) as i32;
                        lo..lo.saturating_add(n)
                    };
                    let (zs, cs, ts) = (span('Z'), span('C'), span('T'));
                    zs.flat_map(move |z| {
                        let ts = ts.clone();
                        cs.clone()
                            .flat_map(move |c| ts.clone().map(move |t| (z, c, t)))
                    })
                })
                .collect();
            // Channels the scene stores only at other H/I/R/V/B coordinates read as 0 here
            // (`absent_channels`); they are not missing planes.
            let present_c = u64::from(s.size_c).saturating_sub(s.absent_channels.len() as u64);
            let expect = u64::from(s.size_z)
                .saturating_mul(present_c)
                .saturating_mul(u64::from(s.size_t));
            if (have.len() as u64) < expect {
                r.push(Finding::error(
                    "missing_planes",
                    format!(
                        "image {si}: {} of {expect} (z,c,t) planes have subblocks",
                        have.len()
                    ),
                ));
            }
        }
        // Resolution protocol: decode the first JPEG / JPEG XR subblock of each
        // (compression, pixel type) pair and report when the bitmap disagrees with the directory.
        let mut sampled = BTreeSet::new();
        for e in &entries {
            if !matches!(e.compression, CompressionId::Jpeg | CompressionId::JpegXr)
                || self.file.part_file(e.file_part).is_none()
                || !sampled.insert((e.compression.name(), e.pixel_type.name()))
            {
                continue;
            }
            match self.decode_entry(e) {
                Ok(Decoded {
                    adjustments: adj, ..
                }) if !adj.is_empty() => r.push(
                    Finding::warning(
                        "resolution_protocol",
                        format!(
                            "{} subblock: {}; read as the directory declares",
                            e.compression.name(),
                            adj.join("; ")
                        ),
                    )
                    .at(e.file_position),
                ),
                Ok(_) => {}
                Err(Error::Unsupported { feature, .. }) => {
                    r.push(Finding::warning("unsupported_subblock", feature).at(e.file_position));
                }
                Err(err) => r.push(
                    Finding::error("bad_subblock", format!("decoding failed: {err}"))
                        .at(e.file_position),
                ),
            }
        }
        if !sampled.is_empty() {
            r.performed("one JPEG / JPEG XR subblock per (compression, pixel type) decoded and compared with its directory entry (resolution protocol)");
        }
        r.push(Finding::info(
            "subblocks_checked",
            format!("{checked} subblock headers verified"),
        ));
        if !self.file.attachments.is_empty() {
            r.push(Finding::info(
                "attachments_checked",
                format!(
                    "{} attachment entries verified",
                    self.file.attachments.len()
                ),
            ));
        }
        Ok(r)
    }
}

impl CziDataset {
    /// Tags of one subblock's `<METADATA>`, reading through `handles` (opened on demand).
    fn read_tags(
        &self,
        handles: &mut BTreeMap<u32, SourceFile>,
        e: &DirectoryEntry,
    ) -> Result<SubBlockTags> {
        let (path, len) = self
            .file
            .part_file(e.file_part)
            .map(|(p, l)| (p.to_path_buf(), l))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "subblock lives in a missing file part"))?;
        let f = match handles.entry(e.file_part) {
            std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(self.fs.open(&path).map_err(|err| Error::io(&path, err))?)
            }
        };
        let (_, sb) = read_subblock_header(f, &path, e.file_position)?;
        if sb.metadata_size == 0 || sb.metadata_size > MAX_SUBBLOCK_METADATA {
            return Ok(SubBlockTags::default());
        }
        let off = e
            .file_position
            .saturating_add(SEGMENT_HEADER_LEN + sb.header_len);
        if off.saturating_add(u64::from(sb.metadata_size)) > len {
            return Ok(SubBlockTags::default());
        }
        let b = read_at(f, &path, off, sb.metadata_size as usize)?;
        Ok(parse_subblock_tags(
            String::from_utf8_lossy(&b).trim_end_matches('\0'),
        ))
    }
}

/// A frame record under construction: acquisition time (Unix s), time stamp (s), fields.
type FrameRow = (Option<f64>, Option<f64>, serde_json::Map<String, Value>);

/// Round to the microsecond so sums of f64 seconds print cleanly.
fn round_us(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::DimensionEntry;

    fn entry(x: (i32, u32, u32), y: (i32, u32, u32), pyramid_type: u8) -> DirectoryEntry {
        let d = |c: char, (start, size, stored_size): (i32, u32, u32)| DimensionEntry {
            dimension: c,
            start,
            size,
            start_coordinate: 0.0,
            stored_size,
        };
        DirectoryEntry {
            pixel_type: PixelTypeId::Gray8,
            file_position: 0,
            file_part: 0,
            compression: CompressionId::Uncompressed,
            pyramid_type,
            dimensions: vec![d('X', x), d('Y', y)],
        }
    }

    #[test]
    fn edge_tiles_snap_to_the_level_factor() {
        let snap = |e: &DirectoryEntry| {
            let [x, y] = level_scale_range(e).unwrap();
            (snap_factor(x), snap_factor(y))
        };
        // 197 / 99 = 1.99 on the X edge, exactly 2 on Y
        assert_eq!(snap(&entry((0, 197, 99), (0, 2048, 1024), 2)), (2.0, 2.0));
        assert_eq!(
            snap(&entry((0, 4096, 1024), (0, 4096, 1024), 2)),
            (4.0, 4.0)
        );
        // Coarse levels built level by level: 1024 stored pixels over 131 234 is 1/128.
        assert_eq!(
            snap(&entry((0, 131_234, 1024), (0, 59_157, 462), 2)),
            (128.0, 128.0)
        );
        // A 3x pyramid keeps its factor.
        assert_eq!(
            snap(&entry((0, 3072, 1024), (0, 3072, 1024), 2)),
            (3.0, 3.0)
        );
        // Placement: floor((start - origin) / f), on both sides of the origin.
        assert_eq!(
            level_place(&entry((3, 2, 1), (5, 2, 1), 2), 2.0, 2.0, 0, 0),
            (1, 2)
        );
        assert_eq!(
            level_place(
                &entry((-67_458, 2, 1), (22_464, 2, 1), 2),
                2.0,
                8.0,
                -67_457,
                22_459
            ),
            (-1, 0)
        );
    }

    #[test]
    fn coarse_single_tile_levels_join_their_power_of_two_level() {
        // The coarse end of zenodo10577621-Young-mouse: the 1/64 level is five tiles (one of
        // them 64.062 x 64.035), 1/128 two (128.222 x 128 and 128.158 x 128.069), 1/256 one.
        let entries = vec![
            entry((-214_560, 65_536, 1024), (17_216, 65_536, 1024), 2),
            entry((-149_024, 59_207, 925), (17_216, 57_882, 905), 2),
            entry((-214_560, 131_299, 1024), (17_280, 69_120, 540), 2),
            entry((-83_261, 59_209, 462), (24_832, 58_399, 456), 2),
            entry((-214_560, 190_538, 743), (17_216, 69_132, 270), 2),
            entry((-214_560, 464, 7), (17_216, 1024, 16), 2),
        ];
        let bounds = Bounds {
            min_x: -214_443,
            min_y: 17_274,
            width: 190_309,
            height: 69_378,
        };
        let levels = build_levels(&entries, &[0, 1, 2, 3, 4, 5], bounds);
        let got: Vec<(f64, u32, u32, usize)> = levels
            .iter()
            .map(|l| (l.scale_x, l.bounds.width, l.bounds.height, l.entries.len()))
            .collect();
        assert_eq!(
            got,
            vec![
                (64.0, 2973, 1084, 3),
                (128.0, 1486, 542, 2),
                (256.0, 743, 271, 1)
            ]
        );
    }

    #[test]
    fn paste_clips_to_the_plane() {
        let mut plane = vec![0u8; 4 * 3];
        let tile = [1u8, 2, 3, 4, 5, 6];
        paste_tile(&mut plane, 4, 3, &tile, -1, 2, 3, 2, 1, None);
        assert_eq!(plane, [0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 0, 0]);
        paste_tile(&mut plane, 4, 3, &tile, 10, 0, 3, 2, 1, None);
        assert_eq!(plane[0], 0);
        // with a mask only valid pixels land: row 0 = 101, row 1 = 010
        let m = ValidMask {
            mask_width: 3,
            mask_height: 2,
            stride: 1,
            bits: vec![0b1010_0000, 0b0100_0000],
        };
        let mut plane = vec![9u8; 3 * 2];
        paste_tile(&mut plane, 3, 2, &tile, 0, 0, 3, 2, 1, Some(&m));
        assert_eq!(plane, [1, 9, 3, 9, 5, 9]);
    }
}
