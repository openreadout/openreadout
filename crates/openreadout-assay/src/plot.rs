//! Curve pictures: the standard curve (standards, fitted line, back-calculated unknowns) or the
//! dose-response curves (one colour per compound, labelled at their right ends), drawn with the
//! figure engine and encoded as PNG. Concentration axes are logarithmic for logistic models;
//! points at concentration 0 cannot be placed on a log axis and are left out (noted).

use openreadout_core::Result;
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Domain, EndLabel, Figure, GridLines, Marker, Panel, ScaleKind,
    Shape, Stroke, TextStyle, Theme, end_labels, format_value, key_row,
};

use crate::output::{AssayOutput, FitReport};

const W: f32 = 960.0;
const H: f32 = 600.0;

fn eval_fit(fit: &FitReport, x: f64) -> f64 {
    let p: Vec<f64> = fit.parameters.iter().map(|p| p.value).collect();
    let at = |i: usize| p.get(i).copied().unwrap_or(f64::NAN);
    match fit.model.as_str() {
        "linear" => at(0) * x + at(1),
        "4pl" => at(3) + (at(0) - at(3)) / (1.0 + (x / at(2)).powf(at(1))),
        _ => at(3) + (at(0) - at(3)) / (1.0 + (x / at(2)).powf(at(1))).powf(at(4)),
    }
}

/// The fitted curve sampled along the panel's x axis (evenly in pixels, so log axes too).
fn curve_points(panel: &Panel, fit: &FitReport) -> Vec<(f64, f64)> {
    let steps = 400;
    (0..=steps)
        .map(|i| {
            let px = panel.rect.x + panel.rect.w * i as f32 / steps as f32;
            let x = panel.x.value(px);
            (x, eval_fit(fit, x))
        })
        .collect()
}

fn domain(values: impl IntoIterator<Item = f64>, log: bool) -> Domain {
    if log {
        let v: Vec<f64> = values.into_iter().filter(|v| *v > 0.0).collect();
        let d = Domain::of(v.iter().copied(), 0.0, ScaleKind::Log);
        // a little room beyond the outermost points
        Domain::log(d.lo / 1.6, d.hi * 1.6)
    } else {
        Domain::of(values, 0.06, ScaleKind::Linear)
    }
}

/// Render the curve of a `curve` or `dose-response` result as PNG bytes. `None` when the
/// result has no curve.
pub fn render(out: &AssayOutput) -> Result<Option<(Vec<u8>, Vec<String>)>> {
    let mut notes = Vec::new();
    let theme = Theme::light().for_size(W);
    let fs = theme.font_size;
    let mut fig = Figure::new(W, H, Some(theme.paper));
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
    let title_style =
        TextStyle::new(theme.title_size(), theme.ink).align(Anchor::Start, Baseline::Alphabetic);
    if let Some(cv) = &out.curve {
        let fit = &cv.fit;
        let log = fit.model != "linear";
        let unknowns: Vec<(f64, f64)> = out
            .wells
            .iter()
            .filter(|w| w.role != "standard" && w.role != "blank" && w.role != "empty")
            .filter_map(|w| Some((w.back_calculated?, w.value?)))
            .filter(|(x, _)| x.is_finite() && (!log || *x > 0.0))
            .collect();
        let xs = fit
            .points
            .iter()
            .map(|p| p.x)
            .chain(unknowns.iter().map(|u| u.0));
        let ys = fit
            .points
            .iter()
            .map(|p| p.y)
            .chain(unknowns.iter().map(|u| u.1));
        let spec = AxesSpec {
            x_label: Some(format!("concentration{unit}")),
            y_label: Some(yl.clone()),
            grid: GridLines::Both,
            top: fs * 3.4,
            ..AxesSpec::default()
        };
        let panel = Panel::fit(
            fig.rect().inset(fs),
            domain(xs, log),
            domain(ys, false),
            &theme,
            &spec,
        );
        fig.text(
            fs,
            fs * 2.0,
            &format!(
                "Standard curve · {} fit · r² {:.4}",
                fit.model, fit.r_squared
            ),
            &title_style,
        );
        panel.axes(&mut fig, &theme, &spec);
        panel.line(
            &mut fig,
            curve_points(&panel, fit),
            Stroke::new(theme.ink, theme.line_width).round(),
        );
        let mut hidden = 0;
        let std_m = Marker::dot(theme.ink, fs * 0.6);
        for p in &fit.points {
            if panel.pt(p.x, p.y).is_some() {
                panel.markers(&mut fig, [(p.x, p.y)], &std_m);
            } else {
                hidden += 1;
            }
        }
        let unk_m = Marker {
            shape: Shape::Circle,
            size: fs * 0.6,
            fill: Some(theme.paper),
            stroke: Some(Stroke::new(theme.accent, theme.line_width)),
        };
        panel.markers(&mut fig, unknowns.iter().copied(), &unk_m);
        if hidden > 0 {
            notes.push(format!(
                "{hidden} standard point(s) at concentration 0 are not drawn on the log axis"
            ));
        }
        key_row(
            &mut fig,
            &theme,
            fs,
            fs * 3.6,
            W - 2.0 * fs,
            &[
                (theme.ink, "standards and fit".to_string()),
                (theme.accent, "unknowns, back-calculated".to_string()),
            ],
            true,
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
        let ylab = if out.wells.iter().any(|w| w.percent_effect.is_some()) {
            "percent effect".to_string()
        } else {
            yl
        };
        let label_w = fits
            .iter()
            .map(|c| {
                openreadout_figure::text_width(&c.compound, openreadout_figure::Face::Sans, fs)
                    + fs * 7.0
            })
            .fold(0.0f32, f32::max)
            .min(W * 0.32);
        let spec = AxesSpec {
            x_label: Some("concentration".into()),
            y_label: Some(ylab),
            grid: GridLines::Both,
            top: fs * 2.0,
            right: label_w,
            ..AxesSpec::default()
        };
        let panel = Panel::fit(
            fig.rect().inset(fs),
            domain(xs.iter().copied(), true),
            domain(ys.iter().copied(), false),
            &theme,
            &spec,
        );
        fig.text(fs, fs * 2.0, "Dose response", &title_style);
        panel.axes(&mut fig, &theme, &spec);
        let mut labels = Vec::new();
        for (i, cp) in fits.iter().enumerate() {
            let col = theme.series(i);
            let Some(fit) = &cp.fit else { continue };
            let pts = curve_points(&panel, fit);
            panel.line(
                &mut fig,
                pts.iter().copied(),
                Stroke::new(col, theme.line_width).round(),
            );
            panel.markers(
                &mut fig,
                fit.points.iter().map(|p| (p.x, p.y)),
                &Marker::dot(col, fs * 0.55),
            );
            let end = pts.iter().rev().find_map(|&(x, y)| panel.pt(x, y));
            if let Some((_, y)) = end {
                labels.push(EndLabel {
                    y: y.clamp(panel.rect.y, panel.rect.bottom()),
                    text: match cp.ec50 {
                        Some(e) => format!("{} · {} {}", cp.compound, cp.kind, format_value(e)),
                        None => cp.compound.clone(),
                    },
                    color: col,
                });
            }
        }
        end_labels(
            &mut fig,
            &theme,
            panel.rect.right(),
            &labels,
            panel.rect.y,
            panel.rect.bottom(),
        );
        let zeros = xs.iter().filter(|x| **x <= 0.0).count();
        if zeros > 0 {
            notes.push(format!(
                "{zeros} point(s) at concentration 0 are not drawn on the log axis"
            ));
        }
    } else {
        return Ok(None);
    }
    let png = fig.to_png(1.0)?;
    Ok(Some((png, notes)))
}
