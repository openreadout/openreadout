//! Overlay previews: every channel of a trace drawn in one panel with a shared y axis — the
//! amplification and melt plots of qPCR files, whose traces say `extra.plot = "overlay"` (one
//! channel per well). Curves are coloured by target when the wells name a few targets (with a
//! direct-label key), no-template controls drawn faint.

use std::collections::BTreeMap;

use openreadout_core::model::TraceInfo;
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Color, Domain, Figure, Panel, ScaleKind, Stroke, TextStyle, key_row,
};

use crate::color::to_hex;
use crate::trace::{axis_title, x_axis};
use crate::{Frame, FramePanel, PreviewOutput, PreviewRequest, TracePreview, TracePreviewChannel};

/// Most samples per channel drawn (overlay traces are short: cycles or melt reads).
const MAX_SAMPLES: u64 = 100_000;
/// Most channels drawn in one overlay.
const MAX_CHANNELS: usize = 1536;
/// Most groups coloured apart; more are drawn in one colour.
const MAX_GROUPS: usize = 7;

fn extra_str<'a>(ch: &'a openreadout_core::model::SignalChannelInfo, k: &str) -> Option<&'a str> {
    ch.extra
        .get(k)
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn is_control(sample: Option<&str>) -> bool {
    sample.is_some_and(|s| {
        let s = s.trim().to_ascii_lowercase();
        s == "ntc" || s.starts_with("no template") || s == "blank" || s == "nac"
    })
}

pub(crate) fn render(
    ds: &mut dyn Dataset,
    t: &TraceInfo,
    sweep: u32,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    let nch = t.channels.len();
    let channels: Vec<u32> = if req.channels.is_empty() {
        (0..nch.min(MAX_CHANNELS) as u32).collect()
    } else {
        for &c in &req.channels {
            if c as usize >= nch {
                return Err(Error::Usage(format!(
                    "channel {c} out of range (trace {} has {nch} channels)",
                    t.index
                )));
            }
        }
        req.channels.clone()
    };
    if nch > MAX_CHANNELS && req.channels.is_empty() {
        out.notes.push(format!(
            "drew the first {MAX_CHANNELS} of {nch} channels; pass channels to choose"
        ));
    }
    let len = openreadout_core::trace::sweep_samples(t, sweep);
    let n = len.min(MAX_SAMPLES);
    let data = openreadout_core::trace::read_channels(ds, t.index, sweep, 0, n, &channels)?;
    let ax = x_axis(t, len, n);
    let samples = data.iter().map(Vec::len).max().unwrap_or(0);
    let step = if samples > 1 {
        (ax.last - ax.first) / (samples - 1) as f64
    } else {
        0.0
    };
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let w = req.max_size.max(64) as f32;
    let h = (w * 0.62).round().min(req.max_size as f32);
    let mut fig = Figure::new(w, h, Some(theme.paper));

    // groups: the wells' targets (or samples) when there are a few
    let group_of = |c: u32| -> Option<String> {
        let ch = &t.channels[c as usize];
        extra_str(ch, "target")
            .or_else(|| extra_str(ch, "sample"))
            .map(str::to_string)
    };
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for &c in &channels {
        if let Some(g) = group_of(c)
            && !is_control(extra_str(&t.channels[c as usize], "sample"))
        {
            *groups.entry(g).or_default() += 1;
        }
    }
    let colored = (2..=MAX_GROUPS).contains(&groups.len());
    let group_color: BTreeMap<String, Color> = groups
        .keys()
        .enumerate()
        .map(|(i, g)| (g.clone(), theme.series(i)))
        .collect();

    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in &data {
        for &x in v {
            if x.is_finite() {
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    }
    let key_h = if colored { fs * 2.2 } else { fs * 0.6 };
    let name = t
        .name
        .clone()
        .unwrap_or_else(|| format!("trace {}", t.index));
    let title = match t.extra.get("dye").and_then(serde_json::Value::as_str) {
        Some(d) if !name.contains(d) => format!("{name} · {d}"),
        _ => name,
    };
    let spec = AxesSpec {
        x_label: Some(axis_title(&ax)),
        y_label: Some(
            t.channels
                .first()
                .and_then(|c| c.unit.clone())
                .map_or_else(|| "fluorescence".to_string(), |u| format!("signal ({u})")),
        ),
        top: key_h,
        ..AxesSpec::default()
    };
    let x_dom = Domain::linear(ax.first.min(ax.last), ax.first.max(ax.last));
    let y_dom = Domain::of([lo, hi], 0.05, ScaleKind::Linear);
    let panel = Panel::fit(fig.rect().inset(fs * 0.8), x_dom, y_dom, &theme, &spec);
    fig.text(
        fs * 0.8,
        fs * 1.4,
        &title,
        &TextStyle::new(fs, theme.muted).align(Anchor::Start, Baseline::Alphabetic),
    );
    panel.axes(&mut fig, &theme, &spec);
    let thin = (theme.line_width * if channels.len() > 24 { 0.6 } else { 0.9 }).max(0.8);
    let mut report = Vec::with_capacity(channels.len());
    // controls first (underneath), then the rest
    let mut order: Vec<usize> = (0..channels.len()).collect();
    order.sort_by_key(|&k| !is_control(extra_str(&t.channels[channels[k] as usize], "sample")));
    for k in order {
        let c = channels[k];
        let ch = &t.channels[c as usize];
        let control = is_control(extra_str(ch, "sample"));
        let col = if control {
            theme.faint
        } else if colored {
            group_of(c)
                .and_then(|g| group_color.get(&g).copied())
                .unwrap_or(theme.ink)
        } else if channels.len() <= 8 {
            theme.series(k)
        } else {
            theme.ink.alpha(0.55)
        };
        let pts = data[k]
            .iter()
            .enumerate()
            .map(|(i, &v)| (ax.first + step * i as f64, v));
        panel.line(&mut fig, pts, Stroke::new(col, thin).round());
        report.push((
            k,
            TracePreviewChannel {
                index: c,
                name: ch.name.clone(),
                unit: ch.unit.clone(),
                color: to_hex(col.to_rgb()),
                min: Some(panel.y.domain().0),
                max: Some(panel.y.domain().1),
            },
        ));
    }
    report.sort_by_key(|(k, _)| *k);
    if colored {
        let mut items: Vec<(Color, String)> =
            group_color.iter().map(|(g, c)| (*c, g.clone())).collect();
        if channels
            .iter()
            .any(|&c| is_control(extra_str(&t.channels[c as usize], "sample")))
        {
            items.push((theme.faint, "no-template controls".into()));
        }
        key_row(
            &mut fig,
            &theme,
            panel.rect.x,
            fs * 2.9,
            panel.rect.w,
            &items,
            false,
        );
    }
    out.notes.push(format!(
        "{} channel(s) overlaid in one panel with a shared y axis{}",
        channels.len(),
        if colored { ", coloured by target" } else { "" }
    ));
    out.trace = Some(TracePreview {
        trace: t.index,
        sweep,
        sample_rate_hz: t.sample_rate_hz,
        sweep_sample_count: len,
        sample_count: n,
        truncated: n < len,
        x_axis: Some(ax),
        channels: report.into_iter().map(|(_, r)| r).collect(),
    });
    Ok((fig, Frame::plots(vec![FramePanel::new(panel)])))
}
