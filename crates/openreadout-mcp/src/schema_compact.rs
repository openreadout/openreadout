//! Compact tool input schemas: every tool's `inputSchema` is read by the model on every
//! session, so the JSON Schema that `schemars` derives is rewritten into an equivalent, shorter
//! form before it is listed. Nothing an agent needs is dropped: every property, type, enum value
//! and description stays.
//!
//! - unit enums (`oneOf` of `const` strings, each with its own description) become `enum: [...]`
//!   with one description (`a` = …; `b` = …), and are inlined where they are referenced;
//! - `Option<T>` (`anyOf: [T, {type: null}]`, `type: [T, "null"]`) becomes `T` (omitting the
//!   property is how an agent leaves it unset);
//! - Rust integer/float `format`s (`uint32`, `double`, …), `"default": null` and `$schema` are
//!   removed; line breaks in descriptions become spaces.
//!
//! Arguments are still parsed by serde from the same field names, so the calls accepted do not
//! change.

use serde_json::{Map, Value};

const NUMERIC_FORMATS: [&str; 12] = [
    "uint", "uint8", "uint16", "uint32", "uint64", "int", "int8", "int16", "int32", "int64",
    "float", "double",
];

/// The compacted form of an input schema object.
pub fn compact(schema: &Map<String, Value>) -> Map<String, Value> {
    let defs = schema
        .get("$defs")
        .or_else(|| schema.get("definitions"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut v = Value::Object(schema.clone());
    if let Value::Object(m) = &mut v {
        m.remove("$schema");
    }
    let mut v = walk(v, &defs, 0);
    // Drop definitions that inlining left unreferenced (directly, or through a kept one).
    if let Value::Object(m) = &mut v {
        let mut text = serde_json::to_string(
            &m.iter()
                .filter(|(k, _)| *k != "$defs")
                .map(|(k, x)| (k.clone(), x.clone()))
                .collect::<Map<String, Value>>(),
        )
        .unwrap_or_default();
        let mut keep = std::collections::BTreeSet::new();
        if let Some(Value::Object(d)) = m.get("$defs") {
            loop {
                let found: Vec<&String> = d
                    .keys()
                    .filter(|n| !keep.contains(*n) && text.contains(&format!("/$defs/{n}\"")))
                    .collect();
                if found.is_empty() {
                    break;
                }
                for n in found {
                    text.push_str(&serde_json::to_string(&d[n]).unwrap_or_default());
                    keep.insert(n.clone());
                }
            }
        }
        if let Some(Value::Object(d)) = m.get_mut("$defs") {
            d.retain(|name, _| keep.contains(name));
            if d.is_empty() {
                m.remove("$defs");
            }
        }
    }
    match v {
        Value::Object(m) => m,
        _ => schema.clone(),
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `oneOf` of string constants → `(values, "a = …; b = …")`.
fn const_enum(items: &[Value]) -> Option<(Vec<Value>, String)> {
    let mut values = Vec::new();
    let mut parts = Vec::new();
    for it in items {
        let o = it.as_object()?;
        let c = o.get("const")?.as_str()?;
        if o.keys()
            .any(|k| !matches!(k.as_str(), "const" | "type" | "description"))
        {
            return None;
        }
        values.push(Value::String(c.to_string()));
        match o.get("description").and_then(Value::as_str) {
            Some(d) => parts.push(format!("`{c}` = {}", one_line(d).trim_end_matches('.'))),
            None => parts.push(format!("`{c}`")),
        }
    }
    (!values.is_empty()).then(|| (values, parts.join("; ")))
}

fn merge_description(outer: Option<&Value>, inner: Option<&Value>) -> Option<Value> {
    let o = outer.and_then(Value::as_str).map(one_line);
    let i = inner.and_then(Value::as_str).map(one_line);
    match (o, i) {
        (Some(o), Some(i)) if o.contains(&i) => Some(Value::String(o)),
        (Some(o), Some(i)) => Some(Value::String(format!("{o} {i}"))),
        (Some(x), None) | (None, Some(x)) => Some(Value::String(x)),
        (None, None) => None,
    }
}

fn walk(v: Value, defs: &Value, depth: u32) -> Value {
    if depth > 32 {
        return v;
    }
    match v {
        Value::Array(a) => Value::Array(a.into_iter().map(|x| walk(x, defs, depth + 1)).collect()),
        Value::Object(mut m) => {
            // `$ref` to a simple enum: inline it.
            if let Some(r) = m.get("$ref").and_then(Value::as_str).map(str::to_string)
                && let Some(name) = r
                    .strip_prefix("#/$defs/")
                    .or(r.strip_prefix("#/definitions/"))
                && let Some(Value::Object(d)) = defs.get(name)
                && d.get("oneOf")
                    .and_then(Value::as_array)
                    .is_some_and(|a| const_enum(a).is_some())
            {
                let desc = m.remove("description");
                m.remove("$ref");
                let mut inlined = d.clone();
                let inner = inlined.remove("description");
                if let Some(x) = merge_description(desc.as_ref(), inner.as_ref()) {
                    inlined.insert("description".into(), x);
                }
                for (k, x) in inlined {
                    m.entry(k).or_insert(x);
                }
            }
            // Option<T>: anyOf [T, null] → T.
            if let Some(Value::Array(a)) = m.get("anyOf")
                && a.len() == 2
                && let Some(pos) = a
                    .iter()
                    .position(|x| x.get("type") == Some(&Value::from("null")))
            {
                let other = a[1 - pos].clone();
                m.remove("anyOf");
                if let Value::Object(o) = other {
                    let desc = m.remove("description");
                    let inner = o.get("description").cloned();
                    for (k, x) in o {
                        if k != "description" {
                            m.entry(k).or_insert(x);
                        }
                    }
                    if let Some(x) = merge_description(desc.as_ref(), inner.as_ref()) {
                        m.insert("description".into(), x);
                    }
                }
                return walk(Value::Object(m), defs, depth + 1);
            }
            // type: [T, "null"] → T.
            if let Some(Value::Array(t)) = m.get("type")
                && t.len() == 2
                && t.contains(&Value::from("null"))
            {
                let keep = t.iter().find(|x| x.as_str() != Some("null")).cloned();
                if let Some(k) = keep {
                    m.insert("type".into(), k);
                }
            }
            // oneOf of string constants → enum.
            if let Some((values, parts)) = m
                .get("oneOf")
                .and_then(Value::as_array)
                .and_then(|a| const_enum(a))
            {
                m.remove("oneOf");
                m.insert("type".into(), Value::from("string"));
                m.insert("enum".into(), Value::Array(values));
                let d = match m.get("description").and_then(Value::as_str) {
                    Some(d) => format!("{} {parts}.", one_line(d)),
                    None => format!("{parts}."),
                };
                m.insert("description".into(), Value::String(d));
            }
            if m.get("format")
                .and_then(Value::as_str)
                .is_some_and(|f| NUMERIC_FORMATS.contains(&f))
            {
                m.remove("format");
            }
            if m.get("default") == Some(&Value::Null) {
                m.remove("default");
            }
            if let Some(Value::String(d)) = m.get_mut("description") {
                *d = one_line(d);
            }
            Value::Object(
                m.into_iter()
                    .map(|(k, x)| (k, walk(x, defs, depth + 1)))
                    .collect(),
            )
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enums_options_and_formats_are_compacted() {
        let s = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "$defs": {"Mode": {"description": "The mode.", "oneOf": [
                {"description": "First\nline.", "type": "string", "const": "a"},
                {"type": "string", "const": "b"}]}},
            "properties": {
                "mode": {"anyOf": [{"$ref": "#/$defs/Mode"}, {"type": "null"}], "description": "Pick one."},
                "n": {"type": ["integer", "null"], "format": "uint32", "minimum": 0, "default": null},
                "file": {"type": "string", "description": "Path"}
            },
            "required": ["file"]
        });
        let c = Value::Object(compact(s.as_object().unwrap()));
        assert!(
            c.get("$schema").is_none() && c.get("$defs").is_none(),
            "{c}"
        );
        let mode = &c["properties"]["mode"];
        assert_eq!(mode["enum"], json!(["a", "b"]));
        assert_eq!(mode["type"], json!("string"));
        let d = mode["description"].as_str().unwrap();
        assert!(
            d.contains("Pick one.") && d.contains("`a` = First line") && d.contains("`b`"),
            "{d}"
        );
        assert_eq!(
            c["properties"]["n"],
            json!({"type": "integer", "minimum": 0})
        );
        assert_eq!(c["required"], json!(["file"]));
    }
}
