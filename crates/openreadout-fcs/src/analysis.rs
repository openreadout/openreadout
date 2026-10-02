//! What `openreadout analyze gate` and `table --compensate/--transform/--workspace/--gatingml` do:
//! read a gating file, bind it to an FCS data set, stream the events through the evaluator,
//! and build the JSON shapes in [`openreadout_core::flow`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::flow::{
    AppliedTransform, CompensationSummary, GateColumn, GateDimension, GateOutput, GroupSummary,
    PopulationNode, PopulationRow, SampleSummary, TableProcessing, TransformSummary,
    WorkspaceSummary,
};
use openreadout_core::model::TableSlice;
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::FORMAT_ID;
use crate::dataset::FcsDataset;
use crate::events::{file_matrix, is_fluorescence_parameter, parameter_names, scale_columns};
use crate::file::DataSet;
use crate::gating::{
    CompMatrix, CompensationRef, CoordinateSpace, Evaluator, GATING_ML_FORMAT_ID, GatingFile,
    Population, Shape, Strategy, Transform, WSP_FORMAT_ID, Workspace, read_gating_file,
};

/// Events read per block while gating a whole data set.
const CHUNK_ROWS: u64 = 1 << 16;

/// What to gate.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct GateRequest {
    /// FlowJo workspace or Gating-ML file.
    pub gating_file: PathBuf,
    /// FCS file to count events in; `None` describes the gating file only.
    pub fcs: Option<PathBuf>,
    /// Workspace sample (name or id); default: matched by file name, `$FIL`, or the only one.
    pub sample: Option<String>,
    /// FCS data set index.
    pub table: u32,
    /// Report only these populations (paths or unique names) and their descendants.
    pub populations: Vec<String>,
    /// Parameters (`$PnN` or `$PnS`; with the workspace's `Comp-` prefix for compensated
    /// values) whose median over each population's events is reported.
    pub medians: Vec<String>,
}

impl GateRequest {
    /// A request for `gating_file`, optionally applied to `fcs`.
    pub fn new(gating_file: impl Into<PathBuf>, fcs: Option<PathBuf>) -> Self {
        GateRequest {
            gating_file: gating_file.into(),
            fcs,
            ..Default::default()
        }
    }
}

/// A gating file with the strategy chosen for one FCS data set.
struct Bound {
    file: GatingFile,
    format: &'static str,
    sample: Option<usize>,
}

impl Bound {
    fn strategy(&self) -> Option<&Strategy> {
        match &self.file {
            GatingFile::GatingMl(s) => Some(s),
            GatingFile::Workspace(w) => self
                .sample
                .and_then(|i| w.samples.get(i))
                .map(|s| &s.strategy),
        }
    }

    fn workspace(&self) -> Option<&Workspace> {
        match &self.file {
            GatingFile::Workspace(w) => Some(w),
            GatingFile::GatingMl(_) => None,
        }
    }
}

fn bind(gating_file: &Path, sample: Option<&str>, fcs: Option<(&Path, &DataSet)>) -> Result<Bound> {
    let file = read_gating_file(gating_file)?;
    let (format, sample) = match &file {
        GatingFile::GatingMl(_) => (GATING_ML_FORMAT_ID, None),
        GatingFile::Workspace(w) => {
            let idx = match fcs {
                Some((p, ds)) => Some(
                    match w.select_sample(
                        sample,
                        p.file_name().and_then(|n| n.to_str()),
                        ds.keyword("$FIL"),
                    ) {
                        Ok(i) => i,
                        // a renamed file: match its acquisition keywords ($TOT, $BTIM, …)
                        Err(e) if sample.is_none() => w
                            .select_sample_by_keywords(&|k| ds.keyword(k).map(str::to_string))
                            .ok_or(e)?,
                        Err(e) => return Err(e),
                    },
                ),
                None => match sample {
                    Some(_) => Some(w.select_sample(sample, None, None)?),
                    None => (w.samples.len() == 1).then_some(0),
                },
            };
            (WSP_FORMAT_ID, idx)
        }
    };
    Ok(Bound {
        file,
        format,
        sample,
    })
}

fn data_set(ds: &FcsDataset, table: u32) -> Result<DataSet> {
    ds.data_sets().get(table as usize).cloned().ok_or_else(|| {
        Error::Usage(format!(
            "table {table} out of range (the file has {} data set(s))",
            ds.data_sets().len()
        ))
    })
}

fn transform_summary(id: &str, t: &Transform) -> TransformSummary {
    TransformSummary {
        id: id.to_string(),
        kind: t.kind().to_string(),
        parameters: t
            .parameters()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        supported: true,
    }
}

fn matrix_summary(m: &CompMatrix, source: &str, used: bool) -> CompensationSummary {
    CompensationSummary {
        name: m.name.clone(),
        source: source.to_string(),
        detectors: m.detectors.clone(),
        fluorochromes: m.fluorochromes.clone(),
        matrix: m.values.clone(),
        spectral: m.spectral,
        used,
    }
}

fn gate_json(s: &Strategy, p: &Population) -> Value {
    match &p.shape {
        Shape::Rectangle => json!({}),
        Shape::Polygon { vertices } => json!({ "vertices": vertices }),
        Shape::Ellipsoid {
            mean,
            covariance,
            distance_square,
        } => json!({ "mean": mean, "covariance": covariance, "distance_square": distance_square }),
        Shape::DisplayEllipse { foci, edge } => {
            json!({ "foci": foci, "edge": edge, "display_bins": 256 })
        }
        Shape::Boolean { op, operands } => json!({
            "op": op.name(),
            "operands": operands.iter().map(|o| json!({
                "population": s.populations.get(o.population).map(Population::path_string),
                "complement": o.complement,
            })).collect::<Vec<_>>(),
        }),
    }
}

fn population_rows(
    s: &Strategy,
    keep: &[usize],
    counts: Option<&[u64]>,
    total: Option<u64>,
    medians: &[BTreeMap<String, f64>],
) -> Vec<PopulationRow> {
    keep.iter()
        .map(|&i| {
            let p = &s.populations[i];
            let count = counts.map(|c| c[i]);
            let parent_count = match p.parent {
                Some(par) => counts.map(|c| c[par]),
                None => total,
            };
            let pct = |n: Option<u64>, d: Option<u64>| match (n, d) {
                (Some(n), Some(d)) if d > 0 => Some(n as f64 * 100.0 / d as f64),
                (Some(_), Some(_)) => Some(0.0),
                _ => None,
            };
            PopulationRow {
                path: p.path_string(),
                name: p.name.clone(),
                parent: p.parent.map(|x| s.populations[x].path_string()),
                gate_type: p.kind.name().to_string(),
                dimensions: p
                    .dimensions
                    .iter()
                    .map(|d| GateDimension {
                        parameter: d.name().to_string(),
                        compensation: d.compensation.label().to_string(),
                        transform: d.transform.clone(),
                        min: d.min,
                        max: d.max,
                    })
                    .collect(),
                gate: gate_json(s, p),
                complement: p.complement,
                coordinates: match p.coordinates {
                    CoordinateSpace::Transformed => "transformed",
                    CoordinateSpace::Untransformed => "untransformed",
                }
                .to_string(),
                count,
                percent_of_parent: pct(count, parent_count),
                percent_of_total: pct(count, total),
                stored_count: p.stored_count,
                medians: medians.get(i).cloned().unwrap_or_default(),
            }
        })
        .collect()
}

/// Build the tree from rows (their paths); quadrant gates become grouping nodes.
fn tree(rows: &[PopulationRow], s: &Strategy, keep: &[usize]) -> Vec<PopulationNode> {
    let mut roots: Vec<PopulationNode> = Vec::new();
    for (row, &i) in rows.iter().zip(keep) {
        let p = &s.populations[i];
        let mut level = &mut roots;
        let mut prefix = String::new();
        let n = p.path.len();
        for (k, seg) in p.path.iter().enumerate() {
            prefix.push('/');
            prefix.push_str(seg);
            let last = k + 1 == n;
            let pos = level
                .iter()
                .position(|c| c.name == *seg && c.path == prefix);
            let idx = if let Some(x) = pos {
                x
            } else {
                {
                    level.push(PopulationNode {
                        name: seg.clone(),
                        path: prefix.clone(),
                        gate_type: if last {
                            row.gate_type.clone()
                        } else if p.quadrant_gate.as_deref() == Some(seg.as_str()) && k + 2 == n {
                            "quadrant-gate".into()
                        } else {
                            String::new()
                        },
                        count: None,
                        children: Vec::new(),
                    });
                    level.len() - 1
                }
            };
            if last {
                level[idx].gate_type = row.gate_type.clone();
                level[idx].count = row.count;
            }
            level = &mut level[idx].children;
        }
    }
    roots
}

fn keep_indices(s: &Strategy, wanted: &[String]) -> Result<Vec<usize>> {
    if wanted.is_empty() {
        return Ok((0..s.populations.len()).collect());
    }
    let mut keep = vec![false; s.populations.len()];
    for w in wanted {
        let i = s.find(w).ok_or_else(|| {
            Error::Usage(format!(
                "population '{w}' is not in the gating file (by path or unique name); populations: {}",
                s.populations
                    .iter()
                    .map(Population::path_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        for j in s.subtree(i) {
            keep[j] = true;
        }
    }
    Ok((0..keep.len()).filter(|&i| keep[i]).collect())
}

/// A parameter whose median `analyze gate --median` reports.
struct MedianParam {
    /// As asked.
    name: String,
    /// Column in the data set.
    column: usize,
    /// Compensate with this matrix first.
    matrix: Option<CompMatrix>,
}

/// Resolve `analyze gate --median` names: `$PnN` or `$PnS`, optionally behind the compensation
/// prefix.
fn median_params(
    wanted: &[String],
    names: &[String],
    labels: &[Option<String>],
    sample: Option<&crate::gating::WorkspaceSample>,
    strategy: &Strategy,
    file_m: Option<&CompMatrix>,
) -> Result<Vec<MedianParam>> {
    let find = |q: &str| {
        names
            .iter()
            .position(|n| n == q)
            .or_else(|| labels.iter().position(|l| l.as_deref() == Some(q)))
    };
    let prefix = sample
        .map(|s| s.matrix_prefix.as_str())
        .filter(|p| !p.is_empty())
        .unwrap_or("Comp-");
    let mut out = Vec::new();
    for w in wanted.iter().flat_map(|w| w.split(',')).map(str::trim) {
        if w.is_empty() {
            continue;
        }
        if let Some(c) = find(w) {
            out.push(MedianParam {
                name: w.to_string(),
                column: c,
                matrix: None,
            });
            continue;
        }
        let Some(column) = w.strip_prefix(prefix).and_then(find) else {
            return Err(Error::Usage(format!(
                "--median {w}: not a parameter of this file (parameters: {}; prefix compensated ones with `{prefix}`)",
                names.join(", ")
            )));
        };
        let matrix = sample
            .and_then(|s| s.matrix.clone())
            .or_else(|| {
                if strategy.matrices.len() == 1 {
                    strategy.matrices.values().next().cloned()
                } else {
                    None
                }
            })
            .or_else(|| file_m.cloned())
            .ok_or_else(|| {
                Error::unsupported(
                    FORMAT_ID,
                    format!("--median {w}: no compensation matrix to compensate with"),
                    "The workspace sample, the gating file and the FCS file define no matrix; ask for the uncompensated parameter instead.",
                )
            })?;
        out.push(MedianParam {
            name: w.to_string(),
            column,
            matrix: Some(matrix),
        });
    }
    Ok(out)
}

/// The median of `v` (the mean of the two middle values for an even count); NaN values are
/// dropped first. `None` when nothing is left. Reorders `v`.
pub fn median(v: &mut Vec<f64>) -> Option<f64> {
    v.retain(|x| !x.is_nan());
    let n = v.len();
    if n == 0 {
        return None;
    }
    let mid = n / 2;
    let (_, hi, _) = v.select_nth_unstable_by(mid, f64::total_cmp);
    let hi = *hi;
    if n % 2 == 1 {
        return Some(hi);
    }
    let lo = v[..mid].iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some(lo / 2.0 + hi / 2.0)
}

/// Population sizes and, for `params`, the median of each population's events.
type Counted = (Vec<u64>, u64, Vec<BTreeMap<String, f64>>);

/// Count events per population in a whole data set and, for `params`, collect each
/// population's values to report their medians.
fn count_populations(
    ds: &mut FcsDataset,
    set: &DataSet,
    table: u32,
    ev: &Evaluator,
    n_pops: usize,
    params: &[MedianParam],
) -> Result<Counted> {
    let total = set.event_count.unwrap_or(0);
    let mut counts = vec![0u64; n_pops];
    let (names, labels) = parameter_names(set);
    let mut values: Vec<Vec<Vec<f64>>> = vec![vec![Vec::new(); params.len()]; n_pops];
    let mut first = 0u64;
    while first < total {
        let t = ds.read_table(table, first, CHUNK_ROWS)?;
        let rows = t.columns.first().map_or(0, Vec::len) as u64;
        if rows == 0 {
            break;
        }
        let mut cols = t.columns;
        scale_columns(set, &mut cols);
        let m = ev.evaluate(&cols)?;
        for (c, mem) in counts.iter_mut().zip(&m) {
            *c += mem.iter().filter(|b| **b).count() as u64;
        }
        if !params.is_empty() {
            let mut compensated: Vec<(String, Vec<Vec<f64>>)> = Vec::new();
            for p in params {
                if let Some(mx) = &p.matrix
                    && !compensated.iter().any(|(n, _)| *n == mx.name)
                {
                    let mut c = cols.clone();
                    apply_matrix(mx, &mut c, &names, &labels)?;
                    compensated.push((mx.name.clone(), c));
                }
            }
            for (k, p) in params.iter().enumerate() {
                let source = match &p.matrix {
                    None => &cols,
                    Some(mx) => compensated
                        .iter()
                        .find(|(n, _)| *n == mx.name)
                        .map_or(&cols, |(_, c)| c),
                };
                let Some(col) = source.get(p.column) else {
                    continue;
                };
                for (pop, mem) in m.iter().enumerate() {
                    values[pop][k].extend(
                        col.iter()
                            .zip(mem)
                            .filter(|(_, inside)| **inside)
                            .map(|(v, _)| *v),
                    );
                }
            }
        }
        first += rows;
    }
    let medians = values
        .into_iter()
        .map(|per| {
            per.into_iter()
                .zip(params)
                .filter_map(|(mut v, p)| median(&mut v).map(|m| (p.name.clone(), m)))
                .collect()
        })
        .collect();
    Ok((counts, total, medians))
}

fn workspace_summary(w: &Workspace) -> WorkspaceSummary {
    WorkspaceSummary {
        version: w.version.clone(),
        flowjo_version: w.flowjo_version.clone(),
        modified: w.modified.clone(),
        groups: w
            .groups
            .iter()
            .map(|g| GroupSummary {
                name: g.name.clone(),
                samples: g
                    .sample_ids
                    .iter()
                    .map(|id| {
                        w.samples
                            .iter()
                            .find(|s| &s.id == id)
                            .map_or_else(|| id.clone(), |s| s.name.clone())
                    })
                    .collect(),
            })
            .collect(),
        samples: w.samples.iter().map(|s| sample_summary(s, false)).collect(),
    }
}

fn sample_summary(s: &crate::gating::WorkspaceSample, keywords: bool) -> SampleSummary {
    SampleSummary {
        id: s.id.clone(),
        name: s.name.clone(),
        uri: s.uri.clone(),
        event_count: s.event_count,
        groups: s.groups.clone(),
        population_count: s.strategy.populations.len(),
        compensation: s.matrix.as_ref().map(|m| m.name.clone()),
        keywords: if keywords {
            s.keywords.iter().cloned().collect()
        } else {
            BTreeMap::new()
        },
    }
}

/// Describe a gating file and, when an FCS file is given, count every population.
pub fn gate(req: &GateRequest) -> Result<GateOutput> {
    let mut fcs = match &req.fcs {
        Some(p) => Some((p.clone(), FcsDataset::open(p)?)),
        None => None,
    };
    let set = match &fcs {
        Some((_, ds)) => Some(data_set(ds, req.table)?),
        None => None,
    };
    let bound = bind(
        &req.gating_file,
        req.sample.as_deref(),
        fcs.as_ref()
            .zip(set.as_ref())
            .map(|((p, _), s)| (p.as_path(), s)),
    )?;
    let mut out = GateOutput {
        path: req.fcs.as_ref().map(|p| p.display().to_string()),
        gating_file: req.gating_file.display().to_string(),
        gating_format: bound.format.to_string(),
        workspace: bound.workspace().map(workspace_summary),
        table: req.fcs.as_ref().map(|_| req.table),
        event_count: set.as_ref().and_then(|s| s.event_count),
        ..Default::default()
    };
    if let (Some(w), Some(i)) = (bound.workspace(), bound.sample) {
        let s = &w.samples[i];
        out.sample = Some(sample_summary(s, true));
        out.notes
            .extend(s.skipped.iter().map(|m| format!("population skipped: {m}")));
    }
    let Some(strategy) = bound.strategy() else {
        if let Some(w) = bound.workspace() {
            out.notes.push(format!(
                "the workspace has {} samples; pass --sample NAME (or an FCS file) to list one sample's populations",
                w.samples.len()
            ));
        }
        return Ok(out);
    };
    let keep = keep_indices(strategy, &req.populations)?;
    let mut counts = None;
    let mut medians: Vec<BTreeMap<String, f64>> = Vec::new();
    let mut used: Vec<String> = Vec::new();
    let file_m = set.as_ref().and_then(file_matrix);
    if !req.medians.is_empty() && fcs.is_none() {
        return Err(Error::Usage(
            "--median needs an FCS file to compute population medians from".into(),
        ));
    }
    if let (Some((_, ds)), Some(set)) = (fcs.as_mut(), set.as_ref()) {
        let (names, labels) = parameter_names(set);
        let ev = Evaluator::new(strategy, &names, &labels, file_m.as_ref())?;
        used = ev
            .compensations_used
            .iter()
            .map(|(_, m, _)| m.clone())
            .collect();
        let ws_sample = match (bound.workspace(), bound.sample) {
            (Some(w), Some(i)) => w.samples.get(i),
            _ => None,
        };
        let params = median_params(
            &req.medians,
            &names,
            &labels,
            ws_sample,
            strategy,
            file_m.as_ref(),
        )?;
        let (c, total, m) =
            count_populations(ds, set, req.table, &ev, strategy.populations.len(), &params)?;
        medians = m;
        for mx in params.iter().filter_map(|p| p.matrix.as_ref()) {
            if !used.contains(&mx.name) {
                used.push(mx.name.clone());
            }
        }
        if !params.is_empty() {
            out.notes.push(format!(
                "medians are of scale values{}",
                if params.iter().any(|p| p.matrix.is_some()) {
                    "; `Comp-` parameters are compensated with the matrix marked used"
                } else {
                    ""
                }
            ));
        }
        counts = Some(c);
        out.event_count = Some(total);
        let mut scale = scale_columns(set, &mut vec![vec![0.0]; set.parameters.len()]);
        scale.dedup();
        if !scale.is_empty() {
            out.notes.push(format!(
                "gates are applied to FCS scale values: {}",
                scale.join("; ")
            ));
        }
    }
    let source = if bound.format == WSP_FORMAT_ID {
        "workspace"
    } else {
        "gating-ml"
    };
    out.compensation = strategy
        .matrices
        .values()
        .map(|m| matrix_summary(m, source, used.contains(&m.name)))
        .collect();
    if let Some(m) = &file_m
        && strategy.populations.iter().any(|p| {
            p.dimensions
                .iter()
                .any(|d| d.compensation == CompensationRef::FromFile)
        })
    {
        out.compensation
            .push(matrix_summary(m, "fcs", used.contains(&m.name)));
    }
    out.transforms = strategy
        .transforms
        .iter()
        .map(|(id, t)| transform_summary(id, t))
        .chain(
            strategy
                .unsupported_transforms
                .iter()
                .map(|(id, what)| TransformSummary {
                    id: id.clone(),
                    kind: what.clone(),
                    parameters: BTreeMap::new(),
                    supported: false,
                }),
        )
        .collect();
    out.populations = population_rows(
        strategy,
        &keep,
        counts.as_deref(),
        out.event_count.filter(|_| counts.is_some()),
        &medians,
    );
    out.tree = tree(&out.populations, strategy, &keep);
    if counts.is_some() {
        let differ: Vec<String> = out
            .populations
            .iter()
            .filter_map(|r| match (r.count, r.stored_count) {
                (Some(c), Some(s)) if c != s => Some(format!("{} ({c} vs {s})", r.path)),
                _ => None,
            })
            .collect();
        if !differ.is_empty() {
            out.notes.push(format!(
                "{} population(s) differ from the counts stored in the gating file (ours vs stored): {}",
                differ.len(),
                differ.iter().take(20).cloned().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    Ok(out)
}

/// Parse a transform given on the command line: `NAME` or `NAME:K=V,K=V`.
///
/// Names and defaults: `linear` (T=262144, A=0), `log` (T=262144, M=4.5), `arcsinh` / `asinh`
/// (Gating-ML fasinh: T=262144, M=4.5, A=0), `logicle` (T=262144, W=0.5, M=4.5, A=0),
/// `hyperlog` (T=262144, W=0.5, M=4.5, A=0), `biex` (FlowJo: neg=0, width=-10,
/// pos=4.41854, maxRange=262144), `flowjo-log` (offset=1, decades=4.5),
/// `arcsinh-cofactor` (cofactor=5; `arcsinh-cofactor:150` also accepted).
pub fn parse_transform_spec(spec: &str) -> Result<Transform> {
    let (name, args) = spec.split_once(':').unwrap_or((spec, ""));
    let name = name.trim().to_ascii_lowercase();
    let mut kv: BTreeMap<String, f64> = BTreeMap::new();
    let mut bare: Vec<f64> = Vec::new();
    for part in args.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match part.split_once('=') {
            Some((k, v)) => {
                let v: f64 = v.trim().parse().map_err(|_| {
                    Error::Usage(format!("transform parameter {k} needs a number, got '{v}'"))
                })?;
                kv.insert(k.trim().to_ascii_lowercase(), v);
            }
            None => bare.push(part.parse().map_err(|_| {
                Error::Usage(format!(
                    "transform argument '{part}' is not K=V or a number"
                ))
            })?),
        }
    }
    let get = |k: &str, d: f64| kv.get(k).copied().unwrap_or(d);
    let t = match name.as_str() {
        "linear" | "flin" => Transform::Linear {
            t: get("t", 262_144.0),
            a: get("a", 0.0),
        },
        "log" | "flog" => Transform::Log {
            t: get("t", 262_144.0),
            m: get("m", 4.5),
        },
        "arcsinh" | "asinh" | "fasinh" => Transform::Asinh {
            t: get("t", 262_144.0),
            m: get("m", 4.5),
            a: get("a", 0.0),
        },
        "logicle" => Transform::Logicle {
            t: get("t", 262_144.0),
            w: get("w", 0.5),
            m: get("m", 4.5),
            a: get("a", 0.0),
        },
        "hyperlog" => Transform::Hyperlog {
            t: get("t", 262_144.0),
            w: get("w", 0.5),
            m: get("m", 4.5),
            a: get("a", 0.0),
        },
        "biex" | "flowjo-biex" => Transform::FlowJoBiex {
            negative: get("neg", 0.0),
            width: get("width", -10.0),
            positive: get("pos", 4.418_54),
            max_value: get("maxrange", 262_144.0),
        },
        "flowjo-log" => Transform::FlowJoLog {
            offset: get("offset", 1.0),
            decades: get("decades", 4.5),
        },
        "arcsinh-cofactor" | "asinh-cofactor" | "cofactor" => Transform::AsinhCofactor {
            cofactor: kv
                .get("cofactor")
                .or_else(|| bare.first())
                .copied()
                .unwrap_or(5.0),
        },
        other => {
            return Err(Error::Usage(format!(
                "unknown transform '{other}'; use linear, log, arcsinh, logicle, hyperlog, biex, flowjo-log or arcsinh-cofactor (parameters as NAME:K=V,…)"
            )));
        }
    };
    t.prepare().map_err(Error::Usage)?;
    Ok(t)
}

/// Compensation for `table`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CompensationChoice {
    /// The gating file's matrix when a gating file with exactly one matrix is given, else the
    /// FCS file's own.
    Auto,
    /// The FCS file's `$SPILLOVER` / `$SPILL` / `SPILL`.
    File,
    /// The matrix of the gating file (the workspace sample's, or the only Gating-ML matrix).
    GatingFile,
}

/// Transforms for `table`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum TransformChoice {
    /// One transform on the given parameters (default: every fluorescence parameter).
    Uniform {
        /// The transform.
        transform: Transform,
        /// Parameters (`$PnN`); empty = fluorescence parameters (not FSC/SSC/Time).
        parameters: Vec<String>,
    },
    /// The FlowJo workspace sample's per-parameter transforms.
    GatingFile,
}

/// Processing options for `table`.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TableOptions {
    /// Compensation to apply.
    pub compensation: Option<CompensationChoice>,
    /// Transforms to apply after compensation.
    pub transform: Option<TransformChoice>,
    /// Gating file (FlowJo workspace or Gating-ML).
    pub gating_file: Option<PathBuf>,
    /// Workspace sample.
    pub sample: Option<String>,
    /// Populations whose membership is appended as 0/1 columns.
    pub populations: Vec<String>,
}

impl TableOptions {
    /// Whether anything beyond raw values was asked for.
    pub fn is_active(&self) -> bool {
        self.compensation.is_some()
            || self.transform.is_some()
            || self.gating_file.is_some()
            || !self.populations.is_empty()
    }
}

/// Rows of a data set after processing.
#[derive(Debug, Clone)]
pub struct ProcessedRows {
    /// Column names (`$PnN`, then `gate:<path>` columns).
    pub names: Vec<String>,
    /// Column labels (`$PnS`; the population path for gate columns).
    pub labels: Vec<Option<String>>,
    /// Column-major values.
    pub columns: Vec<Vec<f64>>,
    /// Rows in the whole data set.
    pub total_rows: u64,
    /// What was done.
    pub processing: TableProcessing,
}

/// Read rows `first_row..first_row+max_rows` of data set `table` and process them.
pub fn processed_rows(
    fcs_path: &Path,
    table: u32,
    first_row: u64,
    max_rows: u64,
    opts: &TableOptions,
) -> Result<ProcessedRows> {
    let mut ds = FcsDataset::open(fcs_path)?;
    let set = data_set(&ds, table)?;
    let raw = ds.read_table(table, first_row, max_rows)?;
    let mut columns = raw.columns;
    let (mut names, mut labels) = parameter_names(&set);
    let mut processing = TableProcessing {
        scale: scale_columns(&set, &mut columns),
        ..Default::default()
    };
    let needs_gating_file = opts.gating_file.is_some();
    if !needs_gating_file
        && (matches!(opts.compensation, Some(CompensationChoice::GatingFile))
            || matches!(opts.transform, Some(TransformChoice::GatingFile))
            || !opts.populations.is_empty())
    {
        return Err(Error::Usage(
            "this option needs a gating file: pass --workspace W.wsp or --gatingml G.xml".into(),
        ));
    }
    let bound = match &opts.gating_file {
        Some(g) => Some(bind(g, opts.sample.as_deref(), Some((fcs_path, &set)))?),
        None => None,
    };
    let file_m = file_matrix(&set);
    // Population membership, computed on scale values (gates carry their own compensation and
    // transforms).
    let mut gate_cols: Vec<(String, Vec<f64>)> = Vec::new();
    if !opts.populations.is_empty()
        && let Some(b) = &bound
    {
        let strategy = b.strategy().ok_or_else(|| {
            Error::Usage("pass --sample to choose the workspace sample whose gates to use".into())
        })?;
        let ev = Evaluator::new(strategy, &names, &labels, file_m.as_ref())?;
        let members = ev.evaluate(&columns)?;
        for q in &opts.populations {
            let i = strategy.find(q).ok_or_else(|| {
                Error::Usage(format!("population '{q}' is not in the gating file"))
            })?;
            let path = strategy.populations[i].path_string();
            let col: Vec<f64> = members[i]
                .iter()
                .map(|&b| if b { 1.0 } else { 0.0 })
                .collect();
            let count = members[i].iter().filter(|b| **b).count() as u64;
            processing.gates.push(GateColumn {
                column: format!("gate:{path}"),
                population: path.clone(),
                gating_file: opts
                    .gating_file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                count_in_rows: count,
            });
            gate_cols.push((path, col));
        }
    }
    // Compensation.
    if let Some(choice) = &opts.compensation {
        let choice = match choice {
            CompensationChoice::Auto => {
                let one = bound
                    .as_ref()
                    .and_then(Bound::strategy)
                    .is_some_and(|s| s.matrices.len() == 1);
                if one {
                    &CompensationChoice::GatingFile
                } else {
                    &CompensationChoice::File
                }
            }
            c => c,
        };
        let (m, source) = match choice {
            CompensationChoice::Auto => unreachable!("resolved above"),
            CompensationChoice::File => (
                file_m.clone().ok_or_else(|| {
                    Error::unsupported(
                        FORMAT_ID,
                        "compensation: the file has no $SPILLOVER, $SPILL or SPILL keyword",
                        "Pass --workspace W.wsp (its sample's matrix) or --gatingml G.xml with --compensate gating.",
                    )
                })?,
                "fcs",
            ),
            CompensationChoice::GatingFile => {
                let b = bound.as_ref().ok_or_else(|| {
                    Error::Usage("--compensate gating needs --workspace or --gatingml".into())
                })?;
                let s = b.strategy().ok_or_else(|| {
                    Error::Usage("pass --sample to choose the workspace sample".into())
                })?;
                let mut ms = s.matrices.values();
                let m = match (ms.next(), ms.next()) {
                    (Some(m), None) => m.clone(),
                    (None, _) => {
                        return Err(Error::Usage("the gating file defines no compensation matrix".into()));
                    }
                    _ => {
                        return Err(Error::Usage(
                            "the gating file defines several matrices; use gate-specific compensation via `analyze gate`".into(),
                        ));
                    }
                };
                (m, if b.format == WSP_FORMAT_ID { "workspace" } else { "gating-ml" })
            }
        };
        apply_matrix(&m, &mut columns, &names, &labels)?;
        processing.compensation = Some(matrix_summary(&m, source, true));
    }
    // Transforms.
    if let Some(choice) = &opts.transform {
        let plan: Vec<(usize, String, Transform)> = match choice {
            TransformChoice::Uniform {
                transform,
                parameters,
            } => {
                let cols: Vec<usize> = if parameters.is_empty() {
                    (0..names.len())
                        .filter(|&i| is_fluorescence_parameter(&names[i]))
                        .collect()
                } else {
                    parameters
                        .iter()
                        .map(|p| {
                            names.iter().position(|n| n == p).ok_or_else(|| {
                                Error::Usage(format!(
                                    "parameter '{p}' is not in the file ({})",
                                    names.join(", ")
                                ))
                            })
                        })
                        .collect::<Result<_>>()?
                };
                cols.into_iter()
                    .map(|i| (i, names[i].clone(), transform.clone()))
                    .collect()
            }
            TransformChoice::GatingFile => {
                let b = bound.as_ref().ok_or_else(|| {
                    Error::Usage("--transform workspace needs --workspace".into())
                })?;
                if b.format != WSP_FORMAT_ID {
                    return Err(Error::Usage(
                        "Gating-ML transforms are not tied to parameters; name one with --transform NAME".into(),
                    ));
                }
                let s = b
                    .strategy()
                    .ok_or_else(|| Error::Usage("pass --sample".into()))?;
                names
                    .iter()
                    .enumerate()
                    .filter_map(|(i, n)| s.transforms.get(n).map(|t| (i, n.clone(), t.clone())))
                    .collect()
            }
        };
        for (i, name, t) in plan {
            let p = t.prepare().map_err(Error::Usage)?;
            for v in &mut columns[i] {
                *v = p.apply(*v);
            }
            processing.transforms.push(AppliedTransform {
                parameter: name,
                kind: t.kind().to_string(),
                parameters: t
                    .parameters()
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            });
        }
    }
    for (path, col) in gate_cols {
        names.push(format!("gate:{path}"));
        labels.push(Some(path));
        columns.push(col);
    }
    Ok(ProcessedRows {
        names,
        labels,
        columns,
        total_rows: set.event_count.unwrap_or(0),
        processing,
    })
}

/// Rows of any reader's table as JSON rows; FCS files are processed when `opts` asks for it
/// (scale values, compensation, transforms, population columns).
pub fn table_slice(
    reg: &openreadout_core::Registry,
    file: &Path,
    table: u32,
    first_row: u64,
    max_rows: u64,
    opts: &TableOptions,
) -> Result<TableSlice> {
    let (det, mut ds) = reg.open(file)?;
    if opts.is_active() {
        if det.format_id != FORMAT_ID {
            return Err(Error::Usage(format!(
                "compensation, transforms and gates apply to FCS files; this is {}",
                det.format_id
            )));
        }
        drop(ds);
        let p = processed_rows(file, table, first_row, max_rows, opts)?;
        let n = p.columns.first().map_or(0, Vec::len);
        return Ok(TableSlice {
            path: file.display().to_string(),
            format: det.format_id.into(),
            table,
            first_row,
            total_rows: p.total_rows,
            rows: (0..n)
                .map(|r| p.columns.iter().map(|c| c[r]).collect())
                .collect(),
            truncated: first_row + (n as u64) < p.total_rows,
            columns: p.names,
            labels: p.labels,
            processing: Some(p.processing),
            filter: None,
        });
    }
    let info = ds.info()?;
    let t = info
        .tables
        .iter()
        .find(|t| t.index == table)
        .cloned()
        .ok_or_else(|| {
            Error::Usage(format!(
                "table {table} not found (file has {} tables)",
                info.tables.len()
            ))
        })?;
    let tab = ds.read_table(table, first_row, max_rows)?;
    let n = tab.columns.first().map_or(0, Vec::len);
    Ok(TableSlice {
        path: file.display().to_string(),
        format: det.format_id.into(),
        table,
        first_row,
        total_rows: t.row_count,
        columns: t.columns.iter().map(|c| c.name.clone()).collect(),
        labels: t.columns.iter().map(|c| c.label.clone()).collect(),
        truncated: first_row + (n as u64) < t.row_count,
        rows: (0..n)
            .map(|r| tab.columns.iter().map(|c| c[r]).collect())
            .collect(),
        processing: None,
        filter: None,
    })
}

fn apply_matrix(
    m: &CompMatrix,
    columns: &mut [Vec<f64>],
    names: &[String],
    labels: &[Option<String>],
) -> Result<()> {
    let solver = m
        .solver()
        .map_err(|e| Error::unsupported(FORMAT_ID, e, "Check the compensation matrix."))?;
    let find = |d: &str| {
        names
            .iter()
            .position(|n| n == d)
            .or_else(|| labels.iter().position(|l| l.as_deref() == Some(d)))
            .ok_or_else(|| {
                Error::Usage(format!(
                    "compensation matrix {} names {d}, which is not a parameter of this file",
                    m.name
                ))
            })
    };
    let det: Vec<usize> = m.detectors.iter().map(|d| find(d)).collect::<Result<_>>()?;
    let out: Vec<usize> = if m.fluorochromes.len() == m.detectors.len() {
        det.clone()
    } else {
        m.fluorochromes
            .iter()
            .map(|d| find(d))
            .collect::<Result<_>>()?
    };
    solver.apply_columns(columns, &det, &out);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn transform_specs() {
        assert!(matches!(
            parse_transform_spec("logicle").unwrap(),
            Transform::Logicle { t, .. } if t == 262_144.0
        ));
        assert!(matches!(
            parse_transform_spec("logicle:T=10000,W=1").unwrap(),
            Transform::Logicle { t, w, .. } if t == 10_000.0 && w == 1.0
        ));
        assert!(matches!(
            parse_transform_spec("arcsinh-cofactor:150").unwrap(),
            Transform::AsinhCofactor { cofactor } if cofactor == 150.0
        ));
        assert!(parse_transform_spec("nope").is_err());
        assert!(parse_transform_spec("logicle:W=4").is_err());
    }
}
