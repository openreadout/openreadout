//! The `acqus` / `acqu` parameter file of a Bruker flex spectrum: JCAMP-DX style `##$KEY= value`
//! lines (`docs/formats/bruker-flex.md` § Parameters). Strings are in angle brackets; array
//! parameters (`(0..n)` followed by values) are kept as the text of their first line.

use std::collections::BTreeMap;

/// Parameters by name (`$` and `.` prefixes removed: `TD`, `IONIZATION MODE`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Params {
    pub(crate) map: BTreeMap<String, String>,
}

impl Params {
    /// Parse the text of a parameter file.
    pub(crate) fn parse(text: &str) -> Self {
        let mut map = BTreeMap::new();
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("##") else {
                continue;
            };
            let Some((k, v)) = rest.split_once('=') else {
                continue;
            };
            let k = k.trim_start_matches(['$', '.']).trim();
            let mut v = v.trim();
            if let Some(s) = v.strip_prefix('<') {
                v = s.strip_suffix('>').unwrap_or(s);
            }
            if !k.is_empty() && map.len() < 10_000 {
                map.entry(k.to_string())
                    .or_insert_with(|| v.trim().to_string());
            }
        }
        Params { map }
    }

    pub(crate) fn get(&self, k: &str) -> Option<&str> {
        self.map
            .get(k)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    pub(crate) fn f64(&self, k: &str) -> Option<f64> {
        self.get(k)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
    }

    /// Whether the text is a flex (XMASS) parameter file rather than a TopSpin one.
    pub(crate) fn is_xmass(text: &str) -> bool {
        text.lines().take(8).any(|l| {
            (l.starts_with("##TITLE=") || l.starts_with("##ORIGIN=")) && l.contains("XMASS")
        })
    }
}

/// The time-of-flight calibration of a spectrum: `tof = DW · i + DELAY` (ns), then m/z from the
/// quadratic `ML3 · s² + b · s + (ML2 − tof) = 0` with `b = √(10¹² / ML1)` and `s = √(m/z)`; a
/// zero `ML3` makes it linear in `s`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Calibration {
    pub(crate) dw: f64,
    pub(crate) delay: f64,
    pub(crate) ml1: f64,
    pub(crate) ml2: f64,
    pub(crate) ml3: f64,
}

impl Calibration {
    pub(crate) fn from(p: &Params) -> Result<Self, String> {
        let need = |k: &str| {
            p.f64(k)
                .ok_or_else(|| format!("the parameter file has no usable {k}"))
        };
        let c = Calibration {
            dw: need("DW")?,
            delay: need("DELAY")?,
            ml1: need("ML1")?,
            ml2: need("ML2")?,
            ml3: p.f64("ML3").unwrap_or(0.0),
        };
        if c.ml1 <= 0.0 {
            return Err(format!("ML1 is {} (it must be positive)", c.ml1));
        }
        Ok(c)
    }

    /// m/z of point `i`.
    pub(crate) fn mz(&self, i: usize) -> f64 {
        let tof = self.dw * i as f64 + self.delay;
        let b = (1e12 / self.ml1).sqrt();
        let c = self.ml2 - tof;
        let s = if self.ml3 == 0.0 {
            c / b
        } else {
            ((b * b - 4.0 * self.ml3 * c).sqrt() - b) / (2.0 * self.ml3)
        };
        s * s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameters() {
        let p = Params::parse(
            "##TITLE=  XMASS Parameter file\n##$TD= 4 \n##$INSTRUM= <RAPIFLEX> \n##.IONIZATION MODE=  LD+\n##$LIFTC= (0..2)\n0 1 2\n",
        );
        assert!(Params::is_xmass("##TITLE=  XMASS Parameter file\n"));
        assert!(!Params::is_xmass("##TITLE= Parameter file, TOPSPIN\n"));
        assert_eq!(p.f64("TD"), Some(4.0));
        assert_eq!(p.get("INSTRUM"), Some("RAPIFLEX"));
        assert_eq!(p.get("IONIZATION MODE"), Some("LD+"));
    }

    #[test]
    fn linear_calibration() {
        // ML3 = 0, ML2 = 0: m/z = tof² · ML1 / 10¹²
        let c = Calibration {
            dw: 1.0,
            delay: 1000.0,
            ml1: 1e6,
            ml2: 0.0,
            ml3: 0.0,
        };
        assert!((c.mz(0) - 1.0).abs() < 1e-12);
    }
}
