//! Window functions for one dimension of a multidimensional data set.
//!
//! A window is a list of weights for the `n` points of an interferogram counted from the true
//! time zero (`t = i·dwell`, `i = 0 … n − 1`); the acquisition time is `T = n·dwell`. With
//! `x = i/n`:
//!
//! | variant | weight |
//! | --- | --- |
//! | `Exponential { line_broadening_hz }` | `e^{−π·LB·t}` |
//! | `LorentzToGauss { line_broadening_hz, gaussian_position }` | `e^{−a·t − b·t²}`, `a = π·LB`, `b = −a / (2·G·T)` (LB < 0 narrows the lines; the Gaussian peaks at `G·T`) |
//! | `Gaussian { line_broadening_hz }` | `e^{−(π·GB·t)²/(4 ln 2)}` (a Gaussian line of `GB` Hz full width at half height) |
//! | `Sine { offset, power }` | `sin(π·offset + π·(1 − offset)·x)^power` (offset 0: sine bell, 0.5: cosine bell) |
//! | `Trapezoid { rise, fall }` | rises linearly from 0 to 1 over `x ∈ [0, rise]`, 1 until `fall`, falls to 0 at `x = 1` |
//! | `Sinc { center, width, power }` | `sinc(width·(x − center))^power`, `sinc(y) = sin(πy)/(πy)` |
//! | `Traficante { line_broadening_hz }` | `E(t) / (E(t)² + E(T − t)²)`, `E(t) = e^{−π·LB·t}` (Traficante and Ziessow 1986), scaled to 1 at `t = 0` |
//!
//! Bruker `WDW` codes map to these (see `crate::nmr::indirect`): 1 EM → `Exponential`, 2 GM →
//! `LorentzToGauss` (`LB`, `GB`), 3 SINE and 4 QSINE → `Sine` with offset `1/SSB` (`SSB` < 2:
//! 0) and power 1 or 2, 5 TRAP → `Trapezoid` (`TM1`, `TM2`), 7 SINC and 8 QSINC → `Sinc`,
//! 9 TRAF → `Traficante`. Sine windows were checked against TopSpin spectra (corpus 2D
//! experiments); the other shapes follow their published definitions and were not compared with
//! vendor spectra.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A window function (module docs).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Window {
    /// No window (all weights 1).
    None,
    /// `e^{−π·LB·t}`.
    Exponential {
        /// Added Lorentzian full width at half height, Hz.
        line_broadening_hz: f64,
    },
    /// `e^{−(π·GB·t)²/(4 ln 2)}`.
    Gaussian {
        /// Gaussian full width at half height, Hz.
        line_broadening_hz: f64,
    },
    /// Lorentz-to-Gauss transformation `e^{−a·t − b·t²}`.
    LorentzToGauss {
        /// Lorentzian width removed (negative) or added, Hz.
        line_broadening_hz: f64,
        /// Where the Gaussian peaks, as a fraction of the acquisition time (0 … 1).
        gaussian_position: f64,
    },
    /// Shifted sine bell raised to `power`.
    Sine {
        /// Start phase as a fraction of π (0 = sine, 0.5 = cosine).
        offset: f64,
        /// Exponent (1 = sine bell, 2 = squared sine bell).
        power: f64,
    },
    /// Trapezoid.
    Trapezoid {
        /// End of the rising edge, fraction of the acquisition time.
        rise: f64,
        /// Start of the falling edge, fraction of the acquisition time.
        fall: f64,
    },
    /// `sinc(width·(x − center))^power`.
    Sinc {
        /// Centre, fraction of the acquisition time.
        center: f64,
        /// Number of sinc lobes across the acquisition.
        width: f64,
        /// Exponent (1 or 2).
        power: f64,
    },
    /// Traficante–Ziessow window.
    Traficante {
        /// Line broadening of the underlying exponential, Hz.
        line_broadening_hz: f64,
    },
}

impl Window {
    /// The weights of `n` points sampled every `dwell_s` seconds (module docs).
    pub fn weights(&self, n: usize, dwell_s: f64) -> Vec<f64> {
        let nf = n.max(1) as f64;
        let t = |i: usize| i as f64 * dwell_s;
        let x = |i: usize| i as f64 / nf;
        let pi = std::f64::consts::PI;
        let w: Vec<f64> = match *self {
            Window::None => vec![1.0; n],
            Window::Exponential { line_broadening_hz } => (0..n)
                .map(|i| (-pi * line_broadening_hz * t(i)).exp())
                .collect(),
            Window::Gaussian { line_broadening_hz } => {
                let c = pi * line_broadening_hz;
                let d = 4.0 * std::f64::consts::LN_2;
                (0..n)
                    .map(|i| {
                        let a = c * t(i);
                        (-(a * a) / d).exp()
                    })
                    .collect()
            }
            Window::LorentzToGauss {
                line_broadening_hz,
                gaussian_position,
            } => {
                let a = pi * line_broadening_hz;
                let aq = nf * dwell_s;
                let b = if gaussian_position > 0.0 && aq > 0.0 {
                    -a / (2.0 * gaussian_position * aq)
                } else {
                    0.0
                };
                (0..n)
                    .map(|i| {
                        let ti = t(i);
                        (-a * ti - b * ti * ti).exp()
                    })
                    .collect()
            }
            Window::Sine { offset, power } => {
                let o = offset.clamp(0.0, 1.0);
                (0..n)
                    .map(|i| {
                        let s = (pi * o + pi * (1.0 - o) * x(i)).sin().max(0.0);
                        s.powf(power)
                    })
                    .collect()
            }
            Window::Trapezoid { rise, fall } => {
                let (r, f) = (rise.clamp(0.0, 1.0), fall.clamp(0.0, 1.0).max(rise));
                (0..n)
                    .map(|i| {
                        let xi = x(i);
                        if xi < r {
                            xi / r
                        } else if xi <= f {
                            1.0
                        } else if f < 1.0 {
                            (1.0 - xi) / (1.0 - f)
                        } else {
                            1.0
                        }
                    })
                    .collect()
            }
            Window::Sinc {
                center,
                width,
                power,
            } => (0..n)
                .map(|i| {
                    let y = width * (x(i) - center);
                    let s = if y.abs() < 1e-12 {
                        1.0
                    } else {
                        (pi * y).sin() / (pi * y)
                    };
                    if (power - 1.0).abs() < f64::EPSILON {
                        s
                    } else {
                        s.abs().powf(power)
                    }
                })
                .collect(),
            Window::Traficante { line_broadening_hz } => {
                let aq = nf * dwell_s;
                let e = |ti: f64| (-pi * line_broadening_hz * ti).exp();
                let raw = |ti: f64| {
                    let (a, b) = (e(ti), e(aq - ti));
                    let d = a * a + b * b;
                    if d > 0.0 { a / d } else { 0.0 }
                };
                let w0 = raw(0.0);
                let norm = if w0 > 0.0 && w0.is_finite() { w0 } else { 1.0 };
                (0..n).map(|i| raw(t(i)) / norm).collect()
            }
        };
        w.into_iter()
            .map(|v| if v.is_finite() { v } else { 0.0 })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_have_their_defining_values() {
        let n = 100;
        let s = Window::Sine {
            offset: 0.0,
            power: 1.0,
        }
        .weights(n, 1e-3);
        assert!(s[0].abs() < 1e-12 && (s[50] - 1.0).abs() < 1e-12);
        let c = Window::Sine {
            offset: 0.5,
            power: 2.0,
        }
        .weights(n, 1e-3);
        assert!((c[0] - 1.0).abs() < 1e-12 && c[99] < 1e-3);
        let e = Window::Exponential {
            line_broadening_hz: 1.0,
        }
        .weights(n, 0.01);
        assert!((e[10] - (-std::f64::consts::PI * 0.1).exp()).abs() < 1e-12);
        let t = Window::Trapezoid {
            rise: 0.1,
            fall: 0.8,
        }
        .weights(n, 1e-3);
        assert!(
            (t[5] - 0.5).abs() < 1e-12 && (t[50] - 1.0).abs() < 1e-12 && (t[90] - 0.5).abs() < 1e-9
        );
        let g = Window::LorentzToGauss {
            line_broadening_hz: -2.0,
            gaussian_position: 0.3,
        }
        .weights(n, 0.01);
        let peak = g
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(peak, 30);
        let tr = Window::Traficante {
            line_broadening_hz: 3.0,
        }
        .weights(n, 0.01);
        assert!((tr[0] - 1.0).abs() < 1e-12 && tr.iter().all(|v| v.is_finite()));
        let si = Window::Sinc {
            center: 0.0,
            width: 2.0,
            power: 1.0,
        }
        .weights(n, 1e-3);
        assert!((si[0] - 1.0).abs() < 1e-12 && si[50].abs() < 1e-12);
        assert_eq!(Window::None.weights(3, 1.0), vec![1.0; 3]);
        assert!(Window::None.weights(0, 1.0).is_empty());
    }
}
