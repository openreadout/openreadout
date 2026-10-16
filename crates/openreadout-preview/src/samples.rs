//! Previews of sample tables (NanoDrop, Qubit): a biologist checks the concentration and purity
//! of each sample. With spectra (NanoDrop Eight, the One's UV-Vis export), the absorbance
//! spectra overlaid with the 230, 260 and 280 nm wavelengths marked, beside a table of each
//! sample's concentration and ratios; without, the concentration of each sample as a bar with
//! its ratios. Ratios outside the usual range of clean nucleic acid are drawn in the accent
//! colour: A260/A280 below 1.7, A260/A230 below 1.8.

use openreadout_core::model::{FileInfo, TableInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, AxesSpec, Baseline, Color, Domain, Face, Figure, GridLines, Panel, Rect, ScaleKind,
    Stroke, TextStyle, Theme, format_value, text_width,
};

use crate::{Frame, FramePanel, PreviewOutput, PreviewRequest};

/// Most samples listed and drawn in colour; the rest are drawn grey and counted.
const MAX_LISTED: usize = 24;
/// Purity limits: below these, a ratio is flagged.
const MIN_260_280: f64 = 1.7;
const MIN_260_230: f64 = 1.8;

/// Does this request draw a sample-table preview?
pub(crate) fn applies(info: &FileInfo, req: &PreviewRequest) -> bool {
    req.trace.is_none()
        && req.sweep.is_none()
        && req.channels.is_empty()
        && req.table.is_none_or(|t| t == 0)
        && req.columns.is_empty()
        && samples_table(info).is_some()
}

fn samples_table(info: &FileInfo) -> Option<&TableInfo> {
    info.tables
        .first()
        .filter(|t| t.extra.get("kind").and_then(|v| v.as_str()) == Some("samples"))
}

/// One sample: name, concentration, ratios.
struct Row {
    name: String,
    conc: f64,
    /// Text the instrument wrote in place of the concentration (`Out of range`).
    note: Option<String>,
    r280: f64,
    r230: f64,
}

pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    let t = samples_table(info).ok_or_else(|| Error::Other("no samples table".into()))?;
    let data = ds.read_table(0, 0, t.row_count)?;
    let col = |name: &str| t.columns.iter().position(|c| c.name == name);
    let num = |k: Option<usize>, i: usize| {
        k.and_then(|k| data.columns.get(k))
            .and_then(|v| v.get(i))
            .copied()
            .unwrap_or(f64::NAN)
    };
    let text_of = |k: Option<usize>, i: usize| -> Option<String> {
        let k = k?;
        let cats = t.columns[k].extra.get("categories")?.as_array()?;
        let code = data.columns.get(k)?.get(i)?;
        cats.get(*code as usize)?.as_str().map(str::to_string)
    };
    let conc_k = col("concentration")
        .or_else(|| col("sample_concentration"))
        .or_else(|| col("tube_concentration"));
    let unit = conc_k
        .and_then(|k| t.columns[k].unit.clone())
        .unwrap_or_default();
    let (name_k, well_k) = (col("sample"), col("well"));
    let note_k = conc_k.and_then(|k| col(&format!("{}_note", t.columns[k].name)));
    let rows: Vec<Row> = (0..t.row_count as usize)
        .map(|i| {
            let mut name = text_of(name_k, i)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("{}", i + 1));
            if let Some(w) = text_of(well_k, i).filter(|s| !s.is_empty()) {
                name = format!("{w} {name}");
            }
            Row {
                name,
                conc: num(conc_k, i),
                note: text_of(note_k, i).filter(|s| !s.is_empty()),
                r280: num(col("a260_a280"), i),
                r230: num(col("a260_a230"), i),
            }
        })
        .collect();
    let theme = crate::theme(req);
    let spectra = info
        .traces
        .first()
        .filter(|tr| tr.extra.get("y_quantity").and_then(|v| v.as_str()) == Some("absorbance"));
    let (fig, frame) = match spectra {
        Some(tr) => spectra_figure(ds, info, tr, &rows, &unit, req, &theme, out)?,
        None => bars_figure(info, &rows, &unit, req, &theme),
    };
    out.notes.push(format!(
        "{} sample(s){}; ratios below {MIN_260_280} (A260/A280) or {MIN_260_230} (A260/A230) are flagged in the accent colour",
        rows.len(),
        if rows.len() > MAX_LISTED {
            format!(", the first {MAX_LISTED} listed (table 0 has all)")
        } else {
            String::new()
        }
    ));
    Ok((fig, frame))
}

/// `s` cut with an ellipsis to fit `w` pixels at size `fs`.
fn fit(s: &str, fs: f32, w: f32) -> String {
    if text_width(s, Face::Sans, fs) <= w {
        return s.to_string();
    }
    let mut chars: Vec<char> = s.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let t: String = chars.iter().collect::<String>() + "…";
        if text_width(&t, Face::Sans, fs) <= w {
            return t;
        }
    }
    "…".into()
}

fn fmt(v: f64, digits: usize) -> String {
    if v.is_finite() {
        format!("{v:.digits$}")
    } else {
        "–".into()
    }
}

/// The colour of sample `k` of `n` (grey past the listed ones).
fn sample_color(theme: &Theme, k: usize) -> Color {
    if k < MAX_LISTED {
        theme.series[k % theme.series.len().max(1)]
    } else {
        theme.faint
    }
}

/// The per-sample table at `(x, y)`: name, concentration, A260/A280, A260/A230. Returns its
/// height.
#[allow(clippy::too_many_arguments)]
fn sample_table(
    fig: &mut Figure,
    theme: &Theme,
    x: f32,
    y: f32,
    w: f32,
    rows: &[Row],
    unit: &str,
    swatch: bool,
) -> f32 {
    let fs = theme.small_size();
    let line = fs * 1.55;
    let head = TextStyle::new(fs, theme.muted).align(Anchor::End, Baseline::Alphabetic);
    let cols = [w * 0.58, w * 0.79, w];
    let has_230 = rows.iter().any(|r| r.r230.is_finite());
    let has_280 = rows.iter().any(|r| r.r280.is_finite());
    let conc_head = if unit.is_empty() {
        "conc.".to_string()
    } else {
        unit.to_string()
    };
    let has_conc = rows.iter().any(|r| r.conc.is_finite());
    if has_conc {
        fig.text(x + cols[0], y, &conc_head, &head);
    }
    if has_280 {
        fig.text(x + cols[1], y, "260/280", &head);
    }
    if has_230 {
        fig.text(x + cols[2], y, "260/230", &head);
    }
    let mut cy = y + line;
    let name_w = cols[0] - text_width("00000.0", Face::Mono, fs) - fs;
    for (k, r) in rows.iter().take(MAX_LISTED).enumerate() {
        let mut nx = x;
        if swatch {
            let c = sample_color(theme, k);
            fig.stroke(
                openreadout_figure::Path::segment(x, cy - fs * 0.35, x + fs * 0.9, cy - fs * 0.35),
                Stroke::new(c, theme.line_width * 1.4),
            );
            nx += fs * 1.3;
        }
        let name = fit(&r.name, fs, name_w - (nx - x));
        fig.text(
            nx,
            cy,
            &name,
            &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
        );
        let num = |v: f64, d: usize, flag: bool| {
            (
                fmt(v, d),
                TextStyle::new(fs, if flag { theme.accent } else { theme.ink })
                    .face(Face::Mono)
                    .align(Anchor::End, Baseline::Alphabetic),
            )
        };
        let digits = if r.conc.abs() >= 100.0 { 1 } else { 2 };
        if has_conc {
            let (s, st) = num(r.conc, digits, false);
            fig.text(x + cols[0], cy, &s, &st);
        }
        if has_280 {
            let (s, st) = num(r.r280, 2, r.r280.is_finite() && r.r280 < MIN_260_280);
            fig.text(x + cols[1], cy, &s, &st);
        }
        if has_230 {
            let (s, st) = num(r.r230, 2, r.r230.is_finite() && r.r230 < MIN_260_230);
            fig.text(x + cols[2], cy, &s, &st);
        }
        cy += line;
    }
    if rows.len() > MAX_LISTED {
        fig.text(
            x,
            cy,
            &format!("+ {} more samples", rows.len() - MAX_LISTED),
            &TextStyle::new(fs, theme.muted).align(Anchor::Start, Baseline::Alphabetic),
        );
        cy += line;
    }
    cy - y
}

#[allow(clippy::too_many_arguments)]
fn spectra_figure(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    tr: &openreadout_core::model::TraceInfo,
    rows: &[Row],
    unit: &str,
    req: &PreviewRequest,
    theme: &Theme,
    out: &mut PreviewOutput,
) -> Result<(Figure, Frame)> {
    let axis = tr.extra.get("axis");
    let first = axis
        .and_then(|a| a.get("first"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let step = axis
        .and_then(|a| a.get("step"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    let last = first + step * tr.sample_count.saturating_sub(1) as f64;
    // the nucleic-acid window when a nucleic-acid export's spectra reach past it (the UV-Vis
    // spectrum export shows its whole range)
    let uv_vis = info
        .tables
        .first()
        .and_then(|t| t.extra.get("layout"))
        .and_then(|v| v.as_str())
        == Some("nanodrop-spectra");
    let (x0, x1) = if !uv_vis && first <= 220.0 && last >= 340.0 {
        (220.0, 340.0)
    } else {
        (first, last)
    };
    let mut lines: Vec<Vec<(f64, f64)>> = Vec::new();
    for s in 0..tr.sweep_count {
        let t = ds.read_trace(tr.index, s, 0, tr.sample_count)?;
        let v = t.channels.first().cloned().unwrap_or_default();
        lines.push(
            v.iter()
                .enumerate()
                .map(|(i, y)| (first + step * i as f64, *y))
                .filter(|(x, y)| *x >= x0 - 1e-9 && *x <= x1 + 1e-9 && y.is_finite())
                .collect(),
        );
    }
    let fs = theme.font_size;
    let w = req.max_size.max(320) as f32;
    let header = fs * 2.2;
    let table_w = (w * 0.34).clamp(fs * 16.0, fs * 26.0);
    let shown = rows.len().min(MAX_LISTED) + usize::from(rows.len() > MAX_LISTED);
    let table_h = theme.small_size() * 1.55 * (shown as f32 + 1.5);
    let panel_h = (w * 0.5).max(table_h);
    let height = (header + panel_h + fs * 3.5).min(req.max_size as f32 * 1.4);
    let mut fig = Figure::new(w, height, Some(theme.paper));
    let title = format!(
        "{} · {} sample{}",
        instrument(info),
        rows.len(),
        if rows.len() == 1 { "" } else { "s" }
    );
    fig.text(
        fs * 0.8,
        fs * 1.4,
        &title,
        &TextStyle::new(fs, theme.ink)
            .face(Face::SansMedium)
            .align(Anchor::Start, Baseline::Alphabetic),
    );
    let all = lines.iter().flatten().map(|p| p.1);
    let y_dom = Domain::of(all, 0.06, ScaleKind::Linear).include(0.0);
    let x_dom = Domain::linear(x0, x1);
    let y_label = tr
        .channels
        .first()
        .and_then(|c| c.extra.get("label"))
        .and_then(|v| v.as_str())
        .unwrap_or("absorbance")
        .to_string();
    let spec = AxesSpec {
        x_label: Some("wavelength (nm)".into()),
        y_label: Some(y_label),
        grid: GridLines::Y,
        ..AxesSpec::default()
    };
    let outer = Rect::new(0.0, header, w - table_w - fs * 1.5, height - header);
    let panel = Panel::fit(
        outer.inset_sides(fs * 0.8, 0.0, fs * 0.6, fs * 0.4),
        x_dom,
        y_dom,
        theme,
        &spec,
    );
    panel.axes(&mut fig, theme, &spec);
    // the wavelengths are labelled when 20 nm leave room for a label
    let gap = panel
        .pt(260.0, 0.0)
        .zip(panel.pt(280.0, 0.0))
        .map_or(0.0, |(a, b)| b.0 - a.0);
    let label = gap > theme.small_size() * 2.2;
    for wl in [230.0, 260.0, 280.0] {
        if wl >= x0 && wl <= x1 {
            panel.rule_x(
                &mut fig,
                wl,
                Stroke::new(theme.rule, theme.hairline()).dashed(3.0, 3.0),
            );
        }
        if label && wl >= x0 && wl <= x1 {
            panel.text(
                &mut fig,
                wl,
                y_dom.lo,
                &format!("{wl:.0}"),
                &TextStyle::new(theme.small_size(), theme.muted)
                    .align(Anchor::Start, Baseline::Bottom),
            );
        }
    }
    // grey ones first, coloured on top
    for k in (0..lines.len()).rev() {
        let c = sample_color(theme, k);
        panel.line(
            &mut fig,
            lines[k].iter().copied(),
            Stroke::new(c, theme.line_width * 0.9).round(),
        );
    }
    sample_table(
        &mut fig,
        theme,
        w - table_w - fs * 0.6,
        header + fs * 1.2,
        table_w,
        rows,
        unit,
        true,
    );
    out.notes.push(format!(
        "spectra of every sample from {x0:.0} to {x1:.0} nm (trace 0, one sweep per sample; preview trace=0 sweep=N draws one alone)"
    ));
    Ok((fig, Frame::plots(vec![FramePanel::new(panel)])))
}

fn bars_figure(
    info: &FileInfo,
    rows: &[Row],
    unit: &str,
    req: &PreviewRequest,
    theme: &Theme,
) -> (Figure, Frame) {
    let fs = theme.font_size;
    let w = req.max_size.max(320) as f32;
    let header = fs * 2.2;
    let shown: Vec<&Row> = rows.iter().take(MAX_LISTED).collect();
    let bar_h = fs * 1.5;
    let panel_h = bar_h * shown.len().max(1) as f32;
    let height = header + fs * 1.6 + panel_h + fs * 3.6;
    let mut fig = Figure::new(w, height, Some(theme.paper));
    let title = format!(
        "{} · {} sample{}",
        instrument(info),
        rows.len(),
        if rows.len() == 1 { "" } else { "s" }
    );
    fig.text(
        fs * 0.8,
        fs * 1.4,
        &title,
        &TextStyle::new(fs, theme.ink)
            .face(Face::SansMedium)
            .align(Anchor::Start, Baseline::Alphabetic),
    );
    let has_280 = rows.iter().any(|r| r.r280.is_finite());
    let has_230 = rows.iter().any(|r| r.r230.is_finite());
    let ratio_w = fs * 4.6 * (f32::from(u8::from(has_280)) + f32::from(u8::from(has_230)));
    let name_w = shown
        .iter()
        .map(|r| text_width(&r.name, Face::Sans, theme.small_size()))
        .fold(fs * 3.0, f32::max)
        .min(w * 0.3);
    let hi = shown
        .iter()
        .map(|r| r.conc)
        .filter(|v| v.is_finite())
        .fold(0.0, f64::max);
    let x_dom = Domain::linear(0.0, if hi > 0.0 { hi } else { 1.0 }).nice(5);
    let n = shown.len().max(1) as f64;
    let y_dom = Domain::linear(0.0, n).reversed();
    let spec = AxesSpec {
        x_label: Some(if unit.is_empty() {
            "concentration".into()
        } else {
            format!("concentration ({unit})")
        }),
        show_y: false,
        grid: GridLines::X,
        ..AxesSpec::default()
    };
    let rect = Rect::new(
        fs * 0.8 + name_w + fs * 0.8,
        header + fs * 1.6,
        w - (fs * 0.8 + name_w + fs * 0.8) - ratio_w - fs * 1.2,
        panel_h,
    );
    let panel = Panel::new(rect, x_dom, y_dom);
    panel.axes(&mut fig, theme, &spec);
    let small = theme.small_size();
    let head = TextStyle::new(small, theme.muted).align(Anchor::End, Baseline::Alphabetic);
    let rx = rect.right() + fs * 0.8;
    let mut cols = Vec::new();
    if has_280 {
        cols.push(("260/280", 0usize));
    }
    if has_230 {
        cols.push(("260/230", 1usize));
    }
    for (j, (h, _)) in cols.iter().enumerate() {
        fig.text(
            rx + fs * 4.6 * (j as f32 + 1.0),
            header + fs * 1.1,
            h,
            &head,
        );
    }
    for (k, r) in shown.iter().enumerate() {
        let yc = k as f64 + 0.5;
        if r.conc.is_finite() {
            panel.rect_data(
                &mut fig,
                0.0,
                k as f64 + 0.18,
                r.conc.max(0.0),
                k as f64 + 0.82,
                Some(theme.ink.mix(theme.paper, 0.35)),
                None,
            );
        }
        let Some((_, py)) = panel.pt(0.0, yc) else {
            continue;
        };
        fig.text(
            rect.x - fs * 0.6,
            py,
            &r.name,
            &TextStyle::new(small, theme.ink).align(Anchor::End, Baseline::Middle),
        );
        if let Some(note) = r.note.as_ref().filter(|_| !r.conc.is_finite()) {
            fig.text(
                rect.x + fs * 0.3,
                py,
                note,
                &TextStyle::new(small, theme.accent).align(Anchor::Start, Baseline::Middle),
            );
        }
        if r.conc.is_finite()
            && let Some((px, _)) = panel.pt(r.conc.max(0.0), yc)
        {
            let label = format_value(r.conc);
            let inside = px - rect.x > text_width(&label, Face::Mono, small) + fs;
            let (tx, anchor, color) = if inside {
                (px - fs * 0.3, Anchor::End, theme.paper)
            } else {
                (px + fs * 0.3, Anchor::Start, theme.ink)
            };
            fig.text(
                tx,
                py,
                &label,
                &TextStyle::new(small, color)
                    .face(Face::Mono)
                    .align(anchor, Baseline::Middle),
            );
        }
        for (j, (_, which)) in cols.iter().enumerate() {
            let (v, lim) = if *which == 0 {
                (r.r280, MIN_260_280)
            } else {
                (r.r230, MIN_260_230)
            };
            let flag = v.is_finite() && v < lim;
            fig.text(
                rx + fs * 4.6 * (j as f32 + 1.0),
                py,
                &fmt(v, 2),
                &TextStyle::new(small, if flag { theme.accent } else { theme.ink })
                    .face(Face::Mono)
                    .align(Anchor::End, Baseline::Middle),
            );
        }
    }
    if rows.len() > MAX_LISTED {
        fig.text(
            fs * 0.8,
            height - fs * 0.8,
            &format!(
                "+ {} more samples (table 0 has all)",
                rows.len() - MAX_LISTED
            ),
            &TextStyle::new(small, theme.muted).align(Anchor::Start, Baseline::Alphabetic),
        );
    }
    (fig, Frame::plots(vec![FramePanel::new(panel)]))
}

/// `NanoDrop Eight`, `Qubit Flex`, or the format name.
fn instrument(info: &FileInfo) -> String {
    let layout = info
        .tables
        .first()
        .and_then(|t| t.extra.get("layout"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    match layout {
        "nanodrop-8000" => "NanoDrop 8000",
        "nanodrop-eight" => "NanoDrop Eight",
        "nanodrop-one" | "nanodrop-spectra" => "NanoDrop One",
        "qubit-4" => "Qubit 4",
        "qubit-flex" => "Qubit Flex",
        _ => return info.format.name.clone(),
    }
    .to_string()
}
