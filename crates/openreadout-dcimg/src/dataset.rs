//! `Dataset` implementation for DCIMG: one image whose frames are time points.
//! See `docs/formats/dcimg.md`.

use openreadout_core::source::SourceFile as File;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_block;
use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo, LsEntry,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use crate::layout::{DcimgLayout, FrameLayout, FrameStamp};
use crate::{DcimgReader, FORMAT_ID};

/// Frames whose counters `check` compares in one read.
const STAMP_BATCH: u32 = 4096;

/// An opened DCIMG file.
#[derive(Debug)]
pub struct DcimgDataset {
    path: PathBuf,
    file: File,
    layout: DcimgLayout,
    /// Counter and time stamp of the first and last frame (read at open for `info`).
    first_stamp: Option<FrameStamp>,
    last_stamp: Option<FrameStamp>,
}

impl DcimgDataset {
    /// Open and parse the headers (and, for version 7, the footer).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&openreadout_core::source::Input::local(path))
    }

    /// Open an [`openreadout_core::source::Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &openreadout_core::source::Input) -> Result<Self> {
        let path = input.path();
        let mut file = input.open()?;
        let layout = DcimgLayout::read(&mut file, path)?;
        let first_stamp = layout.stamps(&mut file, path, 0, 1)?.first().copied();
        let last_stamp = if layout.frame_count > 1 {
            layout
                .stamps(&mut file, path, layout.frame_count - 1, 1)?
                .first()
                .copied()
        } else {
            first_stamp
        };
        Ok(DcimgDataset {
            path: path.to_path_buf(),
            file,
            layout,
            first_stamp,
            last_stamp,
        })
    }

    /// The parsed file structure.
    pub fn layout(&self) -> &DcimgLayout {
        &self.layout
    }

    fn pixel_type(&self) -> PixelType {
        if self.layout.bytes_per_pixel == 1 {
            PixelType::Uint8
        } else {
            PixelType::Uint16
        }
    }

    fn image_info(&self) -> ImageInfo {
        let l = &self.layout;
        let mut info = ImageInfo::new(0, l.width, l.height, self.pixel_type());
        info.size_t = l.frame_count.max(1);
        info.name = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string());
        info.channels = vec![ChannelInfo {
            index: 0,
            ..ChannelInfo::default()
        }];
        info.acquired_at = self.first_stamp.map(|s| s.iso8601());
        if let (Some(a), Some(b)) = (self.first_stamp, self.last_stamp)
            && l.frame_count > 1
        {
            let dt = (b.unix_seconds() - a.unix_seconds()) / f64::from(l.frame_count - 1);
            if dt.is_finite() && dt > 0.0 {
                info.time_increment_s = Some(dt);
            }
        }
        info.instrument = Some(InstrumentInfo {
            manufacturer: Some("Hamamatsu".into()),
            model: l.camera.model.clone(),
            software: None,
            software_version: None,
            detector: l.camera.model.clone(),
        });
        let mut extra = Map::new();
        extra.insert("dcimg_version".into(), json!(l.version));
        extra.insert(
            "frame_layout".into(),
            json!(match l.layout {
                FrameLayout::Packed => "packed",
                _ => "framed",
            }),
        );
        if let Some(s) = &l.camera.serial {
            extra.insert("camera_serial".into(), json!(s));
        }
        if !l.camera.versions.is_empty() {
            extra.insert("camera_versions".into(), json!(l.camera.versions));
        }
        if let Some([x, w, y, h]) = l.sub_array {
            extra.insert(
                "sensor_sub_array".into(),
                json!({"x": x, "width": w, "y": y, "height": h}),
            );
            if w > 0 && l.width > 0 && u32::from(w) % l.width == 0 {
                extra.insert("binning".into(), json!(u32::from(w) / l.width));
            }
        }
        if let Some(sp) = &l.stored_pixels {
            extra.insert(
                "stored_pixels".into(),
                json!({"row": sp.row, "column": sp.column, "count": sp.count, "applied": true}),
            );
        }
        if let (Some(a), Some(b)) = (self.first_stamp, self.last_stamp) {
            extra.insert("frame_counter_range".into(), json!([a.counter, b.counter]));
        }
        info.extra = extra.into_iter().collect();
        info.finish()
    }

    fn read_frame(&mut self, t: u32) -> Result<Vec<u8>> {
        let l = &self.layout;
        let off = l
            .frame_offset(t)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("frame {t} offset overflows")))?;
        let row = u64::from(l.width) * u64::from(l.bytes_per_pixel);
        let plane = openreadout_core::pixel::plane_bytes_checked(
            FORMAT_ID,
            l.width,
            l.height,
            l.bytes_per_pixel as usize,
        )?;
        if off.saturating_add(l.frame_bytes) > l.file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                off,
                format!(
                    "frame {t} needs {} bytes at offset {off} but the file ends at {} (truncated; `check` lists the complete frames)",
                    l.frame_bytes, l.file_len
                ),
            ));
        }
        let raw = read_block(&mut self.file, &self.path, off, l.frame_bytes, l.file_len)?.bytes;
        let mut data =
            if u64::from(l.row_bytes) == row {
                raw
            } else {
                let mut d = Vec::with_capacity(plane);
                for r in 0..l.height as usize {
                    let s = r * l.row_bytes as usize;
                    d.extend_from_slice(raw.get(s..s + row as usize).ok_or_else(|| {
                        Error::corrupt_at(FORMAT_ID, off, "row runs past the frame")
                    })?);
                }
                d
            };
        if let Some(sp) = self.layout.stored_pixels.clone()
            && let Some(values) = self.layout.stored_values(&mut self.file, &self.path, t)?
        {
            let bpp = self.layout.bytes_per_pixel as usize;
            let at = (sp.row as usize * self.layout.width as usize + sp.column as usize) * bpp;
            if let Some(dst) = data.get_mut(at..at + values.len()) {
                dst.copy_from_slice(&values);
            }
        }
        Ok(data)
    }
}

impl Dataset for DcimgDataset {
    fn info(&self) -> Result<FileInfo> {
        let img = self.image_info();
        let l = &self.layout;
        let mut notes = Vec::new();
        if l.stored_pixels.is_some() {
            notes.push("the camera overwrites the first pixels of one row in every frame; the values stored for them are put back on read (extra.stored_pixels)".into());
        }
        if l.complete_frames() < l.frame_count {
            notes.push(format!(
                "file appears truncated: {} of {} frames are complete; run `check`",
                l.complete_frames(),
                l.frame_count
            ));
        }
        for p in &l.problems {
            notes.push(p.detail.clone());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: l.file_len,
            format: DcimgReader.descriptor(),
            format_version: Some(format!("{:#x}", l.version)),
            plane_count: img.plane_count,
            images: vec![img],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let l = &self.layout;
        Ok(json!({
            "file_header": {
                "version": l.version,
                "session_count": l.session_count,
                "header_bytes": l.header_bytes,
                "declared_size": l.declared_size,
            },
            "session": {
                "session_bytes": l.session_bytes,
                "frame_count": l.frame_count,
                "bytes_per_pixel": l.bytes_per_pixel,
                "width": l.width,
                "height": l.height,
                "row_bytes": l.row_bytes,
                "frame_bytes": l.frame_bytes,
                "data_offset": l.data_offset,
                "frame_stride": l.frame_stride,
                "trailer_bytes": l.trailer_bytes,
            },
            "footer": l.footer.map(|(o, n)| json!({"offset": o, "bytes": n})),
            "camera": {
                "model": l.camera.model,
                "serial": l.camera.serial,
                "versions": l.camera.versions,
                "sub_array": l.sub_array,
            },
            "stored_pixels": l.stored_pixels.as_ref().map(|s| json!({
                "row": s.row, "column": s.column, "count": s.count, "table_offset": s.table_offset,
            })),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("images[].size_x", Source::PriorArt),
            ("images[].size_y", Source::PriorArt),
            ("images[].size_t", Source::PriorArt),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].name", Source::Inferred),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].time_increment_s", Source::PriorArt),
            ("images[].instrument", Source::Inferred),
            ("images[].extra.dcimg_version", Source::PriorArt),
            ("images[].extra.frame_layout", Source::PriorArt),
            ("images[].extra.camera_serial", Source::Inferred),
            ("images[].extra.camera_versions", Source::Inferred),
            ("images[].extra.sensor_sub_array", Source::Inferred),
            ("images[].extra.binning", Source::Inferred),
            ("images[].extra.stored_pixels", Source::PriorArt),
            ("images[].extra.frame_counter_range", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let l = &self.layout;
        let mut out = vec![
            LsEntry {
                kind: "metadata".into(),
                name: "file-header".into(),
                offset: Some(0),
                size: Some(l.header_bytes),
                image: None,
                details: json!({"version": format!("{:#x}", l.version), "sessions": l.session_count, "declared_size": l.declared_size}),
            },
            LsEntry {
                kind: "metadata".into(),
                name: "session-header".into(),
                offset: Some(l.header_bytes),
                size: Some(l.data_offset.saturating_sub(l.header_bytes)),
                image: None,
                details: json!({"frames": l.frame_count, "width": l.width, "height": l.height, "bytes_per_pixel": l.bytes_per_pixel}),
            },
        ];
        let shown = l.frame_count.min(64);
        for t in 0..shown {
            out.push(LsEntry {
                kind: "frame".into(),
                name: format!("frame {t}"),
                offset: l.frame_offset(t),
                size: Some(l.frame_bytes + l.trailer_bytes),
                image: Some(0),
                details: json!({"t": t, "trailer_bytes": l.trailer_bytes}),
            });
        }
        if l.frame_count > shown {
            out.push(LsEntry {
                kind: "frame".into(),
                name: format!("frames {shown}..{}", l.frame_count),
                offset: l.frame_offset(shown),
                size: Some(u64::from(l.frame_count - shown) * l.frame_stride),
                image: Some(0),
                details: json!({"count": l.frame_count - shown, "stride": l.frame_stride}),
            });
        }
        if let Some((o, n)) = l.footer {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: "footer".into(),
                offset: Some(o),
                size: Some(n),
                image: None,
                details: json!({
                    "frame_counters": l.counter_table,
                    "time_stamps": l.stamp_table,
                    "stored_pixels": l.stored_pixels.as_ref().and_then(|s| s.table_offset),
                }),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image index {image} out of range (a DCIMG file holds one image)"
            )));
        }
        if idx.c != 0 || idx.z != 0 || idx.t >= self.layout.frame_count {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range (c<1, z<1, t<{})",
                idx.c, idx.z, idx.t, self.layout.frame_count
            )));
        }
        let data = self.read_frame(idx.t)?;
        Ok(Plane {
            width: self.layout.width,
            height: self.layout.height,
            pixel_type: self.pixel_type(),
            samples_per_pixel: 1,
            data,
        })
    }

    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        if image != 0 {
            return Ok((0, Vec::new()));
        }
        let l = &self.layout;
        let n = u32::try_from(limit.unwrap_or(usize::MAX))
            .unwrap_or(u32::MAX)
            .min(l.frame_count);
        let mut file = self
            .file
            .try_clone()
            .map_err(|e| Error::io(&self.path, e))?;
        let stamps = l.stamps(&mut file, &self.path, 0, n)?;
        let t0 = self.first_stamp.map(|s| s.unix_seconds());
        let out = stamps
            .iter()
            .enumerate()
            .map(|(t, s)| {
                let mut r = Map::new();
                r.insert("t".into(), json!(t));
                r.insert("frame".into(), json!(s.counter));
                if let Some(t0) = t0 {
                    r.insert(
                        "delta_t_s".into(),
                        json!(((s.unix_seconds() - t0) * 1e6).round() / 1e6),
                    );
                }
                r.insert("acquired_at".into(), json!(s.iso8601()));
                Value::Object(r)
            })
            .collect();
        Ok((u64::from(l.frame_count), out))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        let l = self.layout.clone();
        r.performed("file header: signature, known version, header size");
        r.performed("session header: frame geometry (width × bytes per pixel ≤ bytes per row, bytes per frame = bytes per row × height)");
        r.performed("every frame (and its trailer) lies inside the file");
        r.performed("the file is as long as its header declares");
        if l.layout == FrameLayout::Packed {
            r.performed("version 7: the footer and its frame counter, time stamp and stored-pixel tables lie inside the file");
        }
        r.performed("frame counters increase by one from frame to frame (gaps are dropped frames)");
        for p in &l.problems {
            let f = match p.code {
                "truncated" | "bad_footer" => Finding::error(p.code, p.detail.clone()),
                _ => Finding::warning(p.code, p.detail.clone()),
            };
            r.push(match p.offset {
                Some(o) => f.at(o),
                None => f,
            });
        }
        let complete = l.complete_frames();
        if complete < l.frame_count {
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "{} of {} frames lie completely inside the file ({} bytes); the rest of the pixel data is missing",
                        complete, l.frame_count, l.file_len
                    ),
                )
                .at(l.frame_offset(complete).unwrap_or(l.file_len)),
            );
        } else if l.layout == FrameLayout::Framed && l.frames_end().is_some_and(|e| e > l.file_len)
        {
            r.push(
                Finding::error(
                    "truncated",
                    "the last frame's trailer (frame counter and time stamp) runs past the end of the file",
                )
                .at(l.file_len),
            );
        }
        if l.file_len < l.declared_size {
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "the header declares {} bytes but the file has {} ({} missing)",
                        l.declared_size,
                        l.file_len,
                        l.declared_size - l.file_len
                    ),
                )
                .at(l.file_len),
            );
        } else if l.file_len > l.declared_size {
            r.push(Finding::warning(
                "trailing_bytes",
                format!(
                    "{} bytes follow the {} bytes the header declares",
                    l.file_len - l.declared_size,
                    l.declared_size
                ),
            ));
        }
        if l.header_bytes.saturating_add(l.session_bytes) > l.file_len {
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "the session ends at {} but the file at {}",
                        l.header_bytes.saturating_add(l.session_bytes),
                        l.file_len
                    ),
                )
                .at(l.file_len),
            );
        }
        // Frame counters: a gap means the camera dropped frames.
        let mut prev: Option<u32> = None;
        let mut gaps = 0u64;
        let mut first_gap = None;
        let mut t = 0u32;
        while t < l.frame_count {
            let batch = l.stamps(&mut self.file, &self.path, t, STAMP_BATCH)?;
            if batch.is_empty() {
                break;
            }
            for (i, s) in batch.iter().enumerate() {
                if let Some(p) = prev
                    && s.counter != p.wrapping_add(1)
                {
                    gaps += 1;
                    first_gap.get_or_insert((t as usize + i, p, s.counter));
                }
                prev = Some(s.counter);
            }
            t = t.saturating_add(batch.len() as u32);
        }
        if let Some((at, a, b)) = first_gap {
            r.push(Finding::warning(
                "frame_counter_gap",
                format!(
                    "the frame counter jumps {gaps} time(s) (first at frame {at}: {a} → {b}); frames may have been dropped during acquisition"
                ),
            ));
        }
        Ok(r)
    }
}
