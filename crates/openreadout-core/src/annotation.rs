//! The caller's marks on a picture: one shape for previews (`annotations`) and plots
//! (`annotate`). Coordinates are those the picture shows: full-resolution pixels on images (as
//! on the rulers), axis units on plots (as on the tick labels), wells on plates.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// What an annotation draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AnnotationType {
    /// a dot at x, y (with its label beside it)
    Point,
    /// a rectangle: corner x, y and width, height
    Box,
    /// a closed outline through points
    Polygon,
    /// an open line through points
    Line,
    /// an arrow from x, y (where the label goes) to x2, y2
    Arrow,
    /// the label alone, at x, y
    Text,
    /// a vertical line at x
    Vline,
    /// a horizontal line at y
    Hline,
    /// a vertical band from x to x2
    Vband,
    /// a horizontal band from y to y2
    Hband,
    /// a ring around a plate well (well; previews of plates only)
    Well,
}

impl AnnotationType {
    /// The name as written in a request.
    pub fn name(self) -> &'static str {
        match self {
            AnnotationType::Point => "point",
            AnnotationType::Box => "box",
            AnnotationType::Polygon => "polygon",
            AnnotationType::Line => "line",
            AnnotationType::Arrow => "arrow",
            AnnotationType::Text => "text",
            AnnotationType::Vline => "vline",
            AnnotationType::Hline => "hline",
            AnnotationType::Vband => "vband",
            AnnotationType::Hband => "hband",
            AnnotationType::Well => "well",
        }
    }
}

/// One annotation, in the coordinates the picture shows: full-resolution pixels on images (as
/// on the rulers), axis units on plots (as on the tick labels), and on plate curve grids the
/// time and value axes every card shares.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Annotation {
    /// What to draw.
    #[serde(rename = "type")]
    pub kind: AnnotationType,
    /// point, text: position; box: corner; arrow: tail; vline, vband: x (vband from x to x2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    /// point, text: position; box: corner; arrow: tail; hline, hband: y (hband from y to y2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    /// arrow: head; vband: other edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x2: Option<f64>,
    /// arrow: head; hband: other edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y2: Option<f64>,
    /// box: width.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// box: height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// polygon, line: the vertices, `[[x, y], ...]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<[f64; 2]>,
    /// Text drawn with the mark (the text itself for type text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `accent` (default, warm red), `ink`, `muted`, `white`, another colour name or `#RRGGBB`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Trace previews with several panels: the channel whose panel it goes in (default: the
    /// first panel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
    /// Plate previews: the well (e.g. `B3`) for type well, or the one card a mark goes in on a
    /// curve grid (default: every card).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub well: Option<String>,
    /// Plots with small multiples: the one small plot it goes in, by its title (e.g. `B3` or
    /// `sweep 6`; default: every one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facet: Option<String>,
}

impl Annotation {
    /// An annotation of `kind` with nothing set.
    pub fn new(kind: AnnotationType) -> Annotation {
        Annotation {
            kind,
            x: None,
            y: None,
            x2: None,
            y2: None,
            width: None,
            height: None,
            points: Vec::new(),
            label: None,
            color: None,
            channel: None,
            well: None,
            facet: None,
        }
    }

    /// A point with a label.
    pub fn point(x: f64, y: f64, label: &str) -> Annotation {
        Annotation {
            x: Some(x),
            y: Some(y),
            label: Some(label.to_string()),
            ..Annotation::new(AnnotationType::Point)
        }
    }

    /// A label alone at (x, y).
    pub fn text(x: f64, y: f64, label: &str) -> Annotation {
        Annotation {
            x: Some(x),
            y: Some(y),
            label: Some(label.to_string()),
            ..Annotation::new(AnnotationType::Text)
        }
    }

    /// A rectangle with its corner at (x, y).
    pub fn rect(x: f64, y: f64, width: f64, height: f64) -> Annotation {
        Annotation {
            x: Some(x),
            y: Some(y),
            width: Some(width),
            height: Some(height),
            ..Annotation::new(AnnotationType::Box)
        }
    }

    /// A straight line from (x0, y0) to (x1, y1).
    pub fn segment(x0: f64, y0: f64, x1: f64, y1: f64) -> Annotation {
        Annotation {
            points: vec![[x0, y0], [x1, y1]],
            ..Annotation::new(AnnotationType::Line)
        }
    }

    /// Check that the annotation has what its type needs (a usage error naming what is
    /// missing).
    pub fn check(&self) -> Result<()> {
        let k = self.kind.name();
        let need = |v: Option<f64>, what: &str| match v {
            Some(v) if v.is_finite() => Ok(()),
            _ => Err(Error::Usage(format!(
                "annotation type {k} needs a finite {what}"
            ))),
        };
        match self.kind {
            AnnotationType::Point => {
                need(self.x, "x")?;
                need(self.y, "y")?;
            }
            AnnotationType::Text => {
                need(self.x, "x")?;
                need(self.y, "y")?;
                if self.label.as_deref().is_none_or(|l| l.trim().is_empty()) {
                    return Err(Error::Usage("annotation type text needs a label".into()));
                }
            }
            AnnotationType::Box => {
                need(self.x, "x")?;
                need(self.y, "y")?;
                need(self.width, "width")?;
                need(self.height, "height")?;
            }
            AnnotationType::Arrow => {
                need(self.x, "x")?;
                need(self.y, "y")?;
                need(self.x2, "x2")?;
                need(self.y2, "y2")?;
            }
            AnnotationType::Polygon | AnnotationType::Line => {
                let min = if self.kind == AnnotationType::Polygon {
                    3
                } else {
                    2
                };
                if self.points.len() < min || self.points.iter().flatten().any(|v| !v.is_finite()) {
                    return Err(Error::Usage(format!(
                        "annotation type {k} needs at least {min} finite points [[x, y], ...]"
                    )));
                }
            }
            AnnotationType::Vline => need(self.x, "x")?,
            AnnotationType::Hline => need(self.y, "y")?,
            AnnotationType::Vband => {
                need(self.x, "x")?;
                need(self.x2, "x2")?;
            }
            AnnotationType::Hband => {
                need(self.y, "y")?;
                need(self.y2, "y2")?;
            }
            AnnotationType::Well => {
                let w = self.well.as_deref().unwrap_or("");
                if crate::plate::parse_well(w).is_none() {
                    return Err(Error::Usage(format!(
                        "annotation type well needs a well such as \"B3\" (got '{w}')"
                    )));
                }
            }
        }
        if let Some(w) = self.well.as_deref()
            && crate::plate::parse_well(w).is_none()
        {
            return Err(Error::Usage(format!(
                "annotation well '{w}' is not a well name such as B3"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_shape_parses_and_checks() {
        let a: Annotation = serde_json::from_value(serde_json::json!(
            {"type": "vband", "x": 206.5, "x2": 240, "label": "two sequences"}
        ))
        .unwrap();
        assert_eq!(
            (a.kind, a.x, a.x2),
            (AnnotationType::Vband, Some(206.5), Some(240.0))
        );
        a.check().unwrap();
        let e = Annotation::new(AnnotationType::Vband).check().unwrap_err();
        assert!(e.to_string().contains("needs a finite x"), "{e}");
        // the old plot spelling is refused with the field it does not know
        let old = serde_json::from_value::<Annotation>(serde_json::json!({"x": [1, 2]}));
        assert!(old.is_err());
    }
}
