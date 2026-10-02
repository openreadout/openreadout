//! Curve pictures: the standard curve (standards, fitted line, back-calculated unknowns) or the
//! dose-response curves (one colour per compound), drawn on the preview crate's canvas and
//! encoded as PNG. Concentration axes are logarithmic for logistic models; points at
//! concentration 0 cannot be placed on a log axis and are left out (noted).

use openreadout_core::Result;
use openreadout_preview::canvas::{Canvas, text_width};
use openreadout_preview::color::{Rgb, TRACE_PALETTE};

use crate::output::{AssayOutput, FitReport};

const W: i64 = 960;
const H: i64 = 600;
const LEFT: i64 = 90;
const RIGHT: i64 = 30;
const TOP: i64 = 50;
const BOTTOM: i64 = 70;
const INK: Rgb = [40, 40, 40];
const GRID: Rgb = [225, 225, 225];
const WHITE: Rgb = [255, 255, 255];

struct Axis {
    lo: f64,
    hi: f64,
    log: bool,
}

impl Axis {
    fn t(&self, v: f64) -> Option<f64> {
        let v = if self.log {
            if v <= 0.0 {
                return None;
            }
            v.log10()
        } else {
            v
        };
        let span = self.hi - self.lo;
        Some(if span > 0.0 {
            (v - self.lo) / span
        } else {
            0.5
        })
    }
}

fn axis(values: &[f64], log: bool) -> Axis {
    let vals: Vec<f64> = values
        .iter()
        .copied()
        .filter(|v| v.is_finite() && (!log || *v > 0.0))
        .map(|v| if log { v.log10() } else { v })
        .collect();
    let lo = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() {
        return Axis {
            lo: 0.0,
            hi: 1.0,
            log,
        };
    }
    let pad = if hi > lo {
        0.05 * (hi - lo)
    } else {
        0.5_f64.max(lo.abs() * 0.1)
    };
    Axis {
        lo: lo - pad,
        hi: hi + pad,
        log,
    }
}

fn short(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-3..1e5).contains(&a) {
        format!("{v:.0e}")
    } else if a >= 100.0 {
        format!("{v:.0}")
    } else if a >= 1.0 {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn ticks(ax: &Axis) -> Vec<(f64, String)> {
    if ax.log {
        let (a, b) = (ax.lo.ceil() as i32, ax.hi.floor() as i32);
        return (a..=b)
            .map(|e| (f64::from(e), short(10f64.powi(e))))
            .collect();
    }
    let span = ax.hi - ax.lo;
    if span <= 0.0 {
        return vec![(ax.lo, short(ax.lo))];
    }
    let raw = span / 6.0;
    let mag = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * mag)
        .find(|s| span / s <= 7.0)
        .unwrap_or(10.0 * mag);
    let mut v = (ax.lo / step).ceil() * step;
    let mut out = Vec::new();
    while v <= ax.hi + 1e-12 * span && out.len() < 20 {
        out.push((v, short(if v.abs() < step * 1e-9 { 0.0 } else { v })));
        v += step;
    }
    out
}

struct Frame {
    x: Axis,
    y: Axis,
}

impl Frame {
    fn px(&self, x: f64, y: f64) -> Option<(i64, i64)> {
        let (tx, ty) = (self.x.t(x)?, self.y.t(y)?);
        let w = (W - LEFT - RIGHT) as f64;
        let h = (H - TOP - BOTTOM) as f64;
        Some((
            LEFT + (tx * w).round() as i64,
            H - BOTTOM - (ty * h).round() as i64,
        ))
    }
}

fn draw_frame(c: &mut Canvas, f: &Frame, title: &str, xl: &str, yl: &str) {
    let (x0, x1, y0, y1) = (LEFT, W - RIGHT, TOP, H - BOTTOM);
    let w = (x1 - x0) as f64;
    let h = (y1 - y0) as f64;
    for (v, lab) in ticks(&f.x) {
        let t = if f.x.hi > f.x.lo {
            (v - f.x.lo) / (f.x.hi - f.x.lo)
        } else {
            0.5
        };
        let x = x0 + (t * w).round() as i64;
        c.line(x, y0, x, y1, GRID);
        c.line(x, y1, x, y1 + 5, INK);
        c.text(x - text_width(&lab, 2) / 2, y1 + 10, 2, INK, &lab);
    }
    for (v, lab) in ticks(&f.y) {
        let t = if f.y.hi > f.y.lo {
            (v - f.y.lo) / (f.y.hi - f.y.lo)
        } else {
            0.5
        };
        let y = y1 - (t * h).round() as i64;
        c.line(x0, y, x1, y, GRID);
        c.line(x0 - 5, y, x0, y, INK);
        c.text(x0 - 10 - text_width(&lab, 2), y - 5, 2, INK, &lab);
    }
    c.line(x0, y1, x1, y1, INK);
    c.line(x0, y0, x0, y1, INK);
    c.text(x0, 15, 3, INK, title);
    c.text(
        i64::midpoint(x0, x1) - text_width(xl, 2) / 2,
        y1 + 35,
        2,
        INK,
        xl,
    );
    c.text(8, y0 - 25, 2, INK, yl);
}

fn marker(c: &mut Canvas, x: i64, y: i64, col: Rgb, filled: bool) {
    if filled {
        c.fill_rect(x - 3, y - 3, 7, 7, col);
    } else {
        for (a, b, d, e) in [
            (-3, -3, 3, -3),
            (3, -3, 3, 3),
            (3, 3, -3, 3),
            (-3, 3, -3, -3),
        ] {
            c.line(x + a, y + b, x + d, y + e, col);
        }
    }
}

fn curve_line(c: &mut Canvas, f: &Frame, eval: &dyn Fn(f64) -> f64, col: Rgb) {
    let mut last: Option<(i64, i64)> = None;
    let steps = 400;
    for i in 0..=steps {
        let t = f64::from(i) / f64::from(steps);
        let xv = f.x.lo + t * (f.x.hi - f.x.lo);
        let x = if f.x.log { 10f64.powf(xv) } else { xv };
        let y = eval(x);
        let p = if y.is_finite()
            && y >= f.y.lo - (f.y.hi - f.y.lo)
            && y <= f.y.hi + (f.y.hi - f.y.lo)
        {
            f.px(x, y)
        } else {
            None
        };
        if let (Some(a), Some(b)) = (last, p) {
            c.line(a.0, a.1, b.0, b.1, col);
            c.line(a.0, a.1 + 1, b.0, b.1 + 1, col);
        }
        last = p;
    }
}

fn eval_fit(fit: &FitReport, x: f64) -> f64 {
    let p: Vec<f64> = fit.parameters.iter().map(|p| p.value).collect();
    match fit.model.as_str() {
        "linear" => p[0] * x + p[1],
        "4pl" => p[3] + (p[0] - p[3]) / (1.0 + (x / p[2]).powf(p[1])),
        _ => p[3] + (p[0] - p[3]) / (1.0 + (x / p[2]).powf(p[1])).powf(p[4]),
    }
}

/// Render the curve of a `curve` or `dose-response` result as PNG bytes. `None` when the
/// result has no curve.
pub fn render(out: &AssayOutput) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut notes = Vec::new();
    let mut c = Canvas::new(W as u32, H as u32, WHITE);
    let unit = out
        .curve
        .as_ref()
        .and_then(|cv| cv.concentration_unit.clone())
        .map(|u| format!(" ({u})"))
        .unwrap_or_default();
    let yl = format!(
        "{}{}",
        out.read.label,
        out.read
            .unit
            .as_ref()
            .map(|u| format!(" ({u})"))
            .unwrap_or_default()
    );
    if let Some(cv) = &out.curve {
        let fit = &cv.fit;
        let log = fit.model != "linear";
        let mut xs: Vec<f64> = fit.points.iter().map(|p| p.x).collect();
        let mut ys: Vec<f64> = fit.points.iter().map(|p| p.y).collect();
        let unknowns: Vec<(f64, f64)> = out
            .wells
            .iter()
            .filter(|w| w.role != "standard" && w.role != "blank" && w.role != "empty")
            .filter_map(|w| Some((w.back_calculated?, w.value?)))
            .filter(|(x, _)| x.is_finite() && (!log || *x > 0.0))
            .collect();
        xs.extend(unknowns.iter().map(|u| u.0));
        ys.extend(unknowns.iter().map(|u| u.1));
        let frame = Frame {
            x: axis(&xs, log),
            y: axis(&ys, false),
        };
        let r2 = format!("r2 {:.4}", fit.r_squared);
        draw_frame(
            &mut c,
            &frame,
            &format!("standard curve {} {}", fit.model, r2),
            &format!("concentration{unit}"),
            &yl,
        );
        curve_line(&mut c, &frame, &|x| eval_fit(fit, x), TRACE_PALETTE[0]);
        let mut hidden = 0;
        for p in &fit.points {
            match frame.px(p.x, p.y) {
                Some((x, y)) => marker(&mut c, x, y, TRACE_PALETTE[0], true),
                None => hidden += 1,
            }
        }
        for (xv, yv) in &unknowns {
            if let Some((x, y)) = frame.px(*xv, *yv) {
                marker(&mut c, x, y, TRACE_PALETTE[1], false);
            }
        }
        if hidden > 0 {
            notes.push(format!(
                "{hidden} standard point(s) at concentration 0 are not drawn on the log axis"
            ));
        }
        c.text(
            W - RIGHT - 300,
            TOP + 5,
            2,
            TRACE_PALETTE[0],
            "SQUARES STANDARDS",
        );
        c.text(
            W - RIGHT - 300,
            TOP + 20,
            2,
            TRACE_PALETTE[1],
            "OUTLINES UNKNOWNS",
        );
    } else if !out.compounds.is_empty() {
        let fits: Vec<&crate::output::CompoundRow> =
            out.compounds.iter().filter(|c| c.fit.is_some()).collect();
        if fits.is_empty() {
            return Ok(None);
        }
        let xs: Vec<f64> = fits
            .iter()
            .flat_map(|c| c.fit.iter().flat_map(|f| f.points.iter().map(|p| p.x)))
            .collect();
        let ys: Vec<f64> = fits
            .iter()
            .flat_map(|c| c.fit.iter().flat_map(|f| f.points.iter().map(|p| p.y)))
            .collect();
        let frame = Frame {
            x: axis(&xs, true),
            y: axis(&ys, false),
        };
        let ylab = if out.wells.iter().any(|w| w.percent_effect.is_some()) {
            "percent effect".to_string()
        } else {
            yl
        };
        draw_frame(&mut c, &frame, "dose response", "concentration", &ylab);
        for (i, cp) in fits.iter().enumerate() {
            let col = TRACE_PALETTE[i % TRACE_PALETTE.len()];
            let Some(fit) = &cp.fit else { continue };
            curve_line(&mut c, &frame, &|x| eval_fit(fit, x), col);
            for p in &fit.points {
                if let Some((x, y)) = frame.px(p.x, p.y) {
                    marker(&mut c, x, y, col, true);
                }
            }
            let label = match cp.ec50 {
                Some(e) => format!("{} {} {}", cp.compound, cp.kind, short(e)),
                None => cp.compound.clone(),
            };
            c.text(W - RIGHT - 330, TOP + 5 + 15 * i as i64, 2, col, &label);
        }
        let zeros = xs.iter().filter(|x| **x <= 0.0).count();
        if zeros > 0 {
            notes.push(format!(
                "{zeros} point(s) at concentration 0 are not drawn on the log axis"
            ));
        }
    } else {
        return Ok(None);
    }
    let png = openreadout_preview::encode(&c, openreadout_preview::Encoding::Png, 90)?;
    Ok(Some((png, notes)))
}
