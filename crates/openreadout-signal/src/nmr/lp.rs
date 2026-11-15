//! Linear prediction of a complex interferogram.
//!
//! The prediction coefficients come from Burg's method (the maximum-entropy recursion: each
//! stage picks the reflection coefficient that minimises the summed forward and backward
//! prediction error). Every reflection coefficient has modulus ≤ 1, so the prediction filter is
//! stable and extended data do not grow, also when the number of coefficients approaches the
//! number of points (where a least-squares fit is underdetermined).
//!
//! - `Forward`: points are appended until the series has `points` points;
//! - `Backward`: the first `points` points are replaced by a prediction from the later ones
//!   (forward prediction of the time-reversed, conjugated series);
//! - `MirrorForward`: before the forward prediction, the series is extended to negative time
//!   by `x(−k) = conj(x(k))` (for data whose first point has zero phase, such as most indirect
//!   dimensions with a zero-order phase of 0), which doubles the data the coefficients are fitted
//!   to.
//!
//! Bruker `ME_mod`: 1/2 forward (real/complex), 3/4 backward, 5/6 mirror image forward; the
//! real variants are done on the complex data.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::fft::Complex;

/// Which way the data are predicted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LpDirection {
    /// Extend the end of the series.
    Forward,
    /// Replace the first points.
    Backward,
    /// Extend the end, fitting the coefficients to the series mirrored to negative time.
    MirrorForward,
}

/// A linear-prediction step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LinearPrediction {
    /// Direction (module docs).
    pub direction: LpDirection,
    /// Number of prediction coefficients.
    pub coefficients: usize,
    /// `Forward`/`MirrorForward`: length of the extended series; `Backward`: number of leading
    /// points replaced. Counted in raw rows for indirect dimensions (two rows per complex point
    /// in hypercomplex data) and in complex points for the direct dimension.
    pub points: usize,
}

/// Burg prediction-error filter `a` (`a[0] = 1`): `x[n] ≈ −Σ_{k≥1} a[k]·x[n−k]`.
pub fn burg_coefficients(x: &[Complex], order: usize) -> Vec<Complex> {
    let n = x.len();
    let order = order.min(n.saturating_sub(1));
    let mut a = vec![Complex::new(1.0, 0.0)];
    if order == 0 {
        return a;
    }
    let mut f: Vec<Complex> = x.to_vec();
    let mut b: Vec<Complex> = x.to_vec();
    for m in 0..order {
        // f[m+1..], b[m..n-1]
        let (mut num, mut den) = (Complex::default(), 0.0);
        for j in m + 1..n {
            let (fj, bj) = (f[j], b[j - 1]);
            num += bj.conj() * fj;
            den += fj.re * fj.re + fj.im * fj.im + bj.re * bj.re + bj.im * bj.im;
        }
        if !(den > 0.0 && den.is_finite()) {
            break;
        }
        let k = num.scale(-2.0 / den);
        let mut next = a.clone();
        next.push(Complex::default());
        let len = next.len();
        for i in 0..len {
            let rev = if len - 1 - i < a.len() {
                a[len - 1 - i].conj()
            } else {
                Complex::default()
            };
            next[i] += k * rev;
        }
        a = next;
        for j in (m + 1..n).rev() {
            let (fj, bj) = (f[j], b[j - 1]);
            f[j] = fj + k * bj;
            b[j] = bj + k.conj() * fj;
        }
    }
    a
}

fn extend(series: &mut Vec<Complex>, a: &[Complex], target: usize) {
    let p = a.len() - 1;
    while series.len() < target {
        let n = series.len();
        let mut v = Complex::default();
        for k in 1..=p.min(n) {
            v += a[k] * series[n - k];
        }
        let v = -v;
        series.push(if v.re.is_finite() && v.im.is_finite() {
            v
        } else {
            Complex::default()
        });
    }
}

/// Apply `lp` to `x` (with `points` already converted to complex points of this series).
pub fn predict(x: &[Complex], lp: &LinearPrediction, points: usize) -> Vec<Complex> {
    if x.is_empty() {
        return Vec::new();
    }
    match lp.direction {
        LpDirection::Forward => {
            let a = burg_coefficients(x, lp.coefficients);
            let mut s = x.to_vec();
            extend(&mut s, &a, points.max(x.len()));
            s
        }
        LpDirection::MirrorForward => {
            if x.len() < 2 {
                return x.to_vec();
            }
            let mut full: Vec<Complex> = x[1..].iter().rev().map(|v| v.conj()).collect();
            full.extend_from_slice(x);
            let a = burg_coefficients(&full, lp.coefficients);
            let start = x.len() - 1;
            extend(&mut full, &a, start + points.max(x.len()));
            full.split_off(start)
        }
        LpDirection::Backward => {
            let k = points.min(x.len().saturating_sub(1));
            if k == 0 {
                return x.to_vec();
            }
            let mut rev: Vec<Complex> = x[k..].iter().rev().map(|v| v.conj()).collect();
            let a = burg_coefficients(&rev, lp.coefficients);
            extend(&mut rev, &a, x.len());
            rev.iter().rev().map(|v| v.conj()).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decay(n: usize, start: usize) -> Vec<Complex> {
        (start..start + n)
            .map(|i| {
                let t = i as f64;
                Complex::from_phase(0.31 * t).scale((-t / 400.0).exp())
                    + Complex::from_phase(-1.1 * t).scale(0.5 * (-t / 250.0).exp())
            })
            .collect()
    }

    #[test]
    fn forward_prediction_continues_a_sum_of_decays() {
        let x = decay(64, 0);
        let want = decay(128, 0);
        let lp = LinearPrediction {
            direction: LpDirection::Forward,
            coefficients: 8,
            points: 128,
        };
        let y = predict(&x, &lp, 128);
        assert_eq!(y.len(), 128);
        // Burg's reflection coefficients are biased towards weaker damping, so the test signal
        // decays slowly; the prediction stays within 1 % of the peak
        for (a, b) in y.iter().zip(&want) {
            assert!((*a - *b).abs() < 1.5e-2, "{a:?} vs {b:?}");
        }
        // mirror image prediction of a zero-phase series
        let m = LinearPrediction {
            direction: LpDirection::MirrorForward,
            ..lp
        };
        let ym = predict(&x, &m, 128);
        assert_eq!(ym.len(), 128);
        assert!(ym.iter().all(|v| v.re.is_finite()));
        // backward: the first 4 points are rebuilt from the rest
        let mut bad = x.clone();
        for v in bad.iter_mut().take(4) {
            *v = Complex::new(9.0, 9.0);
        }
        let b = LinearPrediction {
            direction: LpDirection::Backward,
            coefficients: 8,
            points: 4,
        };
        let yb = predict(&bad, &b, 4);
        for (a, b) in yb.iter().zip(&x).take(4) {
            assert!((*a - *b).abs() < 1e-3, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn many_coefficients_stay_bounded() {
        let x: Vec<Complex> = decay(40, 0)
            .iter()
            .enumerate()
            .map(|(i, v)| *v + Complex::new(0.01 * ((i * 7919) % 13) as f64, 0.0))
            .collect();
        let lp = LinearPrediction {
            direction: LpDirection::Forward,
            coefficients: 38,
            points: 400,
        };
        let y = predict(&x, &lp, 400);
        let peak = x.iter().map(|v| v.abs()).fold(0.0, f64::max);
        assert!(y.iter().all(|v| v.abs() <= 10.0 * peak));
        assert_eq!(predict(&[], &lp, 10).len(), 0);
    }
}
