//! The Seahorse XF level and rate formulas (`openreadout_biophys::{oxygen_levels, ph_level,
//! oxygen_consumption, acidification}`) against a Wave Excel export of a second depositor
//! (`seahorse-seahtrue-pbmc-export`: seahtrue's test workbook, no `.asyr`), recorded by
//! `oracle/seahorse_oracle.py --wave-xlsx` into `corpus/oracle/seahorse/<id>.json`: per reading
//! and well the corrected O2 and pH emissions with Wave's O2 (mmHg) and pH, and Wave's OCR,
//! ECAR and PER per well and measurement.
//!
//! - O2 and pH levels: equal to Wave's to 1e-9 for every reading and well.
//! - OCR within 0.35 pmol/min and ECAR within 0.2 mpH/min of Wave's: the export's time stamps are
//!   rounded to whole seconds (the `.asyr` files' are not; there the agreement is 0.17 pmol/min
//!   and 5e-6 mpH/min, `tests/seahorse_oracle`).
//! - PER = uncorrected ECAR × buffer factor × plate volume × kVol within the ECAR tolerance
//!   scaled by those constants.
//!
//! The export prints Ksv and FO but not the ambient O2 in mM (`COb`); 0.214 mM, the value every
//! `.asyr` file stores, is used, and the ambient level in mmHg follows from FO = 12500 (1 + Ksv CO).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test seahorse_rates -- --nocapture`
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_biophys::{
    OxygenModel, acidification, oxygen_consumption, oxygen_levels, ph_level,
};
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(f64::NAN)
}

fn grid(v: &Value) -> Vec<Vec<f64>> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_array().unwrap().iter().map(f).collect())
        .collect()
}

#[test]
#[allow(clippy::too_many_lines)]
fn wave_export_levels_and_rates() {
    let p = root().join("corpus/oracle/seahorse/seahorse-seahtrue-pbmc-export.json");
    let o: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    let wells: Vec<String> = o["wells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .collect();
    let bg: Vec<usize> = o["background"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            wells
                .iter()
                .position(|w| Some(w.as_str()) == b.as_str())
                .unwrap()
        })
        .collect();
    let c = &o["constants"];
    assert_eq!(c["calculation_method"], "AKOS");
    let (ksv, fo) = (f(&c["ksv"]), f(&c["fo"]));
    let model = OxygenModel {
        f_zero: fo,
        ksv,
        ambient_mmhg: (fo / 12_500.0 - 1.0) / ksv,
        ambient_mm: 0.214,
        tau_ac: f(&c["tau_ac"]),
        tau_aw: f(&c["tau_aw"]),
        tau_w: f(&c["tau_w"]),
        tau_c: f(&c["tau_c"]),
        tau_p: f(&c["tau_p"]),
        chamber_ul: f(&c["chamber_ul"]),
    };
    let gain: Vec<f64> = c["gain"].as_array().unwrap().iter().map(f).collect();
    assert_eq!((gain[0], gain[1]), (0.0, 0.0));
    let ph_cal = f(&c["ph_cal"]);
    let offset = c["edge_offset"].as_u64().unwrap() as usize;
    let o2e = grid(&o["o2_emission"]);
    let o2l = grid(&o["o2_level"]);
    let phe = grid(&o["ph_emission"]);
    let phl = grid(&o["ph_level"]);
    let cal: Vec<f64> = o["ph_calibration_emission"]
        .as_array()
        .unwrap()
        .iter()
        .map(f)
        .collect();
    let times: Vec<f64> = o["time_s"].as_array().unwrap().iter().map(f).collect();
    // levels
    let mut worst = (0.0_f64, 0.0_f64);
    let mut ours_o2 = Vec::new();
    let mut ours_ph = Vec::new();
    for t in 0..times.len() {
        let lo = oxygen_levels(&o2e[t], &bg, &model);
        let lp: Vec<f64> = phe[t]
            .iter()
            .zip(&cal)
            .map(|(e, k)| ph_level(*e, *k, gain[2], gain[3], ph_cal))
            .collect();
        for w in 0..wells.len() {
            worst.0 = worst.0.max((lo[w] - o2l[t][w]).abs());
            worst.1 = worst.1.max((lp[w] - phl[t][w]).abs());
        }
        ours_o2.push(lo);
        ours_ph.push(lp);
    }
    println!(
        "levels: O2 max |Δ| {:.2e} mmHg, pH max |Δ| {:.2e}",
        worst.0, worst.1
    );
    assert!(
        worst.0 < 1e-9 && worst.1 < 1e-9,
        "levels differ from Wave's: {worst:?}"
    );
    // measurements
    let meas: Vec<u64> = o["measurement"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    let nm = *meas.iter().max().unwrap() as usize;
    let spans: Vec<(usize, usize)> = (1..=nm as u64)
        .map(|m| {
            let first = meas.iter().position(|x| *x == m).unwrap();
            let last = meas.iter().rposition(|x| *x == m).unwrap();
            (first, last)
        })
        .collect();
    let column = |g: &Vec<Vec<f64>>, w: usize| -> Vec<f64> { g.iter().map(|r| r[w]).collect() };
    let raw_ecar: Vec<Vec<f64>> = (0..wells.len())
        .map(|w| {
            let p = column(&ours_ph, w);
            spans
                .iter()
                .map(|&s| acidification(&times, &p, s, offset))
                .collect()
        })
        .collect();
    let bf: Vec<f64> = o["buffer_factor"]
        .as_array()
        .unwrap()
        .iter()
        .map(f)
        .collect();
    let kvol = f(&c["kvol"]);
    let volume = f(&c["plate_volume"]);
    let (mut max_ocr, mut max_ecar, mut max_per, mut n) = (0.0_f64, 0.0_f64, 0.0_f64, 0usize);
    for (w, name) in wells.iter().enumerate() {
        if bg.contains(&w) {
            continue;
        }
        let ocr = oxygen_consumption(&times, &column(&ours_o2, w), &spans, &model);
        let rates = o["rates"][name].as_array().unwrap();
        for m in 0..nm {
            let wave = rates[m].as_array().unwrap();
            let bg_ecar: f64 = bg.iter().map(|&b| raw_ecar[b][m]).sum::<f64>() / bg.len() as f64;
            let ecar = raw_ecar[w][m] - bg_ecar;
            max_ocr = max_ocr.max((ocr[m] - f(&wave[1])).abs());
            max_ecar = max_ecar.max((ecar - f(&wave[2])).abs());
            if bf[w] > 0.0 {
                let per = raw_ecar[w][m] * bf[w] * volume * kvol;
                max_per = max_per.max((per - f(&wave[3])).abs() / (bf[w] * volume * kvol));
            }
            n += 1;
        }
    }
    println!(
        "{n} well-measurements: OCR max |Δ| {max_ocr:.3} pmol/min, ECAR {max_ecar:.4} mpH/min, PER {max_per:.4} (in ECAR units)"
    );
    assert!(max_ocr < 0.35, "OCR differs from Wave's by {max_ocr}");
    assert!(max_ecar < 0.2, "ECAR differs from Wave's by {max_ecar}");
    assert!(max_per < 0.2, "PER differs from Wave's by {max_per}");
}
