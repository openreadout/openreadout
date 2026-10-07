//! Synthetic `.wiff` compound files (written by the minimal version-3 compound-file writer in
//! `common`) with `.wiff.scan` companions: a scheduled-MRM acquisition and a TOF data-dependent
//! one; spectra, scheduling, calibration, precursors, traces, `check`, truncation and `.wiff2`.

use std::path::Path;
use std::sync::Arc;

use openreadout_core::FormatReader;
use openreadout_core::reader::Dataset;
use openreadout_core::source::{Fs, Input, MemFs, MemSource};
use openreadout_sciex::SciexWiffReader;

mod common;
use common::{
    compound_file, experiment_header, f32s, index, mass_ranges, preamble, sample_streams, scan_file,
};

/// Three transitions (two share Q1 500.5), windows [1000, 3000], [1000, 2000], [2000, 3000] ms;
/// cycles at 1000, 2000, 3000 ms; the cycle at 2000 ms is all zero (a switch cycle).
fn write_mrm(dir: &Path) -> std::path::PathBuf {
    let base = "MethodSubtree/Method1/DeviceMethod0/Period0/Experiment0";
    let ranges = [
        (500.5f32, 184.0f32, 0.03f32, "A", 30.0f32),
        (500.5, 250.25, 0.03, "A2", 35.0),
        (600.25, 300.5, 0.04, "B", 30.0),
    ];
    // rows: 3 zeros (first half) then the three transitions
    let c1 = f32s(&[-3.01, 10.0, 20.0, -1.01]);
    let c2 = f32s(&[-6.01]);
    let c3 = f32s(&[-3.01, 5.0, -1.01, 7.0]);
    let (scan, spans) = scan_file(&[c1, c2, c3]);
    let idx = index(&[
        (spans[0].0, spans[0].1, 1000.0, 30.0, 0.0, 0.0),
        (spans[1].0, spans[1].1, 2000.0, 0.0, 0.0, 0.0),
        (spans[2].0, spans[2].1, 3000.0, 12.0, 0.0, 0.0),
    ]);
    let mut windows = preamble();
    for (a, e) in [(1000u32, 3000u32), (1000, 2000), (2000, 3000)] {
        windows.extend_from_slice(&a.to_le_bytes());
        windows.extend_from_slice(&e.to_le_bytes());
    }
    let mut streams = sample_streams();
    let hdr = format!("{base}/ExperimentHeader");
    let mr = format!("{base}/MassRangeEx/MassRangeEx");
    let owned = [
        (hdr, experiment_header(4, 0, 3)),
        (mr, mass_ranges(&ranges)),
    ];
    for (p, b) in &owned {
        streams.push((p.as_str(), b.clone()));
    }
    streams.push(("SampleSubtree/Sample1/Idx", idx));
    streams.push((
        "SampleSubtree/Sample1/SampleDAM/sMRMPro_adw1/sMRMPro_adw_Times",
        windows,
    ));
    let wiff = dir.join("mrm.wiff");
    std::fs::write(&wiff, compound_file(&streams)).unwrap();
    std::fs::write(dir.join("mrm.wiff.scan"), scan).unwrap();
    wiff
}

#[test]
fn scheduled_mrm() {
    let tmp = tempfile::tempdir().unwrap();
    let wiff = write_mrm(tmp.path());
    let head = std::fs::read(&wiff).unwrap();
    assert!(SciexWiffReader.sniff(&head[..512], &wiff).is_some());
    let mut ds = SciexWiffReader
        .open(&tmp.path().join("mrm.wiff.scan"))
        .unwrap();
    // opened through the companion: the .wiff is the other member
    assert_eq!(ds.member_files(), vec![wiff.clone()]);
    assert_eq!(
        SciexWiffReader.open(&wiff).unwrap().member_files(),
        vec![tmp.path().join("mrm.wiff.scan")]
    );
    let info = ds.info().unwrap();
    let run = &info.spectra[0];
    // cycle 1 (1000 ms): Q1 500.5 (A, A2); B starts later. Cycle 2 (2000 ms, all zero: a
    // switch cycle): A only — A2's window ends and B's starts exactly there, so both are left
    // out. Cycle 3 (3000 ms): A (its window ends there, but the cycle has data) and B.
    assert_eq!(run.scan_count, 4);
    assert_eq!(
        run.instrument.as_ref().unwrap().model.as_deref(),
        Some("QTRAP 6500+")
    );
    assert_eq!(run.extra["sample_name"], "Blank");
    assert!(!run.extra.contains_key("sample_id"));
    // the stored u32 is the computer's local clock; the synthetic compound file has no storage
    // creation times to give its UTC offset, so the clock is returned without a zone
    assert_eq!(run.extra["acquired_at"], "2021-09-23T16:03:25.000");
    let s0 = ds.read_spectrum(0, 0).unwrap();
    assert_eq!(s0.precursor_mz, Some(500.5));
    assert_eq!(s0.mz, vec![184.0, 250.25]);
    assert_eq!(s0.intensity, vec![10.0, 20.0]);
    assert_eq!(s0.collision_energy, None); // two energies
    assert_eq!(s0.ms_level, 2);
    let s1 = ds.read_spectrum(0, 1).unwrap();
    assert_eq!(s1.rt_s, Some(2.0));
    assert_eq!(
        (s1.mz.clone(), s1.intensity.clone()),
        (vec![184.0], vec![0.0])
    );
    assert_eq!(s1.collision_energy, Some(30.0));
    let s2 = ds.read_spectrum(0, 2).unwrap();
    assert_eq!(
        (s2.mz.clone(), s2.intensity.clone()),
        (vec![184.0], vec![5.0])
    );
    let s3 = ds.read_spectrum(0, 3).unwrap();
    assert_eq!(s3.precursor_mz, Some(600.25));
    assert_eq!(
        (s3.mz.clone(), s3.intensity.clone()),
        (vec![300.5], vec![7.0])
    );
    assert!(
        s3.native_id
            .unwrap()
            .starts_with("sample=1 period=1 cycle=3 experiment=1")
    );
    assert_eq!(ds.find_spectrum(0, 4).unwrap(), Some(3));
    // TIC and BPC traces: one point per cycle
    let t = ds.read_trace(0, 0, 0, 10).unwrap();
    assert_eq!(t.channels[1], vec![30.0, 0.0, 12.0]);
    let b = ds.read_trace(1, 0, 0, 10).unwrap();
    assert_eq!(b.channels[1], vec![20.0, 0.0, 7.0]);
    assert!(ds.check().unwrap().ok);
    // a cut-off .wiff.scan: check fails, the last spectrum does not decode
    let scan = std::fs::read(tmp.path().join("mrm.wiff.scan")).unwrap();
    std::fs::write(tmp.path().join("mrm.wiff.scan"), &scan[..scan.len() - 4]).unwrap();
    let mut ds = SciexWiffReader.open(&wiff).unwrap();
    let rep = ds.check().unwrap();
    assert!(!rep.ok);
    assert!(rep.findings.iter().any(|f| f.code == "truncated"));
    assert!(ds.read_spectrum(0, 3).is_err());
}

#[test]
fn tof_data_dependent() {
    let tmp = tempfile::tempdir().unwrap();
    let base = "MethodSubtree/Method1/DeviceMethod0/Period0";
    // MS1: bins 1000 (count 5) and 1001 (count 300); MS2: bin 900 (count 7)
    let mut ms1 = vec![0xFF, 0xFF, 0xFF, 0xFF];
    ms1.extend_from_slice(&1000u32.to_le_bytes());
    ms1.extend_from_slice(&[0x05, 0x7D, 0x2C, 0x01]);
    let mut ms2 = vec![0xFF, 0xFF, 0xFF, 0xFF];
    ms2.extend_from_slice(&900u32.to_le_bytes());
    ms2.push(0x07);
    let (scan, spans) = scan_file(&[ms1, ms2]);
    let idx = index(&[
        (spans[0].0, spans[0].1, 250.0, 305.0, 300.0, 1001.0),
        (spans[1].0, spans[1].1, 300.0, 7.0, 7.0, 900.0),
        (0, 0, 250.0, 0.0, 1.0, 0.0),
    ]);
    let mut cal = preamble();
    for v in [0.5f64, 1.0] {
        cal.extend_from_slice(&v.to_le_bytes());
    }
    cal.extend_from_slice(&0u32.to_le_bytes());
    cal.extend_from_slice(&3u32.to_le_bytes());
    for _ in 0..3 {
        cal.extend_from_slice(&0.5f64.to_le_bytes());
        cal.extend_from_slice(&2.0f64.to_le_bytes());
        cal.extend_from_slice(&0u32.to_le_bytes());
    }
    let mut tdc = preamble();
    tdc.extend_from_slice(&0.025f64.to_le_bytes());
    let mut dde = preamble();
    // one cycle, two product-ion experiments: slot 0 filled, slot 1 empty
    dde.extend_from_slice(&456.75f64.to_le_bytes());
    dde.extend_from_slice(&0f64.to_le_bytes());
    dde.extend_from_slice(&135f64.to_le_bytes());
    dde.extend_from_slice(&0f64.to_le_bytes());
    dde.extend_from_slice(&[0u8; 32]);
    let mut streams = sample_streams();
    let owned: Vec<(String, Vec<u8>)> = vec![
        (
            format!("{base}/Experiment0/ExperimentHeader"),
            experiment_header(8, 1, 0),
        ),
        (
            format!("{base}/Experiment1/ExperimentHeader"),
            experiment_header(9, 1, 1),
        ),
        (
            format!("{base}/Experiment1/MassRangeEx/MassRangeEx"),
            mass_ranges(&[(15.0, 15.0, 0.0, "", 30.0)]),
        ),
        (
            format!("{base}/Experiment2/ExperimentHeader"),
            experiment_header(9, 1, 1),
        ),
    ];
    for (p, b) in &owned {
        streams.push((p.as_str(), b.clone()));
    }
    streams.push(("SampleSubtree/Sample1/Idx", idx));
    streams.push(("SampleSubtree/Sample1/TOFCalibrationData", cal));
    streams.push(("SampleSubtree/Sample1/TDCInfo", tdc));
    streams.push(("SampleSubtree/Sample1/DDERealTimeData", dde));
    let wiff = tmp.path().join("tof.wiff");
    std::fs::write(&wiff, compound_file(&streams)).unwrap();
    std::fs::write(tmp.path().join("tof.wiff.scan"), scan).unwrap();
    let mut ds = SciexWiffReader.open(&wiff).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.spectra[0].scan_count, 2);
    assert_eq!(info.spectra[0].ms_levels, vec![1, 2]);
    let s = ds.read_spectrum(0, 0).unwrap();
    assert_eq!(s.polarity, "negative");
    assert!(!s.centroided);
    let mz = |bin: f64| (0.5 * (0.025 * bin - 2.0)).powi(2);
    // bins 999 (empty neighbour), 1000, 1001, 1002 (empty neighbour)
    assert_eq!(s.mz, vec![mz(999.0), mz(1000.0), mz(1001.0), mz(1002.0)]);
    assert_eq!(s.intensity, vec![0.0, 5.0, 300.0, 0.0]);
    assert_eq!(s.total_ion_current, Some(305.0));
    assert_eq!(s.base_peak_mz, Some(mz(1001.0)));
    assert_eq!(
        s.native_id.as_deref(),
        Some("sample=1 period=1 cycle=1 experiment=1")
    );
    let p = ds.read_spectrum(0, 1).unwrap();
    assert_eq!(p.ms_level, 2);
    assert_eq!(p.precursor_mz, Some(456.75));
    assert_eq!(p.collision_energy, Some(30.0));
    assert_eq!(
        p.native_id.as_deref(),
        Some("sample=1 period=1 cycle=1 experiment=2")
    );
    assert!(ds.check().unwrap().ok);
}

#[test]
fn wiff2_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("run.wiff2");
    std::fs::write(&p, b"not read").unwrap();
    assert!(SciexWiffReader.sniff(b"not read", &p).is_some());
    let Err(e) = SciexWiffReader.open(&p) else {
        panic!(".wiff2 opened")
    };
    assert_eq!(e.exit_code(), 6);
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
    let wiff = write_mrm(tmp.path());
    let scan = tmp.path().join("mrm.wiff.scan");
    let head = std::fs::read(&wiff).unwrap();
    let fs = mirror(tmp.path());
    let want = answers(SciexWiffReader.open(&wiff).unwrap().as_mut());
    let members = SciexWiffReader.open(&wiff).unwrap().member_files();
    // nothing left on disk: the in-memory copies are all there is
    tmp.close().unwrap();
    assert!(SciexWiffReader.reads_any_source());
    let input = Input::new(&wiff, fs.clone());
    assert!(SciexWiffReader.sniff_input(&head[..512], &input).is_some());
    let mut ds = SciexWiffReader.open_input(&input).unwrap();
    assert_eq!(answers(ds.as_mut()), want);
    assert_eq!(ds.member_files(), members);
    // through the companion, the .wiff beside it is found in the same namespace
    let mut ds = SciexWiffReader.open_input(&Input::new(&scan, fs)).unwrap();
    assert_eq!(answers(ds.as_mut()), want);
    // a .wiff2 buffer is refused like a .wiff2 file
    let err = SciexWiffReader
        .open_input(&Input::from_bytes("x.wiff2", head))
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err.exit_code(), 6, "{err}");
}

#[test]
fn a_scan_companion_finds_its_wiff_whatever_the_case() {
    use std::path::Path;
    use std::sync::Arc;

    use openreadout_core::source::{Fs, MemFs, MemSource};

    let mut mem = MemFs::new();
    mem.insert(
        Path::new("run/X_AQ.wiff"),
        Arc::new(MemSource::new("X_AQ.wiff", vec![0u8; 8])),
    );
    mem.insert(
        Path::new("run/X_aq.wiff.scan"),
        Arc::new(MemSource::new("X_aq.wiff.scan", vec![0u8; 8])),
    );
    let fs = Fs::new(Arc::new(mem));
    assert_eq!(
        openreadout_sciex::wiff_path(&fs, Path::new("run/X_aq.wiff.scan")),
        Path::new("run/X_AQ.wiff")
    );
    assert_eq!(
        openreadout_sciex::wiff_path(&fs, Path::new("run/X_AQ.wiff")),
        Path::new("run/X_AQ.wiff")
    );
}
