//! NMR previews: the spectrum a chemist reads, not the raw FID. The vendor's processed
//! spectrum when the data set stores one, else the FID processed here (the same processing as
//! `analyze nmr-peaks`), drawn on a chemical-shift axis running high to low, with each
//! multiplet labelled once at its tallest line.

use openreadout_core::model::FileInfo;
use openreadout_core::reader::Dataset;
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Domain, Figure, GridLines, LabelPlacer, Layer, Panel, Rect, Stroke,
    TextStyle, superscript,
};
use openreadout_signal::nmr::{self, NmrRequest, PeakOptions};

use crate::color::to_hex;
use crate::marks::{self, Col};
use crate::trace::envelope_points;
use crate::{
    DataMark, Frame, FramePanel, PreviewOutput, PreviewRequest, TracePreview, TracePreviewChannel,
    XAxis,
};

/// Multiplets labelled at most.
const MAX_LABELS: usize = 12;

/// `1H` → `¹H`, `13C` → `¹³C`.
fn nucleus_label(n: &str) -> String {
    let digits: String = n.chars().take_while(char::is_ascii_digit).collect();
    let rest = &n[digits.len()..];
    match digits.parse::<i32>() {
        Ok(d) => format!("{}{rest}", superscript(d)),
        Err(_) => n.to_string(),
    }
}

/// The spectrum of an NMR data set; `None` when there is none to draw (the caller then draws
/// the trace as stored).
pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Option<(Figure, Frame)> {
    let Ok(chosen) = nmr::load(ds, info, &NmrRequest::default()) else {
        return None;
    };
    let s = &chosen.spectrum;
    let n = s.real.len().min(s.axis.size);
    if n < 8 {
        return None;
    }
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let w = req.max_size.max(64) as f32;
    let ncols = w as usize;
    let ax = s.axis;
    let (first, last) = (ax.ppm(0.0), ax.last_ppm());
    let (x_lo, x_hi) = (first.min(last), first.max(last));
    // one column per output pixel
    let mut cols: Vec<Option<Col>> = vec![None; ncols];
    for (i, &v) in s.real.iter().take(n).enumerate() {
        if !v.is_finite() {
            continue;
        }
        let x = ax.ppm(i as f64);
        let t = if x_hi > x_lo {
            (x - x_lo) / (x_hi - x_lo)
        } else {
            0.5
        };
        let c = ((t * ncols as f64) as usize).min(ncols - 1);
        match &mut cols[c] {
            None => cols[c] = Some(Col::new(x, v)),
            Some(col) => col.add(x, v),
        }
    }
    let (lo, hi) = cols
        .iter()
        .flatten()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), c| {
            (a.min(c.min), b.max(c.max))
        });
    let mut y_dom = Domain::of(
        [lo.min(0.0), hi],
        0.04,
        openreadout_figure::ScaleKind::Linear,
    );
    // room above the tallest line for its label
    y_dom.hi += (y_dom.hi - y_dom.lo) * 0.1;
    let x_dom = Domain::linear(x_lo, x_hi).reversed();

    // caption: nucleus, field, where the spectrum came from
    let trace_name = info
        .traces
        .iter()
        .find(|t| t.index == chosen.trace)
        .and_then(|t| t.name.clone())
        .unwrap_or_else(|| format!("trace {}", chosen.trace));
    let mut parts: Vec<String> = Vec::new();
    if let Some(nu) = &chosen.nucleus {
        parts.push(nucleus_label(nu));
    }
    if ax.reference_frequency_mhz > 0.0 {
        parts.push(format!("{:.0} MHz", ax.reference_frequency_mhz));
    }
    parts.push(if chosen.source == "fid" {
        format!("processed from the FID ({trace_name})")
    } else {
        trace_name.clone()
    });
    let caption = parts.join(" · ");

    let header = fs * 1.9;
    let x_axis_h = fs * 3.4;
    let panel_h = (w * 0.46)
        .clamp(120.0, 560.0)
        .min(req.max_size as f32 - header - x_axis_h);
    let height = (header + fs * 1.9 + panel_h + x_axis_h).ceil();
    let mut fig = Figure::new(w, height, Some(theme.paper));
    let spec = AxesSpec {
        y_label: Some("intensity".into()),
        x_label: Some("chemical shift (ppm)".into()),
        grid: GridLines::Y,
        x_ticks: 10,
        ..AxesSpec::default()
    };
    let probe = Panel::fit(
        Rect::new(0.0, 0.0, 400.0, panel_h),
        x_dom,
        y_dom,
        &theme,
        &spec,
    );
    let left = probe.rect.x;
    let rect = Rect::new(
        left + fs * 0.8,
        header + fs * 1.9,
        w - left - fs * 0.8 - fs * 1.6,
        panel_h,
    );
    let panel = Panel::new(rect, x_dom, y_dom);
    fig.group(Layer::Caption, "caption", |fig| {
        fig.text(
            fs * 0.8,
            fs * 1.4,
            &caption,
            &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
        );
    });
    panel.axes(&mut fig, &theme, &spec);
    if y_dom.lo < 0.0 {
        panel.rule_y(&mut fig, 0.0, Stroke::new(theme.rule, theme.hairline()));
    }
    fig.group(Layer::Series, "spectrum", |fig| {
        panel.line_px(
            fig,
            envelope_points(&cols, &panel),
            Stroke::new(theme.ink, theme.line_width * 0.85),
        );
    });

    // peaks: one label per multiplet, at its tallest line, tallest multiplets first
    let (peaks, _sd) = nmr::pick_peaks(&s.real[..n], &ax, &PeakOptions::default());
    let regions = nmr::multiplet_regions(&peaks, &ax);
    let mut tops: Vec<&nmr::Peak> = regions
        .iter()
        .filter_map(|&(a, b)| {
            let (a, b) = (a.min(b), a.max(b));
            peaks
                .iter()
                .filter(|p| p.ppm >= a && p.ppm <= b && p.height > 0.0)
                .max_by(|p, q| p.height.total_cmp(&q.height))
        })
        .collect();
    tops.sort_by(|p, q| q.height.total_cmp(&p.height));
    // lines too small to see at this size are not labelled
    let floor = (y_dom.hi - y_dom.lo) * 0.03;
    let labelled: Vec<(f64, f64, String)> = tops
        .iter()
        .filter(|p| p.height >= floor)
        .take(MAX_LABELS)
        .map(|p| (p.ppm, p.height, format!("{:.2}", p.ppm)))
        .collect();
    let mut placer = LabelPlacer::new(fig.rect());
    let data_marks: Vec<DataMark> =
        marks::label_peaks(&mut fig, &panel, &theme, &labelled, Some(0), &mut placer);
    out.marks = data_marks;
    if chosen.source == "fid" {
        out.notes.push(format!(
            "no processed spectrum is stored: the FID ({trace_name}) was processed here as the NMR peak analysis does; pick trace {} to see the raw FID",
            chosen.trace
        ));
    } else {
        out.notes.push(format!(
            "showing the processed spectrum ({trace_name}); pick a trace to see the stored data as it is (the FID, for one)"
        ));
    }
    out.notes.extend(chosen.notes.iter().take(3).cloned());
    let x_axis = XAxis {
        quantity: "chemical_shift".into(),
        unit: "ppm".into(),
        first,
        last,
    };
    out.trace = Some(TracePreview {
        trace: chosen.trace,
        sweep: 0,
        sample_rate_hz: 0.0,
        sweep_sample_count: n as u64,
        sample_count: n as u64,
        truncated: false,
        x_axis: Some(x_axis),
        channels: vec![TracePreviewChannel {
            index: 0,
            name: "real".into(),
            unit: None,
            color: to_hex(theme.ink.to_rgb()),
            min: Some(y_dom.lo),
            max: Some(y_dom.hi),
        }],
    });
    let panels = vec![FramePanel {
        channel: Some(0),
        ..FramePanel::new(panel)
    }];
    Some((fig, Frame::plots_labelled(panels, placer.taken())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nuclei_read_as_written() {
        assert_eq!(nucleus_label("1H"), "¹H");
        assert_eq!(nucleus_label("13C"), "¹³C");
        assert_eq!(nucleus_label("X"), "X");
    }
}
