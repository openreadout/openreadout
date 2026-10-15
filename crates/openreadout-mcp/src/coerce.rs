//! Forgiving arguments: a single value where a tool takes a list (`"filter": "FITC-A > 1000"`,
//! `"select": "c=0,1,z=2"`, `"channels": 2`) is read as a list of that one value, so an agent
//! does not lose a call to a type error. The input schema still says `array`.

use serde_json::{Map, Value};

/// Wrap every argument whose property is an array in `schema` but whose value is a single
/// string, number, boolean or object.
pub fn lists(schema: &Map<String, Value>, args: &mut Map<String, Value>) {
    let Some(Value::Object(props)) = schema.get("properties") else {
        return;
    };
    for (name, value) in args.iter_mut() {
        let is_array = props
            .get(name)
            .and_then(|p| p.get("type"))
            .is_some_and(|t| match t {
                Value::String(s) => s == "array",
                Value::Array(a) => a.iter().any(|x| x == "array"),
                _ => false,
            });
        if is_array && !matches!(value, Value::Array(_) | Value::Null) {
            *value = Value::Array(vec![value.take()]);
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

    #[test]
    fn single_values_become_lists() {
        let schema = json!({"properties": {
            "filter": {"type": "array", "items": {"type": "string"}},
            "channels": {"type": ["array", "null"]},
            "file": {"type": "string"},
        }});
        let mut args = json!({"filter": "FITC-A > 1000", "channels": 2, "file": "a.fcs"});
        lists(schema.as_object().unwrap(), args.as_object_mut().unwrap());
        assert_eq!(
            args,
            json!({"filter": ["FITC-A > 1000"], "channels": [2], "file": "a.fcs"})
        );
        let mut args = json!({"filter": ["a > 1"], "channels": null});
        let before = args.clone();
        lists(schema.as_object().unwrap(), args.as_object_mut().unwrap());
        assert_eq!(args, before);
    }
}
