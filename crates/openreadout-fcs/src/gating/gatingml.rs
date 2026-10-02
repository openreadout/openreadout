//! Gating-ML 2.0 documents (ISAC): gates, transformations and spectrum matrices at the top
//! level of `<gating:Gating-ML>`, gates linked by `gating:parent_id`.

use std::collections::{BTreeMap, HashMap};

use openreadout_core::{Error, Result};
use roxmltree::Node;

use super::xml::{attr, child, children, elements, line, num_attr, parse};
use super::{
    BoolOp, CompMatrix, CompensationRef, CoordinateSpace, Dimension, DimensionSource,
    GATING_ML_FORMAT_ID, GateKind, Operand, Population, Shape, Strategy, Transform, invert,
};

fn corrupt(n: Node<'_, '_>, msg: impl std::fmt::Display) -> Error {
    Error::corrupt(GATING_ML_FORMAT_ID, format!("line {}: {msg}", line(n)))
}

fn compensation_ref(n: Node<'_, '_>) -> CompensationRef {
    match attr(n, "compensation-ref").map(str::trim) {
        None | Some("" | "uncompensated") => CompensationRef::Uncompensated,
        Some("FCS") => CompensationRef::FromFile,
        Some(m) => CompensationRef::Matrix(m.to_string()),
    }
}

/// `fcs-dimension` name or `new-dimension` transformation reference under `n`.
fn dimension_source(n: Node<'_, '_>) -> Result<DimensionSource> {
    if let Some(d) = child(n, "fcs-dimension") {
        return attr(d, "name")
            .map(|s| DimensionSource::Parameter(s.to_string()))
            .ok_or_else(|| corrupt(d, "fcs-dimension without a name"));
    }
    if let Some(d) = child(n, "new-dimension") {
        return attr(d, "transformation-ref")
            .map(|s| DimensionSource::Ratio(s.to_string()))
            .ok_or_else(|| corrupt(d, "new-dimension without a transformation-ref"));
    }
    Err(corrupt(
        n,
        "dimension has neither fcs-dimension nor new-dimension",
    ))
}

fn parse_dimension(n: Node<'_, '_>) -> Result<Dimension> {
    Ok(Dimension {
        source: dimension_source(n)?,
        compensation: compensation_ref(n),
        transform: attr(n, "transformation-ref").map(str::to_string),
        min: num_attr(n, "min"),
        max: num_attr(n, "max"),
    })
}

fn value_of(n: Node<'_, '_>) -> Result<f64> {
    num_attr(n, "value").ok_or_else(|| corrupt(n, "missing or non-numeric value"))
}

fn vertex(n: Node<'_, '_>) -> Result<[f64; 2]> {
    let c: Vec<f64> = children(n, "coordinate")
        .map(value_of)
        .collect::<Result<_>>()?;
    match c.as_slice() {
        [x, y] => Ok([*x, *y]),
        _ => Err(corrupt(n, "a vertex needs exactly two coordinates")),
    }
}

/// Parse a `transforms:transformation` child element into a transform.
pub(crate) fn parse_transform(n: Node<'_, '_>, format: &'static str) -> Result<Transform> {
    let req = |k: &str| {
        num_attr(n, k).ok_or_else(|| {
            Error::corrupt(
                format,
                format!(
                    "line {}: {} needs a numeric {k}",
                    line(n),
                    n.tag_name().name()
                ),
            )
        })
    };
    Ok(match n.tag_name().name() {
        "flin" => Transform::Linear {
            t: req("T")?,
            a: req("A")?,
        },
        "flog" => Transform::Log {
            t: req("T")?,
            m: req("M")?,
        },
        "fasinh" => Transform::Asinh {
            t: req("T")?,
            m: req("M")?,
            a: req("A")?,
        },
        "logicle" => Transform::Logicle {
            t: req("T")?,
            w: req("W")?,
            m: req("M")?,
            a: req("A")?,
        },
        "hyperlog" => Transform::Hyperlog {
            t: req("T")?,
            w: req("W")?,
            m: req("M")?,
            a: req("A")?,
        },
        "fratio" => {
            let dims: Vec<String> = children(n, "fcs-dimension")
                .filter_map(|d| attr(d, "name").map(str::to_string))
                .collect();
            let [x, y] = <[String; 2]>::try_from(dims).map_err(|_| {
                Error::corrupt(
                    format,
                    format!("line {}: fratio needs two fcs-dimension names", line(n)),
                )
            })?;
            Transform::Ratio {
                numerator: x,
                denominator: y,
                a: req("A")?,
                b: req("B")?,
                c: req("C")?,
            }
        }
        other => {
            return Err(Error::unsupported(
                format,
                format!("transformation <{other}> (line {})", line(n)),
                "Gating-ML 2.0 transformations flin, flog, fasinh, logicle, hyperlog and fratio are supported.",
            ));
        }
    })
}

fn parse_matrix(n: Node<'_, '_>) -> Result<(String, CompMatrix)> {
    let id = attr(n, "id").ok_or_else(|| corrupt(n, "spectrumMatrix without an id"))?;
    let names = |local: &'static str| -> Vec<String> {
        child(n, local)
            .map(|c| {
                children(c, "fcs-dimension")
                    .filter_map(|d| attr(d, "name").map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let fluorochromes = names("fluorochromes");
    let detectors = names("detectors");
    let mut values: Vec<Vec<f64>> = children(n, "spectrum")
        .map(|s| children(s, "coefficient").map(value_of).collect())
        .collect::<Result<_>>()?;
    if values.len() != fluorochromes.len() || values.iter().any(|r| r.len() != detectors.len()) {
        return Err(corrupt(
            n,
            format!(
                "spectrumMatrix {id}: {} fluorochromes × {} detectors, but {} spectrum rows",
                fluorochromes.len(),
                detectors.len(),
                values.len()
            ),
        ));
    }
    if attr(n, "matrix-inverted-already").is_some_and(|v| v.trim() == "true") {
        values = invert(&values).ok_or_else(|| {
            corrupt(
                n,
                format!("spectrumMatrix {id} is inverted already but not invertible"),
            )
        })?;
    }
    let spectral = detectors.len() > fluorochromes.len();
    Ok((
        id.to_string(),
        CompMatrix {
            name: id.to_string(),
            detectors,
            fluorochromes,
            values,
            spectral,
        },
    ))
}

/// A gate before references are resolved.
struct RawGate {
    name: String,
    parent: Option<String>,
    kind: GateKind,
    dimensions: Vec<Dimension>,
    shape: RawShape,
    quadrant_gate: Option<String>,
    gate_id: String,
}

enum RawShape {
    Ready(Shape),
    Boolean(BoolOp, Vec<(String, bool)>),
}

fn parse_gate(n: Node<'_, '_>, out: &mut Vec<RawGate>) -> Result<()> {
    let local = n.tag_name().name();
    if !matches!(
        local,
        "RectangleGate" | "PolygonGate" | "EllipsoidGate" | "QuadrantGate" | "BooleanGate"
    ) {
        return Ok(());
    }
    let id = attr(n, "id")
        .ok_or_else(|| corrupt(n, format!("{local} without a gating:id")))?
        .to_string();
    let parent = attr(n, "parent_id").map(str::to_string);
    let dims =
        || -> Result<Vec<Dimension>> { children(n, "dimension").map(parse_dimension).collect() };
    let simple = |kind, dimensions, shape| RawGate {
        name: id.clone(),
        parent: parent.clone(),
        kind,
        dimensions,
        shape: RawShape::Ready(shape),
        quadrant_gate: None,
        gate_id: id.clone(),
    };
    match local {
        "RectangleGate" => {
            let d = dims()?;
            if d.is_empty() {
                return Err(corrupt(n, format!("rectangle gate {id} has no dimension")));
            }
            out.push(simple(GateKind::Rectangle, d, Shape::Rectangle));
        }
        "PolygonGate" => {
            let d = dims()?;
            let vertices: Vec<[f64; 2]> =
                children(n, "vertex").map(vertex).collect::<Result<_>>()?;
            if d.len() != 2 || vertices.len() < 3 {
                return Err(corrupt(
                    n,
                    format!("polygon gate {id} needs 2 dimensions and at least 3 vertices"),
                ));
            }
            out.push(simple(GateKind::Polygon, d, Shape::Polygon { vertices }));
        }
        "EllipsoidGate" => {
            let d = dims()?;
            let mean: Vec<f64> = child(n, "mean")
                .map(|m| {
                    children(m, "coordinate")
                        .map(value_of)
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            let covariance: Vec<Vec<f64>> = child(n, "covarianceMatrix")
                .map(|c| {
                    children(c, "row")
                        .map(|r| {
                            children(r, "entry")
                                .map(value_of)
                                .collect::<Result<Vec<_>>>()
                        })
                        .collect::<Result<Vec<_>>>()
                })
                .transpose()?
                .unwrap_or_default();
            let distance_square = child(n, "distanceSquare")
                .map(value_of)
                .transpose()?
                .ok_or_else(|| corrupt(n, format!("ellipsoid gate {id} has no distanceSquare")))?;
            let k = d.len();
            if k < 2
                || mean.len() != k
                || covariance.len() != k
                || covariance.iter().any(|r| r.len() != k)
            {
                return Err(corrupt(
                    n,
                    format!(
                        "ellipsoid gate {id}: mean and covariance must match its {k} dimensions"
                    ),
                ));
            }
            out.push(simple(
                GateKind::Ellipsoid,
                d,
                Shape::Ellipsoid {
                    mean,
                    covariance,
                    distance_square,
                },
            ));
        }
        "QuadrantGate" => {
            struct Divider {
                id: String,
                dim: Dimension,
                values: Vec<f64>,
            }
            let dividers: Vec<Divider> = children(n, "divider")
                .map(|dv| {
                    let mut values: Vec<f64> = children(dv, "value")
                        .map(|v| {
                            v.text()
                                .and_then(|t| t.trim().parse::<f64>().ok())
                                .ok_or_else(|| corrupt(v, "divider value is not a number"))
                        })
                        .collect::<Result<_>>()?;
                    values.sort_by(f64::total_cmp);
                    Ok(Divider {
                        id: attr(dv, "id")
                            .ok_or_else(|| corrupt(dv, "divider without an id"))?
                            .to_string(),
                        dim: Dimension {
                            source: dimension_source(dv)?,
                            compensation: compensation_ref(dv),
                            transform: attr(dv, "transformation-ref").map(str::to_string),
                            min: None,
                            max: None,
                        },
                        values,
                    })
                })
                .collect::<Result<_>>()?;
            if dividers.is_empty() {
                return Err(corrupt(n, format!("quadrant gate {id} has no divider")));
            }
            for q in children(n, "Quadrant") {
                let qid = attr(q, "id").ok_or_else(|| corrupt(q, "Quadrant without an id"))?;
                let mut dimensions = Vec::new();
                for pos in children(q, "position") {
                    let dref = attr(pos, "divider_ref")
                        .ok_or_else(|| corrupt(pos, "position without a divider_ref"))?;
                    let loc = num_attr(pos, "location")
                        .ok_or_else(|| corrupt(pos, "position without a numeric location"))?;
                    let dv = dividers
                        .iter()
                        .find(|d| d.id == dref)
                        .ok_or_else(|| corrupt(pos, format!("unknown divider {dref}")))?;
                    let mut dim = dv.dim.clone();
                    dim.min = dv.values.iter().copied().rfind(|v| *v <= loc);
                    dim.max = dv.values.iter().copied().find(|v| *v > loc);
                    dimensions.push(dim);
                }
                out.push(RawGate {
                    name: qid.to_string(),
                    parent: parent.clone(),
                    kind: GateKind::Quadrant,
                    dimensions,
                    shape: RawShape::Ready(Shape::Rectangle),
                    quadrant_gate: Some(id.clone()),
                    gate_id: id.clone(),
                });
            }
        }
        "BooleanGate" => {
            let (op, el) = [
                ("and", BoolOp::And),
                ("or", BoolOp::Or),
                ("not", BoolOp::Not),
            ]
            .into_iter()
            .find_map(|(name, op)| {
                elements(n)
                    .find(|c| c.tag_name().name() == name)
                    .map(|c| (op, c))
            })
            .ok_or_else(|| corrupt(n, format!("boolean gate {id} has no and/or/not")))?;
            let refs: Vec<(String, bool)> = children(el, "gateReference")
                .map(|r| {
                    Ok((
                        attr(r, "ref")
                            .ok_or_else(|| corrupt(r, "gateReference without a ref"))?
                            .to_string(),
                        attr(r, "use-as-complement").is_some_and(|v| v.trim() == "true"),
                    ))
                })
                .collect::<Result<_>>()?;
            if refs.is_empty() || (op == BoolOp::Not && refs.len() != 1) {
                return Err(corrupt(
                    n,
                    format!("boolean gate {id}: `not` takes one reference, and/or at least one"),
                ));
            }
            out.push(RawGate {
                name: id.clone(),
                parent,
                kind: GateKind::Boolean,
                dimensions: Vec::new(),
                shape: RawShape::Boolean(op, refs),
                quadrant_gate: None,
                gate_id: id,
            });
        }
        _ => {}
    }
    Ok(())
}

/// Parse a Gating-ML 2.0 document into a strategy.
pub fn parse_gating_ml(text: &str) -> Result<Strategy> {
    let doc = parse(text, GATING_ML_FORMAT_ID)?;
    let root = doc.root_element();
    if root.tag_name().name() != "Gating-ML" {
        return Err(Error::corrupt(
            GATING_ML_FORMAT_ID,
            format!(
                "root element is <{}>, not <Gating-ML>",
                root.tag_name().name()
            ),
        ));
    }
    let mut transforms = BTreeMap::new();
    let mut matrices = BTreeMap::new();
    let mut raw = Vec::new();
    for n in elements(root) {
        match n.tag_name().name() {
            "transformation" => {
                let id = attr(n, "id").ok_or_else(|| corrupt(n, "transformation without an id"))?;
                let t = elements(n)
                    .next()
                    .ok_or_else(|| corrupt(n, format!("transformation {id} is empty")))?;
                transforms.insert(id.to_string(), parse_transform(t, GATING_ML_FORMAT_ID)?);
            }
            "spectrumMatrix" => {
                let (id, m) = parse_matrix(n)?;
                matrices.insert(id, m);
            }
            _ => parse_gate(n, &mut raw)?,
        }
    }
    build(raw, transforms, matrices)
}

fn build(
    raw: Vec<RawGate>,
    transforms: BTreeMap<String, Transform>,
    matrices: BTreeMap<String, CompMatrix>,
) -> Result<Strategy> {
    let err = |m: String| Error::corrupt(GATING_ML_FORMAT_ID, m);
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, g) in raw.iter().enumerate() {
        if index.insert(g.name.clone(), i).is_some() {
            return Err(err(format!("gate id {} is used twice", g.name)));
        }
    }
    let quadrant_gates: Vec<String> = raw.iter().filter_map(|g| g.quadrant_gate.clone()).collect();
    let mut populations = Vec::with_capacity(raw.len());
    for g in raw {
        let parent = match &g.parent {
            None => None,
            Some(p) => Some(*index.get(p).ok_or_else(|| {
                if quadrant_gates.contains(p) {
                    err(format!(
                        "gate {} names quadrant gate {p} as its parent; only single quadrants can be parents",
                        g.name
                    ))
                } else {
                    err(format!("gate {} has unknown parent {p}", g.name))
                }
            })?),
        };
        let shape = match g.shape {
            RawShape::Ready(s) => s,
            RawShape::Boolean(op, refs) => Shape::Boolean {
                op,
                operands: refs
                    .into_iter()
                    .map(|(r, complement)| {
                        index
                            .get(&r)
                            .map(|&population| Operand {
                                population,
                                complement,
                            })
                            .ok_or_else(|| {
                                err(format!(
                                    "boolean gate {} refers to unknown gate {r}",
                                    g.name
                                ))
                            })
                    })
                    .collect::<Result<_>>()?,
            },
        };
        for d in &g.dimensions {
            if let CompensationRef::Matrix(m) = &d.compensation
                && !matrices.contains_key(m)
            {
                return Err(err(format!(
                    "gate {} uses unknown compensation {m}",
                    g.name
                )));
            }
            if let Some(t) = &d.transform
                && !transforms.contains_key(t)
            {
                return Err(err(format!(
                    "gate {} uses unknown transformation {t}",
                    g.name
                )));
            }
            if let DimensionSource::Ratio(t) = &d.source
                && !matches!(transforms.get(t), Some(Transform::Ratio { .. }))
            {
                return Err(err(format!(
                    "gate {} uses {t}, which is not a ratio transformation",
                    g.name
                )));
            }
        }
        populations.push(Population {
            path: vec![g.name.clone()],
            name: g.name,
            parent,
            kind: g.kind,
            dimensions: g.dimensions,
            shape,
            complement: false,
            coordinates: CoordinateSpace::Transformed,
            gate_id: Some(g.gate_id),
            quadrant_gate: g.quadrant_gate,
            stored_count: None,
            owning_group: None,
        });
    }
    let mut s = Strategy {
        populations,
        transforms,
        matrices,
        unsupported_transforms: BTreeMap::new(),
    };
    s.sort_dependencies().map_err(err)?;
    // Paths, parents first (the order guarantees it).
    for i in 0..s.populations.len() {
        let mut path = match s.populations[i].parent {
            Some(p) => s.populations[p].path.clone(),
            None => Vec::new(),
        };
        if let Some(q) = s.populations[i].quadrant_gate.clone() {
            path.push(q);
        }
        path.push(s.populations[i].name.clone());
        s.populations[i].path = path;
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0"?>
<gating:Gating-ML xmlns:gating="http://www.isac-net.org/std/Gating-ML/v2.0/gating"
 xmlns:transforms="http://www.isac-net.org/std/Gating-ML/v2.0/transformations"
 xmlns:data-type="http://www.isac-net.org/std/Gating-ML/v2.0/datatypes">
 <transforms:transformation transforms:id="L"><transforms:logicle transforms:T="262144" transforms:W="0.5" transforms:M="4.5" transforms:A="0"/></transforms:transformation>
 <gating:RectangleGate gating:id="Child" gating:parent_id="Q++">
  <gating:dimension gating:compensation-ref="uncompensated" gating:min="1"><data-type:fcs-dimension data-type:name="A"/></gating:dimension>
 </gating:RectangleGate>
 <gating:QuadrantGate gating:id="Quad">
  <gating:divider gating:id="d1" gating:compensation-ref="FCS" gating:transformation-ref="L"><data-type:fcs-dimension data-type:name="A"/><gating:value>0.5</gating:value></gating:divider>
  <gating:divider gating:id="d2" gating:compensation-ref="FCS"><data-type:fcs-dimension data-type:name="B"/><gating:value>10</gating:value><gating:value>2</gating:value></gating:divider>
  <gating:Quadrant gating:id="Q++"><gating:position gating:divider_ref="d1" gating:location="1"/><gating:position gating:divider_ref="d2" gating:location="5"/></gating:Quadrant>
 </gating:QuadrantGate>
 <gating:BooleanGate gating:id="NotChild"><gating:not><gating:gateReference gating:ref="Child"/></gating:not></gating:BooleanGate>
</gating:Gating-ML>"#;

    #[test]
    fn parses_quadrants_parents_and_booleans() {
        let s = parse_gating_ml(DOC).unwrap();
        let q = s.find("/Quad/Q++").unwrap();
        let qp = &s.populations[q];
        assert_eq!(qp.kind, GateKind::Quadrant);
        assert_eq!(qp.dimensions[0].min, Some(0.5));
        assert_eq!(qp.dimensions[0].max, None);
        assert_eq!(qp.dimensions[1].min, Some(2.0));
        assert_eq!(qp.dimensions[1].max, Some(10.0));
        assert_eq!(qp.dimensions[0].compensation, CompensationRef::FromFile);
        let c = s.find("Child").unwrap();
        assert_eq!(s.populations[c].path_string(), "/Quad/Q++/Child");
        assert_eq!(s.populations[c].parent, Some(q));
        let b = s.find("NotChild").unwrap();
        assert!(b > c);
        assert!(matches!(
            s.transforms.get("L"),
            Some(Transform::Logicle { .. })
        ));
    }

    #[test]
    fn rejects_unknown_references() {
        let bad = DOC.replace("gating:ref=\"Child\"", "gating:ref=\"Nope\"");
        assert!(parse_gating_ml(&bad).is_err());
        let cyc = DOC.replace(
            "gating:id=\"Quad\"",
            "gating:id=\"Quad\" gating:parent_id=\"Child\"",
        );
        assert!(parse_gating_ml(&cyc).is_err());
    }
}
