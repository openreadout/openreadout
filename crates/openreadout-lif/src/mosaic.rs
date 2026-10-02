//! Tile-scan placement: where each tile of a mosaic (DimID 10) lands in the stitched plane.
//! See `docs/formats/lif.md` § Tile scans.
//!
//! Rule (derived from `aics-tiled` vs LAS X's own merge `aics-merged-tiles`, which it reproduces
//! bit for bit): take each tile's stage position (`PosX`, `PosY`, metres), negate X when `FlipX`
//! and Y when `FlipY`, swap them when `SwapXY`, subtract the minimum over all tiles, divide by the
//! pixel step (`Length / (NumberOfElements - 1)` of the X/Y dimensions) and round half up. Tiles
//! are pasted in file order, so where tiles overlap the later tile wins.

use crate::xml::TileScan;

/// How the tile offsets were derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementMethod {
    /// From `TileScanInfo` stage positions (the normal case).
    StagePosition,
    /// From `FieldX`/`FieldY` grid indices, tiles abutting (positions missing or unusable).
    FieldGrid,
    /// No usable tile metadata: tiles side by side in one row, in file order.
    Row,
}

impl PlacementMethod {
    /// Lowercase name used in JSON.
    pub fn name(self) -> &'static str {
        match self {
            PlacementMethod::StagePosition => "stage_position",
            PlacementMethod::FieldGrid => "field_grid",
            PlacementMethod::Row => "row",
        }
    }
}

/// Top-left pixel of one tile in the stitched plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TilePlacement {
    pub index: u32,
    pub x_px: u32,
    pub y_px: u32,
}

/// The stitched plane's size and every tile's offset, in tile (file) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MosaicLayout {
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub method: PlacementMethod,
    pub placements: Vec<TilePlacement>,
}

impl MosaicLayout {
    /// True when some pixel of tile `i` is covered by a later tile (so it is not fully visible).
    pub fn overlapped_by_later(&self, i: usize) -> bool {
        let Some(a) = self.placements.get(i) else {
            return false;
        };
        self.placements[i + 1..].iter().any(|b| {
            let (ax, ay, bx, by) = (
                u64::from(a.x_px),
                u64::from(a.y_px),
                u64::from(b.x_px),
                u64::from(b.y_px),
            );
            let (w, h) = (u64::from(self.tile_width), u64::from(self.tile_height));
            ax < bx + w && bx < ax + w && ay < by + h && by < ay + h
        })
    }
}

/// Largest stitched plane we will build, in pixels (guards against nonsense stage positions).
const MAX_CANVAS_PIXELS: u64 = 1 << 34;

/// Round half up for the non-negative offsets we produce.
fn round_half_up(v: f64) -> f64 {
    (v + 0.5).floor()
}

fn from_offsets(
    tile_width: u32,
    tile_height: u32,
    method: PlacementMethod,
    offsets: &[(u64, u64)],
) -> Option<MosaicLayout> {
    let max_x = offsets.iter().map(|o| o.0).max()?;
    let max_y = offsets.iter().map(|o| o.1).max()?;
    let width = u32::try_from(max_x.checked_add(u64::from(tile_width))?).ok()?;
    let height = u32::try_from(max_y.checked_add(u64::from(tile_height))?).ok()?;
    if u64::from(width) * u64::from(height) > MAX_CANVAS_PIXELS {
        return None;
    }
    // Reject layouts whose canvas dwarfs the tiles (stage positions that are not a mosaic).
    let tiles_area = offsets.len() as u64 * u64::from(tile_width) * u64::from(tile_height);
    if u64::from(width) * u64::from(height) > tiles_area.saturating_mul(64).max(1) {
        return None;
    }
    let placements = offsets
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            Some(TilePlacement {
                index: u32::try_from(i).ok()?,
                x_px: u32::try_from(x).ok()?,
                y_px: u32::try_from(y).ok()?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(MosaicLayout {
        width,
        height,
        tile_width,
        tile_height,
        method,
        placements,
    })
}

fn stage_offsets(scan: &TileScan, step_x_m: f64, step_y_m: f64) -> Option<Vec<(u64, u64)>> {
    if !(step_x_m.is_finite() && step_y_m.is_finite() && step_x_m > 0.0 && step_y_m > 0.0) {
        return None;
    }
    let pts: Vec<(f64, f64)> = scan
        .tiles
        .iter()
        .map(|t| {
            let sx = if scan.flip_x { -t.pos_x } else { t.pos_x };
            let sy = if scan.flip_y { -t.pos_y } else { t.pos_y };
            if scan.swap_xy { (sy, sx) } else { (sx, sy) }
        })
        .collect();
    if pts.iter().any(|(x, y)| !x.is_finite() || !y.is_finite()) {
        return None;
    }
    let min_x = pts.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let min_y = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let offs: Vec<(u64, u64)> = pts
        .iter()
        .map(|&(x, y)| {
            let ox = round_half_up((x - min_x) / step_x_m);
            let oy = round_half_up((y - min_y) / step_y_m);
            ((0.0..4.0e9).contains(&ox) && (0.0..4.0e9).contains(&oy))
                .then_some((ox as u64, oy as u64))
        })
        .collect::<Option<Vec<_>>>()?;
    // All tiles at one spot while their grid indices differ: positions were not recorded.
    let distinct_fields = scan
        .tiles
        .iter()
        .any(|t| t.field_x != scan.tiles[0].field_x || t.field_y != scan.tiles[0].field_y);
    if distinct_fields && offs.iter().all(|&o| o == offs[0]) {
        return None;
    }
    Some(offs)
}

fn field_offsets(scan: &TileScan, tile_width: u32, tile_height: u32) -> Option<Vec<(u64, u64)>> {
    let min_fx = scan.tiles.iter().map(|t| t.field_x).min()?;
    let min_fy = scan.tiles.iter().map(|t| t.field_y).min()?;
    let max_fx = scan.tiles.iter().map(|t| t.field_x).max()?;
    let max_fy = scan.tiles.iter().map(|t| t.field_y).max()?;
    scan.tiles
        .iter()
        .map(|t| {
            // Same orientation rule as the stage positions: flip first, then swap.
            let gx = if scan.flip_x {
                max_fx.checked_sub(t.field_x)?
            } else {
                t.field_x.checked_sub(min_fx)?
            };
            let gy = if scan.flip_y {
                max_fy.checked_sub(t.field_y)?
            } else {
                t.field_y.checked_sub(min_fy)?
            };
            let (gx, gy) = if scan.swap_xy { (gy, gx) } else { (gx, gy) };
            let gx = u64::try_from(gx).ok()?;
            let gy = u64::try_from(gy).ok()?;
            Some((
                gx.checked_mul(u64::from(tile_width))?,
                gy.checked_mul(u64::from(tile_height))?,
            ))
        })
        .collect()
}

/// Compute the placement of `tile_count` tiles of `tile_width` × `tile_height` pixels.
/// `step_x_m`/`step_y_m` are the pixel sizes in metres. Falls back from stage positions to the
/// field grid to a single row when the metadata is missing, inconsistent, or absurd.
pub fn layout(
    tile_count: u32,
    tile_width: u32,
    tile_height: u32,
    step_x_m: Option<f64>,
    step_y_m: Option<f64>,
    scan: Option<&TileScan>,
) -> MosaicLayout {
    let n = tile_count.max(1) as usize;
    if let Some(scan) = scan.filter(|s| s.tiles.len() == n) {
        if let (Some(sx), Some(sy)) = (step_x_m, step_y_m)
            && let Some(offs) = stage_offsets(scan, sx, sy)
            && let Some(l) = from_offsets(
                tile_width,
                tile_height,
                PlacementMethod::StagePosition,
                &offs,
            )
        {
            return l;
        }
        if let Some(offs) = field_offsets(scan, tile_width, tile_height)
            && let Some(l) =
                from_offsets(tile_width, tile_height, PlacementMethod::FieldGrid, &offs)
        {
            return l;
        }
    }
    let offs: Vec<(u64, u64)> = (0..n as u64)
        .map(|i| (i * u64::from(tile_width), 0))
        .collect();
    from_offsets(tile_width, tile_height, PlacementMethod::Row, &offs).unwrap_or(MosaicLayout {
        width: tile_width,
        height: tile_height,
        tile_width,
        tile_height,
        method: PlacementMethod::Row,
        placements: vec![TilePlacement {
            index: 0,
            x_px: 0,
            y_px: 0,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::TilePosition;

    fn tile(fx: i64, fy: i64, x: f64, y: f64) -> TilePosition {
        TilePosition {
            field_x: fx,
            field_y: fy,
            pos_x: x,
            pos_y: y,
            pos_z: 0.0,
        }
    }

    #[test]
    fn stage_positions_no_flags() {
        // 2x2 tiles of 10 px, 1 µm pixels, stage step 8 µm → 2 px overlap.
        let scan = TileScan {
            tiles: vec![
                tile(0, 0, 1e-3, 2e-3),
                tile(1, 0, 1e-3 + 8e-6, 2e-3),
                tile(0, 1, 1e-3, 2e-3 + 8e-6),
                tile(1, 1, 1e-3 + 8e-6, 2e-3 + 8e-6),
            ],
            ..TileScan::default()
        };
        let l = layout(4, 10, 10, Some(1e-6), Some(1e-6), Some(&scan));
        assert_eq!(l.method, PlacementMethod::StagePosition);
        assert_eq!((l.width, l.height), (18, 18));
        let xy: Vec<(u32, u32)> = l.placements.iter().map(|p| (p.x_px, p.y_px)).collect();
        assert_eq!(xy, [(0, 0), (8, 0), (0, 8), (8, 8)]);
        assert!(l.overlapped_by_later(0));
        assert!(!l.overlapped_by_later(3));
    }

    #[test]
    fn flips_and_swap_match_lasx_merge() {
        // aics-tiled: FlipX=FlipY=SwapXY=1; tile (fx, fy) lands at column max-fy, row max-fx.
        let step = 1.025_133e-4 / 511.0;
        let mut tiles = Vec::new();
        for fx in 0..3 {
            for fy in 0..2 {
                tiles.push(tile(
                    fx,
                    fy,
                    0.075_994_432_873_991_9 + f64::from(fx as i32) * 1.025_133e-4,
                    0.058_041_803_044_632_7 + f64::from(fy as i32) * 1.025_133e-4,
                ));
            }
        }
        let scan = TileScan {
            flip_x: true,
            flip_y: true,
            swap_xy: true,
            tiles,
        };
        let l = layout(6, 512, 512, Some(step), Some(step), Some(&scan));
        assert_eq!(l.method, PlacementMethod::StagePosition);
        assert_eq!((l.width, l.height), (511 + 512, 2 * 511 + 512));
        // tile 0 = (fx 0, fy 0) → column 1, row 2
        assert_eq!((l.placements[0].x_px, l.placements[0].y_px), (511, 1022));
        // last tile = (fx 2, fy 1) → column 0, row 0
        assert_eq!((l.placements[5].x_px, l.placements[5].y_px), (0, 0));
    }

    #[test]
    fn single_flip_without_swap() {
        let scan = TileScan {
            flip_x: true,
            tiles: vec![tile(0, 0, 0.0, 0.0), tile(1, 0, 5e-6, 0.0)],
            ..TileScan::default()
        };
        let l = layout(2, 10, 10, Some(1e-6), Some(1e-6), Some(&scan));
        assert_eq!((l.placements[0].x_px, l.placements[1].x_px), (5, 0));
        assert_eq!((l.width, l.height), (15, 10));
    }

    #[test]
    fn rounding_is_half_up() {
        let scan = TileScan {
            tiles: vec![tile(0, 0, 0.0, 0.0), tile(1, 0, 2.5e-6, 0.0)],
            ..TileScan::default()
        };
        let l = layout(2, 4, 4, Some(1e-6), Some(1e-6), Some(&scan));
        assert_eq!(l.placements[1].x_px, 3);
    }

    #[test]
    fn falls_back_to_field_grid_then_row() {
        // identical positions but distinct fields → grid
        let scan = TileScan {
            tiles: vec![
                tile(0, 0, 0.0, 0.0),
                tile(1, 0, 0.0, 0.0),
                tile(0, 1, 0.0, 0.0),
            ],
            ..TileScan::default()
        };
        let l = layout(3, 4, 6, Some(1e-6), Some(1e-6), Some(&scan));
        assert_eq!(l.method, PlacementMethod::FieldGrid);
        assert_eq!((l.width, l.height), (8, 12));
        // count mismatch → row
        let l = layout(5, 4, 6, Some(1e-6), Some(1e-6), Some(&scan));
        assert_eq!(l.method, PlacementMethod::Row);
        assert_eq!((l.width, l.height), (20, 6));
        // no pixel size → grid
        let scan = TileScan {
            tiles: vec![tile(0, 0, 0.0, 0.0), tile(1, 0, 1e-3, 0.0)],
            ..TileScan::default()
        };
        assert_eq!(
            layout(2, 4, 4, None, None, Some(&scan)).method,
            PlacementMethod::FieldGrid
        );
        // absurd positions (1 m apart for 4 px tiles) → grid, not a gigapixel canvas
        let scan = TileScan {
            tiles: vec![tile(0, 0, 0.0, 0.0), tile(1, 0, 1.0, 0.0)],
            ..TileScan::default()
        };
        assert_eq!(
            layout(2, 4, 4, Some(1e-6), Some(1e-6), Some(&scan)).method,
            PlacementMethod::FieldGrid
        );
        // non-finite and negative-field input never panics
        let scan = TileScan {
            tiles: vec![tile(-3, 0, f64::NAN, 0.0), tile(i64::MAX, 0, 0.0, 0.0)],
            ..TileScan::default()
        };
        let l = layout(2, 4, 4, Some(1e-6), Some(1e-6), Some(&scan));
        assert_eq!(l.method, PlacementMethod::Row);
    }
}
