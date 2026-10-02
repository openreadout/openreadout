//! The analysis pipeline: pick the read, apply the layout, reduce kinetic reads, flag outliers,
//! subtract blanks, then fit or summarise (book/src/guides/plate-analysis.md "Pipeline").

use std::collections::{BTreeMap, BTreeSet};

use openreadout_core::{Error, Result};

use crate::data::{PlateData, ReadInfo};
use crate::fit::{self, FitResult, InverseFail, Model};
use crate::kinetics;
use crate::layout::{self, Layout, Role, WellInfo};
use crate::output::{
    AssayOutput, BlankSummary, CompoundRow, CurvePoint, CurveReport, FitReport, GroupStats,
    GrowthRow, KineticRow, LayoutSummary, OutlierSummary, ParamRow, QualityReport, ReadSummary,
    SampleRow, WellRow,
};
use crate::request::{Analysis, AssayRequest, BlankMode, FitOn, Normalize, OutlierRule, Reduce};
use crate::stats;

/// Smallest default window (points); the default is this or a tenth of the time points,
/// whichever is larger ([`crate::kinetics::default_window`]).
pub const DEFAULT_WINDOW: usize = 5;
/// Default modified z-score cut-off.
pub const DEFAULT_MAD_THRESHOLD: f64 = 3.5;
/// Default Grubbs alpha.
pub const DEFAULT_GRUBBS_ALPHA: f64 = 0.05;
/// Smallest replicate group the modified z-score is applied to (the MAD of fewer values is
/// degenerate: with three values it is the smaller of two distances).
pub const MAD_MIN_GROUP: usize = 5;
/// Standards whose mean back-calculated recovery is within 100 ± this percent (and CV at most
/// this) set LLOQ/ULOQ.
pub const LOQ_TOLERANCE_PERCENT: f64 = 20.0;

fn usage(m: impl Into<String>) -> Error {
    Error::Usage(m.into())
}

/// Build the user layout of a request: the layout file, then `layout_text`, then the well
/// flags (`blank_wells`, `positive_wells`, `negative_wells`, `empty_wells`, `standards`), each
/// overriding the previous well by well.
pub fn user_layout(req: &AssayRequest) -> Result<Option<Layout>> {
    let mut out = Layout::default();
    let mut any = false;
    if let Some(p) = &req.layout {
        let text = std::fs::read(p).map_err(|e| Error::io(p, e))?;
        let text = String::from_utf8_lossy(&text);
        out.merge(layout::parse_layout(&text, p).map_err(Error::Usage)?);
        any = true;
    }
    if let Some(t) = &req.layout_text {
        out.merge(layout::parse_layout(t, "layout_text").map_err(Error::Usage)?);
        any = true;
    }
    let flag_wells = |spec: &Option<String>, name: &str| -> Result<Option<Vec<(u32, u32)>>> {
        spec.as_deref()
            .map(|s| layout::parse_wells(s).map_err(|m| usage(format!("{name}: {m}"))))
            .transpose()
    };
    for (spec, name, role) in [
        (&req.blank_wells, "--blank", Role::Blank),
        (&req.positive_wells, "--positive", Role::Positive),
        (&req.negative_wells, "--negative", Role::Negative),
        (&req.empty_wells, "--empty", Role::Empty),
    ] {
        if let Some(w) = flag_wells(spec, name)? {
            out.set(&w, |i| {
                i.role = Some(role);
                i.concentration = None;
                i.sample = None;
            });
            out.sources.push(name.to_string());
            any = true;
        }
    }
    if let Some(s) = &req.standards {
        for item in s.split(';').map(str::trim).filter(|x| !x.is_empty()) {
            let (w, c) = item.rsplit_once('=').ok_or_else(|| {
                usage(format!(
                    "--standard: `{item}` is not WELLS=CONCENTRATION (e.g. A1,A2=100)"
                ))
            })?;
            let conc: f64 = c
                .trim()
                .parse()
                .map_err(|_| usage(format!("--standard: `{c}` is not a number")))?;
            let wells = layout::parse_wells(w).map_err(|m| usage(format!("--standard: {m}")))?;
            out.set(&wells, |i| {
                i.role = Some(Role::Standard);
                i.concentration = Some(conc);
                i.sample = None;
            });
        }
        out.sources.push("--standard".into());
        any = true;
    }
    Ok(any.then_some(out))
}

fn select_read<'a>(plate: &'a PlateData, req: &AssayRequest) -> Result<&'a ReadInfo> {
    let list = || {
        plate
            .reads
            .iter()
            .map(|r| {
                format!(
                    "{} `{}`{}",
                    r.number,
                    r.label,
                    if r.calculated { " (calculated)" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let time_course = matches!(req.analysis, Analysis::Kinetics | Analysis::Growth);
    // a read with more than one distinct time point
    let kinetic = |r: &ReadInfo| {
        let mut first: Option<f64> = None;
        plate
            .obs
            .iter()
            .filter(|o| o.read == r.number)
            .filter_map(|o| o.time_s)
            .any(|t| *first.get_or_insert(t) != t)
    };
    match req.read.as_deref().map(str::trim) {
        // time-course analyses: the first measured kinetic read
        None | Some("") if time_course => plate
            .reads
            .iter()
            .find(|r| !r.calculated && kinetic(r))
            .or_else(|| plate.reads.iter().find(|r| kinetic(r)))
            .or_else(|| plate.reads.first())
            .ok_or_else(|| usage("the plate has no reads")),
        None | Some("") => plate
            .reads
            .iter()
            .find(|r| !r.calculated)
            .or_else(|| plate.reads.first())
            .ok_or_else(|| usage("the plate has no reads")),
        Some(s) => {
            if let Ok(n) = s.parse::<u32>()
                && let Some(r) = plate.reads.iter().find(|r| r.number == n)
            {
                return Ok(r);
            }
            if let Some(r) = plate.reads.iter().find(|r| r.label.eq_ignore_ascii_case(s)) {
                return Ok(r);
            }
            let l = s.to_ascii_lowercase();
            let hits: Vec<&ReadInfo> = plate
                .reads
                .iter()
                .filter(|r| r.label.to_ascii_lowercase().contains(&l))
                .collect();
            match hits.as_slice() {
                [one] => Ok(one),
                [] => Err(usage(format!("no read matches `{s}`; reads: {}", list()))),
                _ => Err(usage(format!(
                    "`{s}` matches several reads; pick one by number: {}",
                    list()
                ))),
            }
        }
    }
}

/// Per-well time series of the selected read: well → [(time, value)] sorted by time.
type Series = BTreeMap<(u32, u32), Vec<(f64, f64)>>;

fn collect_series(
    plate: &PlateData,
    read: &ReadInfo,
    req: &AssayRequest,
    notes: &mut Vec<String>,
) -> Result<(Series, Option<f64>)> {
    let obs: Vec<&crate::data::Obs> = plate.obs.iter().filter(|o| o.read == read.number).collect();
    // distinct wavelengths within one well and time point → a spectrum
    let mut wls: BTreeSet<u64> = BTreeSet::new();
    let mut per_point: BTreeMap<(u32, u32, u64), BTreeSet<u64>> = BTreeMap::new();
    for o in &obs {
        if let Some(w) = o.wavelength_nm {
            wls.insert(w.to_bits());
            per_point
                .entry((o.row, o.col, o.time_s.unwrap_or(-1.0).to_bits()))
                .or_default()
                .insert(w.to_bits());
        }
    }
    let spectral = per_point.values().any(|s| s.len() > 1);
    let mut chosen = None;
    let keep: Box<dyn Fn(&crate::data::Obs) -> bool> = match req.wavelength_nm {
        Some(w) => {
            let near = wls
                .iter()
                .map(|b| f64::from_bits(*b))
                .min_by(|a, b| (a - w).abs().total_cmp(&(b - w).abs()));
            match near {
                Some(n) if (n - w).abs() <= 0.51 => {
                    chosen = Some(n);
                    Box::new(move |o: &crate::data::Obs| {
                        o.wavelength_nm.is_some_and(|x| (x - n).abs() < 1e-9)
                    })
                }
                _ => {
                    return Err(usage(format!(
                        "read {} has no values at {w} nm (wavelengths: {})",
                        read.number,
                        wl_list(&wls)
                    )));
                }
            }
        }
        None if spectral => {
            return Err(usage(format!(
                "read {} (`{}`) is a spectrum ({} wavelengths: {}); choose one with --wavelength NM",
                read.number,
                read.label,
                wls.len(),
                wl_list(&wls)
            )));
        }
        None => Box::new(|_: &crate::data::Obs| true),
    };
    let mut series: Series = BTreeMap::new();
    for o in obs.into_iter().filter(|o| keep(o)) {
        series
            .entry((o.row, o.col))
            .or_default()
            .push((o.time_s.unwrap_or(f64::NAN), o.value));
    }
    for s in series.values_mut() {
        s.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    if series.is_empty() {
        return Err(usage(format!(
            "read {} (`{}`) has no values",
            read.number, read.label
        )));
    }
    if let Some(w) = chosen {
        notes.push(format!("values at {w} nm"));
    }
    Ok((series, chosen))
}

fn wl_list(wls: &BTreeSet<u64>) -> String {
    let v: Vec<String> = wls
        .iter()
        .take(8)
        .map(|b| format!("{}", f64::from_bits(*b)))
        .collect();
    let more = if wls.len() > 8 { ", …" } else { "" };
    format!("{}{more}", v.join(", "))
}

fn is_kinetic(series: &Series) -> bool {
    series
        .values()
        .any(|s| s.iter().filter(|(t, _)| t.is_finite()).count() > 1)
}

/// One value per well from a time series.
fn reduce_series(points: &[(f64, f64)], reduce: Reduce, window: Option<usize>) -> Option<f64> {
    let (t, y): (Vec<f64>, Vec<f64>) = points.iter().copied().unzip();
    let (t, y) = kinetics::clean(&t, &y);
    if y.is_empty() {
        return None;
    }
    match reduce {
        Reduce::First => y.first().copied(),
        Reduce::Last => y.last().copied(),
        Reduce::Max => y.iter().copied().reduce(f64::max),
        Reduce::Min => y.iter().copied().reduce(f64::min),
        Reduce::Mean => stats::mean(&y),
        Reduce::MaxSlope => kinetics::kinetic(&t, &y, window).map(|k| k.max_slope_per_s * 60.0),
        Reduce::MeanSlope => stats::ols(&t, &y).map(|(s, _, _)| s * 60.0),
        Reduce::Auc => (t.len() >= 2).then(|| stats::trapezoid(&t, &y)),
    }
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v}");
    if s.len() > 12 { format!("{v:.6e}") } else { s }
}

/// A measured well with its layout.
#[derive(Debug, Clone)]
struct Well {
    pos: (u32, u32),
    name: String,
    info: WellInfo,
    role: Option<Role>,
    group: String,
    raw: Option<f64>,
    value: Option<f64>,
    outlier: bool,
    excluded: bool,
}

fn role_id(r: Option<Role>) -> String {
    r.map_or_else(|| "unassigned".to_string(), |r| r.id().to_string())
}

/// Replicate-group keys: the explicit group, else the sample name (with the concentration when
/// one name spans several), else role and concentration/compound, else the well itself.
fn assign_groups(wells: &mut [Well]) {
    let mut concs: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    for w in wells.iter() {
        if let Some(s) = &w.info.sample {
            concs
                .entry(s.clone())
                .or_default()
                .insert(w.info.concentration.unwrap_or(f64::NAN).to_bits());
        }
    }
    for w in wells.iter_mut() {
        let i = &w.info;
        w.group = if let Some(g) = &i.group {
            g.clone()
        } else if let Some(s) = &i.sample {
            match (concs.get(s).map_or(0, BTreeSet::len) > 1, i.concentration) {
                (true, Some(c)) => format!("{s} @ {}", fmt_num(c)),
                _ => s.clone(),
            }
        } else {
            match (w.role, &i.compound, i.concentration) {
                (_, Some(cpd), Some(c)) => format!("{cpd} @ {}", fmt_num(c)),
                (Some(Role::Standard), None, Some(c)) => format!("standard {}", fmt_num(c)),
                (Some(r), None, Some(c)) => format!("{} @ {}", r.id(), fmt_num(c)),
                (
                    Some(r @ (Role::Blank | Role::Positive | Role::Negative | Role::Control)),
                    _,
                    None,
                ) => r.id().to_string(),
                _ => w.name.clone(),
            }
        };
    }
}

/// Flag outliers group by group on `raw`.
fn flag_outliers(wells: &mut [Well], rule: OutlierRule, threshold: Option<f64>) -> Option<f64> {
    let thr = match rule {
        OutlierRule::None => return None,
        OutlierRule::Mad => threshold.unwrap_or(DEFAULT_MAD_THRESHOLD),
        OutlierRule::Grubbs => threshold.unwrap_or(DEFAULT_GRUBBS_ALPHA),
    };
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, w) in wells.iter().enumerate() {
        if w.raw.is_some() && w.role != Some(Role::Empty) {
            groups.entry(w.group.clone()).or_default().push(i);
        }
    }
    for idx in groups.values() {
        let min = if rule == OutlierRule::Mad {
            MAD_MIN_GROUP
        } else {
            3
        };
        if idx.len() < min {
            continue;
        }
        let vals: Vec<f64> = idx.iter().filter_map(|&i| wells[i].raw).collect();
        match rule {
            OutlierRule::Mad => {
                let (Some(med), Some(mad)) = (stats::median(&vals), stats::mad(&vals)) else {
                    continue;
                };
                if mad <= 0.0 {
                    continue;
                }
                for &i in idx {
                    if let Some(v) = wells[i].raw
                        && (0.6745 * (v - med) / mad).abs() > thr
                    {
                        wells[i].outlier = true;
                    }
                }
            }
            OutlierRule::Grubbs => {
                let (Some(m), Some(s), Some(g)) = (
                    stats::mean(&vals),
                    stats::sd(&vals),
                    stats::grubbs_critical(vals.len(), thr),
                ) else {
                    continue;
                };
                if s <= 0.0 {
                    continue;
                }
                let (far, dev) = idx
                    .iter()
                    .filter_map(|&i| wells[i].raw.map(|v| (i, (v - m).abs())))
                    .fold((usize::MAX, -1.0), |a, b| if b.1 > a.1 { b } else { a });
                if far != usize::MAX && dev / s > g {
                    wells[far].outlier = true;
                }
            }
            OutlierRule::None => {}
        }
    }
    Some(thr)
}

fn group_stats(wells: &[&Well], val: impl Fn(&Well) -> Option<f64>) -> Option<GroupStats> {
    let used: Vec<(&str, f64)> = wells
        .iter()
        .filter(|w| !w.excluded)
        .filter_map(|w| val(w).map(|v| (w.name.as_str(), v)))
        .collect();
    let v: Vec<f64> = used.iter().map(|x| x.1).collect();
    Some(GroupStats {
        wells: used.iter().map(|x| x.0.to_string()).collect(),
        n: v.len(),
        mean: stats::mean(&v)?,
        sd: stats::sd(&v),
        cv_percent: stats::cv_percent(&v),
        min: v.iter().copied().fold(f64::INFINITY, f64::min),
        max: v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    })
}

fn quality(wells: &[Well], samples: &[SampleRow]) -> QualityReport {
    let by = |r: Role| -> Vec<&Well> { wells.iter().filter(|w| w.role == Some(r)).collect() };
    let raw = |w: &Well| w.raw;
    let pos = group_stats(&by(Role::Positive), raw);
    let neg = group_stats(&by(Role::Negative), raw);
    let mut q = QualityReport {
        blank: group_stats(&by(Role::Blank), raw),
        samples: group_stats(&by(Role::Sample), raw),
        ..QualityReport::default()
    };
    if let (Some(p), Some(n)) = (&pos, &neg) {
        let diff = (p.mean - n.mean).abs();
        if let (Some(sp), Some(sn)) = (p.sd, n.sd)
            && diff > 0.0
        {
            let z = 1.0 - 3.0 * (sp + sn) / diff;
            q.z_prime = Some(z);
            q.assessment = Some(
                if z >= 0.5 {
                    "excellent"
                } else if z > 0.0 {
                    "marginal"
                } else {
                    "unusable"
                }
                .into(),
            );
            let den = (sp * sp + sn * sn).sqrt();
            q.ssmd = (den > 0.0).then(|| (p.mean - n.mean) / den);
            q.signal_to_noise = (sn > 0.0).then(|| diff / sn);
            if let Some(s) = &q.samples
                && let Some(ss) = s.sd
                && (s.mean - n.mean).abs() > 0.0
            {
                q.z_factor = Some(1.0 - 3.0 * (ss + sn) / (s.mean - n.mean).abs());
            }
        }
        let (hi, lo) = if p.mean >= n.mean {
            (p.mean, n.mean)
        } else {
            (n.mean, p.mean)
        };
        q.signal_to_background = (lo != 0.0).then(|| hi / lo);
    }
    q.positive = pos;
    q.negative = neg;
    let cvs: Vec<f64> = samples
        .iter()
        .filter(|s| s.n_used >= 2 && s.metric == "value")
        .filter_map(|s| s.cv_percent)
        .collect();
    q.median_replicate_cv_percent = stats::median(&cvs);
    q
}

fn fit_report(
    f: &FitResult,
    confidence: f64,
    weighting: fit::Weighting,
    points: Vec<CurvePoint>,
    n_levels: usize,
) -> FitReport {
    let t = stats::t_quantile(0.5 + confidence / 2.0, f.df as f64);
    let parameters = f
        .model
        .param_names()
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let se = f.se(i);
            ParamRow {
                name: (*n).to_string(),
                value: f.params[i],
                se,
                ci_low: se.zip(t).map(|(s, t)| f.params[i] - t * s),
                ci_high: se.zip(t).map(|(s, t)| f.params[i] + t * s),
            }
        })
        .collect();
    let increasing = match f.model {
        Model::Linear => f.params[0] >= 0.0,
        Model::FourPl | Model::FivePl => (f.params[3] - f.params[0]) * f.params[1] > 0.0,
    };
    FitReport {
        model: f.model.id().into(),
        formula: f.model.formula().into(),
        weighting: weighting.id().into(),
        parameters,
        n_points: f.n,
        n_levels,
        df: f.df,
        r_squared: f.r_squared,
        residual_se: (f.df > 0).then(|| (f.sse / f.df as f64).sqrt()),
        sse: f.sse,
        converged: f.converged,
        iterations: f.iterations,
        confidence,
        direction: if increasing {
            "increasing"
        } else {
            "decreasing"
        }
        .into(),
        points,
    }
}

fn range_flag(
    r: std::result::Result<f64, InverseFail>,
    lo: f64,
    hi: f64,
) -> (Option<f64>, &'static str) {
    let eps = 1e-9 * hi.abs().max(1e-300);
    match r {
        Ok(x) if x < lo - eps => (Some(x), "below_range"),
        Ok(x) if x > hi + eps => (Some(x), "above_range"),
        Ok(x) => (Some(x), "in_range"),
        Err(InverseFail::BelowCurve) => (None, "below_curve"),
        Err(InverseFail::AboveCurve) => (None, "above_curve"),
        Err(InverseFail::Flat) => (None, "not_computable"),
    }
}

/// Run an analysis on a plate. `user` is the layout from [`user_layout`].
pub fn run(plate: &PlateData, req: &AssayRequest, user: Option<Layout>) -> Result<AssayOutput> {
    if !(req.confidence > 0.0 && req.confidence < 1.0) {
        return Err(usage("confidence must be between 0 and 1 (e.g. 0.95)"));
    }
    let window = req.window;
    if window.is_some_and(|w| w < 2) {
        return Err(usage("window must be at least 2 points"));
    }
    let mut notes = Vec::new();
    let read = select_read(plate, req)?;
    if read.calculated {
        notes.push(format!(
            "read {} (`{}`) holds values the vendor software calculated",
            read.number, read.label
        ));
    }
    let (mut series, wl) = collect_series(plate, read, req, &mut notes)?;
    let kinetic = is_kinetic(&series);
    if let Some(spec) = &req.wells {
        let keep: BTreeSet<(u32, u32)> = layout::parse_wells(spec)
            .map_err(|m| usage(format!("wells: {m}")))?
            .into_iter()
            .collect();
        series.retain(|k, _| keep.contains(k));
        if series.is_empty() {
            return Err(usage(format!(
                "none of the wells `{spec}` has values in this read"
            )));
        }
    }
    // layout
    let mut lay = Layout::default();
    if req.embedded_layout
        && let Some(e) = plate.embedded.clone()
    {
        lay.merge(e);
    }
    if let Some(u) = user {
        lay.merge(u);
    }
    let role_map = crate::roles::parse_role_map(&req.roles)?;
    let (role_notes, unused_roles) = crate::roles::apply_role_map(&mut lay, &role_map);
    notes.extend(role_notes);
    let mut wells: Vec<Well> = series
        .keys()
        .map(|&pos| {
            let info = lay.wells.get(&pos).cloned().unwrap_or_default();
            let role = info.effective_role();
            Well {
                pos,
                name: layout::well_name(pos.0, pos.1),
                role,
                info,
                group: String::new(),
                raw: None,
                value: None,
                outlier: false,
                excluded: false,
            }
        })
        .collect();
    assign_groups(&mut wells);
    let mut roles: BTreeMap<String, usize> = BTreeMap::new();
    for w in &wells {
        *roles.entry(role_id(w.role)).or_default() += 1;
    }
    let layout_summary = LayoutSummary {
        sources: lay.sources.clone(),
        wells_assigned: wells.iter().filter(|w| w.role.is_some()).count(),
        roles,
        concentration_unit: lay.concentration_unit.clone(),
    };
    let time_points = series.values().map(Vec::len).max();
    let mut warnings = crate::roles::warnings(wells.iter().map(|w| (w.pos, &w.info, w.role)));
    warnings.extend(unused_roles);
    let mut out = AssayOutput {
        format: plate.format.clone(),
        analysis: req.analysis.id().into(),
        table: req.table,
        plate: plate.name.clone(),
        read: ReadSummary {
            number: read.number,
            label: read.label.clone(),
            mode: read.mode.clone(),
            unit: read.unit.clone(),
            calculated: read.calculated,
            kinetic,
            time_points: if kinetic { time_points } else { None },
            wavelength_nm: wl,
            wells_measured: series.len(),
        },
        layout: layout_summary,
        outliers: OutlierSummary {
            rule: match req.outliers {
                OutlierRule::Mad => "mad",
                OutlierRule::Grubbs => "grubbs",
                OutlierRule::None => "none",
            }
            .into(),
            excluded: req.exclude_outliers,
            ..OutlierSummary::default()
        },
        warnings,
        ..AssayOutput::default()
    };
    match req.analysis {
        Analysis::Kinetics | Analysis::Growth => {
            if !kinetic {
                return Err(usage(format!(
                    "read {} (`{}`) has one value per well, not a time course; `{}` needs a kinetic read",
                    read.number,
                    read.label,
                    req.analysis.id()
                )));
            }
            time_course(&mut out, &mut wells, &series, req, window, &mut notes)?;
        }
        _ => {
            if kinetic && req.reduce.is_none() {
                // name the reads the vendor software already reduced (Gen5 `Mean V`, `Max V`)
                let others: Vec<String> = plate
                    .reads
                    .iter()
                    .filter(|r| r.number != read.number && r.calculated)
                    .map(|r| format!("{} `{}`", r.number, r.label))
                    .collect();
                return Err(usage(format!(
                    "read {} is kinetic ({} time points per well): choose how each well's time course becomes one value with --reduce (max-slope, mean-slope, last, max, mean, min, first, auc), or run `analyze assay kinetics`{}",
                    read.number,
                    time_points.unwrap_or(0),
                    if others.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "; or analyse a read the vendor software calculated with --read ({})",
                            others.join(", ")
                        )
                    }
                )));
            }
            endpoint(
                &mut out, &mut wells, &series, req, kinetic, window, &mut notes,
            )?;
        }
    }
    out.notes.extend(notes);
    Ok(out)
}

fn blank_wells_for(wells: &[Well], mode: BlankMode) -> Result<Option<(&'static str, Vec<&Well>)>> {
    let blanks: Vec<&Well> = wells
        .iter()
        .filter(|w| w.role == Some(Role::Blank) && !w.excluded)
        .collect();
    let method = match mode {
        BlankMode::None => return Ok(None),
        BlankMode::Auto | BlankMode::Mean => "mean",
        BlankMode::Median => "median",
    };
    if blanks.is_empty() {
        if mode == BlankMode::Auto {
            return Ok(None);
        }
        return Err(usage(
            "blank subtraction was requested but no well is a blank: mark blanks in the layout (role blank) or with --blank WELLS",
        ));
    }
    Ok(Some((method, blanks)))
}

fn center(method: &str, v: &[f64]) -> Option<f64> {
    if method == "median" {
        stats::median(v)
    } else {
        stats::mean(v)
    }
}

fn endpoint(
    out: &mut AssayOutput,
    wells: &mut [Well],
    series: &Series,
    req: &AssayRequest,
    kinetic: bool,
    window: Option<usize>,
    notes: &mut Vec<String>,
) -> Result<()> {
    let reduce = match (kinetic, req.reduce) {
        (true, None) => {
            return Err(usage(format!(
                "read {} is kinetic ({} time points per well): choose how each well's time course becomes one value with --reduce (max-slope, mean-slope, last, max, mean, min, first, auc), or run `analyze assay kinetics`",
                out.read.number,
                out.read.time_points.unwrap_or(0)
            )));
        }
        (true, Some(r)) => {
            out.reduce = Some(r.id().into());
            Some(r)
        }
        (false, Some(r)) => {
            notes.push(format!(
                "--reduce {} ignored: the read has one value per well",
                r.id()
            ));
            None
        }
        (false, None) => None,
    };
    for w in wells.iter_mut() {
        let pts = &series[&w.pos];
        w.raw = if let Some(r) = reduce {
            reduce_series(pts, r, window)
        } else {
            let v: Vec<f64> = pts.iter().map(|p| p.1).filter(|v| v.is_finite()).collect();
            if v.len() > 1 {
                notes.push(format!(
                    "{}: {} values for one well; their mean is used",
                    w.name,
                    v.len()
                ));
            }
            stats::mean(&v)
        };
        if w.role == Some(Role::Empty) {
            w.excluded = true;
        }
    }
    let thr = flag_outliers(wells, req.outliers, req.outlier_threshold);
    out.outliers.threshold = thr;
    out.outliers.flagged = wells
        .iter()
        .filter(|w| w.outlier)
        .map(|w| w.name.clone())
        .collect();
    if req.exclude_outliers {
        for w in wells.iter_mut().filter(|w| w.outlier) {
            w.excluded = true;
        }
    }
    // blank
    let mut blank_value = 0.0;
    if let Some((method, blanks)) = blank_wells_for(wells, req.blank)? {
        let v: Vec<f64> = blanks.iter().filter_map(|w| w.raw).collect();
        if let Some(b) = center(method, &v) {
            blank_value = b;
            out.blank = Some(BlankSummary {
                method: method.into(),
                wells: blanks
                    .iter()
                    .filter(|w| w.raw.is_some())
                    .map(|w| w.name.clone())
                    .collect(),
                n: v.len(),
                value: b,
                sd: stats::sd(&v),
                per_time_point: false,
            });
        }
    }
    for w in wells.iter_mut() {
        w.value = w.raw.map(|r| r - blank_value);
    }
    let mut samples = sample_rows(wells, |w| w.value, "value");
    let q = quality(wells, &samples);
    let has_controls = q.positive.is_some() && q.negative.is_some();
    let mut rows: Vec<WellRow> = wells.iter().map(well_row).collect();
    match req.analysis {
        Analysis::Curve => curve(out, wells, &mut rows, &mut samples, req, notes)?,
        Analysis::DoseResponse => dose_response(out, wells, &mut rows, req, notes)?,
        _ if req.normalize == Normalize::Controls => {
            percent_of_controls(wells, &mut rows, notes)?;
        }
        Analysis::Qc if !has_controls => {
            return Err(usage(
                "Z′ needs positive and negative control wells: mark them in the layout (role positive / negative) or with --positive WELLS --negative WELLS",
            ));
        }
        _ => {}
    }
    if has_controls || req.analysis == Analysis::Qc {
        out.quality = Some(q);
    }
    out.wells = rows;
    out.samples = samples;
    Ok(())
}

/// Per-well percent effect relative to the control means (`--normalize controls` outside a
/// dose-response fit): 100 (value − mean negative) / (mean positive − mean negative).
fn percent_of_controls(
    wells: &[Well],
    rows: &mut [WellRow],
    notes: &mut Vec<String>,
) -> Result<()> {
    let mean_of = |r: Role| {
        let v: Vec<f64> = wells
            .iter()
            .filter(|w| w.role == Some(r) && !w.excluded)
            .filter_map(|w| w.value)
            .collect();
        stats::mean(&v)
    };
    let (Some(p), Some(n)) = (mean_of(Role::Positive), mean_of(Role::Negative)) else {
        return Err(usage(
            "--normalize controls needs positive and negative control wells: mark them in the layout (role positive / negative), with --positive/--negative WELLS, or name their role with --role NAME=ROLE",
        ));
    };
    if p == n {
        return Err(usage(
            "the positive and negative controls have the same mean",
        ));
    }
    notes.push(format!(
        "percent effect = 100 × (value − {}) / ({} − {}) (negative control = 0 %, positive control = 100 %)",
        fmt_num(n),
        fmt_num(p),
        fmt_num(n)
    ));
    for (w, r) in wells.iter().zip(rows.iter_mut()) {
        r.percent_effect = w.value.map(|v| 100.0 * (v - n) / (p - n));
    }
    Ok(())
}

fn well_row(w: &Well) -> WellRow {
    WellRow {
        well: w.name.clone(),
        row: w.pos.0 + 1,
        col: w.pos.1 + 1,
        role: role_id(w.role),
        group: w.group.clone(),
        sample: w.info.sample.clone(),
        compound: w.info.compound.clone(),
        concentration: w.info.concentration,
        dilution: w.info.dilution,
        raw: w.raw,
        value: w.value,
        outlier: w.outlier,
        excluded: w.excluded,
        extra: w.info.extra.clone(),
        ..WellRow::default()
    }
}

fn sample_rows(wells: &[Well], val: impl Fn(&Well) -> Option<f64>, metric: &str) -> Vec<SampleRow> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Vec<&Well>> = BTreeMap::new();
    for w in wells {
        if w.role == Some(Role::Empty) {
            continue;
        }
        if !groups.contains_key(&w.group) {
            order.push(w.group.clone());
        }
        groups.entry(w.group.clone()).or_default().push(w);
    }
    order
        .into_iter()
        .map(|g| {
            let ws = &groups[&g];
            let used: Vec<f64> = ws
                .iter()
                .filter(|w| !w.excluded)
                .filter_map(|w| val(w))
                .collect();
            let first = ws[0];
            let same = |f: &dyn Fn(&Well) -> Option<f64>| {
                let v = f(first);
                ws.iter().all(|w| f(w) == v).then_some(v).flatten()
            };
            SampleRow {
                group: g.clone(),
                role: role_id(first.role),
                sample: first.info.sample.clone(),
                compound: first.info.compound.clone(),
                concentration: same(&|w: &Well| w.info.concentration),
                dilution: same(&|w: &Well| w.info.dilution),
                wells: ws.iter().map(|w| w.name.clone()).collect(),
                metric: metric.into(),
                n: ws.len(),
                n_used: used.len(),
                mean: stats::mean(&used),
                sd: stats::sd(&used),
                cv_percent: stats::cv_percent(&used),
                outliers: ws
                    .iter()
                    .filter(|w| w.outlier)
                    .map(|w| w.name.clone())
                    .collect(),
                ..SampleRow::default()
            }
        })
        .collect()
}

fn curve(
    out: &mut AssayOutput,
    wells: &[Well],
    rows: &mut [WellRow],
    samples: &mut [SampleRow],
    req: &AssayRequest,
    notes: &mut Vec<String>,
) -> Result<()> {
    let stds: Vec<&Well> = wells
        .iter()
        .filter(|w| {
            w.role == Some(Role::Standard)
                && !w.excluded
                && w.value.is_some()
                && w.info.concentration.is_some()
        })
        .collect();
    if stds.is_empty() {
        let named = wells
            .iter()
            .filter(|w| w.role == Some(Role::Standard))
            .count();
        let hint = if named > 0 {
            format!(
                "{named} wells are standards but have no concentration: give concentrations in a layout (`concentration` column or block) or with --standard WELLS=CONC"
            )
        } else {
            "no standard wells: give a layout with role standard and a concentration, or --standard A1,A2=100;B1,B2=50;…".into()
        };
        return Err(usage(hint));
    }
    let model = req.model.unwrap_or(Model::FourPl);
    // points
    let (x, y, labels): (Vec<f64>, Vec<f64>, Vec<String>) = match req.fit_on {
        FitOn::Replicates => {
            let mut v = (Vec::new(), Vec::new(), Vec::new());
            for w in &stds {
                v.0.push(w.info.concentration.unwrap_or(f64::NAN));
                v.1.push(w.value.unwrap_or(f64::NAN));
                v.2.push(w.name.clone());
            }
            v
        }
        FitOn::Means => {
            let mut by: BTreeMap<u64, (f64, Vec<f64>, String)> = BTreeMap::new();
            for w in &stds {
                let c = w.info.concentration.unwrap_or(f64::NAN);
                let e = by
                    .entry(c.to_bits())
                    .or_insert_with(|| (c, Vec::new(), w.group.clone()));
                e.1.extend(w.value);
            }
            let mut v = (Vec::new(), Vec::new(), Vec::new());
            let mut lv: Vec<_> = by.into_values().collect();
            lv.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (c, ys, g) in lv {
                if let Some(m) = stats::mean(&ys) {
                    v.0.push(c);
                    v.1.push(m);
                    v.2.push(g);
                }
            }
            v
        }
    };
    let f = fit::fit(model, &x, &y, req.weighting)
        .map_err(|e| Error::Usage(format!("standard curve: {e}")))?;
    if !f.converged {
        notes.push(
            "the curve fit stopped before converging; check the standards and the model".into(),
        );
    }
    let mut levels: Vec<f64> = x.clone();
    levels.sort_by(f64::total_cmp);
    levels.dedup();
    let points: Vec<CurvePoint> = x
        .iter()
        .zip(&y)
        .zip(&labels)
        .map(|((&xi, &yi), l)| {
            let fitted = f.eval(xi);
            CurvePoint {
                label: l.clone(),
                x: xi,
                y: yi,
                fitted,
                residual: yi - fitted,
            }
        })
        .collect();
    let (lo, hi) = (levels[0], levels[levels.len() - 1]);
    // back-calculation
    let mut back: BTreeMap<String, Option<f64>> = BTreeMap::new();
    for (w, r) in wells.iter().zip(rows.iter_mut()) {
        if w.role == Some(Role::Empty) {
            continue;
        }
        let Some(v) = w.value else { continue };
        let (bc, flag) = range_flag(f.inverse(v), lo, hi);
        r.back_calculated = bc;
        r.flag = Some(flag.into());
        r.final_concentration = bc.map(|c| c * w.info.dilution.unwrap_or(1.0));
        if w.role == Some(Role::Standard)
            && let (Some(c), Some(nom)) = (bc, w.info.concentration)
            && nom > 0.0
        {
            r.recovery_percent = Some(100.0 * c / nom);
        }
        back.insert(w.name.clone(), bc);
    }
    // LLOQ / ULOQ
    let mut acceptable: Vec<f64> = Vec::new();
    for s in samples.iter().filter(|s| s.role == "standard") {
        let Some(nom) = s.concentration.filter(|c| *c > 0.0) else {
            continue;
        };
        let bcs: Vec<f64> = s
            .wells
            .iter()
            .filter(|n| !wells.iter().any(|w| &w.name == *n && w.excluded))
            .filter_map(|n| back.get(n).copied().flatten())
            .collect();
        let (Some(m), cv) = (stats::mean(&bcs), stats::cv_percent(&bcs)) else {
            continue;
        };
        if bcs.len() == s.n_used
            && (100.0 * m / nom - 100.0).abs() <= LOQ_TOLERANCE_PERCENT
            && cv.is_none_or(|c| c <= LOQ_TOLERANCE_PERCENT)
        {
            acceptable.push(nom);
        }
    }
    acceptable.sort_by(f64::total_cmp);
    let (lloq, uloq, rule) = match (req.lloq, req.uloq) {
        (None, None) => (
            acceptable.first().copied(),
            acceptable.last().copied(),
            format!(
                "lowest and highest standard (concentration > 0) whose back-calculated mean is within 100 ± {LOQ_TOLERANCE_PERCENT} % of nominal with CV ≤ {LOQ_TOLERANCE_PERCENT} %"
            ),
        ),
        (l, u) => (
            l.or(acceptable.first().copied()),
            u.or(acceptable.last().copied()),
            "given (--lloq/--uloq); the other limit from the recovery rule".into(),
        ),
    };
    for (w, r) in wells.iter().zip(rows.iter_mut()) {
        if let (Some(c), false) = (r.back_calculated, w.role == Some(Role::Empty)) {
            r.quantifiable = Some(
                lloq.is_some_and(|l| c >= l * (1.0 - 1e-12))
                    && uloq.is_some_and(|u| c <= u * (1.0 + 1e-12)),
            );
        }
    }
    // per group
    for s in samples.iter_mut() {
        if s.role == "empty" {
            continue;
        }
        let bcs: Vec<f64> = s
            .wells
            .iter()
            .filter(|n| !wells.iter().any(|w| &w.name == *n && w.excluded))
            .filter_map(|n| back.get(n).copied().flatten())
            .collect();
        s.back_calculated_mean = stats::mean(&bcs);
        s.back_calculated_sd = stats::sd(&bcs);
        s.back_calculated_cv_percent = stats::cv_percent(&bcs);
        if let Some(m) = s.mean {
            let (bc, flag) = range_flag(f.inverse(m), lo, hi);
            s.back_calculated_of_mean = bc;
            s.flag = Some(flag.into());
        }
        s.final_concentration = s
            .back_calculated_mean
            .map(|c| c * s.dilution.unwrap_or(1.0));
        if s.role == "standard"
            && let (Some(m), Some(nom)) = (s.back_calculated_mean, s.concentration)
            && nom > 0.0
        {
            s.recovery_percent = Some(100.0 * m / nom);
        }
    }
    if wells.iter().any(|w| w.info.dilution.is_some()) {
        notes.push("final_concentration = back_calculated × dilution".into());
    }
    out.curve = Some(CurveReport {
        fit: fit_report(&f, req.confidence, req.weighting, points, levels.len()),
        fit_on: match req.fit_on {
            FitOn::Replicates => "replicates",
            FitOn::Means => "means",
        }
        .into(),
        range_low: lo,
        range_high: hi,
        lloq,
        uloq,
        loq_rule: rule,
        concentration_unit: out.layout.concentration_unit.clone(),
    });
    Ok(())
}

fn dose_response(
    out: &mut AssayOutput,
    wells: &[Well],
    rows: &mut [WellRow],
    req: &AssayRequest,
    notes: &mut Vec<String>,
) -> Result<()> {
    let model = req.model.unwrap_or(Model::FourPl);
    // normalisation
    let mut inhibitory = None;
    let norm: Option<(f64, f64)> = match req.normalize {
        Normalize::None => None,
        Normalize::Controls => {
            let mean_of = |r: Role| {
                let v: Vec<f64> = wells
                    .iter()
                    .filter(|w| w.role == Some(r) && !w.excluded)
                    .filter_map(|w| w.value)
                    .collect();
                stats::mean(&v)
            };
            let (Some(p), Some(n)) = (mean_of(Role::Positive), mean_of(Role::Negative)) else {
                return Err(usage(
                    "--normalize controls needs positive (full effect) and negative (no effect, vehicle) control wells",
                ));
            };
            if p == n {
                return Err(usage(
                    "the positive and negative controls have the same mean",
                ));
            }
            inhibitory = Some(p < n);
            notes.push(format!(
                "percent effect = 100 × (value − {}) / ({} − {}) (negative control = 0 %, positive control = 100 %)",
                fmt_num(n),
                fmt_num(p),
                fmt_num(n)
            ));
            Some((n, p))
        }
    };
    let pct = |v: f64| norm.map(|(n, p)| 100.0 * (v - n) / (p - n));
    for (w, r) in wells.iter().zip(rows.iter_mut()) {
        if norm.is_some() {
            r.percent_effect = w.value.and_then(pct);
        }
    }
    // series per compound
    let mut series: BTreeMap<String, Vec<&Well>> = BTreeMap::new();
    let mut order = Vec::new();
    for w in wells {
        if w.excluded || w.info.concentration.is_none() || w.value.is_none() {
            continue;
        }
        if !matches!(w.role, Some(Role::Sample) | None) {
            continue;
        }
        let name = w
            .info
            .compound
            .clone()
            .or_else(|| w.info.sample.clone())
            .unwrap_or_else(|| "all".into());
        if !series.contains_key(&name) {
            order.push(name.clone());
        }
        series.entry(name).or_default().push(w);
    }
    if series.is_empty() {
        return Err(usage(
            "no dose wells: give a layout with role sample (or a compound column) and a concentration per well",
        ));
    }
    for name in order {
        let ws = &series[&name];
        let x: Vec<f64> = ws
            .iter()
            .map(|w| w.info.concentration.unwrap_or(f64::NAN))
            .collect();
        let y: Vec<f64> = ws
            .iter()
            .map(|w| {
                let v = w.value.unwrap_or(f64::NAN);
                pct(v).unwrap_or(v)
            })
            .collect();
        let mut levels = x.clone();
        levels.sort_by(f64::total_cmp);
        levels.dedup();
        let pos: Vec<f64> = levels.iter().copied().filter(|c| *c > 0.0).collect();
        let mut row = CompoundRow {
            compound: name.clone(),
            kind: "EC50".into(),
            n_concentrations: levels.len(),
            min_concentration: pos.first().copied().unwrap_or(0.0),
            max_concentration: levels.last().copied().unwrap_or(0.0),
            ..CompoundRow::default()
        };
        match fit::fit(model, &x, &y, req.weighting) {
            Err(e) => row.error = Some(e.0),
            Ok(f) => {
                let points = ws
                    .iter()
                    .zip(x.iter().zip(&y))
                    .map(|(w, (&xi, &yi))| {
                        let fitted = f.eval(xi);
                        CurvePoint {
                            label: w.name.clone(),
                            x: xi,
                            y: yi,
                            fitted,
                            residual: yi - fitted,
                        }
                    })
                    .collect();
                let rep = fit_report(&f, req.confidence, req.weighting, points, levels.len());
                if model == Model::Linear {
                    row.error = Some("dose-response needs a 4pl or 5pl model".into());
                } else {
                    let (a, b, c, d) = (f.params[0], f.params[1], f.params[2], f.params[3]);
                    let increasing = (d - a) * b > 0.0;
                    let (at0, atinf) = if b > 0.0 { (a, d) } else { (d, a) };
                    row.ec50 = Some(c);
                    row.ec50_se = f.se(2);
                    row.log10_ec50 = Some(c.log10());
                    let t = stats::t_quantile(0.5 + req.confidence / 2.0, f.df as f64);
                    if let (Some(s), Some(t)) = (f.se_ln_c(), t) {
                        row.ec50_ci_low = Some((c.ln() - t * s).exp());
                        row.ec50_ci_high = Some((c.ln() + t * s).exp());
                    }
                    row.hill_slope = Some(if increasing { b.abs() } else { -b.abs() });
                    row.top = Some(a.max(d));
                    row.bottom = Some(a.min(d));
                    row.response_at_zero = Some(at0);
                    row.response_at_infinity = Some(atinf);
                    row.kind = match inhibitory {
                        Some(true) => "IC50",
                        Some(false) => "EC50",
                        None if increasing => "EC50",
                        None => "IC50",
                    }
                    .into();
                    if norm.is_some() {
                        row.absolute_ec50 = f.inverse(50.0).ok();
                    }
                    row.extrapolated = pos.first().is_some_and(|lo| c < *lo)
                        || levels.last().is_some_and(|hi| c > *hi);
                    if !f.converged {
                        row.error = Some("the fit stopped before converging".into());
                    }
                }
                row.fit = Some(rep);
            }
        }
        out.compounds.push(row);
    }
    Ok(())
}

fn time_course(
    out: &mut AssayOutput,
    wells: &mut [Well],
    series: &Series,
    req: &AssayRequest,
    window: Option<usize>,
    notes: &mut Vec<String>,
) -> Result<()> {
    for w in wells.iter_mut() {
        if w.role == Some(Role::Empty) {
            w.excluded = true;
        }
    }
    // blank per time point
    let mut blank_at: BTreeMap<u64, f64> = BTreeMap::new();
    if let Some((method, blanks)) = blank_wells_for(wells, req.blank)? {
        let mut by_t: BTreeMap<u64, Vec<f64>> = BTreeMap::new();
        for b in &blanks {
            for (t, v) in &series[&b.pos] {
                if v.is_finite() {
                    by_t.entry(time_key(*t)).or_default().push(*v);
                }
            }
        }
        for (k, v) in &by_t {
            if let Some(c) = center(method, v) {
                blank_at.insert(*k, c);
            }
        }
        let all: Vec<f64> = blank_at.values().copied().collect();
        out.blank = Some(BlankSummary {
            method: method.into(),
            wells: blanks.iter().map(|w| w.name.clone()).collect(),
            n: blanks.len(),
            value: stats::mean(&all).unwrap_or(0.0),
            sd: None,
            per_time_point: true,
        });
    }
    let mut missing_blank = 0usize;
    let mut kin_by_well: BTreeMap<String, f64> = BTreeMap::new();
    for w in wells.iter() {
        if w.excluded {
            continue;
        }
        let pts = &series[&w.pos];
        let t: Vec<f64> = pts.iter().map(|p| p.0).collect();
        let y: Vec<f64> = pts
            .iter()
            .map(|(ti, v)| {
                if blank_at.is_empty() {
                    *v
                } else if let Some(b) = blank_at.get(&time_key(*ti)) {
                    v - b
                } else {
                    missing_blank += 1;
                    f64::NAN
                }
            })
            .collect();
        // growth without blank wells: subtract the well's own minimum (growthcurver's default
        // background correction), unless blank subtraction is off
        let mut background = None;
        let y = if req.analysis == Analysis::Growth
            && blank_at.is_empty()
            && req.blank != BlankMode::None
        {
            let m = y
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .fold(f64::INFINITY, f64::min);
            if m.is_finite() {
                background = Some(m);
                y.iter().map(|v| v - m).collect()
            } else {
                y
            }
        } else {
            y
        };
        match req.analysis {
            Analysis::Kinetics => {
                if let Some(k) = kinetics::kinetic(&t, &y, window) {
                    kin_by_well.insert(w.name.clone(), k.max_slope_per_s * 60.0);
                    out.kinetics.push(KineticRow {
                        well: w.name.clone(),
                        role: role_id(w.role),
                        sample: w.info.sample.clone(),
                        n_points: k.n_points,
                        window: k.window,
                        max_slope_per_min: k.max_slope_per_s * 60.0,
                        max_slope_per_s: k.max_slope_per_s,
                        max_slope_r_squared: k.max_slope_r_squared,
                        time_at_max_slope_s: k.time_at_max_slope_s,
                        window_start_s: k.window_start_s,
                        window_end_s: k.window_end_s,
                        lag_time_s: k.lag_time_s,
                        mean_slope_per_min: k.mean_slope_per_s * 60.0,
                        mean_slope_r_squared: k.mean_slope_r_squared,
                        max_value: k.max_value,
                        time_to_max_s: k.time_to_max_s,
                        min_value: k.min_value,
                        initial_value: k.initial_value,
                        final_value: k.final_value,
                        auc: k.auc,
                    });
                }
            }
            _ => {
                if let Some(g) = kinetics::growth(&t, &y, window, req.growth_threshold) {
                    if let Some(d) = g.doubling_time_h {
                        kin_by_well.insert(w.name.clone(), d);
                    }
                    let l = g.logistic.as_ref();
                    out.growth.push(GrowthRow {
                        well: w.name.clone(),
                        role: role_id(w.role),
                        sample: w.info.sample.clone(),
                        n_points: g.n_points,
                        background,
                        window: g.window,
                        threshold: g.threshold,
                        growth_rate_per_h: g.growth_rate_per_h,
                        doubling_time_h: g.doubling_time_h,
                        doubling_time_min: g.doubling_time_h.map(|h| h * 60.0),
                        exp_phase_start_s: g.exp_phase_start_s,
                        exp_phase_end_s: g.exp_phase_end_s,
                        exp_r_squared: g.exp_r_squared,
                        lag_time_s: g.lag_time_s,
                        max_value: g.max_value,
                        time_to_max_s: g.time_to_max_s,
                        auc_h: g.auc_h,
                        logistic_k: l.map(|l| l.k),
                        logistic_n0: l.map(|l| l.n0),
                        logistic_r_per_h: l.map(|l| l.r_per_h),
                        logistic_doubling_time_h: l.map(|l| l.doubling_time_h),
                        logistic_t_mid_s: l.map(|l| l.t_mid_s),
                        logistic_sigma: l.map(|l| l.sigma),
                    });
                }
            }
        }
    }
    if missing_blank > 0 {
        notes.push(format!(
            "{missing_blank} time points had no blank value at the same time and were left out"
        ));
    }
    let metric = if req.analysis == Analysis::Kinetics {
        "max_slope_per_min"
    } else {
        "doubling_time_h"
    };
    out.samples = sample_rows(wells, |w| kin_by_well.get(&w.name).copied(), metric)
        .into_iter()
        .filter(|s| s.n >= 2)
        .collect();
    Ok(())
}

fn time_key(t: f64) -> u64 {
    // times as written are rounded to the millisecond for matching across wells
    ((t * 1000.0).round()).to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Obs;

    fn plate(values: &[(&str, f64)]) -> PlateData {
        PlateData {
            format: "test".into(),
            rows: 8,
            cols: 12,
            reads: vec![ReadInfo {
                number: 1,
                label: "450".into(),
                mode: None,
                unit: None,
                calculated: false,
            }],
            obs: values
                .iter()
                .map(|(w, v)| {
                    let (row, col) = layout::parse_well(w).unwrap();
                    Obs {
                        row,
                        col,
                        read: 1,
                        wavelength_nm: None,
                        time_s: None,
                        value: *v,
                    }
                })
                .collect(),
            ..PlateData::default()
        }
    }

    #[test]
    fn linear_curve_with_blank_and_back_calculation() {
        // y = 0.01 x + 0.05 (+ blank 0.05)
        let mut v = vec![("H1", 0.05), ("H2", 0.05)];
        let concs = [0.0, 10.0, 20.0, 40.0, 80.0];
        let names = ["A1", "A2", "A3", "A4", "A5", "B1", "B2", "B3", "B4", "B5"];
        for (i, c) in concs.iter().enumerate() {
            v.push((names[i], 0.05 + 0.01 * c + 0.001));
            v.push((names[i + 5], 0.05 + 0.01 * c - 0.001));
        }
        v.push(("C1", 0.05 + 0.01 * 33.0));
        let p = plate(&v);
        let req = AssayRequest {
            analysis: Analysis::Curve,
            model: Some(Model::Linear),
            blank_wells: Some("H1,H2".into()),
            standards: Some("A1,B1=0;A2,B2=10;A3,B3=20;A4,B4=40;A5,B5=80".into()),
            ..AssayRequest::default()
        };
        let out = run(&p, &req, user_layout(&req).unwrap()).unwrap();
        let c1 = out.wells.iter().find(|w| w.well == "C1").unwrap();
        assert!((c1.back_calculated.unwrap() - 33.0).abs() < 1e-9, "{c1:?}");
        assert_eq!(c1.flag.as_deref(), Some("in_range"));
        assert_eq!(out.blank.as_ref().unwrap().n, 2);
        let curve = out.curve.unwrap();
        assert!((curve.fit.parameters[0].value - 0.01).abs() < 1e-12);
        assert_eq!(curve.lloq, Some(10.0));
        assert_eq!(curve.uloq, Some(80.0));
        assert_eq!(
            out.samples
                .iter()
                .find(|s| s.group == "standard 10")
                .unwrap()
                .n,
            2
        );
    }

    #[test]
    fn kinetic_read_needs_reduce() {
        let mut p = plate(&[("A1", 1.0)]);
        p.obs = (0..5)
            .map(|i| Obs {
                row: 0,
                col: 0,
                read: 1,
                wavelength_nm: None,
                time_s: Some(f64::from(i) * 60.0),
                value: f64::from(i),
            })
            .collect();
        let req = AssayRequest::default();
        assert!(matches!(run(&p, &req, None), Err(Error::Usage(_))));
        let req = AssayRequest {
            reduce: Some(Reduce::MeanSlope),
            ..AssayRequest::default()
        };
        let out = run(&p, &req, None).unwrap();
        assert!((out.wells[0].value.unwrap() - 1.0).abs() < 1e-12);
        let req = AssayRequest {
            analysis: Analysis::Kinetics,
            window: Some(3),
            ..AssayRequest::default()
        };
        let out = run(&p, &req, None).unwrap();
        assert!((out.kinetics[0].max_slope_per_min - 1.0).abs() < 1e-12);
    }

    #[test]
    fn z_prime_and_outliers() {
        let p = plate(&[
            ("A1", 100.0),
            ("A2", 102.0),
            ("A3", 98.0),
            ("A4", 100.0),
            ("B1", 10.0),
            ("B2", 11.0),
            ("B3", 9.0),
            ("B4", 10.0),
            ("C1", 50.0),
            ("C2", 51.0),
            ("C3", 200.0),
            ("C4", 49.0),
        ]);
        let req = AssayRequest {
            analysis: Analysis::Qc,
            positive_wells: Some("A1:A4".into()),
            negative_wells: Some("B1:B4".into()),
            layout_text: Some("well,sample\nC1,s\nC2,s\nC3,s\nC4,s\n".into()),
            ..AssayRequest::default()
        };
        let out = run(&p, &req, user_layout(&req).unwrap()).unwrap();
        let q = out.quality.unwrap();
        let (sp, sn) = (
            stats::sd(&[100.0, 102.0, 98.0, 100.0]).unwrap(),
            stats::sd(&[10.0, 11.0, 9.0, 10.0]).unwrap(),
        );
        assert!((q.z_prime.unwrap() - (1.0 - 3.0 * (sp + sn) / 90.0)).abs() < 1e-12);
        assert!((q.signal_to_background.unwrap() - 10.0).abs() < 1e-12);
        assert_eq!(out.outliers.flagged, vec!["C3".to_string()]);
        // no controls → usage error
        let req = AssayRequest {
            analysis: Analysis::Qc,
            ..AssayRequest::default()
        };
        assert!(run(&p, &req, None).is_err());
    }

    #[test]
    fn dose_response_ic50() {
        let doses = [0.001, 0.01, 0.1, 1.0, 10.0, 100.0, 1000.0];
        let mut text = String::from("well,compound,concentration\n");
        let mut v = Vec::new();
        for (i, d) in doses.iter().enumerate() {
            for r in 0..2u32 {
                let w = layout::well_name(r, i as u32);
                let y =
                    5.0 + 95.0 / (1.0 + (d / 2.5f64).powf(1.2)) + if r == 0 { 0.2 } else { -0.2 };
                v.push((w.clone(), y));
                text.push_str(&format!("{w},X,{d}\n"));
            }
        }
        let vals: Vec<(&str, f64)> = v.iter().map(|(w, y)| (w.as_str(), *y)).collect();
        let p = plate(&vals);
        let req = AssayRequest {
            analysis: Analysis::DoseResponse,
            layout_text: Some(text),
            ..AssayRequest::default()
        };
        let out = run(&p, &req, user_layout(&req).unwrap()).unwrap();
        let c = &out.compounds[0];
        assert_eq!(c.kind, "IC50");
        assert!((c.ec50.unwrap() - 2.5).abs() < 0.05, "{c:?}");
        assert!(c.hill_slope.unwrap() < 0.0);
        assert!(c.ec50_ci_low.unwrap() < 2.5 && c.ec50_ci_high.unwrap() > 2.5);
    }
}
