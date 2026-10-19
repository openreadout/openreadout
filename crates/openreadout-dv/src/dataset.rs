//! `Dataset` implementation for DeltaVision: one image of wavelengths × Z × time points, read
//! one section at a time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::Endian;
use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::header::{self, DvHeader, HEADER_LEN, SectionInfo, SectionOrder};
use crate::{DvReader, FORMAT_ID};

/// Extended-header bytes read at most.
pub const MAX_EXT: u64 = 64 << 20;
/// Bytes of one plane read at most.
pub const MAX_PLANE_BYTES: u64 = 2 << 30;

/// An opened DeltaVision file.
#[derive(Debug)]
pub struct DvDataset {
    path: PathBuf,
    fs: Fs,
    handle: Option<SourceFile>,
    file_len: u64,
    header: DvHeader,
    ext: Vec<u8>,
    problems: Vec<Finding>,
}

fn num(v: f32) -> Value {
    if v.is_finite() {
        json!(round(v))
    } else {
        Value::Null
    }
}

/// A float32 as the decimal it was written as (`0.065` rather than `0.06499999761581421`).
fn round(v: f32) -> f64 {
    format!("{v}").parse().unwrap_or(f64::from(v))
}

/// Which program wrote the file, from the processing history softWoRx appends to the title:
/// `softWoRx deconvolution` (a `DECON3D` line), `softWoRx image correction` (`IMGCORR` only) or
/// `acquisition` (neither).
pub fn written_by(titles: &[String]) -> &'static str {
    if titles.iter().any(|t| t.starts_with("DECON3D")) {
        "softWoRx deconvolution"
    } else if titles.iter().any(|t| t.starts_with("IMGCORR")) {
        "softWoRx image correction"
    } else {
        "acquisition"
    }
}

impl DvDataset {
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
        if file_len < HEADER_LEN as u64 {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("{file_len} bytes is shorter than the 1024-byte DeltaVision header"),
            ));
        }
        let mut head = vec![0u8; HEADER_LEN];
        f.read_exact_at(0, &mut head)
            .map_err(|e| Error::io(&path, e))?;
        let header = DvHeader::parse(&head).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                "not a DeltaVision file: the identifier at byte 96 is missing",
            )
        })?;
        let mut problems = Vec::new();
        if header.width <= 0 || header.height <= 0 || header.sections <= 0 {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "the header declares {} × {} × {} sections",
                    header.width, header.height, header.sections
                ),
            ));
        }
        let pt = header::mode_type(header.mode).ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("pixel mode {}", header.mode),
                "This pixel mode (complex 16-bit or unknown) is not read; report the file.",
            )
        })?;
        if header.z_count().is_none() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "{} sections are not a whole number of Z planes for {} wavelengths × {} time points",
                    header.sections,
                    header.channel_count(),
                    header.time_count()
                ),
            ));
        }
        if SectionOrder::from_code(header.sequence).is_none() {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("section order {}", header.sequence),
                "Only the section orders 0 (ZTW), 1 (WZT) and 2 (ZWT) are known; report the file.",
            ));
        }
        let ext_len = u64::try_from(header.ext_len).map_err(|_| {
            Error::corrupt(
                FORMAT_ID,
                format!("negative extended-header length {}", header.ext_len),
            )
        })?;
        let ext_read = ext_len
            .min(MAX_EXT)
            .min(file_len.saturating_sub(HEADER_LEN as u64));
        let mut ext = vec![0u8; usize::try_from(ext_read).unwrap_or(0)];
        f.read_exact_at(HEADER_LEN as u64, &mut ext)
            .map_err(|e| Error::io(&path, e))?;
        let plane = (header.width as u64) * (header.height as u64) * pt.bytes_per_sample() as u64;
        let need = HEADER_LEN as u64 + ext_len + plane * header.sections as u64;
        if need > file_len {
            problems.push(Finding::error(
                "truncated",
                format!(
                    "the header declares {need} bytes ({} sections of {plane} bytes); the file has {file_len}",
                    header.sections
                ),
            ));
        } else if need < file_len {
            problems.push(Finding::warning(
                "trailing_bytes",
                format!("{} bytes after the last section", file_len - need),
            ));
        }
        Ok(DvDataset {
            path,
            fs,
            handle: Some(f),
            file_len,
            header,
            ext,
            problems,
        })
    }

    /// The parsed header.
    pub fn header(&self) -> &DvHeader {
        &self.header
    }

    fn pixel_type(&self) -> openreadout_core::pixel::PixelType {
        header::mode_type(self.header.mode).unwrap_or(openreadout_core::pixel::PixelType::Uint8)
    }

    fn plane_bytes(&self) -> u64 {
        (self.header.width as u64)
            * (self.header.height as u64)
            * self.pixel_type().bytes_per_sample() as u64
    }

    fn data_offset(&self) -> u64 {
        HEADER_LEN as u64 + u64::try_from(self.header.ext_len).unwrap_or(0)
    }

    fn section(&self, c: u32, z: u32, t: u32) -> Option<SectionInfo> {
        let s = self.header.section_of(c, z, t)?;
        header::section_info(&self.ext, &self.header, s)
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
}

impl Dataset for DvDataset {
    fn info(&self) -> Result<FileInfo> {
        let h = &self.header;
        let mut im = ImageInfo::new(0, h.width as u32, h.height as u32, self.pixel_type());
        im.name = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string());
        im.size_c = h.channel_count();
        im.size_z = h.z_count().unwrap_or(1);
        im.size_t = h.time_count();
        im.dimension_order = match SectionOrder::from_code(h.sequence) {
            Some(SectionOrder::Ztw) => "XYZTC",
            Some(SectionOrder::Wzt) => "XYCZT",
            _ => "XYZCT",
        }
        .into();
        let sp = |v: f32| (v.is_finite() && v > 0.0).then(|| round(v));
        im.physical_size = PhysicalSize::micrometres(
            sp(h.spacing_um[0]),
            sp(h.spacing_um[1]),
            if im.size_z > 1 {
                sp(h.spacing_um[2])
            } else {
                None
            },
        );
        im.channels = (0..im.size_c)
            .map(|c| {
                let s = self.section(c, 0, 0);
                let wave = h
                    .wavelength_nm
                    .get(c as usize)
                    .copied()
                    .filter(|w| *w > 0)
                    .map(f64::from);
                let pos = |v: f32| (v.is_finite() && v > 0.0).then(|| round(v));
                ChannelInfo {
                    index: c,
                    name: wave.map(|w| format!("{w}")),
                    excitation_nm: s.and_then(|s| pos(s.excitation_nm)),
                    emission_nm: s.and_then(|s| pos(s.emission_nm)).or(wave),
                    exposure_ms: s
                        .and_then(|s| pos(s.exposure_s))
                        .map(|x| round((x * 1e3) as f32)),
                    ..ChannelInfo::default()
                }
            })
            .collect();
        if im.size_t > 1 {
            let t0 = self.section(0, 0, 0).map(|s| s.time_s);
            let t1 = self.section(0, 0, 1).map(|s| s.time_s);
            if let (Some(a), Some(b)) = (t0, t1)
                && b > a
            {
                im.time_increment_s = Some(round(b - a));
            }
        }
        let mut extra = BTreeMap::new();
        extra.insert(
            "section_order".into(),
            json!(SectionOrder::from_code(h.sequence).map(SectionOrder::name)),
        );
        extra.insert("pixel_mode".into(), json!(h.mode));
        extra.insert(
            "byte_order".into(),
            json!(match h.endian {
                Endian::Little => "little-endian",
                Endian::Big => "big-endian",
            }),
        );
        extra.insert("lens_id".into(), json!(h.lens_id));
        extra.insert("image_type".into(), json!(h.image_type));
        extra.insert(
            "wavelengths_nm".into(),
            json!(h.wavelength_nm[..(h.channel_count() as usize).min(5)].to_vec()),
        );
        if !h.titles.is_empty() {
            extra.insert("titles".into(), json!(h.titles));
        }
        extra.insert("written_by".into(), json!(written_by(&h.titles)));
        if !self.ext.is_empty() {
            extra.insert(
                "extended_header".into(),
                json!({"integers": h.ext_ints, "floats": h.ext_floats}),
            );
        }
        im.extra = extra;
        let im = im.finish();
        let mut notes = vec![format!(
            "DeltaVision {} × {} × {} sections ({} wavelength(s) × {} Z × {} time point(s), order {}), {}",
            h.width,
            h.height,
            h.sections,
            im.size_c,
            im.size_z,
            im.size_t,
            SectionOrder::from_code(h.sequence).map_or("?", SectionOrder::name),
            match h.endian {
                Endian::Little => "little-endian",
                Endian::Big => "big-endian",
            }
        )];
        if self
            .problems
            .iter()
            .any(|p| p.severity == openreadout_core::model::Severity::Error)
        {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.file_len,
            format: DvReader.descriptor(),
            format_version: None,
            plane_count: im.plane_count,
            images: vec![im],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h = &self.header;
        let mut sections = Vec::new();
        let n = u64::try_from(h.sections).unwrap_or(0).min(4096);
        for s in 0..n {
            if let Some(x) = header::section_info(&self.ext, h, s) {
                sections.push(json!({
                    "section": s, "photosensor": num(x.photosensor), "time_s": num(x.time_s),
                    "stage_um": x.stage_um.map(num), "intensity": x.intensity.map(num),
                    "exposure_s": num(x.exposure_s), "nd_filter": num(x.nd_filter),
                    "excitation_nm": num(x.excitation_nm), "emission_nm": num(x.emission_nm),
                    "intensity_scaling": num(x.intensity_scaling),
                    "energy_conversion": num(x.energy_conversion),
                }));
            }
        }
        Ok(json!({
            "header": {
                "width": h.width, "height": h.height, "sections": h.sections, "mode": h.mode,
                "start": h.start, "sampling": h.sampling, "spacing_um": h.spacing_um.map(num),
                "intensity": h.intensity.map(num), "extended_header_bytes": h.ext_len,
                "extended_integers": h.ext_ints, "extended_floats": h.ext_floats,
                "image_type": h.image_type, "lens_id": h.lens_id, "time_points": h.time_points,
                "sequence": h.sequence, "wavelengths": h.wavelengths, "wavelength_nm": h.wavelength_nm,
                "other_ranges": h.other_ranges.map(|r| r.map(num)), "origin_um": h.origin_um.map(num),
                "tilt": h.tilt.map(num), "titles": h.titles,
            },
            "sections": sections,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("images[].size_x", Source::PriorArt),
            ("images[].size_y", Source::PriorArt),
            ("images[].size_z", Source::PriorArt),
            ("images[].size_c", Source::PriorArt),
            ("images[].size_t", Source::PriorArt),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].dimension_order", Source::PriorArt),
            ("images[].physical_size", Source::PriorArt),
            ("images[].time_increment_s", Source::PriorArt),
            ("images[].channels[].name", Source::Inferred),
            ("images[].channels[].excitation_nm", Source::PriorArt),
            ("images[].channels[].emission_nm", Source::PriorArt),
            ("images[].channels[].exposure_ms", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let h = &self.header;
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: "header".into(),
            offset: Some(0),
            size: Some(HEADER_LEN as u64),
            image: None,
            details: json!({"mode": h.mode, "sections": h.sections}),
        }];
        if h.ext_len > 0 {
            out.push(LsEntry {
                kind: "extended_header".into(),
                name: "extended header".into(),
                offset: Some(HEADER_LEN as u64),
                size: Some(h.ext_len as u64),
                image: None,
                details: json!({"integers": h.ext_ints, "floats": h.ext_floats}),
            });
        }
        out.push(LsEntry {
            kind: "data".into(),
            name: "sections".into(),
            offset: Some(self.data_offset()),
            size: Some(self.plane_bytes() * h.sections as u64),
            image: Some(0),
            details: json!({"section_bytes": self.plane_bytes()}),
        });
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image index {image} out of range (1 image)"
            )));
        }
        let h = self.header.clone();
        let s = h.section_of(index.c, index.z, index.t).ok_or_else(|| {
            Error::Usage(format!(
                "plane c={} z={} t={} out of range",
                index.c, index.z, index.t
            ))
        })?;
        let bytes = self.plane_bytes();
        if bytes > MAX_PLANE_BYTES {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("sections of {bytes} bytes"),
                "The section is larger than this reader decodes at once.",
            ));
        }
        let at = self.data_offset() + s * bytes;
        if at + bytes > self.file_len {
            return Err(Error::corrupt_at(
                FORMAT_ID,
                at,
                format!(
                    "section {s} runs past the end of the file ({} bytes); the file is cut short",
                    self.file_len
                ),
            ));
        }
        let mut data = vec![0u8; bytes as usize];
        let path = self.path.clone();
        self.handle()?
            .read_exact_at(at, &mut data)
            .map_err(|e| Error::io(&path, e))?;
        let pt = self.pixel_type();
        if h.endian == Endian::Big {
            let w = match pt {
                openreadout_core::pixel::PixelType::ComplexFloat => 4,
                p => p.bytes_per_sample(),
            };
            if w > 1 {
                for c in data.chunks_exact_mut(w) {
                    c.reverse();
                }
            }
        }
        Ok(Plane {
            width: h.width as u32,
            height: h.height as u32,
            pixel_type: pt,
            samples_per_pixel: 1,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("header: DeltaVision identifier and byte order, sizes, pixel mode, sections = wavelengths × Z × time points, section order; file length = header + extended header + sections");
        for p in &self.problems {
            r.push(p.clone());
        }
        let h = &self.header;
        let ints = i64::from(h.ext_ints);
        let floats = i64::from(h.ext_floats);
        let need = (ints + floats) * 4 * i64::from(h.sections);
        if h.ext_len > 0 && need > i64::from(h.ext_len) {
            r.push(Finding::warning(
                "extended_header",
                format!(
                    "{} sections of {} integers and {} floats need {need} bytes; the extended header has {}",
                    h.sections, ints, floats, h.ext_len
                ),
            ));
        }
        Ok(r)
    }
}
