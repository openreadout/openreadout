//! Synthetic `.wiff` compound files (written by a minimal version-3 compound-file writer below)
//! with `.wiff.scan` companions: a scheduled-MRM acquisition and a TOF data-dependent one;
//! spectra, scheduling, calibration, precursors, traces, `check`, truncation and `.wiff2`.

use std::path::Path;
use std::sync::Arc;

use openreadout_core::FormatReader;
use openreadout_core::reader::Dataset;
use openreadout_core::source::{Fs, Input, MemFs, MemSource};
use openreadout_sciex::SciexWiffReader;

const FREE: u32 = 0xFFFF_FFFF;
const END: u32 = 0xFFFF_FFFE;
const FATSECT: u32 = 0xFFFF_FFFD;

/// A version-3 compound file (512-byte sectors, one FAT sector, no mini stream) holding
/// `streams` at their `/`-separated paths.
fn compound_file(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    // directory tree: (name, kind, children)
    struct Node {
        name: String,
        stream: Option<usize>,
        children: Vec<usize>,
    }
    let mut nodes = vec![Node {
        name: "Root Entry".into(),
        stream: None,
        children: Vec::new(),
    }];
    for (si, (path, _)) in streams.iter().enumerate() {
        let mut cur = 0;
        let parts: Vec<&str> = path.split('/').collect();
        for (k, part) in parts.iter().enumerate() {
            let last = k + 1 == parts.len();
            let found = nodes[cur]
                .children
                .iter()
                .copied()
                .find(|&c| nodes[c].name == *part);
            cur = if let Some(c) = found {
                c
            } else {
                nodes.push(Node {
                    name: (*part).to_string(),
                    stream: last.then_some(si),
                    children: Vec::new(),
                });
                let id = nodes.len() - 1;
                nodes[cur].children.push(id);
                id
            };
        }
    }
    let dir_sectors = nodes.len().div_ceil(4);
    let mut data_sectors = Vec::new(); // (stream index, first sector, sector count)
    let mut next = 1 + dir_sectors as u32;
    for (i, (_, b)) in streams.iter().enumerate() {
        let n = b.len().div_ceil(512).max(1) as u32;
        data_sectors.push((i, next, n));
        next += n;
    }
    assert!(next <= 128, "test file too large for one FAT sector");
    let total = next as usize;
    let mut f = vec![0u8; 512 * (total + 1)];
    f[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    f[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
    f[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    f[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
    f[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    f[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    f[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
    f[0x30..0x34].copy_from_slice(&1u32.to_le_bytes());
    f[0x38..0x3C].copy_from_slice(&0u32.to_le_bytes()); // no mini stream
    f[0x3C..0x40].copy_from_slice(&END.to_le_bytes());
    f[0x44..0x48].copy_from_slice(&END.to_le_bytes());
    for i in 0..109 {
        f[0x4C + 4 * i..0x50 + 4 * i].copy_from_slice(&FREE.to_le_bytes());
    }
    f[0x4C..0x50].copy_from_slice(&0u32.to_le_bytes());
    let mut fat = vec![FREE; 128];
    fat[0] = FATSECT;
    for s in 0..dir_sectors {
        fat[1 + s] = if s + 1 == dir_sectors {
            END
        } else {
            2 + s as u32
        };
    }
    for &(_, first, n) in &data_sectors {
        for k in 0..n {
            fat[(first + k) as usize] = if k + 1 == n { END } else { first + k + 1 };
        }
    }
    for (i, v) in fat.iter().enumerate() {
        f[512 + 4 * i..516 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    for (id, node) in nodes.iter().enumerate() {
        let o = 512 * 2 + 128 * id;
        for (k, u) in node.name.encode_utf16().enumerate() {
            f[o + 2 * k..o + 2 * k + 2].copy_from_slice(&u.to_le_bytes());
        }
        f[o + 0x40..o + 0x42].copy_from_slice(&((node.name.len() as u16 + 1) * 2).to_le_bytes());
        f[o + 0x42] = match (id, node.stream) {
            (0, _) => 5,
            (_, Some(_)) => 2,
            _ => 1,
        };
        f[o + 0x44..o + 0x48].copy_from_slice(&FREE.to_le_bytes());
        // siblings: each child points right to the next child of the same parent
        let right = nodes
            .iter()
            .find_map(|p| {
                let pos = p.children.iter().position(|&c| c == id)?;
                Some(p.children.get(pos + 1).map_or(FREE, |&c| c as u32))
            })
            .unwrap_or(FREE);
        f[o + 0x48..o + 0x4C].copy_from_slice(&right.to_le_bytes());
        let child = node.children.first().map_or(FREE, |&c| c as u32);
        f[o + 0x4C..o + 0x50].copy_from_slice(&child.to_le_bytes());
        let (start, size) = match node.stream {
            Some(si) => {
                let (_, first, _) = data_sectors[si];
                (first, streams[si].1.len() as u32)
            }
            None => (END, 0),
        };
        f[o + 0x74..o + 0x78].copy_from_slice(&start.to_le_bytes());
        f[o + 0x78..o + 0x7C].copy_from_slice(&size.to_le_bytes());
    }
    for &(si, first, _) in &data_sectors {
        let o = 512 * (first as usize + 1);
        f[o..o + streams[si].1.len()].copy_from_slice(&streams[si].1);
    }
    f
}

fn preamble() -> Vec<u8> {
    let mut b = vec![0u8; 32];
    b[4] = 4;
    b
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn experiment_header(scan_type: u16, polarity: u16, ranges: u32) -> Vec<u8> {
    let mut b = vec![0u8; 0xB4];
    b[0x56..0x58].copy_from_slice(&polarity.to_le_bytes());
    b[0x7A..0x7C].copy_from_slice(&scan_type.to_le_bytes());
    b[0xB0..0xB4].copy_from_slice(&ranges.to_le_bytes());
    b
}

/// `MassRangeEx` with (Q1, Q3, expected RT, name, CE) per transition.
fn mass_ranges(ranges: &[(f32, f32, f32, &str, f32)]) -> Vec<u8> {
    let mut b = preamble();
    b.extend_from_slice(&2u32.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes());
    for &(q1, q3, rt, name, ce) in ranges {
        b.extend_from_slice(&q1.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&q3.to_le_bytes());
        b.extend_from_slice(&rt.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        let n = utf16(name);
        b.extend_from_slice(&(n.len() as u16).to_le_bytes());
        b.extend_from_slice(&n);
        let k = utf16("CE");
        b.extend_from_slice(&(k.len() as u16).to_le_bytes());
        b.extend_from_slice(&k);
        b.extend_from_slice(&ce.to_le_bytes());
        b.extend_from_slice(&ce.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
    }
    b
}

/// Index records: (offset, length, time ms, TIC, base-peak intensity, base-peak bin).
fn index(records: &[(u32, u32, f64, f64, f64, f64)]) -> Vec<u8> {
    let mut b = preamble();
    for &(off, len, t, tic, bpi, bp) in records {
        let mut r = vec![0u8; 54];
        r[0..4].copy_from_slice(&off.to_le_bytes());
        r[4..8].copy_from_slice(&len.to_le_bytes());
        r[8..16].copy_from_slice(&t.to_le_bytes());
        r[16..18].copy_from_slice(&6u16.to_le_bytes());
        r[18..26].copy_from_slice(&tic.to_le_bytes());
        r[26..34].copy_from_slice(&bpi.to_le_bytes());
        r[42..50].copy_from_slice(&bp.to_le_bytes());
        b.extend(r);
    }
    b
}

fn sample_streams() -> Vec<(&'static str, Vec<u8>)> {
    let mut data = preamble();
    data[0x1C] = 4; // this stream's preamble ends in a non-zero u32
    data.extend_from_slice(&100u32.to_le_bytes());
    for s in ["Blank", "N/A"] {
        let u = utf16(s);
        data.extend_from_slice(&(u.len() as u16).to_le_bytes());
        data.extend(u);
    }
    let mut table = vec![0u8; 0x4A];
    table[0x3E..0x42].copy_from_slice(&1_632_413_005u32.to_le_bytes());
    let mut fr = preamble();
    fr.extend(utf16("Analyst 1.7.2&File Version:  1.00"));
    let mut log = preamble();
    log.extend(utf16(
        "Mass Spectrometer:QTRAP 6500+:0:,Component ID: QTRAP 6500+,Serial Number: XY1",
    ));
    vec![
        ("FileRec_Str", fr),
        ("SampleSubtree/SampleTable", table),
        ("SampleSubtree/Sample1/Log", log),
        ("SampleSubtree/Sample1/SampleDABE/DATA", data),
    ]
}

fn scan_file(chunks: &[Vec<u8>]) -> (Vec<u8>, Vec<(u32, u32)>) {
    let mut b = vec![0u8; 0x2C];
    let mut spans = Vec::new();
    for c in chunks {
        spans.push(((b.len() - 0x2C) as u32, c.len() as u32));
        b.extend_from_slice(c);
    }
    (b, spans)
}

fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

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
