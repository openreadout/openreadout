//! Fragment sizing against a size standard: find the standard's peaks in its dye channel, match
//! them to the ladder's fragment lengths, and size every scan between the first and the last
//! matched peak with the Local Southern method (Elder and Southern 1983).
//!
//! Matching is a dynamic programme over the candidate peaks (sorted by scan) and the ladder's
//! lengths (ascending). A match is a pair (length, peak); consecutive matches give a slope
//! (scans per nucleotide), and mobility changes slowly along a run, so a good assignment keeps
//! the slope steady from one interval to the next. The score adds, per match, a reward; per
//! change of slope, a cost on the log ratio; per ladder length skipped inside the run or at its
//! ends, a cost; per tall unmatched peak between two matches, a cost; per peak whose height is
//! far from the ladder's typical height, a cost. The assignment with the lowest total wins.

use crate::ladders::Ladder;

/// A peak of a channel: scan (sub-scan apex), height above the baseline, and its sample index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    /// Apex position, scans (parabolic interpolation of the three top samples).
    pub scan: f64,
    /// Height above the local baseline.
    pub height: f64,
}

/// A ladder fragment matched to a peak.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LadderMatch {
    /// The fragment's length, nucleotides.
    pub size: f64,
    /// The peak's apex, scans.
    pub scan: f64,
    /// The peak's height.
    pub height: f64,
    /// Size the Local Southern fit through the other matches gives this peak, minus `size`
    /// (`None` at the ends, where three other matches on its sides are missing).
    pub residual: Option<f64>,
}

/// The result of sizing one run.
#[derive(Debug, Clone)]
pub struct Sizing {
    /// The size standard.
    pub ladder: &'static Ladder,
    /// Matched fragments, ascending.
    pub matches: Vec<LadderMatch>,
    /// Ladder lengths with no peak.
    pub missing: Vec<f64>,
    /// Mean absolute leave-one-out residual of the interior matches, nucleotides.
    pub mean_abs_residual: Option<f64>,
    /// Largest absolute leave-one-out residual, nucleotides.
    pub max_abs_residual: Option<f64>,
    /// Quality score in 0–1: the share of ladder fragments matched, reduced by the residuals.
    pub quality: f64,
}

impl Sizing {
    /// Size of a position (scans) by Local Southern; `None` outside the matched range.
    pub fn size_at(&self, scan: f64) -> Option<f64> {
        local_southern(&self.matches, scan)
    }

    /// First and last matched scan.
    pub fn range(&self) -> Option<(f64, f64)> {
        Some((self.matches.first()?.scan, self.matches.last()?.scan))
    }
}

/// Points per baseline block.
const BLOCK: usize = 64;

/// The baseline of a raw trace: the 20th percentile of each block of samples, interpolated.
pub fn baseline(y: &[f64]) -> Vec<f64> {
    let n = y.len();
    if n == 0 {
        return Vec::new();
    }
    let mut knots: Vec<(f64, f64)> = Vec::new();
    for (b, chunk) in y.chunks(BLOCK).enumerate() {
        let mut v: Vec<f64> = chunk.iter().copied().filter(|x| x.is_finite()).collect();
        if v.is_empty() {
            continue;
        }
        v.sort_by(f64::total_cmp);
        let q = v[(v.len() - 1) / 5];
        knots.push(((b * BLOCK) as f64 + (chunk.len() as f64 - 1.0) / 2.0, q));
    }
    // a block that sits on a peak lifts its percentile: take the lower of each knot and the
    // mean of its neighbours
    let k2: Vec<(f64, f64)> = (0..knots.len())
        .map(|i| {
            let lo = knots[i.saturating_sub(1)].1;
            let hi = knots[(i + 1).min(knots.len() - 1)].1;
            (knots[i].0, knots[i].1.min((lo + hi) / 2.0))
        })
        .collect();
    let mut out = Vec::with_capacity(n);
    let mut k = 0usize;
    for i in 0..n {
        let x = i as f64;
        while k + 1 < k2.len() && k2[k + 1].0 < x {
            k += 1;
        }
        let v = match (k2.get(k), k2.get(k + 1)) {
            (Some(a), Some(b)) if x >= a.0 => a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0),
            (Some(a), _) => a.1,
            _ => 0.0,
        };
        out.push(v);
    }
    out
}

/// Robust noise of a trace: the median absolute first difference, scaled to a standard
/// deviation.
pub fn noise(y: &[f64]) -> f64 {
    let mut d: Vec<f64> = y
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .filter(|v| v.is_finite())
        .collect();
    if d.is_empty() {
        return 1.0;
    }
    let mid = d.len() / 2;
    let (_, m, _) = d.select_nth_unstable_by(mid, f64::total_cmp);
    (*m * 1.4826 / std::f64::consts::SQRT_2).max(1.0)
}

/// Peaks of a trace: local maxima of the baseline-corrected signal at least `min_height` high,
/// no two within `half_width` samples.
pub fn find_peaks(y: &[f64], min_height: f64, half_width: usize) -> Vec<Peak> {
    let base = baseline(y);
    let z: Vec<f64> = y.iter().zip(&base).map(|(a, b)| a - b).collect();
    let n = z.len();
    let mut out: Vec<Peak> = Vec::new();
    let w = half_width.max(1);
    let mut i = 1;
    while i + 1 < n {
        let v = z[i];
        if v >= min_height && v > z[i - 1] && v >= z[i + 1] {
            let lo = i.saturating_sub(w);
            let hi = (i + w).min(n - 1);
            if z[lo..=hi].iter().all(|&u| u <= v) {
                let (a, c) = (z[i - 1], z[i + 1]);
                let den = a - 2.0 * v + c;
                let shift = if den < 0.0 {
                    (0.5 * (a - c) / den).clamp(-0.5, 0.5)
                } else {
                    0.0
                };
                out.push(Peak {
                    scan: i as f64 + shift,
                    height: v,
                });
                i += w;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Weights of the matching score.
const MATCH_REWARD: f64 = 3.0;
const SLOPE_WEIGHT: f64 = 16.0;
const SKIP_INSIDE: f64 = 6.0;
const SKIP_END: f64 = 2.0;
const TALL_BETWEEN: f64 = 3.0;
const HEIGHT_WEIGHT: f64 = 0.5;
/// Ladder lengths a match may jump over (1 = none skipped).
const MAX_STEP: usize = 2;

/// Match ladder `sizes` to `peaks` (ascending scans). Returns (ladder index, peak index) pairs.
pub fn match_ladder(sizes: &[f64], peaks: &[Peak]) -> Vec<(usize, usize)> {
    let n = sizes.len();
    if n < 3 || peaks.len() < 3 {
        return Vec::new();
    }
    // the candidates: the tallest few, back in scan order
    let keep = (n + n / 2 + 10).min(peaks.len());
    let mut by_height: Vec<usize> = (0..peaks.len()).collect();
    by_height.sort_by(|&a, &b| peaks[b].height.total_cmp(&peaks[a].height));
    let mut idx: Vec<usize> = by_height.iter().take(keep).copied().collect();
    let mut tops: Vec<f64> = by_height
        .iter()
        .take(n.min(peaks.len()))
        .map(|&i| peaks[i].height)
        .collect();
    tops.sort_by(f64::total_cmp);
    let h_ref = tops[tops.len() / 2].max(1e-9);
    // peaks far taller than the ladder's typical peak are spikes or saturation, not fragments
    idx.retain(|&i| peaks[i].height <= 8.0 * h_ref);
    idx.sort_by(|&a, &b| peaks[a].scan.total_cmp(&peaks[b].scan));
    let c: Vec<Peak> = idx.iter().map(|&i| peaks[i]).collect();
    let m = c.len();
    if m < 3 {
        return Vec::new();
    }
    let hcost: Vec<f64> = c
        .iter()
        .map(|p| HEIGHT_WEIGHT * (p.height / h_ref).ln().powi(2))
        .collect();
    // tall[j]: candidates up to j (exclusive) taller than 40 % of the typical ladder peak
    let mut tall = vec![0u32; m + 1];
    for j in 0..m {
        tall[j + 1] = tall[j] + u32::from(c[j].height > 0.4 * h_ref);
    }
    let between = |a: usize, b: usize| f64::from(tall[b] - tall[a + 1]) * TALL_BETWEEN;
    // state (i, d, jp, j): ladder i matched to candidate j, the previous match being ladder i-d
    // at candidate jp
    let states = n * MAX_STEP * m * m;
    if states > 40_000_000 {
        return Vec::new();
    }
    let at = |i: usize, d: usize, jp: usize, j: usize| ((i * MAX_STEP + (d - 1)) * m + jp) * m + j;
    let mut cost = vec![f64::INFINITY; states];
    let mut back = vec![u32::MAX; states];
    // starts: the first two matches
    for i in 0..n {
        for d in 1..=MAX_STEP {
            let Some(ip) = i.checked_sub(d) else { continue };
            for jp in 0..m {
                for j in jp + 1..m {
                    let v = SKIP_END * ip as f64 + SKIP_INSIDE * (d - 1) as f64
                        - 2.0 * MATCH_REWARD
                        + hcost[jp]
                        + hcost[j]
                        + between(jp, j);
                    cost[at(i, d, jp, j)] = v;
                }
            }
        }
    }
    // extend in ladder order
    for i in 0..n {
        for d in 1..=MAX_STEP {
            let Some(ip) = i.checked_sub(d) else { continue };
            for jp in 0..m {
                for j in jp + 1..m {
                    let s0 = at(i, d, jp, j);
                    let base = cost[s0];
                    if !base.is_finite() {
                        continue;
                    }
                    let slope = (c[j].scan - c[jp].scan) / (sizes[i] - sizes[ip]);
                    if slope <= 0.0 {
                        continue;
                    }
                    for d2 in 1..=MAX_STEP {
                        let i2 = i + d2;
                        if i2 >= n {
                            break;
                        }
                        let dl = sizes[i2] - sizes[i];
                        let (lo, hi) = (c[j].scan + slope * dl * 0.4, c[j].scan + slope * dl * 2.5);
                        let first = c.partition_point(|p| p.scan < lo).max(j + 1);
                        for j2 in first..m {
                            if c[j2].scan > hi {
                                break;
                            }
                            let s2 = (c[j2].scan - c[j].scan) / dl;
                            let v = base
                                + SLOPE_WEIGHT * (s2 / slope).ln().powi(2)
                                + SKIP_INSIDE * (d2 - 1) as f64
                                - MATCH_REWARD
                                + hcost[j2]
                                + between(j, j2);
                            let k = at(i2, d2, j, j2);
                            if v < cost[k] {
                                cost[k] = v;
                                back[k] = u32::try_from(s0).unwrap_or(u32::MAX);
                            }
                        }
                    }
                }
            }
        }
    }
    // the best end
    let mut best = (f64::INFINITY, usize::MAX);
    for i in 0..n {
        for d in 1..=MAX_STEP {
            if i < d {
                continue;
            }
            for jp in 0..m {
                for j in jp + 1..m {
                    let k = at(i, d, jp, j);
                    let v = cost[k] + SKIP_END * (n - 1 - i) as f64;
                    if v < best.0 {
                        best = (v, k);
                    }
                }
            }
        }
    }
    if best.1 == usize::MAX {
        return Vec::new();
    }
    // trace back
    let decode = |k: usize| {
        let j = k % m;
        let r = k / m;
        let jp = r % m;
        let r = r / m;
        let d = r % MAX_STEP + 1;
        let i = r / MAX_STEP;
        (i, d, jp, j)
    };
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut k = best.1;
    loop {
        let (i, d, jp, j) = decode(k);
        out.push((i, idx[j]));
        let prev = back[k];
        if prev == u32::MAX {
            out.push((i - d, idx[jp]));
            break;
        }
        k = prev as usize;
    }
    out.reverse();
    out
}

/// Size `scan` from the matches by Local Southern: the average of the fit through the two
/// matches below and one above and the fit through one below and two above; at the ends the
/// one fit available; between only two matches, linear interpolation.
pub fn local_southern(m: &[LadderMatch], scan: f64) -> Option<f64> {
    let n = m.len();
    if n < 2 || !scan.is_finite() {
        return None;
    }
    let (first, last) = (m[0].scan, m[n - 1].scan);
    if scan < first || scan > last {
        return None;
    }
    // k: the match at or below scan, with k + 1 above
    let k = m
        .partition_point(|p| p.scan <= scan)
        .saturating_sub(1)
        .min(n - 2);
    let fit = |a: usize| -> Option<f64> {
        let pts = [m.get(a)?, m.get(a + 1)?, m.get(a + 2)?];
        southern3(
            [pts[0].scan, pts[1].scan, pts[2].scan],
            [pts[0].size, pts[1].size, pts[2].size],
            scan,
        )
    };
    let lower = k.checked_sub(1).and_then(fit);
    let upper = fit(k);
    match (lower, upper) {
        (Some(a), Some(b)) => Some((a + b) / 2.0),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => {
            let (a, b) = (m[k], m[k + 1]);
            let t = (scan - a.scan) / (b.scan - a.scan);
            t.is_finite().then(|| a.size + t * (b.size - a.size))
        }
    }
}

/// The size at `x` of the curve `L = c / (m − m0) + L0` through three points (scans `m`, sizes
/// `l`); linear interpolation when the three are collinear or the fit is singular.
fn southern3(m: [f64; 3], l: [f64; 3], x: f64) -> Option<f64> {
    // L·m = m0·L + L0·m + K, with c = K + L0·m0: linear in (m0, L0, K)
    let a = [[l[0], m[0], 1.0], [l[1], m[1], 1.0], [l[2], m[2], 1.0]];
    let r = [l[0] * m[0], l[1] * m[1], l[2] * m[2]];
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let d = det(a);
    let linear = || {
        let (i, j) = if x <= m[1] { (0, 1) } else { (1, 2) };
        let t = (x - m[i]) / (m[j] - m[i]);
        t.is_finite().then(|| l[i] + t * (l[j] - l[i]))
    };
    if d.abs() < 1e-12 * (m[2] - m[0]).abs().max(1.0) * (l[2] - l[0]).abs().max(1.0) {
        return linear();
    }
    let col = |k: usize| {
        let mut b = a;
        for (row, &v) in b.iter_mut().zip(&r) {
            row[k] = v;
        }
        det(b) / d
    };
    let (m0, l0, kk) = (col(0), col(1), col(2));
    let c = kk + l0 * m0;
    // the pole must lie outside the three points, else the curve is not monotonic between them
    let (lo, hi) = (m[0].min(m[2]), m[0].max(m[2]));
    if (lo..=hi).contains(&m0) || !c.is_finite() {
        return linear();
    }
    let v = c / (x - m0) + l0;
    if v.is_finite() { Some(v) } else { linear() }
}

/// Size the run: match `ladder` to the peaks of the standard's channel `y`.
pub fn size_with(ladder: &'static Ladder, peaks: &[Peak]) -> Option<Sizing> {
    let pairs = match_ladder(ladder.sizes, peaks);
    if pairs.len() < 4 {
        return None;
    }
    let mut matches: Vec<LadderMatch> = pairs
        .iter()
        .map(|&(i, j)| LadderMatch {
            size: ladder.sizes[i],
            scan: peaks[j].scan,
            height: peaks[j].height,
            residual: None,
        })
        .collect();
    // leave-one-out residuals of interior matches
    let mut res = Vec::new();
    for k in 1..matches.len().saturating_sub(1) {
        let mut others = matches.clone();
        let me = others.remove(k);
        if let Some(s) = local_southern(&others, me.scan) {
            let r = s - me.size;
            matches[k].residual = Some(r);
            res.push(r.abs());
        }
    }
    let mean = (!res.is_empty()).then(|| res.iter().sum::<f64>() / res.len() as f64);
    let max = res.iter().copied().reduce(f64::max);
    let matched: Vec<f64> = matches.iter().map(|m| m.size).collect();
    let missing: Vec<f64> = ladder
        .sizes
        .iter()
        .copied()
        .filter(|s| !matched.contains(s))
        .collect();
    let coverage = matches.len() as f64 / ladder.sizes.len() as f64;
    let quality = coverage * (-mean.unwrap_or(0.0) / 4.0).exp();
    Some(Sizing {
        ladder,
        matches,
        missing,
        mean_abs_residual: mean,
        max_abs_residual: max,
        quality,
    })
}

/// Size against the best of `ladders`: the highest quality, then the most matches.
pub fn best_of(ladders: &[&'static Ladder], peaks: &[Peak]) -> Option<Sizing> {
    ladders
        .iter()
        .filter_map(|l| size_with(l, peaks))
        .max_by(|a, b| {
            let qa = (a.quality * 100.0).round();
            let qb = (b.quality * 100.0).round();
            qa.total_cmp(&qb)
                .then(a.matches.len().cmp(&b.matches.len()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ladders::LADDERS;

    /// A synthetic run: Gaussian peaks at scans from a Southern-like curve, plus clutter.
    fn run(ladder: &Ladder, clutter: &[(f64, f64)]) -> (Vec<f64>, Vec<f64>) {
        let scan_of = |s: f64| 1000.0 + 9.0 * s - 2000.0 / (s + 40.0);
        let n = (scan_of(ladder.sizes[ladder.sizes.len() - 1]) + 400.0) as usize;
        let mut y = vec![20.0; n];
        let mut put = |at: f64, h: f64| {
            for (i, v) in y.iter_mut().enumerate() {
                let d = (i as f64 - at) / 3.0;
                *v += h * (-0.5 * d * d).exp();
            }
        };
        let truth: Vec<f64> = ladder.sizes.iter().map(|&s| scan_of(s)).collect();
        for (k, &t) in truth.iter().enumerate() {
            put(t, 300.0 + 20.0 * (k % 3) as f64);
        }
        for &(at, h) in clutter {
            put(at, h);
        }
        (y, truth)
    }

    #[test]
    fn matches_a_clean_ladder() {
        let l = &LADDERS[0];
        let (y, truth) = run(l, &[]);
        let peaks = find_peaks(&y, 50.0, 3);
        let s = size_with(l, &peaks).unwrap();
        assert_eq!(s.matches.len(), l.sizes.len());
        for (m, t) in s.matches.iter().zip(&truth) {
            assert!((m.scan - t).abs() < 0.6, "{} vs {t}", m.scan);
        }
        assert!(
            s.mean_abs_residual.unwrap() < 0.5,
            "{:?}",
            s.mean_abs_residual
        );
        // a scan between two fragments sizes between them
        let mid = (truth[7] + truth[8]) / 2.0;
        let v = s.size_at(mid).unwrap();
        assert!(v > 200.0 && v < 250.0, "{v}");
        assert!(s.size_at(truth[0] - 10.0).is_none());
    }

    #[test]
    fn ignores_spikes_primer_and_a_missing_fragment() {
        let l = &LADDERS[4]; // GS500ROX
        let (mut y, truth) = run(l, &[(700.0, 9000.0), (1500.0, 5000.0), (1180.0, 90.0)]);
        // remove the 250 fragment
        let at = truth[8] as usize;
        for v in &mut y[at - 12..at + 12] {
            *v = 20.0;
        }
        let peaks = find_peaks(&y, 50.0, 3);
        let s = size_with(l, &peaks).unwrap();
        assert_eq!(s.missing, vec![250.0]);
        assert!(
            s.matches
                .iter()
                .all(|m| truth.iter().any(|t| (m.scan - t).abs() < 0.6))
        );
    }

    #[test]
    fn picks_the_ladder_that_fits() {
        let gs500 = &LADDERS[4];
        let (y, _) = run(gs500, &[]);
        let peaks = find_peaks(&y, 50.0, 3);
        let rox: Vec<&'static Ladder> = crate::ladders::for_dye("ROX");
        assert_eq!(best_of(&rox, &peaks).unwrap().ladder.name, "GS500ROX");
        let gs600 = &LADDERS[1];
        let (y, _) = run(gs600, &[]);
        let peaks = find_peaks(&y, 50.0, 3);
        let liz = crate::ladders::for_dye("LIZ");
        assert_eq!(best_of(&liz, &peaks).unwrap().ladder.name, "GS600LIZ");
    }

    #[test]
    fn southern_through_three_points_is_exact_on_its_curve() {
        let f = |m: f64| 3000.0 / (m - 100.0) * -1.0 + 600.0;
        let ms = [400.0, 900.0, 1600.0];
        let v = southern3(ms, ms.map(f), 1200.0).unwrap();
        assert!((v - f(1200.0)).abs() < 1e-6);
        // collinear points interpolate linearly
        let v = southern3([0.0, 1.0, 2.0], [10.0, 20.0, 30.0], 1.5).unwrap();
        assert!((v - 25.0).abs() < 1e-9);
    }

    #[test]
    fn degenerate_inputs_give_nothing() {
        assert!(match_ladder(&[1.0, 2.0], &[]).is_empty());
        assert!(find_peaks(&[], 1.0, 3).is_empty());
        assert!(baseline(&[]).is_empty());
        assert!(local_southern(&[], 1.0).is_none());
    }
}
