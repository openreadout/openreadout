//! The power-spectrum inset of an image preview (`fft`): the averaged power spectrum of square
//! tiles from the middle of the plane read, log-scaled, zero frequency in the middle. On a
//! cryo-EM micrograph it shows the Thon rings (defocus, astigmatism, drift); on a lattice it
//! shows the lattice spots.

use openreadout_signal::fft::{Complex, fft_in_place};

/// The bin factor for a plane of `short` pixels on its shorter side and `pixel_a` ångström per
/// pixel: about 1.6 Å per binned pixel when the pixel size is known (Nyquist near 3 Å, where
/// Thon rings of a micrograph are), else down to about 1024 pixels.
pub(crate) fn bin_for(short: usize, pixel_a: Option<f64>) -> usize {
    match pixel_a.filter(|p| *p > 0.0 && p.is_finite()) {
        Some(p) => ((1.6 / p).floor() as usize).clamp(1, 16),
        None => (short / 1024).clamp(1, 16),
    }
}

/// Largest tile side.
const MAX_TILE: usize = 512;
/// Smallest plane side that gets an inset.
pub(crate) const MIN_SIDE: usize = 128;
/// Tiles averaged at most along each axis.
const MAX_TILES: usize = 8;

/// The log power spectrum of a `w`×`h` plane, averaged `bin` × `bin` first, as `(side, values
/// in 0..=1)`; `None` when the plane is too small. NaN samples count as the tile's mean.
pub(crate) fn power_spectrum(
    src: &[f32],
    w: usize,
    h: usize,
    bin: usize,
) -> Option<(usize, Vec<f32>)> {
    if src.len() < w * h {
        return None;
    }
    let bin = bin.max(1);
    let binned;
    let (src, w, h) = if bin > 1 {
        let (bw, bh) = (w / bin, h / bin);
        binned = (0..bw * bh)
            .map(|i| {
                let (bx, by) = (i % bw, i / bw);
                let (mut s, mut n) = (0f32, 0u32);
                for y in by * bin..(by + 1) * bin {
                    for &v in &src[y * w + bx * bin..y * w + (bx + 1) * bin] {
                        if v.is_finite() {
                            s += v;
                            n += 1;
                        }
                    }
                }
                if n > 0 { s / n as f32 } else { f32::NAN }
            })
            .collect::<Vec<f32>>();
        (&binned[..], bw, bh)
    } else {
        (src, w, h)
    };
    let short = w.min(h);
    if short < MIN_SIDE {
        return None;
    }
    let tile = (1usize << (usize::BITS - 1 - short.leading_zeros())).min(MAX_TILE);
    let nx = (w / tile).clamp(1, MAX_TILES);
    let ny = (h / tile).clamp(1, MAX_TILES);
    let x0 = (w - nx * tile) / 2;
    let y0 = (h - ny * tile) / 2;
    let hann: Vec<f64> = (0..tile)
        .map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / tile as f64).cos())
        .collect();
    let mut power = vec![0f64; tile * tile];
    let mut buf = vec![Complex::default(); tile * tile];
    let mut col = vec![Complex::default(); tile];
    for ty in 0..ny {
        for tx in 0..nx {
            let (ox, oy) = (x0 + tx * tile, y0 + ty * tile);
            let (mut sum, mut n) = (0f64, 0usize);
            for y in 0..tile {
                for &v in &src[(oy + y) * w + ox..(oy + y) * w + ox + tile] {
                    if v.is_finite() {
                        sum += f64::from(v);
                        n += 1;
                    }
                }
            }
            let mean = if n > 0 { sum / n as f64 } else { 0.0 };
            for y in 0..tile {
                let row = &src[(oy + y) * w + ox..(oy + y) * w + ox + tile];
                for (x, &v) in row.iter().enumerate() {
                    let v = if v.is_finite() { f64::from(v) } else { mean };
                    buf[y * tile + x] = Complex {
                        re: (v - mean) * hann[x] * hann[y],
                        im: 0.0,
                    };
                }
            }
            for r in buf.chunks_exact_mut(tile) {
                fft_in_place(r, false).ok()?;
            }
            for x in 0..tile {
                for y in 0..tile {
                    col[y] = buf[y * tile + x];
                }
                fft_in_place(&mut col, false).ok()?;
                for y in 0..tile {
                    let c = col[y];
                    power[y * tile + x] += c.re * c.re + c.im * c.im;
                }
            }
        }
    }
    // Zero frequency in the middle, log scale.
    let half = tile / 2;
    let mut out = vec![0f32; tile * tile];
    for y in 0..tile {
        for x in 0..tile {
            let p = power[((y + half) % tile) * tile + (x + half) % tile];
            out[y * tile + x] = p.ln_1p() as f32;
        }
    }
    // Take out the smooth fall of power with frequency: subtract the rotational average,
    // smoothed over a wide band of radii, so that rings (Thon rings) and spots stand out.
    let nr = half + 2;
    let (mut rsum, mut rcount) = (vec![0f64; nr * 2], vec![0usize; nr * 2]);
    let radius = |x: usize, y: usize| {
        let (dx, dy) = (x as f64 - half as f64, y as f64 - half as f64);
        (dx * dx + dy * dy).sqrt()
    };
    for y in 0..tile {
        for x in 0..tile {
            let r = radius(x, y).round() as usize;
            if r < rsum.len() {
                rsum[r] += f64::from(out[y * tile + x]);
                rcount[r] += 1;
            }
        }
    }
    let profile: Vec<f64> = rsum
        .iter()
        .zip(&rcount)
        .map(|(s, &n)| if n > 0 { s / n as f64 } else { 0.0 })
        .collect();
    let win = (tile / 32).max(2);
    let smooth: Vec<f64> = (0..profile.len())
        .map(|r| {
            let (a, b) = (r.saturating_sub(win), (r + win + 1).min(profile.len()));
            let used: Vec<f64> = (a..b)
                .filter(|&k| rcount[k] > 0)
                .map(|k| profile[k])
                .collect();
            if used.is_empty() {
                0.0
            } else {
                used.iter().sum::<f64>() / used.len() as f64
            }
        })
        .collect();
    for y in 0..tile {
        for x in 0..tile {
            let r = radius(x, y);
            let k = (r.floor() as usize).min(smooth.len() - 2);
            let f = r - k as f64;
            let bg = smooth[k] * (1.0 - f) + smooth[k + 1] * f;
            out[y * tile + x] -= bg as f32;
        }
    }
    // Contrast from the 1st to the 99.5th percentile, the middle 5 × 5 left out.
    let mut vals: Vec<f32> = out
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let (x, y) = (i % tile, i / tile);
            x.abs_diff(half) > 2 || y.abs_diff(half) > 2
        })
        .map(|(_, v)| *v)
        .collect();
    vals.sort_by(f32::total_cmp);
    let pick = |p: f64| vals[((vals.len() - 1) as f64 * p) as usize];
    let (lo, hi) = (pick(0.01), pick(0.995));
    let span = (hi - lo).max(1e-12);
    for v in &mut out {
        *v = ((*v - lo) / span).clamp(0.0, 1.0);
    }
    Some((tile, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stripe_pattern_gives_two_peaks_on_its_axis() {
        // vertical stripes with a period of 8 pixels: power at x = ±side/8 from the middle
        let (w, h) = (256usize, 256usize);
        let src: Vec<f32> = (0..w * h)
            .map(|i| ((i % w) as f32 * std::f32::consts::TAU / 8.0).sin())
            .collect();
        let (side, ps) = power_spectrum(&src, w, h, 1).unwrap();
        assert_eq!(side, 256);
        let mid = side / 2;
        let at = |x: usize, y: usize| ps[y * side + x];
        assert!(at(mid + side / 8, mid) > 0.99);
        assert!(at(mid - side / 8, mid) > 0.99);
        assert!(at(mid, mid + side / 8) < 0.5);
        assert!(power_spectrum(&src[..100], 10, 10, 1).is_none());
        // binned 2 × 2: the period halves to 4 binned pixels
        let (side, ps) = power_spectrum(&src, w, h, 2).unwrap();
        assert_eq!(side, 128);
        assert!(ps[(side / 2) * side + side / 2 + side / 4] > 0.99);
    }
}
