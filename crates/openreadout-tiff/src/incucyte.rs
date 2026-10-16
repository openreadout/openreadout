//! Sartorius Incucyte TIFF exports. A raw export holds the image on page 0 and a 1 × 1 page
//! (NewSubfileType 1) whose tag 60020 is an `<ImageInfo>` XML document in Windows-1252 with the
//! acquisition settings; the "for viewing" exports are RGB renderings without it. See
//! `docs/formats/tiff.md` § Incucyte and `docs/provenance/tiff.md`.

use openreadout_core::model::{ObjectiveInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use serde_json::{Map, Value, json};

use crate::container::Ifd;
use crate::dataset::{TiffDataset, text_value};
use crate::imagers::{ImagerSettings, Param, duration_s, leading_number};
use crate::tags;

/// Tag of the `<ImageInfo>` XML.
pub(crate) const INCUCYTE_INFO_TAG: u16 = 60020;
/// Largest XML read.
const MAX_INFO_BYTES: u64 = 1 << 20;

/// Whether page 0's `Software` names Incucyte.
pub(crate) fn is_incucyte(page0: &Ifd) -> bool {
    page0
        .text(tags::SOFTWARE)
        .is_some_and(|s| s.trim().to_ascii_lowercase().starts_with("incucyte"))
}

/// The settings in an `<ImageInfo>` document (already decoded to text).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct IncucyteInfo {
    pub(crate) pixel_size_um: Option<f64>,
    pub(crate) magnification: Option<f64>,
    pub(crate) image_type: Option<String>,
    pub(crate) exposure_s: Option<f64>,
    /// `TimeStamp`, `MM/DD/YYYY HH:MM:SS` → ISO 8601 (local time).
    pub(crate) acquired_at: Option<String>,
    pub(crate) hardware: Option<String>,
    pub(crate) serial: Option<String>,
    pub(crate) camera: Option<String>,
    /// `images[].extra.incucyte`.
    pub(crate) extra: Map<String, Value>,
}

/// `07/22/2020 13:33:44` → `2020-07-22T13:33:44`.
fn us_datetime(text: &str) -> Option<String> {
    let (date, time) = text.trim().split_once(' ')?;
    let mut parts = date.split('/');
    let (month, day, year) = (
        parts.next()?.parse::<u32>().ok()?,
        parts.next()?.parse::<u32>().ok()?,
        parts.next()?.parse::<u32>().ok()?,
    );
    let clock = time.trim();
    let cb = clock.as_bytes();
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(1990..=2200).contains(&year)
        || cb.len() != 8
        || cb[2] != b':'
        || cb[5] != b':'
    {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}T{clock}"))
}

/// Parses the `<ImageInfo>` XML; `None` when it is not one.
pub(crate) fn parse_info(xml: &str) -> Option<IncucyteInfo> {
    let start = xml.find("<ImageInfo")?;
    let doc = roxmltree::Document::parse(xml[start..].trim_end_matches('\0')).ok()?;
    let root = doc.root_element();
    let find = |path: &[&str]| -> Option<String> {
        let mut n = root;
        for p in path {
            n = n.children().find(|c| c.has_tag_name(*p))?;
        }
        let t = n.text()?.trim();
        (!t.is_empty()).then(|| t.to_string())
    };
    let mut extra = Map::new();
    let mut put = |k: &str, v: Option<Value>| {
        if let Some(v) = v {
            extra.insert(k.into(), v);
        }
    };
    let image_type = find(&["AcqInfo", "ImageType"]);
    let exposure_s = find(&["AcqInfo", "CameraSettings", "ExpTime"]).and_then(|t| duration_s(&t));
    let magnification = find(&["AcqInfo", "ObjectiveMagnification"])
        .and_then(|t| leading_number(&t))
        .filter(|v| *v > 0.0);
    let pixel_size_um = find(&["AcqInfo", "FullResPixelSpacing"])
        .filter(|t| t.ends_with("µm") || t.ends_with(" um"))
        .and_then(|t| leading_number(&t))
        .filter(|v| *v > 0.0);
    let num = |p: &[&str]| find(p).and_then(|t| leading_number(&t)).map(|v| json!(v));
    put("image_type", image_type.clone().map(Value::from));
    put(
        "well",
        find(&["AcqInfo", "Location", "WellorSector"]).map(Value::from),
    );
    put(
        "image_in_well",
        num(&["AcqInfo", "Location", "UnityBasedImageIndex"]),
    );
    put(
        "images_per_well",
        num(&["AcqInfo", "Location", "ImagesPerWellorSector"]),
    );
    put(
        "focus_position_um",
        num(&["AcqInfo", "Location", "FocusPosition"]),
    );
    put(
        "camera_gain",
        num(&["AcqInfo", "CameraSettings", "ExpGain"]),
    );
    put(
        "gantry_temperature_c",
        num(&["AcqInfo", "GantryTemperature"]),
    );
    put(
        "drawer_temperature_c",
        num(&["AcqInfo", "DrawerTemperature"]),
    );
    put("exposure_s", exposure_s.map(|v| json!(v)));
    put(
        "vessel_id",
        find(&["ScanInfo", "VesselID"]).map(Value::from),
    );
    put(
        "scan_type",
        find(&["ScanInfo", "ScanType"]).map(Value::from),
    );
    put(
        "scan_time",
        find(&["ScanInfo", "ScanDateTime"]).map(Value::from),
    );
    put(
        "labware",
        find(&["ScanInfo", "Labware", "Name"]).map(Value::from),
    );
    put(
        "optical_module",
        find(&["DeviceInfo", "OpticalModule", "Name"]).map(Value::from),
    );
    put(
        "firmware_version",
        find(&["FirmwareInfo", "Version"]).map(Value::from),
    );
    let acquired_at = find(&["AcqInfo", "TimeStamp"]).and_then(|t| us_datetime(&t));
    Some(IncucyteInfo {
        pixel_size_um,
        magnification,
        image_type,
        exposure_s,
        acquired_at,
        hardware: find(&["DeviceInfo", "HardwareName"]),
        serial: find(&["DeviceInfo", "ControllerSerialNumber"]),
        camera: find(&["DeviceInfo", "Camera", "Model"]),
        extra,
    })
}

impl TiffDataset {
    /// Incucyte: settings from the metadata page, which is neither an image nor a level; the
    /// export time in tag 306 is not the acquisition time.
    pub(crate) fn apply_incucyte(&mut self) {
        let pages = self.main().ifds.len();
        let meta_page = (1..pages).find(|&p| {
            let ifd = &self.main().ifds[p];
            ifd.field(INCUCYTE_INFO_TAG).is_some()
                && ifd.uint(tags::IMAGE_WIDTH) == Some(1)
                && ifd.uint(tags::IMAGE_LENGTH) == Some(1)
        });
        let info = meta_page
            .and_then(|p| self.raw_field(p, INCUCYTE_INFO_TAG, MAX_INFO_BYTES))
            // Windows-1252: the bytes the XML uses (`µ`, `°`) are the same in Latin-1
            .map(|b| b.iter().map(|&c| char::from(c)).collect::<String>())
            .and_then(|x| parse_info(&x));
        if let Some(mp) = meta_page {
            self.attachments.retain(|a| a.page != mp);
            for s in &mut self.series {
                s.levels.retain(|l| l.page != Some(mp));
                s.info.pyramid_levels = 1 + s.levels.len() as u32;
            }
        }
        let page0 = self.main().ifds[0].clone();
        let mut s = ImagerSettings {
            vendor: Some("Sartorius".into()),
            software: text_value(&page0, tags::SOFTWARE),
            replaces_time: true,
            ..ImagerSettings::default()
        };
        s.origins
            .insert("vendor", "TIFF tag Software (`IncuCyte`)".into());
        if let Some(i) = &info {
            s.model.clone_from(&i.hardware);
            s.serial.clone_from(&i.serial);
            s.detector.clone_from(&i.camera);
            s.acquired_at.clone_from(&i.acquired_at);
            s.exposure_s = i.exposure_s;
            s.mode.clone_from(&i.image_type);
            s.extra.clone_from(&i.extra);
            s.origins
                .insert("model", "tag 60020 `DeviceInfo/HardwareName`".into());
            s.origins.insert(
                "serial",
                "tag 60020 `DeviceInfo/ControllerSerialNumber`".into(),
            );
            s.origins
                .insert("acquired_at", "tag 60020 `AcqInfo/TimeStamp`".into());
            if let Some(t) = i.exposure_s {
                s.params.push(Param {
                    name: "exposure_time",
                    value: json!(t),
                    unit: Some("s"),
                    from: "tag 60020 `AcqInfo/CameraSettings/ExpTime`".into(),
                });
            }
            if let Some(m) = i.magnification {
                s.params.push(Param {
                    name: "objective_magnification",
                    value: json!(m),
                    unit: None,
                    from: "tag 60020 `AcqInfo/ObjectiveMagnification`".into(),
                });
            }
        } else {
            s.notes.push("Incucyte export without its metadata page: no acquisition time (tag 306 holds the export time), pixel size or settings".into());
        }
        // `apply_imager` names the channel from `mode` and sets `extra.imager`; Incucyte keeps
        // its own key
        let extra = std::mem::take(&mut s.extra);
        self.apply_imager(&page0, s);
        for series in &mut self.series {
            let im = &mut series.info;
            if let Some(i) = &info {
                if let Some(px) = i.pixel_size_um {
                    im.physical_size = PhysicalSize::micrometres(Some(px), Some(px), None);
                }
                if let Some(m) = i.magnification {
                    im.objective = Some(ObjectiveInfo {
                        nominal_magnification: Some(m),
                        ..ObjectiveInfo::default()
                    });
                }
            }
            if !extra.is_empty() {
                im.extra
                    .insert("incucyte".into(), Value::Object(extra.clone()));
            }
        }
        if let Some(mp) = meta_page {
            self.notes.push(format!(
                "page {mp} (1 × 1 pixels) holds the Incucyte metadata; it is not an image"
            ));
        }
        self.set_provenance(&[
            ("images[].physical_size", Source::Inferred),
            ("images[].objective.nominal_magnification", Source::Inferred),
            ("images[].extra.incucyte", Source::Inferred),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_info() {
        let x = "<ImageInfo>\n<AcqInfo><ObjectiveMagnification>10</ObjectiveMagnification>\n<FullResPixelSpacing>1.244 \u{b5}m</FullResPixelSpacing>\n<ImageType>HD Phase Image</ImageType><Location><WellorSector>A1</WellorSector></Location><CameraSettings><ExpTime>0.46 msec</ExpTime></CameraSettings><TimeStamp>07/22/2020 13:33:44</TimeStamp></AcqInfo><DeviceInfo><HardwareName>IncuCyte S3</HardwareName></DeviceInfo></ImageInfo>\0";
        let i = parse_info(x).unwrap();
        assert_eq!(i.pixel_size_um, Some(1.244));
        assert_eq!(i.magnification, Some(10.0));
        assert_eq!(i.acquired_at.as_deref(), Some("2020-07-22T13:33:44"));
        assert_eq!(i.hardware.as_deref(), Some("IncuCyte S3"));
        assert_eq!(i.extra["well"], json!("A1"));
        assert!((i.exposure_s.unwrap() - 0.00046).abs() < 1e-12);
        assert!(parse_info("<ImageInfo><unclosed>").is_none());
        assert!(parse_info("nothing").is_none());
        assert_eq!(us_datetime("22/07/2020 13:33:44"), None);
    }
}
