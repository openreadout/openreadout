//! Mass-spectrum previews: centroided spectra as sticks, profile spectra as a line (min/max
//! envelope per pixel column), m/z increasing left to right, intensity from zero; the largest
//! peaks labelled with their m/z, the precursor of a fragment scan marked.

use openreadout_core::model::FileInfo;
use openreadout_core::reader::{Dataset, SpectrumView, spectrum_by_scan};
use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Domain, Face, Figure, LabelPlacer, Layer, Panel, Path, Stroke,
    TextStyle,
};

use crate::marks::{self, Col};
use crate::trace::{envelope_points, fmt_num};
use crate::{Frame, FramePanel, Marks, PreviewOutput, PreviewRequest, SpectrumPreview, XAxis};

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    let run = req.run.unwrap_or(0);
    let r = info
        .spectra
        .iter()
        .find(|s| s.index == run)
        .ok_or_else(|| {
            if info.spectra.is_empty() {
                Error::unsupported(
                    "preview",
                    format!("spectrum preview of a {} file", info.format.name),
                    "This file holds no mass spectra; see `openreadout info` for what it contains.",
                )
            } else {
                Error::Usage(format!(
                    "run {run} out of range (file has {} runs)",
                    info.spectra.len()
                ))
            }
        })?;
    let view = if req.centroid {
        SpectrumView::Centroid
    } else {
        SpectrumView::Primary
    };
    let (index, sp) = if let Some(scan) = req.scan {
        let sp = spectrum_by_scan(ds, run, scan, view)?;
        (sp.index, sp)
    } else {
        let i = req.spectrum.unwrap_or(0);
        if i >= r.scan_count {
            return Err(Error::Usage(format!(
                "spectrum {i} out of range (run {run} has {} spectra, 0..{})",
                r.scan_count, r.scan_count
            )));
        }
        (i, ds.read_spectrum_view(run, i, view)?)
    };
    let theme = crate::theme(req);
    let fs = theme.font_size;
    let w = req.max_size.max(64) as f32;
    let h = (w * 0.5).round().min(req.max_size as f32);
    let mut fig = Figure::new(w, h, Some(theme.paper));

    let pts: Vec<(f64, f64)> = sp
        .mz
        .iter()
        .zip(&sp.intensity)
        .map(|(&m, &i)| (m, f64::from(i)))
        .filter(|(m, i)| m.is_finite() && i.is_finite())
        .collect();
    let (mut lo, mut hi) = pts
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
            (a.min(p.0), b.max(p.0))
        });
    if let Some([a, b]) = sp.scan_window_mz
        && a.is_finite()
        && b.is_finite()
        && b > a
    {
        lo = lo.min(a);
        hi = hi.max(b);
    }
    if !(lo.is_finite() && hi.is_finite()) {
        (lo, hi) = (0.0, 1.0);
    } else if hi <= lo {
        (lo, hi) = (lo - 0.5, hi + 0.5);
    }
    let imax = pts.iter().map(|p| p.1).fold(0.0f64, f64::max);
    let sticks = sp.centroided || pts.len() < 2;
    let labels_on = matches!(req.marks, Marks::Auto | Marks::Peaks);

    // caption
    let mut caption = format!(
        "MS{} · scan {} · RT {}",
        sp.ms_level,
        sp.scan_number,
        sp.rt_s.map_or_else(
            || "not stated".into(),
            |t| format!("{} min", fmt_num(t / 60.0))
        )
    );
    if let Some(p) = sp.precursor_mz.filter(|p| p.is_finite()) {
        caption.push_str(&format!(" · precursor m/z {p:.4}"));
    }
    let spec = AxesSpec {
        x_label: Some("m/z".into()),
        y_label: Some("intensity".into()),
        top: fs * 1.6,
        ..AxesSpec::default()
    };
    let y_top = if imax > 0.0 {
        imax * if labels_on { 1.12 } else { 1.04 }
    } else {
        1.0
    };
    let panel = Panel::fit(
        fig.rect().inset(fs * 0.8),
        Domain::linear(lo, hi),
        Domain::linear(0.0, y_top),
        &theme,
        &spec,
    );
    fig.group(Layer::Caption, "caption", |fig| {
        fig.text(
            fs * 0.8,
            fs * 1.4,
            &caption,
            &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
        );
    });
    panel.axes(&mut fig, &theme, &spec);
    // precursor: a dashed accent rule under the data
    if let Some(p) = sp
        .precursor_mz
        .filter(|p| p.is_finite() && *p >= lo && *p <= hi)
    {
        fig.group(Layer::Annotation, "precursor", |fig| {
            panel.rule_x(
                fig,
                p,
                Stroke::new(theme.accent.alpha(0.7), theme.hairline()).dashed(4.0, 3.0),
            );
            if let Some(px) = panel.x.px(p) {
                let st = TextStyle::new(theme.small_size(), theme.accent)
                    .face(Face::Mono)
                    .align(Anchor::Start, Baseline::Top);
                fig.text(px + 4.0, panel.rect.y + 2.0, "precursor", &st);
            }
        });
    }
    let ncols = panel.rect.w.max(1.0) as usize;
    let mut cols: Vec<Option<Col>> = vec![None; ncols];
    for &(m, i) in &pts {
        let t = (m - lo) / (hi - lo);
        let c = ((t * ncols as f64) as usize).min(ncols - 1);
        match &mut cols[c] {
            None => cols[c] = Some(Col::new(m, i)),
            Some(col) => col.add(m, i),
        }
    }
    let ink = Stroke::new(theme.ink, (theme.line_width * 0.8).max(1.0));
    fig.group(Layer::Series, "spectrum", |fig| {
        if sticks {
            let mut path = Path::new();
            let base = panel.rect.bottom();
            for c in cols.iter().flatten() {
                if let Some((px, py)) = panel.pt(c.x_at_max, c.max.max(0.0)) {
                    path.move_to(px, base).line_to(px, py.min(base - 0.5));
                }
            }
            fig.shape(path, None, Some(ink), Some(panel.rect));
        } else {
            // profile: each column starts where the previous ended
            let mut zeroed = cols.clone();
            for c in zeroed.iter_mut().flatten() {
                c.min = c.min.max(0.0);
            }
            panel.line_px(fig, envelope_points(&zeroed, &panel), ink);
        }
    });
    let mut placer = LabelPlacer::new(fig.rect());
    if labels_on && imax > 0.0 {
        let min_gap = (fs * 4.5) as usize;
        let idx = marks::prominent(&cols, marks::MAX_PEAKS, min_gap.max(1), 0.02);
        // m/z to four decimals, as mass spectrometrists quote them
        let dec = 4;
        let peaks: Vec<(f64, f64, String)> = idx
            .iter()
            .filter_map(|&i| cols[i])
            .map(|c| (c.x_at_max, c.max, marks::peak_label(c.x_at_max, dec)))
            .collect();
        out.marks = marks::label_peaks(&mut fig, &panel, &theme, &peaks, None, &mut placer);
    }
    let x_axis = XAxis {
        quantity: "mz".into(),
        unit: "m/z".into(),
        first: lo,
        last: hi,
    };
    out.spectrum = Some(SpectrumPreview {
        run,
        index,
        scan_number: sp.scan_number,
        ms_level: sp.ms_level,
        rt_s: sp.rt_s,
        style: if sticks { "sticks" } else { "profile" }.into(),
        point_count: sp.mz.len() as u64,
        x_axis,
        max_intensity: (imax > 0.0).then_some(imax),
        precursor_mz: sp.precursor_mz,
    });
    Ok((
        fig,
        Frame::plots_labelled(vec![FramePanel::new(panel)], placer.taken()),
    ))
}
