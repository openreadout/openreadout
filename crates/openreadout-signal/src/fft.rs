//! A minimal complex type and an in-place radix-2 fast Fourier transform.
//!
//! The transform is the textbook iterative Cooley–Tukey algorithm (bit-reversal permutation,
//! then butterflies with twiddles computed per stage from `sin`/`cos` of the exact angle, which
//! keeps the error at the level of a direct DFT for the sizes NMR uses, up to 2^22 points).
//! Lengths must be powers of two: NMR processing zero-fills to one anyway.

use std::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub};

/// A complex number of two `f64`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Complex {
    /// `re + i·im`.
    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }
    /// `e^{iθ}`.
    pub fn from_phase(theta: f64) -> Self {
        let (s, c) = theta.sin_cos();
        Self { re: c, im: s }
    }
    /// Complex conjugate.
    pub fn conj(self) -> Self {
        Self {
            re: self.re,
            im: -self.im,
        }
    }
    /// Modulus.
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    /// Multiply by a real number.
    pub fn scale(self, k: f64) -> Self {
        Self {
            re: self.re * k,
            im: self.im * k,
        }
    }
}

impl Add for Complex {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.re + o.re, self.im + o.im)
    }
}
impl AddAssign for Complex {
    fn add_assign(&mut self, o: Self) {
        self.re += o.re;
        self.im += o.im;
    }
}
impl Sub for Complex {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for Complex {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Self::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
}
impl MulAssign for Complex {
    fn mul_assign(&mut self, o: Self) {
        *self = *self * o;
    }
}
impl Neg for Complex {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.re, -self.im)
    }
}

/// Smallest power of two `>= n` (1 for `n <= 1`), or `None` when it does not fit in `usize`.
pub fn next_pow2(n: usize) -> Option<usize> {
    n.max(1).checked_next_power_of_two()
}

/// In-place forward DFT `X_k = Σ x_m e^{-2πi mk/n}` (`inverse = false`) or the unnormalized
/// inverse `x_m = Σ X_k e^{+2πi mk/n}` (`inverse = true`; divide by `n` yourself).
///
/// # Errors
/// `data.len()` must be a power of two (an empty slice is a no-op).
pub fn fft_in_place(data: &mut [Complex], inverse: bool) -> Result<(), String> {
    let n = data.len();
    if n <= 1 {
        return Ok(());
    }
    if !n.is_power_of_two() {
        return Err(format!("FFT length {n} is not a power of two"));
    }
    // bit-reversal permutation
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            data.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let step = sign * 2.0 * std::f64::consts::PI / len as f64;
        // twiddles for this stage, computed directly (no recurrence drift)
        let tw: Vec<Complex> = (0..half)
            .map(|k| Complex::from_phase(step * k as f64))
            .collect();
        for chunk in data.chunks_exact_mut(len) {
            let (a, b) = chunk.split_at_mut(half);
            for ((x, y), w) in a.iter_mut().zip(b.iter_mut()).zip(&tw) {
                let t = *y * *w;
                *y = *x - t;
                *x += t;
            }
        }
        len *= 2;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dft(x: &[Complex]) -> Vec<Complex> {
        let n = x.len();
        (0..n)
            .map(|k| {
                let mut s = Complex::default();
                for (m, v) in x.iter().enumerate() {
                    let th = -2.0 * std::f64::consts::PI * (m * k) as f64 / n as f64;
                    s += *v * Complex::from_phase(th);
                }
                s
            })
            .collect()
    }

    #[test]
    fn matches_direct_dft() {
        for n in [1usize, 2, 4, 8, 64, 256] {
            let x: Vec<Complex> = (0..n)
                .map(|i| Complex::new((i as f64 * 0.37).sin() + 0.1, (i as f64 * 1.3).cos()))
                .collect();
            let mut y = x.clone();
            fft_in_place(&mut y, false).unwrap();
            let d = dft(&x);
            for (a, b) in y.iter().zip(&d) {
                assert!((a.re - b.re).abs() < 1e-9 && (a.im - b.im).abs() < 1e-9);
            }
            fft_in_place(&mut y, true).unwrap();
            for (a, b) in y.iter().zip(&x) {
                assert!((a.re / n as f64 - b.re).abs() < 1e-12);
                assert!((a.im / n as f64 - b.im).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn rejects_non_power_of_two() {
        let mut v = vec![Complex::default(); 12];
        assert!(fft_in_place(&mut v, false).is_err());
        assert_eq!(next_pow2(12), Some(16));
        assert_eq!(next_pow2(0), Some(1));
    }
}
