//! FlowJo 10 workspaces (`.wsp`): samples with their keywords, compensation matrix, per-channel
//! transforms and gate tree; groups with their member samples.
//!
//! Element names come from the files (`docs/formats/flowjo-wsp.md`); the gates themselves are
//! Gating-ML 2.0 elements with coordinates in untransformed (scale) values.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use openreadout_core::{Error, Result};
use roxmltree::Node;

use super::gatingml::parse_transform;
use super::xml::{attr, child, children, elements, line, num_attr, parse};
use super::{
    BoolOp, CompMatrix, CompensationRef, CoordinateSpace, Dimension, DimensionSource, GateKind,
    Operand, Population, Shape, Strategy, Transform, WSP_FORMAT_ID,
};

/// A parsed FlowJo workspace.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    /// Workspace format version (`Workspace/@version`, e.g. `20.0`).
    pub version: Option<String>,
    /// FlowJo release that wrote it (`flowJoVersion`).
    pub flowjo_version: Option<String>,
    /// Last modification as FlowJo wrote it (`modDate`).
    pub modified: Option<String>,
    /// Samples in `SampleList` order.
    pub samples: Vec<WorkspaceSample>,
    /// Groups (`Groups/GroupNode`) with their sample ids.
    pub groups: Vec<Group>,
}

/// A sample group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Group {
    /// Group name (`All Samples`, `Compensation`, user groups).
    pub name: String,
    /// Member sample ids.
    pub sample_ids: Vec<String>,
}

/// One sample of a workspace.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceSample {
    /// FlowJo sample id (`sampleID`).
    pub id: String,
    /// Sample name (`SampleNode/@name`, usually the FCS file name).
    pub name: String,
    /// Where FlowJo found the FCS file (`DataSet/@uri`).
    pub uri: Option<String>,
    /// Keywords as FlowJo stored them (FCS TEXT keywords plus FlowJo's own).
    pub keywords: Vec<(String, String)>,
    /// Event count FlowJo stored for the sample (`SampleNode/@count`).
    pub event_count: Option<u64>,
    /// The sample's compensation matrix, if any.
    pub matrix: Option<CompMatrix>,
    /// Prefix FlowJo puts before compensated parameter names (`Comp-`).
    pub matrix_prefix: String,
    /// Suffix after compensated parameter names (usually empty).
    pub matrix_suffix: String,
    /// Gate tree, transforms (keyed by parameter name) and the matrix.
    pub strategy: Strategy,
    /// Populations that could not be read, with the reason (their subtrees are left out).
    pub skipped: Vec<String>,
    /// Groups the sample belongs to.
    pub groups: Vec<String>,
}

impl WorkspaceSample {
    /// A keyword value by name (case-insensitive).
    pub fn keyword(&self, name: &str) -> Option<&str> {
        self.keywords
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The file name at the end of `uri`, percent-decoded.
    pub fn uri_file_name(&self) -> Option<String> {
        let u = self.uri.as_deref()?;
        let last = u.rsplit(['/', '\\']).next()?;
        Some(percent_decode(last))
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(h) = b.get(i + 1..i + 3)
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(h).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

impl Workspace {
    /// Choose the sample for an FCS file: by `wanted` (sample name or id) when given, else by
    /// file name (sample name or `DataSet` file name), else by the file's `$FIL` keyword, else
    /// the only sample.
    pub fn select_sample(
        &self,
        wanted: Option<&str>,
        fcs_file_name: Option<&str>,
        fcs_fil: Option<&str>,
    ) -> Result<usize> {
        let names = || {
            self.samples
                .iter()
                .map(|s| format!("{} (id {})", s.name, s.id))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if let Some(w) = wanted {
            return self
                .samples
                .iter()
                .position(|s| s.name == w || s.id == w)
                .ok_or_else(|| {
                    Error::Usage(format!(
                        "sample '{w}' is not in the workspace; its samples are: {}",
                        names()
                    ))
                });
        }
        if let Some(f) = fcs_file_name {
            let hits: Vec<usize> = self
                .samples
                .iter()
                .enumerate()
                .filter(|(_, s)| s.name == f || s.uri_file_name().as_deref() == Some(f))
                .map(|(i, _)| i)
                .collect();
            if hits.len() == 1 {
                return Ok(hits[0]);
            }
        }
        if let Some(fil) = fcs_fil.map(str::trim).filter(|s| !s.is_empty()) {
            let hits: Vec<usize> = self
                .samples
                .iter()
                .enumerate()
                .filter(|(_, s)| s.keyword("$FIL").map(str::trim) == Some(fil))
                .map(|(i, _)| i)
                .collect();
            if hits.len() == 1 {
                return Ok(hits[0]);
            }
        }
        if self.samples.len() == 1 {
            return Ok(0);
        }
        Err(Error::Usage(format!(
            "cannot tell which workspace sample this FCS file is; pass --sample NAME. Samples: {}",
            names()
        )))
    }

    /// The one sample whose stored FCS keywords identify the same acquisition as the file's
    /// (for a file renamed since the workspace was saved): every one of [`ACQUISITION_KEYWORDS`]
    /// present on both sides agrees, at least three of them are compared, and `$TOT` is one of
    /// them. `None` when no sample, or more than one, matches.
    pub fn select_sample_by_keywords(
        &self,
        keyword: &dyn Fn(&str) -> Option<String>,
    ) -> Option<usize> {
        let hits: Vec<usize> = self
            .samples
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let mut compared = 0;
                let mut tot = false;
                for k in ACQUISITION_KEYWORDS {
                    if let (Some(a), Some(b)) = (s.keyword(k), keyword(k)) {
                        if a.trim() != b.trim() {
                            return false;
                        }
                        compared += 1;
                        tot |= *k == "$TOT";
                    }
                }
                compared >= 3 && tot
            })
            .map(|(i, _)| i)
            .collect();
        (hits.len() == 1).then(|| hits[0])
    }
}

/// FCS keywords that together identify one acquisition: event count, start and end times,
/// date, cytometer and its serial number.
pub const ACQUISITION_KEYWORDS: &[&str] = &["$TOT", "$BTIM", "$ETIM", "$DATE", "$CYT", "$CYTSN"];

/// Parse a FlowJo workspace document. `path` is used for messages only.
pub fn parse_workspace(text: &str, path: &Path) -> Result<Workspace> {
    let doc = parse(text, WSP_FORMAT_ID)?;
    let root = doc.root_element();
    if root.tag_name().name() != "Workspace" {
        return Err(Error::corrupt(
            WSP_FORMAT_ID,
            format!(
                "{}: root element is <{}>, not <Workspace>",
                path.display(),
                root.tag_name().name()
            ),
        ));
    }
    let mut ws = Workspace {
        version: attr(root, "version").map(str::to_string),
        flowjo_version: attr(root, "flowJoVersion").map(str::to_string),
        modified: attr(root, "modDate").map(str::to_string),
        ..Default::default()
    };
    if let Some(groups) = child(root, "Groups") {
        for g in children(groups, "GroupNode") {
            let name = attr(g, "name").unwrap_or_default().to_string();
            let sample_ids = child(g, "Group")
                .and_then(|gr| child(gr, "SampleRefs"))
                .map(|r| {
                    children(r, "SampleRef")
                        .filter_map(|s| attr(s, "sampleID").map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            ws.groups.push(Group { name, sample_ids });
        }
    }
    if let Some(list) = child(root, "SampleList") {
        for s in children(list, "Sample") {
            let mut sample = parse_sample(s)?;
            sample.groups = ws
                .groups
                .iter()
                .filter(|g| g.sample_ids.contains(&sample.id))
                .map(|g| g.name.clone())
                .collect();
            ws.samples.push(sample);
        }
    }
    Ok(ws)
}

fn corrupt(n: Node<'_, '_>, msg: impl std::fmt::Display) -> Error {
    Error::corrupt(WSP_FORMAT_ID, format!("line {}: {msg}", line(n)))
}

fn count_attr(n: Node<'_, '_>) -> Option<u64> {
    attr(n, "count")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .and_then(|v| u64::try_from(v).ok())
}

fn parse_matrix(m: Node<'_, '_>) -> Result<(CompMatrix, String, String)> {
    let name = attr(m, "name").unwrap_or("Compensation").to_string();
    let prefix = attr(m, "prefix").unwrap_or("Comp-").to_string();
    let suffix = attr(m, "suffix").unwrap_or_default().to_string();
    let spectral = attr(m, "spectral").is_some_and(|v| v.trim() == "1");
    if spectral
        && let Some(algo) = attr(m, "weightOptAlgorithmType")
        && algo != "OLS"
    {
        return Err(Error::unsupported(
            WSP_FORMAT_ID,
            format!("spectral matrix {name} with weighting {algo}"),
            "Only ordinary least-squares (OLS) spectral unmixing is implemented.",
        ));
    }
    let detectors: Vec<String> = child(m, "parameters")
        .map(|p| {
            children(p, "parameter")
                .filter_map(|x| attr(x, "name").map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut row_names = Vec::new();
    let mut column_names: Vec<String> = Vec::new();
    for (i, sp) in children(m, "spillover").enumerate() {
        row_names.push(attr(sp, "parameter").unwrap_or_default().to_string());
        let mut row = Vec::new();
        for c in children(sp, "coefficient") {
            row.push(
                num_attr(c, "value").ok_or_else(|| corrupt(c, "coefficient without a value"))?,
            );
            if i == 0 {
                column_names.push(attr(c, "parameter").unwrap_or_default().to_string());
            }
        }
        rows.push(row);
    }
    // Columns follow the coefficients' parameter names; rows the spillover elements. For a
    // conventional matrix both are the parameter list.
    let columns = if column_names.iter().all(|c| !c.is_empty()) && !column_names.is_empty() {
        column_names
    } else {
        detectors.clone()
    };
    if rows.iter().any(|r| r.len() != columns.len()) || rows.is_empty() {
        return Err(corrupt(
            m,
            format!("spillover matrix {name}: rows and columns do not match"),
        ));
    }
    let fluorochromes = if row_names.iter().all(|r| !r.is_empty()) {
        row_names
    } else {
        columns.iter().take(rows.len()).cloned().collect()
    };
    let spectral = spectral || columns.len() > rows.len();
    Ok((
        CompMatrix {
            name,
            detectors: columns,
            fluorochromes,
            values: rows,
            spectral,
        },
        prefix,
        suffix,
    ))
}

fn parse_wsp_transform(n: Node<'_, '_>) -> std::result::Result<Transform, String> {
    let num = |k: &str| {
        num_attr(n, k).ok_or_else(|| format!("{} without a numeric {k}", n.tag_name().name()))
    };
    match n.tag_name().name() {
        "linear" => Ok(Transform::Linear {
            t: num("maxRange")?,
            a: num("minRange")?,
        }),
        "log" => Ok(Transform::FlowJoLog {
            offset: num("offset")?,
            decades: num("decades")?,
        }),
        "biex" => Ok(Transform::FlowJoBiex {
            negative: num("neg")?,
            width: num("width")?,
            positive: num("pos")?,
            max_value: num("maxRange")?,
        }),
        "logicle" | "fasinh" | "hyperlog" | "flin" | "flog" => {
            parse_transform(n, WSP_FORMAT_ID).map_err(|e| e.to_string())
        }
        other => Err(format!("FlowJo transform <{other}>")),
    }
}

fn parse_sample(s: Node<'_, '_>) -> Result<WorkspaceSample> {
    let node = child(s, "SampleNode").ok_or_else(|| corrupt(s, "Sample without a SampleNode"))?;
    let dataset = child(s, "DataSet");
    let mut sample = WorkspaceSample {
        id: attr(node, "sampleID")
            .or_else(|| dataset.and_then(|d| attr(d, "sampleID")))
            .unwrap_or_default()
            .to_string(),
        name: attr(node, "name").unwrap_or_default().to_string(),
        uri: dataset.and_then(|d| attr(d, "uri")).map(str::to_string),
        event_count: count_attr(node),
        ..Default::default()
    };
    if let Some(k) = child(s, "Keywords") {
        sample.keywords = children(k, "Keyword")
            .filter_map(|x| {
                Some((
                    attr(x, "name")?.to_string(),
                    attr(x, "value").unwrap_or_default().to_string(),
                ))
            })
            .collect();
    }
    let mut strategy = Strategy::default();
    if let Some(m) = children(s, "spilloverMatrix").next() {
        let (matrix, prefix, suffix) = parse_matrix(m)?;
        strategy
            .matrices
            .insert(matrix.name.clone(), matrix.clone());
        sample.matrix = Some(matrix);
        sample.matrix_prefix = prefix;
        sample.matrix_suffix = suffix;
    }
    if let Some(tr) = child(s, "Transformations") {
        for t in elements(tr) {
            let Some(param) = child(t, "parameter").and_then(|p| attr(p, "name")) else {
                continue;
            };
            match parse_wsp_transform(t) {
                Ok(x) => {
                    strategy.transforms.insert(param.to_string(), x);
                }
                Err(e) => {
                    strategy.unsupported_transforms.insert(param.to_string(), e);
                }
            }
        }
    }
    let ctx = Ctx {
        matrix: sample.matrix.as_ref(),
        prefix: &sample.matrix_prefix,
        suffix: &sample.matrix_suffix,
    };
    let mut pending: Vec<PendingBool> = Vec::new();
    if let Some(sub) = child(node, "Subpopulations") {
        walk(
            sub,
            None,
            &[],
            &ctx,
            &mut strategy,
            &mut pending,
            &mut sample.skipped,
        )?;
    }
    resolve_transform_ids(&mut strategy);
    // Resolve Boolean dependents by path (`A/B/C`, root excluded).
    let by_path: HashMap<String, usize> = strategy
        .populations
        .iter()
        .enumerate()
        .map(|(i, p)| (p.path.join("/"), i))
        .collect();
    for pb in pending {
        let mut operands = Vec::new();
        let mut missing = None;
        for d in &pb.dependents {
            match by_path.get(d.trim_start_matches('/')) {
                Some(&i) => operands.push(Operand {
                    population: i,
                    complement: false,
                }),
                None => missing = Some(d.clone()),
            }
        }
        if let Some(m) = missing {
            sample.skipped.push(format!(
                "{}: Boolean gate refers to {m}, which is not in the tree",
                format_path(&strategy.populations[pb.index].path)
            ));
            // Leave it evaluating to nothing: an empty OR.
            strategy.populations[pb.index].shape = Shape::Boolean {
                op: BoolOp::Or,
                operands: Vec::new(),
            };
            continue;
        }
        if let Shape::Boolean { operands: o, .. } = &mut strategy.populations[pb.index].shape {
            *o = operands;
        }
    }
    strategy
        .sort_dependencies()
        .map_err(|e| Error::corrupt(WSP_FORMAT_ID, format!("sample {}: {e}", sample.name)))?;
    sample.strategy = strategy;
    Ok(sample)
}

fn format_path(p: &[String]) -> String {
    format!("/{}", p.join("/"))
}

struct Ctx<'a> {
    matrix: Option<&'a CompMatrix>,
    prefix: &'a str,
    suffix: &'a str,
}

struct PendingBool {
    index: usize,
    dependents: Vec<String>,
}

fn walk(
    sub: Node<'_, '_>,
    parent: Option<usize>,
    parent_path: &[String],
    ctx: &Ctx<'_>,
    strategy: &mut Strategy,
    pending: &mut Vec<PendingBool>,
    skipped: &mut Vec<String>,
) -> Result<()> {
    for n in elements(sub) {
        let tag = n.tag_name().name();
        let bool_op = match tag {
            "AndNode" => Some(BoolOp::And),
            "OrNode" => Some(BoolOp::Or),
            "NotNode" => Some(BoolOp::Not),
            "Population" => None,
            _ => continue,
        };
        let name = attr(n, "name").unwrap_or_default().to_string();
        let mut path = parent_path.to_vec();
        path.push(name.clone());
        let base = Population {
            name: name.clone(),
            path: path.clone(),
            parent,
            kind: GateKind::Boolean,
            dimensions: Vec::new(),
            shape: Shape::Rectangle,
            complement: false,
            coordinates: CoordinateSpace::Untransformed,
            gate_id: None,
            quadrant_gate: None,
            stored_count: count_attr(n),
            owning_group: attr(n, "owningGroup").map(str::to_string),
        };
        let pop = if let Some(op) = bool_op {
            let dependents: Vec<String> = child(n, "Dependents")
                .map(|d| {
                    children(d, "Dependent")
                        .filter_map(|x| attr(x, "name").map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            pending.push(PendingBool {
                index: strategy.populations.len(),
                dependents,
            });
            Population {
                shape: Shape::Boolean {
                    op,
                    operands: Vec::new(),
                },
                ..base
            }
        } else {
            match population_gate(n, ctx) {
                Ok(p) => Population {
                    kind: p.kind,
                    dimensions: p.dimensions,
                    shape: p.shape,
                    complement: p.complement,
                    coordinates: p.coordinates,
                    gate_id: p.gate_id,
                    ..base
                },
                Err(e) => {
                    skipped.push(format!("{}: {e}", format_path(&path)));
                    continue;
                }
            }
        };
        let index = strategy.populations.len();
        strategy.populations.push(pop);
        if let Some(s) = child(n, "Subpopulations") {
            walk(s, Some(index), &path, ctx, strategy, pending, skipped)?;
        }
    }
    Ok(())
}

struct GateParts {
    kind: GateKind,
    dimensions: Vec<Dimension>,
    shape: Shape,
    complement: bool,
    coordinates: CoordinateSpace,
    gate_id: Option<String>,
}

fn population_gate(pop: Node<'_, '_>, ctx: &Ctx<'_>) -> std::result::Result<GateParts, String> {
    let gate = child(pop, "Gate").ok_or("population without a Gate")?;
    let g = elements(gate).next().ok_or("empty Gate element")?;
    let complement = attr(g, "eventsInside").is_some_and(|v| v.trim() == "0");
    let gate_id = attr(g, "id")
        .or_else(|| attr(gate, "id"))
        .map(str::to_string);
    let dims: Vec<Dimension> = children(g, "dimension")
        .map(|d| wsp_dimension(d, ctx))
        .collect::<std::result::Result<_, _>>()?;
    let vertex = |v: Node<'_, '_>| -> std::result::Result<[f64; 2], String> {
        let c: Vec<f64> = children(v, "coordinate")
            .filter_map(|c| num_attr(c, "value"))
            .collect();
        match c.as_slice() {
            [x, y] => Ok([*x, *y]),
            _ => Err(format!("line {}: vertex needs two coordinates", line(v))),
        }
    };
    let (kind, shape) = match g.tag_name().name() {
        "RectangleGate" => (GateKind::Rectangle, Shape::Rectangle),
        "PolygonGate" => {
            let vertices = children(g, "vertex")
                .map(vertex)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if vertices.len() < 3 || dims.len() != 2 {
                return Err("polygon gate needs two dimensions and three vertices".into());
            }
            (GateKind::Polygon, Shape::Polygon { vertices })
        }
        "EllipsoidGate" => {
            if dims.len() != 2 {
                return Err("ellipse gate needs two dimensions".into());
            }
            let foci: Vec<[f64; 2]> = child(g, "foci")
                .map(|f| {
                    children(f, "vertex")
                        .map(vertex)
                        .collect::<std::result::Result<_, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            let edge: Vec<[f64; 2]> = child(g, "edge")
                .map(|f| {
                    children(f, "vertex")
                        .map(vertex)
                        .collect::<std::result::Result<_, _>>()
                })
                .transpose()?
                .unwrap_or_default();
            if foci.len() != 2 || edge.len() < 4 {
                return Err("ellipse gate needs two foci and four edge points".into());
            }
            (
                GateKind::Ellipsoid,
                Shape::DisplayEllipse {
                    foci: [foci[0], foci[1]],
                    edge,
                },
            )
        }
        other => {
            return Err(format!(
                "gate element <{other}> is not supported in workspaces yet"
            ));
        }
    };
    let coordinates = if matches!(shape, Shape::DisplayEllipse { .. }) {
        CoordinateSpace::Transformed
    } else {
        CoordinateSpace::Untransformed
    };
    Ok(GateParts {
        kind,
        dimensions: dims,
        shape,
        complement,
        coordinates,
        gate_id,
    })
}

/// A workspace gate dimension: `Comp-X` (prefix/suffix of the sample's matrix) means parameter
/// `X` compensated by that matrix; the transform is the sample's transform for the parameter.
fn wsp_dimension(d: Node<'_, '_>, ctx: &Ctx<'_>) -> std::result::Result<Dimension, String> {
    let name = child(d, "fcs-dimension")
        .and_then(|f| attr(f, "name"))
        .ok_or_else(|| format!("line {}: dimension without an fcs-dimension name", line(d)))?;
    let mut base = name;
    let mut compensation = CompensationRef::Uncompensated;
    if let Some(m) = ctx.matrix {
        let stripped = name
            .strip_prefix(ctx.prefix)
            .filter(|_| !ctx.prefix.is_empty())
            .map(|s| {
                if ctx.suffix.is_empty() {
                    s
                } else {
                    s.strip_suffix(ctx.suffix).unwrap_or(s)
                }
            });
        let candidate = match stripped {
            Some(s) => Some(s),
            None if ctx.prefix.is_empty() && !ctx.suffix.is_empty() => {
                name.strip_suffix(ctx.suffix)
            }
            None if ctx.prefix.is_empty() && ctx.suffix.is_empty() => Some(name),
            None => None,
        };
        if let Some(c) = candidate
            && (m.detectors.iter().any(|x| x == c) || m.fluorochromes.iter().any(|x| x == c))
        {
            base = c;
            compensation = CompensationRef::Matrix(m.name.clone());
        }
    }
    Ok(Dimension {
        source: DimensionSource::Parameter(base.to_string()),
        compensation,
        transform: Some(base.to_string()),
        min: num_attr(d, "min"),
        max: num_attr(d, "max"),
    })
}

/// Drop transform references to parameters the sample has no transform for (FlowJo leaves
/// such parameters untransformed).
fn resolve_transform_ids(strategy: &mut Strategy) {
    let known: BTreeSet<String> = strategy
        .transforms
        .keys()
        .chain(strategy.unsupported_transforms.keys())
        .cloned()
        .collect();
    for p in &mut strategy.populations {
        for d in &mut p.dimensions {
            if let Some(t) = &d.transform
                && !known.contains(t)
            {
                d.transform = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_uri_names() {
        assert_eq!(percent_decode("Zam47%20500k.fcs"), "Zam47 500k.fcs");
        assert_eq!(percent_decode("a%2"), "a%2");
    }
}
