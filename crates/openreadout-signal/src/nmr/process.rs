//! 1-D processing of a free induction decay into a referenced, phased spectrum.
//!
//! Pipeline (each step recorded in [`ProcessingRecord::steps`]):
//!
//! 1. copy the FID into a zero-filled buffer of `size` complex points (a power of two;
//!    default: the vendor's stored transform size, else the next power of two ≥ 2 × the FID);
//!    optionally complex-conjugate it (vendor convention, see `FidParameters::conjugate`);
//! 2. digital-filter group delay: rotate the buffer left by the integer part of the delay (the
//!    filter's pre-response lands at the end of the buffer, i.e. at negative time) and apply the
//!    fractional part after the transform as a linear phase `e^{2πi·frac·f/size}`, `f` the
//!    signed frequency index;
//! 3. apodization: exponential `e^{−π·LB·t}` or Gaussian `e^{−(π·GB·t)²/(4 ln 2)}` (a Gaussian
//!    line of `GB` Hz full width at half height), `t` in seconds from the true time zero;
//! 4. first point × 0.5 (removes the constant offset a discrete transform of a sampled decay
//!    otherwise adds);
//! 5. forward FFT, reordered so that index 0 is the high-frequency (high-ppm, left) edge and the
//!    carrier sits at index `size/2`;
//! 6. phase correction `S_k · e^{−i(φ0 + φ1·k/size)}` (degrees; `k` counted from the left edge):
//!    stored phases, automatic phasing ([`crate::nmr::phase`]), manual values, or a magnitude
//!    spectrum;
//! 7. baseline correction on the real part ([`crate::nmr::baseline`]);
//! 8. ppm referencing: point `k` is at `first + k·step` with
//!    `first = ((carrier − reference)·10⁶ + SW/2) / reference` and
//!    `step = −SW / (size · reference)` (frequencies in MHz, `SW` in Hz).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use openreadout_core::{Error, Result};

use crate::fft::{Complex, fft_in_place, next_pow2};
use crate::nmr::baseline::{self, BaselineMode};
use crate::nmr::phase;

/// `PhaseMode::Default` distrusts stored phases that leave more than this share of the
/// signal-region intensity negative ([`phase::negative_fraction`]).
pub const STORED_PHASE_MAX_NEGATIVE: f64 = 0.2;

/// Largest transform, in complex points (2^22 = 4 Mi points, 64 MiB of complex values).
pub const MAX_TRANSFORM_SIZE: usize = 1 << 22;

/// What the processing needs to know about an acquisition.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct FidParameters {
    /// Spectral width (Hz): the complex sampling rate.
    pub spectral_width_hz: f64,
    /// Carrier (transmitter) frequency, MHz: the centre of the spectrum.
    pub carrier_frequency_mhz: f64,
    /// Frequency of 0 ppm, MHz (the referencing frequency).
    pub reference_frequency_mhz: f64,
    /// Digital-filter group delay in points (0 when the data carry none).
    pub group_delay_points: f64,
    /// Complex-conjugate the FID before the transform (reverses the frequency axis).
    pub conjugate: bool,
    /// Observed nucleus, e.g. `1H`, `13C`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nucleus: Option<String>,
}

/// Processing parameters the vendor software stored with the data, in this crate's conventions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct StoredProcessing {
    /// Where they came from, e.g. `pdata/1/procs` or `procpar`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Transform size in complex points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>,
    /// Exponential line broadening, Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_broadening_hz: Option<f64>,
    /// Zero-order phase, degrees (`φ0` of the module docs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase0_deg: Option<f64>,
    /// First-order phase across the whole spectrum, degrees (`φ1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase1_deg: Option<f64>,
    /// True when the vendor software also stored a processed spectrum made with these values.
    #[serde(default)]
    pub spectrum_stored: bool,
    /// Factor applied to the first FID point (Bruker `FCOR`); `None`: 0.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_point_factor: Option<f64>,
    /// Complex FID points the vendor processing used (Bruker `TDeff` / 2); `None`: all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_domain_points: Option<usize>,
}

impl StoredProcessing {
    /// Stored phases worth using: both present and not the untouched `0, 0` of a data set that
    /// was never phased (unless a processed spectrum was stored with them).
    pub fn usable_phases(&self) -> Option<(f64, f64)> {
        let (p0, p1) = (self.phase0_deg?, self.phase1_deg.unwrap_or(0.0));
        if !(p0.is_finite() && p1.is_finite()) {
            return None;
        }
        if p0 == 0.0 && p1 == 0.0 && !self.spectrum_stored {
            return None;
        }
        Some((p0, p1))
    }
}

/// Window function applied to the FID.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Apodization {
    /// No window.
    None,
    /// `e^{−π·LB·t}`: Lorentzian lines broadened by `line_broadening_hz`.
    Exponential {
        /// Added Lorentzian full width at half height, Hz.
        line_broadening_hz: f64,
    },
    /// `e^{−(π·GB·t)²/(4 ln 2)}`: multiplies each line by a Gaussian of `line_broadening_hz`
    /// full width at half height.
    Gaussian {
        /// Gaussian full width at half height, Hz.
        line_broadening_hz: f64,
    },
}

/// How the spectrum is phased.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PhaseMode {
    /// Stored phases when the data set has usable ones that fit the data (at most 20 % of the
    /// signal negative, else automatic phasing if it does at least twice as well), otherwise
    /// automatic.
    #[default]
    Default,
    /// The vendor's stored phases (error when there are none).
    Stored,
    /// Automatic phasing (ACME entropy minimisation, see [`crate::nmr::phase`]).
    Auto,
    /// Given phases, degrees.
    Manual {
        /// Zero-order phase `φ0`, degrees.
        phase0_deg: f64,
        /// First-order phase `φ1`, degrees.
        phase1_deg: f64,
    },
    /// Magnitude spectrum `|S|` (no phasing needed; broader lines).
    Magnitude,
    /// No phase correction.
    None,
}

/// Processing choices. `Default` gives the documented defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProcessOptions {
    /// Transform size in complex points (rounded up to a power of two). `None`: stored size,
    /// else the next power of two ≥ 2 × the FID length.
    #[serde(default)]
    pub size: Option<usize>,
    /// Window. `None`: exponential with the stored line broadening, else 0.3 Hz for 1H, 3H and
    /// 19F and 1 Hz for other nuclei.
    #[serde(default)]
    pub apodization: Option<Apodization>,
    /// Phasing.
    #[serde(default)]
    pub phase: PhaseMode,
    /// Baseline correction.
    #[serde(default)]
    pub baseline: BaselineMode,
    /// Ignore the digital-filter group delay (for data already corrected).
    #[serde(default)]
    pub ignore_group_delay: bool,
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            size: None,
            apodization: None,
            phase: PhaseMode::Default,
            baseline: BaselineMode::default(),
            ignore_group_delay: false,
        }
    }
}

/// A regular chemical-shift axis: point `k` is at `first_ppm + k · step_ppm`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct PpmAxis {
    /// Chemical shift of point 0 (the left, high-frequency edge), ppm.
    pub first_ppm: f64,
    /// Increment per point, ppm (negative: shifts decrease left to right).
    pub step_ppm: f64,
    /// Number of points.
    pub size: usize,
    /// Frequency of 0 ppm, MHz (converts ppm to Hz).
    pub reference_frequency_mhz: f64,
}

impl PpmAxis {
    /// Chemical shift at (fractional) point `k`.
    pub fn ppm(&self, k: f64) -> f64 {
        self.first_ppm + k * self.step_ppm
    }
    /// Fractional point index of chemical shift `ppm`.
    pub fn index_of(&self, ppm: f64) -> f64 {
        if self.step_ppm == 0.0 {
            0.0
        } else {
            (ppm - self.first_ppm) / self.step_ppm
        }
    }
    /// Chemical shift of the last point.
    pub fn last_ppm(&self) -> f64 {
        self.ppm(self.size.saturating_sub(1) as f64)
    }
    /// Hz per point (positive).
    pub fn hz_per_point(&self) -> f64 {
        (self.step_ppm * self.reference_frequency_mhz).abs()
    }
}

/// A 1-D spectrum: real (absorptive, after phasing) and imaginary parts on a ppm axis.
#[derive(Debug, Clone, Default)]
pub struct Spectrum {
    /// Real part.
    pub real: Vec<f64>,
    /// Imaginary part (empty for magnitude spectra and spectra read without one).
    pub imag: Vec<f64>,
    /// Chemical-shift axis.
    pub axis: PpmAxis,
}

/// What was done, with the values actually used.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProcessingRecord {
    /// Steps in order, e.g. `zero_fill`, `group_delay`, `apodization`, `fft`, `phase`, `baseline`.
    pub steps: Vec<String>,
    /// Complex points in the FID.
    pub fid_points: usize,
    /// Transform size, complex points.
    pub size: usize,
    /// Window applied.
    pub apodization: Option<Apodization>,
    /// Where the window width came from: `stored`, `default`, `option`.
    pub apodization_source: String,
    /// Group delay removed, points.
    pub group_delay_points: f64,
    /// Phase mode actually used: `stored`, `auto`, `manual`, `magnitude`, `none`.
    pub phase_mode: String,
    /// Zero-order phase applied, degrees.
    pub phase0_deg: f64,
    /// First-order phase applied, degrees.
    pub phase1_deg: f64,
    /// Baseline correction applied.
    pub baseline: BaselineMode,
    /// Parameters used.
    pub parameters: FidParameters,
    /// Stored processing values found (may be unused).
    pub stored: StoredProcessing,
    /// Remarks (fallbacks, clamping).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn default_line_broadening(nucleus: Option<&str>) -> f64 {
    match nucleus.map(str::trim) {
        Some("1H" | "3H" | "19F" | "H1" | "F19") => 0.3,
        _ => 1.0,
    }
}

/// Chemical-shift axis of a transform of `size` points (module docs, step 8).
pub fn ppm_axis(p: &FidParameters, size: usize) -> PpmAxis {
    let r = p.reference_frequency_mhz;
    let sw = p.spectral_width_hz;
    PpmAxis {
        first_ppm: ((p.carrier_frequency_mhz - r) * 1e6 + sw / 2.0) / r,
        step_ppm: -sw / (size as f64 * r),
        size,
        reference_frequency_mhz: r,
    }
}

fn check_parameters(fid: &[Complex], p: &FidParameters) -> Result<()> {
    let bad = |what: &str| {
        Err(Error::unsupported(
            "nmr-processing",
            format!("processing this FID: {what}"),
            "The acquisition parameters needed to process the FID are missing or invalid; run `openreadout info <file>` to see them, or pick a processed spectrum trace with `--trace`.",
        ))
    };
    if fid.is_empty() {
        return bad("the FID is empty");
    }
    if !(p.spectral_width_hz.is_finite() && p.spectral_width_hz > 0.0) {
        return bad("no spectral width");
    }
    if !(p.reference_frequency_mhz.is_finite() && p.reference_frequency_mhz > 0.0) {
        return bad("no spectrometer frequency");
    }
    if !(p.carrier_frequency_mhz.is_finite() && p.carrier_frequency_mhz > 0.0) {
        return bad("no carrier frequency");
    }
    if !p.group_delay_points.is_finite() || p.group_delay_points < 0.0 {
        return bad("negative or non-finite group delay");
    }
    Ok(())
}

/// Transform size: an explicit request, else the stored size, else 2 × the FID, as a power of
/// two within [`MAX_TRANSFORM_SIZE`].
fn transform_size(
    fid_len: usize,
    stored: &StoredProcessing,
    opts: &ProcessOptions,
    notes: &mut Vec<String>,
) -> Result<usize> {
    let want = opts
        .size
        .or(stored.size)
        .unwrap_or_else(|| fid_len.saturating_mul(2));
    let n = next_pow2(want.max(2)).unwrap_or(MAX_TRANSFORM_SIZE);
    if n != want {
        notes.push(format!(
            "transform size {want} rounded up to {n} (power of two)"
        ));
    }
    if n > MAX_TRANSFORM_SIZE {
        return Err(Error::Usage(format!(
            "transform size {n} exceeds the maximum {MAX_TRANSFORM_SIZE} complex points"
        )));
    }
    Ok(n)
}

/// Process a complex FID into a phased, referenced spectrum (module docs).
pub fn process_fid(
    fid: &[Complex],
    p: &FidParameters,
    stored: &StoredProcessing,
    opts: &ProcessOptions,
) -> Result<(Spectrum, ProcessingRecord)> {
    check_parameters(fid, p)?;
    let mut rec = ProcessingRecord {
        fid_points: fid.len(),
        parameters: p.clone(),
        stored: stored.clone(),
        baseline: opts.baseline,
        ..ProcessingRecord::default()
    };
    // the points the vendor processing used (TDeff)
    let fid = match stored.time_domain_points {
        Some(k) if k > 0 && k < fid.len() => {
            rec.notes.push(format!(
                "FID truncated from {} to the {k} points the stored processing used",
                fid.len()
            ));
            &fid[..k]
        }
        _ => fid,
    };
    let n = transform_size(fid.len(), stored, opts, &mut rec.notes)?;
    rec.size = n;
    let mut x = vec![Complex::default(); n];
    for (dst, src) in x.iter_mut().zip(fid) {
        *dst = if p.conjugate { src.conj() } else { *src };
        if !(dst.re.is_finite() && dst.im.is_finite()) {
            *dst = Complex::default();
        }
    }
    if fid.len() > n {
        rec.notes
            .push(format!("FID truncated from {} to {n} points", fid.len()));
    }
    rec.steps.push(format!("zero_fill:{n}"));
    if p.conjugate {
        rec.steps.push("conjugate".into());
    }

    // digital filter: integer part as a rotation, fractional part as a phase ramp after FFT
    let g = if opts.ignore_group_delay {
        0.0
    } else {
        p.group_delay_points
    };
    let gi = g.floor() as usize;
    let (gi, frac) = if gi >= n.min(fid.len()) {
        rec.notes.push(format!(
            "group delay {g} points is not shorter than the data; not removed"
        ));
        (0, 0.0)
    } else {
        (gi, g - gi as f64)
    };
    rec.group_delay_points = gi as f64 + frac;
    if gi > 0 {
        x.rotate_left(gi);
    }
    if gi > 0 || frac > 0.0 {
        rec.steps
            .push(format!("group_delay:{}", rec.group_delay_points));
    }

    // apodization over true time t = k / SW; the wrapped pre-response keeps weight 1
    let (apod, source) = match opts.apodization {
        Some(a) => (a, "option"),
        None => match stored.line_broadening_hz.filter(|v| v.is_finite()) {
            Some(lb) => (
                Apodization::Exponential {
                    line_broadening_hz: lb,
                },
                "stored",
            ),
            None => (
                Apodization::Exponential {
                    line_broadening_hz: default_line_broadening(p.nucleus.as_deref()),
                },
                "default",
            ),
        },
    };
    let dt = 1.0 / p.spectral_width_hz;
    let live = n - gi;
    match apod {
        Apodization::None => {}
        Apodization::Exponential { line_broadening_hz } => {
            let k = -std::f64::consts::PI * line_broadening_hz * dt;
            for (i, v) in x.iter_mut().take(live).enumerate() {
                *v = v.scale((k * i as f64).exp());
            }
        }
        Apodization::Gaussian { line_broadening_hz } => {
            let c = std::f64::consts::PI * line_broadening_hz * dt;
            let d = 4.0 * std::f64::consts::LN_2;
            for (i, v) in x.iter_mut().take(live).enumerate() {
                let a = c * i as f64;
                *v = v.scale((-(a * a) / d).exp());
            }
        }
    }
    rec.apodization = Some(apod);
    rec.apodization_source = source.into();
    rec.steps.push("apodization".into());
    let fcor = stored
        .first_point_factor
        .filter(|v| v.is_finite() && (0.0..=2.0).contains(v))
        .unwrap_or(0.5);
    x[0] = x[0].scale(fcor);
    rec.steps.push(format!("first_point:{fcor}"));

    fft_in_place(&mut x, false).map_err(Error::Other)?;
    if frac > 0.0 {
        // the fractional part as a linear phase about the carrier, plus the zero-order term
        // π·(1 − frac) that makes the result equal to a shift by the next whole point with the
        // remaining fraction's ramp pivoted at the spectrum edge: the spectrum stored phases
        // (Bruker PHC0) refer to (docs/formats/nmr-processing.md)
        let w = 2.0 * std::f64::consts::PI * frac / n as f64;
        let z = std::f64::consts::PI * (1.0 - frac);
        let half = n / 2;
        for (j, v) in x.iter_mut().enumerate() {
            let f = if j < half {
                j as f64
            } else {
                j as f64 - n as f64
            };
            *v *= Complex::from_phase(w * f + z);
        }
        rec.steps
            .push(format!("group_delay_zero_order:{:.4}", z.to_degrees()));
    }
    // index 0 = highest frequency, carrier at n/2
    let half = n / 2;
    let mut s: Vec<Complex> = (0..n).map(|k| x[(half + n - k) % n]).collect();
    drop(x);
    rec.steps.push("fft".into());

    // phase
    let (mode, p0, p1) = match opts.phase {
        PhaseMode::Default => {
            if let Some((a, b)) = stored.usable_phases() {
                // stored phases that leave much of the signal negative do not fit these data
                // (a different convention or console); automatic phasing is tried instead
                let neg = phase::negative_fraction(&s, a, b);
                if neg > STORED_PHASE_MAX_NEGATIVE {
                    let (c, d) = phase::autophase(&s);
                    let neg_auto = phase::negative_fraction(&s, c, d);
                    if neg_auto < neg / 2.0 {
                        rec.notes.push(format!(
                        "stored phases {a:.2}/{b:.2} deg leave {:.0} % of the signal negative; automatic phases ({:.0} %) used instead",
                        100.0 * neg,
                        100.0 * neg_auto
                    ));
                        ("auto", c, d)
                    } else {
                        ("stored", a, b)
                    }
                } else {
                    ("stored", a, b)
                }
            } else {
                let (a, b) = phase::autophase(&s);
                ("auto", a, b)
            }
        }
        PhaseMode::Stored => match stored.usable_phases() {
            Some((a, b)) => ("stored", a, b),
            None => {
                return Err(Error::unsupported(
                    "nmr-processing",
                    "stored phases: this data set has none",
                    "Use `--phase auto` (automatic phasing) or `--phase P0,P1` (degrees).",
                ));
            }
        },
        PhaseMode::Auto => {
            let (a, b) = phase::autophase(&s);
            ("auto", a, b)
        }
        PhaseMode::Manual {
            phase0_deg,
            phase1_deg,
        } => ("manual", phase0_deg, phase1_deg),
        PhaseMode::Magnitude => ("magnitude", 0.0, 0.0),
        PhaseMode::None => ("none", 0.0, 0.0),
    };
    rec.phase_mode = mode.into();
    rec.phase0_deg = p0;
    rec.phase1_deg = p1;
    let (real, imag) = if mode == "magnitude" {
        rec.steps.push("magnitude".into());
        (s.iter().map(|v| v.abs()).collect::<Vec<_>>(), Vec::new())
    } else {
        if mode != "none" {
            phase::apply_phase(&mut s, p0, p1);
            rec.steps.push(format!("phase:{mode}"));
        }
        (
            s.iter().map(|v| v.re).collect(),
            s.iter().map(|v| v.im).collect(),
        )
    };
    let mut spec = Spectrum {
        real,
        imag,
        axis: ppm_axis(p, n),
    };
    if let Some(step) = baseline::correct(&mut spec.real, opts.baseline) {
        rec.steps.push(step);
    }
    rec.steps.push("reference".into());
    Ok((spec, rec))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decaying complex exponential at `hz` from the carrier, preceded by `delay` points of
    /// zeros (a crude digital-filter delay).
    fn synth(n: usize, sw: f64, hz: f64, t2: f64, delay: usize, p0_deg: f64) -> Vec<Complex> {
        let ph = p0_deg.to_radians();
        (0..n)
            .map(|i| {
                if i < delay {
                    return Complex::default();
                }
                let t = (i - delay) as f64 / sw;
                Complex::from_phase(2.0 * std::f64::consts::PI * hz * t + ph).scale((-t / t2).exp())
            })
            .collect()
    }

    fn params(sw: f64, gd: f64) -> FidParameters {
        FidParameters {
            spectral_width_hz: sw,
            carrier_frequency_mhz: 400.000_002,
            reference_frequency_mhz: 400.0,
            group_delay_points: gd,
            conjugate: true,
            nucleus: Some("1H".into()),
        }
    }

    #[test]
    fn peak_lands_at_expected_ppm() {
        let sw = 4000.0;
        // Bruker-like convention: conjugated FID, so the line at -400 Hz (raw) is at +400 Hz.
        let hz = 820.0 * sw / 8192.0; // on the grid
        let fid = synth(4096, sw, -hz, 0.2, 0, 0.0);
        let opts = ProcessOptions {
            phase: PhaseMode::None,
            baseline: BaselineMode::None,
            ..ProcessOptions::default()
        };
        let (s, rec) =
            process_fid(&fid, &params(sw, 0.0), &StoredProcessing::default(), &opts).unwrap();
        assert_eq!(rec.size, 8192);
        let (k, _) = s
            .real
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        // carrier at 2 Hz above 0 ppm; the line is `hz` above the carrier
        let ppm = s.axis.ppm(k as f64);
        let want = (2.0 + hz) / 400.0;
        assert!(
            (ppm - want).abs() < 0.5 * s.axis.step_ppm.abs(),
            "ppm {ppm} want {want}"
        );
        // absorptive: the peak is positive and imaginary near zero at the top
        assert!(s.real[k] > 0.0);
        assert!(s.imag[k].abs() < 0.05 * s.real[k]);
    }

    #[test]
    fn group_delay_is_removed() {
        let sw = 4000.0;
        let fid = synth(4096, sw, -250.0, 0.2, 40, 0.0);
        let opts = ProcessOptions {
            phase: PhaseMode::None,
            baseline: BaselineMode::None,
            ..ProcessOptions::default()
        };
        let (s, _) =
            process_fid(&fid, &params(sw, 40.0), &StoredProcessing::default(), &opts).unwrap();
        let (k, _) = s
            .real
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert!(s.imag[k].abs() < 0.05 * s.real[k], "not absorptive");
    }

    #[test]
    fn manual_phase_undoes_a_known_phase() {
        let sw = 4000.0;
        // conjugation turns +30 degrees into -30; phase correction φ0 = -30 restores it
        let fid = synth(2048, sw, -(307.0 * sw / 4096.0), 0.3, 0, 30.0);
        let opts = ProcessOptions {
            phase: PhaseMode::Manual {
                phase0_deg: -30.0,
                phase1_deg: 0.0,
            },
            baseline: BaselineMode::None,
            ..ProcessOptions::default()
        };
        let (s, rec) =
            process_fid(&fid, &params(sw, 0.0), &StoredProcessing::default(), &opts).unwrap();
        assert_eq!(rec.phase_mode, "manual");
        let k = s
            .real
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert!(s.imag[k].abs() < 0.05 * s.real[k]);
    }

    #[test]
    fn rejects_bad_parameters_cleanly() {
        let fid = vec![Complex::new(1.0, 0.0); 16];
        let mut p = params(0.0, 0.0);
        assert!(
            process_fid(
                &fid,
                &p,
                &StoredProcessing::default(),
                &ProcessOptions::default()
            )
            .is_err()
        );
        p.spectral_width_hz = 1000.0;
        p.group_delay_points = f64::NAN;
        assert!(
            process_fid(
                &fid,
                &p,
                &StoredProcessing::default(),
                &ProcessOptions::default()
            )
            .is_err()
        );
        assert!(
            process_fid(
                &[],
                &params(1000.0, 0.0),
                &StoredProcessing::default(),
                &ProcessOptions::default()
            )
            .is_err()
        );
        // a group delay longer than the data is ignored with a note, not a panic
        let (_, rec) = process_fid(
            &fid,
            &params(1000.0, 100.0),
            &StoredProcessing::default(),
            &ProcessOptions::default(),
        )
        .unwrap();
        assert!(!rec.notes.is_empty());
    }

    #[test]
    fn stored_phases_need_evidence() {
        let mut s = StoredProcessing {
            phase0_deg: Some(0.0),
            phase1_deg: Some(0.0),
            ..StoredProcessing::default()
        };
        assert_eq!(s.usable_phases(), None);
        s.spectrum_stored = true;
        assert_eq!(s.usable_phases(), Some((0.0, 0.0)));
        s.phase0_deg = Some(12.0);
        s.spectrum_stored = false;
        assert_eq!(s.usable_phases(), Some((12.0, 0.0)));
    }
}
