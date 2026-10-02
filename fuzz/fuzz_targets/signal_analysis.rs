//! Signal analyses on arbitrary samples and parameters: NMR processing (group delay, apodization,
//! FFT, phasing, baseline), peak picking and integration, the JEOL digital-filter parameter
//! parser, action-potential detection and passive fits, band-pass filtering and extracellular
//! detection. None of them may panic; outputs must stay finite where documented.
#![no_main]

use libfuzzer_sys::fuzz_target;

use openreadout_signal::ephys::{ap, extracellular, filter, passive};
use openreadout_signal::nmr::{
    self, source::jeol_group_delay, BaselineMode, FidParameters, PeakOptions, PhaseMode,
    ProcessOptions, StoredProcessing,
};
use openreadout_signal::Complex;

fn f64s(data: &[u8]) -> Vec<f64> {
    data.chunks_exact(4)
        .map(|c| f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
        .take(1 << 14)
        .collect()
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 16 {
        return;
    }
    let (head, body) = data.split_at(16);
    let x = f64s(body);
    let text = String::from_utf8_lossy(&head[..8]);
    let _ = jeol_group_delay(&text, &String::from_utf8_lossy(&head[8..]));

    // NMR
    let fid: Vec<Complex> = x.chunks_exact(2).map(|c| Complex::new(c[0], c[1])).collect();
    let p = FidParameters {
        spectral_width_hz: f64::from(head[0]) * 100.0 + 1.0,
        carrier_frequency_mhz: 400.0 + f64::from(head[1]) * 1e-4,
        reference_frequency_mhz: 400.0,
        group_delay_points: f64::from(head[2]) / 3.0,
        conjugate: head[3] & 1 == 1,
        nucleus: None,
    };
    let stored = StoredProcessing {
        size: Some(usize::from(head[4]) * 16),
        phase0_deg: Some(f64::from(head[5])),
        phase1_deg: Some(f64::from(head[6]) - 128.0),
        line_broadening_hz: Some(f64::from(head[7]) / 10.0),
        ..StoredProcessing::default()
    };
    let phase = match head[8] % 4 {
        0 => PhaseMode::Default,
        1 => PhaseMode::Auto,
        2 => PhaseMode::Magnitude,
        _ => PhaseMode::None,
    };
    let opts = ProcessOptions {
        size: Some(64 + usize::from(head[9]) * 8),
        phase,
        baseline: if head[10] & 1 == 1 {
            BaselineMode::Polynomial { order: u32::from(head[10] % 9) }
        } else {
            BaselineMode::None
        },
        ..ProcessOptions::default()
    };
    if let Ok((s, _)) = nmr::process_fid(&fid, &p, &stored, &opts) {
        let (peaks, _) = nmr::pick_peaks(&s.real, &s.axis, &PeakOptions::default());
        for pk in &peaks {
            assert!(pk.ppm.is_finite());
        }
        let _ = nmr::integrate(&s.real, &s.axis, &[(10.0, -2.0), (1.0, 1.0)], Some((1, 2.0)));
    }

    // electrophysiology
    let fs = f64::from(head[11]) * 1000.0 + 1.0;
    let _ = ap::detect(&x, fs, 0, x.len(), 0, &ap::ApSettings::default());
    let n = x.len();
    let _ = passive::current_step(&x, fs, n / 4, n / 2, -50.0);
    let _ = passive::test_pulse(&x, fs, n / 4, n / 2, 10.0);
    if let Some(sos) = filter::bandpass_sos(usize::from(head[12] % 8), 300.0, 6000.0, 30000.0) {
        let y = filter::filtfilt(&sos, &x);
        let noise = extracellular::mad_noise(&y);
        let _ = extracellular::detect_peaks(&y, 30000.0, noise, &extracellular::DetectSettings::default());
    }
});
