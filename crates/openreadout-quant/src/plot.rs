//! Pictures of chromatograms, optionally with their integrated peaks shaded, baselines drawn
//! and peaks numbered at their apex: the picture a chromatographer checks an integration
//! against. Drawn with the figure engine; deterministic: the same input gives the same bytes.

use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Domain, Face, Figure, Panel, Path, ScaleKind, Stroke, TextStyle,
    Theme,
};

use crate::analyze::ChromatogramPeaks;
use crate::extract::Chromatogram;

/// Height of one chromatogram panel, pixels.
pub const PANEL_HEIGHT: u32 = 280;

/// Draw the chromatograms, one panel each, `width` pixels wide. `peaks[k]`, when present, is
/// drawn over panel `k`.
pub fn render(chroms: &[Chromatogram], peaks: &[Option<&ChromatogramPeaks>], width: u32) -> Figure {
    render_labelled(chroms, peaks, &[], width)
}

/// [`render`] with an x-axis title per panel (`x_labels[k]`; `retention time (min)` where none
/// is given), for spectra drawn the same way.
pub fn render_labelled(
    chroms: &[Chromatogram],
    peaks: &[Option<&ChromatogramPeaks>],
    x_labels: &[String],
    width: u32,
) -> Figure {
    let width = width.clamp(200, 8192) as f32;
    let theme = Theme::light().for_size(width.min(1400.0));
    let n = chroms.len().max(1);
    let ph = PANEL_HEIGHT as f32;
    let mut fig = Figure::new(width, ph * n as f32, Some(theme.paper));
    for (k, ch) in chroms.iter().enumerate() {
        let label = x_labels
            .get(k)
            .map_or("retention time (min)", String::as_str);
        let area = openreadout_figure::Rect::new(0.0, ph * k as f32, width, ph);
        panel(
            &mut fig,
            &theme,
            ch,
            peaks.get(k).copied().flatten(),
            area,
            k,
            label,
        );
    }
    fig
}

fn panel(
    fig: &mut Figure,
    theme: &Theme,
    ch: &Chromatogram,
    pk: Option<&ChromatogramPeaks>,
    area: openreadout_figure::Rect,
    k: usize,
    x_label: &str,
) {
    let fs = theme.font_size;
    let title = format!(
        "{}{}",
        ch.label,
        ch.intensity_unit
            .as_deref()
            .map(|u| format!(" ({u})"))
            .unwrap_or_default()
    );
    let pts: Vec<(f64, f64)> = ch
        .rt_min
        .iter()
        .zip(&ch.intensity)
        .filter(|(t, y)| t.is_finite() && y.is_finite())
        .map(|(t, y)| (*t, *y))
        .collect();
    let title_style = TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic);
    if pts.len() < 2 {
        fig.text(area.x + fs, area.y + fs * 1.6, &title, &title_style);
        fig.text(
            area.x + fs,
            area.y + area.h / 2.0,
            "no data",
            &TextStyle::new(fs, theme.muted),
        );
        return;
    }
    let (tmin, tmax) = (pts[0].0, pts[pts.len() - 1].0);
    let ymax = pts.iter().fold(f64::NEG_INFINITY, |m, p| m.max(p.1));
    let ymin = pts.iter().fold(0.0f64, |m, p| m.min(p.1));
    let spec = AxesSpec {
        x_label: Some(x_label.to_string()),
        y_ticks: 4,
        top: fs * 1.4,
        ..AxesSpec::default()
    };
    let y_dom = Domain::of([ymin, ymax + (ymax - ymin) * 0.1], 0.0, ScaleKind::Linear);
    let p = Panel::fit(
        area.inset(fs * 0.8),
        Domain::linear(tmin, tmax),
        y_dom,
        theme,
        &spec,
    );
    fig.text(area.x + fs * 0.8, area.y + fs * 1.6, &title, &title_style);
    p.axes(fig, theme, &spec);
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
    let fills = [theme.band, theme.accent.alpha(0.18)];
    if let Some(pk) = pk {
        for (j, pe) in pk.peaks.iter().enumerate() {
            // the integrated area: between the baseline and the signal, start to end
            let steps = 64;
            let mut tri = Vec::with_capacity(steps + 1);
            for s in 0..=steps {
                let t = pe.start_min + (pe.end_min - pe.start_min) * s as f64 / steps as f64;
                let f = if pe.end_min > pe.start_min {
                    (t - pe.start_min) / (pe.end_min - pe.start_min)
                } else {
                    0.0
                };
                let b = pe.baseline_start + (pe.baseline_end - pe.baseline_start) * f;
                let y = at(t);
                tri.push((t, b.min(y), b.max(y)));
            }
            p.area(fig, &tri, fills[j % 2]);
        }
    }
    let color = theme.series(k);
    let mut env = openreadout_figure::Envelope::new((p.rect.w * 1.5).max(2.0) as usize, tmin, tmax);
    for &(t, y) in &pts {
        env.push(t, y);
    }
    p.line_px(
        fig,
        env.points(&p),
        Stroke::new(color, theme.line_width * 0.85),
    );
    if let Some(pk) = pk {
        let num = TextStyle::new(theme.small_size(), theme.ink)
            .face(Face::Mono)
            .align(Anchor::Middle, Baseline::Alphabetic)
            .halo(theme.paper, 2.0);
        for pe in &pk.peaks {
            p.line(
                fig,
                [
                    (pe.start_min, pe.baseline_start),
                    (pe.end_min, pe.baseline_end),
                ],
                Stroke::new(theme.accent, theme.hairline()),
            );
        }
        // numbers at the apexes, tallest peaks first; a number that would cover another is
        // left out (its peak keeps its baseline and shading)
        let mut order: Vec<&crate::peaks::Peak> = pk.peaks.iter().collect();
        order.sort_by(|a, b| b.height.total_cmp(&a.height));
        let mut taken: Vec<openreadout_figure::Rect> = Vec::new();
        for pe in order {
            let Some((x, y)) = p.pt(pe.rt_min, at(pe.rt_min)) else {
                continue;
            };
            let ly = (y - fs * 0.6).max(p.rect.y + fs * 0.4);
            let label = pe.number.to_string();
            let b = num.bounds(x, ly, &label).inset(-2.0);
            if taken.iter().any(|t| t.intersects(&b)) {
                continue;
            }
            taken.push(b);
            fig.stroke(
                Path::segment(x, y - 2.0, x, y - fs * 0.45),
                Stroke::new(theme.accent, theme.hairline()),
            );
            fig.text(x, ly, &label, &num);
        }
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
        let a = render(std::slice::from_ref(&ch), &[Some(&pk)], 600).to_raster(1.0);
        let b = render(std::slice::from_ref(&ch), &[Some(&pk)], 600).to_raster(1.0);
        assert_eq!(a, b);
        assert_eq!((a.width, a.height), (600, PANEL_HEIGHT));
        assert!(!a.is_gray());
        let empty = render(&[Chromatogram::default()], &[None], 300).to_raster(1.0);
        assert_eq!(empty.height, PANEL_HEIGHT);
    }
}
