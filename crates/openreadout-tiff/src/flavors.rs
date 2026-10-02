//! Small text conventions layered on TIFF by slide scanners and acquisition software:
//! Aperio SVS descriptions, PerkinElmer QPI page XML, Micro-Manager JSON.
//! See `docs/formats/tiff.md` § Aperio SVS, § PerkinElmer QPTIFF, § Micro-Manager.

use openreadout_core::Result;
use openreadout_core::model::{InstrumentInfo, ObjectiveInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use serde_json::{Map, Value, json};

use crate::dataset::{Attachment, Level, TiffDataset};
use crate::tags;

/// Parsed Aperio `ImageDescription`: a header line, then `|`-separated `key = value` pairs.
#[derive(Debug, Clone, Default)]
pub struct AperioDescription {
    /// Everything before the first `|` (library version, level geometry, codec).
    pub header: String,
    pub keys: Map<String, Value>,
}

/// `None` unless the description starts with `Aperio`.
pub fn parse_aperio(desc: &str) -> Option<AperioDescription> {
    if !desc.starts_with("Aperio") {
        return None;
    }
    let mut parts = desc.split('|');
    let header = parts.next().unwrap_or_default().trim().to_string();
    let mut keys = Map::new();
    for p in parts {
        if let Some((k, v)) = p.split_once('=') {
            let v = v.trim();
            let val = v
                .parse::<i64>()
                .map(Value::from)
                .ok()
                .or_else(|| {
                    v.parse::<f64>()
                        .ok()
                        .filter(|f| f.is_finite())
                        .map(Value::from)
                })
                .unwrap_or_else(|| Value::String(v.to_string()));
            keys.insert(k.trim().to_string(), val);
        }
    }
    Some(AperioDescription { header, keys })
}

impl AperioDescription {
    pub fn number(&self, key: &str) -> Option<f64> {
        match self.keys.get(key)? {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
    pub fn text(&self, key: &str) -> Option<String> {
        match self.keys.get(key)? {
            Value::String(s) => Some(s.clone()),
            v => Some(v.to_string()),
        }
    }
    /// `Date = MM/DD/YY` and `Time = HH:MM:SS` → ISO-8601 (local time, no zone).
    pub fn acquired_at(&self) -> Option<String> {
        let date = self.text("Date")?;
        let mut d = date.split('/');
        let (m, dd, y) = (
            d.next()?.trim().parse::<u32>().ok()?,
            d.next()?.trim().parse::<u32>().ok()?,
            d.next()?.trim().parse::<u32>().ok()?,
        );
        let y = if y < 100 { 2000 + y } else { y };
        if !(1..=12).contains(&m) || !(1..=31).contains(&dd) {
            return None;
        }
        let time = self
            .text("Time")
            .filter(|t| t.len() >= 8 && t.as_bytes()[2] == b':')
            .map_or_else(|| "00:00:00".to_string(), |t| t[..8].to_string());
        Some(format!("{y:04}-{m:02}-{dd:02}T{time}"))
    }
}

/// Per-page PerkinElmer QPI description (`<PerkinElmer-QPI-ImageDescription>`).
#[derive(Debug, Clone, Default)]
pub struct QpiPage {
    /// `FullResolution`, `Thumbnail`, `ReducedResolution`, `Overview`, `Label`, ...
    pub image_type: Option<String>,
    pub name: Option<String>,
    /// `#RRGGBB`.
    pub color: Option<String>,
    pub objective: Option<String>,
    pub magnification: Option<f64>,
    pub pixel_size_um: Option<f64>,
    pub exposure_time: Option<f64>,
    pub slide_id: Option<String>,
    pub acquisition_software: Option<String>,
    pub instrument_type: Option<String>,
    /// Every simple (text-only) child element, names untouched.
    pub fields: Map<String, Value>,
}

/// `None` unless the text is a QPI description.
pub fn parse_qpi(desc: &str) -> Option<QpiPage> {
    if !desc.contains("PerkinElmer-QPI-ImageDescription") {
        return None;
    }
    // Some writers declare encoding="utf-16" while storing UTF-8; the declaration is ignored.
    let start = desc.find("<PerkinElmer-QPI-ImageDescription")?;
    let doc = roxmltree::Document::parse(&desc[start..]).ok()?;
    let root = doc.root_element();
    let mut fields = Map::new();
    for c in root.children().filter(roxmltree::Node::is_element) {
        if c.children().all(|n| n.is_text()) {
            let t = c.text().unwrap_or_default().trim().to_string();
            fields.insert(c.tag_name().name().to_string(), Value::String(t));
        }
    }
    let get = |k: &str| {
        fields
            .get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let color = get("Color").and_then(|c| {
        let v: Vec<u8> = c
            .split(',')
            .filter_map(|p| p.trim().parse::<u8>().ok())
            .collect();
        (v.len() == 3).then(|| format!("#{:02X}{:02X}{:02X}", v[0], v[1], v[2]))
    });
    let deep = |k: &str| {
        root.descendants()
            .find(|n| n.tag_name().name() == k)
            .and_then(|n| n.text())
            .and_then(|t| t.trim().parse::<f64>().ok())
    };
    Some(QpiPage {
        image_type: get("ImageType"),
        name: get("Name"),
        color,
        objective: get("Objective"),
        magnification: deep("Magnification").filter(|m| *m > 0.0),
        pixel_size_um: deep("PixelSizeMicrons"),
        exposure_time: get("ExposureTime").and_then(|v| v.parse().ok()),
        slide_id: get("SlideID"),
        acquisition_software: get("AcquisitionSoftware"),
        instrument_type: get("InstrumentType"),
        fields,
    })
}

/// Micro-Manager per-plane metadata (tag 51123): a JSON object; a few keys are normalized.
#[derive(Debug, Clone, Default)]
pub struct MicroManagerPlane {
    pub json: Map<String, Value>,
    pub pixel_size_um: Option<f64>,
    pub channel: Option<String>,
    pub exposure_ms: Option<f64>,
    pub camera: Option<String>,
    pub time: Option<String>,
}

pub fn parse_micromanager(text: &str) -> Option<MicroManagerPlane> {
    let v: Value = serde_json::from_str(text.trim_end_matches('\0')).ok()?;
    let Value::Object(json) = v else {
        return None;
    };
    let num = |k: &str| match json.get(k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    };
    let text = |k: &str| {
        json.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Some(MicroManagerPlane {
        pixel_size_um: num("PixelSizeUm").filter(|v| *v > 0.0),
        channel: text("Channel"),
        exposure_ms: num("Exposure-ms"),
        camera: text("Camera").or_else(|| text("Core-Camera")),
        time: text("Time"),
        json,
    })
}

impl TiffDataset {
    pub(crate) fn build_svs(&mut self, ap: &AperioDescription) -> Result<()> {
        let n = self.main().ifds.len();
        let base = self.page_dims(0);
        let mut z_pages = vec![0usize];
        let mut levels = Vec::new();
        let mut idx = 1;
        if n > 1 && self.main().ifds[1].field(tags::TILE_WIDTH).is_none() {
            self.attachments.push(Attachment {
                role: "thumbnail".into(),
                page: 1,
            });
            idx = 2;
        }
        while idx < n {
            let ifd = &self.main().ifds[idx];
            let reduced = ifd.uint(tags::NEW_SUBFILE_TYPE).unwrap_or(0) & 1 == 1;
            if ifd.field(tags::TILE_WIDTH).is_none() || reduced {
                break;
            }
            let d = self.page_dims(idx);
            if d == base {
                z_pages.push(idx);
            } else {
                levels.push(Level {
                    width: d.0,
                    height: d.1,
                    page: Some(idx),
                    sub_ifd: None,
                    pages: Vec::new(),
                });
            }
            idx += 1;
        }
        while idx < n {
            let sub = self.main().ifds[idx]
                .uint(tags::NEW_SUBFILE_TYPE)
                .unwrap_or(0);
            self.attachments.push(Attachment {
                role: match sub {
                    9 => "macro",
                    1 => "label",
                    _ => "associated",
                }
                .into(),
                page: idx,
            });
            idx += 1;
        }
        let name = ap.text("Filename").or_else(|| ap.text("Title"));
        let mut info = self.push_slide(name, &z_pages, &[], levels)?;
        if let Some(mpp) = ap.number("MPP").filter(|v| *v > 0.0) {
            info.physical_size = PhysicalSize::micrometres(Some(mpp), Some(mpp), None);
        }
        if let Some(mag) = ap.number("AppMag").filter(|v| *v > 0.0) {
            info.objective = Some(ObjectiveInfo {
                nominal_magnification: Some(mag),
                ..ObjectiveInfo::default()
            });
        }
        info.acquired_at = ap.acquired_at();
        info.instrument = Some(InstrumentInfo {
            manufacturer: Some("Aperio".into()),
            model: ap.text("ScanScope ID").map(|s| format!("ScanScope {s}")),
            software: Some(
                ap.header
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            )
            .filter(|s| !s.is_empty()),
            ..InstrumentInfo::default()
        });
        info.extra
            .insert("aperio".into(), Value::Object(ap.keys.clone()));
        self.replace_last(info);
        self.set_provenance(&[
            ("images[].physical_size", Source::PriorArt),
            ("images[].objective.nominal_magnification", Source::PriorArt),
            ("images[].acquired_at", Source::PriorArt),
            ("images[].pyramid_levels", Source::PriorArt),
            ("images[].size_z", Source::PriorArt),
        ]);
        Ok(())
    }
}

impl TiffDataset {
    pub(crate) fn build_qptiff(&mut self) -> Result<()> {
        let n = self.main().ifds.len();
        let pages: Vec<(usize, Option<QpiPage>, (u32, u32))> = (0..n)
            .map(|p| {
                let q = self.main().ifds[p]
                    .text(tags::IMAGE_DESCRIPTION)
                    .and_then(parse_qpi);
                (p, q, self.page_dims(p))
            })
            .collect();
        let base = pages[0].2;
        let mut channels = Vec::new();
        let mut idx = 0;
        while idx < n
            && pages[idx].2 == base
            && pages[idx].1.as_ref().is_none_or(|q| {
                q.image_type
                    .as_deref()
                    .is_none_or(|t| t == "FullResolution")
            })
        {
            channels.push(idx);
            idx += 1;
        }
        let mut levels = Vec::new();
        for (p, q, d) in &pages[idx..] {
            match q.as_ref().and_then(|q| q.image_type.as_deref()) {
                Some("ReducedResolution") => {
                    if !levels.iter().any(|l: &Level| (l.width, l.height) == *d) {
                        levels.push(Level {
                            width: d.0,
                            height: d.1,
                            page: Some(*p),
                            sub_ifd: None,
                            pages: Vec::new(),
                        });
                    }
                }
                Some(t) => self.attachments.push(Attachment {
                    role: t.to_ascii_lowercase(),
                    page: *p,
                }),
                None => self.attachments.push(Attachment {
                    role: "associated".into(),
                    page: *p,
                }),
            }
        }
        let first = pages[0].1.clone().unwrap_or_default();
        let mut info = self.push_slide(first.slide_id.clone(), &[], &channels, levels)?;
        if channels.len() > 1 || info.samples_per_pixel == 1 {
            for (ci, &p) in channels.iter().enumerate() {
                if let (Some(ch), Some(q)) = (info.channels.get_mut(ci), pages[p].1.as_ref()) {
                    ch.name = q.name.clone();
                    ch.color = q.color.clone();
                }
            }
        }
        // The unit of QPI ExposureTime is not documented publicly: keep the raw values.
        let exposures: Vec<Value> = channels
            .iter()
            .map(|&p| json!(pages[p].1.as_ref().and_then(|q| q.exposure_time)))
            .collect();
        if exposures.iter().any(|v| !v.is_null()) {
            info.extra
                .insert("qpi_exposure_time_raw".into(), Value::Array(exposures));
        }
        if let Some(mpp) = first.pixel_size_um.filter(|v| *v > 0.0) {
            info.physical_size = PhysicalSize::micrometres(Some(mpp), Some(mpp), None);
        }
        if first.objective.is_some() || first.magnification.is_some() {
            info.objective = Some(ObjectiveInfo {
                model: first.objective.clone(),
                nominal_magnification: first.magnification,
                ..ObjectiveInfo::default()
            });
        }
        info.instrument = Some(InstrumentInfo {
            manufacturer: Some("PerkinElmer".into()),
            model: first.instrument_type.clone(),
            software: first.acquisition_software.clone(),
            ..InstrumentInfo::default()
        });
        self.replace_last(info);
        self.set_provenance(&[
            ("images[].size_c", Source::PriorArt),
            ("images[].channels[].name", Source::Inferred),
            ("images[].channels[].color", Source::Inferred),
            ("images[].physical_size", Source::Spec),
            ("images[].pyramid_levels", Source::PriorArt),
        ]);
        Ok(())
    }
}

impl TiffDataset {
    pub(crate) fn apply_micromanager(&mut self, mm: &MicroManagerPlane) {
        let Some(s) = self.series.first_mut() else {
            return;
        };
        let info = &mut s.info;
        if info.physical_size.x.is_none()
            && let Some(px) = mm.pixel_size_um
        {
            info.physical_size.x = Some(px);
            info.physical_size.y = Some(px);
        }
        if info.size_c == 1
            && let Some(ch) = info.channels.first_mut()
        {
            if ch.name.is_none() {
                ch.name.clone_from(&mm.channel);
            }
            if ch.exposure_ms.is_none() {
                ch.exposure_ms = mm.exposure_ms;
            }
        }
        if let Some(cam) = &mm.camera {
            let inst = info.instrument.get_or_insert_with(InstrumentInfo::default);
            if inst.detector.is_none() {
                inst.detector = Some(cam.clone());
            }
            if inst.software.is_none() {
                inst.software = Some("Micro-Manager".into());
            }
        }
        if info.acquired_at.is_none() {
            info.acquired_at.clone_from(&mm.time);
        }
        let mut keep = Map::new();
        for k in [
            "PixelSizeUm",
            "Channel",
            "Exposure-ms",
            "Camera",
            "Time",
            "Binning",
            "ElapsedTime-ms",
            "PositionName",
            "MicroManagerVersion",
        ] {
            if let Some(v) = mm.json.get(k) {
                keep.insert(k.into(), v.clone());
            }
        }
        info.extra
            .insert("micromanager".into(), Value::Object(keep));
        self.provenance
            .insert("images[0].extra.micromanager".into(), Source::PriorArt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aperio_description() {
        let d = parse_aperio("Aperio Image Library v10.0.51\r\n46920x33014 [0,100 46000x32914] (256x256) JPEG/RGB Q=30|AppMag = 20|MPP = 0.4990|Date = 12/29/09|Time = 09:59:15").unwrap();
        assert_eq!(d.number("AppMag"), Some(20.0));
        assert_eq!(d.number("MPP"), Some(0.499));
        assert_eq!(d.acquired_at().as_deref(), Some("2009-12-29T09:59:15"));
        assert!(parse_aperio("ImageJ=1.5").is_none());
    }

    #[test]
    fn qpi_page() {
        let q = parse_qpi("<?xml version=\"1.0\" encoding=\"utf-16\"?>\n<PerkinElmer-QPI-ImageDescription><ImageType>FullResolution</ImageType><Name>Eosin</Name><Color>255,60,157</Color></PerkinElmer-QPI-ImageDescription>").unwrap();
        assert_eq!(q.name.as_deref(), Some("Eosin"));
        assert_eq!(q.color.as_deref(), Some("#FF3C9D"));
        assert_eq!(q.image_type.as_deref(), Some("FullResolution"));
    }

    #[test]
    fn micromanager_json() {
        let m = parse_micromanager(r#"{"PixelSizeUm":0.5,"Channel":"FITC","Exposure-ms":"10"}"#)
            .unwrap();
        assert_eq!(m.pixel_size_um, Some(0.5));
        assert_eq!(m.exposure_ms, Some(10.0));
    }
}
