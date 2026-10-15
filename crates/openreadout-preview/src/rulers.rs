//! Coordinate rulers, scale bar and optional grid around an image preview.
//!
//! The data (the downsampled plane) sits on paper, inside a hairline frame, at whole-pixel
//! offsets so its pixels reach the output unchanged. The top and left edges carry ticks
//! labelled in full-resolution (level 0) pixel coordinates of the source plane, whatever
//! pyramid level, downsample and region were drawn, so an agent can read coordinates off the
//! picture and ask for `region {x, y, width, height}` in the same numbers. Unlabelled minor
//! ticks split each step in five (or two), so positions between labels can be read too. The
//! bottom edge says what the numbers are and, when the physical pixel size is known, carries
//! a scale bar.

use std::sync::Arc;

use openreadout_core::region::Region;
use openreadout_figure::{
    Anchor, Baseline, Face, Figure, Path, Raster, Rect, Stroke, TextStyle, Theme, metrics,
    text_width,
};

use crate::{ScaleBar, SourcePoint};

/// Most ticks per side (the 1-2-5 series then gives 4.4–11 steps over the span).
const MAX_TICKS: f64 = 11.0;
/// Caption at the bottom left, telling what the ruler numbers are.
const CAPTION: &str = "full-resolution px";

/// Where the data sits in source (full-resolution) pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Mapping {
    /// Full-resolution coordinate of the data's left and top edges.
    pub origin: (f64, f64),
    /// Full-resolution pixels per drawn pixel, horizontally and vertically.
    pub per_pixel: (f64, f64),
}

impl Mapping {
    /// The full-resolution rectangle covered by `w`×`h` drawn pixels, rounded outward and
    /// clipped to `size`, the right and bottom edges of what was read (the last block of a
    /// downsample may be partial).
    pub fn covered(&self, w: u32, h: u32, size: (u32, u32)) -> Region {
        let x0 = self.origin.0.floor().max(0.0);
        let y0 = self.origin.1.floor().max(0.0);
        let x1 = (self.origin.0 + f64::from(w) * self.per_pixel.0)
            .ceil()
            .min(f64::from(size.0));
        let y1 = (self.origin.1 + f64::from(h) * self.per_pixel.1)
            .ceil()
            .min(f64::from(size.1));
        let (x0, y0) = (x0 as u32, y0 as u32);
        Region::new(
            x0,
            y0,
            (x1 as u32).saturating_sub(x0).max(1),
            (y1 as u32).saturating_sub(y0).max(1),
        )
    }

    /// The drawn-pixel position (relative to the data's top-left corner) of a full-resolution
    /// point.
    pub fn px_of(self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.origin.0) / self.per_pixel.0,
            (y - self.origin.1) / self.per_pixel.1,
        )
    }
}

/// Type size of the ruler labels for a preview whose longer side may be `max_size`.
pub(crate) fn label_size(max_size: u32) -> f32 {
    (max_size as f32 / 64.0).clamp(9.0, 22.0).round()
}

/// The frame around the data: margins in whole pixels, from the type size and the widest
/// labels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Layout {
    /// Ruler label size in pixels.
    pub fs: f32,
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Layout {
    /// Margins for labels up to `max_x` / `max_y` (full-resolution coordinates).
    pub fn new(max_size: u32, max_x: u32, max_y: u32) -> Self {
        let fs = label_size(max_size);
        let m = metrics(Face::Mono, fs);
        let (tick, gap, pad) = (tick_len(fs), gap(fs), pad(fs));
        let xw = text_width(&max_x.to_string(), Face::Mono, fs);
        let yw = text_width(&max_y.to_string(), Face::Mono, fs);
        Layout {
            fs,
            left: (pad + yw + gap + tick).ceil() as u32,
            top: (pad + m.cap_height + gap + tick).ceil() as u32,
            right: (xw / 2.0 + pad).ceil() as u32,
            bottom: (gap * 2.0 + fs * 1.15 + pad).ceil() as u32,
        }
    }
    /// Horizontal margins.
    pub fn h(&self) -> u32 {
        self.left + self.right
    }
    /// Vertical margins.
    pub fn v(&self) -> u32 {
        self.top + self.bottom
    }
}

fn tick_len(fs: f32) -> f32 {
    (fs * 0.5).round().max(3.0)
}

fn gap(fs: f32) -> f32 {
    (fs * 0.3).round().max(2.0)
}

fn pad(fs: f32) -> f32 {
    (fs * 0.45).round().max(2.0)
}

/// The smallest step of the series 1, 2, 5, 10, 20, 50, … that is at least `min`.
fn nice_ceil(min: f64) -> f64 {
    if !min.is_finite() || min <= 0.0 {
        return 1.0;
    }
    let mut p = 10f64.powi(min.log10().floor() as i32);
    loop {
        for m in [1.0, 2.0, 5.0] {
            let v = m * p;
            // tolerate floating-point noise at exact decades
            if v >= min * (1.0 - 1e-9) {
                return v;
            }
        }
        p *= 10.0;
    }
}

/// The largest value of the series …, 0.1, 0.2, 0.5, 1, 2, 5, 10, … that is at most `max`.
fn nice_floor(max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return 0.0;
    }
    let p = 10f64.powi(max.log10().floor() as i32);
    for m in [5.0, 2.0, 1.0] {
        let v = m * p;
        if v <= max * (1.0 + 1e-9) {
            return v;
        }
    }
    p
}

/// A whole-pixel tick step over `span` source pixels: at most [`MAX_TICKS`] ticks, and ticks at
/// least `min_gap` source pixels apart (room for their labels).
pub(crate) fn tick_step(span: f64, min_gap: f64) -> u64 {
    let v = nice_ceil((span / MAX_TICKS).max(min_gap).max(1.0));
    v.round().max(1.0) as u64
}

/// The minor step inside a major `step`: a fifth for 1× and 5× steps, a half for 2× steps,
/// whole pixels only.
fn minor_step(step: u64) -> Option<u64> {
    let lead = step / 10u64.pow(step.max(1).ilog10());
    let m = if lead == 2 { step / 2 } else { step / 5 };
    (m >= 1 && m < step && step.is_multiple_of(m)).then_some(m)
}

/// Tick positions `(source coordinate, drawn pixel)` along an axis of `len` drawn pixels.
pub(crate) fn ticks(origin: f64, per_pixel: f64, len: u32, step: u64) -> Vec<(u64, u32)> {
    let mut out = Vec::new();
    if step == 0 || per_pixel <= 0.0 || len == 0 {
        return out;
    }
    let start = (origin / step as f64).ceil().max(0.0) as u64;
    let end = origin + f64::from(len) * per_pixel;
    let mut k = start;
    while let Some(v) = k.checked_mul(step) {
        if v as f64 >= end || out.len() > 400 {
            break;
        }
        let px = ((v as f64 - origin) / per_pixel).round();
        if px >= 0.0 && px < f64::from(len) {
            out.push((v, px as u32));
        }
        k += 1;
    }
    out
}

/// A scale bar about a fifth of `data_w` long, for `um_per_pixel` µm per drawn pixel.
pub(crate) fn scale_bar(um_per_pixel: f64, data_w: u32) -> Option<ScaleBar> {
    if !um_per_pixel.is_finite() || um_per_pixel <= 0.0 {
        return None;
    }
    let um = nice_floor(f64::from(data_w) / 5.0 * um_per_pixel);
    let px = (um / um_per_pixel).round();
    if um <= 0.0 || px < 3.0 {
        return None;
    }
    let (length, unit) = if um >= 1000.0 {
        (um / 1000.0, "mm")
    } else if um < 1.0 {
        (um * 1000.0, "nm")
    } else {
        (um, "µm")
    };
    // the series values are exact in decimal: strip float noise such as 0.30000000000000004
    let length = (length * 1e6).round() / 1e6;
    Some(ScaleBar {
        length,
        unit: unit.into(),
        length_um: um,
        pixels: px as u32,
    })
}

pub(crate) fn fmt_len(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as u64)
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// What [`decorate`] drew.
pub(crate) struct Decorated {
    pub figure: Figure,
    /// The data rectangle inside the figure.
    pub plot_area: Region,
    pub scale_bar: Option<ScaleBar>,
}

/// Grid lines that show on dark (fluorescence) and bright (brightfield) data alike: a faint
/// dark line under a lighter one.
pub(crate) fn grid_line(fig: &mut Figure, x0: f32, y0: f32, x1: f32, y1: f32) {
    fig.line(
        x0,
        y0,
        x1,
        y1,
        Stroke::new(openreadout_figure::Color::rgb(0, 0, 0).alpha(0.28), 2.0),
    );
    fig.line(
        x0,
        y0,
        x1,
        y1,
        Stroke::new(
            openreadout_figure::Color::rgb(255, 255, 255).alpha(0.55),
            1.0,
        ),
    );
}

/// Frame `data` with rulers (and a scale bar when `um_per_source_px` is known; grid lines at
/// the ticks when `grid`).
pub(crate) fn decorate(
    data: Raster,
    layout: Layout,
    map: Mapping,
    um_per_source_px: Option<f64>,
    grid: bool,
    theme: &Theme,
) -> Decorated {
    let fs = layout.fs;
    let (dw, dh) = (data.width, data.height);
    let (cw, ch) = (dw + layout.h(), dh + layout.v());
    let mut fig = Figure::new(cw as f32, ch as f32, Some(theme.paper));
    let (l, t) = (layout.left as f32, layout.top as f32);
    let (w, h) = (dw as f32, dh as f32);
    fig.image(Rect::new(l, t, w, h), Arc::new(data), false);
    let (tick, gap, pad) = (tick_len(fs), gap(fs), pad(fs));
    let m = metrics(Face::Mono, fs);
    let ink = Stroke::new(theme.muted, 1.0);
    let minor = Stroke::new(theme.faint, 1.0);
    let label = TextStyle::new(fs, theme.ink).face(Face::Mono);

    // x ruler (top)
    let max_x = map.origin.0 + f64::from(dw) * map.per_pixel.0;
    let label_w = f64::from(text_width(
        &(max_x.ceil() as u64).to_string(),
        Face::Mono,
        fs,
    ));
    let step_x = tick_step(
        f64::from(dw) * map.per_pixel.0,
        (label_w + f64::from(fs) * 1.2) * map.per_pixel.0,
    );
    let mut grid_lines: Vec<(f32, f32, f32, f32)> = Vec::new();
    if let Some(ms) = minor_step(step_x) {
        let ticks_minor = ticks(map.origin.0, map.per_pixel.0, dw, ms);
        if ticks_minor.len() < dw as usize / 4 {
            for (v, px) in ticks_minor {
                if v % step_x != 0 {
                    let x = l + px as f32 + 0.5;
                    fig.line(x, t - 1.0 - tick * 0.55, x, t - 1.0, minor.clone());
                }
            }
        }
    }
    let xst = label.clone().align(Anchor::Middle, Baseline::Alphabetic);
    for (v, px) in ticks(map.origin.0, map.per_pixel.0, dw, step_x) {
        let x = l + px as f32 + 0.5;
        fig.line(x, t - 1.0 - tick, x, t - 1.0, ink.clone());
        if grid {
            grid_lines.push((x, t, x, t + h));
        }
        let s = v.to_string();
        let lw = text_width(&s, Face::Mono, fs);
        // (a label wider than the whole picture, e.g. a 1 px region of a huge plane, starts at 0)
        let lx = x.clamp(lw / 2.0, (cw as f32 - lw / 2.0).max(lw / 2.0));
        fig.text(lx, pad + m.cap_height, &s, &xst);
    }
    // y ruler (left)
    let row = f64::from(m.cap_height);
    let step_y = tick_step(
        f64::from(dh) * map.per_pixel.1,
        (row + f64::from(fs) * 1.3) * map.per_pixel.1,
    );
    if let Some(ms) = minor_step(step_y) {
        let ticks_minor = ticks(map.origin.1, map.per_pixel.1, dh, ms);
        if ticks_minor.len() < dh as usize / 4 {
            for (v, py) in ticks_minor {
                if v % step_y != 0 {
                    let y = t + py as f32 + 0.5;
                    fig.line(l - 1.0 - tick * 0.55, y, l - 1.0, y, minor.clone());
                }
            }
        }
    }
    let yst = label.clone().align(Anchor::End, Baseline::Middle);
    for (v, py) in ticks(map.origin.1, map.per_pixel.1, dh, step_y) {
        let y = t + py as f32 + 0.5;
        fig.line(l - 1.0 - tick, y, l - 1.0, y, ink.clone());
        if grid {
            grid_lines.push((l, y, l + w, y));
        }
        let half = m.cap_height / 2.0;
        let ly = y.clamp(half, (ch as f32 - half).max(half));
        fig.text(l - 1.0 - tick - gap, ly, &v.to_string(), &yst);
    }
    for (x0, y0, x1, y1) in grid_lines {
        grid_line(&mut fig, x0, y0, x1, y1);
    }
    // hairline frame, just outside the data
    fig.stroke(
        Path::rect(l - 0.5, t - 0.5, w + 1.0, h + 1.0),
        Stroke::new(theme.rule, 1.0),
    );

    // bottom row: caption left, scale bar right
    let mid = t + h + 1.0 + gap + fs * 0.6;
    let bar = um_per_source_px.and_then(|um| scale_bar(um * map.per_pixel.0, dw));
    let small = TextStyle::new(fs, theme.muted).align(Anchor::Start, Baseline::Middle);
    let mut right_edge = l + w;
    if let Some(b) = &bar {
        let bar_px = b.pixels as f32;
        let bx = l + w - bar_px;
        let thick = (fs * 0.28).round().max(2.0);
        fig.fill_rect(Rect::new(bx, mid - thick / 2.0, bar_px, thick), theme.ink);
        // end caps
        let cap = (fs * 0.35).round().max(2.0);
        fig.fill_rect(Rect::new(bx, mid - cap, 1.0, cap * 2.0), theme.ink);
        fig.fill_rect(
            Rect::new(bx + bar_px - 1.0, mid - cap, 1.0, cap * 2.0),
            theme.ink,
        );
        let text = format!("{} {}", fmt_len(b.length), b.unit);
        let st = TextStyle::new(fs, theme.ink).align(Anchor::End, Baseline::Middle);
        let r = fig.text(bx - gap * 1.5, mid, &text, &st);
        right_edge = r.x - gap * 2.0;
    }
    if text_width(CAPTION, Face::Sans, fs) <= right_edge - (l - 1.0) {
        fig.text(l - 1.0, mid, CAPTION, &small);
    }
    Decorated {
        figure: fig,
        plot_area: Region::new(layout.left, layout.top, dw, dh),
        scale_bar: bar,
    }
}

/// The mapping reported for a raster halved by [`Raster::half`] (to fit a byte budget): the
/// plot area shrinks, each drawn pixel covers twice the source, and the origin moves by half a
/// source step when the plot area started on an odd column or row.
pub(crate) fn halve(
    plot: Region,
    origin: SourcePoint,
    per_pixel: (f64, f64),
) -> (Region, SourcePoint, (f64, f64)) {
    let nx = plot.x / 2;
    let ny = plot.y / 2;
    let origin = SourcePoint {
        x: origin.x - f64::from(plot.x - 2 * nx) * per_pixel.0,
        y: origin.y - f64::from(plot.y - 2 * ny) * per_pixel.1,
    };
    let right = (plot.x + plot.width).div_ceil(2);
    let bottom = (plot.y + plot.height).div_ceil(2);
    (
        Region::new(
            nx,
            ny,
            right.saturating_sub(nx).max(1),
            bottom.saturating_sub(ny).max(1),
        ),
        origin,
        (per_pixel.0 * 2.0, per_pixel.1 * 2.0),
    )
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn nice_steps() {
        assert_eq!(nice_ceil(1.0), 1.0);
        assert_eq!(nice_ceil(1.1), 2.0);
        assert_eq!(nice_ceil(3.0), 5.0);
        assert_eq!(nice_ceil(7.0), 10.0);
        assert_eq!(nice_ceil(100.0), 100.0);
        assert_eq!(nice_ceil(101.0), 200.0);
        assert_eq!(nice_ceil(0.3), 0.5);
        assert_eq!(nice_floor(99.0), 50.0);
        assert_eq!(nice_floor(100.0), 100.0);
        assert_eq!(nice_floor(0.07), 0.05);
        // about 5–10 ticks over typical spans, labels permitting
        for span in [48.0, 640.0, 1000.0, 2220.0, 46_000.0, 99_999.0, 150_000.0] {
            let step = tick_step(span, 0.0);
            let n = span / step as f64;
            assert!((4.0..=11.0).contains(&n), "{span}: step {step}, {n} ticks");
            assert!(
                [1, 2, 5].contains(&(step / 10u64.pow(step.ilog10()))),
                "{step}"
            );
        }
        // wide labels push the step up
        assert_eq!(tick_step(1000.0, 150.0), 200);
        assert_eq!(tick_step(5.0, 0.0), 1, "never below one pixel");
        assert_eq!(tick_step(2048.0, 0.0), 200);
        assert_eq!(minor_step(1000), Some(200));
        assert_eq!(minor_step(2000), Some(1000));
        assert_eq!(minor_step(500), Some(100));
        assert_eq!(minor_step(1), None);
        assert_eq!(minor_step(2), Some(1));
    }

    #[test]
    fn tick_positions_follow_the_mapping() {
        // 100 drawn px showing source x 1000..2600 (16 source px per drawn px)
        let t = ticks(1000.0, 16.0, 100, 500);
        assert_eq!(t, vec![(1000, 0), (1500, 31), (2000, 63), (2500, 94)]);
        // origin between ticks
        let t = ticks(1010.0, 2.0, 50, 20);
        assert_eq!(t.first(), Some(&(1020, 5)));
        assert_eq!(t.last(), Some(&(1100, 45)));
    }

    #[test]
    fn scale_bars() {
        // 0.5 µm/px, 400 px wide → at most 40 µm → 20 µm, 40 px
        let b = scale_bar(0.5, 400).unwrap();
        assert_eq!((b.length, b.unit.as_str(), b.pixels), (20.0, "µm", 40));
        // 64 µm/px over 500 px → 6400 µm → 5 mm
        let b = scale_bar(64.0, 500).unwrap();
        assert_eq!((b.length, b.unit.as_str(), b.pixels), (5.0, "mm", 78));
        assert!((b.length_um - 5000.0).abs() < 1e-9);
        // 5 nm/px over 300 px → 0.3 µm → 200 nm
        let b = scale_bar(0.005, 300).unwrap();
        assert_eq!((b.length, b.unit.as_str(), b.pixels), (200.0, "nm", 40));
        assert!(scale_bar(0.0, 300).is_none());
        assert!(scale_bar(1.0, 10).is_none(), "too short to draw");
    }

    #[test]
    fn halving_keeps_the_mapping() {
        let plot = Region::new(41, 20, 300, 201);
        let (p, o, s) = halve(plot, SourcePoint { x: 1000.0, y: 0.0 }, (4.0, 4.0));
        assert_eq!(p, Region::new(20, 10, 151, 101));
        assert_eq!(s, (8.0, 8.0));
        // png column 20 of the halved image is column 40 of the original: one before the data
        assert!((o.x - 996.0).abs() < 1e-9 && o.y.abs() < 1e-9);
        // a source point maps to the same place either way
        let src = 1000.0 + (141.0 - 41.0) * 4.0; // original column 141
        let halved = o.x + (f64::from(141u32 / 2) - f64::from(p.x)) * s.0;
        assert!((src - halved).abs() <= 4.0 + 1e-9);
    }

    #[test]
    fn narrow_regions_of_huge_planes_do_not_panic() {
        // a 1 × 1 px look at x = 3 999 999 999: labels wider than the picture
        let layout = Layout::new(400, u32::MAX, u32::MAX);
        let data = Raster::new(1, 1, [9, 9, 9]);
        let map = Mapping {
            origin: (3_999_999_999.0, 3_999_999_999.0),
            per_pixel: (1.0, 1.0),
        };
        let d = decorate(data, layout, map, Some(0.1), true, &Theme::light());
        assert_eq!(d.plot_area.width, 1);
        assert!(d.scale_bar.is_none());
        let _ = d.figure.to_raster(1.0);
    }

    #[test]
    fn layout_and_decoration() {
        let layout = Layout::new(400, 12_000, 800);
        let data = Raster::new(300, 200, [0, 0, 0]);
        let map = Mapping {
            origin: (2000.0, 100.0),
            per_pixel: (10.0, 10.0),
        };
        let d = decorate(
            data.clone(),
            layout,
            map,
            Some(0.25),
            false,
            &Theme::light(),
        );
        assert_eq!(d.plot_area.width, 300);
        let r = d.figure.to_raster(1.0);
        assert_eq!((r.width, r.height), (300 + layout.h(), 200 + layout.v()));
        let b = d.scale_bar.unwrap();
        // 2.5 µm per drawn px, 300 px → ≤ 150 µm → 100 µm = 40 px
        assert_eq!((b.length, b.pixels), (100.0, 40));
        // the data is copied unchanged into the plot area
        assert_eq!(r.get(d.plot_area.x + 5, d.plot_area.y + 5), Some([0, 0, 0]));
        assert_eq!(
            r.get(d.plot_area.x + 299, d.plot_area.y + 199),
            Some([0, 0, 0])
        );
        // grid lines lighten dark data
        let g = decorate(data, layout, map, None, true, &Theme::light());
        assert!(g.scale_bar.is_none());
        let gr = g.figure.to_raster(1.0);
        let lit = (0..300).any(|x| gr.get(layout.left + x, layout.top + 50).unwrap()[0] > 0);
        assert!(lit);
        assert_eq!(
            map.covered(300, 200, (100_000, 100_000)),
            Region::new(2000, 100, 3000, 2000)
        );
    }
}
