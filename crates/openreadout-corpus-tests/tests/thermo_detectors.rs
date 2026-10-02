//! Thermo `.raw` detector controllers (UV/DAD channels, PDA field, analog channels) against
//! the evidence the files themselves hold. No independent reader or vendor export exists for
//! these controllers (see `docs/provenance/thermo-raw.md`, 2026-09-24), so the checks are:
//!
//! - the Vanquish DAD records single-wavelength channels *and* the 3-D field: each channel
//!   must equal the PDA column at the channel's wavelength (from the method text) to within the
//!   PDA's 1 µAU storage step;
//! - the index stores each sample's value (UV, analog) or each spectrum's total (PDA): `check`
//!   must find every sample equal to it;
//! - sampling rates equal the method's `Data_Collection_Rate` settings.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test thermo_detectors -- --nocapture`
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_core::FormatReader;
use openreadout_core::model::TraceInfo;
use openreadout_thermo::ThermoRawReader;

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../corpus/files")
                .canonicalize()
                .unwrap()
        },
        PathBuf::from,
    )
}

fn column(ds: &mut Box<dyn openreadout_core::Dataset>, t: &TraceInfo, c: usize) -> Vec<f64> {
    let tr = ds.read_trace(t.index, 0, 0, t.sample_count).unwrap();
    tr.channels[c].clone()
}

#[test]
fn vanquish_dad_channels_equal_the_pda_field() {
    let path = corpus_dir().join("zenodo19222374-cannabis-neg-h1-1.raw");
    if !path.exists() {
        eprintln!(
            "skip: {} missing (cargo xtask corpus fetch --tier standard --only zenodo19222374)",
            path.display()
        );
        return;
    }
    let mut ds = ThermoRawReader.open(&path).unwrap();
    let info = ds.info().unwrap();
    let pda = info
        .traces
        .iter()
        .find(|t| t.extra.get("detector").and_then(|v| v.as_str()) == Some("pda"))
        .expect("a PDA trace")
        .clone();
    assert_eq!(pda.sample_count, 28_800);
    assert_eq!(pda.channels.len(), 76, "76 wavelengths 200..500 step 4");
    let wl: Vec<f64> = pda
        .channels
        .iter()
        .map(|c| c.extra["wavelength_nm"].as_f64().unwrap())
        .collect();
    let pda_all = ds.read_trace(pda.index, 0, 0, pda.sample_count).unwrap();
    let mut compared = 0;
    for t in info
        .traces
        .iter()
        .filter(|t| t.name.as_deref().is_some_and(|n| n.starts_with("UV_VIS")))
    {
        let w = t.channels[0].extra["wavelength_nm"].as_f64().unwrap();
        let uv = column(&mut ds, t, 0);
        assert_eq!(
            t.extra["axis"], pda.extra["axis"],
            "{:?}: same time grid",
            t.name
        );
        assert!(
            (t.sample_rate_hz - 20.0).abs() < 1e-6,
            "UV.Data_Collection_Rate 20 Hz"
        );
        let on_grid = wl.iter().position(|&x| (x - w).abs() < 1e-9);
        let reference: Vec<f64> = if let Some(k) = on_grid {
            pda_all.channels[k].clone()
        } else {
            // between two grid points (254 nm): their mean
            let k = wl.iter().position(|&x| x > w).unwrap();
            pda_all.channels[k - 1]
                .iter()
                .zip(&pda_all.channels[k])
                .map(|(a, b)| (a + b) / 2.0)
                .collect()
        };
        let max_diff = uv
            .iter()
            .zip(&reference)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f64, f64::max);
        let n = uv.len() as f64;
        let (ma, mb) = (
            uv.iter().sum::<f64>() / n,
            reference.iter().sum::<f64>() / n,
        );
        let cov: f64 = uv
            .iter()
            .zip(&reference)
            .map(|(a, b)| (a - ma) * (b - mb))
            .sum();
        let va: f64 = uv.iter().map(|a| (a - ma).powi(2)).sum();
        let vb: f64 = reference.iter().map(|b| (b - mb).powi(2)).sum();
        let corr = cov / (va * vb).sqrt();
        println!(
            "{:<9} {w:>5} nm: max |channel − PDA| = {max_diff:.5} mAU over {} samples, r = {corr:.8}",
            t.name.as_deref().unwrap_or(""),
            uv.len()
        );
        if on_grid.is_some() {
            assert!(max_diff <= 0.0011, "{:?}: {max_diff}", t.name);
        } else {
            assert!(corr > 0.999, "{:?}: r = {corr}", t.name);
        }
        compared += 1;
    }
    assert_eq!(compared, 4, "UV_VIS_1, 2, 3 and 8 were acquired");
    let cad = info
        .traces
        .iter()
        .find(|t| t.name.as_deref() == Some("CAD_1"))
        .unwrap();
    assert!(
        (cad.sample_rate_hz - 2.0).abs() < 1e-6,
        "CAD.Data_Collection_Rate 2 Hz"
    );
    let rep = ds.check().unwrap();
    assert!(
        !rep.findings.iter().any(|f| f.code.starts_with("detector")),
        "{:?}",
        rep.findings
    );
}

#[test]
fn analog_controllers_of_other_instruments() {
    for (file, want) in [
        (
            "mtbls1820-lumos-uplc-31.raw",
            vec!["CC_Temp", "Pump_Pressure"],
        ),
        ("mtbls797-dotsha05.raw", vec!["Thermo Exactive Orbitrap"]),
    ] {
        let path = corpus_dir().join(file);
        if !path.exists() {
            eprintln!("skip: {file} missing");
            continue;
        }
        let mut ds = ThermoRawReader.open(&path).unwrap();
        let info = ds.info().unwrap();
        let names: Vec<String> = info.traces.iter().filter_map(|t| t.name.clone()).collect();
        assert_eq!(names, want, "{file}");
        for t in &info.traces {
            let tr = ds.read_trace(t.index, 0, 0, t.sample_count).unwrap();
            assert_eq!(tr.channels[0].len() as u64, t.sample_count);
            assert_eq!(
                t.channels[0].name, "time",
                "analog channels carry their times"
            );
            if t.name.as_deref() == Some("CC_Temp") {
                // ColumnComp.CC.Temperature.Nominal: 40.00 [°C]
                assert!(tr.channels[1].iter().all(|v| (v - 40.0).abs() < 0.1));
                assert_eq!(t.channels[1].unit.as_deref(), Some("°C"));
            }
        }
        let rep = ds.check().unwrap();
        assert!(
            !rep.findings.iter().any(|f| f.code.starts_with("detector")),
            "{file}: {:?}",
            rep.findings
        );
    }
}

/// The first independent reference: GNPS/MassIVE's msconvert mzML of MTBLS773 (an Accela PDA
/// detector on an LTQ Orbitrap XL, file version 63), summarized by `oracle/thermo_detectors.py`
/// into `corpus/oracle/mtbls773-001-blank-start.json` → `detector_export`.
///
/// - every PDA spectrum (10,501 × 401 stored integers) must equal the export's, and their times;
/// - the export's `UV 1` chromatogram is channel A: its values must equal ours; its times are one
///   channel interval (0.1 s) later than ours (the export numbers samples from 1; channel A on
///   our grid lines up with the PDA's 280 nm band, see the provenance log);
/// - `PDA 1` is each spectrum's total over its 401 points.
#[test]
fn accela_pda_matches_an_independent_conversion() {
    let id = "mtbls773-001-blank-start";
    let path = corpus_dir().join(format!("{id}.raw"));
    if !path.exists() {
        eprintln!(
            "skip: {} missing (cargo xtask corpus fetch --tier standard --only mtbls773)",
            path.display()
        );
        return;
    }
    let oracle_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus/oracle")
        .join(format!("{id}.json"));
    let oracle: serde_json::Value = serde_json::from_str(
        &openreadout_corpus_tests::oracle_json::read_to_string(&oracle_path).unwrap(),
    )
    .unwrap();
    let ex = &oracle["detector_export"];
    let spectra = &ex["detector_spectra"][0];
    let chrom = |name: &str| {
        ex["chromatograms"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == name)
            .unwrap()
            .clone()
    };
    let f = |v: &serde_json::Value| v.as_f64().unwrap();

    let mut ds = ThermoRawReader.open(&path).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(
        info.traces.len(),
        2,
        "the PDA field and the channel controller"
    );
    let pda = info
        .traces
        .iter()
        .find(|t| t.extra.get("detector").and_then(|v| v.as_str()) == Some("pda"))
        .unwrap()
        .clone();
    let uv = info
        .traces
        .iter()
        .find(|t| t.extra.get("detector").and_then(|v| v.as_str()) == Some("channel"))
        .unwrap()
        .clone();

    // PDA spectra: count, wavelength grid, times, every value.
    let n = spectra["spectrum_count"].as_u64().unwrap();
    assert_eq!(pda.sample_count, n);
    let wl = &spectra["wavelengths_nm"];
    assert_eq!(pda.channels.len() as u64, wl["n"].as_u64().unwrap());
    assert_eq!(pda.channels[0].extra["wavelength_nm"], f(&wl["first"]));
    assert_eq!(
        pda.channels.last().unwrap().extra["wavelength_nm"],
        f(&wl["last"])
    );
    let times = &spectra["times"];
    let step_s = 1.0 / pda.sample_rate_hz;
    assert!((pda.start_s.unwrap() - f(&times["first_min"]) * 60.0).abs() < 1e-9);
    assert!((step_s - f(&times["step_min"]) * 60.0).abs() < 1e-9);
    assert!(f(&times["max_departure_min"]) < 1e-9);
    let all = ds.read_trace(pda.index, 0, 0, n).unwrap();
    let scale = pda.channels[0].scale;
    let mut bytes = Vec::with_capacity((n as usize) * all.channels.len() * 4);
    let mut sums = Vec::with_capacity(n as usize);
    let mut off_integer = 0.0f64;
    for k in 0..n as usize {
        let mut s = 0i64;
        for c in &all.channels {
            let stored = c[k] / scale;
            off_integer = off_integer.max((stored - stored.round()).abs());
            #[allow(clippy::cast_possible_truncation)]
            let v = stored.round() as i32;
            s += i64::from(v);
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        sums.push(s);
    }
    assert!(off_integer < 1e-6, "PDA values are stored integers × scale");
    let hash = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
    assert_eq!(
        hash,
        spectra["intensity"]["xxh3_i32"].as_str().unwrap(),
        "every PDA value (spectrum after spectrum) equals the export's"
    );
    for s in spectra["sample_spectra"].as_array().unwrap() {
        let k = s[0].as_u64().unwrap() as usize;
        for (c, v) in all.channels.iter().zip(s[1].as_array().unwrap()) {
            assert!((c[k] / scale - f(v)).abs() < 1e-6, "spectrum {k}");
        }
    }
    let total: i64 = sums.iter().sum();
    #[allow(clippy::cast_precision_loss)]
    let total = total as f64;
    assert!((total - f(&spectra["intensity"]["sum"])).abs() < 0.5);
    println!(
        "PDA: {n} spectra × {} wavelengths equal to the export (xxh3 {hash}), times on its grid",
        all.channels.len()
    );

    // `PDA 1`: spectrum total over the point count, as float32.
    let p1 = chrom("PDA 1");
    #[allow(clippy::cast_precision_loss)]
    let pts = all.channels.len() as f64;
    let mut f32_bytes = Vec::with_capacity(sums.len() * 4);
    for s in &sums {
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let v = (*s as f64 / pts) as f32;
        f32_bytes.extend_from_slice(&v.to_le_bytes());
    }
    assert_eq!(
        format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&f32_bytes)),
        p1["values"]["xxh3_f32"].as_str().unwrap(),
        "PDA 1 = spectrum total / {pts}"
    );

    // `UV 1` = channel A.
    let u1 = chrom("UV 1");
    assert_eq!(uv.sample_count, u1["values"]["n"].as_u64().unwrap());
    assert_eq!(uv.channels[0].name, "Channel A");
    assert_eq!(uv.channels[0].extra["wavelength_nm"], 280.0);
    assert_eq!(uv.channels[1].extra["wavelength_nm"], 365.0);
    assert_eq!(uv.channels[2].extra["wavelength_nm"], 520.0);
    assert!(
        (uv.sample_rate_hz - 10.0).abs() < 1e-9,
        "Channel sample rate (Hz): 10"
    );
    assert!((pda.sample_rate_hz - 5.0).abs() < 1e-9, "Scan Rate (Hz): 5");
    let ch = ds.read_trace(uv.index, 0, 0, uv.sample_count).unwrap();
    let a_scale = uv.channels[0].scale;
    let mut a_bytes = Vec::with_capacity(ch.channels[0].len() * 4);
    for v in &ch.channels[0] {
        #[allow(clippy::cast_possible_truncation)]
        let stored = (v / a_scale).round() as i32;
        a_bytes.extend_from_slice(&stored.to_le_bytes());
    }
    assert_eq!(
        format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&a_bytes)),
        u1["values"]["xxh3_i32"].as_str().unwrap(),
        "UV 1 = channel A, value for value"
    );
    let t = &u1["times"];
    let interval_min = 1.0 / (uv.sample_rate_hz * 60.0);
    let shift = f(&t["first_min"]) - uv.start_s.unwrap() / 60.0;
    assert!(
        (shift - interval_min).abs() < 1e-6,
        "the export's UV times are one sample later than ours ({shift} min)"
    );
    assert!((f(&t["step_min"]) - interval_min).abs() < 1e-8);
    println!(
        "UV 1: {} samples equal to channel A; export times = ours + {:.4} s",
        uv.sample_count,
        shift * 60.0
    );
    let rep = ds.check().unwrap();
    assert!(
        !rep.findings.iter().any(|f| f.code.starts_with("detector")),
        "{:?}",
        rep.findings
    );
}
