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

/// Cells per side of the grid that finds the boxes near a candidate spot.
const GRID: usize = 64;

/// Places label boxes near their anchors without overlapping each other, the anchors already
/// marked, or anything reserved; inside `bounds` whenever possible.
///
/// The boxes are also kept in a coarse grid over `bounds` (boxes outside it go in the edge
/// cells), so a spot is checked against the boxes near it only and placing many labels stays
/// fast.
#[derive(Debug, Clone)]
pub struct LabelPlacer {
    bounds: Rect,
    taken: Vec<Rect>,
    /// Indices into `taken`, per grid cell, row-major.
    grid: Vec<Vec<u32>>,
}

impl LabelPlacer {
    /// A placer for labels inside `bounds`.
    pub fn new(bounds: Rect) -> LabelPlacer {
        LabelPlacer {
            bounds,
            taken: Vec::new(),
            grid: vec![Vec::new(); GRID * GRID],
        }
    }

    /// The grid cells `r` covers, as column and row ranges (clamped to the grid).
    fn cells(
        &self,
        r: &Rect,
    ) -> (
        std::ops::RangeInclusive<usize>,
        std::ops::RangeInclusive<usize>,
    ) {
        let cw = (self.bounds.w / GRID as f32).max(1.0);
        let ch = (self.bounds.h / GRID as f32).max(1.0);
        // `as usize` saturates: left of the bounds (and NaN) is cell 0
        let col = |x: f32| (((x - self.bounds.x) / cw) as usize).min(GRID - 1);
        let row = |y: f32| (((y - self.bounds.y) / ch) as usize).min(GRID - 1);
        let (x0, x1) = (r.x.min(r.right()), r.x.max(r.right()));
        let (y0, y1) = (r.y.min(r.bottom()), r.y.max(r.bottom()));
        (col(x0)..=col(x1), row(y0)..=row(y1))
    }

    /// Add `r` to the boxes labels keep off.
    fn push(&mut self, r: Rect) {
        let i = u32::try_from(self.taken.len()).unwrap_or(u32::MAX);
        let (cols, rows) = self.cells(&r);
        for y in rows {
            for x in cols.clone() {
                self.grid[y * GRID + x].push(i);
            }
        }
        self.taken.push(r);
    }

    /// The boxes that may overlap `r`, each once.
    fn near(&self, r: &Rect) -> Vec<u32> {
        let (cols, rows) = self.cells(r);
        let mut ids: Vec<u32> = Vec::new();
        for y in rows {
            for x in cols.clone() {
                ids.extend(&self.grid[y * GRID + x]);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Keep labels off `r` (a marker, a title, a legend).
    pub fn reserve(&mut self, r: Rect) {
        self.push(r);
    }

    /// Everything placed or reserved so far.
    pub fn taken(&self) -> &[Rect] {
        &self.taken
    }

    /// True when `r` lies inside the bounds and overlaps nothing reserved.
    pub fn free(&self, r: &Rect) -> bool {
        self.bounds.contains_rect(r)
            && !self
                .near(r)
                .iter()
                .any(|&i| self.taken[i as usize].intersects(r))
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
                let mut cost: f32 = self
                    .near(&r)
                    .iter()
                    .map(|&i| self.taken[i as usize].overlap_area(&r))
                    .sum();
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
        self.push(r);
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

    /// The grid finds every box a spot overlaps, also boxes outside the bounds, and many labels
    /// are placed quickly.
    #[test]
    fn the_grid_sees_every_neighbour() {
        let mut p = LabelPlacer::new(Rect::new(0.0, 0.0, 1000.0, 600.0));
        p.reserve(Rect::new(-50.0, -50.0, 1100.0, 20.0));
        assert!(!p.free(&Rect::new(500.0, -40.0, 10.0, 10.0)));
        assert!(!p.free(&Rect::new(990.0, -45.0, 5.0, 5.0)));
        let t = std::time::Instant::now();
        let placed: Vec<Rect> = (0..5000)
            .map(|i| {
                let (x, y) = ((i * 37 % 1000) as f32, (i * 53 % 600) as f32);
                p.place(x, y, 30.0, 10.0, 3.0)
            })
            .collect();
        assert!(t.elapsed().as_secs_f32() < 5.0, "{:?}", t.elapsed());
        // the first labels, placed while there was room, overlap nothing
        for (i, a) in placed[..40].iter().enumerate() {
            for b in &placed[..i] {
                assert!(!a.intersects(b), "{a:?} {b:?}");
            }
        }
    }
}
