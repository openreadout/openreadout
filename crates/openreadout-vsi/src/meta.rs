//! Normalized values from the `.vsi` record tree. Tag numbers and their meaning were found by
//! differential analysis (`docs/provenance/vsi.md`); names here are ours.

use crate::tree::{TagRecord, TagTree, find, path};

/// Per-file record set.
pub const TAG_FILE: u32 = 2000;
/// A stack (image); its index is the stack id (`stack<id>` directory).
pub const TAG_STACK: u32 = 2001;
/// Stack: sizes of the non-XY dimensions.
pub const TAG_DIM_SIZES: u32 = 2003;
/// Stack: background sample bytes.
pub const TAG_BACKGROUND: u32 = 2034;
/// Stack: settings record set.
pub const TAG_SETTINGS: u32 = 2005;
/// Settings: stack name.
pub const TAG_NAME: u32 = 2030;
/// Settings: pixel size x, y.
pub const TAG_PIXEL_SIZE: u32 = 2019;
/// Settings: unit of the pixel size.
pub const TAG_PIXEL_UNIT: u32 = 2020;
/// Settings: acquisition time, Unix seconds.
pub const TAG_TIME: u32 = 2015;
/// Settings: stage position x, y.
pub const TAG_STAGE: u32 = 2018;
/// Settings: device record sets.
pub const TAG_DEVICES: u32 = 2043;
/// Stack: one dimension description per non-XY dimension (index = dimension number).
pub const TAG_DIMENSION: u32 = 2007;
/// Dimension: one entry per element (index = element number).
pub const TAG_DIM_ENTRY: u32 = 2008;
/// Dimension entry: kind of the dimension (1 = Z, 2 = T, 4 = channel).
pub const TAG_DIM_KIND: u32 = 2023;
/// Dimension entry: Z start and step (value containers).
pub const TAG_Z_START: u32 = 2012;
pub const TAG_Z_STEP: u32 = 2013;
/// Channel entry: name, emission and second wavelength (value containers, nm).
pub const TAG_CHANNEL_NAME: u32 = 2021;
pub const TAG_EMISSION: u32 = 2417;
pub const TAG_EXCITATION: u32 = 2474;
/// Settings (and channel entries): name of the illumination setting (`BF`, `Label Scan`).
pub const TAG_SETTING_NAME: u32 = 2419;
/// Stack: per-plane record (index = plane number, first dimension fastest).
pub const TAG_PLANE: u32 = 2002;
/// Plane record: layout set (only in the first plane record of a stack).
pub const TAG_PLANE_LAYOUT: u32 = 2018;
/// Plane record: second layout set (rectangle and box, no tile origin).
pub const TAG_PLANE_LAYOUT_ALT: u32 = 2037;
/// Layout: image rectangle `[x, y, width, height]` in pixels.
pub const TAG_IMAGE_RECT: u32 = 2053;
/// Layout: where tile column 0 / row 0 starts in the image, in pixels (i32 per dimension).
pub const TAG_TILE_ORIGIN: u32 = 2410;
/// Plane record: per-plane values set (Z position, time).
pub const TAG_PLANE_VALUES: u32 = 2006;
/// Per-plane values: Z position (value container, µm).
pub const TAG_PLANE_Z: u32 = 2014;
/// Per-plane values: time (value container, ms).
pub const TAG_PLANE_TIME: u32 = 2017;
/// File info record set (path 2000/2004/2109): software name and version.
pub const TAG_FILE_INFO: [u32; 3] = [2000, 2004, 2109];
pub const TAG_SOFTWARE: u32 = 34;
pub const TAG_SOFTWARE_VERSION: u32 = 35;
/// Objective device values (inside a device record set, under 120114).
pub const TAG_OBJECTIVE_SET: u32 = 120_114;
pub const TAG_OBJ_MAGNIFICATION: u32 = 120_060;
pub const TAG_OBJ_NA: u32 = 120_061;
pub const TAG_OBJ_WORKING_DISTANCE: u32 = 120_062;
pub const TAG_OBJ_NAME: u32 = 120_063;
/// Objective device: refractive index of the immersion medium.
pub const TAG_OBJ_REFRACTIVE_INDEX: u32 = 120_079;
/// Device record: device type code (`DEVICE_CAMERA`, `DEVICE_FRAME`, ...).
pub const TAG_DEVICE_TYPE: u32 = 120_130;
/// Device record: device name, model and manufacturer (UTF-16 text).
pub const TAG_DEVICE_NAME: u32 = 120_116;
pub const TAG_DEVICE_MODEL: u32 = 120_132;
pub const TAG_DEVICE_MAKER: u32 = 120_133;
/// Camera device values: exposure in µs.
pub const TAG_CAM_EXPOSURE_US: u32 = 100_002;
/// Device type codes: the camera and the microscope frame.
pub const DEVICE_CAMERA: i64 = 0;
pub const DEVICE_FRAME: i64 = 40_500;

/// Kind of a non-XY dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DimKind {
    Z,
    T,
    Channel,
    /// A code we have not seen with a known meaning.
    Other(i64),
    /// No kind recorded.
    Unknown,
}

impl DimKind {
    pub fn from_code(code: Option<i64>) -> Self {
        match code {
            Some(1) => DimKind::Z,
            Some(2) => DimKind::T,
            Some(4) => DimKind::Channel,
            Some(c) => DimKind::Other(c),
            None => DimKind::Unknown,
        }
    }
    pub fn name(self) -> String {
        match self {
            DimKind::Z => "z".into(),
            DimKind::T => "t".into(),
            DimKind::Channel => "c".into(),
            DimKind::Other(c) => format!("kind {c}"),
            DimKind::Unknown => "unknown".into(),
        }
    }
}

/// A channel of a channel dimension.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelMeta {
    pub name: Option<String>,
    pub emission_nm: Option<f64>,
    pub excitation_nm: Option<f64>,
    /// Camera exposure of this channel (the channel's own camera device record), ms.
    pub exposure_ms: Option<f64>,
}

/// One non-XY dimension.
#[derive(Debug, Clone, PartialEq)]
pub struct DimMeta {
    pub kind: DimKind,
    pub z_start_um: Option<f64>,
    pub z_step_um: Option<f64>,
    pub channels: Vec<ChannelMeta>,
}

/// The objective found in a stack's devices.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectiveMeta {
    pub name: Option<String>,
    pub magnification: Option<f64>,
    pub numerical_aperture: Option<f64>,
    pub working_distance_um: Option<f64>,
    /// Refractive index of the immersion medium the objective is set for.
    pub refractive_index: Option<f64>,
}

/// One stack as described in the `.vsi`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StackMeta {
    pub id: u32,
    pub name: Option<String>,
    pub dim_sizes: Vec<i32>,
    pub dims: Vec<DimMeta>,
    /// Pixel size in µm.
    pub pixel_size_um: Option<(f64, f64)>,
    pub stage_position_um: Option<(f64, f64)>,
    pub acquired_unix: Option<i64>,
    pub background: Option<Vec<u8>>,
    pub objective: Option<ObjectiveMeta>,
    /// Per-plane times in ms, plane number = first dimension fastest.
    pub plane_times_ms: Vec<Option<f64>>,
    /// Per-plane Z positions in µm (same order).
    pub plane_z_um: Vec<Option<f64>>,
    /// The camera device (type 0) of the stack settings: name, and its exposure in ms.
    pub camera: Option<String>,
    pub exposure_ms: Option<f64>,
    /// The microscope frame device (type 40500): model, else name.
    pub microscope: Option<String>,
    /// Name of the stack's illumination setting (the channel name of stacks without a
    /// channel dimension, e.g. `BF` for brightfield slides).
    pub setting_name: Option<String>,
    /// Image rectangle `[x, y, width, height]` from the first plane record's layout.
    pub image_rect: Option<[i64; 4]>,
    /// Pixel position of tile column 0, row 0 in the image (`x`, `y`); non-zero in stitched
    /// images whose tile grid does not start at the image corner.
    pub tile_origin: Option<(i64, i64)>,
}

/// Everything we normalize from a `.vsi`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VsiMeta {
    pub software: Option<String>,
    pub software_version: Option<String>,
    pub stacks: Vec<StackMeta>,
}

/// Factor from a unit string `10^<e><base>^<p>` to `10^target_exp` of `base`
/// (e.g. `10^-6m^1` → µm is 1, `10^-9m^1` → µm is 1e-3, `10^-3s^1` → s is 1e-3).
pub fn unit_factor(unit: &str, base: &str, target_exp: i32) -> Option<f64> {
    let rest = unit.strip_prefix("10^")?;
    let pos = rest.find(base)?;
    let exp: i32 = rest[..pos].parse().ok()?;
    let power = rest[pos + base.len()..].strip_prefix('^')?;
    if power != "1" {
        return None;
    }
    Some(10f64.powi(exp - target_exp))
}

fn quantity_in(tree: &TagTree, r: Option<&TagRecord>, base: &str, target_exp: i32) -> Option<f64> {
    let (v, unit) = tree.quantity(r?)?;
    let f = match unit {
        Some(u) => unit_factor(&u, base, target_exp)?,
        None => 1.0,
    };
    Some(v * f)
}

fn objective(tree: &TagTree, settings: &TagRecord) -> Option<ObjectiveMeta> {
    let devices = find(settings.children(), TAG_DEVICES, None)?;
    for dev in devices.children() {
        let Some(set) = find(dev.children(), TAG_OBJECTIVE_SET, None) else {
            continue;
        };
        let kids = set.children();
        let mag = find(kids, TAG_OBJ_MAGNIFICATION, None).and_then(|r| tree.number(r));
        let na = find(kids, TAG_OBJ_NA, None).and_then(|r| tree.number(r));
        if mag.is_none() || na.is_none() {
            continue;
        }
        return Some(ObjectiveMeta {
            name: find(kids, TAG_OBJ_NAME, None).and_then(|r| tree.text(r)),
            magnification: mag,
            numerical_aperture: na,
            working_distance_um: quantity_in(
                tree,
                find(kids, TAG_OBJ_WORKING_DISTANCE, None),
                "m",
                -6,
            ),
            refractive_index: find(kids, TAG_OBJ_REFRACTIVE_INDEX, None)
                .and_then(|r| tree.number(r))
                .filter(|v| (1.0..=2.0).contains(v)),
        });
    }
    None
}

/// The device records under `holder`'s device set (2043) whose type code is `code`.
fn devices<'a>(holder: &'a [TagRecord], tree: &TagTree, code: i64) -> Vec<&'a TagRecord> {
    find(holder, TAG_DEVICES, None)
        .map(|d| {
            d.children()
                .iter()
                .filter(|dev| {
                    find(dev.children(), TAG_DEVICE_TYPE, None)
                        .and_then(|t| tree.number(t))
                        .is_some_and(|v| v as i64 == code)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Exposure (ms) of the camera device under `holder`'s device set.
fn camera_exposure_ms(holder: &[TagRecord], tree: &TagTree) -> Option<f64> {
    devices(holder, tree, DEVICE_CAMERA)
        .into_iter()
        .find_map(|dev| {
            let vals = find(dev.children(), TAG_OBJECTIVE_SET, None)?;
            let us =
                find(vals.children(), TAG_CAM_EXPOSURE_US, None).and_then(|r| tree.number(r))?;
            (us > 0.0).then_some(us / 1000.0)
        })
}

/// Name (or model) of the first device of type `code` under `holder`'s device set.
fn device_name(holder: &[TagRecord], tree: &TagTree, code: i64, tags: &[u32]) -> Option<String> {
    devices(holder, tree, code).into_iter().find_map(|dev| {
        tags.iter().find_map(|t| {
            find(dev.children(), *t, None)
                .and_then(|r| tree.text(r))
                .filter(|s| !s.trim().is_empty())
        })
    })
}

#[allow(clippy::many_single_char_names)]
fn stack(tree: &TagTree, r: &TagRecord) -> StackMeta {
    let kids = r.children();
    let settings = find(kids, TAG_SETTINGS, None);
    let s = settings.map_or(&[][..], TagRecord::children);
    let unit = find(s, TAG_PIXEL_UNIT, None).and_then(|u| tree.text(u));
    let um = unit
        .as_deref()
        .map_or(Some(1.0), |u| unit_factor(u, "m", -6));
    let pair = |tag: u32| {
        let v = tree.floats(find(s, tag, None)?)?;
        let f = um?;
        (v.len() >= 2 && v[0].is_finite() && v[1].is_finite()).then(|| (v[0] * f, v[1] * f))
    };
    let mut dims = Vec::new();
    let mut dim_records: Vec<&TagRecord> = kids.iter().filter(|c| c.tag == TAG_DIMENSION).collect();
    dim_records.sort_by_key(|c| c.index.unwrap_or(u32::MAX));
    for d in dim_records {
        let mut entries: Vec<&TagRecord> = d
            .children()
            .iter()
            .filter(|c| c.tag == TAG_DIM_ENTRY)
            .collect();
        entries.sort_by_key(|c| c.index.unwrap_or(u32::MAX));
        let first = entries.first().map_or(&[][..], |e| e.children());
        let kind = DimKind::from_code(
            find(first, TAG_DIM_KIND, None)
                .and_then(|k| tree.number(k))
                .map(|v| v as i64),
        );
        let channels = if kind == DimKind::Channel {
            entries
                .iter()
                .map(|e| {
                    let k = e.children();
                    ChannelMeta {
                        name: find(k, TAG_CHANNEL_NAME, None).and_then(|n| tree.text(n)),
                        emission_nm: quantity_in(tree, find(k, TAG_EMISSION, None), "m", -9),
                        excitation_nm: quantity_in(tree, find(k, TAG_EXCITATION, None), "m", -9),
                        exposure_ms: camera_exposure_ms(k, tree),
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        dims.push(DimMeta {
            kind,
            z_start_um: quantity_in(tree, find(first, TAG_Z_START, None), "m", -6),
            z_step_um: quantity_in(tree, find(first, TAG_Z_STEP, None), "m", -6),
            channels,
        });
    }
    let mut planes: Vec<(u32, Option<f64>, Option<f64>)> = kids
        .iter()
        .filter(|c| c.tag == TAG_PLANE)
        .map(|p| {
            let value = |tag: u32, base: &str, exp: i32| {
                p.children()
                    .iter()
                    .filter(|c| c.tag == TAG_PLANE_VALUES)
                    .flat_map(TagRecord::children)
                    .find(|c| c.tag == tag)
                    .and_then(|q| {
                        let (v, u) = tree.quantity(q)?;
                        let f = unit_factor(u.as_deref()?, base, exp)?;
                        Some(v * f)
                    })
            };
            (
                p.index.unwrap_or(0),
                value(TAG_PLANE_TIME, "s", -3),
                value(TAG_PLANE_Z, "m", -6),
            )
        })
        .collect();
    planes.sort_by_key(|p| p.0);
    // The layout sets sit in the first plane record (lowest index): 2018 (with the tile
    // origin) and/or 2037 (rectangle and box only; the only one in some cellSens 3.2 files).
    let first_plane = kids
        .iter()
        .filter(|c| c.tag == TAG_PLANE)
        .min_by_key(|c| c.index.unwrap_or(u32::MAX))
        .map_or(&[][..], TagRecord::children);
    let layout = find(first_plane, TAG_PLANE_LAYOUT, None).map_or(&[][..], TagRecord::children);
    let layout_alt =
        find(first_plane, TAG_PLANE_LAYOUT_ALT, None).map_or(&[][..], TagRecord::children);
    let image_rect = [layout, layout_alt]
        .iter()
        .filter_map(|l| find(l, TAG_IMAGE_RECT, None).and_then(|r| tree.ints(r)))
        .find(|v| v.len() == 4 && v[2] > 0 && v[3] > 0)
        .map(|v| [v[0], v[1], v[2], v[3]].map(i64::from));
    let tile_origin = find(layout, TAG_TILE_ORIGIN, None)
        .and_then(|r| tree.ints(r))
        .filter(|v| v.len() >= 2)
        .map(|v| (i64::from(v[0]), i64::from(v[1])));
    StackMeta {
        id: r.index.unwrap_or(0),
        name: find(s, TAG_NAME, None).and_then(|n| tree.text(n)),
        dim_sizes: find(kids, TAG_DIM_SIZES, None)
            .and_then(|d| tree.ints(d))
            .unwrap_or_default(),
        dims,
        pixel_size_um: pair(TAG_PIXEL_SIZE),
        stage_position_um: pair(TAG_STAGE),
        acquired_unix: find(s, TAG_TIME, None).and_then(|t| tree.int64(t)),
        background: find(kids, TAG_BACKGROUND, None).map(|b| tree.bytes(b).to_vec()),
        objective: settings.and_then(|st| objective(tree, st)),
        plane_times_ms: planes.iter().map(|p| p.1).collect(),
        plane_z_um: planes.iter().map(|p| p.2).collect(),
        camera: device_name(s, tree, DEVICE_CAMERA, &[TAG_DEVICE_NAME, TAG_DEVICE_MODEL]),
        exposure_ms: camera_exposure_ms(s, tree),
        microscope: device_name(s, tree, DEVICE_FRAME, &[TAG_DEVICE_MODEL, TAG_DEVICE_NAME]),
        setting_name: find(s, TAG_SETTING_NAME, None)
            .and_then(|n| tree.text(n))
            .filter(|n| !n.trim().is_empty()),
        image_rect,
        tile_origin,
    }
}

/// Collect the stacks and file info from a parsed tree.
pub fn read_meta(tree: &TagTree) -> VsiMeta {
    let file = find(&tree.root, TAG_FILE, None);
    let stacks = file
        .map(|f| {
            f.children()
                .iter()
                .filter(|c| c.tag == TAG_STACK && c.index.is_some())
                .map(|c| stack(tree, c))
                .collect()
        })
        .unwrap_or_default();
    let info = path(&tree.root, &TAG_FILE_INFO);
    let text = |tag: u32| {
        info.and_then(|i| find(i.children(), tag, None))
            .and_then(|r| tree.text(r))
    };
    VsiMeta {
        software: text(TAG_SOFTWARE),
        software_version: text(TAG_SOFTWARE_VERSION),
        stacks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(unit_factor("10^-6m^1", "m", -6), Some(1.0));
        assert!((unit_factor("10^-9m^1", "m", -6).unwrap() - 1e-3).abs() < 1e-15);
        assert!((unit_factor("10^-3s^1", "s", 0).unwrap() - 1e-3).abs() < 1e-15);
        assert_eq!(unit_factor("10^0s^-1", "s", 0), None);
        assert_eq!(unit_factor("furlongs", "m", 0), None);
    }

    #[test]
    fn kinds() {
        assert_eq!(DimKind::from_code(Some(1)), DimKind::Z);
        assert_eq!(DimKind::from_code(Some(4)), DimKind::Channel);
        assert_eq!(DimKind::from_code(Some(9)), DimKind::Other(9));
        assert_eq!(DimKind::from_code(None).name(), "unknown");
    }
}
