//! Descriptive statistics and the Student-t distribution (quantiles for confidence intervals,
//! Grubbs critical values). Written from the textbook definitions: the regularized incomplete
//! beta function by its continued fraction (DLMF 8.17.22) evaluated with the modified Lentz
//! method, and ln Γ by the Lanczos approximation.

/// Arithmetic mean; `None` for an empty slice.
pub fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

/// Sample standard deviation (n − 1 denominator); `None` below two values.
pub fn sd(v: &[f64]) -> Option<f64> {
    if v.len() < 2 {
        return None;
    }
    let m = mean(v)?;
    let ss: f64 = v.iter().map(|x| (x - m) * (x - m)).sum();
    Some((ss / (v.len() - 1) as f64).sqrt())
}

/// Coefficient of variation in percent (sample SD / |mean| × 100); `None` below two values or
/// for a zero mean.
pub fn cv_percent(v: &[f64]) -> Option<f64> {
    let m = mean(v)?;
    let s = sd(v)?;
    (m != 0.0).then(|| 100.0 * s / m.abs())
}

/// Median; `None` for an empty slice.
pub fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len();
    Some(if n % 2 == 1 {
        s[n / 2]
    } else {
        f64::midpoint(s[n / 2 - 1], s[n / 2])
    })
}

/// Median absolute deviation from the median (unscaled).
pub fn mad(v: &[f64]) -> Option<f64> {
    let m = median(v)?;
    let dev: Vec<f64> = v.iter().map(|x| (x - m).abs()).collect();
    median(&dev)
}

/// ln Γ(x) for x > 0 (Lanczos approximation, g = 7, 9 terms; relative error below 1e-14).
pub fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // reflection: Γ(x) Γ(1 − x) = π / sin(πx)
        let s = (std::f64::consts::PI * x).sin().abs();
        return std::f64::consts::PI.ln() - s.ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = C[0];
    let t = x + G + 0.5;
    for (i, c) in C.iter().enumerate().skip(1) {
        a += c / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// Continued fraction of the incomplete beta function (modified Lentz).
fn beta_cf(x: f64, a: f64, b: f64) -> f64 {
    const TINY: f64 = 1e-300;
    const EPS: f64 = 1e-15;
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < TINY {
        d = TINY;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..=10_000 {
        let m = f64::from(m);
        let m2 = 2.0 * m;
        // even step
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        h *= d * c;
        // odd step
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < TINY {
            d = TINY;
        }
        c = 1.0 + aa / c;
        if c.abs() < TINY {
            c = TINY;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < EPS {
            break;
        }
    }
    h
}

/// Regularized incomplete beta function I_x(a, b) for 0 ≤ x ≤ 1, a, b > 0.
pub fn reg_inc_beta(x: f64, a: f64, b: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let ln_front = ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln();
    let front = ln_front.exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        front * beta_cf(x, a, b) / a
    } else {
        1.0 - front * beta_cf(1.0 - x, b, a) / b
    }
}

/// Cumulative distribution function of Student's t with `df` degrees of freedom.
pub fn t_cdf(t: f64, df: f64) -> f64 {
    if !t.is_finite() {
        return if t > 0.0 { 1.0 } else { 0.0 };
    }
    let x = df / (df + t * t);
    let tail = 0.5 * reg_inc_beta(x, 0.5 * df, 0.5);
    if t >= 0.0 { 1.0 - tail } else { tail }
}

/// Quantile of Student's t: the `t` with `P(T ≤ t) = p`. `None` for p outside (0, 1) or df ≤ 0.
pub fn t_quantile(p: f64, df: f64) -> Option<f64> {
    if p.is_nan() || p <= 0.0 || p >= 1.0 || df.is_nan() || df <= 0.0 || !df.is_finite() {
        return None;
    }
    if (p - 0.5).abs() < 1e-300 {
        return Some(0.0);
    }
    let upper = p > 0.5;
    let q = if upper { p } else { 1.0 - p };
    // bracket, then bisect: robust for every df ≥ tiny
    let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
    while t_cdf(hi, df) < q {
        hi *= 2.0;
        if hi > 1e300 {
            return None;
        }
    }
    for _ in 0..200 {
        let mid = f64::midpoint(lo, hi);
        if t_cdf(mid, df) < q {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo <= 1e-14 * hi.max(1e-300) {
            break;
        }
    }
    let t = f64::midpoint(lo, hi);
    Some(if upper { t } else { -t })
}

/// Two-sided Grubbs critical value G for n values at significance `alpha`:
/// ((n − 1)/√n) · √(t² / (n − 2 + t²)), t the upper α/(2n) quantile of t with n − 2 df.
pub fn grubbs_critical(n: usize, alpha: f64) -> Option<f64> {
    if n < 3 {
        return None;
    }
    let nf = n as f64;
    let t = t_quantile(1.0 - alpha / (2.0 * nf), nf - 2.0)?;
    Some((nf - 1.0) / nf.sqrt() * (t * t / (nf - 2.0 + t * t)).sqrt())
}

/// Trapezoidal integral of y over x (x ascending).
pub fn trapezoid(x: &[f64], y: &[f64]) -> f64 {
    x.windows(2)
        .zip(y.windows(2))
        .map(|(xs, ys)| 0.5 * (xs[1] - xs[0]) * (ys[0] + ys[1]))
        .sum()
}

/// Ordinary least-squares line through (x, y): (slope, intercept, r²). `None` below two
/// points or when x does not vary.
pub fn ols(x: &[f64], y: &[f64]) -> Option<(f64, f64, f64)> {
    let n = x.len();
    if n < 2 || y.len() != n {
        return None;
    }
    let mx = mean(x)?;
    let my = mean(y)?;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for (a, b) in x.iter().zip(y) {
        sxx += (a - mx) * (a - mx);
        sxy += (a - mx) * (b - my);
        syy += (b - my) * (b - my);
    }
    if sxx <= 0.0 {
        return None;
    }
    let slope = sxy / sxx;
    let r2 = if syy > 0.0 {
        (sxy * sxy) / (sxx * syy)
    } else {
        1.0
    };
    Some((slope, my - slope * mx, r2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * b.abs().max(1e-12)
    }

    #[test]
    fn descriptive() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(mean(&v), Some(2.5));
        assert!(close(sd(&v).unwrap(), 1.290_994_448_735_805_6, 1e-12));
        assert_eq!(median(&v), Some(2.5));
        assert_eq!(mad(&[1.0, 2.0, 3.0, 100.0]), Some(1.0));
        assert!(sd(&[1.0]).is_none());
    }

    #[test]
    fn gamma() {
        assert!(close(ln_gamma(5.0), 24f64.ln(), 1e-13));
        assert!(close(
            ln_gamma(0.5),
            std::f64::consts::PI.sqrt().ln(),
            1e-13
        ));
    }

    #[test]
    fn student_t_quantiles_match_scipy() {
        // scipy.stats.t.ppf(0.975, df)
        for (df, want) in [
            (1.0, 12.706_204_736_174_698),
            (2.0, 4.302_652_729_749_464),
            (5.0, 2.570_581_835_636_314),
            (10.0, 2.228_138_851_986_274),
            (30.0, 2.042_272_456_301_238),
            (1000.0, 1.962_339_080_826_407_8),
        ] {
            let got = t_quantile(0.975, df).unwrap();
            assert!(close(got, want, 1e-10), "df {df}: {got} vs {want}");
        }
        assert!(close(
            t_quantile(0.025, 5.0).unwrap(),
            -2.570_581_835_636_314,
            1e-10
        ));
        assert!(close(t_cdf(2.0, 7.0), 0.957_190_335_718_512, 1e-12));
    }

    #[test]
    fn grubbs() {
        // G critical for n = 10, alpha = 0.05 (two-sided): 2.29 (published tables: 2.290)
        let g = grubbs_critical(10, 0.05).unwrap();
        assert!((g - 2.290).abs() < 0.001, "{g}");
        assert!(grubbs_critical(2, 0.05).is_none());
    }

    #[test]
    fn line() {
        let (s, i, r2) = ols(&[0.0, 1.0, 2.0], &[1.0, 3.0, 5.0]).unwrap();
        assert!(close(s, 2.0, 1e-12) && close(i, 1.0, 1e-12) && close(r2, 1.0, 1e-12));
        assert!(ols(&[1.0, 1.0], &[1.0, 2.0]).is_none());
        assert!(close(
            trapezoid(&[0.0, 1.0, 2.0], &[0.0, 1.0, 0.0]),
            1.0,
            1e-12
        ));
    }
}
