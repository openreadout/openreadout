//! SMV diffraction images (`.img`): the ADSC layout of a `{ KEY=value; … }` text header padded
//! to `HEADER_BYTES`, then one uncompressed image. Written by ADSC and other CCD detectors and
//! by electron-diffraction software (Instamatic, MicroED converters). See `docs/formats/smv.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::diffraction::Geometry;
use openreadout_core::model::{
    ChannelInfo, CheckReport, DetectConfidence, FileInfo, Finding, FormatDescriptor, ImageInfo,
    InstrumentInfo, LsEntry,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex};
use openreadout_core::source::{Input, SourceFile};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use crate::common::{self, number};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "smv";
/// Largest header accepted (`HEADER_BYTES`).
const MAX_HEADER: u64 = 1 << 20;

/// The SMV reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct SmvReader;

impl FormatReader for SmvReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::SMV)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "SMV (ADSC style)".into(),
            vendor: "ADSC and others (open layout)".into(),
            extensions: vec!["img".into(), "smv".into()],
            family: "diffraction".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::SMV.confidence,
            known_gaps: vec![
                "Uncompressed 16- and 32-bit images only; compressed SMV variants are refused (exit 6)".into(),
                "The beam centre follows the ADSC convention (millimetres, X along the slow axis) except on files that name the TimePix detector at Stockholm University (pixels); Rigaku-style headers keep their keys raw and get no beam centre".into(),
                "Each file is one image: the files of a rotation sweep are not grouped (use `batch` over them)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], _path: &Path) -> Option<Detection> {
        let peeked;
        let head = if openreadout_core::gzip::is_gzip(head) {
            peeked = openreadout_core::gzip::peek(head, 1024)?;
            &peeked[..]
        } else {
            head
        };
        looks_like_smv(head).then_some(Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(SmvDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// `{` then `HEADER_BYTES=` within the first lines.
pub fn looks_like_smv(head: &[u8]) -> bool {
    let start = head.iter().position(|b| !b.is_ascii_whitespace());
    let Some(s) = start.filter(|s| *s < 4) else {
        return false;
    };
    if head[s] != b'{' {
        return false;
    }
    let window = &head[s..head.len().min(s + 128)];
    window
        .windows(13)
        .any(|w| w.eq_ignore_ascii_case(b"HEADER_BYTES="))
}

/// The parsed header.
#[derive(Debug, Clone)]
pub struct SmvHeader {
    /// Every `KEY=value` pair in file order (keys as written).
    pub fields: Vec<(String, String)>,
    /// `HEADER_BYTES`: where the image starts.
    pub header_bytes: u64,
    /// `SIZE1` (samples per row) and `SIZE2` (rows).
    pub size: [u32; 2],
    /// Sample type from `TYPE`.
    pub pixel_type: PixelType,
    /// `BYTE_ORDER=big_endian`.
    pub big_endian: bool,
}

impl SmvHeader {
    /// Parse the text between `{` and `}`.
    pub fn parse(text: &str) -> Result<SmvHeader> {
        let body = text.trim_start().strip_prefix('{').unwrap_or(text);
        let body = body.split('}').next().unwrap_or(body);
        let mut fields = Vec::new();
        for item in body.split(';') {
            if let Some((k, v)) = item.split_once('=') {
                let k = k.trim();
                if !k.is_empty() {
                    fields.push((k.to_string(), v.trim().to_string()));
                }
            }
        }
        let mut h = SmvHeader {
            fields,
            header_bytes: 0,
            size: [0, 0],
            pixel_type: PixelType::Uint16,
            big_endian: false,
        };
        let need = |h: &SmvHeader, k: &str| -> Result<u64> {
            h.get(k)
                .and_then(|v| v.parse::<u64>().ok())
                .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("header has no usable {k}")))
        };
        h.header_bytes = need(&h, "HEADER_BYTES")?;
        let s1 = need(&h, "SIZE1")?;
        let s2 = need(&h, "SIZE2")?;
        let dim = h
            .get("DIM")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(2);
        if dim != 2 {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("DIM={dim}"),
                "only two-dimensional SMV images are read",
            ));
        }
        let to32 = |v: u64, k: &str| {
            u32::try_from(v)
                .ok()
                .filter(|v| *v > 0)
                .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("{k}={v} is not a usable size")))
        };
        h.size = [to32(s1, "SIZE1")?, to32(s2, "SIZE2")?];
        if !(512..=MAX_HEADER).contains(&h.header_bytes) {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "HEADER_BYTES={} is outside 512..{MAX_HEADER}",
                    h.header_bytes
                ),
            ));
        }
        let ty = h
            .get("TYPE")
            .unwrap_or("unsigned_short")
            .to_ascii_lowercase();
        h.pixel_type = match ty.as_str() {
            "unsigned_short" | "mad_unsigned_short" => PixelType::Uint16,
            "signed_short" | "short" => PixelType::Int16,
            "unsigned_int" | "unsigned_long" => PixelType::Uint32,
            "signed_int" | "int" | "long" | "signed_long" => PixelType::Int32,
            "float" | "float_ieee" => PixelType::Float,
            other => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("TYPE={other}"),
                    "16- and 32-bit integer and 32-bit float SMV images are read",
                ));
            }
        };
        if let Some(c) = h.get("COMPRESSION")
            && !c.eq_ignore_ascii_case("none")
        {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("COMPRESSION={c}"),
                "only uncompressed SMV images are read",
            ));
        }
        h.big_endian = h
            .get("BYTE_ORDER")
            .is_some_and(|b| b.to_ascii_lowercase().starts_with("big"));
        Ok(h)
    }

    /// The value of `key` (case-insensitive), first occurrence.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    fn num(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(number)
    }

    /// Bytes of image data the header declares.
    pub fn data_len(&self) -> Option<u64> {
        u64::from(self.size[0])
            .checked_mul(u64::from(self.size[1]))?
            .checked_mul(self.pixel_type.bytes_per_sample() as u64)
    }

    /// True when the header names the TimePix detector at Stockholm University, whose beam
    /// centre is written in pixels.
    pub fn beam_center_in_pixels(&self) -> bool {
        self.get("BEAMLINE")
            .is_some_and(|b| b.eq_ignore_ascii_case("TimePix_SU"))
    }

    /// The shared geometry.
    pub fn geometry(&self) -> Geometry {
        let px = self.num("PIXEL_SIZE").filter(|p| *p > 0.0);
        let bx = self.num("BEAM_CENTER_X");
        let by = self.num("BEAM_CENTER_Y");
        let beam = match (bx, by, px) {
            (Some(x), Some(y), _) if self.beam_center_in_pixels() => Some([x, y]),
            // ADSC: millimetres, X along the slow axis, Y along the fast axis.
            (Some(x), Some(y), Some(p)) => Some([y / p, x / p]),
            _ => None,
        };
        let wavelength_a = self.num("WAVELENGTH").filter(|w| *w > 0.0);
        let overload = self
            .num("CCD_IMAGE_SATURATION")
            .or_else(|| self.num("SATURATED_VALUE"))
            .or(match self.pixel_type {
                PixelType::Uint16 => Some(65535.0),
                _ => None,
            });
        Geometry {
            radiation: common::radiation_of(wavelength_a),
            wavelength_a,
            distance_mm: self.num("DISTANCE").filter(|d| *d > 0.0),
            beam_center_px: beam,
            pixel_size_mm: px.map(|p| [p, p]),
            two_theta_deg: self.num("TWOTHETA").filter(|t| *t != 0.0),
            scan_axis: self.get("AXIS").map(str::to_ascii_lowercase),
            oscillation_start_deg: self.num("OSC_START").or_else(|| self.num("PHI")),
            oscillation_width_deg: self.num("OSC_RANGE"),
            exposure_s: self.num("TIME"),
            overload,
            masked_values: Vec::new(),
            negative_is_masked: false,
        }
    }
}

/// An opened SMV file.
#[derive(Debug)]
pub struct SmvDataset {
    path: PathBuf,
    file: SourceFile,
    file_len: u64,
    header: SmvHeader,
    gzip: Option<(u64, Option<String>)>,
}

impl SmvDataset {
    /// Open an SMV file (plain or gzip-compressed) and parse its header.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let o = common::open(input, FORMAT_ID)?;
        // The header length is in the header: read the first KiB, then the whole header.
        let first = common::read_exact(&o.file, 0, o.len.min(1024), o.len, FORMAT_ID, "header")?;
        if !looks_like_smv(&first) {
            return Err(Error::corrupt(
                FORMAT_ID,
                "the file does not start with an SMV header (`{` and HEADER_BYTES=)",
            ));
        }
        let mut header = SmvHeader::parse(&String::from_utf8_lossy(&first))?;
        if header.header_bytes > first.len() as u64 {
            let all = common::read_exact(
                &o.file,
                0,
                header.header_bytes.min(o.len),
                o.len,
                FORMAT_ID,
                "header",
            )?;
            header = SmvHeader::parse(&String::from_utf8_lossy(&all))?;
        }
        Ok(SmvDataset {
            path: input.path().to_path_buf(),
            file: o.file,
            file_len: o.len,
            header,
            gzip: o.gzip,
        })
    }

    /// The parsed header.
    pub fn header(&self) -> &SmvHeader {
        &self.header
    }

    fn image_info(&self) -> ImageInfo {
        let h = &self.header;
        let mut im = ImageInfo::new(0, h.size[0], h.size[1], h.pixel_type);
        im.name = self
            .path
            .file_stem()
            .map(|n| openreadout_core::gzip::strip_gz(&n.to_string_lossy()).to_string());
        im.channels = vec![ChannelInfo {
            index: 0,
            ..ChannelInfo::default()
        }];
        let detector = h
            .get("DETECTOR_NAME")
            .or_else(|| h.get("DETECTOR_NAMES"))
            .or_else(|| h.get("BEAMLINE"))
            .map(str::to_string);
        if detector.is_some() {
            im.instrument = Some(InstrumentInfo {
                manufacturer: None,
                model: None,
                software: None,
                software_version: None,
                detector,
            });
        }
        h.geometry().store(&mut im);
        let mut smv = Map::new();
        for (key, ours) in [
            ("BEAMLINE", "beamline"),
            ("DETECTOR_SN", "detector_serial"),
            ("DATE", "date"),
            ("BIN", "binning"),
            ("GAIN", "gain"),
            ("IMAGE_PEDESTAL", "pedestal"),
        ] {
            if let Some(v) = h.get(key) {
                smv.insert(
                    ours.into(),
                    number(v).map_or_else(|| json!(v), |n| json!(n)),
                );
            }
        }
        smv.insert(
            "beam_center_convention".into(),
            json!(if h.beam_center_in_pixels() {
                "pixels (fast, slow)"
            } else {
                "ADSC (mm, X slow, Y fast)"
            }),
        );
        im.extra.insert("smv".into(), Value::Object(smv));
        im.finish()
    }

    fn data_len(&self) -> Result<u64> {
        self.header
            .data_len()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "image size overflows"))
    }
}

impl Dataset for SmvDataset {
    fn info(&self) -> Result<FileInfo> {
        let im = self.image_info();
        let mut notes = Vec::new();
        let need = self.header.header_bytes.saturating_add(self.data_len()?);
        if need > self.file_len {
            notes.push(format!(
                "file appears truncated: header and image need {need} bytes, the file has {}; run `check`",
                self.file_len
            ));
        }
        if let Some((compressed, problem)) = &self.gzip {
            notes.push(format!(
                "gzip-compressed: {compressed} bytes decompress to {}",
                self.file_len
            ));
            if let Some(p) = problem {
                notes.push(format!("gzip: {p}"));
            }
        }
        if self.header.get("IMAGE_PEDESTAL").is_some() {
            notes.push("IMAGE_PEDESTAL is recorded: values are returned as stored, the pedestal is not subtracted".into());
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.gzip.as_ref().map_or(self.file_len, |g| g.0),
            format: SmvReader.descriptor(),
            format_version: None,
            plane_count: im.plane_count,
            images: vec![im],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        openreadout_core::diffraction::experiment(&self.info().ok()?.images)
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let header: BTreeMap<&str, &str> = self
            .header
            .fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        Ok(json!({"header": header}))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("images[].size_x", Source::PriorArt),
            ("images[].size_y", Source::PriorArt),
            ("images[].pixel_type", Source::PriorArt),
            ("images[].name", Source::Inferred),
            ("images[].instrument", Source::Inferred),
            ("images[].extra.smv", Source::PriorArt),
            ("images[].extra.diffraction.wavelength_a", Source::PriorArt),
            ("images[].extra.diffraction.distance_mm", Source::PriorArt),
            (
                "images[].extra.diffraction.beam_center_px",
                Source::PriorArt,
            ),
            ("images[].extra.diffraction.pixel_size_mm", Source::PriorArt),
            (
                "images[].extra.diffraction.oscillation_start_deg",
                Source::PriorArt,
            ),
            (
                "images[].extra.diffraction.oscillation_width_deg",
                Source::PriorArt,
            ),
            ("images[].extra.diffraction.exposure_s", Source::PriorArt),
            ("images[].extra.diffraction.overload", Source::Inferred),
            ("images[].extra.diffraction.radiation", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![
            LsEntry {
                kind: "metadata".into(),
                name: "header".into(),
                offset: Some(0),
                size: Some(self.header.header_bytes),
                image: None,
                details: json!({"fields": self.header.fields.len()}),
            },
            LsEntry {
                kind: "image".into(),
                name: "image".into(),
                offset: Some(self.header.header_bytes),
                size: self.header.data_len(),
                image: Some(0),
                details: json!({"size": self.header.size, "type": self.header.pixel_type.ome_name(), "big_endian": self.header.big_endian}),
            },
        ])
    }

    fn read_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        if image != 0 || idx.c != 0 || idx.z != 0 || idx.t != 0 {
            return Err(Error::Usage(format!(
                "image {image} plane c={} z={} t={} out of range (an SMV file holds one image, one plane)",
                idx.c, idx.z, idx.t
            )));
        }
        let len = self.data_len()?;
        let mut data = common::read_exact(
            &self.file,
            self.header.header_bytes,
            len,
            self.file_len,
            FORMAT_ID,
            "image",
        )?;
        let bps = self.header.pixel_type.bytes_per_sample();
        if self.header.big_endian && bps > 1 {
            for c in data.chunks_exact_mut(bps) {
                c.reverse();
            }
        }
        Ok(Plane {
            width: self.header.size[0],
            height: self.header.size[1],
            pixel_type: self.header.pixel_type,
            samples_per_pixel: 1,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed(
            "header: `{ KEY=value; … }` with HEADER_BYTES, SIZE1, SIZE2, TYPE and BYTE_ORDER",
        );
        r.performed("the file holds the header and SIZE1 × SIZE2 samples");
        r.performed("the geometry values (distance, wavelength, pixel size) are positive numbers when present");
        let need = self.header.header_bytes.saturating_add(self.data_len()?);
        if need > self.file_len {
            r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "header and image need {need} bytes but the file has {}",
                        self.file_len
                    ),
                )
                .at(self.file_len),
            );
        } else if need < self.file_len {
            r.push(Finding::warning(
                "trailing_bytes",
                format!("{} bytes follow the image", self.file_len - need),
            ));
        }
        if let Some((_, Some(p))) = &self.gzip {
            r.push(Finding::error("truncated", format!("gzip: {p}")));
        }
        for k in ["DISTANCE", "WAVELENGTH", "PIXEL_SIZE"] {
            if let Some(v) = self.header.get(k)
                && number(v).is_none_or(|n| n <= 0.0)
            {
                r.push(Finding::warning(
                    "bad_geometry",
                    format!("{k}={v} is not a positive number"),
                ));
            }
        }
        let g = self.header.geometry();
        if g.beam_center_px.is_some_and(|[x, y]| {
            x < 0.0
                || y < 0.0
                || x > f64::from(self.header.size[0])
                || y > f64::from(self.header.size[1])
        }) {
            r.push(Finding::warning(
                "beam_center_outside",
                "the beam centre lies outside the image; its unit convention may differ from the one assumed (extra.smv.beam_center_convention)",
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADSC: &str = "{\nHEADER_BYTES=  512;\nTYPE=unsigned_short;\nBYTE_ORDER=little_endian;\nDIM=2;\nSIZE1= 2048;\nSIZE2= 2048;\nDISTANCE= 1065.7;\nWAVELENGTH=  0.02501404;\nPIXEL_SIZE=   0.028;\nOSC_RANGE=   0.888;\nOSC_START= -54.672;\nBEAM_CENTER_X=  29.800;\nBEAM_CENTER_Y=  29.900;\nTIME=0.222;\nBEAMLINE=CETAD_TUI;\n}\n";

    #[test]
    fn adsc_header_and_beam_centre() {
        assert!(looks_like_smv(ADSC.as_bytes()));
        let h = SmvHeader::parse(ADSC).unwrap();
        assert_eq!(h.header_bytes, 512);
        assert_eq!(h.size, [2048, 2048]);
        assert_eq!(h.pixel_type, PixelType::Uint16);
        let g = h.geometry();
        let [x, y] = g.beam_center_px.unwrap();
        assert!((x - 29.9 / 0.028).abs() < 1e-9 && (y - 29.8 / 0.028).abs() < 1e-9);
        assert_eq!(
            g.radiation,
            Some(openreadout_core::diffraction::Radiation::Electron)
        );
        assert_eq!(g.oscillation_width_deg, Some(0.888));
        assert_eq!(g.overload, Some(65535.0));
    }

    #[test]
    fn timepix_beam_centre_is_in_pixels() {
        let t = "{\nHEADER_BYTES=512;\nDIM=2;\nBYTE_ORDER=little_endian;\nTYPE=unsigned_short;\nSIZE1=516;\nSIZE2=516;\nPIXEL_SIZE=0.055;\nBEAMLINE=TimePix_SU;\nBEAM_CENTER_X=256.3600;\nBEAM_CENTER_Y=249.0400;\n}";
        let g = SmvHeader::parse(t).unwrap().geometry();
        assert_eq!(g.beam_center_px, Some([256.36, 249.04]));
    }

    #[test]
    fn bad_headers_are_errors() {
        assert!(SmvHeader::parse("{\nHEADER_BYTES=512;\nSIZE1=10;\n}").is_err());
        assert!(SmvHeader::parse("{\nHEADER_BYTES=12;\nSIZE1=10;\nSIZE2=10;\n}").is_err());
        assert!(
            SmvHeader::parse("{\nHEADER_BYTES=512;\nSIZE1=10;\nSIZE2=10;\nTYPE=weird;\n}").is_err()
        );
        assert!(!looks_like_smv(b"hello"));
    }
}
