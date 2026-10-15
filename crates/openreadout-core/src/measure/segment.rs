//! Image-processing steps for counting objects: smoothing, background, thresholds, the
//! distance transform, seeded watershed, Laplacian-of-Gaussian spot detection, colour
//! deconvolution and outlines. Planes are `f32`, row-major, `w × h`.
//!
//! Algorithm sources (cited in `book/src/reference/commands/stats.md`):
//! - Otsu, "A threshold selection method from gray-level histograms", IEEE SMC 9 (1979).
//! - Zack, Rogers and Latt, "Automatic measurement of sister chromatid exchange frequency",
//!   J Histochem Cytochem 25 (1977) (the triangle threshold).
//! - Felzenszwalb and Huttenlocher, "Distance transforms of sampled functions", Theory of
//!   Computing 8 (2012) (the exact Euclidean distance transform).
//! - Beucher and Meyer, "The morphological approach to segmentation: the watershed
//!   transformation" (1993) (marker-controlled watershed by priority flooding).
//! - Lindeberg, "Feature detection with automatic scale selection", IJCV 30 (1998) (the
//!   scale-normalised Laplacian of Gaussian for spots).
//! - Ruifrok and Johnston, "Quantification of histochemical staining by color deconvolution",
//!   Anal Quant Cytol Histol 23 (2001) (stain vectors for hematoxylin, eosin and DAB).

// x, y, w, h are the natural names in image arithmetic.
#![allow(clippy::many_single_char_names)]

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Gaussian blur with σ in pixels (edges repeat the border value). σ ≤ 0 copies.
pub(crate) fn gaussian(v: &[f32], w: usize, h: usize, sigma: f64) -> Vec<f32> {
    if sigma <= 0.0 || w == 0 || h == 0 {
        return v.to_vec();
    }
    let rad = (3.0 * sigma).ceil().max(1.0) as usize;
    let mut k: Vec<f32> = (0..=2 * rad)
        .map(|i| {
            let d = i as f64 - rad as f64;
            (-(d * d) / (2.0 * sigma * sigma)).exp() as f32
        })
        .collect();
    let s: f32 = k.iter().sum();
    for x in &mut k {
        *x /= s;
    }
    let mut tmp = vec![0f32; v.len()];
    let mut line = vec![0f32; w.max(h) + 2 * rad];
    for y in 0..h {
        let row = &v[y * w..(y + 1) * w];
        for (i, l) in line.iter_mut().take(w + 2 * rad).enumerate() {
            *l = row[i.saturating_sub(rad).min(w - 1)];
        }
        for x in 0..w {
            tmp[y * w + x] = k.iter().zip(&line[x..]).map(|(a, b)| a * b).sum();
        }
    }
    let mut out = vec![0f32; v.len()];
    for x in 0..w {
        for (i, l) in line.iter_mut().take(h + 2 * rad).enumerate() {
            *l = tmp[i.saturating_sub(rad).min(h - 1) * w + x];
        }
        for y in 0..h {
            out[y * w + x] = k.iter().zip(&line[y..]).map(|(a, b)| a * b).sum();
        }
    }
    out
}

/// Mean of each `f × f` block (the last blocks may be smaller).
pub(crate) fn shrink(v: &[f32], w: usize, h: usize, f: usize) -> (Vec<f32>, usize, usize) {
    let (sw, sh) = (w.div_ceil(f), h.div_ceil(f));
    let mut sum = vec![0f32; sw * sh];
    let mut n = vec![0u32; sw * sh];
    for y in 0..h {
        for x in 0..w {
            let j = (y / f) * sw + x / f;
            sum[j] += v[y * w + x];
            n[j] += 1;
        }
    }
    for (s, c) in sum.iter_mut().zip(&n) {
        *s /= (*c).max(1) as f32;
    }
    (sum, sw, sh)
}

/// Bilinear enlargement of a plane shrunk by `f` back to `w × h`.
fn enlarge(s: &[f32], sw: usize, sh: usize, f: usize, w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let fy = ((y as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let ay = fy - y0 as f32;
        for x in 0..w {
            let fx = ((x as f32 + 0.5) / f as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let ax = fx - x0 as f32;
            let top = s[y0 * sw + x0] * (1.0 - ax) + s[y0 * sw + x1] * ax;
            let bot = s[y1 * sw + x0] * (1.0 - ax) + s[y1 * sw + x1] * ax;
            out[y * w + x] = top * (1.0 - ay) + bot * ay;
        }
    }
    out
}

/// Minimum (or maximum) over a square window of radius `r`, separable.
pub(crate) fn rank_filter(v: &[f32], w: usize, h: usize, r: usize, max: bool) -> Vec<f32> {
    let pick = |a: f32, b: f32| if max { a.max(b) } else { a.min(b) };
    let mut tmp = vec![0f32; v.len()];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            tmp[y * w + x] = v[y * w + a..=y * w + b]
                .iter()
                .copied()
                .reduce(pick)
                .unwrap_or(0.0);
        }
    }
    let mut out = vec![0f32; v.len()];
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for x in 0..w {
            let mut m = tmp[a * w + x];
            for yy in a + 1..=b {
                m = pick(m, tmp[yy * w + x]);
            }
            out[y * w + x] = m;
        }
    }
    out
}

/// The slowly varying background under objects of diameter `d` pixels: a grey-level opening
/// (minimum then maximum over a window as wide as an object) of the plane shrunk to about
/// four pixels per object diameter, smoothed and enlarged back.
pub(crate) fn background(v: &[f32], w: usize, h: usize, d: f64) -> Vec<f32> {
    let f = ((d / 4.0).floor() as usize).clamp(1, 64);
    let (s, sw, sh) = shrink(v, w, h, f);
    let r = ((d / f as f64).ceil() as usize).max(1);
    let open = rank_filter(&rank_filter(&s, sw, sh, r, false), sw, sh, r, true);
    let smooth = gaussian(&open, sw, sh, r as f64 / 2.0);
    enlarge(&smooth, sw, sh, f, w, h)
}

/// The value below which a fraction `q` of the finite values lie (`q` in 0..=1).
pub(crate) fn quantile(v: &[f32], q: f64) -> Option<f32> {
    let mut s: Vec<f32> = if v.len() > 1_000_000 {
        // a strided sample is plenty for a quantile
        let step = v.len() / 500_000;
        v.iter().step_by(step).copied().filter(|x| x.is_finite()).collect()
    } else {
        v.iter().copied().filter(|x| x.is_finite()).collect()
    };
    if s.is_empty() {
        return None;
    }
    let k = ((s.len() - 1) as f64 * q.clamp(0.0, 1.0)).round() as usize;
    let (_, x, _) = s.select_nth_unstable_by(k, f32::total_cmp);
    Some(*x)
}

/// A 256-bin histogram of the values between `lo` and `hi` (values outside go to the end bins).
fn histogram(v: &[f32], lo: f32, hi: f32) -> Vec<f64> {
    let mut hist = vec![0f64; 256];
    let scale = 255.0 / (hi - lo);
    for x in v.iter().filter(|x| x.is_finite()) {
        let b = ((x - lo) * scale).clamp(0.0, 255.0) as usize;
        hist[b] += 1.0;
    }
    hist
}

/// Otsu's threshold of the values between the 0.1th and 99.9th percentile (outliers such as
/// hot pixels would otherwise squeeze everything into a few bins), and how well it separates
/// the two classes (between-class over total variance, 0..1).
pub(crate) fn otsu(v: &[f32]) -> Option<(f32, f64)> {
    let lo = quantile(v, 0.001)?;
    let hi = quantile(v, 0.999)?;
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let hist = histogram(v, lo, hi);
    let total: f64 = hist.iter().sum();
    let sum_all: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * c).sum();
    let mean = sum_all / total;
    let var_t: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, c)| c * (i as f64 - mean).powi(2))
        .sum::<f64>()
        / total;
    let (mut w0, mut sum0) = (0.0, 0.0);
    let (mut best, mut best_i) = (-1.0, 0usize);
    for (i, c) in hist.iter().enumerate() {
        w0 += c;
        sum0 += i as f64 * c;
        let w1 = total - w0;
        if w0 == 0.0 || w1 <= 0.0 {
            continue;
        }
        let m0 = sum0 / w0;
        let m1 = (sum_all - sum0) / w1;
        let between = w0 * w1 * (m0 - m1).powi(2) / (total * total);
        if between > best {
            best = between;
            best_i = i;
        }
    }
    let t = lo + (best_i as f32 + 1.0) * (hi - lo) / 255.0;
    let sep = if var_t > 0.0 { best / var_t } else { 0.0 };
    Some((t, sep))
}

/// Otsu's two thresholds splitting the values into three classes (low, middle, high), by an
/// exhaustive search over a 256-bin histogram of the 0.1th to 99.9th percentile.
pub(crate) fn otsu3(v: &[f32]) -> Option<(f32, f32)> {
    let lo = quantile(v, 0.001)?;
    let hi = quantile(v, 0.999)?;
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let hist = histogram(v, lo, hi);
    // prefix sums of counts and of bin index × count
    let mut p = vec![0.0f64; 257];
    let mut q = vec![0.0f64; 257];
    for i in 0..256 {
        p[i + 1] = p[i] + hist[i];
        q[i + 1] = q[i] + i as f64 * hist[i];
    }
    let class = |a: usize, b: usize| -> f64 {
        let n = p[b] - p[a];
        if n <= 0.0 {
            return 0.0;
        }
        let m = q[b] - q[a];
        m * m / n
    };
    let mut best = (f64::NEG_INFINITY, 0usize, 0usize);
    for t1 in 1..255 {
        for t2 in t1 + 1..256 {
            let s = class(0, t1) + class(t1, t2) + class(t2, 256);
            if s > best.0 {
                best = (s, t1, t2);
            }
        }
    }
    let at = |b: usize| lo + b as f32 * (hi - lo) / 255.0;
    Some((at(best.1), at(best.2)))
}

/// The triangle threshold: the histogram level farthest below the line from its peak to the
/// far end of its longer tail. Suits a large background peak with few, sparse objects.
pub(crate) fn triangle(v: &[f32]) -> Option<f32> {
    let lo = quantile(v, 0.001)?;
    let hi = quantile(v, 0.999)?;
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let hist = histogram(v, lo, hi);
    let peak = (0..256).max_by(|a, b| hist[*a].total_cmp(&hist[*b]))?;
    let last = (0..256).rev().find(|i| hist[*i] > 0.0)?;
    let first = (0..256).find(|i| hist[*i] > 0.0)?;
    // the longer tail
    let (end, step): (usize, isize) = if last - peak >= peak - first {
        (last, 1)
    } else {
        (first, -1)
    };
    let (x0, y0, x1, y1) = (peak as f64, hist[peak], end as f64, hist[end]);
    let norm = ((y1 - y0).powi(2) + (x1 - x0).powi(2)).sqrt();
    if norm == 0.0 {
        return None;
    }
    let mut best = (0.0, peak);
    let mut i = peak as isize;
    while i != end as isize {
        let x = i as f64;
        let d = ((y1 - y0) * x - (x1 - x0) * hist[i as usize] + x1 * y0 - y1 * x0).abs() / norm;
        if d > best.0 {
            best = (d, i as usize);
        }
        i += step;
    }
    Some(lo + (best.1 as f32 + 0.5) * (hi - lo) / 255.0)
}

/// Background regions that do not reach the border (4-connected) and have at most
/// `max_area` pixels become foreground: holes inside a nucleus belong to it, while the larger
/// gaps enclosed by a ring of touching cells stay background.
pub(crate) fn fill_holes(fg: &mut [bool], w: usize, h: usize, max_area: usize) {
    let mut seen = vec![false; w * h];
    let mut stack = Vec::new();
    let mut region = Vec::new();
    for s in 0..w * h {
        if fg[s] || seen[s] {
            continue;
        }
        seen[s] = true;
        stack.push(s);
        region.clear();
        let mut border = false;
        while let Some(i) = stack.pop() {
            region.push(i);
            let (x, y) = (i % w, i / w);
            border |= x == 0 || y == 0 || x == w - 1 || y == h - 1;
            let mut visit = |j: usize| {
                if !fg[j] && !seen[j] {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        if !border && region.len() <= max_area {
            for &i in &region {
                fg[i] = true;
            }
        }
    }
}

/// Squared distance transform of one line (Felzenszwalb and Huttenlocher's lower envelope of
/// parabolas). `f` holds 0 at background and a large value elsewhere.
fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let sq = |i: usize| (i * i) as f64;
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let mut s;
        loop {
            let p = v[k];
            s = ((f[q] + sq(q)) - (f[p] + sq(p))) / (2.0 * (q - p) as f64);
            // z[0] is -inf, so this stops at k = 0
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, dq) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        let dx = q as f64 - p as f64;
        *dq = dx * dx + f[p];
    }
}

/// Euclidean distance of each foreground pixel to the nearest background pixel (pixels
/// outside the plane count as foreground, so objects cut by the edge are not made thinner).
pub(crate) fn distance(fg: &[bool], w: usize, h: usize) -> Vec<f32> {
    let big = ((w * w + h * h) as f64) * 4.0 + 1.0;
    let n = w.max(h);
    let (mut f, mut d) = (vec![0f64; n], vec![0f64; n]);
    let (mut v, mut z) = (vec![0usize; n], vec![0f64; n + 1]);
    let mut sq: Vec<f64> = fg.iter().map(|b| if *b { big } else { 0.0 }).collect();
    for x in 0..w {
        for y in 0..h {
            f[y] = sq[y * w + x];
        }
        edt_1d(&f[..h], &mut d[..h], &mut v, &mut z);
        for y in 0..h {
            sq[y * w + x] = d[y];
        }
    }
    for y in 0..h {
        f[..w].copy_from_slice(&sq[y * w..(y + 1) * w]);
        edt_1d(&f[..w], &mut d[..w], &mut v, &mut z);
        sq[y * w..(y + 1) * w].copy_from_slice(&d[..w]);
    }
    // a plane with no background at all: every pixel is as deep as the plane is wide
    sq.iter()
        .map(|x| if *x >= big { n as f32 } else { x.sqrt() as f32 })
        .collect()
}

/// Label the 8-connected foreground regions 1, 2, … (0 = background); returns the count.
pub(crate) fn components(fg: &[bool], w: usize, h: usize) -> (Vec<u32>, u32) {
    let mut lab = vec![0u32; w * h];
    let mut n = 0u32;
    let mut stack = Vec::new();
    for s in 0..w * h {
        if !fg[s] || lab[s] != 0 {
            continue;
        }
        n += 1;
        lab[s] = n;
        stack.push(s);
        while let Some(i) = stack.pop() {
            for j in neighbours8(i, w, h) {
                if fg[j] && lab[j] == 0 {
                    lab[j] = n;
                    stack.push(j);
                }
            }
        }
    }
    (lab, n)
}

/// The up to eight neighbours of pixel `i`.
fn neighbours8(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = ((i % w) as isize, (i / w) as isize);
    (-1isize..=1)
        .flat_map(move |dy| (-1isize..=1).map(move |dx| (dx, dy)))
        .filter(|&(dx, dy)| dx != 0 || dy != 0)
        .filter_map(move |(dx, dy)| {
            let (nx, ny) = (x + dx, y + dy);
            (nx >= 0 && ny >= 0 && nx < w as isize && ny < h as isize)
                .then(|| ny as usize * w + nx as usize)
        })
}

/// Seeds for the watershed: the regional maxima of `m` inside the mask that are the largest
/// value within `radius` pixels and at least `floor`. A plateau of equal maxima is one seed.
/// Returns a label image of seeds (1, 2, …) and their count.
pub(crate) fn seeds(
    m: &[f32],
    fg: &[bool],
    w: usize,
    h: usize,
    radius: usize,
    floor: f32,
) -> (Vec<u32>, u32) {
    let masked: Vec<f32> = m
        .iter()
        .zip(fg)
        .map(|(v, f)| if *f { *v } else { f32::NEG_INFINITY })
        .collect();
    let mx = rank_filter(&masked, w, h, radius.max(1), true);
    let peak: Vec<bool> = (0..w * h)
        .map(|i| fg[i] && masked[i] >= floor && masked[i] >= mx[i])
        .collect();
    components(&peak, w, h)
}

/// Marker-controlled watershed: grow the seed labels over the mask in order of decreasing
/// `m` (each pixel joins the neighbour region it is reached from first). Mask pixels that no
/// seed reaches keep label 0.
pub(crate) fn watershed(m: &[f32], fg: &[bool], seeds: &[u32], w: usize, h: usize) -> Vec<u32> {
    let mut lab = seeds.to_vec();
    // priority: higher m first; quantised to keep the heap entries small
    let (lo, hi) = m
        .iter()
        .zip(fg)
        .filter(|(_, f)| **f)
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), (v, _)| {
            (a.min(*v), b.max(*v))
        });
    let scale = if hi > lo { 65535.0 / (hi - lo) } else { 0.0 };
    let key = |i: usize| -> u32 { ((m[i] - lo) * scale).clamp(0.0, 65535.0) as u32 };
    let mut heap: BinaryHeap<(u32, Reverse<u64>, u32)> = BinaryHeap::new();
    let mut queued = vec![false; w * h];
    let mut seq = 0u64;
    for i in 0..w * h {
        if lab[i] != 0 {
            queued[i] = true;
            for j in neighbours8(i, w, h) {
                if fg[j] && lab[j] == 0 && !queued[j] {
                    queued[j] = true;
                    heap.push((key(j), Reverse(seq), j as u32));
                    seq += 1;
                }
            }
        }
    }
    while let Some((_, _, j)) = heap.pop() {
        let j = j as usize;
        // the label of the deepest labelled neighbour
        let mut best: Option<(f32, u32)> = None;
        for k in neighbours8(j, w, h) {
            if lab[k] != 0 && best.is_none_or(|b| m[k] > b.0) {
                best = Some((m[k], lab[k]));
            }
        }
        let Some((_, l)) = best else { continue };
        lab[j] = l;
        for k in neighbours8(j, w, h) {
            if fg[k] && lab[k] == 0 && !queued[k] {
                queued[k] = true;
                heap.push((key(k), Reverse(seq), k as u32));
                seq += 1;
            }
        }
    }
    lab
}

/// Renumber labels 1..n in order of first appearance, dropping regions under `min_area` pixels
/// (they merge into the neighbour they share the longest border with, or vanish when they
/// touch none). Returns the label image and the count.
pub(crate) fn tidy_labels(lab: &mut [u32], w: usize, h: usize, min_area: usize) -> u32 {
    let n = lab.iter().copied().max().unwrap_or(0) as usize;
    let mut area = vec![0usize; n + 1];
    for &l in lab.iter() {
        area[l as usize] += 1;
    }
    // small regions: the neighbour with the longest shared border
    let mut target: Vec<u32> = (0..=n as u32).collect();
    let mut border: std::collections::HashMap<(u32, u32), usize> = std::collections::HashMap::new();
    for i in 0..w * h {
        let a = lab[i];
        if a == 0 || area[a as usize] >= min_area {
            continue;
        }
        let (x, y) = (i % w, i / w);
        for j in [
            (x + 1 < w).then(|| i + 1),
            (x > 0).then(|| i - 1),
            (y + 1 < h).then(|| i + w),
            (y > 0).then(|| i - w),
        ]
        .into_iter()
        .flatten()
        {
            let b = lab[j];
            if b != 0 && b != a {
                *border.entry((a, b)).or_default() += 1;
            }
        }
    }
    let mut best: Vec<(usize, u32)> = vec![(0, 0); n + 1];
    for ((a, b), c) in border {
        let e = &mut best[a as usize];
        if c > e.0 || (c == e.0 && b < e.1) {
            *e = (c, b);
        }
    }
    for a in 1..=n {
        if area[a] > 0 && area[a] < min_area {
            target[a] = best[a].1;
        }
    }
    // follow chains of small regions merging into small regions (bounded: each step moves to a
    // different label, and a cycle stops at its smallest member)
    let resolve = |mut a: u32| -> u32 {
        for _ in 0..8 {
            let t = target[a as usize];
            if t == a || t == 0 {
                return t;
            }
            if area[t as usize] >= min_area {
                return t;
            }
            a = t;
        }
        0
    };
    let mut merged_area = area.clone();
    let mut final_of: Vec<u32> = (0..=n as u32).collect();
    for a in 1..=n {
        if area[a] > 0 && area[a] < min_area {
            let t = resolve(a as u32);
            final_of[a] = t;
            if t != 0 {
                merged_area[t as usize] += area[a];
            }
        }
    }
    let mut ids = vec![0u32; n + 1];
    let mut next = 0u32;
    for l in lab.iter_mut() {
        if *l == 0 {
            continue;
        }
        let f = final_of[*l as usize] as usize;
        if f == 0 || merged_area[f] < min_area {
            *l = 0;
            continue;
        }
        if ids[f] == 0 {
            next += 1;
            ids[f] = next;
        }
        *l = ids[f];
    }
    next
}

/// Scale-normalised Laplacian of Gaussian, negated so that bright blobs of radius about
/// `sigma · √2` give positive peaks.
pub(crate) fn log_filter(v: &[f32], w: usize, h: usize, sigma: f64) -> Vec<f32> {
    let g = gaussian(v, w, h, sigma);
    let s2 = (sigma * sigma) as f32;
    let at = |x: usize, y: usize| g[y * w + x];
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let c = at(x, y);
            let l = at(x.saturating_sub(1), y);
            let r = at((x + 1).min(w - 1), y);
            let u = at(x, y.saturating_sub(1));
            let d = at(x, (y + 1).min(h - 1));
            out[y * w + x] = -(l + r + u + d - 4.0 * c) * s2;
        }
    }
    out
}

/// The diameter (pixels) of the most prominent bright blobs: the scale at which the
/// scale-normalised Laplacian of Gaussian responds most strongly at its strongest local maxima
/// (Lindeberg's automatic scale selection). Coarse scales are computed on halved copies of the
/// plane, so the cost stays near that of a few blurs.
pub(crate) fn blob_diameter(v: &[f32], w: usize, h: usize) -> Option<f64> {
    // scores of sigma = 2^(i/2) pixels of the full plane, i = 0..
    let mut scores: Vec<(f64, f64)> = Vec::new();
    let (mut cur, mut cw, mut ch, mut scale) = (v.to_vec(), w, h, 1.0f64);
    for i in 0..14 {
        let sigma = 2f64.powf(f64::from(i) / 2.0);
        // keep the working sigma between 1 and 2.9 pixels of the current copy
        while sigma / scale > 2.9 && cw >= 32 && ch >= 32 {
            let (s, sw, sh) = shrink(&cur, cw, ch, 2);
            cur = s;
            cw = sw;
            ch = sh;
            scale *= 2.0;
        }
        let sl = sigma / scale;
        // a blob of this size must fit several times in the plane
        if sigma * 2.0 * std::f64::consts::SQRT_2 * 4.0 > (w.min(h)) as f64 {
            break;
        }
        let r = log_filter(&cur, cw, ch, sl);
        let mx = rank_filter(&r, cw, ch, (sl.ceil() as usize).max(1), true);
        let mut peaks: Vec<f32> = r
            .iter()
            .zip(&mx)
            .filter(|(a, m)| **a > 0.0 && **a >= **m)
            .map(|(a, _)| *a)
            .collect();
        if peaks.is_empty() {
            continue;
        }
        peaks.sort_by(|a, b| b.total_cmp(a));
        let k = peaks.len().min(30);
        let score = peaks[..k].iter().map(|x| f64::from(*x)).sum::<f64>() / k as f64;
        scores.push((sigma, score));
    }
    let best = (0..scores.len()).max_by(|a, b| scores[*a].1.total_cmp(&scores[*b].1))?;
    // a parabola through the best score and its neighbours, in log sigma
    let ls = |i: usize| scores[i].0.ln();
    let mut log_sigma = ls(best);
    if best > 0 && best + 1 < scores.len() {
        let (a, b, c) = (scores[best - 1].1, scores[best].1, scores[best + 1].1);
        let den = a - 2.0 * b + c;
        if den < 0.0 {
            let off = 0.5 * (a - c) / den;
            log_sigma += off.clamp(-0.5, 0.5) * (ls(best + 1) - ls(best));
        }
    }
    Some(2.0 * std::f64::consts::SQRT_2 * log_sigma.exp())
}

/// Optical density `-log10(I / I0)` of a transmitted-light value (`I0` the light through
/// empty glass), clamped at 0.
pub(crate) fn optical_density(v: f32, i0: f32) -> f32 {
    (-((v.max(0.0) + 1.0) / (i0 + 1.0)).log10()).max(0.0)
}

/// Stain optical-density vectors (red, green, blue) from Ruifrok and Johnston.
pub(crate) const HEMATOXYLIN: [f64; 3] = [0.650, 0.704, 0.286];
pub(crate) const EOSIN: [f64; 3] = [0.072, 0.990, 0.105];
pub(crate) const DAB: [f64; 3] = [0.268, 0.570, 0.776];

/// The inverse of the matrix whose rows are the three stain vectors (normalised), so that
/// `od · inv` gives the amount of each stain; `None` when the vectors are dependent.
pub(crate) fn unmixing(stains: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let n = |v: [f64; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / l, v[1] / l, v[2] / l]
    };
    let m = [n(stains[0]), n(stains[1]), n(stains[2])];
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-6 {
        return None;
    }
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    // inverse = adjugate / det; od (row vector) · inv = concentrations
    Some([
        [c(1, 1, 2, 2) / det, -c(0, 1, 2, 2) / det, c(0, 1, 1, 2) / det],
        [-c(1, 0, 2, 2) / det, c(0, 0, 2, 2) / det, -c(0, 0, 1, 2) / det],
        [c(1, 0, 2, 1) / det, -c(0, 0, 2, 1) / det, c(0, 0, 1, 1) / det],
    ])
}

/// The outline of the pixels of label `l` around the pixel `start` (its first pixel in row
/// order), as the corners of the pixel edges between the object and the rest, simplified to
/// within `tol` pixels. Coordinates are pixel corners (pixel `(x, y)` spans x..x+1, y..y+1).
pub(crate) fn outline(lab: &[u32], w: usize, h: usize, l: u32, start: usize, tol: f64) -> Vec<[f64; 2]> {
    let inside = |x: isize, y: isize| -> bool {
        x >= 0 && y >= 0 && x < w as isize && y < h as isize && lab[y as usize * w + x as usize] == l
    };
    // walk the boundary with the object on the right: directions 0 right, 1 down, 2 left, 3 up
    let (sx, sy) = ((start % w) as isize, (start / w) as isize);
    let (mut x, mut y, mut dir) = (sx, sy, 0u8);
    let mut pts: Vec<[f64; 2]> = vec![[sx as f64, sy as f64]];
    let limit = 4 * (w + 1) * (h + 1);
    for _ in 0..limit {
        // the corner (x, y) moving in `dir`; the pixels ahead-left and ahead-right
        let (al, ar) = match dir {
            0 => ((x, y - 1), (x, y)),
            1 => ((x, y), (x - 1, y)),
            2 => ((x - 1, y), (x - 1, y - 1)),
            _ => ((x - 1, y - 1), (x, y - 1)),
        };
        let (l_in, r_in) = (inside(al.0, al.1), inside(ar.0, ar.1));
        dir = if l_in {
            (dir + 3) % 4 // turn left
        } else if r_in {
            dir // straight on
        } else {
            (dir + 1) % 4 // turn right
        };
        match dir {
            0 => x += 1,
            1 => y += 1,
            2 => x -= 1,
            _ => y -= 1,
        }
        pts.push([x as f64, y as f64]);
        if x == sx && y == sy {
            break;
        }
    }
    simplify(&pts, tol)
}

/// Douglas–Peucker simplification of a closed outline.
fn simplify(p: &[[f64; 2]], tol: f64) -> Vec<[f64; 2]> {
    if p.len() < 4 {
        return p.to_vec();
    }
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    let last = p.len() - 1;
    // split at the point farthest from the start so that both halves are open polylines
    let far = (1..last)
        .max_by(|a, b| {
            let d = |i: usize| (p[i][0] - p[0][0]).hypot(p[i][1] - p[0][1]);
            d(*a).total_cmp(&d(*b))
        })
        .unwrap_or(last);
    keep[far] = true;
    keep[last] = true;
    let mut stack = vec![(0usize, far), (far, last)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ax, ay, bx, by) = (p[a][0], p[a][1], p[b][0], p[b][1]);
        let len = (bx - ax).hypot(by - ay);
        let mut best = (0.0, a);
        for (i, q) in p.iter().enumerate().take(b).skip(a + 1) {
            let d = if len == 0.0 {
                (q[0] - ax).hypot(q[1] - ay)
            } else {
                ((bx - ax) * (ay - q[1]) - (ax - q[0]) * (by - ay)).abs() / len
            };
            if d > best.0 {
                best = (d, i);
            }
        }
        if best.0 > tol {
            keep[best.1] = true;
            stack.push((a, best.1));
            stack.push((best.1, b));
        }
    }
    // the last point repeats the first
    p[..last]
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(q, _)| *q)
        .collect()
}

/// Area of the convex hull of a set of points (Andrew's monotone chain).
pub(crate) fn hull_area(pts: &mut [[f64; 2]]) -> f64 {
    if pts.len() < 3 {
        return 0.0;
    }
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut hull: Vec<[f64; 2]> = Vec::with_capacity(pts.len() * 2);
    for &p in pts.iter() {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower = hull.len() + 1;
    for &p in pts.iter().rev().skip(1) {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop();
    let mut a = 0.0;
    for i in 0..hull.len() {
        let (p, q) = (hull[i], hull[(i + 1) % hull.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    a.abs() / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_of_a_square() {
        let (w, h) = (9, 9);
        let fg: Vec<bool> = (0..81)
            .map(|i| (2..7).contains(&(i % 9)) && (2..7).contains(&(i / 9)))
            .collect();
        let d = distance(&fg, w, h);
        assert_eq!(d[4 * 9 + 4], 3.0);
        assert_eq!(d[2 * 9 + 2], 1.0);
        assert_eq!(d[0], 0.0);
    }

    #[test]
    fn thresholds_split_two_levels() {
        let mut v = vec![10f32; 900];
        v.extend(vec![200f32; 100]);
        let (t, sep) = otsu(&v).unwrap();
        assert!(t > 10.0 && t < 200.0, "{t}");
        assert!(sep > 0.9);
        let t = triangle(&v).unwrap();
        assert!(t > 10.0 && t < 200.0, "{t}");
    }

    #[test]
    fn outline_of_a_square_has_four_corners() {
        let (w, h) = (6, 6);
        let lab: Vec<u32> = (0..36)
            .map(|i| u32::from((1..4).contains(&(i % 6)) && (2..5).contains(&(i / 6))))
            .collect();
        let o = outline(&lab, w, h, 1, 2 * 6 + 1, 0.5);
        assert_eq!(o.len(), 4, "{o:?}");
        assert!(o.contains(&[1.0, 2.0]) && o.contains(&[4.0, 5.0]));
    }

    #[test]
    fn unmixing_recovers_hematoxylin() {
        let inv = unmixing([HEMATOXYLIN, EOSIN, DAB]).unwrap();
        let l = (HEMATOXYLIN.iter().map(|x| x * x).sum::<f64>()).sqrt();
        let od: Vec<f64> = HEMATOXYLIN.iter().map(|x| 0.8 * x / l).collect();
        let c: Vec<f64> = (0..3)
            .map(|j| (0..3).map(|k| od[k] * inv[k][j]).sum())
            .collect();
        assert!((c[0] - 0.8).abs() < 1e-9 && c[1].abs() < 1e-9 && c[2].abs() < 1e-9, "{c:?}");
    }

    #[test]
    fn hull_of_a_square() {
        let mut p = vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [1.0, 1.0]];
        assert!((hull_area(&mut p) - 4.0).abs() < 1e-9);
    }
}
