//! Apply a gating strategy to events.
//!
//! Every step is per event (compensation, transforms, gate tests, parent and Boolean
//! combinations), so the evaluator runs on any block of rows and callers stream a file in
//! chunks. Input values are FCS scale values ([`crate::scale_columns`]).

use std::collections::HashMap;

use openreadout_core::{Error, Result};

use super::{
    BoolOp, CompMatrix, CompensationRef, CoordinateSpace, DimensionSource, Population, Prepared,
    Shape, Solver, Strategy, Transform, WSP_FORMAT_ID,
};

/// Membership count of one population.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopulationCount {
    /// Index in [`Strategy::populations`].
    pub population: usize,
    /// Events in the population.
    pub count: u64,
}

struct CompPlan {
    solver: Solver,
    detector_cols: Vec<usize>,
    output_cols: Vec<usize>,
    /// Column → compensated output position.
    output_of: HashMap<usize, usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ValueKey {
    comp: Option<usize>,
    col: usize,
    transform: Option<usize>,
}

#[derive(Clone, Copy)]
enum DimPlan {
    Column(ValueKey),
    Ratio {
        comp: Option<usize>,
        x: usize,
        y: usize,
        ratio: usize,
        transform: Option<usize>,
    },
}

enum ShapePlan {
    Rect(Vec<(Option<f64>, Option<f64>)>),
    Polygon {
        vertices: Vec<[f64; 2]>,
        bbox: [f64; 4],
    },
    Ellipsoid {
        mean: Vec<f64>,
        inverse: Vec<Vec<f64>>,
        distance_square: f64,
    },
    Boolean {
        op: BoolOp,
        operands: Vec<(usize, bool)>,
    },
}

struct GatePlan {
    dims: Vec<DimPlan>,
    shape: ShapePlan,
    complement: bool,
    parent: Option<usize>,
}

/// A strategy bound to one file's parameters, ready to evaluate blocks of events.
pub struct Evaluator {
    columns: usize,
    comps: Vec<CompPlan>,
    transforms: Vec<Prepared>,
    gates: Vec<GatePlan>,
    /// Compensations in use, for reports: (reference label, matrix name, detectors).
    pub compensations_used: Vec<(String, String, Vec<String>)>,
}

impl std::fmt::Debug for Evaluator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Evaluator")
            .field("populations", &self.gates.len())
            .field("compensations", &self.comps.len())
            .finish_non_exhaustive()
    }
}

fn unsupported(feature: String, hint: &str) -> Error {
    Error::unsupported(WSP_FORMAT_ID, feature, hint)
}

/// Find a parameter column: `$PnN`, then `$PnS`, then FlowJo's `$PnN` spelling (`/` → `_`).
fn find_column(name: &str, names: &[String], labels: &[Option<String>]) -> Option<usize> {
    names
        .iter()
        .position(|n| n == name)
        .or_else(|| names.iter().position(|n| n.trim() == name.trim()))
        .or_else(|| labels.iter().position(|l| l.as_deref() == Some(name)))
        .or_else(|| names.iter().position(|n| n.replace('/', "_") == name))
}

impl Evaluator {
    /// Bind `strategy` to a file whose parameters are `names` (`$PnN`) and `labels` (`$PnS`).
    /// `file_matrix` is the file's own spillover matrix (used by `FCS` compensation references).
    pub fn new(
        strategy: &Strategy,
        names: &[String],
        labels: &[Option<String>],
        file_matrix: Option<&CompMatrix>,
    ) -> Result<Evaluator> {
        let mut ev = Evaluator {
            columns: names.len(),
            comps: Vec::new(),
            transforms: Vec::new(),
            gates: Vec::new(),
            compensations_used: Vec::new(),
        };
        let mut comp_index: HashMap<CompensationRef, Option<usize>> = HashMap::new();
        let mut transform_index: HashMap<String, usize> = HashMap::new();
        for p in &strategy.populations {
            let plan = ev.plan_gate(
                p,
                strategy,
                names,
                labels,
                file_matrix,
                &mut comp_index,
                &mut transform_index,
            )?;
            ev.gates.push(plan);
        }
        Ok(ev)
    }

    fn comp_for(
        &mut self,
        r: &CompensationRef,
        strategy: &Strategy,
        names: &[String],
        labels: &[Option<String>],
        file_matrix: Option<&CompMatrix>,
        cache: &mut HashMap<CompensationRef, Option<usize>>,
    ) -> Result<Option<usize>> {
        if let Some(&c) = cache.get(r) {
            return Ok(c);
        }
        let matrix =
            match r {
                CompensationRef::Uncompensated => None,
                // Gating-ML: `FCS` without a spillover in the file means uncompensated.
                CompensationRef::FromFile => file_matrix,
                CompensationRef::Matrix(m) => Some(strategy.matrices.get(m).ok_or_else(|| {
                    Error::Usage(format!("compensation matrix {m} is not defined"))
                })?),
            };
        let idx = match matrix {
            None => None,
            Some(m) => {
                let solver = m.solver().map_err(|e| {
                    unsupported(e, "Check the matrix in the workspace or FCS file.")
                })?;
                let detector_cols = m
                    .detectors
                    .iter()
                    .map(|d| {
                        find_column(d, names, labels).ok_or_else(|| {
                            Error::Usage(format!(
                                "compensation matrix {} names detector {d}, which is not a parameter of this file",
                                m.name
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let output_cols = if m.fluorochromes.len() == m.detectors.len() {
                    detector_cols.clone()
                } else {
                    m.fluorochromes
                        .iter()
                        .map(|f| {
                            find_column(f, names, labels).ok_or_else(|| {
                                Error::Usage(format!(
                                    "spectral matrix {} unmixes into {f}, which is not a parameter of this file",
                                    m.name
                                ))
                            })
                        })
                        .collect::<Result<Vec<_>>>()?
                };
                let output_of = output_cols
                    .iter()
                    .enumerate()
                    .map(|(i, &c)| (c, i))
                    .collect();
                self.comps.push(CompPlan {
                    solver,
                    detector_cols,
                    output_cols,
                    output_of,
                });
                self.compensations_used.push((
                    r.label().to_string(),
                    m.name.clone(),
                    m.detectors.clone(),
                ));
                Some(self.comps.len() - 1)
            }
        };
        cache.insert(r.clone(), idx);
        Ok(idx)
    }

    fn transform_for(
        &mut self,
        id: &str,
        strategy: &Strategy,
        cache: &mut HashMap<String, usize>,
    ) -> Result<usize> {
        if let Some(&i) = cache.get(id) {
            return Ok(i);
        }
        let t = strategy.transforms.get(id).ok_or_else(|| {
            if let Some(what) = strategy.unsupported_transforms.get(id) {
                unsupported(
                    format!("{what} (used on {id})"),
                    "FlowJo transforms linear, log, logicle, biex and ArcSinh (fasinh) are evaluated; re-save the channel's transform as one of them in FlowJo.",
                )
            } else {
                Error::Usage(format!("transformation {id} is not defined"))
            }
        })?;
        let p = t
            .prepare()
            .map_err(|e| unsupported(e, "Correct the transform parameters in the gating file."))?;
        self.transforms.push(p);
        cache.insert(id.to_string(), self.transforms.len() - 1);
        Ok(self.transforms.len() - 1)
    }

    fn plan_gate(
        &mut self,
        p: &Population,
        strategy: &Strategy,
        names: &[String],
        labels: &[Option<String>],
        file_matrix: Option<&CompMatrix>,
        comp_cache: &mut HashMap<CompensationRef, Option<usize>>,
        xform_cache: &mut HashMap<String, usize>,
    ) -> Result<GatePlan> {
        let mut dims = Vec::new();
        for d in &p.dimensions {
            let comp = self.comp_for(
                &d.compensation,
                strategy,
                names,
                labels,
                file_matrix,
                comp_cache,
            )?;
            let transform = match &d.transform {
                Some(t) => Some(self.transform_for(t, strategy, xform_cache)?),
                None => None,
            };
            let plan = match &d.source {
                DimensionSource::Parameter(name) => {
                    let col = find_column(name, names, labels)
                        .or_else(|| {
                            // A Gating-ML fluorochrome name of the referenced matrix.
                            let CompensationRef::Matrix(m) = &d.compensation else {
                                return None;
                            };
                            let m = strategy.matrices.get(m)?;
                            let i = m.fluorochromes.iter().position(|f| f == name)?;
                            let det = m.detectors.get(i)?;
                            find_column(det, names, labels)
                        })
                        .ok_or_else(|| {
                            Error::Usage(format!(
                                "gate {} uses parameter '{name}', which this file does not have (parameters: {})",
                                p.path_string(),
                                names.join(", ")
                            ))
                        })?;
                    DimPlan::Column(ValueKey {
                        comp,
                        col,
                        transform,
                    })
                }
                DimensionSource::Ratio(id) => {
                    let ratio = self.transform_for(id, strategy, xform_cache)?;
                    let Some(Transform::Ratio {
                        numerator,
                        denominator,
                        ..
                    }) = strategy.transforms.get(id)
                    else {
                        return Err(Error::Usage(format!("{id} is not a ratio transformation")));
                    };
                    let col = |n: &str| {
                        find_column(n, names, labels).ok_or_else(|| {
                            Error::Usage(format!(
                                "ratio {id} uses parameter '{n}', which this file does not have"
                            ))
                        })
                    };
                    DimPlan::Ratio {
                        comp,
                        x: col(numerator)?,
                        y: col(denominator)?,
                        ratio,
                        transform,
                    }
                }
            };
            dims.push(plan);
        }
        // Moves stored coordinates into the space the values are compared in.
        let to_space = |v: f64, dim: usize| -> f64 {
            if p.coordinates == CoordinateSpace::Transformed {
                return v;
            }
            let t = match dims.get(dim) {
                Some(DimPlan::Column(k)) => k.transform,
                Some(DimPlan::Ratio { transform, .. }) => *transform,
                None => None,
            };
            t.map_or(v, |i| self.transforms[i].apply(v))
        };
        let shape = match &p.shape {
            Shape::Rectangle => ShapePlan::Rect(
                p.dimensions
                    .iter()
                    .enumerate()
                    .map(|(i, d)| (d.min.map(|v| to_space(v, i)), d.max.map(|v| to_space(v, i))))
                    .collect(),
            ),
            Shape::Polygon { vertices } => {
                let v: Vec<[f64; 2]> = vertices
                    .iter()
                    .map(|[x, y]| [to_space(*x, 0), to_space(*y, 1)])
                    .collect();
                polygon_plan(v)
            }
            Shape::DisplayEllipse { foci, edge } => {
                let range = |i: usize| match dims.get(i) {
                    Some(DimPlan::Column(k)) => k
                        .transform
                        .map_or(1.0, |t| self.transforms[t].display_range()),
                    _ => 1.0,
                };
                let v = display_ellipse_polygon(*foci, edge, [range(0), range(1)]).ok_or_else(
                    || {
                        Error::corrupt(
                            WSP_FORMAT_ID,
                            format!(
                                "ellipse gate {}: cannot determine its major axis",
                                p.path_string()
                            ),
                        )
                    },
                )?;
                polygon_plan(v)
            }
            Shape::Ellipsoid {
                mean,
                covariance,
                distance_square,
            } => {
                let inverse = super::invert(covariance).ok_or_else(|| {
                    Error::corrupt(
                        WSP_FORMAT_ID,
                        format!(
                            "ellipsoid gate {}: covariance matrix is singular",
                            p.path_string()
                        ),
                    )
                })?;
                ShapePlan::Ellipsoid {
                    mean: mean
                        .iter()
                        .enumerate()
                        .map(|(i, m)| to_space(*m, i))
                        .collect(),
                    inverse,
                    distance_square: *distance_square,
                }
            }
            Shape::Boolean { op, operands } => ShapePlan::Boolean {
                op: *op,
                operands: operands
                    .iter()
                    .map(|o| (o.population, o.complement))
                    .collect(),
            },
        };
        Ok(GatePlan {
            dims,
            shape,
            complement: p.complement,
            parent: p.parent,
        })
    }

    /// Membership of every population for a block of events (`columns[c][r]`, all columns the
    /// same length). Result: `[population][row]`.
    pub fn evaluate(&self, columns: &[Vec<f64>]) -> Result<Vec<Vec<bool>>> {
        if columns.len() != self.columns {
            return Err(Error::Usage(format!(
                "expected {} columns, got {}",
                self.columns,
                columns.len()
            )));
        }
        let n = columns.first().map_or(0, Vec::len);
        // Compensated outputs per matrix: [comp][output][row].
        let compensated: Vec<Vec<Vec<f64>>> = self
            .comps
            .iter()
            .map(|c| {
                let mut out = vec![vec![0.0; n]; c.output_cols.len()];
                let mut raw = vec![0.0; c.detector_cols.len()];
                let mut row = Vec::new();
                for r in 0..n {
                    for (j, &col) in c.detector_cols.iter().enumerate() {
                        raw[j] = columns[col][r];
                    }
                    c.solver.apply_row(&raw, &mut row);
                    for (i, v) in row.iter().enumerate() {
                        out[i][r] = *v;
                    }
                }
                out
            })
            .collect();
        let base = |comp: Option<usize>, col: usize| -> &[f64] {
            if let Some(ci) = comp
                && let Some(&o) = self.comps[ci].output_of.get(&col)
            {
                return &compensated[ci][o];
            }
            &columns[col]
        };
        let mut cache: HashMap<ValueKey, Vec<f64>> = HashMap::new();
        let mut value = |key: ValueKey| -> Vec<f64> {
            if key.transform.is_none() {
                return base(key.comp, key.col).to_vec();
            }
            cache
                .entry(key)
                .or_insert_with(|| {
                    let t = &self.transforms[key.transform.unwrap_or_default()];
                    base(key.comp, key.col)
                        .iter()
                        .map(|&v| t.apply(v))
                        .collect()
                })
                .clone()
        };
        let mut members: Vec<Vec<bool>> = Vec::with_capacity(self.gates.len());
        for g in &self.gates {
            let dim_values: Vec<Vec<f64>> = g
                .dims
                .iter()
                .map(|d| match *d {
                    DimPlan::Column(k) => value(k),
                    DimPlan::Ratio {
                        comp,
                        x,
                        y,
                        ratio,
                        transform,
                    } => {
                        let (xs, ys) = (base(comp, x), base(comp, y));
                        let r = &self.transforms[ratio];
                        xs.iter()
                            .zip(ys)
                            .map(|(&a, &b)| {
                                let v = r.ratio(a, b);
                                transform.map_or(v, |t| self.transforms[t].apply(v))
                            })
                            .collect()
                    }
                })
                .collect();
            let mut inside: Vec<bool> = match &g.shape {
                ShapePlan::Rect(bounds) => (0..n)
                    .map(|r| {
                        bounds.iter().zip(&dim_values).all(|(&(lo, hi), v)| {
                            let x = v[r];
                            lo.is_none_or(|l| x >= l) && hi.is_none_or(|h| x < h)
                        })
                    })
                    .collect(),
                ShapePlan::Polygon { vertices, bbox } => (0..n)
                    .map(|r| point_in_polygon(dim_values[0][r], dim_values[1][r], vertices, bbox))
                    .collect(),
                ShapePlan::Ellipsoid {
                    mean,
                    inverse,
                    distance_square,
                } => {
                    let k = mean.len();
                    let mut d = vec![0.0; k];
                    (0..n)
                        .map(|r| {
                            for i in 0..k {
                                d[i] = dim_values[i][r] - mean[i];
                            }
                            let mut s = 0.0;
                            for i in 0..k {
                                let mut row = 0.0;
                                for j in 0..k {
                                    row += d[j] * inverse[j][i];
                                }
                                s += row * d[i];
                            }
                            s <= *distance_square
                        })
                        .collect()
                }
                ShapePlan::Boolean { op, operands } => {
                    let get = |(i, c): &(usize, bool), r: usize| members[*i][r] != *c;
                    (0..n)
                        .map(|r| match op {
                            BoolOp::And => {
                                !operands.is_empty() && operands.iter().all(|o| get(o, r))
                            }
                            BoolOp::Or => operands.iter().any(|o| get(o, r)),
                            BoolOp::Not => operands.first().is_some_and(|o| !get(o, r)),
                        })
                        .collect()
                }
            };
            if g.complement {
                for v in &mut inside {
                    *v = !*v;
                }
            }
            if let Some(p) = g.parent {
                for (v, &pm) in inside.iter_mut().zip(&members[p]) {
                    *v &= pm;
                }
            }
            members.push(inside);
        }
        Ok(members)
    }
}

fn polygon_plan(vertices: Vec<[f64; 2]>) -> ShapePlan {
    let mut bbox = [
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for [x, y] in &vertices {
        bbox[0] = bbox[0].min(*x);
        bbox[1] = bbox[1].max(*x);
        bbox[2] = bbox[2].min(*y);
        bbox[3] = bbox[3].max(*y);
    }
    ShapePlan::Polygon { vertices, bbox }
}

/// Winding-number test (Sunday), inside when the winding count is odd; points outside the
/// bounding box are outside.
#[allow(clippy::many_single_char_names)] // geometry in the notation of its sources
fn point_in_polygon(x: f64, y: f64, v: &[[f64; 2]], bbox: &[f64; 4]) -> bool {
    if x < bbox[0] || x > bbox[1] || y < bbox[2] || y > bbox[3] || x.is_nan() || y.is_nan() {
        return false;
    }
    let n = v.len();
    let mut wind: i64 = 0;
    for i in 0..n {
        let a = v[i];
        let b = v[(i + 1) % n];
        let is_left = (b[0] - a[0]) * (y - a[1]) - (x - a[0]) * (b[1] - a[1]);
        if a[1] <= y {
            if y < b[1] && is_left > 0.0 {
                wind += 1;
            }
        } else if b[1] <= y && is_left < 0.0 {
            wind -= 1;
        }
    }
    wind % 2 != 0
}

/// FlowJo ellipse (foci and edge points in 256-bin display units) as a 128-vertex polygon in
/// transformed units: display / 256 × `range` per axis.
#[allow(clippy::many_single_char_names)] // geometry in the notation of its sources
fn display_ellipse_polygon(
    foci: [[f64; 2]; 2],
    edge: &[[f64; 2]],
    range: [f64; 2],
) -> Option<Vec<[f64; 2]>> {
    const N: usize = 128;
    let f0 = [foci[0][0] / 256.0, foci[0][1] / 256.0];
    let f1 = [foci[1][0] / 256.0, foci[1][1] / 256.0];
    let center = [f64::midpoint(f0[0], f1[0]), f64::midpoint(f0[1], f1[1])];
    let theta = ((f1[1] - f0[1]) / (f1[0] - f0[0])).atan();
    let (s, c) = theta.sin_cos();
    // Row vector times the rotation matrix [[c, -s], [s, c]].
    let rot = |p: [f64; 2]| [p[0] * c + p[1] * s, -p[0] * s + p[1] * c];
    let focus = rot([f0[0] - center[0], f0[1] - center[1]]);
    let e = |i: usize| -> Option<[f64; 2]> {
        let p = edge.get(i)?;
        let q = rot([p[0] / 256.0 - center[0], p[1] / 256.0 - center[1]]);
        Some([q[0].abs(), q[1].abs()])
    };
    let (rv1, rv3) = (e(0)?, e(2)?);
    let argmax = |v: [f64; 2]| usize::from(v[1] > v[0]);
    let (p1, p3) = (argmax(rv1), argmax(rv3));
    if p1 == p3 {
        return None;
    }
    let a = rv1[p1].max(rv3[p3]);
    let b = (focus[0] * focus[0] - a * a).abs().sqrt();
    // Inverse rotation of a row vector: times [[c, s], [-s, c]].
    let unrot = |p: [f64; 2]| [p[0] * c - p[1] * s, p[0] * s + p[1] * c];
    Some(
        (0..N)
            .map(|i| {
                let ang = 2.0 * std::f64::consts::PI * (i as f64 / N as f64);
                let (x, y) = if p1 == 0 {
                    (a * ang.cos(), b * ang.sin())
                } else {
                    (b * ang.cos(), a * ang.sin())
                };
                let q = unrot([x, y]);
                [(q[0] + center[0]) * range[0], (q[1] + center[1]) * range[1]]
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gating::{CoordinateSpace, Dimension, GateKind, Operand};

    fn dim(name: &str, min: Option<f64>, max: Option<f64>) -> Dimension {
        Dimension {
            source: DimensionSource::Parameter(name.into()),
            compensation: CompensationRef::Uncompensated,
            transform: None,
            min,
            max,
        }
    }

    fn pop(
        name: &str,
        parent: Option<usize>,
        kind: GateKind,
        dims: Vec<Dimension>,
        shape: Shape,
    ) -> Population {
        Population {
            name: name.into(),
            path: vec![name.into()],
            parent,
            kind,
            dimensions: dims,
            shape,
            complement: false,
            coordinates: CoordinateSpace::Transformed,
            gate_id: None,
            quadrant_gate: None,
            stored_count: None,
            owning_group: None,
        }
    }

    #[test]
    fn rectangle_polygon_boolean_and_parent() {
        let s = Strategy {
            populations: vec![
                pop(
                    "r",
                    None,
                    GateKind::Rectangle,
                    vec![dim("A", Some(1.0), Some(3.0))],
                    Shape::Rectangle,
                ),
                pop(
                    "p",
                    Some(0),
                    GateKind::Polygon,
                    vec![dim("A", None, None), dim("B", None, None)],
                    Shape::Polygon {
                        vertices: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
                    },
                ),
                pop(
                    "not_r",
                    None,
                    GateKind::Boolean,
                    vec![],
                    Shape::Boolean {
                        op: BoolOp::Not,
                        operands: vec![Operand {
                            population: 0,
                            complement: false,
                        }],
                    },
                ),
            ],
            ..Default::default()
        };
        let names = vec!["A".to_string(), "B".to_string()];
        let ev = Evaluator::new(&s, &names, &[None, None], None).unwrap();
        let cols = vec![vec![0.5, 1.0, 2.0, 3.0], vec![5.0, 5.0, 20.0, 5.0]];
        let m = ev.evaluate(&cols).unwrap();
        assert_eq!(m[0], vec![false, true, true, false]);
        assert_eq!(m[1], vec![false, true, false, false]);
        assert_eq!(m[2], vec![true, false, false, true]);
    }

    #[test]
    fn ellipse_matches_its_axes() {
        // FlowJo foci on the x axis around (128, 128), semi-major 64 display units.
        let foci = [[88.0, 128.0], [168.0, 128.0]];
        let edge = vec![[192.0, 128.0], [64.0, 128.0], [128.0, 176.0], [128.0, 80.0]];
        let v = display_ellipse_polygon(foci, &edge, [1.0, 1.0]).unwrap();
        assert_eq!(v.len(), 128);
        assert!((v[0][0] - 0.75).abs() < 1e-12 && (v[0][1] - 0.5).abs() < 1e-12);
        // b = sqrt(|f² − a²|) = sqrt(0.25² − 0.15625²)
        let b = (0.25f64.powi(2) - (40.0f64 / 256.0).powi(2)).sqrt();
        assert!((v[32][1] - (0.5 + b)).abs() < 1e-12);
    }

    #[test]
    fn winding_rule_counts_odd_crossings() {
        let v = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        let plan = polygon_plan(v);
        let ShapePlan::Polygon { vertices, bbox } = &plan else {
            panic!()
        };
        assert!(point_in_polygon(2.0, 2.0, vertices, bbox));
        assert!(!point_in_polygon(5.0, 2.0, vertices, bbox));
        assert!(point_in_polygon(0.0, 2.0, vertices, bbox));
        assert!(!point_in_polygon(f64::NAN, 2.0, vertices, bbox));
    }
}
