//! Small, exact-enough statistics for group summaries: descriptive statistics, Welch's t-test and
//! the Mann–Whitney U test, with p-values defined as SciPy defines them
//! (`scipy.stats.ttest_ind(equal_var=False)`, `scipy.stats.mannwhitneyu(method="auto")`, both
//! two-sided), and checked against SciPy in the tests and the corpus harness.

/// Descriptive statistics of the finite values.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Describe {
    /// Finite values.
    pub n: usize,
    /// Mean.
    pub mean: Option<f64>,
    /// Sample standard deviation (n − 1).
    pub sd: Option<f64>,
    /// Standard error of the mean (sd / √n).
    pub sem: Option<f64>,
    /// Median (mean of the two middle values when n is even).
    pub median: Option<f64>,
    /// Smallest value.
    pub min: Option<f64>,
    /// Largest value.
    pub max: Option<f64>,
    /// Coefficient of variation, sd / |mean| × 100 (absent when the mean is 0).
    pub cv_percent: Option<f64>,
}

/// Describe `values` (NaN and infinities are ignored).
pub fn describe(values: &[f64]) -> Describe {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| x.is_finite()).collect();
    let n = v.len();
    if n == 0 {
        return Describe::default();
    }
    v.sort_by(f64::total_cmp);
    let mean = kahan_sum(&v) / n as f64;
    let sd = (n > 1).then(|| {
        let ss: f64 = v.iter().map(|x| (x - mean) * (x - mean)).sum();
        (ss / (n - 1) as f64).sqrt()
    });
    let median = if n % 2 == 1 {
        v[n / 2]
    } else {
        v[n / 2 - 1] / 2.0 + v[n / 2] / 2.0
    };
    Describe {
        n,
        mean: Some(mean),
        sd,
        sem: sd.map(|s| s / (n as f64).sqrt()),
        median: Some(median),
        min: v.first().copied(),
        max: v.last().copied(),
        cv_percent: sd.filter(|_| mean != 0.0).map(|s| s / mean.abs() * 100.0),
    }
}

fn kahan_sum(v: &[f64]) -> f64 {
    let (mut s, mut c) = (0.0f64, 0.0f64);
    for &x in v {
        let y = x - c;
        let t = s + y;
        c = (t - s) - y;
        s = t;
    }
    s
}

/// Result of a two-sample test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TestResult {
    /// The statistic: t (Welch) or U of the first sample (Mann–Whitney).
    pub statistic: f64,
    /// Degrees of freedom (Welch only).
    pub df: Option<f64>,
    /// Two-sided p-value.
    pub p_value: f64,
    /// `exact` or `asymptotic` (Mann–Whitney), `welch`.
    pub method: &'static str,
}

/// Welch's unequal-variance t-test of `a` against `b`, two-sided. `None` with fewer than two
/// finite values in either sample or zero variance in both.
pub fn welch(a: &[f64], b: &[f64]) -> Option<TestResult> {
    let da = describe(a);
    let db = describe(b);
    if da.n < 2 || db.n < 2 {
        return None;
    }
    let (ma, mb) = (da.mean?, db.mean?);
    let va = da.sd? * da.sd? / da.n as f64;
    let vb = db.sd? * db.sd? / db.n as f64;
    let se2 = va + vb;
    if se2 <= 0.0 {
        return None;
    }
    let t = (ma - mb) / se2.sqrt();
    let df = se2 * se2 / (va * va / (da.n - 1) as f64 + vb * vb / (db.n - 1) as f64);
    let p = 2.0 * student_t_sf(t.abs(), df);
    Some(TestResult {
        statistic: t,
        df: Some(df),
        p_value: p.clamp(0.0, 1.0),
        method: "welch",
    })
}

/// The Mann–Whitney U test of `a` against `b`, two-sided, as `scipy.stats.mannwhitneyu` with
/// its defaults: the exact null distribution when either sample has at most 8 values and there
/// are no ties, else the normal approximation with the tie correction and a continuity
/// correction. The statistic is U of `a`.
#[allow(clippy::float_cmp)] // ties are exactly equal values, as ranks define them
pub fn mann_whitney(a: &[f64], b: &[f64]) -> Option<TestResult> {
    let a: Vec<f64> = a.iter().copied().filter(|x| x.is_finite()).collect();
    let b: Vec<f64> = b.iter().copied().filter(|x| x.is_finite()).collect();
    let (n1, n2) = (a.len(), b.len());
    if n1 == 0 || n2 == 0 {
        return None;
    }
    // average ranks over the pooled sample
    let mut all: Vec<(f64, usize)> = a
        .iter()
        .map(|&x| (x, 0))
        .chain(b.iter().map(|&x| (x, 1)))
        .collect();
    all.sort_by(|x, y| x.0.total_cmp(&y.0));
    let n = all.len();
    let mut ranks = vec![0.0f64; n];
    let mut ties: Vec<f64> = Vec::new();
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && all[j].0 == all[i].0 {
            j += 1;
        }
        let r = (i + j + 1) as f64 / 2.0; // ranks i+1 ..= j
        for rk in &mut ranks[i..j] {
            *rk = r;
        }
        if j - i > 1 {
            ties.push((j - i) as f64);
        }
        i = j;
    }
    let r1: f64 = all
        .iter()
        .zip(&ranks)
        .filter(|((_, g), _)| *g == 0)
        .map(|(_, r)| r)
        .sum();
    let (f1, f2) = (n1 as f64, n2 as f64);
    let u1 = r1 - f1 * (f1 + 1.0) / 2.0;
    let u2 = f1 * f2 - u1;
    let exact = (n1 <= 8 || n2 <= 8) && ties.is_empty();
    let p = if exact {
        let u = u1.max(u2).round() as usize;
        2.0 * exact_u_sf(n1, n2, u)
    } else {
        let mu = f1 * f2 / 2.0;
        let nn = f1 + f2;
        let tie_term: f64 = ties.iter().map(|t| t * t * t - t).sum();
        let s = (f1 * f2 / 12.0 * ((nn + 1.0) - tie_term / (nn * (nn - 1.0)))).sqrt();
        let num = u1 - mu;
        let num = num - 0.5 * num.signum();
        let z = if s > 0.0 { num / s } else { 0.0 };
        2.0 * normal_sf(z.abs())
    };
    Some(TestResult {
        statistic: u1,
        df: None,
        p_value: p.clamp(0.0, 1.0),
        method: if exact { "exact" } else { "asymptotic" },
    })
}

/// P(U ≥ u) under the null, for sample sizes `n1`, `n2` (no ties).
fn exact_u_sf(n1: usize, n2: usize, u: usize) -> f64 {
    let (m, n) = (n1.min(n2), n1.max(n2));
    // generating function: prod_{i=1..m} (1 - q^(n+i)) / (1 - q^i)
    let deg = m * n;
    let mut poly = vec![0i128; deg + 1];
    poly[0] = 1;
    for i in 1..=m {
        let d = n + i;
        for k in (d..=deg).rev() {
            poly[k] -= poly[k - d];
        }
        for k in i..=deg {
            poly[k] += poly[k - i];
        }
    }
    let total: f64 = poly.iter().map(|&c| c as f64).sum();
    let tail: f64 = poly.iter().skip(u.min(deg + 1)).map(|&c| c as f64).sum();
    tail / total
}

// ---------------------------------------------------------------- special functions

/// ln Γ(x) for x > 0 (Lanczos, g = 7, 9 terms; relative error about 1e-15).
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
        // reflection
        let pi = std::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = C[0];
    let t = x + G + 0.5;
    for (i, c) in C.iter().enumerate().skip(1) {
        a += c / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// Regularized incomplete beta I_x(a, b) (continued fraction, modified Lentz).
pub fn inc_beta(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let ln_front = ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln();
    if x < (a + 1.0) / (a + b + 2.0) {
        ln_front.exp() * beta_cf(a, b, x) / a
    } else {
        1.0 - ln_front.exp() * beta_cf(b, a, 1.0 - x) / b
    }
}

fn beta_cf(a: f64, b: f64, x: f64) -> f64 {
    const TINY: f64 = 1e-300;
    const EPS: f64 = 1e-16;
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < TINY {
        d = TINY;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..10_000 {
        let m = f64::from(m);
        let m2 = 2.0 * m;
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

/// P(T > t) for Student's t with `df` degrees of freedom, t ≥ 0.
pub fn student_t_sf(t: f64, df: f64) -> f64 {
    if !t.is_finite() {
        return 0.0;
    }
    let x = df / (df + t * t);
    0.5 * inc_beta(df / 2.0, 0.5, x)
}

/// Regularized upper incomplete gamma Q(a, x).
fn gamma_q(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    let ln_front = -x + a * x.ln() - ln_gamma(a);
    if x < a + 1.0 {
        // series for P
        let mut sum = 1.0 / a;
        let mut term = sum;
        let mut ap = a;
        for _ in 0..10_000 {
            ap += 1.0;
            term *= x / ap;
            sum += term;
            if term.abs() < sum.abs() * 1e-17 {
                break;
            }
        }
        1.0 - sum * ln_front.exp()
    } else {
        // continued fraction for Q (modified Lentz)
        const TINY: f64 = 1e-300;
        let mut b = x + 1.0 - a;
        let mut c = 1.0 / TINY;
        let mut d = 1.0 / b;
        let mut h = d;
        for i in 1..10_000 {
            let i = f64::from(i);
            let an = -i * (i - a);
            b += 2.0;
            d = an * d + b;
            if d.abs() < TINY {
                d = TINY;
            }
            c = b + an / c;
            if c.abs() < TINY {
                c = TINY;
            }
            d = 1.0 / d;
            let del = d * c;
            h *= del;
            if (del - 1.0).abs() < 1e-16 {
                break;
            }
        }
        ln_front.exp() * h
    }
}

/// P(Z > z) for a standard normal Z.
pub fn normal_sf(z: f64) -> f64 {
    if z < 0.0 {
        return 1.0 - normal_sf(-z);
    }
    // erfc(z/√2)/2 = Q(1/2, z²/2)/2
    0.5 * gamma_q(0.5, z * z / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * b.abs().max(1e-300)
    }

    #[test]
    fn describe_matches_numpy_conventions() {
        let d = describe(&[1.0, 2.0, 3.0, 4.0, f64::NAN]);
        assert_eq!(d.n, 4);
        assert_eq!(d.median, Some(2.5));
        assert!(close(d.sd.unwrap(), 1.290_994_448_735_805_6, 1e-14));
        assert!(close(d.cv_percent.unwrap(), 51.639_777_949_432_22, 1e-12));
        assert_eq!(describe(&[]).n, 0);
        assert_eq!(describe(&[5.0]).sd, None);
    }

    // Expected values from SciPy 1.18.1:
    //   ttest_ind([1,2,3,4,5],[2.5,3.5,6,7,9,10],equal_var=False)
    //   mannwhitneyu(...) with the defaults
    #[test]
    fn welch_matches_scipy() {
        let r = welch(&[1.0, 2.0, 3.0, 4.0, 5.0], &[2.5, 3.5, 6.0, 7.0, 9.0, 10.0]).unwrap();
        assert!(close(r.statistic, -2.380_277_794_628_895_5, 1e-12), "{r:?}");
        assert!(close(r.df.unwrap(), 7.857_404_091_103_993, 1e-12), "{r:?}");
        assert!(close(r.p_value, 0.045_072_979_320_042_485, 1e-9), "{r:?}");
        assert!(welch(&[1.0], &[1.0, 2.0]).is_none());
    }

    #[test]
    fn mann_whitney_matches_scipy() {
        // exact (n <= 8, no ties)
        let r = mann_whitney(&[1.0, 2.0, 3.0, 4.0, 5.0], &[2.5, 3.5, 6.0, 7.0, 9.0, 10.0]).unwrap();
        assert_eq!(r.method, "exact");
        assert!(close(r.statistic, 5.0, 1e-15));
        assert!(close(r.p_value, 0.082_251_082_251_082_26, 1e-12), "{r:?}");
        // asymptotic with ties
        let a = [1.0, 2.0, 2.0, 3.0, 4.0, 5.0, 5.0, 6.0, 7.0, 8.0];
        let b = [3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 9.0, 10.0, 11.0];
        let r = mann_whitney(&a, &b).unwrap();
        assert_eq!(r.method, "asymptotic");
        assert!(close(r.statistic, 20.5, 1e-15));
        assert!(close(r.p_value, 0.027_713_658_786_859_403, 1e-9), "{r:?}");
    }

    #[test]
    fn special_functions() {
        assert!(close(ln_gamma(0.5), 0.572_364_942_924_7, 1e-14));
        assert!(close(ln_gamma(10.0), 12.801_827_480_081_469, 1e-14));
        assert!(close(normal_sf(1.96), 0.024_997_895_148_220_435, 1e-12));
        assert!(close(normal_sf(0.0), 0.5, 1e-15));
        assert!(close(normal_sf(6.0), 9.865_876_450_376_944e-10, 1e-9));
        assert!(close(
            student_t_sf(2.0, 5.0),
            0.050_969_739_414_929_16,
            1e-11
        ));
        assert!(close(inc_beta(2.0, 3.0, 0.4), 0.5248, 1e-12));
    }
}
