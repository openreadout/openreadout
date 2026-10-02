//! Mass-spectrum previews: centroided spectra as sticks, profile spectra as a line (min/max
//! envelope per pixel column), m/z increasing left to right, intensity from zero to the maximum.

use openreadout_core::model::FileInfo;
use openreadout_core::reader::{Dataset, SpectrumView, spectrum_by_scan};
use openreadout_core::{Error, Result};

use crate::canvas::Canvas;
use crate::trace::{draw_x_labels, fmt_num};
use crate::{PreviewOutput, PreviewRequest, SpectrumPreview, XAxis};

const INK: [u8; 3] = [31, 119, 180];

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<Canvas> {
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
    let width = req.max_size.max(64);
    let plot_h = (width * 3 / 8).clamp(64, 480);
    let axis_scale: i64 = if width >= 256 { 2 } else { 1 };
    let strip_h = (5 * axis_scale + 6) as u32;
    let top = 4u32 + (5 * axis_scale + 4) as u32;
    let height = top + plot_h + 4 + strip_h;
    let mut canvas = Canvas::new(width, height, [255, 255, 255]);
    let plot_w = u64::from(width - 8);
    canvas.fill_rect(
        4,
        i64::from(top),
        plot_w as i64,
        i64::from(plot_h),
        [247, 247, 247],
    );

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
    let base = i64::from(top + plot_h - 1);
    let ph = i64::from(plot_h - 1);
    let x_of = |m: f64| 4 + (((m - lo) / (hi - lo)) * (plot_w - 1) as f64).round() as i64;
    let y_of = |v: f64| {
        if imax > 0.0 {
            base - ((v.max(0.0) / imax) * ph as f64).round() as i64
        } else {
            base
        }
    };
    let sticks = sp.centroided || pts.len() < 2;
    if sticks {
        for &(m, i) in &pts {
            let x = x_of(m);
            canvas.line(x, base, x, y_of(i), INK);
        }
    } else {
        // envelope per column, joined to its neighbours
        let mut cols: Vec<Option<(f64, f64, f64, f64)>> = vec![None; plot_w as usize];
        let mut sorted = pts.clone();
        sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
        for &(m, i) in &sorted {
            let x = (x_of(m) - 4).clamp(0, plot_w as i64 - 1) as usize;
            cols[x] = Some(match cols[x] {
                None => (i, i, i, i),
                Some((mn, mx, f, _)) => (mn.min(i), mx.max(i), f, i),
            });
        }
        let mut prev: Option<(i64, f64)> = None;
        for (x, c) in cols.iter().enumerate() {
            let Some((mn, mx, first, last)) = *c else {
                continue;
            };
            let px = 4 + x as i64;
            if let Some((pxp, pl)) = prev {
                canvas.line(pxp, y_of(pl), px, y_of(first), INK);
            }
            canvas.line(px, y_of(mx), px, y_of(mn), INK);
            prev = Some((px, last));
        }
    }
    if let Some(p) = sp
        .precursor_mz
        .filter(|p| p.is_finite() && *p >= lo && *p <= hi)
    {
        let x = x_of(p);
        for y in (i64::from(top)..=base).step_by(4) {
            canvas.set(x, y, [214, 39, 40]);
        }
    }
    let title = format!(
        "MS{} scan {} RT {}{}",
        sp.ms_level,
        sp.scan_number,
        sp.rt_s.map_or_else(
            || "not stated".into(),
            |t| format!("{} min", fmt_num(t / 60.0))
        ),
        if imax > 0.0 {
            format!(" max {}", fmt_num(imax))
        } else {
            String::new()
        }
    );
    canvas.text(4, 3, axis_scale, [90, 90, 90], &title);
    let x_axis = XAxis {
        quantity: "mz".into(),
        unit: "m/z".into(),
        first: lo,
        last: hi,
    };
    draw_x_labels(&mut canvas, &x_axis, axis_scale, strip_h);
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
    Ok(canvas)
}
