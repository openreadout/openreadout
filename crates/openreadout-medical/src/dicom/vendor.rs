//! The data set as JSON (`info --view full` → `vendor`): elements by keyword (private and
//! unknown ones as `(gggg,eeee)`), text and numbers as values, binary values summarized,
//! sequences as arrays of objects. Patient-identifying values are withheld.

use serde_json::{Map, Value, json};

use super::dictionary::{name, tag_text};
use super::element::is_text_vr;
use super::parse::{Element, Item, Value as V};
use super::phi::{WITHHELD, kind_of};
use crate::util::num;

/// Items of one sequence listed in full; longer sequences (per-frame functional groups of a
/// whole-slide image) keep the first ones and say how many there are.
const MAX_ITEMS: usize = 64;
/// Longest text kept per value.
const MAX_TEXT: usize = 4096;

/// The JSON tree of `item`.
pub(crate) fn tree(item: &Item) -> Value {
    Value::Object(object(item, 0))
}

fn object(item: &Item, depth: u32) -> Map<String, Value> {
    let mut m = Map::new();
    for e in item {
        let mut key = name(e.tag);
        if m.contains_key(&key) {
            key = tag_text(e.tag);
        }
        m.insert(key, value(e, depth));
    }
    m
}

fn value(e: &Element, depth: u32) -> Value {
    if kind_of(e).is_some() {
        return Value::from(WITHHELD);
    }
    // Free text in private elements (odd groups) can hold anything, names included; private
    // creator names (gggg,0010-00FF) only name the vendor's block.
    if (e.tag >> 16) % 2 == 1
        && (e.tag & 0xFFFF) > 0x00FF
        && matches!(&e.vr, b"LO" | b"SH" | b"LT" | b"ST" | b"UT" | b"UC")
    {
        return Value::from(WITHHELD);
    }
    let vr = String::from_utf8_lossy(&e.vr).to_string();
    match &e.value {
        V::Seq(items) => {
            if depth > 12 {
                return json!({"vr": "SQ", "items": items.len()});
            }
            let mut v: Vec<Value> = items
                .iter()
                .take(MAX_ITEMS)
                .map(|i| Value::Object(object(i, depth + 1)))
                .collect();
            if items.len() > MAX_ITEMS {
                v.push(json!({"truncated": true, "items": items.len()}));
            }
            Value::Array(v)
        }
        V::Deferred { len, .. } => json!({"vr": vr, "bytes": len}),
        V::Encapsulated { .. } => json!({"vr": vr, "encapsulated": true}),
        V::Bytes(b) => {
            if is_text_vr(&e.vr) {
                let vals = e.strings();
                let mut vals: Vec<Value> = vals
                    .into_iter()
                    .map(|s| {
                        let s: String = s.chars().take(MAX_TEXT).collect();
                        if matches!(&e.vr, b"DS" | b"IS") {
                            s.trim().parse::<f64>().map_or(Value::from(s), num)
                        } else {
                            Value::from(s)
                        }
                    })
                    .collect();
                return match vals.len() {
                    0 => Value::Null,
                    1 => vals.remove(0),
                    _ => Value::Array(vals),
                };
            }
            if &e.vr == b"AT" {
                let tags: Vec<Value> = e
                    .tags()
                    .into_iter()
                    .map(|t| Value::from(tag_text(t)))
                    .collect();
                return if tags.len() == 1 {
                    tags.into_iter().next().unwrap_or(Value::Null)
                } else {
                    Value::Array(tags)
                };
            }
            if matches!(
                &e.vr,
                b"US" | b"SS" | b"UL" | b"SL" | b"FL" | b"FD" | b"UV" | b"SV"
            ) {
                let n = e.numbers();
                if n.len() <= 64 {
                    return match n.len() {
                        0 => Value::Null,
                        1 => num(n[0]),
                        _ => Value::Array(n.into_iter().map(num).collect()),
                    };
                }
                return json!({"vr": vr, "values": n.len()});
            }
            // UN values (often private elements) are not decoded: they may hold personal data
            json!({"vr": vr, "bytes": b.len()})
        }
    }
}
