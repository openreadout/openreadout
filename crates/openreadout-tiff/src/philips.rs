//! Philips TIFF whole-slide exports: page 0's `ImageDescription` is an XML `DataObject` tree
//! (`ObjectType="DPUfsImport"`) of DICOM-named attributes; the reduced pages are the pyramid,
//! pages described `Label…`/`Macro…` the associated images, which may also be base64 JPEGs in
//! the XML. See `docs/formats/tiff.md` § Philips TIFF.

use openreadout_core::Result;
use openreadout_core::model::{InstrumentInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use serde_json::{Map, Value, json};

use crate::dataset::{Attachment, Level, TiffDataset, text_value};
use crate::tags;

/// One `DataObject` of the metadata tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhilipsObject {
    /// `ObjectType` (`DPUfsImport`, `DPScannedImage`, `PixelDataRepresentation`).
    pub object_type: String,
    /// `Attribute` elements with a text value: `(Name, PMSVR type, value)`, file order.
    pub attributes: Vec<(String, String, String)>,
    /// `DataObject`s inside `IDataObjectArray` attributes: `(attribute Name, objects)`.
    pub arrays: Vec<(String, Vec<PhilipsObject>)>,
}

impl PhilipsObject {
    /// The value of attribute `name`.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, _, v)| v.as_str())
    }

    /// The objects of the array attribute `name`.
    pub fn array(&self, name: &str) -> &[PhilipsObject] {
        self.arrays
            .iter()
            .find(|(n, _)| n == name)
            .map_or(&[], |(_, v)| v.as_slice())
    }

    /// Attributes as a JSON object (arrays of objects nested), names as stored.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("ObjectType".into(), Value::String(self.object_type.clone()));
        for (n, _, v) in &self.attributes {
            // image data (base64 JPEG) is left out: it is an attachment
            if n != "PIM_DP_IMAGE_DATA" {
                m.insert(n.clone(), Value::String(v.clone()));
            }
        }
        for (n, objs) in &self.arrays {
            m.insert(
                n.clone(),
                Value::Array(objs.iter().map(PhilipsObject::to_json).collect()),
            );
        }
        Value::Object(m)
    }
}

fn parse_object(node: roxmltree::Node<'_, '_>, depth: u32) -> PhilipsObject {
    let mut o = PhilipsObject {
        object_type: node.attribute("ObjectType").unwrap_or_default().to_string(),
        ..PhilipsObject::default()
    };
    if depth > 16 {
        return o;
    }
    for a in node
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "Attribute")
    {
        let name = a.attribute("Name").unwrap_or_default().to_string();
        let kind = a.attribute("PMSVR").unwrap_or_default().to_string();
        let objs: Vec<PhilipsObject> = a
            .descendants()
            .filter(|n| {
                n.is_element()
                    && n.tag_name().name() == "DataObject"
                    && n.parent().is_some_and(|p| p.tag_name().name() == "Array")
                    && n.parent().and_then(|p| p.parent()).is_some_and(|p| p == a)
            })
            .map(|n| parse_object(n, depth + 1))
            .collect();
        if kind == "IDataObjectArray" || !objs.is_empty() {
            o.arrays.push((name, objs));
        } else {
            o.attributes
                .push((name, kind, a.text().unwrap_or_default().trim().to_string()));
        }
    }
    o
}

/// Parse page 0's description; `None` unless its root is a `DataObject` of type `DPUfsImport`.
pub fn parse_philips(desc: &str) -> Option<PhilipsObject> {
    let start = desc.find('<')?;
    let doc = roxmltree::Document::parse(desc[start..].trim_end_matches('\0')).ok()?;
    let root = doc.root_element();
    (root.tag_name().name() == "DataObject" && root.attribute("ObjectType") == Some("DPUfsImport"))
        .then(|| parse_object(root, 0))
}

/// Values of an `IStringArray` / `IDoubleArray` attribute: `"a" "b"` (quotes optional).
pub fn string_array(v: &str) -> Vec<String> {
    let t = v.trim();
    if !t.contains('"') {
        return t.split_whitespace().map(str::to_string).collect();
    }
    t.split('"')
        .enumerate()
        .filter(|(i, _)| i % 2 == 1)
        .map(|(_, s)| s.to_string())
        .collect()
}

/// `DICOM_PIXEL_SPACING` (millimetres, row then column) → (x, y) in micrometres.
pub fn pixel_spacing_um(v: &str) -> Option<(f64, f64)> {
    let a: Vec<f64> = string_array(v)
        .iter()
        .filter_map(|s| s.parse().ok())
        .filter(|x: &f64| x.is_finite() && *x > 0.0)
        .collect();
    match a[..] {
        [row, col, ..] => Some((col * 1000.0, row * 1000.0)),
        [one] => Some((one * 1000.0, one * 1000.0)),
        [] => None,
    }
}

/// `DICOM_ACQUISITION_DATETIME` (`YYYYMMDDhhmmss[.ffffff]`) → ISO 8601 (no zone).
pub fn dicom_datetime(v: &str) -> Option<String> {
    let t = v.trim();
    let (main, frac) = t.split_once('.').unwrap_or((t, ""));
    if main.len() < 8 || !main.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let g = |a: usize, b: usize| main.get(a..b).unwrap_or("00");
    let (year, mo, day) = (g(0, 4), g(4, 6), g(6, 8));
    if !(1..=12).contains(&mo.parse::<u32>().ok()?) || !(1..=31).contains(&day.parse::<u32>().ok()?)
    {
        return None;
    }
    let mut iso = format!("{year}-{mo}-{day}T{}:{}:{}", g(8, 10), g(10, 12), g(12, 14));
    let frac = frac.trim_end_matches('0');
    if !frac.is_empty() && frac.chars().all(|c| c.is_ascii_digit()) {
        iso.push('.');
        iso.push_str(frac);
    }
    Some(iso)
}

/// Standard base64 (with or without padding and line breaks); `None` on any other byte.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// The scanned image of `image_type` (`WSI`, `LABELIMAGE`, `MACROIMAGE`).
pub fn scanned_image<'a>(root: &'a PhilipsObject, image_type: &str) -> Option<&'a PhilipsObject> {
    root.array("PIM_DP_SCANNED_IMAGES")
        .iter()
        .find(|o| o.attr("PIM_DP_IMAGE_TYPE") == Some(image_type))
}

impl TiffDataset {
    /// Philips TIFF: page 0 and its reduced pages are one pyramidal image; `Label`/`Macro`
    /// pages and the XML's label and macro JPEGs are attachments; metadata from the XML tree.
    pub(crate) fn build_philips(&mut self, root: &PhilipsObject) -> Result<()> {
        let base_dims = self.page_dims(0);
        let mut levels = self.philips_levels(base_dims);
        let wsi = scanned_image(root, "WSI");
        let wsi_attr = |k: &str| wsi.and_then(|w| w.attr(k)).map(str::trim);
        let top = |k: &str| root.attr(k).map(str::trim).filter(|v| !v.is_empty());
        let reps = representation_spacings(wsi);
        fit_levels(&mut levels, &reps, base_dims);
        let mut info = self.push_slide(None, &[0], &[], levels)?;
        if let Some((x, y)) = wsi_attr("DICOM_PIXEL_SPACING").and_then(pixel_spacing_um) {
            info.physical_size = PhysicalSize::micrometres(Some(x), Some(y), None);
        }
        let page0 = self.main().ifds[0].clone();
        let versions = top("DICOM_SOFTWARE_VERSIONS").map(string_array);
        info.instrument = Some(InstrumentInfo {
            manufacturer: top("DICOM_MANUFACTURER").map(str::to_string),
            model: top("DICOM_MANUFACTURERS_MODEL_NAME").map(str::to_string),
            software: text_value(&page0, tags::SOFTWARE),
            software_version: versions.as_ref().and_then(|v| v.first().cloned()),
            ..InstrumentInfo::default()
        });
        info.acquired_at = top("DICOM_ACQUISITION_DATETIME").and_then(dicom_datetime);
        let mut ph = Map::new();
        if let Some(b) = top("PIM_DP_UFS_BARCODE").and_then(base64_decode) {
            ph.insert(
                "barcode".into(),
                json!(String::from_utf8_lossy(&b).trim().to_string()),
            );
        }
        for (k, key) in [
            ("device_serial_number", "DICOM_DEVICE_SERIAL_NUMBER"),
            ("interface_version", "PIM_DP_UFS_INTERFACE_VERSION"),
        ] {
            if let Some(v) = top(key) {
                ph.insert(k.into(), json!(v));
            }
        }
        if let Some(v) = versions {
            ph.insert("software_versions".into(), json!(v));
        }
        if let Some(d) = wsi_attr("DICOM_DERIVATION_DESCRIPTION") {
            ph.insert("derivation".into(), json!(d));
        }
        if !reps.is_empty() {
            ph.insert(
                "representations".into(),
                json!(
                    reps.iter()
                        .map(|(k, (x, y))| json!({"number": k, "pixel_size_um": [x, y]}))
                        .collect::<Vec<_>>()
                ),
            );
        }
        ph.insert("metadata".into(), root.to_json());
        info.extra.insert("philips".into(), Value::Object(ph));
        for (kind, name) in [("LABELIMAGE", "label"), ("MACROIMAGE", "macro")] {
            if let Some(b) = scanned_image(root, kind)
                .and_then(|o| o.attr("PIM_DP_IMAGE_DATA"))
                .and_then(base64_decode)
                .filter(|b| b.starts_with(&[0xFF, 0xD8]))
            {
                self.embedded.push((name.to_string(), b));
            }
        }
        let sparse = self.main().ifds.first().is_some_and(|ifd| {
            ifd.uints(tags::TILE_BYTE_COUNTS)
                .is_some_and(|v| v.contains(&0))
        });
        if sparse {
            self.notes.push("Philips TIFF: tiles outside the scanned regions are not stored; they are returned white, as the scanner renders them in its downsampled levels".into());
        }
        self.replace_last(info);
        self.set_provenance(&[
            ("images[].physical_size", Source::PriorArt),
            ("images[].pyramid_levels", Source::PriorArt),
            ("images[].resolution_levels", Source::PriorArt),
            ("images[].instrument", Source::Inferred),
            ("images[].acquired_at", Source::Inferred),
        ]);
        Ok(())
    }

    /// The pages after page 0: tiled pages smaller than it are its pyramid levels (in page
    /// order), `Label…`/`Macro…` pages and anything else are attachments.
    fn philips_levels(&mut self, base_dims: (u32, u32)) -> Vec<Level> {
        let mut levels = Vec::new();
        for p in 1..self.main().ifds.len() {
            let ifd = &self.main().ifds[p];
            let desc = ifd.text(tags::IMAGE_DESCRIPTION).unwrap_or_default();
            let desc = desc.trim_start();
            let tiled = ifd.field(tags::TILE_WIDTH).is_some();
            let d = self.page_dims(p);
            let role = if desc.starts_with("Label") {
                Some("label")
            } else if desc.starts_with("Macro") {
                Some("macro")
            } else if tiled && d.0 < base_dims.0 && d.1 < base_dims.1 {
                None
            } else {
                Some("associated")
            };
            match role {
                Some(r) => self.attachments.push(Attachment {
                    role: r.into(),
                    page: p,
                }),
                None => levels.push(Level {
                    width: d.0,
                    height: d.1,
                    page: Some(p),
                    sub_ifd: None,
                    pages: vec![p],
                }),
            }
        }
        levels
    }
}

/// The pixel spacing (µm) of each pixel-data representation of the WSI image, by number.
fn representation_spacings(wsi: Option<&PhilipsObject>) -> Vec<(u32, (f64, f64))> {
    wsi.map(|w| w.array("PIIM_PIXEL_DATA_REPRESENTATION_SEQUENCE"))
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            let k = r
                .attr("PIIM_PIXEL_DATA_REPRESENTATION_NUMBER")?
                .trim()
                .parse()
                .ok()?;
            Some((k, pixel_spacing_um(r.attr("DICOM_PIXEL_SPACING")?)?))
        })
        .collect()
}

/// Level k is representation k: its pixel spacing over level 0's is the downsample, and the
/// level's image is the base size over the downsample (the TIFF page is padded to whole
/// tiles, so its size is not the level's).
fn fit_levels(levels: &mut [Level], reps: &[(u32, (f64, f64))], base_dims: (u32, u32)) {
    let rep = |k: u32| reps.iter().find(|(n, _)| *n == k).map(|(_, s)| *s);
    let Some((x0, y0)) = rep(0) else {
        return;
    };
    // spacings are rounded to 6 significant digits: snap near-integer ratios
    let ratio = |a: f64, b: f64| {
        let r = a / b;
        if (r - r.round()).abs() < 1e-3 * r {
            r.round()
        } else {
            r
        }
    };
    for (k, lv) in levels.iter_mut().enumerate() {
        if let Some((x, y)) = rep(k as u32 + 1) {
            let (dx, dy) = (ratio(x, x0), ratio(y, y0));
            if dx >= 1.0 && dy >= 1.0 {
                let w = (f64::from(base_dims.0) / dx).floor() as u32;
                let h = (f64::from(base_dims.1) / dy).floor() as u32;
                lv.width = w.clamp(1, lv.width);
                lv.height = h.clamp(1, lv.height);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESC: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<DataObject ObjectType="DPUfsImport">
<Attribute Name="DICOM_MANUFACTURER" Group="0x0008" Element="0x0070" PMSVR="IString">Hamamatsu</Attribute>
<Attribute Name="PIM_DP_SCANNED_IMAGES" Group="0x301D" Element="0x1003" PMSVR="IDataObjectArray">
<Array>
<DataObject ObjectType="DPScannedImage">
<Attribute Name="PIM_DP_IMAGE_TYPE" Group="0x301D" Element="0x1004" PMSVR="IString">WSI</Attribute>
<Attribute Name="DICOM_PIXEL_SPACING" Group="0x0028" Element="0x0030" PMSVR="IDoubleArray">&quot;0.000226891&quot; &quot;0.000226907&quot;</Attribute>
<Attribute Name="PIIM_PIXEL_DATA_REPRESENTATION_SEQUENCE" Group="0x1001" Element="0x8B01" PMSVR="IDataObjectArray">
<Array>
<DataObject ObjectType="PixelDataRepresentation">
<Attribute Name="PIIM_PIXEL_DATA_REPRESENTATION_NUMBER" Group="0x1001" Element="0x8B02" PMSVR="IUInt16">0</Attribute>
</DataObject>
</Array>
</Attribute>
</DataObject>
<DataObject ObjectType="DPScannedImage">
<Attribute Name="PIM_DP_IMAGE_TYPE" Group="0x301D" Element="0x1004" PMSVR="IString">LABELIMAGE</Attribute>
<Attribute Name="PIM_DP_IMAGE_DATA" Group="0x301D" Element="0x1005" PMSVR="IString">/9j/4A==</Attribute>
</DataObject>
</Array>
</Attribute>
<Attribute Name="PIM_DP_UFS_BARCODE" Group="0x301D" Element="0x1002" PMSVR="IString">MzMxMTk0MA==</Attribute>
</DataObject>"#;

    #[test]
    fn tree_values_and_helpers() {
        let r = parse_philips(DESC).unwrap();
        assert_eq!(r.attr("DICOM_MANUFACTURER"), Some("Hamamatsu"));
        let wsi = scanned_image(&r, "WSI").unwrap();
        let (x, y) = pixel_spacing_um(wsi.attr("DICOM_PIXEL_SPACING").unwrap()).unwrap();
        assert!((x - 0.226_907).abs() < 1e-9 && (y - 0.226_891).abs() < 1e-9);
        assert_eq!(
            wsi.array("PIIM_PIXEL_DATA_REPRESENTATION_SEQUENCE").len(),
            1
        );
        let label = scanned_image(&r, "LABELIMAGE").unwrap();
        assert_eq!(
            base64_decode(label.attr("PIM_DP_IMAGE_DATA").unwrap()).unwrap(),
            vec![0xFF, 0xD8, 0xFF, 0xE0]
        );
        assert_eq!(
            base64_decode(r.attr("PIM_DP_UFS_BARCODE").unwrap()).unwrap(),
            b"3311940"
        );
        assert!(
            r.to_json()["PIM_DP_SCANNED_IMAGES"][1]
                .get("PIM_DP_IMAGE_DATA")
                .is_none()
        );
        assert_eq!(
            dicom_datetime("20210609111602.000000").as_deref(),
            Some("2021-06-09T11:16:02")
        );
        assert!(dicom_datetime("junk").is_none());
        assert_eq!(string_array("\"1.8\" \"R51\""), vec!["1.8", "R51"]);
        assert!(parse_philips("<DataObject ObjectType=\"Other\"/>").is_none());
        assert!(base64_decode("a$").is_none());
    }
}
