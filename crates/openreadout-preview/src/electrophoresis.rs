//! Capillary electrophoresis previews. A Sanger trace is drawn as the chromatogram people read:
//! the four bases' traces, the called base above each peak in its colour, the quality of each
//! call as a bar underneath, and the whole read's quality in a strip above with the window
//! shown. The x axis is the base position, so `x_range` and annotations are in bases. A fragment
//! analysis run is drawn as electropherograms on a base-pair axis: one panel per dye with its
//! tallest peaks labelled by size, and the size standard at the bottom with each fragment
//! labelled by its length.

use openreadout_core::model::{FileInfo, TableInfo, TraceInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Color, Domain, Face, Figure, GridLines, LabelPlacer, Panel, Rect,
    Stroke, TextStyle, Theme,
};
use serde_json::Value;

use crate::color::to_hex;
use crate::marks::{self, Col};
use crate::trace::envelope_points;
use crate::{
    DataMark, Frame, FramePanel, PreviewOutput, PreviewRequest, TracePreview, TracePreviewChannel,
    XAxis,
};

/// Width of one base in a default Sanger window, pixels.
const BASE_PX: f32 = 13.0;
/// Most bases a Sanger window shows.
const MAX_WINDOW_BASES: usize = 2000;
/// Peaks labelled per dye panel of an electropherogram.
const MAX_FRAGMENT_LABELS: usize = 10;

/// The theme series colour (index into `Theme::series`) that shows a base or a dye the way
/// analysis software does, in the figure engine's palette: A green, C blue, G ink, T red; FAM
/// blue, VIC/HEX/JOE green, NED/TAMRA ochre (yellow on screen), PET/ROX red, LIZ brown (orange
/// on screen). `None` for other names. The viewer colours the same series by this index.
pub fn series_index_of(name: &str) -> Option<usize> {
    let d = name.trim().to_ascii_uppercase();
    Some(match d.as_str() {
        "A" => 4,
        "C" => 2,
        "G" => 0,
        "T" => 1,
        _ if d.contains("FAM") || d.contains("BLUE") => 2,
        _ if ["VIC", "HEX", "JOE", "GREEN"].iter().any(|k| d.contains(k)) => 4,
        _ if ["NED", "TAMRA", "TET", "YELLOW"]
            .iter()
            .any(|k| d.contains(k)) =>
        {
            3
        }
        _ if ["PET", "ROX", "RED"].iter().any(|k| d.contains(k)) => 1,
        _ if d.contains("LIZ") || d.contains("ORANGE") => 7,
        _ => return None,
    })
}

/// The colour of a base or dye; `k` picks a series colour for names the rule does not know.
fn trace_color(theme: &Theme, name: &str, k: usize) -> Color {
    theme.series(series_index_of(name).unwrap_or(k + 2))
}

/// Draw the file's own picture when it is a Sanger read or a sized fragment run and the request
/// leaves the choice to the preview (no channels; no trace, or the trace this picture shows).
pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<Option<(Figure, Frame)>> {
    if !req.channels.is_empty() || req.sweep.is_some_and(|s| s > 0) {
        return Ok(None);
    }
    if let Some((calls, trace)) = sanger_parts(info) {
        if req.trace.is_some_and(|t| t != trace.index) {
            return Ok(None);
        }
        return sanger(ds, info, req, out, calls, trace).map(Some);
    }
    if let Some(trace) = sized_trace(info) {
        if req.trace.is_some_and(|t| t != trace.index) {
            return Ok(None);
        }
        return electropherogram(ds, info, req, out, trace).map(Some);
    }
    Ok(None)
}

fn kind(extra: &std::collections::BTreeMap<String, Value>) -> Option<&str> {
    extra.get("kind").and_then(Value::as_str)
}

/// The base-call table and the trace its `scan` column indexes.
fn sanger_parts(info: &FileInfo) -> Option<(&TableInfo, &TraceInfo)> {
    let calls = info
        .tables
        .iter()
        .find(|t| kind(&t.extra) == Some("base_calls"))?;
    if !calls.columns.iter().any(|c| c.name == "scan") {
        return None;
    }
    let trace = info.traces.iter().find(|t| {
        t.extra.get("base_calls_table").and_then(Value::as_u64) == Some(u64::from(calls.index))
    })?;
    Some((calls, trace))
}

/// The electropherogram on a size axis.
fn sized_trace(info: &FileInfo) -> Option<&TraceInfo> {
    info.traces.iter().find(|t| {
        kind(&t.extra) == Some("electropherogram")
            && t.extra
                .get("axis")
                .and_then(|a| a.get("quantity"))
                .and_then(Value::as_str)
                == Some("size")
    })
}

/// A column of a table by name.
fn column(
    table: &openreadout_core::model::Table,
    info: &TableInfo,
    name: &str,
) -> Option<Vec<f64>> {
    let k = info.columns.iter().position(|c| c.name == name)?;
    table.columns.get(k).cloned()
}

/// Text of a coded column.
fn texts(
    table: &openreadout_core::model::Table,
    info: &TableInfo,
    name: &str,
) -> Option<Vec<String>> {
    let k = info.columns.iter().position(|c| c.name == name)?;
    let cats: Vec<String> = info.columns[k]
        .extra
        .get("categories")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    Some(
        table.columns[k]
            .iter()
            .map(|c| cats.get(*c as usize).cloned().unwrap_or_default())
            .collect(),
    )
}

/// A short description of the run for the figure's header.
fn header_parts(ds: &dyn Dataset, info: &FileInfo) -> Vec<String> {
    let e = openreadout_core::experiment::of_dataset(ds, info);
    let mut parts = Vec::new();
    if let Some(n) = e.sample.as_ref().and_then(|s| s.name.clone()) {
        parts.push(n);
    }
    if let Some(m) = e.instrument.as_ref().and_then(|i| i.model.clone()) {
        parts.push(m);
    }
    parts
}

/// Piecewise-linear map from scans to fractional base positions through the called peaks
/// (`scans[i]` is base `i + 1`), extended past the ends by the median spacing.
struct BaseAxis {
    scans: Vec<f64>,
    spacing: f64,
}

impl BaseAxis {
    fn new(scans: Vec<f64>) -> Option<BaseAxis> {
        if scans.len() < 2 || scans.windows(2).any(|w| !(w[1] > w[0])) {
            return None;
        }
        let mut d: Vec<f64> = scans.windows(2).map(|w| w[1] - w[0]).collect();
        d.sort_by(f64::total_cmp);
        let spacing = d[d.len() / 2].max(1e-6);
        Some(BaseAxis { scans, spacing })
    }

    /// Base position (1-based) of a scan.
    fn pos(&self, scan: f64) -> f64 {
        let s = &self.scans;
        let n = s.len();
        if scan <= s[0] {
            return 1.0 - (s[0] - scan) / self.spacing;
        }
        if scan >= s[n - 1] {
            return n as f64 + (scan - s[n - 1]) / self.spacing;
        }
        let k = s.partition_point(|v| *v <= scan) - 1;
        k as f64 + 1.0 + (scan - s[k]) / (s[k + 1] - s[k])
    }

    /// Scan of a base position.
    fn scan(&self, pos: f64) -> f64 {
        let s = &self.scans;
        let n = s.len();
        if pos <= 1.0 {
            return s[0] - (1.0 - pos) * self.spacing;
        }
        if pos >= n as f64 {
            return s[n - 1] + (pos - n as f64) * self.spacing;
        }
        let k = (pos.floor() as usize).clamp(1, n - 1);
        let t = pos - k as f64;
        s[k - 1] + t * (s[k] - s[k - 1])
    }
}

/// The first base from which the next 20 average quality 20 or more.
fn good_start(q: &[f64]) -> usize {
    let w = 20.min(q.len());
    if w == 0 {
        return 0;
    }
    let mut sum: f64 = q[..w].iter().sum();
    for i in 0..=q.len() - w {
        if i > 0 {
            sum += q[i + w - 1] - q[i - 1];
        }
        if sum / w as f64 >= 20.0 {
            return i;
        }
    }
    0
}

#[allow(clippy::too_many_lines)]
fn sanger(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
    calls_info: &TableInfo,
    t: &TraceInfo,
) -> Result<(Figure, Frame)> {
    let calls = ds.read_table(calls_info.index, 0, calls_info.row_count)?;
    let bases = texts(&calls, calls_info, "base").unwrap_or_default();
    let scans = column(&calls, calls_info, "scan").unwrap_or_default();
    // basecallers without quality values (the ABI basecaller of the 310) write zeros
    let quality = column(&calls, calls_info, "quality").filter(|q| q.iter().any(|v| *v > 0.0));
    let n = bases.len().min(scans.len());
    let axis = BaseAxis::new(scans[..n].to_vec()).ok_or_else(|| {
        Error::unsupported(
            "preview",
            "a Sanger trace whose called peaks do not rise",
            "Preview the trace itself with trace and channels, or read the calls with openreadout_table.",
        )
    })?;
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let w = req.max_size.max(320) as f32;
    let left = fs * 3.6;
    let right = fs * 1.2;
    let plot_w = w - left - right;
    // the window, in bases
    let (lo, hi) = match req.x_range {
        Some([a, b]) => {
            let (a, b) = (a.min(b), a.max(b));
            let a = a.max(0.5).min(n as f64 + 0.5);
            let b = b.max(a + 1.0).min(a + MAX_WINDOW_BASES as f64);
            (a, b)
        }
        None => {
            let per = ((plot_w / BASE_PX) as usize).clamp(20, 160);
            let start = quality.as_deref().map_or(0, good_start);
            let first = start + 1;
            let last = (first + per - 1).min(n.max(1));
            let first = last.saturating_sub(per - 1).max(1);
            (first as f64 - 0.5, last as f64 + 0.5)
        }
    };
    let (scan_lo, scan_hi) = (axis.scan(lo).floor().max(0.0), axis.scan(hi).ceil());
    let sweep_len = openreadout_core::trace::sweep_samples(t, 0);
    let s0 = (scan_lo as u64).min(sweep_len);
    let s1 = (scan_hi.max(0.0) as u64).min(sweep_len);
    let tr = ds.read_trace(t.index, 0, s0, s1.saturating_sub(s0))?;
    let base_channels: Vec<(u32, String)> = t
        .channels
        .iter()
        .filter(|c| matches!(c.name.as_str(), "A" | "C" | "G" | "T"))
        .map(|c| (c.index, c.name.clone()))
        .collect();
    let y_hi = base_channels
        .iter()
        .filter_map(|(c, _)| tr.channels.get(*c as usize))
        .flat_map(|v| v.iter().copied())
        .filter(|v| v.is_finite())
        .fold(1.0f64, f64::max);

    // layout
    let header = fs * 1.9;
    let strip_h = fs * 2.4;
    let strip_gap = fs * 1.6;
    let letters_h = fs * 1.6;
    let main_h = (w * 0.30).clamp(150.0, 380.0);
    let qual_h = (w * 0.07).clamp(36.0, 80.0);
    let axis_h = fs * 2.6;
    let has_q = quality.is_some();
    let height = header
        + if has_q { strip_h + strip_gap } else { 0.0 }
        + letters_h
        + main_h
        + if has_q { fs * 0.4 + qual_h } else { 0.0 }
        + axis_h;
    let mut fig = Figure::new(w, height.ceil(), Some(theme.paper));
    let mut placer = LabelPlacer::new(fig.rect());

    // header
    let mut parts = header_parts(ds, info);
    if let Some(q) = &quality {
        let mean = q.iter().sum::<f64>() / q.len().max(1) as f64;
        let q20 = q.iter().filter(|v| **v >= 20.0).count();
        parts.push(format!("{n} bases · mean QV {mean:.0} · {q20} at QV 20+"));
    } else {
        parts.push(format!("{n} bases"));
    }
    fig.text(
        fs * 0.8,
        fs * 1.4,
        &parts.join(" · "),
        &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
    );
    let small = theme.small_size();
    let label_style = TextStyle::new(small, theme.muted).align(Anchor::End, Baseline::Middle);
    let mut y = header;
    let mut panels = Vec::new();

    // the read's quality, base by base, with the window marked
    if let Some(q) = &quality {
        let rect = Rect::new(left, y, plot_w, strip_h);
        let p = Panel::new(
            rect,
            Domain::linear(0.5, n as f64 + 0.5),
            Domain::linear(0.0, q.iter().copied().fold(40.0, f64::max)),
        );
        fig.fill_rect(rect, theme.panel);
        p.band_x(&mut fig, lo, hi, theme.band);
        let bars: Vec<(f64, f64)> = q
            .iter()
            .enumerate()
            .map(|(i, v)| (i as f64 + 1.0, *v))
            .collect();
        p.bars(&mut fig, &bars, 1.0, 0.0, theme.muted.alpha(0.45));
        let (a, b) = (
            p.x.px(lo).unwrap_or(rect.x),
            p.x.px(hi).unwrap_or(rect.right()),
        );
        let edge = Stroke::new(theme.accent, theme.hairline());
        fig.line(a, rect.y, a, rect.bottom(), edge.clone());
        fig.line(b, rect.y, b, rect.bottom(), edge);
        fig.text(left - fs * 0.5, rect.y + rect.h / 2.0, "QV", &label_style);
        let tick = TextStyle::new(small, theme.muted).align(Anchor::Start, Baseline::Top);
        fig.text(rect.x, rect.bottom() + 2.0, "1", &tick);
        fig.text(
            rect.right(),
            rect.bottom() + 2.0,
            &n.to_string(),
            &TextStyle::new(small, theme.muted).align(Anchor::End, Baseline::Top),
        );
        y += strip_h + strip_gap;
    }

    // the traces, on a base-position axis
    let x_dom = Domain::linear(lo, hi);
    let letters_y = y + letters_h * 0.55;
    y += letters_h;
    let rect = Rect::new(left, y, plot_w, main_h);
    let main = Panel::new(rect, x_dom, Domain::linear(0.0, y_hi * 1.04));
    fig.line(
        rect.x,
        rect.bottom(),
        rect.right(),
        rect.bottom(),
        Stroke::new(theme.rule, theme.hairline()),
    );
    let ncols = (plot_w as usize).max(16);
    let mut report = Vec::new();
    for (c, name) in &base_channels {
        let Some(v) = tr.channels.get(*c as usize) else {
            continue;
        };
        let color = trace_color(&theme, name, 0);
        let samples = v.len();
        if samples <= ncols * 2 {
            let pts = v
                .iter()
                .enumerate()
                .map(|(i, y)| (axis.pos(s0 as f64 + i as f64), *y));
            main.line(&mut fig, pts, Stroke::new(color, theme.line_width));
        } else {
            let mut cols: Vec<Option<Col>> = vec![None; ncols];
            for (i, &yv) in v.iter().enumerate() {
                let x = axis.pos(s0 as f64 + i as f64);
                let t = (x - lo) / (hi - lo);
                if !(0.0..=1.0).contains(&t) || !yv.is_finite() {
                    continue;
                }
                let k = ((t * ncols as f64) as usize).min(ncols - 1);
                match &mut cols[k] {
                    None => cols[k] = Some(Col::new(x, yv)),
                    Some(col) => col.add(x, yv),
                }
            }
            main.line_px(
                &mut fig,
                envelope_points(&cols, &main),
                Stroke::new(color, theme.line_width * 0.85),
            );
        }
        report.push(TracePreviewChannel {
            index: *c,
            name: name.clone(),
            unit: t.channels[*c as usize].unit.clone(),
            color: to_hex(color.to_rgb()),
            min: Some(0.0),
            max: Some(y_hi * 1.04),
        });
    }
    // called bases above their peaks, when there is room for letters
    let px_per_base = plot_w as f64 / (hi - lo);
    let first_base = (lo.ceil().max(1.0)) as usize;
    let last_base = (hi.floor() as usize).min(n);
    let letters = px_per_base >= f64::from(small) * 0.75;
    let mut marks_out: Vec<DataMark> = Vec::new();
    if letters {
        let lsize = (small * 1.05).min((px_per_base as f32) * 1.25);
        for i in first_base..=last_base {
            let b = &bases[i - 1];
            let Some(px) = main.x.px(i as f64) else {
                continue;
            };
            let st = TextStyle::new(
                lsize,
                series_index_of(b).map_or(theme.muted, |i| theme.series(i)),
            )
            .face(Face::Mono)
            .align(Anchor::Middle, Baseline::Middle);
            let r = fig.text(px, letters_y, b, &st);
            placer.reserve(r);
        }
    } else {
        out.notes.push(format!(
            "{} bases are too many to letter at this size; give x_range of about {} bases to see the calls",
            last_base.saturating_sub(first_base) + 1,
            (plot_w / BASE_PX) as usize
        ));
    }
    // base numbers every 10 (or every 50/100 when crowded)
    let every = [10usize, 20, 50, 100, 200, 500]
        .into_iter()
        .find(|e| px_per_base * *e as f64 >= f64::from(fs) * 3.0)
        .unwrap_or(1000);
    y += main_h;
    panels.push(FramePanel {
        channel: None,
        ..FramePanel::new(main)
    });

    // quality of each call in the window
    if let Some(q) = &quality {
        y += fs * 0.4;
        let rect = Rect::new(left, y, plot_w, qual_h);
        let q_hi = q.iter().copied().fold(40.0, f64::max);
        let qp = Panel::new(rect, x_dom, Domain::linear(0.0, q_hi));
        let width = 0.72;
        let (good, poor): (Vec<(f64, f64)>, Vec<(f64, f64)>) = (first_base..=last_base)
            .map(|i| (i as f64, q[i - 1]))
            .partition(|(_, v)| *v >= 20.0);
        qp.bars(&mut fig, &good, width, 0.0, theme.muted.alpha(0.5));
        qp.bars(&mut fig, &poor, width, 0.0, theme.accent.alpha(0.8));
        qp.rule_y(
            &mut fig,
            20.0,
            Stroke::new(theme.rule, theme.hairline()).dashed(3.0, 3.0),
        );
        fig.text(left - fs * 0.5, rect.y + rect.h / 2.0, "QV", &label_style);
        fig.text(
            rect.right() + 2.0,
            qp.y.px(20.0).unwrap_or(rect.y),
            "20",
            &TextStyle::new(small * 0.9, theme.muted).align(Anchor::Start, Baseline::Middle),
        );
        y += qual_h;
        panels.push(FramePanel::new(qp));
    }
    // the x axis: base positions
    let tick_style = TextStyle::new(small, theme.muted)
        .face(Face::Mono)
        .align(Anchor::Middle, Baseline::Top);
    let first_tick = first_base.div_ceil(every) * every;
    let mut k = first_tick.max(every);
    while k <= last_base {
        if let Some(px) = main.x.px(k as f64) {
            fig.line(
                px,
                y,
                px,
                y + fs * 0.35,
                Stroke::new(theme.rule, theme.hairline()),
            );
            fig.text(px, y + fs * 0.5, &k.to_string(), &tick_style);
        }
        k += every;
    }
    fig.text(
        w - right,
        y + fs * 1.9,
        "base position",
        &TextStyle::new(small, theme.muted).align(Anchor::End, Baseline::Top),
    );
    if letters {
        for i in first_base..=last_base {
            marks_out.push(DataMark {
                kind: "base".into(),
                x: i as f64,
                y: q_or(&quality, i),
                label: Some(bases[i - 1].clone()),
                channel: None,
            });
        }
    }
    out.marks = marks_out;
    out.notes.push(format!(
        "bases {}–{} of {n}; x_range [a, b] (base positions) shows another window",
        first_base, last_base
    ));
    out.trace = Some(TracePreview {
        trace: t.index,
        sweep: 0,
        sample_rate_hz: t.sample_rate_hz,
        sweep_sample_count: sweep_len,
        sample_count: s1.saturating_sub(s0),
        truncated: s1.saturating_sub(s0) < sweep_len,
        x_axis: Some(XAxis {
            quantity: "base_position".into(),
            unit: String::new(),
            first: lo,
            last: hi,
        }),
        channels: report,
    });
    Ok((fig, Frame::plots_labelled(panels, placer.taken())))
}

fn q_or(q: &Option<Vec<f64>>, i: usize) -> f64 {
    q.as_ref()
        .and_then(|q| q.get(i - 1).copied())
        .unwrap_or(f64::NAN)
}

/// A fragment run's peaks to label in one dye: (size, height, label), tallest first.
fn dye_peaks(
    peaks: Option<&(openreadout_core::model::Table, TableInfo)>,
    dye: &str,
    lo: f64,
    hi: f64,
) -> Option<Vec<(f64, f64)>> {
    let (t, info) = peaks?;
    let dyes = texts(t, info, "dye")?;
    let size = column(t, info, "size")?;
    let height = column(t, info, "height")?;
    let mut v: Vec<(f64, f64)> = dyes
        .iter()
        .zip(size.iter().zip(&height))
        .filter(|(d, (s, _))| d.trim() == dye.trim() && s.is_finite() && **s >= lo && **s <= hi)
        .map(|(_, (s, h))| (*s, *h))
        .collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    Some(v)
}

/// The median and the noise (median absolute first difference, as a standard deviation) of
/// `y` where `x` lies in `lo..=hi`.
fn noise_in(x: &[f64], y: &[f64], lo: f64, hi: f64) -> (f64, f64) {
    let v: Vec<f64> = x
        .iter()
        .zip(y)
        .filter(|(a, b)| **a >= lo && **a <= hi && b.is_finite())
        .map(|(_, b)| *b)
        .collect();
    if v.len() < 3 {
        return (0.0, 0.0);
    }
    let mut s = v.clone();
    s.sort_by(f64::total_cmp);
    let median = s[s.len() / 2];
    let mut d: Vec<f64> = v.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    d.sort_by(f64::total_cmp);
    (median, d[d.len() / 2] * 1.4826 / std::f64::consts::SQRT_2)
}

#[allow(clippy::too_many_lines)]
fn electropherogram(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
    t: &TraceInfo,
) -> Result<(Figure, Frame)> {
    let n = openreadout_core::trace::sweep_samples(t, 0);
    let tr = ds.read_trace(t.index, 0, 0, n)?;
    let size = tr
        .channels
        .first()
        .ok_or_else(|| Error::Other("sized trace without channels".into()))?
        .clone();
    let std_ch = t
        .extra
        .get("size_standard_channel")
        .and_then(Value::as_u64)
        .map(|c| c as usize);
    let table = |key: &str| -> Option<TableInfo> {
        let k = t.extra.get(key).and_then(Value::as_u64)?;
        info.tables
            .iter()
            .find(|x| u64::from(x.index) == k)
            .cloned()
    };
    let ladder = match table("size_standard_table") {
        Some(ti) => Some((ds.read_table(ti.index, 0, ti.row_count)?, ti)),
        None => None,
    };
    let peaks = match table("peaks_table") {
        Some(ti) => Some((ds.read_table(ti.index, 0, ti.row_count)?, ti)),
        None => None,
    };
    let ladder_sizes: Vec<f64> = ladder
        .as_ref()
        .and_then(|(t, i)| column(t, i, "size"))
        .unwrap_or_default();
    let (size_lo, size_hi) = (
        size.iter()
            .copied()
            .filter(|v| v.is_finite())
            .fold(f64::INFINITY, f64::min),
        size.iter()
            .copied()
            .filter(|v| v.is_finite())
            .fold(f64::NEG_INFINITY, f64::max),
    );
    if !(size_lo.is_finite() && size_hi > size_lo) {
        return Err(Error::corrupt(
            "preview",
            "the sized trace has no size range",
        ));
    }
    // the window: the ladder's range with a little room, or what was asked
    let (lo, hi) = match req.x_range {
        Some([a, b]) => (a.min(b).max(size_lo), a.max(b).min(size_hi)),
        None => match (ladder_sizes.first(), ladder_sizes.last()) {
            (Some(a), Some(b)) if b > a => {
                let pad = (b - a) * 0.03;
                ((a - pad).max(size_lo), (b + pad).min(size_hi))
            }
            _ => (size_lo, size_hi),
        },
    };
    if hi <= lo {
        return Err(Error::Usage(format!(
            "x_range outside the sized range ({size_lo:.1}–{size_hi:.1} bp)"
        )));
    }
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let small = theme.small_size();
    let w = req.max_size.max(320) as f32;
    let ncols = (w as usize).max(16);
    // the dye channels, sample dyes first, the size standard last
    let mut order: Vec<usize> = (1..t.channels.len())
        .filter(|c| Some(*c) != std_ch)
        .collect();
    if let Some(s) = std_ch.filter(|s| *s < t.channels.len()) {
        order.push(s);
    }
    let cols_of = |v: &[f64]| -> Vec<Option<Col>> {
        let mut cols: Vec<Option<Col>> = vec![None; ncols];
        for (x, y) in size.iter().zip(v) {
            if !(x.is_finite() && y.is_finite()) || *x < lo || *x > hi {
                continue;
            }
            let k = (((x - lo) / (hi - lo)) * ncols as f64) as usize;
            let k = k.min(ncols - 1);
            match &mut cols[k] {
                None => cols[k] = Some(Col::new(*x, *y)),
                Some(c) => c.add(*x, *y),
            }
        }
        cols
    };
    let header = fs * 1.9;
    let title_h = fs * 1.7;
    let gap = fs * 0.5;
    let axis_h = fs * 3.4;
    let np = order.len().max(1);
    let max_h = req.max_size as f32;
    let each = ((w * 0.62) / np as f32).clamp(64.0, 220.0);
    let fit = ((max_h - header - axis_h) / np as f32 - title_h - gap).max(30.0);
    let panel_h = each.min(fit);
    let height = header + np as f32 * (title_h + panel_h + gap) - gap + axis_h;
    let mut fig = Figure::new(w, height.ceil(), Some(theme.paper));
    let mut placer = LabelPlacer::new(fig.rect());
    // header: sample, instrument, standard and who sized it
    let mut parts = header_parts(ds, info);
    if let Some((_, li)) = &ladder
        && let Some(p) = li.extra.get("product").and_then(Value::as_str)
    {
        parts.push(p.to_string());
    }
    parts.push(match t.extra.get("sized_by").and_then(Value::as_str) {
        Some("instrument") => format!(
            "sized by the instrument{}",
            t.extra
                .get("sizing_software")
                .and_then(Value::as_str)
                .map_or(String::new(), |s| format!(" ({s})"))
        ),
        _ => "sized here (Local Southern)".into(),
    });
    fig.text(
        fs * 0.8,
        fs * 1.4,
        &parts.join(" · "),
        &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
    );
    let x_dom = Domain::linear(lo, hi);
    let all_cols: Vec<Vec<Option<Col>>> = order.iter().map(|&c| cols_of(&tr.channels[c])).collect();
    let y_doms: Vec<Domain> = all_cols
        .iter()
        .map(|cs| {
            let top = cs.iter().flatten().map(|c| c.max).fold(1.0f64, f64::max);
            let bot = cs.iter().flatten().map(|c| c.min).fold(0.0f64, f64::min);
            Domain::linear(bot, top * 1.18)
        })
        .collect();
    let left_w = y_doms
        .iter()
        .map(|d| {
            Panel::fit(
                Rect::new(0.0, 0.0, 400.0, panel_h),
                x_dom,
                *d,
                &theme,
                &AxesSpec::default(),
            )
            .rect
            .x
        })
        .fold(0.0f32, f32::max);
    let mut panels = Vec::new();
    let mut report = Vec::new();
    let mut data_marks = Vec::new();
    for (k, &c) in order.iter().enumerate() {
        let ch = &t.channels[c];
        let is_std = Some(c) == std_ch;
        let top = header + k as f32 * (title_h + panel_h + gap) + title_h;
        let last = k + 1 == order.len();
        let rect = Rect::new(left_w + fs * 0.8, top, w - left_w - fs * 2.4, panel_h);
        let panel = Panel::new(rect, x_dom, y_doms[k]);
        let dye = ch
            .extra
            .get("dye")
            .and_then(Value::as_str)
            .unwrap_or(&ch.name)
            .to_string();
        let color = trace_color(&theme, &dye, k);
        let spec = AxesSpec {
            x_label: last.then(|| "size (bp)".to_string()),
            show_x: last,
            grid: GridLines::Y,
            y_ticks: 3,
            ..AxesSpec::default()
        };
        if !last {
            fig.line(
                rect.x,
                rect.bottom(),
                rect.right(),
                rect.bottom(),
                Stroke::new(theme.grid, theme.hairline()),
            );
        }
        panel.axes(&mut fig, &theme, &spec);
        fig.text(
            rect.x,
            top - fs * 0.45,
            &if is_std {
                format!("{dye} · size standard")
            } else {
                dye.clone()
            },
            &TextStyle::new(small, color)
                .face(Face::SansMedium)
                .align(Anchor::Start, Baseline::Alphabetic),
        );
        panel.line_px(
            &mut fig,
            envelope_points(&all_cols[k], &panel),
            Stroke::new(color, theme.line_width * 0.85),
        );
        // labels: the standard's fragments by length, a dye's tallest peaks by size
        let height_at = |x: f64, half: f64| -> f64 {
            size.iter()
                .zip(&tr.channels[c])
                .filter(|(s, _)| (**s - x).abs() <= half)
                .map(|(_, y)| *y)
                .fold(f64::NEG_INFINITY, f64::max)
        };
        let labels: Vec<(f64, f64, String)> = if is_std {
            ladder_sizes
                .iter()
                .filter(|s| **s >= lo && **s <= hi)
                .map(|s| (*s, height_at(*s, 1.5), format!("{s:.0}")))
                .filter(|(_, h, _)| h.is_finite())
                .collect()
        } else {
            // peaks must stand out of the noise: 5 noise deviations for the instrument's
            // peaks, 10 for peaks found here
            let (base, sd) = noise_in(&size, &tr.channels[c], lo, hi);
            let floor = (y_doms[k].hi * 0.06).max(base + 5.0 * sd);
            let found_floor = floor.max(base + 10.0 * sd);
            let from_table = dye_peaks(peaks.as_ref(), &ch.name, lo, hi).map(|v| {
                v.into_iter()
                    .filter(|(_, h)| *h >= floor)
                    .take(MAX_FRAGMENT_LABELS)
                    .map(|(s, _)| (s, height_at(s, 0.6), format!("{s:.1}")))
                    .filter(|(_, h, _)| h.is_finite())
                    .collect::<Vec<_>>()
            });
            from_table.unwrap_or_else(|| {
                let min_gap = (fs * 3.5 / (rect.w / ncols as f32).max(0.01)) as usize;
                marks::prominent(&all_cols[k], MAX_FRAGMENT_LABELS, min_gap.max(1), 0.06)
                    .iter()
                    .filter_map(|&i| all_cols[k][i])
                    .filter(|col| col.max >= found_floor)
                    .map(|col| (col.x_at_max, col.max, format!("{:.1}", col.x_at_max)))
                    .collect()
            })
        };
        data_marks.extend(marks::label_peaks(
            &mut fig,
            &panel,
            &theme,
            &labels,
            Some(c as u32),
            &mut placer,
        ));
        let (ylo, yhi) = panel.y.domain();
        report.push(TracePreviewChannel {
            index: c as u32,
            name: ch.name.clone(),
            unit: ch.unit.clone(),
            color: to_hex(color.to_rgb()),
            min: Some(ylo),
            max: Some(yhi),
        });
        panels.push(FramePanel {
            channel: Some(c as u32),
            ..FramePanel::new(panel)
        });
    }
    out.marks = data_marks;
    out.notes.push(format!(
        "{lo:.0}–{hi:.0} bp of the sized range {size_lo:.0}–{size_hi:.0} bp; x_range [a, b] in bp zooms; openreadout_peaks on trace {} lists the peaks",
        t.index
    ));
    out.trace = Some(TracePreview {
        trace: t.index,
        sweep: 0,
        sample_rate_hz: 0.0,
        sweep_sample_count: n,
        sample_count: n,
        truncated: false,
        x_axis: Some(XAxis {
            quantity: "size".into(),
            unit: "bp".into(),
            first: lo,
            last: hi,
        }),
        channels: report,
    });
    Ok((fig, Frame::plots_labelled(panels, placer.taken())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_axis_maps_scans_to_positions_and_back() {
        let a = BaseAxis::new(vec![10.0, 20.0, 40.0]).unwrap();
        assert_eq!(a.pos(10.0), 1.0);
        assert_eq!(a.pos(30.0), 2.5);
        assert_eq!(a.scan(2.5), 30.0);
        assert_eq!(a.pos(0.0), 1.0 - 10.0 / a.spacing);
        assert!(BaseAxis::new(vec![1.0, 1.0]).is_none());
    }

    #[test]
    fn the_window_starts_where_quality_rises() {
        let mut q = vec![5.0; 30];
        q.extend(vec![40.0; 100]);
        let s = good_start(&q);
        assert!(s > 10 && s <= 30, "{s}");
        assert_eq!(good_start(&[]), 0);
    }
}
