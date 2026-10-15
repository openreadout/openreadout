//! Rectangles and layout helpers.

/// An axis-aligned rectangle in figure pixels (y down).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    /// From two corners, in any order.
    pub fn from_corners(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::new(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs())
    }

    /// Right edge.
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    /// Bottom edge.
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Centre.
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    /// Shrunk by `d` on every side (grown when negative).
    pub fn inset(&self, d: f32) -> Rect {
        self.inset_sides(d, d, d, d)
    }

    /// Shrunk by different amounts: left, top, right, bottom.
    pub fn inset_sides(&self, l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect::new(
            self.x + l,
            self.y + t,
            (self.w - l - r).max(0.0),
            (self.h - t - b).max(0.0),
        )
    }

    /// True when the two rectangles overlap (touching edges do not count).
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    /// Area shared with `o`.
    pub fn overlap_area(&self, o: &Rect) -> f32 {
        let w = self.right().min(o.right()) - self.x.max(o.x);
        let h = self.bottom().min(o.bottom()) - self.y.max(o.y);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    }

    /// True when `o` lies entirely inside.
    pub fn contains_rect(&self, o: &Rect) -> bool {
        o.x >= self.x - 1e-3
            && o.y >= self.y - 1e-3
            && o.right() <= self.right() + 1e-3
            && o.bottom() <= self.bottom() + 1e-3
    }

    /// True when the point lies inside (edges included).
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }

    /// The smallest rectangle holding both.
    pub fn union(&self, o: &Rect) -> Rect {
        Rect::from_corners(
            self.x.min(o.x),
            self.y.min(o.y),
            self.right().max(o.right()),
            self.bottom().max(o.bottom()),
        )
    }

    /// Moved by `(dx, dy)`.
    pub fn translate(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// Snapped outward to whole pixels.
    pub fn snap(&self) -> Rect {
        Rect::from_corners(
            self.x.floor(),
            self.y.floor(),
            self.right().ceil(),
            self.bottom().ceil(),
        )
    }
}

/// Cells of a `rows` × `cols` grid filling `area`, row by row, `gap_x` / `gap_y` pixels apart:
/// the frames of small multiples.
pub fn grid(area: Rect, rows: usize, cols: usize, gap_x: f32, gap_y: f32) -> Vec<Rect> {
    if rows == 0 || cols == 0 {
        return Vec::new();
    }
    let cw = ((area.w - gap_x * (cols - 1) as f32) / cols as f32).max(0.0);
    let ch = ((area.h - gap_y * (rows - 1) as f32) / rows as f32).max(0.0);
    let mut out = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            out.push(Rect::new(
                area.x + c as f32 * (cw + gap_x),
                area.y + r as f32 * (ch + gap_y),
                cw,
                ch,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rects_and_grids() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(5.0, 5.0, 10.0, 10.0);
        assert!(a.intersects(&b));
        assert!((a.overlap_area(&b) - 25.0).abs() < 1e-6);
        assert!(!a.intersects(&Rect::new(10.0, 0.0, 5.0, 5.0)));
        assert_eq!(a.union(&b), Rect::new(0.0, 0.0, 15.0, 15.0));
        let g = grid(Rect::new(0.0, 0.0, 110.0, 50.0), 2, 3, 10.0, 10.0);
        assert_eq!(g.len(), 6);
        assert_eq!(g[4], Rect::new(40.0, 30.0, 30.0, 20.0));
    }
}
