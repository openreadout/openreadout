//! Flow analysis on FCS events: FlowJo workspaces (`.wsp`), Gating-ML 2.0 documents,
//! compensation and data-scale transforms, and a gate evaluator that counts populations.
//!
//! Vocabulary: `docs/formats/flowjo-wsp.md`. Provenance: `docs/provenance/flowjo-wsp.md`.
//!
//! The pieces:
//! - [`read_gating_file`] opens a `.wsp` or Gating-ML file ([`GatingFile`]); each workspace
//!   sample, or the one Gating-ML document, is a [`Strategy`]: populations with their gates,
//!   the transforms and compensation matrices they refer to.
//! - [`Evaluator`] applies a strategy to event rows (scale values, see
//!   [`crate::scale_columns`]) and returns each population's membership.

#[doc(hidden)]
pub mod compensation;
#[doc(hidden)]
pub mod evaluate;
#[doc(hidden)]
pub mod gatingml;
#[doc(hidden)]
pub mod transform;
#[doc(hidden)]
pub mod wsp;
mod xml;

use std::collections::BTreeMap;
use std::path::Path;

pub use compensation::{CompMatrix, Solver, invert};
pub use evaluate::{Evaluator, PopulationCount};
pub use gatingml::parse_gating_ml;
pub use transform::{Prepared, Transform};
pub use wsp::{Group, Workspace, WorkspaceSample, parse_workspace};

/// Format id of FlowJo workspaces in errors and JSON.
pub const WSP_FORMAT_ID: &str = "flowjo-wsp";
/// Format id of Gating-ML 2.0 documents in errors and JSON.
pub const GATING_ML_FORMAT_ID: &str = "gating-ml";

/// Which compensation a gate dimension uses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CompensationRef {
    /// Raw (scale) values.
    Uncompensated,
    /// The FCS file's own spillover keyword (`$SPILLOVER`, `$SPILL`, `SPILL`); uncompensated
    /// when the file has none (Gating-ML `FCS`).
    FromFile,
    /// A matrix of the strategy, by name.
    Matrix(String),
}

impl CompensationRef {
    /// `uncompensated`, `FCS` or the matrix name.
    pub fn label(&self) -> &str {
        match self {
            CompensationRef::Uncompensated => "uncompensated",
            CompensationRef::FromFile => "FCS",
            CompensationRef::Matrix(m) => m,
        }
    }
}

/// Where a gate dimension's values come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DimensionSource {
    /// A parameter by name (`$PnN`, or a Gating-ML fluorochrome name of the referenced matrix).
    Parameter(String),
    /// A new dimension computed by a ratio transform (Gating-ML `new-dimension`), by id.
    Ratio(String),
}

/// One axis of a gate.
#[derive(Debug, Clone, PartialEq)]
pub struct Dimension {
    /// The values the gate reads.
    pub source: DimensionSource,
    /// Compensation applied first.
    pub compensation: CompensationRef,
    /// Transform applied next (id in [`Strategy::transforms`]).
    pub transform: Option<String>,
    /// Inclusive lower bound (rectangle and quadrant gates).
    pub min: Option<f64>,
    /// Exclusive upper bound (rectangle and quadrant gates).
    pub max: Option<f64>,
}

impl Dimension {
    /// The parameter or ratio id, for display.
    pub fn name(&self) -> &str {
        match &self.source {
            DimensionSource::Parameter(p) | DimensionSource::Ratio(p) => p,
        }
    }
}

/// Kind of gate that defines a population.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateKind {
    /// Range, rectangle or hyper-rectangle: every dimension within `[min, max)`.
    Rectangle,
    /// Two-dimensional polygon (winding-number rule).
    Polygon,
    /// Ellipse or ellipsoid.
    Ellipsoid,
    /// One quadrant of a quadrant gate (a rectangle over the dividers).
    Quadrant,
    /// `and`, `or` or `not` of other populations.
    Boolean,
}

impl GateKind {
    /// `rectangle`, `polygon`, `ellipsoid`, `quadrant`, `boolean`.
    pub fn name(self) -> &'static str {
        match self {
            GateKind::Rectangle => "rectangle",
            GateKind::Polygon => "polygon",
            GateKind::Ellipsoid => "ellipsoid",
            GateKind::Quadrant => "quadrant",
            GateKind::Boolean => "boolean",
        }
    }
}

/// Boolean operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolOp {
    /// All operands.
    And,
    /// Any operand.
    Or,
    /// Not the (single) operand.
    Not,
}

impl BoolOp {
    /// `and`, `or`, `not`.
    pub fn name(self) -> &'static str {
        match self {
            BoolOp::And => "and",
            BoolOp::Or => "or",
            BoolOp::Not => "not",
        }
    }
}

/// One operand of a Boolean gate.
#[derive(Debug, Clone, PartialEq)]
pub struct Operand {
    /// Index of the referenced population in [`Strategy::populations`].
    pub population: usize,
    /// Use the referenced population's complement.
    pub complement: bool,
}

/// The geometry of a gate, in the coordinates the file stores.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// Bounds are on the dimensions.
    Rectangle,
    /// Vertices `[x, y]` over the two dimensions.
    Polygon {
        /// Polygon vertices.
        vertices: Vec<[f64; 2]>,
    },
    /// Gating-ML ellipsoid: `(p − mean)ᵀ C⁻¹ (p − mean) <= distance_square`.
    Ellipsoid {
        /// Centre.
        mean: Vec<f64>,
        /// Covariance matrix `C`.
        covariance: Vec<Vec<f64>>,
        /// Square of the Mahalanobis distance of the boundary.
        distance_square: f64,
    },
    /// FlowJo ellipse: two foci and four edge points in FlowJo's 256-bin display space of the
    /// transformed dimensions; evaluated as a 128-vertex polygon.
    DisplayEllipse {
        /// The two foci.
        foci: [[f64; 2]; 2],
        /// Edge points (FlowJo writes four).
        edge: Vec<[f64; 2]>,
    },
    /// Boolean combination of other populations.
    Boolean {
        /// The operation.
        op: BoolOp,
        /// Operands.
        operands: Vec<Operand>,
    },
}

/// Space in which a gate's stored coordinates live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinateSpace {
    /// After the dimension's transform (Gating-ML).
    Transformed,
    /// Before it (FlowJo stores scale values; the evaluator transforms them).
    Untransformed,
}

/// A population: the events of its parent that fall inside its gate.
#[derive(Debug, Clone, PartialEq)]
pub struct Population {
    /// Name (FlowJo population name, Gating-ML gate or quadrant id).
    pub name: String,
    /// Names from the root down to this population (a quadrant's path includes its quadrant
    /// gate's id).
    pub path: Vec<String>,
    /// Index of the parent population; `None` for populations of all events.
    pub parent: Option<usize>,
    /// Gate kind.
    pub kind: GateKind,
    /// Gate dimensions (empty for Boolean gates).
    pub dimensions: Vec<Dimension>,
    /// Gate geometry.
    pub shape: Shape,
    /// Events outside the gate belong to the population (FlowJo `eventsInside="0"`).
    pub complement: bool,
    /// Where the coordinates live.
    pub coordinates: CoordinateSpace,
    /// Gate id as written (`gating:id`).
    pub gate_id: Option<String>,
    /// For quadrants: the id of the quadrant gate they belong to.
    pub quadrant_gate: Option<String>,
    /// The event count the writing software stored with the population (FlowJo `count`;
    /// absent or negative when it was not computed).
    pub stored_count: Option<u64>,
    /// FlowJo `owningGroup` (empty: a gate specific to this sample).
    pub owning_group: Option<String>,
}

impl Population {
    /// `/`-joined path with a leading `/`.
    pub fn path_string(&self) -> String {
        format!("/{}", self.path.join("/"))
    }
}

/// A gating hierarchy with the transforms and matrices its gates use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Strategy {
    /// Populations, parents before children and operands before the Boolean gates using them.
    pub populations: Vec<Population>,
    /// Transforms by id (FlowJo: by parameter name).
    pub transforms: BTreeMap<String, Transform>,
    /// Compensation matrices by name.
    pub matrices: BTreeMap<String, CompMatrix>,
    /// Transforms the file defines but this reader does not evaluate: id → element name.
    /// A gate that uses one is reported as unsupported.
    pub unsupported_transforms: BTreeMap<String, String>,
}

impl Strategy {
    /// Find a population by path (`/A/B`, `A/B`) or, when unique, by name.
    pub fn find(&self, query: &str) -> Option<usize> {
        let q = query.trim();
        let q = q.strip_prefix('/').unwrap_or(q);
        if let Some(i) = self.populations.iter().position(|p| p.path.join("/") == q) {
            return Some(i);
        }
        let by_name: Vec<usize> = self
            .populations
            .iter()
            .enumerate()
            .filter(|(_, p)| p.name == q)
            .map(|(i, _)| i)
            .collect();
        (by_name.len() == 1).then(|| by_name[0])
    }

    /// Indices of `i` and all its descendants (by parent links).
    pub fn subtree(&self, i: usize) -> Vec<usize> {
        let mut keep = vec![false; self.populations.len()];
        if i < keep.len() {
            keep[i] = true;
        }
        // Parents come first, so one pass suffices.
        for (j, p) in self.populations.iter().enumerate() {
            if let Some(par) = p.parent
                && keep.get(par).copied().unwrap_or(false)
            {
                keep[j] = true;
            }
        }
        keep.iter()
            .enumerate()
            .filter(|(_, k)| **k)
            .map(|(j, _)| j)
            .collect()
    }

    /// Reorder populations so parents and Boolean operands come first; error on cycles.
    pub(crate) fn sort_dependencies(&mut self) -> Result<(), String> {
        let n = self.populations.len();
        let deps: Vec<Vec<usize>> = self
            .populations
            .iter()
            .map(|p| {
                let mut d: Vec<usize> = p.parent.into_iter().collect();
                if let Shape::Boolean { operands, .. } = &p.shape {
                    d.extend(operands.iter().map(|o| o.population));
                }
                d
            })
            .collect();
        let mut state = vec![0u8; n]; // 0 new, 1 visiting, 2 done
        let mut order = Vec::with_capacity(n);
        for start in 0..n {
            if state[start] != 0 {
                continue;
            }
            // Iterative DFS: (node, next dependency index).
            let mut stack = vec![(start, 0usize)];
            state[start] = 1;
            while let Some(&mut (node, ref mut k)) = stack.last_mut() {
                if let Some(&dep) = deps[node].get(*k) {
                    *k += 1;
                    if dep >= n {
                        return Err(format!("population {node} refers to a missing population"));
                    }
                    match state[dep] {
                        0 => {
                            state[dep] = 1;
                            stack.push((dep, 0));
                        }
                        1 => {
                            return Err(format!(
                                "cyclic gate dependency through {}",
                                self.populations[dep].path_string()
                            ));
                        }
                        _ => {}
                    }
                } else {
                    state[node] = 2;
                    order.push(node);
                    stack.pop();
                }
            }
        }
        let mut new_index = vec![0usize; n];
        for (new, &old) in order.iter().enumerate() {
            new_index[old] = new;
        }
        let mut pops: Vec<Option<Population>> = std::mem::take(&mut self.populations)
            .into_iter()
            .map(Some)
            .collect();
        self.populations = order
            .iter()
            .map(|&old| {
                let mut p = pops[old].take().expect("each index once");
                p.parent = p.parent.map(|x| new_index[x]);
                if let Shape::Boolean { operands, .. } = &mut p.shape {
                    for o in operands {
                        o.population = new_index[o.population];
                    }
                }
                p
            })
            .collect();
        Ok(())
    }
}

/// A parsed gating file.
#[derive(Debug, Clone)]
pub enum GatingFile {
    /// A FlowJo workspace.
    Workspace(Workspace),
    /// A Gating-ML 2.0 document (one strategy for any sample).
    GatingMl(Strategy),
}

/// Read a FlowJo workspace or a Gating-ML 2.0 document, telling them apart by the root element.
pub fn read_gating_file(path: &Path) -> openreadout_core::Result<GatingFile> {
    let text = xml::read_text(path)?;
    let root_name = xml::root_local_name(&text).unwrap_or_default();
    if root_name == "Workspace" {
        return Ok(GatingFile::Workspace(parse_workspace(&text, path)?));
    }
    if root_name == "Gating-ML" {
        return Ok(GatingFile::GatingMl(parse_gating_ml(&text)?));
    }
    Err(openreadout_core::Error::unsupported(
        WSP_FORMAT_ID,
        format!(
            "{}: root element <{root_name}> is neither a FlowJo <Workspace> nor <Gating-ML>",
            path.display()
        ),
        "Pass a FlowJo 10 workspace (.wsp) with --workspace or a Gating-ML 2.0 file with --gatingml.",
    ))
}
