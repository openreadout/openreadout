//! Curve models and least-squares fitting: straight line (closed form), four- and
//! five-parameter logistic (Levenberg–Marquardt with analytic derivatives, several starting
//! points), parameter standard errors and confidence intervals, and inverse prediction
//! (back-calculation of a concentration from a signal).
//!
//! Model conventions (book/src/guides/plate-analysis.md):
//!
//! - `linear`: y = slope · x + intercept
//! - `4pl`: y = d + (a − d) / (1 + (x/c)^b), with b > 0: a is the response at zero
//!   concentration, d the response at infinite concentration, c the inflection point (EC50), b
//!   the slope factor. (Gen5 writes `Y = (A-D)/(1+(X/C)^B) + D`, SkanIt `y = d + (a-d)/(1+(x/c)^b)`:
//!   the same parameters.)
//! - `5pl`: y = d + (a − d) / (1 + (x/c)^b)^g, g > 0 the asymmetry; b may be negative (the two
//!   signs are different asymmetric shapes).
//!
//! The fit minimises Σ wᵢ (yᵢ − f(xᵢ))². Internally c and g are fitted on the log scale (they
//! must stay positive); reported standard errors are the Wald (Gauss–Newton) ones,
//! s² (JᵀWJ)⁻¹ with s² = SSE / (n − p), transformed to the reported scale by the delta method,
//! which is exact for this reparameterisation at the optimum.

use serde::{Deserialize, Serialize};

use crate::linalg;
use crate::stats;

/// A curve model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum Model {
    /// y = slope · x + intercept.
    #[serde(rename = "linear")]
    Linear,
    /// Four-parameter logistic: y = d + (a − d) / (1 + (x/c)^b).
    #[serde(rename = "4pl")]
    FourPl,
    /// Five-parameter logistic: y = d + (a − d) / (1 + (x/c)^b)^g.
    #[serde(rename = "5pl")]
    FivePl,
}

impl Model {
    /// The model id (`linear`, `4pl`, `5pl`).
    pub fn id(self) -> &'static str {
        match self {
            Model::Linear => "linear",
            Model::FourPl => "4pl",
            Model::FivePl => "5pl",
        }
    }
    /// The formula in the parameter names reported.
    pub fn formula(self) -> &'static str {
        match self {
            Model::Linear => "y = slope * x + intercept",
            Model::FourPl => "y = d + (a - d) / (1 + (x/c)^b)",
            Model::FivePl => "y = d + (a - d) / (1 + (x/c)^b)^g",
        }
    }
    /// Reported parameter names, in order.
    pub fn param_names(self) -> &'static [&'static str] {
        match self {
            Model::Linear => &["slope", "intercept"],
            Model::FourPl => &["a", "b", "c", "d"],
            Model::FivePl => &["a", "b", "c", "d", "g"],
        }
    }
    /// Number of parameters.
    pub fn n_params(self) -> usize {
        self.param_names().len()
    }
    /// Parse `linear`, `4pl`, `5pl` (also `4-pl`, `4p`, `logistic`).
    pub fn parse(s: &str) -> Option<Model> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_', ' '], "")
            .as_str()
        {
            "linear" | "lin" | "line" => Some(Model::Linear),
            "4pl" | "4p" | "4param" | "fourpl" | "logistic" => Some(Model::FourPl),
            "5pl" | "5p" | "5param" | "fivepl" => Some(Model::FivePl),
            _ => None,
        }
    }
}

/// Weights of the least-squares fit, from the observed values.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
pub enum Weighting {
    /// Every point weight 1 (ordinary least squares).
    #[default]
    #[serde(rename = "none")]
    None,
    /// w = 1/|y|.
    #[serde(rename = "1/y")]
    InverseY,
    /// w = 1/y².
    #[serde(rename = "1/y2")]
    InverseY2,
    /// w = 1/x (x > 0).
    #[serde(rename = "1/x")]
    InverseX,
    /// w = 1/x² (x > 0).
    #[serde(rename = "1/x2")]
    InverseX2,
}

impl Weighting {
    /// Id as written on the command line.
    pub fn id(self) -> &'static str {
        match self {
            Weighting::None => "none",
            Weighting::InverseY => "1/y",
            Weighting::InverseY2 => "1/y2",
            Weighting::InverseX => "1/x",
            Weighting::InverseX2 => "1/x2",
        }
    }
    /// Parse `none`, `1/y`, `1/y2` (`1/y^2`), `1/x`, `1/x2`.
    pub fn parse(s: &str) -> Option<Weighting> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['^', ' '], "")
            .as_str()
        {
            "none" | "" | "1" | "equal" => Some(Weighting::None),
            "1/y" => Some(Weighting::InverseY),
            "1/y2" | "1/yy" => Some(Weighting::InverseY2),
            "1/x" => Some(Weighting::InverseX),
            "1/x2" | "1/xx" => Some(Weighting::InverseX2),
            _ => None,
        }
    }
    /// The weight of one point; `None` when it cannot be formed (y = 0 for 1/y, x ≤ 0 for 1/x).
    pub fn weight(self, x: f64, y: f64) -> Option<f64> {
        let w = match self {
            Weighting::None => 1.0,
            Weighting::InverseY => 1.0 / y.abs(),
            Weighting::InverseY2 => 1.0 / (y * y),
            Weighting::InverseX => {
                if x <= 0.0 {
                    return None;
                }
                1.0 / x
            }
            Weighting::InverseX2 => {
                if x <= 0.0 {
                    return None;
                }
                1.0 / (x * x)
            }
        };
        (w.is_finite() && w > 0.0).then_some(w)
    }
}

/// Why a signal could not be converted to a concentration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InverseFail {
    /// The signal lies beyond the curve's zero-concentration asymptote (conc ≤ 0).
    BelowCurve,
    /// The signal lies beyond the curve's infinite-concentration asymptote.
    AboveCurve,
    /// The curve is flat (slope 0 or a = d).
    Flat,
}

/// A fitted curve.
#[derive(Debug, Clone)]
pub struct FitResult {
    /// The model.
    pub model: Model,
    /// Reported parameters (see [`Model::param_names`]).
    pub params: Vec<f64>,
    /// Covariance of the reported parameters (p × p, row-major), when estimable (df > 0).
    pub cov: Option<Vec<f64>>,
    /// Weighted sum of squared residuals.
    pub sse: f64,
    /// Points used.
    pub n: usize,
    /// Residual degrees of freedom (n − p).
    pub df: usize,
    /// Coefficient of determination 1 − SSE/SST (weighted when the fit is weighted).
    pub r_squared: f64,
    /// Whether the optimiser met its convergence criterion.
    pub converged: bool,
    /// Optimiser iterations (0 for the closed-form line).
    pub iterations: usize,
}

impl FitResult {
    /// Predicted response at `x`.
    pub fn eval(&self, x: f64) -> f64 {
        eval_reported(self.model, &self.params, x)
    }

    /// Standard error of parameter `i` (reported scale).
    pub fn se(&self, i: usize) -> Option<f64> {
        let p = self.params.len();
        let c = self.cov.as_ref()?;
        let v = *c.get(i * p + i)?;
        (v.is_finite() && v >= 0.0).then(|| v.sqrt())
    }

    /// Standard error of ln c (logistic models): SE(c) / c.
    pub fn se_ln_c(&self) -> Option<f64> {
        if self.model == Model::Linear {
            return None;
        }
        Some(self.se(2)? / self.params[2])
    }

    /// Concentration giving response `y` (inverse prediction).
    pub fn inverse(&self, y: f64) -> Result<f64, InverseFail> {
        let p = &self.params;
        match self.model {
            Model::Linear => {
                if p[0] == 0.0 {
                    return Err(InverseFail::Flat);
                }
                Ok((y - p[1]) / p[0])
            }
            Model::FourPl | Model::FivePl => {
                let (a, b, c, d) = (p[0], p[1], p[2], p[3]);
                let g = if self.model == Model::FivePl {
                    p[4]
                } else {
                    1.0
                };
                if a == d || b == 0.0 {
                    return Err(InverseFail::Flat);
                }
                // t = (y − d)/(a − d) = (1 + u)^(−g), u = (x/c)^b
                let t = (y - d) / (a - d);
                // t → 1 at x → 0 when b > 0, at x → ∞ when b < 0
                let (at_one, at_zero) = if b > 0.0 {
                    (InverseFail::BelowCurve, InverseFail::AboveCurve)
                } else {
                    (InverseFail::AboveCurve, InverseFail::BelowCurve)
                };
                if t >= 1.0 {
                    return Err(at_one);
                }
                if t <= 0.0 {
                    return Err(at_zero);
                }
                let u = t.powf(-1.0 / g) - 1.0;
                if u <= 0.0 {
                    return Err(at_one);
                }
                let x = c * u.powf(1.0 / b);
                if x.is_finite() {
                    Ok(x)
                } else if x > 0.0 {
                    Err(at_zero)
                } else {
                    Err(at_one)
                }
            }
        }
    }
}

/// Model value from reported parameters.
fn eval_reported(model: Model, p: &[f64], x: f64) -> f64 {
    match model {
        Model::Linear => p[0] * x + p[1],
        Model::FourPl => eval_logistic(&[p[0], p[1], p[2].ln(), p[3]], x, None),
        Model::FivePl => eval_logistic(&[p[0], p[1], p[2].ln(), p[3], p[4].ln()], x, None),
    }
}

/// Stable ln(1 + e^z).
fn softplus(z: f64) -> f64 {
    if z > 0.0 {
        z + (-z).exp().ln_1p()
    } else {
        z.exp().ln_1p()
    }
}

/// Stable 1 / (1 + e^−z).
fn sigmoid(z: f64) -> f64 {
    if z >= 0.0 {
        1.0 / (1.0 + (-z).exp())
    } else {
        let e = z.exp();
        e / (1.0 + e)
    }
}

/// Logistic model value (and gradient into `grad` when given) at internal parameters
/// `[a, b, ln c, d]` (4PL) or `[a, b, ln c, d, ln g]` (5PL).
fn eval_logistic(t: &[f64], x: f64, grad: Option<&mut [f64]>) -> f64 {
    let (a, b, lnc, d) = (t[0], t[1], t[2], t[3]);
    let five = t.len() == 5;
    let g = if five { t[4].exp() } else { 1.0 };
    if x <= 0.0 || !x.is_finite() {
        // limits at x = 0: u = (x/c)^b → 0 (b > 0) or ∞ (b < 0)
        let f_frac = if b > 0.0 {
            1.0
        } else if b < 0.0 {
            0.0
        } else {
            2f64.powf(-g)
        };
        if let Some(gr) = grad {
            gr[0] = f_frac;
            gr[1] = 0.0;
            gr[2] = 0.0;
            gr[3] = 1.0 - f_frac;
            if five {
                gr[4] = if b == 0.0 {
                    (a - d) * f_frac * -(2f64.ln()) * g
                } else {
                    0.0
                };
            }
        }
        return d + (a - d) * f_frac;
    }
    let lx = x.ln() - lnc;
    let z = b * lx;
    let (f_frac, dz) = if five {
        let sp = softplus(z);
        let f = (-g * sp).exp();
        // dF/dz = −g F σ(z)
        (f, -g * f * sigmoid(z))
    } else {
        let f = sigmoid(-z);
        (f, -f * (1.0 - f))
    };
    if let Some(gr) = grad {
        gr[0] = f_frac;
        gr[1] = (a - d) * dz * lx;
        gr[2] = (a - d) * dz * -b;
        gr[3] = 1.0 - f_frac;
        if five {
            gr[4] = (a - d) * f_frac * -softplus(z) * g;
        }
    }
    d + (a - d) * f_frac
}

/// Failure to fit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FitError(pub String);

impl std::fmt::Display for FitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Fit `model` to points (x, y) with `weighting`. Points with a non-finite x or y are
/// rejected by the caller; weights that cannot be formed are an error.
pub fn fit(
    model: Model,
    x: &[f64],
    y: &[f64],
    weighting: Weighting,
) -> Result<FitResult, FitError> {
    if x.len() != y.len() {
        return Err(FitError("x and y differ in length".into()));
    }
    let mut w = Vec::with_capacity(x.len());
    for (&xi, &yi) in x.iter().zip(y) {
        if !xi.is_finite() || !yi.is_finite() {
            return Err(FitError("a point is not a finite number".into()));
        }
        w.push(weighting.weight(xi, yi).ok_or_else(|| {
            FitError(format!(
                "weighting {} cannot weight the point x = {xi}, y = {yi}",
                weighting.id()
            ))
        })?);
    }
    let mut levels: Vec<f64> = x.to_vec();
    levels.sort_by(f64::total_cmp);
    levels.dedup();
    let p = model.n_params();
    let min_levels = match model {
        Model::Linear => 2,
        Model::FourPl => 4,
        Model::FivePl => 5,
    };
    if levels.len() < min_levels {
        return Err(FitError(format!(
            "a {} fit needs at least {min_levels} distinct concentrations; got {}",
            model.id(),
            levels.len()
        )));
    }
    if model != Model::Linear && !x.iter().any(|v| *v > 0.0) {
        return Err(FitError(
            "a logistic fit needs positive concentrations".into(),
        ));
    }
    let n = x.len();
    let mut res = match model {
        Model::Linear => fit_linear(x, y, &w),
        Model::FourPl | Model::FivePl => fit_logistic(model, x, y, &w),
    }?;
    res.n = n;
    res.df = n.saturating_sub(p);
    // R² (weighted when weighted)
    let sw: f64 = w.iter().sum();
    let ybar = w.iter().zip(y).map(|(wi, yi)| wi * yi).sum::<f64>() / sw;
    let sst: f64 = w
        .iter()
        .zip(y)
        .map(|(wi, yi)| wi * (yi - ybar) * (yi - ybar))
        .sum();
    res.r_squared = if sst > 0.0 { 1.0 - res.sse / sst } else { 1.0 };
    Ok(res)
}

fn fit_linear(x: &[f64], y: &[f64], w: &[f64]) -> Result<FitResult, FitError> {
    // weighted normal equations for [slope, intercept]
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for ((&xi, &yi), &wi) in x.iter().zip(y).zip(w) {
        sw += wi;
        sx += wi * xi;
        sy += wi * yi;
        sxx += wi * xi * xi;
        sxy += wi * xi * yi;
    }
    let ata = [sxx, sx, sx, sw];
    let sol = linalg::solve(&ata, &[sxy, sy], 2)
        .ok_or_else(|| FitError("the concentrations do not vary".into()))?;
    let sse: f64 = x
        .iter()
        .zip(y)
        .zip(w)
        .map(|((xi, yi), wi)| {
            let r = yi - (sol[0] * xi + sol[1]);
            wi * r * r
        })
        .sum();
    let n = x.len();
    let cov = (n > 2)
        .then(|| linalg::invert(&ata, 2))
        .flatten()
        .map(|inv| {
            let s2 = sse / (n - 2) as f64;
            inv.iter().map(|v| v * s2).collect()
        });
    Ok(FitResult {
        model: Model::Linear,
        params: sol,
        cov,
        sse,
        n,
        df: 0,
        r_squared: 0.0,
        converged: true,
        iterations: 0,
    })
}

/// A model for the optimiser: value at `x` for parameters `t`, writing the gradient into the
/// slice when one is given.
pub(crate) type ModelFn<'a> = &'a dyn Fn(&[f64], f64, Option<&mut [f64]>) -> f64;

/// Weighted SSE at parameters `t`.
fn sse_at(f: ModelFn<'_>, t: &[f64], x: &[f64], y: &[f64], w: &[f64]) -> f64 {
    let mut s = 0.0;
    for ((&xi, &yi), &wi) in x.iter().zip(y).zip(w) {
        let r = yi - f(t, xi, None);
        s += wi * r * r;
    }
    if s.is_finite() { s } else { f64::INFINITY }
}

/// Bounds that keep the optimiser away from overflow: ln c within 15 e-folds of the data, |b| ≤
/// 100, ln g within ±8.
struct Bounds {
    lnc: (f64, f64),
}

impl Bounds {
    fn ok(&self, t: &[f64]) -> bool {
        t.iter().all(|v| v.is_finite())
            && t[1].abs() <= 100.0
            && t[2] >= self.lnc.0
            && t[2] <= self.lnc.1
            && (t.len() < 5 || t[4].abs() <= 8.0)
    }
}

/// Result of one Levenberg–Marquardt run.
pub(crate) struct LmOutcome {
    pub(crate) theta: Vec<f64>,
    pub(crate) sse: f64,
    pub(crate) converged: bool,
    pub(crate) iterations: usize,
}

/// Levenberg–Marquardt (Marquardt's diagonal scaling) from `start`; a trial step outside `ok`
/// is rejected like a step that raises the SSE. Stops when a step lowers the SSE by less than
/// 1e-13 relative or moves the parameters by less than 1e-12 relative, or when no step lowers
/// it (a minimum to working precision); at most 2000 iterations.
pub(crate) fn levenberg_marquardt(
    f: ModelFn<'_>,
    ok: &dyn Fn(&[f64]) -> bool,
    start: Vec<f64>,
    x: &[f64],
    y: &[f64],
    w: &[f64],
) -> LmOutcome {
    let p = start.len();
    let n = x.len();
    let mut theta = start;
    let mut sse = sse_at(f, &theta, x, y, w);
    let mut lambda = 1e-3;
    let mut grad = vec![0.0; p];
    let mut converged = false;
    let mut it = 0;
    while it < 2000 {
        it += 1;
        // normal equations
        let mut jtj = vec![0.0; p * p];
        let mut jtr = vec![0.0; p];
        for i in 0..n {
            let fi = f(&theta, x[i], Some(&mut grad));
            let r = y[i] - fi;
            for j in 0..p {
                jtr[j] += w[i] * grad[j] * r;
                for k in j..p {
                    jtj[j * p + k] += w[i] * grad[j] * grad[k];
                }
            }
        }
        for j in 0..p {
            for k in 0..j {
                jtj[j * p + k] = jtj[k * p + j];
            }
        }
        let max_diag = (0..p).map(|j| jtj[j * p + j]).fold(0.0_f64, f64::max);
        let mut improved = false;
        while lambda <= 1e16 {
            let mut a = jtj.clone();
            for j in 0..p {
                a[j * p + j] += lambda * jtj[j * p + j].max(max_diag * 1e-12).max(1e-300);
            }
            let Some(delta) = linalg::solve(&a, &jtr, p) else {
                lambda *= 10.0;
                continue;
            };
            let trial: Vec<f64> = theta.iter().zip(&delta).map(|(t, d)| t + d).collect();
            let s_new = if ok(&trial) {
                sse_at(f, &trial, x, y, w)
            } else {
                f64::INFINITY
            };
            if s_new < sse {
                let rel = (sse - s_new) / sse.max(1e-300);
                let step: f64 = delta.iter().map(|d| d * d).sum::<f64>().sqrt();
                let size: f64 = theta.iter().map(|t| t * t).sum::<f64>().sqrt();
                theta = trial;
                sse = s_new;
                lambda = (lambda / 10.0).max(1e-12);
                improved = true;
                if rel < 1e-13 || step <= 1e-12 * (size + 1e-12) {
                    converged = true;
                }
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            // no step lowers the SSE: a minimum to working precision
            converged = true;
            break;
        }
        if converged || sse == 0.0 {
            converged = true;
            break;
        }
    }
    LmOutcome {
        theta,
        sse,
        converged,
        iterations: it,
    }
}

/// Means of y per distinct x, sorted by x.
fn level_means(x: &[f64], y: &[f64]) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = x.iter().copied().zip(y.iter().copied()).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64, usize)> = Vec::new();
    for (xi, yi) in pts {
        match out.last_mut() {
            Some(l) if l.0 == xi => {
                l.1 += yi;
                l.2 += 1;
            }
            _ => out.push((xi, yi, 1)),
        }
    }
    out.into_iter().map(|(x, s, k)| (x, s / k as f64)).collect()
}

/// Starting points for the 4PL: asymptotes from the lowest and highest concentration, c where
/// the level means cross half-way (log-interpolated), b from the logit linearisation, plus a
/// grid of slopes.
fn logistic_starts(x: &[f64], y: &[f64]) -> Vec<[f64; 4]> {
    let lm = level_means(x, y);
    let (y_lo, y_hi) = (lm[0].1, lm[lm.len() - 1].1);
    let span = y_hi - y_lo;
    let pos: Vec<(f64, f64)> = lm.iter().copied().filter(|(x, _)| *x > 0.0).collect();
    let (lx_min, lx_max) = (pos[0].0.ln(), pos[pos.len() - 1].0.ln());
    let a0 = y_lo - 0.05 * span;
    let d0 = y_hi + 0.05 * span;
    let half = f64::midpoint(a0, d0);
    let mut lnc0 = f64::midpoint(lx_min, lx_max);
    for win in pos.windows(2) {
        let ((x1, y1), (x2, y2)) = (win[0], win[1]);
        if (y1 - half) * (y2 - half) <= 0.0 && y1 != y2 {
            let f = (half - y1) / (y2 - y1);
            lnc0 = x1.ln() + f * (x2.ln() - x1.ln());
            break;
        }
    }
    let mut starts = Vec::new();
    // logit linearisation: ln((a − y)/(y − d)) = b (ln x − ln c)
    let (mut lxs, mut ts) = (Vec::new(), Vec::new());
    for (xi, yi) in &pos {
        let num = a0 - yi;
        let den = yi - d0;
        if num / den > 0.0 {
            lxs.push(xi.ln());
            ts.push((num / den).ln());
        }
    }
    if let Some((b, icpt, _)) = stats::ols(&lxs, &ts)
        && b.is_finite()
        && b.abs() > 1e-6
        && b.abs() < 50.0
    {
        starts.push([a0, b, -icpt / b, d0]);
    }
    for b in [1.0, 0.5, 2.0, 4.0] {
        starts.push([a0, b, lnc0, d0]);
    }
    starts
}

fn fit_logistic(model: Model, x: &[f64], y: &[f64], w: &[f64]) -> Result<FitResult, FitError> {
    let pos: Vec<f64> = x.iter().copied().filter(|v| *v > 0.0).collect();
    let lmin = pos.iter().copied().fold(f64::INFINITY, f64::min).ln();
    let lmax = pos.iter().copied().fold(f64::NEG_INFINITY, f64::max).ln();
    let bounds = Bounds {
        lnc: (lmin - 15.0, lmax + 15.0),
    };
    let mut best: Option<LmOutcome> = None;
    let consider = |o: LmOutcome, best: &mut Option<LmOutcome>| {
        let better = match best {
            None => true,
            Some(b) => {
                o.sse < b.sse * (1.0 - 1e-12) || (!b.converged && o.converged && o.sse <= b.sse)
            }
        };
        if o.sse.is_finite() && better {
            *best = Some(o);
        }
    };
    let model_fn = |t: &[f64], xi: f64, g: Option<&mut [f64]>| eval_logistic(t, xi, g);
    let ok = |t: &[f64]| bounds.ok(t);
    let starts4 = logistic_starts(x, y);
    let mut best4: Option<LmOutcome> = None;
    for s in &starts4 {
        let o = levenberg_marquardt(&model_fn, &ok, s.to_vec(), x, y, w);
        consider(o, &mut best4);
    }
    let best4 =
        best4.ok_or_else(|| FitError("the 4PL fit did not produce a finite curve".into()))?;
    if model == Model::FourPl {
        best = Some(best4);
    } else {
        // 5PL: from the 4PL optimum (g = 1), its mirror image (b → −b, a ↔ d) and g ∈ {0.5, 2}
        let t = &best4.theta;
        let mut starts5 = Vec::new();
        for lng in [0.0, 0.5f64.ln(), 2f64.ln()] {
            starts5.push(vec![t[0], t[1], t[2], t[3], lng]);
            starts5.push(vec![t[3], -t[1], t[2], t[0], lng]);
        }
        for s in starts5 {
            let o = levenberg_marquardt(&model_fn, &ok, s, x, y, w);
            consider(o, &mut best);
        }
    }
    let o = best.ok_or_else(|| FitError("the 5PL fit did not produce a finite curve".into()))?;
    let p = o.theta.len();
    let n = x.len();
    // covariance on the internal scale
    let mut jtj = vec![0.0; p * p];
    let mut grad = vec![0.0; p];
    for i in 0..n {
        eval_logistic(&o.theta, x[i], Some(&mut grad));
        for j in 0..p {
            for k in 0..p {
                jtj[j * p + k] += w[i] * grad[j] * grad[k];
            }
        }
    }
    let cov_int = (n > p)
        .then(|| linalg::invert(&jtj, p))
        .flatten()
        .map(|inv| {
            let s2 = o.sse / (n - p) as f64;
            inv.into_iter().map(|v| v * s2).collect::<Vec<f64>>()
        });
    // internal → reported: c = e^{ln c}, g = e^{ln g} (delta method: D Σ D)
    let mut params = o.theta.clone();
    params[2] = o.theta[2].exp();
    let mut dscale = vec![1.0; p];
    dscale[2] = params[2];
    if p == 5 {
        params[4] = o.theta[4].exp();
        dscale[4] = params[4];
    }
    let mut cov = cov_int.map(|c| {
        let mut out = vec![0.0; p * p];
        for j in 0..p {
            for k in 0..p {
                out[j * p + k] = c[j * p + k] * dscale[j] * dscale[k];
            }
        }
        out
    });
    // 4PL canonical form: b > 0 (swap a and d when the optimiser found b < 0; the same curve)
    if model == Model::FourPl && params[1] < 0.0 {
        params.swap(0, 3);
        params[1] = -params[1];
        if let Some(c) = cov.as_mut() {
            // T maps (a, b, c, d) → (d, −b, c, a): Σ' = T Σ Tᵀ
            let perm = [3usize, 1, 2, 0];
            let sign = [1.0, -1.0, 1.0, 1.0];
            let old = c.clone();
            for j in 0..4 {
                for k in 0..4 {
                    c[j * 4 + k] = sign[j] * sign[k] * old[perm[j] * 4 + perm[k]];
                }
            }
        }
    }
    Ok(FitResult {
        model,
        params,
        cov,
        sse: o.sse,
        n,
        df: 0,
        r_squared: 0.0,
        converged: o.converged,
        iterations: o.iterations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn four(a: f64, b: f64, c: f64, d: f64, x: f64) -> f64 {
        d + (a - d) / (1.0 + (x / c).powf(b))
    }

    #[test]
    fn exact_4pl_is_recovered() {
        let xs = [0.0, 0.1, 0.3, 1.0, 3.0, 10.0, 30.0, 100.0];
        let mut x = Vec::new();
        let mut y = Vec::new();
        for &c in &xs {
            for k in 0..2 {
                x.push(c);
                y.push(four(0.05, 1.3, 4.0, 2.5, c) + if k == 0 { 1e-4 } else { -1e-4 });
            }
        }
        let f = fit(Model::FourPl, &x, &y, Weighting::None).unwrap();
        assert!(f.converged);
        let p = &f.params;
        assert!(
            (p[0] - 0.05).abs() < 1e-3 && (p[1] - 1.3).abs() < 1e-3,
            "{p:?}"
        );
        assert!(
            (p[2] - 4.0).abs() < 1e-3 && (p[3] - 2.5).abs() < 1e-3,
            "{p:?}"
        );
        assert!(f.r_squared > 0.999_999);
        let x_back = f.inverse(four(0.05, 1.3, 4.0, 2.5, 7.0)).unwrap();
        assert!((x_back - 7.0).abs() < 1e-2);
        assert_eq!(f.inverse(3.0), Err(InverseFail::AboveCurve));
        assert_eq!(f.inverse(0.0), Err(InverseFail::BelowCurve));
    }

    #[test]
    fn decreasing_4pl_is_canonical() {
        let xs = [0.01, 0.1, 1.0, 10.0, 100.0, 1000.0];
        let x: Vec<f64> = xs.to_vec();
        let y: Vec<f64> = xs.iter().map(|&c| four(100.0, 1.0, 5.0, 2.0, c)).collect();
        let f = fit(Model::FourPl, &x, &y, Weighting::None).unwrap();
        assert!(f.params[1] > 0.0);
        assert!((f.params[0] - 100.0).abs() < 1e-4 && (f.params[3] - 2.0).abs() < 1e-4);
        assert!((f.params[2] - 5.0).abs() < 1e-5);
    }

    #[test]
    fn linear_fit() {
        let f = fit(
            Model::Linear,
            &[0.0, 1.0, 2.0, 3.0],
            &[1.0, 3.1, 4.9, 7.0],
            Weighting::None,
        )
        .unwrap();
        assert!((f.params[0] - 1.98).abs() < 1e-9);
        assert!(f.se(0).is_some());
        assert!((f.inverse(f.eval(1.5)).unwrap() - 1.5).abs() < 1e-12);
    }

    #[test]
    fn five_pl_recovers_asymmetry() {
        let g = 0.4;
        let xs: Vec<f64> = (0..12).map(|i| 0.01 * 3f64.powi(i)).collect();
        let y: Vec<f64> = xs
            .iter()
            .map(|&x| 2.0 + (0.1 - 2.0) / (1.0 + (x / 3.0).powf(1.5)).powf(g))
            .collect();
        let f = fit(Model::FivePl, &xs, &y, Weighting::None).unwrap();
        assert!(f.sse < 1e-12, "{f:?}");
        let back = f.inverse(f.eval(2.0)).unwrap();
        assert!((back - 2.0).abs() < 1e-6);
    }

    #[test]
    fn too_few_levels() {
        assert!(
            fit(
                Model::FourPl,
                &[1.0, 2.0, 3.0],
                &[1.0, 2.0, 3.0],
                Weighting::None
            )
            .is_err()
        );
        assert!(fit(Model::Linear, &[1.0, 1.0], &[1.0, 2.0], Weighting::None).is_err());
    }
}
