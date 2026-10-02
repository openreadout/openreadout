//! Compensation (spillover) matrices and applying them to event rows.
//!
//! A conventional matrix is square: `S[i][j]` is the fraction of fluorochrome `i`'s signal seen
//! by detector `j`, so raw = true · S and true = raw · S⁻¹. A spectral matrix has more detectors
//! (columns) than fluorochromes (rows); the unmixed values are the ordinary least-squares
//! solution `true = raw · Sᵀ (S Sᵀ)⁻¹`. Derivation: `docs/provenance/flowjo-wsp.md`.

// Numerical code written in the notation of its sources (Moore & Parks 2012, Gating-ML 2.0),
// with exact float comparisons where an iteration checks for a fixed point.
#![allow(clippy::many_single_char_names, clippy::float_cmp)]

use crate::layout::SpilloverMatrix;

/// A compensation or unmixing matrix with the names of its rows and columns.
#[derive(Debug, Clone, PartialEq)]
pub struct CompMatrix {
    /// Name or id (`$SPILLOVER`, a Gating-ML `spectrumMatrix` id, a FlowJo matrix name).
    pub name: String,
    /// Column names: the detectors (`$PnN`) the matrix reads.
    pub detectors: Vec<String>,
    /// Row names: fluorochromes (Gating-ML) or the detectors that receive the unmixed values.
    /// Equal to `detectors` for a square FCS spillover matrix.
    pub fluorochromes: Vec<String>,
    /// Row-major coefficients, `fluorochromes.len()` rows × `detectors.len()` columns.
    pub values: Vec<Vec<f64>>,
    /// More detectors than rows: unmixed by ordinary least squares.
    pub spectral: bool,
}

impl CompMatrix {
    /// A square matrix from an FCS spillover keyword (`$SPILLOVER`, `$SPILL`, `SPILL`).
    pub fn from_spillover(m: &SpilloverMatrix) -> Option<CompMatrix> {
        if m.parameters.is_empty() || m.parameters.len() != m.values.len() {
            return None;
        }
        Some(CompMatrix {
            name: m.keyword.clone(),
            detectors: m.parameters.clone(),
            fluorochromes: m.parameters.clone(),
            values: m.values.clone(),
            spectral: false,
        })
    }

    /// Check the shape and compute the matrix that maps detector rows to compensated rows.
    pub fn solver(&self) -> Result<Solver, String> {
        let rows = self.values.len();
        let cols = self.detectors.len();
        if rows == 0 || cols == 0 {
            return Err(format!("compensation matrix {} is empty", self.name));
        }
        if self.values.iter().any(|r| r.len() != cols) {
            return Err(format!(
                "compensation matrix {}: every row needs {cols} coefficients",
                self.name
            ));
        }
        if self.values.iter().flatten().any(|v| !v.is_finite()) {
            return Err(format!(
                "compensation matrix {} holds a non-finite coefficient",
                self.name
            ));
        }
        if rows > cols {
            return Err(format!(
                "compensation matrix {} has more rows ({rows}) than detectors ({cols})",
                self.name
            ));
        }
        // K maps a row of detector values (length cols) to `rows` output values: out = raw · K.
        let k = if rows == cols {
            invert(&self.values)
                .ok_or_else(|| format!("compensation matrix {} is singular", self.name))?
        } else {
            // K = Sᵀ (S Sᵀ)⁻¹
            let sst: Vec<Vec<f64>> = (0..rows)
                .map(|i| {
                    (0..rows)
                        .map(|j| {
                            (0..cols)
                                .map(|c| self.values[i][c] * self.values[j][c])
                                .sum()
                        })
                        .collect()
                })
                .collect();
            let inv = invert(&sst).ok_or_else(|| {
                format!(
                    "spectral matrix {} is rank-deficient (its rows are not independent)",
                    self.name
                )
            })?;
            (0..cols)
                .map(|c| {
                    (0..rows)
                        .map(|j| (0..rows).map(|i| self.values[i][c] * inv[i][j]).sum())
                        .collect()
                })
                .collect()
        };
        Ok(Solver { k, outputs: rows })
    }
}

/// The precomputed right-hand matrix of a compensation.
#[derive(Debug, Clone)]
pub struct Solver {
    /// `detectors × outputs`.
    k: Vec<Vec<f64>>,
    outputs: usize,
}

impl Solver {
    /// Compensate one event: `raw` holds the detector values in the matrix's column order;
    /// the result has one value per matrix row.
    pub fn apply_row(&self, raw: &[f64], out: &mut Vec<f64>) {
        out.clear();
        out.resize(self.outputs, 0.0);
        for (d, &x) in raw.iter().enumerate() {
            if x == 0.0 {
                continue;
            }
            for (o, slot) in out.iter_mut().enumerate() {
                *slot += x * self.k[d][o];
            }
        }
    }

    /// Compensate column-major data in place. `detector_cols[j]` is the column holding the
    /// matrix's detector `j`; `output_cols[i]` receives row `i`'s values.
    #[allow(clippy::needless_range_loop)] // rows index several columns at once
    pub fn apply_columns(
        &self,
        columns: &mut [Vec<f64>],
        detector_cols: &[usize],
        output_cols: &[usize],
    ) {
        let n = detector_cols
            .iter()
            .chain(output_cols)
            .filter_map(|&c| columns.get(c).map(Vec::len))
            .min()
            .unwrap_or(0);
        let mut raw = vec![0.0; detector_cols.len()];
        let mut out = Vec::with_capacity(self.outputs);
        for r in 0..n {
            for (slot, &c) in raw.iter_mut().zip(detector_cols) {
                *slot = columns[c][r];
            }
            self.apply_row(&raw, &mut out);
            for (i, &c) in output_cols.iter().enumerate() {
                columns[c][r] = out[i];
            }
        }
    }
}

/// Gauss–Jordan inverse with partial pivoting. `None` when the matrix is singular.
pub fn invert(m: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    if m.iter().any(|r| r.len() != n) {
        return None;
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    let scale = m
        .iter()
        .flatten()
        .fold(0.0_f64, |acc, v| acc.max(v.abs()))
        .max(f64::MIN_POSITIVE);
    for col in 0..n {
        let pivot = (col..n).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() <= scale * 1e-14 {
            return None;
        }
        a.swap(col, pivot);
        inv.swap(col, pivot);
        let p = a[col][col];
        for j in 0..n {
            a[col][j] /= p;
            inv[col][j] /= p;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let f = a[row][col];
            if f == 0.0 {
                continue;
            }
            for j in 0..n {
                a[row][j] -= f * a[col][j];
                inv[row][j] -= f * inv[col][j];
            }
        }
    }
    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_compensation_undoes_spillover() {
        let m = CompMatrix {
            name: "t".into(),
            detectors: vec!["A".into(), "B".into()],
            fluorochromes: vec!["A".into(), "B".into()],
            values: vec![vec![1.0, 0.2], vec![0.1, 1.0]],
            spectral: false,
        };
        // true (100, 50) → raw = true · S = (105, 70)
        let s = m.solver().unwrap();
        let mut out = Vec::new();
        s.apply_row(&[105.0, 70.0], &mut out);
        assert!((out[0] - 100.0).abs() < 1e-9 && (out[1] - 50.0).abs() < 1e-9);
    }

    #[test]
    fn spectral_least_squares_recovers_exact_mixture() {
        let m = CompMatrix {
            name: "s".into(),
            detectors: vec!["d1".into(), "d2".into(), "d3".into()],
            fluorochromes: vec!["f1".into(), "f2".into()],
            values: vec![vec![1.0, 0.5, 0.1], vec![0.2, 1.0, 0.7]],
            spectral: true,
        };
        let t = [30.0, 12.0];
        let raw: Vec<f64> = (0..3)
            .map(|c| t[0] * m.values[0][c] + t[1] * m.values[1][c])
            .collect();
        let mut out = Vec::new();
        m.solver().unwrap().apply_row(&raw, &mut out);
        assert!((out[0] - 30.0).abs() < 1e-9 && (out[1] - 12.0).abs() < 1e-9);
    }

    #[test]
    fn singular_is_an_error() {
        let m = CompMatrix {
            name: "z".into(),
            detectors: vec!["A".into(), "B".into()],
            fluorochromes: vec!["A".into(), "B".into()],
            values: vec![vec![1.0, 1.0], vec![1.0, 1.0]],
            spectral: false,
        };
        assert!(m.solver().is_err());
    }
}
