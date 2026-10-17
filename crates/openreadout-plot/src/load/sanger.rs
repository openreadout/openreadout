//! Sanger reads (ABI .ab1): the four bases' traces on a base-position axis, with the called
//! bases and their quality, read from the trace and the base-call table the reader gives.

use openreadout_core::Result;
use openreadout_core::model::{FileInfo, Table, TableInfo, TraceInfo};
use serde_json::Value;

use super::{Ctx, usage};
use crate::model::{BaseCalls, Geom, Quantity, Series, Source};
use crate::spec::{MarkKind, SeriesSpec};

/// Bases drawn by default.
const DEFAULT_BASES: usize = 80;
/// Most bases drawn.
const MAX_BASES: f64 = 2000.0;

fn kind(extra: &std::collections::BTreeMap<String, Value>) -> Option<&str> {
    extra.get("kind").and_then(Value::as_str)
}

/// The base-call table and the trace its `scan` column indexes (`trace` picks the trace).
pub(super) fn parts(info: &FileInfo, trace: Option<u32>) -> Option<(&TableInfo, &TraceInfo)> {
    info.traces
        .iter()
        .filter(|t| trace.is_none_or(|i| i == t.index))
        .find_map(|t| {
            let k = t.extra.get("base_calls_table").and_then(Value::as_u64)?;
            let calls = info.tables.iter().find(|c| u64::from(c.index) == k)?;
            (kind(&calls.extra) == Some("base_calls")
                && calls.columns.iter().any(|c| c.name == "scan"))
            .then_some((calls, t))
        })
}

fn column(t: &Table, info: &TableInfo, name: &str) -> Option<Vec<f64>> {
    let k = info.columns.iter().position(|c| c.name == name)?;
    t.columns.get(k).cloned()
}

/// The text of a coded column.
fn texts(t: &Table, info: &TableInfo, name: &str) -> Option<Vec<String>> {
    let k = info.columns.iter().position(|c| c.name == name)?;
    let cats: Vec<String> = info.columns[k]
        .extra
        .get("categories")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    Some(
        t.columns
            .get(k)?
            .iter()
            .map(|c| cats.get(*c as usize).cloned().unwrap_or_default())
            .collect(),
    )
}

/// Piecewise-linear map between scans and base positions through the called peaks
/// (`scans[i]` is base `i + 1`), extended past the ends by the median spacing.
struct BaseAxis {
    scans: Vec<f64>,
    spacing: f64,
}

impl BaseAxis {
    fn new(scans: Vec<f64>) -> Option<BaseAxis> {
        if scans.len() < 2 || scans.windows(2).any(|w| w[1] <= w[0] || w[1].is_nan()) {
            return None;
        }
        let mut d: Vec<f64> = scans.windows(2).map(|w| w[1] - w[0]).collect();
        d.sort_by(f64::total_cmp);
        let spacing = d[d.len() / 2].max(1e-6);
        Some(BaseAxis { scans, spacing })
    }

    fn pos(&self, scan: f64) -> f64 {
        let s = &self.scans;
        let n = s.len();
        if scan <= s[0] {
            return 1.0 - (s[0] - scan) / self.spacing;
        }
        if scan >= s[n - 1] {
            return n as f64 + (scan - s[n - 1]) / self.spacing;
        }
        let k = s.partition_point(|v| *v <= scan).saturating_sub(1);
        k as f64 + 1.0 + (scan - s[k]) / (s[k + 1] - s[k])
    }

    fn scan(&self, pos: f64) -> f64 {
        let s = &self.scans;
        let n = s.len();
        if pos <= 1.0 {
            return s[0] - (1.0 - pos) * self.spacing;
        }
        if pos >= n as f64 {
            return s[n - 1] + (pos - n as f64) * self.spacing;
        }
        let k = (pos.floor() as usize).clamp(1, n - 1);
        s[k - 1] + (pos - k as f64) * (s[k] - s[k - 1])
    }
}

/// The first base from which the next 20 average quality 20 or more.
fn good_start(q: &[f64]) -> usize {
    let w = 20.min(q.len());
    if w == 0 {
        return 0;
    }
    (0..=q.len() - w)
        .find(|&i| q[i..i + w].iter().sum::<f64>() / w as f64 >= 20.0)
        .unwrap_or(0)
}

/// The four bases' traces of a Sanger read in a window of base positions; the first series
/// carries the base calls.
pub(super) fn sanger(ctx: &mut Ctx<'_>, spec: &SeriesSpec, file: &str) -> Result<Vec<Series>> {
    let (_, mut ds, info) = ctx.open(file)?;
    let Some((calls_info, t)) = parts(&info, spec.trace) else {
        return Err(usage(match spec.trace {
            Some(i) => format!(
                "{file}: trace {i} has no base calls; show=sanger draws the analyzed trace of a Sanger read (leave trace unset), show=trace draws any trace"
            ),
            None => format!(
                "{file}: not a Sanger read (no base-call table); show=sanger draws ABI .ab1 reads"
            ),
        }));
    };
    let (calls_info, t) = (calls_info.clone(), t.clone());
    let table = ds.read_table(calls_info.index, 0, calls_info.row_count)?;
    let bases = texts(&table, &calls_info, "base").unwrap_or_default();
    let scans = column(&table, &calls_info, "scan").unwrap_or_default();
    // basecallers without quality values write zeros
    let quality = column(&table, &calls_info, "quality").filter(|q| q.iter().any(|v| *v > 0.0));
    let n = bases.len().min(scans.len());
    let axis = BaseAxis::new(scans[..n].to_vec()).ok_or_else(|| {
        usage(format!(
            "{file}: the called peaks of this Sanger read do not rise; draw the trace itself with show=trace"
        ))
    })?;
    // the window, in bases
    let (lo, hi) = if let Some([a, b]) = spec.x_range.or(ctx.x_range) {
        let (a, b) = (a.min(b), a.max(b));
        let a = a.max(0.5).min(n as f64 + 0.5);
        (a, b.max(a + 1.0).min(a + MAX_BASES))
    } else {
        let first = quality.as_deref().map_or(0, good_start) + 1;
        let last = (first + DEFAULT_BASES - 1).min(n.max(1));
        let first = (last + 1).saturating_sub(DEFAULT_BASES).max(1);
        (first as f64 - 0.5, last as f64 + 0.5)
    };
    let len = openreadout_core::trace::sweep_samples(&t, 0);
    let s0 = (axis.scan(lo).floor().max(0.0) as u64).min(len);
    let s1 = (axis.scan(hi).ceil().max(0.0) as u64 + 1).min(len);
    let base_channels: Vec<(u32, String, Option<String>)> = t
        .channels
        .iter()
        .filter(|c| matches!(c.name.as_str(), "A" | "C" | "G" | "T"))
        .map(|c| (c.index, c.name.clone(), c.unit.clone()))
        .collect();
    let count = s1.saturating_sub(s0) as usize;
    ctx.spend(count.saturating_mul(base_channels.len()))?;
    let tr = ds.read_trace(t.index, 0, s0, s1.saturating_sub(s0))?;
    let first = (lo.ceil().max(1.0)) as usize;
    let last = (hi.floor() as usize).min(n);
    let mut out = Vec::new();
    for (c, name, unit) in &base_channels {
        let Some(v) = tr.channels.get(*c as usize) else {
            continue;
        };
        let pts: Vec<(f64, f64)> = v
            .iter()
            .enumerate()
            .map(|(i, y)| (axis.pos(s0 as f64 + i as f64), *y))
            .collect();
        let mut s = Series::new(
            name.clone(),
            Geom::Line(pts),
            MarkKind::Line,
            Source::file(file, format!("trace {} channel {name}", t.index)),
        )
        .axes(
            Quantity::new("base position", None),
            Quantity::new("signal", unit.as_deref()),
        );
        s.palette = openreadout_figure::series_index_of(name);
        out.push(s);
    }
    let Some(head) = out.first_mut() else {
        return Err(usage(format!(
            "{file}: trace {} has no A, C, G and T channels",
            t.index
        )));
    };
    head.calls = Some(BaseCalls {
        window: (lo, hi),
        calls: (first..=last)
            .map(|i| {
                (
                    i as f64,
                    bases[i - 1].clone(),
                    quality.as_ref().and_then(|q| q.get(i - 1).copied()),
                )
            })
            .collect(),
    });
    ctx.notes.push(format!(
        "{}: bases {first}–{last} of {n}; x_range [a, b] in bases draws another window",
        super::file_label(file)
    ));
    Ok(out)
}
