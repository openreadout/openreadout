//! Data-scale transforms: Gating-ML 2.0 `flin`, `flog`, `fasinh`, `logicle`, `hyperlog`,
//! `fratio`; FlowJo's `log` and `biex`; and the cofactor arcsinh common in mass cytometry.
//!
//! Derivation: `docs/provenance/flowjo-wsp.md` (Gating-ML 2.0 definitions; Moore & Parks 2012
//! for logicle; FlowKit's BSD-3 port of the FlowJo biex table).

// Numerical code written in the notation of its sources (Moore & Parks 2012, Gating-ML 2.0),
// with exact float comparisons where an iteration checks for a fixed point.
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use std::f64::consts::LN_10;

/// One transform, with the parameters the file (or the command line) gives.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Transform {
    /// Gating-ML `flin`: `(x + A) / (T + A)`. FlowJo `linear` elements map to `T = maxRange`,
    /// `A = minRange`.
    Linear {
        /// Top of scale.
        t: f64,
        /// Offset.
        a: f64,
    },
    /// Gating-ML `flog`: `log10(x / T) / M + 1` (not a number for `x <= 0`).
    Log {
        /// Top of scale.
        t: f64,
        /// Decades.
        m: f64,
    },
    /// Gating-ML `fasinh`: `(asinh(x · sinh(M·ln10) / T) + A·ln10) / ((M + A)·ln10)`.
    Asinh {
        /// Top of scale.
        t: f64,
        /// Decades.
        m: f64,
        /// Additional negative decades.
        a: f64,
    },
    /// Gating-ML `logicle` (Moore & Parks 2012).
    Logicle {
        /// Top of scale.
        t: f64,
        /// Linearization width in decades.
        w: f64,
        /// Decades.
        m: f64,
        /// Additional negative decades.
        a: f64,
    },
    /// Gating-ML `hyperlog`.
    Hyperlog {
        /// Top of scale.
        t: f64,
        /// Linearization width in decades.
        w: f64,
        /// Decades.
        m: f64,
        /// Additional negative decades.
        a: f64,
    },
    /// Gating-ML `fratio` over two parameters: `A · (x − B) / (y − C)`. Defines a new dimension.
    Ratio {
        /// Numerator parameter.
        numerator: String,
        /// Denominator parameter.
        denominator: String,
        /// Scale.
        a: f64,
        /// Numerator offset.
        b: f64,
        /// Denominator offset.
        c: f64,
    },
    /// FlowJo `log`: `(log10(max(x, offset)) − log10(offset)) / decades`.
    FlowJoLog {
        /// Smallest value kept on scale.
        offset: f64,
        /// Decades.
        decades: f64,
    },
    /// FlowJo `biex`: a 4097-point lookup table, linearly interpolated, output 0–4096.
    FlowJoBiex {
        /// Extra negative decades (`neg`).
        negative: f64,
        /// Width basis (`width`, negative, e.g. −10).
        width: f64,
        /// Positive decades (`pos`).
        positive: f64,
        /// Top of the linear scale (`maxRange`).
        max_value: f64,
    },
    /// `asinh(x / cofactor)`: the arcsinh convention of mass cytometry (cofactor 5) and of
    /// some flow pipelines (cofactor 150). Not a Gating-ML transform.
    AsinhCofactor {
        /// Divisor applied before the arcsinh.
        cofactor: f64,
    },
}

impl Transform {
    /// Short name used in JSON (`linear`, `log`, `arcsinh`, `logicle`, `hyperlog`, `ratio`,
    /// `flowjo-log`, `flowjo-biex`, `arcsinh-cofactor`).
    pub fn kind(&self) -> &'static str {
        match self {
            Transform::Linear { .. } => "linear",
            Transform::Log { .. } => "log",
            Transform::Asinh { .. } => "arcsinh",
            Transform::Logicle { .. } => "logicle",
            Transform::Hyperlog { .. } => "hyperlog",
            Transform::Ratio { .. } => "ratio",
            Transform::FlowJoLog { .. } => "flowjo-log",
            Transform::FlowJoBiex { .. } => "flowjo-biex",
            Transform::AsinhCofactor { .. } => "arcsinh-cofactor",
        }
    }

    /// Parameters as `name → value` pairs (JSON).
    pub fn parameters(&self) -> Vec<(&'static str, f64)> {
        match *self {
            Transform::Linear { t, a } => vec![("T", t), ("A", a)],
            Transform::Log { t, m } => vec![("T", t), ("M", m)],
            Transform::Asinh { t, m, a } => vec![("T", t), ("M", m), ("A", a)],
            Transform::Logicle { t, w, m, a } | Transform::Hyperlog { t, w, m, a } => {
                vec![("T", t), ("W", w), ("M", m), ("A", a)]
            }
            Transform::Ratio { a, b, c, .. } => vec![("A", a), ("B", b), ("C", c)],
            Transform::FlowJoLog { offset, decades } => {
                vec![("offset", offset), ("decades", decades)]
            }
            Transform::FlowJoBiex {
                negative,
                width,
                positive,
                max_value,
            } => vec![
                ("neg", negative),
                ("width", width),
                ("pos", positive),
                ("maxRange", max_value),
            ],
            Transform::AsinhCofactor { cofactor } => vec![("cofactor", cofactor)],
        }
    }

    /// Check the parameters and precompute what the transform needs.
    pub fn prepare(&self) -> Result<Prepared, String> {
        let pos = |name: &str, v: f64| {
            if v.is_finite() && v > 0.0 {
                Ok(())
            } else {
                Err(format!(
                    "{} {name} must be a positive number, got {v}",
                    self.kind()
                ))
            }
        };
        let finite = |name: &str, v: f64| {
            if v.is_finite() {
                Ok(())
            } else {
                Err(format!("{} {name} must be a finite number", self.kind()))
            }
        };
        Ok(match *self {
            Transform::Linear { t, a } => {
                finite("T", t)?;
                finite("A", a)?;
                if t + a == 0.0 {
                    return Err("linear: T + A must not be 0".into());
                }
                Prepared::Linear { t, a }
            }
            Transform::Log { t, m } => {
                pos("T", t)?;
                pos("M", m)?;
                Prepared::Log { t, m }
            }
            Transform::Asinh { t, m, a } => {
                pos("T", t)?;
                pos("M", m)?;
                finite("A", a)?;
                if m + a <= 0.0 {
                    return Err("arcsinh: M + A must be positive".into());
                }
                Prepared::Asinh {
                    pre_scale: (m * LN_10).sinh() / t,
                    shift: a * LN_10,
                    divisor: (m + a) * LN_10,
                }
            }
            Transform::Logicle { t, w, m, a } => {
                pos("T", t)?;
                pos("M", m)?;
                finite("W", w)?;
                finite("A", a)?;
                if w < 0.0 || 2.0 * w > m || -a > w || a + w > m - w {
                    return Err(format!(
                        "logicle parameters out of range (need 0 <= W <= M/2, -W <= A <= M - 2W): T={t} W={w} M={m} A={a}"
                    ));
                }
                Prepared::Logicle(Box::new(Biexponential::logicle(t, w, m, a)))
            }
            Transform::Hyperlog { t, w, m, a } => {
                pos("T", t)?;
                pos("M", m)?;
                pos("W", w)?;
                finite("A", a)?;
                if 2.0 * w > m || -a > w || a + w > m - w {
                    return Err(format!(
                        "hyperlog parameters out of range: T={t} W={w} M={m} A={a}"
                    ));
                }
                Prepared::Hyperlog(Box::new(Biexponential::hyperlog(t, w, m, a)))
            }
            Transform::Ratio { a, b, c, .. } => {
                finite("A", a)?;
                finite("B", b)?;
                finite("C", c)?;
                Prepared::Ratio { a, b, c }
            }
            Transform::FlowJoLog { offset, decades } => {
                pos("offset", offset)?;
                pos("decades", decades)?;
                Prepared::FlowJoLog { offset, decades }
            }
            Transform::FlowJoBiex {
                negative,
                width,
                positive,
                max_value,
            } => {
                finite("neg", negative)?;
                pos("pos", positive)?;
                pos("maxRange", max_value)?;
                if !(width.is_finite() && width < 0.0) {
                    return Err(format!("flowjo-biex width must be negative, got {width}"));
                }
                Prepared::FlowJoBiex(Box::new(biex_table(
                    4096, positive, negative, width, max_value,
                )?))
            }
            Transform::AsinhCofactor { cofactor } => {
                pos("cofactor", cofactor)?;
                Prepared::AsinhCofactor { cofactor }
            }
        })
    }
}

/// A transform ready to apply (constants and tables computed once).
#[derive(Debug, Clone)]
pub enum Prepared {
    #[doc(hidden)]
    Linear { t: f64, a: f64 },
    #[doc(hidden)]
    Log { t: f64, m: f64 },
    #[doc(hidden)]
    Asinh {
        pre_scale: f64,
        shift: f64,
        divisor: f64,
    },
    #[doc(hidden)]
    Logicle(Box<Biexponential>),
    #[doc(hidden)]
    Hyperlog(Box<Biexponential>),
    #[doc(hidden)]
    Ratio { a: f64, b: f64, c: f64 },
    #[doc(hidden)]
    FlowJoLog { offset: f64, decades: f64 },
    #[doc(hidden)]
    FlowJoBiex(Box<BiexTable>),
    #[doc(hidden)]
    AsinhCofactor { cofactor: f64 },
}

impl Prepared {
    /// Transform one value (not for `Ratio`, which needs two; see [`Prepared::ratio`]).
    pub fn apply(&self, x: f64) -> f64 {
        match self {
            Prepared::Linear { t, a } => (x + a) / (t + a),
            Prepared::Log { t, m } => (x / t).log10() / m + 1.0,
            Prepared::Asinh {
                pre_scale,
                shift,
                divisor,
            } => ((x * pre_scale).asinh() + shift) / divisor,
            Prepared::Logicle(b) => b.logicle_scale(x),
            Prepared::Hyperlog(b) => b.hyperlog_scale(x),
            Prepared::Ratio { .. } => f64::NAN,
            Prepared::FlowJoLog { offset, decades } => {
                (x.max(*offset).log10() - offset.log10()) / decades
            }
            Prepared::FlowJoBiex(t) => t.apply(x),
            Prepared::AsinhCofactor { cofactor } => (x / cofactor).asinh(),
        }
    }

    /// The Gating-ML ratio of two values.
    pub fn ratio(&self, x: f64, y: f64) -> f64 {
        match self {
            Prepared::Ratio { a, b, c } => a * (x - b) / (y - c),
            _ => f64::NAN,
        }
    }

    /// Top of the transformed display range used by FlowJo's 256-bin ellipse coordinates: 4096
    /// for FlowJo biex (its table output), 1 for everything else.
    pub fn display_range(&self) -> f64 {
        match self {
            Prepared::FlowJoBiex(_) => 4096.0,
            _ => 1.0,
        }
    }
}

const TAYLOR_LENGTH: usize = 16;

/// Constants of the logicle / hyperlog functions (Moore & Parks 2012 notation).
#[derive(Debug, Clone)]
pub struct Biexponential {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    f: f64,
    w: f64,
    x1: f64,
    x_taylor: f64,
    taylor: [f64; TAYLOR_LENGTH],
    /// Hyperlog only: data value at `x0`, where the initial guess switches to logarithmic.
    inverse_x0: f64,
}

/// Root `d` of `2 (ln d − ln b) + w (b + d) = 0` in `(0, b]` (bracketed Newton, after RTSAFE).
fn solve_d(b: f64, w: f64) -> f64 {
    if w == 0.0 {
        return b;
    }
    let tolerance = 2.0 * b * f64::EPSILON;
    let (mut d_lo, mut d_hi) = (0.0_f64, b);
    let mut d = f64::midpoint(d_lo, d_hi);
    let mut last_delta = d_hi - d_lo;
    let f_b = -2.0 * b.ln() + w * b;
    let mut f = 2.0 * d.ln() + w * d + f_b;
    let mut last_f = f64::NAN;
    for _ in 1..40 {
        let df = 2.0 / d + w;
        let delta;
        if ((d - d_hi) * df - f) * ((d - d_lo) * df - f) >= 0.0
            || (1.9 * f).abs() > (last_delta * df).abs()
        {
            delta = (d_hi - d_lo) / 2.0;
            d = d_lo + delta;
            if d == d_lo {
                return d;
            }
        } else {
            delta = f / df;
            let t = d;
            d -= delta;
            if d == t {
                return d;
            }
        }
        if delta.abs() < tolerance {
            return d;
        }
        last_delta = delta;
        f = 2.0 * d.ln() + w * d + f_b;
        if f == 0.0 || f == last_f {
            return d;
        }
        last_f = f;
        if f < 0.0 {
            d_lo = d;
        } else {
            d_hi = d;
        }
    }
    d
}

impl Biexponential {
    fn logicle(t: f64, w_dec: f64, m: f64, a_dec: f64) -> Self {
        let w = w_dec / (m + a_dec);
        let x2 = a_dec / (m + a_dec);
        let x1 = x2 + w;
        let x0 = x2 + 2.0 * w;
        let b = (m + a_dec) * LN_10;
        let d = solve_d(b, w);
        let c_a = (x0 * (b + d)).exp();
        let mf_a = (b * x1).exp() - c_a / (d * x1).exp();
        let a = t / ((b.exp() - mf_a) - c_a / d.exp());
        let c = c_a * a;
        let f = -mf_a * a;
        let mut taylor = [0.0; TAYLOR_LENGTH];
        let mut pos_coef = a * (b * x1).exp();
        let mut neg_coef = -c / (d * x1).exp();
        for (i, slot) in taylor.iter_mut().enumerate() {
            pos_coef *= b / (i as f64 + 1.0);
            neg_coef *= -d / (i as f64 + 1.0);
            *slot = pos_coef + neg_coef;
        }
        taylor[1] = 0.0; // the logicle condition makes it exactly zero
        Biexponential {
            a,
            b,
            c,
            d,
            f,
            w,
            x1,
            x_taylor: x1 + w / 4.0,
            taylor,
            inverse_x0: 0.0,
        }
    }

    fn hyperlog(t: f64, w_dec: f64, m: f64, a_dec: f64) -> Self {
        let w = w_dec / (m + a_dec);
        let x2 = a_dec / (m + a_dec);
        let x1 = x2 + w;
        let x0 = x2 + 2.0 * w;
        let b = (m + a_dec) * LN_10;
        let e0 = (b * x0).exp();
        let c_a = e0 / w;
        let f_a = (b * x1).exp() + c_a * x1;
        let a = t / (b.exp() + c_a - f_a);
        let c = c_a * a;
        let f = f_a * a;
        let mut taylor = [0.0; TAYLOR_LENGTH];
        let mut coef = a * (b * x1).exp();
        for (i, slot) in taylor.iter_mut().enumerate() {
            coef *= b / (i as f64 + 1.0);
            *slot = coef;
        }
        taylor[0] += c;
        let mut me = Biexponential {
            a,
            b,
            c,
            d: 0.0,
            f,
            w,
            x1,
            x_taylor: x1 + w / 4.0,
            taylor,
            inverse_x0: 0.0,
        };
        let negative = x0 < x1;
        let x0r = if negative { 2.0 * x1 - x0 } else { x0 };
        let mut inv = if x0r < me.x_taylor {
            me.series_logicle(x0r)
        } else {
            a * (b * x0r).exp() + c * x0r
        };
        if negative {
            inv = -inv;
        }
        me.inverse_x0 = inv;
        me
    }

    /// Taylor series around `x1` without the first-order term (logicle; `taylor[1] == 0`).
    fn series_logicle(&self, scale: f64) -> f64 {
        let x = scale - self.x1;
        let mut sum = self.taylor[TAYLOR_LENGTH - 1] * x;
        for i in (2..TAYLOR_LENGTH - 1).rev() {
            sum = (sum + self.taylor[i]) * x;
        }
        (sum * x + self.taylor[0]) * x
    }

    /// Full Taylor series around `x1` (hyperlog).
    fn series_full(&self, scale: f64) -> f64 {
        let x = scale - self.x1;
        let mut sum = self.taylor[TAYLOR_LENGTH - 1] * x;
        for i in (0..TAYLOR_LENGTH - 1).rev() {
            sum = (sum + self.taylor[i]) * x;
        }
        sum
    }

    /// Logicle value of a data value (Halley's method on the biexponential).
    pub fn logicle_scale(&self, value: f64) -> f64 {
        if value == 0.0 {
            return self.x1;
        }
        if !value.is_finite() {
            return f64::NAN;
        }
        let negative = value < 0.0;
        let value = value.abs();
        let mut x = if value < self.f {
            self.x1 + value / self.taylor[0]
        } else {
            (value / self.a).ln() / self.b
        };
        let tolerance = if x > 1.0 {
            3.0 * x * f64::EPSILON
        } else {
            3.0 * f64::EPSILON
        };
        for _ in 0..40 {
            let ae2bx = self.a * (self.b * x).exp();
            let ce2mdx = self.c / (self.d * x).exp();
            let y = if x < self.x_taylor {
                self.series_logicle(x) - value
            } else {
                (ae2bx + self.f) - (ce2mdx + value)
            };
            let abe2bx = self.b * ae2bx;
            let cde2mdx = self.d * ce2mdx;
            let dy = abe2bx + cde2mdx;
            let ddy = self.b * abe2bx - self.d * cde2mdx;
            let delta = y / (dy * (1.0 - y * ddy / (2.0 * dy * dy)));
            x -= delta;
            if delta.abs() < tolerance {
                return if negative { 2.0 * self.x1 - x } else { x };
            }
        }
        f64::NAN
    }

    /// Data value of a logicle scale value.
    pub fn logicle_inverse(&self, scale: f64) -> f64 {
        let negative = scale < self.x1;
        let s = if negative {
            2.0 * self.x1 - scale
        } else {
            scale
        };
        let v = if s < self.x_taylor {
            self.series_logicle(s)
        } else {
            (self.a * (self.b * s).exp() + self.f) - self.c / (self.d * s).exp()
        };
        if negative { -v } else { v }
    }

    /// Hyperlog value of a data value.
    pub fn hyperlog_scale(&self, value: f64) -> f64 {
        if value == 0.0 {
            return self.x1;
        }
        if !value.is_finite() {
            return f64::NAN;
        }
        let negative = value < 0.0;
        let value = value.abs();
        let mut x = if value < self.inverse_x0 {
            self.x1 + value * self.w / self.inverse_x0
        } else {
            (value / self.a).ln() / self.b
        };
        let tolerance = 3.0 * f64::EPSILON;
        for _ in 0..10 {
            let ae2bx = self.a * (self.b * x).exp();
            let y = if x < self.x_taylor {
                self.series_full(x) - value
            } else {
                (ae2bx + self.c * x) - (self.f + value)
            };
            let abe2bx = self.b * ae2bx;
            let dy = abe2bx + self.c;
            let ddy = self.b * abe2bx;
            let delta = y / (dy * (1.0 - y * ddy / (2.0 * dy * dy)));
            x -= delta;
            if delta.abs() < tolerance {
                return if negative { 2.0 * self.x1 - x } else { x };
            }
        }
        // Same outcome as the reference implementation, which gives up after ten steps.
        f64::NAN
    }
}

/// FlowJo's biex lookup table: sorted data values and their channel (0–4096).
#[derive(Debug, Clone)]
pub struct BiexTable {
    input: Vec<f64>,
    output: Vec<f64>,
}

impl BiexTable {
    /// Linear interpolation, clamped to the table's output range.
    pub fn apply(&self, x: f64) -> f64 {
        if x.is_nan() {
            return f64::NAN;
        }
        let n = self.input.len();
        if n == 0 {
            return f64::NAN;
        }
        if x < self.input[0] {
            return self.output[0];
        }
        if x > self.input[n - 1] {
            return self.output[n - 1];
        }
        // First index whose input is >= x.
        let hi = self.input.partition_point(|&v| v < x);
        if hi == 0 {
            return self.output[0];
        }
        let lo = hi - 1;
        let (x0, x1) = (self.input[lo], self.input[hi]);
        let (y0, y1) = (self.output[lo], self.output[hi]);
        if x1 == x0 {
            return y1;
        }
        y0 + (x - x0) * (y1 - y0) / (x1 - x0)
    }
}

/// Root of `2 ln d + w d − 2 ln b + w b = 0` as FlowJo's table builder finds it.
fn biex_log_root(b: f64, w: f64) -> f64 {
    if w == 0.0 {
        return b;
    }
    let (mut x_lo, mut x_hi) = (0.0_f64, b);
    let mut d = f64::midpoint(x_lo, x_hi);
    let mut dx_last = (x_lo - x_hi).trunc().abs();
    let fb = -2.0 * b.ln() + w * b;
    let mut f = 2.0 * d.ln() + w * b + fb;
    let mut df = 2.0 / d + w;
    for _ in 0..100 {
        let dx;
        if (((d - x_hi) * df - f) - ((d - x_lo) * df - f)) > 0.0
            || (2.0 * f).abs() > (dx_last * df).abs()
        {
            dx = (x_hi - x_lo) / 2.0;
            d = x_lo + dx;
            if d == x_lo {
                return d;
            }
        } else {
            dx = f / df;
            let t = d;
            d -= dx;
            if d == t {
                return d;
            }
        }
        if dx.abs() < 1.0e-12 {
            return d;
        }
        dx_last = dx;
        f = 2.0 * d.ln() + w * d + fb;
        df = 2.0 / d + w;
        if f < 0.0 {
            x_lo = d;
        } else {
            x_hi = d;
        }
    }
    d
}

/// Build FlowJo's biex table (`channel_range` = 4096 in every workspace seen).
fn biex_table(
    channel_range: usize,
    pos: f64,
    neg: f64,
    width_basis: f64,
    max_value: f64,
) -> Result<BiexTable, String> {
    let range = channel_range as f64;
    let mut decades = pos;
    let mut width = (-width_basis).log10();
    decades -= width / 2.0;
    let extra = neg.max(0.0) + width / 2.0;
    let mut zero_point = ((extra * range) / (extra + decades)).trunc();
    zero_point = zero_point.min(range / 2.0).trunc();
    if !(zero_point.is_finite() && zero_point >= 0.0) {
        return Err(format!(
            "flowjo-biex parameters give no zero point (pos {pos}, neg {neg}, width {width_basis})"
        ));
    }
    let zp = zero_point as usize;
    if zp > 0 {
        decades = extra * range / zero_point;
    }
    width /= 2.0 * decades;
    let positive_range = LN_10 * decades;
    let minimum = max_value / positive_range.exp();
    let negative_range = biex_log_root(positive_range, width);
    let n_points = channel_range + 1;
    let n = n_points as f64;
    let mut positive: Vec<f64> = (0..n_points)
        .map(|i| (i as f64 / n * positive_range).exp())
        .collect();
    let mut negative: Vec<f64> = (0..n_points)
        .map(|i| (i as f64 / n * -negative_range).exp())
        .collect();
    let s = ((positive_range + negative_range) * (width + extra / decades)).exp();
    for v in &mut negative {
        *v *= s;
    }
    if zp >= n_points || 2 * zp >= n_points {
        return Err("flowjo-biex zero point outside the table".into());
    }
    let s = positive[zp] - negative[zp];
    for i in zp..n_points {
        positive[i] = minimum * ((positive[i] - negative[i]) - s);
    }
    for i in 0..zp {
        positive[i] = -positive[2 * zp - i];
    }
    let output: Vec<f64> = (0..n_points).map(|i| i as f64).collect();
    if positive
        .windows(2)
        .any(|w| w[0].is_nan() || w[1].is_nan() || w[0] > w[1])
    {
        return Err(format!(
            "flowjo-biex table is not increasing (pos {pos}, neg {neg}, width {width_basis})"
        ));
    }
    Ok(BiexTable {
        input: positive,
        output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * a.abs().max(b.abs()).max(1e-12)
    }

    #[test]
    fn logicle_round_trip_and_anchors() {
        let t = Transform::Logicle {
            t: 262_144.0,
            w: 0.5,
            m: 4.5,
            a: 0.0,
        };
        let Prepared::Logicle(b) = t.prepare().unwrap() else {
            panic!()
        };
        assert!(close(b.logicle_scale(262_144.0), 1.0, 1e-12));
        let zero = b.logicle_scale(0.0);
        assert!(close(zero, 0.5 / 4.5, 1e-12));
        for v in [-500.0, -1.0, 0.3, 17.0, 1000.0, 1.0e5] {
            let y = b.logicle_scale(v);
            assert!(close(b.logicle_inverse(y), v, 1e-9), "{v}");
        }
    }

    #[test]
    fn asinh_and_log_and_linear() {
        let p = Transform::Asinh {
            t: 262_144.0,
            m: 4.5,
            a: 0.0,
        }
        .prepare()
        .unwrap();
        assert!(close(p.apply(262_144.0), 1.0, 1e-12));
        assert_eq!(p.apply(0.0), 0.0);
        let p = Transform::Log {
            t: 10_000.0,
            m: 5.0,
        }
        .prepare()
        .unwrap();
        assert!(close(p.apply(10_000.0), 1.0, 1e-15));
        assert!(p.apply(-1.0).is_nan());
        let p = Transform::Linear {
            t: 10_000.0,
            a: 500.0,
        }
        .prepare()
        .unwrap();
        assert!(close(p.apply(9_500.0), 10_000.0 / 10_500.0, 1e-15));
    }

    #[test]
    fn biex_table_is_monotone_and_anchored() {
        let p = Transform::FlowJoBiex {
            negative: 0.0,
            width: -10.0,
            positive: 4.418_54,
            max_value: 262_144.0,
        }
        .prepare()
        .unwrap();
        assert_eq!(p.apply(-1.0e9), 0.0);
        assert_eq!(p.apply(1.0e12), 4096.0);
        let a = p.apply(1000.0);
        let b = p.apply(10_000.0);
        assert!(a < b);
        assert!(close(p.apply(262_144.0), 4096.0, 1e-3));
    }

    #[test]
    fn hyperlog_zero_and_top() {
        let t = Transform::Hyperlog {
            t: 10_000.0,
            w: 1.0,
            m: 4.5,
            a: 0.0,
        };
        let p = t.prepare().unwrap();
        assert!(close(p.apply(0.0), 1.0 / 4.5, 1e-12));
        assert!(close(p.apply(10_000.0), 1.0, 1e-9));
    }

    #[test]
    fn rejects_bad_parameters() {
        assert!(
            Transform::Logicle {
                t: 1.0,
                w: 3.0,
                m: 4.5,
                a: 0.0
            }
            .prepare()
            .is_err()
        );
        assert!(Transform::Log { t: 0.0, m: 1.0 }.prepare().is_err());
        assert!(
            Transform::AsinhCofactor { cofactor: 0.0 }
                .prepare()
                .is_err()
        );
    }
}
