//! A size limit on what one tool call returns. Most tools answer summary-first, but some files
//! make long lists (a qPCR plate's 384 wells in every trace, per-well fits of a whole plate).
//! Clients refuse or spill results over a token limit (Claude Code: 25,000 by default), and a
//! chat host with no file tools cannot read a spilled result at all. Over the limit, text
//! blocks beside the JSON (an SVG) are left out first, then the longest lists are cut, then the
//! largest remaining parts (a vendor metadata tree) are left out, until the reply fits; a note
//! says what was cut and how to get the rest.

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

/// Room kept for the note [`fit`] adds.
const NOTE_ROOM: usize = 1_000;

/// Cut `result` to `max` characters of text when it is longer, and add a note on what was cut.
/// Every text block counts: the JSON and any block beside it.
pub fn fit(result: &mut CallToolResult, tool: &str, max: usize) {
    let json = result.structured_content.as_ref().map(Value::to_string);
    let is_json = |t: &str| json.as_deref() == Some(t);
    let text_len = |r: &CallToolResult| -> usize {
        r.content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.len()))
            .sum()
    };
    let total = text_len(result);
    if total <= max {
        return;
    }
    let mut said: Vec<String> = Vec::new();
    // text blocks beside the JSON, largest first
    let json_len = json.as_ref().map_or(0, String::len);
    let mut others: Vec<(usize, usize)> = result
        .content
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            c.as_text()
                .filter(|t| !is_json(&t.text))
                .map(|t| (i, t.text.len()))
        })
        .collect();
    others.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let mut rest: usize = others.iter().map(|o| o.1).sum();
    for (i, n) in others {
        if json_len.min(max) + rest <= max {
            break;
        }
        result.content[i] = ContentBlock::text(left_out_block(tool, n, max));
        rest -= n;
        said.push(format!("a text block of {n} characters was left out"));
    }
    let room = max
        .saturating_sub(rest)
        .saturating_sub(NOTE_ROOM)
        .max(max / 2);
    if let (Some(v), Some(before)) = (result.structured_content.as_mut(), json.as_ref())
        && before.len() > room
    {
        let lists = cut(v, room);
        if !lists.is_empty() {
            let l: Vec<String> = lists
                .iter()
                .map(|(path, kept, of)| format!("{path} kept {kept} of {of}"))
                .collect();
            said.push(format!("its longest lists were cut: {}", l.join("; ")));
        }
        let parts = prune(v, room);
        if !parts.is_empty() {
            let p: Vec<String> = parts
                .iter()
                .map(|(path, n)| format!("{path} ({n} characters)"))
                .collect();
            said.push(format!("these parts were left out: {}", p.join("; ")));
        }
        let after = v.to_string();
        for c in &mut result.content {
            if c.as_text().is_some_and(|t| t.text == *before) {
                *c = ContentBlock::text(after.clone());
            }
        }
    }
    if said.is_empty() {
        return;
    }
    result.content.push(ContentBlock::text(format!(
        "This reply was {total} characters, over the {max} a reply may have, so {}. To see the rest, narrow the call ({}), or write everything to a file with openreadout_export or openreadout_batch (output).",
        said.join("; "),
        narrowing(tool)
    )));
}

/// What stands in for a text block left out.
fn left_out_block(tool: &str, n: usize, max: usize) -> String {
    if tool == "openreadout_preview" {
        format!(
            "The SVG ({n} characters) is over the {max}-character reply limit, so it was left out: openreadout_export with preview = these arguments and output = a .svg path saves it as a file."
        )
    } else {
        format!(
            "A text block of {n} characters was left out: it is over the {max}-character reply limit."
        )
    }
}

/// Leave out the largest parts of `v` until it serializes to at most `budget` characters: the
/// smallest single part whose removal is enough, else the largest part, each time. A long
/// string is shortened rather than removed. Returns each part left out, as (JSON pointer, its
/// size in characters).
fn prune(v: &mut Value, budget: usize) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for _ in 0..100 {
        let size = v.to_string().len();
        if size <= budget {
            break;
        }
        let over = size - budget;
        let mut path: Vec<PathPart> = Vec::new();
        let mut cur: &Value = v;
        let mut target: Option<(Vec<PathPart>, usize)> = None;
        // go down while one child alone is enough to remove
        loop {
            let Some((part, child, n)) = largest_child(cur) else {
                break;
            };
            path.push(part);
            if n < over + 64 {
                // no single part is enough: take the largest one (at the top level only, so
                // the next round can look again)
                if target.is_none() {
                    target = Some((path.clone(), n));
                }
                break;
            }
            target = Some((path.clone(), n));
            cur = child;
        }
        let Some((path, n)) = target else {
            break;
        };
        let Some(node) = at(v, &path) else {
            break;
        };
        let marker = format!("(left out: {n} characters)");
        if n <= marker.len() + 2 {
            break;
        }
        match node {
            Value::String(s) if s.len() > over + 200 => {
                let keep = s.len() - over - 100;
                let mut end = keep;
                while !s.is_char_boundary(end) {
                    end -= 1;
                }
                s.truncate(end);
                s.push_str(" … (shortened)");
            }
            _ => *node = Value::String(marker),
        }
        out.push((pointer(&path), n));
    }
    out
}

/// The child (object member or list item) of `v` with the most characters.
fn largest_child(v: &Value) -> Option<(PathPart, &Value, usize)> {
    let children: Box<dyn Iterator<Item = (PathPart, &Value)>> = match v {
        Value::Object(o) => Box::new(o.iter().map(|(k, x)| (PathPart::Key(k.clone()), x))),
        Value::Array(a) => Box::new(a.iter().enumerate().map(|(i, x)| (PathPart::Index(i), x))),
        _ => return None,
    };
    children
        .map(|(p, x)| {
            let n = x.to_string().len();
            (p, x, n)
        })
        .max_by_key(|c| c.2)
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

    /// Large objects (a vendor metadata tree) and lists of a few large items are left out with
    /// a note, and text blocks beside the JSON count towards the limit.
    #[test]
    fn large_parts_and_extra_text_are_left_out() {
        let vendor: serde_json::Map<String, Value> = (0..3000)
            .map(|i| (format!("Key{i}"), json!(format!("value number {i}"))))
            .collect();
        let v = json!({"path": "x", "images": [{"name": "a"}], "vendor": vendor});
        let mut r = CallToolResult::structured(v);
        fit(&mut r, "openreadout_info", MAX_CHARS);
        let sc = r.structured_content.as_ref().unwrap();
        assert!(
            sc.to_string().len() <= MAX_CHARS,
            "{}",
            sc.to_string().len()
        );
        assert_eq!(sc["path"], "x");
        assert!(sc["vendor"].as_str().unwrap().starts_with("(left out"));
        let note = r.content.last().unwrap().as_text().unwrap().text.clone();
        assert!(note.contains("/vendor ("), "{note}");

        let svg = "<svg>".to_string() + &"x".repeat(1_500_000);
        let mut r = CallToolResult::structured(json!({"kind": "trace"}));
        r.content.push(ContentBlock::text(svg));
        fit(&mut r, "openreadout_preview", MAX_CHARS);
        let total: usize = r
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.len()))
            .sum();
        assert!(total <= MAX_CHARS, "{total}");
        assert!(
            r.content[1]
                .as_text()
                .unwrap()
                .text
                .contains("openreadout_export")
        );
    }

    #[test]
    fn small_results_are_untouched() {
        let mut r = CallToolResult::structured(json!({"a": [1, 2, 3]}));
        fit(&mut r, "t", MAX_CHARS);
        assert_eq!(r.content.len(), 1);
    }
}
