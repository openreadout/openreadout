//! Sequencing previews, drawn from the tables and traces the sequencing readers expose (no
//! dependency on the readers themselves):
//!
//! - **Run quality** of an Illumina run folder: % ≥ Q30 by cycle per lane with the reads
//!   marked, intensity and base composition by cycle, cluster density and pass filter per lane,
//!   and the quality-score distribution: the views a sequencing core checks first.
//! - **Squiggle** of a nanopore read: the whole read's current, a zoom where single samples
//!   show, and the read durations of the file.
//! - **Reads** of a FASTQ or BAM file: the read-length histogram with the N50 marked, the mean
//!   read quality histogram and the mean quality by position.

use std::collections::BTreeMap;

use openreadout_core::model::{FileInfo, TableInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::trace::read_channels;
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Color, Domain, EndLabel, Envelope, Face, Figure, Panel, Rect,
    ScaleKind, Stroke, TextStyle, end_labels, format_value,
};
use serde_json::Value;

use crate::color::to_hex;
use crate::{
    Frame, FramePanel, PreviewOutput, PreviewRequest, TracePreview, TracePreviewChannel, XAxis,
};

/// Most rows read from one table for a preview.
const MAX_ROWS: u64 = 500_000;

/// The table whose `extra.kind` is `kind`.
fn table<'a>(info: &'a FileInfo, kind: &str) -> Option<&'a TableInfo> {
    info.tables
        .iter()
        .find(|t| t.extra.get("kind").and_then(Value::as_str) == Some(kind))
}

/// A table read whole (up to `MAX_ROWS`), as named columns.
struct Cols {
    names: Vec<String>,
    cols: Vec<Vec<f64>>,
}

impl Cols {
    fn read(ds: &mut dyn Dataset, t: &TableInfo) -> Option<Cols> {
        let tab = ds.read_table(t.index, 0, t.row_count.min(MAX_ROWS)).ok()?;
        Some(Cols {
            names: t.columns.iter().map(|c| c.name.clone()).collect(),
            cols: tab.columns,
        })
    }
    fn get(&self, name: &str) -> &[f64] {
        self.names
            .iter()
            .position(|n| n == name)
            .and_then(|i| self.cols.get(i))
            .map_or(&[], Vec::as_slice)
    }
    fn rows(&self) -> usize {
        self.cols.first().map_or(0, Vec::len)
    }
}

/// Draw `title` above `outer` and return the panel inside it, axes drawn.
fn titled_panel(
    fig: &mut Figure,
    theme: &openreadout_figure::Theme,
    outer: Rect,
    title: &str,
    x: Domain,
    y: Domain,
    spec: &AxesSpec,
) -> Panel {
    let fs = theme.font_size;
    fig.text(
        outer.x,
        outer.y + fs,
        title,
        &TextStyle::new(fs, theme.ink)
            .face(Face::SansMedium)
            .align(Anchor::Start, Baseline::Alphabetic),
    );
    let inner = Rect::new(outer.x, outer.y + fs * 1.6, outer.w, outer.h - fs * 1.6);
    let panel = Panel::fit(inner, x, y, theme, spec);
    panel.axes(fig, theme, spec);
    panel
}

fn finite_range(v: impl IntoIterator<Item = f64>) -> Option<(f64, f64)> {
    let (lo, hi) = v
        .into_iter()
        .filter(|x| x.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
            (a.min(x), b.max(x))
        });
    (lo <= hi).then_some((lo, hi))
}

fn caption(fig: &mut Figure, theme: &openreadout_figure::Theme, text: &str, w: f32) {
    let fs = theme.font_size;
    let st = TextStyle::new(fs, theme.muted).align(Anchor::Start, Baseline::Alphabetic);
    let mut t = text.to_string();
    while st.width(&t) > w - fs * 1.6 && t.chars().count() > 8 {
        t = t.chars().take(t.chars().count() - 2).collect::<String>();
        t.push('…');
        t = t.replace("……", "…");
    }
    fig.text(fs * 0.8, fs * 1.4, &t, &st);
}

/// The view a sequencing data set gets by default; `None` when it has nothing to draw (the
/// caller then draws as usual).
pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Option<(Figure, Frame)> {
    if table(info, "cycle_metrics").is_some_and(|t| t.row_count > 0) {
        return run_quality(ds, info, req, out);
    }
    if table(info, "nanopore_reads").is_some() && !info.traces.is_empty() {
        return squiggle(ds, info, req, out);
    }
    if table(info, "length_histogram").is_some_and(|t| t.row_count > 0) {
        return reads(ds, info, req, out);
    }
    None
}

/// Short labels of the reads: `R1`, `I1`, `I2`, `R2`, with their cycle ranges.
fn read_labels(info: &FileInfo) -> Vec<(String, f64, f64)> {
    let Some(t) = info.traces.first() else {
        return vec![];
    };
    let indexed: Vec<bool> = table(info, "run_summary")
        .and_then(|s| s.extra.get("reads"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|r| r.get("indexed").and_then(Value::as_bool).unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    let (mut r, mut i) = (0, 0);
    t.extra
        .get("reads")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .filter_map(|(k, x)| {
                    let a = x.get("first_cycle")?.as_f64()?;
                    let b = x.get("last_cycle")?.as_f64()?;
                    let label = if indexed.get(k).copied().unwrap_or(false) {
                        i += 1;
                        format!("I{i}")
                    } else {
                        r += 1;
                        format!("R{r}")
                    };
                    Some((label, a, b))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn run_quality(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Option<(Figure, Frame)> {
    let cyc = Cols::read(ds, table(info, "cycle_metrics")?)?;
    let summary_t = table(info, "run_summary")?;
    let summary = Cols::read(ds, summary_t)?;
    let hist = table(info, "qscore_histogram").and_then(|t| Cols::read(ds, t));
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let w = req.max_size.max(320) as f32;
    let h = (w * 0.78).round();
    let mut fig = Figure::new(w, h, Some(theme.paper));
    // ---- caption
    let ex = &summary_t.extra;
    let s = |k: &str| ex.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let totals = ex.get("totals").cloned().unwrap_or(Value::Null);
    let num = |k: &str| totals.get(k).and_then(Value::as_f64);
    let mut parts = vec![s("run_id")];
    let model = s("instrument_model");
    if !model.is_empty() {
        parts.push(model);
    }
    if let Some(rs) = totals.get("read_structure").and_then(Value::as_str) {
        parts.push(rs.to_string());
    }
    if let Some(y) = num("yield_gbp") {
        parts.push(format!("{} Gbp", format_value(y)));
    }
    if let Some(q) = num("percent_q30") {
        parts.push(format!("{q:.1}% ≥ Q30"));
    }
    if let Some(p) = num("reads_pf_m") {
        parts.push(format!("{} M reads PF", format_value(p)));
    }
    caption(&mut fig, &theme, &parts.join(" · "), w);
    let reads = read_labels(info);
    let total_cycles = info
        .traces
        .first()
        .map_or(0.0, |t| t.sample_count as f64);
    let x_dom = Domain::linear(0.5, total_cycles.max(1.0) + 0.5);
    let lanes: Vec<u16> = {
        let mut l: Vec<u16> = cyc.get("lane").iter().map(|x| *x as u16).collect();
        l.sort_unstable();
        l.dedup();
        l
    };
    let lane_series = |name: &str, lane: u16| -> Vec<(f64, f64)> {
        let lc = cyc.get("lane");
        let cc = cyc.get("cycle");
        let vc = cyc.get(name);
        (0..cyc.rows())
            .filter(|&i| lc.get(i).is_some_and(|l| *l as u16 == lane))
            .filter_map(|i| Some((*cc.get(i)?, *vc.get(i)?)))
            .collect()
    };
    let mean_series = |name: &str| -> Vec<(f64, f64)> {
        let mut acc: BTreeMap<u64, (f64, u32)> = BTreeMap::new();
        let (cc, vc) = (cyc.get("cycle"), cyc.get(name));
        for i in 0..cyc.rows() {
            if let (Some(c), Some(v)) = (cc.get(i), vc.get(i))
                && v.is_finite()
            {
                let e = acc.entry(*c as u64).or_default();
                e.0 += v;
                e.1 += 1;
            }
        }
        acc.into_iter()
            .map(|(c, (s, n))| (c as f64, s / f64::from(n)))
            .collect()
    };
    let margin = fs * 0.8;
    let top = fs * 2.4;
    let gap = fs * 1.2;
    let full_w = w - 2.0 * margin;
    let row_h = (h - top - margin - 2.0 * gap) / 3.0;
    let left_w = full_w * 0.6;
    let right_x = margin + left_w + gap * 1.5;
    let right_w = full_w - left_w - gap * 1.5;
    let mut panels = Vec::new();
    let read_bands = |fig: &mut Figure, p: &Panel, label: bool| {
        for (k, (name, a, b)) in reads.iter().enumerate() {
            if k % 2 == 1 {
                p.band_x(fig, a - 0.5, b + 0.5, theme.grid.alpha(0.5));
            }
            if label {
                let mid = (a + b) / 2.0;
                if let Some(px) = p.x.px(mid) {
                    fig.text(
                        px,
                        p.rect.y + fs * 1.0,
                        name,
                        &TextStyle::new(theme.small_size(), theme.muted)
                            .align(Anchor::Middle, Baseline::Alphabetic),
                    );
                }
            }
        }
    };
    // ---- row 1: % >= Q30 by cycle, per lane
    let all_q: Vec<f64> = cyc.get("percent_q30").to_vec();
    let (qlo, _) = finite_range(all_q.iter().copied()).unwrap_or((0.0, 100.0));
    let y_q = Domain::linear((qlo - 5.0).clamp(0.0, 90.0).floor(), 100.0).nice(5);
    let spec_q = AxesSpec {
        y_label: Some("% ≥ Q30".into()),
        x_label: Some("cycle".into()),
        right: fs * 4.0,
        ..AxesSpec::default()
    };
    let p1 = titled_panel(
        &mut fig,
        &theme,
        Rect::new(margin, top, full_w, row_h),
        "Quality: % of base calls ≥ Q30, by cycle",
        x_dom,
        y_q,
        &spec_q,
    );
    read_bands(&mut fig, &p1, true);
    let mut labels = Vec::new();
    for (k, lane) in lanes.iter().enumerate() {
        let color = theme.series(k);
        let pts = lane_series("percent_q30", *lane);
        p1.line(&mut fig, pts.iter().copied(), Stroke::new(color, theme.line_width));
        if let Some((x, y)) = pts.iter().rev().find(|p| p.1.is_finite())
            && let Some((px, py)) = p1.pt(*x, *y)
        {
            let _ = px;
            labels.push(EndLabel {
                y: py,
                text: format!("lane {lane}"),
                color,
            });
        }
    }
    end_labels(&mut fig, &theme, p1.rect.right() + fs * 0.3, &labels, p1.rect.y, p1.rect.bottom());
    panels.push(FramePanel::new(p1.clone()));
    // ---- row 2 left: intensity by cycle (mean of lanes), per channel
    let int_names: Vec<String> = cyc
        .names
        .iter()
        .filter(|n| n.starts_with("intensity_"))
        .cloned()
        .collect();
    let y2 = top + row_h + gap;
    if !int_names.is_empty() {
        let series: Vec<(String, Vec<(f64, f64)>)> = int_names
            .iter()
            .map(|n| (n.trim_start_matches("intensity_").to_ascii_uppercase(), mean_series(n)))
            .collect();
        let (lo, hi) = finite_range(series.iter().flat_map(|s| s.1.iter().map(|p| p.1)))
            .unwrap_or((0.0, 1.0));
        let _ = lo;
        let y = Domain::linear(0.0, hi.max(1.0) * 1.08).nice(4);
        let spec = AxesSpec {
            y_label: Some("intensity".into()),
            x_label: Some("cycle".into()),
            right: fs * 3.0,
            ..AxesSpec::default()
        };
        let p = titled_panel(
            &mut fig,
            &theme,
            Rect::new(margin, y2, left_w, row_h),
            "Intensity by cycle (90th percentile, mean of tiles)",
            x_dom,
            y,
            &spec,
        );
        read_bands(&mut fig, &p, false);
        let mut lab = Vec::new();
        for (k, (name, pts)) in series.iter().enumerate() {
            let color = channel_color(&theme, name, k);
            p.line(&mut fig, pts.iter().copied(), Stroke::new(color, theme.line_width * 0.9));
            if let Some((x, yv)) = pts.iter().rev().find(|p| p.1.is_finite())
                && let Some((_, py)) = p.pt(*x, *yv)
            {
                lab.push(EndLabel {
                    y: py,
                    text: name.clone(),
                    color,
                });
            }
        }
        end_labels(&mut fig, &theme, p.rect.right() + fs * 0.3, &lab, p.rect.y, p.rect.bottom());
        panels.push(FramePanel::new(p));
    }
    // ---- row 2 right: cluster density and pass filter per lane (first read's rows)
    {
        let lc = summary.get("lane");
        let dens = summary.get("density");
        let pf = summary.get("percent_pf");
        let readc = summary.get("read");
        let first_read = readc.first().copied().unwrap_or(1.0);
        let rows: Vec<(f64, f64, f64)> = (0..summary.rows())
            .filter(|&i| readc.get(i) == Some(&first_read))
            .filter_map(|i| Some((*lc.get(i)?, *dens.get(i)?, *pf.get(i)?)))
            .collect();
        // tile spread per lane (min and max density over its tiles)
        let tiles = table(info, "tile_metrics").and_then(|t| Cols::read(ds, t));
        let spread = |lane: f64| -> Option<(f64, f64)> {
            let tc = tiles.as_ref()?;
            let (tl, td) = (tc.get("lane"), tc.get("density"));
            finite_range(
                (0..tc.rows())
                    .filter(|&i| tl.get(i).is_some_and(|l| (*l - lane).abs() < 0.5))
                    .filter_map(|i| td.get(i).copied()),
            )
        };
        let hi = rows
            .iter()
            .map(|r| spread(r.0).map_or(r.1, |s| s.1.max(r.1)))
            .fold(0.0f64, f64::max);
        if !rows.is_empty() && hi > 0.0 {
            let x = Domain::linear(0.4, rows.len().max(2) as f64 + 0.6);
            let y = Domain::linear(0.0, hi * 1.2).nice(4);
            let spec = AxesSpec {
                y_label: Some("K/mm² (grey: all, colour: passing filter)".into()),
                x_ticks: 0,
                show_x: false,
                ..AxesSpec::default()
            };
            let p = titled_panel(
                &mut fig,
                &theme,
                Rect::new(right_x, y2, right_w, row_h),
                "Cluster density per lane",
                x,
                y,
                &spec,
            );
            for (k, (lane, d, pfp)) in rows.iter().enumerate() {
                let xi = k as f64 + 1.0;
                p.bars(&mut fig, &[(xi, *d)], 0.62, 0.0, theme.faint);
                if pfp.is_finite() {
                    p.bars(&mut fig, &[(xi, d * pfp / 100.0)], 0.62, 0.0, theme.series(k));
                }
                let mut top_v = *d;
                if let Some((a, b)) = spread(*lane).filter(|(a, b)| b > a) {
                    let st = Stroke::new(theme.ink, theme.hairline());
                    p.line(&mut fig, [(xi, a), (xi, b)], st.clone());
                    p.line(&mut fig, [(xi - 0.08, a), (xi + 0.08, a)], st.clone());
                    p.line(&mut fig, [(xi - 0.08, b), (xi + 0.08, b)], st);
                    top_v = top_v.max(b);
                }
                let st = TextStyle::new(theme.small_size(), theme.ink)
                    .align(Anchor::Middle, Baseline::Alphabetic);
                if let Some((px, py)) = p.pt(xi, top_v) {
                    let label = if pfp.is_finite() {
                        format!("{d:.0} · {pfp:.0}% PF")
                    } else {
                        format!("{d:.0}")
                    };
                    fig.text(px, py - fs * 0.4, &label, &st);
                }
                if let Some(px) = p.x.px(xi) {
                    fig.text(
                        px,
                        p.rect.bottom() + fs * 1.2,
                        &format!("lane {}", *lane as u32),
                        &TextStyle::new(theme.small_size(), theme.muted)
                            .align(Anchor::Middle, Baseline::Alphabetic),
                    );
                }
            }
            panels.push(FramePanel::new(p));
        }
    }
    // ---- row 3 left: base composition by cycle
    let y3 = top + 2.0 * (row_h + gap);
    {
        let bases = ["a", "c", "g", "t", "n"];
        let series: Vec<(String, Vec<(f64, f64)>)> = bases
            .iter()
            .map(|b| (b.to_ascii_uppercase(), mean_series(&format!("percent_{b}"))))
            .filter(|s| s.1.iter().any(|p| p.1.is_finite()))
            .collect();
        if !series.is_empty() {
            let hi = finite_range(series.iter().flat_map(|s| s.1.iter().map(|p| p.1)))
                .map_or(100.0, |r| r.1);
            let y = Domain::linear(0.0, (hi * 1.05).min(100.0).max(30.0)).nice(4);
            let spec = AxesSpec {
                y_label: Some("% of base calls".into()),
                x_label: Some("cycle".into()),
                right: fs * 2.4,
                ..AxesSpec::default()
            };
            let p = titled_panel(
                &mut fig,
                &theme,
                Rect::new(margin, y3, left_w, row_h),
                "Base composition by cycle",
                x_dom,
                y,
                &spec,
            );
            read_bands(&mut fig, &p, false);
            let mut lab = Vec::new();
            for (k, (name, pts)) in series.iter().enumerate() {
                let color = base_color(&theme, name, k);
                p.line(&mut fig, pts.iter().copied(), Stroke::new(color, theme.line_width * 0.9));
                if let Some((x, yv)) = pts.iter().rev().find(|p| p.1.is_finite())
                    && let Some((_, py)) = p.pt(*x, *yv)
                {
                    lab.push(EndLabel {
                        y: py,
                        text: name.clone(),
                        color,
                    });
                }
            }
            end_labels(&mut fig, &theme, p.rect.right() + fs * 0.3, &lab, p.rect.y, p.rect.bottom());
            panels.push(FramePanel::new(p));
        }
    }
    // ---- row 3 right: quality-score distribution (all cycles and lanes)
    if let Some(hc) = &hist {
        let mut by_q: BTreeMap<u32, f64> = BTreeMap::new();
        let (qc, nc) = (hc.get("qscore"), hc.get("base_calls"));
        for i in 0..hc.rows() {
            if let (Some(q), Some(n)) = (qc.get(i), nc.get(i)) {
                *by_q.entry(*q as u32).or_default() += n;
            }
        }
        let total: f64 = by_q.values().sum();
        if total > 0.0 {
            let max_q = by_q.keys().next_back().copied().unwrap_or(40).max(40);
            let hi = by_q.values().fold(0.0f64, |a, b| a.max(*b)) / 1e9;
            let x = Domain::linear(0.0, f64::from(max_q) + 1.0);
            let y = Domain::linear(0.0, hi * 1.15).nice(4);
            let spec = AxesSpec {
                y_label: Some("base calls (G)".into()),
                x_label: Some("Q score".into()),
                ..AxesSpec::default()
            };
            let p = titled_panel(
                &mut fig,
                &theme,
                Rect::new(right_x, y3, right_w, row_h),
                "Q-score distribution (all cycles)",
                x,
                y,
                &spec,
            );
            p.band_x(&mut fig, 30.0, f64::from(max_q) + 1.0, theme.band.alpha(0.35));
            let width = if by_q.len() <= 8 { 2.4 } else { 0.85 };
            for (q, n) in &by_q {
                let color = if *q >= 30 { theme.accent } else { theme.ink };
                p.bars(&mut fig, &[(f64::from(*q), n / 1e9)], width, 0.0, color);
            }
            let q30: f64 = by_q.range(30..).map(|(_, n)| n).sum();
            if let Some((px, py)) = p.pt(30.0, hi * 1.08) {
                fig.text(
                    px + fs * 0.3,
                    py,
                    &format!("≥ Q30: {:.1}%", q30 / total * 100.0),
                    &TextStyle::new(theme.small_size(), theme.accent)
                        .align(Anchor::Start, Baseline::Middle),
                );
            }
            panels.push(FramePanel::new(p));
        }
    }
    out.kind = "run-quality".into();
    out.notes.push(
        "Illumina run quality from the InterOp metrics (no base calls read); the numbers are in info → tables (run summary, cycles, tiles)".into(),
    );
    Some((fig, Frame::plots(panels)))
}

/// Conventional colours for bases (A green, C blue, G black/ink, T red, N grey).
fn base_color(theme: &openreadout_figure::Theme, b: &str, k: usize) -> Color {
    match b {
        "A" => Color::hex(0x2E8B57),
        "C" => Color::hex(0x2F6DB5),
        "G" => theme.ink,
        "T" => Color::hex(0xC8452D),
        "N" => theme.faint,
        _ => theme.series(k),
    }
}

/// Channel colours: bases as above, `RED`/`GREEN`/`BLUE` image channels as their colour.
fn channel_color(theme: &openreadout_figure::Theme, c: &str, k: usize) -> Color {
    match c {
        "RED" => Color::hex(0xC8452D),
        "GREEN" => Color::hex(0x2E8B57),
        "BLUE" => Color::hex(0x2F6DB5),
        other => base_color(theme, other, k),
    }
}

fn squiggle(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Option<(Figure, Frame)> {
    let t = info.traces.first()?;
    let rt = table(info, "nanopore_reads")?;
    let reads = Cols::read(ds, rt)?;
    let n_reads = t.sweep_count;
    if n_reads == 0 {
        return None;
    }
    let read = req.sweep.unwrap_or(0).min(n_reads - 1);
    let rate = if t.sample_rate_hz > 0.0 { t.sample_rate_hz } else { 1.0 };
    let n = openreadout_core::trace::sweep_samples(t, read);
    let samples = read_channels(ds, t.index, read, 0, n, &[0]).ok()?;
    let sig = samples.into_iter().next()?;
    if sig.is_empty() {
        return None;
    }
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let w = req.max_size.max(320) as f32;
    let h = (w * 0.66).round();
    let mut fig = Figure::new(w, h, Some(theme.paper));
    let ids = rt
        .columns
        .iter()
        .find(|c| c.name == "read_id")
        .and_then(|c| c.extra.get("categories"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let r = read as usize;
    let id = reads
        .get("read_id")
        .get(r)
        .and_then(|k| ids.get(*k as usize))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let val = |name: &str| reads.get(name).get(r).copied().filter(|x| x.is_finite());
    let mut parts = vec![format!("read {read} of {n_reads}")];
    if !id.is_empty() {
        parts.push(id.clone());
    }
    if let Some(c) = val("channel") {
        parts.push(format!("channel {c:.0}"));
    }
    if let Some(wl) = val("well") {
        parts.push(format!("well {wl:.0}"));
    }
    parts.push(format!("{} s at {} kHz", format_value(sig.len() as f64 / rate), format_value(rate / 1000.0)));
    caption(&mut fig, &theme, &parts.join(" · "), w);
    let margin = fs * 0.8;
    let top = fs * 2.4;
    let gap = fs * 1.2;
    let full_w = w - 2.0 * margin;
    let h1 = (h - top - margin - 2.0 * gap) * 0.42;
    let h2 = (h - top - margin - 2.0 * gap) * 0.33;
    let h3 = h - top - margin - 2.0 * gap - h1 - h2;
    // robust y range: 0.1–99.9 percentiles, so a spike does not flatten the read
    let mut sorted: Vec<f64> = sig.iter().copied().filter(|x| x.is_finite()).collect();
    sorted.sort_by(f64::total_cmp);
    let q = |f: f64| sorted[((sorted.len() - 1) as f64 * f) as usize];
    let (lo, hi) = (q(0.001), q(0.999));
    let y = Domain::of([lo, hi], 0.06, ScaleKind::Linear).nice(4);
    let dur = sig.len() as f64 / rate;
    let x = Domain::linear(0.0, dur);
    let spec = AxesSpec {
        y_label: Some("current (pA)".into()),
        x_label: Some("time (s)".into()),
        ..AxesSpec::default()
    };
    let p1 = titled_panel(
        &mut fig,
        &theme,
        Rect::new(margin, top, full_w, h1),
        "Whole read",
        x,
        y,
        &spec,
    );
    let mut env = Envelope::new(p1.rect.w.max(1.0) as usize, 0.0, dur);
    for (i, v) in sig.iter().enumerate() {
        if v.is_finite() {
            env.push(i as f64 / rate, *v);
        }
    }
    // the zoom: 0.4 s (or 2000 samples) starting a fifth of the way into the read
    let zn = ((0.4 * rate) as usize).clamp(200, 2000).min(sig.len());
    let z0 = (sig.len() / 5).min(sig.len() - zn);
    let (zx0, zx1) = (z0 as f64 / rate, (z0 + zn) as f64 / rate);
    p1.line_px(&mut fig, env.points(&p1), Stroke::new(theme.ink, theme.line_width * 0.6));
    p1.band_x(&mut fig, zx0, zx1, theme.accent.alpha(0.28));
    p1.rule_x(&mut fig, zx0, Stroke::new(theme.accent, theme.hairline()));
    p1.rule_x(&mut fig, zx1, Stroke::new(theme.accent, theme.hairline()));
    let zy = finite_range(sig[z0..z0 + zn].iter().copied()).unwrap_or((lo, hi));
    let y2d = Domain::of([zy.0, zy.1], 0.08, ScaleKind::Linear).nice(4);
    let p2 = titled_panel(
        &mut fig,
        &theme,
        Rect::new(margin, top + h1 + gap, full_w, h2),
        &format!("Zoom: samples {z0}–{} (the shaded part above)", z0 + zn),
        Domain::linear(zx0, zx1),
        y2d,
        &spec,
    );
    p2.line(
        &mut fig,
        sig[z0..z0 + zn]
            .iter()
            .enumerate()
            .map(|(i, v)| ((z0 + i) as f64 / rate, *v)),
        Stroke::new(theme.accent, theme.line_width * 0.8),
    );
    // read durations of the file
    let mut panels = vec![FramePanel::new(p1), FramePanel::new(p2)];
    let durs: Vec<f64> = reads
        .get("duration")
        .iter()
        .copied()
        .filter(|d| d.is_finite() && *d > 0.0)
        .collect();
    if (2..50).contains(&durs.len()) {
        // few reads: one tick per read, this read in the accent
        let (dlo, dhi) = finite_range(durs.iter().copied())?;
        let _ = dlo;
        let xd = Domain::linear(0.0, dhi * 1.04).nice(5);
        let spec3 = AxesSpec {
            x_label: Some("read duration (s)".into()),
            show_y: false,
            grid: openreadout_figure::GridLines::None,
            ..AxesSpec::default()
        };
        let p3 = titled_panel(
            &mut fig,
            &theme,
            Rect::new(margin, top + h1 + h2 + 2.0 * gap, full_w, h3),
            &format!("Read durations of the file ({} reads; this one in red)", durs.len()),
            xd,
            Domain::linear(0.0, 1.0),
            &spec3,
        );
        let this = val("duration");
        for d in &durs {
            let is_this = this.is_some_and(|t| (t - d).abs() < 1e-12);
            let (c, wdt) = if is_this {
                (theme.accent, theme.line_width * 2.0)
            } else {
                (theme.ink.alpha(0.7), theme.line_width * 1.2)
            };
            p3.line(&mut fig, [(*d, 0.15), (*d, 0.85)], Stroke::new(c, wdt));
        }
        panels.push(FramePanel::new(p3));
    } else if durs.len() >= 50 {
        let (dlo, dhi) = finite_range(durs.iter().copied())?;
        let log = dhi / dlo.max(1e-6) > 50.0;
        let bins = 40usize;
        let edges: Vec<f64> = (0..=bins)
            .map(|i| {
                let f = i as f64 / bins as f64;
                if log {
                    (dlo.ln() + (dhi.ln() - dlo.ln()) * f).exp()
                } else {
                    dlo + (dhi - dlo) * f
                }
            })
            .collect();
        let mut counts = vec![0f64; bins];
        for d in &durs {
            let k = edges.partition_point(|e| e <= d).saturating_sub(1).min(bins - 1);
            counts[k] += 1.0;
        }
        let cmax = counts.iter().copied().fold(0.0, f64::max);
        let xd = if log {
            Domain::log(edges[0], edges[bins])
        } else {
            Domain::linear(edges[0], edges[bins])
        };
        let spec3 = AxesSpec {
            y_label: Some("reads".into()),
            x_label: Some("read duration (s)".into()),
            ..AxesSpec::default()
        };
        let p3 = titled_panel(
            &mut fig,
            &theme,
            Rect::new(margin, top + h1 + h2 + 2.0 * gap, full_w, h3),
            &format!("Read durations of the file ({} reads)", durs.len()),
            xd,
            Domain::linear(0.0, cmax * 1.1).nice(3),
            &spec3,
        );
        for (k, c) in counts.iter().enumerate() {
            if *c > 0.0 {
                p3.rect_data(&mut fig, edges[k], 0.0, edges[k + 1], *c, Some(theme.ink.alpha(0.75)), None);
            }
        }
        if let Some(d) = val("duration") {
            p3.rule_x(&mut fig, d, Stroke::new(theme.accent, theme.line_width));
        }
        panels.push(FramePanel::new(p3));
    }
    out.kind = "squiggle".into();
    out.trace = Some(TracePreview {
        trace: t.index,
        sweep: read,
        sample_rate_hz: rate,
        sweep_sample_count: n,
        sample_count: sig.len() as u64,
        truncated: false,
        x_axis: Some(XAxis {
            quantity: "time".into(),
            unit: "s".into(),
            first: 0.0,
            last: dur,
        }),
        channels: vec![TracePreviewChannel {
            index: 0,
            name: "current".into(),
            unit: Some("pA".into()),
            color: to_hex(theme.ink.to_rgb()),
            min: Some(lo),
            max: Some(hi),
        }],
    });
    if req.sweep.is_none() && n_reads > 1 {
        out.notes.push(format!(
            "showing read 0 of {n_reads}; pass sweep N for another read (table 0 lists them)"
        ));
    }
    Some((fig, Frame::plots(panels)))
}

fn reads(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Option<(Figure, Frame)> {
    let lt = table(info, "length_histogram")?;
    let lens = Cols::read(ds, lt)?;
    let qual = table(info, "read_quality_histogram").and_then(|t| Cols::read(ds, t));
    let pos = table(info, "quality_by_position").and_then(|t| Cols::read(ds, t));
    let summary = info
        .tables
        .first()
        .and_then(|t| t.extra.get("summary"))
        .cloned()
        .unwrap_or(Value::Null);
    let num = |k: &str| summary.get(k).and_then(Value::as_f64);
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let w = req.max_size.max(320) as f32;
    let h = (w * 0.62).round();
    let mut fig = Figure::new(w, h, Some(theme.paper));
    let mut parts = Vec::new();
    let exact = info
        .tables
        .first()
        .and_then(|t| t.extra.get("read_count_exact"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if let Some(r) = num("reads") {
        let est = num("reads_estimated").filter(|_| !exact);
        parts.push(match est {
            Some(e) => format!("≈ {} reads (first {} read)", format_value(e), format_value(r)),
            None => format!("{} reads", format_value(r)),
        });
    }
    if let Some(b) = num("bases") {
        parts.push(format!("{} bases", si(b)));
    }
    if let Some(n50) = num("length_n50") {
        parts.push(format!("N50 {}", si(n50)));
    }
    if let Some(q) = num("percent_q30") {
        parts.push(format!("{q:.1}% bases ≥ Q30"));
    }
    if let Some(p) = info
        .tables
        .first()
        .and_then(|t| t.extra.get("platform"))
        .and_then(Value::as_str)
    {
        parts.push(p.to_string());
    }
    caption(&mut fig, &theme, &parts.join(" · "), w);
    let margin = fs * 0.8;
    let top = fs * 2.4;
    let gap = fs * 1.2;
    let full_w = w - 2.0 * margin;
    let h1 = (h - top - margin - gap) * 0.55;
    let h2 = h - top - margin - gap - h1;
    // ---- length histogram
    let (from, to, cnt) = (lens.get("length_from"), lens.get("length_to"), lens.get("reads"));
    let lo = from.first().copied().unwrap_or(0.0);
    let hi = to.last().copied().unwrap_or(1.0) + 1.0;
    let log = lt.extra.get("bins").and_then(Value::as_str) == Some("logarithmic");
    let xd = if log {
        Domain::log(lo.max(1.0), hi)
    } else {
        Domain::linear(lo, hi.max(lo + 1.0))
    };
    let cmax = cnt.iter().copied().fold(0.0, f64::max);
    let spec = AxesSpec {
        y_label: Some("reads".into()),
        x_label: Some("read length (bases)".into()),
        ..AxesSpec::default()
    };
    let p1 = titled_panel(
        &mut fig,
        &theme,
        Rect::new(margin, top, full_w, h1),
        "Read lengths",
        xd,
        Domain::linear(0.0, cmax.max(1.0) * 1.12).nice(4),
        &spec,
    );
    for i in 0..cnt.len().min(from.len()).min(to.len()) {
        if cnt[i] > 0.0 {
            p1.rect_data(&mut fig, from[i], 0.0, to[i] + 1.0, cnt[i], Some(theme.ink.alpha(0.8)), None);
        }
    }
    if let Some(n50) = num("length_n50") {
        p1.rule_x(&mut fig, n50, Stroke::new(theme.accent, theme.line_width).dashed(4.0, 3.0));
        if let Some((px, py)) = p1.pt(n50, cmax * 1.05) {
            fig.text(
                px + fs * 0.3,
                py,
                &format!("N50 {}", si(n50)),
                &TextStyle::new(theme.small_size(), theme.accent).align(Anchor::Start, Baseline::Middle),
            );
        }
    }
    let mut panels = vec![FramePanel::new(p1)];
    let half = (full_w - gap * 1.5) / 2.0;
    // ---- read quality histogram
    if let Some(qc) = &qual
        && qc.rows() > 0
    {
        let (q, n) = (qc.get("mean_quality"), qc.get("reads"));
        let qmax = q.iter().copied().fold(0.0, f64::max).max(40.0);
        let qmin = q.iter().copied().fold(f64::INFINITY, f64::min);
        let qlo = if qmin.is_finite() { ((qmin - 2.0) / 5.0).floor().max(0.0) * 5.0 } else { 0.0 };
        let nmax = n.iter().copied().fold(0.0, f64::max);
        let spec = AxesSpec {
            y_label: Some("reads".into()),
            x_label: Some("mean read quality (Phred)".into()),
            ..AxesSpec::default()
        };
        let p = titled_panel(
            &mut fig,
            &theme,
            Rect::new(margin, top + h1 + gap, half, h2),
            "Read quality",
            Domain::linear(qlo, qmax + 1.0),
            Domain::linear(0.0, nmax.max(1.0) * 1.12).nice(3),
            &spec,
        );
        for (qq, nn) in q.iter().zip(n) {
            let c = if *qq >= 30.0 { theme.accent } else { theme.ink.alpha(0.8) };
            p.rect_data(&mut fig, *qq, 0.0, qq + 0.9, *nn, Some(c), None);
        }
        panels.push(FramePanel::new(p));
    }
    // ---- quality by position
    if let Some(pc) = &pos
        && pc.rows() > 0
    {
        let (p0, mq) = (pc.get("position"), pc.get("mean_quality"));
        let pts: Vec<(f64, f64)> = p0.iter().copied().zip(mq.iter().copied()).collect();
        let xmax = pc.get("position_end").last().copied().unwrap_or(1.0);
        let (_, qhi) = finite_range(mq.iter().copied()).unwrap_or((0.0, 40.0));
        let spec = AxesSpec {
            y_label: Some("mean quality".into()),
            x_label: Some("position in read".into()),
            ..AxesSpec::default()
        };
        let p = titled_panel(
            &mut fig,
            &theme,
            Rect::new(margin + half + gap * 1.5, top + h1 + gap, half, h2),
            "Quality by position",
            Domain::linear(1.0, xmax.max(2.0)),
            Domain::linear(0.0, (qhi * 1.1).max(40.0)).nice(4),
            &spec,
        );
        p.rule_y(&mut fig, 30.0, Stroke::new(theme.accent, theme.hairline()).dashed(4.0, 3.0));
        if let Some((px, py)) = p.pt(xmax.max(2.0), 30.0) {
            fig.text(
                px,
                py - fs * 0.3,
                "Q30",
                &TextStyle::new(theme.small_size(), theme.accent)
                    .align(Anchor::End, Baseline::Alphabetic),
            );
        }
        p.line(&mut fig, pts, Stroke::new(theme.ink, theme.line_width));
        panels.push(FramePanel::new(p));
    }
    out.kind = "reads".into();
    if !exact {
        out.notes.push(
            "from the reads info scanned at the start of the file; `check` reads every record".into(),
        );
    }
    Some((fig, Frame::plots(panels)))
}

/// A count with a k/M/G suffix.
fn si(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{}G", format_value(v / 1e9))
    } else if a >= 1e6 {
        format!("{}M", format_value(v / 1e6))
    } else if a >= 1e4 {
        format!("{}k", format_value(v / 1e3))
    } else {
        format_value(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixes() {
        assert_eq!(si(2_967.0), "2967");
        assert_eq!(si(9_474_839.0), "9.475M");
    }
}
