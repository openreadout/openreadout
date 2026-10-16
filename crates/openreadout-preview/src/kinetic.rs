//! Kinetic plate previews: one small growth curve per well, in plate positions (row letters,
//! column numbers), all on the same time and value axes, coloured by the plate layout's groups
//! (a SoftMax Pro group, a sample name) with a direct-label key and blank wells faint. A heat
//! map of the last time point would hide a well that crashed and regrew; the curves show it.

use std::collections::BTreeMap;

use openreadout_core::model::{FileInfo, TableInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use openreadout_figure::{
    Anchor, Baseline, Color, Domain, Envelope, Face, Figure, Layer, Panel, Path, Rect, ScaleKind,
    Stroke, TextStyle, category_colors, format_tick, format_value, is_blank_label, key_row,
    text_width,
};

use crate::annotate::row_name;
use crate::plate::parse_well;
use crate::{Frame, FramePanel, PlateGroup, PlatePreview, PreviewOutput, PreviewRequest};

/// Most table rows read.
const MAX_ROWS: u64 = 2_000_000;
/// Most groups coloured apart.
const MAX_GROUPS: usize = 8;
/// Longest group name drawn in full.
const MAX_GROUP_CHARS: usize = 18;

fn col_index(t: &TableInfo, names: &[&str]) -> Option<usize> {
    t.columns
        .iter()
        .position(|c| names.iter().any(|n| c.name.trim().eq_ignore_ascii_case(n)))
}

/// Well series of a kinetic table: `(row, col) → [(time_s, value)]`, plus the value name.
struct Series {
    wells: BTreeMap<(u32, u32), Vec<(f64, f64)>>,
    value: String,
    /// The wavelength read, when the table has one.
    wavelength_nm: Option<f64>,
    notes: Vec<String>,
}

fn distinct(v: &[f64]) -> usize {
    let mut b: Vec<u64> = v
        .iter()
        .filter(|x| x.is_finite())
        .map(|x| x.to_bits())
        .collect();
    b.sort_unstable();
    b.dedup();
    b.len()
}

/// Read the per-well time series when `t` is a kinetic plate table; `None` otherwise.
fn read_series(
    ds: &mut dyn Dataset,
    t: &TableInfo,
    value_col: Option<&str>,
) -> Result<Option<Series>> {
    let time_c = col_index(t, &["time_s", "time"]);
    let wide: Vec<(usize, (u32, u32))> = t
        .columns
        .iter()
        .enumerate()
        .filter_map(|(i, c)| parse_well(&c.name).map(|w| (i, w)))
        .collect();
    let row_c = col_index(t, &["row"]);
    let colc = col_index(t, &["column", "col"]);
    let well_c = col_index(t, &["well"]);
    let long = (row_c.is_some() && colc.is_some()) || well_c.is_some();
    let Some(time_c) = time_c else {
        return Ok(None);
    };
    if t.row_count < 2 || (!long && wide.len() < 2) {
        return Ok(None);
    }
    let rows = t.row_count.min(MAX_ROWS);
    let tab = ds.read_table(t.index, 0, rows)?;
    let get = |i: usize| tab.columns.get(i).cloned().unwrap_or_default();
    let times = get(time_c);
    if distinct(&times) < 3 {
        return Ok(None);
    }
    let mut notes = Vec::new();
    if rows < t.row_count {
        notes.push(format!("read the first {rows} of {} rows", t.row_count));
    }
    let mut wells: BTreeMap<(u32, u32), Vec<(f64, f64)>> = BTreeMap::new();
    let value;
    let mut wavelength_nm = None;
    if long {
        let skip = [
            well_c,
            row_c,
            colc,
            Some(time_c),
            col_index(t, &["read"]),
            col_index(t, &["wavelength_nm"]),
        ];
        let vi = match value_col {
            Some(name) => t
                .columns
                .iter()
                .position(|c| c.name == name)
                .ok_or_else(|| {
                    Error::Usage(format!(
                        "column '{name}' not in table {} ({})",
                        t.index,
                        t.columns
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })?,
            None => col_index(t, &["value"])
                .or_else(|| (0..t.columns.len()).find(|i| !skip.contains(&Some(*i))))
                .ok_or_else(|| Error::Usage(format!("table {} has no value column", t.index)))?,
        };
        value = t.columns[vi].name.clone();
        let vals = get(vi);
        // the first read at its first wavelength
        let first_of = |c: Option<usize>| -> Option<(Vec<f64>, f64)> {
            let v = get(c?);
            let m = v
                .iter()
                .copied()
                .filter(|x| x.is_finite())
                .fold(f64::INFINITY, f64::min);
            (m.is_finite() && distinct(&v) > 1).then_some((v, m))
        };
        let read = first_of(col_index(t, &["read"]));
        let wl = first_of(col_index(t, &["wavelength_nm"]));
        if let Some((_, m)) = &read {
            notes.push(format!("showing read {m}"));
        }
        if let Some((_, m)) = &wl {
            notes.push(format!("showing wavelength {m} nm"));
        }
        wavelength_nm = match &wl {
            Some((_, m)) => Some(*m),
            None => col_index(t, &["wavelength_nm"])
                .and_then(|c| get(c).into_iter().find(|x| x.is_finite())),
        };
        let pos: Vec<Option<(u32, u32)>> = if let (Some(rc), Some(cc)) = (row_c, colc) {
            let (r, c) = (get(rc), get(cc));
            let rb = if r.contains(&0.0) { 0.0 } else { 1.0 };
            let cb = if c.contains(&0.0) { 0.0 } else { 1.0 };
            r.iter()
                .zip(&c)
                .map(|(&r, &c)| {
                    let (r, c) = (r - rb, c - cb);
                    (r >= 0.0
                        && c >= 0.0
                        && r.fract() == 0.0
                        && c.fract() == 0.0
                        && r < 64.0
                        && c < 64.0)
                        .then_some((r as u32, c as u32))
                })
                .collect()
        } else {
            let w = get(well_c.unwrap_or(0));
            let base = if w.contains(&0.0) { 0.0 } else { 1.0 };
            let maxw = w
                .iter()
                .copied()
                .filter(|x| x.is_finite())
                .fold(0.0, f64::max)
                - base;
            let per_row = if maxw >= 96.0 { 24.0 } else { 12.0 };
            w.iter()
                .map(|&x| {
                    let k = x - base;
                    (x.is_finite() && k >= 0.0 && k.fract() == 0.0 && k < per_row * 32.0)
                        .then(|| ((k / per_row).floor() as u32, (k % per_row) as u32))
                })
                .collect()
        };
        for i in 0..vals.len() {
            let keep = |c: &Option<(Vec<f64>, f64)>| {
                c.as_ref()
                    .is_none_or(|(v, m)| v.get(i).copied().map(f64::to_bits) == Some(m.to_bits()))
            };
            if !keep(&read) || !keep(&wl) {
                continue;
            }
            if let (Some(Some(p)), Some(&tm), Some(&v)) = (pos.get(i), times.get(i), vals.get(i))
                && tm.is_finite()
            {
                wells.entry(*p).or_default().push((tm, v));
            }
        }
    } else {
        value = "value".into();
        for (i, w) in &wide {
            let v = get(*i);
            let s = wells.entry(*w).or_default();
            for (tm, x) in times.iter().zip(&v) {
                if tm.is_finite() {
                    s.push((*tm, *x));
                }
            }
        }
    }
    for s in wells.values_mut() {
        s.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    if wells.is_empty() {
        return Ok(None);
    }
    Ok(Some(Series {
        wells,
        value,
        wavelength_nm,
        notes,
    }))
}

/// The layout field to colour by: 2–8 groups (blanks aside) covering the most wells; a field
/// named like a group wins ties.
/// A layout field and its label per well.
type Grouping = (String, BTreeMap<(u32, u32), String>);

fn choose_grouping(t: &TableInfo) -> Option<Grouping> {
    let layout = t.extra.get("layout")?.as_object()?;
    let mut best: Option<((usize, bool), Grouping)> = None;
    for (key, map) in layout {
        let Some(map) = map.as_object() else { continue };
        let mut wells = BTreeMap::new();
        for (w, v) in map {
            if let (Some(p), Some(v)) = (
                parse_well(w),
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_f64().map(format_value)),
            ) {
                wells.insert(p, v);
            }
        }
        let mut names: Vec<&String> = wells.values().filter(|v| !is_blank_label(v)).collect();
        names.sort();
        names.dedup();
        if !(2..=MAX_GROUPS).contains(&names.len()) {
            continue;
        }
        let groupish = key.to_ascii_lowercase().contains("group");
        let score = (wells.len(), groupish);
        if best.as_ref().is_none_or(|b| score > b.0) {
            best = Some((score, (key.clone(), wells)));
        }
    }
    best.map(|(_, g)| g)
}

/// Wells the layout marks blank: a `Role` (or similar) of blank, or a blank-like group name.
fn blank_wells(t: &TableInfo) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    if let Some(layout) = t.extra.get("layout").and_then(|l| l.as_object()) {
        for map in layout.values().filter_map(|m| m.as_object()) {
            for (w, v) in map {
                if let (Some(p), Some(v)) = (parse_well(w), v.as_str())
                    && is_blank_label(v)
                {
                    out.push(p);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn short(s: &str) -> String {
    if s.chars().count() <= MAX_GROUP_CHARS {
        s.to_string()
    } else {
        let mut o: String = s.chars().take(MAX_GROUP_CHARS - 1).collect();
        o.push('…');
        o
    }
}

/// Draw a kinetic plate as small multiples; `None` when the table is not a kinetic plate read.
pub(crate) fn render(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &PreviewRequest,
    out: &mut PreviewOutput,
) -> Result<Option<(Figure, Frame)>> {
    let ti = req.table.unwrap_or(0);
    let Some(t) = info.tables.iter().find(|t| t.index == ti) else {
        return Ok(None);
    };
    let Some(series) = read_series(ds, t, req.columns.first().map(String::as_str))? else {
        return Ok(None);
    };
    out.notes.extend(series.notes.iter().cloned());
    let declared = |k: &str| {
        t.extra
            .get(k)
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| (1..=64).contains(v))
    };
    let wells = &series.wells;
    let fits = |r: u32, c: u32| wells.keys().all(|(wr, wc)| *wr < r && *wc < c);
    let (nr, nc) = match (declared("plate_rows"), declared("plate_columns")) {
        (Some(r), Some(c)) if fits(r, c) => (r, c),
        _ if fits(8, 12) => (8, 12),
        _ if fits(16, 24) => (16, 24),
        _ => (32, 48),
    };
    let blanks = blank_wells(t);
    let grouping = choose_grouping(t);
    let theme = crate::theme(req);
    let fs = theme.font_size;

    // time axis and its unit; values shared by every card
    let (t0, t1) = wells
        .values()
        .flat_map(|s| s.iter().map(|p| p.0))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
            (a.min(x), b.max(x))
        });
    let span = t1 - t0;
    let (unit, div) = if span > 2.0 * 3600.0 {
        ("h", 3600.0)
    } else if span > 120.0 {
        ("min", 60.0)
    } else {
        ("s", 1.0)
    };
    let counted = |p: &(u32, u32)| !blanks.contains(p);
    let (vlo, vhi) = wells
        .iter()
        .filter(|(p, _)| counted(p) || blanks.len() == wells.len())
        .flat_map(|(_, s)| s.iter().map(|p| p.1))
        .filter(|v| v.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
            (a.min(x), b.max(x))
        });
    let y_dom = Domain::of(
        [vlo.min(0.0).max(vlo - (vhi - vlo) * 0.02), vhi],
        0.04,
        ScaleKind::Linear,
    );
    let x_dom = Domain::linear(t0 / div, t1 / div);

    // groups: colours in order of first appearance, row by row
    let mut group_names: Vec<String> = Vec::new();
    if let Some((_, g)) = &grouping {
        for r in 0..nr {
            for c in 0..nc {
                if let Some(name) = g.get(&(r, c))
                    && !is_blank_label(name)
                    && wells.contains_key(&(r, c))
                    && !group_names.contains(name)
                {
                    group_names.push(name.clone());
                }
            }
        }
    }
    let color_of_group: BTreeMap<String, Color> = group_names
        .iter()
        .cloned()
        .zip(category_colors(&theme, &group_names))
        .collect();
    let well_color = |p: (u32, u32)| -> Color {
        if blanks.contains(&p) {
            return theme.faint;
        }
        grouping
            .as_ref()
            .and_then(|(_, g)| g.get(&p))
            .and_then(|n| color_of_group.get(n))
            .copied()
            .unwrap_or(theme.ink)
    };
    // a row belongs to one group when all its non-blank wells do: label row blocks with it
    let row_group: Vec<Option<String>> = (0..nr)
        .map(|r| {
            let (_, g) = grouping.as_ref()?;
            let names: Vec<&String> = (0..nc)
                .filter(|c| !blanks.contains(&(r, *c)) && wells.contains_key(&(r, *c)))
                .filter_map(|c| g.get(&(r, c)))
                .collect();
            let first = names.first()?;
            names.iter().all(|n| n == first).then(|| (*first).clone())
        })
        .collect();
    let blocks = row_group.iter().all(Option::is_some) && !group_names.is_empty();

    // layout
    let w = req.max_size.max(64) as f32;
    let pad = fs * 0.8;
    let block_label_w = if blocks {
        group_names
            .iter()
            .map(|n| text_width(&short(n), Face::SansMedium, fs * 1.05))
            .fold(0.0f32, f32::max)
            + fs * 0.9
    } else {
        0.0
    };
    let row_lab_w = text_width("WW", Face::Sans, fs * 0.9) + fs * 0.3;
    let header = fs * 2.0
        + if group_names.is_empty() {
            0.0
        } else {
            fs * 1.6
        };
    let col_lab_h = fs * 1.5;
    let gap = (w / nc as f32 * 0.07).clamp(2.0, 8.0);
    let block_gap = if blocks { fs * 0.7 } else { 0.0 };
    let n_block_breaks = if blocks {
        row_group.windows(2).filter(|p| p[0] != p[1]).count() as f32
    } else {
        0.0
    };
    let grid_x = pad + block_label_w + row_lab_w;
    let card_w = ((w - grid_x - pad - gap * (nc as f32 - 1.0)) / nc as f32).max(6.0);
    let footer = fs * 2.4;
    let max_card_h = (req.max_size as f32
        - header
        - col_lab_h
        - footer
        - n_block_breaks * block_gap
        - gap * (nr as f32 - 1.0))
        / nr as f32;
    let card_h = (card_w * 0.72).min(max_card_h).max(6.0);
    let grid_y = header + col_lab_h;
    let height =
        grid_y + nr as f32 * card_h + (nr as f32 - 1.0) * gap + n_block_breaks * block_gap + footer;
    let mut fig = Figure::new(w, height.ceil(), Some(theme.paper));

    // title and key
    // "Absorbance at 600 nm, 19 h, one card per well"
    let mode = t
        .extra
        .get("read_modes")
        .and_then(|m| m.get(0).or(Some(m)))
        .and_then(serde_json::Value::as_str)
        .map(|m| {
            let mut c = m.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase().collect::<String>() + c.as_str()
            })
        });
    // an image-analysis metric (Incucyte) is named by the read itself: `Phase Object Confluence`
    let mode = match mode {
        Some(m) if m == "Imaging" => t.name.clone().or(Some(m)),
        m => m,
    };
    let measure = match (mode, series.wavelength_nm) {
        (Some(m), Some(wl)) => format!("{m} at {} nm", format_value(wl)),
        (Some(m), None) => m,
        (None, Some(wl)) => format!("{} at {} nm", series.value, format_value(wl)),
        (None, None) => series.value.clone(),
    };
    let title = format!(
        "{measure} over {} {unit}, one card per well",
        format_tick(x_dom.hi - x_dom.lo, (x_dom.hi - x_dom.lo) / 10.0)
    );
    fig.group(Layer::Caption, "caption", |fig| {
        fig.text(
            pad,
            fs * 1.4,
            &title,
            &TextStyle::new(fs, theme.ink).align(Anchor::Start, Baseline::Alphabetic),
        );
    });
    let mut key: Vec<(Color, String)> = group_names
        .iter()
        .map(|n| (color_of_group[n], short(n)))
        .collect();
    if !blanks.is_empty() && !key.is_empty() {
        key.push((theme.faint, "blank".into()));
    }
    let mut slot = crate::KeySlot {
        x: pad,
        y: fs * 2.9,
        max_x: w - pad,
    };
    if !key.is_empty() {
        let used = key_row(&mut fig, &theme, pad, fs * 2.9, w - pad * 2.0, &key, false);
        slot.x = used.right() + fs * 1.2;
    }
    // column numbers
    let label = TextStyle::new(fs * 0.9, theme.muted);
    let every = ((text_width("24", Face::Sans, fs * 0.9) + fs * 0.4) / (card_w + gap))
        .ceil()
        .max(1.0) as u32;
    fig.group(Layer::Axis, "columns", |fig| {
        for c in 0..nc {
            if c == 0 || (c + 1) % every == 0 {
                fig.text(
                    grid_x + c as f32 * (card_w + gap) + card_w / 2.0,
                    grid_y - fs * 0.45,
                    &(c + 1).to_string(),
                    &label.clone().align(Anchor::Middle, Baseline::Alphabetic),
                );
            }
        }
    });
    // cards
    let mut panels = Vec::new();
    let mut y = grid_y;
    let radius = (card_w.min(card_h) * 0.08).clamp(1.5, 6.0);
    let inset = (card_w.min(card_h) * 0.07).clamp(1.5, 5.0);
    let row_every = ((fs * 1.0) / (card_h + gap)).ceil().max(1.0) as u32;
    let mut block_start: Option<(f32, String)> = None;
    for r in 0..nr {
        if r > 0 && blocks && row_group[r as usize] != row_group[r as usize - 1] {
            if let Some((ys, name)) = block_start.take() {
                block_label(
                    &mut fig,
                    &theme,
                    pad,
                    ys,
                    y - gap,
                    &name,
                    color_of_group.get(&name).copied(),
                );
            }
            y += block_gap;
        }
        if blocks && block_start.is_none() {
            block_start = row_group[r as usize].clone().map(|n| (y, n));
        }
        if r % row_every == 0 {
            fig.group(Layer::Axis, &format!("row {}", row_name(r)), |fig| {
                fig.text(
                    grid_x - fs * 0.45,
                    y + card_h / 2.0,
                    &row_name(r),
                    &label.clone().align(Anchor::End, Baseline::Middle),
                );
            });
        }
        for c in 0..nc {
            let x = grid_x + c as f32 * (card_w + gap);
            let card = Rect::new(x, y, card_w, card_h);
            let present = wells.get(&(r, c));
            let fill = if present.is_some() {
                theme.panel
            } else {
                theme.paper
            };
            let inner = card.inset(inset);
            let panel = Panel::new(inner, x_dom, y_dom);
            let well = format!("{}{}", row_name(r), c + 1);
            fig.group(Layer::Panel, &well, |fig| {
                fig.shape(
                    Path::round_rect(card.x, card.y, card.w, card.h, radius),
                    Some(fill),
                    Some(Stroke::new(theme.rule, 1.0)),
                    None,
                );
                if let Some(s) = present {
                    let mut env =
                        Envelope::new((inner.w.max(1.0) * 1.5) as usize, x_dom.lo, x_dom.hi);
                    for (tm, v) in s {
                        env.push(tm / div, *v);
                    }
                    let col = well_color((r, c));
                    let lw = (card_w / 55.0).clamp(0.9, 1.8)
                        * if blanks.contains(&(r, c)) { 0.8 } else { 1.0 };
                    fig.group(Layer::Series, &well, |fig| {
                        panel.line_px(fig, env.points(&panel), Stroke::new(col, lw).round());
                    });
                }
            });
            panels.push(FramePanel {
                well: Some((r, c)),
                ..FramePanel::new(panel)
            });
        }
        y += card_h + gap;
    }
    if let Some((ys, name)) = block_start.take() {
        block_label(
            &mut fig,
            &theme,
            pad,
            ys,
            y - gap,
            &name,
            color_of_group.get(&name).copied(),
        );
    }
    // footer: the shared axes in words
    let ystep = (y_dom.hi - y_dom.lo) / 10.0;
    let axes_note = format!(
        "every card shares its axes: time {}–{} {unit} across, {} {}–{} up",
        format_tick(x_dom.lo, (x_dom.hi - x_dom.lo) / 10.0),
        format_tick(x_dom.hi, (x_dom.hi - x_dom.lo) / 10.0),
        series.value,
        format_tick(y_dom.lo, ystep),
        format_tick(y_dom.hi, ystep),
    );
    fig.group(Layer::Annotation, "scale", |fig| {
        fig.text(
            grid_x,
            y - gap + fs * 1.5,
            &axes_note,
            &TextStyle::new(theme.small_size(), theme.muted)
                .align(Anchor::Start, Baseline::Alphabetic),
        );
    });

    let mut groups: Vec<PlateGroup> = group_names
        .iter()
        .map(|n| PlateGroup {
            name: n.clone(),
            color: color_of_group[n].to_hex(),
            wells: grouping.as_ref().map_or(0, |(_, g)| {
                g.iter()
                    .filter(|(p, v)| *v == n && wells.contains_key(p) && !blanks.contains(p))
                    .count() as u32
            }),
            blank: false,
        })
        .collect();
    if !blanks.is_empty() {
        groups.push(PlateGroup {
            name: "blank".into(),
            color: theme.faint.to_hex(),
            wells: blanks.iter().filter(|p| wells.contains_key(p)).count() as u32,
            blank: true,
        });
    }
    out.notes.push(format!(
        "kinetic read: one curve per well ({} wells, {} time points); every card shares the time and value axes",
        wells.len(),
        wells.values().map(Vec::len).max().unwrap_or(0)
    ));
    out.plate = Some(PlatePreview {
        table: ti,
        layout: if col_index(t, &["well", "row"]).is_some() {
            "long"
        } else {
            "wide"
        }
        .into(),
        view: "curves".into(),
        rows: nr,
        columns: nc,
        wells: wells.len() as u32,
        value: series.value.clone(),
        min: Some(y_dom.lo),
        max: Some(y_dom.hi),
        time: Some([x_dom.lo, x_dom.hi]),
        time_unit: Some(unit.into()),
        grouped_by: grouping.map(|(k, _)| k),
        groups,
    });
    Ok(Some((
        fig,
        Frame::Plots {
            panels,
            key: Some(slot),
            taken: Vec::new(),
        },
    )))
}

/// A group's name beside the rows it fills, vertically centred, in its colour.
fn block_label(
    fig: &mut Figure,
    theme: &openreadout_figure::Theme,
    x: f32,
    y0: f32,
    y1: f32,
    name: &str,
    color: Option<Color>,
) {
    let st = TextStyle::new(theme.font_size * 1.05, color.unwrap_or(theme.ink))
        .face(Face::SansMedium)
        .align(Anchor::Start, Baseline::Middle);
    fig.group(Layer::Annotation, name, |fig| {
        fig.text(x, f32::midpoint(y0, y1), &short(name), &st);
    });
}
