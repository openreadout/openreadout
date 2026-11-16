//! 2D NMR spectra as contour plots, the way chemists read them: F2 runs along the bottom and
//! F1 up the right side, both from high to low shift (a COSY diagonal runs from bottom left to
//! top right), with the skyline projections along the top and the left.
//!
//! - **Data**: [`openreadout_signal::nmr::read_pooled`] reduces the spectrum to about two
//!   pixels per cell, keeping each cell's largest and smallest value (max and min pooling), so a
//!   one-point-wide line survives any reduction and a 4k × 4k spectrum is read once, row by
//!   row.
//! - **Levels**: `levels` contours spaced geometrically (a constant factor, at least 1.25 and
//!   at most 2) from `floor` × the noise SD to the largest value. The noise SD is the cross-peak
//!   picker's (the larger of the run and maxima estimates) when cross peaks are marked, else the
//!   run estimate of the rows read. Negative contours are drawn at the same levels below zero in
//!   the accent colour; positive ones in ink. Lower levels are lighter.
//! - **Contours**: marching squares on the cell grid, saddles resolved by the cell's mean,
//!   segments joined into polylines (one path per level and sign).
//! - **Marks**: the picker's cross peaks as small open circles (`marks: auto`), numbered by
//!   their index in the `analyze nmr-peaks` list with `marks: peaks`.
//! - **Axes**: the nucleus on each axis label; a J-resolved F1 axis is drawn in Hz and an axis
//!   whose unit the file names (DOSY) in that unit.

use std::collections::HashMap;

use openreadout_core::model::FileInfo;
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Color, Domain, Figure, GridLines, LabelPlacer, Layer, Panel, Path,
    Rect, Stroke, TextStyle, Theme, superscript,
};
use openreadout_signal::nmr::{
    CrossPeakOptions, DimensionAxis, Nmr2dReport, Nmr2dRequest, PooledRequest, PooledView,
    analyze_2d, read_pooled, slices2d,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{DataMark, Frame, FramePanel, Marks, PreviewOutput, PreviewRequest};

/// Default lowest contour, in noise SDs.
pub const DEFAULT_CONTOUR_FLOOR: f64 = 6.0;
/// Default number of contour levels of each sign.
pub const DEFAULT_CONTOUR_LEVELS: u32 = 12;
/// Most cross peaks marked.
const MAX_MARKED: usize = 300;
/// Most cross peaks numbered.
const MAX_NUMBERED: usize = 40;

/// What a 2D NMR preview drew.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Nmr2dPreview {
    /// The 2D spectrum drawn (see `info` → `traces[]`).
    pub trace: u32,
    /// Points per dimension of the whole spectrum, slowest first.
    pub shape: Vec<usize>,
    /// F1 then F2: what each axis shows and its window.
    pub axes: Vec<Nmr2dAxis>,
    /// Contour levels drawn (positive; the negative ones are the same below zero).
    pub levels: Vec<f64>,
    /// Lowest level in noise SDs.
    pub floor_snr: f64,
    /// The noise SD the levels are based on.
    pub noise_sd: f64,
    /// Spectrum points per cell along F1 and F2 (max and min pooling).
    pub points_per_cell: [usize; 2],
    /// Negative contours were drawn (in the accent colour).
    pub negative: bool,
    /// Cross peaks marked.
    pub cross_peaks: usize,
}

/// One axis of a 2D NMR preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Nmr2dAxis {
    /// `f1` or `f2`.
    pub dimension: String,
    /// Nucleus, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nucleus: Option<String>,
    /// `chemical_shift`, `coupling` or `other`.
    pub quantity: String,
    /// Unit of `first` and `last`.
    pub unit: String,
    /// Value at the left (F2) or bottom (F1) edge of the plot.
    pub first: f64,
    /// Value at the right (F2) or top (F1) edge.
    pub last: f64,
}

/// How the 2D view is drawn.
#[derive(Debug, Clone)]
pub struct Nmr2dView {
    /// The 2D spectrum (default: the first one).
    pub trace: Option<u32>,
    /// F2 window, ppm (either order).
    pub x_range: Option<[f64; 2]>,
    /// F1 window, in the unit the axis shows (ppm; Hz for a J-resolved F1).
    pub y_range: Option<[f64; 2]>,
    /// Lowest contour in noise SDs.
    pub floor: f64,
    /// Contour levels of each sign.
    pub levels: u32,
    /// Mark the picked cross peaks.
    pub markers: bool,
    /// Number them too.
    pub numbers: bool,
}

impl Default for Nmr2dView {
    fn default() -> Self {
        Self {
            trace: None,
            x_range: None,
            y_range: None,
            floor: DEFAULT_CONTOUR_FLOOR,
            levels: DEFAULT_CONTOUR_LEVELS,
            markers: true,
            numbers: false,
        }
    }
}

/// `1H` → `¹H`.
pub(crate) fn nucleus_label(n: &str) -> String {
    let digits: String = n.chars().take_while(char::is_ascii_digit).collect();
    let rest = &n[digits.len()..];
    match digits.parse::<i32>() {
        Ok(d) => format!("{}{rest}", superscript(d)),
        Err(_) => n.to_string(),
    }
}

/// The experiment, from the pulse program name (`hsqcedetgpsisp2.3` → `edited HSQC`).
pub fn experiment_name(program: &str) -> Option<&'static str> {
    let p = program.to_ascii_lowercase();
    let starts = |k: &str| p.starts_with(k);
    Some(if starts("hsqced") {
        "edited HSQC"
    } else if starts("hsqc") || p.contains("hsqc") {
        "HSQC"
    } else if starts("hmqc") {
        "HMQC"
    } else if starts("hmbc") || p.contains("hmbc") {
        "HMBC"
    } else if starts("cosy") {
        "COSY"
    } else if starts("tocsy") || starts("mlev") || starts("dipsi") {
        "TOCSY"
    } else if starts("noesy") {
        "NOESY"
    } else if starts("roesy") {
        "ROESY"
    } else if p.contains("jres") {
        "J-resolved"
    } else if starts("ledbp") || starts("stebp") || starts("ledgp") || p.contains("dosy") {
        "DOSY"
    } else if p.contains("trosy") {
        "TROSY"
    } else {
        return None;
    })
}

/// `F2 ¹H (ppm)`, `F1 J (Hz)`, `F1 (log(m2/s))`.
fn axis_title(d: usize, a: &DimensionAxis) -> String {
    let nu = a.nucleus.as_deref().map(nucleus_label);
    let unit = a.unit.replace("m2", "m²");
    match (a.quantity.as_str(), nu) {
        ("coupling", _) => format!("F{} J ({unit})", d + 1),
        ("chemical_shift", Some(n)) => format!("F{} {n} ({unit})", d + 1),
        _ => format!("F{} ({unit})", d + 1),
    }
}

/// Geometric contour levels from `base` to `top` (at most `n`).
pub fn contour_levels(base: f64, top: f64, n: u32) -> Vec<f64> {
    if !(base > 0.0 && top > base && n > 0) {
        return if base > 0.0 && top >= base {
            vec![base]
        } else {
            Vec::new()
        };
    }
    let n = n.max(1) as f64;
    let factor = (top / base).powf(1.0 / n).clamp(1.25, 2.0);
    let mut out = Vec::new();
    let mut l = base;
    while l < top && out.len() < n as usize {
        out.push(l);
        l *= factor;
    }
    out
}

/// Marching squares: the contour of `grid` (`rows` × `cols`, row-major) at `level`, through
/// cell centres at pixel columns `xs` and rows `ys`, joined into polylines.
pub fn contour_path(
    grid: &[f64],
    rows: usize,
    cols: usize,
    level: f64,
    xs: &[f32],
    ys: &[f32],
) -> Path {
    let mut path = Path::new();
    if rows < 2 || cols < 2 || grid.len() < rows * cols {
        return path;
    }
    let at = |r: usize, c: usize| grid[r * cols + c];
    // edge ids: horizontal edge (r, c)-(r, c+1) = 2·(r·cols + c), vertical (r, c)-(r+1, c) = +1
    let h = |r: usize, c: usize| 2 * (r * cols + c);
    let v = |r: usize, c: usize| 2 * (r * cols + c) + 1;
    let point = |e: usize| -> (f32, f32) {
        let k = e / 2;
        let (r, c) = (k / cols, k % cols);
        let (a, b, (x0, y0), (x1, y1)) = if e % 2 == 0 {
            (at(r, c), at(r, c + 1), (xs[c], ys[r]), (xs[c + 1], ys[r]))
        } else {
            (at(r, c), at(r + 1, c), (xs[c], ys[r]), (xs[c], ys[r + 1]))
        };
        let t = if (b - a).abs() > 0.0 {
            ((level - a) / (b - a)).clamp(0.0, 1.0) as f32
        } else {
            0.5
        };
        (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t)
    };
    let mut segs: Vec<(usize, usize)> = Vec::new();
    for r in 0..rows - 1 {
        for c in 0..cols - 1 {
            let (a, b, cc, d) = (at(r, c), at(r, c + 1), at(r + 1, c + 1), at(r + 1, c));
            let idx = (usize::from(a >= level) << 3)
                | (usize::from(b >= level) << 2)
                | (usize::from(cc >= level) << 1)
                | usize::from(d >= level);
            let (top, right, bottom, left) = (h(r, c), v(r, c + 1), h(r + 1, c), v(r, c));
            let centre = (a + b + cc + d) / 4.0 >= level;
            match idx {
                1 | 14 => segs.push((left, bottom)),
                2 | 13 => segs.push((bottom, right)),
                3 | 12 => segs.push((left, right)),
                4 | 11 => segs.push((top, right)),
                6 | 9 => segs.push((top, bottom)),
                7 | 8 => segs.push((left, top)),
                5 => {
                    if centre {
                        segs.push((left, top));
                        segs.push((bottom, right));
                    } else {
                        segs.push((top, right));
                        segs.push((left, bottom));
                    }
                }
                10 => {
                    if centre {
                        segs.push((top, right));
                        segs.push((left, bottom));
                    } else {
                        segs.push((left, top));
                        segs.push((bottom, right));
                    }
                }
                _ => {}
            }
        }
    }
    // join segments that share an edge point into polylines
    let mut by_edge: HashMap<usize, Vec<usize>> = HashMap::with_capacity(segs.len() * 2);
    for (i, &(a, b)) in segs.iter().enumerate() {
        by_edge.entry(a).or_default().push(i);
        by_edge.entry(b).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let next = |edge: usize, used: &[bool]| -> Option<usize> {
        by_edge.get(&edge)?.iter().copied().find(|&s| !used[s])
    };
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let (a, b) = segs[start];
        let mut fwd = vec![a, b];
        let mut end = b;
        while let Some(s) = next(end, &used) {
            used[s] = true;
            let (p, q) = segs[s];
            end = if p == end { q } else { p };
            fwd.push(end);
        }
        let mut back = Vec::new();
        let mut end = a;
        while let Some(s) = next(end, &used) {
            used[s] = true;
            let (p, q) = segs[s];
            end = if p == end { q } else { p };
            back.push(end);
        }
        back.reverse();
        back.extend(fwd);
        let mut first = true;
        for e in back {
            let (x, y) = point(e);
            if first {
                path.move_to(x, y);
                first = false;
            } else {
                path.line_to(x, y);
            }
        }
    }
    path
}

/// What [`draw`] drew, for the preview's output and frame.
#[derive(Debug)]
pub struct Drawn {
    /// The contour panel (F2 along x, F1 along y in the unit shown).
    pub panel: Panel,
    /// The preview summary.
    pub summary: Nmr2dPreview,
    /// The cross peaks marked.
    pub marks: Vec<DataMark>,
    /// Labels placed (the caller's labels keep off them).
    pub taken: Vec<Rect>,
    /// Remarks.
    pub notes: Vec<String>,
}

/// The F1 window in ppm from one in the unit shown.
fn f1_ppm_range(view: &Nmr2dView, dim: &DimensionAxis) -> Option<(f64, f64)> {
    view.y_range.map(|[a, b]| (dim.unshow(a), dim.unshow(b)))
}

/// Read and draw the 2D spectrum into `area` of `fig`. `report` supplies the cross peaks to
/// mark (and the noise SD); without it and with `view.markers`, the picker runs on the window.
#[allow(clippy::too_many_lines)]
pub fn draw(
    fig: &mut Figure,
    area: Rect,
    theme: &Theme,
    ds: &mut dyn Dataset,
    info: &FileInfo,
    view: &Nmr2dView,
    report: Option<&Nmr2dReport>,
) -> Result<Drawn> {
    let fs = theme.font_size;
    // what F1 measures, before reading (its window may be given in Hz)
    let vendor = ds.vendor_metadata().unwrap_or(serde_json::Value::Null);
    let trace = openreadout_signal::nmr::source2d::choose_matrix_trace(info, &vendor, view.trace)?;
    let t = info
        .traces
        .iter()
        .find(|t| t.index == trace)
        .ok_or_else(|| Error::Usage(format!("trace {trace} out of range")))?;
    let lay = openreadout_signal::nmr::source2d::matrix_layout(t, &vendor).ok_or_else(|| {
        Error::unsupported(
            "preview",
            format!("trace {trace} has no matrix layout"),
            "Pick a stored 2D spectrum (see `openreadout info`).",
        )
    })?;
    let dims = slices2d::dimension_axes(info, t, &vendor, &lay);
    let nd = dims.len();
    let (d1, d2) = (&dims[nd - 2], &dims[nd - 1]);
    let x_range = view.x_range.map(|[a, b]| (a, b));
    let y_range = f1_ppm_range(view, d1);

    // layout: caption row, top strip, panel, x axis; left strip, panel, right axis
    let header = fs * 2.0;
    let strip = (area.w.min(area.h) * 0.11).clamp(fs * 3.0, fs * 8.0);
    let gap = (fs * 0.35).round();
    let right_axis = fs * 4.6;
    let bottom_axis = fs * 3.6;
    let px_w = (area.w - strip - gap - right_axis).max(40.0);
    let px_h = (area.h - header - strip - gap - bottom_axis).max(40.0);
    let rect = Rect::new(
        area.x + strip + gap,
        area.y + header + strip + gap,
        px_w,
        px_h,
    );
    let top_strip = Rect::new(rect.x, rect.y - gap - strip, rect.w, strip);
    let left_strip = Rect::new(rect.x - gap - strip, rect.y, strip, rect.h);

    let cells = (
        ((rect.h / 2.0) as usize).clamp(16, 1024),
        ((rect.w / 2.0) as usize).clamp(16, 1024),
    );
    let pooled = read_pooled(
        ds,
        info,
        &PooledRequest {
            trace: Some(trace),
            ranges_ppm: [y_range, x_range],
            cells,
            ..PooledRequest::default()
        },
    )?;
    let mut notes = pooled.notes.clone();

    // the cross peaks to mark, and the noise the levels start from
    let picked: Option<Nmr2dReport> = match report {
        Some(r) => Some(r.clone()),
        None if view.markers || view.numbers => {
            let mut ranges = vec![None; nd];
            ranges[nd - 2] = y_range;
            ranges[nd - 1] = x_range;
            match analyze_2d(
                ds,
                info,
                &Nmr2dRequest {
                    trace: Some(trace),
                    peaks: CrossPeakOptions {
                        ranges_ppm: ranges,
                        ..CrossPeakOptions::default()
                    },
                    ..Nmr2dRequest::default()
                },
            ) {
                Ok(r) => Some(r),
                Err(e) => {
                    notes.push(format!("cross peaks not marked: {e}"));
                    None
                }
            }
        }
        None => None,
    };
    let noise = picked
        .as_ref()
        .map(|r| r.noise_sd)
        .filter(|n| *n > 0.0)
        .unwrap_or(pooled.noise_sd);
    let base = view.floor.max(0.1) * noise;
    let largest_pos = pooled.max.iter().copied().fold(0.0f64, f64::max);
    let largest_neg = -pooled.min.iter().copied().fold(0.0f64, f64::min);
    let top = largest_pos.max(largest_neg);
    let levels = contour_levels(base, top, view.levels);
    let negative = largest_neg > base;

    // domains: both axes high to low; F1 in the unit shown, high values at the bottom
    let ppm_lim = |a: &openreadout_signal::nmr::PpmAxis| {
        let (p, q) = (a.first_ppm, a.last_ppm());
        (p.min(q), p.max(q))
    };
    let (x_lo, x_hi) = ppm_lim(&pooled.axes[1]);
    let (y_lo_ppm, y_hi_ppm) = ppm_lim(&pooled.axes[0]);
    let (ya, yb) = (d1.show(y_lo_ppm), d1.show(y_hi_ppm));
    let x_dom = Domain::linear(x_lo, x_hi.max(x_lo + 1e-9)).reversed();
    let y_dom = Domain::linear(ya.min(yb), ya.max(yb).max(ya.min(yb) + 1e-9)).reversed();
    let panel = Panel::new(rect, x_dom, y_dom);

    // grid, under everything
    let xt = panel.x.ticks(8);
    let yt = panel.y.ticks(8);
    fig.group(Layer::Grid, "grid", |fig| {
        let g = Stroke::new(theme.grid, theme.hairline());
        for t in xt.iter().filter(|t| t.major) {
            fig.line(t.px, rect.y, t.px, rect.bottom(), g.clone());
        }
        for t in yt.iter().filter(|t| t.major) {
            fig.line(rect.x, t.px, rect.right(), t.px, g.clone());
        }
    });
    // the diagonal of a homonuclear spectrum
    if d1.is_shift() && d2.is_shift() && d1.nucleus.is_some() && d1.nucleus == d2.nucleus {
        let (lo, hi) = (x_lo.max(y_lo_ppm), x_hi.min(y_hi_ppm));
        if hi > lo {
            fig.group(Layer::Grid, "diagonal", |fig| {
                if let (Some(a), Some(b)) = (panel.pt(lo, d1.show(lo)), panel.pt(hi, d1.show(hi))) {
                    fig.shape(
                        Path::segment(a.0, a.1, b.0, b.1),
                        None,
                        Some(Stroke::new(theme.rule, theme.hairline()).dashed(4.0, 3.0)),
                        Some(rect),
                    );
                }
            });
        }
    }
    // contours: positive from the max grid, negative from the min grid
    let xs: Vec<f32> = (0..pooled.cols)
        .map(|c| panel.x.px(pooled.cell_ppm(1, c as f64)).unwrap_or(f32::NAN))
        .collect();
    let ys: Vec<f32> = (0..pooled.rows)
        .map(|r| {
            panel
                .y
                .px(d1.show(pooled.cell_ppm(0, r as f64)))
                .unwrap_or(f32::NAN)
        })
        .collect();
    let width = (theme.line_width * 0.55).max(0.6);
    let n = levels.len().max(1);
    let shade = |c: Color, k: usize| -> Color {
        let t = if n > 1 {
            k as f32 / (n - 1) as f32
        } else {
            1.0
        };
        c.mix(theme.paper, 0.55 * (1.0 - t))
    };
    fig.group(Layer::Series, "contours positive", |fig| {
        for (k, &l) in levels.iter().enumerate() {
            let p = contour_path(&pooled.max, pooled.rows, pooled.cols, l, &xs, &ys);
            if !p.is_empty() {
                fig.shape(
                    p,
                    None,
                    Some(Stroke::new(shade(theme.ink, k), width)),
                    Some(rect),
                );
            }
        }
    });
    if negative {
        let neg: Vec<f64> = pooled.min.iter().map(|v| -v).collect();
        fig.group(Layer::Series, "contours negative", |fig| {
            for (k, &l) in levels.iter().enumerate() {
                let p = contour_path(&neg, pooled.rows, pooled.cols, l, &xs, &ys);
                if !p.is_empty() {
                    fig.shape(
                        p,
                        None,
                        Some(Stroke::new(shade(theme.accent, k), width)),
                        Some(rect),
                    );
                }
            }
        });
    }
    // frame of the panel
    fig.group(Layer::Axis, "frame", |fig| {
        fig.stroke(
            Path::rect(rect.x, rect.y, rect.w, rect.h),
            Stroke::new(theme.rule, theme.hairline()),
        );
    });

    // projections
    draw_projections(
        fig, theme, &pooled, &panel, d1, top_strip, left_strip, negative,
    );

    // x axis (bottom) from the panel; F1 axis on the right, drawn here
    let spec = AxesSpec {
        x_label: Some(axis_title(nd - 1, d2)),
        show_y: false,
        grid: GridLines::None,
        x_ticks: 8,
        ..AxesSpec::default()
    };
    panel.axes(fig, theme, &spec);
    fig.group(Layer::Axis, "y axis", |fig| {
        let tick = (fs * 0.4).round();
        let pad = (fs * 0.5).round();
        let axis = Stroke::new(theme.rule, theme.hairline());
        let st = TextStyle::new(fs, theme.muted).align(Anchor::Start, Baseline::Middle);
        let mut last = f32::NEG_INFINITY;
        let mut widest = 0.0f32;
        let mut sorted: Vec<_> = yt.iter().collect();
        sorted.sort_by(|a, b| a.px.total_cmp(&b.px));
        for t in sorted {
            let len = if t.major { tick } else { tick * 0.6 };
            fig.line(rect.right(), t.px, rect.right() + len, t.px, axis.clone());
            if t.major && !t.label.is_empty() {
                let b = st.bounds(rect.right() + tick + pad * 0.6, t.px, &t.label);
                if b.y > last + fs * 0.15 {
                    fig.text(rect.right() + tick + pad * 0.6, t.px, &t.label, &st);
                    last = b.bottom();
                    widest = widest.max(b.w);
                }
            }
        }
        let title = axis_title(nd - 2, d1);
        let x = (rect.right() + tick + pad * 0.6 + widest + fs * 0.9).min(area.right() - fs * 0.3);
        let st = TextStyle::new(fs, theme.muted)
            .align(Anchor::End, Baseline::Alphabetic)
            .rotate(90.0);
        // reads top to bottom, ending at the bottom of the panel
        fig.text(x - fs * 0.25, rect.bottom(), &title, &st);
    });

    // caption and key
    let program = slices2d::pulse_program(info);
    let mut parts: Vec<String> = Vec::new();
    let pair = match (&d2.nucleus, &d1.nucleus) {
        (Some(a), Some(b)) if a != b => format!("{}–{}", nucleus_label(a), nucleus_label(b)),
        (Some(a), _) => nucleus_label(a),
        _ => String::new(),
    };
    let what = program.as_deref().and_then(experiment_name);
    let head = [pair.as_str(), what.unwrap_or("2D NMR")]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    parts.push(head);
    let mhz = lay.axes[nd - 1].reference_frequency_mhz;
    if mhz > 0.0 {
        parts.push(format!("{mhz:.0} MHz"));
    }
    parts.push(t.name.clone().unwrap_or_else(|| format!("trace {trace}")));
    if let Some(p) = &program {
        parts.push(p.clone());
    }
    let caption = parts.join(" · ");
    let mut keys: Vec<(Color, String)> = vec![(theme.ink, "positive".into())];
    if negative {
        keys.push((theme.accent, "negative".into()));
    }
    let key_w: f32 = keys
        .iter()
        .map(|(_, t)| fs * 1.1 + fs * 0.4 + TextStyle::new(fs, theme.ink).width(t) + fs * 1.2)
        .sum();
    let cap_style = TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic);
    let room = area.w - key_w - fs;
    let caption = fit_text(&caption, &cap_style, room);
    fig.group(Layer::Caption, "caption", |fig| {
        fig.text(area.x + fs * 0.2, area.y + fs * 1.3, &caption, &cap_style);
    });
    if negative {
        openreadout_figure::key_row(
            fig,
            theme,
            area.right() - key_w,
            area.y + fs * 0.95,
            key_w,
            &keys,
            false,
        );
    }

    // cross peaks
    let mut marks = Vec::new();
    let mut placer = LabelPlacer::new(rect);
    let mut marked = 0usize;
    if let Some(r) = &picked
        && (view.markers || view.numbers)
    {
        let mut order: Vec<usize> = (0..r.peaks.len()).collect();
        order.sort_by(|&a, &b| r.peaks[b].height.abs().total_cmp(&r.peaks[a].height.abs()));
        let ring = Stroke::new(theme.series(2), (theme.line_width * 0.7).max(0.8));
        let label = TextStyle::new(theme.small_size(), theme.series(2))
            .align(Anchor::Start, Baseline::Top)
            .halo(theme.paper, 2.0);
        fig.group(Layer::Annotation, "cross peaks", |fig| {
            for (rank, &i) in order.iter().take(MAX_MARKED).enumerate() {
                let p = &r.peaks[i];
                let (Some(&f1), Some(&f2)) =
                    (p.ppm.get(p.ppm.len().saturating_sub(2)), p.ppm.last())
                else {
                    continue;
                };
                let y = d1.show(f1);
                let Some((px, py)) = panel.pt(f2, y) else {
                    continue;
                };
                if !rect.contains(px, py) {
                    continue;
                }
                let rad = (fs * 0.32).max(2.5);
                fig.stroke(Path::circle(px, py, rad), ring.clone());
                placer.reserve(Rect::new(px - rad, py - rad, rad * 2.0, rad * 2.0));
                let text = format!("{i}");
                let mut drawn = None;
                if view.numbers && rank < MAX_NUMBERED {
                    // the first free spot beside the mark; none free, no number
                    let (w, h) = (label.width(&text), theme.small_size() * 1.1);
                    let g = rad * 0.9;
                    let spots = [
                        (px + g, py - h - g * 0.2),
                        (px + g, py + g * 0.2),
                        (px - g - w, py - h - g * 0.2),
                        (px - g - w, py + g * 0.2),
                    ];
                    if let Some(b) = spots
                        .iter()
                        .map(|&(x, y)| Rect::new(x, y, w, h))
                        .find(|b| placer.free(b))
                    {
                        fig.text(b.x, b.y, &text, &label);
                        placer.reserve(b);
                        drawn = Some(text);
                    }
                }
                marked += 1;
                marks.push(DataMark {
                    kind: "cross_peak".into(),
                    x: f2,
                    y,
                    label: drawn,
                    channel: None,
                });
            }
        });
    }
    if levels.is_empty() {
        notes.push(format!(
            "nothing in the window rises above {:.1} × the noise SD ({noise:.3e}); lower contour_floor",
            view.floor
        ));
    }
    let shown = |d: &DimensionAxis, a: &openreadout_signal::nmr::PpmAxis| {
        (d.show(a.first_ppm), d.show(a.last_ppm()))
    };
    let (f1a, f1b) = shown(d1, &pooled.axes[0]);
    let summary = Nmr2dPreview {
        trace,
        shape: pooled.full_shape.clone(),
        axes: vec![
            Nmr2dAxis {
                dimension: "f1".into(),
                nucleus: d1.nucleus.clone(),
                quantity: d1.quantity.clone(),
                unit: d1.unit.clone(),
                first: f1a.max(f1b),
                last: f1a.min(f1b),
            },
            Nmr2dAxis {
                dimension: "f2".into(),
                nucleus: d2.nucleus.clone(),
                quantity: d2.quantity.clone(),
                unit: d2.unit.clone(),
                first: x_hi,
                last: x_lo,
            },
        ],
        levels: levels.clone(),
        floor_snr: view.floor,
        noise_sd: noise,
        points_per_cell: [pooled.factor.0, pooled.factor.1],
        negative,
        cross_peaks: marked,
    };
    Ok(Drawn {
        panel,
        summary,
        marks,
        taken: placer.taken().to_vec(),
        notes,
    })
}

/// Shorten `text` with an ellipsis to fit `max_w`.
fn fit_text(text: &str, st: &TextStyle, max_w: f32) -> String {
    if st.width(text) <= max_w {
        return text.to_string();
    }
    let mut s: Vec<char> = text.chars().collect();
    while !s.is_empty() && st.width(&format!("{}…", s.iter().collect::<String>())) > max_w {
        s.pop();
    }
    format!("{}…", s.iter().collect::<String>())
}

/// The skyline projections: onto F2 along the top, onto F1 along the left (values growing to
/// the left), positive in ink and, when the spectrum has negative peaks, their magnitude in
/// the accent colour.
#[allow(clippy::too_many_arguments)]
fn draw_projections(
    fig: &mut Figure,
    theme: &Theme,
    v: &PooledView,
    panel: &Panel,
    d1: &DimensionAxis,
    top: Rect,
    left: Rect,
    negative: bool,
) {
    let pos_top = v.f2_projection.0.iter().copied().fold(0.0f64, f64::max);
    let neg_top = -v.f2_projection.1.iter().copied().fold(0.0f64, f64::min);
    let scale_x = pos_top.max(if negative { neg_top } else { 0.0 });
    let pos_left = v.f1_projection.0.iter().copied().fold(0.0f64, f64::max);
    let neg_left = -v.f1_projection.1.iter().copied().fold(0.0f64, f64::min);
    let scale_y = pos_left.max(if negative { neg_left } else { 0.0 });
    let lw = (theme.line_width * 0.7).max(0.8);
    // one value per pixel: the largest of the points it covers
    let columns =
        |values: &[f64], sign: f64, axis: &openreadout_signal::nmr::PpmAxis, along_x: bool| {
            let mut px: HashMap<i32, f64> = HashMap::new();
            for (k, &val) in values.iter().enumerate() {
                let ppm = axis.ppm(k as f64);
                let p = if along_x {
                    panel.x.px(ppm)
                } else {
                    panel.y.px(d1.show(ppm))
                };
                if let Some(p) = p {
                    let e = px.entry(p.round() as i32).or_insert(f64::NEG_INFINITY);
                    *e = e.max(sign * val);
                }
            }
            let mut out: Vec<(i32, f64)> = px.into_iter().collect();
            out.sort_by_key(|p| p.0);
            out
        };
    fig.group(Layer::Series, "projection f2", |fig| {
        fig.line(
            top.x,
            top.bottom(),
            top.right(),
            top.bottom(),
            Stroke::new(theme.rule, theme.hairline()),
        );
        if scale_x <= 0.0 {
            return;
        }
        let mut series = vec![(1.0, theme.ink)];
        if negative {
            series.push((-1.0, theme.accent));
        }
        for (sign, color) in series {
            let pts: Vec<(f32, f32)> =
                columns(&v.f2_projection.choose(sign), sign, &v.axes[1], true)
                    .into_iter()
                    .map(|(x, y)| {
                        let t = (y.max(0.0) / scale_x) as f32;
                        (x as f32, top.bottom() - t * (top.h - 1.0))
                    })
                    .collect();
            fig.shape(
                Path::polyline(pts),
                None,
                Some(Stroke::new(color, lw)),
                Some(top),
            );
        }
    });
    fig.group(Layer::Series, "projection f1", |fig| {
        fig.line(
            left.right(),
            left.y,
            left.right(),
            left.bottom(),
            Stroke::new(theme.rule, theme.hairline()),
        );
        if scale_y <= 0.0 {
            return;
        }
        let mut series = vec![(1.0, theme.ink)];
        if negative {
            series.push((-1.0, theme.accent));
        }
        for (sign, color) in series {
            let pts: Vec<(f32, f32)> =
                columns(&v.f1_projection.choose(sign), sign, &v.axes[0], false)
                    .into_iter()
                    .map(|(y, x)| {
                        let t = (x.max(0.0) / scale_y) as f32;
                        (left.right() - t * (left.w - 1.0), y as f32)
                    })
                    .collect();
            fig.shape(
                Path::polyline(pts),
                None,
                Some(Stroke::new(color, lw)),
                Some(left),
            );
        }
    });
}

/// The positive (`sign` > 0) or negative projection of a pair.
trait Choose {
    fn choose(&self, sign: f64) -> Vec<f64>;
}

impl Choose for (Vec<f64>, Vec<f64>) {
    fn choose(&self, sign: f64) -> Vec<f64> {
        if sign > 0.0 {
            self.0.clone()
        } else {
            self.1.clone()
        }
    }
}

/// The view a preview request asks for.
pub(crate) fn view_of(req: &PreviewRequest, trace: Option<u32>) -> Nmr2dView {
    Nmr2dView {
        trace,
        x_range: req.x_range,
        y_range: req.y_range,
        floor: req.contour_floor.unwrap_or(DEFAULT_CONTOUR_FLOOR),
        levels: req.contour_levels.unwrap_or(DEFAULT_CONTOUR_LEVELS),
        markers: !matches!(req.marks, Marks::None),
        numbers: matches!(req.marks, Marks::Peaks),
    }
}

/// The 2D preview of an NMR data set: a figure `max_size` wide.
pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    trace: Option<u32>,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    let theme = crate::theme(req);
    let w = req.max_size.max(64) as f32;
    let h = w;
    let mut fig = Figure::new(w, h, Some(theme.paper));
    let pad = theme.font_size * 0.6;
    let area = Rect::new(pad, pad * 0.6, w - pad * 2.0, h - pad * 1.2);
    let view = view_of(req, trace);
    let d = draw(&mut fig, area, &theme, ds, info, &view, None)?;
    out.kind = "nmr_2d".into();
    out.marks = d.marks;
    out.notes.extend(d.notes);
    out.notes.push(
        "a 2D spectrum as contours (F2 along the bottom, F1 on the right, both high to low); zoom with x_range (F2) and y_range (F1), slice it with openreadout_trace slice".into(),
    );
    out.nmr_2d = Some(d.summary);
    let panels = vec![FramePanel::new(d.panel)];
    Ok((fig, Frame::plots_labelled(panels, &d.taken)))
}

/// A figure of `width` px with the cross peaks of `report` marked and numbered.
pub fn report_figure(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    report: &Nmr2dReport,
    width: u32,
    dark: bool,
) -> Result<Figure> {
    let theme = if dark { Theme::dark() } else { Theme::light() }.for_size(width as f32);
    let w = width.max(64) as f32;
    let mut fig = Figure::new(w, w, Some(theme.paper));
    let pad = theme.font_size * 0.6;
    let area = Rect::new(pad, pad * 0.6, w - pad * 2.0, w - pad * 1.2);
    let n = report.axes.len();
    let range = |d: usize| {
        report
            .peak_options
            .ranges_ppm
            .get(d)
            .copied()
            .flatten()
            .map(|(a, b)| [a, b])
    };
    let dim1 = report.dimensions.get(n.saturating_sub(2)).cloned();
    let view = Nmr2dView {
        trace: Some(report.trace),
        x_range: range(n - 1),
        y_range: range(n.saturating_sub(2))
            .map(|[a, b]| dim1.as_ref().map_or([a, b], |d| [d.show(a), d.show(b)])),
        markers: true,
        numbers: true,
        ..Nmr2dView::default()
    };
    draw(&mut fig, area, &theme, ds, info, &view, Some(report))?;
    fig.title = Some(format!("cross peaks of {}", report.path));
    Ok(fig)
}

/// Most rows a stack draws (spread evenly over the series).
const MAX_STACK_ROWS: usize = 48;

/// A pseudo-2D series (DOSY, T1/T2, kinetics) as a stack of 1D spectra, one per row, each
/// lifted by the same step and labelled with the value that was varied (`extra.series`).
pub(crate) fn render_stack(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    trace: u32,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    use openreadout_signal::nmr::source::spectrum_axis;
    let t = info
        .traces
        .iter()
        .find(|t| t.index == trace)
        .ok_or_else(|| Error::Usage(format!("trace {trace} out of range")))?
        .clone();
    let axis = spectrum_axis(&t).ok_or_else(|| {
        Error::unsupported(
            "preview",
            format!("trace {trace} has no shift axis"),
            "Pick a processed spectrum (see `openreadout info`).",
        )
    })?;
    let series: Option<openreadout_signal::nmr::Series> = t
        .extra
        .get("series")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let rows = t.sweep_count.max(1) as usize;
    let step = rows.div_ceil(MAX_STACK_ROWS).max(1);
    let picked: Vec<usize> = (0..rows).step_by(step).collect();
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let w = req.max_size.max(64) as f32;
    let h = (w * 0.75).min(req.max_size as f32);
    let n = (t.sample_count as usize).min(axis.size);
    let (x0, x1) = (axis.ppm(0.0), axis.last_ppm());
    let (mut lo, mut hi) = (x0.min(x1), x0.max(x1));
    if let Some([a, b]) = req.x_range {
        lo = a.min(b).max(lo);
        hi = a.max(b).min(hi);
    }
    let x_dom = Domain::linear(lo, hi.max(lo + 1e-9)).reversed();
    let ncols = (w as usize).max(16);
    // per row: one (min, max) per pixel column
    let mut stacks: Vec<Vec<Option<(f64, f64)>>> = Vec::new();
    let mut top = 0.0f64;
    for &r in &picked {
        let tr = ds.read_trace(trace, r as u32, 0, n as u64)?;
        let v = tr.channels.into_iter().next().unwrap_or_default();
        let mut cols: Vec<Option<(f64, f64)>> = vec![None; ncols];
        for (k, &y) in v.iter().enumerate() {
            let x = axis.ppm(k as f64);
            if !(y.is_finite() && x >= lo && x <= hi) {
                continue;
            }
            let c = (((hi - x) / (hi - lo).max(1e-12)) * (ncols - 1) as f64) as usize;
            let e = cols[c.min(ncols - 1)].get_or_insert((y, y));
            e.0 = e.0.min(y);
            e.1 = e.1.max(y);
            top = top.max(y.abs());
        }
        stacks.push(cols);
    }
    let header = fs * 2.0;
    let left = fs * 6.5;
    let rect = Rect::new(
        left,
        header + fs * 0.6,
        w - left - fs * 1.6,
        h - header - fs * 4.4,
    );
    let m = picked.len().max(1) as f64;
    // each row gets one step; its tallest line reaches three steps
    let y_dom = Domain::linear(-0.3, m + 2.2);
    let panel = Panel::new(rect, x_dom, y_dom);
    let mut fig = Figure::new(w, h, Some(theme.paper));
    let spec = AxesSpec {
        x_label: Some(match t.extra.get("nucleus").and_then(|v| v.as_str()) {
            Some(nu) => format!("{} (ppm)", nucleus_label(nu)),
            None => "chemical shift (ppm)".into(),
        }),
        show_y: false,
        grid: GridLines::None,
        x_ticks: 10,
        ..AxesSpec::default()
    };
    panel.axes(&mut fig, &theme, &spec);
    let scale = if top > 0.0 { 3.0 / top } else { 1.0 };
    let label =
        TextStyle::new(theme.small_size(), theme.muted).align(Anchor::End, Baseline::Middle);
    let value_of = |r: usize| -> String {
        match series.as_ref().and_then(|s| s.values.get(r)) {
            Some(v) => openreadout_figure::format_value(*v),
            None => format!("row {r}"),
        }
    };
    let every = (picked.len() / 12).max(1);
    fig.group(Layer::Series, "stack", |fig| {
        for (i, (cols, &r)) in stacks.iter().zip(&picked).enumerate().rev() {
            let base = i as f64;
            let mut pts = Vec::new();
            for (c, v) in cols.iter().enumerate() {
                let Some((a, b)) = v else { continue };
                let x = hi - (c as f64 + 0.5) / ncols as f64 * (hi - lo);
                for y in [*a, *b] {
                    if let Some(p) = panel.pt(x, base + y * scale) {
                        pts.push(p);
                    }
                }
            }
            let t = if picked.len() > 1 {
                i as f32 / (picked.len() - 1) as f32
            } else {
                0.0
            };
            let color = theme.ink.mix(theme.series(2), t);
            panel.line_px(
                fig,
                pts,
                Stroke::new(color, (theme.line_width * 0.6).max(0.7)),
            );
            if i % every == 0
                && let Some((_, py)) = panel.pt(hi, base)
            {
                fig.text(rect.x - fs * 0.4, py, &value_of(r), &label);
            }
        }
    });
    let what = series
        .as_ref()
        .map(|s| match &s.unit {
            Some(u) => format!("{} ({u})", s.name),
            None => s.name.clone(),
        })
        .unwrap_or_else(|| "row".into());
    let program = slices2d::pulse_program(info);
    let caption = [
        program
            .as_deref()
            .and_then(experiment_name)
            .unwrap_or("pseudo-2D series")
            .to_string(),
        format!(
            "{} rows{}",
            rows,
            if step > 1 {
                format!(", every {step}th drawn")
            } else {
                String::new()
            }
        ),
        format!("left: {what}"),
    ]
    .join(" · ");
    fig.group(Layer::Caption, "caption", |fig| {
        fig.text(
            fs * 0.6,
            fs * 1.3,
            &caption,
            &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
        );
    });
    out.kind = "trace".into();
    out.notes.push(format!(
        "a pseudo-2D series: one spectrum per row, lifted by row and labelled with {what}; pick 1D peaks of a row with openreadout_nmr_peaks sweep"
    ));
    Ok((
        fig,
        Frame::plots_labelled(vec![FramePanel::new(panel)], &[]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_geometric_and_bounded() {
        let l = contour_levels(10.0, 10_000.0, 12);
        assert!(l.len() <= 12 && l.len() >= 10);
        let f = l[1] / l[0];
        assert!(f >= 1.25 && f <= 2.0);
        assert!((l[2] / l[1] - f).abs() < 1e-9);
        assert!(*l.last().unwrap() < 10_000.0);
        // a small dynamic range still gives a few levels, 1.25 apart
        let l = contour_levels(10.0, 20.0, 12);
        assert_eq!(l.len(), 4);
        assert!(contour_levels(0.0, 1.0, 12).is_empty());
    }

    #[test]
    fn a_single_peak_gives_a_closed_contour() {
        // a 5 × 5 grid with a peak in the middle
        let mut g = vec![0.0; 25];
        g[12] = 10.0;
        let xs: Vec<f32> = (0..5).map(|i| i as f32 * 10.0).collect();
        let p = contour_path(&g, 5, 5, 5.0, &xs, &xs);
        let segs = p.segments();
        // one polyline: a move and four lines around the middle point, closed on itself
        let moves = segs
            .iter()
            .filter(|s| matches!(s, openreadout_figure::Segment::MoveTo(..)))
            .count();
        assert_eq!(moves, 1);
        let (x0, y0, x1, y1) = p.bounds().unwrap();
        assert!((x0 - 15.0).abs() < 1e-3 && (x1 - 25.0).abs() < 1e-3);
        assert!((y0 - 15.0).abs() < 1e-3 && (y1 - 25.0).abs() < 1e-3);
        // nothing above the level: no path
        assert!(contour_path(&g, 5, 5, 20.0, &xs, &xs).is_empty());
    }

    #[test]
    fn experiments_are_named_from_the_pulse_program() {
        assert_eq!(experiment_name("hsqcedetgpsisp2.3"), Some("edited HSQC"));
        assert_eq!(experiment_name("hmbcetgpl3nd"), Some("HMBC"));
        assert_eq!(experiment_name("cosygpppqf"), Some("COSY"));
        assert_eq!(experiment_name("noesygpphpp"), Some("NOESY"));
        assert_eq!(experiment_name("ledbpgp2s"), Some("DOSY"));
        assert_eq!(experiment_name("jresgpprqf"), Some("J-resolved"));
        assert_eq!(experiment_name("zg30"), None);
    }
}
