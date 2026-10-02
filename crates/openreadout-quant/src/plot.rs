//! PNG/JPEG plots of chromatograms, optionally with their integrated peaks shaded, baselines
//! drawn and peaks numbered: the picture a chromatographer checks an integration against.
//! Deterministic: the same input gives the same bytes.

use openreadout_preview::Canvas;
use openreadout_preview::color::TRACE_PALETTE;

use crate::analyze::ChromatogramPeaks;
use crate::extract::Chromatogram;

const BG: [u8; 3] = [255, 255, 255];
const AXIS: [u8; 3] = [90, 90, 90];
const TEXT: [u8; 3] = [40, 40, 40];
const BASE: [u8; 3] = [200, 30, 30];
const FILLS: [[u8; 3]; 2] = [[190, 215, 245], [250, 215, 170]];
const LEFT: i64 = 64;
const RIGHT: i64 = 14;
const TOP: i64 = 22;
const BOTTOM: i64 = 24;
/// Height of one chromatogram panel, pixels.
pub const PANEL_HEIGHT: u32 = 240;

fn nice_step(span: f64, ticks: f64) -> f64 {
    if !(span.is_finite() && span > 0.0) {
        return 1.0;
    }
    let raw = span / ticks;
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

fn label(v: f64) -> String {
    if v != 0.0 && (v.abs() >= 1e5 || v.abs() < 1e-2) {
        let e = v.abs().log10().floor() as i32;
        format!("{:.2}E{e}", v / 10f64.powi(e))
    } else {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Draw the chromatograms, one panel each, `width` pixels wide. `peaks[k]`, when present, is
/// drawn over panel `k`.
pub fn render(chroms: &[Chromatogram], peaks: &[Option<&ChromatogramPeaks>], width: u32) -> Canvas {
    render_labelled(chroms, peaks, &[], width)
}

/// [`render`] with an x-axis label per panel (`x_labels[k]`; `MIN` where none is given), for
/// spectra drawn the same way.
pub fn render_labelled(
    chroms: &[Chromatogram],
    peaks: &[Option<&ChromatogramPeaks>],
    x_labels: &[String],
    width: u32,
) -> Canvas {
    let width = width.clamp(200, 8192);
    let n = chroms.len().max(1) as u32;
    let mut c = Canvas::new(width, PANEL_HEIGHT * n, BG);
    for (k, ch) in chroms.iter().enumerate() {
        let top = i64::from(PANEL_HEIGHT) * k as i64;
        let label = x_labels.get(k).map_or("MIN", String::as_str);
        panel(&mut c, ch, peaks.get(k).copied().flatten(), top, k, label);
    }
    c
}

fn panel(
    c: &mut Canvas,
    ch: &Chromatogram,
    pk: Option<&ChromatogramPeaks>,
    top: i64,
    k: usize,
    x_label: &str,
) {
    let w = i64::from(c.width);
    let (x0, x1) = (LEFT, w - RIGHT);
    let (y0, y1) = (top + TOP, top + i64::from(PANEL_HEIGHT) - BOTTOM);
    let title = format!(
        "{}{}",
        ch.label,
        ch.intensity_unit
            .as_deref()
            .map(|u| format!(" ({u})"))
            .unwrap_or_default()
    );
    c.text(x0, top + 6, 2, TEXT, &title);
    c.line(x0, y1, x1, y1, AXIS);
    c.line(x0, y0, x0, y1, AXIS);
    let pts: Vec<(f64, f64)> = ch
        .rt_min
        .iter()
        .zip(&ch.intensity)
        .filter(|(t, y)| t.is_finite() && y.is_finite())
        .map(|(t, y)| (*t, *y))
        .collect();
    if pts.len() < 2 {
        c.text(x0 + 8, i64::midpoint(y0, y1), 2, TEXT, "NO DATA");
        return;
    }
    let (tmin, tmax) = (pts[0].0, pts[pts.len() - 1].0);
    let ymax = pts.iter().fold(f64::NEG_INFINITY, |m, p| m.max(p.1));
    let ymin = pts.iter().fold(0.0f64, |m, p| m.min(p.1));
    let yspan = if ymax > ymin {
        (ymax - ymin) * 1.08
    } else {
        1.0
    };
    let tspan = if tmax > tmin { tmax - tmin } else { 1.0 };
    let px = |t: f64| x0 + ((t - tmin) / tspan * (x1 - x0) as f64).round() as i64;
    let py = |y: f64| y1 - ((y - ymin) / yspan * (y1 - y0) as f64).round() as i64;
    // signal value at time t (linear interpolation)
    let at = |t: f64| -> f64 {
        let i = pts.partition_point(|p| p.0 < t);
        if i == 0 {
            return pts[0].1;
        }
        if i >= pts.len() {
            return pts[pts.len() - 1].1;
        }
        let (a, b) = (pts[i - 1], pts[i]);
        if b.0 == a.0 {
            b.1
        } else {
            a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0)
        }
    };
    if let Some(pk) = pk {
        for (j, p) in pk.peaks.iter().enumerate() {
            let fill = FILLS[j % 2];
            let (xa, xb) = (px(p.start_min), px(p.end_min));
            for x in xa..=xb {
                let t = tmin + (x - x0) as f64 / (x1 - x0) as f64 * tspan;
                let f = if p.end_min > p.start_min {
                    (t - p.start_min) / (p.end_min - p.start_min)
                } else {
                    0.0
                };
                let b = p.baseline_start + (p.baseline_end - p.baseline_start) * f;
                let (ya, yb) = (py(b), py(at(t)));
                c.line(x, ya.min(yb), x, ya.max(yb), fill);
            }
        }
    }
    let color = TRACE_PALETTE[k % TRACE_PALETTE.len()];
    let mut prev: Option<(i64, i64)> = None;
    for &(t, y) in &pts {
        let q = (px(t), py(y));
        if let Some(p) = prev
            && p != q
        {
            c.line(p.0, p.1, q.0, q.1, color);
        }
        prev = Some(q);
    }
    if let Some(pk) = pk {
        for p in &pk.peaks {
            c.line(
                px(p.start_min),
                py(p.baseline_start),
                px(p.end_min),
                py(p.baseline_end),
                BASE,
            );
            let (xa, ya) = (px(p.rt_min), py(at(p.rt_min)));
            let s = p.number.to_string();
            c.text(
                xa - openreadout_preview::canvas::text_width(&s, 1) / 2,
                (ya - 8).max(y0 - 2),
                1,
                TEXT,
                &s,
            );
        }
    }
    // axes: time ticks, intensity range
    let step = nice_step(tspan, 8.0);
    let mut t = (tmin / step).ceil() * step;
    while t <= tmax + 1e-9 {
        let x = px(t);
        c.line(x, y1, x, y1 + 3, AXIS);
        let s = label(t);
        c.text(
            x - openreadout_preview::canvas::text_width(&s, 1) / 2,
            y1 + 6,
            1,
            TEXT,
            &s,
        );
        t += step;
    }
    let s = x_label;
    c.text(
        x1 - openreadout_preview::canvas::text_width(s, 1),
        y1 + 14,
        1,
        TEXT,
        s,
    );
    for v in [ymin, ymin + (ymax - ymin) / 2.0, ymax] {
        let y = py(v);
        c.line(x0 - 3, y, x0, y, AXIS);
        let s = label(v);
        c.text(
            x0 - 6 - openreadout_preview::canvas::text_width(&s, 1),
            y - 2,
            1,
            TEXT,
            &s,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plot_is_deterministic_and_sized() {
        let rt: Vec<f64> = (0..500).map(|i| f64::from(i) * 0.02).collect();
        let y: Vec<f64> = rt
            .iter()
            .map(|t| 100.0 * (-0.5 * ((t - 5.0) / 0.1f64).powi(2)).exp())
            .collect();
        let ch = Chromatogram {
            label: "test".into(),
            rt_min: rt.clone(),
            intensity: y.clone(),
            ..Chromatogram::default()
        };
        let table =
            crate::peaks::find_peaks(&rt, &y, &crate::peaks::PeakParams::default()).unwrap();
        let pk = ChromatogramPeaks {
            peaks: table.peaks,
            ..ChromatogramPeaks::default()
        };
        let a = render(std::slice::from_ref(&ch), &[Some(&pk)], 600);
        let b = render(std::slice::from_ref(&ch), &[Some(&pk)], 600);
        assert_eq!(a, b);
        assert_eq!((a.width, a.height), (600, PANEL_HEIGHT));
        assert!(!a.is_gray());
        let empty = render(&[Chromatogram::default()], &[None], 300);
        assert_eq!(empty.height, PANEL_HEIGHT);
        assert_eq!(nice_step(10.0, 8.0), 1.0);
        assert_eq!(label(1_500_000.0), "1.50E6");
    }
}
