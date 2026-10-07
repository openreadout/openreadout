//! Synthetic Zetasizer `.dts` files built from `docs/formats/malvern-zetasizer.md` (a minimal
//! compound-file writer below): a size and a zeta record read back into the records table;
//! damage ends in clean errors, and an ambiguous or missing result block returns no results.
#![allow(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::needless_range_loop
)] // exact synthetic values; sector arithmetic

use std::collections::BTreeMap;

use openreadout_biophys::{ZETASIZER_FORMAT_ID, ZetasizerReader};
use openreadout_core::source::Input;
use openreadout_core::{Dataset, Error, FormatReader};

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

fn string(text: &str) -> Vec<u8> {
    let u: Vec<u8> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut b = (u.len() as u32).to_le_bytes().to_vec();
    b.push(1);
    b.extend(u);
    b
}

fn f32s(b: &mut Vec<u8>, v: &[f32]) {
    for x in v {
        b.extend(x.to_le_bytes());
    }
}

fn counted(b: &mut Vec<u8>, v: &[f32]) {
    b.extend((v.len() as u32).to_le_bytes());
    f32s(b, v);
}

/// A record: header, dispersant, material block, sample name, padding, results.
fn record(kind: u16, number: u32, sample: &str, results: &[u8]) -> Vec<u8> {
    record_with(kind, number, sample, results, 1)
}

/// A record whose material block begins with `u32 head`.
fn record_with(kind: u16, number: u32, sample: &str, results: &[u8], head: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(13u16.to_le_bytes());
    b.extend(kind.to_le_bytes());
    b.extend([0u8; 8]);
    b.extend(number.to_le_bytes());
    b.extend(string("7.13"));
    b.extend(45_785.5f64.to_le_bytes()); // 2025-05-08T12:00:00
    b.extend(string("MAL1234567"));
    b.extend(1u32.to_le_bytes());
    b.extend(2f32.to_le_bytes());
    b.extend(24.5f32.to_le_bytes());
    b.extend([0u8; 40]);
    b.extend(string("Water"));
    b.extend([0u8; 30]);
    b.extend(string("Polystyrene latex"));
    let mut blk = vec![0u8; 46];
    blk[..4].copy_from_slice(&head.to_le_bytes());
    blk[4..6].copy_from_slice(&[1, 0]);
    blk[32..].copy_from_slice(&[2, 0, 0, 0, 1, 0, 0, 2, 0, 0, 0, 1, 0, 0]);
    b.extend(blk);
    b.extend(string(sample));
    b.extend([0u8; 64]);
    b.extend(results);
    b.extend([0u8; 64]);
    b
}

fn size_block() -> Vec<u8> {
    let mut b = Vec::new();
    f32s(&mut b, &[150.0, 0.2]);
    counted(&mut b, &[0.8; 12]);
    for (m, a, w) in [
        (
            &[160.0f32, 5000.0][..],
            &[95.0f32, 5.0][..],
            &[40.0f32, 300.0][..],
        ),
        (&[60.0][..], &[100.0][..], &[20.0][..]),
        (&[90.0][..], &[100.0][..], &[30.0][..]),
    ] {
        counted(&mut b, m);
        counted(&mut b, a);
        counted(&mut b, w);
    }
    b
}

fn zeta_block() -> Vec<u8> {
    let mut b = Vec::new();
    counted(&mut b, &[100.0, 0.0, 0.0, 0.0, 0.0]);
    counted(&mut b, &[-30.5, 0.0, 0.0, 0.0, 0.0]);
    b.extend(0.25f64.to_le_bytes());
    f32s(
        &mut b,
        &[149.0, -30.0, 4.5, -2.35, 0.35, 0.0, 0.0, 0.0, 0.0, 0.0],
    );
    counted(&mut b, &[4.5, 0.0, 0.0, 0.0, 0.0]);
    counted(&mut b, &[100.0, 0.0, 0.0, 0.0, 0.0]);
    counted(&mut b, &[-2.35, 0.0, 0.0, 0.0, 0.0]);
    counted(&mut b, &[0.35, 0.0, 0.0, 0.0, 0.0]);
    b
}

fn header() -> Vec<u8> {
    let mut h = vec![1u8, 0];
    h.extend([
        0xc0, 0xfe, 0xe4, 0xa2, 0xb2, 0x26, 0xd6, 0x11, 0x99, 0x1b, 0x00, 0x90, 0x27, 0x9b, 0x77,
        0x0c,
    ]);
    h.extend([1, 0]);
    h.extend(9u32.to_le_bytes());
    h
}

fn file(records: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
    let mut streams = vec![
        ("Header", header()),
        ("Deleted", vec![1, 0, 0, 0, 0, 0, 0, 0]),
    ];
    streams.extend(records);
    compound_file(&streams)
}

fn open(b: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    ZetasizerReader.open_input(&Input::from_bytes("m.dts", b))
}

fn cell(ds: &mut Box<dyn Dataset>, col: &str, row: usize) -> (f64, Option<String>) {
    let info = ds.info().unwrap();
    let k = info.tables[0]
        .columns
        .iter()
        .position(|c| c.name == col)
        .unwrap();
    let t = ds.read_table(0, 0, u64::MAX).unwrap();
    let v = t.columns[k][row];
    let text = info.tables[0].columns[k]
        .extra
        .get("categories")
        .and_then(|c| c.get(v as usize))
        .and_then(|s| s.as_str())
        .map(str::to_string);
    (v, text)
}

#[test]
fn reads_size_and_zeta_records() {
    let b = file(vec![
        ("REC8", record(2, 8, "latex zeta 1", &zeta_block())),
        ("REC7", record(1, 7, "latex 1", &size_block())),
    ]);
    let det = ZetasizerReader
        .sniff(&b[..512], std::path::Path::new("m.dts"))
        .unwrap();
    assert_eq!(det.format_id, ZETASIZER_FORMAT_ID);
    let mut ds = open(b).unwrap();
    // rows in record order
    assert_eq!(cell(&mut ds, "record", 0).0, 7.0);
    assert_eq!(cell(&mut ds, "kind", 0).1.as_deref(), Some("size"));
    assert_eq!(
        cell(&mut ds, "sample_name", 1).1.as_deref(),
        Some("latex zeta 1")
    );
    assert_eq!(
        cell(&mut ds, "measured_at", 0).1.as_deref(),
        Some("2025-05-08T12:00:00")
    );
    assert_eq!(cell(&mut ds, "temperature", 0).0, 24.5);
    // size results: Z-average, PdI, intensity peak means and areas; widths withheld
    assert_eq!(cell(&mut ds, "z_average", 0).0, 150.0);
    assert_eq!(cell(&mut ds, "pdi", 0).0, f64::from(0.2f32));
    assert_eq!(cell(&mut ds, "peak1_mean", 0).0, 160.0);
    assert_eq!(cell(&mut ds, "peak2_mean", 0).0, 5000.0);
    assert_eq!(cell(&mut ds, "peak2_area", 0).0, 5.0);
    assert!(cell(&mut ds, "peak1_width", 0).0.is_nan());
    assert!(cell(&mut ds, "peak3_mean", 0).0.is_nan());
    assert!(cell(&mut ds, "z_average", 1).0.is_nan());
    assert_eq!(cell(&mut ds, "zeta_potential", 1).0, -30.0);
    assert_eq!(cell(&mut ds, "mobility", 1).0, f64::from(-2.35f32));
    assert_eq!(cell(&mut ds, "conductivity", 1).0, 0.25);
    assert_eq!(cell(&mut ds, "zeta_peak1_mean", 1).0, -30.5);
    assert!(cell(&mut ds, "zeta_peak2_mean", 1).0.is_nan());
    let e = ds.experiment().unwrap();
    assert_eq!(e.instrument.unwrap().serial.as_deref(), Some("MAL1234567"));
}

#[test]
fn material_block_beginning_with_two() {
    let b = file(vec![(
        "REC1",
        record_with(1, 1, "latex 2", &size_block(), 2),
    )]);
    let mut ds = open(b).unwrap();
    assert_eq!(
        cell(&mut ds, "sample_name", 0).1.as_deref(),
        Some("latex 2")
    );
}

#[test]
fn ambiguous_or_missing_results_return_none() {
    // two size blocks in one record: neither is returned
    let mut two = size_block();
    two.extend([0u8; 16]);
    two.extend(size_block());
    let mut ds = open(file(vec![("REC1", record(1, 1, "a", &two))])).unwrap();
    assert!(cell(&mut ds, "z_average", 0).0.is_nan());
    // no block: the record is still listed
    let mut ds = open(file(vec![("REC1", record(1, 1, "a", &[]))])).unwrap();
    assert_eq!(cell(&mut ds, "sample_name", 0).1.as_deref(), Some("a"));
    assert!(cell(&mut ds, "z_average", 0).0.is_nan());
}

#[test]
fn damage_is_a_clean_error() {
    let good = file(vec![("REC1", record(1, 1, "a", &size_block()))]);
    for cut in [100, 512, 1024, good.len() / 2, good.len() - 1] {
        let _ = open(good[..cut].to_vec());
    }
    // not a Zetasizer compound file
    let other = compound_file(&[("Contents", vec![0u8; 64])]);
    assert!(matches!(open(other), Err(Error::Unsupported { .. })));
    // a record too short for its header
    let short = file(vec![("REC1", vec![13, 0, 1, 0])]);
    assert!(matches!(open(short), Err(Error::Corrupt { .. })));
    // every byte flipped in turn: an answer or an error
    for off in (512..good.len()).step_by(29) {
        let mut m = good.clone();
        m[off] ^= 0x5A;
        let _ = open(m).and_then(|mut d| d.read_table(0, 0, u64::MAX).map(|_| ()));
    }
}
