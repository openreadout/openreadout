//! Getting a panel's series ready to draw: time units, histogram bins, stacking, categories,
//! axis ranges and labels, and colours.

use openreadout_core::Result;
use openreadout_figure::{Color, Domain, ScaleKind, Theme};

use crate::load::{seconds_in, time_unit, usage};
use crate::model::{Extent, Geom, Quantity, Series, Weight};
use crate::spec::{AxisSpec, MarkKind, PanelSpec};

/// An axis ready to draw.
#[derive(Debug, Clone)]
pub(crate) struct AxisRes {
    pub domain: Domain,
    pub label: String,
    pub unit: Option<String>,
    /// `linear`, `log`, `category` or a flow transform.
    pub scale: String,
    /// Ticks of a transformed axis.
    pub ticks: Option<Vec<(f64, String)>>,
    /// Category names (categorical x).
    pub cats: Vec<String>,
}

impl AxisRes {
    pub fn is_cat(&self) -> bool {
        !self.cats.is_empty()
    }
}

fn continuous(m: MarkKind) -> bool {
    matches!(m, MarkKind::Line | MarkKind::Area | MarkKind::Sticks)
}

/// Convert time axes to the unit shown, bin histograms, stack, and work out both axes.
pub(crate) fn prepare(series: &mut [Series], spec: &PanelSpec) -> Result<(AxisRes, AxisRes)> {
    let xs = spec.x.clone().unwrap_or_default();
    let ys = spec.y.clone().unwrap_or_default();
    // time unit
    if !series.is_empty() && series.iter().all(|s| s.x.is_time()) {
        let mut e = Extent::default();
        for s in series.iter() {
            e.merge(&s.geom.extent());
        }
        let span = if e.has_x() {
            e.x.1 - e.x.0.min(0.0)
        } else {
            1.0
        };
        let (unit, k) = if let Some(u) = xs.unit.as_deref() {
            (
                u.to_string(),
                seconds_in(u).ok_or_else(|| usage(format!("x unit `{u}`: s, min, h or ms")))?,
            )
        } else {
            let (u, k) = time_unit(span);
            (u.to_string(), k)
        };
        for s in series.iter_mut() {
            s.geom.scale_x(1.0 / k);
            for c in &mut s.callouts {
                c.x /= k;
                if c.auto {
                    c.text = format!("{} {unit}", short_num(c.x));
                }
            }
            s.x.unit = Some(unit.clone());
        }
    }
    bin_histograms(series, &xs);
    if let Some(f) = spec.stack.filter(|f| f.is_finite() && *f > 0.0) {
        let tallest = series
            .iter()
            .map(|s| {
                let e = s.geom.extent();
                if e.has_y() { e.y.1 - e.y.0 } else { 0.0 }
            })
            .fold(0.0f64, f64::max);
        for (i, s) in series.iter_mut().enumerate() {
            let base = s.geom.extent().y.0;
            let d = i as f64 * f * tallest - if base.is_finite() { base } else { 0.0 };
            s.geom.shift_y(d);
            for c in &mut s.callouts {
                c.y += d;
            }
        }
    }
    // curves rising out of a baseline of noise around zero, on a log axis: show each from
    // where it last leaves the noise, on an axis that starts there
    let floor = if (ys.log || series.iter().any(|s| s.y_log)) && ys.range.is_none() {
        let top = series
            .iter()
            .map(|s| s.geom.extent().y.1)
            .filter(|v| v.is_finite())
            .fold(f64::NEG_INFINITY, f64::max);
        log_floor(series).filter(|f| *f < top)
    } else {
        None
    };
    if let Some(f) = floor {
        hide_below(series, f);
    }
    // categories
    let mut cats: Vec<String> = Vec::new();
    for s in series.iter() {
        if let Geom::Bars { cats: c, .. } = &s.geom {
            for x in c {
                if !cats.contains(x) {
                    cats.push(x.clone());
                }
            }
        }
    }
    let (ext, cont) = data_extent(series.iter());
    let first = series.first();
    let x_ticks = series.iter().find_map(|s| s.x_ticks.clone());
    let x_log = xs.log || series.iter().any(|s| s.x_log);
    let x_rev = xs
        .reverse
        .unwrap_or_else(|| series.iter().any(|s| s.x_reverse));
    let x_label = xs
        .label
        .clone()
        .unwrap_or_else(|| label_of(series.iter().map(|s| &s.x)));
    let ax = if cats.is_empty() {
        let mut xr = ext.x;
        if xs.range.is_none() && !series.is_empty() && series.iter().all(|s| s.x_focus.is_some()) {
            let f = series
                .iter()
                .filter_map(|s| s.x_focus)
                .fold((f64::INFINITY, f64::NEG_INFINITY), |a, f| {
                    (a.0.min(f.0), a.1.max(f.1))
                });
            xr = (f.0.max(ext.x.0), f.1.min(ext.x.1));
            if xr.0 >= xr.1 {
                xr = ext.x;
            }
        }
        let mut d = axis_domain(&xs, xr, ext.x_pos, x_log, !cont && x_ticks.is_none(), false);
        if x_rev {
            d = d.reversed();
        }
        AxisRes {
            domain: d,
            label: x_label,
            unit: first.and_then(|s| s.x.unit.clone()),
            scale: scale_name(x_log, x_ticks.is_some(), first.map(|s| &s.x)),
            ticks: x_ticks,
            cats: Vec::new(),
        }
    } else {
        AxisRes {
            domain: Domain::linear(-0.6, cats.len() as f64 - 0.4),
            label: xs.label.clone().unwrap_or_else(|| {
                let l = label_of(series.iter().map(|s| &s.x));
                if matches!(
                    l.as_str(),
                    "well" | "group" | "x" | "name" | "population" | "sample"
                ) {
                    String::new()
                } else {
                    l
                }
            }),
            unit: None,
            scale: "category".into(),
            ticks: None,
            cats,
        }
    };
    let refs: Vec<&Series> = series.iter().collect();
    let ay = y_axis(&refs, spec, &ax, floor);
    repick_peaks(series, &ax, &ay);
    Ok((ax, ay))
}

/// The y axis of `series`, drawn under the x axis `ax`.
fn y_axis(series: &[&Series], spec: &PanelSpec, ax: &AxisRes, floor: Option<f64>) -> AxisRes {
    let xs = spec.x.clone().unwrap_or_default();
    let ys = spec.y.clone().unwrap_or_default();
    let (ext, _) = data_extent(series.iter().copied());
    let first = series.first();
    let y_ticks = series.iter().find_map(|s| s.y_ticks.clone());
    let y_label = ys
        .label
        .clone()
        .unwrap_or_else(|| label_of(series.iter().map(|s| &s.y)));
    // y: from zero for bars, areas, sticks and counts; headroom for labels
    let y_log = ys.log || series.iter().any(|s| s.y_log);
    let zero_based = series.iter().any(|s| {
        matches!(s.mark, MarkKind::Bars | MarkKind::Area | MarkKind::Sticks)
            || matches!(s.geom, Geom::Bars { .. })
    });
    let mut yr = ext.y;
    // an x range (a zoom) fits y to what it shows
    if let (Some([a, b]), None, true) = (xs.range, ys.range, ax.cats.is_empty()) {
        let (lo, hi) = (a.min(b), a.max(b));
        let mut shown = Extent::default();
        for s in series {
            shown.merge(&s.geom.extent_in(lo, hi));
            for c in s.callouts.iter().filter(|c| c.x >= lo && c.x <= hi) {
                shown.add(f64::NAN, c.y);
            }
        }
        if shown.y.0 <= shown.y.1 {
            yr = shown.y;
        }
    }
    if let Some(f) = floor {
        yr.0 = f;
    }
    if !y_log && yr.0 <= yr.1 && y_ticks.is_none() {
        let near_zero = yr.0 >= 0.0 && yr.0 < 0.35 * yr.1;
        if zero_based || near_zero {
            yr.0 = yr.0.min(0.0);
        }
        let span = (yr.1 - yr.0).max(f64::MIN_POSITIVE);
        // room above for labels on the highest points
        let labelled = series.iter().any(|s| {
            s.callouts
                .iter()
                .any(|c| !c.text.is_empty() && c.y > yr.1 - 0.12 * span)
        });
        if labelled {
            yr.1 += span * 0.1;
        }
    }
    let stacked = spec.stack.is_some_and(|f| f > 0.0);
    let y_ticks = if stacked { Some(Vec::new()) } else { y_ticks };
    let y_label = if stacked && ys.label.is_none() {
        format!("{y_label}, offset")
    } else {
        y_label
    };
    // bars and areas on a log axis grow from the decade below the smallest value, so the
    // shortest bar still shows
    let y_pos = if y_log && zero_based && ext.y_pos.is_finite() {
        10f64.powf((ext.y_pos * 0.99).log10().floor())
    } else {
        ext.y_pos
    };
    AxisRes {
        domain: axis_domain(&ys, yr, y_pos, y_log, y_ticks.is_none(), true),
        label: y_label,
        unit: first.and_then(|s| s.y.unit.clone()),
        scale: if stacked {
            "linear".into()
        } else {
            scale_name(y_log, y_ticks.is_some(), first.map(|s| &s.y))
        },
        ticks: y_ticks,
        cats: Vec::new(),
    }
}

/// The y axis of one small multiple that does not share the panel's y range.
pub(crate) fn facet_y(members: &[&Series], spec: &PanelSpec, ax: &AxisRes) -> AxisRes {
    y_axis(members, spec, ax, None)
}

/// Whether the small multiples sit at levels further apart than the tallest of them is high
/// (EEG channels with their own DC offsets): on one shared y range each would draw as a flat
/// line, so each gets its own. A y range or a log y axis keeps them shared.
pub(crate) fn facets_apart(groups: &[Vec<&Series>], spec: &PanelSpec) -> bool {
    let ys = spec.y.clone().unwrap_or_default();
    if groups.len() < 2 || ys.range.is_some() || ys.log || spec.stack.is_some() {
        return false;
    }
    let mut mids = Vec::new();
    let mut tallest = 0.0f64;
    for g in groups {
        if g.iter().any(|s| s.y_log || s.y_ticks.is_some()) {
            return false;
        }
        let (e, _) = data_extent(g.iter().copied());
        if !e.has_y() {
            continue;
        }
        mids.push(f64::midpoint(e.y.0, e.y.1));
        tallest = tallest.max(e.y.1 - e.y.0);
    }
    let lo = mids.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = mids.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    mids.len() > 1 && hi - lo > tallest
}

/// The extent of the series' data, bars (from zero) and callouts, and whether every series is
/// drawn as a continuous line, area or event cloud.
fn data_extent<'a>(series: impl Iterator<Item = &'a Series>) -> (Extent, bool) {
    let mut ext = Extent::default();
    let mut cont = true;
    for s in series {
        if let Geom::Bars { vals, err, .. } = &s.geom {
            // every bar, so that a log axis starts below the shortest one
            ext.add(f64::NAN, 0.0);
            for (i, &v) in vals.iter().enumerate() {
                let d = err.get(i).copied().filter(|d| d.is_finite()).unwrap_or(0.0);
                ext.add(f64::NAN, v - d);
                ext.add(f64::NAN, v);
                ext.add(f64::NAN, v + d);
            }
        } else {
            ext.merge(&s.geom.extent());
            cont &= continuous(s.mark) || matches!(s.geom, Geom::Events { .. });
        }
        for c in &s.callouts {
            ext.add(f64::NAN, c.y);
        }
    }
    (ext, cont)
}

/// The noise floor of lines that start from a baseline at zero (baseline-subtracted
/// amplification curves): five noise standard deviations. Each line's noise comes from the
/// steps between its first 15 % of points (a robust spread, so a slow drift counts little); the
/// lines whose early points sit within three noise SDs of zero are the ones that start there,
/// and the lower quartile of their noise sets the floor. `None` when no line starts at zero,
/// where a log axis shows the baselines well.
pub(crate) fn log_floor(series: &[Series]) -> Option<f64> {
    let mut noises = Vec::new();
    for s in series {
        let Geom::Line(pts) = &s.geom else { continue };
        // a baseline needs at least four points in the first 15 %: a short series (the eleven
        // frequencies of a rheometer sweep) has no noise to tell apart from its data
        let k = pts.len() * 15 / 100;
        if k < 4 {
            continue;
        }
        let early: Vec<f64> = pts[..k]
            .iter()
            .map(|p| p.1)
            .filter(|v| v.is_finite())
            .collect();
        if early.len() < 4 {
            continue;
        }
        let mut sorted = early.clone();
        sorted.sort_by(f64::total_cmp);
        let level = sorted[sorted.len() / 2];
        let mut steps: Vec<f64> = early.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        steps.sort_by(f64::total_cmp);
        let noise = 1.4826 * steps[steps.len() / 2] / std::f64::consts::SQRT_2;
        if noise > 0.0 && level.abs() < 3.0 * noise {
            noises.push(noise);
        }
    }
    if noises.is_empty() {
        return None;
    }
    noises.sort_by(f64::total_cmp);
    Some(5.0 * noises[noises.len() / 4])
}

/// Blank each line's points before it last rises above `floor`, so the noise under the floor
/// draws nothing and every curve enters the panel once. Points after the line's last one above
/// the floor stay as they are: a line that ends low keeps the rise before it.
fn hide_below(series: &mut [Series], floor: f64) {
    for s in series.iter_mut() {
        let Geom::Line(pts) = &mut s.geom else {
            continue;
        };
        let end = pts.iter().rposition(|p| p.1 >= floor).unwrap_or(pts.len());
        if let Some(last_low) = pts[..end].iter().rposition(|p| p.1.is_nan() || p.1 < floor) {
            for p in pts.iter_mut().take(last_low) {
                p.1 = f64::NAN;
            }
            // the line starts where it crosses the floor, on the panel's bottom edge
            let next = pts.get(last_low + 1).copied();
            let p = &mut pts[last_low];
            *p = match next {
                Some((x1, y1)) if p.1.is_finite() && y1 > p.1 => {
                    (p.0 + (floor - p.1) / (y1 - p.1) * (x1 - p.0), floor)
                }
                _ => (p.0, f64::NAN),
            };
        }
    }
}

/// Peaks are picked again among the points the axes show: a zoomed window or a clipped y range
/// (a solvent peak cut off) labels the peaks the reader can see.
fn repick_peaks(series: &mut [Series], ax: &AxisRes, ay: &AxisRes) {
    let (x0, x1) = (
        ax.domain.lo.min(ax.domain.hi),
        ax.domain.lo.max(ax.domain.hi),
    );
    let ytop = ay.domain.lo.max(ay.domain.hi);
    for s in series.iter_mut() {
        if s.peaks == 0 || ax.is_cat() {
            continue;
        }
        let Geom::Line(p) = &s.geom else { continue };
        let vis: Vec<(f64, f64)> = p
            .iter()
            .copied()
            .filter(|q| q.0 >= x0 && q.0 <= x1 && q.1.is_finite())
            .collect();
        let fully = vis.iter().all(|q| q.1 <= ytop) && vis.len() == p.len();
        if fully || vis.len() < 3 {
            continue;
        }
        // clipped points become a plateau at the top, which is no peak
        let vis: Vec<(f64, f64)> = vis.into_iter().map(|q| (q.0, q.1.min(ytop))).collect();
        let span = x1 - x0;
        let picked: Vec<(f64, f64)> = if s.mark == MarkKind::Sticks {
            let base = vis.iter().map(|q| q.1).fold(0.0f64, f64::max);
            let mut v: Vec<(f64, f64)> =
                vis.iter().copied().filter(|q| q.1 >= 0.02 * base).collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            let mut out: Vec<(f64, f64)> = Vec::new();
            for c in v {
                if out.iter().all(|o| (o.0 - c.0).abs() >= span / 80.0) {
                    out.push(c);
                    if out.len() == s.peaks {
                        break;
                    }
                }
            }
            out
        } else {
            crate::load::top_peaks(&vis, s.peaks * 2, span / 60.0)
                .into_iter()
                .filter(|q| q.1 < ytop * (1.0 - 1e-9))
                .take(s.peaks)
                .collect()
        };
        let time = s.x.is_time() || s.x.name.contains("time");
        let unit = s.x.unit.clone().unwrap_or_default();
        s.callouts = picked
            .into_iter()
            .map(|(x, y)| crate::model::Callout {
                x,
                y,
                text: if time {
                    format!("{} {unit}", short_num(x))
                } else {
                    crate::load::fmt_at(&s.x, x)
                },
                auto: false,
            })
            .collect();
    }
}

/// A position as a label: up to three decimals, trailing zeros dropped.
fn short_num(v: f64) -> String {
    let s = if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else if v.abs() >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    };
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn scale_name(log: bool, transformed: bool, q: Option<&Quantity>) -> String {
    if transformed {
        let _ = q;
        "logicle".into()
    } else if log {
        "log".into()
    } else {
        "linear".into()
    }
}

/// One label for several series: their common quantity, or the first two joined.
fn label_of<'a>(qs: impl Iterator<Item = &'a Quantity>) -> String {
    let mut names: Vec<String> = Vec::new();
    for q in qs {
        let l = q.label();
        if !names.contains(&l) {
            names.push(l);
        }
    }
    match names.len() {
        0 => String::new(),
        1 | 2 => names.join(" / "),
        _ => format!("{} / …", names[0]),
    }
}

fn axis_domain(a: &AxisSpec, r: (f64, f64), pos: f64, log: bool, nice: bool, pad: bool) -> Domain {
    if let Some([lo, hi]) = a.range {
        return if log {
            Domain::log(lo.min(hi), lo.max(hi))
        } else {
            Domain::linear(lo.min(hi), lo.max(hi))
        };
    }
    if log {
        let lo = if r.0 > 0.0 { r.0 } else { pos };
        // a log y axis shows at most four decades below the top: noise near zero stays out
        let lo = if pad { lo.max(r.1 * 1e-4) } else { lo };
        return Domain::log(lo, r.1);
    }
    let (lo, hi) = if r.0 <= r.1 { r } else { (0.0, 1.0) };
    if nice {
        let d = Domain::of([lo, hi], if pad { 0.03 } else { 0.05 }, ScaleKind::Linear);
        // keep a zero baseline at zero
        let d = if lo == 0.0 {
            Domain::linear(0.0, d.hi)
        } else {
            d
        };
        let n = d.nice(5);
        if lo < 0.0 && hi > 0.0 && -lo < 0.06 * hi {
            // a little noise below zero: no whole tick step of empty space for it
            Domain::linear(lo - 0.01 * (hi - lo), n.hi)
        } else {
            n
        }
    } else if pad {
        let p = (hi - lo) * 0.03;
        Domain::linear(lo - if lo == 0.0 { 0.0 } else { p }, hi + p)
    } else {
        Domain::linear(lo, hi)
    }
}

/// Histograms of a panel share their bins; with several series, counts become percentages.
/// Bins are equal on the axis as drawn: on a log x axis they are equal steps of log10(x), and
/// values at or below zero, which a log axis cannot show, fall in no bin.
fn bin_histograms(series: &mut [Series], xs: &AxisSpec) {
    let hist: Vec<usize> = series
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.geom, Geom::Hist(_)))
        .map(|(i, _)| i)
        .collect();
    if hist.is_empty() {
        return;
    }
    let log = (xs.log || series.iter().any(|s| s.x_log))
        && hist.iter().all(|&i| series[i].x_ticks.is_none());
    // positions on the axis as drawn, and back
    let fwd = |x: f64| if log { x.log10() } else { x };
    let back = |t: f64| if log { 10f64.powf(t) } else { t };
    let shown = |x: f64| x.is_finite() && (!log || x > 0.0);
    let mut all: Vec<f64> = Vec::new();
    for &i in &hist {
        if let Geom::Hist(v) = &series[i].geom {
            all.extend(v.iter().copied().filter(|&x| shown(x)).map(fwd));
        }
    }
    all.sort_by(f64::total_cmp);
    let q = |p: f64| {
        all.get(((all.len() as f64 - 1.0) * p).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    let (lo, hi) = match xs.range {
        Some([a, b]) if !log || a.min(b) > 0.0 => (fwd(a.min(b)), fwd(a.max(b))),
        _ => (q(0.0005), q(0.9995)),
    };
    let (lo, hi) = if hi > lo {
        (lo, hi)
    } else {
        (lo - 0.5, lo + 0.5)
    };
    let bins = ((all.len() as f64 / hist.len() as f64).sqrt() as usize).clamp(24, 160);
    let w = (hi - lo) / bins as f64;
    let percent = hist.len() > 1;
    for &i in &hist {
        let s = &mut series[i];
        let Geom::Hist(v) = &s.geom else { continue };
        let mut counts = vec![0f64; bins];
        let mut n = 0usize;
        for &x in v {
            if x.is_finite() {
                n += 1;
                if !shown(x) {
                    continue;
                }
                let t = fwd(x);
                if t >= lo && t <= hi {
                    let k = (((t - lo) / w) as usize).min(bins - 1);
                    counts[k] += 1.0;
                }
            }
        }
        if percent && n > 0 {
            for c in &mut counts {
                *c *= 100.0 / n as f64;
            }
        }
        let mut pts = Vec::with_capacity(bins * 2 + 2);
        pts.push((back(lo), 0.0));
        for (k, c) in counts.iter().enumerate() {
            pts.push((back(lo + k as f64 * w), *c));
            pts.push((back(lo + (k + 1) as f64 * w), *c));
        }
        pts.push((back(hi), 0.0));
        s.geom = Geom::Line(pts);
        s.mark = MarkKind::Area;
        s.y = Quantity::new(
            if percent {
                "% of events"
            } else {
                s.y.name.as_str()
            },
            None,
        );
        if s.y.name.is_empty() {
            s.y.name = "count".into();
        }
    }
}

/// A named colour.
pub(crate) fn named_color(theme: &Theme, name: &str) -> Option<Color> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "ink" | "black" => theme.ink,
        "accent" | "red" | "warm" => theme.accent,
        "muted" | "gray" | "grey" => theme.muted,
        "faint" | "light" => theme.faint,
        "blue" => theme.series(2),
        "ochre" | "yellow" | "orange" => theme.series(3),
        "green" => theme.series(4),
        "purple" => theme.series(5),
        "teal" => theme.series(6),
        "brown" => theme.series(7),
        other => return Color::parse(other),
    })
}

/// Give every series its colour: asked-for colours stay; colour classes share one (two classes:
/// ink and accent); blanks and context are faint; the rest take the series palette in order.
pub(crate) fn assign_colors(
    theme: &Theme,
    series: &mut [Series],
    faceted: bool,
) -> Vec<(String, Color)> {
    let mut classes: Vec<(String, Color)> = Vec::new();
    for s in series.iter() {
        if let (Some(c), Some(col)) = (&s.class, s.color)
            && !classes.iter().any(|k| &k.0 == c)
        {
            classes.push((c.clone(), col));
        }
    }
    let mut next = 0usize;
    let mut take = |classes: &[(String, Color)]| loop {
        let c = theme.series(next);
        next += 1;
        if !classes.iter().any(|k| k.1 == c) || next > 64 {
            return c;
        }
    };
    for s in series.iter_mut() {
        if s.color.is_some() {
            continue;
        }
        if let Some(i) = s.palette {
            s.color = Some(theme.series(i));
            continue;
        }
        if let Some(c) = &s.class {
            if c == "blank" {
                s.color = Some(theme.faint);
                continue;
            }
            if let Some(k) = classes.iter().find(|k| &k.0 == c) {
                s.color = Some(k.1);
            } else {
                let col = take(&classes);
                classes.push((c.clone(), col));
                s.color = Some(col);
            }
        } else if s.weight == Weight::Context {
            s.color = Some(theme.faint);
        } else if s.weight == Weight::Many || (faceted && s.weight == Weight::Main) {
            s.color = Some(theme.ink);
        } else {
            s.color = Some(take(&classes));
        }
    }
    classes
}
