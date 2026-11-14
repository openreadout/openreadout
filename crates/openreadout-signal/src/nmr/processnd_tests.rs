//! Synthetic 2D and 3D FIDs for every encoding of [`super::processnd`].

use crate::fft::Complex;
use crate::nmr::lp::{LinearPrediction, LpDirection};
use crate::nmr::process::{FidParameters, PhaseMode};
use crate::nmr::processnd::*;
use crate::nmr::window::Window;

const SW2: f64 = 2000.0;
const SW1: f64 = 1000.0;
// on the frequency grids of the transforms (512 and 128 points), so the dispersive parts
// vanish at the top of the line
const F2: f64 = 76.0 * SW2 / 512.0;
const F1: f64 = 25.0 * SW1 / 128.0;

fn direct() -> FidParameters {
    FidParameters {
        spectral_width_hz: SW2,
        carrier_frequency_mhz: 400.0,
        reference_frequency_mhz: 400.0,
        group_delay_points: 0.0,
        conjugate: false,
        nucleus: Some("1H".into()),
    }
}

fn dim(encoding: Encoding, rows: usize) -> DimensionParameters {
    DimensionParameters {
        nucleus: Some("13C".into()),
        spectral_width_hz: SW1,
        carrier_frequency_mhz: 100.0,
        reference_frequency_mhz: 100.0,
        rows,
        encoding,
        ..DimensionParameters::default()
    }
}

/// F2 signal (complex), `n` points.
fn s2(n: usize) -> Vec<Complex> {
    (0..n)
        .map(|i| {
            let t = i as f64 / SW2;
            Complex::from_phase(2.0 * std::f64::consts::PI * F2 * t).scale((-t / 0.05).exp())
        })
        .collect()
}

fn mul(v: &[Complex], z: Complex) -> Vec<Complex> {
    v.iter().map(|x| *x * z).collect()
}

fn scaled(v: &[Complex], k: f64) -> Vec<Complex> {
    v.iter().map(|x| x.scale(k)).collect()
}

/// Rows of a 2D data set with one peak at (F1, F2) Hz from the carriers.
fn rows_for(enc: Encoding, m: usize) -> Vec<Vec<Complex>> {
    let s = s2(256);
    let i = Complex::new(0.0, 1.0);
    let w1 = 2.0 * std::f64::consts::PI * F1;
    let mut out = Vec::new();
    for k in 0..m {
        match enc {
            Encoding::States | Encoding::StatesTppi | Encoding::EchoAntiecho => {
                let t = k as f64 / SW1;
                let a = (-t / 0.03).exp();
                let sg = if enc == Encoding::StatesTppi && k % 2 == 1 {
                    -1.0
                } else {
                    1.0
                };
                let c = scaled(&s, (w1 * t).cos() * a * sg);
                let d = scaled(&s, (w1 * t).sin() * a * sg);
                if enc == Encoding::EchoAntiecho {
                    let ic = mul(&c, i);
                    out.push(ic.iter().zip(&d).map(|(p, q)| (-*p - *q).scale(0.5)).collect());
                    out.push(ic.iter().zip(&d).map(|(p, q)| (-*p + *q).scale(0.5)).collect());
                } else {
                    out.push(c);
                    out.push(d);
                }
            }
            Encoding::Complex => {
                let t = k as f64 / SW1;
                out.push(mul(&s, Complex::from_phase(-w1 * t).scale((-t / 0.03).exp())));
            }
            Encoding::Tppi | Encoding::Sequential => {
                // real points at twice the rate, frequency counted from the low edge
                let nu = SW1 / 2.0 - F1;
                let t = k as f64 / (2.0 * SW1);
                let a = (-t / 0.03).exp();
                out.push(scaled(
                    &s,
                    (2.0 * std::f64::consts::PI * nu * t).cos() * a,
                ));
            }
            Encoding::Series => out.push(scaled(&s, 1.0 - 0.1 * k as f64)),
        }
    }
    out
}

fn opts(phased: bool) -> NdOptions {
    let d = DimensionOptions {
        window: Some(Window::None),
        phase: if phased {
            PhaseMode::Manual {
                phase0_deg: 0.0,
                phase1_deg: 0.0,
            }
        } else {
            PhaseMode::Default
        },
        ..DimensionOptions::default()
    };
    NdOptions {
        direct: d.clone(),
        indirect: vec![d],
        keep_components: true,
        ..NdOptions::default()
    }
}

fn argmax(v: &[f64]) -> usize {
    v.iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |x| x.0)
}

fn run(rows: &[Vec<Complex>], dims: &[DimensionParameters], o: &NdOptions) -> SpectrumNd {
    process_nd(rows, &direct(), &StoredDimension::default(), dims, o).unwrap()
}

#[test]
fn every_encoding_puts_the_peak_where_it_belongs() {
    for enc in [
        Encoding::States,
        Encoding::StatesTppi,
        Encoding::EchoAntiecho,
        Encoding::Complex,
        Encoding::Tppi,
        Encoding::Sequential,
    ] {
        let rows = rows_for(enc, 64);
        let phased = enc != Encoding::Complex;
        let s = run(&rows, &[dim(enc, rows.len())], &opts(phased));
        assert_eq!(s.ndim(), 2);
        let k = argmax(&s.real);
        let (i, j) = (k / s.shape[1], k % s.shape[1]);
        let ppm1 = s.axes[0].ppm(i as f64);
        let ppm2 = s.axes[1].ppm(j as f64);
        assert!(
            (ppm2 - F2 / 400.0).abs() <= 1.01 * s.axes[1].step_ppm.abs(),
            "{enc:?}: F2 {ppm2}"
        );
        assert!(
            (ppm1 - F1 / 100.0).abs() <= 1.01 * s.axes[0].step_ppm.abs(),
            "{enc:?}: F1 {ppm1} want {}",
            F1 / 100.0
        );
        assert!(s.real[k] > 0.0);
        if phased {
            // absorptive in both dimensions: the imaginary parts are small at the top
            assert!(!s.components.is_empty());
            for c in &s.components {
                assert!(
                    c.values[k].abs() < 0.1 * s.real[k],
                    "{enc:?} component {} = {} vs {}",
                    c.mask,
                    c.values[k],
                    s.real[k]
                );
            }
        } else {
            assert_eq!(s.record.dimensions[0].phase_mode, "magnitude");
        }
        // projections and traces pass through the peak
        let p = s.projection(1, Projection::Skyline).unwrap();
        assert_eq!(argmax(&p), j);
        let col = s.trace_at_ppm(0, &[ppm2]).unwrap();
        assert_eq!(argmax(&col), i);
        let row = s.trace_at_ppm(1, &[ppm1]).unwrap();
        assert_eq!(argmax(&row), j);
        assert!(s.projection(0, Projection::Sum).is_some());
        assert!(s.projection(0, Projection::NegativeSkyline).is_some());
        assert!(s.projection(2, Projection::Sum).is_none());
        assert_eq!(s.get(&[i, j]), Some(s.real[k]));
        assert_eq!(s.get(&[i]), None);
    }
}

#[test]
fn a_series_is_transformed_in_f2_only() {
    let rows = rows_for(Encoding::Series, 8);
    let mut d = dim(Encoding::Series, 8);
    d.series = Some(Series {
        name: "vdlist".into(),
        values: (0..8).map(f64::from).collect(),
        unit: Some("s".into()),
        source: None,
    });
    let s = run(&rows, &[d], &opts(true));
    assert_eq!(s.shape[0], 8);
    assert_eq!(s.series.as_ref().unwrap().name, "vdlist");
    assert_eq!(s.axes[0].step_ppm, 0.0);
    let r0 = s.trace(1, &[0]).unwrap();
    let r5 = s.trace(1, &[5]).unwrap();
    let (a, b) = (r0[argmax(&r0)], r5[argmax(&r0)]);
    assert!((b / a - 0.5).abs() < 1e-6, "{b} / {a}");
}

#[test]
fn three_dimensions_states_states() {
    let (m1, m2) = (8usize, 8usize);
    let s = s2(64);
    let w2 = 2.0 * std::f64::consts::PI * F1;
    let w1 = 2.0 * std::f64::consts::PI * 100.0;
    let mut rows = Vec::new();
    // the slowest dimension outer, the pairs of the faster one inner
    for k1 in 0..m2 {
        let t1 = k1 as f64 / SW1;
        for p1 in 0..2 {
            for k2 in 0..m1 {
                let t2 = k2 as f64 / SW1;
                for p2 in 0..2 {
                    let a = if p2 == 0 { (w2 * t2).cos() } else { (w2 * t2).sin() };
                    let b = if p1 == 0 { (w1 * t1).cos() } else { (w1 * t1).sin() };
                    rows.push(scaled(&s, a * b));
                }
            }
        }
    }
    let mut o = opts(true);
    o.indirect.push(o.indirect[0].clone());
    let dims = [dim(Encoding::States, 2 * m1), dim(Encoding::States, 2 * m2)];
    let r = run(&rows, &dims, &o);
    assert_eq!(r.ndim(), 3);
    let k = argmax(&r.real);
    let (n2, n3) = (r.shape[1], r.shape[2]);
    let (i, j, l) = (k / (n2 * n3), (k / n3) % n2, k % n3);
    assert!((r.axes[0].ppm(i as f64) - 1.0).abs() <= 1.01 * r.axes[0].step_ppm.abs());
    assert!((r.axes[1].ppm(j as f64) - F1 / 100.0).abs() <= 1.01 * r.axes[1].step_ppm.abs());
    assert!((r.axes[2].ppm(l as f64) - F2 / 400.0).abs() <= 1.01 * r.axes[2].step_ppm.abs());
    assert_eq!(r.components.len(), 7);
    assert_eq!(r.record.dimensions.len(), 3);
    assert_eq!(r.record.dimensions[0].label, "F1");
}

#[test]
fn linear_prediction_magnitude_and_symmetrisation_run() {
    let rows = rows_for(Encoding::States, 32);
    let mut d = dim(Encoding::States, rows.len());
    d.stored.linear_prediction = Some(LinearPrediction {
        direction: LpDirection::Forward,
        coefficients: 8,
        points: 128,
    });
    d.stored.phase_mode = Some(StoredPhaseMode::Magnitude);
    let mut o = opts(false);
    o.indirect[0].size = Some(256);
    o.direct.size = Some(256);
    o.symmetrize = true;
    let s = run(&rows, &[d], &o);
    assert_eq!(s.record.dimensions[0].points, 64);
    assert!(s.real.iter().all(|v| v.is_finite() && *v >= 0.0));
    assert!(s.record.steps.iter().any(|x| x == "symmetrize"));
    assert!(s.components.is_empty());
    // reversal flips the axis and the data
    let mut o2 = opts(true);
    o2.indirect[0].reverse = Some(true);
    let rows = rows_for(Encoding::States, 64);
    let a = run(&rows, &[dim(Encoding::States, 128)], &opts(true));
    let b = run(&rows, &[dim(Encoding::States, 128)], &o2);
    let (ka, kb) = (argmax(&a.real), argmax(&b.real));
    let (ia, ib) = (ka / a.shape[1], kb / b.shape[1]);
    assert_eq!(ia + ib, a.shape[0] - 1);
    assert!((a.axes[0].ppm(ia as f64) - b.axes[0].ppm(ib as f64)).abs() < 1e-9);
}

#[test]
fn bad_input_is_refused_cleanly() {
    let rows = rows_for(Encoding::States, 4);
    let st = StoredDimension::default();
    let no = NdOptions::default();
    let mut p = direct();
    p.spectral_width_hz = 0.0;
    assert!(process_nd(&rows, &p, &st, &[dim(Encoding::States, 8)], &no).is_err());
    assert!(process_nd(&[], &direct(), &st, &[dim(Encoding::States, 8)], &no).is_err());
    let mut d = dim(Encoding::States, 8);
    d.spectral_width_hz = f64::NAN;
    assert!(process_nd(&rows, &direct(), &st, &[d], &no).is_err());
    assert!(process_nd(&rows, &direct(), &st, &[dim(Encoding::States, 0)], &no).is_err());
    // fewer rows than the dimension says: zeros, with a note
    let s = process_nd(&rows, &direct(), &st, &[dim(Encoding::States, 16)], &no).unwrap();
    assert!(s.record.notes.iter().any(|n| n.contains("missing rows")));
    // a single row cannot form a pair
    assert!(process_nd(&rows[..1], &direct(), &st, &[dim(Encoding::States, 1)], &no).is_err());
    // non-finite samples are zeroed, not propagated
    let mut r = rows_for(Encoding::States, 8);
    r[3][5] = Complex::new(f64::NAN, f64::INFINITY);
    let s = process_nd(&r, &direct(), &st, &[dim(Encoding::States, 16)], &no).unwrap();
    assert!(s.real.iter().all(|v| v.is_finite()));
}
