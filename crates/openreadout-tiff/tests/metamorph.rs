//! Synthetic MetaMorph files (docs/formats/tiff.md § MetaMorph): an STK stack written by hand
//! with its UIC tags, and a `.nd` series of STK files (missing and truncated members).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::Error;
use openreadout_core::model::Severity;
use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, MemFs, MemSource};
use openreadout_tiff::TiffReader;

const W: u32 = 8;
const H: u32 = 4;

fn plane(seed: u32, z: u32) -> Vec<u8> {
    (0..W * H)
        .flat_map(|i| ((seed * 1000 + z * 100 + i) as u16).to_le_bytes())
        .collect()
}

/// One IFD entry: tag, type, count, value-or-offset (little-endian classic TIFF).
fn entry(b: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: u32) {
    b.extend(tag.to_le_bytes());
    b.extend(typ.to_le_bytes());
    b.extend(count.to_le_bytes());
    b.extend(value.to_le_bytes());
}

/// An STK file: `n` planes of W×H uint16, Z distance 0.5 µm, calibration 0.25 µm, stage X
/// −22440.1 (a signed numerator), per-plane creation times 100 ms apart.
fn stk(seed: u32, n: u32) -> Vec<u8> {
    let mut b = b"II*\0\0\0\0\0".to_vec();
    for z in 0..n {
        b.extend(plane(seed, z));
    }
    let pad = |b: &mut Vec<u8>| {
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
    };
    pad(&mut b);
    // out-of-line values
    let rational_at = b.len() as u32; // 0.25 = 1/4
    b.extend(1u32.to_le_bytes());
    b.extend(4u32.to_le_bytes());
    let units_at = b.len() as u32;
    b.extend(3u32.to_le_bytes());
    b.extend(b"um\0\0");
    let uic1_at = b.len() as u32;
    for (id, v) in [
        (3u32, 1u32),
        (4, rational_at),
        (5, rational_at),
        (6, units_at),
    ] {
        b.extend(id.to_le_bytes());
        b.extend(v.to_le_bytes());
    }
    let uic2_at = b.len() as u32;
    for z in 0..n {
        for v in [
            1u32,
            2,
            2_457_811,
            37_495_325 + 100 * z,
            2_457_811,
            37_496_468,
        ] {
            b.extend(v.to_le_bytes());
        }
    }
    let uic3_at = b.len() as u32;
    for _ in 0..n {
        b.extend(525u32.to_le_bytes());
        b.extend(1u32.to_le_bytes());
    }
    let uic4_at = b.len() as u32;
    b.extend(28u16.to_le_bytes());
    for _ in 0..n {
        for v in [(-22_440_100i32) as u32, 1000, 24_603_000, 10_000] {
            b.extend(v.to_le_bytes());
        }
    }
    b.extend(37u16.to_le_bytes());
    for _ in 0..n {
        b.extend(4u32.to_le_bytes());
        b.extend(b"B02\0");
    }
    b.extend(0u16.to_le_bytes());
    let desc: Vec<u8> = (0..n)
        .flat_map(|_| b"Exposure: 40 ms\r\nBinning: 1\0".to_vec())
        .collect();
    let desc_at = b.len() as u32;
    b.extend(&desc);
    pad(&mut b);
    let ifd = b.len() as u32;
    b[4..8].copy_from_slice(&ifd.to_le_bytes());
    let entries: Vec<(u16, u16, u32, u32)> = vec![
        (256, 4, 1, W),
        (257, 4, 1, H),
        (258, 3, 1, 16),
        (259, 3, 1, 1),
        (262, 3, 1, 1),
        (270, 2, desc.len() as u32, desc_at),
        (273, 4, 1, 8),
        (277, 3, 1, 1),
        (278, 4, 1, H),
        (279, 4, 1, W * H * 2),
        (33628, 4, 4, uic1_at),
        (33629, 5, n, uic2_at),
        (33630, 5, n, uic3_at),
        (33631, 4, n, uic4_at),
    ];
    b.extend((entries.len() as u16).to_le_bytes());
    for (t, ty, c, v) in entries {
        entry(&mut b, t, ty, c, v);
    }
    b.extend(0u32.to_le_bytes());
    b
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn stk_stack_reads_planes_calibration_and_frames() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "a.stk", &stk(1, 3));
    let mut ds = TiffReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert!(info.notes[0].contains("metamorph-stk"), "{:?}", info.notes);
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_z, im.size_t), (W, H, 3, 1));
    assert_eq!(im.physical_size.x, Some(0.25));
    assert_eq!(im.physical_size.z, Some(0.5));
    assert_eq!(im.acquired_at.as_deref(), Some("2017-02-27T10:24:55.325"));
    assert_eq!(im.channels[0].exposure_ms, Some(40.0));
    for z in 0..3 {
        let pl = ds.read_plane(0, PlaneIndex { c: 0, z, t: 0 }).unwrap();
        assert_eq!(pl.data, plane(1, z), "z {z}");
    }
    let (n, recs) = ds.frames(0, None).unwrap();
    assert_eq!(n, 3);
    assert_eq!(recs[2]["delta_t_s"], 0.2);
    assert_eq!(recs[0]["stage_x_um"], -22440.1);
    assert_eq!(recs[0]["wavelength_nm"], 525.0);
    assert!(ds.check().unwrap().ok);
}

#[test]
fn truncated_stk_fails_check_and_its_last_plane() {
    let dir = tempfile::tempdir().unwrap();
    let full = stk(1, 3);
    // keep the IFD and values but drop plane 2's pixels: move the planes' end past the file
    let mut cut = full.clone();
    // rewrite the strip offset so the stack starts near the end of the file
    let start = (full.len() as u32) - W * H * 2 - 10;
    let ifd = u32::from_le_bytes(full[4..8].try_into().unwrap()) as usize;
    let n = u16::from_le_bytes([full[ifd], full[ifd + 1]]) as usize;
    for i in 0..n {
        let e = ifd + 2 + 12 * i;
        if u16::from_le_bytes([cut[e], cut[e + 1]]) == 273 {
            cut[e + 8..e + 12].copy_from_slice(&start.to_le_bytes());
        }
    }
    let p = write(dir.path(), "cut.stk", &cut);
    let mut ds = TiffReader.open(&p).unwrap();
    let r = ds.check().unwrap();
    assert!(!r.ok);
    assert!(
        r.findings
            .iter()
            .any(|f| f.code == "truncated" && f.severity == Severity::Error)
    );
    let e = ds
        .read_plane(0, PlaneIndex { c: 0, z: 2, t: 0 })
        .unwrap_err();
    assert!(matches!(e, Error::Corrupt { .. }), "{e}");
    assert_eq!(e.exit_code(), 4);
}

const ND: &str = "\"NDInfoFile\", Version 1.0\r\n\"Description\", File recreated from images.\r\n\"StartTime1\", 20170227 10:24:55.280\r\n\"DoTimelapse\", TRUE\r\n\"NTimePoints\", 2\r\n\"DoStage\", TRUE\r\n\"NStagePositions\", 2\r\n\"Stage1\", \"B2\"\r\n\"Stage2\", \"B3\"\r\n\"DoWave\", TRUE\r\n\"NWavelengths\", 2\r\n\"WaveName1\", \"Green\"\r\n\"WaveDoZ1\", TRUE\r\n\"WaveName2\", \"Blue\"\r\n\"WaveDoZ2\", TRUE\r\n\"DoZSeries\", TRUE\r\n\"NZSteps\", 3\r\n\"ZStepSize\", 0,50\r\n\"WaveInFileName\", TRUE\r\n\"EndFile\"\r\n";

/// Seed of the file of stage `s`, wave `w`, time `t` (zero-based).
fn seed(s: u32, w: u32, t: u32) -> u32 {
    s * 100 + w * 10 + t + 1
}

fn nd_set(dir: &Path) -> PathBuf {
    for s in 0..2 {
        for (w, wave) in ["Green", "Blue"].iter().enumerate() {
            for t in 0..2 {
                let name = format!("set_w{}{wave}_s{}_t{}.STK", w + 1, s + 1, t + 1);
                write(dir, &name, &stk(seed(s, w as u32, t), 3));
            }
        }
    }
    write(dir, "set.nd", ND.as_bytes())
}

#[test]
fn nd_series_maps_stages_waves_times_and_z() {
    let dir = tempfile::tempdir().unwrap();
    let nd = nd_set(dir.path());
    // a file past NTimePoints is noted, not read
    write(dir.path(), "set_w1Green_s1_t3.stk", &stk(9, 3));
    let head = std::fs::read(&nd).unwrap();
    assert!(TiffReader.sniff(&head, &nd).is_some());
    let mut ds = TiffReader.open(&nd).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 2);
    assert!(
        info.notes
            .iter()
            .any(|n| n.contains("past the 2 time point"))
    );
    let im = &info.images[1];
    assert_eq!(im.name.as_deref(), Some("B3"));
    assert_eq!((im.size_c, im.size_z, im.size_t), (2, 3, 2));
    assert_eq!(im.physical_size.z, Some(0.5));
    assert_eq!(im.channels[1].name.as_deref(), Some("Blue"));
    assert_eq!(im.acquired_at.as_deref(), Some("2017-02-27T10:24:55.280"));
    for (c, z, t) in [(0, 0, 0), (1, 2, 1), (0, 1, 1)] {
        let pl = ds.read_plane(1, PlaneIndex { c, z, t }).unwrap();
        assert_eq!(pl.data, plane(seed(1, c, t), z), "c {c} z {z} t {t}");
    }
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    assert!(ds.member_files().len() >= 8);
}

#[test]
fn nd_series_with_missing_and_cut_members() {
    let dir = tempfile::tempdir().unwrap();
    let nd = nd_set(dir.path());
    std::fs::remove_file(dir.path().join("set_w2Blue_s1_t2.STK")).unwrap();
    let cut = stk(seed(1, 0, 0), 3);
    // an STK cut inside its planes: its IFD lies past the end (check: `truncated`)
    write(dir.path(), "set_w1Green_s2_t1.STK", &cut[..100]);
    let mut ds = TiffReader.open(&nd).unwrap();
    let info = ds.info().unwrap();
    assert!(
        info.notes.iter().any(|n| n.contains("1 of the 8 files")),
        "{:?}",
        info.notes
    );
    let e = ds
        .read_plane(0, PlaneIndex { c: 1, z: 0, t: 1 })
        .unwrap_err();
    assert_eq!(e.exit_code(), 4, "{e}");
    let e = ds
        .read_plane(1, PlaneIndex { c: 0, z: 0, t: 0 })
        .unwrap_err();
    assert_eq!(e.exit_code(), 4, "{e}");
    assert!(ds.read_plane(0, PlaneIndex { c: 0, z: 2, t: 1 }).is_ok());
    let r = ds.check().unwrap();
    assert!(!r.ok);
    let codes: Vec<&str> = r.findings.iter().map(|f| f.code.as_str()).collect();
    assert!(codes.contains(&"missing_file"), "{codes:?}");
    assert!(codes.contains(&"truncated"), "{codes:?}");
}

/// The files of `dir` (written by the other tests' helpers) copied into memory under `mem/`:
/// readers must find siblings through the input's `Fs`, as for a dropped folder in the
/// browser.
fn in_memory(dir: &Path, open: &str) -> Input {
    let mut fs = MemFs::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let src = MemSource::new(name.clone(), std::fs::read(&p).unwrap());
        fs.insert(Path::new("mem").join(&name), Arc::new(src));
    }
    Input::new(Path::new("mem").join(open), Fs::new(Arc::new(fs)))
}

#[test]
fn nd_series_finds_its_members_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    nd_set(dir.path());
    let mut ds = TiffReader
        .open_input(&in_memory(dir.path(), "set.nd"))
        .unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 2);
    assert_eq!(info.images[1].size_c, 2);
    let pl = ds.read_plane(1, PlaneIndex { c: 1, z: 2, t: 1 }).unwrap();
    assert_eq!(pl.data, plane(seed(1, 1, 1), 2));
    assert!(ds.check().unwrap().ok);
}
