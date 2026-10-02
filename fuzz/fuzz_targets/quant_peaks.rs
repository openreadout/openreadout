//! Quantitation: the compound-list parser (CSV/TSV/JSON) on the input as text, and peak
//! detection + integration (every baseline mode) and manual integration on the input read as a
//! chromatogram of little-endian f32 (time, intensity) pairs — unsorted, repeated, NaN and
//! infinite values included.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = openreadout_quant::targets::parse_compounds(text);
    }
    let vals: Vec<f64> = data
        .chunks_exact(4)
        .map(|c| f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
        .take(1 << 16)
        .collect();
    let (t, y): (Vec<f64>, Vec<f64>) = vals.chunks_exact(2).map(|p| (p[0], p[1])).unzip();
    for mode in ["auto", "drop", "valley", "tangent"] {
        let mut p = openreadout_quant::peaks::PeakParams::default();
        p.baseline = mode.parse().unwrap();
        if let Ok(tab) = openreadout_quant::peaks::find_peaks(&t, &y, &p) {
            for pk in &tab.peaks {
                let _ = openreadout_quant::peaks::integrate_range(
                    &t,
                    &y,
                    pk.start_min,
                    pk.end_min,
                    None,
                    &p,
                );
            }
        }
    }
    if let (Some(a), Some(b)) = (t.first(), t.last()) {
        let _ = openreadout_quant::peaks::integrate_range(&t, &y, *a, *b, None, &Default::default());
    }
});
