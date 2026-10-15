//! Forgiving arguments, read against the tool's input schema so an agent does not lose a call
//! to a type error:
//!
//! - a single value where a tool takes a list (`"filter": "FITC-A > 1000"`,
//!   `"select": "c=0,1,z=2"`, `"channels": 2`) is read as a list of that one value;
//! - a string holding JSON where an object or a list is expected (`"preview": "{\"file\": …}"`)
//!   is read as that object or list. Models sometimes send a nested argument this way.
//!
//! Both apply at any depth the schema describes, and to the `plot` and `preview` objects of
//! `openreadout_export`, which are read against `openreadout_plot`'s and `openreadout_preview`'s
//! own schemas. The input schemas still say `array` and `object`.

use serde_json::{Map, Value};

/// How deep the schema walk goes (a plot spec nests panels, series and annotations).
const MAX_DEPTH: usize = 12;

/// The `openreadout_export` arguments that hold the arguments of another tool.
pub const NESTED_TOOL_ARGS: [(&str, &str); 2] =
    [("plot", "openreadout_plot"), ("preview", "openreadout_preview")];

/// Read `args` against the input `schema` of their tool (see the module notes).
pub fn args(schema: &Map<String, Value>, args: &mut Map<String, Value>) {
    let root = Value::Object(schema.clone());
    object(&root, &root, args, 0);
}

/// The schema a `$ref` points at, or the schema itself.
fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    let mut s = schema;
    for _ in 0..8 {
        let Some(r) = s.get("$ref").and_then(Value::as_str) else {
            break;
        };
        let target = if let Some(n) = r.strip_prefix("#/$defs/") {
            root.get("$defs").and_then(|d| d.get(n))
        } else if let Some(n) = r.strip_prefix("#/definitions/") {
            root.get("definitions").and_then(|d| d.get(n))
        } else {
            None
        };
        match target {
            Some(t) => s = t,
            None => break,
        }
    }
    s
}

/// The schema and its `anyOf`/`oneOf`/`allOf` alternatives, resolved.
fn alternatives<'a>(root: &'a Value, schema: &'a Value, out: &mut Vec<&'a Value>, depth: usize) {
    let s = resolve(root, schema);
    out.push(s);
    if depth > 4 {
        return;
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(Value::Array(alts)) = s.get(key) {
            for a in alts {
                alternatives(root, a, out, depth + 1);
            }
        }
    }
}

/// The JSON types a schema (with its alternatives) accepts; empty when it does not say.
fn types(alts: &[&Value]) -> Vec<String> {
    let mut t = Vec::new();
    for s in alts {
        match s.get("type") {
            Some(Value::String(x)) => t.push(x.clone()),
            Some(Value::Array(xs)) => {
                t.extend(xs.iter().filter_map(|x| x.as_str().map(String::from)));
            }
            _ => {}
        }
        if s.get("properties").is_some() {
            t.push("object".into());
        }
        if s.get("items").is_some() {
            t.push("array".into());
        }
        if let Some(c) = s.get("const").or_else(|| s.get("enum").and_then(|e| e.get(0))) {
            t.push(json_type(c).into());
        }
    }
    t
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn accepts(types: &[String], v: &Value) -> bool {
    let t = json_type(v);
    types
        .iter()
        .any(|x| x == t || (x == "number" && t == "integer"))
}

/// Fix one value against its schema, then the values inside it.
fn value(root: &Value, schema: &Value, v: &mut Value, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let mut alts = Vec::new();
    alternatives(root, schema, &mut alts, 0);
    let t = types(&alts);
    if t.is_empty() {
        return;
    }
    // A string holding JSON where an object or a list is expected.
    if let Value::String(s) = v {
        let trimmed = s.trim();
        if t.iter().any(|x| x == "object" || x == "array")
            && !t.iter().any(|x| x == "string")
            && (trimmed.starts_with('{') || trimmed.starts_with('['))
            && let Ok(parsed) = serde_json::from_str::<Value>(trimmed)
            && accepts(&t, &parsed)
        {
            *v = parsed;
        }
    }
    // A single value where a list is expected.
    if t.iter().any(|x| x == "array")
        && !matches!(v, Value::Array(_) | Value::Null)
        && !accepts(&t, v)
    {
        *v = Value::Array(vec![v.take()]);
    }
    match v {
        Value::Object(m) => {
            for s in alts.iter().filter(|s| s.get("properties").is_some()) {
                object(root, s, m, depth + 1);
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = alts.iter().find_map(|s| s.get("items")) {
                for x in items {
                    value(root, item_schema, x, depth + 1);
                }
            }
        }
        _ => {}
    }
}

/// Fix each property of an object against the object's schema.
fn object(root: &Value, schema: &Value, m: &mut Map<String, Value>, depth: usize) {
    let Some(Value::Object(props)) = schema.get("properties") else {
        return;
    };
    for (name, v) in m.iter_mut() {
        if let Some(p) = props.get(name) {
            value(root, p, v, depth);
        }
    }
}

/// The tool's arguments and their types, as a hint for an argument error.
fn argument_hint(tool: Option<&rmcp::model::Tool>) -> Option<String> {
    let Some(Value::Object(props)) = tool.and_then(|t| t.input_schema.get("properties")) else {
        return None;
    };
    let args: Vec<String> = props
        .iter()
        .map(|(name, p)| match p.get("type") {
            Some(Value::String(t)) => format!("{name} ({t})"),
            _ => name.clone(),
        })
        .collect();
    Some(format!("arguments: {}", args.join(", ")))
}

/// An argument error without a hint (a wrong type or a missing field) gets one: the tool's
/// arguments and their types. The router reports these either as an error or as a result with
/// `isError` whose text starts `failed to deserialize parameters`.
pub fn with_argument_hint(
    r: Result<rmcp::model::CallToolResponse, rmcp::ErrorData>,
    tool: Option<&rmcp::model::Tool>,
) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
    match r {
        Err(mut e) if e.code == rmcp::model::ErrorCode::INVALID_PARAMS && e.data.is_none() => {
            if let Some(h) = argument_hint(tool) {
                e.data = Some(serde_json::json!({"code": "usage", "exit_code": 2, "hint": h}));
            }
            Err(e)
        }
        Ok(rmcp::model::CallToolResponse::Complete(mut result))
            if result.is_error == Some(true)
                && result.content.iter().any(|c| {
                    c.as_text()
                        .is_some_and(|t| t.text.starts_with("failed to deserialize parameters"))
                }) =>
        {
            if let Some(h) = argument_hint(tool) {
                result
                    .content
                    .push(rmcp::model::ContentBlock::text(format!("hint: {h}")));
            }
            Ok(result.into())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixed(schema: Value, mut a: Value) -> Value {
        args(schema.as_object().unwrap(), a.as_object_mut().unwrap());
        a
    }

    #[test]
    fn single_values_become_lists() {
        let schema = json!({"properties": {
            "filter": {"type": "array", "items": {"type": "string"}},
            "channels": {"type": ["array", "null"]},
            "file": {"type": "string"},
        }});
        assert_eq!(
            fixed(
                schema.clone(),
                json!({"filter": "FITC-A > 1000", "channels": 2, "file": "a.fcs"})
            ),
            json!({"filter": ["FITC-A > 1000"], "channels": [2], "file": "a.fcs"})
        );
        let before = json!({"filter": ["a > 1"], "channels": null});
        assert_eq!(fixed(schema, before.clone()), before);
    }

    #[test]
    fn json_strings_become_objects_and_lists() {
        let schema = json!({
            "properties": {
                "preview": {"type": "object"},
                "region": {"anyOf": [{"$ref": "#/$defs/Region"}, {"type": "null"}]},
                "wells": {"type": "array", "items": {"type": "string"}},
                "title": {"type": "string"},
                "panels": {"type": "array", "items": {"$ref": "#/$defs/Panel"}},
            },
            "$defs": {
                "Region": {"type": "object", "properties": {"x": {"type": "integer"}}},
                "Panel": {"type": "object", "properties": {
                    "series": {"type": "array", "items": {"type": "object"}},
                }},
            }
        });
        let panels = json!([{"series": json!({"file": "x"}).to_string()}]).to_string();
        let got = fixed(
            schema,
            json!({
                "preview": json!({"file": "a.czi", "region": {"x": 1}}).to_string(),
                "region": " {\"x\": 5} ",
                "wells": "[\"A1\", \"B2\"]",
                "title": "{not touched}",
                "panels": panels,
            }),
        );
        assert_eq!(
            got,
            json!({
                "preview": {"file": "a.czi", "region": {"x": 1}},
                "region": {"x": 5},
                "wells": ["A1", "B2"],
                "title": "{not touched}",
                "panels": [{"series": [{"file": "x"}]}],
            })
        );
    }

    #[test]
    fn broken_json_is_left_for_the_error() {
        let schema = json!({"properties": {"preview": {"type": "object"}}});
        let a = json!({"preview": "{\"file\": \"a.czi\""});
        assert_eq!(fixed(schema, a.clone()), a);
    }
}
