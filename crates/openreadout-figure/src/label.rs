//! Labels that keep out of each other's way, and label "pills" that read on any image.

use crate::color::Color;
use crate::geom::Rect;
use crate::path::Path;
use crate::scene::{Anchor, Baseline, Figure, Stroke, TextStyle};

/// Where a label may go relative to its anchor point, tried in this order.
const CANDIDATES: [(f32, f32); 8] = [
    (1.0, -1.0),  // up right
    (1.0, 1.0),   // down right
    (-1.0, -1.0), // up left
    (-1.0, 1.0),  // down left
    (1.0, 0.0),   // right
    (-1.0, 0.0),  // left
    (0.0, -1.0),  // above
    (0.0, 1.0),   // below
];

/// Places label boxes near their anchors without overlapping each other, the anchors already
/// marked, or anything reserved; inside `bounds` whenever possible.
#[derive(Debug, Clone)]
pub struct LabelPlacer {
    bounds: Rect,
    taken: Vec<Rect>,
}

impl LabelPlacer {
    /// A placer for labels inside `bounds`.
    pub fn new(bounds: Rect) -> LabelPlacer {
        LabelPlacer {
            bounds,
            taken: Vec::new(),
        }
    }

    /// Keep labels off `r` (a marker, a title, a legend).
    pub fn reserve(&mut self, r: Rect) {
        self.taken.push(r);
    }

    /// True when `r` lies inside the bounds and overlaps nothing reserved.
    pub fn free(&self, r: &Rect) -> bool {
        self.bounds.contains_rect(r) && !self.taken.iter().any(|t| t.intersects(r))
    }

    /// Choose a spot for a `w` × `h` box near `(x, y)`, `gap` pixels from it; the spot with the
    /// least overlap wins (the first candidate on ties). The box is then reserved.
    pub fn place(&mut self, x: f32, y: f32, w: f32, h: f32, gap: f32) -> Rect {
        let mut best: Option<(f32, Rect)> = None;
        for ring in 0..3 {
            let g = gap + ring as f32 * h * 0.9;
            for &(dx, dy) in &CANDIDATES {
                let bx = if dx > 0.0 {
                    x + g
                } else if dx < 0.0 {
                    x - g - w
                } else {
                    x - w / 2.0
                };
                let by = if dy > 0.0 {
                    y + g * 0.6
                } else if dy < 0.0 {
                    y - g * 0.6 - h
                } else {
                    y - h / 2.0
                };
                let r = Rect::new(bx, by, w, h);
                let mut cost: f32 = self.taken.iter().map(|t| t.overlap_area(&r)).sum();
                // leaving the bounds costs as much as covering the outside part
                let inside = self.bounds.overlap_area(&r);
                cost += (w * h - inside) * 2.0;
                // farther rings cost a little
                cost += ring as f32 * 0.05 * w * h;
                if best.is_none_or(|(c, _)| cost < c - 1e-3) {
                    best = Some((cost, r));
                }
                if cost <= 1e-3 && ring == 0 {
                    break;
                }
            }
            if best.is_some_and(|(c, _)| c <= 1e-3) {
                break;
            }
        }
        let r = best.map_or(Rect::new(x, y, w, h), |(_, r)| r);
        self.taken.push(r);
        r
    }
}

/// How a label pill looks.
#[derive(Debug, Clone, PartialEq)]
pub struct Pill {
    /// Text style (alignment is set by the pill).
    pub text: TextStyle,
    /// Background.
    pub fill: Color,
    /// Outline.
    pub stroke: Option<Stroke>,
    /// Padding around the text, horizontal and vertical.
    pub pad: (f32, f32),
}

impl Pill {
    /// Size of the pill for `text`.
    pub fn size(&self, text: &str) -> (f32, f32) {
        let b = self
            .text
            .clone()
            .align(Anchor::Start, Baseline::Alphabetic)
            .bounds(0.0, 0.0, text);
        (b.w + 2.0 * self.pad.0, b.h + 2.0 * self.pad.1)
    }

    /// Draw the pill with its box at `r` (from [`Pill::size`]).
    pub fn draw(&self, fig: &mut Figure, r: Rect, text: &str) {
        fig.shape(
            Path::round_rect(r.x, r.y, r.w, r.h, r.h / 2.0),
            Some(self.fill),
            self.stroke.clone(),
            None,
        );
        let st = self.text.clone().align(Anchor::Middle, Baseline::Middle);
        let (cx, cy) = r.center();
        // optical centre: the box spans cap height to descender
        let m = crate::font::metrics(st.face, st.size);
        fig.text(cx, cy - (m.descent - 0.0) / 2.0 + 0.5, text, &st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_avoid_each_other() {
        let mut p = LabelPlacer::new(Rect::new(0.0, 0.0, 200.0, 200.0));
        let a = p.place(100.0, 100.0, 40.0, 12.0, 4.0);
        let b = p.place(100.0, 100.0, 40.0, 12.0, 4.0);
        assert!(!a.intersects(&b), "{a:?} {b:?}");
        // near the right edge, the label goes left
        let c = p.place(195.0, 50.0, 40.0, 12.0, 4.0);
        assert!(c.right() <= 200.0, "{c:?}");
    }
}
