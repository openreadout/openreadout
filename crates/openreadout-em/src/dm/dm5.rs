//! DM5: the same tag tree as DM3/DM4, stored in HDF5 (`docs/formats/dm.md`, "DM5").
//!
//! Groups are tag groups, attributes are data tags, and an image's pixels are the HDF5 dataset
//! `ImageList/[i]/ImageData/Data`. Entries named `[k]` are the unnamed entries of a DM tag
//! group (list items and `Dimensions`), in `k` order.

use std::collections::HashMap;

use hdf5_pure::{AttrValue, Datatype, DatatypeByteOrder, Layout};
use serde_json::{Value, json};

use super::tags::{Tag, TagGroup};

/// Deepest group nesting read (DM tag trees are shallow; this bounds malformed files).
const MAX_DM5_DEPTH: usize = 64;
/// Most groups read in one file (a real DM5 tree has a few thousand; a malformed file whose
/// groups link back to their parents would otherwise be walked exponentially).
const MAX_DM5_GROUPS: usize = 50_000;

/// Where the pixels of one `ImageList` entry are, in a DM5 file.
#[derive(Debug, Clone, PartialEq)]
pub struct Dm5Data {
    /// HDF5 path of the `Data` dataset.
    pub path: String,
    /// HDF5 dataset shape (slowest first).
    pub shape: Vec<u64>,
    /// Bytes per stored element.
    pub element_bytes: u64,
    /// Absolute byte range of contiguous storage; `None` for chunked or compact datasets.
    pub contiguous: Option<(u64, u64)>,
    /// Numbers stored big-endian.
    pub big_endian: bool,
}

fn big_endian(d: &Datatype) -> bool {
    match d {
        Datatype::FixedPoint { byte_order, .. } | Datatype::FloatingPoint { byte_order, .. } => {
            *byte_order == DatatypeByteOrder::BigEndian
        }
        Datatype::Compound { members, .. } => {
            members.first().is_some_and(|m| big_endian(&m.datatype))
        }
        Datatype::Array { base_type, .. } => big_endian(base_type),
        _ => false,
    }
}

/// `[k]` → `k`.
fn list_index(name: &str) -> Option<u64> {
    name.strip_prefix('[')?.strip_suffix(']')?.parse().ok()
}

fn attr_json(v: &AttrValue) -> Value {
    fn nums<T: Copy + Into<f64>>(a: &[T]) -> Value {
        if a.len() == 1 {
            json!(a[0].into())
        } else {
            Value::Array(a.iter().map(|x| json!((*x).into())).collect())
        }
    }
    match v {
        AttrValue::F32(x) => json!(f64::from(*x)),
        AttrValue::F64(x) => json!(*x),
        AttrValue::I8(x) => json!(*x),
        AttrValue::I16(x) => json!(*x),
        AttrValue::I32(x) => json!(*x),
        AttrValue::I64(x) => json!(*x),
        AttrValue::U8(x) => json!(*x),
        AttrValue::U16(x) => json!(*x),
        AttrValue::U32(x) => json!(*x),
        AttrValue::U64(x) => json!(*x),
        AttrValue::F32Array(a) => nums(a),
        AttrValue::F64Array(a) => nums(a),
        AttrValue::I8Array(a) => nums(a),
        AttrValue::I16Array(a) => nums(a),
        AttrValue::I32Array(a) => nums(a),
        AttrValue::U8Array(a) => nums(a),
        AttrValue::U16Array(a) => nums(a),
        AttrValue::U32Array(a) => nums(a),
        AttrValue::I64Array(a) => Value::Array(a.iter().map(|x| json!(x)).collect()),
        AttrValue::U64Array(a) => Value::Array(a.iter().map(|x| json!(x)).collect()),
        AttrValue::String(s) | AttrValue::AsciiString(s) => Value::from(s.trim_end_matches('\0')),
        AttrValue::StringSized { value, .. } | AttrValue::AsciiStringSized { value, .. } => {
            Value::from(value.trim_end_matches('\0'))
        }
        AttrValue::StringArray(a) | AttrValue::AsciiStringArray(a) => {
            Value::Array(a.iter().map(|s| Value::from(s.as_str())).collect())
        }
        _ => Value::Null,
    }
}

/// Sort entries: named ones by name, then `[k]` ones (as unnamed entries) by `k`.
fn ordered(mut named: Vec<(String, Tag)>, mut listed: Vec<(u64, Tag)>) -> Vec<(String, Tag)> {
    named.sort_by(|a, b| a.0.cmp(&b.0));
    listed.sort_by_key(|(k, _)| *k);
    named
        .into_iter()
        .chain(listed.into_iter().map(|(_, t)| (String::new(), t)))
        .collect()
}

/// Groups visited so far and the most the walk may visit.
struct Walk {
    visited: usize,
    limit: usize,
}

/// Read the HDF5 tree below `g` as a DM tag group; `Data` datasets found at
/// `ImageList/[i]/ImageData/Data` are recorded in `data` by list index.
fn read_group(
    group: &hdf5_pure::Group,
    path: &str,
    depth: usize,
    walk: &mut Walk,
    data: &mut HashMap<usize, Dm5Data>,
    issues: &mut Vec<String>,
) -> TagGroup {
    let mut named = Vec::new();
    let mut listed = Vec::new();
    if depth > MAX_DM5_DEPTH {
        issues.push(format!("{path}: groups nested deeper than {MAX_DM5_DEPTH}"));
        return TagGroup::default();
    }
    walk.visited += 1;
    if walk.visited > walk.limit {
        if walk.visited == walk.limit + 1 {
            issues.push(format!(
                "more than {} groups (a group linked into its own subtree?); the rest of the tree is not read",
                walk.limit
            ));
        }
        return TagGroup::default();
    }
    match group.attrs() {
        Ok(attrs) => {
            for (name, v) in attrs {
                let tag = Tag::Data(super::tags::TagValue::Json(attr_json(&v)));
                match list_index(&name) {
                    Some(k) => listed.push((k, tag)),
                    None => named.push((name, tag)),
                }
            }
        }
        Err(e) => issues.push(format!("{path}: attributes unreadable: {e}")),
    }
    let sub = |n: &str| {
        if path.is_empty() {
            n.to_string()
        } else {
            format!("{path}/{n}")
        }
    };
    for name in group.groups().unwrap_or_default() {
        if walk.visited > walk.limit {
            break;
        }
        // Opened from the parent's handle: resolving the full path from the root at every
        // visit made a cyclic file (groups hard-linked into their own subtree) take ~20 s.
        let g = match group.group(&name) {
            Ok(child) => read_group(&child, &sub(&name), depth + 1, walk, data, issues),
            Err(e) => {
                issues.push(format!("{}: {e}", sub(&name)));
                TagGroup::default()
            }
        };
        match list_index(&name) {
            Some(k) => listed.push((k, Tag::Group(g))),
            None => named.push((name, Tag::Group(g))),
        }
    }
    for name in group.datasets().unwrap_or_default() {
        let p = sub(&name);
        let Ok(ds) = group.dataset(&name) else {
            issues.push(format!("{p}: dataset unreadable"));
            continue;
        };
        let shape = ds.shape().unwrap_or_default();
        let element_bytes = ds.element_size().unwrap_or(0);
        let contiguous = match ds.layout() {
            Ok(Layout::Contiguous {
                address: Some(a),
                size,
            }) => Some((a, size)),
            _ => None,
        };
        let big = ds.datatype().is_ok_and(|d| big_endian(&d));
        let count: u64 = shape.iter().product();
        // `ImageList/[i]/ImageData/Data`
        let parts: Vec<&str> = p.split('/').collect();
        if let ["ImageList", item, "ImageData", "Data"] = parts.as_slice()
            && let Some(k) = list_index(item).and_then(|k| usize::try_from(k).ok())
        {
            data.insert(
                k,
                Dm5Data {
                    path: p.clone(),
                    shape: shape.clone(),
                    element_bytes,
                    contiguous,
                    big_endian: big,
                },
            );
        }
        let tag = Tag::Data(super::tags::TagValue::Json(json!({
            "hdf5_dataset": p, "shape": shape, "element_bytes": element_bytes, "count": count,
        })));
        match list_index(&name) {
            Some(k) => listed.push((k, tag)),
            None => named.push((name, tag)),
        }
    }
    TagGroup {
        entries: ordered(named, listed),
    }
}

/// The whole DM5 file as a tag tree, the `Data` dataset of each `ImageList` entry, and
/// problems met (unreadable groups or attributes).
/// `file_len` bounds the walk: a file cannot hold more distinct groups than object headers
/// (at least 16 bytes each) fit in it, so more visits can only be a cycle.
pub(crate) fn read_tree(
    file: &hdf5_pure::File,
    file_len: u64,
) -> (TagGroup, HashMap<usize, Dm5Data>, Vec<String>) {
    let mut data = HashMap::new();
    let mut issues = Vec::new();
    let limit = MAX_DM5_GROUPS.min(
        usize::try_from(file_len / 16)
            .unwrap_or(usize::MAX)
            .max(1024),
    );
    let mut walk = Walk { visited: 0, limit };
    let root = read_group(&file.root(), "", 0, &mut walk, &mut data, &mut issues);
    (root, data, issues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_names() {
        assert_eq!(list_index("[0]"), Some(0));
        assert_eq!(list_index("[12]"), Some(12));
        assert_eq!(list_index("Data"), None);
        assert_eq!(list_index("[x]"), None);
    }

    #[test]
    fn listed_entries_follow_named_ones_in_index_order() {
        let d = |v: i64| Tag::Data(super::super::tags::TagValue::Json(json!(v)));
        let e = ordered(
            vec![("b".into(), d(1)), ("a".into(), d(2))],
            vec![(10, d(3)), (2, d(4))],
        );
        let names: Vec<&str> = e.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["a", "b", "", ""]);
        assert_eq!(e[2].1, d(4));
    }
}
