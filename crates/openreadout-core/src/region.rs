//! Region reads and resolution levels.
//!
//! A [`Region`] is a rectangle of one plane in the pixel coordinates of one resolution level
//! (level 0 = full resolution). [`crate::Dataset::read_region`] decodes only that rectangle:
//! readers whose pixels are stored in tiles, subblocks or chunks (CZI, tiled TIFF and
//! whole-slide TIFF flavours, VSI/ETS, Imaris, OME-Zarr) touch only the tiles the region
//! overlaps, so a 512 × 512 window of a 40 GB slide costs a few tiles and a few megabytes.
//! Every other reader gets the default: read the whole plane at that level, then crop.
//!
//! [`ResolutionLevel`] is the geometry of one level (`info` → `images[].resolution_levels`):
//! its size, its downsampling factor relative to level 0 and the natural tile a region read
//! should align to. [`TileCache`] is a small byte-bounded cache of decoded tiles readers use
//! so that neighbouring region reads (dask chunks, export blocks) do not decode a tile twice.

// x, y, w, h are the natural names of a rectangle.
#![allow(clippy::many_single_char_names)]

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::ImageInfo;
use crate::pixel::{Plane, plane_bytes_checked};

/// A rectangle of a plane, in the pixel coordinates of the resolution level it is read at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
pub struct Region {
    /// Left edge (column of the first pixel).
    pub x: u32,
    /// Top edge (row of the first pixel).
    pub y: u32,
    /// Width in pixels (at least 1).
    pub width: u32,
    /// Height in pixels (at least 1).
    pub height: u32,
}

impl Region {
    /// A `width` × `height` rectangle whose top-left pixel is `(x, y)`.
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    /// The whole of a `width` × `height` plane.
    pub fn full(width: u32, height: u32) -> Self {
        Region::new(0, 0, width, height)
    }

    /// Parse `X,Y,W,H` (the CLI's `--region`), all non-negative integers, W and H ≥ 1.
    pub fn parse(s: &str) -> Result<Region> {
        let bad = || {
            Error::Usage(format!(
                "--region '{s}': expected X,Y,WIDTH,HEIGHT in pixels (e.g. 1024,2048,512,512), \
                 in the coordinates of the level read (--level, default 0)"
            ))
        };
        let parts: Vec<u32> = s
            .split(',')
            .map(|p| p.trim().parse::<u32>())
            .collect::<std::result::Result<_, _>>()
            .map_err(|_| bad())?;
        let [x, y, w, h] = parts[..] else {
            return Err(bad());
        };
        if w == 0 || h == 0 {
            return Err(Error::Usage(format!(
                "--region '{s}': width and height must be at least 1"
            )));
        }
        Ok(Region::new(x, y, w, h))
    }

    /// One past the last column.
    pub fn right(&self) -> u64 {
        u64::from(self.x) + u64::from(self.width)
    }

    /// One past the last row.
    pub fn bottom(&self) -> u64 {
        u64::from(self.y) + u64::from(self.height)
    }

    /// True when the region is the whole `width` × `height` plane.
    pub fn is_full(&self, width: u32, height: u32) -> bool {
        *self == Region::full(width, height)
    }

    /// A usage error (exit 2) unless the region is non-empty and lies inside a `width` ×
    /// `height` plane. `what` names the plane in the message (e.g. `image 0 level 2`).
    pub fn check_within(&self, width: u32, height: u32, what: &str) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err(Error::Usage(format!(
                "region {self} is empty; width and height must be at least 1"
            )));
        }
        if self.right() > u64::from(width) || self.bottom() > u64::from(height) {
            return Err(Error::Usage(format!(
                "region {self} extends past {what}, which is {width} x {height} pixels; \
                 regions are in the pixel coordinates of the level read \
                 (`info` → images[].resolution_levels lists each level's size)"
            )));
        }
        Ok(())
    }

    /// The overlap with the rectangle at `(x, y)` of `w` × `h` (signed, may start left of or
    /// above 0), as `(x0, y0, x1, y1)` in the same coordinates; `None` when they do not meet.
    pub fn overlap(&self, x: i64, y: i64, w: u64, h: u64) -> Option<(i64, i64, i64, i64)> {
        let wi = i64::try_from(w).ok()?;
        let hi = i64::try_from(h).ok()?;
        let x0 = x.max(i64::from(self.x));
        let y0 = y.max(i64::from(self.y));
        let x1 = x.saturating_add(wi).min(self.right() as i64);
        let y1 = y.saturating_add(hi).min(self.bottom() as i64);
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
    }

    /// Map a region of a `from` (width, height) level onto a `to` level of the same image: the
    /// smallest `to` rectangle covering it (start rounded down, end rounded up), clamped to
    /// the `to` plane and never empty.
    pub fn rescale(&self, from: (u32, u32), to: (u32, u32)) -> Region {
        let map = |v: u64, n_from: u32, n_to: u32, up: bool| -> u64 {
            if n_from == 0 {
                return 0;
            }
            let num = u128::from(v) * u128::from(n_to);
            let d = u128::from(n_from);
            let r = if up { num.div_ceil(d) } else { num / d };
            u64::try_from(r).unwrap_or(u64::MAX).min(u64::from(n_to))
        };
        let x0 = map(u64::from(self.x), from.0, to.0, false);
        let y0 = map(u64::from(self.y), from.1, to.1, false);
        let x1 = map(self.right(), from.0, to.0, true).max(x0 + 1);
        let y1 = map(self.bottom(), from.1, to.1, true).max(y0 + 1);
        let clamp = |v: u64, n: u32| v.min(u64::from(n)) as u32;
        let (x0, y0) = (
            clamp(x0, to.0.saturating_sub(1)),
            clamp(y0, to.1.saturating_sub(1)),
        );
        let (x1, y1) = (clamp(x1, to.0), clamp(y1, to.1));
        Region::new(
            x0,
            y0,
            x1.saturating_sub(x0).max(1),
            y1.saturating_sub(y0).max(1),
        )
    }
}

impl std::fmt::Display for Region {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{},{},{},{}", self.x, self.y, self.width, self.height)
    }
}

/// Allocate the buffer of a region plane (zeros), refusing regions above
/// [`crate::pixel::MAX_PLANE_BYTES`] with the usual "plane too large" error.
pub fn region_buffer(
    format: &'static str,
    region: Region,
    bytes_per_pixel: usize,
) -> Result<Vec<u8>> {
    let n = plane_bytes_checked(format, region.width, region.height, bytes_per_pixel)?;
    Ok(vec![0u8; n])
}

/// A decoded tile placed in a plane: `width` × `height` pixels, rows of `row_bytes` bytes
/// (≥ `width` × bytes per pixel), its top-left pixel at `(x, y)` in the plane's coordinates
/// (it may start left of or above the plane).
#[derive(Debug, Clone, Copy)]
pub struct PlacedTile<'a> {
    /// The tile's samples, row by row.
    pub data: &'a [u8],
    /// Bytes per tile row.
    pub row_bytes: usize,
    /// Column of the tile's first pixel in the plane.
    pub x: i64,
    /// Row of the tile's first pixel in the plane.
    pub y: i64,
    /// Tile width in pixels.
    pub width: u32,
    /// Tile height in pixels.
    pub height: u32,
}

/// Copy the part of `tile` that overlaps `region` into `out` (the region's buffer, rows of
/// `region.width * bpp` bytes). Pixels outside the tile or the region are left untouched; short
/// tile data is copied as far as it goes.
pub fn paste_into_region(out: &mut [u8], region: Region, tile: &PlacedTile<'_>, bpp: usize) {
    let (tx, ty) = (tile.x, tile.y);
    let Some((x0, y0, x1, y1)) =
        region.overlap(tx, ty, u64::from(tile.width), u64::from(tile.height))
    else {
        return;
    };
    let out_row = region.width as usize * bpp;
    let len = (x1 - x0) as usize * bpp;
    let src_x = (x0 - tx) as usize * bpp;
    let dst_x = (x0 - i64::from(region.x)) as usize * bpp;
    for y in y0..y1 {
        let s = (y - ty) as usize * tile.row_bytes + src_x;
        let d = (y - i64::from(region.y)) as usize * out_row + dst_x;
        if let (Some(src), Some(dst)) = (tile.data.get(s..s + len), out.get_mut(d..d + len)) {
            dst.copy_from_slice(src);
        }
    }
}

/// Crop `region` out of a whole plane (the generic region read). A usage error when the
/// region does not lie inside the plane.
pub fn crop(plane: Plane, region: Region, what: &str) -> Result<Plane> {
    region.check_within(plane.width, plane.height, what)?;
    if region.is_full(plane.width, plane.height) {
        return Ok(plane);
    }
    let bpp = plane.samples_per_pixel.max(1) as usize * plane.pixel_type.bytes_per_sample();
    let row = plane.row_bytes();
    if plane.data.len() < row * plane.height as usize {
        return Err(Error::Other(format!(
            "plane holds {} bytes, expected {}",
            plane.data.len(),
            row * plane.height as usize
        )));
    }
    let out_row = region.width as usize * bpp;
    let mut data = Vec::with_capacity(out_row * region.height as usize);
    for y in region.y..region.y + region.height {
        let s = y as usize * row + region.x as usize * bpp;
        data.extend_from_slice(&plane.data[s..s + out_row]);
    }
    Ok(Plane {
        width: region.width,
        height: region.height,
        pixel_type: plane.pixel_type,
        samples_per_pixel: plane.samples_per_pixel,
        data,
    })
}

/// Geometry of one resolution level of an image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ResolutionLevel {
    /// Level index: 0 is full resolution, larger is more downsampled (`--level`).
    pub level: u32,
    /// Width in pixels.
    pub size_x: u32,
    /// Height in pixels.
    pub size_y: u32,
    /// Level-0 width over this level's width (1 for level 0; 2, 4, ... for a halving pyramid).
    pub downsample_x: f64,
    /// Level-0 height over this level's height.
    pub downsample_y: f64,
    /// Width of the tiles (subblocks, chunks) the level is stored in, when it is tiled: region
    /// reads aligned to this grid decode each tile once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile_width: Option<u32>,
    /// Height of the stored tiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tile_height: Option<u32>,
    /// Number of z planes at this level, when it differs from the image's `size_z` (Imaris
    /// downsamples z as well).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_z: Option<u32>,
}

impl ResolutionLevel {
    /// Level `level` of `size_x` × `size_y` pixels in an image whose level 0 is
    /// `base_x` × `base_y`; downsampling factors are the size ratios, snapped to the power of
    /// two `2^k` when the level is level 0 halved `k` times (rounding either way: within one
    /// pixel of `base / 2^k`).
    pub fn new(level: u32, size_x: u32, size_y: u32, base_x: u32, base_y: u32) -> Self {
        let f = |b: u32, s: u32| {
            if s == 0 {
                return 1.0;
            }
            let r = f64::from(b) / f64::from(s);
            let p = r.log2().round().clamp(0.0, 62.0).exp2();
            if (f64::from(b) / p - f64::from(s)).abs() <= 1.0 {
                return p;
            }
            (r * 1e6).round() / 1e6
        };
        ResolutionLevel {
            level,
            size_x,
            size_y,
            downsample_x: f(base_x, size_x),
            downsample_y: f(base_y, size_y),
            tile_width: None,
            tile_height: None,
            size_z: None,
        }
    }

    /// With the stored tile size (ignored when it is 0 or not smaller than the level).
    #[must_use]
    pub fn with_tile(mut self, width: u32, height: u32) -> Self {
        if width > 0 && height > 0 && (width < self.size_x || height < self.size_y) {
            self.tile_width = Some(width.min(self.size_x));
            self.tile_height = Some(height.min(self.size_y));
        }
        self
    }
}

/// The resolution levels of an image, level 0 first: the reader's `resolution_levels` when it
/// filled them, otherwise level 0 alone (from `size_x`/`size_y`).
pub fn levels_of(im: &ImageInfo) -> Vec<ResolutionLevel> {
    if im.resolution_levels.is_empty() {
        vec![ResolutionLevel::new(
            0, im.size_x, im.size_y, im.size_x, im.size_y,
        )]
    } else {
        im.resolution_levels.clone()
    }
}

/// `(width, height)` of level `level` of `im`, or a usage error naming the levels there are.
pub fn level_size(im: &ImageInfo, level: u32) -> Result<(u32, u32)> {
    if level == 0 {
        return Ok((im.size_x, im.size_y));
    }
    let levels = levels_of(im);
    levels
        .iter()
        .find(|l| l.level == level)
        .map(|l| (l.size_x, l.size_y))
        .ok_or_else(|| {
            Error::Usage(format!(
                "pyramid level {level} out of range for image {} ({} level{}: 0..{})",
                im.index,
                im.pyramid_levels.max(1),
                if im.pyramid_levels > 1 { "s" } else { "" },
                im.pyramid_levels.max(1)
            ))
        })
}

/// A cache of decoded tiles, bounded by the bytes it holds; the least recently used tile goes
/// first. Readers keep one per open file so that overlapping or neighbouring region reads
/// decode each tile once.
#[derive(Debug)]
pub struct TileCache<K, V = Vec<u8>> {
    budget: usize,
    held: usize,
    tick: u64,
    map: HashMap<K, (Arc<V>, usize, u64)>,
}

/// Default budget of a [`TileCache`]: 64 MiB of decoded samples.
pub const DEFAULT_TILE_CACHE_BYTES: usize = 64 << 20;

impl<K: Hash + Eq + Clone, V> Default for TileCache<K, V> {
    fn default() -> Self {
        TileCache::new(DEFAULT_TILE_CACHE_BYTES)
    }
}

impl<K: Hash + Eq + Clone, V> TileCache<K, V> {
    /// An empty cache that holds at most `budget` bytes (0 disables it).
    pub fn new(budget: usize) -> Self {
        TileCache {
            budget,
            held: 0,
            tick: 0,
            map: HashMap::new(),
        }
    }

    /// The tile stored under `key`, if cached (and mark it recently used).
    pub fn get(&mut self, key: &K) -> Option<Arc<V>> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(key).map(|(v, _, used)| {
            *used = t;
            v.clone()
        })
    }

    /// Store a decoded tile of `bytes` bytes; evicts the least recently used tiles to stay
    /// within the budget. Tiles larger than a quarter of the budget are not kept.
    pub fn put(&mut self, key: K, tile: Arc<V>, bytes: usize) {
        if self.budget == 0 || bytes > self.budget / 4 {
            return;
        }
        self.tick += 1;
        if let Some((_, old, _)) = self.map.insert(key, (tile, bytes, self.tick)) {
            self.held -= old;
        }
        self.held += bytes;
        while self.held > self.budget {
            let Some(k) = self
                .map
                .iter()
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some((_, n, _)) = self.map.remove(&k) {
                self.held -= n;
            }
        }
    }

    /// Bytes currently held.
    pub fn bytes(&self) -> usize {
        self.held
    }

    /// Drop every tile.
    pub fn clear(&mut self) {
        self.map.clear();
        self.held = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel::PixelType;

    fn ramp(w: u32, h: u32) -> Plane {
        Plane {
            width: w,
            height: h,
            pixel_type: PixelType::Uint16,
            samples_per_pixel: 1,
            data: (0..w * h).flat_map(|v| (v as u16).to_le_bytes()).collect(),
        }
    }

    #[test]
    fn parse_and_check() {
        let r = Region::parse("10, 20,30,40").unwrap();
        assert_eq!(r, Region::new(10, 20, 30, 40));
        assert_eq!(r.to_string(), "10,20,30,40");
        for bad in ["1,2,3", "1,2,3,4,5", "-1,0,1,1", "a,b,c,d", "0,0,0,5"] {
            assert_eq!(Region::parse(bad).unwrap_err().exit_code(), 2, "{bad}");
        }
        assert!(r.check_within(40, 60, "p").is_ok());
        let e = r.check_within(39, 60, "image 0 level 1").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.to_string().contains("39 x 60"), "{e}");
    }

    #[test]
    fn crop_matches_slicing() {
        let p = ramp(7, 5);
        let c = crop(p.clone(), Region::new(2, 1, 3, 2), "p").unwrap();
        let v: Vec<u16> = c
            .data
            .chunks(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(v, [9, 10, 11, 16, 17, 18]);
        assert_eq!(
            crop(p.clone(), Region::full(7, 5), "p").unwrap().data,
            p.data
        );
        assert!(crop(p, Region::new(5, 0, 3, 1), "p").is_err());
    }

    #[test]
    fn paste_clips_to_tile_and_region() {
        // A 4x4 tile at (-2, 1) pasted into region (0, 0, 3, 3) of a plane.
        let tile: Vec<u8> = (0..16).collect();
        let r = Region::new(0, 0, 3, 3);
        let mut out = vec![0xFFu8; 9];
        paste_into_region(
            &mut out,
            r,
            &PlacedTile {
                data: &tile,
                row_bytes: 4,
                x: -2,
                y: 1,
                width: 4,
                height: 4,
            },
            1,
        );
        assert_eq!(out, [0xFF, 0xFF, 0xFF, 2, 3, 0xFF, 6, 7, 0xFF]);
        // No overlap: untouched.
        let mut out2 = vec![1u8; 9];
        paste_into_region(
            &mut out2,
            r,
            &PlacedTile {
                data: &tile,
                row_bytes: 4,
                x: 3,
                y: 0,
                width: 4,
                height: 4,
            },
            1,
        );
        assert_eq!(out2, [1u8; 9]);
    }

    #[test]
    fn rescale_covers_the_region() {
        let r = Region::new(101, 51, 10, 10);
        let s = r.rescale((1000, 500), (250, 125));
        assert_eq!(s, Region::new(25, 12, 3, 4));
        // Always at least one pixel and inside the target.
        let t = Region::new(999, 499, 1, 1).rescale((1000, 500), (3, 2));
        assert_eq!(t, Region::new(2, 1, 1, 1));
    }

    #[test]
    #[allow(clippy::float_cmp)] // the factors are exact powers of two (or exact ratios)
    fn halving_pyramids_have_power_of_two_factors() {
        let l = ResolutionLevel::new(2, 16793, 14581, 67170, 58321);
        assert_eq!((l.downsample_x, l.downsample_y), (4.0, 4.0));
        assert_eq!(
            ResolutionLevel::new(8, 263, 228, 67170, 58321).downsample_x,
            256.0
        );
        // A 3x pyramid keeps its own ratio.
        assert_eq!(ResolutionLevel::new(1, 100, 67, 300, 200).downsample_x, 3.0);
        assert!((ResolutionLevel::new(1, 97, 67, 300, 200).downsample_x - 3.092_784).abs() < 1e-6);
    }

    #[test]
    fn cache_evicts_least_recently_used() {
        let mut c: TileCache<u32> = TileCache::new(40);
        c.put(1, Arc::new(vec![0; 10]), 10);
        c.put(2, Arc::new(vec![0; 10]), 10);
        c.put(3, Arc::new(vec![0; 10]), 10);
        assert!(c.get(&1).is_some());
        c.put(4, Arc::new(vec![0; 10]), 10);
        c.put(5, Arc::new(vec![0; 10]), 10);
        assert!(c.bytes() <= 40);
        assert!(c.get(&2).is_none(), "2 was least recently used");
        assert!(c.get(&1).is_some());
        c.put(6, Arc::new(vec![0; 11]), 11); // above a quarter of the budget: not kept
        assert!(c.get(&6).is_none());
    }
}
