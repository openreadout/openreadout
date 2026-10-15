//! Plate-reader wells: kinetic curves per well or per layout group, and endpoint values.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use openreadout_assay::layout::{Layout, Role, WellInfo, parse_wells, row_label, well_name};
use openreadout_core::Result;

use super::{Ctx, glob, usage};
use crate::model::{Geom, Quantity, Series, Source, Weight};
use crate::spec::{ColorBy, MarkKind, SeriesSpec, Spread, Subtract};

/// Plate facts a plate facet uses: its size and the layout group of each row.
#[derive(Debug, Clone, Default)]
pub(crate) struct PlateHints {
    pub rows: u32,
    pub cols: u32,
    /// The one layout group of each row's sample wells, when a row has one.
    pub row_groups: Vec<Option<String>>,
}

/// The unit a time axis spanning `span_s` seconds is shown in, and its length in seconds.
pub(crate) fn time_unit(span_s: f64) -> (&'static str, f64) {
    if span_s >= 4.0 * 3600.0 {
        ("h", 3600.0)
    } else if span_s >= 300.0 {
        ("min", 60.0)
    } else if span_s >= 2.0 {
        ("s", 1.0)
    } else {
        ("ms", 1e-3)
    }
}

/// Seconds in a named time unit.
pub(crate) fn seconds_in(unit: &str) -> Option<f64> {
    match unit {
        "ms" => Some(1e-3),
        "s" => Some(1.0),
        "min" => Some(60.0),
        "h" => Some(3600.0),
        _ => None,
    }
}

/// The layout group of a well: the export's group, else its sample name, else its role.
fn group_of(w: &WellInfo) -> Option<String> {
    w.group
        .clone()
        .or_else(|| {
            w.extra
                .iter()
                .find(|(k, _)| k.to_ascii_lowercase().contains("group"))
                .map(|(_, v)| v.clone())
        })
        .or_else(|| w.sample.clone())
        .or_else(|| w.compound.clone())
        .or_else(|| w.role_label.clone())
        .filter(|g| !g.trim().is_empty())
}

struct Well {
    pos: (u32, u32),
    group: Option<String>,
    blank: bool,
    /// (time s, value), by time.
    pts: Vec<(f64, f64)>,
}

pub(super) fn wells(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let plate = openreadout_assay::load_plate(ctx.reg, Path::new(file), 0)?;
    if plate.reads.is_empty() {
        return Err(usage(format!("{file}: no plate reads")));
    }
    let read = match spec.read.as_deref() {
        None => plate
            .reads
            .iter()
            .find(|r| !r.calculated)
            .unwrap_or(&plate.reads[0])
            .clone(),
        Some(s) => {
            let by_num = s.trim().parse::<u32>().ok().and_then(|n| plate.reads.iter().find(|r| r.number == n));
            let by_label = plate.reads.iter().find(|r| r.label.eq_ignore_ascii_case(s)).or_else(|| {
                let m: Vec<_> = plate
                    .reads
                    .iter()
                    .filter(|r| r.label.to_lowercase().contains(&s.to_lowercase()))
                    .collect();
                (m.len() == 1).then(|| m[0])
            });
            by_num.or(by_label).cloned().ok_or_else(|| {
                usage(format!(
                    "read `{s}` not found; reads: {}",
                    plate
                        .reads
                        .iter()
                        .map(|r| format!("{} ({})", r.number, r.label))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?
        }
    };
    let layout: Layout = plate.embedded.clone().unwrap_or_default();
    let mut by_well: BTreeMap<(u32, u32), Vec<(f64, f64)>> = BTreeMap::new();
    for o in plate.obs.iter().filter(|o| o.read == read.number) {
        by_well
            .entry((o.row, o.col))
            .or_default()
            .push((o.time_s.unwrap_or(f64::NAN), o.value));
    }
    let kinetic = by_well.values().any(|v| v.len() > 1 && v.iter().all(|p| p.0.is_finite()));
    let mut wells: Vec<Well> = Vec::new();
    for (pos, mut pts) in by_well {
        pts.sort_by(|a, b| a.0.total_cmp(&b.0));
        let info = layout.wells.get(&pos);
        if info.and_then(WellInfo::effective_role) == Some(Role::Empty) {
            continue;
        }
        wells.push(Well {
            pos,
            group: info.and_then(group_of),
            blank: info.and_then(WellInfo::effective_role) == Some(Role::Blank),
            pts,
        });
    }
    let groups: Vec<String> = {
        let mut seen = Vec::new();
        for w in &wells {
            if let Some(g) = &w.group
                && !seen.contains(g)
            {
                seen.push(g.clone());
            }
        }
        seen
    };
    // plate size and row groups for a plate facet
    let rows = plate.rows.max(wells.iter().map(|w| w.pos.0 + 1).max().unwrap_or(0));
    let cols = plate.cols.max(wells.iter().map(|w| w.pos.1 + 1).max().unwrap_or(0));
    let row_groups = (0..rows)
        .map(|r| {
            let gs: BTreeSet<&String> = wells
                .iter()
                .filter(|w| w.pos.0 == r && !w.blank)
                .filter_map(|w| w.group.as_ref())
                .collect();
            (gs.len() == 1).then(|| (*gs.iter().next().unwrap_or(&&String::new())).clone())
        })
        .collect();
    ctx.plate = Some(PlateHints { rows, cols, row_groups });

    // subtraction
    let subtract = spec.subtract.unwrap_or(Subtract::None);
    if subtract == Subtract::Blank {
        let blanks: Vec<Vec<(f64, f64)>> = wells.iter().filter(|w| w.blank).map(|w| w.pts.clone()).collect();
        if blanks.is_empty() {
            return Err(usage(format!(
                "{file}: subtract=blank needs blank wells, and the plate's layout has none (subtract=first takes each well's first reading instead)"
            )));
        }
        let n = blanks.iter().map(Vec::len).max().unwrap_or(0);
        let mean: Vec<f64> = (0..n)
            .map(|k| {
                let v: Vec<f64> = blanks.iter().filter_map(|w| w.get(k)).map(|p| p.1).filter(|v| v.is_finite()).collect();
                if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 }
            })
            .collect();
        for w in &mut wells {
            for (k, p) in w.pts.iter_mut().enumerate() {
                p.1 -= mean.get(k).copied().unwrap_or(f64::NAN);
            }
        }
        ctx.notes.push(format!(
            "subtracted the mean of {} blank well(s) at each reading",
            blanks.len()
        ));
    } else if subtract == Subtract::First {
        for w in &mut wells {
            if let Some(first) = w.pts.iter().map(|p| p.1).find(|v| v.is_finite()) {
                w.pts.iter_mut().for_each(|p| p.1 -= first);
            }
        }
    }

    // selection
    let picked: Option<Vec<(u32, u32)>> = spec
        .wells
        .as_deref()
        .map(parse_wells)
        .transpose()
        .map_err(|m| usage(format!("wells: {m}")))?;
    if !spec.groups.is_empty() {
        for g in &spec.groups {
            if !groups.iter().any(|x| glob(g, x)) {
                return Err(usage(format!(
                    "no layout group matches `{g}`; the plate's groups are: {}",
                    if groups.is_empty() { "none (the file stores no layout)".to_string() } else { groups.join(", ") }
                )));
            }
        }
    }
    let selected = |w: &Well| -> bool {
        if let Some(p) = &picked
            && !p.contains(&w.pos)
        {
            return false;
        }
        if !spec.groups.is_empty() {
            return w.group.as_ref().is_some_and(|g| spec.groups.iter().any(|p| glob(p, g)));
        }
        true
    };
    let filtered = picked.is_some() || !spec.groups.is_empty();
    let by_group = match spec.by.as_deref() {
        None => !kinetic,
        Some("well" | "wells") => false,
        Some("group" | "groups") => true,
        Some(other) => {
            return Err(usage(format!("plate `by` is `well` or `group`, not `{other}`")));
        }
    };
    let wells: Vec<Well> = wells
        .into_iter()
        .filter(|w| selected(w) && !(w.blank && by_group))
        .collect();
    if wells.is_empty() {
        return Err(usage(format!("{file}: no wells selected")));
    }
    let _ = filtered;

    let unit = read.unit.as_deref();
    let yname = {
        let base = match (read.mode.as_deref(), plate_wavelength(&plate, read.number)) {
            (Some(m), Some(wl)) if m.to_lowercase().contains("absorb") => format!("absorbance {wl:.0} nm"),
            (_, Some(wl)) if read.label.trim().is_empty() || read.label.starts_with("Plate") => format!("{wl:.0} nm"),
            _ if read.label.trim().is_empty() => "value".to_string(),
            _ => read.label.clone(),
        };
        match subtract {
            Subtract::None => base,
            Subtract::Blank => format!("{base}, blank-subtracted"),
            Subtract::First => format!("{base}, change from the first reading"),
        }
    };
    let yq = Quantity::new(yname, unit);
    let xq = Quantity::new("time", Some("s"));
    let sel_txt = match (&spec.wells, spec.groups.is_empty()) {
        (Some(w), _) => format!("wells {w}"),
        (None, false) => format!("groups {}", spec.groups.join(", ")),
        (None, true) => "all wells".to_string(),
    };
    let src = |extra: &str| Source::file(file, format!("read {} ({}), {sel_txt}{extra}", read.number, read.label));

    // classes for colouring
    let span = wells
        .iter()
        .flat_map(|w| w.pts.iter().map(|p| p.0))
        .filter(|t| t.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |a, t| (a.0.min(t), a.1.max(t)));
    let shown_unit = ctx
        .x_unit
        .as_deref()
        .and_then(seconds_in)
        .unwrap_or_else(|| time_unit(span.1 - span.0).1);
    let class_of = class_fn(spec, &groups, shown_unit, rows)?;

    let mut out = Vec::new();
    if !by_group && !kinetic {
        // endpoint values: one bar per well
        let cats: Vec<String> = wells.iter().map(|w| well_name(w.pos.0, w.pos.1)).collect();
        let vals: Vec<f64> = wells.iter().map(|w| w.pts.first().map_or(f64::NAN, |p| p.1)).collect();
        let mut s = Series::new(read.label.clone(), Geom::Bars { cats: cats.clone(), vals, err: Vec::new() }, MarkKind::Bars, src(""))
            .axes(Quantity::new("well", None), yq);
        s.x_cats = Some(cats);
        return Ok(vec![s]);
    }
    if !by_group {
        let many = wells.len() > 12;
        for w in &wells {
            let name = well_name(w.pos.0, w.pos.1);
            let mut s = Series::new(name.clone(), Geom::Line(w.pts.clone()), MarkKind::Line, src(""))
                .axes(xq.clone(), yq.clone());
            s.facet = Some(name);
            s.well = Some(w.pos);
            s.class = if w.blank { Some("blank".to_string()) } else { class_of(w) };
            s.weight = if w.blank {
                Weight::Context
            } else if many {
                Weight::Many
            } else {
                Weight::Main
            };
            out.push(s);
        }
        return Ok(out);
    }
    // per group: mean and spread
    let spread = spec.spread.unwrap_or(Spread::Sd);
    let pooled = spec.groups.iter().any(|g| g.contains('*'));
    let key_of = |w: &Well| -> String {
        let g = w.group.clone().unwrap_or_else(|| "unassigned".into());
        if pooled {
            spec.groups
                .iter()
                .find(|p| glob(p, &g))
                .map_or(g, |p| pattern_name(p))
        } else {
            g
        }
    };
    let mut order: Vec<String> = Vec::new();
    for w in &wells {
        let g = key_of(w);
        if !order.contains(&g) {
            order.push(g);
        }
    }
    if kinetic {
        for g in &order {
            let members: Vec<&Well> = wells
                .iter()
                .filter(|w| key_of(w) == *g)
                .collect();
            let n = members.iter().map(|w| w.pts.len()).max().unwrap_or(0);
            let mut band = Vec::with_capacity(n);
            for k in 0..n {
                let pts: Vec<(f64, f64)> = members.iter().filter_map(|w| w.pts.get(k)).copied().collect();
                let ts: Vec<f64> = pts.iter().map(|p| p.0).filter(|v| v.is_finite()).collect();
                let vs: Vec<f64> = pts.iter().map(|p| p.1).filter(|v| v.is_finite()).collect();
                if ts.is_empty() || vs.is_empty() {
                    continue;
                }
                let t = ts.iter().sum::<f64>() / ts.len() as f64;
                let (m, lo, hi) = summarize(&vs, spread);
                band.push((t, m, lo, hi));
            }
            let mut s = Series::new(
                g.clone(),
                Geom::Band(band),
                MarkKind::Line,
                src(&format!(", group {g}: mean{} of {} wells", spread_txt(spread), members.len())),
            )
            .axes(xq.clone(), yq.clone());
            s.facet = Some(g.clone());
            out.push(s);
        }
    } else {
        let mut vals = Vec::new();
        let mut err = Vec::new();
        for g in &order {
            let vs: Vec<f64> = wells
                .iter()
                .filter(|w| key_of(w) == *g)
                .filter_map(|w| w.pts.first().map(|p| p.1))
                .filter(|v| v.is_finite())
                .collect();
            let (m, lo, _) = summarize(&vs, spread);
            vals.push(m);
            err.push(m - lo);
        }
        let mut s = Series::new(
            read.label.clone(),
            Geom::Bars { cats: order.clone(), vals, err },
            MarkKind::Bars,
            src(&format!(", mean{} per group", spread_txt(spread))),
        )
        .axes(Quantity::new("group", None), yq);
        s.x_cats = Some(order);
        out.push(s);
    }
    Ok(out)
}

/// The name of a pooled group: its pattern without the wildcards.
fn pattern_name(p: &str) -> String {
    p.replace('*', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn plate_wavelength(plate: &openreadout_assay::PlateData, read: u32) -> Option<f64> {
    plate
        .obs
        .iter()
        .find(|o| o.read == read)
        .and_then(|o| o.wavelength_nm)
}

fn spread_txt(s: Spread) -> &'static str {
    match s {
        Spread::Sd => " ± SD",
        Spread::Sem => " ± SEM",
        Spread::Range => " and range",
        Spread::None => "",
    }
}

/// Mean and the spread band around it.
pub(crate) fn summarize(v: &[f64], spread: Spread) -> (f64, f64, f64) {
    if v.is_empty() {
        return (f64::NAN, f64::NAN, f64::NAN);
    }
    let n = v.len() as f64;
    let m = v.iter().sum::<f64>() / n;
    let sd = if v.len() > 1 {
        (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (n - 1.0)).sqrt()
    } else {
        0.0
    };
    match spread {
        Spread::Sd => (m, m - sd, m + sd),
        Spread::Sem => (m, m - sd / n.sqrt(), m + sd / n.sqrt()),
        Spread::Range => (
            m,
            v.iter().copied().fold(f64::INFINITY, f64::min),
            v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ),
        Spread::None => (m, m, m),
    }
}

type ClassFn<'a> = Box<dyn Fn(&Well) -> Option<String> + 'a>;

fn class_fn<'a>(spec: &'a SeriesSpec, groups: &'a [String], unit_s: f64, _rows: u32) -> Result<ClassFn<'a>> {
    Ok(match &spec.color_by {
        None => {
            if groups.len() > 1 {
                Box::new(|w: &Well| w.group.clone())
            } else {
                Box::new(|_| None)
            }
        }
        Some(ColorBy::Field(f)) => match f.as_str() {
            "group" | "groups" => Box::new(|w: &Well| w.group.clone()),
            "row" | "rows" => Box::new(|w: &Well| Some(format!("row {}", row_label(w.pos.0)))),
            "column" | "col" | "columns" => Box::new(|w: &Well| Some(format!("column {}", w.pos.1 + 1))),
            "none" => Box::new(|_| None),
            other => {
                return Err(usage(format!(
                    "color_by `{other}`: group, row, column, none, a map of classes to wells, or a rule {{at, below|above, label, else}}"
                )));
            }
        },
        Some(ColorBy::Classes(m)) => {
            let mut sets: Vec<(String, Vec<(u32, u32)>)> = Vec::new();
            for (name, wells) in m {
                let w = parse_wells(wells).map_err(|e| usage(format!("color_by `{name}`: {e}")))?;
                sets.push((name.clone(), w));
            }
            Box::new(move |w: &Well| {
                Some(
                    sets.iter()
                        .find(|(_, ws)| ws.contains(&w.pos))
                        .map_or_else(|| "other".to_string(), |(n, _)| n.clone()),
                )
            })
        }
        Some(ColorBy::Rule(r)) => {
            if r.below.is_none() && r.above.is_none() {
                return Err(usage("color_by rule needs `below` or `above`"));
            }
            let at_s = r.at * unit_s;
            let yes = r.label.clone().unwrap_or_else(|| "flagged".into());
            let no = r.otherwise.clone().unwrap_or_else(|| "other".into());
            let (below, above) = (r.below, r.above);
            Box::new(move |w: &Well| {
                let v = w
                    .pts
                    .iter()
                    .filter(|p| p.0.is_finite() && p.1.is_finite())
                    .min_by(|a, b| (a.0 - at_s).abs().total_cmp(&(b.0 - at_s).abs()))
                    .map(|p| p.1)?;
                let flagged = below.is_some_and(|b| v < b) || above.is_some_and(|a| v > a);
                Some(if flagged { yes.clone() } else { no.clone() })
            })
        }
    })
}
