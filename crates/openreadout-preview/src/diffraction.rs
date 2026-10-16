//! Diffraction images (`extra.diffraction` on the image, see
//! `openreadout_core::diffraction`): pixels without a measurement drawn as no data, overloads
//! in red, shrinking by the maximum so that one-pixel Bragg spots stay visible, and resolution
//! rings around the beam centre.

use openreadout_core::diffraction::Geometry;
use openreadout_core::model::ImageInfo;
use openreadout_core::region::Region;
use openreadout_figure::{
    Color, Face, Figure, LabelPlacer, Path, Pill, Rect, Stroke, TextStyle, Theme,
};

use crate::DataMark;
use crate::rulers::Mapping;

/// `auto` contrast on diffraction images: logarithmic from the 90th percentile (above most of
/// the background) to the 99.95th (the spots).
pub(crate) const AUTO_LOG: (f64, f64) = (90.0, 99.95);
/// Colour of pixels without a measurement (module gaps, bad pixels): a cool light grey that
/// reads as "not data" on the light ground of an inverted image.
pub(crate) const NO_DATA: [u8; 3] = [212, 221, 232];
/// Colour of overloaded pixels.
pub(crate) const OVERLOAD: [u8; 3] = [214, 48, 39];
/// Colour of the resolution rings and their labels.
const RING: Color = Color::rgb(36, 99, 178);
/// Resolutions (Å) rings may be drawn at.
const NICE_D: &[f64] = &[
    50.0, 30.0, 20.0, 15.0, 10.0, 8.0, 6.0, 5.0, 4.0, 3.5, 3.0, 2.5, 2.2, 2.0, 1.8, 1.6, 1.5, 1.4,
    1.3, 1.2, 1.1, 1.0, 0.9, 0.8, 0.7, 0.6, 0.5,
];
/// Rings drawn at most.
const MAX_RINGS: usize = 5;

/// What the preview needs from an image's diffraction geometry.
#[derive(Debug, Clone)]
pub(crate) struct Diffraction {
    pub geometry: Geometry,
}

impl Diffraction {
    pub(crate) fn of(im: &ImageInfo) -> Option<Diffraction> {
        Geometry::of(im).map(|geometry| Diffraction { geometry })
    }

    /// Set pixels without a measurement to NaN (drawn as no data, left out of the contrast).
    pub(crate) fn mask(&self, v: &mut [f32]) {
        let g = &self.geometry;
        if !g.negative_is_masked && g.masked_values.is_empty() {
            return;
        }
        for x in v.iter_mut() {
            if g.is_masked(f64::from(*x)) {
                *x = f32::NAN;
            }
        }
    }

    pub(crate) fn overloaded(&self, v: f32) -> bool {
        v.is_finite() && self.geometry.is_overloaded(f64::from(v))
    }
}

/// Shrink a `w`×`h` plane to `ow`×`oh` keeping each output cell's largest finite sample (NaN
/// when it has none). Enlarging takes the sample under each cell's centre.
pub(crate) fn resample_max(
    src: &[f32],
    w: usize,
    h: usize,
    ow: usize,
    oh: usize,
) -> (usize, usize, Vec<f32>) {
    let span = |n_in: usize, n_out: usize, o: usize| -> (usize, usize) {
        let k = n_in as f64 / n_out.max(1) as f64;
        if k <= 1.0 {
            let c = (((o as f64 + 0.5) * k) as usize).min(n_in.saturating_sub(1));
            return (c, c + 1);
        }
        let a = (o as f64 * k).floor() as usize;
        let b = (((o + 1) as f64 * k).ceil() as usize).min(n_in);
        (a.min(n_in.saturating_sub(1)), b.max(a + 1))
    };
    let xs: Vec<(usize, usize)> = (0..ow).map(|o| span(w, ow, o)).collect();
    // rows first: h rows of ow maxima
    let mut rows = vec![f32::NAN; h * ow];
    for y in 0..h {
        let line = &src[y * w..(y + 1) * w];
        for (ox, &(a, b)) in xs.iter().enumerate() {
            let mut m = f32::NAN;
            for &v in &line[a..b.min(w)] {
                if v.is_finite() && (m.is_nan() || v > m) {
                    m = v;
                }
            }
            rows[y * ow + ox] = m;
        }
    }
    let mut out = vec![f32::NAN; ow * oh];
    for oy in 0..oh {
        let (a, b) = span(h, oh, oy);
        for ox in 0..ow {
            let mut m = f32::NAN;
            for y in a..b.min(h) {
                let v = rows[y * ow + ox];
                if v.is_finite() && (m.is_nan() || v > m) {
                    m = v;
                }
            }
            out[oy * ow + ox] = m;
        }
    }
    (ow, oh, out)
}

/// The resolutions of the rings to draw on a picture showing `shown` (full-resolution pixels):
/// up to [`MAX_RINGS`] nice values spread evenly in radius out to the farthest corner shown.
fn ring_resolutions(g: &Geometry, shown: Region) -> Vec<(f64, f64)> {
    let Some([bx, by]) = g.beam_center_px else {
        return Vec::new();
    };
    let corners = [
        (f64::from(shown.x), f64::from(shown.y)),
        (f64::from(shown.x + shown.width), f64::from(shown.y)),
        (f64::from(shown.x), f64::from(shown.y + shown.height)),
        (
            f64::from(shown.x + shown.width),
            f64::from(shown.y + shown.height),
        ),
    ];
    let far = corners
        .iter()
        .map(|(x, y)| ((x - bx).powi(2) + (y - by).powi(2)).sqrt())
        .fold(0.0, f64::max);
    if far <= 0.0 {
        return Vec::new();
    }
    let mut out: Vec<(f64, f64)> = Vec::new();
    for k in 1..=MAX_RINGS {
        let target = far * k as f64 / (MAX_RINGS as f64 + 0.3);
        let Some(d) = g.resolution_at_radius(target) else {
            continue;
        };
        // the nice value nearest in radius
        let best = NICE_D
            .iter()
            .filter_map(|&n| g.ring_radius_px(n).map(|r| (n, r)))
            .min_by(|a, b| (a.1 - target).abs().total_cmp(&(b.1 - target).abs()));
        if let Some((n, r)) = best
            && r <= far
            && !out.iter().any(|(m, _)| m.total_cmp(&n).is_eq())
            && (n - d).abs() / d < 0.5
        {
            out.push((n, r));
        }
    }
    out
}

fn fmt_d(d: f64) -> String {
    if d >= 10.0 || d.fract() == 0.0 && d >= 5.0 {
        format!("{d:.0} Å")
    } else {
        format!("{d:.1} Å")
    }
}

/// Draw resolution rings and the beam centre over the image in `plot` (PNG pixels) whose
/// full-resolution coordinates `map` gives. Returns the marks drawn.
pub(crate) fn draw_rings(
    fig: &mut Figure,
    plot: Region,
    map: Mapping,
    shown: Region,
    g: &Geometry,
    theme: &Theme,
) -> Vec<DataMark> {
    let mut marks = Vec::new();
    let Some([bx, by]) = g.beam_center_px else {
        return marks;
    };
    let rings = ring_resolutions(g, shown);
    let (ox, oy) = (plot.x as f32, plot.y as f32);
    let area = Rect::new(ox, oy, plot.width as f32, plot.height as f32);
    let to_px = |x: f64, y: f64| {
        let (px, py) = map.px_of(x, y);
        (ox + px as f32, oy + py as f32)
    };
    let inside = |(x, y): (f32, f32)| {
        x >= area.x && x <= area.x + area.w && y >= area.y && y <= area.y + area.h
    };
    let lw = (theme.line_width * 0.9).max(1.0);
    let fs = theme.font_size * 0.9;
    let pill = Pill {
        text: TextStyle::new(fs, RING).face(Face::SansMedium),
        fill: Color::rgb(247, 245, 240).alpha(0.88),
        stroke: None,
        pad: ((fs * 0.45).round(), (fs * 0.2).round()),
    };
    let mut placer = LabelPlacer::new(area.inset(2.0));
    for (d, r) in &rings {
        // The circle as a polyline cut where it leaves the picture.
        let steps = 360;
        let mut path = Path::new();
        let mut pen = false;
        let mut label_at: Option<(f32, f32)> = None;
        for i in 0..=steps {
            let a = std::f64::consts::TAU * f64::from(i) / f64::from(steps);
            let p = to_px(bx + r * a.cos(), by + r * a.sin());
            if inside(p) {
                if pen {
                    path.line_to(p.0, p.1);
                } else {
                    path.move_to(p.0, p.1);
                    pen = true;
                }
            } else {
                pen = false;
            }
        }
        // The label sits where the ring crosses the upper right, else wherever it is shown.
        for deg in [-45.0f64, -135.0, 45.0, 135.0, -90.0, 0.0, 90.0, 180.0] {
            let a = deg.to_radians();
            let p = to_px(bx + r * a.cos(), by + r * a.sin());
            if inside(p) {
                label_at = Some(p);
                break;
            }
        }
        if path.is_empty() {
            continue;
        }
        fig.stroke(
            path,
            Stroke::new(RING.alpha(0.75), lw).dashed(lw * 5.0, lw * 3.0),
        );
        let text = fmt_d(*d);
        if let Some((x, y)) = label_at {
            let (w, h) = pill.size(&text);
            let rect = placer.place(x - w / 2.0, y - h / 2.0, w, h, 0.0);
            pill.draw(fig, rect, &text);
        }
        marks.push(DataMark {
            kind: "resolution_ring".into(),
            x: *d,
            y: (r * 100.0).round() / 100.0,
            label: Some(text),
            channel: None,
        });
    }
    // The beam centre: a small cross.
    let (cx, cy) = to_px(bx, by);
    if inside((cx, cy)) {
        let s = (fs * 0.7).max(5.0);
        let st = Stroke::new(RING, lw * 1.2);
        fig.line(cx - s, cy, cx + s, cy, st.clone());
        fig.line(cx, cy - s, cx, cy + s, st);
    }
    marks.push(DataMark {
        kind: "beam_center".into(),
        x: bx,
        y: by,
        label: None,
        channel: None,
    });
    marks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_resampling_keeps_single_bright_pixels() {
        let mut src = vec![0f32; 16];
        src[5] = 100.0;
        src[15] = f32::NAN;
        let (w, h, v) = resample_max(&src, 4, 4, 2, 2);
        assert_eq!((w, h), (2, 2));
        assert_eq!(v, vec![100.0, 0.0, 0.0, 0.0]);
        let all_nan = vec![f32::NAN; 4];
        let (_, _, v) = resample_max(&all_nan, 2, 2, 1, 1);
        assert!(v[0].is_nan());
    }

    #[test]
    fn rings_are_nice_values_inside_the_picture() {
        let g = Geometry {
            wavelength_a: Some(0.9795),
            distance_mm: Some(190.18),
            beam_center_px: Some([1235.34, 1279.08]),
            pixel_size_mm: Some([0.172, 0.172]),
            ..Geometry::default()
        };
        let r = ring_resolutions(&g, Region::new(0, 0, 2463, 2527));
        assert!(r.len() >= 4, "{r:?}");
        assert!(r.iter().all(|(d, _)| NICE_D.contains(d)));
        assert!(r.windows(2).all(|w| w[0].1 < w[1].1));
        assert_eq!(fmt_d(3.0), "3.0 Å");
        assert_eq!(fmt_d(10.0), "10 Å");
    }
}
