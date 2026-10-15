//! Drawing one series into a plot panel.

use std::sync::Arc;

use openreadout_figure::{
    Anchor, Baseline, Color, Envelope, Figure, Marker, Panel, Path, Raster, Rect, Shape, Stroke,
    TextStyle, Theme, format_value,
};

use crate::model::{Geom, Series, Weight};
use crate::spec::MarkKind;

/// Sizes that follow the panel: thinner lines in small multiples.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ink {
    pub line: f32,
    pub dot: f32,
    pub small: bool,
    /// Filled bands and areas in the panel (fills get lighter the more overlap).
    pub fills: usize,
}

impl Ink {
    pub fn for_panel(theme: &Theme, rect: Rect) -> Ink {
        let short = rect.w.min(rect.h);
        let small = short < 140.0;
        Ink {
            line: if small {
                (short / 70.0).clamp(1.0, theme.line_width)
            } else {
                theme.line_width
            },
            dot: if small {
                (short / 30.0).clamp(2.5, 5.0)
            } else {
                theme.font_size * 0.42
            },
            small,
            fills: 1,
        }
    }
}

fn color_of(s: &Series, theme: &Theme) -> Color {
    s.color.unwrap_or(theme.ink)
}

/// The stroke of a line series.
fn line_stroke(s: &Series, theme: &Theme, ink: Ink, n_many: usize) -> Stroke {
    let c = color_of(s, theme);
    match s.weight {
        Weight::Main => Stroke::new(c, ink.line).round(),
        Weight::Fit => Stroke::new(c, ink.line * 0.9).round(),
        Weight::Context => Stroke::new(c, ink.line * 0.85).round(),
        Weight::Many => {
            // many overlapping lines: thinner and see-through, more so the more there are
            let a = if ink.small {
                0.95
            } else if n_many > 40 {
                0.45
            } else {
                0.6
            };
            Stroke::new(c.alpha(a), ink.line * if ink.small { 1.0 } else { 0.8 }).round()
        }
    }
}

/// Pixel points of a line, reduced to a min/max envelope when denser than the panel.
fn line_px(p: &Panel, pts: &[(f64, f64)]) -> Vec<(f32, f32)> {
    let cols = (p.rect.w.max(1.0) * 1.5) as usize;
    let monotone =
        pts.windows(2).all(|w| w[1].0 >= w[0].0) || pts.windows(2).all(|w| w[1].0 <= w[0].0);
    if pts.len() > cols * 3 && monotone {
        let d = p.x_domain();
        let (x0, x1) = (
            pts.first().map_or(d.lo, |q| q.0),
            pts.last().map_or(d.hi, |q| q.0),
        );
        let (x0, x1) = (x0.min(x1), x0.max(x1));
        let mut env = Envelope::new(cols, x0, x1);
        for &(x, y) in pts {
            env.push(x, y);
        }
        return env.points(p);
    }
    pts.iter()
        .map(|&(x, y)| p.pt(x, y).unwrap_or((f32::NAN, f32::NAN)))
        .collect()
}

/// Draw `s` (everything but events, which `events_layer` draws).
pub(crate) fn draw_series(
    fig: &mut Figure,
    p: &Panel,
    theme: &Theme,
    s: &Series,
    ink: Ink,
    n_many: usize,
    dodge: Option<(usize, usize)>,
    cats: &[String],
) {
    let c = color_of(s, theme);
    let clip = Some(p.rect);
    match (&s.geom, s.mark) {
        (Geom::Line(pts), MarkKind::Sticks) => {
            let base =
                p.y.px(0.0_f64.max(p.y_domain().lo))
                    .unwrap_or(p.rect.bottom());
            let mut path = Path::new();
            for &(x, y) in pts {
                if let Some((px, py)) = p.pt(x, y) {
                    path.move_to(px, base);
                    path.line_to(px, py);
                }
            }
            fig.shape(
                path,
                None,
                Some(Stroke::new(c, (ink.line * 0.7).max(1.0))),
                clip,
            );
        }
        (Geom::Line(pts), MarkKind::Area) => {
            let base = 0.0_f64.max(p.y_domain().lo);
            p.fill_to(
                fig,
                pts,
                base,
                c.alpha(fill_alpha(ink.fills) * if s.weight == Weight::Main { 1.0 } else { 0.6 }),
            );
            p.line_px(fig, line_px(p, pts), Stroke::new(c, ink.line * 0.9));
        }
        (Geom::Line(pts), MarkKind::Points | MarkKind::LinePoints) => {
            if s.mark == MarkKind::LinePoints {
                p.line_px(
                    fig,
                    line_px(p, pts),
                    Stroke::new(c.alpha(0.7), ink.line * 0.8),
                );
            }
            p.markers(fig, pts.iter().copied(), &Marker::dot(c, ink.dot));
        }
        (Geom::Line(pts), _) => {
            let mut st = line_stroke(s, theme, ink, n_many);
            if pts.len() as f32 > p.rect.w * 3.0 {
                // a dense signal drawn as its envelope: a finer pen keeps its structure
                st.width *= 0.65;
            }
            p.line_px(fig, line_px(p, pts), st);
        }
        (Geom::Band(b), _) => {
            let tri: Vec<(f64, f64, f64)> = b.iter().map(|&(x, _, lo, hi)| (x, lo, hi)).collect();
            p.area(fig, &tri, c.alpha(fill_alpha(ink.fills)));
            let mid: Vec<(f64, f64)> = b.iter().map(|&(x, m, _, _)| (x, m)).collect();
            p.line_px(fig, line_px(p, &mid), line_stroke(s, theme, ink, n_many));
        }
        (Geom::Points { pts, err }, MarkKind::Bars) => {
            let mut xs: Vec<f64> = pts.iter().map(|q| q.0).collect();
            xs.sort_by(f64::total_cmp);
            let gap = xs
                .windows(2)
                .map(|w| w[1] - w[0])
                .filter(|d| *d > 0.0)
                .fold(f64::INFINITY, f64::min);
            let w = if gap.is_finite() { gap * 0.68 } else { 1.0 };
            p.bars(fig, pts, w, 0.0, c);
            error_bars(fig, p, theme, pts, err, ink);
            if !err.iter().any(|e| e.is_finite() && *e > 0.0) {
                bar_values(fig, p, theme, pts, ink);
            }
        }
        (Geom::Points { pts, err }, m) => {
            if m == MarkKind::LinePoints || m == MarkKind::Line {
                p.line(
                    fig,
                    pts.iter().copied(),
                    Stroke::new(c.alpha(0.75), ink.line * 0.85).round(),
                );
            }
            error_bars(fig, p, theme, pts, err, ink);
            if m != MarkKind::Line {
                // the many (samples, replicates) as rings, so they read on a line
                let mk = if s.weight == Weight::Many {
                    Marker {
                        shape: Shape::Circle,
                        size: ink.dot * 0.9,
                        fill: Some(theme.paper),
                        stroke: Some(Stroke::new(c, (ink.dot * 0.26).max(1.2))),
                    }
                } else {
                    Marker {
                        shape: Shape::Circle,
                        size: ink.dot,
                        fill: Some(c),
                        stroke: Some(Stroke::new(theme.paper, (ink.dot * 0.18).max(1.0))),
                    }
                };
                p.markers(fig, pts.iter().copied(), &mk);
            }
        }
        (
            Geom::Bars {
                cats: bc,
                vals,
                err,
            },
            _,
        ) => {
            let (k, m) = dodge.unwrap_or((0, 1));
            let per_unit = (p.x.px(1.0).unwrap_or(1.0) - p.x.px(0.0).unwrap_or(0.0))
                .abs()
                .max(1e-3);
            // bars of a group sit side by side; no bar is wider than about five letters
            let slot = (0.72 / m as f64).min(f64::from(theme.font_size * 6.2 / per_unit));
            let mut pts = Vec::new();
            let mut errs = Vec::new();
            for (i, name) in bc.iter().enumerate() {
                let Some(ci) = cats.iter().position(|x| x == name) else {
                    continue;
                };
                let x = ci as f64 - slot * m as f64 / 2.0 + slot * (k as f64 + 0.5);
                let v = vals.get(i).copied().unwrap_or(f64::NAN);
                if v.is_finite() {
                    pts.push((x, v));
                    errs.push(err.get(i).copied().unwrap_or(f64::NAN));
                }
            }
            p.bars(fig, &pts, slot * 0.86, 0.0, c);
            error_bars(fig, p, theme, &pts, &errs, ink);
            if m == 1 && !errs.iter().any(|e| e.is_finite() && *e > 0.0) {
                bar_values(fig, p, theme, &pts, ink);
            }
        }
        (Geom::Events { .. } | Geom::Hist(_), _) => {}
    }
}

/// Opacity of a fill: lighter when several overlap.
fn fill_alpha(n: usize) -> f32 {
    match n {
        0 | 1 => 0.17,
        2 => 0.12,
        3 | 4 => 0.09,
        _ => 0.06,
    }
}

fn error_bars(
    fig: &mut Figure,
    p: &Panel,
    theme: &Theme,
    pts: &[(f64, f64)],
    err: &[f64],
    ink: Ink,
) {
    let st = Stroke::new(theme.muted, theme.hairline().max(ink.line * 0.6));
    let cap = ink.dot * 0.6;
    for (i, &(x, y)) in pts.iter().enumerate() {
        let Some(&e) = err.get(i) else { continue };
        if !e.is_finite() || e <= 0.0 {
            continue;
        }
        if let (Some(px), Some(a), Some(b)) = (p.x.px(x), p.y.px(y - e), p.y.px(y + e)) {
            fig.shape(
                Path::segment(px, a, px, b),
                None,
                Some(st.clone()),
                Some(p.rect),
            );
            fig.line(px - cap, a, px + cap, a, st.clone());
            fig.line(px - cap, b, px + cap, b, st.clone());
        }
    }
}

/// Value labels over a few bars.
fn bar_values(fig: &mut Figure, p: &Panel, theme: &Theme, pts: &[(f64, f64)], ink: Ink) {
    if pts.len() > 18 || ink.small {
        return;
    }
    let fs = theme.small_size();
    // values only where they fit side by side over their bars
    let mut xs: Vec<f32> = pts.iter().filter_map(|&(x, _)| p.x.px(x)).collect();
    xs.sort_by(f32::total_cmp);
    let room = xs
        .windows(2)
        .map(|w| w[1] - w[0])
        .fold(f32::INFINITY, f32::min);
    let widest = pts
        .iter()
        .map(|&(_, y)| format_value(y).chars().count())
        .max()
        .unwrap_or(0) as f32;
    if room < (widest + 1.0) * fs * 0.6 {
        return;
    }
    let st = TextStyle::new(fs, theme.muted).align(Anchor::Middle, Baseline::Alphabetic);
    for &(x, y) in pts {
        if let Some((px, py)) = p.pt(x, y.max(0.0)) {
            let t = format_value(y);
            if p.rect.contains(px, py) {
                fig.text(px, py - fs * 0.45, &t, &st);
            }
        }
    }
}

/// A 3 × 3 box blur.
fn blur(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; v.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            let mut n = 0.0;
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    if xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h {
                        let wgt = if dx == 0 && dy == 0 { 2.0 } else { 1.0 };
                        s += v[yy as usize * w + xx as usize] * wgt;
                        n += wgt;
                    }
                }
            }
            out[y * w + x] = s / n * 2.0;
        }
    }
    out
}

/// All event series of a panel as one picture: a density of every series, in its colour,
/// over the panel background.
pub(crate) fn events_layer(
    fig: &mut Figure,
    p: &Panel,
    theme: &Theme,
    series: &[&Series],
    scale: f32,
) {
    let w = ((p.rect.w * scale).round() as u32).max(1);
    let h = ((p.rect.h * scale).round() as u32).max(1);
    let bg = theme.paper;
    let mut r = Raster::new(w, h, bg.to_rgb());
    let mut acc: Vec<[f32; 3]> =
        vec![[f32::from(bg.r), f32::from(bg.g), f32::from(bg.b)]; (w * h) as usize];
    for s in series {
        let Geom::Events { x, y } = &s.geom else {
            continue;
        };
        let mut counts = vec![0f32; (w * h) as usize];
        for (&a, &b) in x.iter().zip(y) {
            let (Some(px), Some(py)) = (p.x.px(a), p.y.px(b)) else {
                continue;
            };
            let cx = ((px - p.rect.x) * scale).floor();
            let cy = ((py - p.rect.y) * scale).floor();
            if cx < 0.0 || cy < 0.0 || cx >= w as f32 || cy >= h as f32 {
                continue;
            }
            let k = cy as usize * w as usize + cx as usize;
            counts[k] += 1.0;
        }
        let counts = blur(&counts, w as usize, h as usize);
        let max = counts.iter().copied().fold(0.0f32, f32::max);
        if max <= 0.0 {
            continue;
        }
        let col = s.color.unwrap_or(theme.ink);
        let target = [f32::from(col.r), f32::from(col.g), f32::from(col.b)];
        let context = s.weight == Weight::Context;
        let lmax = (max + 1.0).ln();
        for (k, &n) in counts.iter().enumerate() {
            if n < 0.04 {
                continue;
            }
            // log density: single events stay visible, dense cores saturate
            let t = ((n + 1.0).ln() / lmax).clamp(0.0, 1.0);
            let a = (if context {
                0.22 + 0.5 * t
            } else {
                0.3 + 0.7 * t
            }) * (n / 0.35).min(1.0);
            let px = &mut acc[k];
            for i in 0..3 {
                px[i] = px[i] * (1.0 - a) + target[i] * a;
            }
        }
    }
    for (k, px) in acc.iter().enumerate() {
        let x = (k % w as usize) as i64;
        let y = (k / w as usize) as i64;
        r.set(x, y, [px[0] as u8, px[1] as u8, px[2] as u8]);
    }
    fig.image(p.rect, Arc::new(r), true);
}
