//! Spectral bands and regions (`analyze peaks --x-range` on spectra): band detection and region
//! integration on the input read as little-endian f32 (x, y) pairs — unsorted, repeated, NaN and
//! infinite values included — with regions taken from the first values, as both absorbance-like
//! (maxima) and transmittance-like (minima) spectra.
#![no_main]

use libfuzzer_sys::fuzz_target;

use openreadout_quant::bands::{RegionBaseline, Spectrum, analyse, integrate_region};

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    let vals: Vec<f64> = data
        .chunks_exact(4)
        .map(|c| f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
        .take(1 << 16)
        .collect();
    let (x, y): (Vec<f64>, Vec<f64>) = vals.chunks_exact(2).map(|p| (p[0], p[1])).unzip();
    let ranges: Vec<[f64; 2]> = vals.chunks_exact(2).take(3).map(|p| [p[0], p[1]]).collect();
    for baseline in [RegionBaseline::Linear, RegionBaseline::None] {
        for minima in [false, true] {
            // unsorted input straight into the region integrator
            for r in &ranges {
                let _ = integrate_region(&x, &y, *r, baseline, minima);
            }
            // and as read_spectrum hands it over: finite, x ascending
            let mut pts: Vec<(f64, f64)> = x
                .iter()
                .zip(&y)
                .filter(|(a, b)| a.is_finite() && b.is_finite())
                .map(|(a, b)| (*a, *b))
                .collect();
            pts.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (xs, ys): (Vec<f64>, Vec<f64>) = pts.into_iter().unzip();
            let s = Spectrum {
                x: xs,
                y: ys,
                minima,
                x_quantity: "wavenumber".into(),
                ..Spectrum::default()
            };
            let _ = analyse(&s, &Default::default(), &ranges, baseline);
        }
    }
});
