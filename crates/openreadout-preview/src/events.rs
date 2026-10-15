//! Event previews: flow cytometry events as a density plot of two parameters (darker where
//! more events fall), or a histogram of one. Scatter parameters (FSC, SSC) and time use linear
//! axes; fluorescence uses an asinh axis, linear near zero and logarithmic beyond, so negative
//! compensated values stay visible.

use std::sync::Arc;

use openreadout_core::model::{FileInfo, TableInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Domain, Figure, GridLines, Panel, Path, Raster, ScaleKind,
    TextStyle,
};

use crate::{EventsPreview, Frame, FramePanel, PreviewOutput, PreviewRequest};

/// Most events drawn.
const MAX_EVENTS: u64 = 500_000;
/// The asinh cofactor for fluorescence axes.
const COFACTOR: f64 = 150.0;

/// True when table `table` of the file holds cytometry events.
pub(crate) fn is_events(info: &FileInfo, table: u32) -> bool {
    info.format.family == "flow-cytometry" && info.tables.iter().any(|t| t.index == table)
}

fn find_col(t: &TableInfo, name: &str) -> Option<usize> {
    t.columns.iter().position(|c| c.name == name).or_else(|| {
        t.columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    })
}

fn default_cols(t: &TableInfo) -> Vec<usize> {
    let pick = |want: &[&str]| want.iter().find_map(|w| find_col(t, w));
    if let (Some(a), Some(b)) = (
        pick(&["FSC-A", "FSC-H", "FSC"]),
        pick(&["SSC-A", "SSC-H", "SSC"]),
    ) {
        return vec![a, b];
    }
    t.columns
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.name.eq_ignore_ascii_case("time"))
        .map(|(i, _)| i)
        .take(2)
        .collect()
}

fn is_linear(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.starts_with("FSC")
        || n.starts_with("SSC")
        || n == "TIME"
        || n.contains("WIDTH")
        || n.ends_with("-W")
}

fn domain_for(name: &str, v: &[f64]) -> (Domain, &'static str) {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    s.sort_by(f64::total_cmp);
    let q = |p: f64| -> f64 {
        if s.is_empty() {
            return 0.0;
        }
        s[((s.len() - 1) as f64 * p).round() as usize]
    };
    // robust ends, a little padded: scatter from the 0.5th to the 99.5th percentile (a few
    // debris events and doublets would squeeze the population into a corner), fluorescence
    // from the 0.05th to the 99.95th
    if is_linear(name) {
        let (lo, hi) = (q(0.005), q(0.995));
        (
            Domain::of([lo.min(0.0), hi], 0.05, ScaleKind::Linear),
            "linear",
        )
    } else {
        let (lo, hi) = (q(0.0005), q(0.9995));
        let span = hi - lo;
        (
            Domain::asinh(lo - span * 0.02, hi + span * 0.05, COFACTOR),
            "asinh",
        )
    }
}

/// `290172` → `290,172`.
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The step between neighbouring recorded values when the parameter is stored on a grid
/// (integer channels, a coarse ADC), from up to 20,000 values; 0 when it looks continuous.
fn resolution(v: &[f64]) -> f64 {
    let step = (v.len() / 20_000).max(1);
    let mut s: Vec<f64> = v
        .iter()
        .step_by(step)
        .copied()
        .filter(|x| x.is_finite())
        .collect();
    s.sort_by(f64::total_cmp);
    s.dedup();
    if s.len() < 3 {
        return 0.0;
    }
    let mut gaps: Vec<f64> = s.windows(2).map(|w| w[1] - w[0]).collect();
    gaps.sort_by(f64::total_cmp);
    // the typical gap between distinct values; a continuous parameter has many tiny gaps
    let typical = gaps[gaps.len() / 4];
    let span = s[s.len() - 1] - s[0];
    if span > 0.0 && typical > span * 1e-5 {
        typical
    } else {
        0.0
    }
}

/// Bins for a histogram along `scale` (pixels 0..1000): 160, fewer when the values sit on a
/// grid so coarse that narrow bins would alternate full and empty (a comb).
fn histogram_bins(v: &[f64], scale: &openreadout_figure::Scale) -> usize {
    let r = resolution(v);
    let mut bins = 160usize;
    if r <= 0.0 {
        return bins;
    }
    while bins > 24 {
        // the narrowest bin in data units
        let narrowest = (0..bins)
            .map(|i| {
                let a = scale.value(1000.0 * i as f32 / bins as f32);
                let b = scale.value(1000.0 * (i + 1) as f32 / bins as f32);
                (b - a).abs()
            })
            .fold(f64::INFINITY, f64::min);
        if narrowest >= r * 4.0 {
            break;
        }
        bins = bins * 3 / 4;
    }
    bins
}

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    let ti = req.table.unwrap_or(0);
    let t = info
        .tables
        .iter()
        .find(|t| t.index == ti)
        .ok_or_else(|| {
            Error::Usage(format!(
                "table {ti} out of range (file has {} tables)",
                info.tables.len()
            ))
        })?
        .clone();
    let cols: Vec<usize> = if req.columns.is_empty() {
        default_cols(&t)
    } else {
        if req.columns.len() > 2 {
            return Err(Error::Usage(format!(
                "an event preview draws one or two parameters (x, y); got {}",
                req.columns.len()
            )));
        }
        req.columns
            .iter()
            .map(|n| {
                find_col(&t, n).ok_or_else(|| {
                    Error::Usage(format!(
                        "parameter '{n}' not in table {ti} ({})",
                        t.columns
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })
            })
            .collect::<Result<_>>()?
    };
    if cols.is_empty() {
        return Err(Error::Usage(format!(
            "table {ti} has no parameters to draw"
        )));
    }
    let n = t.row_count.min(MAX_EVENTS);
    if n < t.row_count {
        out.notes
            .push(format!("drew the first {n} of {} events", t.row_count));
    }
    let tab = ds.read_table(ti, 0, n)?;
    let get = |i: usize| tab.columns.get(i).cloned().unwrap_or_default();
    let xs = get(cols[0]);
    let x_name = t.columns[cols[0]].name.clone();
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let size = req.max_size.max(64) as f32;
    let (xd, xk) = domain_for(&x_name, &xs);
    let mut scales = vec![xk.to_string()];
    let mut fig;
    let panel;
    let mut y_name = None;
    if let Some(&yc) = cols.get(1) {
        let ys = get(yc);
        let yn = t.columns[yc].name.clone();
        let (yd, yk) = domain_for(&yn, &ys);
        scales.push(yk.into());
        fig = Figure::new(size, (size * 0.86).round(), Some(theme.paper));
        let spec = AxesSpec {
            x_label: Some(x_name.clone()),
            y_label: Some(yn.clone()),
            grid: GridLines::None,
            top: fs * 1.4,
            ..AxesSpec::default()
        };
        panel = Panel::fit(fig.rect().inset(fs * 0.8), xd, yd, &theme, &spec);
        panel.axes(&mut fig, &theme, &spec);
        // density: a grid of bins over the panel, coloured by log count
        let bw = panel.rect.w.round().clamp(16.0, 1600.0) as usize;
        let bh = panel.rect.h.round().clamp(16.0, 1600.0) as usize;
        let mut counts = vec![0u32; bw * bh];
        let r = panel.rect;
        for (&x, &y) in xs.iter().zip(&ys) {
            if let (Some(px), Some(py)) = (panel.x.px(x), panel.y.px(y)) {
                let fx = (px - r.x) / r.w;
                let fy = (py - r.y) / r.h;
                if (0.0..1.0).contains(&fx) && (0.0..1.0).contains(&fy) {
                    let (i, j) = ((fx * bw as f32) as usize, (fy * bh as f32) as usize);
                    counts[j * bw + i] = counts[j * bw + i].saturating_add(1);
                }
            }
        }
        // one bin per pixel, smoothed with a 5 × 5 binomial kernel (two passes), so single
        // events read as soft dots and dense populations as smooth clouds
        let k = [1.0, 4.0, 6.0, 4.0, 1.0];
        let mut tmp = vec![0f64; bw * bh];
        for j in 0..bh {
            for i in 0..bw {
                let mut s = 0f64;
                for (d, w) in k.iter().enumerate() {
                    let x = i as i64 + d as i64 - 2;
                    if x >= 0 && (x as usize) < bw {
                        s += w * f64::from(counts[j * bw + x as usize]);
                    }
                }
                tmp[j * bw + i] = s / 16.0;
            }
        }
        let mut dens = vec![0f64; bw * bh];
        for j in 0..bh {
            for i in 0..bw {
                let mut s = 0f64;
                for (d, w) in k.iter().enumerate() {
                    let y = j as i64 + d as i64 - 2;
                    if y >= 0 && (y as usize) < bh {
                        s += w * tmp[y as usize * bw + i];
                    }
                }
                dens[j * bw + i] = s / 16.0;
            }
        }
        let top = dens.iter().copied().fold(0.0, f64::max).max(1e-9);
        let ltop = top.ln_1p();
        let mut img = Raster::new(bw as u32, bh as u32, theme.paper.to_rgb());
        for (k, &c) in dens.iter().enumerate() {
            if c > 0.01 {
                let t = 0.1 + 0.9 * c.ln_1p() / ltop;
                let col = theme.sequential.at(t).to_rgb();
                img.rgb[k * 3..k * 3 + 3].copy_from_slice(&col);
            }
        }
        fig.image(r, Arc::new(img), true);
        y_name = Some(yn);
    } else {
        fig = Figure::new(size, (size * 0.6).round(), Some(theme.paper));
        let probe = Panel::new(
            openreadout_figure::Rect::new(0.0, 0.0, 1000.0, 100.0),
            xd,
            Domain::linear(0.0, 1.0),
        );
        let bins = histogram_bins(&xs, &probe.x);
        let mut counts = vec![0u32; bins];
        for &x in &xs {
            if let Some(px) = probe.x.px(x)
                && (0.0..1000.0).contains(&px)
            {
                let i = ((px / 1000.0) * bins as f32) as usize;
                counts[i.min(bins - 1)] += 1;
            }
        }
        let top = f64::from(counts.iter().copied().max().unwrap_or(1).max(1));
        let spec = AxesSpec {
            x_label: Some(x_name.clone()),
            y_label: Some("events".into()),
            top: fs * 1.4,
            ..AxesSpec::default()
        };
        panel = Panel::fit(
            fig.rect().inset(fs * 0.8),
            xd,
            Domain::linear(0.0, top * 1.05).nice(4),
            &theme,
            &spec,
        );
        panel.axes(&mut fig, &theme, &spec);
        let r = panel.rect;
        let mut path = Path::new();
        path.move_to(r.x, r.bottom());
        for (i, &c) in counts.iter().enumerate() {
            let x0 = r.x + r.w * i as f32 / bins as f32;
            let x1 = r.x + r.w * (i + 1) as f32 / bins as f32;
            let y = panel.y.px(f64::from(c)).unwrap_or(r.bottom());
            path.line_to(x0, y).line_to(x1, y);
        }
        path.line_to(r.right(), r.bottom()).close();
        fig.shape(
            path,
            Some(theme.ink.alpha(0.18)),
            Some(openreadout_figure::Stroke::new(theme.ink, theme.hairline())),
            Some(r),
        );
    }
    let caption = format!(
        "{} events{}",
        thousands(n),
        y_name.as_deref().map_or_else(String::new, |_| {
            if req.dark {
                " · brighter = more events".into()
            } else {
                " · darker = more events".into()
            }
        })
    );
    fig.text(
        fs * 0.8,
        fs * 1.4,
        &caption,
        &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
    );
    out.events = Some(EventsPreview {
        table: ti,
        x: x_name,
        y: y_name,
        events: n,
        total_events: t.row_count,
        scales,
    });
    Ok((fig, Frame::plots(vec![FramePanel::new(panel)])))
}
