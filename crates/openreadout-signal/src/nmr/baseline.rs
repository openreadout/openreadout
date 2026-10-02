//! Baseline correction of a real spectrum.
//!
//! `Polynomial { order }`: the spectrum is cut into 512 equal blocks (at least 8 points each).
//! A block counts as baseline when its range (max − min) is below 8 × the noise standard
//! deviation (robust estimate, [`crate::nmr::peaks::noise_sd`]), the block-flatness idea of
//! Dietrich, Rüdel and Neumann (J. Magn. Reson. 91, 1–11, 1991). A least-squares polynomial
//! of `order` (Legendre basis on `x ∈ [−1, 1]`) is fitted to the baseline blocks' medians, blocks
//! further than 3 noise SDs from the fit are dropped and the fit repeated (up to 10 times), and
//! the polynomial is subtracted from every point. With fewer than `order + 2` baseline blocks
//! the spectrum is left unchanged and no step is recorded.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::nmr::peaks::noise_sd;

/// Baseline correction choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BaselineMode {
    /// No correction.
    None,
    /// Polynomial through automatically recognised baseline blocks (module docs).
    Polynomial {
        /// Polynomial order (0 = constant offset); at most 8.
        order: u32,
    },
    /// The default: polynomial of order 1 (offset and tilt).
    #[default]
    Default,
}

impl BaselineMode {
    /// The polynomial order this mode fits, if any.
    pub fn order(self) -> Option<u32> {
        match self {
            BaselineMode::None => None,
            BaselineMode::Polynomial { order } => Some(order.min(8)),
            BaselineMode::Default => Some(1),
        }
    }
}

const BLOCKS: usize = 512;

fn legendre(x: f64, order: usize, out: &mut Vec<f64>) {
    out.clear();
    out.push(1.0);
    if order >= 1 {
        out.push(x);
    }
    for n in 1..order {
        let nf = n as f64;
        let next = ((2.0 * nf + 1.0) * x * out[n] - nf * out[n - 1]) / (nf + 1.0);
        out.push(next);
    }
}

/// Solve the symmetric system `a · c = b` by Gaussian elimination with partial pivoting.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let (top, rest) = a.split_at_mut(col + 1);
        let pivot = &top[col];
        for (off, r) in rest.iter_mut().enumerate() {
            let f = r[col] / pivot[col];
            for (x, p) in r.iter_mut().zip(pivot.iter()).skip(col) {
                *x -= f * p;
            }
            b[col + 1 + off] -= f * b[col];
        }
    }
    let mut c = vec![0.0; n];
    for row in (0..n).rev() {
        let mut s = b[row];
        for k in row + 1..n {
            s -= a[row][k] * c[k];
        }
        c[row] = s / a[row][row];
    }
    c.iter().all(|v| v.is_finite()).then_some(c)
}

fn fit(points: &[(f64, f64)], order: usize) -> Option<Vec<f64>> {
    let m = order + 1;
    let mut a = vec![vec![0.0; m]; m];
    let mut b = vec![0.0; m];
    let mut basis = Vec::with_capacity(m);
    for &(x, y) in points {
        legendre(x, order, &mut basis);
        for i in 0..m {
            b[i] += basis[i] * y;
            for j in 0..m {
                a[i][j] += basis[i] * basis[j];
            }
        }
    }
    solve(a, b)
}

fn eval(c: &[f64], x: f64, basis: &mut Vec<f64>) -> f64 {
    legendre(x, c.len() - 1, basis);
    c.iter().zip(basis.iter()).map(|(a, b)| a * b).sum()
}

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        f64::midpoint(v[n / 2 - 1], v[n / 2])
    }
}

/// The fitted baseline of `y` (module docs), or `None` when too few baseline blocks are found.
pub fn polynomial_baseline(y: &[f64], order: u32) -> Option<Vec<f64>> {
    let n = y.len();
    let order = order.min(8) as usize;
    if n < 16 || y.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let sd = noise_sd(y);
    if !(sd > 0.0) {
        return None;
    }
    let bs = (n / BLOCKS).max(8);
    let xpos = |i: f64| 2.0 * i / (n - 1) as f64 - 1.0;
    let mut blocks: Vec<(f64, f64)> = Vec::new();
    for (b, chunk) in y.chunks(bs).enumerate() {
        let lo = chunk.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = chunk.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if hi - lo < 8.0 * sd {
            let mut c = chunk.to_vec();
            let centre = (b * bs) as f64 + (chunk.len() as f64 - 1.0) / 2.0;
            blocks.push((xpos(centre), median(&mut c)));
        }
    }
    let mut basis = Vec::new();
    let mut coef = None;
    for _ in 0..10 {
        if blocks.len() < order + 2 {
            return None;
        }
        let c = fit(&blocks, order)?;
        let before = blocks.len();
        blocks.retain(|&(x, v)| (v - eval(&c, x, &mut basis)).abs() <= 3.0 * sd);
        coef = Some(c);
        if blocks.len() == before {
            break;
        }
    }
    let c = coef?;
    Some(
        (0..n)
            .map(|i| eval(&c, xpos(i as f64), &mut basis))
            .collect(),
    )
}

/// Apply `mode` to `y` in place; returns the step name when a correction was applied.
pub fn correct(y: &mut [f64], mode: BaselineMode) -> Option<String> {
    let order = mode.order()?;
    let base = polynomial_baseline(y, order)?;
    for (v, b) in y.iter_mut().zip(base) {
        *v -= b;
    }
    Some(format!("baseline:polynomial:{order}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Deterministic pseudo-noise in [-0.5, 0.5) (splitmix64 of the index).
    pub(crate) fn noise(i: usize) -> f64 {
        let mut z = (i as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        ((z >> 11) as f64 / (1u64 << 53) as f64) - 0.5
    }

    #[test]
    fn removes_offset_and_tilt_under_peaks() {
        let n = 16384;
        let y0: Vec<f64> = (0..n)
            .map(|i| {
                let x = i as f64;
                let peak = 500.0 / (1.0 + ((x - 4000.0) / 5.0).powi(2))
                    + 300.0 / (1.0 + ((x - 11000.0) / 5.0).powi(2));
                peak + noise(i)
            })
            .collect();
        let mut y: Vec<f64> = y0
            .iter()
            .enumerate()
            .map(|(i, v)| v + 40.0 + 0.002 * i as f64)
            .collect();
        let step = correct(&mut y, BaselineMode::Polynomial { order: 1 });
        assert_eq!(step.as_deref(), Some("baseline:polynomial:1"));
        for (a, b) in y.iter().zip(&y0).step_by(97) {
            assert!((a - b).abs() < 0.2, "{a} vs {b}");
        }
    }

    #[test]
    fn degenerate_input_is_left_alone() {
        let mut y = vec![1.0; 8];
        assert!(correct(&mut y, BaselineMode::Default).is_none());
        let mut y = vec![f64::NAN; 100];
        assert!(correct(&mut y, BaselineMode::Default).is_none());
        let mut y = vec![0.0; 100];
        assert!(correct(&mut y, BaselineMode::Default).is_none());
        assert!(correct(&mut y, BaselineMode::None).is_none());
    }
}
