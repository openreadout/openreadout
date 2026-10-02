//! Synthetic files of the four formats built from the layouts in `docs/formats/`: values, axes,
//! metadata, OPUS block pairing and ordering, OMNIC groups, a WiRE map laid out from stage
//! coordinates, and truncation/corruption (clean errors, never a panic).
#![allow(clippy::many_single_char_names)] // byte layouts built field by field

use openreadout_core::source::Input;
use openreadout_core::{Dataset, FormatReader, PlaneIndex};
use openreadout_spectro::{OmnicReader, OpusReader, PeSpReader, WdfReader};

fn open(
    r: &dyn FormatReader,
    name: &str,
    bytes: Vec<u8>,
) -> openreadout_core::Result<Box<dyn Dataset>> {
    r.open_input(&Input::from_bytes(name, bytes))
}

// ---------------------------------------------------------------- OPUS

enum P {
    I(i32),
    F(f64),
    T(&'static str),
}

fn params(list: &[(&str, P)]) -> Vec<u8> {
    let mut b = Vec::new();
    for (name, v) in list {
        let mut key = name.as_bytes().to_vec();
        key.resize(4, 0);
        b.extend_from_slice(&key);
        match v {
            P::I(i) => {
                b.extend_from_slice(&0i16.to_le_bytes());
                b.extend_from_slice(&2i16.to_le_bytes());
                b.extend_from_slice(&i.to_le_bytes());
            }
            P::F(f) => {
                b.extend_from_slice(&1i16.to_le_bytes());
                b.extend_from_slice(&4i16.to_le_bytes());
                b.extend_from_slice(&f.to_le_bytes());
            }
            P::T(s) => {
                let mut t = s.as_bytes().to_vec();
                t.push(0);
                if t.len() % 2 == 1 {
                    t.push(0);
                }
                b.extend_from_slice(&2i16.to_le_bytes());
                b.extend_from_slice(&((t.len() / 2) as i16).to_le_bytes());
                b.extend_from_slice(&t);
            }
        }
    }
    b.extend_from_slice(b"END\0\0\0\0\0");
    while b.len() % 4 != 0 {
        b.push(0);
    }
    b
}

fn type_word(part: u32, role: u32, params: u32, data: u32, ext: u32) -> u32 {
    part | (role << 2) | (params << 4) | (data << 10) | (ext << 19)
}

/// An OPUS file with the given (type word, body) blocks.
fn opus_file(blocks: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let slots = 16u32;
    let dir_at = 24u32;
    let mut out = Vec::new();
    out.extend_from_slice(&[0x0a, 0x0a, 0xfe, 0xfe]);
    out.extend_from_slice(&920_622.0f64.to_le_bytes());
    out.extend_from_slice(&dir_at.to_le_bytes());
    out.extend_from_slice(&slots.to_le_bytes());
    out.extend_from_slice(&((blocks.len() + 1) as u32).to_le_bytes());
    let mut dir = vec![0u8; (slots * 12) as usize];
    let mut body = Vec::new();
    let mut at = dir_at + slots * 12;
    // entry 0: the directory itself
    dir[0..4].copy_from_slice(&type_word(0, 0, 0, 13, 0).to_le_bytes());
    dir[4..8].copy_from_slice(&(slots * 3).to_le_bytes());
    dir[8..12].copy_from_slice(&dir_at.to_le_bytes());
    for (i, (w, b)) in blocks.iter().enumerate() {
        let e = (i + 1) * 12;
        dir[e..e + 4].copy_from_slice(&w.to_le_bytes());
        dir[e + 4..e + 8].copy_from_slice(&((b.len() / 4) as u32).to_le_bytes());
        dir[e + 8..e + 12].copy_from_slice(&at.to_le_bytes());
        at += b.len() as u32;
        body.extend_from_slice(b);
    }
    out.extend_from_slice(&dir);
    out.extend_from_slice(&body);
    out
}

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn status(npt: i32, fxv: f64, lxv: f64, csf: f64, mny: f64, mxy: f64) -> Vec<u8> {
    params(&[
        ("DPF", P::I(1)),
        ("NPT", P::I(npt)),
        ("FXV", P::F(fxv)),
        ("LXV", P::F(lxv)),
        ("CSF", P::F(csf)),
        ("MXY", P::F(mxy)),
        ("MNY", P::F(mny)),
        ("DAT", P::T("11/05/2020")),
        ("TIM", P::T("08:59:44.322 (GMT+12)")),
        ("DXU", P::T("WN")),
    ])
}

fn sample_opus() -> Vec<u8> {
    let ab = [0.1f32, 0.5, 0.9, 0.3];
    let old_ab = [1.0f32, 2.0, 3.0, 4.0];
    let sm = [10.0f32, 20.0, 30.0, 40.0, 99.0]; // one extra value past NPT
    opus_file(&[
        (
            type_word(0, 0, 3, 0, 0),
            params(&[("RES", P::F(4.0)), ("NSS", P::I(32)), ("PLF", P::T("AB"))]),
        ),
        (
            type_word(0, 0, 4, 0, 0),
            params(&[("APF", P::T("B3")), ("ZFF", P::T("2"))]),
        ),
        (
            type_word(0, 0, 2, 0, 0),
            params(&[("LWN", P::F(15798.0)), ("INS", P::T("VERTEX 70"))]),
        ),
        (
            type_word(0, 0, 10, 0, 0),
            params(&[("SNM", P::T("quartz")), ("CNM", P::T("ana"))]),
        ),
        (
            type_word(0, 2, 3, 0, 0),
            params(&[("RES", P::F(4.0)), ("NSR", P::I(64))]),
        ),
        // an older absorbance block (data + status), then the current one later in the file
        (type_word(3, 3, 0, 4, 0), floats(&old_ab)),
        (
            type_word(3, 3, 1, 4, 0),
            status(4, 4000.0, 1000.0, 1.0, 1.0, 4.0),
        ),
        (type_word(3, 1, 0, 1, 0), floats(&sm)),
        (
            type_word(3, 1, 1, 1, 0),
            status(4, 4000.0, 1000.0, 0.5, 5.0, 20.0),
        ),
        (type_word(3, 3, 0, 4, 0), floats(&ab)),
        (
            type_word(3, 3, 1, 4, 0),
            status(4, 4000.0, 1000.0, 1.0, 0.1, 0.9),
        ),
    ])
}

#[test]
fn opus_values_axes_and_order() {
    let mut ds = open(&OpusReader, "s.0", sample_opus()).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 3);
    let t0 = &info.traces[0];
    assert_eq!(t0.name.as_deref(), Some("absorbance"));
    assert_eq!(t0.channels[0].unit.as_deref(), Some("AU"));
    assert_eq!(t0.extra["axis"]["quantity"], "wavenumber");
    assert_eq!(t0.extra["axis"]["first"], 4000.0);
    assert_eq!(t0.extra["axis"]["step"], -1000.0);
    assert_eq!(t0.extra["resolution_cm1"], 4.0);
    assert_eq!(t0.extra["scans"], 32);
    assert_eq!(t0.extra["background_scans"], 64);
    assert_eq!(t0.extra["apodization_name"], "Blackman-Harris 3-term");
    assert_eq!(t0.extra["zero_filling_factor"], 2.0);
    assert_eq!(t0.extra["acquired_at"], "2020-05-11T08:59:44.322+12:00");
    // the later absorbance block comes first; its status is found by its extremes
    let v = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(
        v.channels[0],
        vec![
            f64::from(0.1f32),
            f64::from(0.5f32),
            0.9f32.into(),
            0.3f32.into()
        ]
    );
    let v = ds.read_trace(1, 0, 0, u64::MAX).unwrap();
    assert_eq!(v.channels[0], vec![1.0, 2.0, 3.0, 4.0]);
    // CSF scales; NPT cuts the extra stored value
    assert_eq!(
        info.traces[2].name.as_deref(),
        Some("sample single channel")
    );
    let v = ds.read_trace(2, 0, 1, 2).unwrap();
    assert_eq!(v.channels[0], vec![10.0, 15.0]);
    let e = ds.experiment().unwrap();
    assert_eq!(e.sample.unwrap().id.as_deref(), Some("quartz"));
    assert_eq!(e.instrument.unwrap().model.as_deref(), Some("VERTEX 70"));
    assert!(ds.check().unwrap().ok);
}

#[test]
fn opus_truncations_never_panic() {
    let full = sample_opus();
    for n in (0..full.len()).step_by(7) {
        let r = open(&OpusReader, "t.0", full[..n].to_vec());
        if let Ok(mut ds) = r {
            let _ = ds.info();
            let _ = ds.check();
            if let Ok(info) = ds.info() {
                for t in &info.traces {
                    let _ = ds.read_trace(t.index, 0, 0, u64::MAX);
                }
            }
        } else if let Err(e) = r {
            assert!(matches!(e.exit_code(), 4..=6), "{e}");
        }
    }
    // a bad directory offset is a corrupt file (exit 4)
    let mut bad = sample_opus();
    bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    let e = open(&OpusReader, "b.0", bad).err().unwrap();
    assert_eq!(e.exit_code(), 4);
}

// ---------------------------------------------------------------- OMNIC

fn omnic_header(points: u32, x_code: u8, y_code: u8, first: f32, last: f32, scans: u32) -> Vec<u8> {
    let mut h = vec![0u8; 200];
    h[4..8].copy_from_slice(&points.to_le_bytes());
    h[8] = x_code;
    h[12] = y_code;
    h[16..20].copy_from_slice(&first.to_le_bytes());
    h[20..24].copy_from_slice(&last.to_le_bytes());
    h[36..40].copy_from_slice(&scans.to_le_bytes());
    h[52..56].copy_from_slice(&16u32.to_le_bytes());
    h[80..84].copy_from_slice(&15798.0f32.to_le_bytes());
    h
}

/// An OMNIC file: title and records (key, spectrum number, body).
fn omnic_file(title: &str, time: u32, records: &[(u8, u16, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0u8; 304];
    out[..18].copy_from_slice(b"Spectral Data File");
    out[30..30 + title.len()].copy_from_slice(title.as_bytes());
    out[294..296].copy_from_slice(&(records.len() as u16).to_le_bytes());
    out[296..300].copy_from_slice(&time.to_le_bytes());
    let mut table = vec![0u8; records.len() * 16];
    let mut at = 304 + table.len();
    let mut body = Vec::new();
    for (i, (key, spec, b)) in records.iter().enumerate() {
        let e = i * 16;
        table[e] = *key;
        table[e + 2..e + 6].copy_from_slice(&(at as u32).to_le_bytes());
        table[e + 6..e + 10].copy_from_slice(&(b.len() as u32).to_le_bytes());
        table[e + 10..e + 12].copy_from_slice(&spec.to_le_bytes());
        at += b.len();
        body.extend_from_slice(b);
    }
    out.extend_from_slice(&table);
    out.extend_from_slice(&body);
    out
}

#[test]
fn omnic_single_spectrum() {
    let hist = b"Collect Sample\r\n\t Final format:\tAbsorbance\r\n\t Resolution:\t 4.000 from 400 to 4000\r\n\t Bench Serial Number:ABC123;\r\n".to_vec();
    let f = omnic_file(
        "calcite",
        3_908_015_303,
        &[
            (2, 0, omnic_header(3, 1, 17, 4000.0, 400.0, 32)),
            (27, 0, hist),
            (3, 0, floats(&[0.25, 0.5, 0.75])),
            (102, 0, floats(&[1.0, -1.0])),
        ],
    );
    let mut ds = open(&OmnicReader, "c.spa", f.clone()).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 2);
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("calcite"));
    assert_eq!(t.channels[0].name, "absorbance");
    assert_eq!(t.extra["axis"]["first"], 4000.0);
    assert_eq!(t.extra["axis"]["last"], 400.0);
    assert_eq!(t.extra["scans"], 32);
    assert_eq!(t.extra["resolution_cm1"], 4.0);
    assert_eq!(t.extra["instrument_serial"], "ABC123");
    assert_eq!(t.extra["acquired_at"], "2023-11-02T15:48:23Z");
    assert_eq!(
        ds.read_trace(0, 0, 0, 9).unwrap().channels[0],
        vec![0.25, 0.5, 0.75]
    );
    assert_eq!(info.traces[1].name.as_deref(), Some("sample interferogram"));
    assert_eq!(
        ds.read_trace(1, 0, 0, 9).unwrap().channels[0],
        vec![1.0, -1.0]
    );
    for n in (0..f.len()).step_by(5) {
        if let Ok(mut d) = open(&OmnicReader, "c.spa", f[..n].to_vec()) {
            let _ = d.info();
            let _ = d.check();
            let _ = d.read_trace(0, 0, 0, 9);
        }
    }
}

#[test]
fn omnic_group_file() {
    let mut recs = Vec::new();
    for s in 0..3u16 {
        recs.push((2, s, omnic_header(2, 1, 17, 3000.0, 1000.0, 8)));
        let mut title = format!("spec{s}.spa").into_bytes();
        title.resize(256, 0);
        title.extend_from_slice(&(3_700_000_000u32 + u32::from(s) * 60).to_le_bytes());
        recs.push((107, s, title));
        recs.push((3, s, floats(&[f32::from(s), f32::from(s) + 0.5])));
    }
    let mut ds = open(&OmnicReader, "g.spg", omnic_file("g.spg", 0, &recs)).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 1);
    assert_eq!(info.traces[0].sweep_count, 3);
    assert_eq!(info.traces[0].extra["spectrum_titles"][2], "spec2.spa");
    assert_eq!(
        ds.read_trace(0, 2, 0, 9).unwrap().channels[0],
        vec![2.0, 2.5]
    );
    let tab = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(tab.columns[2], vec![0.0, 60.0, 120.0]);
}

#[test]
fn omnic_series_without_a_series_record_is_corrupt() {
    // `.srs` series are read; a key table without the series record is a clean corrupt error
    let mut f = vec![0u8; 400];
    f[..18].copy_from_slice(b"Spectral Exte File");
    let e = open(&OmnicReader, "s.srs", f).err().unwrap();
    assert_eq!(e.exit_code(), 4);
    assert!(e.hint().is_some());
}

// ---------------------------------------------------------------- WDF

fn block(name: [u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = name.to_vec();
    b.extend_from_slice(&0i32.to_le_bytes());
    b.extend_from_slice(&((body.len() + 16) as u64).to_le_bytes());
    b.extend_from_slice(body);
    b
}

/// A 2 × 2 WiRE map stored in column order (like StreamLine), 3 points per spectrum.
fn wdf_map() -> Vec<u8> {
    let (points, count) = (3u32, 4u64);
    let mut h = vec![0u8; 496];
    let o = |a: usize| a - 16;
    h[o(0x3c)..o(0x40)].copy_from_slice(&points.to_le_bytes());
    h[o(0x40)..o(0x48)].copy_from_slice(&count.to_le_bytes());
    h[o(0x48)..o(0x50)].copy_from_slice(&count.to_le_bytes());
    h[o(0x50)..o(0x54)].copy_from_slice(&2u32.to_le_bytes());
    h[o(0x58)..o(0x5c)].copy_from_slice(&points.to_le_bytes());
    h[o(0x5c)..o(0x60)].copy_from_slice(&2u32.to_le_bytes());
    h[o(0x60)..o(0x64)].copy_from_slice(b"WiRE");
    for (i, v) in [5u16, 3, 0, 1].iter().enumerate() {
        h[o(0x78) + 2 * i..o(0x78) + 2 * i + 2].copy_from_slice(&v.to_le_bytes());
    }
    h[o(0x84)..o(0x88)].copy_from_slice(&3u32.to_le_bytes());
    h[o(0x88)..o(0x90)].copy_from_slice(&132_223_104_000_000_000u64.to_le_bytes());
    h[o(0x98)..o(0x9c)].copy_from_slice(&6u32.to_le_bytes());
    h[o(0x9c)..o(0xa0)].copy_from_slice(&(1e7f32 / 532.0).to_le_bytes());
    h[o(0xf0)..o(0xf0) + 6].copy_from_slice(b"my map");
    let mut f = b"WDF1".to_vec();
    f.extend_from_slice(&1i32.to_le_bytes());
    f.extend_from_slice(&512u64.to_le_bytes());
    f.extend_from_slice(&h);
    // spectra k = 0..4, point p: 100k + p
    let data: Vec<f32> = (0..4)
        .flat_map(|k| (0..3).map(move |p| (100 * k + p) as f32))
        .collect();
    f.extend(block(*b"DATA", &floats(&data)));
    let mut x = 1u32.to_le_bytes().to_vec();
    x.extend_from_slice(&1u32.to_le_bytes());
    x.extend(floats(&[1300.0, 1301.5, 1302.7]));
    f.extend(block(*b"XLST", &x));
    // column order: (x, y) = (0,0), (0,1), (1,0), (1,1) in µm steps of 2
    let xs = [10.0f64, 10.0, 12.0, 12.0];
    let ys = [20.0f64, 22.0, 20.0, 22.0];
    let mut org = 2u32.to_le_bytes().to_vec();
    for (ty, name, vals) in [(0x8000_0003u32, b"X", xs), (0x8000_0004, b"Y", ys)] {
        org.extend_from_slice(&ty.to_le_bytes());
        org.extend_from_slice(&5u32.to_le_bytes());
        let mut n = name.to_vec();
        n.resize(16, 0);
        org.extend_from_slice(&n);
        for v in vals {
            org.extend_from_slice(&v.to_le_bytes());
        }
    }
    f.extend(block(*b"ORGN", &org));
    let mut m = 2u32.to_le_bytes().to_vec();
    m.extend_from_slice(&0u32.to_le_bytes());
    for v in [10.0f32, 20.0, 0.0, 2.0, 2.0, 1.0] {
        m.extend_from_slice(&v.to_le_bytes());
    }
    for v in [2u32, 2, 1, 0] {
        m.extend_from_slice(&v.to_le_bytes());
    }
    f.extend(block(*b"WMAP", &m));
    f
}

#[test]
fn wdf_map_from_coordinates() {
    let f = wdf_map();
    let mut ds = open(&WdfReader, "m.wdf", f.clone()).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(t.sweep_count, 4);
    assert_eq!(t.channels.len(), 2);
    assert_eq!(t.extra["axis"]["irregular"], true);
    assert!((t.extra["laser_wavelength_nm"].as_f64().unwrap() - 532.0).abs() < 1e-3);
    assert_eq!(t.extra["accumulations"], 2);
    let tr = ds.read_trace(0, 2, 0, 9).unwrap();
    assert_eq!(tr.channels[0], vec![1300.0, 1301.5, f64::from(1302.7f32)]);
    assert_eq!(tr.channels[1], vec![200.0, 201.0, 202.0]);
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_c), (2, 2, 3));
    // pixel (row, col): spectrum 0 at (0,0), 1 at (1,0), 2 at (0,1), 3 at (1,1)
    let plane = ds.read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
    let px: Vec<f32> = plane
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    assert_eq!(px, vec![1.0, 201.0, 101.0, 301.0]);
    let tab = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(tab.columns[1], vec![10.0, 10.0, 12.0, 12.0]);
    assert_eq!(
        ds.experiment().unwrap().sample.unwrap().id.as_deref(),
        Some("my map")
    );
    for n in (0..f.len()).step_by(3) {
        match open(&WdfReader, "m.wdf", f[..n].to_vec()) {
            Ok(mut d) => {
                let _ = d.info();
                let _ = d.check();
                let _ = d.read_trace(0, 3, 0, 9);
                let _ = d.read_plane(0, PlaneIndex::default());
            }
            Err(e) => assert!(matches!(e.exit_code(), 4..=6), "{e}"),
        }
    }
}

// ---------------------------------------------------------------- PerkinElmer

fn member_block(id: u16, body: &[u8]) -> Vec<u8> {
    let mut b = id.to_le_bytes().to_vec();
    b.extend_from_slice(&(body.len() as i32).to_le_bytes());
    b.extend_from_slice(body);
    b
}

fn text(s: &str) -> Vec<u8> {
    let mut b = vec![0x23, 0x75];
    b.extend_from_slice(&(s.len() as u16).to_le_bytes());
    b.extend_from_slice(s.as_bytes());
    b
}

fn pe_file() -> Vec<u8> {
    let mut inner = Vec::new();
    let mut pair = vec![0x1d, 0x75];
    pair.extend_from_slice(&4000f64.to_le_bytes());
    pair.extend_from_slice(&3998f64.to_le_bytes());
    inner.extend(member_block(35698, &pair));
    let mut n = vec![0x2b, 0x75];
    n.extend_from_slice(&3u32.to_le_bytes());
    inner.extend(member_block(35701, &n));
    inner.extend(member_block(35703, &text("cm-1")));
    inner.extend(member_block(35704, &text("%T")));
    inner.extend(member_block(35713, &text("film")));
    let mut arr = vec![0x16, 0x75];
    arr.extend_from_slice(&24u32.to_le_bytes());
    for v in [99.5f64, 98.25, 97.0] {
        arr.extend_from_slice(&v.to_le_bytes());
    }
    inner.extend(member_block(35708, &arr));
    let mut f = b"PEPE".to_vec();
    let mut d = b"2D constant interval DataSet file".to_vec();
    d.resize(40, 0);
    f.extend(d);
    f.extend(member_block(120, &inner));
    f
}

#[test]
fn perkinelmer_values() {
    let f = pe_file();
    let mut ds = open(&PeSpReader, "f.sp", f.clone()).unwrap();
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!(t.name.as_deref(), Some("film"));
    assert_eq!(t.channels[0].name, "transmittance");
    assert_eq!(t.channels[0].unit.as_deref(), Some("%"));
    assert_eq!(t.extra["axis"]["step"], -1.0);
    assert_eq!(
        ds.read_trace(0, 0, 0, 9).unwrap().channels[0],
        vec![99.5, 98.25, 97.0]
    );
    for n in (0..f.len()).step_by(3) {
        match open(&PeSpReader, "f.sp", f[..n].to_vec()) {
            Ok(mut d) => {
                let _ = d.info();
                let _ = d.check();
                let _ = d.read_trace(0, 0, 0, 9);
            }
            Err(e) => assert!(matches!(e.exit_code(), 4..=6), "{e}"),
        }
    }
}

#[test]
fn sniffing() {
    use std::path::Path;
    assert!(
        OpusReader
            .sniff(&[0x0a, 0x0a, 0xfe, 0xfe, 0], Path::new("x.bin"))
            .is_some()
    );
    assert!(
        OmnicReader
            .sniff(b"Spectral Data File....", Path::new("x"))
            .is_some()
    );
    assert!(WdfReader.sniff(b"WDF1\x01\0\0\0", Path::new("x")).is_some());
    assert!(PeSpReader.sniff(b"PEPE2D", Path::new("x")).is_some());
    assert!(
        OpusReader
            .sniff(b"hello world", Path::new("x.txt"))
            .is_none()
    );
}
