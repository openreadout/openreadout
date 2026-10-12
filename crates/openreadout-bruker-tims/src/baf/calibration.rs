//! Sample index to m/z for BAF spectra: the time-of-flight calibrations (kinds 3 and 10) with
//! their temperature drift compensation, the FT-ICR calibration, and the recalibration stored
//! in `Calibrator.ami`. `docs/formats/bruker-baf.md` § Calibration.

use openreadout_core::bytes::{le_f64, le_u16, le_u32};

/// Largest residual polynomial accepted.
const MAX_POLY: usize = 32;

/// A calibration the reader can apply.
#[derive(Debug, Clone, PartialEq)]
pub enum Calibration {
    /// Time of flight, kinds 3 (residual polynomial) and 10 (mass shift).
    Tof(TofCalibration),
    /// FT-ICR: `m/z = c3 / (f + c2)`, `f = f_high · (1 − index / n_fft)`.
    Ft(FtCalibration),
}

/// Time-of-flight constants.
#[derive(Debug, Clone, PartialEq)]
pub struct TofCalibration {
    /// Payload kind: 3 or 10.
    pub kind: u16,
    /// Delay of the first sample, ns.
    pub delay_ns: f64,
    /// Time per sample, ns.
    pub timebase_ns: f64,
    /// Time offset, ns.
    pub t0_ns: f64,
    /// Field constant K (k1 = 1e6 / √K).
    pub k: f64,
    /// Quadratic term Q.
    pub q: f64,
    /// Residual polynomial (kind 3), lowest order first, and the m/z range it applies to.
    pub poly: Vec<f64>,
    /// See `poly`.
    pub poly_range: (f64, f64),
    /// Mass shift subtracted (kind 10).
    pub shift: f64,
    /// Detector temperatures the constants belong to.
    pub temperatures: Option<(f64, f64)>,
}

/// FT-ICR constants.
#[derive(Debug, Clone, PartialEq)]
pub struct FtCalibration {
    /// `c2`, a frequency offset (Hz).
    pub c2: f64,
    /// `c3`, the scale (m/z · Hz).
    pub c3: f64,
    /// Transform size.
    pub n_fft: u32,
    /// Highest frequency (Hz).
    pub f_high: f64,
}

/// Why a payload is not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalibrationIssue {
    /// A kind no development file showed.
    Unknown(String),
    /// The bytes do not hold what the kind needs.
    Damaged(String),
}

impl std::fmt::Display for CalibrationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CalibrationIssue::Unknown(s) | CalibrationIssue::Damaged(s) => f.write_str(s),
        }
    }
}

fn d(p: &[u8], at: usize) -> Result<f64, CalibrationIssue> {
    le_f64(p, at).ok_or_else(|| CalibrationIssue::Damaged("calibration payload cut short".into()))
}

fn find(p: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    p.get(from..)
        .and_then(|b| openreadout_core::bytes::find(b, pat))
        .map(|i| i + from)
}

/// Parse a calibration payload (sub-record `0xBFA00006` after its 12 bytes, or a payload found
/// in `Calibrator.ami`).
pub fn parse_payload(p: &[u8]) -> Result<Calibration, CalibrationIssue> {
    let kind = le_u16(p, 0).ok_or_else(|| CalibrationIssue::Damaged("empty calibration".into()))?;
    match kind {
        3 => {
            let mut c = TofCalibration {
                kind,
                delay_ns: d(p, 4)?,
                timebase_ns: d(p, 12)?,
                t0_ns: d(p, 20)?,
                k: d(p, 28)?,
                q: d(p, 36)?,
                poly: Vec::new(),
                poly_range: (d(p, 72)?, d(p, 80)?),
                shift: 0.0,
                temperatures: None,
            };
            let n = usize::from(le_u16(p, 90).ok_or_else(|| {
                CalibrationIssue::Damaged("calibration payload cut short".into())
            })?);
            if n > MAX_POLY {
                return Err(CalibrationIssue::Damaged(format!(
                    "residual polynomial of {n} terms"
                )));
            }
            for k in 0..n {
                c.poly.push(d(p, 96 + 8 * k)?);
            }
            let tail = 96 + 8 * n;
            if p.get(tail + 24..tail + 26) == Some(&[0xA0, 0xCF]) {
                c.temperatures = Some((d(p, tail + 44)?, d(p, tail + 52)?));
            }
            check_tof(&c)?;
            Ok(Calibration::Tof(c))
        }
        10 => {
            let extra = d(p, 44)?;
            if extra != 0.0 {
                return Err(CalibrationIssue::Unknown(
                    "a kind-10 calibration with a non-zero sixth constant".into(),
                ));
            }
            let mut c = TofCalibration {
                kind,
                delay_ns: d(p, 4)?,
                timebase_ns: d(p, 12)?,
                t0_ns: d(p, 20)?,
                k: d(p, 28)?,
                q: d(p, 36)?,
                poly: Vec::new(),
                poly_range: (0.0, 0.0),
                shift: d(p, 52)?,
                temperatures: None,
            };
            if let Some(i) = find(p, 60, &0xBFACu32.to_le_bytes())
                && i >= 76
            {
                c.temperatures = Some((d(p, i - 16)?, d(p, i - 8)?));
            }
            check_tof(&c)?;
            Ok(Calibration::Tof(c))
        }
        2 | 12 => Err(CalibrationIssue::Unknown(format!(
            "calibration kind {kind} (the reference, uncompensated form) on a spectrum"
        ))),
        _ if le_u32(p, 4) == Some(4) => {
            let c = FtCalibration {
                c2: d(p, 16)?,
                c3: d(p, 24)?,
                n_fft: le_u32(p, 36).unwrap_or(0),
                f_high: d(p, 52)?,
            };
            if c.n_fft == 0 || !c.f_high.is_finite() || !c.c3.is_finite() || !c.c2.is_finite() {
                return Err(CalibrationIssue::Damaged(
                    "FT-ICR calibration with a zero transform size or non-finite constants".into(),
                ));
            }
            Ok(Calibration::Ft(c))
        }
        _ => Err(CalibrationIssue::Unknown(format!(
            "calibration kind {kind} ({:#06x} {:#010x})",
            kind,
            le_u32(p, 4).unwrap_or(0)
        ))),
    }
}

fn check_tof(c: &TofCalibration) -> Result<(), CalibrationIssue> {
    let ok = [c.delay_ns, c.timebase_ns, c.t0_ns, c.k, c.q, c.shift]
        .iter()
        .all(|v| v.is_finite())
        && c.k > 0.0
        && c.timebase_ns > 0.0
        && c.poly.iter().all(|v| v.is_finite());
    if ok {
        Ok(())
    } else {
        Err(CalibrationIssue::Damaged(
            "time-of-flight calibration with a non-positive K or time base, or non-finite constants".into(),
        ))
    }
}

impl Calibration {
    /// m/z of a (fractional) sample index.
    pub fn mz(&self, index: f64) -> f64 {
        match self {
            Calibration::Tof(c) => c.mz(index),
            Calibration::Ft(c) => {
                let f = c.f_high - index * c.f_high / f64::from(c.n_fft);
                c.c3 / (f + c.c2)
            }
        }
    }

    /// Short name for `info` and assurance.
    pub fn name(&self) -> String {
        match self {
            Calibration::Tof(c) => format!("time of flight, kind {}", c.kind),
            Calibration::Ft(_) => "FT-ICR".into(),
        }
    }
}

impl TofCalibration {
    /// m/z of a sample index: `t = delay + timebase · index`, √(m/z) from the quadratic, then
    /// the residual polynomial (kind 3, fading as exp(−d²) outside its range) or the shift
    /// (kind 10).
    pub fn mz(&self, index: f64) -> f64 {
        let t = self.delay_ns + self.timebase_ns * index;
        let k1 = 1e6 / self.k.sqrt();
        let x = if self.q == 0.0 {
            (t - self.t0_ns) / k1
        } else {
            (-k1 + (k1 * k1 - 4.0 * self.q * (self.t0_ns - t)).sqrt()) / (2.0 * self.q)
        };
        let m = x * x;
        if self.kind == 10 {
            return m - self.shift;
        }
        if self.poly.is_empty() {
            return m;
        }
        let (lo, hi) = self.poly_range;
        let xc = m.clamp(lo.min(hi), hi.max(lo));
        let mut c = 0.0;
        for p in self.poly.iter().rev() {
            c = c * xc + p;
        }
        let dist = if m < lo {
            lo - m
        } else if m > hi {
            m - hi
        } else {
            0.0
        };
        m - c * (-dist * dist).exp()
    }
}

/// Temperature drift compensation of the time-of-flight constants.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DriftSettings {
    /// Reference temperatures 1 and 2.
    pub reference: (f64, f64),
    /// Gradients 1 and 2 (ppm per degree).
    pub gradient: (f64, f64),
    /// Whether compensation is on (`TOF_SwitchTempCompensation`).
    pub enabled: bool,
    /// Whether only temperature 1 is used (`TOF_UseOnlyOneTemperature`).
    pub one_sensor: bool,
}

impl DriftSettings {
    /// The factor K is multiplied (and Q divided) by at temperatures `t`.
    pub fn factor(&self, t: (f64, f64)) -> f64 {
        if !self.enabled {
            return 1.0;
        }
        let mut s = self.gradient.0 * (t.0 - self.reference.0);
        if !self.one_sensor {
            s += self.gradient.1 * (t.1 - self.reference.1);
        }
        1.0 - 1e-6 * s
    }
}

/// A reference calibration of the acquisition header (kinds 2 and 12): its t0, K, Q and the
/// drift reference temperatures and gradients after the marker `2, 2`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReferenceCalibration {
    /// Time offset, ns (matches the spectra it belongs to).
    pub t0_ns: f64,
    /// K at the reference temperatures.
    pub k: f64,
    /// Q at the reference temperatures.
    pub q: f64,
    /// Reference temperatures.
    pub reference: (f64, f64),
    /// Gradients (ppm per degree).
    pub gradient: (f64, f64),
}

/// Parse a reference calibration payload.
pub fn parse_reference(p: &[u8]) -> Option<ReferenceCalibration> {
    let kind = le_u16(p, 0)?;
    if kind != 2 && kind != 12 {
        return None;
    }
    let i = find(p, 44, &[2, 0, 2, 0])?;
    let r = ReferenceCalibration {
        t0_ns: le_f64(p, 20)?,
        k: le_f64(p, 28)?,
        q: le_f64(p, 36)?,
        reference: (le_f64(p, i + 4)?, le_f64(p, i + 12)?),
        gradient: (le_f64(p, i + 20)?, le_f64(p, i + 28)?),
    };
    [
        r.t0_ns,
        r.k,
        r.q,
        r.reference.0,
        r.reference.1,
        r.gradient.0,
        r.gradient.1,
    ]
    .iter()
    .all(|v| v.is_finite())
    .then_some(r)
}

/// The calibration payload `Calibrator.ami` holds for spectra calibrated like `first`: the
/// first payload of the same kind, delay and time base, found by scanning for the kind word.
// The payload must hold exactly the spectra's delay and time base.
#[allow(clippy::float_cmp)]
pub fn find_recalibration(ami: &[u8], first: &TofCalibration) -> Option<TofCalibration> {
    let pat = first.kind.to_le_bytes();
    let mut from = 0usize;
    while let Some(i) = find(ami, from, &pat) {
        from = i + 1;
        let end = (i + 400).min(ami.len());
        if let Ok(Calibration::Tof(c)) = parse_payload(&ami[i..end])
            && c.kind == first.kind
            && c.delay_ns == first.delay_ns
            && c.timebase_ns == first.timebase_ns
            && c.k > 0.0
            && c.k < 1e9
        {
            return Some(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tof_and_ft_formulas() {
        // With Q = 0 and no polynomial: √m = (t − t0) / k1.
        let c = TofCalibration {
            kind: 3,
            delay_ns: 0.0,
            timebase_ns: 1.0,
            t0_ns: 0.0,
            k: 1e12, // k1 = 1
            q: 0.0,
            poly: Vec::new(),
            poly_range: (0.0, 0.0),
            shift: 0.0,
            temperatures: None,
        };
        assert_eq!(c.mz(10.0), 100.0);
        // A constant residual of 1 inside [0, 200]; it fades outside.
        let mut p = c.clone();
        p.poly = vec![1.0];
        p.poly_range = (0.0, 50.0);
        assert_eq!(p.mz(5.0), 24.0);
        assert!((p.mz(10.0) - (100.0 - (-(50.0f64 * 50.0)).exp())).abs() < 1e-12);
        let mut s = c.clone();
        s.kind = 10;
        s.shift = 0.5;
        assert_eq!(s.mz(10.0), 99.5);
        let ft = Calibration::Ft(FtCalibration {
            c2: 0.0,
            c3: 1e6,
            n_fft: 4,
            f_high: 1000.0,
        });
        assert_eq!(ft.mz(2.0), 2000.0);
        let drift = DriftSettings {
            reference: (30.0, 30.0),
            gradient: (2.0, 1.0),
            enabled: true,
            one_sensor: false,
        };
        assert!((drift.factor((31.0, 32.0)) - (1.0 - 4e-6)).abs() < 1e-15);
    }

    #[test]
    fn payloads() {
        let mut p = vec![0u8; 140];
        p[0..2].copy_from_slice(&3u16.to_le_bytes());
        for (at, v) in [
            (4, 1.0f64),
            (12, 0.5),
            (20, 2.0),
            (28, 1e8),
            (36, 0.0),
            (72, 10.0),
            (80, 900.0),
        ] {
            p[at..at + 8].copy_from_slice(&v.to_le_bytes());
        }
        p[88..90].copy_from_slice(&1u16.to_le_bytes());
        p[90..92].copy_from_slice(&1u16.to_le_bytes());
        p[96..104].copy_from_slice(&0.25f64.to_le_bytes());
        let Ok(Calibration::Tof(c)) = parse_payload(&p) else {
            panic!("kind 3")
        };
        assert_eq!(
            (c.poly.clone(), c.poly_range, c.temperatures),
            (vec![0.25], (10.0, 900.0), None)
        );
        let mut bad = p.clone();
        bad[28..36].copy_from_slice(&(-1.0f64).to_le_bytes());
        assert!(matches!(
            parse_payload(&bad),
            Err(CalibrationIssue::Damaged(_))
        ));
        assert!(matches!(
            parse_payload(&[7, 0, 1, 0, 0, 0, 0, 0]),
            Err(CalibrationIssue::Unknown(_))
        ));
        assert!(parse_payload(&p[..50]).is_err());
        assert!(find_recalibration(&p, &c).is_some());
    }
}
