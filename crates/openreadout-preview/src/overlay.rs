//! Overlay previews: every channel of a trace drawn in one panel with a shared y axis — the
//! amplification and melt plots of qPCR files, whose traces say `extra.plot = "overlay"` (one
//! channel per well).

use openreadout_core::model::TraceInfo;
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};

use crate::canvas::{Canvas, text_width};
use crate::color::{self, TRACE_PALETTE};
use crate::trace::{draw_x_labels, fmt_num, x_axis};
use crate::{PreviewOutput, PreviewRequest, TracePreview, TracePreviewChannel};

/// Most samples per channel drawn (overlay traces are short: cycles or melt reads).
const MAX_SAMPLES: u64 = 100_000;
/// Most channels drawn in one overlay.
const MAX_CHANNELS: usize = 1536;

pub(crate) fn render(
    ds: &mut dyn Dataset,
    t: &TraceInfo,
    sweep: u32,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<Canvas> {
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
    let tr = ds.read_trace(t.index, sweep, 0, n)?;
    let data: Vec<&Vec<f64>> = channels
        .iter()
        .map(|&c| {
            tr.channels
                .get(c as usize)
                .ok_or_else(|| Error::Other(format!("reader returned no channel {c}")))
        })
        .collect::<Result<_>>()?;
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in &data {
        for &x in *v {
            if x.is_finite() {
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
    }
    let width = req.max_size.max(64);
    let height = (width * 2 / 3).clamp(64, req.max_size.max(64));
    let axis_scale: i64 = if width >= 256 { 2 } else { 1 };
    let axis_h = (5 * axis_scale + 6) as u32;
    let mut canvas = Canvas::new(width, height, [255, 255, 255]);
    let (px0, py0) = (4i64, 4i64);
    let pw = i64::from(width) - 8;
    let ph = i64::from(height) - 8 - i64::from(axis_h);
    canvas.fill_rect(px0, py0, pw, ph, [247, 247, 247]);
    let range = (lo.is_finite() && hi.is_finite()).then(|| {
        let pad = if hi > lo {
            (hi - lo) * 0.05
        } else {
            lo.abs().max(1.0) * 0.05
        };
        (lo - pad, hi + pad)
    });
    let mut report = Vec::with_capacity(channels.len());
    let samples = data.iter().map(|v| v.len()).max().unwrap_or(0);
    if let Some((lo, hi)) = range {
        let y_of =
            |v: f64| py0 + (ph - 1) - (((v - lo) / (hi - lo)) * (ph - 1) as f64).round() as i64;
        let x_of = |i: usize| {
            px0 + if samples > 1 {
                (i as i64 * (pw - 1)) / (samples as i64 - 1)
            } else {
                0
            }
        };
        if lo < 0.0 && hi > 0.0 {
            let y0 = y_of(0.0);
            canvas.line(px0, y0, px0 + pw - 1, y0, [210, 210, 210]);
        }
        for (k, v) in data.iter().enumerate() {
            let col = TRACE_PALETTE[k % TRACE_PALETTE.len()];
            for i in 1..v.len() {
                let (a, b) = (v[i - 1], v[i]);
                if a.is_finite() && b.is_finite() {
                    canvas.line(x_of(i - 1), y_of(a), x_of(i), y_of(b), col);
                } else if b.is_finite() {
                    canvas.set(x_of(i), y_of(b), col);
                }
            }
            let ch = &t.channels[channels[k] as usize];
            report.push(TracePreviewChannel {
                index: channels[k],
                name: ch.name.clone(),
                unit: ch.unit.clone(),
                color: color::to_hex(col),
                min: Some(lo),
                max: Some(hi),
            });
        }
        let grey = [120, 120, 120];
        let (hs, ls) = (fmt_num(hi), fmt_num(lo));
        let right = px0 + pw - 4;
        canvas.text(
            right - text_width(&hs, axis_scale),
            py0 + 3,
            axis_scale,
            grey,
            &hs,
        );
        canvas.text(
            right - text_width(&ls, axis_scale),
            py0 + ph - 3 - 5 * axis_scale,
            axis_scale,
            grey,
            &ls,
        );
    }
    let label: String = t
        .name
        .clone()
        .unwrap_or_else(|| format!("trace {}", t.index))
        .chars()
        .take(48)
        .collect();
    canvas.text(px0 + 4, py0 + 3, axis_scale, [90, 90, 90], &label);
    let ax = x_axis(t, len, n);
    draw_x_labels(&mut canvas, &ax, axis_scale, axis_h);
    out.notes.push(format!(
        "{} channel(s) overlaid in one panel with a shared y axis",
        channels.len()
    ));
    out.trace = Some(TracePreview {
        trace: t.index,
        sweep,
        sample_rate_hz: t.sample_rate_hz,
        sweep_sample_count: len,
        sample_count: n,
        truncated: n < len,
        x_axis: Some(ax),
        channels: report,
    });
    Ok(canvas)
}
