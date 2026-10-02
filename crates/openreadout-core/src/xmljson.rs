//! Lossless-enough XML → JSON conversion for vendor metadata blocks.
//!
//! Rules: attributes become `"@name": "value"`; child elements become keys; repeated
//! children become arrays; text content becomes `"#text"`. Nothing is renamed, so the
//! result is the vendor's own vocabulary and is placed under `vendor` in `info --view full`.

use serde_json::{Map, Value};

/// Deepest element nesting converted. Vendor metadata nests a few dozen levels at most;
/// deeper subtrees become the string `"<nested too deep>"` so a crafted document cannot
/// exhaust the stack (here, or later when the JSON is serialized or dropped).
pub const MAX_DEPTH: u32 = 256;

/// Convert an XML document to JSON. Returns `None` if the XML does not parse.
pub fn xml_to_json(xml: &str) -> Option<Value> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    let mut obj = Map::new();
    obj.insert(root.tag_name().name().to_string(), node_to_json(root, 0));
    Some(Value::Object(obj))
}

fn node_to_json(node: roxmltree::Node<'_, '_>, depth: u32) -> Value {
    if depth > MAX_DEPTH {
        return Value::String("<nested too deep>".into());
    }
    let mut obj = Map::new();
    for a in node.attributes() {
        obj.insert(format!("@{}", a.name()), parse_scalar(a.value()));
    }
    let mut text = String::new();
    for child in node.children() {
        if child.is_element() {
            let name = child.tag_name().name().to_string();
            let v = node_to_json(child, depth + 1);
            match obj.get_mut(&name) {
                Some(Value::Array(arr)) => arr.push(v),
                Some(existing) => {
                    let prev = existing.take();
                    *existing = Value::Array(vec![prev, v]);
                }
                None => {
                    obj.insert(name, v);
                }
            }
        } else if child.is_text()
            && let Some(t) = child.text()
        {
            let t = t.trim();
            if !t.is_empty() {
                text.push_str(t);
            }
        }
    }
    if obj.is_empty() {
        return if text.is_empty() {
            Value::Null
        } else {
            parse_scalar(&text)
        };
    }
    if !text.is_empty() {
        obj.insert("#text".into(), parse_scalar(&text));
    }
    Value::Object(obj)
}

/// Keep numbers as numbers where unambiguous; everything else stays a string.
fn parse_scalar(s: &str) -> Value {
    if s.is_empty() {
        return Value::String(String::new());
    }
    if let Ok(i) = s.parse::<i64>()
        && !s.starts_with('+')
        && !(s.len() > 1 && s.starts_with('0') && s != "0")
    {
        return Value::Number(i.into());
    }
    if let Ok(f) = s.parse::<f64>()
        && f.is_finite()
        && s.chars().any(|c| c == '.' || c == 'e' || c == 'E')
        && !s.starts_with('+')
        && let Some(n) = serde_json::Number::from_f64(f)
    {
        return Value::Number(n);
    }
    Value::String(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_attributes_children_and_repeats() {
        let v = xml_to_json(r#"<A x="1" y="a.b"><B v="2.5"/><B v="3"/><C>text</C></A>"#).unwrap();
        assert_eq!(v["A"]["@x"], 1);
        assert_eq!(v["A"]["@y"], "a.b");
        assert_eq!(v["A"]["B"][1]["@v"], 3);
        assert_eq!(v["A"]["C"], "text");
    }
}
