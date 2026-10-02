//! Coordinate rulers, scale bar and optional grid around an image preview.
//!
//! The data (the downsampled plane) sits in a white frame. The top and left edges carry tick
//! marks labelled in full-resolution (level 0) pixel coordinates of the source plane, whatever
//! pyramid level, downsample and region were drawn, so an agent can read coordinates off the
//! picture and ask for `region {x, y, width, height}` in the same numbers. The bottom edge says
//! so (`RULERS: FULL-RES PX`) and, when the physical pixel size is known, carries a scale bar.

use openreadout_core::region::Region;

use crate::canvas::{Canvas, text_cased_width, text_width};
use crate::{ScaleBar, SourcePoint};

/// Margin background.
const PAPER: [u8; 3] = [255, 255, 255];
/// Frame and tick colour.
const INK: [u8; 3] = [70, 70, 70];
/// Label colour.
const TEXT: [u8; 3] = [20, 20, 20];
/// Most ticks per side (the 1-2-5 series then gives 4.4–11 steps over the span).
const MAX_TICKS: f64 = 11.0;
/// Grid line opacity out of 256.
const GRID_ALPHA: u32 = 96;
/// Caption at the bottom left, telling what the ruler numbers are.
const CAPTION: &str = "RULERS: FULL-RES PX";

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
}

/// Text scale for a preview whose longer side may be `max_size`: 3×5 font pixels become 2×2
/// from 200 px and 3×3 from 1400 px, so labels survive a viewer's own downscaling.
pub(crate) fn text_scale(max_size: u32) -> i64 {
    match max_size {
        0..200 => 1,
        200..1400 => 2,
        _ => 3,
    }
}

/// The frame around the data: margins in pixels, from the text scale and the widest labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    /// Text scale (font pixel size).
    pub s: i64,
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Layout {
    /// Margins for labels up to `max_x` / `max_y` (full-resolution coordinates).
    pub fn new(max_size: u32, max_x: u32, max_y: u32) -> Self {
        let s = text_scale(max_size);
        let (tick, gap, pad) = (tick_len(s), s + 1, s + 1);
        let xw = text_width(&max_x.to_string(), s);
        let yw = text_width(&max_y.to_string(), s);
        let row = 5 * s;
        Layout {
            s,
            left: (pad + yw + gap + tick) as u32,
            top: (pad + row + gap + tick) as u32,
            right: (xw / 2 + pad) as u32,
            bottom: (2 * gap + row + pad) as u32,
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

fn tick_len(s: i64) -> i64 {
    3 * s
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
        if v as f64 >= end || out.len() > 64 {
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

fn fmt_len(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as u64)
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Lighten a dark pixel, darken a bright one: a grid line that shows on fluorescence (dark)
/// and brightfield (white) data alike, and keeps grey images grey.
fn grid_pixel(c: &mut Canvas, x: i64, y: i64) {
    if x < 0 || y < 0 || x >= i64::from(c.width) || y >= i64::from(c.height) {
        return;
    }
    let i = (y as usize * c.width as usize + x as usize) * 3;
    let p = &mut c.rgb[i..i + 3];
    let lum = (u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3;
    let target = if lum < 128 { 255u32 } else { 0 };
    for v in p.iter_mut() {
        let old = u32::from(*v);
        *v = ((old * (256 - GRID_ALPHA) + target * GRID_ALPHA + 128) >> 8) as u8;
    }
}

/// What [`decorate`] drew.
pub(crate) struct Decorated {
    pub canvas: Canvas,
    /// The data rectangle inside the canvas.
    pub plot_area: Region,
    pub scale_bar: Option<ScaleBar>,
}

/// Frame `data` with rulers (and a scale bar when `um_per_source_px` is known; grid lines at
/// the ticks when `grid`).
pub(crate) fn decorate(
    data: &Canvas,
    layout: Layout,
    map: Mapping,
    um_per_source_px: Option<f64>,
    grid: bool,
) -> Decorated {
    let s = layout.s;
    let (dw, dh) = (data.width, data.height);
    let (cw, ch) = (dw + layout.h(), dh + layout.v());
    let mut c = Canvas::new(cw, ch, PAPER);
    let (l, t) = (i64::from(layout.left), i64::from(layout.top));
    for y in 0..dh as usize {
        let src = y * dw as usize * 3;
        let dst = ((y + layout.top as usize) * cw as usize + layout.left as usize) * 3;
        c.rgb[dst..dst + dw as usize * 3].copy_from_slice(&data.rgb[src..src + dw as usize * 3]);
    }
    let (w, h) = (i64::from(dw), i64::from(dh));
    // frame, one pixel outside the data
    c.line(l - 1, t - 1, l + w, t - 1, INK);
    c.line(l - 1, t + h, l + w, t + h, INK);
    c.line(l - 1, t - 1, l - 1, t + h, INK);
    c.line(l + w, t - 1, l + w, t + h, INK);
    let (tick, gap, pad) = (tick_len(s), s + 1, s + 1);
    let row = 5 * s;

    // x ruler (top)
    let max_x = map.origin.0 + f64::from(dw) * map.per_pixel.0;
    let label_w = text_width(&(max_x.ceil() as u64).to_string(), s);
    let step_x = tick_step(
        f64::from(dw) * map.per_pixel.0,
        (label_w + 3 * s) as f64 * map.per_pixel.0,
    );
    for (v, px) in ticks(map.origin.0, map.per_pixel.0, dw, step_x) {
        let x = l + i64::from(px);
        c.line(x, t - 1 - tick, x, t - 2, INK);
        if grid {
            for y in t..t + h {
                grid_pixel(&mut c, x, y);
            }
        }
        let label = v.to_string();
        let lw = text_width(&label, s) - s;
        // (a label wider than the whole picture, e.g. a 1 px region of a huge plane, starts at 0)
        let lx = (x - lw / 2).clamp(0, (i64::from(cw) - lw).max(0));
        c.text(lx, pad, s, TEXT, &label);
    }
    // y ruler (left)
    let step_y = tick_step(
        f64::from(dh) * map.per_pixel.1,
        (row + 4 * s) as f64 * map.per_pixel.1,
    );
    for (v, py) in ticks(map.origin.1, map.per_pixel.1, dh, step_y) {
        let y = t + i64::from(py);
        c.line(l - 1 - tick, y, l - 2, y, INK);
        if grid {
            for x in l..l + w {
                grid_pixel(&mut c, x, y);
            }
        }
        let label = v.to_string();
        let lw = text_width(&label, s) - s;
        let ly = (y - row / 2).clamp(0, (i64::from(ch) - row).max(0));
        c.text(l - 1 - tick - gap - lw, ly, s, TEXT, &label);
    }

    // bottom row: caption left, scale bar right
    let by = t + h + 1 + gap;
    let bar = um_per_source_px.and_then(|um| scale_bar(um * map.per_pixel.0, dw));
    let mut right_edge = l + w;
    if let Some(b) = &bar {
        let bar_px = i64::from(b.pixels);
        let bx = l + w - bar_px;
        let thick = (2 * s).max(2);
        c.fill_rect(bx, by + (row - thick) / 2, bar_px, thick, [0, 0, 0]);
        // end caps
        c.fill_rect(bx, by, s.max(1), row, [0, 0, 0]);
        c.fill_rect(bx + bar_px - s.max(1), by, s.max(1), row, [0, 0, 0]);
        let text = format!("{} {}", fmt_len(b.length), b.unit);
        let tw = text_cased_width(&text, s) - s;
        let tx = bx - gap - s - tw;
        c.text_cased(tx, by, s, TEXT, &text);
        right_edge = tx - 2 * gap;
    }
    if text_width(CAPTION, s) <= right_edge - (l - 1) {
        c.text(l - 1, by, s, TEXT, CAPTION);
    }
    Decorated {
        canvas: c,
        plot_area: Region::new(layout.left, layout.top, dw, dh),
        scale_bar: bar,
    }
}

/// The mapping reported for a canvas halved by [`Canvas::half`] (to fit a size or byte budget):
/// the plot area shrinks, each drawn pixel covers twice the source, and the origin moves by
/// half a source step when the plot area started on an odd column or row.
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
        let data = Canvas::new(1, 1, [9, 9, 9]);
        let map = Mapping {
            origin: (3_999_999_999.0, 3_999_999_999.0),
            per_pixel: (1.0, 1.0),
        };
        let d = decorate(&data, layout, map, Some(0.1), true);
        assert_eq!(d.plot_area.width, 1);
        assert!(d.scale_bar.is_none());
    }

    #[test]
    fn layout_and_decoration() {
        let layout = Layout::new(400, 12_000, 800);
        assert_eq!(layout.s, 2);
        let data = Canvas::new(300, 200, [0, 0, 0]);
        let map = Mapping {
            origin: (2000.0, 100.0),
            per_pixel: (10.0, 10.0),
        };
        let d = decorate(&data, layout, map, Some(0.25), false);
        assert_eq!(d.plot_area.width, 300);
        assert_eq!(
            (d.canvas.width, d.canvas.height),
            (300 + layout.h(), 200 + layout.v())
        );
        assert!(d.canvas.is_gray());
        let b = d.scale_bar.unwrap();
        // 2.5 µm per drawn px, 300 px → ≤ 150 µm → 100 µm = 40 px
        assert_eq!((b.length, b.pixels), (100.0, 40));
        // the data is copied unchanged into the plot area
        let px = |c: &Canvas, x: u32, y: u32| {
            let i = (y as usize * c.width as usize + x as usize) * 3;
            c.rgb[i]
        };
        assert_eq!(px(&d.canvas, d.plot_area.x + 5, d.plot_area.y + 5), 0);
        assert_eq!(px(&d.canvas, 0, d.canvas.height - 1), 255);
        // grid lines lighten dark data
        let g = decorate(&data, layout, map, None, true);
        assert!(g.scale_bar.is_none());
        let lit = (0..300).any(|x| px(&g.canvas, layout.left + x, layout.top + 50) > 0);
        assert!(lit);
        assert_eq!(
            map.covered(300, 200, (100_000, 100_000)),
            Region::new(2000, 100, 3000, 2000)
        );
    }
}
