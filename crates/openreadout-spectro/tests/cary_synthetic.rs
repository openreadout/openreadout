//! Synthetic Agilent (Varian) Cary files built from `docs/formats/agilent-cary.md`: the store
//! chain, spectrum and baseline stores with their texts, sets and the kinetics schedule, the
//! refusals of layouts not validated, and damage (clean errors, never a panic).
#![allow(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::many_single_char_names
)] // exact synthetic values; short names for spectra, texts and values

use openreadout_core::source::Input;
use openreadout_core::{Dataset, Error, FormatReader};
use openreadout_spectro::{CARY_FORMAT_ID, CaryReader};

fn open(name: &str, bytes: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    CaryReader.open_input(&Input::from_bytes(name, bytes))
}

fn text(b: &mut Vec<u8>, s: &str) {
    b.extend((s.len() as u32).to_le_bytes());
    b.extend(s.as_bytes());
}

/// One store: class name, size from the store's first byte, then `body`.
fn store(class: &str, body: &[u8]) -> Vec<u8> {
    let mut s = Vec::new();
    text(&mut s, class);
    let size = s.len() + 4 + body.len();
    s.extend((size as u32).to_le_bytes());
    s.extend(body);
    s
}

/// A spectrum (or baseline) store body: the 1,028-byte header, the (x, y) pairs, the texts.
fn spectrum_body(version: u32, xs: &[f32], ys: &[f32], texts: &[&str]) -> Vec<u8> {
    let mut h = vec![0u8; 1028];
    h[4..8].copy_from_slice(&version.to_le_bytes());
    let min = |v: &[f32]| v.iter().copied().fold(f32::INFINITY, f32::min);
    let max = |v: &[f32]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    h[8..12].copy_from_slice(&min(xs).to_le_bytes());
    h[12..16].copy_from_slice(&max(xs).to_le_bytes());
    h[16..20].copy_from_slice(&min(ys).to_le_bytes());
    h[20..24].copy_from_slice(&max(ys).to_le_bytes());
    h[24..28].copy_from_slice(&(xs.len() as u32).to_le_bytes());
    for (x, y) in xs.iter().zip(ys) {
        h.extend(x.to_le_bytes());
        h.extend(y.to_le_bytes());
    }
    for t in texts {
        text(&mut h, t);
    }
    h
}

fn file(stores: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 0x3E];
    b[0] = 0x11;
    b[1..0x12].copy_from_slice(b"Varian UV-VIS-NIR");
    for s in stores {
        b.extend(s);
    }
    b.extend([0u8; 4]);
    b
}

const XS: [f32; 5] = [500.0, 499.0, 498.0, 497.0, 496.0];

fn sample_texts<'a>(name: &'a str, time: &'a str, ymode: &'a str) -> Vec<&'a str> {
    vec![
        name,
        time,
        "Operator Name : analyst",
        "Scan Software Version: 3.00(182)",
        "Parameter List : ",
        "  Instrument                        Cary 4000",
        "  X Mode                            Nanometers",
        ymode,
        "  UV-Vis Scan Rate (nm/min)         600.000",
        "  Start (nm)                        500.00",
        "  Stop (nm)                         496.00",
        "Method Log : ",
        "Method Name : C:\\methods\\abs.msw",
        "End Method Modifications",
        "<SBW (nm)> , 2.000",
    ]
}

#[test]
fn one_spectrum() {
    let ys = [0.1f32, 0.2, 0.3, 0.25, 0.15];
    let t = sample_texts(
        "Au",
        "Collection Time: 12/18/2024 10:12:52 PM",
        "  Y Mode                            Abs",
    );
    let b = file(&[
        store("TContinuumStore", &spectrum_body(149, &XS, &ys, &t)),
        store("TGraphStore", &[0; 16]),
    ]);
    let mut ds = open("au.dsw", b).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format.id, CARY_FORMAT_ID);
    assert_eq!(info.traces.len(), 1);
    let tr = &info.traces[0];
    assert_eq!(tr.channels[0].name, "absorbance");
    assert_eq!(tr.channels[0].unit.as_deref(), Some("AU"));
    assert_eq!(tr.extra["axis"]["first"], 500.0);
    assert_eq!(tr.extra["axis"]["last"], 496.0);
    assert_eq!(tr.extra["axis"]["unit"], "nm");
    let v = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(
        v.channels[0],
        ys.iter().map(|&y| f64::from(y)).collect::<Vec<_>>()
    );
    let e = ds.experiment().unwrap();
    let i = e.instrument.unwrap();
    assert_eq!(i.model.as_deref(), Some("Cary 4000"));
    assert_eq!(i.software_version.as_deref(), Some("3.00(182)"));
    let a = e.acquisition.unwrap();
    assert_eq!(a.started_at.as_deref(), Some("2024-12-18T22:12:52"));
    assert_eq!(a.operator.as_deref(), Some("analyst"));
    assert!(ds.check().unwrap().ok);
    // the graph store is listed, not decoded
    assert!(
        ds.entries()
            .unwrap()
            .iter()
            .any(|e| e.name == "TGraphStore")
    );
}

#[test]
fn batch_with_baseline_and_schedule() {
    let mut stores = Vec::new();
    for (k, time) in [
        "Collection Time: 8/3/2020 11:49:14 AM",
        "Collection Time: 8/3/2020 11:50:08 AM",
    ]
    .iter()
    .enumerate()
    {
        let ys: Vec<f32> = (0..5).map(|i| (k * 10 + i) as f32).collect();
        let sched = format!("[Time] , {}.000", 2 * k);
        let mut t = sample_texts("s", time, "  Ordinate mode                     Abs");
        t.push(&sched);
        stores.push(store("TContinuumStore", &spectrum_body(149, &XS, &ys, &t)));
    }
    stores.push(store(
        "TBaselineStore",
        &spectrum_body(
            149,
            &XS,
            &[1.0; 5],
            &sample_texts(
                "Baseline 100%T",
                "Collection Time: 8/3/2020 11:40:00 AM",
                "  Y Mode   Abs",
            ),
        ),
    ));
    let mut ds = open("k.bsk", file(&stores)).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 2);
    assert_eq!(info.traces[0].sweep_count, 2);
    assert!(
        info.traces[1]
            .name
            .as_deref()
            .unwrap_or_default()
            .starts_with("baseline")
    );
    let v = ds.read_trace(0, 1, 0, u64::MAX).unwrap();
    assert_eq!(v.channels[0][0], 10.0);
    // per-spectrum times: collected 0 and 54 s, scheduled 0 and 2 min
    let t = ds.read_table(0, 0, 10).unwrap();
    let cols = &info.tables[0].columns;
    let col = |name: &str| cols.iter().position(|c| c.name == name).unwrap();
    assert_eq!(t.columns[col("collected_s")], vec![0.0, 54.0]);
    assert_eq!(t.columns[col("scheduled_min")], vec![0.0, 2.0]);
}

#[test]
fn refusals_and_damage() {
    let ys = [0.1f32; 5];
    let t = sample_texts(
        "x",
        "Collection Time: 1/2/2020 1:02:03 PM",
        "  Y Mode   Abs",
    );
    // an unvalidated store version
    let b = file(&[store("TContinuumStore", &spectrum_body(150, &XS, &ys, &t))]);
    assert!(matches!(open("x.dsw", b), Err(Error::Unsupported { .. })));
    // no spectrum store
    assert!(matches!(
        open("x.dsw", file(&[store("TGraphStore", &[0; 8])])),
        Err(Error::Unsupported { .. })
    ));
    // not a Cary file
    assert!(open("x.dsw", vec![0u8; 200]).is_err());
    // truncated anywhere, bytes flipped anywhere: a clean answer
    let good = file(&[store("TContinuumStore", &spectrum_body(149, &XS, &ys, &t))]);
    for cut in (0..good.len()).step_by(7) {
        if let Ok(mut ds) = open("x.dsw", good[..cut].to_vec()) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, u64::MAX);
        }
    }
    for at in (0..good.len()).step_by(3) {
        let mut b = good.clone();
        b[at] ^= 0xA5;
        if let Ok(mut ds) = open("x.dsw", b) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, u64::MAX);
            let _ = ds.vendor_metadata();
        }
    }
}
