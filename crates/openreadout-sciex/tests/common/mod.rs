//! Builders for synthetic `.wiff` compound files and `.wiff.scan` companions, shared by the
//! integration tests.
// Each test binary uses some of these.
#![allow(dead_code)]

const FREE: u32 = 0xFFFF_FFFF;
const END: u32 = 0xFFFF_FFFE;
const FATSECT: u32 = 0xFFFF_FFFD;

/// A version-3 compound file (512-byte sectors, one FAT sector, no mini stream) holding
/// `streams` at their `/`-separated paths.
pub fn compound_file(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
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

pub fn preamble() -> Vec<u8> {
    let mut b = vec![0u8; 32];
    b[4] = 4;
    b
}

pub fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

pub fn experiment_header(scan_type: u16, polarity: u16, ranges: u32) -> Vec<u8> {
    let mut b = vec![0u8; 0xB4];
    b[0x56..0x58].copy_from_slice(&polarity.to_le_bytes());
    b[0x7A..0x7C].copy_from_slice(&scan_type.to_le_bytes());
    b[0xB0..0xB4].copy_from_slice(&ranges.to_le_bytes());
    b
}

/// `MassRangeEx` with (Q1, Q3, expected RT, name, CE) per transition.
pub fn mass_ranges(ranges: &[(f32, f32, f32, &str, f32)]) -> Vec<u8> {
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
pub fn index(records: &[(u32, u32, f64, f64, f64, f64)]) -> Vec<u8> {
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

pub fn sample_streams() -> Vec<(&'static str, Vec<u8>)> {
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

pub fn scan_file(chunks: &[Vec<u8>]) -> (Vec<u8>, Vec<(u32, u32)>) {
    let mut b = vec![0u8; 0x2C];
    let mut spans = Vec::new();
    for c in chunks {
        spans.push(((b.len() - 0x2C) as u32, c.len() as u32));
        b.extend_from_slice(c);
    }
    (b, spans)
}

pub fn f32s(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}
