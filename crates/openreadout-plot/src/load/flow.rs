//! Flow cytometry: events of two parameters (coloured by population) and histograms.

use std::path::{Path, PathBuf};

use openreadout_core::Result;
use openreadout_fcs::analysis::{
    CompensationChoice, TableOptions, TransformChoice, processed_rows,
};
use openreadout_fcs::gating::Prepared;

use super::{Ctx, usage};
use crate::model::{Geom, Quantity, Series, Source, Weight};
use crate::spec::{MarkKind, SeriesSpec};

/// Most events read from one file.
const MAX_EVENTS: u64 = 2_000_000;

/// One parameter's values after the transform, with ticks placed in raw units.
struct Param {
    name: String,
    label: String,
    values: Vec<f64>,
    ticks: Option<Vec<(f64, String)>>,
    transform: String,
}

struct Read {
    names: Vec<String>,
    labels: Vec<Option<String>>,
    columns: Vec<Vec<f64>>,
    total: u64,
    transformed_by_file: bool,
}

fn options(spec: &SeriesSpec) -> Result<TableOptions> {
    let mut opts = TableOptions::default();
    opts.gating_file = spec
        .workspace
        .as_ref()
        .or(spec.gatingml.as_ref())
        .map(PathBuf::from);
    opts.populations = spec.populations.clone();
    if !opts.populations.is_empty() && opts.gating_file.is_none() {
        return Err(usage(
            "populations need the gating: give workspace (FlowJo .wsp) or gatingml",
        ));
    }
    opts.compensation = match spec.compensate.as_deref() {
        None if opts.gating_file.is_some() => Some(CompensationChoice::Auto),
        None => None,
        Some("auto") => Some(CompensationChoice::Auto),
        Some("fcs") => Some(CompensationChoice::File),
        Some("gating" | "workspace" | "gatingml") => Some(CompensationChoice::GatingFile),
        Some("none") => None,
        Some(other) => {
            return Err(usage(format!(
                "compensate `{other}`: auto, fcs, gating or none"
            )));
        }
    };
    if spec.transform.as_deref() == Some("workspace") {
        opts.transform = Some(TransformChoice::GatingFile);
    }
    Ok(opts)
}

fn read(file: &str, spec: &SeriesSpec) -> Result<Read> {
    let opts = options(spec)?;
    let p = processed_rows(Path::new(file), 0, 0, MAX_EVENTS, &opts)?;
    Ok(Read {
        names: p.names,
        labels: p.labels,
        columns: p.columns,
        total: p.total_rows,
        transformed_by_file: spec.transform.as_deref() == Some("workspace"),
    })
}

fn find(r: &Read, want: &str, file: &str) -> Result<usize> {
    let w = want.trim();
    r.names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(w))
        .or_else(|| {
            r.labels
                .iter()
                .position(|l| l.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(w)))
        })
        .or_else(|| {
            // `Comp-FITC-A` and `FITC-A` name the same compensated column
            let bare = w.strip_prefix("Comp-").unwrap_or(w);
            r.names.iter().position(|n| n.eq_ignore_ascii_case(bare))
        })
        .ok_or_else(|| {
            let list: Vec<String> = r
                .names
                .iter()
                .zip(&r.labels)
                .filter(|(n, _)| !n.starts_with("gate:"))
                .map(|(n, l)| match l {
                    Some(l) if l != n => format!("{n} ({l})"),
                    _ => n.clone(),
                })
                .collect();
            usage(format!(
                "{file}: no parameter `{want}`; parameters: {}",
                list.join(", ")
            ))
        })
}

/// Transform a column for display: the asked transform, else logicle for fluorescence and
/// linear for scatter and time.
fn param(r: &Read, i: usize, spec: &SeriesSpec) -> Result<Param> {
    let name = r.names[i].clone();
    let label = r.labels[i]
        .clone()
        .filter(|l| !l.trim().is_empty() && *l != name);
    let display = match &label {
        Some(l) => format!("{name} · {l}"),
        None => name.clone(),
    };
    let raw = &r.columns[i];
    if r.transformed_by_file {
        return Ok(Param {
            name,
            label: display,
            values: raw.clone(),
            ticks: None,
            transform: "workspace".into(),
        });
    }
    let fluor = openreadout_fcs::is_fluorescence_parameter(&name) && !name.starts_with("gate:");
    let spec_t = spec.transform.as_deref().map(str::to_ascii_lowercase);
    let wanted = match spec_t.as_deref() {
        Some(t) => t.to_string(),
        None if fluor => "logicle".into(),
        None => "linear".into(),
    };
    if wanted == "linear" || wanted == "none" {
        return Ok(Param {
            name,
            label: display,
            values: raw.clone(),
            ticks: None,
            transform: "linear".into(),
        });
    }
    // top of scale from the data: the decade above the 99.9th percentile
    let mut sorted: Vec<f64> = raw.iter().copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(f64::total_cmp);
    let top = sorted
        .get(((sorted.len() as f64) * 0.999) as usize)
        .or(sorted.last())
        .copied()
        .unwrap_or(1.0)
        .max(1.0);
    let decades = top.log10().ceil().clamp(3.0, 7.0);
    let t_top = 10f64.powf(decades);
    let spec_str = if wanted.contains(':')
        || !matches!(wanted.as_str(), "logicle" | "arcsinh" | "asinh" | "log")
    {
        wanted.clone()
    } else {
        match wanted.as_str() {
            "logicle" => format!("logicle:T={t_top},W=0.5,M={},A=0", decades + 0.5),
            "log" => format!("log:T={t_top},M={decades}"),
            _ => format!("arcsinh:T={t_top},M={},A=0", decades + 0.5),
        }
    };
    let t = openreadout_fcs::analysis::parse_transform_spec(&spec_str)?;
    let prep: Prepared = t
        .prepare()
        .map_err(|e| usage(format!("transform {spec_str}: {e}")))?;
    let values: Vec<f64> = raw.iter().map(|&v| prep.apply(v)).collect();
    let mut ticks = Vec::new();
    if wanted != "log" {
        ticks.push((prep.apply(0.0), "0".to_string()));
    }
    for d in 1..=(decades as i32) {
        let v = 10f64.powi(d);
        let p = prep.apply(v);
        if p.is_finite() && d >= 2 {
            ticks.push((p, format!("10{}", openreadout_figure::superscript(d))));
        }
    }
    if wanted != "log" && sorted.first().is_some_and(|&lo| lo < -1000.0) {
        let p = prep.apply(-1000.0);
        if p.is_finite() {
            ticks.insert(0, (p, "−10³".into()));
        }
    }
    Ok(Param {
        name,
        label: display,
        values,
        ticks: Some(ticks),
        transform: spec_str,
    })
}

pub(super) fn events(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let (Some(xn), Some(yn)) = (&spec.x, &spec.y) else {
        return Err(usage(
            "FCS events need x and y (parameter names, $PnN or $PnS)",
        ));
    };
    let r = read(file, spec)?;
    let xi = find(&r, xn, file)?;
    let yi = find(&r, yn, file)?;
    let px = param(&r, xi, spec)?;
    let py = param(&r, yi, spec)?;
    let n = px.values.len();
    if (n as u64) < r.total {
        ctx.warnings.push(format!(
            "{}: drew the first {n} of {} events",
            super::file_label(file),
            r.total
        ));
    }
    let base_src = format!(
        "{} × {}, {} events{}",
        px.name,
        py.name,
        n,
        if px.transform == "linear" && py.transform == "linear" {
            String::new()
        } else {
            format!(", x {} / y {}", px.transform, py.transform)
        }
    );
    let xq = Quantity::new(px.label.clone(), None);
    let yq = Quantity::new(py.label.clone(), None);
    let mut out = Vec::new();
    let mut base = Series::new(
        if spec.populations.is_empty() {
            "events".to_string()
        } else {
            "all events".to_string()
        },
        Geom::Events {
            x: px.values.clone(),
            y: py.values.clone(),
        },
        MarkKind::Density,
        Source::file(file, base_src.clone()),
    )
    .axes(xq.clone(), yq.clone());
    base.x_ticks.clone_from(&px.ticks);
    base.y_ticks.clone_from(&py.ticks);
    if !spec.populations.is_empty() {
        base.weight = Weight::Context;
    }
    out.push(base);
    for (k, name) in r.names.iter().enumerate() {
        let Some(path) = name.strip_prefix("gate:") else {
            continue;
        };
        let member = &r.columns[k];
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        for (j, &m) in member.iter().enumerate() {
            if m > 0.5 {
                xs.push(px.values[j]);
                ys.push(py.values[j]);
            }
        }
        let leaf = path
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or(path)
            .to_string();
        let pct = if n > 0 {
            100.0 * xs.len() as f64 / n as f64
        } else {
            0.0
        };
        let mut s = Series::new(
            format!("{leaf} ({pct:.1}% of events)"),
            Geom::Events { x: xs, y: ys },
            MarkKind::Points,
            Source::file(file, format!("{base_src}, population {path}")),
        )
        .axes(xq.clone(), yq.clone());
        s.x_ticks.clone_from(&px.ticks);
        s.y_ticks.clone_from(&py.ticks);
        out.push(s);
    }
    Ok(out)
}

pub(super) fn histogram(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let Some(xn) = &spec.x else {
        let r = read(file, spec)?;
        let list: Vec<&String> = r.names.iter().filter(|n| !n.starts_with("gate:")).collect();
        return Err(usage(format!(
            "{file}: give x, the parameter to plot (y too for a dot plot); parameters: {}",
            list.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    };
    let r = read(file, spec)?;
    let xi = find(&r, xn, file)?;
    let px = param(&r, xi, spec)?;
    if (px.values.len() as u64) < r.total {
        ctx.warnings.push(format!(
            "{}: binned the first {} of {} events",
            super::file_label(file),
            px.values.len(),
            r.total
        ));
    }
    let mut out = Vec::new();
    let pops: Vec<(usize, String)> = r
        .names
        .iter()
        .enumerate()
        .filter_map(|(k, n)| n.strip_prefix("gate:").map(|p| (k, p.to_string())))
        .collect();
    let groups: Vec<(String, Vec<f64>)> = if pops.is_empty() {
        vec![(px.name.clone(), px.values.clone())]
    } else {
        pops.iter()
            .map(|(k, p)| {
                let leaf = p
                    .rsplit('/')
                    .find(|s| !s.is_empty())
                    .unwrap_or(p)
                    .to_string();
                let v: Vec<f64> = r.columns[*k]
                    .iter()
                    .zip(&px.values)
                    .filter(|(m, _)| **m > 0.5)
                    .map(|(_, v)| *v)
                    .collect();
                (leaf, v)
            })
            .collect()
    };
    for (name, v) in groups {
        let mut s = Series::new(
            name.clone(),
            Geom::Hist(v),
            MarkKind::Area,
            Source::file(
                file,
                format!(
                    "{} histogram ({}), population {name}",
                    px.name, px.transform
                ),
            ),
        )
        .axes(
            Quantity::new(px.label.clone(), None),
            Quantity::new("events", None),
        );
        s.x_ticks.clone_from(&px.ticks);
        out.push(s);
    }
    Ok(out)
}
