//! The XML documents we normalize: image properties, channel settings, frame properties.
//! Element names are matched by local name (the namespace prefixes vary between FluoView
//! versions). Vocabulary and meaning: `docs/formats/oir.md`.

use roxmltree::{Document, Node};

/// Plane geometry from a frame-properties (or image-definition) document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameGeometry {
    pub width: u32,
    pub height: u32,
    /// Bytes per sample.
    pub depth: u32,
    /// Significant bits per sample.
    pub bit_count: u32,
    /// `GlayScale` (sic) or `RGB`, as written.
    pub color_type: String,
}

/// One frame's properties: its name (`l001z001_0_1`) and axis positions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameRecord {
    pub name: String,
    pub created: Option<String>,
    pub geometry: FrameGeometry,
    /// `(axis type, position)`: `TIMELAPSE` in ms, `ZSTACK` in µm, `LAMBDA` in nm.
    pub positions: Vec<(String, f64)>,
}

/// An acquisition axis from `imageProperties/imageInfo/axis`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxisDesc {
    /// `ZSTACK`, `TIMELAPSE`, `LAMBDA`, ...
    pub axis: String,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub step: Option<f64>,
    pub max_size: Option<u32>,
}

/// A channel from `imageProperties` (`channel` elements with an `id`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelDesc {
    pub id: String,
    pub order: i64,
    pub name: Option<String>,
    /// Detection band start/end (nm).
    pub band_start_nm: Option<f64>,
    pub band_end_nm: Option<f64>,
    /// Laser settings the channel uses (`laserDataId`).
    pub laser_ids: Vec<String>,
}

/// A laser line (`imagingMainLaser`): id, wavelength and whether it was on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LaserLine {
    pub id: String,
    pub wavelength_nm: Option<f64>,
    pub enabled: bool,
}

/// The objective (`objectiveLens`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectiveDesc {
    pub name: Option<String>,
    pub magnification: Option<f64>,
    pub numerical_aperture: Option<f64>,
    pub immersion: Option<String>,
    pub working_distance_mm: Option<f64>,
}

/// What we take from the `lsmimage:imageProperties` document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageProperties {
    pub created: Option<String>,
    pub system_name: Option<String>,
    pub system_version: Option<String>,
    pub microscope: Option<String>,
    /// Channels, first occurrence per id, stably sorted by `order`.
    pub channels: Vec<ChannelDesc>,
    pub axes: Vec<AxisDesc>,
    /// Pixel size (first `length` element with `x`/`y`), in `pixel_unit`.
    pub pixel_length_x: Option<f64>,
    pub pixel_length_y: Option<f64>,
    /// `MICRO_METER`, ...
    pub pixel_unit: Option<String>,
    pub objective: Option<ObjectiveDesc>,
    pub lasers: Vec<LaserLine>,
}

/// What we take from an `lsmimage:lsmChannel` document (per-channel detector and dye).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelSettings {
    pub id: String,
    pub detector: Option<String>,
    pub dye_name: Option<String>,
    pub dye_excitation_nm: Option<f64>,
    pub dye_emission_nm: Option<f64>,
    pub bit_count: Option<u32>,
}

fn local<'a>(n: &Node<'a, '_>) -> &'a str {
    n.tag_name().name()
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && local(c) == name)
}

fn child_text(n: Node<'_, '_>, name: &str) -> Option<String> {
    child(n, name)
        .and_then(|c| c.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn child_f64(n: Node<'_, '_>, name: &str) -> Option<f64> {
    child_text(n, name)
        .and_then(|t| t.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

fn desc_text(n: Node<'_, '_>, name: &str) -> Option<String> {
    n.descendants()
        .find(|d| d.is_element() && local(d) == name)
        .and_then(|d| d.text())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn desc_f64(n: Node<'_, '_>, name: &str) -> Option<f64> {
    desc_text(n, name)
        .and_then(|t| t.parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// Parse an XML document (ASCII text); `None` if it is not well formed.
pub fn parse(text: &str) -> Option<Document<'_>> {
    Document::parse(text).ok()
}

/// Geometry from the first `imageDefinition`-like element holding `width`/`height`.
pub fn geometry(root: Node<'_, '_>) -> FrameGeometry {
    let def = root
        .descendants()
        .find(|d| d.is_element() && local(d) == "imageDefinition" && child(*d, "width").is_some())
        .unwrap_or(root);
    let num = |name: &str| {
        child_text(def, name)
            .and_then(|t| t.parse::<u32>().ok())
            .unwrap_or(0)
    };
    FrameGeometry {
        width: num("width"),
        height: num("height"),
        depth: num("depth"),
        bit_count: num("bitCounts"),
        color_type: child_text(def, "colorType").unwrap_or_default(),
    }
}

/// A frame-properties document.
pub fn frame_record(text: &str) -> Option<FrameRecord> {
    let doc = parse(text)?;
    let root = doc.root_element();
    let general = child(root, "general");
    let mut name = general
        .and_then(|g| child_text(g, "name"))
        .unwrap_or_default();
    if name.is_empty() {
        name = root.attribute("id").unwrap_or_default().to_string();
    }
    let positions = root
        .descendants()
        .filter(|d| d.is_element() && local(d) == "axisValue")
        .filter_map(|a| Some((child_text(a, "axisType")?, child_f64(a, "position")?)))
        .collect();
    Some(FrameRecord {
        name,
        created: general.and_then(|g| child_text(g, "creationDateTime")),
        geometry: geometry(root),
        positions,
    })
}

/// The `lsmimage:imageProperties` document.
pub fn image_properties(text: &str) -> Option<ImageProperties> {
    let doc = parse(text)?;
    let root = doc.root_element();
    let mut p = ImageProperties {
        created: desc_text(root, "creationDateTime"),
        ..ImageProperties::default()
    };
    if let Some(sys) = child(root, "system") {
        p.system_name = child_text(sys, "systemName");
        p.system_version = child_text(sys, "systemVersion");
    }
    if let Some(m) = child(root, "microscope") {
        p.microscope = child_text(m, "name");
    }
    // channels: first occurrence per id, stable sort by `order`
    let mut seen = std::collections::HashSet::new();
    for c in root
        .descendants()
        .filter(|d| d.is_element() && local(d) == "channel")
    {
        let Some(id) = c.attribute("id").filter(|i| !i.is_empty()) else {
            continue;
        };
        if !seen.insert(id.to_string()) {
            continue;
        }
        p.channels.push(ChannelDesc {
            id: id.to_string(),
            order: c
                .attribute("order")
                .and_then(|o| o.parse().ok())
                .unwrap_or(0),
            name: child_text(c, "name"),
            band_start_nm: desc_f64(c, "startWavelength"),
            band_end_nm: desc_f64(c, "endWavelength"),
            laser_ids: c
                .children()
                .filter(|d| d.is_element() && local(d) == "laserDataId")
                .filter_map(|d| d.text().map(|t| t.trim().to_string()))
                .collect(),
        });
    }
    p.channels.sort_by_key(|c| c.order);
    let axis_desc = |a: Node<'_, '_>| {
        Some(AxisDesc {
            axis: child_text(a, "axis")?,
            start: child_f64(a, "startPosition"),
            end: child_f64(a, "endPosition"),
            step: child_f64(a, "step"),
            max_size: child_text(a, "maxSize").and_then(|t| t.parse().ok()),
        })
    };
    // The acquired axes are listed under `imageInfo`; the snapshot written after the first frame
    // lacks them, and then the enabled acquisition-settings axes are the declaration.
    if let Some(info) = child(root, "imageInfo") {
        p.axes = info
            .children()
            .filter(|d| d.is_element() && local(d) == "axis")
            .filter_map(axis_desc)
            .collect();
    }
    if p.axes.is_empty() {
        for a in root.descendants().filter(|d| {
            d.is_element()
                && local(d) == "axis"
                && d.attribute("enable") == Some("true")
                && d.attribute("paramEnable") != Some("false")
        }) {
            if let Some(ad) = axis_desc(a)
                && !p.axes.iter().any(|x| x.axis == ad.axis)
            {
                p.axes.push(ad);
            }
        }
    }
    if let Some(len) = root
        .descendants()
        .find(|d| d.is_element() && local(d) == "length" && child(*d, "x").is_some())
    {
        p.pixel_length_x = child_f64(len, "x");
        p.pixel_length_y = child_f64(len, "y");
        p.pixel_unit = len
            .parent_element()
            .and_then(|par| child(par, "pixelUnit"))
            .and_then(|u| child_text(u, "x"));
    }
    if let Some(o) = root
        .descendants()
        .find(|d| d.is_element() && local(d) == "objectiveLens")
    {
        p.objective = Some(ObjectiveDesc {
            name: child_text(o, "name"),
            magnification: child_f64(o, "magnification"),
            numerical_aperture: child_f64(o, "naValue"),
            immersion: child_text(o, "immersion"),
            working_distance_mm: child_f64(o, "wdValue"),
        });
    }
    p.lasers = root
        .descendants()
        .filter(|d| d.is_element() && local(d) == "imagingMainLaser")
        .filter_map(|l| {
            Some(LaserLine {
                id: l.attribute("id")?.to_string(),
                wavelength_nm: child_f64(l, "wavelength"),
                enabled: l.attribute("enable") == Some("true"),
            })
        })
        .collect();
    Some(p)
}

/// An `lsmimage:lsmChannel` document.
pub fn channel_settings(text: &str) -> Option<ChannelSettings> {
    let doc = parse(text)?;
    let root = doc.root_element();
    let dye = root
        .descendants()
        .find(|d| d.is_element() && local(d) == "dyeData");
    Some(ChannelSettings {
        id: root.attribute("id")?.to_string(),
        detector: child_text(root, "deviceName"),
        dye_name: child_text(root, "dyeName").or_else(|| dye.and_then(|d| child_text(d, "name"))),
        dye_excitation_nm: dye.and_then(|d| child_f64(d, "excitationWavelength")),
        dye_emission_nm: dye.and_then(|d| child_f64(d, "emissionWavelength")),
        bit_count: child_text(root, "bitCount").and_then(|t| t.parse().ok()),
    })
}

/// The `lut:name` of a LUT document (`Gray`, `Red`, ...).
pub fn lut_name(text: &str) -> Option<String> {
    let doc = parse(text)?;
    child_text(doc.root_element(), "name")
}

/// Display colour for a LUT that is a plain colour name.
pub fn lut_color(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "red" => Some("#FF0000"),
        "green" => Some("#00FF00"),
        "blue" => Some("#0000FF"),
        "cyan" => Some("#00FFFF"),
        "magenta" => Some("#FF00FF"),
        "yellow" => Some("#FFFF00"),
        "gray" | "grey" | "white" => Some("#FFFFFF"),
        _ => None,
    }
}

/// Factor from a `pixelUnit` word to micrometres.
pub fn unit_to_um(unit: Option<&str>) -> Option<f64> {
    match unit.unwrap_or("MICRO_METER") {
        "MICRO_METER" => Some(1.0),
        "NANO_METER" => Some(1e-3),
        "MILLI_METER" => Some(1e3),
        "METER" => Some(1e6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROPS: &str = r#"<?xml version="1.0" encoding="ASCII"?>
<lsmimage:imageProperties xmlns:lsmimage="a" xmlns:base="b" xmlns:commonimage="c" xmlns:commonphase="d" xmlns:commonparam="e" xmlns:opticalelement="f">
  <commonimage:general><base:creationDateTime>2024-12-02T09:52:10.944+08:00</base:creationDateTime></commonimage:general>
  <commonimage:system><base:systemName>FV4000</base:systemName><base:systemVersion>3.1.1.67</base:systemVersion></commonimage:system>
  <commonimage:microscope><base:name>IX83</base:name></commonimage:microscope>
  <commonimage:imageInfo>
    <commonimage:phase id="p"><commonphase:group id="g">
      <commonphase:channel id="B" order="3"><commonphase:name>CH3</commonphase:name></commonphase:channel>
      <commonphase:channel id="A" order="2">
        <x><opticalelement:startWavelength>500</opticalelement:startWavelength><opticalelement:endWavelength>540</opticalelement:endWavelength></x>
        <commonphase:name>CH2</commonphase:name>
        <commonphase:length><commonparam:x>2.5</commonparam:x><commonparam:y>2.5</commonparam:y></commonphase:length>
        <commonphase:pixelUnit><commonphase:x>MICRO_METER</commonphase:x></commonphase:pixelUnit>
        <commonphase:laserDataId>L488</commonphase:laserDataId>
      </commonphase:channel>
    </commonphase:group></commonimage:phase>
    <commonimage:axis><commonparam:axis>ZSTACK</commonparam:axis><commonparam:step>3.93</commonparam:step><commonparam:maxSize>8</commonparam:maxSize></commonimage:axis>
  </commonimage:imageInfo>
  <other><commonphase:channel id="A" order="9"/></other>
  <commonimage:objectiveLens><opticalelement:name>UPLXAPO20X</opticalelement:name><opticalelement:magnification>20.0</opticalelement:magnification><opticalelement:naValue>0.8</opticalelement:naValue><opticalelement:immersion>DRY</opticalelement:immersion></commonimage:objectiveLens>
  <lsmimage:imagingMainLaser enable="true" id="L488"><commonimage:wavelength>488</commonimage:wavelength></lsmimage:imagingMainLaser>
</lsmimage:imageProperties>"#;

    #[test]
    fn image_properties_subset() {
        let p = image_properties(PROPS).unwrap();
        assert_eq!(p.system_name.as_deref(), Some("FV4000"));
        assert_eq!(p.microscope.as_deref(), Some("IX83"));
        let ids: Vec<&str> = p.channels.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["A", "B"]);
        assert_eq!(p.channels[0].band_start_nm, Some(500.0));
        assert_eq!(p.channels[0].laser_ids, ["L488"]);
        assert_eq!(p.axes[0].axis, "ZSTACK");
        assert_eq!(p.axes[0].max_size, Some(8));
        assert_eq!(p.pixel_length_x, Some(2.5));
        assert_eq!(p.pixel_unit.as_deref(), Some("MICRO_METER"));
        assert_eq!(p.objective.as_ref().unwrap().numerical_aperture, Some(0.8));
        assert!(p.lasers[0].enabled);
        assert_eq!(p.lasers[0].wavelength_nm, Some(488.0));
    }

    #[test]
    fn frame_properties() {
        let f = frame_record(r#"<?xml version="1.0"?><lsmframe:frameProperties xmlns:lsmframe="a" xmlns:commonframe="b" xmlns:base="c" id="l001z002_0_1">
  <commonframe:general><base:name>l001z002_0_1</base:name></commonframe:general>
  <commonframe:imageDefinition><base:colorType>GlayScale</base:colorType><base:width>512</base:width><base:height>256</base:height><base:depth>2</base:depth><base:bitCounts>12</base:bitCounts></commonframe:imageDefinition>
  <commonframe:axisValue><commonframe:axisType>ZSTACK</commonframe:axisType><commonframe:position>4573.63</commonframe:position></commonframe:axisValue>
</lsmframe:frameProperties>"#).unwrap();
        assert_eq!(f.name, "l001z002_0_1");
        assert_eq!(
            (
                f.geometry.width,
                f.geometry.height,
                f.geometry.depth,
                f.geometry.bit_count
            ),
            (512, 256, 2, 12)
        );
        assert_eq!(f.positions, vec![("ZSTACK".to_string(), 4573.63)]);
    }

    #[test]
    fn units_and_colors() {
        assert_eq!(unit_to_um(Some("NANO_METER")), Some(1e-3));
        assert_eq!(unit_to_um(None), Some(1.0));
        assert_eq!(unit_to_um(Some("FURLONG")), None);
        assert_eq!(lut_color("Red"), Some("#FF0000"));
        assert_eq!(lut_color("Fire"), None);
    }
}
