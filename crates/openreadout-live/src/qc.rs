//! Live QC: metrics computed on each new plane or scan and declarative rules over them.
//!
//! Metrics (all dimensionless):
//!
//! | metric | on | meaning |
//! | --- | --- | --- |
//! | `saturated_fraction` | image plane | fraction of samples at the detector's ceiling: 2^bits − 1 when the file records its significant bits (`images[].extra.bits_significant`, `significant_bits`, `component_bit_count`), else the pixel type's maximum |
//! | `sharpness_ratio` | image plane | variance of the Laplacian of the plane (downsampled to at most 256 px a side) divided by the median of the first `baseline` planes of the same image and channel: a focus-drift proxy |
//! | `interval_ratio` | time point | gap between the arrival of time point t and t-1 divided by the median gap so far: dropped or delayed frames |
//! | `tic_ratio` | MS scan | total ion current of an MS1 scan divided by the median of the first `baseline` MS1 scans: spray or signal drop |
//!
//! A rule names a metric and a bound (`min` and/or `max`); a value outside the bound gives a
//! `qc` event naming the rule and the measured value. Rules live in a TOML file
//! ([`DEFAULT_RULES`] is the built-in set):
//!
//! ```toml
//! [[rule]]
//! name = "saturation"
//! technique = "imaging"
//! metric = "saturated_fraction"
//! max = 0.01
//! ```

#![allow(clippy::many_single_char_names)] // w/h/x/y/p are the usual image names

use std::collections::HashMap;

use openreadout_core::reader::PlaneIndex;
use openreadout_core::{Error, PixelType, Plane, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The built-in rules (`watch --qc` without `--qc-rules`).
pub const DEFAULT_RULES: &str = r#"# OpenReadout live QC rules (https://openreadout.github.io/openreadout/guides/lab-shares.html#qc-rules). Copy, edit, pass with --qc-rules.

[[rule]]
name = "saturation"
technique = "imaging"
metric = "saturated_fraction"
max = 0.01
description = "more than 1% of the plane's samples sit at the detector's maximum"

[[rule]]
name = "focus_drift"
technique = "imaging"
metric = "sharpness_ratio"
min = 0.5
baseline = 3
description = "the plane is less than half as sharp as the first planes of its channel"

[[rule]]
name = "dropped_frames"
technique = "imaging"
metric = "interval_ratio"
max = 1.8
baseline = 3
description = "a time point arrived after more than 1.8 times the median interval"

[[rule]]
name = "tic_drop"
technique = "mass_spectrometry"
metric = "tic_ratio"
min = 0.2
baseline = 5
description = "an MS1 scan's total ion current fell below 20% of the first scans' median"
"#;

/// A QC metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Metric {
    /// Fraction of samples at the detector's ceiling (recorded bit depth, else the pixel
    /// type's maximum).
    SaturatedFraction,
    /// Plane sharpness relative to the first planes of the same image and channel.
    SharpnessRatio,
    /// Gap before a new time point relative to the median gap.
    IntervalRatio,
    /// MS1 total ion current relative to the first MS1 scans.
    TicRatio,
}

impl Metric {
    /// The snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Metric::SaturatedFraction => "saturated_fraction",
            Metric::SharpnessRatio => "sharpness_ratio",
            Metric::IntervalRatio => "interval_ratio",
            Metric::TicRatio => "tic_ratio",
        }
    }
}

/// One rule.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Name reported in `qc` events.
    pub name: String,
    /// `imaging`, `mass_spectrometry` or `any`: which data the rule applies to.
    #[serde(default = "any")]
    pub technique: String,
    /// The metric.
    pub metric: Metric,
    /// Lower bound; a smaller value fires the rule.
    #[serde(default)]
    pub min: Option<f64>,
    /// Upper bound; a larger value fires the rule.
    #[serde(default)]
    pub max: Option<f64>,
    /// Planes, scans or intervals that form the baseline of a ratio metric (default 3).
    #[serde(default)]
    pub baseline: Option<usize>,
    /// Plain-words explanation, copied into events.
    #[serde(default)]
    pub description: Option<String>,
}

fn any() -> String {
    "any".into()
}

/// A rule set.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Rules {
    /// The rules, evaluated in order.
    pub rule: Vec<Rule>,
}

impl Rules {
    /// Parse a TOML rule file.
    pub fn parse(text: &str) -> Result<Rules> {
        let r: Rules = toml::from_str(text).map_err(|e| Error::Usage(format!("QC rules: {e}")))?;
        for x in &r.rule {
            if x.min.is_none() && x.max.is_none() {
                return Err(Error::Usage(format!(
                    "QC rule '{}' needs `min` or `max`",
                    x.name
                )));
            }
            if !["imaging", "mass_spectrometry", "any"].contains(&x.technique.as_str()) {
                return Err(Error::Usage(format!(
                    "QC rule '{}': technique must be imaging, mass_spectrometry or any",
                    x.name
                )));
            }
        }
        Ok(r)
    }

    /// The built-in rules.
    pub fn defaults() -> Rules {
        Rules::parse(DEFAULT_RULES).unwrap_or(Rules { rule: Vec::new() })
    }

    fn wants(&self, m: Metric) -> bool {
        self.rule.iter().any(|r| r.metric == m)
    }

    fn baseline(&self, m: Metric) -> usize {
        self.rule
            .iter()
            .filter(|r| r.metric == m)
            .filter_map(|r| r.baseline)
            .max()
            .unwrap_or(3)
            .max(1)
    }

    /// Rules on `m` that `value` violates, as findings.
    pub fn evaluate(&self, m: Metric, technique: &str, value: f64) -> Vec<QcFinding> {
        self.rule
            .iter()
            .filter(|r| r.metric == m && (r.technique == "any" || r.technique == technique))
            .filter_map(|r| {
                let (fired, bound, threshold) = match (r.min, r.max) {
                    (Some(lo), _) if value < lo => (true, "min", lo),
                    (_, Some(hi)) if value > hi => (true, "max", hi),
                    _ => (false, "", 0.0),
                };
                fired.then(|| QcFinding {
                    rule: r.name.clone(),
                    metric: m,
                    value: (value * 1e6).round() / 1e6,
                    bound: bound.into(),
                    threshold,
                    description: r.description.clone(),
                    saturation_level: None,
                })
            })
            .collect()
    }
}

/// A fired rule.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QcFinding {
    /// Rule name.
    pub rule: String,
    /// Metric measured.
    pub metric: Metric,
    /// Measured value.
    pub value: f64,
    /// `min` or `max`: which bound was crossed.
    pub bound: String,
    /// The bound's value.
    pub threshold: f64,
    /// The rule's description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `saturated_fraction` only: the sample value counted as saturated (2^bits − 1 from the
    /// file's recorded bit depth, else the pixel type's maximum).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation_level: Option<f64>,
}

/// Samples of a plane as `f64` (first sample of each pixel), little-endian as `Plane` stores them.
pub fn samples(p: &Plane) -> Vec<f64> {
    let bps = p.pixel_type.bytes_per_sample();
    let spp = p.samples_per_pixel.max(1) as usize;
    let stride = bps * spp;
    let n = (p.width as usize).saturating_mul(p.height as usize);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = i * stride;
        let Some(b) = p.data.get(o..o + bps) else {
            break;
        };
        let v = match p.pixel_type {
            PixelType::Uint8 => f64::from(b[0]),
            PixelType::Int8 => f64::from(b[0] as i8),
            PixelType::Uint16 => f64::from(u16::from_le_bytes([b[0], b[1]])),
            PixelType::Int16 => f64::from(i16::from_le_bytes([b[0], b[1]])),
            PixelType::Uint32 => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            PixelType::Int32 => f64::from(i32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            PixelType::Float => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            PixelType::Double => {
                f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            }
            _ => 0.0,
        };
        out.push(v);
    }
    out
}

/// Fraction of samples at the pixel type's maximum (`None` for floating point).
pub fn saturated_fraction(p: &Plane) -> Option<f64> {
    saturated_fraction_at(p, None).map(|(f, _)| f)
}

/// The pixel type's maximum (`None` for floating point).
fn type_max(p: PixelType) -> Option<f64> {
    Some(match p {
        PixelType::Uint8 => f64::from(u8::MAX),
        PixelType::Uint16 => f64::from(u16::MAX),
        PixelType::Uint32 => f64::from(u32::MAX),
        PixelType::Int8 => f64::from(i8::MAX),
        PixelType::Int16 => f64::from(i16::MAX),
        PixelType::Int32 => f64::from(i32::MAX),
        _ => return None,
    })
}

/// Fraction of samples at `level` (a recorded detector ceiling such as 16383 for 14-bit data,
/// see `openreadout_core::stats::recorded_saturation`), else at the pixel type's maximum;
/// with the level used. A sample above `level` proves the recorded depth wrong, and the type's
/// maximum is used instead. `None` for floating point.
pub fn saturated_fraction_at(p: &Plane, level: Option<f64>) -> Option<(f64, f64)> {
    let max = type_max(p.pixel_type)?;
    let s = samples(p);
    if s.is_empty() {
        return None;
    }
    let top = s.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let at = level.filter(|l| *l < max && top <= *l).unwrap_or(max);
    Some((
        s.iter().filter(|v| **v >= at).count() as f64 / s.len() as f64,
        at,
    ))
}

/// Variance of the 4-neighbour Laplacian of the plane, block-averaged to at most 256 pixels a
/// side first (so noise does not dominate and large planes stay cheap).
pub fn sharpness(p: &Plane) -> Option<f64> {
    let (w, h) = (p.width as usize, p.height as usize);
    if w < 3 || h < 3 {
        return None;
    }
    let s = samples(p);
    if s.len() < w * h {
        return None;
    }
    let f = w.max(h).div_ceil(256).max(1);
    let (dw, dh) = (w / f, h / f);
    if dw < 3 || dh < 3 {
        return None;
    }
    let mut d = vec![0f64; dw * dh];
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = 0.0;
            for yy in 0..f {
                for xx in 0..f {
                    acc += s[(y * f + yy) * w + x * f + xx];
                }
            }
            d[y * dw + x] = acc / (f * f) as f64;
        }
    }
    let mut sum = 0.0;
    let mut sum2 = 0.0;
    let mut n = 0.0;
    for y in 1..dh - 1 {
        for x in 1..dw - 1 {
            let c = d[y * dw + x];
            let l =
                d[y * dw + x - 1] + d[y * dw + x + 1] + d[(y - 1) * dw + x] + d[(y + 1) * dw + x]
                    - 4.0 * c;
            sum += l;
            sum2 += l * l;
            n += 1.0;
        }
    }
    let mean = sum / n;
    Some((sum2 / n - mean * mean).max(0.0))
}

fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let m = s.len() / 2;
    Some(if s.len().is_multiple_of(2) {
        f64::midpoint(s[m - 1], s[m])
    } else {
        s[m]
    })
}

/// Per-data-set QC state: the baselines ratio metrics compare against. Bounded: at most
/// `baseline` values per key plus the last arrival time per image.
#[derive(Debug, Default)]
pub struct QcState {
    sharp: HashMap<(u32, u32), Vec<f64>>,
    tic: Vec<f64>,
    /// Per image: last time point seen, its arrival (s), recent gaps.
    arrivals: HashMap<u32, (u32, f64, Vec<f64>)>,
}

/// Most recent gaps kept for the interval median.
const MAX_GAPS: usize = 64;

impl QcState {
    /// Metrics of a new plane, evaluated against `rules`. `arrival_s` is when the plane was
    /// seen (seconds on any monotonic clock); `t` its time index.
    pub fn plane(
        &mut self,
        rules: &Rules,
        image: u32,
        c: u32,
        t: u32,
        p: &Plane,
        arrival_s: f64,
    ) -> Vec<QcFinding> {
        self.plane_at(rules, image, PlaneIndex { c, z: 0, t }, p, arrival_s, None)
    }

    /// [`QcState::plane`] with the detector's recorded saturation level (2^bits − 1, from
    /// `openreadout_core::stats::recorded_saturation`); `None` uses the pixel type's maximum.
    pub fn plane_at(
        &mut self,
        rules: &Rules,
        image: u32,
        index: PlaneIndex,
        p: &Plane,
        arrival_s: f64,
        level: Option<f64>,
    ) -> Vec<QcFinding> {
        let mut out = Vec::new();
        if rules.wants(Metric::SaturatedFraction)
            && let Some((v, at)) = saturated_fraction_at(p, level)
        {
            out.extend(
                rules
                    .evaluate(Metric::SaturatedFraction, "imaging", v)
                    .into_iter()
                    .map(|mut f| {
                        f.saturation_level = Some(at);
                        f
                    }),
            );
        }
        if rules.wants(Metric::SharpnessRatio)
            && let Some(v) = sharpness(p)
        {
            let n = rules.baseline(Metric::SharpnessRatio);
            let base = self.sharp.entry((image, index.c)).or_default();
            if base.len() < n {
                base.push(v);
            } else if let Some(m) = median(base).filter(|m| *m > 0.0) {
                out.extend(rules.evaluate(Metric::SharpnessRatio, "imaging", v / m));
            }
        }
        if rules.wants(Metric::IntervalRatio) {
            out.extend(self.time_point(rules, image, index.t, arrival_s));
        }
        out
    }

    /// Record the arrival of time point `t` of `image`; a gap far above the median fires
    /// `interval_ratio` rules. Planes of a time point already seen are ignored.
    pub fn time_point(&mut self, rules: &Rules, image: u32, t: u32, at_s: f64) -> Vec<QcFinding> {
        let n = rules.baseline(Metric::IntervalRatio);
        let e = self.arrivals.entry(image).or_insert((t, at_s, Vec::new()));
        if t <= e.0 {
            return Vec::new();
        }
        let gap = at_s - e.1;
        e.0 = t;
        e.1 = at_s;
        let mut out = Vec::new();
        if e.2.len() >= n
            && let Some(m) = median(&e.2).filter(|m| *m > 0.0)
        {
            out = rules.evaluate(Metric::IntervalRatio, "imaging", gap / m);
        }
        e.2.push(gap);
        if e.2.len() > MAX_GAPS {
            e.2.remove(0);
        }
        out
    }

    /// Metrics of a new MS scan (`ms_level`, total ion current).
    pub fn scan(&mut self, rules: &Rules, ms_level: u32, tic: f64) -> Vec<QcFinding> {
        if ms_level != 1 || !rules.wants(Metric::TicRatio) || !tic.is_finite() {
            return Vec::new();
        }
        let n = rules.baseline(Metric::TicRatio);
        if self.tic.len() < n {
            self.tic.push(tic);
            return Vec::new();
        }
        median(&self.tic)
            .filter(|m| *m > 0.0)
            .map(|m| rules.evaluate(Metric::TicRatio, "mass_spectrometry", tic / m))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(w: u32, h: u32, f: impl Fn(u32, u32) -> u16) -> Plane {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&f(x, y).to_le_bytes());
            }
        }
        Plane {
            width: w,
            height: h,
            pixel_type: PixelType::Uint16,
            samples_per_pixel: 1,
            data,
        }
    }

    #[test]
    fn defaults_parse() {
        let r = Rules::defaults();
        assert_eq!(r.rule.len(), 4);
        assert!(Rules::parse("[[rule]]\nname='x'\nmetric='tic_ratio'\n").is_err());
        assert!(Rules::parse("[[rule]]\nname='x'\nmetric='nope'\nmin=1\n").is_err());
    }

    #[test]
    fn saturation_fires() {
        let r = Rules::defaults();
        let p = plane(10, 10, |x, _| if x < 2 { u16::MAX } else { 100 });
        assert!((saturated_fraction(&p).unwrap() - 0.2).abs() < 1e-9);
        let mut st = QcState::default();
        let f = st.plane(&r, 0, 0, 0, &p, 0.0);
        assert!(
            f.iter()
                .any(|f| f.rule == "saturation" && (f.value - 0.2).abs() < 1e-9)
        );
        let ok = plane(10, 10, |_, _| 100);
        assert!(
            st.plane(&r, 0, 0, 1, &ok, 1.0)
                .iter()
                .all(|f| f.rule != "saturation")
        );
    }

    /// 14-bit data in uint16: saturation at 16383 when the file records the depth.
    #[test]
    fn saturation_uses_the_recorded_bit_depth() {
        let r = Rules::defaults();
        let p = plane(10, 10, |x, _| if x < 2 { 16383 } else { 100 });
        assert_eq!(saturated_fraction(&p), Some(0.0));
        let (f, at) = saturated_fraction_at(&p, Some(16383.0)).unwrap();
        assert!((f - 0.2).abs() < 1e-9);
        assert!((at - 16383.0).abs() < f64::EPSILON);
        let mut st = QcState::default();
        let found = st.plane_at(&r, 0, PlaneIndex::default(), &p, 0.0, Some(16383.0));
        let hit = found.iter().find(|f| f.rule == "saturation").unwrap();
        assert_eq!(hit.saturation_level, Some(16383.0));
        assert!(
            st.plane(&r, 0, 0, 1, &p, 1.0)
                .iter()
                .all(|f| f.rule != "saturation")
        );
        // a sample above the recorded depth: the type's maximum
        let q = plane(10, 10, |x, _| if x < 2 { u16::MAX } else { 20000 });
        let (f, at) = saturated_fraction_at(&q, Some(16383.0)).unwrap();
        assert!((f - 0.2).abs() < 1e-9);
        assert!((at - 65535.0).abs() < f64::EPSILON);
    }

    #[test]
    fn focus_drift_fires_on_blur() {
        let r = Rules::defaults();
        let sharp = plane(
            64,
            64,
            |x, y| if (x / 4 + y / 4) % 2 == 0 { 1000 } else { 0 },
        );
        let blurred = plane(64, 64, |x, _| 500 + (x as u16));
        let mut st = QcState::default();
        for t in 0..3 {
            assert!(st.plane(&r, 0, 0, t, &sharp, f64::from(t)).is_empty());
        }
        assert!(st.plane(&r, 0, 0, 3, &sharp, 3.0).is_empty());
        let f = st.plane(&r, 0, 0, 4, &blurred, 4.0);
        assert!(
            f.iter().any(|f| f.rule == "focus_drift" && f.value < 0.5),
            "{f:?}"
        );
    }

    #[test]
    fn dropped_frames_fire_on_gap() {
        let r = Rules::defaults();
        let mut st = QcState::default();
        let mut fired = Vec::new();
        for (t, at) in [(0, 0.0), (1, 1.0), (2, 2.0), (3, 3.0), (4, 4.0), (5, 7.0)] {
            fired.extend(st.time_point(&r, 0, t, at));
        }
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].rule, "dropped_frames");
        assert!((fired[0].value - 3.0).abs() < 1e-9);
    }

    #[test]
    fn tic_drop_fires() {
        let r = Rules::defaults();
        let mut st = QcState::default();
        for _ in 0..5 {
            assert!(st.scan(&r, 1, 1e6).is_empty());
        }
        assert!(st.scan(&r, 2, 1.0).is_empty(), "MS2 scans are not compared");
        assert!(st.scan(&r, 1, 9e5).is_empty());
        let f = st.scan(&r, 1, 1e5);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "tic_drop");
    }
}
