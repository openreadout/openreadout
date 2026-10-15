//! Trace previews: one sweep, one panel per channel, sharing the x axis at the bottom. Each
//! pixel column shows the min/max envelope of the samples that fall into it, joined to its
//! neighbours, so spikes survive any amount of downsampling. Spectra and chromatograms get
//! their largest peaks labelled; current-clamp sweeps can get their action potentials marked.

use openreadout_core::model::{FileInfo, TraceInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Domain, Face, Figure, GridLines, LabelPlacer, Panel, Path, Rect,
    Stroke, TextStyle, Theme, format_value,
};

use crate::color::to_hex;
use crate::marks::{self, Col};
use crate::{
    DataMark, Frame, FramePanel, Marks, PreviewOutput, PreviewRequest, TracePreview,
    TracePreviewChannel, XAxis,
};

/// Channels drawn when none are requested.
const DEFAULT_CHANNELS: usize = 8;
/// Most samples per channel read for one preview (the rest of a longer sweep is not drawn).
pub const MAX_TRACE_PREVIEW_SAMPLES: u64 = 20_000_000;
/// Values requested from the reader per batch, across all of the trace's channels.
const BATCH_VALUES: u64 = 1 << 23;
/// Most samples of a voltage channel kept for spike detection.
const MAX_SPIKE_SAMPLES: u64 = 10_000_000;

/// Families whose traces are spectra or chromatograms: their peaks are labelled by default.
const PEAK_FAMILIES: [&str; 6] = [
    "spectroscopy",
    "chromatography",
    "mass-spectrometry",
    "nmr",
    "diffraction",
    "epr",
];

fn find_trace(info: &FileInfo, ti: u32) -> Result<TraceInfo> {
    info.traces
        .iter()
        .find(|t| t.index == ti)
        .cloned()
        .ok_or_else(|| {
            if info.traces.is_empty() && !info.images.is_empty() {
                Error::unsupported(
                    "preview",
                    format!("trace preview of a {} file", info.format.name),
                    "This file holds images, not traces. Pick image channels with a selection such as `c=0-2` (`select` in MCP, `--select` on the command line) instead of `channels` or `--channel`, which pick trace channels.",
                )
            } else if info.traces.is_empty() {
                Error::unsupported(
                    "preview",
                    format!("trace preview of a {} file", info.format.name),
                    "This file holds no traces; see `openreadout info` for what it contains.",
                )
            } else {
                Error::Usage(format!(
                    "trace {ti} out of range (file has {} traces)",
                    info.traces.len()
                ))
            }
        })
}

/// The channel holding an irregular x axis (`extra.axis.irregular`, `channel`).
fn x_channel(t: &TraceInfo) -> Option<u32> {
    t.extra
        .get("axis")
        .filter(|a| a.get("irregular").and_then(serde_json::Value::as_bool) == Some(true))
        .and_then(|a| a.get("channel"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|c| u32::try_from(c).ok())
}

/// Should peaks be labelled on this trace?
fn wants_peaks(info: &FileInfo, ax: &XAxis, marks: Marks) -> bool {
    match marks {
        Marks::Peaks => true,
        Marks::Auto => {
            let family = info.format.family.as_str();
            match ax.quantity.as_str() {
                // a time axis has peaks worth naming only on a chromatogram
                "time" => family == "chromatography" || family == "mass-spectrometry",
                "sample" | "cycle" | "temperature" => false,
                _ => PEAK_FAMILIES.contains(&family),
            }
        }
        _ => false,
    }
}

/// `time (s)`, `chemical shift (ppm)`, …
pub(crate) fn axis_title(ax: &XAxis) -> String {
    let q = ax.quantity.replace('_', " ");
    if ax.unit.is_empty() {
        q
    } else if q == "mz" {
        "m/z".into()
    } else {
        format!("{q} ({})", ax.unit)
    }
}

/// A channel title: `Vm (mV)`.
fn channel_title(name: &str, unit: Option<&str>) -> String {
    let name: String = name.chars().take(48).collect();
    match unit.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => format!("{name} ({u})"),
        None => name,
    }
}

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    let ti = req.trace.unwrap_or(0);
    let sweep = req.sweep.unwrap_or(0);
    let t = find_trace(info, ti)?;
    if sweep >= t.sweep_count {
        return Err(Error::Usage(format!(
            "sweep {sweep} out of range (trace {ti} has {} sweeps, 0..{})",
            t.sweep_count, t.sweep_count
        )));
    }
    // Readers mark traces whose channels belong in one panel (qPCR amplification/melt plots).
    if t.extra.get("plot").and_then(serde_json::Value::as_str) == Some("overlay") {
        return crate::overlay::render(ds, &t, sweep, req, out);
    }
    let nch = t.channels.len();
    if nch == 0 {
        return Err(Error::Usage(format!("trace {ti} has no channels")));
    }
    // An irregular abscissa stored as a channel (a grating spectrometer's Raman-shift list, a
    // peak table's x) is the x axis, not a signal to draw.
    let xch = x_channel(&t).filter(|c| (*c as usize) < nch && nch > 1);
    let channels: Vec<u32> = if req.channels.is_empty() {
        let signal: Vec<u32> = (0..nch as u32).filter(|c| xch != Some(*c)).collect();
        if signal.len() > DEFAULT_CHANNELS {
            out.notes.push(format!(
                "showing the first {DEFAULT_CHANNELS} of {nch} channels; pass channels to choose"
            ));
        }
        signal.into_iter().take(DEFAULT_CHANNELS).collect()
    } else {
        for &c in &req.channels {
            if c as usize >= nch {
                return Err(Error::Usage(format!(
                    "channel {c} out of range (trace {ti} has {nch} channels)"
                )));
            }
        }
        req.channels.clone()
    };
    let sweep_len = openreadout_core::trace::sweep_samples(&t, sweep);
    let n = sweep_len.min(MAX_TRACE_PREVIEW_SAMPLES);
    let ax = x_axis(&t, sweep_len, n);
    let theme = crate::theme(req);
    let width = req.max_size.max(64);
    let ncols = width as usize;
    // x of sample i (uniform axes), or the x channel's value
    let step = if n > 1 {
        (ax.last - ax.first) / (n - 1) as f64
    } else {
        0.0
    };
    let mut cols: Vec<Vec<Option<Col>>> = vec![vec![None; ncols]; channels.len()];
    // Spikes: the voltage channel's samples, in mV.
    let spike_ch: Option<(usize, f64)> = (req.marks == Marks::Spikes)
        .then(|| {
            channels.iter().enumerate().find_map(|(k, &c)| {
                openreadout_signal::ephys::to_mv(t.channels[c as usize].unit.as_deref())
                    .map(|f| (k, f))
            })
        })
        .flatten();
    if req.marks == Marks::Spikes && spike_ch.is_none() {
        out.notes.push(
            "marks=spikes needs a voltage channel (mV or V); none is drawn, so no spikes were marked".into(),
        );
    }
    let keep_spike = spike_ch.filter(|_| n <= MAX_SPIKE_SAMPLES);
    if spike_ch.is_some() && keep_spike.is_none() {
        out.notes.push(format!(
            "spikes are marked on sweeps of at most {MAX_SPIKE_SAMPLES} samples; use openreadout_ephys_features for longer ones"
        ));
    }
    let mut volts: Vec<f64> = Vec::new();
    // irregular x values decide the columns; read the whole x channel first
    let mut xs: Option<Vec<f64>> = None;
    if let Some(xc) = xch {
        let tr = openreadout_core::trace::read_channels(ds, ti, sweep, 0, n, &[xc])?;
        xs = tr.into_iter().next();
    }
    let (x_lo, x_hi) = match &xs {
        Some(v) => v
            .iter()
            .copied()
            .filter(|x| x.is_finite())
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
                (a.min(x), b.max(x))
            }),
        None => (ax.first.min(ax.last), ax.first.max(ax.last)),
    };
    let col_of = |x: f64| -> usize {
        let span = x_hi - x_lo;
        let t = if span > 0.0 { (x - x_lo) / span } else { 0.5 };
        ((t * ncols as f64) as usize).min(ncols - 1)
    };
    let batch = (BATCH_VALUES / nch as u64).clamp(4096, 1 << 20);
    let mut pos = 0u64;
    while pos < n {
        let want = batch.min(n - pos);
        let tr = ds.read_trace(ti, sweep, pos, want)?;
        let got = tr.channels.first().map_or(0, Vec::len) as u64;
        for (k, &c) in channels.iter().enumerate() {
            let data = tr
                .channels
                .get(c as usize)
                .ok_or_else(|| Error::Other(format!("reader returned no channel {c}")))?;
            for (i, &v) in data.iter().enumerate() {
                let si = pos + i as u64;
                if keep_spike.is_some_and(|(sk, _)| sk == k) {
                    volts.push(v);
                }
                if !v.is_finite() {
                    continue;
                }
                let x = match &xs {
                    Some(xv) => match xv.get(si as usize) {
                        Some(&x) if x.is_finite() => x,
                        _ => continue,
                    },
                    None => ax.first + step * si as f64,
                };
                let slot = &mut cols[k][col_of(x)];
                match slot {
                    None => *slot = Some(Col::new(x, v)),
                    Some(c) => c.add(x, v),
                }
            }
        }
        if got == 0 {
            break;
        }
        pos += got;
    }
    let drawn = pos.min(n);
    if drawn < sweep_len {
        out.notes.push(format!(
            "drew the first {drawn} of {sweep_len} samples of the sweep"
        ));
    }
    let peaks_on = wants_peaks(info, &ax, req.marks);

    // Layout: panels stacked, the x axis under the last one.
    let nc = channels.len();
    let fs = theme.font_size;
    // a caption when the trace name says more than the channel titles, or there are sweeps
    let name = t.name.clone().unwrap_or_else(|| format!("trace {ti}"));
    let one_title = nc == 1 && t.channels[channels[0] as usize].name == name;
    let caption = (t.sweep_count > 1 || !one_title).then(|| {
        if t.sweep_count > 1 {
            format!("{name} · sweep {sweep} of {}", t.sweep_count)
        } else {
            name.clone()
        }
    });
    let header = if caption.is_some() {
        fs * 1.9
    } else {
        fs * 0.3
    };
    // above each panel: its title; between panels: a little air
    let title_h = fs * 1.9;
    let gap = fs * 0.9;
    let x_axis_h = fs * 3.4;
    let w = width as f32;
    let max_h = req.max_size as f32;
    let panel_h = if nc == 1 {
        (w * 0.42).clamp(110.0, 520.0)
    } else {
        ((w * 0.62) / nc as f32).clamp(70.0, 260.0)
    };
    let fit_h = ((max_h - header - x_axis_h - (title_h + gap) * nc as f32) / nc as f32).max(24.0);
    let panel_h = panel_h.min(fit_h);
    let height = header + nc as f32 * (title_h + panel_h + gap) - gap + x_axis_h;
    let mut fig = Figure::new(w, height.ceil(), Some(theme.paper));
    // x domain: the drawn window, left to right as the axis runs (NMR runs high to low)
    let reversed = ax.last < ax.first;
    let x_dom = {
        let d = Domain::linear(x_lo, x_hi);
        if reversed { d.reversed() } else { d }
    };
    // y domains, one per panel, and the widest tick label decides the left margin
    let y_doms: Vec<Domain> = cols
        .iter()
        .map(|cs| {
            let lo = cs
                .iter()
                .flatten()
                .map(|c| c.min)
                .fold(f64::INFINITY, f64::min);
            let hi = cs
                .iter()
                .flatten()
                .map(|c| c.max)
                .fold(f64::NEG_INFINITY, f64::max);
            let mut d = Domain::of([lo, hi], 0.06, openreadout_figure::ScaleKind::Linear);
            if peaks_on {
                // room above the tallest peak for its label
                d.hi += (d.hi - d.lo) * 0.12;
            }
            d
        })
        .collect();
    let probe_specs: Vec<AxesSpec> = (0..nc).map(|_| AxesSpec::default()).collect();
    let left_w = y_doms
        .iter()
        .zip(&probe_specs)
        .map(|(d, s)| {
            let p = Panel::fit(Rect::new(0.0, 0.0, 400.0, panel_h), x_dom, *d, &theme, s);
            p.rect.x
        })
        .fold(0.0f32, f32::max);
    if let Some(c) = &caption {
        let cap_style = TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic);
        fig.text(fs * 0.8, fs * 1.4, c, &cap_style);
    }
    let mut report = Vec::new();
    let mut panels = Vec::new();
    let mut placer = LabelPlacer::new(fig.rect());
    let mut data_marks: Vec<DataMark> = Vec::new();
    for (k, &c) in channels.iter().enumerate() {
        let ch = &t.channels[c as usize];
        let top = header + k as f32 * (title_h + panel_h + gap) + title_h;
        let last = k + 1 == nc;
        let rect = Rect::new(
            left_w + fs * 0.8,
            top,
            w - left_w - fs * 0.8 - fs * 1.6,
            panel_h,
        );
        let panel = Panel::new(rect, x_dom, y_doms[k]);
        let spec = AxesSpec {
            y_label: Some(channel_title(&ch.name, ch.unit.as_deref())),
            x_label: last.then(|| axis_title(&ax)),
            show_x: last,
            grid: GridLines::Y,
            ..AxesSpec::default()
        };
        if !last {
            // a hairline under each upper panel instead of a full axis
            fig.line(
                rect.x,
                rect.bottom(),
                rect.right(),
                rect.bottom(),
                Stroke::new(theme.grid, theme.hairline()),
            );
        }
        panel.axes(&mut fig, &theme, &spec);
        // zero line
        let (ylo, yhi) = panel.y.domain();
        if ylo < 0.0 && yhi > 0.0 {
            panel.rule_y(&mut fig, 0.0, Stroke::new(theme.rule, theme.hairline()));
        }
        let color = if nc == 1 { theme.ink } else { theme.series(k) };
        let pts = envelope_points(&cols[k], &panel);
        panel.line_px(&mut fig, pts, Stroke::new(color, theme.line_width * 0.85));
        if peaks_on {
            let resolution = (x_hi - x_lo).abs() / f64::from(rect.w.max(1.0));
            let dec = marks::decimals_for(resolution);
            let min_gap = (fs * 3.5 / (rect.w / ncols as f32).max(0.01)) as usize;
            let idx = marks::prominent(&cols[k], marks::MAX_PEAKS, min_gap.max(1), 0.04);
            let peaks: Vec<(f64, f64, String)> = idx
                .iter()
                .filter_map(|&i| cols[k][i])
                .map(|c| (c.x_at_max, c.max, marks::peak_label(c.x_at_max, dec)))
                .collect();
            data_marks.extend(marks::label_peaks(
                &mut fig,
                &panel,
                &theme,
                &peaks,
                Some(c),
                &mut placer,
            ));
        }
        if let Some((sk, factor)) = keep_spike
            && sk == k
        {
            data_marks.extend(spikes(
                &mut fig,
                &panel,
                &theme,
                &volts,
                factor,
                &t,
                sweep,
                &ax,
                c,
                &mut out.notes,
            ));
        }
        report.push(TracePreviewChannel {
            index: c,
            name: ch.name.clone(),
            unit: ch.unit.clone(),
            color: to_hex(color.to_rgb()),
            min: Some(ylo),
            max: Some(yhi),
        });
        panels.push(FramePanel {
            channel: Some(c),
            ..FramePanel::new(panel)
        });
    }
    out.marks = data_marks;
    out.trace = Some(TracePreview {
        trace: ti,
        sweep,
        sample_rate_hz: t.sample_rate_hz,
        sweep_sample_count: sweep_len,
        sample_count: drawn,
        truncated: drawn < sweep_len,
        x_axis: Some(ax),
        channels: report,
    });
    Ok((fig, Frame::plots(panels)))
}

/// The envelope of `cols` as a line in `panel`'s pixels: each column's first, extremes and
/// last value at the column's x.
pub(crate) fn envelope_points(cols: &[Option<Col>], panel: &Panel) -> Vec<(f32, f32)> {
    let mut pts = Vec::with_capacity(cols.len() * 4);
    for c in cols.iter().flatten() {
        let Some(px) = panel.x.px(c.x_at_max) else {
            continue;
        };
        let py = |v: f64| panel.y.px(v).unwrap_or(f32::NAN);
        pts.push((px, py(c.first)));
        if c.min < c.max {
            pts.push((px, py(c.min)));
            pts.push((px, py(c.max)));
        }
        pts.push((px, py(c.last)));
    }
    pts
}

/// Detect action potentials in `volts` (the channel's own unit; `factor` converts to mV) and
/// mark each with a small triangle above its peak; the count goes next to the panel title.
#[allow(clippy::too_many_arguments)]
fn spikes(
    fig: &mut Figure,
    panel: &Panel,
    theme: &Theme,
    volts: &[f64],
    factor: f64,
    t: &TraceInfo,
    sweep: u32,
    ax: &XAxis,
    channel: u32,
    notes: &mut Vec<String>,
) -> Vec<DataMark> {
    let mv: Vec<f64> = volts.iter().map(|v| v * factor).collect();
    let fs_hz = t.sample_rate_hz;
    let found = openreadout_signal::ephys::ap::detect(
        &mv,
        fs_hz,
        0,
        mv.len(),
        sweep,
        &openreadout_signal::ephys::ApSettings::default(),
    );
    let to_axis = |s: f64| if ax.unit == "min" { s / 60.0 } else { s };
    let start = t.start_s.unwrap_or(0.0);
    let size = theme.font_size * 0.55;
    let mut out = Vec::new();
    for s in &found {
        let x = to_axis(start + s.peak_time_s);
        let y = s.peak_mv / factor;
        if let Some((px, py)) = panel.pt(x, y) {
            let ty = (py - size * 0.6).max(panel.rect.y + size);
            fig.fill(
                Path::polygon([
                    (px, ty),
                    (px - size * 0.5, ty - size * 0.85),
                    (px + size * 0.5, ty - size * 0.85),
                ]),
                theme.accent,
            );
        }
        out.push(DataMark {
            kind: "spike".into(),
            x,
            y,
            label: None,
            channel: Some(channel),
        });
    }
    let text = match found.len() {
        0 => "no action potentials (−20 mV crossings)".to_string(),
        1 => "1 action potential".to_string(),
        n => format!("{n} action potentials"),
    };
    let st = TextStyle::new(theme.small_size(), theme.accent)
        .face(Face::SansMedium)
        .align(Anchor::End, Baseline::Alphabetic);
    fig.text(
        panel.rect.right(),
        panel.rect.y - theme.font_size * 0.75,
        &text,
        &st,
    );
    notes.push(format!(
        "{text}: action potentials are upward crossings of −20 mV (as openreadout_ephys_features counts them)"
    ));
    out
}

/// The x axis of the drawn window: the reader's `extra.axis` (NMR chemical shift, JCAMP-DX x
/// values) when present, otherwise time from `start_s` and `sample_rate_hz` (minutes when the
/// window spans more than five minutes), otherwise the sample index.
pub(crate) fn x_axis(t: &TraceInfo, sweep_len: u64, drawn: u64) -> XAxis {
    let last_i = drawn.saturating_sub(1) as f64;
    if let Some(a) = t.extra.get("axis") {
        let num = |k: &str| a.get(k).and_then(serde_json::Value::as_f64);
        let first = num("first");
        let step = num("step").or_else(|| {
            let (f, l) = (num("first")?, num("last")?);
            (sweep_len > 1).then(|| (l - f) / (sweep_len - 1) as f64)
        });
        if let (Some(first), Some(step)) = (first, step) {
            return XAxis {
                quantity: a
                    .get("quantity")
                    .and_then(|v| v.as_str())
                    .unwrap_or("x")
                    .into(),
                unit: a.get("unit").and_then(|v| v.as_str()).unwrap_or("").into(),
                first,
                last: first + step * last_i,
            };
        }
    }
    if t.sample_rate_hz > 0.0 && t.sample_rate_hz.is_finite() {
        let start = t.start_s.unwrap_or(0.0);
        let end = start + last_i / t.sample_rate_hz;
        if end - start > 300.0 {
            return XAxis {
                quantity: "time".into(),
                unit: "min".into(),
                first: start / 60.0,
                last: end / 60.0,
            };
        }
        return XAxis {
            quantity: "time".into(),
            unit: "s".into(),
            first: start,
            last: end,
        };
    }
    XAxis {
        quantity: "sample".into(),
        unit: String::new(),
        first: 0.0,
        last: last_i,
    }
}

/// A short decimal label (at most 4 significant digits; an exponent outside 1e-3..1e6).
pub(crate) fn fmt_num(v: f64) -> String {
    format_value(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_short() {
        assert_eq!(fmt_num(0.0), "0");
        assert_eq!(fmt_num(12.3456), "12.35");
        assert_eq!(fmt_num(1234.6), "1235");
        assert_eq!(fmt_num(-0.5), "-0.5");
        assert_eq!(fmt_num(2e7), "2e7");
        let ax = XAxis {
            quantity: "chemical_shift".into(),
            unit: "ppm".into(),
            first: 10.0,
            last: 0.0,
        };
        assert_eq!(axis_title(&ax), "chemical shift (ppm)");
        assert_eq!(channel_title("Vm", Some("mV")), "Vm (mV)");
    }
}
