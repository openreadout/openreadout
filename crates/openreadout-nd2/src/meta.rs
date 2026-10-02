//! Normalized views of the LV / variant metadata: attributes, the experiment loop tree, frame
//! layout, picture planes (channels), events and ROIs. Names: `docs/formats/nd2.md`.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

/// `SLxImageAttributes`.
#[derive(Debug, Clone, Default)]
pub struct Attributes {
    pub width: u32,
    pub height: u32,
    /// Row stride in bytes (may include padding).
    pub row_bytes: u32,
    /// Interleaved samples per pixel across all channels.
    pub components: u32,
    pub bits_in_memory: u32,
    pub bits_significant: u32,
    pub frame_count: u32,
    pub compression: Option<i64>,
    /// `ePixelType`: 1 = unsigned integer, 2 = float (inferred).
    pub pixel_kind: Option<i64>,
    pub virtual_components: Option<u32>,
}

/// LV trees wrap their content in a single `SLx…` key; variant XML wraps it in `no_name`.
pub fn unwrap_root(v: &Value) -> &Value {
    match v.as_object() {
        Some(o) if o.len() == 1 => match o.values().next() {
            Some(inner) if inner.is_object() => inner,
            _ => v,
        },
        _ => v,
    }
}

impl Attributes {
    pub fn from_lv(v: &Value) -> Option<Self> {
        let a = unwrap_root(v);
        let g = |k: &str| a.get(k).and_then(Value::as_u64).map(|x| x as u32);
        Some(Attributes {
            width: g("uiWidth")?,
            height: g("uiHeight")?,
            row_bytes: g("uiWidthBytes").unwrap_or(0),
            components: g("uiComp").unwrap_or(1),
            bits_in_memory: g("uiBpcInMemory").unwrap_or(16),
            bits_significant: g("uiBpcSignificant").unwrap_or(16),
            frame_count: g("uiSequenceCount").unwrap_or(1),
            compression: a.get("eCompression").and_then(Value::as_i64),
            pixel_kind: a.get("ePixelType").and_then(Value::as_i64),
            virtual_components: g("uiVirtualComponents"),
        })
    }
}

/// The elements of a list-like value: an LV level whose items share the empty name (`{"": [...]}`),
/// a variant list (`{"_00": …, "_01": …}` or `{"i0000000000": …}`, in stored order), or an array.
pub(crate) fn list_items(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => match o.get("") {
            Some(Value::Array(a)) => a.iter().collect(),
            Some(single) => vec![single],
            // Repeated element names (`<no_name/><no_name/>`) were merged into an array.
            None => o
                .values()
                .flat_map(|v| match v {
                    Value::Array(a) => a.iter().collect::<Vec<_>>(),
                    other => vec![other],
                })
                .collect(),
        },
        _ => Vec::new(),
    }
}

/// A validity mask (`pItemValid`, `pPeriodValid`): LV byte arrays, variant lists of booleans,
/// or base64 strings. Empty when absent.
pub(crate) fn bool_list(v: Option<&Value>) -> Vec<bool> {
    let one = |b: &Value| b.as_bool().unwrap_or_else(|| b.as_u64().unwrap_or(1) != 0);
    match v {
        Some(Value::Array(a)) => a.iter().map(one).collect(),
        Some(Value::String(s)) => base64_decode(s).into_iter().map(|b| b != 0).collect(),
        Some(o @ Value::Object(_)) => list_items(o).into_iter().map(one).collect(),
        _ => Vec::new(),
    }
}

fn f64_at(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

fn str_at(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Loop kinds from `eType` (numbering from the `nd2` package, BSD-3; see provenance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopKind {
    TimeLoop,
    XyPositionLoop,
    ZStackLoop,
    /// `eType` 6: the λ (picture-plane) loop. Channels are interleaved inside each frame, so it
    /// never indexes frames.
    SpectralLoop,
    CustomLoop,
    NeTimeLoop,
    Other(i64),
}

impl LoopKind {
    pub fn from_raw(v: i64) -> Self {
        match v {
            1 => LoopKind::TimeLoop,
            2 => LoopKind::XyPositionLoop,
            4 => LoopKind::ZStackLoop,
            6 => LoopKind::SpectralLoop,
            7 => LoopKind::CustomLoop,
            8 => LoopKind::NeTimeLoop,
            o => LoopKind::Other(o),
        }
    }
    pub fn name(self) -> String {
        match self {
            LoopKind::TimeLoop => "time_loop".into(),
            LoopKind::XyPositionLoop => "xy_position_loop".into(),
            LoopKind::ZStackLoop => "z_stack_loop".into(),
            LoopKind::SpectralLoop => "spectral_loop".into(),
            LoopKind::CustomLoop => "custom_loop".into(),
            LoopKind::NeTimeLoop => "ne_time_loop".into(),
            LoopKind::Other(v) => format!("loop_type_{v}"),
        }
    }
    /// Which image axis the loop indexes: `P` positions (images), `Z`, or `T` (time, and
    /// custom/unknown loops folded into T).
    pub fn axis(self) -> char {
        match self {
            LoopKind::XyPositionLoop => 'P',
            LoopKind::ZStackLoop => 'Z',
            _ => 'T',
        }
    }
}

/// A stage position from an XY position loop.
#[derive(Debug, Clone, Default)]
pub struct Position {
    pub pos_x: f64,
    pub pos_y: f64,
    pub pos_z: f64,
    pub pos_name: Option<String>,
    pub pfs_offset: Option<f64>,
}

/// One valid period of an NE (non-equidistant) time loop.
#[derive(Debug, Clone, Default)]
pub struct Period {
    pub count: u32,
    pub period_ms: Option<f64>,
    pub start_ms: Option<f64>,
    pub duration_ms: Option<f64>,
}

/// One counted node of the experiment loop tree, flattened outermost-first.
#[derive(Debug, Clone)]
pub struct Loop {
    pub kind: LoopKind,
    /// Frames this loop contributes (after validity masks).
    pub count: u32,
    /// `uiCount` as written (counts masked-out items and invalid periods too).
    pub declared_count: Option<u32>,
    /// Nesting depth (spectral loops do not add a level).
    pub level: u32,
    pub z_step_um: Option<f64>,
    pub period_ms: Option<f64>,
    pub periods: Vec<Period>,
    pub positions: Vec<Position>,
    pub repeat_count: Option<u32>,
    /// Items (positions, periods) switched off by the validity mask.
    pub invalid_items: u32,
}

/// Walk `SLxExperiment` (or a legacy `RLxExperiment` tree) and return the counted loops
/// outermost-first. Rules (mirroring the `nd2` package, see provenance):
/// a node without loop parameters, or whose count is 0, ends its branch; a spectral loop does
/// not add a nesting level; at a level already holding a loop, a sibling of the same kind
/// replaces it only if it counts more frames, and a sibling of another kind is ignored.
pub fn loops_from_lv(v: &Value) -> Vec<Loop> {
    let mut out = Vec::new();
    let mut exp = unwrap_root(v);
    // Legacy `AIM1`: `{vMetadata: {…}, vUnknownData: {…}}`.
    if exp.get("eType").is_none()
        && let Some(inner) = exp.get("vMetadata").filter(|m| m.get("eType").is_some())
    {
        exp = inner;
    }
    if exp.get("eType").is_some() || exp.get("ppNextLevelEx").is_some() {
        walk(exp, 0, &mut out);
    } else if let Some(o) = exp.as_object() {
        // Old legacy layout: sibling `LoopNo00`, `LoopNo01`, … outermost first.
        let mut keys: Vec<&String> = o.keys().filter(|k| k.starts_with("LoopNo")).collect();
        keys.sort();
        for (i, k) in keys.into_iter().enumerate() {
            walk(&o[k], i as u32, &mut out);
        }
    }
    out
}

fn unwrap_pars(p: &Value) -> &Value {
    match p.as_object() {
        Some(o) if o.len() == 1 => match o.iter().next() {
            Some((k, inner)) if inner.is_object() && (k == "i0000000000" || k == "no_name") => {
                inner
            }
            _ => p,
        },
        _ => p,
    }
}

fn walk(node: &Value, level: u32, out: &mut Vec<Loop>) {
    let Some(raw_pars) = node.get("uLoopPars") else {
        return;
    };
    if raw_pars.as_object().is_none_or(Map::is_empty) {
        return;
    }
    let pars = unwrap_pars(raw_pars);
    let kind = LoopKind::from_raw(node.get("eType").and_then(Value::as_i64).unwrap_or(0));
    if kind == LoopKind::Other(0) {
        return;
    }
    let declared = pars
        .get("uiCount")
        .and_then(Value::as_u64)
        .map(|c| c as u32);
    let mut lp = Loop {
        kind,
        count: declared.unwrap_or(0),
        declared_count: declared,
        level,
        z_step_um: None,
        period_ms: None,
        periods: Vec::new(),
        positions: Vec::new(),
        repeat_count: node
            .get("uiRepeatCount")
            .and_then(Value::as_u64)
            .map(|c| c as u32),
        invalid_items: 0,
    };
    match kind {
        LoopKind::TimeLoop => lp.period_ms = f64_at(pars, "dPeriod"),
        LoopKind::XyPositionLoop => {
            let (positions, invalid) = positions_from(pars, node.get("pItemValid"));
            lp.invalid_items = invalid;
            if positions.is_empty() {
                lp.count = lp.count.saturating_sub(invalid);
            } else {
                lp.count = positions.len() as u32;
            }
            lp.positions = positions;
        }
        LoopKind::ZStackLoop => {
            let step = f64_at(pars, "dZStep").unwrap_or(0.0);
            let (lo, hi) = (
                f64_at(pars, "dZLow").unwrap_or(0.0),
                f64_at(pars, "dZHigh").unwrap_or(0.0),
            );
            lp.z_step_um = Some(if step == 0.0 && lp.count > 1 {
                (hi - lo).abs() / f64::from(lp.count - 1)
            } else {
                step.abs()
            });
        }
        LoopKind::NeTimeLoop => {
            let (periods, invalid) = periods_from(pars);
            lp.invalid_items = invalid;
            lp.count = periods.iter().map(|p| p.count).sum();
            lp.period_ms = periods.first().and_then(|p| p.period_ms);
            lp.periods = periods;
        }
        LoopKind::SpectralLoop => {
            if let Some(c) = pars
                .get("pPlanes")
                .and_then(|p| p.get("uiCount"))
                .and_then(Value::as_u64)
            {
                lp.count = c as u32;
            }
        }
        LoopKind::CustomLoop | LoopKind::Other(_) => {}
    }
    if lp.count == 0 {
        return;
    }
    let child_level = if kind == LoopKind::SpectralLoop {
        level
    } else {
        match out.last_mut() {
            None => out.push(lp),
            Some(last) if last.level < level => out.push(lp),
            Some(last) if last.level == level && last.kind == kind => {
                if last.count < lp.count {
                    *last = lp;
                }
            }
            Some(_) => {}
        }
        level + 1
    };
    if let Some(next) = node.get("ppNextLevelEx") {
        for c in list_items(next) {
            walk(c, child_level, out);
        }
    }
}

fn positions_from(pars: &Value, valid: Option<&Value>) -> (Vec<Position>, u32) {
    let valid = bool_list(valid);
    let rel = pars
        .get("bRelativeXY")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let (rx, ry) = if rel {
        (
            f64_at(pars, "dReferenceX").unwrap_or(0.0),
            f64_at(pars, "dReferenceY").unwrap_or(0.0),
        )
    } else {
        (0.0, 0.0)
    };
    let mut all: Vec<Position> = if let Some(points) = pars.get("Points") {
        list_items(points)
            .into_iter()
            .map(|p| Position {
                pos_x: rx + f64_at(p, "dPosX").unwrap_or(0.0),
                pos_y: ry + f64_at(p, "dPosY").unwrap_or(0.0),
                pos_z: f64_at(p, "dPosZ").unwrap_or(0.0),
                pos_name: str_at(p, "dPosName").or_else(|| str_at(p, "pPosName")),
                pfs_offset: f64_at(p, "dPFSOffset").filter(|o| *o >= 0.0),
            })
            .collect()
    } else if let Some(xs) = pars.get("dPosX") {
        // Variant / legacy layout: parallel lists `dPosX`, `dPosY`, `dPosZ`, `pPosName`.
        let col = |k: &str| pars.get(k).map(list_items).unwrap_or_default();
        let (ys, zs, names, pfs) = (
            col("dPosY"),
            col("dPosZ"),
            col("pPosName"),
            col("dPFSOffset"),
        );
        list_items(xs)
            .into_iter()
            .enumerate()
            .map(|(i, x)| Position {
                pos_x: rx + x.as_f64().unwrap_or(0.0),
                pos_y: ry + ys.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0),
                pos_z: zs.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0),
                pos_name: names
                    .get(i)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                pfs_offset: pfs.get(i).and_then(|v| v.as_f64()).filter(|o| *o >= 0.0),
            })
            .collect()
    } else {
        Vec::new()
    };
    if !valid.is_empty() {
        all = all
            .into_iter()
            .enumerate()
            .filter(|(i, _)| valid.get(*i).copied().unwrap_or(true))
            .map(|(_, p)| p)
            .collect();
    }
    (all, valid.iter().filter(|v| !**v).count() as u32)
}

fn periods_from(pars: &Value) -> (Vec<Period>, u32) {
    let periods = pars.get("pPeriod").map(list_items).unwrap_or_default();
    let valid = bool_list(pars.get("pPeriodValid"));
    let mut out = Vec::new();
    let mut invalid = 0u32;
    for (i, p) in periods.into_iter().enumerate() {
        if !valid.get(i).copied().unwrap_or(true) {
            invalid += 1;
            continue;
        }
        let p = unwrap_pars(p);
        let count = p.get("uiCount").and_then(Value::as_u64).unwrap_or(0) as u32;
        if count == 0 {
            continue;
        }
        out.push(Period {
            count,
            period_ms: f64_at(p, "dPeriod"),
            start_ms: f64_at(p, "dStart"),
            duration_ms: f64_at(p, "dDuration"),
        });
    }
    (out, invalid)
}

/// Minimal base64 decoder (variant XML stores byte arrays base64-encoded).
pub fn base64_decode(s: &str) -> Vec<u8> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        let Some(v) = val(c) else { continue };
        acc = ((acc << 6) | v) & 0x00FF_FFFF;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    out
}

/// How frames map onto (position, time, z).
#[derive(Debug, Clone, Default)]
pub struct FrameLayout {
    pub t_count: u32,
    pub p_count: u32,
    pub z_count: u32,
    /// Counts of every counted loop, outermost first, with an axis letter each.
    pub order: Vec<(char, u32)>,
    /// Why the layout differs from the loop tree (extra frames, interrupted acquisition).
    pub layout_note: Option<String>,
}

impl FrameLayout {
    pub fn from_loops(loops: &[Loop], frame_count: u32) -> Self {
        let mut order: Vec<(char, u32)> = loops.iter().map(|l| (l.kind.axis(), l.count)).collect();
        let product: u64 = order.iter().map(|(_, c)| u64::from(*c)).product();
        let fc = u64::from(frame_count.max(1));
        let mut note = None;
        if order.is_empty() {
            order = vec![('T', frame_count.max(1))];
        } else if product < fc {
            note = Some(format!(
                "the file holds {fc} frames but the loop tree describes {product}; the extra frames are not addressed"
            ));
        } else if product > fc {
            // Interrupted acquisition: shrink the outermost loop to the frames actually written.
            let inner: u64 = order.iter().skip(1).map(|(_, c)| u64::from(*c)).product();
            let outer = fc.div_ceil(inner.max(1)).max(1);
            note = Some(format!(
                "the loop tree describes {product} frames but the file holds {fc} (interrupted acquisition?); the outermost loop is cut to {outer} and frames past {fc} are reported missing"
            ));
            order[0].1 = u32::try_from(outer).unwrap_or(u32::MAX);
        }
        let prod = |axis: char| {
            order
                .iter()
                .filter(|(a, _)| *a == axis)
                .map(|(_, c)| *c)
                .product::<u32>()
        };
        FrameLayout {
            t_count: prod('T'),
            p_count: prod('P'),
            z_count: prod('Z'),
            order,
            layout_note: note,
        }
    }

    /// Frame index for (p, t, z) following the loop nesting order.
    pub fn frame_index(&self, p: u32, t: u32, z: u32) -> u32 {
        let mut idx = 0u64;
        let (mut tp, mut tt, mut tz) = (p, t, z);
        // Consume each axis value from innermost to outermost using each loop's count.
        let mut mult = 1u64;
        for (axis, count) in self.order.iter().rev() {
            let count = (*count).max(1);
            let slot = match axis {
                'T' => &mut tt,
                'P' => &mut tp,
                _ => &mut tz,
            };
            let val = *slot % count;
            *slot /= count;
            idx += u64::from(val) * mult;
            mult *= u64::from(count);
        }
        u32::try_from(idx).unwrap_or(u32::MAX)
    }

    /// Per-loop indices (outermost first) of frame `frame`.
    pub fn loop_indices(&self, frame: u32) -> Vec<u32> {
        let mut rest = frame;
        let mut out = vec![0u32; self.order.len()];
        for (i, (_, count)) in self.order.iter().enumerate().rev() {
            let count = (*count).max(1);
            out[i] = rest % count;
            rest /= count;
        }
        out
    }

    /// (p, t, z) of frame `frame`.
    pub fn coords(&self, frame: u32) -> (u32, u32, u32) {
        let idx = self.loop_indices(frame);
        let (mut p, mut t, mut z) = (0u32, 0u32, 0u32);
        for ((axis, count), v) in self.order.iter().zip(idx) {
            let slot = match axis {
                'T' => &mut t,
                'P' => &mut p,
                _ => &mut z,
            };
            *slot = *slot * count + v;
        }
        (p, t, z)
    }

    /// Number of frames the layout addresses.
    pub fn frames(&self) -> u64 {
        self.order.iter().map(|(_, c)| u64::from(*c)).product()
    }
}

// ---------------------------------------------------------------- picture planes (channels)

/// Modality bits of `uiModalityMask` with our names (bit values from the `nd2` package's
/// public enumeration, BSD-3; see provenance).
pub const MODALITY_BITS: &[(u64, &str)] = &[
    (0x1, "fluorescence"),
    (0x2, "brightfield"),
    (0x10, "phase_contrast"),
    (0x20, "dic"),
    (0x40, "rcm"),
    (0x80, "vcs"),
    (0x100, "camera"),
    (0x200, "laser_scanning_confocal"),
    (0x400, "spinning_disk_confocal"),
    (0x800, "swept_field_confocal_slit"),
    (0x1000, "swept_field_confocal_pinhole"),
    (0x2000, "dsd_confocal"),
    (0x4000, "sim"),
    (0x8000, "isim"),
    (0x1_0000, "multiphoton"),
    (0x2_0000, "tirf"),
    (0x4_0000, "live_sr"),
    (0x10_0000, "pmt"),
    (0x20_0000, "spectral"),
    (0x40_0000, "vaas_if"),
    (0x80_0000, "vaas_nf"),
    (0x100_0000, "transmitted_light_detector"),
    (0x200_0000, "non_descanned_detector"),
    (0x400_0000, "virtual_filter"),
    (0x800_0000, "gaasp"),
    (0x1000_0000, "remainder"),
    (0x2000_0000, "aux"),
    (0x4000_0000, "sora"),
];

/// Older files store a modality enumeration (`eModality`) instead of a mask.
fn modality_mask_from_enum(e: i64) -> u64 {
    match e {
        0 => 0x1 | 0x100,
        1 => 0x2 | 0x100,
        2 => 0x1 | 0x200,
        3 => 0x1 | 0x400,
        4 => 0x1 | 0x800,
        5 => 0x1 | 0x1_0000 | 0x200,
        6 => 0x2 | 0x10,
        7 => 0x2 | 0x20,
        8 => 0x1 | 0x20_0000 | 0x200,
        9 | 11 => 0x1 | 0x80_0000 | 0x200,
        10 => 0x1 | 0x40_0000 | 0x200,
        12 => 0x2000,
        _ => 0x1 | 0x100,
    }
}

/// Modality names of a mask; a mask with neither the fluorescence nor the brightfield bit is
/// read as brightfield for 3-component (RGB) planes and fluorescence otherwise.
pub fn modality_flags(mask: u64, component_count: u32) -> Vec<&'static str> {
    if mask & 0x3 == 0 {
        return vec![if component_count == 3 {
            "brightfield"
        } else {
            "fluorescence"
        }];
    }
    MODALITY_BITS
        .iter()
        .filter(|(b, _)| mask & b != 0)
        .map(|(_, n)| *n)
        .collect()
}

/// A readable acquisition mode from modality names.
pub fn acquisition_mode(flags: &[&str]) -> Option<String> {
    let has = |n: &str| flags.contains(&n);
    Some(
        if has("brightfield") && !has("fluorescence") {
            if has("phase_contrast") {
                "Phase Contrast"
            } else if has("dic") {
                "DIC"
            } else {
                "Brightfield"
            }
        } else if has("fluorescence") {
            if has("spinning_disk_confocal") {
                "Spinning Disk Confocal Fluorescence"
            } else if has("multiphoton") {
                "Multiphoton Fluorescence"
            } else if has("swept_field_confocal_slit") || has("swept_field_confocal_pinhole") {
                "Swept Field Confocal Fluorescence"
            } else if has("laser_scanning_confocal") {
                "Laser Scanning Confocal Fluorescence"
            } else if has("tirf") {
                "TIRF Fluorescence"
            } else {
                "Widefield Fluorescence"
            }
        } else {
            return None;
        }
        .to_string(),
    )
}

/// Detector settings of one picture plane.
#[derive(Debug, Clone, Default)]
pub struct CameraSetting {
    pub detector: Option<String>,
    pub exposure_ms: Option<f64>,
    /// `"2x2"`.
    pub binning: Option<String>,
    /// Calibrated/analog gain (older files).
    pub gain: Option<f64>,
    /// EM gain multiplier (newer files).
    pub em_gain: Option<f64>,
    /// `sSpecSettings` text parsed into `key: value` lines.
    pub settings_text: BTreeMap<String, String>,
}

/// One picture plane (channel) from `sPicturePlanes/sPlaneNew` (or `sPlane`).
#[derive(Debug, Clone, Default)]
pub struct PlaneDesc {
    pub description: Option<String>,
    pub component_count: u32,
    pub sample_index: u32,
    /// Display colour as written (`uiColor`, bytes R, G, B, A from the low end).
    pub color_abgr: Option<u32>,
    pub modality_mask: Option<u64>,
    pub modality: Vec<&'static str>,
    pub fluorophore: Option<String>,
    pub filters: Vec<String>,
    pub emission_nm: Option<f64>,
    pub excitation_nm: Option<f64>,
    /// `[start, end]` when the emission filter is described by a rising and a falling edge.
    pub emission_band_nm: Option<[f64; 2]>,
    pub camera: CameraSetting,
}

/// A spectrum point's wavelength (`dWavelength`, or integral `uiWavelength` in legacy files).
fn point_wavelength(p: &Value) -> f64 {
    f64_at(p, "dWavelength")
        .or_else(|| f64_at(p, "uiWavelength"))
        .unwrap_or(0.0)
}

fn spectrum_points(spectrum: Option<&Value>) -> Vec<&Value> {
    spectrum
        .and_then(|s| s.get("pPoint"))
        .map(list_items)
        .unwrap_or_default()
}

/// Wavelength of the point with the largest transmission value (first one on ties); 0 if none.
fn spectrum_peak(spectrum: Option<&Value>) -> f64 {
    let mut best: Option<(f64, f64)> = None;
    for p in spectrum_points(spectrum) {
        let tv = f64_at(p, "dTValue").unwrap_or(0.0);
        if best.is_none_or(|(b, _)| tv > b) {
            best = Some((tv, point_wavelength(p)));
        }
    }
    best.map_or(0.0, |(_, w)| w)
}

/// `[low, high]` of a filter spectrum described by a rising (`eType` 2) and a falling (`eType` 3)
/// edge.
fn band_edges(spectrum: Option<&Value>) -> Option<[f64; 2]> {
    let points = spectrum_points(spectrum);
    let edge = |t: i64| {
        points
            .iter()
            .find(|p| p.get("eType").and_then(Value::as_i64) == Some(t))
            .map(|p| point_wavelength(p))
            .filter(|w| *w > 0.0)
    };
    let (lo, hi) = (edge(2)?, edge(3)?);
    Some([lo.min(hi), lo.max(hi)])
}

/// A filter's characteristic wavelength: the centre of its band when it is given by edges,
/// else its transmission peak.
fn filter_wavelength(spectrum: Option<&Value>) -> f64 {
    band_edges(spectrum).map_or_else(|| spectrum_peak(spectrum), |[lo, hi]| f64::midpoint(lo, hi))
}

fn plane_wavelengths(plane: &Value, index: usize) -> (Option<f64>, Option<f64>, Option<[f64; 2]>) {
    let probe = plane.get("pFluorescentProbe");
    let filter = plane
        .get("pFilterPath")
        .and_then(|f| f.get("m_pFilter"))
        .and_then(|f| list_items(f).into_iter().next());
    let mut ex = spectrum_peak(probe.and_then(|p| p.get("m_ExcitationSpectrum")));
    if ex == 0.0
        && let Some(f) = filter
    {
        let spec = f.get("m_ExcitationSpectrum");
        let points = spectrum_points(spec);
        let count = spec
            .and_then(|s| s.get("uiCount"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if count > 1
            && points
                .iter()
                .all(|p| p.get("eType").and_then(Value::as_i64) == Some(4))
        {
            ex = points.get(index).map_or(0.0, |p| point_wavelength(p));
        }
        if ex == 0.0 {
            ex = filter_wavelength(spec);
        }
    }
    let mut em = spectrum_peak(probe.and_then(|p| p.get("m_EmissionSpectrum")));
    let mut band = None;
    if let Some(f) = filter {
        let spec = f.get("m_EmissionSpectrum");
        if em == 0.0 {
            em = filter_wavelength(spec);
        }
        band = band_edges(spec);
    }
    let pos = |w: f64| (w > 0.0).then_some(w);
    (pos(ex), pos(em), band)
}

fn parse_settings_text(s: &str) -> BTreeMap<String, String> {
    s.lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        .collect()
}

fn camera_from(sample: Option<&Value>, plane: &Value) -> CameraSetting {
    let mut c = CameraSetting::default();
    if let Some(s) = sample {
        // Newer layout: `sSampleSetting/aK/{pCameraSetting, dExposureTime, sSpecSettings}`.
        let cam = s.get("pCameraSetting");
        c.detector =
            cam.and_then(|c| str_at(c, "CameraUserName").or_else(|| str_at(c, "CameraUniqueName")));
        let q = cam.and_then(|c| c.get("PropertiesQuality"));
        c.exposure_ms = f64_at(s, "dExposureTime")
            .or_else(|| q.and_then(|q| f64_at(q, "Exposure")))
            .filter(|e| *e >= 0.0);
        c.em_gain = q
            .and_then(|q| f64_at(q, "GainMultiplier"))
            .filter(|g| *g > 0.0);
        if let Some(fmt) = cam
            .and_then(|c| c.get("FormatQuality"))
            .and_then(|f| f.get("fmtDesc"))
            && let (Some(bx), Some(by)) = (f64_at(fmt, "dBinningX"), f64_at(fmt, "dBinningY"))
        {
            c.binning = Some(format!("{bx}x{by}"));
        }
        if let Some(t) = s.get("sSpecSettings").and_then(Value::as_str) {
            c.settings_text = parse_settings_text(t);
        }
    }
    if let Some(cam) = plane.get("sCameraSetting") {
        // Older layout: per-plane `sCameraSetting`.
        c.detector = c.detector.or_else(|| str_at(cam, "sCameraName"));
        c.exposure_ms = c
            .exposure_ms
            .or_else(|| f64_at(cam, "dExposure").filter(|e| *e >= 0.0));
        c.gain = f64_at(cam, "dGain").filter(|g| *g > 0.0);
        if c.binning.is_none()
            && let (Some(bx), Some(by)) = (f64_at(cam, "dCamBinningX"), f64_at(cam, "dCamBinningY"))
            && bx > 0.0
        {
            c.binning = Some(format!("{bx}x{by}"));
        }
        if c.settings_text.is_empty()
            && let Some(t) = cam.get("sSpecSettings").and_then(Value::as_str)
        {
            c.settings_text = parse_settings_text(t);
        }
    }
    if c.binning.is_none() {
        c.binning = c.settings_text.get("Binning").cloned();
    }
    c
}

pub fn planes_from_lv(v: &Value) -> Vec<PlaneDesc> {
    let pm = unwrap_root(v);
    let pp = pm.get("sPicturePlanes");
    let Some(planes) = pp
        .and_then(|p| p.get("sPlaneNew").or_else(|| p.get("sPlane")))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let samples = pp.and_then(|p| p.get("sSampleSetting"));
    // One sample setting shared by every plane (a point scanner acquiring all channels at once).
    let single_sample = samples
        .and_then(Value::as_object)
        .filter(|m| m.len() == 1)
        .and_then(|m| m.values().next());
    let mut keys: Vec<&String> = planes.keys().collect();
    keys.sort_by_key(|k| k.trim_start_matches('a').parse::<u32>().unwrap_or(u32::MAX));
    keys.iter()
        .enumerate()
        .map(|(i, k)| {
            let p = &planes[*k];
            let probe = p.get("pFluorescentProbe");
            let component_count = p.get("uiCompCount").and_then(Value::as_u64).unwrap_or(1) as u32;
            let sample_index = p.get("uiSampleIndex").and_then(Value::as_u64).unwrap_or(0) as u32;
            let sk = if sample_index == 0 {
                i as u32
            } else {
                sample_index
            };
            let mask = p.get("uiModalityMask").and_then(Value::as_u64).or_else(|| {
                p.get("eModality")
                    .and_then(Value::as_i64)
                    .map(modality_mask_from_enum)
            });
            let (excitation_nm, emission_nm, emission_band_nm) = plane_wavelengths(p, i);
            let filters = p
                .get("pFilterPath")
                .and_then(|f| f.get("m_pFilter"))
                .map(list_items)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|f| str_at(f, "m_sName").or_else(|| str_at(f, "m_sUserName")))
                .collect();
            PlaneDesc {
                description: str_at(p, "sDescription").or_else(|| str_at(p, "sOpticalConfigName")),
                component_count,
                sample_index,
                color_abgr: p
                    .get("uiColor")
                    .or_else(|| probe.and_then(|q| q.get("m_uiColor")))
                    .and_then(Value::as_u64)
                    .map(|c| c as u32),
                modality_mask: mask,
                modality: modality_flags(mask.unwrap_or(0x1 | 0x100), component_count),
                fluorophore: probe.and_then(|q| str_at(q, "m_sName")),
                filters,
                emission_nm,
                excitation_nm,
                emission_band_nm,
                camera: camera_from(
                    samples
                        .and_then(|s| s.get(format!("a{sk}")))
                        .or(single_sample),
                    p,
                ),
            }
        })
        .collect()
}

/// `#RRGGBB` from a `uiColor` value (red in the lowest byte).
pub fn color_hex(abgr: u32) -> String {
    format!(
        "#{:02X}{:02X}{:02X}",
        abgr & 0xFF,
        (abgr >> 8) & 0xFF,
        (abgr >> 16) & 0xFF
    )
}

// ---------------------------------------------------------------- time

/// Julian day number of the Unix epoch.
const JDN_UNIX_EPOCH: f64 = 2_440_587.5;

/// ISO-8601 UTC (millisecond precision) for a Julian day number, or `None` when the value is
/// implausible (before 1900 or after 2100; uninitialized clocks store small numbers).
pub fn jdn_to_iso8601(jdn: f64) -> Option<String> {
    if !(2_415_020.5..2_488_069.5).contains(&jdn) {
        return None;
    }
    let ms = ((jdn - JDN_UNIX_EPOCH) * 86_400_000.0).round() as i64;
    Some(openreadout_core::time::unix_to_iso8601(
        ms.div_euclid(1000),
        ms.rem_euclid(1000) as u32,
    ))
}

/// Normalize the free-text acquisition date (`TextInfoItem_9`) to ISO-8601 *local* time
/// without an offset (`2009-03-06T10:58:30`): the text is written in the acquisition PC's
/// local time zone and locale, and the file does not record either. Accepted shapes: a date of
/// three numbers separated by `/`, `-` or `.` (year first when it has four digits, else year
/// last), a time `h:mm[:ss]`, and an optional `AM`/`PM`. Day and month are told apart only
/// when one of them exceeds 12 or they are equal; the corpus mixes month-first
/// (`9/28/2021 9:34:47 AM`) and day-first (`06/03/2009 10:58:30 AM`, 6 March per the Julian
/// day) texts, so ambiguous dates give `None` rather than a guess.
#[allow(clippy::many_single_char_names)]
pub fn text_datetime_to_iso8601(text: &str) -> Option<String> {
    let mut parts = text.split_whitespace();
    let date = parts.next()?;
    let time = parts.next()?;
    let meridiem = parts.next().map(str::to_ascii_uppercase);
    let d: Vec<u32> = date
        .split(['/', '-', '.'])
        .map(|p| p.parse::<u32>().ok())
        .collect::<Option<_>>()?;
    if d.len() != 3 {
        return None;
    }
    let (year, a, b) = if date.split(['/', '-', '.']).next()?.len() == 4 {
        (d[0], d[1], d[2]) // Y-M-D
    } else {
        (d[2], d[0], d[1])
    };
    let (month, day) = if date.split(['/', '-', '.']).next()?.len() == 4 || a == b {
        (a, b)
    } else if a > 12 && b <= 12 {
        (b, a)
    } else if b > 12 && a <= 12 {
        (a, b)
    } else {
        return None;
    };
    let t: Vec<u32> = time
        .split(':')
        .map(|p| p.parse::<u32>().ok())
        .collect::<Option<_>>()?;
    let (mut hour, minute, second) = match t.as_slice() {
        [h, m] => (*h, *m, 0),
        [h, m, s] => (*h, *m, *s),
        _ => return None,
    };
    match meridiem.as_deref() {
        Some("AM") if hour == 12 => hour = 0,
        Some("PM") if hour < 12 => hour += 12,
        Some("AM" | "PM") | None => {}
        Some(_) => return None,
    }
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return None,
    };
    if !(1900..=2100).contains(&year)
        || day == 0
        || day > days_in_month
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}"
    ))
}

// ---------------------------------------------------------------- events

/// Event meanings (codes from the `nd2` package's public enumeration, BSD-3), our names.
const EVENT_MEANINGS: &[&str] = &[
    "unspecified",
    "autofocus",
    "user_1_old",
    "user_2_old",
    "user_3_old",
    "user_4_old",
    "jobs",
    "command",
    "macro",
    "pause",
    "resume",
    "cancel",
    "ram_grab_zero_time",
    "time_loop_next_phase",
    "refocus",
    "stimulation",
    "external_stimulation",
    "experiment_start",
    "experiment_end",
    "phase_start",
    "phase_end",
    "before_xy_move",
    "after_xy_move",
    "before_z_series",
    "after_z_series",
    "before_lambda_loop",
    "after_lambda_loop",
    "before_large_image",
    "after_large_image",
    "before_stimulation",
    "after_stimulation",
    "user_events",
    "stream_data",
    "user_1",
    "user_2",
    "user_3",
    "user_4",
    "user_5",
    "user_6",
    "user_7",
    "user_8",
    "before_capture",
    "after_capture",
    "real_time_ttl_data",
    "no_acquisition_start",
    "no_acquisition_end",
    "hardware_error",
    "storm_event",
    "incubation_info",
    "incubation_error",
    "interactive_experiment_end",
    "experiment_pause",
    "wid_replenishment_start",
    "wid_replenishment_end",
    "nstorm",
];

fn event_record(
    time_ms: Option<f64>,
    code: i64,
    description: Option<String>,
    data: Option<String>,
) -> Value {
    let meaning = usize::try_from(code)
        .ok()
        .and_then(|c| EVENT_MEANINGS.get(c))
        .map_or_else(|| format!("meaning_{code}"), |s| (*s).to_string());
    let mut o = Map::new();
    o.insert("time_ms".into(), json!(time_ms));
    o.insert("meaning".into(), Value::String(meaning));
    o.insert("meaning_code".into(), Value::from(code));
    if let Some(d) = description {
        o.insert("description".into(), Value::String(d));
    }
    if let Some(d) = data {
        o.insert("data".into(), Value::String(d));
    }
    Value::Object(o)
}

/// Normalize an experiment-record tree (`ImageEventsLV!`, `ImageEvents!`,
/// `CustomData|ExperimentEventsV1_0!`, legacy `IEVE`) into event records.
pub fn events_from_lv(v: &Value) -> Vec<Value> {
    let rec = if v.get("pEvents").is_some() || v.get("pFirstEvent").is_some() {
        v
    } else {
        unwrap_root(v)
    };
    let mut out = Vec::new();
    if let Some(events) = rec.get("pEvents") {
        // Compact records: T time, T2 second clock, M meaning, D description, A data.
        for e in list_items(events) {
            out.push(event_record(
                f64_at(e, "T"),
                e.get("M").and_then(Value::as_i64).unwrap_or(0),
                str_at(e, "D"),
                str_at(e, "A"),
            ));
        }
    } else if let Some(first) = rec.get("pFirstEvent") {
        let items: Vec<&Value> = match first.get("no_name") {
            Some(Value::Array(a)) => a.iter().collect(),
            Some(one) => vec![one],
            None => list_items(first),
        };
        for e in items {
            out.push(event_record(
                f64_at(e, "dTime"),
                e.get("eMeaning").and_then(Value::as_i64).unwrap_or(0),
                str_at(e, "wsDescription"),
                str_at(e, "wsData"),
            ));
        }
    }
    out
}

// ---------------------------------------------------------------- ROIs

/// Drop the lowercase type prefix of every key (`m_vect2PerMPoint_Size` → `2PerMPoint_Size`,
/// `dCenterX` → `CenterX`) so both spellings of the ROI tree read the same.
fn strip_prefixes(v: &Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| {
                    let s = k.trim_start_matches(|c: char| c.is_ascii_lowercase() || c == '_');
                    (
                        if s.is_empty() {
                            k.clone()
                        } else {
                            s.to_string()
                        },
                        strip_prefixes(v),
                    )
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip_prefixes).collect()),
        other => other.clone(),
    }
}

const ROI_SHAPES: &[&str] = &[
    "any",
    "raster",
    "point",
    "rectangle",
    "ellipse",
    "polygon",
    "bezier",
    "line",
    "polyline",
    "circle",
    "square",
    "ring",
    "spiral",
];
const ROI_ROLES: &[&str] = &["any", "standard", "background", "reference", "stimulation"];
const ROI_SCOPES: &[&str] = &["any", "global", "per_position"];

fn name_of(table: &[&str], code: Option<i64>) -> Option<String> {
    let c = code?;
    Some(
        usize::try_from(c)
            .ok()
            .and_then(|i| table.get(i))
            .map_or_else(|| format!("code_{c}"), |s| (*s).to_string()),
    )
}

#[allow(clippy::many_single_char_names)]
fn roi_record(r: &Value, position: Option<u64>) -> Value {
    let info = r.get("Info").cloned().unwrap_or(Value::Null);
    let mut o = Map::new();
    o.insert("id".into(), json!(r.get("Id").and_then(Value::as_i64)));
    if let Some(l) = str_at(&info, "Label") {
        o.insert("label".into(), Value::String(l));
    }
    o.insert(
        "shape".into(),
        json!(name_of(
            ROI_SHAPES,
            info.get("ShapeType").and_then(Value::as_i64)
        )),
    );
    o.insert(
        "role".into(),
        json!(name_of(
            ROI_ROLES,
            info.get("InterpType").and_then(Value::as_i64)
        )),
    );
    o.insert(
        "scope".into(),
        json!(name_of(
            ROI_SCOPES,
            info.get("Scope").and_then(Value::as_i64)
        )),
    );
    if let Some(p) = position {
        o.insert("position_index".into(), Value::from(p));
    }
    if let Some(c) = info.get("Color").and_then(Value::as_u64) {
        o.insert("color".into(), Value::String(color_hex(c as u32)));
    }
    let n = r
        .get("AnimParams_Size")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut keyframes = Vec::new();
    for i in 0..n.min(4096) {
        let Some(a) = r.get(format!("AnimParams_{i}")) else {
            continue;
        };
        let bs = a.get("BoxShape");
        let ex = a.get("ExtrudedShape");
        let base: Vec<Value> = ex
            .map(|e| {
                let m = e
                    .get("BasePoints_Size")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                (0..m.min(65_536))
                    .filter_map(|j| e.get(format!("BasePoints_{j}")))
                    .map(|p| {
                        let vals: Vec<f64> = p
                            .as_object()
                            .map(|o| o.values().filter_map(Value::as_f64).collect())
                            .unwrap_or_default();
                        json!([vals.first(), vals.get(1)])
                    })
                    .collect()
            })
            .unwrap_or_default();
        keyframes.push(json!({
            "time_ms": f64_at(a, "TimeMs"),
            "center": [f64_at(a, "CenterX"), f64_at(a, "CenterY"), f64_at(a, "CenterZ")],
            "rotation_z": f64_at(a, "RotationZ"),
            "box_size": bs.map(|b| json!([f64_at(b, "SizeX"), f64_at(b, "SizeY"), f64_at(b, "SizeZ")])),
            "extrusion_z": ex.and_then(|e| f64_at(e, "SizeZ")),
            "base_points": base,
        }));
    }
    o.insert("keyframes".into(), Value::Array(keyframes));
    Value::Object(o)
}

/// Normalize `CustomData|RoiMetadata_v1!`: global ROIs (`Global_Size`, `Global_<i>`) and
/// per-position ROIs (`2PerMPoint_Size`, `2PerMPoint_<p>/{Size, <i>}`).
pub fn rois_from_lv(v: &Value) -> Vec<Value> {
    let t = strip_prefixes(unwrap_root(v));
    let mut out = Vec::new();
    let n = t.get("Global_Size").and_then(Value::as_u64).unwrap_or(0);
    for i in 0..n.min(65_536) {
        if let Some(r) = t.get(format!("Global_{i}")) {
            out.push(roi_record(r, None));
        }
    }
    let np = t
        .get("2PerMPoint_Size")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    for p in 0..np.min(65_536) {
        let Some(pp) = t.get(format!("2PerMPoint_{p}")) else {
            continue;
        };
        let m = pp.get("Size").and_then(Value::as_u64).unwrap_or(0);
        for i in 0..m.min(65_536) {
            if let Some(r) = pp.get(i.to_string()) {
                out.push(roi_record(r, Some(p)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(order: Vec<(char, u32)>) -> FrameLayout {
        let prod = |a: char| order.iter().filter(|o| o.0 == a).map(|o| o.1).product();
        FrameLayout {
            t_count: prod('T'),
            p_count: prod('P'),
            z_count: prod('Z'),
            order,
            layout_note: None,
        }
    }

    #[test]
    fn frame_index_follows_nesting() {
        let l = layout(vec![('T', 3), ('P', 4), ('Z', 5)]);
        assert_eq!(l.frame_index(0, 0, 0), 0);
        assert_eq!(l.frame_index(0, 0, 1), 1);
        assert_eq!(l.frame_index(1, 0, 0), 5);
        assert_eq!(l.frame_index(0, 1, 0), 20);
        assert_eq!(l.frame_index(3, 2, 4), 2 * 20 + 3 * 5 + 4);
        assert_eq!(l.coords(2 * 20 + 3 * 5 + 4), (3, 2, 4));
    }

    #[test]
    fn custom_loop_folds_into_t() {
        let l = layout(vec![('T', 2), ('T', 3)]);
        assert_eq!(l.t_count, 6);
        for t in 0..6 {
            assert_eq!(l.frame_index(0, t, 0), t);
            assert_eq!(l.coords(t), (0, t, 0));
        }
    }

    #[test]
    fn loop_tree_masks_and_merging() {
        let exp = json!({"SLxExperiment": {
            "eType": 2, "pItemValid": [0, 1, 1],
            "uLoopPars": {"uiCount": 3, "Points": {"": [
                {"dPosX": 1.0, "dPosY": 2.0, "dPosZ": 3.0, "dPosName": "a"},
                {"dPosX": 4.0, "dPosY": 5.0, "dPosZ": 6.0, "dPosName": "b"},
                {"dPosX": 7.0, "dPosY": 8.0, "dPosZ": 9.0, "dPosName": ""}]}},
            "ppNextLevelEx": {"": {
                "eType": 6, "uLoopPars": {"pPlanes": {"uiCount": 2}},
                "ppNextLevelEx": {"": [
                    {"eType": 4, "uLoopPars": {"uiCount": 5, "dZStep": 0.0, "dZLow": 0.0, "dZHigh": 2.0}},
                    {"eType": 4, "uLoopPars": {"uiCount": 3, "dZStep": 1.0}}
                ]}
            }}
        }});
        let loops = loops_from_lv(&exp);
        assert_eq!(loops.len(), 2);
        assert_eq!(loops[0].kind, LoopKind::XyPositionLoop);
        assert_eq!(loops[0].count, 2);
        assert_eq!(loops[0].invalid_items, 1);
        assert_eq!(loops[0].positions[0].pos_name.as_deref(), Some("b"));
        assert_eq!(loops[1].kind, LoopKind::ZStackLoop);
        assert_eq!(loops[1].count, 5);
        assert_eq!(loops[1].z_step_um, Some(0.5));
    }

    #[test]
    fn ne_time_periods_respect_validity() {
        let exp = json!({"no_name": {"eType": 8, "uLoopPars": {"no_name": {
            "uiCount": 6,
            "pPeriod": {"_00": {"uiCount": 4, "dPeriod": 5000.0}, "_01": {"uiCount": 2, "dPeriod": 60000.0}},
            "pPeriodValid": {"_00": true, "_01": false}}}}});
        let loops = loops_from_lv(&exp);
        assert_eq!(loops[0].count, 4);
        assert_eq!(loops[0].periods.len(), 1);
        assert_eq!(loops[0].invalid_items, 1);
    }

    #[test]
    fn layout_handles_extra_and_missing_frames() {
        let lp = |kind, count| Loop {
            kind,
            count,
            declared_count: Some(count),
            level: 0,
            z_step_um: None,
            period_ms: None,
            periods: Vec::new(),
            positions: Vec::new(),
            repeat_count: None,
            invalid_items: 0,
        };
        let loops = vec![lp(LoopKind::TimeLoop, 10), lp(LoopKind::ZStackLoop, 5)];
        let l = FrameLayout::from_loops(&loops, 51);
        assert_eq!((l.t_count, l.z_count), (10, 5));
        assert!(l.layout_note.is_some());
        let l = FrameLayout::from_loops(&loops, 23);
        assert_eq!((l.t_count, l.z_count), (5, 5));
        assert!(l.layout_note.unwrap().contains("interrupted"));
    }

    #[test]
    fn wavelengths_follow_probe_then_filter() {
        let plane = json!({
            "uiCompCount": 1,
            "pFluorescentProbe": {"m_ExcitationSpectrum": {"uiCount": 0, "pPoint": {}}},
            "pFilterPath": {"m_pFilter": {"i0000000000": {
                "m_sName": "GFP",
                "m_ExcitationSpectrum": {"uiCount": 1, "pPoint": {"Point0": {"eType": 4, "dWavelength": 488.0, "dTValue": 0.0}}},
                "m_EmissionSpectrum": {"uiCount": 2, "pPoint": {
                    "Point0": {"eType": 2, "uiWavelength": 500, "dTValue": 0.0},
                    "Point1": {"eType": 3, "uiWavelength": 550, "dTValue": 0.0}}}
            }}}
        });
        let (ex, em, band) = plane_wavelengths(&plane, 0);
        assert_eq!(ex, Some(488.0));
        assert_eq!(em, Some(525.0)); // centre of the 500-550 nm band, not its rising edge
        assert_eq!(band, Some([500.0, 550.0]));
    }

    #[test]
    fn modality_and_color() {
        assert_eq!(
            modality_flags(1025, 1),
            vec!["fluorescence", "spinning_disk_confocal"]
        );
        assert_eq!(modality_flags(0x100, 3), vec!["brightfield"]);
        assert_eq!(
            acquisition_mode(&["fluorescence", "spinning_disk_confocal"]).as_deref(),
            Some("Spinning Disk Confocal Fluorescence")
        );
        assert_eq!(color_hex(65280), "#00FF00");
        assert_eq!(color_hex(255), "#FF0000");
    }

    #[test]
    fn jdn_conversion() {
        // ome-karl-sample-image frame 0 (the text info says 11:15:06 local, UTC+2)
        assert_eq!(
            jdn_to_iso8601(2_457_910.885_497_453_6).as_deref(),
            Some("2017-06-06T09:15:06.980Z")
        );
        assert_eq!(jdn_to_iso8601(397.72), None);
    }

    #[test]
    fn text_dates_normalize_only_when_unambiguous() {
        let f = text_datetime_to_iso8601;
        // corpus texts (TextInfoItem_9)
        assert_eq!(
            f("9/28/2021  9:34:47 AM").as_deref(),
            Some("2021-09-28T09:34:47")
        );
        assert_eq!(
            f("7/19/2017  1:44:22 PM").as_deref(),
            Some("2017-07-19T13:44:22")
        );
        assert_eq!(
            f("29/12/2010  12:13:08").as_deref(),
            Some("2010-12-29T12:13:08")
        );
        assert_eq!(
            f("13/07/2023  4:31:35 pm").as_deref(),
            Some("2023-07-13T16:31:35")
        );
        assert_eq!(
            f("06/06/2017  11:15:06").as_deref(),
            Some("2017-06-06T11:15:06")
        );
        assert_eq!(
            f("17/02/2026  11:36:24").as_deref(),
            Some("2026-02-17T11:36:24")
        );
        assert_eq!(
            f("12/25/2020 12:05:00 AM").as_deref(),
            Some("2020-12-25T00:05:00")
        );
        assert_eq!(
            f("2020-12-25 12:05").as_deref(),
            Some("2020-12-25T12:05:00")
        );
        // day/month ambiguous: `06/03/2009` is 6 March in that file, `11/3/2009` unknown
        assert_eq!(f("06/03/2009  10:58:30 AM"), None);
        assert_eq!(f("11/3/2009  12:20:54 PM"), None);
        assert_eq!(f("3-7-2026  13:32:25"), None);
        // garbage
        assert_eq!(f("31/02/2020 10:00:00"), None);
        assert_eq!(f("yesterday"), None);
        assert_eq!(f("9/28/2021 25:00:00"), None);
    }

    #[test]
    fn events_and_rois() {
        let ev = json!({"RLxExperimentRecord": {"uiCount": 1, "pEvents": {"": {"T": 12.5, "M": 7, "D": "", "A": "Wait(2);"}}}});
        let e = events_from_lv(&ev);
        assert_eq!(e[0]["meaning"], "command");
        assert_eq!(e[0]["data"], "Wait(2);");
        let legacy = json!({"pFirstEvent": {"no_name": [{"dTime": 1.0, "eMeaning": 17}, {"dTime": 2.0, "eMeaning": 18}]}});
        let e = events_from_lv(&legacy);
        assert_eq!(e.len(), 2);
        assert_eq!(e[1]["meaning"], "experiment_end");
        let rois = json!({"RoiMetadata_v1": {
            "m_vectGlobal_Size": 1,
            "m_vectGlobal_0": {"Id": 3, "Info": {"ShapeType": 3, "InterpType": 1, "Label": "cell"},
                "AnimParams_Size": 1,
                "AnimParams_0": {"TimeMs": 0.0, "CenterX": 10.0, "CenterY": -5.0, "CenterZ": 0.0,
                    "BoxShape": {"SizeX": 4.0, "SizeY": 2.0, "SizeZ": 0.0}}}}});
        let r = rois_from_lv(&rois);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0]["shape"], "rectangle");
        assert_eq!(r[0]["label"], "cell");
        assert_eq!(r[0]["keyframes"][0]["center"][0], 10.0);
    }
}
