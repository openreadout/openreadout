//! Molecular Dynamics GEL files (`.gel` of Typhoon, Storm and FLA scanners): TIFF pages carrying
//! the MD tags 33445-33452. `MDFileTag` 2 marks square-root data (counts = value² × scale), 128
//! linear data (counts = value × scale), `MDScalePixel` the scale (tag names and the two rules as
//! tifffile documents them; `docs/provenance/tiff.md`). Planes are returned as the counts, float32.

use openreadout_core::PixelType;
use serde_json::{Map, Value, json};

use crate::container::Ifd;
use crate::dataset::TiffDataset;

/// `MDFileTag`: 2 square-root data, 128 linear data.
pub(crate) const MD_FILE_TAG: u16 = 33445;
/// `MDScalePixel`: the scale (a rational).
pub(crate) const MD_SCALE_PIXEL: u16 = 33446;
/// `MDLabName`: the lab or user name.
pub(crate) const MD_LAB_NAME: u16 = 33448;
/// `MDSampleInfo`: `key=value` lines (scanner, laser, PMT, date).
pub(crate) const MD_SAMPLE_INFO: u16 = 33449;
/// `MDPrepDate`: `YYYY:MM:DD`.
pub(crate) const MD_PREP_DATE: u16 = 33450;
/// `MDPrepTime`: `HH:MM:SS`.
pub(crate) const MD_PREP_TIME: u16 = 33451;
/// `MDFileUnits`: the unit of the counts.
pub(crate) const MD_FILE_UNITS: u16 = 33452;

/// The MD encoding of a file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MdGel {
    /// `MDFileTag` (2 square root, 128 linear).
    pub file_tag: u64,
    /// `MDScalePixel`.
    pub scale: f64,
    /// Pixel type of the stored samples.
    pub stored: PixelType,
    /// `MDFileUnits`.
    pub units: Option<String>,
    /// The metadata for `extra.md_gel`.
    pub meta: Value,
    /// Scan date and time from `MDPrepDate`/`MDPrepTime` (local, ISO-8601).
    pub prepared_at: Option<String>,
    /// `MDSampleInfo` pairs.
    pub sample_info: Vec<(String, String)>,
}

fn text(ifd: &Ifd, tag: u16) -> Option<String> {
    ifd.text(tag)
        .map(|s| {
            s.trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .to_string()
        })
        .filter(|s| !s.is_empty())
}

/// The MD tags of a page, when it has an `MDFileTag` this module applies (2 or 128) and a
/// positive scale; `stored` is the page's sample type.
pub(crate) fn parse(ifd: &Ifd, stored: PixelType) -> Option<MdGel> {
    let file_tag = ifd.uint(MD_FILE_TAG)?;
    let scale = ifd.float(MD_SCALE_PIXEL)?;
    if !matches!(file_tag, 2 | 128) || !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    if !matches!(stored, PixelType::Uint8 | PixelType::Uint16) {
        return None;
    }
    let units = text(ifd, MD_FILE_UNITS);
    let sample_info: Vec<(String, String)> = text(ifd, MD_SAMPLE_INFO)
        .unwrap_or_default()
        .split(['\r', '\n'])
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            let (k, v) = (k.trim(), v.trim());
            (!k.is_empty()).then(|| (k.to_string(), v.to_string()))
        })
        .collect();
    let prepared_at = match (text(ifd, MD_PREP_DATE), text(ifd, MD_PREP_TIME)) {
        (Some(d), Some(t))
            if d.len() == 10 && d.as_bytes()[4] == b':' && d.as_bytes()[7] == b':' =>
        {
            Some(format!("{}T{t}", d.replace(':', "-")))
        }
        _ => None,
    };
    let mut info = Map::new();
    for (k, v) in &sample_info {
        info.insert(k.clone(), json!(v));
    }
    let meta = json!({
        "file_tag": file_tag,
        "encoding": if file_tag == 2 { "square root" } else { "linear" },
        "scale": scale,
        "units": units,
        "lab_name": text(ifd, MD_LAB_NAME),
        "prepared_at": prepared_at,
        "sample_info": Value::Object(info),
        "stored_pixel_type": format!("{stored:?}").to_lowercase(),
    });
    Some(MdGel {
        file_tag,
        scale,
        stored,
        units,
        meta,
        prepared_at,
        sample_info,
    })
}

impl MdGel {
    /// The counts of stored samples, as little-endian float32 bytes. The arithmetic is float32
    /// throughout (the square, then the product with the scale rounded to float32), as tifffile
    /// computes it, so the two agree bit for bit.
    pub(crate) fn linearize(&self, data: &[u8]) -> Vec<u8> {
        // the scale as float32, deliberately
        let scale = self.scale as f32;
        let f = |v: f32| -> [u8; 4] {
            let x = if self.file_tag == 2 {
                (v * v) * scale
            } else {
                v * scale
            };
            x.to_le_bytes()
        };
        match self.stored {
            PixelType::Uint8 => data.iter().flat_map(|&b| f(f32::from(b))).collect(),
            _ => data
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|c| f(f32::from(u16::from_le_bytes(*c))))
                .collect(),
        }
    }

    /// A sample-info value.
    pub(crate) fn info(&self, key: &str) -> Option<&str> {
        self.sample_info
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty() && *v != "-")
    }
}

impl TiffDataset {
    /// Molecular Dynamics GEL tags on the first page: every image is returned as counts
    /// (float32), when all images are single-sample 8- or 16-bit.
    pub(crate) fn apply_md_gel(&mut self, page0: &Ifd) {
        let Some(first) = self.series.first() else {
            return;
        };
        let stored = first.info.pixel_type;
        let uniform = self
            .series
            .iter()
            .all(|s| s.info.pixel_type == stored && s.info.samples_per_pixel == 1);
        let Some(g) = parse(page0, stored).filter(|_| uniform) else {
            self.notes.push(
                "Molecular Dynamics GEL tags with an encoding or sample layout this reader does not convert: values are as stored".into(),
            );
            return;
        };
        let rule = if g.file_tag == 2 {
            format!("stored value squared × {}", g.scale)
        } else {
            format!("stored value × {}", g.scale)
        };
        for s in &mut self.series {
            s.info.pixel_type = PixelType::Float;
            s.info.extra.insert("md_gel".into(), g.meta.clone());
            if s.info.acquired_at.is_none() {
                s.info.acquired_at.clone_from(&g.prepared_at);
            }
            if s.info.instrument.is_none()
                && let Some(sw) = g.info("Software")
            {
                s.info.instrument = Some(openreadout_core::model::InstrumentInfo {
                    software: Some(sw.to_string()),
                    ..Default::default()
                });
            }
        }
        self.notes.push(format!(
            "Molecular Dynamics GEL (Typhoon/Storm/FLA scan): values are the counts{} ({rule}), float32",
            g.units.as_deref().map(|u| format!(" in {u}")).unwrap_or_default()
        ));
        self.md_gel = Some(g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gel(file_tag: u64, stored: PixelType) -> MdGel {
        MdGel {
            file_tag,
            scale: 0.5,
            stored,
            units: None,
            meta: Value::Null,
            prepared_at: None,
            sample_info: vec![
                ("Serial number".into(), "123".into()),
                ("Method name".into(), "-".into()),
            ],
        }
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact values
    fn square_root_and_linear() {
        let g = gel(2, PixelType::Uint16);
        let out = g.linearize(&[3, 0, 0, 1]);
        assert_eq!(f32::from_le_bytes(out[..4].try_into().unwrap()), 4.5);
        assert_eq!(f32::from_le_bytes(out[4..].try_into().unwrap()), 32_768.0);
        let g = gel(128, PixelType::Uint8);
        assert_eq!(g.linearize(&[10]), 5f32.to_le_bytes());
        assert_eq!(g.info("Serial number"), Some("123"));
        assert_eq!(g.info("Method name"), None);
    }
}
