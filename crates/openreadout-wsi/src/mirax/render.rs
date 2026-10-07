//! Placing a MIRAX level's stored images: every image (or, at coarse levels, every camera
//! position inside an image, a "subtile") goes to its camera's recorded position scaled to the
//! level, generally a fractional pixel. Composition as derived in `docs/formats/mirax.md`
//! § Placement: bilinear resampling at the fractional offset with transparent outside the
//! source, sources whose own rectangle starts at a fractional pixel resampled once into an
//! integer-sized surface first, and a "saturate" rule painting in reverse raster order of the
//! grid, so the image later in raster order wins where camera photos overlap. At integer
//! offsets (every level-0 image) this copies the stored samples unchanged.

use std::collections::HashMap;
use std::sync::Arc;

use openreadout_core::region::Region;
use openreadout_core::{Error, Result};

use super::slide::{FORMAT_ID, MiraxSlide};
use crate::image::RgbImage;

/// Side of the buckets of the spatial index, in level pixels.
const BUCKET: f64 = 256.0;

/// One placed source rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Subtile {
    /// Grid index of the stored image it comes from.
    pub image: u32,
    /// Source rectangle in the image: x, y, width, height (pixels, may be fractional).
    pub src: [f64; 4],
    /// Destination of the rectangle's top-left corner, level pixels.
    pub dest: (f64, f64),
    /// Raster position of its first grid cell: (row, column).
    pub order: (u32, u32),
}

/// Every subtile of one level, with a coarse spatial index.
#[derive(Debug, Clone, Default)]
pub struct LevelLayout {
    /// The subtiles.
    pub subtiles: Vec<Subtile>,
    buckets: HashMap<(i64, i64), Vec<u32>>,
    /// Stored images whose camera has no usable position (not drawn).
    pub unplaced: usize,
}

impl LevelLayout {
    /// The layout of level `level` of `slide`.
    #[allow(clippy::many_single_char_names)]
    pub fn build(slide: &MiraxSlide, level: usize) -> Result<LevelLayout> {
        let lv = slide.levels.get(level).ok_or_else(|| {
            Error::Usage(format!(
                "level {level} does not exist ({} levels)",
                slide.levels.len()
            ))
        })?;
        let (nx, _) = slide.grid;
        let div = u64::from(slide.divisions);
        let s = lv.step;
        let s0 = slide.levels[0].step;
        let ds = (s / s0.max(1)) as f64;
        let (w, h) = (f64::from(lv.image_size.0), f64::from(lv.image_size.1));
        let mut out = LevelLayout::default();
        let mut keys: Vec<u32> = lv.images.keys().copied().collect();
        keys.sort_unstable();
        let Some((cams, _)) = &slide.cameras else {
            // Slides exported with overlaps removed have no position table; how their images
            // are placed has not been confirmed on any file: refuse rather than guess.
            return Err(Error::unsupported(
                FORMAT_ID,
                "a MIRAX slide without a camera position table (exported with overlaps removed)",
                "No development file of this kind has been validated; `info`, `info --view structure`, `check` and the attachments still work. Please report the file (`openreadout report FILE`).",
            ));
        };
        if (s < div && div % s != 0) || (s >= div && s % div != 0) {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(
                    "level {level}: {s} grid cells per image with {div} images per camera photo side"
                ),
                "Images are expected to cover whole camera photos or whole divisions of one.",
            ));
        }
        let valid = valid_cameras(slide);
        let (cx, cy) = slide.camera_grid();
        let pos = |col: u64, row: u64| -> Option<(f64, f64)> {
            if col >= u64::from(cx) || row >= u64::from(cy) {
                return None;
            }
            let k = (row * u64::from(cx) + col) as usize;
            if !valid.get(k).copied().unwrap_or(false) {
                return None;
            }
            let c = cams.get(k)?;
            Some((f64::from(c.x) / ds, f64::from(c.y) / ds))
        };
        for idx in keys {
            let (gx, gy) = (u64::from(idx % nx), u64::from(idx / nx));
            if s <= div {
                let Some((px, py)) = pos(gx / div, gy / div) else {
                    out.unplaced += 1;
                    continue;
                };
                let ox = ((gx % div) / s) as f64 * w;
                let oy = ((gy % div) / s) as f64 * h;
                out.push(Subtile {
                    image: idx,
                    src: [0.0, 0.0, w, h],
                    dest: (px + ox, py + oy),
                    order: (gy as u32, gx as u32),
                });
            } else {
                let n = s / div;
                let (sw, sh) = (w / n as f64, h / n as f64);
                let mut placed = 0usize;
                for j in 0..n {
                    for i in 0..n {
                        let Some(dest) = pos(gx / div + i, gy / div + j) else {
                            continue;
                        };
                        placed += 1;
                        out.push(Subtile {
                            image: idx,
                            src: [i as f64 * w / n as f64, j as f64 * h / n as f64, sw, sh],
                            dest,
                            order: ((gy + j * div) as u32, (gx + i * div) as u32),
                        });
                    }
                }
                if placed == 0 {
                    out.unplaced += 1;
                }
            }
        }
        Ok(out)
    }

    fn push(&mut self, t: Subtile) {
        let k = self.subtiles.len() as u32;
        let (x0, y0) = (t.dest.0 - 1.0, t.dest.1 - 1.0);
        let (x1, y1) = (
            t.dest.0 + t.src[2].ceil() + 1.0,
            t.dest.1 + t.src[3].ceil() + 1.0,
        );
        for by in (y0 / BUCKET).floor() as i64..=(y1 / BUCKET).floor() as i64 {
            for bx in (x0 / BUCKET).floor() as i64..=(x1 / BUCKET).floor() as i64 {
                self.buckets.entry((bx, by)).or_default().push(k);
            }
        }
        self.subtiles.push(t);
    }

    /// Subtiles that can touch `region`, in painting order (reverse raster order).
    pub fn candidates(&self, region: Region) -> Vec<Subtile> {
        let (rx0, ry0) = (f64::from(region.x), f64::from(region.y));
        let (rx1, ry1) = (
            rx0 + f64::from(region.width),
            ry0 + f64::from(region.height),
        );
        let mut ids: Vec<u32> = Vec::new();
        for by in (ry0 / BUCKET).floor() as i64..=((ry1 - 1.0) / BUCKET).floor() as i64 {
            for bx in (rx0 / BUCKET).floor() as i64..=((rx1 - 1.0) / BUCKET).floor() as i64 {
                if let Some(v) = self.buckets.get(&(bx, by)) {
                    ids.extend_from_slice(v);
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        let mut out: Vec<Subtile> = ids
            .into_iter()
            .filter_map(|k| self.subtiles.get(k as usize).copied())
            .filter(|t| {
                t.dest.0 - 1.0 < rx1
                    && t.dest.1 - 1.0 < ry1
                    && t.dest.0 + t.src[2].ceil() + 1.0 > rx0
                    && t.dest.1 + t.src[3].ceil() + 1.0 > ry0
            })
            .collect();
        out.sort_by_key(|t| std::cmp::Reverse(t.order));
        out
    }

    /// The stored images the subtiles of `region` come from.
    pub fn images_for(&self, region: Region) -> Vec<u32> {
        let mut v: Vec<u32> = self.candidates(region).iter().map(|t| t.image).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// Cameras whose images are drawn: covered by a stored level-0 image and, when the table's
/// flags are used at all (version 1.9 and later), flagged 1.
pub fn valid_cameras(slide: &MiraxSlide) -> Vec<bool> {
    let Some((cams, _)) = &slide.cameras else {
        return Vec::new();
    };
    let (cx, cy) = slide.camera_grid();
    let mut v = vec![false; cams.len()];
    let l0 = &slide.levels[0];
    let div = u64::from(slide.divisions);
    let span = (l0.step / div).max(1);
    let nx = slide.grid.0;
    for &idx in l0.images.keys() {
        let (gx, gy) = (u64::from(idx % nx), u64::from(idx / nx));
        for j in 0..span {
            for i in 0..span {
                let (c, r) = (gx / div + i, gy / div + j);
                if c < u64::from(cx)
                    && r < u64::from(cy)
                    && let Some(slot) = v.get_mut((r * u64::from(cx) + c) as usize)
                {
                    *slot = true;
                }
            }
        }
    }
    if cams.iter().any(|c| c.flag == 1) {
        for (slot, c) in v.iter_mut().zip(cams) {
            *slot &= c.flag == 1;
        }
    }
    v
}

/// A premultiplied RGBA surface of `f32` samples.
struct Surface<'a> {
    w: usize,
    h: usize,
    kind: SurfaceKind<'a>,
}

enum SurfaceKind<'a> {
    /// Pixels `(x0.., y0..)` of a stored image, opaque.
    View {
        img: &'a RgbImage,
        x0: usize,
        y0: usize,
    },
    /// A resampled copy: colour (premultiplied) and alpha per pixel.
    Owned { rgb: Vec<f32>, alpha: Vec<f32> },
}

impl Surface<'_> {
    /// Premultiplied colour and alpha of pixel (x, y), transparent outside.
    #[inline]
    fn px(&self, x: i64, y: i64) -> ([f32; 3], f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return ([0.0; 3], 0.0);
        }
        let (x, y) = (x as usize, y as usize);
        match &self.kind {
            SurfaceKind::View { img, x0, y0 } => {
                let (ix, iy) = (x0 + x, y0 + y);
                if ix >= img.width as usize || iy >= img.height as usize {
                    return ([0.0; 3], 0.0);
                }
                let o = (iy * img.width as usize + ix) * 3;
                let p = &img.data[o..o + 3];
                ([f32::from(p[0]), f32::from(p[1]), f32::from(p[2])], 1.0)
            }
            SurfaceKind::Owned { rgb, alpha } => {
                let k = y * self.w + x;
                ([rgb[k * 3], rgb[k * 3 + 1], rgb[k * 3 + 2]], alpha[k])
            }
        }
    }

    /// Bilinear sample at (u, v) (pixel-index coordinates: pixel i's centre is at i).
    #[inline]
    #[allow(clippy::many_single_char_names)]
    fn sample(&self, u: f64, v: f64) -> ([f32; 3], f32) {
        let (u0, v0) = (u.floor(), v.floor());
        let (fu, fv) = ((u - u0) as f32, (v - v0) as f32);
        let (iu, iv) = (u0 as i64, v0 as i64);
        let mut c = [0f32; 3];
        let mut a = 0f32;
        for (dy, wy) in [(0i64, 1.0 - fv), (1, fv)] {
            if wy == 0.0 {
                continue;
            }
            for (dx, wx) in [(0i64, 1.0 - fu), (1, fu)] {
                let wgt = wx * wy;
                if wgt == 0.0 {
                    continue;
                }
                let (p, pa) = self.px(iu + dx, iv + dy);
                c[0] += wgt * p[0];
                c[1] += wgt * p[1];
                c[2] += wgt * p[2];
                a += wgt * pa;
            }
        }
        (c, a)
    }
}

/// Compose `region` of a level from its subtiles (`layout.candidates(region)`), fetching
/// decoded images with `image`, over the background colour `bg` (R, G, B). Returns
/// interleaved 8-bit R, G, B.
#[allow(clippy::many_single_char_names)]
pub fn compose(
    subtiles: &[Subtile],
    region: Region,
    bg: [u8; 3],
    mut image: impl FnMut(u32) -> Result<Arc<RgbImage>>,
) -> Result<Vec<u8>> {
    let (rw, rh) = (region.width as usize, region.height as usize);
    let n = rw
        .checked_mul(rh)
        .ok_or_else(|| Error::Usage("region too large".into()))?;
    let mut col = vec![0f32; n * 3];
    let mut alpha = vec![0f32; n];
    let (rx0, ry0) = (i64::from(region.x), i64::from(region.y));
    for t in subtiles {
        let img = image(t.image)?;
        let [sx, sy, sw, sh] = t.src;
        let (cw, ch) = (sw.ceil().max(0.0) as usize, sh.ceil().max(0.0) as usize);
        let integral =
            sx.fract() == 0.0 && sy.fract() == 0.0 && sw.fract() == 0.0 && sh.fract() == 0.0;
        let surface = if integral {
            Surface {
                w: cw,
                h: ch,
                kind: SurfaceKind::View {
                    img: &img,
                    x0: sx as usize,
                    y0: sy as usize,
                },
            }
        } else {
            // Resample the fractional source rectangle into its own surface first.
            let whole = Surface {
                w: img.width as usize,
                h: img.height as usize,
                kind: SurfaceKind::View {
                    img: &img,
                    x0: 0,
                    y0: 0,
                },
            };
            let mut rgb = vec![0f32; cw * ch * 3];
            let mut al = vec![0f32; cw * ch];
            for j in 0..ch {
                for i in 0..cw {
                    let (c, a) = whole.sample(sx + i as f64, sy + j as f64);
                    let k = j * cw + i;
                    rgb[k * 3..k * 3 + 3].copy_from_slice(&c);
                    al[k] = a;
                }
            }
            Surface {
                w: cw,
                h: ch,
                kind: SurfaceKind::Owned { rgb, alpha: al },
            }
        };
        let (dx, dy) = t.dest;
        let x_lo = (dx.floor() as i64 - 1).max(rx0);
        let x_hi = ((dx + cw as f64).ceil() as i64 + 1).min(rx0 + rw as i64);
        let y_lo = (dy.floor() as i64 - 1).max(ry0);
        let y_hi = ((dy + ch as f64).ceil() as i64 + 1).min(ry0 + rh as i64);
        for y in y_lo..y_hi {
            let v = y as f64 - dy;
            let row = (y - ry0) as usize * rw;
            for x in x_lo..x_hi {
                let k = row + (x - rx0) as usize;
                let have = alpha[k];
                if have >= 1.0 {
                    continue;
                }
                let (c, a) = surface.sample(x as f64 - dx, v);
                if a <= 0.0 {
                    continue;
                }
                // Saturate: the source adds only what the destination still lacks.
                let f = ((1.0 - have) / a).min(1.0);
                col[k * 3] += c[0] * f;
                col[k * 3 + 1] += c[1] * f;
                col[k * 3 + 2] += c[2] * f;
                alpha[k] = (have + a * f).min(1.0);
            }
        }
    }
    let mut out = vec![0u8; n * 3];
    for k in 0..n {
        let rest = 1.0 - alpha[k];
        for c in 0..3 {
            let v = col[k * 3 + c] + rest * f32::from(bg[c]);
            out[k * 3 + c] = (v + 0.5).floor().clamp(0.0, 255.0) as u8;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Arc<RgbImage> {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&f(x, y));
            }
        }
        Arc::new(RgbImage {
            width: w,
            height: h,
            data,
        })
    }

    #[test]
    fn integer_offsets_copy_samples_and_later_wins() {
        let a = img(4, 4, |_, _| [10, 20, 30]);
        let b = img(4, 4, |x, _| [100 + x as u8, 0, 0]);
        // b is later in raster order (column 1): painted first in reverse order, wins overlap
        let subs = vec![
            Subtile {
                image: 1,
                src: [0.0, 0.0, 4.0, 4.0],
                dest: (2.0, 0.0),
                order: (0, 1),
            },
            Subtile {
                image: 0,
                src: [0.0, 0.0, 4.0, 4.0],
                dest: (0.0, 0.0),
                order: (0, 0),
            },
        ];
        let out = compose(&subs, Region::new(0, 0, 7, 1), [255, 255, 255], |i| {
            Ok(if i == 0 { a.clone() } else { b.clone() })
        })
        .unwrap();
        let r: Vec<u8> = out.chunks(3).map(|p| p[0]).collect();
        assert_eq!(r, vec![10, 10, 100, 101, 102, 103, 255]);
    }

    #[test]
    fn half_pixel_seam_averages_with_full_opacity() {
        // two images of one camera meeting at x = 2.5: the seam pixel is their mean
        let a = img(2, 1, |_, _| [100, 100, 100]);
        let b = img(2, 1, |_, _| [200, 200, 200]);
        let subs = vec![
            Subtile {
                image: 1,
                src: [0.0, 0.0, 2.0, 1.0],
                dest: (2.5, 0.0),
                order: (0, 1),
            },
            Subtile {
                image: 0,
                src: [0.0, 0.0, 2.0, 1.0],
                dest: (0.5, 0.0),
                order: (0, 0),
            },
        ];
        let out = compose(&subs, Region::new(0, 0, 5, 1), [0, 0, 0], |i| {
            Ok(if i == 0 { a.clone() } else { b.clone() })
        })
        .unwrap();
        let r: Vec<u8> = out.chunks(3).map(|p| p[0]).collect();
        // pixel 0: half of a over black; 1: a; 2: seam (mean); 3: b; 4: half of b over black
        assert_eq!(r, vec![50, 100, 150, 200, 100]);
    }
}
