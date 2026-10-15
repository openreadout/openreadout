//! Plate-reader fits from openreadout_assay: dose-response curves and standard curves.

use openreadout_assay::FitReport;
use openreadout_batch::analyze::{AnalyzeArgs, AnalyzeKind, AnalyzeOutput};
use openreadout_core::Result;
use serde_json::{Value, json};

use super::{Ctx, usage};
use crate::model::{Geom, Quantity, Series, Source, Weight};
use crate::spec::{MarkKind, SeriesSpec, Show};

/// The fitted model at `x`.
pub(crate) fn eval_fit(fit: &FitReport, x: f64) -> f64 {
    let p: Vec<f64> = fit.parameters.iter().map(|p| p.value).collect();
    let get = |i: usize| p.get(i).copied().unwrap_or(f64::NAN);
    match fit.model.as_str() {
        "linear" => get(0) * x + get(1),
        "4pl" => get(3) + (get(0) - get(3)) / (1.0 + (x / get(2)).powf(get(1))),
        _ => get(3) + (get(0) - get(3)) / (1.0 + (x / get(2)).powf(get(1))).powf(get(4)),
    }
}

fn curve_points(fit: &FitReport, lo: f64, hi: f64, log: bool) -> Vec<(f64, f64)> {
    let n = 160;
    (0..=n)
        .map(|i| {
            let t = f64::from(i) / f64::from(n);
            let x = if log {
                10f64.powf(lo.log10() + t * (hi.log10() - lo.log10()))
            } else {
                lo + t * (hi - lo)
            };
            (x, eval_fit(fit, x))
        })
        .filter(|p| p.1.is_finite())
        .collect()
}

fn short(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else if v.abs() >= 1.0 {
        format!("{v:.2}")
    } else {
        s
    };
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

pub(super) fn curves(
    ctx: &mut Ctx<'_>,
    spec: &SeriesSpec,
    file: &str,
    show: Show,
) -> Result<Vec<Series>> {
    let mut options = spec.assay.clone().unwrap_or_default();
    let analysis = if show == Show::StandardCurve {
        "curve"
    } else {
        "dose-response"
    };
    options.entry("analysis").or_insert_with(|| json!(analysis));
    options.remove("plot");
    let args = AnalyzeArgs {
        file: file.to_string(),
        kind: AnalyzeKind::Assay,
        options,
        strict: None,
    };
    let AnalyzeOutput::Assay(out) = openreadout_batch::analyze::run(ctx.reg, &args)?.0 else {
        return Err(usage("assay analysis returned something else"));
    };
    ctx.warnings.extend(out.warnings.iter().cloned());
    let normalized = out.wells.iter().any(|w| w.percent_effect.is_some());
    let yq = if normalized {
        Quantity::new("effect", Some("%"))
    } else {
        Quantity::new(out.read.label.clone(), out.read.unit.as_deref())
    };
    let xq = Quantity::new("concentration", out.layout.concentration_unit.as_deref());
    let mut series = Vec::new();
    if let Some(cv) = &out.curve {
        let fit = &cv.fit;
        let log = fit.model != "linear";
        let std_pts: Vec<(f64, f64)> = fit
            .points
            .iter()
            .map(|p| (p.x, p.y))
            .filter(|p| !log || p.0 > 0.0)
            .collect();
        let unknowns: Vec<(f64, f64)> = out
            .wells
            .iter()
            .filter(|w| !matches!(w.role.as_str(), "standard" | "blank" | "empty"))
            .filter_map(|w| Some((w.back_calculated?, w.value?)))
            .filter(|p| p.0.is_finite() && (!log || p.0 > 0.0))
            .collect();
        let xs: Vec<f64> = std_pts.iter().chain(&unknowns).map(|p| p.0).collect();
        let lo = xs.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let src = |what: &str| {
            Source::file(
                file,
                format!(
                    "standard curve ({} fit, {}), {what}",
                    fit.model, fit.weighting
                ),
            )
        };
        let mut c = Series::new(
            format!("{} fit · r² {:.4}", fit.model, fit.r_squared),
            Geom::Line(curve_points(fit, lo, hi, log)),
            MarkKind::Line,
            src("fitted curve"),
        )
        .axes(xq.clone(), yq.clone());
        c.weight = Weight::Fit;
        c.class = Some("standards".into());
        c.x_log = log;
        let mut s = Series::new(
            "standards",
            Geom::Points {
                pts: std_pts,
                err: Vec::new(),
            },
            MarkKind::Points,
            src("standards"),
        )
        .axes(xq.clone(), yq.clone());
        s.class = Some("standards".into());
        s.x_log = log;
        series.push(c);
        if !unknowns.is_empty() {
            let mut u = Series::new(
                "samples (back-calculated)",
                Geom::Points {
                    pts: unknowns,
                    err: Vec::new(),
                },
                MarkKind::Points,
                src("samples at their back-calculated concentration"),
            )
            .axes(xq.clone(), yq.clone());
            u.class = Some("samples".into());
            u.x_log = log;
            u.weight = Weight::Many;
            series.push(u);
        }
        // standards on top
        series.push(s);
        ctx.notes.push(format!(
            "standard curve {} r² {:.4}",
            fit.model, fit.r_squared
        ));
        return Ok(series);
    }
    let fits: Vec<_> = out.compounds.iter().filter(|c| c.fit.is_some()).collect();
    if fits.is_empty() {
        let why: Vec<String> = out
            .compounds
            .iter()
            .filter_map(|c| c.error.as_ref().map(|e| format!("{}: {e}", c.compound)))
            .collect();
        return Err(usage(format!(
            "{file}: no dose-response fit ({}); the layout must give compounds and concentrations (assay: {{layout, ...}})",
            if why.is_empty() {
                "no compounds found".to_string()
            } else {
                why.join("; ")
            }
        )));
    }
    for c in fits {
        let Some(fit) = &c.fit else { continue };
        let pts: Vec<(f64, f64)> = fit
            .points
            .iter()
            .map(|p| (p.x, p.y))
            .filter(|p| p.0 > 0.0)
            .collect();
        let lo = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let hi = pts.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        let label = c.compound.clone();
        let what = if c.kind.to_ascii_lowercase().contains("ic") {
            "IC50"
        } else {
            "EC50"
        };
        let src = |what: &str| {
            Source::file(
                file,
                format!("dose-response {} ({} fit), {what}", c.compound, fit.model),
            )
        };
        let mut line = Series::new(
            label.clone(),
            Geom::Line(curve_points(fit, lo, hi, true)),
            MarkKind::Line,
            src("fitted curve"),
        )
        .axes(xq.clone(), yq.clone());
        line.weight = Weight::Fit;
        line.class = Some(c.compound.clone());
        line.x_log = true;
        if let Some(e) = c.ec50.filter(|e| *e > 0.0 && *e >= lo && *e <= hi) {
            let unit = xq
                .unit
                .as_deref()
                .map(|u| format!(" {u}"))
                .unwrap_or_default();
            line.callouts.push(crate::model::Callout {
                x: e,
                y: eval_fit(fit, e),
                text: format!("{what} {}{unit}", short(e)),
                auto: false,
            });
        }
        let mut p = Series::new(
            label,
            Geom::Points {
                pts,
                err: Vec::new(),
            },
            MarkKind::Points,
            src("wells"),
        )
        .axes(xq.clone(), yq.clone());
        p.class = Some(c.compound.clone());
        p.x_log = true;
        p.facet = Some(c.compound.clone());
        line.facet = Some(c.compound.clone());
        series.push(line);
        series.push(p);
        if let Some(e) = c.ec50 {
            ctx.notes.push(format!(
                "{}: EC50 {}{}",
                c.compound,
                short(e),
                if c.extrapolated {
                    " (extrapolated)"
                } else {
                    ""
                }
            ));
        }
    }
    let _ = Value::Null;
    Ok(series)
}
