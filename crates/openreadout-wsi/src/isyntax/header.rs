//! The XML header of an iSyntax file: `DataObject` elements holding `Attribute` elements (a name,
//! a DICOM-style group and element, a value type and a value; arrays of data objects nest). The
//! header ends at the first byte 0x04. `docs/formats/philips-isyntax.md` § Header.

use std::collections::BTreeMap;

use base64::Engine as _;
use openreadout_core::{Error, Result};
use roxmltree::Node;
use serde_json::{Map, Value, json};

use super::FORMAT_ID;

/// Attributes whose value is base64 data (left out of the metadata tree).
const BASE64_ATTRIBUTES: [&str; 3] = [
    "UFS_IMAGE_BLOCK_HEADER_TABLE",
    "PIM_DP_IMAGE_DATA",
    "DICOM_ICCPROFILE",
];

/// One `UFS_IMAGE_DIMENSION_RANGE`: `start step end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DimRange {
    /// First value.
    pub start: i64,
    /// Step between values.
    pub step: i64,
    /// Last value.
    pub end: i64,
}

impl DimRange {
    /// Number of values in the range (`(end + step − start) / step`, a step of 0 counts as 1).
    pub fn count(&self) -> i64 {
        let step = if self.step == 0 { 1 } else { self.step };
        (self.end + self.step - self.start) / step
    }
}

/// One `UFSImageBlockHeaderTemplate`: the size of a code block and what it holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockTemplate {
    /// Ranges of the five block dimensions (x, y, colour component, scale, wavelet coefficient).
    pub ranges: Vec<DimRange>,
    /// `UFS_IMAGE_BLOCK_COMPRESSION_METHOD`.
    pub compression: Option<String>,
}

/// One `UFSImageDimension` of the general header.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dimension {
    /// `UFS_IMAGE_DIMENSION_NAME` (`x`, `y`, `component`, `scale`, `waveletcoef`).
    pub name: String,
    /// `UFS_IMAGE_DIMENSION_UNIT` (`MicroMeter`).
    pub unit: Option<String>,
    /// `UFS_IMAGE_DIMENSION_SCALE_FACTOR`: size of one step in `unit`.
    pub scale_factor: Option<f64>,
}

/// One `DPScannedImage`: the slide (`WSI`), the label or the macro image.
#[derive(Debug, Clone, Default)]
pub struct ScannedImage {
    /// `PIM_DP_IMAGE_TYPE`: `WSI`, `LABELIMAGE` or `MACROIMAGE`.
    pub kind: String,
    /// Single-valued attributes by name (base64 values left out).
    pub attributes: BTreeMap<String, String>,
    /// `PIM_DP_IMAGE_DATA`, decoded (a JPEG for the label and macro images).
    pub image_data: Option<Vec<u8>>,
    /// `DICOM_ICCPROFILE`, decoded.
    pub icc_profile: Option<Vec<u8>>,
    /// `UFS_IMAGE_BLOCK_HEADER_TABLE`, decoded.
    pub block_table: Option<Vec<u8>>,
    /// `UFS_IMAGE_BLOCK_HEADER_TEMPLATES`.
    pub templates: Vec<BlockTemplate>,
    /// `UFS_IMAGE_GENERAL_HEADERS` → `UFS_IMAGE_DIMENSIONS`.
    pub dimensions: Vec<Dimension>,
    /// `UFS_IMAGE_GENERAL_HEADERS` → `UFS_IMAGE_DIMENSION_RANGES`.
    pub ranges: Vec<DimRange>,
    /// `UFS_IMAGE_NUMBER_OF_BLOCKS`.
    pub number_of_blocks: Option<u64>,
    /// Whether the data model's cluster headers (`UFS_IMAGE_CLUSTER_HEADER_TABLE`) are present.
    pub has_cluster_table: bool,
}

/// The parsed header.
#[derive(Debug, Clone, Default)]
pub struct Header {
    /// Byte offset of the 0x04 that ends the XML.
    pub length: u64,
    /// Single-valued attributes of the file (`DPUfsImport`) by name.
    pub attributes: BTreeMap<String, String>,
    /// The scanned images, in file order.
    pub images: Vec<ScannedImage>,
    /// The whole header as a JSON tree (base64 values replaced by their length).
    pub tree: Value,
}

impl Header {
    /// The first scanned image of `kind`.
    pub fn image(&self, kind: &str) -> Option<&ScannedImage> {
        self.images.iter().find(|i| i.kind == kind)
    }

    /// A file-level attribute.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes.get(name).map(String::as_str)
    }

    /// `PIM_DP_UFS_INTERFACE_VERSION` as (major, minor): 5.x for the first data model, 100.x
    /// and later for the second.
    pub fn data_model(&self) -> Option<(u32, u32)> {
        let v = self.attribute("PIM_DP_UFS_INTERFACE_VERSION")?;
        let (a, b) = v.split_once('.').unwrap_or((v, "0"));
        Some((a.trim().parse().ok()?, b.trim().parse().unwrap_or(0)))
    }
}

fn bad(detail: impl Into<String>) -> Error {
    Error::corrupt(FORMAT_ID, format!("XML header: {}", detail.into()))
}

fn name_of<'a>(n: Node<'a, '_>) -> &'a str {
    n.attribute("Name").unwrap_or("")
}

fn is(n: Node<'_, '_>, tag: &str) -> bool {
    n.is_element() && n.tag_name().name() == tag
}

/// The `DataObject` children of an attribute's `Array`.
fn objects<'a, 'i>(attr: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    attr.children()
        .filter(|c| is(*c, "Array"))
        .flat_map(|a| a.children().filter(|c| is(*c, "DataObject")))
}

/// The `Attribute` children of a data object.
fn attributes<'a, 'i>(obj: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    obj.children().filter(|c| is(*c, "Attribute"))
}

fn text(n: Node<'_, '_>) -> String {
    n.children()
        .filter(Node::is_text)
        .filter_map(|t| t.text())
        .collect::<String>()
}

fn decode_b64(name: &str, s: &str) -> Result<Vec<u8>> {
    let clean: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    // A trailing '/' that breaks the length is dropped (as libisyntax does).
    base64::engine::general_purpose::STANDARD
        .decode(&clean)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(clean.trim_end_matches('/')))
        .or_else(|_| {
            base64::engine::general_purpose::STANDARD_NO_PAD.decode(clean.trim_end_matches('='))
        })
        .map_err(|e| bad(format!("{name} is not valid base64 ({e})")))
}

fn parse_range(s: &str) -> Option<DimRange> {
    let v: Vec<i64> = s
        .split_whitespace()
        .filter_map(|t| t.trim_matches('"').parse().ok())
        .collect();
    (v.len() == 3).then(|| DimRange {
        start: v[0],
        step: v[1],
        end: v[2],
    })
}

fn ranges_of(obj: Node<'_, '_>) -> Vec<DimRange> {
    attributes(obj)
        .filter(|a| name_of(*a) == "UFS_IMAGE_DIMENSION_RANGES")
        .flat_map(objects)
        .filter_map(|r| {
            attributes(r)
                .find(|a| name_of(*a) == "UFS_IMAGE_DIMENSION_RANGE")
                .and_then(|a| parse_range(&text(a)))
        })
        .collect()
}

fn to_json(obj: Node<'_, '_>) -> Value {
    let mut m = Map::new();
    if let Some(t) = obj.attribute("ObjectType") {
        m.insert("object_type".into(), json!(t));
    }
    for a in attributes(obj) {
        let name = name_of(a).to_string();
        let v = if a.children().any(|c| is(c, "Array")) {
            Value::Array(objects(a).map(to_json).collect())
        } else if BASE64_ATTRIBUTES.contains(&name.as_str()) {
            json!(format!("<base64, {} characters>", text(a).len()))
        } else {
            json!(text(a))
        };
        m.insert(name, v);
    }
    Value::Object(m)
}

fn scanned_image(obj: Node<'_, '_>) -> Result<ScannedImage> {
    let mut im = ScannedImage::default();
    for a in attributes(obj) {
        let name = name_of(a);
        match name {
            "PIM_DP_IMAGE_DATA" => im.image_data = Some(decode_b64(name, &text(a))?),
            "UFS_IMAGE_BLOCK_HEADER_TABLE" => im.block_table = Some(decode_b64(name, &text(a))?),
            "UFS_IMAGE_CLUSTER_HEADER_TABLE" => im.has_cluster_table = true,
            "DP_COLOR_MANAGEMENT" => {
                for cm in objects(a) {
                    if let Some(p) = attributes(cm).find(|x| name_of(*x) == "DICOM_ICCPROFILE") {
                        im.icc_profile = Some(decode_b64("DICOM_ICCPROFILE", &text(p))?);
                    }
                }
            }
            "UFS_IMAGE_BLOCK_HEADER_TEMPLATES" => {
                for t in objects(a) {
                    im.templates.push(BlockTemplate {
                        ranges: ranges_of(t),
                        compression: attributes(t)
                            .find(|x| name_of(*x) == "UFS_IMAGE_BLOCK_COMPRESSION_METHOD")
                            .map(|x| text(x).trim().to_string()),
                    });
                }
            }
            "UFS_IMAGE_GENERAL_HEADERS" => {
                // One general header per image in the corpus file; the first one is used.
                if let Some(g) = objects(a).next() {
                    im.ranges = ranges_of(g);
                    for ga in attributes(g) {
                        match name_of(ga) {
                            "UFS_IMAGE_NUMBER_OF_BLOCKS" => {
                                im.number_of_blocks = text(ga).trim().parse().ok();
                            }
                            "UFS_IMAGE_DIMENSIONS" => {
                                for d in objects(ga) {
                                    let get = |n: &str| {
                                        attributes(d)
                                            .find(|x| name_of(*x) == n)
                                            .map(|x| text(x).trim().to_string())
                                    };
                                    im.dimensions.push(Dimension {
                                        name: get("UFS_IMAGE_DIMENSION_NAME").unwrap_or_default(),
                                        unit: get("UFS_IMAGE_DIMENSION_UNIT"),
                                        scale_factor: get("UFS_IMAGE_DIMENSION_SCALE_FACTOR")
                                            .and_then(|v| v.parse().ok()),
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {
                if !a.children().any(|c| is(c, "Array")) {
                    im.attributes.insert(name.to_string(), text(a));
                }
            }
        }
    }
    im.kind = im
        .attributes
        .get("PIM_DP_IMAGE_TYPE")
        .cloned()
        .unwrap_or_default();
    Ok(im)
}

/// Parse the XML text of the header (`bytes` = the file up to, not including, the 0x04).
pub fn parse(bytes: &[u8]) -> Result<Header> {
    let xml = std::str::from_utf8(bytes).map_err(|e| bad(format!("not UTF-8 ({e})")))?;
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            ..roxmltree::ParsingOptions::default()
        },
    )
    .map_err(|e| bad(e.to_string()))?;
    let root = doc.root_element();
    if !is(root, "DataObject") || root.attribute("ObjectType") != Some("DPUfsImport") {
        return Err(bad("the root element is not a DPUfsImport data object"));
    }
    let mut h = Header {
        length: bytes.len() as u64,
        tree: to_json(root),
        ..Header::default()
    };
    for a in attributes(root) {
        let name = name_of(a);
        if name == "PIM_DP_SCANNED_IMAGES" {
            for obj in objects(a) {
                h.images.push(scanned_image(obj)?);
            }
        } else if !a.children().any(|c| is(c, "Array")) {
            h.attributes.insert(name.to_string(), text(a));
        }
    }
    Ok(h)
}

/// Quoted values of a string-array attribute (`"a" "b"` → `[a, b]`); a bare value is one item.
pub fn string_array(v: &str) -> Vec<String> {
    let v = v.trim();
    if !v.contains('"') {
        return if v.is_empty() {
            Vec::new()
        } else {
            vec![v.to_string()]
        };
    }
    v.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The barcode (`PIM_DP_UFS_BARCODE`, base64), trimmed; `None` when absent or blank.
pub fn barcode(h: &Header) -> Option<String> {
    let v = h.attribute("PIM_DP_UFS_BARCODE")?;
    let raw = decode_b64("PIM_DP_UFS_BARCODE", v).ok()?;
    let s = String::from_utf8_lossy(&raw).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<DataObject ObjectType="DPUfsImport">
<Attribute Name="PIM_DP_UFS_INTERFACE_VERSION" Group="0x301D" Element="0x1001" PMSVR="IString">5.0</Attribute><Attribute Name="DICOM_SOFTWARE_VERSIONS" Group="0x0018" Element="0x1020" PMSVR="IStringArray">"1.8.6824" "20180906_R51"</Attribute><Attribute Name="PIM_DP_UFS_BARCODE" Group="0x301D" Element="0x1002" PMSVR="IString">ICAgIA==</Attribute><Attribute Name="PIM_DP_SCANNED_IMAGES" Group="0x301D" Element="0x1003" PMSVR="IDataObjectArray">
<Array>
<DataObject ObjectType="DPScannedImage">
<Attribute Name="PIM_DP_IMAGE_TYPE" Group="0x301D" Element="0x1004" PMSVR="IString">WSI</Attribute><Attribute Name="UFS_IMAGE_BLOCK_HEADER_TABLE" Group="0x301D" Element="0x2014" PMSVR="IString">AAAAAA==</Attribute>
<Attribute Name="UFS_IMAGE_GENERAL_HEADERS" Group="0x301D" Element="0x2000" PMSVR="IDataObjectArray">
<Array>
<DataObject ObjectType="UFSImageGeneralHeader">
<Attribute Name="UFS_IMAGE_NUMBER_OF_BLOCKS" Group="0x301D" Element="0x2001" PMSVR="IUInt32">7</Attribute><Attribute Name="UFS_IMAGE_DIMENSION_RANGES" Group="0x301D" Element="0x200A" PMSVR="IDataObjectArray">
<Array>
<DataObject ObjectType="UFSImageDimensionRange">
<Attribute Name="UFS_IMAGE_DIMENSION_RANGE" Group="0x301D" Element="0x200B" PMSVR="IInt32Array">13531 1 52442</Attribute></DataObject>
</Array>
</Attribute>
</DataObject>
</Array>
</Attribute>
</DataObject>
<DataObject ObjectType="DPScannedImage">
<Attribute Name="PIM_DP_IMAGE_TYPE" Group="0x301D" Element="0x1004" PMSVR="IString">LABELIMAGE</Attribute><Attribute Name="PIM_DP_IMAGE_DATA" Group="0x301D" Element="0x1005" PMSVR="IString">/9g=</Attribute></DataObject>
</Array>
</Attribute>
</DataObject>"#;

    #[test]
    fn reads_the_structure() {
        let h = parse(XML.as_bytes()).unwrap();
        assert_eq!(h.data_model(), Some((5, 0)));
        assert_eq!(h.images.len(), 2);
        let wsi = h.image("WSI").unwrap();
        assert_eq!(wsi.block_table.as_deref(), Some(&[0u8, 0, 0, 0][..]));
        assert_eq!(wsi.number_of_blocks, Some(7));
        assert_eq!(wsi.ranges[0].count(), 38912);
        assert_eq!(
            h.image("LABELIMAGE").unwrap().image_data.as_deref(),
            Some(&[0xFF, 0xD8][..])
        );
        assert_eq!(
            string_array(h.attribute("DICOM_SOFTWARE_VERSIONS").unwrap()),
            ["1.8.6824", "20180906_R51"]
        );
        assert_eq!(barcode(&h), None);
        assert_eq!(
            h.tree["PIM_DP_SCANNED_IMAGES"][0]["UFS_IMAGE_BLOCK_HEADER_TABLE"],
            json!("<base64, 8 characters>")
        );
        assert!(parse(b"<DataObject ObjectType=\"Other\"/>").is_err());
        assert!(parse(b"<DataObject").is_err());
    }
}
