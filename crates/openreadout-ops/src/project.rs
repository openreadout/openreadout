//! Output projection (`--only` on the CLI): keep only the values an agent
//! asked for, by JSON pointer, so a 35 MB `info` of a screening plate can answer "what is the
//! pixel size" in a few bytes.
//!
//! A pointer is RFC 6901 (`/images/0/physical_size/x`) relative to the command's `data`, with
//! two conveniences: a leading `/data` is dropped (pointers copied from the envelope work), and
//! `*` as a segment maps the rest of the pointer over every element of an array (or every value
//! of an object), giving an array (`/images/*/name` → one name per image). The projection is an
//! object from each pointer as given to its value; a pointer that matches nothing maps to `null`
//! and is also listed under `"missing"`, so nothing is dropped silently.

use serde_json::{Map, Value};

use openreadout_core::{Error, Result};

/// Most pointers one projection takes.
pub const MAX_POINTERS: usize = 64;

/// Split a comma-separated list of pointers (commas inside a pointer are not supported).
pub fn parse_list(items: &[String]) -> Vec<String> {
    items
        .iter()
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Project `data` onto `pointers`: `{pointer: value, …}` plus `missing: [pointer, …]` when some
/// matched nothing.
pub fn pick(data: &Value, pointers: &[String]) -> Result<Value> {
    if pointers.len() > MAX_POINTERS {
        return Err(Error::Usage(format!(
            "--only takes at most {MAX_POINTERS} pointers ({} given)",
            pointers.len()
        )));
    }
    let mut out = Map::new();
    let mut missing = Vec::new();
    for p in pointers {
        let segs = segments(data, p)?;
        let v = resolve(data, &segs);
        if v.is_none() {
            missing.push(Value::String(p.clone()));
        }
        out.insert(p.clone(), v.unwrap_or(Value::Null));
    }
    if !missing.is_empty() {
        out.insert("missing".into(), Value::Array(missing));
    }
    Ok(Value::Object(out))
}

/// The unescaped segments of `pointer` (`~1` → `/`, `~0` → `~`), relative to `data`.
fn segments(data: &Value, pointer: &str) -> Result<Vec<String>> {
    let p = pointer.trim();
    let p = p.strip_prefix('#').unwrap_or(p);
    let body = p.strip_prefix('/').unwrap_or(p);
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let mut segs: Vec<String> = body
        .split('/')
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect();
    // Pointers copied from the envelope (`/data/images/0`) address the same values.
    if segs.first().is_some_and(|s| s == "data") && data.get("data").is_none() {
        segs.remove(0);
    }
    if segs.iter().any(String::is_empty) {
        return Err(Error::Usage(format!(
            "--only {pointer:?}: empty segment (write pointers as /images/0/size_x)"
        )));
    }
    Ok(segs)
}

fn resolve(v: &Value, segs: &[String]) -> Option<Value> {
    let Some((head, rest)) = segs.split_first() else {
        return Some(v.clone());
    };
    if head == "*" {
        let items: Vec<&Value> = match v {
            Value::Array(a) => a.iter().collect(),
            Value::Object(m) => m.values().collect(),
            _ => return None,
        };
        return Some(Value::Array(
            items
                .into_iter()
                .map(|x| resolve(x, rest).unwrap_or(Value::Null))
                .collect(),
        ));
    }
    let next = match v {
        Value::Object(m) => m.get(head.as_str()),
        Value::Array(a) => head.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    }?;
    resolve(next, rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_pointers_wildcards_and_reports_missing() {
        let d = json!({"images": [{"name": "a", "size_x": 4}, {"name": "b"}], "experiment": {"sample": {"id": "S1"}}});
        let p = pick(
            &d,
            &parse_list(&[
                "/images/0/size_x,/images/*/name".into(),
                "data/experiment/sample/id".into(),
                "/images/*/size_x".into(),
                "/nope".into(),
            ]),
        )
        .unwrap();
        assert_eq!(p["/images/0/size_x"], json!(4));
        assert_eq!(p["/images/*/name"], json!(["a", "b"]));
        assert_eq!(p["data/experiment/sample/id"], json!("S1"));
        assert_eq!(p["/images/*/size_x"], json!([4, null]));
        assert_eq!(p["/nope"], Value::Null);
        assert_eq!(p["missing"], json!(["/nope"]));
        assert!(pick(&d, &["/images//x".into()]).is_err());
        assert_eq!(pick(&d, &["/".into()]).unwrap()["/"], d);
    }
}
