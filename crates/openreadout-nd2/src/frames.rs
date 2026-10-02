//! Per-frame acquisition records: the custom-data tag table (`CustomDataVar|CustomDataV2_0!`),
//! the per-frame arrays it describes (`CustomData|<tag>!`), acquisition times
//! (`CustomData|AcqTimesCache!`) and, for legacy files, the per-frame XML (`VIMD`).
//! Field names: `docs/formats/nd2.md` § Per-frame records.

use openreadout_core::bytes::utf16le_z;
use serde_json::{Map, Value, json};

use crate::meta::{jdn_to_iso8601, list_items, unwrap_root};

/// One column of per-frame custom data.
#[derive(Debug, Clone)]
pub struct CustomTag {
    /// Chunk suffix: the column lives in `CustomData|<tag_id>!`.
    pub tag_id: String,
    pub description: String,
    pub unit: String,
    /// 1 = UTF-16 strings in equal slots, 2 = i32, 3 = f64.
    pub value_kind: i64,
    /// Number of values (frames).
    pub size: u64,
}

/// Parse the tag table (`CustomTagDescription_v1.0/Tag<n>/{ID, Type, Size, Desc, Unit}`).
pub fn custom_tags_from_lv(v: &Value) -> Vec<CustomTag> {
    let root = unwrap_root(v);
    let table = root
        .get("CustomTagDescription_v1.0")
        .or_else(|| root.get("CustomTagDescription_v1"))
        .unwrap_or(root);
    let Some(o) = table.as_object() else {
        return Vec::new();
    };
    let mut keyed: Vec<(u32, &Value)> = o
        .iter()
        .filter_map(|(k, t)| {
            k.strip_prefix("Tag")
                .and_then(|n| n.parse::<u32>().ok())
                .map(|n| (n, t))
        })
        .collect();
    keyed.sort_by_key(|(n, _)| *n);
    let s = |t: &Value, k: &str| {
        t.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    keyed
        .into_iter()
        .filter_map(|(_, t)| {
            let tag_id = s(t, "ID");
            (!tag_id.is_empty()).then(|| CustomTag {
                tag_id,
                description: s(t, "Desc"),
                unit: s(t, "Unit").trim().to_string(),
                value_kind: t.get("Type").and_then(Value::as_i64).unwrap_or(0),
                size: t.get("Size").and_then(Value::as_u64).unwrap_or(0),
            })
        })
        .collect()
}

/// Little-endian f64 array (`AcqTimesCache`, stage positions).
pub fn f64_values(raw: &[u8]) -> Vec<f64> {
    raw.as_chunks::<8>()
        .0
        .iter()
        .map(|c| f64::from_le_bytes(*c))
        .collect()
}

/// Decode a tag column.
pub fn tag_values(tag: &CustomTag, raw: &[u8]) -> Vec<Value> {
    let num = |f: f64| serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number);
    match tag.value_kind {
        3 => f64_values(raw).into_iter().map(num).collect(),
        2 => raw
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| Value::from(i32::from_le_bytes(*c)))
            .collect(),
        1 => {
            let n = usize::try_from(tag.size).unwrap_or(0).max(1);
            let slot = raw.len() / n;
            if slot < 2 {
                return Vec::new();
            }
            raw.chunks(slot)
                .take(n)
                .map(|w| Value::String(utf16le_z(w)))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// Normalized record field for a tag id, if we name it.
pub fn record_field(tag_id: &str) -> Option<&'static str> {
    Some(match tag_id {
        "X" => "stage_x_um",
        "Y" => "stage_y_um",
        "Z" => "stage_z_um",
        "PFS_STATUS" => "pfs_status",
        "PFS_OFFSET" => "pfs_offset",
        "Camera_ExposureTime1" => "exposure_ms",
        "CameraTemp1" => "camera_temperature_c",
        _ => return None,
    })
}

/// The tag that feeds `stage_z_um`: `Z` when present, else the lowest-numbered `Z<n>` drive.
pub fn stage_z_tag(tags: &[CustomTag]) -> Option<&str> {
    if tags.iter().any(|t| t.tag_id == "Z") {
        return Some("Z");
    }
    tags.iter()
        .filter(|t| {
            t.tag_id.len() > 1
                && t.tag_id.starts_with('Z')
                && t.tag_id[1..].chars().all(|c| c.is_ascii_digit())
        })
        .min_by_key(|t| t.tag_id[1..].parse::<u32>().unwrap_or(u32::MAX))
        .map(|t| t.tag_id.as_str())
}

/// Start a record: sequence index, image coordinates, relative and absolute time.
pub fn base_record(
    frame: u32,
    t: u32,
    z: u32,
    time_ms: Option<f64>,
    start_jdn: Option<f64>,
) -> Map<String, Value> {
    let mut r = Map::new();
    r.insert("frame".into(), Value::from(frame));
    r.insert("t".into(), Value::from(t));
    r.insert("z".into(), Value::from(z));
    if let Some(ms) = time_ms.filter(|m| m.is_finite()) {
        r.insert("time_ms".into(), json!(ms));
        if let Some(iso) = start_jdn.and_then(|j| jdn_to_iso8601(j + ms / 86_400_000.0)) {
            r.insert("acquired_at".into(), Value::String(iso));
        }
    }
    r
}

/// Record fields from one legacy per-frame XML box (`VIMD`).
pub fn legacy_record_fields(r: &mut Map<String, Value>, vimd: &Value) {
    let num = |k: &str| vimd.get(k).and_then(Value::as_f64);
    for (k, field) in [
        ("dXPos", "stage_x_um"),
        ("dYPos", "stage_y_um"),
        ("dZPos", "stage_z_um"),
    ] {
        if let Some(v) = num(k) {
            r.insert(field.into(), json!(v));
        }
    }
    let exposures: Vec<Value> = vimd
        .get("sPicturePlanes")
        .and_then(|p| p.get("sPlane"))
        .map(list_items)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| {
            p.get("sCameraSetting")
                .and_then(|c| c.get("dExposure"))
                .and_then(Value::as_f64)
        })
        .map(|e| json!(e))
        .collect();
    if !exposures.is_empty() {
        r.insert("exposure_ms_per_channel".into(), Value::Array(exposures));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_table_and_columns() {
        let v = json!({"CustomTagDescription_v1.0": {
            "Tag1": {"ID": "PFS_STATUS", "Type": 2, "Size": 2, "Desc": "PFS Status", "Unit": ""},
            "Tag0": {"ID": "X", "Type": 3, "Size": 2, "Desc": "X Coord", "Unit": "µm"},
            "Tag2": {"ID": "Z2", "Type": 3, "Size": 2, "Desc": "Ti ZDrive", "Unit": "µm"}}});
        let tags = custom_tags_from_lv(&v);
        assert_eq!(tags[0].tag_id, "X");
        assert_eq!(stage_z_tag(&tags), Some("Z2"));
        let mut raw = Vec::new();
        raw.extend_from_slice(&1.5f64.to_le_bytes());
        raw.extend_from_slice(&(-2.0f64).to_le_bytes());
        assert_eq!(tag_values(&tags[0], &raw), vec![json!(1.5), json!(-2.0)]);
        let raw: Vec<u8> = [7i32, -1].iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(tag_values(&tags[1], &raw), vec![json!(7), json!(-1)]);
        let s = CustomTag {
            tag_id: "S".into(),
            description: String::new(),
            unit: String::new(),
            value_kind: 1,
            size: 2,
        };
        let raw: Vec<u8> = "ab\0\0cd\0\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(tag_values(&s, &raw), vec![json!("ab"), json!("cd")]);
    }

    #[test]
    fn record_times() {
        let r = base_record(3, 1, 2, Some(1000.0), Some(2_459_486.0));
        assert_eq!(r["time_ms"], 1000.0);
        assert_eq!(r["acquired_at"], "2021-09-28T12:00:01.000Z");
    }
}
