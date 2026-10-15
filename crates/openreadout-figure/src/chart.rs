//! Plot panels: a rectangle with an x and a y scale, the marks drawn in data units (lines,
//! areas, bands, rules, markers, bars, cells, text, arrows) and quiet axes around it.
//!
//! ```
//! use openreadout_figure::{Domain, Figure, Panel, AxesSpec, Theme, Stroke};
//!
//! let theme = Theme::light().for_size(640.0);
//! let mut fig = Figure::new(640.0, 400.0, Some(theme.paper));
//! let spec = AxesSpec { x_label: Some("time (h)".into()), y_label: Some("OD600".into()), ..AxesSpec::default() };
//! let panel = Panel::fit(fig.rect(), Domain::linear(0.0, 24.0), Domain::linear(0.0, 1.2).nice(5), &theme, &spec);
//! panel.axes(&mut fig, &theme, &spec);
//! panel.line(&mut fig, (0..=24).map(|h| (f64::from(h), 1.2 * (1.0 - (-f64::from(h) / 6.0).exp()))), Stroke::new(theme.ink, theme.line_width));
//! panel.band_x(&mut fig, 7.4, 8.6, theme.band);
//! let png = fig.to_png(1.0).unwrap();
//! assert_eq!(&png[1..4], b"PNG");
//! ```

use crate::color::{Color, Colormap};
use crate::font::{self, Face};
use crate::geom::Rect;
use crate::path::Path;
use crate::scale::{Scale, ScaleKind, Tick, format_tick};
use crate::scene::{Anchor, Baseline, Figure, Stroke, TextStyle};
use crate::theme::Theme;

/// The data range of one axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Domain {
    /// Low end.
    pub lo: f64,
    /// High end.
    pub hi: f64,
    /// Linear or log.
    pub kind: ScaleKind,
    /// Run high to low (left to right, or bottom to top).
    pub reversed: bool,
}

impl Domain {
    /// A linear range. Equal or non-finite ends are widened so something can be drawn.
    pub fn linear(lo: f64, hi: f64) -> Domain {
        let (lo, hi) = sane(lo, hi);
        Domain {
            lo,
            hi,
            kind: ScaleKind::Linear,
            reversed: false,
        }
    }

    /// A log range (ends ≤ 0 fall back to 1–10).
    pub fn log(lo: f64, hi: f64) -> Domain {
        let ok = |v: f64| v.is_finite() && v > 0.0;
        let (lo, hi) = match (ok(lo), ok(hi)) {
            (true, true) if hi > lo => (lo, hi),
            (true, true) => (lo / 2.0, lo * 2.0),
            (true, false) => (lo, lo * 10.0),
            (false, true) => (hi / 10.0, hi),
            (false, false) => (1.0, 10.0),
        };
        Domain {
            lo,
            hi,
            kind: ScaleKind::Log,
            reversed: false,
        }
    }

    /// The range of finite `values` (log: of the positive ones), padded by `pad` of the span on
    /// each side (linear only).
    pub fn of(values: impl IntoIterator<Item = f64>, pad: f64, kind: ScaleKind) -> Domain {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for v in values {
            if v.is_finite() && (kind != ScaleKind::Log || v > 0.0) {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        match kind {
            ScaleKind::Linear => {
                let (lo, hi) = sane(lo, hi);
                let p = (hi - lo) * pad;
                Domain::linear(lo - p, hi + p)
            }
            ScaleKind::Log => Domain::log(lo, hi),
            ScaleKind::Asinh(c) => Domain {
                kind: ScaleKind::Asinh(c),
                ..Domain::linear(lo, hi)
            },
        }
    }

    /// An asinh range (`asinh(v / cofactor)`), for data spanning decades on both sides of zero.
    pub fn asinh(lo: f64, hi: f64, cofactor: f64) -> Domain {
        Domain {
            kind: ScaleKind::Asinh(cofactor),
            ..Domain::linear(lo, hi)
        }
    }

    /// The same range, reversed.
    pub fn reversed(mut self) -> Domain {
        self.reversed = !self.reversed;
        self
    }

    /// Include `v` (e.g. zero for bars).
    pub fn include(mut self, v: f64) -> Domain {
        if v.is_finite() && (self.kind != ScaleKind::Log || v > 0.0) {
            self.lo = self.lo.min(v);
            self.hi = self.hi.max(v);
        }
        self
    }

    /// Extend to nice tick values for about `target` ticks.
    pub fn nice(self, target: usize) -> Domain {
        let s = self.scale(0.0, 1.0).nice(target);
        let (lo, hi) = s.domain();
        Domain { lo, hi, ..self }
    }

    /// The scale from this range onto `px0`..`px1` (swapped when reversed).
    pub fn scale(&self, px0: f32, px1: f32) -> Scale {
        let (a, b) = if self.reversed {
            (px1, px0)
        } else {
            (px0, px1)
        };
        match self.kind {
            ScaleKind::Linear => Scale::linear(self.lo, self.hi, a, b),
            ScaleKind::Log => Scale::log(self.lo, self.hi, a, b),
            ScaleKind::Asinh(c) => Scale {
                kind: ScaleKind::Asinh(c),
                ..Scale::linear(self.lo, self.hi, a, b)
            },
        }
    }
}

fn sane(lo: f64, hi: f64) -> (f64, f64) {
    match (lo.is_finite(), hi.is_finite()) {
        (true, true) if hi > lo => (lo, hi),
        (true, true) if hi < lo => (hi, lo),
        (true, true) => {
            let d = lo.abs().max(1.0) * 0.05;
            (lo - d, hi + d)
        }
        (true, false) => (lo, lo + 1.0),
        (false, true) => (hi - 1.0, hi),
        (false, false) => (0.0, 1.0),
    }
}

/// Grid lines at the ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GridLines {
    /// None.
    None,
    /// Horizontal lines at the y ticks (default).
    #[default]
    Y,
    /// Vertical lines at the x ticks.
    X,
    /// Both.
    Both,
}

/// What the axes around a panel show.
#[derive(Debug, Clone, PartialEq)]
pub struct AxesSpec {
    /// Title of the x axis, with its unit (`time (s)`), drawn under the tick labels at the right.
    pub x_label: Option<String>,
    /// Title of the y axis, with its unit, drawn above the axis at the left.
    pub y_label: Option<String>,
    /// About this many x ticks.
    pub x_ticks: usize,
    /// About this many y ticks.
    pub y_ticks: usize,
    /// Show the x axis (ticks and labels).
    pub show_x: bool,
    /// Show the y axis (ticks and labels).
    pub show_y: bool,
    /// Grid lines.
    pub grid: GridLines,
    /// Extra room on the right, e.g. for direct labels at the ends of lines.
    pub right: f32,
    /// Extra room on top, e.g. for a title.
    pub top: f32,
    /// Fill the panel with the theme's panel colour.
    pub fill: bool,
}

impl Default for AxesSpec {
    fn default() -> Self {
        AxesSpec {
            x_label: None,
            y_label: None,
            x_ticks: 6,
            y_ticks: 5,
            show_x: true,
            show_y: true,
            grid: GridLines::Y,
            right: 0.0,
            top: 0.0,
            fill: false,
        }
    }
}

/// A plot panel: where the data goes, and its scales.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Panel {
    /// The data rectangle.
    pub rect: Rect,
    /// Horizontal scale.
    pub x: Scale,
    /// Vertical scale (low values at the bottom unless the domain is reversed).
    pub y: Scale,
}

/// A marker shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    /// Filled or ringed circle (default).
    #[default]
    Circle,
    /// Square.
    Square,
    /// Diamond.
    Diamond,
    /// Upward triangle.
    Triangle,
    /// ×.
    Cross,
    /// +.
    Plus,
}

/// How markers are drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    /// Shape.
    pub shape: Shape,
    /// Diameter in pixels.
    pub size: f32,
    /// Fill (ignored for × and +).
    pub fill: Option<Color>,
    /// Outline (× and + use only this).
    pub stroke: Option<Stroke>,
}

impl Marker {
    /// A filled circle of diameter `size`.
    pub fn dot(color: Color, size: f32) -> Marker {
        Marker {
            shape: Shape::Circle,
            size,
            fill: Some(color),
            stroke: None,
        }
    }
}

/// The outline of a marker centred on `(x, y)`.
pub fn marker_path(shape: Shape, x: f32, y: f32, size: f32) -> Path {
    let r = size / 2.0;
    match shape {
        Shape::Circle => Path::circle(x, y, r),
        Shape::Square => Path::rect(x - r * 0.9, y - r * 0.9, r * 1.8, r * 1.8),
        Shape::Diamond => {
            Path::polygon([(x, y - r * 1.2), (x + r, y), (x, y + r * 1.2), (x - r, y)])
        }
        Shape::Triangle => Path::polygon([
            (x, y - r * 1.15),
            (x + r * 1.05, y + r * 0.7),
            (x - r * 1.05, y + r * 0.7),
        ]),
        Shape::Cross => {
            let mut p = Path::segment(x - r, y - r, x + r, y + r);
            p.extend(&Path::segment(x - r, y + r, x + r, y - r));
            p
        }
        Shape::Plus => {
            let mut p = Path::segment(x - r, y, x + r, y);
            p.extend(&Path::segment(x, y - r, x, y + r));
            p
        }
    }
}

/// Draw a marker at pixel `(x, y)`.
pub fn draw_marker(fig: &mut Figure, m: &Marker, x: f32, y: f32) {
    let p = marker_path(m.shape, x, y, m.size);
    match m.shape {
        Shape::Cross | Shape::Plus => {
            let s = m
                .stroke
                .clone()
                .or_else(|| m.fill.map(|c| Stroke::new(c, (m.size / 5.0).max(1.0))));
            if let Some(s) = s {
                fig.stroke(p, s);
            }
        }
        _ => {
            fig.shape(p, m.fill, m.stroke.clone(), None);
        }
    }
}

/// The shaft and head of an arrow from `(x0, y0)` to `(x1, y1)` drawn `width` wide: stroke the
/// shaft, fill the head. `None` for a zero-length arrow.
pub fn arrow_parts(x0: f32, y0: f32, x1: f32, y1: f32, width: f32) -> Option<(Path, Path)> {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = (dx * dx + dy * dy).sqrt();
    if !len.is_finite() || len < 1e-3 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let head = (width * 4.5).max(7.0).min(len * 0.6);
    let half = head * 0.42;
    let (bx, by) = (x1 - ux * head, y1 - uy * head);
    Some((
        Path::segment(x0, y0, x1 - ux * head * 0.6, y1 - uy * head * 0.6),
        Path::polygon([
            (x1, y1),
            (bx - uy * half, by + ux * half),
            (bx + uy * half, by - ux * half),
        ]),
    ))
}

/// An arrow from `(x0, y0)` to `(x1, y1)` in pixels, with its head at the end.
pub fn arrow_px(fig: &mut Figure, x0: f32, y0: f32, x1: f32, y1: f32, stroke: Stroke) {
    if let Some((shaft, head)) = arrow_parts(x0, y0, x1, y1, stroke.width) {
        let color = stroke.color;
        fig.stroke(shaft, stroke);
        fig.fill(head, color);
    }
}

impl Panel {
    /// A panel over `rect` with these data ranges.
    pub fn new(rect: Rect, x: Domain, y: Domain) -> Panel {
        Panel {
            rect,
            x: x.scale(rect.x, rect.right()),
            y: y.scale(rect.bottom(), rect.y),
        }
    }

    /// The panel inside `outer` that leaves room for the axes `spec` describes (tick labels
    /// measured for these domains).
    pub fn fit(outer: Rect, x: Domain, y: Domain, theme: &Theme, spec: &AxesSpec) -> Panel {
        let fs = theme.font_size;
        let probe = Panel::new(Rect::new(0.0, 0.0, 100.0, 400.0), x, y);
        let tick = (fs * 0.4).round();
        let pad = (fs * 0.5).round();
        let left = if spec.show_y {
            let w = probe
                .y
                .ticks(spec.y_ticks)
                .iter()
                .map(|t| font::text_width(&t.label, Face::Sans, fs))
                .fold(0.0f32, f32::max);
            w + tick + pad
        } else {
            0.0
        };
        let top = spec.top
            + if spec.y_label.is_some() && spec.show_y {
                fs * 1.7
            } else {
                fs * 0.6
            };
        let bottom = if spec.show_x {
            tick + pad * 0.6
                + fs * 1.1
                + if spec.x_label.is_some() {
                    fs * 1.4
                } else {
                    0.0
                }
        } else {
            fs * 0.4
        };
        // the last x tick label may stick out on the right
        let right = spec.right.max(if spec.show_x { fs * 1.2 } else { 0.0 });
        let rect = outer.inset_sides(left + 2.0, top, right, bottom);
        Panel::new(rect, x, y)
    }

    /// Pixel position of a data point (`None` when either value has no position).
    pub fn pt(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        Some((self.x.px(x)?, self.y.px(y)?))
    }

    /// The x domain as a [`Domain`].
    pub fn x_domain(&self) -> Domain {
        domain_of(&self.x, self.rect.x)
    }

    /// The y domain as a [`Domain`].
    pub fn y_domain(&self) -> Domain {
        domain_of(&self.y, self.rect.bottom())
    }

    /// A line through data points, clipped to the panel; non-finite points break it.
    pub fn line(
        &self,
        fig: &mut Figure,
        pts: impl IntoIterator<Item = (f64, f64)>,
        stroke: Stroke,
    ) {
        let p = Path::polyline(
            pts.into_iter()
                .map(|(x, y)| self.pt(x, y).unwrap_or((f32::NAN, f32::NAN))),
        );
        fig.shape(p, None, Some(stroke), Some(self.rect));
    }

    /// A line through pixel points (already mapped, e.g. from an [`Envelope`]), clipped.
    pub fn line_px(
        &self,
        fig: &mut Figure,
        pts: impl IntoIterator<Item = (f32, f32)>,
        stroke: Stroke,
    ) {
        fig.shape(Path::polyline(pts), None, Some(stroke), Some(self.rect));
    }

    /// The area between `lo` and `hi` over x: `(x, lo, hi)` triples (a confidence band, a
    /// min/max range).
    pub fn area(&self, fig: &mut Figure, pts: &[(f64, f64, f64)], fill: Color) {
        let mut runs: Vec<Vec<(f32, f32, f32)>> = vec![Vec::new()];
        for &(x, lo, hi) in pts {
            if let (Some(px), Some(a), Some(b)) = (self.x.px(x), self.y.px(lo), self.y.px(hi)) {
                if let Some(r) = runs.last_mut() {
                    r.push((px, a, b));
                }
            } else {
                runs.push(Vec::new());
            }
        }
        for r in runs.iter().filter(|r| r.len() >= 2) {
            let poly = r
                .iter()
                .map(|p| (p.0, p.2))
                .chain(r.iter().rev().map(|p| (p.0, p.1)));
            fig.shape(Path::polygon(poly), Some(fill), None, Some(self.rect));
        }
    }

    /// The area between a line and `base` (e.g. 0).
    pub fn fill_to(&self, fig: &mut Figure, pts: &[(f64, f64)], base: f64, fill: Color) {
        let tri: Vec<(f64, f64, f64)> = pts
            .iter()
            .map(|&(x, y)| (x, base.min(y), base.max(y)))
            .collect();
        self.area(fig, &tri, fill);
    }

    /// A vertical band over the panel's full height from `x0` to `x1`.
    pub fn band_x(&self, fig: &mut Figure, x0: f64, x1: f64, fill: Color) {
        if let (Some(a), Some(b)) = (self.x.px(x0), self.x.px(x1)) {
            let (a, b) = (a.min(b).max(self.rect.x), a.max(b).min(self.rect.right()));
            if b > a {
                fig.fill_rect(Rect::new(a, self.rect.y, b - a, self.rect.h), fill);
            }
        }
    }

    /// A horizontal band over the panel's full width from `y0` to `y1`.
    pub fn band_y(&self, fig: &mut Figure, y0: f64, y1: f64, fill: Color) {
        if let (Some(a), Some(b)) = (self.y.px(y0), self.y.px(y1)) {
            let (a, b) = (a.min(b).max(self.rect.y), a.max(b).min(self.rect.bottom()));
            if b > a {
                fig.fill_rect(Rect::new(self.rect.x, a, self.rect.w, b - a), fill);
            }
        }
    }

    /// A vertical rule at `x`.
    pub fn rule_x(&self, fig: &mut Figure, x: f64, stroke: Stroke) {
        if let Some(px) = self.x.px(x)
            && px >= self.rect.x - 0.5
            && px <= self.rect.right() + 0.5
        {
            fig.line(px, self.rect.y, px, self.rect.bottom(), stroke);
        }
    }

    /// A horizontal rule at `y`.
    pub fn rule_y(&self, fig: &mut Figure, y: f64, stroke: Stroke) {
        if let Some(py) = self.y.px(y)
            && py >= self.rect.y - 0.5
            && py <= self.rect.bottom() + 0.5
        {
            fig.line(self.rect.x, py, self.rect.right(), py, stroke);
        }
    }

    /// Markers at data points (those outside the panel are left out).
    pub fn markers(&self, fig: &mut Figure, pts: impl IntoIterator<Item = (f64, f64)>, m: &Marker) {
        let r = self.rect.inset(-m.size / 2.0);
        for (x, y) in pts {
            if let Some((px, py)) = self.pt(x, y)
                && r.contains(px, py)
            {
                draw_marker(fig, m, px, py);
            }
        }
    }

    /// A data-space rectangle (`x0..x1` × `y0..y1`), clipped to the panel: histogram bins,
    /// heat-map cells, bars.
    pub fn rect_data(
        &self,
        fig: &mut Figure,
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        fill: Option<Color>,
        stroke: Option<Stroke>,
    ) {
        let (Some(a), Some(b), Some(c), Some(d)) =
            (self.x.px(x0), self.x.px(x1), self.y.px(y0), self.y.px(y1))
        else {
            return;
        };
        let r = Rect::from_corners(a, c, b, d);
        fig.shape(
            Path::rect(r.x, r.y, r.w, r.h),
            fill,
            stroke,
            Some(self.rect),
        );
    }

    /// Vertical bars centred on `x`, `width` data units wide, from `base` to `y`. On a log axis,
    /// where zero has no position, a base at or below zero is the bottom of the axis.
    pub fn bars(&self, fig: &mut Figure, bars: &[(f64, f64)], width: f64, base: f64, fill: Color) {
        let base = if self.y.kind == ScaleKind::Log && base <= 0.0 {
            self.y.lo.min(self.y.hi)
        } else {
            base
        };
        for &(x, y) in bars {
            self.rect_data(
                fig,
                x - width / 2.0,
                base,
                x + width / 2.0,
                y,
                Some(fill),
                None,
            );
        }
    }

    /// A heat map: `values[row * nx + col]` over cells bounded by `x_edges` (nx + 1) and
    /// `y_edges` (ny + 1), coloured by `cmap` between `lo` and `hi`; non-finite cells are left
    /// out.
    pub fn heatmap(
        &self,
        fig: &mut Figure,
        x_edges: &[f64],
        y_edges: &[f64],
        values: &[f64],
        cmap: &Colormap,
        (lo, hi): (f64, f64),
    ) {
        let nx = x_edges.len().saturating_sub(1);
        let ny = y_edges.len().saturating_sub(1);
        for r in 0..ny {
            for c in 0..nx {
                let Some(&v) = values.get(r * nx + c) else {
                    continue;
                };
                if !v.is_finite() {
                    continue;
                }
                let t = if hi > lo { (v - lo) / (hi - lo) } else { 0.5 };
                self.rect_data(
                    fig,
                    x_edges[c],
                    y_edges[r],
                    x_edges[c + 1],
                    y_edges[r + 1],
                    Some(cmap.at(t)),
                    None,
                );
            }
        }
    }

    /// Text at a data point. Returns its box.
    pub fn text(
        &self,
        fig: &mut Figure,
        x: f64,
        y: f64,
        text: &str,
        style: &TextStyle,
    ) -> Option<Rect> {
        let (px, py) = self.pt(x, y)?;
        Some(fig.text(px, py, text, style))
    }

    /// An arrow between data points.
    pub fn arrow(&self, fig: &mut Figure, from: (f64, f64), to: (f64, f64), stroke: Stroke) {
        if let (Some(a), Some(b)) = (self.pt(from.0, from.1), self.pt(to.0, to.1)) {
            arrow_px(fig, a.0, a.1, b.0, b.1, stroke);
        }
    }

    /// Draw the axes: the x baseline with ticks and labels, y labels with faint grid lines (no
    /// y axis line), and the axis titles. Draw them before the data so the data sits on top.
    pub fn axes(&self, fig: &mut Figure, theme: &Theme, spec: &AxesSpec) -> AxesDrawn {
        let fs = theme.font_size;
        let hair = theme.hairline();
        let tick = (fs * 0.4).round();
        let pad = (fs * 0.5).round();
        let r = self.rect;
        if spec.fill {
            fig.fill_rect(r, theme.panel);
        }
        let xt = if spec.show_x {
            self.x.ticks(spec.x_ticks)
        } else {
            Vec::new()
        };
        let yt = if spec.show_y {
            self.y.ticks(spec.y_ticks)
        } else {
            Vec::new()
        };
        let grid_x = matches!(spec.grid, GridLines::X | GridLines::Both);
        let grid_y = matches!(spec.grid, GridLines::Y | GridLines::Both);
        let gstroke = Stroke::new(theme.grid, hair);
        if grid_y {
            for t in yt.iter().filter(|t| t.major) {
                if (t.px - r.bottom()).abs() > 0.5 {
                    fig.line(r.x, t.px, r.right(), t.px, gstroke.clone());
                }
            }
        }
        if grid_x {
            for t in xt.iter().filter(|t| t.major) {
                fig.line(t.px, r.y, t.px, r.bottom(), gstroke.clone());
            }
        }
        let label = TextStyle::new(fs, theme.muted);
        let mut x_boxes = Vec::new();
        if spec.show_x {
            let axis = Stroke::new(theme.rule, hair);
            fig.line(r.x, r.bottom(), r.right(), r.bottom(), axis.clone());
            let ls = label.clone().align(Anchor::Middle, Baseline::Top);
            let mut last_right = f32::NEG_INFINITY;
            // left to right on the page, whichever way the axis runs
            let mut by_px: Vec<&Tick> = xt.iter().collect();
            by_px.sort_by(|a, b| a.px.total_cmp(&b.px));
            for t in by_px {
                let len = if t.major { tick } else { tick * 0.6 };
                fig.line(t.px, r.bottom(), t.px, r.bottom() + len, axis.clone());
                if t.major && !t.label.is_empty() {
                    let b = ls.bounds(t.px, r.bottom() + tick + pad * 0.6, &t.label);
                    if b.x > last_right + fs * 0.4 && b.right() <= fig.width {
                        fig.text(t.px, r.bottom() + tick + pad * 0.6, &t.label, &ls);
                        last_right = b.right();
                        x_boxes.push(b);
                    }
                }
            }
            if let Some(xl) = &spec.x_label {
                let y = r.bottom() + tick + pad * 0.6 + fs * 1.1 + fs * 1.05;
                let st = label.clone().align(Anchor::End, Baseline::Alphabetic);
                fig.text(r.right(), y, xl, &st);
            }
        }
        if spec.show_y {
            let ls = label.clone().align(Anchor::End, Baseline::Middle);
            let mut last_top = f32::INFINITY;
            // bottom to top, skipping labels that would collide
            let mut sorted: Vec<&Tick> = yt
                .iter()
                .filter(|t| t.major && !t.label.is_empty())
                .collect();
            sorted.sort_by(|a, b| b.px.total_cmp(&a.px));
            for t in sorted {
                let b = ls.bounds(r.x - pad, t.px, &t.label);
                if b.bottom() < last_top - fs * 0.15 {
                    fig.text(r.x - pad, t.px, &t.label, &ls);
                    last_top = b.y;
                }
            }
            if let Some(yl) = &spec.y_label {
                let st = label.clone().align(Anchor::Start, Baseline::Alphabetic);
                let w = font::text_width(yl, Face::Sans, fs);
                let x = (r.x - pad - fs * 2.0)
                    .max(2.0)
                    .min((fig.width - w).max(0.0));
                fig.text(x, r.y - fs * 0.75, yl, &st);
            }
        }
        AxesDrawn {
            x_ticks: xt,
            y_ticks: yt,
        }
    }
}

/// The ticks the axes drew.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AxesDrawn {
    /// X ticks.
    pub x_ticks: Vec<Tick>,
    /// Y ticks.
    pub y_ticks: Vec<Tick>,
}

fn domain_of(s: &Scale, px0: f32) -> Domain {
    // the scale's lo sits at px0 unless reversed
    let reversed = (s.px0 - px0).abs() > 0.5;
    Domain {
        lo: s.lo,
        hi: s.hi,
        kind: s.kind,
        reversed,
    }
}

/// A min/max envelope of dense data, one column per pixel: millions of samples become a line a
/// few thousand points long that still shows every spike.
#[derive(Debug, Clone)]
pub struct Envelope {
    cols: Vec<Option<(f64, f64, f64, f64)>>,
    x0: f64,
    x1: f64,
}

impl Envelope {
    /// `columns` columns over x from `x0` to `x1`.
    pub fn new(columns: usize, x0: f64, x1: f64) -> Envelope {
        Envelope {
            cols: vec![None; columns.max(1)],
            x0,
            x1,
        }
    }

    /// Add one sample (non-finite values are skipped).
    pub fn push(&mut self, x: f64, y: f64) {
        if !(x.is_finite() && y.is_finite()) {
            return;
        }
        let n = self.cols.len();
        let span = self.x1 - self.x0;
        let t = if span == 0.0 {
            0.0
        } else {
            (x - self.x0) / span
        };
        if !(0.0..=1.0).contains(&t) {
            return;
        }
        let i = ((t * n as f64) as usize).min(n - 1);
        let slot = &mut self.cols[i];
        *slot = Some(match *slot {
            None => (y, y, y, y),
            Some((first, mn, mx, _)) => (first, mn.min(y), mx.max(y), y),
        });
    }

    /// Add one sample by column index (for evenly sampled data mapped by the caller).
    pub fn push_col(&mut self, col: usize, y: f64) {
        if !y.is_finite() {
            return;
        }
        if let Some(slot) = self.cols.get_mut(col) {
            *slot = Some(match *slot {
                None => (y, y, y, y),
                Some((first, mn, mx, _)) => (first, mn.min(y), mx.max(y), y),
            });
        }
    }

    /// The smallest and largest value seen.
    pub fn range(&self) -> Option<(f64, f64)> {
        let mut r: Option<(f64, f64)> = None;
        for &(_, mn, mx, _) in self.cols.iter().flatten() {
            r = Some(r.map_or((mn, mx), |(a, b)| (a.min(mn), b.max(mx))));
        }
        r
    }

    /// The line through the columns of `panel`: first, min, max, last of each column, at the
    /// column's pixel position. Empty columns join their neighbours.
    pub fn points(&self, panel: &Panel) -> Vec<(f32, f32)> {
        let n = self.cols.len();
        let mut out = Vec::with_capacity(n * 4);
        let step = (self.x1 - self.x0) / n as f64;
        for (i, c) in self.cols.iter().enumerate() {
            let Some((first, mn, mx, last)) = *c else {
                continue;
            };
            let x = self.x0 + (i as f64 + 0.5) * step;
            let Some(px) = panel.x.px(x) else {
                continue;
            };
            let py = |v: f64| panel.y.px(v).unwrap_or(f32::NAN);
            out.push((px, py(first)));
            if mn < mx {
                // visit the extreme met first, then the other
                let (a, b) = if (first - mn).abs() < (first - mx).abs() {
                    (mn, mx)
                } else {
                    (mx, mn)
                };
                out.push((px, py(a)));
                out.push((px, py(b)));
            }
            out.push((px, py(last)));
        }
        out
    }
}

/// Spread label positions so neighbours stay at least `gap` apart, keeping each as close to
/// its target as the others allow, within `lo..=hi`. Returns the positions in input order.
pub fn spread(targets: &[f32], gap: f32, lo: f32, hi: f32) -> Vec<f32> {
    let n = targets.len();
    if n == 0 {
        return Vec::new();
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| targets[a].total_cmp(&targets[b]));
    let mut pos: Vec<f32> = order.iter().map(|&i| targets[i].clamp(lo, hi)).collect();
    // pool adjacent violators (isotonic regression with spacing)
    let mut blocks: Vec<(f32, usize, usize)> = Vec::new(); // (mean of shifted targets, start, len)
    for (k, &p) in pos.iter().enumerate() {
        let shifted = p - gap * k as f32;
        blocks.push((shifted, k, 1));
        while blocks.len() >= 2 {
            let (m2, s2, l2) = blocks[blocks.len() - 1];
            let (m1, s1, l1) = blocks[blocks.len() - 2];
            if m1 <= m2 {
                break;
            }
            blocks.pop();
            blocks.pop();
            let l = l1 + l2;
            blocks.push(((m1 * l1 as f32 + m2 * l2 as f32) / l as f32, s1, l));
            let _ = s2;
        }
    }
    for &(m, s, l) in &blocks {
        for (k, p) in pos.iter_mut().enumerate().skip(s).take(l) {
            *p = m + gap * k as f32;
        }
    }
    // keep inside the bounds, pushing the whole stack if needed
    let top_over = pos.last().map_or(0.0, |p| (p - hi).max(0.0));
    if top_over > 0.0 {
        for p in &mut pos {
            *p -= top_over;
        }
    }
    let bottom_under = pos.first().map_or(0.0, |p| (lo - p).max(0.0));
    if bottom_under > 0.0 {
        for p in &mut pos {
            *p += bottom_under;
        }
    }
    let mut out = vec![0.0; n];
    for (k, &i) in order.iter().enumerate() {
        out[i] = pos[k];
    }
    out
}

/// A label at the right end of a line: its y target in pixels, text and colour.
#[derive(Debug, Clone, PartialEq)]
pub struct EndLabel {
    /// Where the line ends (pixels).
    pub y: f32,
    /// Label text.
    pub text: String,
    /// Colour (usually the line's).
    pub color: Color,
}

/// Direct labels to the right of `x` (pixels), one line of type apart, kept between `lo` and
/// `hi` when they fit (a stack taller than that grows upwards past `lo`). When any label had to
/// move off its line's end, every label steps out a little further and the moved ones get an
/// elbow leader: level with the line's end, a slanted run, level again into the label, so a
/// crowded stack still reads line by line.
pub fn end_labels(fig: &mut Figure, theme: &Theme, x: f32, labels: &[EndLabel], lo: f32, hi: f32) {
    let fs = theme.font_size;
    let gap = fs * 1.3;
    let ys: Vec<f32> = labels.iter().map(|l| l.y).collect();
    let need = gap * labels.len().saturating_sub(1) as f32;
    let lo = lo.min(hi - need).max(fs * 0.6);
    let pos = spread(&ys, gap, lo, hi);
    let moved = |a: f32, b: f32| (a - b).abs() > fs * 0.25;
    let any_moved = labels.iter().zip(&pos).any(|(l, &y)| moved(l.y, y));
    let tx = x + if any_moved { fs * 1.6 } else { fs * 0.5 };
    for (l, &y) in labels.iter().zip(&pos) {
        if moved(l.y, y) {
            let run = fs * 0.3;
            fig.stroke(
                Path::polyline([
                    (x + 2.0, l.y),
                    (x + 2.0 + run, l.y),
                    (tx - run - 2.0, y),
                    (tx - 3.0, y),
                ]),
                Stroke::new(l.color.alpha(0.55), theme.hairline()),
            );
        } else if any_moved {
            fig.stroke(
                Path::segment(x + 2.0, l.y, tx - 3.0, y),
                Stroke::new(l.color.alpha(0.55), theme.hairline()),
            );
        }
        let st = TextStyle::new(fs, l.color).align(Anchor::Start, Baseline::Middle);
        fig.text(tx, y, &l.text, &st);
    }
}

/// How far right of the panel [`end_labels`] puts its text, at most (room to reserve).
pub fn end_label_indent(theme: &Theme) -> f32 {
    theme.font_size * 1.6
}

/// A row of keys (a short line or block in each colour, then its text) starting at `(x, y)`
/// (the row's vertical middle), wrapping inside `max_w`. Returns the box used.
pub fn key_row(
    fig: &mut Figure,
    theme: &Theme,
    x: f32,
    y: f32,
    max_w: f32,
    items: &[(Color, String)],
    block: bool,
) -> Rect {
    let fs = theme.font_size;
    let st = TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Middle);
    let sw = fs * 1.1;
    let (mut cx, mut cy) = (x, y);
    let mut used = Rect::new(x, y - fs * 0.6, 0.0, fs * 1.2);
    for (c, t) in items {
        let w = sw + fs * 0.4 + st.width(t);
        if cx > x && cx + w > x + max_w {
            cx = x;
            cy += fs * 1.5;
        }
        if block {
            fig.fill(
                Path::round_rect(cx, cy - fs * 0.4, sw, fs * 0.8, fs * 0.15),
                *c,
            );
        } else {
            fig.stroke(
                Path::segment(cx, cy, cx + sw, cy),
                Stroke::new(*c, theme.line_width * 1.4).round(),
            );
        }
        let b = fig.text(cx + sw + fs * 0.4, cy, t, &st);
        used = used.union(&b);
        cx += w + fs * 1.2;
    }
    used
}

/// A horizontal colour bar over `rect` for `cmap` from `lo` to `hi`, with end labels under it
/// and an optional caption to its left.
pub fn color_bar(
    fig: &mut Figure,
    theme: &Theme,
    rect: Rect,
    cmap: &Colormap,
    lo: f64,
    hi: f64,
    caption: Option<&str>,
) {
    let steps = (rect.w.max(1.0) as usize).min(256);
    let dw = rect.w / steps as f32;
    for i in 0..steps {
        let t = if steps > 1 {
            i as f64 / (steps - 1) as f64
        } else {
            0.5
        };
        fig.fill_rect(
            Rect::new(rect.x + i as f32 * dw, rect.y, dw + 0.6, rect.h),
            cmap.at(t),
        );
    }
    let fs = theme.small_size();
    let st = TextStyle::new(fs, theme.muted);
    let step = (hi - lo).abs() / 4.0;
    fig.text(
        rect.x,
        rect.bottom() + fs * 1.15,
        &format_tick(lo, step),
        &st.clone().align(Anchor::Start, Baseline::Alphabetic),
    );
    fig.text(
        rect.right(),
        rect.bottom() + fs * 1.15,
        &format_tick(hi, step),
        &st.clone().align(Anchor::End, Baseline::Alphabetic),
    );
    if let Some(c) = caption {
        fig.text(
            rect.x - fs * 0.6,
            rect.y + rect.h / 2.0,
            c,
            &st.align(Anchor::End, Baseline::Middle),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spreads_labels() {
        let p = spread(&[10.0, 11.0, 12.0, 100.0], 10.0, 0.0, 200.0);
        assert!(p[1] - p[0] >= 9.99 && p[2] - p[1] >= 9.99, "{p:?}");
        assert!((p[3] - 100.0).abs() < 1e-3);
        // centred on the cluster
        assert!((p[1] - 11.0).abs() < 1e-3, "{p:?}");
        let q = spread(&[195.0, 196.0], 10.0, 0.0, 200.0);
        assert!(q[1] <= 200.0 && q[1] - q[0] >= 9.99, "{q:?}");
    }

    #[test]
    fn panels_map_and_draw() {
        let theme = Theme::light().for_size(400.0);
        let mut fig = Figure::new(400.0, 300.0, Some(theme.paper));
        let spec = AxesSpec {
            x_label: Some("time (s)".into()),
            y_label: Some("current (pA)".into()),
            ..AxesSpec::default()
        };
        let p = Panel::fit(
            fig.rect(),
            Domain::linear(0.0, 10.0),
            Domain::linear(-50.0, 50.0),
            &theme,
            &spec,
        );
        assert!(p.rect.x > 10.0 && p.rect.bottom() < 290.0);
        let (x, y) = p.pt(0.0, -50.0).unwrap();
        assert!((x - p.rect.x).abs() < 1e-3 && (y - p.rect.bottom()).abs() < 1e-3);
        let drawn = p.axes(&mut fig, &theme, &spec);
        assert!(drawn.x_ticks.len() >= 4);
        p.line(
            &mut fig,
            [(0.0, 0.0), (10.0, 40.0)],
            Stroke::new(theme.ink, 1.5),
        );
        p.band_x(&mut fig, 2.0, 3.0, theme.band);
        p.markers(&mut fig, [(5.0, 5.0)], &Marker::dot(theme.accent, 6.0));
        p.arrow(
            &mut fig,
            (1.0, 1.0),
            (4.0, 20.0),
            Stroke::new(theme.accent, 1.5),
        );
        let mut env = Envelope::new(100, 0.0, 10.0);
        for i in 0..10_000 {
            let t = f64::from(i) / 1000.0;
            env.push(t, (t * 7.0).sin() * 40.0);
        }
        assert!(env.points(&p).len() <= 400);
        let r = fig.to_raster(1.0);
        assert_eq!(r.width, 400);
        // reversed domains keep the reversal
        let q = Panel::new(
            p.rect,
            Domain::linear(0.0, 10.0).reversed(),
            Domain::linear(0.0, 1.0),
        );
        assert!(q.x_domain().reversed);
        assert!(!q.y_domain().reversed);
    }
}
