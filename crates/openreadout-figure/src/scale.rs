//! Scales from data values to pixels, nice tick positions and short tick labels.

/// Linear or logarithmic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleKind {
    /// Equal steps for equal differences.
    #[default]
    Linear,
    /// Equal steps for equal ratios (base 10). Values ≤ 0 have no position.
    Log,
}

/// A tick: its value, its pixel position and its label.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    /// Data value.
    pub value: f64,
    /// Position in pixels.
    pub px: f32,
    /// Label (empty for minor ticks).
    pub label: String,
    /// True for a labelled (major) tick.
    pub major: bool,
}

/// Maps a data interval onto a pixel interval. `lo` maps to `px0` and `hi` to `px1`, so a
/// reversed axis (NMR chemical shift, y pointing up) is `px0 > px1` or `lo > hi`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    /// Data value at `px0`.
    pub lo: f64,
    /// Data value at `px1`.
    pub hi: f64,
    /// Pixel position of `lo`.
    pub px0: f32,
    /// Pixel position of `hi`.
    pub px1: f32,
    /// Linear or log.
    pub kind: ScaleKind,
}

impl Scale {
    /// A linear scale.
    pub fn linear(lo: f64, hi: f64, px0: f32, px1: f32) -> Scale {
        Scale {
            lo,
            hi,
            px0,
            px1,
            kind: ScaleKind::Linear,
        }
    }

    /// A log scale (`lo` and `hi` must be > 0; others are replaced by 1 and 10).
    pub fn log(lo: f64, hi: f64, px0: f32, px1: f32) -> Scale {
        let ok = |v: f64| v.is_finite() && v > 0.0;
        let (lo, hi) = if ok(lo) && ok(hi) {
            (lo, hi)
        } else {
            (1.0, 10.0)
        };
        Scale {
            lo,
            hi,
            px0,
            px1,
            kind: ScaleKind::Log,
        }
    }

    fn t(&self, v: f64) -> Option<f64> {
        if !v.is_finite() {
            return None;
        }
        let (a, b, v) = match self.kind {
            ScaleKind::Linear => (self.lo, self.hi, v),
            ScaleKind::Log => {
                if v <= 0.0 {
                    return None;
                }
                (self.lo.log10(), self.hi.log10(), v.log10())
            }
        };
        let span = b - a;
        Some(if span != 0.0 && span.is_finite() {
            (v - a) / span
        } else {
            0.5
        })
    }

    /// Pixel position of `v` (`None` for non-finite values and for v ≤ 0 on a log scale).
    pub fn px(&self, v: f64) -> Option<f32> {
        let t = self.t(v)?;
        let p = f64::from(self.px0) + t * f64::from(self.px1 - self.px0);
        // keep wild values drawable: far outside the canvas, but finite
        Some(p.clamp(-1e6, 1e6) as f32)
    }

    /// Data value at pixel `px`.
    pub fn value(&self, px: f32) -> f64 {
        let span = f64::from(self.px1 - self.px0);
        let t = if span == 0.0 {
            0.0
        } else {
            f64::from(px - self.px0) / span
        };
        match self.kind {
            ScaleKind::Linear => self.lo + t * (self.hi - self.lo),
            ScaleKind::Log => 10f64.powf(self.lo.log10() + t * (self.hi.log10() - self.lo.log10())),
        }
    }

    /// The smaller and larger data ends.
    pub fn domain(&self) -> (f64, f64) {
        (self.lo.min(self.hi), self.lo.max(self.hi))
    }

    /// Pixel length.
    pub fn len(&self) -> f32 {
        (self.px1 - self.px0).abs()
    }

    /// True for a zero-length scale.
    pub fn is_empty(&self) -> bool {
        self.len() == 0.0
    }

    /// Extend the data ends outward to the nearest tick for about `target` ticks (linear) or to
    /// whole decades (log).
    pub fn nice(mut self, target: usize) -> Scale {
        let (a, b) = self.domain();
        let flip = self.lo > self.hi;
        let (a, b) = match self.kind {
            ScaleKind::Linear => {
                let step = nice_step(b - a, target.max(2));
                if step > 0.0 {
                    ((a / step).floor() * step, (b / step).ceil() * step)
                } else {
                    (a, b)
                }
            }
            ScaleKind::Log => (10f64.powf(a.log10().floor()), 10f64.powf(b.log10().ceil())),
        };
        if flip {
            (self.lo, self.hi) = (b, a);
        } else {
            (self.lo, self.hi) = (a, b);
        }
        self
    }

    /// About `target` labelled ticks inside the domain, labelled with [`format_tick`]; log
    /// scales get decades (and 2× and 5× minor ticks when few decades are shown).
    pub fn ticks(&self, target: usize) -> Vec<Tick> {
        let (a, b) = self.domain();
        if !(a.is_finite() && b.is_finite()) {
            return Vec::new();
        }
        let eps = (b - a).abs() * 1e-9;
        match self.kind {
            ScaleKind::Linear => {
                let step = nice_step(b - a, target.max(2));
                if step <= 0.0 || !step.is_finite() {
                    return self
                        .px(a)
                        .map(|px| {
                            vec![Tick {
                                value: a,
                                px,
                                label: format_tick(a, 0.0),
                                major: true,
                            }]
                        })
                        .unwrap_or_default();
                }
                let first = (a / step - 1e-9).ceil() as i64;
                let last = (b / step + 1e-9).floor() as i64;
                let mut out = Vec::new();
                for k in first..=last.min(first.saturating_add(200)) {
                    let v = k as f64 * step;
                    let v = if v.abs() < step * 1e-9 { 0.0 } else { v };
                    if v < a - eps || v > b + eps {
                        continue;
                    }
                    if let Some(px) = self.px(v) {
                        out.push(Tick {
                            value: v,
                            px,
                            label: format_tick(v, step),
                            major: true,
                        });
                    }
                }
                out
            }
            ScaleKind::Log => {
                let (la, lb) = (a.log10(), b.log10());
                let decades = (lb - la).max(0.0);
                let every = ((decades / target.max(1) as f64).ceil() as i32).max(1);
                let mut out = Vec::new();
                let d0 = la.floor() as i32;
                let d1 = lb.ceil() as i32;
                for d in d0..=d1.min(d0.saturating_add(400)) {
                    for m in [1u8, 2, 5] {
                        let v = f64::from(m) * 10f64.powi(d);
                        if v < a * (1.0 - 1e-9) || v > b * (1.0 + 1e-9) {
                            continue;
                        }
                        if m != 1 && decades > 3.0 {
                            continue;
                        }
                        let labelled = if decades <= 1.5 {
                            true
                        } else {
                            m == 1 && d.rem_euclid(every) == 0
                        };
                        let label = if labelled && (1000.0..1e4).contains(&v) {
                            // match the SI labels of the decades above (1k, 10k, …)
                            format!("{}k", format_tick(v / 1000.0, v / 1000.0))
                        } else if labelled {
                            format_tick(v, v)
                        } else {
                            String::new()
                        };
                        if let Some(px) = self.px(v) {
                            out.push(Tick {
                                value: v,
                                px,
                                major: !label.is_empty(),
                                label,
                            });
                        }
                    }
                }
                out
            }
        }
    }
}

/// The step of the 1-2-5 series that gives about `target` intervals over `span`.
pub fn nice_step(span: f64, target: usize) -> f64 {
    let span = span.abs();
    if !span.is_finite() || span <= 0.0 {
        return 0.0;
    }
    let raw = span / target.max(1) as f64;
    let p = 10f64.powf(raw.log10().floor());
    let m = raw / p;
    let k = if m < 1.5 {
        1.0
    } else if m < 3.5 {
        2.0
    } else if m < 7.5 {
        5.0
    } else {
        10.0
    };
    k * p
}

/// A tick label for `v` on an axis whose ticks are `step` apart: as many decimals as the step
/// needs; SI suffixes from 10 000 up (`20k`, `1.5M`); an exponent below 0.001 (`2e-5`).
pub fn format_tick(v: f64, step: f64) -> String {
    if !v.is_finite() {
        return "–".into();
    }
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    let step = if step.is_finite() && step > 0.0 {
        step
    } else {
        a
    };
    if a >= 1e4 {
        let (div, suffix) = if a >= 1e12 {
            (1e12, "T")
        } else if a >= 1e9 {
            (1e9, "G")
        } else if a >= 1e6 {
            (1e6, "M")
        } else {
            (1e3, "k")
        };
        let s = step / div;
        let dec = decimals_for(s).min(3);
        return format!("{}{suffix}", trim(format!("{:.dec$}", v / div)));
    }
    if a < 1e-3 {
        let e = a.log10().floor() as i32;
        let m = v / 10f64.powi(e);
        let ms = trim(format!("{m:.1}"));
        return format!("{ms}e{e}");
    }
    let dec = decimals_for(step).min(6);
    trim(format!("{v:.dec$}"))
}

/// A number written briefly (about 4 significant digits), for labels that are not ticks.
pub fn format_value(v: f64) -> String {
    if !v.is_finite() {
        return "–".into();
    }
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-3..1e6).contains(&a) {
        let e = a.log10().floor() as i32;
        let m = v / 10f64.powi(e);
        return format!("{}e{e}", trim(format!("{m:.2}")));
    }
    let decimals = (3 - a.log10().floor() as i32).clamp(0, 6) as usize;
    trim(format!("{v:.decimals$}"))
}

fn decimals_for(step: f64) -> usize {
    if step >= 1.0 || step <= 0.0 {
        return 0;
    }
    let d = (-step.log10()).ceil();
    // 0.25 needs 2 decimals, 0.5 needs 1
    let base = d.max(0.0) as usize;
    let scaled = step * 10f64.powi(base as i32);
    if (scaled - scaled.round()).abs() > 1e-6 {
        base + 1
    } else {
        base
    }
}

fn trim(s: String) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_ticks_are_nice() {
        let s = Scale::linear(0.0, 97.0, 0.0, 100.0);
        let t: Vec<String> = s.ticks(5).into_iter().map(|t| t.label).collect();
        assert_eq!(t, vec!["0", "20", "40", "60", "80"]);
        let s = Scale::linear(-0.3, 0.3, 0.0, 100.0);
        let t: Vec<String> = s.ticks(6).into_iter().map(|t| t.label).collect();
        assert_eq!(t, vec!["-0.3", "-0.2", "-0.1", "0", "0.1", "0.2", "0.3"]);
        let n = Scale::linear(0.0, 97.0, 0.0, 100.0).nice(5);
        assert!((n.hi - 100.0).abs() < 1e-9);
        // reversed (NMR) keeps its direction
        let r = Scale::linear(10.0, 0.0, 0.0, 100.0);
        assert_eq!(r.px(10.0), Some(0.0));
        assert!((r.value(100.0)).abs() < 1e-9);
    }

    #[test]
    fn labels_are_short() {
        assert_eq!(format_tick(20000.0, 5000.0), "20k");
        assert_eq!(format_tick(1_500_000.0, 500_000.0), "1.5M");
        assert_eq!(format_tick(0.25, 0.05), "0.25");
        assert_eq!(format_tick(0.00002, 0.00001), "2e-5");
        assert_eq!(format_tick(1234.0, 1.0), "1234");
        assert_eq!(format_value(12.3456), "12.35");
        assert_eq!(format_value(2e7), "2e7");
    }

    #[test]
    fn log_ticks_are_decades() {
        let s = Scale::log(1.0, 1e6, 0.0, 600.0);
        let majors: Vec<String> = s
            .ticks(7)
            .into_iter()
            .filter(|t| t.major)
            .map(|t| t.label)
            .collect();
        assert_eq!(majors, vec!["1", "10", "100", "1k", "10k", "100k", "1M"]);
        assert_eq!(s.px(0.0), None);
        assert!((s.px(1000.0).unwrap() - 300.0).abs() < 1e-3);
        let narrow = Scale::log(1.0, 20.0, 0.0, 100.0);
        assert!(narrow.ticks(5).iter().any(|t| t.label == "5"));
    }
}
