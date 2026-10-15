//! The object pipeline before 2026-10-15, kept only to measure the new one against it.
#![allow(clippy::all, dead_code, missing_docs)]

/// Separable Gaussian blur with edge clamping.
fn blur(v: &[f64], w: usize, h: usize, sigma: f64) -> Vec<f64> {
    if sigma <= 0.0 {
        return v.to_vec();
    }
    let rad = (3.0 * sigma).ceil() as isize;
    let k: Vec<f64> = (-rad..=rad)
        .map(|i| (-(i as f64).powi(2) / (2.0 * sigma * sigma)).exp())
        .collect();
    let ks: f64 = k.iter().sum();
    let k: Vec<f64> = k.iter().map(|x| x / ks).collect();
    let clamp = |i: isize, n: usize| i.clamp(0, n as isize - 1) as usize;
    let mut tmp = vec![0.0; v.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() {
                s += kv * v[y * w + clamp(x as isize + j as isize - rad, w)];
            }
            tmp[y * w + x] = s;
        }
    }
    let mut out = vec![0.0; v.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for (j, kv) in k.iter().enumerate() {
                s += kv * tmp[clamp(y as isize + j as isize - rad, h) * w + x];
            }
            out[y * w + x] = s;
        }
    }
    out
}

/// Otsu's threshold over a 256-bin histogram of the finite values.
fn otsu(v: &[f64]) -> Option<f64> {
    let (lo, hi) = v
        .iter()
        .filter(|x| x.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| {
            (a.min(*x), b.max(*x))
        });
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return None;
    }
    let bins = 256usize;
    let mut hist = vec![0u64; bins];
    let scale = (bins - 1) as f64 / (hi - lo);
    for x in v.iter().filter(|x| x.is_finite()) {
        hist[((x - lo) * scale) as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    let sum_all: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, c)| i as f64 * *c as f64)
        .sum();
    let (mut w0, mut sum0) = (0u64, 0.0);
    let (mut best, mut best_i) = (-1.0, 0usize);
    for (i, c) in hist.iter().enumerate() {
        w0 += c;
        if w0 == 0 {
            continue;
        }
        let w1 = total - w0;
        if w1 == 0 {
            break;
        }
        sum0 += i as f64 * *c as f64;
        let m0 = sum0 / w0 as f64;
        let m1 = (sum_all - sum0) / w1 as f64;
        let between = w0 as f64 * w1 as f64 * (m0 - m1).powi(2);
        if between > best {
            best = between;
            best_i = i;
        }
    }
    Some(lo + (best_i as f64 + 0.5) / scale)
}

/// Background pixels that the border cannot reach (4-connected) become foreground: holes
/// inside a nucleus or cell are part of it.
fn fill_holes(mut fg: Vec<bool>, w: usize, h: usize) -> Vec<bool> {
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        stack.push(x);
        stack.push((h - 1) * w + x);
    }
    for y in 0..h {
        stack.push(y * w);
        stack.push(y * w + w - 1);
    }
    while let Some(i) = stack.pop() {
        if fg[i] || outside[i] {
            continue;
        }
        outside[i] = true;
        let (x, y) = (i % w, i / w);
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < w {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - w);
        }
        if y + 1 < h {
            stack.push(i + w);
        }
    }
    for (f, o) in fg.iter_mut().zip(&outside) {
        *f = !*o;
    }
    fg
}

/// City-block-and-diagonal (3-4 chamfer) distance of each foreground pixel to the background,
/// in thirds of a pixel.
fn chamfer(fg: &[bool], w: usize, h: usize) -> Vec<u16> {
    let big = u16::MAX / 2;
    let mut d: Vec<u16> = fg.iter().map(|f| if *f { big } else { 0 }).collect();
    let at = |d: &[u16], x: isize, y: isize| -> u16 {
        if x < 0 || y < 0 || x >= w as isize || y >= h as isize {
            0
        } else {
            d[y as usize * w + x as usize]
        }
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let i = y as usize * w + x as usize;
            if d[i] == 0 {
                continue;
            }
            let m = (at(&d, x - 1, y) + 3)
                .min(at(&d, x, y - 1) + 3)
                .min(at(&d, x - 1, y - 1) + 4)
                .min(at(&d, x + 1, y - 1) + 4);
            d[i] = d[i].min(m);
        }
    }
    for y in (0..h as isize).rev() {
        for x in (0..w as isize).rev() {
            let i = y as usize * w + x as usize;
            if d[i] == 0 {
                continue;
            }
            let m = (at(&d, x + 1, y) + 3)
                .min(at(&d, x, y + 1) + 3)
                .min(at(&d, x + 1, y + 1) + 4)
                .min(at(&d, x - 1, y + 1) + 4);
            d[i] = d[i].min(m);
        }
    }
    d
}

fn find(parent: &mut [u32], mut i: u32) -> u32 {
    while parent[i as usize] != i {
        let p = parent[parent[i as usize] as usize];
        parent[i as usize] = p;
        i = p;
    }
    i
}

/// Label 8-connected foreground objects (1, 2, …; 0 = background) and how many labels there
/// are. With `split`, touching objects are cut apart at narrow necks: pixels are flooded from
/// the inside out in order of their distance to the background, and two growing regions that
/// meet stay apart when the neck is at most three quarters as wide as the smaller one is deep
/// (a distance-transform watershed with that persistence rule).
fn label_objects(fg: &[bool], w: usize, h: usize, split: bool) -> (Vec<u32>, u32) {
    const NONE: u32 = u32::MAX;
    let d = if split {
        chamfer(fg, w, h)
    } else {
        fg.iter().map(|f| u16::from(*f)).collect()
    };
    // pixels by decreasing distance (counting sort)
    let top = d.iter().copied().max().unwrap_or(0) as usize;
    let mut count = vec![0usize; top + 2];
    for &v in &d {
        if v > 0 {
            count[v as usize] += 1;
        }
    }
    let mut start = vec![0usize; top + 2];
    for v in (1..=top).rev() {
        start[v - 1] = start[v] + count[v];
    }
    let total = start[0];
    let mut order = vec![0u32; total];
    let mut fill = start.clone();
    for (i, &v) in d.iter().enumerate() {
        if v > 0 {
            order[fill[v as usize]] = i as u32;
            fill[v as usize] += 1;
        }
    }
    let mut parent = vec![NONE; w * h];
    // depth (distance at the seed) of each region root
    let mut peak = vec![0u16; w * h];
    let significant = |peak: u16, saddle: u16| -> bool {
        // the neck (saddle) at most 3/4 of the depth, and at least 1.5 px shallower
        u32::from(saddle) * 4 <= u32::from(peak) * 3 && peak.saturating_sub(saddle) >= 4
    };
    let mut roots: Vec<u32> = Vec::with_capacity(8);
    for &pi in &order {
        let i = pi as usize;
        let (x, y) = ((i % w) as isize, (i / w) as isize);
        roots.clear();
        for dy in -1isize..=1 {
            for dx in -1isize..=1 {
                let (nx, ny) = (x + dx, y + dy);
                if (dx == 0 && dy == 0) || nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize
                {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if parent[j] != NONE {
                    let r = find(&mut parent, j as u32);
                    if !roots.contains(&r) {
                        roots.push(r);
                    }
                }
            }
        }
        if roots.is_empty() {
            parent[i] = pi;
            peak[i] = d[i];
            continue;
        }
        roots.sort_by_key(|r| std::cmp::Reverse(peak[*r as usize]));
        let main = roots[0];
        parent[i] = main;
        if !split {
            for &r in &roots[1..] {
                parent[r as usize] = main;
            }
            continue;
        }
        // merge regions that are not significant at this saddle into the deepest one
        let saddle = d[i];
        for &r in &roots[1..] {
            if !significant(peak[r as usize], saddle) {
                parent[r as usize] = main;
            }
        }
    }
    let mut label = vec![0u32; w * h];
    let mut ids = std::collections::HashMap::new();
    for i in 0..w * h {
        if parent[i] == NONE {
            continue;
        }
        let r = find(&mut parent, i as u32);
        let next = ids.len() as u32 + 1;
        label[i] = *ids.entry(r).or_insert(next);
    }
    (label, ids.len() as u32)
}


/// Baseline (temporary): the label image of the old object pipeline on one component.
#[doc(hidden)]
pub fn baseline_labels(raw: &[f64], w: usize, h: usize, dark: bool) -> Vec<u32> {
    let smooth = blur(raw, w, h, 1.0);
    let Some(t) = otsu(&smooth) else {
        return vec![0; w * h];
    };
    let fg: Vec<bool> = smooth
        .iter()
        .map(|v| if dark { *v < t } else { *v > t })
        .collect();
    let fg = fill_holes(fg, w, h);
    let (label, n) = label_objects(&fg, w, h, true);
    let mut cnt = vec![0u64; n as usize + 1];
    for &l in &label {
        cnt[l as usize] += 1;
    }
    label
        .into_iter()
        .map(|l| if l > 0 && cnt[l as usize] < 10 { 0 } else { l })
        .collect()
}
