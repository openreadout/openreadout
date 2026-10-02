//! Trace previews: a sparkline of one sweep per channel. Each pixel column shows the min/max
//! envelope of the samples that fall into it, joined to its neighbours, so spikes survive any
//! amount of downsampling.

use openreadout_core::model::FileInfo;
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};

use crate::canvas::Canvas;
use crate::color::{self, TRACE_PALETTE};
use crate::{PreviewOutput, PreviewRequest, TracePreview, TracePreviewChannel, XAxis};

/// Channels drawn when none are requested.
const DEFAULT_CHANNELS: usize = 8;
/// Most samples per channel read for one preview (the rest of a longer sweep is not drawn).
pub const MAX_TRACE_PREVIEW_SAMPLES: u64 = 20_000_000;
/// Values requested from the reader per batch, across all of the trace's channels.
const BATCH_VALUES: u64 = 1 << 23;

#[derive(Clone, Copy)]
struct Col {
    min: f64,
    max: f64,
    first: f64,
    last: f64,
}

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<Canvas> {
    let ti = req.trace.unwrap_or(0);
    let sweep = req.sweep.unwrap_or(0);
    let t = info
        .traces
        .iter()
        .find(|t| t.index == ti)
        .ok_or_else(|| {
            if info.traces.is_empty() {
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
        })?
        .clone();
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
    // An irregular abscissa stored as a channel (`extra.axis.irregular`, `channel`: a grating
    // spectrometer's Raman-shift list, a peak table's x) is the x axis, not a signal to draw.
    let x_channel = t
        .extra
        .get("axis")
        .filter(|a| a.get("irregular").and_then(serde_json::Value::as_bool) == Some(true))
        .and_then(|a| a.get("channel"))
        .and_then(serde_json::Value::as_u64);
    let channels: Vec<u32> = if req.channels.is_empty() {
        let signal: Vec<u32> = (0..nch as u32)
            .filter(|c| x_channel != Some(u64::from(*c)) || nch == 1)
            .collect();
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
    let width = req.max_size.max(64);
    let plot_w = u64::from(width - 8);
    let mut cols: Vec<Vec<Option<Col>>> = vec![vec![None; plot_w as usize]; channels.len()];
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
                if !v.is_finite() {
                    continue;
                }
                let x = ((u128::from(pos + i as u64) * u128::from(plot_w)) / u128::from(n.max(1)))
                    as usize;
                let slot = &mut cols[k][x.min(plot_w as usize - 1)];
                *slot = Some(match *slot {
                    None => Col {
                        min: v,
                        max: v,
                        first: v,
                        last: v,
                    },
                    Some(c) => Col {
                        min: c.min.min(v),
                        max: c.max.max(v),
                        first: c.first,
                        last: v,
                    },
                });
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
    let nc = channels.len() as u32;
    let gap = 4u32;
    let axis_scale: i64 = if width >= 256 { 2 } else { 1 };
    let axis_h = (5 * axis_scale + 6) as u32;
    // Panels share the height left under `max_size` (at least 8 px each).
    let fit = req
        .max_size
        .saturating_sub(axis_h + (nc + 1) * gap)
        .checked_div(nc)
        .unwrap_or(0)
        .max(8);
    let panel_h = if nc == 1 {
        (width / 4).clamp(64, 256)
    } else {
        (width * 3 / 4 / nc).clamp(32, 160)
    }
    .min(fit);
    let height = nc * panel_h + (nc + 1) * gap + axis_h;
    let mut canvas = Canvas::new(width, height, [255, 255, 255]);
    let mut report = Vec::new();
    for (k, &c) in channels.iter().enumerate() {
        let ch = &t.channels[c as usize];
        let top = i64::from(gap + k as u32 * (panel_h + gap));
        let ph = i64::from(panel_h);
        canvas.fill_rect(4, top, plot_w as i64, ph, [247, 247, 247]);
        let col = TRACE_PALETTE[k % TRACE_PALETTE.len()];
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for s in cols[k].iter().flatten() {
            lo = lo.min(s.min);
            hi = hi.max(s.max);
        }
        let range = if lo.is_finite() && hi.is_finite() {
            let pad = if hi > lo {
                (hi - lo) * 0.05
            } else {
                lo.abs().max(1.0) * 0.05
            };
            Some((lo - pad, hi + pad))
        } else {
            None
        };
        if let Some((lo, hi)) = range {
            let y_of =
                |v: f64| top + (ph - 1) - (((v - lo) / (hi - lo)) * (ph - 1) as f64).round() as i64;
            if lo < 0.0 && hi > 0.0 {
                let y0 = y_of(0.0);
                canvas.line(4, y0, 4 + plot_w as i64 - 1, y0, [210, 210, 210]);
            }
            let mut prev: Option<(i64, f64)> = None;
            for (x, s) in cols[k].iter().enumerate() {
                let Some(s) = s else {
                    continue;
                };
                let px = 4 + x as i64;
                if let Some((pxp, last)) = prev {
                    canvas.line(pxp, y_of(last), px, y_of(s.first), col);
                }
                canvas.line(px, y_of(s.max), px, y_of(s.min), col);
                prev = Some((px, s.last));
            }
        }
        let scale = if panel_h >= 48 { 2 } else { 1 };
        let unit = ch.unit.as_deref().unwrap_or("").trim();
        let mut label: String = ch.name.chars().take(40).collect();
        if !unit.is_empty() {
            label.push_str(&format!(" ({unit})"));
        }
        canvas.text(8, top + 3, scale, [90, 90, 90], &label);
        // The y range of the panel, at its right edge (top = max, bottom = min).
        if let Some((lo, hi)) = range {
            let right = 4 + plot_w as i64 - 4;
            let hi_s = fmt_num(hi);
            let lo_s = fmt_num(lo);
            let grey = [120, 120, 120];
            canvas.text(
                right - crate::canvas::text_width(&hi_s, scale),
                top + 3,
                scale,
                grey,
                &hi_s,
            );
            canvas.text(
                right - crate::canvas::text_width(&lo_s, scale),
                top + ph - 3 - 5 * scale,
                scale,
                grey,
                &lo_s,
            );
        }
        report.push(TracePreviewChannel {
            index: c,
            name: ch.name.clone(),
            unit: ch.unit.clone(),
            color: color::to_hex(col),
            min: range.map(|r| r.0),
            max: range.map(|r| r.1),
        });
    }
    let x_axis = x_axis(&t, sweep_len, drawn);
    draw_x_labels(&mut canvas, &x_axis, axis_scale, axis_h);
    out.trace = Some(TracePreview {
        trace: ti,
        sweep,
        sample_rate_hz: t.sample_rate_hz,
        sweep_sample_count: sweep_len,
        sample_count: drawn,
        truncated: drawn < sweep_len,
        x_axis: Some(x_axis),
        channels: report,
    });
    Ok(canvas)
}

/// Label the two ends of the x axis in the strip at the bottom of the canvas.
pub(crate) fn draw_x_labels(canvas: &mut Canvas, a: &XAxis, scale: i64, strip_h: u32) {
    let y = i64::from(canvas.height - strip_h) + 2;
    let left = format!("{} {}", fmt_num(a.first), a.unit);
    let right = format!("{} {}", fmt_num(a.last), a.unit);
    canvas.text(4, y, scale, [90, 90, 90], left.trim());
    let rw = crate::canvas::text_width(right.trim(), scale);
    canvas.text(
        i64::from(canvas.width) - 4 - rw,
        y,
        scale,
        [90, 90, 90],
        right.trim(),
    );
}

/// The x axis of the drawn window: the reader's `extra.axis` (NMR chemical shift, JCAMP-DX x
/// values) when present, otherwise time from `start_s` and `sample_rate_hz` (minutes when the
/// window spans more than five minutes), otherwise the sample index.
pub(crate) fn x_axis(t: &openreadout_core::model::TraceInfo, sweep_len: u64, drawn: u64) -> XAxis {
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
    if !v.is_finite() {
        return "-".into();
    }
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-3..1e6).contains(&a) {
        return format!("{v:.2e}");
    }
    let decimals = (3 - a.log10().floor() as i32).clamp(0, 6) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
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
        assert_eq!(fmt_num(2e7), "2.00e7");
    }
}
