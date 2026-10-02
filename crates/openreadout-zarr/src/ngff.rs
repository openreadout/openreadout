//! OME-NGFF group metadata (open specification, <https://ngff.openmicroscopy.org>): versions
//! 0.1–0.5, `multiscales` (axes, datasets, coordinate transformations), `omero`, `labels`,
//! `plate`, `well` and the `bioformats2raw.layout` convention. Pure functions of JSON.

use serde_json::Value;

use crate::store::{Store, join};
use openreadout_core::Result;

/// Which Zarr version stores a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZarrFormat {
    V2,
    V3,
}

impl ZarrFormat {
    pub fn number(self) -> u32 {
        match self {
            ZarrFormat::V2 => 2,
            ZarrFormat::V3 => 3,
        }
    }
}

/// The NGFF attributes of a group: for Zarr v3 (NGFF 0.5) the `ome` object inside
/// `attributes`, for Zarr v2 the `.zattrs` document. `None` when the group does not exist.
#[derive(Debug, Clone)]
pub struct GroupAttrs {
    pub format: ZarrFormat,
    /// NGFF keys (`multiscales`, `omero`, `plate`, `well`, `labels`, ...).
    pub ngff: Value,
    /// The whole attribute document as stored.
    pub raw: Value,
}

/// Read a group's attributes (`zarr.json` first, then `.zattrs`/`.zgroup`).
pub fn group_attrs(store: &Store, group: &str) -> Result<Option<GroupAttrs>> {
    if let Some(doc) = store.json(&join(group, "zarr.json"))? {
        let attrs = doc.get("attributes").cloned().unwrap_or(Value::Null);
        let ngff = match attrs.get("ome") {
            Some(o) if o.is_object() => o.clone(),
            _ => attrs.clone(),
        };
        return Ok(Some(GroupAttrs {
            format: ZarrFormat::V3,
            ngff,
            raw: attrs,
        }));
    }
    let attrs = store.json(&join(group, ".zattrs"))?;
    if attrs.is_none() && !store.exists(&join(group, ".zgroup")) {
        return Ok(None);
    }
    let attrs = attrs.unwrap_or(Value::Null);
    Ok(Some(GroupAttrs {
        format: ZarrFormat::V2,
        ngff: attrs.clone(),
        raw: attrs,
    }))
}

/// One axis of a multiscales image.
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    pub name: String,
    /// `space`, `time`, `channel` or a custom type.
    pub kind: Option<String>,
    pub unit: Option<String>,
}

/// One resolution level (`datasets[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    /// Array path relative to the store root.
    pub path: String,
    /// Scale per axis (dataset scale times the multiscales-wide scale); 1 when absent.
    pub scale: Vec<f64>,
    /// Translation per axis (the coordinate of the first sample, dataset and multiscales-wide
    /// translations composed), when either is given.
    pub translation: Option<Vec<f64>>,
}

/// One `multiscales` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Multiscale {
    /// Group path relative to the store root.
    pub group: String,
    pub name: Option<String>,
    pub version: Option<String>,
    pub axes: Vec<Axis>,
    /// True when the metadata names no axes (NGFF 0.1/0.2: implicitly `t, c, z, y, x`).
    pub implied_axes: bool,
    pub levels: Vec<Level>,
    /// Downsampling method (`type`), when given.
    pub method: Option<String>,
}

fn str_of(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn f64_list(v: Option<&Value>) -> Option<Vec<f64>> {
    v?.as_array()?.iter().map(Value::as_f64).collect()
}

/// `scale` and `translation` of a `coordinateTransformations` list.
fn transforms(v: Option<&Value>) -> (Option<Vec<f64>>, Option<Vec<f64>>) {
    let mut scale = None;
    let mut translation = None;
    for t in v.and_then(Value::as_array).into_iter().flatten() {
        match t.get("type").and_then(Value::as_str) {
            Some("scale") => scale = f64_list(t.get("scale")),
            Some("translation") => translation = f64_list(t.get("translation")),
            _ => {}
        }
    }
    (scale, translation)
}

/// Parse every `multiscales` entry of a group's NGFF attributes. `version` is the group-level
/// NGFF version (0.5 keeps it in the `ome` object), used when an entry carries none.
#[allow(clippy::many_single_char_names)]
pub fn multiscales(group: &str, ngff: &Value) -> Vec<Multiscale> {
    let group_version = str_of(ngff.get("version"));
    let mut out = Vec::new();
    for m in ngff
        .get("multiscales")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (axes, implied) = match m.get("axes").and_then(Value::as_array) {
            Some(list) => (
                list.iter()
                    .map(|a| match a {
                        Value::String(s) => Axis {
                            name: s.clone(),
                            kind: None,
                            unit: None,
                        },
                        _ => Axis {
                            name: str_of(a.get("name")).unwrap_or_default(),
                            kind: str_of(a.get("type")),
                            unit: str_of(a.get("unit")),
                        },
                    })
                    .collect(),
                false,
            ),
            None => (
                ["t", "c", "z", "y", "x"]
                    .iter()
                    .map(|n| Axis {
                        name: (*n).to_string(),
                        kind: None,
                        unit: None,
                    })
                    .collect::<Vec<_>>(),
                true,
            ),
        };
        let n = axes.len();
        let (g_scale, g_trans) = transforms(m.get("coordinateTransformations"));
        let levels = m
            .get("datasets")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|d| {
                let path = str_of(d.get("path"))?;
                let (s, t) = transforms(d.get("coordinateTransformations"));
                let mut scale = s.filter(|s| s.len() == n).unwrap_or_else(|| vec![1.0; n]);
                let g = g_scale.as_ref().filter(|g| g.len() == n);
                // the dataset transform comes first, then the multiscales-wide one:
                // coordinate = g_scale * (d_scale * index + d_trans) + g_trans
                let t = t.filter(|t| t.len() == n);
                let gt = g_trans.as_ref().filter(|t| t.len() == n);
                let translation = (t.is_some() || gt.is_some()).then(|| {
                    (0..n)
                        .map(|k| {
                            let d = t.as_ref().map_or(0.0, |t| t[k]);
                            g.map_or(1.0, |g| g[k]) * d + gt.map_or(0.0, |t| t[k])
                        })
                        .collect()
                });
                if let Some(g) = g {
                    for (a, b) in scale.iter_mut().zip(g) {
                        *a *= b;
                    }
                }
                Some(Level {
                    path: join(group, &path),
                    scale,
                    translation,
                })
            })
            .collect();
        out.push(Multiscale {
            group: group.to_string(),
            name: str_of(m.get("name")),
            version: str_of(m.get("version")).or_else(|| group_version.clone()),
            axes,
            implied_axes: implied,
            levels,
            method: str_of(m.get("type")),
        });
    }
    out
}

/// Length unit (NGFF uses UDUNITS-2 names) → micrometres per unit.
pub fn length_um(unit: &str) -> Option<f64> {
    Some(match unit.trim() {
        "angstrom" | "Å" => 1e-4,
        "picometer" => 1e-6,
        "nanometer" | "nm" => 1e-3,
        "micrometer" | "micron" | "µm" | "um" | "μm" => 1.0,
        "millimeter" | "mm" => 1e3,
        "centimeter" | "cm" => 1e4,
        "decimeter" => 1e5,
        "meter" | "m" => 1e6,
        "kilometer" => 1e9,
        "inch" => 25_400.0,
        "foot" => 304_800.0,
        _ => return None,
    })
}

/// Time unit → seconds per unit.
pub fn time_s(unit: &str) -> Option<f64> {
    Some(match unit.trim() {
        "picosecond" => 1e-12,
        "nanosecond" => 1e-9,
        "microsecond" => 1e-6,
        "millisecond" | "ms" => 1e-3,
        "centisecond" => 1e-2,
        "decisecond" => 1e-1,
        "second" | "s" => 1.0,
        "minute" | "min" => 60.0,
        "hour" | "h" => 3600.0,
        "day" => 86_400.0,
        _ => return None,
    })
}

/// NGFF colour (`RRGGBB`, sometimes with `#`) → `#RRGGBB`.
pub fn color(v: Option<&Value>) -> Option<String> {
    let s = v?.as_str()?.trim().trim_start_matches('#');
    (s.len() == 6 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", s.to_ascii_uppercase()))
}

/// The NGFF version of a group's attributes: `ome.version` (0.5), or the version inside
/// `multiscales[0]`, `plate`, `well` or `image-label` (0.1–0.4).
pub fn version_of(ngff: &Value) -> Option<String> {
    str_of(ngff.get("version"))
        .or_else(|| {
            ngff.get("multiscales")
                .and_then(|m| m.get(0))
                .and_then(|m| str_of(m.get("version")))
        })
        .or_else(|| ngff.get("plate").and_then(|p| str_of(p.get("version"))))
        .or_else(|| ngff.get("well").and_then(|p| str_of(p.get("version"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn axes_scales_and_versions() {
        let a = json!({"multiscales": [{"version": "0.4", "name": "n",
            "axes": [{"name": "c", "type": "channel"}, {"name": "y", "type": "space", "unit": "micrometer"}, {"name": "x", "type": "space", "unit": "micrometer"}],
            "datasets": [{"path": "0", "coordinateTransformations": [{"type": "scale", "scale": [1.0, 0.5, 0.5]}]},
                         {"path": "1", "coordinateTransformations": [{"type": "scale", "scale": [1.0, 1.0, 1.0]}, {"type": "translation", "translation": [0.0, 0.25, 0.25]}]}],
            "coordinateTransformations": [{"type": "scale", "scale": [1.0, 2.0, 2.0]}]}]});
        let m = multiscales("well/0", &a);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].levels[0].path, "well/0/0");
        assert_eq!(m[0].levels[0].scale, vec![1.0, 1.0, 1.0]);
        // dataset translation then the multiscales-wide scale: 2 x 0.25
        assert_eq!(m[0].levels[1].translation, Some(vec![0.0, 0.5, 0.5]));
        assert_eq!(m[0].axes[1].unit.as_deref(), Some("micrometer"));
        assert_eq!(version_of(&a).as_deref(), Some("0.4"));
        // 0.2: no axes → t c z y x
        let old = json!({"multiscales": [{"version": "0.2", "datasets": [{"path": "0"}]}]});
        let m = multiscales("", &old);
        assert!(m[0].implied_axes);
        assert_eq!(m[0].axes.len(), 5);
        assert_eq!(m[0].levels[0].scale, vec![1.0; 5]);
        // 0.3: axes as strings
        let v3 = json!({"multiscales": [{"axes": ["z", "y", "x"], "datasets": [{"path": "s0"}]}], "version": "0.5"});
        let m = multiscales("", &v3);
        assert_eq!(m[0].axes[0].name, "z");
        assert_eq!(m[0].version.as_deref(), Some("0.5"));
        assert_eq!(length_um("nanometer"), Some(1e-3));
        assert_eq!(time_s("minute"), Some(60.0));
        assert_eq!(color(Some(&json!("ff00aa"))).as_deref(), Some("#FF00AA"));
        assert_eq!(color(Some(&json!("red"))), None);
    }
}
