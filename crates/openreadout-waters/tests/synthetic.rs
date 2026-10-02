//! A synthetic MassLynx `.raw` directory with a TOF MS function, a TOF MS/MS function and a
//! lock-spray reference function (12-byte values, 22-byte index records, `.STS` statistics):
//! spectra order, calibration, precursors, TIC/BPC traces, `check` and truncation.

use std::path::Path;
use std::sync::Arc;

use openreadout_core::FormatReader;
use openreadout_core::reader::Dataset;
use openreadout_core::source::{Fs, Input, MemFs, MemSource};
use openreadout_waters::{WatersRawReader, decode_mass27};

/// The 12-byte mass word of `m` (5-bit exponent, 27-bit mantissa with its top bit set).
fn mass_word(m: f64) -> u32 {
    let e = m.log2().floor() as i32 + 1;
    let mant = (m * 2f64.powi(27 - e)).round() as u32;
    ((e as u32) << 27) | mant
}

/// The packed 22/10 intensity word of a small integer.
fn intensity_word(v: u32) -> u32 {
    (21 << 22) | v
}

fn idx_record(offset: u32, count: u32, tic: f32, rt: f32) -> Vec<u8> {
    let mut r = Vec::with_capacity(22);
    r.extend_from_slice(&offset.to_le_bytes());
    r.extend_from_slice(&((32 << 22) | count).to_le_bytes());
    r.extend_from_slice(&tic.to_le_bytes());
    r.extend_from_slice(&rt.to_le_bytes());
    r.extend_from_slice(&[0u8; 6]);
    r
}

fn values(peaks: &[(f64, u32)]) -> Vec<u8> {
    let mut b = Vec::new();
    for &(m, i) in peaks {
        b.extend_from_slice(&intensity_word(i).to_le_bytes());
        b.extend_from_slice(&mass_word(m).to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
    }
    b
}

/// A `.STS` file with the fields `Set Mass` (f32) and `Collision Energy` (f32), one record per
/// value pair.
fn stats(records: &[(f32, f32)]) -> Vec<u8> {
    let n_fields = 2u16;
    let header_len = 32 + 48 * n_fields;
    let mut b = vec![0u8; header_len as usize];
    b[0..2].copy_from_slice(&header_len.to_le_bytes());
    b[2..4].copy_from_slice(&1u16.to_le_bytes());
    b[4..6].copy_from_slice(&8u16.to_le_bytes());
    b[6..8].copy_from_slice(&n_fields.to_le_bytes());
    for (k, (name, off)) in [("Set Mass", 0u16), ("Collision Energy", 4)]
        .iter()
        .enumerate()
    {
        let d = 32 + 48 * k;
        b[d..d + 2].copy_from_slice(&(k as u16 + 1).to_le_bytes());
        b[d + 2..d + 4].copy_from_slice(&3u16.to_le_bytes());
        b[d + 4..d + 6].copy_from_slice(&off.to_le_bytes());
        b[d + 6..d + 6 + name.len()].copy_from_slice(name.as_bytes());
        b[d + 32..d + 34].copy_from_slice(&4u16.to_le_bytes());
    }
    for &(a, c) in records {
        b.extend_from_slice(&a.to_le_bytes());
        b.extend_from_slice(&c.to_le_bytes());
    }
    b
}

fn function_block(code: u16, start: f32, end: f32, lo: f32, hi: f32) -> Vec<u8> {
    let mut b = vec![0u8; 416];
    b[0..2].copy_from_slice(&code.to_le_bytes());
    b[10..14].copy_from_slice(&start.to_le_bytes());
    b[14..18].copy_from_slice(&end.to_le_bytes());
    b[0xA0..0xA4].copy_from_slice(&lo.to_le_bytes());
    b[0x120..0x124].copy_from_slice(&hi.to_le_bytes());
    b
}

fn write_raw(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("_HEADER.TXT"),
        "$$ Version: 01.00\r\n$$ Acquired Name: synthetic\r\n$$ Acquired Date: 23-Sep-2026\r\n$$ Acquired Time: 10:11:12\r\n$$ Instrument: SYNAPT-XS#AB123\r\n$$ Cal Function 1: 0.5,1.0,T0\r\n$$ Cal Function 2: 0.5,1.0,T0\r\n$$ Cal Function 3: 0.0,1.0,T0\r\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("_extern.inf"),
        "Created by Masslynx v4.2\nPolarity\t\t\tES+\nFunction Parameters - Function 1 - TOF MS FUNCTION\nData Format\t\t\tCentroid\nFunction Parameters - Function 2 - TOF MSMS FUNCTION\nFunction Parameters - Function 3 - REFERENCE\n[LOCK SPRAY]\nLock Mass\t\t556.2766\n",
    )
    .unwrap();
    let mut functs = function_block(0x12, 0.0, 5.0, 50.0, 400.0);
    functs.extend(function_block(0x10, 0.0, 5.0, 100.0, 600.0));
    functs.extend(function_block(0x12, 0.0, 5.0, 50.0, 1000.0));
    std::fs::write(dir.join("_FUNCTNS.INF"), functs).unwrap();
    // function 1: two MS scans at 1.0 and 2.0 min
    let s1 = values(&[(100.0, 10), (200.0, 30)]);
    let s2 = values(&[(150.0, 7)]);
    let mut dat = s1.clone();
    dat.extend(&s2);
    std::fs::write(dir.join("_FUNC001.DAT"), &dat).unwrap();
    let mut idx = idx_record(0, 2, 40.0, 1.0);
    idx.extend(idx_record(s1.len() as u32, 1, 7.0, 2.0));
    std::fs::write(dir.join("_FUNC001.IDX"), idx).unwrap();
    // function 2: one MS/MS scan at 1.5 min, precursor 500.25 at 20 eV
    std::fs::write(dir.join("_FUNC002.DAT"), values(&[(120.0, 5), (300.0, 9)])).unwrap();
    std::fs::write(dir.join("_FUNC002.IDX"), idx_record(0, 2, 14.0, 1.5)).unwrap();
    std::fs::write(dir.join("_FUNC002.STS"), stats(&[(500.25, 20.0)])).unwrap();
    // function 3: lock-spray reference at 1.0 min (ties with function 1: comes after it)
    std::fs::write(dir.join("_FUNC003.DAT"), values(&[(556.25, 100)])).unwrap();
    std::fs::write(dir.join("_FUNC003.IDX"), idx_record(0, 1, 100.0, 1.0)).unwrap();
}

#[test]
#[allow(clippy::float_cmp)]
fn spectra_calibration_and_order() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run.raw");
    write_raw(&dir);
    assert!(WatersRawReader.sniff(&[], &dir).is_some());
    let mut ds = WatersRawReader.open(&dir).unwrap();
    let info = ds.info().unwrap();
    let run = &info.spectra[0];
    assert_eq!(run.scan_count, 4);
    assert_eq!(run.ms_levels, vec![1, 2]);
    assert_eq!(run.rt_range_s, Some([60.0, 120.0]));
    let inst = run.instrument.as_ref().unwrap();
    assert_eq!(inst.model.as_deref(), Some("SYNAPT-XS"));
    assert_eq!(inst.software_version.as_deref(), Some("4.2"));
    assert_eq!(run.extra["instrument_serial"], "AB123");
    assert_eq!(run.extra["acquired_at"], "2026-09-23T10:11:12");
    // merged by time, ties by function number
    let ids: Vec<String> = (0..4)
        .map(|i| ds.read_spectrum(0, i).unwrap().native_id.unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "function=1 process=0 scan=1",
            "function=3 process=0 scan=1",
            "function=2 process=0 scan=1",
            "function=1 process=0 scan=2",
        ]
    );
    let sp = ds.read_spectrum(0, 0).unwrap();
    // T0 calibration m + 0.5, rounded to single precision
    assert_eq!(sp.mz, vec![100.5, 200.5]);
    assert_eq!(sp.intensity, vec![10.0, 30.0]);
    assert_eq!(sp.total_ion_current, Some(40.0));
    assert_eq!(sp.base_peak_mz, Some(200.5));
    assert_eq!(sp.polarity, "positive");
    assert!(sp.centroided);
    assert_eq!(sp.scan_window_mz, Some([50.0, 400.0]));
    let lock = ds.read_spectrum(0, 1).unwrap();
    assert_eq!(lock.ms_level, 1);
    assert_eq!(lock.extra["lock_mass_reference"], true);
    assert_eq!(lock.mz, vec![556.25]);
    let ms2 = ds.read_spectrum(0, 2).unwrap();
    assert_eq!(ms2.ms_level, 2);
    assert_eq!(ms2.precursor_mz, Some(500.25));
    assert_eq!(ms2.collision_energy, Some(20.0));
    assert_eq!(ms2.activation.as_deref(), Some("HCD"));
    assert_eq!(ds.find_spectrum(0, 2).unwrap(), Some(3));
    // TIC and BPC traces over all four spectra
    let tic = info
        .traces
        .iter()
        .find(|t| t.name.as_deref() == Some("TIC"))
        .unwrap();
    let t = ds.read_trace(tic.index, 0, 0, 10).unwrap();
    assert_eq!(t.channels[0], vec![60.0, 60.0, 90.0, 120.0]);
    assert_eq!(t.channels[1], vec![40.0, 100.0, 14.0, 7.0]);
    let bpc = ds.read_trace(tic.index + 1, 0, 0, 10).unwrap();
    assert_eq!(bpc.channels[1], vec![30.0, 100.0, 9.0, 7.0]);
    let rep = ds.check().unwrap();
    assert!(rep.ok, "{:?}", rep.findings);
    assert_eq!(decode_mass27(mass_word(100.0)), 100.0);
}

#[test]
fn truncated_data_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("cut.raw");
    write_raw(&dir);
    let dat = std::fs::read(dir.join("_FUNC001.DAT")).unwrap();
    std::fs::write(dir.join("_FUNC001.DAT"), &dat[..dat.len() - 5]).unwrap();
    let mut ds = WatersRawReader.open(&dir).unwrap();
    let rep = ds.check().unwrap();
    assert!(!rep.ok);
    assert!(
        rep.findings.iter().any(|f| f.code == "truncated"),
        "{:?}",
        rep.findings
    );
    // the last scan of function 1 is the last spectrum
    assert!(ds.read_spectrum(0, 3).is_err());
}

/// The files directly under `dir`, held in memory under the same paths.
fn mirror(dir: &Path) -> Fs {
    let mut fs = MemFs::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_file() {
            let b = std::fs::read(&p).unwrap();
            fs.insert(&p, Arc::new(MemSource::new(p.display().to_string(), b)));
        }
    }
    Fs::new(Arc::new(fs))
}

/// Everything a reader answers about a data set: info, every spectrum, check.
fn answers(ds: &mut dyn Dataset) -> serde_json::Value {
    let info = ds.info().unwrap();
    let spectra: Vec<serde_json::Value> = info
        .spectra
        .iter()
        .flat_map(|r| (0..r.scan_count).map(move |i| (r.index, i)))
        .map(|(run, i)| serde_json::to_value(ds.read_spectrum(run, i).unwrap()).unwrap())
        .collect();
    serde_json::json!({
        "info": info,
        "spectra": spectra,
        "check": ds.check().unwrap(),
    })
}

#[test]
fn reads_the_same_from_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("run.raw");
    write_raw(&dir);
    let fs = mirror(&dir);
    let want = answers(WatersRawReader.open(&dir).unwrap().as_mut());
    // nothing left on disk: the in-memory copy is all there is
    tmp.close().unwrap();
    let input = Input::new(&dir, fs);
    assert!(WatersRawReader.reads_any_source());
    assert!(WatersRawReader.sniff_input(&[], &input).is_some());
    let mut ds = WatersRawReader.open_input(&input).unwrap();
    assert_eq!(answers(ds.as_mut()), want);
}
