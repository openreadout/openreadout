//! The first look at a mass-spectrometry run: its total-ion chromatogram, built from the
//! total ion current each MS1 scan header records (the base-peak intensity when no TIC is
//! stored), with the apex and the other prominent peaks labelled with their retention time.
//! Only readers that list scan headers without decoding peaks take this path, so it stays quick
//! on runs of tens of thousands of spectra; others preview spectrum 0 as before.

use openreadout_core::Result;
use openreadout_core::model::FileInfo;
use openreadout_core::reader::Dataset;
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Domain, Face, Figure, GridLines, LabelPlacer, Panel, Path, Rect,
    ScaleKind, Stroke, TextStyle,
};

use crate::marks::{self, Col};
use crate::trace::envelope_points;
use crate::{ChromatogramPreview, Frame, FramePanel, PreviewOutput, PreviewRequest};

/// Fewer MS1 scans than this make no chromatogram; the preview shows a spectrum instead.
const MIN_SCANS: usize = 20;

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<Option<(Figure, Frame)>> {
    let Some(run) = info.spectra.first().map(|s| s.index) else {
        return Ok(None);
    };
    let mut tic: Vec<(f64, f64)> = Vec::new();
    let mut bpc: Vec<(f64, f64)> = Vec::new();
    let mut scans = 0u64;
    let listed = ds.visit_scan_headers(run, 0, &mut |h| {
        scans += 1;
        if h.ms_level == 1
            && let Some(rt) = h.rt_s.filter(|v| v.is_finite())
        {
            if let Some(t) = h.total_ion_current.filter(|v| v.is_finite()) {
                tic.push((rt / 60.0, t));
            }
            if let Some(b) = h.base_peak_intensity.filter(|v| v.is_finite()) {
                bpc.push((rt / 60.0, b));
            }
        }
        true
    })?;
    if !listed {
        return Ok(None);
    }
    let (signal, pts) = if tic.len() >= MIN_SCANS {
        ("tic", tic)
    } else if bpc.len() >= MIN_SCANS {
        ("bpc", bpc)
    } else {
        return Ok(None);
    };
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let w = req.max_size.max(64) as f32;
    let ncols = w as usize;
    let (x_lo, x_hi) = pts
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
            (a.min(p.0), b.max(p.0))
        });
    let mut cols: Vec<Option<Col>> = vec![None; ncols];
    for &(x, y) in &pts {
        let t = if x_hi > x_lo {
            (x - x_lo) / (x_hi - x_lo)
        } else {
            0.5
        };
        let c = ((t * ncols as f64) as usize).min(ncols - 1);
        match &mut cols[c] {
            None => cols[c] = Some(Col::new(x, y)),
            Some(col) => col.add(x, y),
        }
    }
    let apex = pts
        .iter()
        .copied()
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((x_lo, 0.0));
    let y_top = apex.1.max(0.0);
    let mut y_dom = Domain::of([0.0, y_top], 0.0, ScaleKind::Linear);
    y_dom.hi += (y_dom.hi - y_dom.lo) * 0.14;
    let x_dom = Domain::linear(x_lo, x_hi);

    let header = fs * 1.9;
    let panel_h = (w * 0.42).clamp(120.0, 520.0);
    let height = (header + fs * 1.9 + panel_h + fs * 3.4).ceil();
    let mut fig = Figure::new(w, height, Some(theme.paper));
    let what = if signal == "tic" {
        "Total ion current"
    } else {
        "Base peak intensity"
    };
    fig.text(
        fs * 0.8,
        fs * 1.4,
        &format!("{what} · {} MS1 scans of {scans}", pts.len()),
        &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
    );
    let spec = AxesSpec {
        y_label: Some("intensity".into()),
        x_label: Some("retention time (min)".into()),
        grid: GridLines::Y,
        ..AxesSpec::default()
    };
    let probe = Panel::fit(
        Rect::new(0.0, 0.0, 400.0, panel_h),
        x_dom,
        y_dom,
        &theme,
        &spec,
    );
    let rect = Rect::new(
        probe.rect.x + fs * 0.8,
        header + fs * 1.9,
        w - probe.rect.x - fs * 0.8 - fs * 1.6,
        panel_h,
    );
    let panel = Panel::new(rect, x_dom, y_dom);
    panel.axes(&mut fig, &theme, &spec);
    panel.line_px(
        &mut fig,
        envelope_points(&cols, &panel),
        Stroke::new(theme.ink, theme.line_width * 0.85),
    );
    // the apex, labelled in the accent; the other prominent peaks in ink
    let mut placer = LabelPlacer::new(fig.rect());
    let mut data_marks = Vec::new();
    if let Some((px, py)) = panel.pt(apex.0, apex.1) {
        let st = TextStyle::new(theme.small_size(), theme.accent)
            .face(Face::Mono)
            .align(Anchor::Middle, Baseline::Alphabetic)
            .halo(theme.paper, (fs * 0.18).max(1.5));
        let label = format!("{:.1} min", apex.0);
        let ly = py - fs * 0.5;
        let b = st.bounds(px, ly, &label);
        let b = Rect::new(b.x.max(rect.x).min(rect.right() - b.w), b.y, b.w, b.h);
        fig.fill(Path::circle(px, py, fs * 0.22), theme.accent);
        fig.text(b.x + b.w / 2.0, ly, &label, &st);
        placer.reserve(Rect::new(b.x - 2.0, b.y - 2.0, b.w + 4.0, b.h + 4.0));
        data_marks.push(crate::DataMark {
            kind: "peak".into(),
            x: apex.0,
            y: apex.1,
            label: Some(label),
            channel: None,
        });
    }
    let resolution = (x_hi - x_lo).abs() / f64::from(rect.w.max(1.0));
    let dec = marks::decimals_for(resolution).min(2);
    let min_gap = (fs * 3.5) as usize;
    let others: Vec<(f64, f64, String)> =
        marks::prominent(&cols, marks::MAX_PEAKS, min_gap.max(1), 0.08)
            .iter()
            .filter_map(|&i| cols[i])
            .filter(|c| (c.x_at_max - apex.0).abs() > resolution * 2.0)
            .map(|c| (c.x_at_max, c.max, marks::peak_label(c.x_at_max, dec)))
            .collect();
    data_marks.extend(marks::label_peaks(
        &mut fig,
        &panel,
        &theme,
        &others,
        None,
        &mut placer,
    ));
    out.marks = data_marks;
    out.chromatogram = Some(ChromatogramPreview {
        run,
        signal: signal.into(),
        ms1_scans: pts.len() as u64,
        scans,
        apex_rt_min: apex.0,
        apex_intensity: apex.1,
    });
    out.notes.push(format!(
        "{} from the {} MS1 scan headers; pass spectrum or scan for a mass spectrum",
        if signal == "tic" {
            "total-ion chromatogram"
        } else {
            "base-peak chromatogram"
        },
        pts.len()
    ));
    Ok(Some((fig, Frame::plots(vec![FramePanel::new(panel)]))))
}
