//! Synthetic JASCO `.jws` files built from `docs/formats/jasco-jws.md`: the flat container and the
//! compound file (a minimal compound-file writer at the end), multi-channel CD layouts, the
//! refusals of layouts not validated, and damage (clean errors, never a panic).
#![allow(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::needless_range_loop,
    clippy::many_single_char_names
)] // exact synthetic values; sector arithmetic

use std::collections::BTreeMap;

use openreadout_core::source::Input;
use openreadout_core::{Dataset, Error, FormatReader};
use openreadout_spectro::{JWS_FORMAT_ID, JwsReader};

fn open(name: &str, bytes: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    JwsReader.open_input(&Input::from_bytes(name, bytes))
}

fn put_str(b: &mut [u8], at: usize, s: &str) {
    b[at..at + s.len()].copy_from_slice(s.as_bytes());
}

/// A flat `L~S ` file: V-730 style, `ymode`, descending 700-400 nm in 5 nm steps.
fn flat(ymode: u8, values: &[f32]) -> Vec<u8> {
    let mut b = vec![0u8; 0x740];
    put_str(&mut b, 0, "L~S ");
    put_str(&mut b, 0x08, "SPECMAN");
    put_str(&mut b, 0x20, "R2.0.0");
    put_str(&mut b, 0x30, "i80x86");
    b[0x84..0x88].copy_from_slice(&(values.len() as i32).to_le_bytes());
    let last = 700.0 - 5.0 * (values.len() as f64 - 1.0);
    b[0x88..0x90].copy_from_slice(&700.0f64.to_le_bytes());
    b[0x90..0x98].copy_from_slice(&last.to_le_bytes());
    b[0x98..0xA0].copy_from_slice(&(-5.0f64).to_le_bytes());
    b[0xA0] = 3;
    b[0xA1..0xA4].copy_from_slice(&[1, 0, 0x10]);
    b[0xA4] = ymode;
    b[0xC8..0xD0].copy_from_slice(&(values.len() as i64 * 4).to_le_bytes());
    put_str(&mut b, 0x140, "V-730");
    put_str(&mut b, 0x160, "H470561798");
    put_str(&mut b, 0x180, "blank");
    b[0x2C0..0x2C4].copy_from_slice(&1_780_556_881i32.to_le_bytes());
    for v in values {
        b.extend(v.to_le_bytes());
    }
    b
}

fn utf16(s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let mut b = ((units.len() * 2) as u32).to_le_bytes().to_vec();
    for u in units {
        b.extend(u.to_le_bytes());
    }
    b
}

/// `DataInfo` for `nchan` channels with descriptors `desc` (x first).
fn datainfo(
    nchan: u32,
    grid: u32,
    points: u32,
    first: f64,
    last: f64,
    step: f64,
    desc: [u32; 4],
) -> Vec<u8> {
    let mut b = Vec::new();
    for w in [3u32, 1, 0, nchan, grid, points] {
        b.extend(w.to_le_bytes());
    }
    for f in [first, last, step] {
        b.extend(f.to_le_bytes());
    }
    for d in desc {
        b.extend(d.to_le_bytes());
    }
    b.resize(96 + 44 * (nchan as usize - 1), 0);
    b
}

fn header_stream() -> Vec<u8> {
    let mut h = vec![0u8; 1024];
    for (k, u) in "L~".encode_utf16().enumerate() {
        h[2 * k..2 * k + 2].copy_from_slice(&u.to_le_bytes());
    }
    for (k, u) in "SPCMAN2".encode_utf16().enumerate() {
        h[8 + 2 * k..10 + 2 * k].copy_from_slice(&u.to_le_bytes());
    }
    h
}

/// A J-1500 CD file: CD, HT, absorbance over 5 points (260-252 nm).
fn cd_file(desc: [u32; 4], ydata_points: usize) -> Vec<u8> {
    let mut y = Vec::new();
    for ch in 0..3 {
        for i in 0..ydata_points {
            y.extend(((ch * 100 + i) as f32).to_le_bytes());
        }
    }
    let mut module = 2u32.to_le_bytes().to_vec();
    module.extend(utf16("CD-1500"));
    module.extend(2u16.to_le_bytes());
    module.extend(0x4003u16.to_le_bytes());
    module.extend(utf16("J-1500"));
    module.extend(utf16("A0000001"));
    let mut sample = 1u32.to_le_bytes().to_vec();
    sample.extend(utf16("lysozyme"));
    sample.extend(utf16("1 mm cell"));
    sample.extend(1u32.to_le_bytes());
    sample.extend(1u32.to_le_bytes());
    sample.extend(7u16.to_le_bytes());
    sample.extend(44_931.450_532_407_405f64.to_le_bytes());
    let mut meas = Vec::new();
    for w in [1u32, 1, 1] {
        meas.extend(w.to_le_bytes());
    }
    meas.extend(20u32.to_le_bytes());
    meas.extend(8u16.to_le_bytes());
    meas.extend(utf16("200 mdeg/1.0 dOD"));
    compound_file(&[
        ("Header", header_stream()),
        ("DataInfo", datainfo(3, 1, 5, 260.0, 252.0, -2.0, desc)),
        ("Y-Data", y),
        ("ModuleInfo", module),
        ("SampleInfo", sample),
        ("MeasParam", meas),
    ])
}

const CD_DESC: [u32; 4] = [0x1000_0103, 0x1001, 0x2001, 0x3];

#[test]
fn flat_file() {
    let vals: Vec<f32> = (0..61).map(|i| i as f32 * 0.001).collect();
    let mut ds = open("abs.jws", flat(3, &vals)).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format.id, JWS_FORMAT_ID);
    let t = &info.traces[0];
    assert_eq!(t.extra["y_quantity"], "absorbance");
    assert_eq!(t.extra["axis"]["first"], 700.0);
    assert_eq!(t.extra["axis"]["last"], 400.0);
    assert_eq!(t.extra["axis"]["unit"], "nm");
    let tr = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(tr.channels[0][60], f64::from(60.0f32 * 0.001));
    let e = ds.experiment().unwrap();
    assert_eq!(e.instrument.unwrap().model.as_deref(), Some("V-730"));
    assert_eq!(
        e.acquisition.unwrap().started_at.as_deref(),
        Some("2026-06-04T07:08:01Z")
    );
    assert!(ds.check().unwrap().ok);
    // unknown y mode and wrong data length are refused
    assert!(matches!(
        open("x.jws", flat(0x55, &vals)),
        Err(Error::Unsupported { .. })
    ));
    let mut short = flat(3, &vals);
    short.truncate(short.len() - 4);
    assert!(open("x.jws", short).is_err());
}

#[test]
fn compound_cd_file() {
    let mut ds = open("cd.jws", cd_file(CD_DESC, 5)).unwrap();
    let info = ds.info().unwrap();
    let names: Vec<_> = info
        .traces
        .iter()
        .map(|t| t.extra["y_quantity"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["circular_dichroism", "ht_voltage", "absorbance"]);
    assert_eq!(info.traces[0].channels[0].unit.as_deref(), Some("mdeg"));
    assert_eq!(info.traces[1].channels[0].unit.as_deref(), Some("V"));
    assert_eq!(info.traces[2].extra["axis"]["last"], 252.0);
    let ht = ds.read_trace(1, 0, 0, u64::MAX).unwrap();
    assert_eq!(ht.channels[0], vec![100.0, 101.0, 102.0, 103.0, 104.0]);
    let e = ds.experiment().unwrap();
    assert_eq!(
        e.instrument.as_ref().unwrap().model.as_deref(),
        Some("J-1500")
    );
    assert_eq!(
        e.sample
            .as_ref()
            .unwrap()
            .name
            .as_deref()
            .or(e.sample.as_ref().unwrap().id.as_deref()),
        Some("lysozyme")
    );
    assert_eq!(
        e.acquisition.unwrap().started_at.as_deref(),
        Some("2023-01-05T10:48:46Z")
    );
    assert!(ds.check().unwrap().ok);
}

#[test]
fn compound_refusals_and_damage() {
    // descriptor slots out of pattern
    assert!(matches!(
        open(
            "x.jws",
            cd_file([0x1000_0103, 0x1001, 0x2001, 0x1000_0103], 5)
        ),
        Err(Error::Unsupported { .. })
    ));
    // unknown x axis
    assert!(matches!(
        open("x.jws", cd_file([0x3000_0000, 0x1001, 0x2001, 0x3], 5)),
        Err(Error::Unsupported { .. })
    ));
    // Y-Data one point short
    assert!(matches!(
        open("x.jws", cd_file(CD_DESC, 4)),
        Err(Error::Corrupt { .. })
    ));
    // not a JASCO compound file
    let other = compound_file(&[("Data", vec![1, 2, 3])]);
    assert!(open("x.jws", other).is_err());
    // cuts and flipped bytes never panic
    let bytes = cd_file(CD_DESC, 5);
    for cut in (0..bytes.len()).step_by(61) {
        if let Ok(mut ds) = open("cut.jws", bytes[..cut].to_vec()) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, u64::MAX);
        }
    }
    for at in (0..bytes.len()).step_by(37) {
        let mut b = bytes.clone();
        b[at] ^= 0x5A;
        if let Ok(mut ds) = open("flip.jws", b) {
            let _ = ds.check();
            for t in 0..3 {
                let _ = ds.read_trace(t, 0, 0, u64::MAX);
            }
        }
    }
    let flat_bytes = flat(0, &[100.0; 11]);
    for cut in (0..flat_bytes.len()).step_by(29) {
        let _ = open("cut.jws", flat_bytes[..cut].to_vec()).map(|mut d| d.check());
    }
}

// ---------------------------------------------------------------- a minimal compound-file writer

const END: u32 = 0xFFFF_FFFE;
const FREE: u32 = 0xFFFF_FFFF;
const NONE: u32 = 0xFFFF_FFFF;

/// A version-3 compound file holding `streams` (`/`-separated paths), every stream in regular
/// 512-byte sectors (mini-stream cutoff 0).
fn compound_file(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    // directory: root, storages, streams
    struct Node {
        name: String,
        kind: u8,
        data: Vec<u8>,
        children: Vec<usize>,
    }
    let mut nodes = vec![Node {
        name: "Root Entry".into(),
        kind: 5,
        data: Vec::new(),
        children: Vec::new(),
    }];
    let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
    for (path, data) in streams {
        let mut parent = 0;
        let parts: Vec<&str> = path.split('/').collect();
        for k in 0..parts.len() - 1 {
            let key = parts[..=k].join("/");
            parent = *dirs.entry(key).or_insert_with(|| {
                nodes.push(Node {
                    name: parts[k].into(),
                    kind: 1,
                    data: Vec::new(),
                    children: Vec::new(),
                });
                let id = nodes.len() - 1;
                nodes[parent].children.push(id);
                id
            });
        }
        nodes.push(Node {
            name: parts[parts.len() - 1].into(),
            kind: 2,
            data: data.clone(),
            children: Vec::new(),
        });
        let id = nodes.len() - 1;
        nodes[parent].children.push(id);
    }
    let sectors = |n: usize| n.div_ceil(512);
    let dir_secs = sectors(nodes.len() * 128);
    let data_secs: usize = nodes.iter().map(|n| sectors(n.data.len())).sum();
    let mut fat_secs = 1;
    while fat_secs * 128 < fat_secs + dir_secs + data_secs {
        fat_secs += 1;
    }
    let total = fat_secs + dir_secs + data_secs;
    let mut fat = vec![FREE; fat_secs * 128];
    for f in fat.iter_mut().take(fat_secs) {
        *f = 0xFFFF_FFFD;
    }
    let chain = |fat: &mut Vec<u32>, first: usize, n: usize| {
        for s in first..first + n {
            fat[s] = if s + 1 < first + n {
                (s + 1) as u32
            } else {
                END
            };
        }
    };
    chain(&mut fat, fat_secs, dir_secs);
    let mut next = fat_secs + dir_secs;
    let mut starts = vec![END; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        let k = sectors(n.data.len());
        if k > 0 {
            starts[i] = next as u32;
            chain(&mut fat, next, k);
            next += k;
        }
    }
    let mut f = vec![0u8; 512 * (1 + total)];
    f[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    f[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
    f[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    f[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
    f[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    f[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    f[0x2C..0x30].copy_from_slice(&(fat_secs as u32).to_le_bytes());
    f[0x30..0x34].copy_from_slice(&(fat_secs as u32).to_le_bytes());
    f[0x38..0x3C].copy_from_slice(&0u32.to_le_bytes());
    f[0x3C..0x40].copy_from_slice(&END.to_le_bytes());
    f[0x44..0x48].copy_from_slice(&END.to_le_bytes());
    for i in 0..109 {
        let v = if i < fat_secs { i as u32 } else { FREE };
        f[0x4C + 4 * i..0x50 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in fat.iter().enumerate() {
        f[512 + 4 * i..516 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    let dir0 = 512 * (1 + fat_secs);
    // right-sibling chains under each storage
    let mut right = vec![NONE; nodes.len()];
    for n in &nodes {
        for w in n.children.windows(2) {
            right[w[0]] = w[1] as u32;
        }
    }
    for (i, n) in nodes.iter().enumerate() {
        let o = dir0 + 128 * i;
        let units: Vec<u16> = n.name.encode_utf16().collect();
        for (k, u) in units.iter().enumerate() {
            f[o + 2 * k..o + 2 * k + 2].copy_from_slice(&u.to_le_bytes());
        }
        f[o + 0x40..o + 0x42].copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
        f[o + 0x42] = n.kind;
        f[o + 0x43] = 1;
        f[o + 0x44..o + 0x48].copy_from_slice(&NONE.to_le_bytes());
        f[o + 0x48..o + 0x4C].copy_from_slice(&right[i].to_le_bytes());
        let child = n.children.first().map_or(NONE, |&c| c as u32);
        f[o + 0x4C..o + 0x50].copy_from_slice(&child.to_le_bytes());
        f[o + 0x74..o + 0x78].copy_from_slice(&starts[i].to_le_bytes());
        f[o + 0x78..o + 0x7C].copy_from_slice(&(n.data.len() as u32).to_le_bytes());
        if let Ok(s) = usize::try_from(starts[i])
            && s != END as usize
        {
            let at = 512 * (1 + s);
            f[at..at + n.data.len()].copy_from_slice(&n.data);
        }
    }
    // unused directory slots are empty entries
    for i in nodes.len()..dir_secs * 4 {
        let o = dir0 + 128 * i;
        for off in [0x44, 0x48, 0x4C] {
            f[o + off..o + off + 4].copy_from_slice(&NONE.to_le_bytes());
        }
    }
    f
}
