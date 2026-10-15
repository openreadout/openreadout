//! Vector paths in figure coordinates (pixels at scale 1, y down).

use std::fmt::Write as _;

/// One segment of a [`Path`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    /// Start a new subpath.
    MoveTo(f32, f32),
    /// Straight line.
    LineTo(f32, f32),
    /// Quadratic Bézier: control point, end point.
    QuadTo(f32, f32, f32, f32),
    /// Cubic Bézier: two control points, end point.
    CubicTo(f32, f32, f32, f32, f32, f32),
    /// Close the current subpath.
    Close,
}

/// A vector path: subpaths of lines and curves. Non-finite coordinates are dropped when
/// added, so a path built from data with gaps stays drawable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    segs: Vec<Segment>,
    open: bool,
}

fn ok(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

impl Path {
    /// An empty path.
    pub fn new() -> Self {
        Path::default()
    }

    /// The segments.
    pub fn segments(&self) -> &[Segment] {
        &self.segs
    }

    /// True when nothing would be drawn.
    pub fn is_empty(&self) -> bool {
        !self
            .segs
            .iter()
            .any(|s| !matches!(s, Segment::MoveTo(..) | Segment::Close))
    }

    /// Start a subpath at `(x, y)`.
    pub fn move_to(&mut self, x: f32, y: f32) -> &mut Self {
        if ok(&[x, y]) {
            self.segs.push(Segment::MoveTo(x, y));
            self.open = true;
        }
        self
    }

    /// Line to `(x, y)` (a move when no subpath is open).
    pub fn line_to(&mut self, x: f32, y: f32) -> &mut Self {
        if ok(&[x, y]) {
            if self.open {
                self.segs.push(Segment::LineTo(x, y));
            } else {
                self.move_to(x, y);
            }
        }
        self
    }

    /// Quadratic curve.
    pub fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) -> &mut Self {
        if ok(&[x1, y1, x, y]) && self.open {
            self.segs.push(Segment::QuadTo(x1, y1, x, y));
        }
        self
    }

    /// Cubic curve.
    pub fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) -> &mut Self {
        if ok(&[x1, y1, x2, y2, x, y]) && self.open {
            self.segs.push(Segment::CubicTo(x1, y1, x2, y2, x, y));
        }
        self
    }

    /// Close the current subpath.
    pub fn close(&mut self) -> &mut Self {
        if self.open {
            self.segs.push(Segment::Close);
            self.open = false;
        }
        self
    }

    /// End the current subpath without closing it: the next point starts a new one (a gap).
    pub fn lift(&mut self) -> &mut Self {
        self.open = false;
        self
    }

    /// Append another path's subpaths.
    pub fn extend(&mut self, other: &Path) -> &mut Self {
        self.segs.extend_from_slice(&other.segs);
        self.open = other.open;
        self
    }

    /// A polyline through `points`; non-finite points break the line into pieces.
    pub fn polyline(points: impl IntoIterator<Item = (f32, f32)>) -> Path {
        let mut p = Path::new();
        for (x, y) in points {
            if ok(&[x, y]) {
                p.line_to(x, y);
            } else {
                p.lift();
            }
        }
        p
    }

    /// A closed polygon.
    pub fn polygon(points: impl IntoIterator<Item = (f32, f32)>) -> Path {
        let mut p = Path::polyline(points);
        p.close();
        p
    }

    /// A rectangle.
    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Path {
        Path::polygon([(x, y), (x + w, y), (x + w, y + h), (x, y + h)])
    }

    /// A rectangle with rounded corners of radius `r`.
    pub fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        if r <= 0.0 {
            return Path::rect(x, y, w, h);
        }
        // 0.5523: cubic approximation of a quarter circle
        let c = r * (1.0 - 0.552_284_8);
        let mut p = Path::new();
        p.move_to(x + r, y)
            .line_to(x + w - r, y)
            .cubic_to(x + w - c, y, x + w, y + c, x + w, y + r)
            .line_to(x + w, y + h - r)
            .cubic_to(x + w, y + h - c, x + w - c, y + h, x + w - r, y + h)
            .line_to(x + r, y + h)
            .cubic_to(x + c, y + h, x, y + h - c, x, y + h - r)
            .line_to(x, y + r)
            .cubic_to(x, y + c, x + c, y, x + r, y)
            .close();
        p
    }

    /// A circle.
    pub fn circle(cx: f32, cy: f32, r: f32) -> Path {
        let k = r * 0.552_284_8;
        let mut p = Path::new();
        p.move_to(cx + r, cy)
            .cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r)
            .cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy)
            .cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r)
            .cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy)
            .close();
        p
    }

    /// A straight segment.
    pub fn segment(x0: f32, y0: f32, x1: f32, y1: f32) -> Path {
        let mut p = Path::new();
        p.move_to(x0, y0).line_to(x1, y1);
        p
    }

    /// Bounding box `(x0, y0, x1, y1)` of the points (control points included).
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut b: Option<(f32, f32, f32, f32)> = None;
        let mut add = |x: f32, y: f32| {
            b = Some(match b {
                None => (x, y, x, y),
                Some((a, c, d, e)) => (a.min(x), c.min(y), d.max(x), e.max(y)),
            });
        };
        for s in &self.segs {
            match *s {
                Segment::MoveTo(x, y) | Segment::LineTo(x, y) => add(x, y),
                Segment::QuadTo(a, b, x, y) => {
                    add(a, b);
                    add(x, y);
                }
                Segment::CubicTo(a, b, c, d, x, y) => {
                    add(a, b);
                    add(c, d);
                    add(x, y);
                }
                Segment::Close => {}
            }
        }
        b
    }

    /// SVG path data, coordinates rounded to 0.01 px.
    pub fn to_svg_data(&self) -> String {
        let mut s = String::new();
        let n = |v: f32| fmt_coord(v);
        for seg in &self.segs {
            let _ = match *seg {
                Segment::MoveTo(x, y) => write!(s, "M{} {}", n(x), n(y)),
                Segment::LineTo(x, y) => write!(s, "L{} {}", n(x), n(y)),
                Segment::QuadTo(a, b, x, y) => {
                    write!(s, "Q{} {} {} {}", n(a), n(b), n(x), n(y))
                }
                Segment::CubicTo(a, b, c, d, x, y) => {
                    write!(s, "C{} {} {} {} {} {}", n(a), n(b), n(c), n(d), n(x), n(y))
                }
                Segment::Close => write!(s, "Z"),
            };
        }
        s
    }

    /// The same path in tiny-skia form, scaled by `k`.
    pub(crate) fn to_skia(&self, k: f32) -> Option<tiny_skia::Path> {
        let mut b = tiny_skia::PathBuilder::new();
        for seg in &self.segs {
            match *seg {
                Segment::MoveTo(x, y) => b.move_to(x * k, y * k),
                Segment::LineTo(x, y) => b.line_to(x * k, y * k),
                Segment::QuadTo(a, c, x, y) => b.quad_to(a * k, c * k, x * k, y * k),
                Segment::CubicTo(a, c, d, e, x, y) => {
                    b.cubic_to(a * k, c * k, d * k, e * k, x * k, y * k);
                }
                Segment::Close => b.close(),
            }
        }
        b.finish()
    }
}

/// A coordinate for SVG: two decimals at most, no trailing zeros.
pub(crate) fn fmt_coord(v: f32) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        let s = format!("{r:.2}");
        s.trim_end_matches('0').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polylines_break_at_gaps() {
        let p = Path::polyline([
            (0.0, 0.0),
            (1.0, 1.0),
            (f32::NAN, 0.0),
            (2.0, 2.0),
            (3.0, 3.0),
        ]);
        assert_eq!(p.to_svg_data(), "M0 0L1 1M2 2L3 3");
        assert_eq!(p.bounds(), Some((0.0, 0.0, 3.0, 3.0)));
        assert!(Path::new().is_empty());
        assert!(Path::polyline([(1.0, 1.0)]).is_empty());
        assert_eq!(fmt_coord(1.256), "1.26");
        assert_eq!(fmt_coord(-0.5), "-0.5");
    }
}
