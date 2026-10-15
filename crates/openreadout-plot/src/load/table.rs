//! Table files (openreadout_batch output, any CSV or Parquet) and inline values.

use std::path::Path;

use openreadout_batch::summarize::Filter;
use openreadout_batch::{Table, Value};
use openreadout_core::Result;

use super::{Ctx, usage};
use crate::model::{Callout, Geom, Quantity, Series, Source};
use crate::spec::{InlineValues, MarkKind, SeriesSpec, Show};

fn column<'a>(t: &'a Table, name: &str, path: &str) -> Result<(usize, &'a openreadout_batch::Column)> {
    t.columns
        .iter()
        .enumerate()
        .find(|(_, c)| c.name == name)
        .or_else(|| t.columns.iter().enumerate().find(|(_, c)| c.name.eq_ignore_ascii_case(name)))
        .ok_or_else(|| {
            usage(format!(
                "{path}: no column `{name}`; columns: {}",
                t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
            ))
        })
}

pub(super) fn table(ctx: &mut Ctx<'_>, spec: &SeriesSpec, path: &str) -> Result<Vec<Series>> {
    let (t, _) = openreadout_batch::write::read_table(Path::new(path))?;
    let filters: Vec<Filter> = spec.where_.iter().map(|w| Filter::parse(w)).collect::<Result<_>>()?;
    let mut fidx = Vec::new();
    for f in &filters {
        fidx.push(column(&t, &f.column, path)?.0);
    }
    let rows: Vec<&Vec<Value>> = t
        .rows
        .iter()
        .filter(|r| filters.iter().zip(&fidx).all(|(f, &i)| r.get(i).is_some_and(|v| f.keeps(v))))
        .collect();
    if rows.is_empty() {
        return Err(usage(format!("{path}: no rows left after the `where` filters")));
    }
    let Some(xn) = &spec.x else {
        return Err(usage(format!(
            "{path}: name the columns to plot with x (and y); columns: {}",
            t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
        )));
    };
    let (xi, xc) = column(&t, xn, path)?;
    let xq = Quantity::new(xc.name.clone(), xc.unit.as_deref());
    let by = spec.by.as_deref().map(|b| column(&t, b, path)).transpose()?;
    let mut groups: Vec<(String, Vec<&Vec<Value>>)> = Vec::new();
    for r in &rows {
        let key = by.map_or_else(String::new, |(bi, _)| r.get(bi).map(Value::text).unwrap_or_default());
        // file paths name their series by the file
        let key = if key.contains('/') && key.contains('.') { super::file_label(&key) } else { key };
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => g.1.push(r),
            None => groups.push((key, vec![r])),
        }
    }
    if spec.show == Some(Show::Histogram) {
        return Ok(groups
            .into_iter()
            .map(|(g, rs)| {
                let v: Vec<f64> = rs.iter().filter_map(|r| r.get(xi).and_then(Value::as_f64)).collect();
                let name = if g.is_empty() { xc.name.clone() } else { g };
                Series::new(name, Geom::Hist(v), MarkKind::Area, Source::file(path, format!("column {}", xc.name)))
                    .axes(xq.clone(), Quantity::new("rows", None))
            })
            .collect());
    }
    let Some(yn) = &spec.y else {
        return Err(usage(format!("{path}: give y, the value column")));
    };
    let (yi, yc) = column(&t, yn, path)?;
    let yq = Quantity::new(yc.name.clone(), yc.unit.as_deref());
    let ei = spec.error.as_deref().map(|e| column(&t, e, path).map(|c| c.0)).transpose()?;
    let numeric_x = rows
        .iter()
        .filter_map(|r| r.get(xi))
        .filter(|v| !v.is_null())
        .all(|v| v.as_f64().is_some() && !matches!(v, Value::Text(_)));
    let mut out = Vec::new();
    let sel = |g: &str| {
        let mut s = format!("columns {} × {}", xc.name, yc.name);
        if !g.is_empty() {
            s.push_str(&format!(", {} = {g}", spec.by.as_deref().unwrap_or("")));
        }
        if !spec.where_.is_empty() {
            s.push_str(&format!(", where {}", spec.where_.join(" and ")));
        }
        s
    };
    let mut averaged = false;
    for (g, rs) in &groups {
        let name = if g.is_empty() { yc.name.clone() } else { g.clone() };
        if numeric_x {
            let mut pts = Vec::new();
            let mut err = Vec::new();
            for r in rs {
                let (Some(x), Some(y)) = (r.get(xi).and_then(Value::as_f64), r.get(yi).and_then(Value::as_f64)) else {
                    continue;
                };
                pts.push((x, y));
                err.push(ei.and_then(|i| r.get(i)).and_then(Value::as_f64).unwrap_or(f64::NAN));
            }
            let mut idx: Vec<usize> = (0..pts.len()).collect();
            idx.sort_by(|&a, &b| pts[a].0.total_cmp(&pts[b].0));
            let pts2: Vec<(f64, f64)> = idx.iter().map(|&i| pts[i]).collect();
            let err2: Vec<f64> = if ei.is_some() { idx.iter().map(|&i| err[i]).collect() } else { Vec::new() };
            let mark = if groups.len() > 1 { MarkKind::LinePoints } else { MarkKind::Points };
            out.push(
                Series::new(name, Geom::Points { pts: pts2, err: err2 }, mark, Source::file(path, sel(g)))
                    .axes(xq.clone(), yq.clone()),
            );
        } else {
            let mut cats: Vec<String> = Vec::new();
            let mut sums: Vec<(f64, usize, f64)> = Vec::new();
            for r in rs {
                let c = r.get(xi).map(Value::text).unwrap_or_default();
                let Some(y) = r.get(yi).and_then(Value::as_f64) else { continue };
                let e = ei.and_then(|i| r.get(i)).and_then(Value::as_f64).unwrap_or(f64::NAN);
                match cats.iter().position(|x| *x == c) {
                    Some(k) => {
                        sums[k].0 += y;
                        sums[k].1 += 1;
                        averaged = true;
                    }
                    None => {
                        cats.push(c);
                        sums.push((y, 1, e));
                    }
                }
            }
            let vals: Vec<f64> = sums.iter().map(|s| s.0 / s.1 as f64).collect();
            let err: Vec<f64> = if ei.is_some() { sums.iter().map(|s| if s.1 == 1 { s.2 } else { f64::NAN }).collect() } else { Vec::new() };
            let mut s = Series::new(name, Geom::Bars { cats: cats.clone(), vals, err }, MarkKind::Bars, Source::file(path, sel(g)))
                .axes(xq.clone(), yq.clone());
            s.x_cats = Some(cats);
            out.push(s);
        }
    }
    if averaged {
        ctx.warnings.push(format!(
            "{}: several rows share an x category; their mean is drawn (filter with `where` or split with `by`)",
            super::file_label(path)
        ));
    }
    Ok(out)
}

pub(super) fn inline(spec: &SeriesSpec, v: &InlineValues) -> Result<Vec<Series>> {
    if v.x.len() != v.y.len() {
        return Err(usage(format!(
            "values: x has {} entries and y {}; they must match",
            v.x.len(),
            v.y.len()
        )));
    }
    if !v.error.is_empty() && v.error.len() != v.y.len() {
        return Err(usage("values: error must have one entry per y"));
    }
    let nums: Option<Vec<f64>> = v.x.iter().map(serde_json::Value::as_f64).collect();
    let xq = Quantity::new(spec.x.clone().unwrap_or_else(|| "x".into()), v.x_unit.as_deref());
    let yq = Quantity::new(spec.y.clone().unwrap_or_else(|| "y".into()), v.y_unit.as_deref());
    let name = spec.label.clone().unwrap_or_default();
    let src = Source {
        file: None,
        selector: format!("inline values ({} points)", v.y.len()),
    };
    let mut s = match nums {
        Some(xs) => Series::new(
            name,
            Geom::Points { pts: xs.into_iter().zip(v.y.iter().copied()).collect(), err: v.error.clone() },
            MarkKind::Points,
            src,
        ),
        None => {
            let cats: Vec<String> = v
                .x
                .iter()
                .map(|c| c.as_str().map_or_else(|| c.to_string(), str::to_string))
                .collect();
            let mut s = Series::new(
                name,
                Geom::Bars { cats: cats.clone(), vals: v.y.clone(), err: v.error.clone() },
                MarkKind::Bars,
                src,
            );
            s.x_cats = Some(cats);
            s
        }
    }
    .axes(xq, yq);
    if !v.labels.is_empty() {
        let pos: Vec<(f64, f64)> = match &s.geom {
            Geom::Points { pts, .. } => pts.clone(),
            Geom::Bars { vals, .. } => vals.iter().enumerate().map(|(i, &y)| (i as f64, y)).collect(),
            _ => Vec::new(),
        };
        s.callouts = pos
            .into_iter()
            .zip(&v.labels)
            .filter(|(_, l)| !l.is_empty())
            .map(|((x, y), l)| Callout { x, y, text: l.clone(), auto: false })
            .collect();
    }
    if let Some(m) = spec.mark {
        s.mark = m;
    }
    Ok(vec![s])
}
