//! Laying out and drawing a figure: panels, small multiples, plate grids, axes, legends,
//! callouts and annotations, in the OpenReadout look.

mod marks;
mod prep;

use openreadout_core::Result;
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Color, Domain, EndLabel, Face, Figure, GridLines, LabelPlacer,
    Marker, Panel, Path, Rect, Stroke, TextStyle, Theme, arrow_px, end_labels, format_tick,
    key_row, spread,
};
use serde_json::{Value, json};

use crate::load::PlateHints;
use crate::model::{Geom, Series, Weight};
use crate::output::{AxisOutput, Marked, PanelOutput, SeriesOutput};
use crate::spec::{Annotation, Facet, Legend, MarkKind, PanelSpec};
use marks::{Ink, draw_series, events_layer};
pub(crate) use prep::{AxisRes, named_color, prepare as prepare_axes};

/// A panel's data, read and ready to lay out.
pub(crate) struct PanelIn {
    pub title: Option<String>,
    pub spec: PanelSpec,
    pub series: Vec<Series>,
    pub plate: Option<PlateHints>,
}

/// What drawing a panel returns: its summary and the data the viewer needs for hover.
pub(crate) struct PanelDrawn {
    pub summary: PanelOutput,
    pub hover: Vec<Value>,
}

/// The kind of layout a panel takes.
pub(crate) fn layout_of(p: &PanelIn) -> &'static str {
    match p.spec.facet {
        Some(Facet::Plate) => "plate",
        Some(Facet::Series) => "facets",
        None => "plot",
    }
}

/// Facet keys in order of first appearance.
fn facet_keys(series: &[Series]) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for s in series {
        let k = s.facet.clone().unwrap_or_else(|| s.name.clone());
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys
}

/// Rows and columns of a grid of `n` small multiples `width` px wide: cells about 380 px wide.
fn grid_shape(n: usize, cols: Option<u32>, width: f32) -> (usize, usize) {
    let n = n.max(1);
    let c = match cols {
        Some(c) if c > 0 => (c as usize).min(n),
        _ => {
            let c = ((width / 380.0).round() as usize).clamp(1, n.min(6));
            let rows = n.div_ceil(c);
            n.div_ceil(rows)
        }
    };
    (n.div_ceil(c), c)
}

/// Height of one small multiple `cell_w` px wide.
fn cell_height(cell_w: f32, cols: usize) -> f32 {
    if cols == 1 {
        (cell_w * 0.26).clamp(150.0, 300.0)
    } else {
        (cell_w * 0.62).clamp(120.0, 340.0)
    }
}

/// The height a panel would like at `width`.
pub(crate) fn preferred_height(p: &PanelIn, width: f32, columns: Option<u32>, fs: f32) -> f32 {
    match p.spec.facet {
        Some(Facet::Plate) => {
            let (r, c) = p.plate.as_ref().map_or((8, 12), |h| (h.rows.max(1), h.cols.max(1)));
            let blocks = p.plate.as_ref().map_or(1, |h| {
                h.row_groups.windows(2).filter(|w| w[0] != w[1]).count() + 1
            });
            let left = fs * 6.0;
            let gap = ((width - left) / c as f32 * 0.07).clamp(3.0, fs * 0.6);
            let cw = (width - left - gap * (c as f32 - 1.0)) / c as f32;
            r as f32 * (cw * 0.78 + gap) + fs * (2.0 + 3.6) + (blocks.min(r as usize) as f32 - 1.0) * fs * 0.9
        }
        Some(Facet::Series) => {
            let n = facet_keys(&p.series).len();
            let (rows, cols) = grid_shape(n, columns, width);
            let cell_w = width / cols as f32;
            rows as f32 * (cell_height(cell_w, cols) + fs * 2.2) + fs * 3.0
        }
        None => {
            if p.series.iter().any(|s| matches!(s.geom, Geom::Events { .. })) {
                width.min(900.0) * 0.82
            } else if width < 760.0 {
                width * 0.82
            } else {
                width * 0.56
            }
        }
    }
}

fn axis_out(a: &AxisRes) -> AxisOutput {
    let d = a.domain;
    AxisOutput {
        label: a.label.clone(),
        unit: a.unit.clone(),
        range: if a.is_cat() {
            None
        } else if d.reversed {
            Some([tidy(d.hi), tidy(d.lo)])
        } else {
            Some([tidy(d.lo), tidy(d.hi)])
        },
        scale: a.scale.clone(),
        categories: a.cats.clone(),
    }
}

/// Round to 6 significant digits for the summary.
pub(crate) fn tidy(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let d = 5 - v.abs().log10().floor() as i32;
    if !(-300..=300).contains(&d) {
        return v;
    }
    let m = 10f64.powi(d);
    (v * m).round() / m
}

fn mark_name(s: &Series) -> &'static str {
    match (&s.geom, s.mark) {
        (Geom::Band(_), _) => "band",
        (Geom::Events { .. }, _) => "events",
        (Geom::Bars { .. }, _) | (_, MarkKind::Bars) => "bars",
        (_, MarkKind::Sticks) => "sticks",
        (_, MarkKind::Area) => "area",
        (_, MarkKind::Points) => "points",
        (_, MarkKind::LinePoints) => "line+points",
        _ => "line",
    }
}

fn series_out(s: &Series) -> SeriesOutput {
    let e = s.geom.extent();
    SeriesOutput {
        name: s.name.clone(),
        file: s.source.file.clone(),
        data: s.source.selector.clone(),
        mark: mark_name(s).into(),
        points: s.geom.len(),
        x_range: (e.has_x() && !matches!(s.geom, Geom::Bars { .. })).then(|| [tidy(e.x.0), tidy(e.x.1)]),
        y_range: e.has_y().then(|| [tidy(e.y.0), tidy(e.y.1)]),
        count: None,
        class: s.class.clone(),
        color: s.color.map_or_else(String::new, Color::to_hex),
        marked: s
            .callouts
            .iter()
            .map(|c| Marked { x: tidy(c.x), y: tidy(c.y), text: c.text.clone() })
            .collect(),
    }
}

/// The series list of a summary: many wells or curves of one class become one entry.
fn summarize_series(series: &[Series]) -> Vec<SeriesOutput> {
    if series.len() <= 16 {
        return series.iter().map(series_out).collect();
    }
    let mut out: Vec<SeriesOutput> = Vec::new();
    let mut members: Vec<Vec<String>> = Vec::new();
    for s in series {
        let key = s.class.clone().unwrap_or_else(|| "series".into());
        let o = series_out(s);
        if let Some(i) = out.iter().position(|x| x.class.as_deref().unwrap_or("series") == key) {
            let a = &mut out[i];
            a.points += o.points;
            a.count = Some(a.count.unwrap_or(1) + 1);
            a.x_range = merge_range(a.x_range, o.x_range);
            a.y_range = merge_range(a.y_range, o.y_range);
            a.marked.extend(o.marked);
            members[i].push(s.name.clone());
        } else {
            let mut o = o;
            o.count = Some(1);
            o.class = Some(key.clone());
            o.name.clone_from(&key);
            out.push(o);
            members.push(vec![s.name.clone()]);
        }
    }
    for (o, m) in out.iter_mut().zip(&members) {
        let list = if m.len() > 12 {
            format!("{}, … {} ({} series)", m[..6].join(", "), m[m.len() - 1], m.len())
        } else {
            m.join(", ")
        };
        o.data = format!("{}; {list}", o.data);
        o.marked.truncate(40);
    }
    out
}

fn merge_range(a: Option<[f64; 2]>, b: Option<[f64; 2]>) -> Option<[f64; 2]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([a[0].min(b[0]), a[1].max(b[1])]),
        (a, b) => a.or(b),
    }
}

/// Draw one panel into `area`.
pub(crate) fn draw_panel(fig: &mut Figure, theme: &Theme, area: Rect, mut p: PanelIn, columns: Option<u32>) -> Result<PanelDrawn> {
    let (ax, ay) = prep::prepare(&mut p.series, &p.spec)?;
    let classes = prep::assign_colors(theme, &mut p.series, p.spec.facet.is_some());
    let mut area = area;
    if let Some(t) = &p.title {
        let st = TextStyle::new(theme.font_size * 1.05, theme.ink).face(Face::SansMedium);
        fig.text(area.x, area.y + theme.font_size * 0.95, t, &st);
        area = area.inset_sides(0.0, theme.font_size * 1.7, 0.0, 0.0);
    }
    let layout = layout_of(&p);
    let mut hover = Vec::new();
    let mut ann = Vec::new();
    let facets = match p.spec.facet {
        Some(Facet::Plate) => Some(draw_plate(fig, theme, area, &p, &ax, &ay, &classes, &mut hover, &mut ann)),
        Some(Facet::Series) => Some(draw_facets(fig, theme, area, &p, &ax, &ay, &classes, columns, &mut hover, &mut ann)),
        None => {
            draw_plot(fig, theme, area, &p, &ax, &ay, &classes, &mut hover, &mut ann);
            None
        }
    };
    Ok(PanelDrawn {
        summary: PanelOutput {
            title: p.title.clone(),
            layout: layout.into(),
            facets,
            x: axis_out(&ax),
            y: axis_out(&ay),
            series: summarize_series(&p.series),
            annotations: ann,
        },
        hover,
    })
}

/// Legend entries: one per class, else one per series (context series left out).
fn legend_entries(series: &[Series]) -> Vec<(String, Color, bool)> {
    let mut out: Vec<(String, Color, bool)> = Vec::new();
    for s in series {
        if s.weight == Weight::Context && s.class.is_none() {
            continue;
        }
        let name = s.class.clone().unwrap_or_else(|| s.name.clone());
        if name.is_empty() {
            continue;
        }
        let block = matches!(s.geom, Geom::Bars { .. } | Geom::Events { .. }) || s.mark == MarkKind::Bars;
        if !out.iter().any(|e| e.0 == name) {
            out.push((name, s.color.unwrap_or(Color::rgb(0, 0, 0)), block));
        }
    }
    out
}

fn axes_spec(ax: &AxisRes, ay: &AxisRes, right: f32, top: f32) -> AxesSpec {
    AxesSpec {
        x_label: (!ax.label.is_empty()).then(|| ax.label.clone()),
        y_label: (!ay.label.is_empty()).then(|| ay.label.clone()),
        x_ticks: 6,
        y_ticks: 5,
        show_x: true,
        show_y: true,
        grid: GridLines::Y,
        right,
        top,
        fill: false,
    }
}

/// Axes, with category names or a transform's ticks where the panel needs them.
fn draw_axes(fig: &mut Figure, theme: &Theme, p: &Panel, ax: &AxisRes, ay: &AxisRes, spec: &AxesSpec) {
    let custom_x = ax.is_cat() || ax.ticks.is_some();
    let custom_y = ay.ticks.is_some();
    let mut s = spec.clone();
    if custom_x {
        s.show_x = false;
    }
    if custom_y {
        s.show_y = false;
        s.grid = GridLines::None;
    }
    p.axes(fig, theme, &s);
    let fs = theme.font_size;
    let hair = theme.hairline();
    let tick = (fs * 0.4).round();
    let label = TextStyle::new(fs, theme.muted);
    let r = p.rect;
    if custom_x && spec.show_x {
        let axis = Stroke::new(theme.rule, hair);
        fig.line(r.x, r.bottom(), r.right(), r.bottom(), axis.clone());
        let ticks: Vec<(f64, String)> = if ax.is_cat() {
            ax.cats.iter().enumerate().map(|(i, c)| (i as f64, c.clone())).collect()
        } else {
            ax.ticks.clone().unwrap_or_default()
        };
        let room = if ticks.is_empty() { r.w } else { r.w / ticks.len() as f32 };
        let ls = label.clone().align(Anchor::Middle, Baseline::Top);
        let widest = ticks.iter().map(|t| ls.width(&t.1)).fold(0.0f32, f32::max);
        let rotate = ax.is_cat() && widest > room * 0.95;
        for (v, t) in &ticks {
            let Some(px) = p.x.px(*v) else { continue };
            if px < r.x - 1.0 || px > r.right() + 1.0 {
                continue;
            }
            fig.line(px, r.bottom(), px, r.bottom() + tick, axis.clone());
            if rotate {
                let st = label.clone().align(Anchor::End, Baseline::Middle).rotate(-35.0);
                fig.text(px, r.bottom() + tick + fs * 0.4, &shorten(t, 22), &st);
            } else {
                fig.text(px, r.bottom() + tick + fs * 0.3, t, &ls);
            }
        }
        if let Some(xl) = &spec.x_label {
            let y = r.bottom() + tick + fs * 0.3 + fs * 1.1 + fs * 1.05;
            if !rotate {
                fig.text(r.right(), y, xl, &label.clone().align(Anchor::End, Baseline::Alphabetic));
            }
        }
    }
    if custom_y && spec.show_y {
        let grid = Stroke::new(theme.grid, hair);
        let ls = label.clone().align(Anchor::End, Baseline::Middle);
        for (v, t) in ay.ticks.clone().unwrap_or_default() {
            let Some(py) = p.y.px(v) else { continue };
            if py < r.y - 1.0 || py > r.bottom() + 1.0 {
                continue;
            }
            fig.line(r.x, py, r.right(), py, grid.clone());
            fig.text(r.x - fs * 0.5, py, &t, &ls);
        }
        if let Some(yl) = &spec.y_label {
            fig.text((r.x - fs * 2.5).max(2.0), r.y - fs * 0.75, yl, &label.clone().align(Anchor::Start, Baseline::Alphabetic));
        }
    }
    if ax.ticks.is_some() && spec.show_x {
        // the grid of a transformed x axis
        let grid = Stroke::new(theme.grid, hair);
        for (v, _) in ax.ticks.clone().unwrap_or_default() {
            if let Some(px) = p.x.px(v)
                && px > r.x + 1.0
                && px < r.right() - 1.0
            {
                fig.line(px, r.y, px, r.bottom(), grid.clone());
            }
        }
    }
}

fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// Order: context first, then the many, then the main data and fits.
fn draw_order(series: &[Series]) -> Vec<usize> {
    let rank = |s: &Series| match s.weight {
        Weight::Context => 0,
        Weight::Many => 1,
        Weight::Main => 2,
        Weight::Fit => 2,
    };
    let mut idx: Vec<usize> = (0..series.len()).collect();
    idx.sort_by_key(|&i| rank(&series[i]));
    idx
}

/// All the marks of a panel.
fn draw_data(fig: &mut Figure, theme: &Theme, p: &Panel, series: &[&Series], cats: &[String], ink: Ink) {
    let events: Vec<&Series> = series.iter().copied().filter(|s| matches!(s.geom, Geom::Events { .. })).collect();
    if !events.is_empty() {
        events_layer(fig, p, theme, &events, if ink.small { 1.0 } else { 0.5 });
    }
    let n_many = series.iter().filter(|s| s.weight == Weight::Many).count();
    let mut ink = ink;
    ink.fills = series
        .iter()
        .filter(|s| matches!(s.geom, Geom::Band(_)) || s.mark == MarkKind::Area)
        .count();
    let bars: Vec<usize> = series
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s.geom, Geom::Bars { .. }))
        .map(|(i, _)| i)
        .collect();
    let owned: Vec<Series> = series.iter().map(|s| (*s).clone()).collect();
    for i in draw_order(&owned) {
        let s = series[i];
        let dodge = bars.iter().position(|&b| b == i).map(|k| (k, bars.len()));
        draw_series(fig, p, theme, s, ink, n_many, dodge, cats);
    }
}

/// Markers and labels on peaks, apexes and spikes.
fn draw_callouts(fig: &mut Figure, theme: &Theme, p: &Panel, series: &[&Series], placer: &mut LabelPlacer, ink: Ink) {
    let fs = theme.small_size();
    let st = TextStyle::new(fs, theme.ink).halo(theme.paper, fs * 0.22);
    let mut taken: Vec<Rect> = Vec::new();
    // one series with callouts marks them in the accent; several, each in its own colour
    let several = series.iter().filter(|s| s.callouts.iter().any(|c| !c.text.is_empty())).count() > 1;
    for s in series {
        let col = s.color.unwrap_or(theme.ink);
        let col = if several { col } else { theme.accent };
        let many = s.callouts.len() > 30;
        for c in &s.callouts {
            let Some((px, py)) = p.pt(c.x, c.y) else { continue };
            if c.text.is_empty() && px >= p.rect.x - 1.0 && px <= p.rect.right() + 1.0 {
                let len = (p.rect.h * 0.06).clamp(4.0, theme.font_size * 0.7);
                fig.line(px, p.rect.y, px, p.rect.y + len, Stroke::new(theme.accent, theme.hairline().max(1.0)));
                continue;
            }
            if !p.rect.inset(-2.0).contains(px, py) {
                continue;
            }
            if c.text.is_empty() {
                // a spike: a tick in a raster strip along the top of the plot
                let len = (p.rect.h * 0.06).clamp(4.0, theme.font_size * 0.7);
                fig.line(px, p.rect.y, px, p.rect.y + len, Stroke::new(theme.accent, theme.hairline().max(1.0)));
                continue;
            }
            let m = Marker::dot(if many { s.color.unwrap_or(theme.ink) } else { col }, (ink.dot * 0.9).max(4.0));
            openreadout_figure::draw_marker(fig, &m, px, py);
            placer.reserve(Rect::new(px - m.size / 2.0, py - m.size / 2.0, m.size, m.size));
            let w = st.width(&c.text);
            let h = fs * 1.05;
            let above = Rect::new(px - w / 2.0, py - fs * 0.5 - h, w, h);
            let r = if taken.iter().all(|t| !t.intersects(&above)) && p.rect.contains_rect(&above) {
                placer.reserve(above);
                above
            } else {
                placer.place(px, py, w, h, fs * 0.45)
            };
            taken.push(r);
            fig.text(r.x, r.bottom() - fs * 0.22, &c.text, &st);
            // a hairline leader when the label had to move away
            let (cx, cy) = r.center();
            let dist = ((cx - px).powi(2) + (cy - py).powi(2)).sqrt();
            if dist > w * 0.5 + fs * 1.4 {
                let lx = px.clamp(r.x, r.right());
                let ly = if py < r.y { r.y } else { r.bottom() };
                fig.line(px, py, lx, ly, Stroke::new(theme.muted.alpha(0.7), theme.hairline()));
            }
        }
    }
}

fn ann_color(theme: &Theme, a: &Annotation) -> Color {
    a.color.as_deref().and_then(|c| named_color(theme, c)).unwrap_or(theme.accent)
}

fn fmt_num(v: f64) -> String {
    format_tick(v, (v.abs() / 100.0).max(1e-9))
}

/// Bands go under the data.
fn draw_bands(fig: &mut Figure, theme: &Theme, p: &Panel, anns: &[Annotation], facet: Option<&str>, notes: &mut Vec<String>) {
    for a in anns.iter().filter(|a| facet_ok(a, facet)) {
        let band_fill = a
            .color
            .as_deref()
            .and_then(|c| named_color(theme, c))
            .map_or(theme.band, |c| c.alpha(0.16));
        if let Some(crate::spec::OneOrMany::Many(v)) = &a.x
            && v.len() == 2
            && a.y.is_none()
        {
            p.band_x(fig, v[0], v[1], band_fill);
            notes.push(format!("x band {}–{}{}", fmt_num(v[0]), fmt_num(v[1]), a.label.as_ref().map(|l| format!(": {l}")).unwrap_or_default()));
        }
        if let Some(crate::spec::OneOrMany::Many(v)) = &a.y
            && v.len() == 2
            && a.x.is_none()
        {
            p.band_y(fig, v[0], v[1], band_fill);
            notes.push(format!("y band {}–{}{}", fmt_num(v[0]), fmt_num(v[1]), a.label.as_ref().map(|l| format!(": {l}")).unwrap_or_default()));
        }
    }
}

fn facet_ok(a: &Annotation, facet: Option<&str>) -> bool {
    match (&a.facet, facet) {
        (None, _) => true,
        (Some(want), Some(f)) => want.eq_ignore_ascii_case(f) || f.to_lowercase().starts_with(&format!("{} ", want.to_lowercase())),
        (Some(_), None) => true,
    }
}

/// Lines, points, arrows and the labels of bands go over the data.
fn draw_marks_over(fig: &mut Figure, theme: &Theme, p: &Panel, anns: &[Annotation], facet: Option<&str>, placer: &mut LabelPlacer, small: bool, notes: &mut Vec<String>) {
    use crate::spec::OneOrMany::{Many, One};
    let fs = if small { theme.small_size() } else { theme.font_size };
    for a in anns.iter().filter(|a| facet_ok(a, facet)) {
        let col = ann_color(theme, a);
        let st = TextStyle::new(fs, col).halo(theme.paper, fs * 0.3);
        let plain = TextStyle::new(fs, col);
        let label = a.label.clone().unwrap_or_default();
        match (&a.x, &a.y) {
            (Some(Many(v)), None) if v.len() == 2 => {
                if !label.is_empty() && !small {
                    let (Some(x0), Some(x1)) = (p.x.px(v[0]), p.x.px(v[1])) else { continue };
                    let cx = ((x0 + x1) / 2.0).clamp(p.rect.x, p.rect.right());
                    let w = st.width(&label);
                    let x = (cx - w / 2.0).clamp(p.rect.x + 2.0, p.rect.right() - w - 2.0);
                    let y = p.rect.y + fs * 1.15;
                    fig.text(x, y, &label, &plain);
                    placer.reserve(Rect::new(x, y - fs, w, fs * 1.3));
                }
            }
            (None, Some(Many(v))) if v.len() == 2 => {
                if !label.is_empty() && !small {
                    let (Some(y0), Some(y1)) = (p.y.px(v[0]), p.y.px(v[1])) else { continue };
                    let y = y0.min(y1) + fs * 1.1;
                    let w = st.width(&label);
                    fig.text(p.rect.right() - w - fs * 0.4, y, &label, &st);
                    placer.reserve(Rect::new(p.rect.right() - w - fs * 0.4, y - fs, w, fs * 1.3));
                }
            }
            (Some(One(x)), None) => {
                p.rule_x(fig, *x, Stroke::new(col.alpha(0.85), theme.hairline() * 1.2).dashed(fs * 0.35, fs * 0.3));
                notes.push(format!("x line at {}{}", fmt_num(*x), if label.is_empty() { String::new() } else { format!(": {label}") }));
                if !label.is_empty() && !small
                    && let Some(px) = p.x.px(*x)
                {
                    let w = st.width(&label);
                    let left = px + fs * 0.35 + w > p.rect.right();
                    let tx = if left { px - fs * 0.35 - w } else { px + fs * 0.35 };
                    let y = p.rect.y + fs * 1.15;
                    fig.text(tx, y, &label, &st);
                    placer.reserve(Rect::new(tx, y - fs, w, fs * 1.3));
                }
            }
            (None, Some(One(y))) => {
                p.rule_y(fig, *y, Stroke::new(col.alpha(0.85), theme.hairline() * 1.2).dashed(fs * 0.35, fs * 0.3));
                notes.push(format!("y line at {}{}", fmt_num(*y), if label.is_empty() { String::new() } else { format!(": {label}") }));
                if !label.is_empty() && !small
                    && let Some(py) = p.y.px(*y)
                {
                    let w = st.width(&label);
                    let tx = p.rect.right() - w - fs * 0.3;
                    fig.text(tx, py - fs * 0.35, &label, &st);
                    placer.reserve(Rect::new(tx, py - fs * 1.3, w, fs * 1.2));
                }
            }
            (Some(One(x)), Some(One(y))) => {
                let Some((px, py)) = p.pt(*x, *y) else { continue };
                notes.push(format!("point ({}, {}){}", fmt_num(*x), fmt_num(*y), if label.is_empty() { String::new() } else { format!(": {label}") }));
                if let Some([tx, ty]) = a.to {
                    // an arrow from the label to the target
                    let Some((qx, qy)) = p.pt(tx, ty) else { continue };
                    let w = st.width(&label);
                    let r = Rect::new(px - w / 2.0, py - fs * 0.75, w, fs * 1.05);
                    if !label.is_empty() {
                        fig.text(r.x, py + fs * 0.3, &label, &st);
                        placer.reserve(r);
                    }
                    let (sx, sy) = edge_point(r.inset(-fs * 0.3), qx, qy);
                    arrow_px(fig, sx, sy, qx, qy, Stroke::new(col, theme.hairline() * 1.4));
                    continue;
                }
                if a.marker != Some(false) {
                    let m = Marker::dot(col, fs * 0.55);
                    openreadout_figure::draw_marker(fig, &m, px, py);
                    placer.reserve(Rect::new(px - m.size / 2.0, py - m.size / 2.0, m.size, m.size));
                }
                if !label.is_empty() {
                    if a.marker == Some(false) {
                        let st2 = st.clone().align(Anchor::Middle, Baseline::Middle);
                        let b = fig.text(px, py, &label, &st2);
                        placer.reserve(b);
                    } else {
                        let w = st.width(&label);
                        let r = placer.place(px, py, w, fs * 1.05, fs * 0.5);
                        fig.text(r.x, r.bottom() - fs * 0.22, &label, &st);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Where the line from the centre of `r` towards `(x, y)` leaves `r`.
fn edge_point(r: Rect, x: f32, y: f32) -> (f32, f32) {
    let (cx, cy) = r.center();
    let (dx, dy) = (x - cx, y - cy);
    if dx.abs() < 1e-3 && dy.abs() < 1e-3 {
        return (cx, cy);
    }
    let tx = if dx.abs() > 1e-6 { (r.w / 2.0) / dx.abs() } else { f32::INFINITY };
    let ty = if dy.abs() > 1e-6 { (r.h / 2.0) / dy.abs() } else { f32::INFINITY };
    let t = tx.min(ty).min(1.0);
    (cx + dx * t, cy + dy * t)
}

/// Check the annotations make sense before drawing them.
pub(crate) fn check_annotations(anns: &[Annotation]) -> Result<()> {
    use crate::spec::OneOrMany::{Many, One};
    for (i, a) in anns.iter().enumerate() {
        let ok = matches!(
            (&a.x, &a.y),
            (Some(Many(v)), None) | (None, Some(Many(v))) if v.len() == 2
        ) || matches!((&a.x, &a.y), (Some(One(_)), None) | (None, Some(One(_))) | (Some(One(_)), Some(One(_))));
        if !ok {
            return Err(crate::load::usage(format!(
                "annotate[{i}]: give x or y as a number (a line) or [a, b] (a band), or both as numbers (a labelled point)"
            )));
        }
        if a.to.is_some() && !matches!((&a.x, &a.y), (Some(One(_)), Some(One(_)))) {
            return Err(crate::load::usage(format!("annotate[{i}]: `to` (an arrow) needs x and y numbers for the label's position")));
        }
    }
    Ok(())
}

fn hover_panel(p: &Panel, title: Option<&str>, series: &[&Series]) -> Value {
    let dom = |d: Domain| json!({"lo": d.lo, "hi": d.hi, "log": d.kind == openreadout_figure::ScaleKind::Log, "reversed": d.reversed});
    let ss: Vec<Value> = series
        .iter()
        .filter(|s| !matches!(s.geom, Geom::Events { .. }))
        .take(120)
        .map(|s| {
            let pts: Vec<(f64, f64)> = match &s.geom {
                Geom::Line(p) | Geom::Points { pts: p, .. } => p.clone(),
                Geom::Band(b) => b.iter().map(|q| (q.0, q.1)).collect(),
                Geom::Bars { vals, .. } => vals.iter().enumerate().map(|(i, v)| (i as f64, *v)).collect(),
                _ => Vec::new(),
            };
            let cap = if series.len() > 24 { 120 } else { 400 };
            let step = pts.len().div_ceil(cap).max(1);
            let pts: Vec<[f64; 2]> = pts.iter().step_by(step).map(|q| [tidy(q.0), tidy(q.1)]).collect();
            json!({"name": s.name, "color": s.color.map(Color::to_hex), "points": pts})
        })
        .collect();
    json!({
        "title": title,
        "rect": [p.rect.x, p.rect.y, p.rect.w, p.rect.h],
        "x": dom(p.x_domain()),
        "y": dom(p.y_domain()),
        "series": ss,
    })
}

/// A single plot with axes, legend, callouts and annotations.
#[allow(clippy::too_many_arguments)]
fn draw_plot(fig: &mut Figure, theme: &Theme, area: Rect, p: &PanelIn, ax: &AxisRes, ay: &AxisRes, _classes: &[(String, Color)], hover: &mut Vec<Value>, ann: &mut Vec<String>) {
    let fs = theme.font_size;
    let entries = legend_entries(&p.series);
    let is_line = |s: &Series| matches!(s.geom, Geom::Line(_) | Geom::Band(_)) && !matches!(s.mark, MarkKind::Sticks | MarkKind::Area);
    // every entry has a line whose end can carry its name
    let line_like = entries.iter().all(|e| {
        p.series
            .iter()
            .any(|s| is_line(s) && s.class.clone().unwrap_or_else(|| s.name.clone()) == e.0)
    });
    let hist = !p.series.is_empty() && p.series.iter().all(|s| s.mark == MarkKind::Area);
    let legend = p.spec.legend.unwrap_or(if entries.len() <= 1 {
        Legend::None
    } else if entries.len() <= 10 && (line_like || hist) && !ax.is_cat() {
        Legend::Direct
    } else {
        Legend::Key
    });
    let mut area = area;
    if p.series.iter().any(|s| matches!(s.geom, Geom::Events { .. })) && area.w > area.h * 1.25 {
        // dot plots read best near square
        area = Rect::new(area.x, area.y, area.h * 1.25, area.h);
    }
    if legend == Legend::Key && !entries.is_empty() {
        let items: Vec<(Color, String)> = entries.iter().map(|e| (e.1, e.0.clone())).collect();
        let block = entries.iter().any(|e| e.2);
        let used = key_row(fig, theme, area.x, area.y + fs * 0.6, area.w, &items, block);
        area = area.inset_sides(0.0, used.bottom() - area.y + fs * 0.6, 0.0, 0.0);
    }
    let right = if legend == Legend::Direct && !hist {
        let st = TextStyle::new(fs, theme.ink);
        let w = entries.iter().map(|e| st.width(&e.0)).fold(0.0f32, f32::max);
        (w + fs * 1.6).min(area.w * 0.32)
    } else {
        0.0
    };
    let spec = axes_spec(ax, ay, right, 0.0);
    let mut fit_spec = spec.clone();
    if ax.is_cat() {
        // rotated category names need room
        let st = TextStyle::new(fs, theme.muted);
        let widest = ax.cats.iter().map(|c| st.width(&shorten(c, 22))).fold(0.0f32, f32::max);
        let room = area.w / ax.cats.len().max(1) as f32;
        if widest > room * 0.95 {
            fit_spec.x_label = None;
            let extra = widest * 0.6;
            area = area.inset_sides(0.0, 0.0, 0.0, extra);
        }
    }
    let panel = Panel::fit(area, ax.domain, ay.domain, theme, &fit_spec);
    let ink = Ink::for_panel(theme, panel.rect);
    draw_bands(fig, theme, &panel, &p.spec.annotate, None, ann);
    draw_axes(fig, theme, &panel, ax, ay, &spec);
    let refs: Vec<&Series> = p.series.iter().collect();
    draw_data(fig, theme, &panel, &refs, &ax.cats, ink);
    let mut placer = LabelPlacer::new(panel.rect);
    draw_marks_over(fig, theme, &panel, &p.spec.annotate, None, &mut placer, false, ann);
    draw_callouts(fig, theme, &panel, &refs, &mut placer, ink);
    if legend == Legend::Direct && hist {
        peak_labels(fig, theme, &panel, &p.series, &mut placer);
    } else if legend == Legend::Direct {
        direct_labels(fig, theme, &panel, &p.series, area.right());
    }
    hover.push(hover_panel(&panel, p.title.as_deref(), &refs));
}

/// Each histogram named at its highest bin.
fn peak_labels(fig: &mut Figure, theme: &Theme, panel: &Panel, series: &[Series], placer: &mut LabelPlacer) {
    let fs = theme.font_size;
    for s in series {
        let Geom::Line(pts) = &s.geom else { continue };
        let Some(&(x, y)) = pts.iter().filter(|q| q.1.is_finite()).max_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
        let Some((px, py)) = panel.pt(x, y) else { continue };
        let st = TextStyle::new(fs, label_color(theme, s.color.unwrap_or(theme.ink))).halo(theme.paper, fs * 0.3);
        let w = st.width(&s.name);
        let r = placer.place(px, py, w, fs * 1.1, fs * 0.5);
        fig.text(r.x, r.bottom() - fs * 0.25, &s.name, &st);
    }
}

/// Text in a series colour, readable: faint colours print muted.
fn label_color(theme: &Theme, c: Color) -> Color {
    if c == theme.faint || c.opacity() < 0.5 {
        theme.muted
    } else {
        c.alpha(1.0)
    }
}

/// Labels at the right end of each line (one per class), kept apart.
fn direct_labels(fig: &mut Figure, theme: &Theme, panel: &Panel, series: &[Series], right_edge: f32) {
    let mut groups: Vec<(String, Color, Vec<f32>)> = Vec::new();
    for s in series {
        if s.weight == Weight::Context && s.class.is_none() {
            continue;
        }
        let name = s.class.clone().unwrap_or_else(|| s.name.clone());
        if name.is_empty() {
            continue;
        }
        let last = match &s.geom {
            Geom::Line(p) => last_visible(panel, p.iter().copied()),
            Geom::Band(b) => last_visible(panel, b.iter().map(|q| (q.0, q.1))),
            _ => None,
        };
        let Some(y) = last else { continue };
        match groups.iter_mut().find(|g| g.0 == name) {
            Some(g) => g.2.push(y),
            None => groups.push((name, s.color.unwrap_or(theme.ink), vec![y])),
        }
    }
    let room = (right_edge - panel.rect.right() - theme.font_size * 1.0).max(theme.font_size * 3.0);
    let st = TextStyle::new(theme.font_size, theme.ink);
    let labels: Vec<EndLabel> = groups
        .into_iter()
        .map(|(text, color, ys)| {
            let mut ys = ys;
            ys.sort_by(f32::total_cmp);
            let mut t = text;
            while st.width(&t) > room && t.chars().count() > 4 {
                let n = t.chars().count();
                t = format!("{}…", t.chars().take(n - 2).collect::<String>());
            }
            EndLabel { y: ys[ys.len() / 2], text: t, color: label_color(theme, color) }
        })
        .collect();
    end_labels(fig, theme, panel.rect.right(), &labels, panel.rect.y + theme.font_size * 0.5, panel.rect.bottom());
}

/// The pixel y of the last point of a line that lies inside the panel's x range.
fn last_visible(p: &Panel, pts: impl Iterator<Item = (f64, f64)>) -> Option<f32> {
    let d = p.x_domain();
    let (lo, hi) = (d.lo.min(d.hi), d.lo.max(d.hi));
    let mut best: Option<(f32, f32)> = None;
    for (x, y) in pts {
        if !(x >= lo && x <= hi && y.is_finite()) {
            continue;
        }
        let (Some(px), Some(py)) = (p.x.px(x), p.y.px(y)) else { continue };
        if best.is_none_or(|b| px > b.0) {
            best = Some((px, py.clamp(p.rect.y, p.rect.bottom())));
        }
    }
    best.map(|b| b.1)
}

/// Small multiples, one per facet key, on shared axes.
#[allow(clippy::too_many_arguments)]
fn draw_facets(fig: &mut Figure, theme: &Theme, area: Rect, p: &PanelIn, ax: &AxisRes, ay: &AxisRes, classes: &[(String, Color)], columns: Option<u32>, hover: &mut Vec<Value>, ann: &mut Vec<String>) -> usize {
    let fs = theme.font_size;
    let keys = facet_keys(&p.series);
    let (rows, cols) = grid_shape(keys.len(), columns, area.w);
    let mut area = area;
    // one key for colour classes
    let entries = legend_entries(&p.series);
    if !classes.is_empty() && entries.len() > 1 {
        let items: Vec<(Color, String)> = entries.iter().map(|e| (e.1, e.0.clone())).collect();
        let used = key_row(fig, theme, area.x, area.y + fs * 0.6, area.w, &items, entries.iter().any(|e| e.2));
        area = area.inset_sides(0.0, used.bottom() - area.y + fs * 0.4, 0.0, 0.0);
    }
    let small_fs = theme.small_size();
    let gap_x = fs * 1.2;
    let gap_y = fs * 0.9 + small_fs * 1.6;
    let cells = openreadout_figure::grid(area.inset_sides(0.0, small_fs * 1.6, 0.0, 0.0), rows, cols, gap_x, gap_y);
    let spec = axes_spec(ax, ay, 0.0, 0.0);
    let Some(c0) = cells.first().copied() else { return 0 };
    // every cell's plot area has the same offsets, so the plots line up
    let mut probe = spec.clone();
    let sub = theme.clone();
    probe.x_label = if rows > 0 { probe.x_label } else { None };
    let fitted = Panel::fit(c0, ax.domain, ay.domain, &sub, &probe).rect;
    let (dl, dt) = (fitted.x - c0.x, fitted.y - c0.y);
    let (dr, db) = (c0.right() - fitted.right(), c0.bottom() - fitted.bottom());
    let mut notes_once = Vec::new();
    for (k, key) in keys.iter().enumerate() {
        let Some(&cell) = cells.get(k) else { break };
        let (r, c) = (k / cols, k % cols);
        let rect = cell.inset_sides(dl, dt, dr, db);
        let panel = Panel::new(rect, ax.domain, ay.domain);
        let mut s = spec.clone();
        s.show_y = c == 0;
        s.show_x = r == rows - 1 || k + cols >= keys.len();
        // the axis names once: y on the first plot, x on the last
        if k != 0 {
            s.y_label = None;
        }
        if k != keys.len() - 1 {
            s.x_label = None;
        }
        if rect.h < 200.0 {
            s.y_ticks = 3;
        } else if rect.h < 320.0 {
            s.y_ticks = 4;
        }
        let mut sink = Vec::new();
        draw_bands(fig, theme, &panel, &p.spec.annotate, Some(key), if k == 0 { &mut notes_once } else { &mut sink });
        draw_axes(fig, theme, &panel, ax, ay, &s);
        let members: Vec<&Series> = p
            .series
            .iter()
            .filter(|x| x.facet.clone().unwrap_or_else(|| x.name.clone()) == *key)
            .collect();
        let ink = Ink::for_panel(theme, panel.rect);
        draw_data(fig, theme, &panel, &members, &ax.cats, ink);
        let mut placer = LabelPlacer::new(panel.rect);
        draw_marks_over(fig, theme, &panel, &p.spec.annotate, Some(key), &mut placer, false, if k == 0 { &mut notes_once } else { &mut sink });
        draw_callouts(fig, theme, &panel, &members, &mut placer, ink);
        // facet title, above the plot
        let st = TextStyle::new(small_fs * 1.08, theme.ink).face(Face::SansMedium);
        let ty = cell.y - small_fs * 0.55;
        fig.text(rect.x, ty, &shorten(key, 60), &st);
        hover.push(hover_panel(&panel, Some(key), &members));
    }
    ann.extend(notes_once);
    keys.len()
}

/// Plate wells in their plate positions: one card per well, column and row headings, the
/// layout groups as row blocks.
#[allow(clippy::too_many_arguments)]
fn draw_plate(fig: &mut Figure, theme: &Theme, area: Rect, p: &PanelIn, ax: &AxisRes, ay: &AxisRes, classes: &[(String, Color)], hover: &mut Vec<Value>, ann: &mut Vec<String>) -> usize {
    let fs = theme.font_size;
    let hints = p.plate.clone().unwrap_or(PlateHints {
        rows: p.series.iter().filter_map(|s| s.well).map(|w| w.0 + 1).max().unwrap_or(8),
        cols: p.series.iter().filter_map(|s| s.well).map(|w| w.1 + 1).max().unwrap_or(12),
        row_groups: Vec::new(),
    });
    let (rows, cols) = (hints.rows.max(1) as usize, hints.cols.max(1) as usize);
    // row blocks: rows sharing a layout group, or the labels given
    let mut blocks: Vec<(usize, usize, String)> = Vec::new(); // (first row, rows, label)
    if p.spec.row_labels.len() == rows {
        for (i, l) in p.spec.row_labels.iter().enumerate() {
            blocks.push((i, 1, l.clone()));
        }
    } else {
        let groups: Vec<Option<String>> = (0..rows).map(|r| hints.row_groups.get(r).cloned().flatten()).collect();
        let mut r = 0;
        while r < rows {
            let g = groups[r].clone();
            let mut e = r + 1;
            while e < rows && groups[e] == g && g.is_some() {
                e += 1;
            }
            blocks.push((r, e - r, g.unwrap_or_else(|| openreadout_assay::layout::row_label(r as u32))));
            r = e;
        }
        if !p.spec.row_labels.is_empty() && p.spec.row_labels.len() == blocks.len() {
            for (b, l) in blocks.iter_mut().zip(&p.spec.row_labels) {
                b.2.clone_from(l);
            }
        }
    }
    let grouped = blocks.iter().any(|b| b.1 > 1);
    let label_fs = if grouped { fs * 1.25 } else { fs };
    let label_st = TextStyle::new(label_fs, if grouped { theme.ink } else { theme.muted }).align(Anchor::End, Baseline::Middle);
    let left = blocks.iter().map(|b| label_st.width(&b.2)).fold(0.0f32, f32::max) + fs * 1.2;
    let head = fs * 2.0;
    let entries = legend_entries(&p.series);
    let foot = fs * 3.2;
    let block_gap = if grouped { fs * 0.9 } else { 0.0 };
    let grid_area = area.inset_sides(left, head, 0.0, foot);
    let gap = (grid_area.w / cols as f32 * 0.07).clamp(3.0, fs * 0.6);
    let n_gaps = blocks.len().saturating_sub(1) as f32;
    let cw = (grid_area.w - gap * (cols as f32 - 1.0)) / cols as f32;
    let ch_avail = (grid_area.h - gap * (rows as f32 - 1.0) - block_gap * n_gaps) / rows as f32;
    let ch = ch_avail.min(cw * 0.78).max(8.0);
    let row_y = |r: usize| -> f32 {
        let b = blocks.iter().position(|b| r >= b.0 && r < b.0 + b.1).unwrap_or(0);
        grid_area.y + r as f32 * (ch + gap) + b as f32 * block_gap
    };
    // column headings
    let col_st = TextStyle::new(fs, theme.muted).align(Anchor::Middle, Baseline::Alphabetic);
    for c in 0..cols {
        let x = grid_area.x + c as f32 * (cw + gap) + cw / 2.0;
        let t = p.spec.column_labels.get(c).cloned().unwrap_or_else(|| (c + 1).to_string());
        fig.text(x, grid_area.y - fs * 0.7, &t, &col_st);
    }
    for b in &blocks {
        let y0 = row_y(b.0);
        let y1 = row_y(b.0 + b.1 - 1) + ch;
        fig.text(grid_area.x - fs * 0.9, (y0 + y1) / 2.0, &b.2, &label_st);
    }
    let card = theme.panel;
    let n_many = p.series.len();
    let mut drawn = 0;
    let mut sink = Vec::new();
    let mut first_panel: Option<Panel> = None;
    for r in 0..rows {
        for c in 0..cols {
            let x = grid_area.x + c as f32 * (cw + gap);
            let y = row_y(r);
            let cell = Rect::new(x, y, cw, ch);
            fig.shape(
                Path::round_rect(cell.x, cell.y, cell.w, cell.h, (cw * 0.07).clamp(2.0, 7.0)),
                Some(card),
                Some(Stroke::new(theme.rule, 1.0)),
                None,
            );
            let members: Vec<&Series> = p.series.iter().filter(|s| s.well == Some((r as u32, c as u32))).collect();
            if members.is_empty() {
                continue;
            }
            let pad = (cw * 0.07).clamp(3.0, 9.0);
            let panel = Panel::new(cell.inset(pad), ax.domain, ay.domain);
            draw_bands(fig, theme, &panel, &p.spec.annotate, members[0].facet.as_deref(), if drawn == 0 { ann } else { &mut sink });
            let ink = Ink { line: (cw / 75.0).clamp(1.0, theme.line_width), dot: 3.0, small: true, fills: 1 };
            let _ = n_many;
            for s in &members {
                let mut s2 = (*s).clone();
                if s2.weight == Weight::Many {
                    s2.weight = Weight::Main;
                }
                draw_series(fig, &panel, theme, &s2, ink, 1, None, &[]);
            }
            let mut placer = LabelPlacer::new(panel.rect);
            draw_marks_over(fig, theme, &panel, &p.spec.annotate, members[0].facet.as_deref(), &mut placer, true, &mut sink);
            hover.push(hover_panel(&panel, members[0].facet.as_deref(), &members));
            if first_panel.is_none() {
                first_panel = Some(panel);
            }
            drawn += 1;
        }
    }
    // key and the shared scale, under the grid
    let bottom = row_y(rows - 1) + ch + fs * 1.5;
    let mut kx = grid_area.x;
    if !classes.is_empty() && entries.len() > 1 {
        let items: Vec<(Color, String)> = entries.iter().map(|e| (e.1, e.0.clone())).collect();
        let used = key_row(fig, theme, kx, bottom, grid_area.w * 0.6, &items, false);
        kx = used.right() + fs * 1.5;
    }
    let range = |a: &AxisRes| {
        let d = a.domain;
        let (lo, hi) = (d.lo.min(d.hi), d.lo.max(d.hi));
        let step = (hi - lo) / 4.0;
        let (a, b) = (format_tick(lo, step).replace('-', "−"), format_tick(hi, step).replace('-', "−"));
        if lo < 0.0 { format!("{a} to {b}") } else { format!("{a}–{b}") }
    };
    let scale_txt = format!(
        "every well on one scale: {} {}, {} {}",
        ax.label.split(" (").next().unwrap_or("x"),
        with_unit(&range(ax), ax.unit.as_deref()),
        ay.label.split(" (").next().unwrap_or("y"),
        with_unit(&range(ay), ay.unit.as_deref())
    );
    let st = TextStyle::new(theme.small_size(), theme.muted).align(Anchor::End, Baseline::Middle);
    if kx + st.width(&scale_txt) < grid_area.right() {
        fig.text(grid_area.right(), bottom, &scale_txt, &st);
    } else {
        fig.text(grid_area.right(), bottom + fs * 1.4, &scale_txt, &st);
    }
    let _ = spread;
    let _ = EndLabel { y: 0.0, text: String::new(), color: theme.ink };
    drawn
}

fn with_unit(s: &str, u: Option<&str>) -> String {
    match u {
        Some(u) => format!("{s} {u}"),
        None => s.to_string(),
    }
}
