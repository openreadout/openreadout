//! Decoder for the XML "variant" metadata used by version-2 ND2 files
//! (`<variant version="1.0"><no_name runtype="CLxListVariant"><uiWidth runtype="lx_uint32" value="696"/>…`)
//! and by the XML boxes of legacy (JPEG 2000-based) files.
//! Produces the same JSON shape as the LV decoder so the normalizers are shared.

use roxmltree::{Document, Node};
use serde_json::{Map, Value};

/// Deepest element nesting converted (real files nest a few levels; deeper subtrees are
/// replaced by `null` so a crafted document cannot exhaust the stack).
const MAX_DEPTH: u32 = 128;

/// Decode a variant XML document. The root `no_name` wrapper is unwrapped.
pub fn variant_decode(xml: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(xml);
    let text = text.trim_end_matches('\0');
    let doc = Document::parse(text).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    let mut obj = Map::new();
    for child in root.children().filter(roxmltree::Node::is_element) {
        insert(
            &mut obj,
            child.tag_name().name().to_string(),
            node_value(child, 0),
        );
    }
    Ok(Value::Object(obj))
}

/// Decode one XML box of a legacy file. Documents that wrap a `variant` (for example
/// `<MetadataSeq _SEQUENCE_INDEX="0"><variant><no_name>…`) yield the content of its single
/// `no_name` child; attribute-style documents (`<Calibration><CalibrationValues Calibration_11="…"/>`,
/// `<TextInfo><TextInfoItem Text="…" Index="0"/>…`) yield the root element with its attributes
/// as strings.
pub fn legacy_xml_decode(xml: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(xml);
    let text = text.trim_end_matches('\0');
    let doc = Document::parse(text).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if let Some(var) = root
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "variant")
    {
        let v = node_value(var, 0);
        return Ok(match v {
            Value::Object(mut o) if o.len() == 1 && o.contains_key("no_name") => {
                o.remove("no_name").unwrap_or(Value::Null)
            }
            other => other,
        });
    }
    Ok(node_value(root, 0))
}

fn insert(map: &mut Map<String, Value>, name: String, val: Value) {
    match map.get_mut(&name) {
        Some(Value::Array(arr)) => arr.push(val),
        Some(existing) => {
            let prev = existing.take();
            *existing = Value::Array(vec![prev, val]);
        }
        None => {
            map.insert(name, val);
        }
    }
}

fn node_value(n: Node<'_, '_>, depth: u32) -> Value {
    if depth > MAX_DEPTH {
        return Value::Null;
    }
    let runtype = n.attribute("runtype").unwrap_or("");
    if let Some(v) = n.attribute("value") {
        return match runtype {
            "lx_uint32" | "lx_uint64" => v
                .parse::<u64>()
                .map_or_else(|_| Value::String(v.into()), Value::from),
            "lx_int32" | "lx_int64" => v
                .parse::<i64>()
                .map_or_else(|_| Value::String(v.into()), Value::from),
            "double" | "float" => v
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map_or(Value::Null, Value::Number),
            "bool" => Value::Bool(v == "true" || v == "1"),
            "CLxByteArray" => Value::Array(
                crate::meta::base64_decode(v)
                    .into_iter()
                    .map(Value::from)
                    .collect(),
            ),
            _ => Value::String(v.to_string()),
        };
    }
    let mut obj = Map::new();
    for a in n.attributes() {
        if !matches!(a.name(), "runtype" | "version") {
            obj.insert(a.name().to_string(), Value::String(a.value().to_string()));
        }
    }
    for child in n.children().filter(roxmltree::Node::is_element) {
        insert(
            &mut obj,
            child.tag_name().name().to_string(),
            node_value(child, depth + 1),
        );
    }
    Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_variant_xml() {
        let x = br#"<?xml version="1.0"?><variant version="1.0"><no_name runtype="CLxListVariant"><uiWidth runtype="lx_uint32" value="696"/><d runtype="double" value="0.5"/><b runtype="bool" value="true"/><s runtype="CLxStringW" value="hi"/><L runtype="CLxListVariant"><_00 runtype="lx_int32" value="-1"/></L></no_name></variant>"#;
        let v = variant_decode(x).unwrap();
        assert_eq!(v["no_name"]["uiWidth"], 696);
        assert_eq!(v["no_name"]["d"], 0.5);
        assert_eq!(v["no_name"]["b"], true);
        assert_eq!(v["no_name"]["L"]["_00"], -1);
    }

    #[test]
    fn decodes_legacy_boxes() {
        let x = br#"<!--c--><CalibrationSeq _SEQUENCE_INDEX="0"><variant version="1.0"><no_name runtype="CLxListVariant"><dCalibration runtype="double" value="50.0"/></no_name></variant></CalibrationSeq>"#;
        assert_eq!(legacy_xml_decode(x).unwrap()["dCalibration"], 50.0);
        let x = br#"<!--c--><TextInfo Count="2"><TextInfoItem Text="" Index="0"/><TextInfoItem Text="Dimensions: T(3)" Index="1"/></TextInfo>"#;
        let v = legacy_xml_decode(x).unwrap();
        assert_eq!(v["TextInfoItem"][1]["Text"], "Dimensions: T(3)");
        assert_eq!(v["Count"], "2");
    }
}
