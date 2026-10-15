//! A size limit on what one tool call returns. Most tools answer summary-first, but some files
//! make long lists (a qPCR plate's 384 wells in every trace, per-well fits of a whole plate).
//! Clients refuse or spill results over a token limit (Claude Code: 25,000 by default), and a
//! chat host with no file tools cannot read a spilled result at all. Over the limit, the
//! longest lists are cut until the reply fits, and a note says what was cut and how to get the
//! rest.

use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::Value;

/// Characters of JSON a result may have. Claude Code refused 57,771 characters of growth rows
/// and took 40,501 characters of table rows: numbers and short names tokenize densely.
pub const MAX_CHARS: usize = 38_000;

/// Characters of an `openreadout_info` summary: what a file holds should read at a glance.
const INFO_CHARS: usize = 20_000;

/// The limit for a call: tighter for `openreadout_info` except its full view.
pub fn max_chars(tool: &str, args: Option<&serde_json::Map<String, Value>>) -> usize {
    let view = args.and_then(|a| a.get("view")).and_then(Value::as_str);
    if tool == "openreadout_info" && view != Some("full") {
        INFO_CHARS
    } else {
        MAX_CHARS
    }
}

/// Lists at most this long are left whole: they are the structure of the reply (a file's
/// traces, a figure's panels), not its bulk.
const KEEP_WHOLE: usize = 12;

/// Cut `result` to `max` characters when it is longer, and add a note on what was cut.
pub fn fit(result: &mut CallToolResult, tool: &str, max: usize) {
    let Some(v) = result.structured_content.as_mut() else {
        return;
    };
    let before = v.to_string();
    if before.len() <= max {
        return;
    }
    let cuts = cut(v, max);
    if cuts.is_empty() {
        return;
    }
    let after = v.to_string();
    for c in &mut result.content {
        if c.as_text().is_some_and(|t| t.text == before) {
            *c = ContentBlock::text(after.clone());
        }
    }
    let lists: Vec<String> = cuts
        .iter()
        .map(|(path, kept, of)| format!("{path} kept {kept} of {of}"))
        .collect();
    result.content.push(ContentBlock::text(format!(
        "This reply was {} characters, over the {max} a reply may have, so its longest lists were cut: {}. To see the rest, narrow the call ({}), or write everything to a file with openreadout_export or openreadout_batch (output).",
        before.len(),
        lists.join("; "),
        narrowing(tool)
    )));
}

/// The arguments that narrow a call of `tool`.
fn narrowing(tool: &str) -> &'static str {
    match tool {
        "openreadout_assay" => "wells, such as E1:H12",
        "openreadout_qpcr" => "target, sample, well or max_records",
        "openreadout_gate" => "populations",
        "openreadout_ephys_features" => "sweeps or max_spikes",
        "openreadout_table" => "max_rows, first_row or filter",
        "openreadout_batch" => "limit and offset, or fields",
        "openreadout_info" => "view=structure with kind, or openreadout_trace for one trace",
        _ => "its selection arguments",
    }
}

/// Cut the longest lists in `v` until it serializes to at most `budget` characters. Returns
/// each list cut, as (JSON pointer with `*` for indices, items kept, items before).
fn cut(v: &mut Value, budget: usize) -> Vec<(String, usize, usize)> {
    let mut cuts: Vec<(String, usize, usize)> = Vec::new();
    for _ in 0..200 {
        let size = v.to_string().len();
        if size <= budget {
            break;
        }
        let mut best: Option<(Vec<PathPart>, usize)> = None;
        longest(v, &mut Vec::new(), &mut best);
        let Some((path, list_size)) = best else {
            break;
        };
        let Some(Value::Array(a)) = at(v, &path) else {
            break;
        };
        let n = a.len();
        // keep the share of the list that brings the whole reply under budget
        let over = size - budget;
        let per_item = list_size / n.max(1);
        let drop = over.div_ceil(per_item.max(1)).max(1);
        let keep = n.saturating_sub(drop).max(KEEP_WHOLE.min(n) / 2).max(1);
        if keep >= n {
            break;
        }
        a.truncate(keep);
        let name = pointer(&path);
        match cuts.iter_mut().find(|c| c.0 == name) {
            Some(c) => c.1 = c.1.min(keep),
            None => cuts.push((name, keep, n)),
        }
    }
    cuts
}

#[derive(Clone)]
enum PathPart {
    Key(String),
    Index(usize),
}

/// The list with the most characters among those longer than [`KEEP_WHOLE`] items.
fn longest(v: &Value, path: &mut Vec<PathPart>, best: &mut Option<(Vec<PathPart>, usize)>) {
    match v {
        Value::Array(a) => {
            if a.len() > KEEP_WHOLE {
                let size = v.to_string().len();
                if best.as_ref().is_none_or(|b| size > b.1) {
                    *best = Some((path.clone(), size));
                }
            }
            for (i, x) in a.iter().enumerate() {
                path.push(PathPart::Index(i));
                longest(x, path, best);
                path.pop();
            }
        }
        Value::Object(o) => {
            for (k, x) in o {
                path.push(PathPart::Key(k.clone()));
                longest(x, path, best);
                path.pop();
            }
        }
        _ => {}
    }
}

fn at<'a>(v: &'a mut Value, path: &[PathPart]) -> Option<&'a mut Value> {
    let mut cur = v;
    for p in path {
        cur = match (p, cur) {
            (PathPart::Key(k), Value::Object(o)) => o.get_mut(k)?,
            (PathPart::Index(i), Value::Array(a)) => a.get_mut(*i)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn pointer(path: &[PathPart]) -> String {
    let mut s = String::new();
    for p in path {
        s.push('/');
        match p {
            PathPart::Key(k) => s.push_str(k),
            PathPart::Index(_) => s.push('*'),
        }
    }
    if s.is_empty() { "/".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn long_lists_are_cut_to_fit_and_noted() {
        let rows: Vec<Value> = (0..2000)
            .map(|i| json!({"well": format!("W{i}"), "value": i}))
            .collect();
        let traces: Vec<Value> = (0..3)
            .map(|t| json!({"index": t, "channels": (0..500).map(|c| json!({"name": format!("c{c}")})).collect::<Vec<_>>()}))
            .collect();
        let v = json!({"path": "x", "records": rows, "traces": traces, "few": [1, 2, 3]});
        let mut r = CallToolResult::structured(v);
        fit(&mut r, "openreadout_qpcr", MAX_CHARS);
        let sc = r.structured_content.as_ref().unwrap();
        assert!(sc.to_string().len() <= MAX_CHARS);
        assert_eq!(
            sc["traces"].as_array().unwrap().len(),
            3,
            "short lists stay whole"
        );
        assert_eq!(sc["few"], json!([1, 2, 3]));
        let text: Vec<&str> = r
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.as_str()))
            .collect();
        assert_eq!(serde_json::from_str::<Value>(text[0]).unwrap(), *sc);
        assert!(
            text[1].contains("/records kept") && text[1].contains("of 2000"),
            "{}",
            text[1]
        );
    }

    #[test]
    fn small_results_are_untouched() {
        let mut r = CallToolResult::structured(json!({"a": [1, 2, 3]}));
        fit(&mut r, "t", MAX_CHARS);
        assert_eq!(r.content.len(), 1);
    }
}
