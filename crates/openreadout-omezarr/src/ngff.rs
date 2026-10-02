//! OME-NGFF 0.5 group attributes (`multiscales`, `omero`, `bioformats2raw.layout`, `series`).
//! Spec: <https://ngff.openmicroscopy.org/0.5/> (open standard). Pure functions of the
//! normalized [`ImageInfo`], so they can be unit-tested without writing anything.

use openreadout_core::PixelType;
use openreadout_core::model::ImageInfo;
use serde_json::{Value, json};

/// The OME-NGFF version written.
pub const NGFF_VERSION: &str = "0.5";

/// Axis names in array order. OME-NGFF requires time, then channel, then space (`zyx`).
pub const AXES: [&str; 5] = ["t", "c", "z", "y", "x"];

/// Planes with a side above this many pixels get a pyramid when the level count is automatic.
pub const AUTO_PYRAMID_THRESHOLD: u32 = 1024;

/// Default display colours for channels whose file records none (cycled).
const DEFAULT_COLORS: [&str; 6] = ["00FF00", "FF00FF", "00FFFF", "FF0000", "0000FF", "FFFF00"];

/// Width and height of every resolution level, largest first. Each level halves the previous
/// one (rounding up, so edge pixels are kept). With `levels == None` levels are added while
/// the current level is larger than [`AUTO_PYRAMID_THRESHOLD`] in either dimension; an
/// explicit count stops early once a level is 1×1.
pub fn plan_levels(width: u32, height: u32, levels: Option<u32>) -> Vec<(u32, u32)> {
    let mut out = vec![(width.max(1), height.max(1))];
    loop {
        let &(w, h) = out.last().expect("non-empty");
        let more = match levels {
            None => w > AUTO_PYRAMID_THRESHOLD || h > AUTO_PYRAMID_THRESHOLD,
            Some(n) => (out.len() as u32) < n.max(1) && (w > 1 || h > 1),
        };
        if !more {
            return out;
        }
        out.push((w.div_ceil(2), h.div_ceil(2)));
    }
}

/// Per-level `scale` vectors (t, c, z, y, x). Space axes use the physical pixel size in
/// micrometres when known, otherwise the downsampling factor relative to level 0 (as the
/// spec requires); time uses `time_increment_s` when known.
pub fn level_scales(im: &ImageInfo, level_sizes: &[(u32, u32)]) -> Vec<[f64; 5]> {
    let (w0, h0) = level_sizes.first().copied().unwrap_or((1, 1));
    level_sizes
        .iter()
        .map(|&(w, h)| {
            let fx = f64::from(w0) / f64::from(w.max(1));
            let fy = f64::from(h0) / f64::from(h.max(1));
            // Exact powers of two for the pyramid we build (ceil-halving can make the ratio
            // slightly off for odd sizes; the nominal factor is what readers expect).
            let fx = fx.log2().round().exp2();
            let fy = fy.log2().round().exp2();
            [
                im.time_increment_s.filter(|v| *v > 0.0).unwrap_or(1.0),
                1.0,
                im.physical_size.z.filter(|v| *v > 0.0).unwrap_or(1.0),
                im.physical_size.y.filter(|v| *v > 0.0).unwrap_or(1.0) * fy,
                im.physical_size.x.filter(|v| *v > 0.0).unwrap_or(1.0) * fx,
            ]
        })
        .collect()
}

/// The `axes` list. Units are given only for axes whose physical size is known.
pub fn axes(im: &ImageInfo) -> Value {
    let space = |known: Option<f64>, name: &str| {
        let mut a = json!({"name": name, "type": "space"});
        if known.is_some_and(|v| v > 0.0) {
            a["unit"] = json!("micrometer");
        }
        a
    };
    let mut t = json!({"name": "t", "type": "time"});
    if im.time_increment_s.is_some_and(|v| v > 0.0) {
        t["unit"] = json!("second");
    }
    json!([
        t,
        {"name": "c", "type": "channel"},
        space(im.physical_size.z, "z"),
        space(im.physical_size.y, "y"),
        space(im.physical_size.x, "x"),
    ])
}

/// One `multiscales` entry for an image written with `level_sizes` (paths `"0"`, `"1"`, ...).
pub fn multiscales(im: &ImageInfo, level_sizes: &[(u32, u32)], creator: &str) -> Value {
    let datasets: Vec<Value> = level_scales(im, level_sizes)
        .iter()
        .enumerate()
        .map(|(i, s)| {
            json!({
                "path": i.to_string(),
                "coordinateTransformations": [{"type": "scale", "scale": s}],
            })
        })
        .collect();
    json!({
        "name": image_name(im),
        "axes": axes(im),
        "datasets": datasets,
        "type": "mean",
        "metadata": {
            "description": "Each level is the previous level downsampled 2x in y and x by the mean of 2x2 pixel blocks (partial blocks at odd edges average the pixels present); z, c and t are not downsampled.",
            "method": creator,
        },
    })
}

/// One `multiscales` entry for levels of the given sizes and downsampling factors (relative to
/// level 0, x and y): the streaming writer's source-copied or mean pyramids.
pub fn multiscales_with_factors(
    im: &ImageInfo,
    level_sizes: &[(u32, u32)],
    factors: &[(f64, f64)],
    mode: openreadout_ometiff::PyramidMode,
    creator: &str,
) -> Value {
    let datasets: Vec<Value> = level_sizes
        .iter()
        .zip(factors)
        .enumerate()
        .map(|(i, (_, &(fx, fy)))| {
            let s = [
                im.time_increment_s.filter(|v| *v > 0.0).unwrap_or(1.0),
                1.0,
                im.physical_size.z.filter(|v| *v > 0.0).unwrap_or(1.0),
                im.physical_size.y.filter(|v| *v > 0.0).unwrap_or(1.0) * fy,
                im.physical_size.x.filter(|v| *v > 0.0).unwrap_or(1.0) * fx,
            ];
            json!({
                "path": i.to_string(),
                "coordinateTransformations": [{"type": "scale", "scale": s}],
            })
        })
        .collect();
    let (kind, description) = match mode {
        openreadout_ometiff::PyramidMode::Source => (
            "source",
            "The levels are the source file's own reduced resolutions (its pyramid), copied; z, c and t are not downsampled.",
        ),
        _ => (
            "mean",
            "Each level is the previous level downsampled 2x in y and x by the mean of 2x2 pixel blocks (partial blocks at odd edges average the pixels present); z, c and t are not downsampled.",
        ),
    };
    json!({
        "name": image_name(im),
        "axes": axes(im),
        "datasets": datasets,
        "type": kind,
        "metadata": {
            "description": description,
            "method": creator,
        },
    })
}

/// Display name of an image.
pub fn image_name(im: &ImageInfo) -> String {
    im.name
        .clone()
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("Image {}", im.index))
}

/// Observed sample range of one written (Zarr) channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelRange {
    pub min: f64,
    pub max: f64,
}

impl ChannelRange {
    pub const EMPTY: ChannelRange = ChannelRange {
        min: f64::INFINITY,
        max: f64::NEG_INFINITY,
    };

    pub fn is_empty(&self) -> bool {
        self.min > self.max
    }
}

/// The full representable range of a pixel type (`None` for floating point).
pub fn type_range(p: PixelType) -> Option<(f64, f64)> {
    Some(match p {
        PixelType::Int8 => (f64::from(i8::MIN), f64::from(i8::MAX)),
        PixelType::Int16 => (f64::from(i16::MIN), f64::from(i16::MAX)),
        PixelType::Int32 => (f64::from(i32::MIN), f64::from(i32::MAX)),
        PixelType::Uint8 => (0.0, f64::from(u8::MAX)),
        PixelType::Uint16 => (0.0, f64::from(u16::MAX)),
        PixelType::Uint32 => (0.0, f64::from(u32::MAX)),
        _ => return None,
    })
}

/// Normalize `#RRGGBB` / `RRGGBB` to the six upper-case hex digits OME-NGFF wants.
fn ngff_color(c: &str) -> Option<String> {
    let h = c.trim().trim_start_matches('#');
    (h.len() == 6 && h.chars().all(|ch| ch.is_ascii_hexdigit())).then(|| h.to_ascii_uppercase())
}

/// The transitional `omero` block: one entry per written Zarr channel. `c_map` lists the
/// source channel indices that were exported; interleaved RGB samples (`samples_per_pixel`
/// 3) become three consecutive Zarr channels coloured red, green and blue. `ranges` holds
/// the observed min/max per Zarr channel and sets each channel's display window.
pub fn omero(im: &ImageInfo, c_map: &[u32], ranges: &[ChannelRange], default_z: u32) -> Value {
    let spp = im.samples_per_pixel.max(1) as usize;
    let mut channels = Vec::new();
    for (ci, &orig) in c_map.iter().enumerate() {
        let info = im.channels.iter().find(|c| c.index == orig);
        let base = info
            .and_then(|c| c.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("Channel {orig}"));
        for s in 0..spp {
            let zc = ci * spp + s;
            let (label, color) = if spp == 3 {
                (
                    format!("{base} ({})", ["R", "G", "B"][s]),
                    ["FF0000", "00FF00", "0000FF"][s].to_string(),
                )
            } else if spp > 1 {
                (format!("{base} (sample {s})"), "FFFFFF".to_string())
            } else {
                let color = info
                    .and_then(|c| c.color.as_deref())
                    .and_then(ngff_color)
                    .unwrap_or_else(|| {
                        if c_map.len() == 1 {
                            "FFFFFF".to_string()
                        } else {
                            DEFAULT_COLORS[ci % DEFAULT_COLORS.len()].to_string()
                        }
                    });
                (base.clone(), color)
            };
            let r = ranges.get(zc).copied().unwrap_or(ChannelRange::EMPTY);
            let (dmin, dmax) = if r.is_empty() {
                (0.0, 0.0)
            } else {
                (r.min, r.max)
            };
            let (tmin, tmax) = type_range(im.pixel_type).unwrap_or((dmin, dmax));
            channels.push(json!({
                "active": true,
                "coefficient": 1,
                "color": color,
                "family": "linear",
                "inverted": false,
                "label": label,
                "window": {"start": num(dmin), "end": num(dmax), "min": num(tmin), "max": num(tmax)},
            }));
        }
    }
    json!({
        "name": image_name(im),
        "channels": channels,
        "rdefs": {
            "defaultT": 0,
            "defaultZ": default_z,
            "model": if channels.len() == 1 { "greyscale" } else { "color" },
        },
    })
}

/// Integers as JSON integers, everything else as floats.
fn num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9.0e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

/// `attributes` of an image group.
pub fn image_attributes(multiscales: Value, omero: Value) -> Value {
    json!({"ome": {"version": NGFF_VERSION, "multiscales": [multiscales], "omero": omero}})
}

/// `attributes` of the root of a multi-image collection.
pub fn collection_root_attributes() -> Value {
    json!({"ome": {"version": NGFF_VERSION, "bioformats2raw.layout": 3}})
}

/// `attributes` of the root of an HCS plate: every row and column of the plate, and the imaged
/// wells as `(path, rowIndex, columnIndex)`.
pub fn plate_attributes(
    name: Option<&str>,
    rows: &[String],
    columns: &[String],
    wells: &[(String, u32, u32)],
    field_count: usize,
) -> Value {
    let mut plate = serde_json::Map::new();
    if let Some(n) = name {
        plate.insert("name".into(), json!(n));
    }
    plate.insert(
        "rows".into(),
        Value::Array(rows.iter().map(|r| json!({"name": r})).collect()),
    );
    plate.insert(
        "columns".into(),
        Value::Array(columns.iter().map(|c| json!({"name": c})).collect()),
    );
    plate.insert(
        "wells".into(),
        Value::Array(
            wells
                .iter()
                .map(|(p, r, c)| json!({"path": p, "rowIndex": r, "columnIndex": c}))
                .collect(),
        ),
    );
    plate.insert("field_count".into(), json!(field_count));
    plate.insert("version".into(), json!(NGFF_VERSION));
    json!({"ome": {"version": NGFF_VERSION, "plate": Value::Object(plate)}})
}

/// `attributes` of a well group: its field image groups in order.
pub fn well_attributes(fields: &[String]) -> Value {
    json!({"ome": {"version": NGFF_VERSION, "well": {"images": fields.iter().map(|f| json!({"path": f})).collect::<Vec<_>>()}}})
}

/// `attributes` of the `OME` group of a collection, listing the image groups in order.
pub fn series_attributes(paths: &[String]) -> Value {
    json!({"ome": {"version": NGFF_VERSION, "series": paths}})
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::model::{ChannelInfo, PhysicalSize};

    fn image() -> ImageInfo {
        let mut im = ImageInfo::new(0, 2048, 1500, PixelType::Uint16);
        im.size_c = 2;
        im.size_z = 5;
        im.physical_size = PhysicalSize::micrometres(Some(0.25), Some(0.25), Some(1.0));
        im.time_increment_s = Some(2.0);
        im.name = Some("scene A".into());
        im.channels = vec![
            ChannelInfo {
                index: 0,
                name: Some("DAPI".into()),
                color: Some("#0000ff".into()),
                ..Default::default()
            },
            ChannelInfo {
                index: 1,
                name: Some("GFP".into()),
                ..Default::default()
            },
        ];
        im.finish()
    }

    #[test]
    fn auto_levels_stop_at_threshold() {
        assert_eq!(plan_levels(512, 512, None), vec![(512, 512)]);
        assert_eq!(plan_levels(1024, 1024, None), vec![(1024, 1024)]);
        assert_eq!(
            plan_levels(4097, 300, None),
            vec![(4097, 300), (2049, 150), (1025, 75), (513, 38)]
        );
        assert_eq!(plan_levels(2048, 1500, None).len(), 2);
    }

    #[test]
    fn explicit_levels() {
        assert_eq!(plan_levels(100, 60, Some(1)), vec![(100, 60)]);
        assert_eq!(
            plan_levels(100, 60, Some(3)),
            vec![(100, 60), (50, 30), (25, 15)]
        );
        assert_eq!(plan_levels(2, 1, Some(9)), vec![(2, 1), (1, 1)]);
        assert_eq!(plan_levels(10, 10, Some(0)), vec![(10, 10)]);
    }

    #[test]
    fn multiscales_axes_units_and_scales() {
        let im = image();
        let levels = plan_levels(im.size_x, im.size_y, None);
        let m = multiscales(&im, &levels, "test");
        let names: Vec<&str> = m["axes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, AXES);
        assert_eq!(m["axes"][0]["unit"], "second");
        assert_eq!(m["axes"][1]["type"], "channel");
        assert!(m["axes"][1].get("unit").is_none());
        assert_eq!(m["axes"][4]["unit"], "micrometer");
        assert_eq!(m["datasets"].as_array().unwrap().len(), 2);
        assert_eq!(m["datasets"][0]["path"], "0");
        assert_eq!(
            m["datasets"][0]["coordinateTransformations"][0]["scale"],
            json!([2.0, 1.0, 1.0, 0.25, 0.25])
        );
        assert_eq!(
            m["datasets"][1]["coordinateTransformations"][0]["scale"],
            json!([2.0, 1.0, 1.0, 0.5, 0.5])
        );
        assert_eq!(m["name"], "scene A");
    }

    #[test]
    fn unknown_sizes_use_relative_factors_without_units() {
        let im = ImageInfo::new(3, 3000, 3000, PixelType::Uint8).finish();
        let levels = plan_levels(3000, 3000, None);
        assert_eq!(levels.len(), 3);
        let m = multiscales(&im, &levels, "test");
        assert!(m["axes"][4].get("unit").is_none());
        assert!(m["axes"][0].get("unit").is_none());
        assert_eq!(
            m["datasets"][2]["coordinateTransformations"][0]["scale"],
            json!([1.0, 1.0, 1.0, 4.0, 4.0])
        );
        assert_eq!(m["name"], "Image 3");
    }

    #[test]
    fn omero_channels_colors_and_windows() {
        let im = image();
        let ranges = [
            ChannelRange {
                min: 10.0,
                max: 4000.0,
            },
            ChannelRange::EMPTY,
        ];
        let o = omero(&im, &[0, 1], &ranges, 2);
        let ch = o["channels"].as_array().unwrap();
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[0]["label"], "DAPI");
        assert_eq!(ch[0]["color"], "0000FF");
        assert_eq!(
            ch[0]["window"],
            json!({"start": 10, "end": 4000, "min": 0, "max": 65535})
        );
        assert_eq!(ch[1]["color"], DEFAULT_COLORS[1]);
        assert_eq!(ch[1]["window"]["end"], 0);
        assert_eq!(o["rdefs"]["defaultZ"], 2);
        assert_eq!(o["rdefs"]["model"], "color");
        // a channel subset keeps the source labels
        let o = omero(&im, &[1], &ranges[..1], 0);
        assert_eq!(o["channels"][0]["label"], "GFP");
        assert_eq!(o["channels"][0]["color"], "FFFFFF");
        assert_eq!(o["rdefs"]["model"], "greyscale");
    }

    #[test]
    fn omero_expands_rgb_samples() {
        let mut im = ImageInfo::new(0, 64, 64, PixelType::Uint8);
        im.samples_per_pixel = 3;
        let o = omero(&im, &[0], &[], 0);
        let ch = o["channels"].as_array().unwrap();
        assert_eq!(ch.len(), 3);
        assert_eq!(ch[0]["label"], "Channel 0 (R)");
        assert_eq!(ch[1]["color"], "00FF00");
        assert_eq!(ch[2]["color"], "0000FF");
        assert_eq!(ch[2]["window"]["max"], 255);
    }

    #[test]
    fn float_window_uses_data_range() {
        let im = ImageInfo::new(0, 8, 8, PixelType::Float);
        let o = omero(
            &im,
            &[0],
            &[ChannelRange {
                min: -0.5,
                max: 2.25,
            }],
            0,
        );
        assert_eq!(
            o["channels"][0]["window"],
            json!({"start": -0.5, "end": 2.25, "min": -0.5, "max": 2.25})
        );
    }

    #[test]
    fn collection_attributes() {
        assert_eq!(
            collection_root_attributes(),
            json!({"ome": {"version": "0.5", "bioformats2raw.layout": 3}})
        );
        assert_eq!(
            series_attributes(&["0".into(), "1".into()]),
            json!({"ome": {"version": "0.5", "series": ["0", "1"]}})
        );
        let a = image_attributes(json!({}), json!({}));
        assert_eq!(a["ome"]["version"], "0.5");
        assert!(a["ome"]["multiscales"].is_array());
    }
}
