//! Tiny dense linear algebra for the normal equations of curve fits (at most 5 parameters):
//! Gaussian elimination with partial pivoting and a matrix inverse built on it. Row-major
//! `n × n` matrices in a `Vec<f64>`.

/// Solve `A x = b` in place (A is `n × n`, row-major). `None` when A is singular to working
/// precision.
pub fn solve(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    if a.len() != n * n || b.len() != n {
        return None;
    }
    let mut m = a.to_vec();
    let mut x = b.to_vec();
    let scale = m.iter().fold(0.0_f64, |s, v| s.max(v.abs())).max(1e-300);
    for col in 0..n {
        let mut piv = col;
        for r in col + 1..n {
            if m[r * n + col].abs() > m[piv * n + col].abs() {
                piv = r;
            }
        }
        if m[piv * n + col].abs() <= scale * 1e-15 {
            return None;
        }
        if piv != col {
            for c in 0..n {
                m.swap(piv * n + c, col * n + c);
            }
            x.swap(piv, col);
        }
        let d = m[col * n + col];
        for r in col + 1..n {
            let f = m[r * n + col] / d;
            if f != 0.0 {
                for c in col..n {
                    m[r * n + c] -= f * m[col * n + c];
                }
                x[r] -= f * x[col];
            }
        }
    }
    for col in (0..n).rev() {
        let mut s = x[col];
        for c in col + 1..n {
            s -= m[col * n + c] * x[c];
        }
        x[col] = s / m[col * n + col];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Inverse of `A` (`n × n`, row-major); `None` when singular.
pub fn invert(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut inv = vec![0.0; n * n];
    for j in 0..n {
        let mut e = vec![0.0; n];
        e[j] = 1.0;
        let col = solve(a, &e, n)?;
        for i in 0..n {
            inv[i * n + j] = col[i];
        }
    }
    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_and_inverts() {
        let a = [4.0, 1.0, 2.0, 1.0, 3.0, 0.0, 2.0, 0.0, 5.0];
        let x = solve(&a, &[1.0, 2.0, 3.0], 3).unwrap();
        // check A x = b
        for i in 0..3 {
            let s: f64 = (0..3).map(|j| a[i * 3 + j] * x[j]).sum();
            assert!((s - [1.0, 2.0, 3.0][i]).abs() < 1e-12);
        }
        let inv = invert(&a, 3).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let s: f64 = (0..3).map(|k| a[i * 3 + k] * inv[k * 3 + j]).sum();
                assert!((s - f64::from(u8::from(i == j))).abs() < 1e-12);
            }
        }
        assert!(solve(&[1.0, 2.0, 2.0, 4.0], &[1.0, 1.0], 2).is_none());
    }
}
