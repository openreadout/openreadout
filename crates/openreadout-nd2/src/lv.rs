//! Decoder for the "LV" typed key-value encoding used by ND2 metadata chunks.
//! See `docs/formats/nd2.md` § LV encoding.

use openreadout_core::bytes::{le_f64, le_i32, le_i64, le_u32, le_u64, utf16le};
use serde_json::{Map, Value};

/// Nesting bound for levels, compressed blocks and nested byte arrays (malformed-input guard).
const MAX_DEPTH: usize = 64;
/// Byte arrays up to this length are exposed as numbers; longer ones as a size placeholder.
const MAX_INLINE_BYTES: usize = 4096;

/// Decode a whole LV buffer into JSON. Repeated names become arrays.
pub fn lv_decode(buf: &[u8]) -> Result<Value, String> {
    let mut out = Map::new();
    decode_into(buf, 0, &mut out)?;
    Ok(Value::Object(out))
}

/// Decode the items of `buf` into `out`. A compressed block (type 76) splices its items into
/// the same level.
fn decode_into(buf: &[u8], depth: usize, out: &mut Map<String, Value>) -> Result<(), String> {
    let mut pos = 0usize;
    while pos < buf.len() {
        match lv_item(buf, pos, depth)? {
            Item::Named(name, val, next) => {
                insert(out, name, val);
                if next <= pos {
                    break;
                }
                pos = next;
            }
            Item::Compressed(inner) => {
                decode_into(&inner, depth + 1, out)?;
                break;
            }
        }
    }
    Ok(())
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

fn utf16_at(buf: &[u8], pos: usize, units: usize) -> Result<String, String> {
    let end = pos
        .checked_add(units.saturating_mul(2))
        .ok_or_else(|| format!("LV name length overflows at {pos}"))?;
    if end > buf.len() {
        return Err(format!("LV name runs past buffer at {pos}"));
    }
    Ok(utf16le(&buf[pos..end]).trim_end_matches('\0').to_string())
}

fn need(buf: &[u8], pos: usize, n: usize) -> Result<(), String> {
    if pos.checked_add(n).is_none_or(|end| end > buf.len()) {
        Err(format!("LV value truncated at {pos}"))
    } else {
        Ok(())
    }
}

fn field<T>(v: Option<T>, pos: usize) -> Result<T, String> {
    v.ok_or_else(|| format!("LV value truncated at {pos}"))
}

/// A byte array is either a nested LV structure (it then starts with a plausible item header:
/// a known type byte and a name of at least one character, or a compressed block) or opaque
/// bytes (masks such as `pItemValid`, matrices). Nested decoding is attempted only for
/// plausible headers and falls back to the bytes on any error.
fn byte_array_value(bytes: &[u8], depth: usize) -> Value {
    let plausible =
        bytes.len() >= 4 && (bytes[0] == 76 || ((1..=11).contains(&bytes[0]) && bytes[1] >= 2));
    if plausible && depth < MAX_DEPTH {
        let mut o = Map::new();
        if decode_into(bytes, depth + 1, &mut o).is_ok() && !o.is_empty() {
            return Value::Object(o);
        }
    }
    if bytes.len() <= MAX_INLINE_BYTES {
        Value::Array(bytes.iter().map(|&b| Value::from(b)).collect())
    } else {
        Value::String(format!("<{} bytes>", bytes.len()))
    }
}

enum Item {
    /// (name, value, next position)
    Named(String, Value, usize),
    /// Inflated payload of a compressed block; it runs to the end of the buffer.
    Compressed(Vec<u8>),
}

/// Decode one item at `pos`.
fn lv_item(buf: &[u8], pos: usize, depth: usize) -> Result<Item, String> {
    need(buf, pos, 2)?;
    let item_type = buf[pos];
    let name_units = buf[pos + 1] as usize;
    if item_type == 76 {
        // `L`: after a 10-byte header the rest of the buffer is a zlib stream of more items.
        if depth >= MAX_DEPTH {
            return Err(format!("LV nesting too deep at {pos}"));
        }
        let packed = buf.get(pos + 10..).unwrap_or_default();
        let raw = openreadout_codecs::zlib_decode(packed, 0)
            .map_err(|e| format!("compressed LV block at {pos}: {e}"))?;
        return Ok(Item::Compressed(raw));
    }
    let name = utf16_at(buf, pos + 2, name_units)?;
    let mut p = pos + 2 + 2 * name_units;
    let val = match item_type {
        1 => {
            need(buf, p, 1)?;
            let v = Value::Bool(buf[p] != 0);
            p += 1;
            v
        }
        2 => {
            let v = Value::from(field(le_i32(buf, p), p)?);
            p += 4;
            v
        }
        3 => {
            let v = Value::from(field(le_u32(buf, p), p)?);
            p += 4;
            v
        }
        4 => {
            let v = Value::from(field(le_i64(buf, p), p)?);
            p += 8;
            v
        }
        5 | 7 => {
            let v = Value::from(field(le_u64(buf, p), p)?);
            p += 8;
            v
        }
        6 => {
            let f = field(le_f64(buf, p), p)?;
            p += 8;
            serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
        }
        8 => {
            let mut k = p;
            while k + 1 < buf.len() && !(buf[k] == 0 && buf[k + 1] == 0) {
                k += 2;
            }
            let units = (k.min(buf.len()) - p) / 2;
            let s = utf16_at(buf, p, units)?;
            p = (k + 2).min(buf.len());
            Value::String(s)
        }
        9 => {
            let len = usize::try_from(field(le_u64(buf, p), p)?)
                .map_err(|_| format!("LV byte array length overflows at {pos}"))?;
            p += 8;
            need(buf, p, len)?;
            let v = byte_array_value(&buf[p..p + len], depth);
            p += len;
            v
        }
        11 => {
            need(buf, p, 12)?;
            if depth >= MAX_DEPTH {
                return Err(format!("LV nesting too deep at {pos}"));
            }
            let child_count = le_u32(buf, p).unwrap_or(0) as usize;
            p += 12;
            let mut level = Map::new();
            for _ in 0..child_count {
                if p >= buf.len() {
                    break;
                }
                match lv_item(buf, p, depth + 1)? {
                    Item::Named(n, v, next) => {
                        insert(&mut level, n, v);
                        p = next;
                    }
                    Item::Compressed(inner) => {
                        decode_into(&inner, depth + 1, &mut level)?;
                        p = buf.len();
                    }
                }
            }
            p = p.saturating_add(child_count.saturating_mul(8));
            Value::Object(level)
        }
        t => return Err(format!("unknown LV item type {t} at {pos} (name {name:?})")),
    };
    Ok(Item::Named(name, val, p.min(buf.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_scalars_and_levels() {
        // level "L" with one u32 child "n" = 7
        let mut b = vec![11u8, 2, b'L', 0, 0, 0];
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&[3u8, 2, b'n', 0, 0, 0]);
        b.extend_from_slice(&7u32.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes()); // offset table
        let v = lv_decode(&b).unwrap();
        assert_eq!(v["L"]["n"], 7);
    }

    #[test]
    fn byte_arrays_stay_numbers_unless_nested_lv() {
        // "m" = bytes [1, 0, 1] (a validity mask, not a plausible LV header)
        let mut b = vec![9u8, 2, b'm', 0, 0, 0];
        b.extend_from_slice(&3u64.to_le_bytes());
        b.extend_from_slice(&[1, 0, 1]);
        let v = lv_decode(&b).unwrap();
        assert_eq!(v["m"], serde_json::json!([1, 0, 1]));
        // "x" = nested LV holding u32 "ab" = 5
        let mut inner = vec![3u8, 3, b'a', 0, b'b', 0, 0, 0];
        inner.extend_from_slice(&5u32.to_le_bytes());
        let mut b = vec![9u8, 2, b'x', 0, 0, 0];
        b.extend_from_slice(&(inner.len() as u64).to_le_bytes());
        b.extend_from_slice(&inner);
        let v = lv_decode(&b).unwrap();
        assert_eq!(v["x"]["ab"], 5);
    }

    #[test]
    fn truncated_input_is_an_error_not_a_panic() {
        let b = vec![
            9u8, 2, b'm', 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F,
        ];
        assert!(lv_decode(&b).is_err());
        assert!(lv_decode(&[11u8, 1, 0, 0]).is_err());
    }
}
