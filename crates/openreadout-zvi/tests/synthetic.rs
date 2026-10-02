//! A SYNTHETIC ZVI written by a tiny compound-file writer below (MS-CFB version 3, every stream
//! in regular sectors), following the layout documented in `docs/formats/zvi.md`: two channels ×
//! two z planes of 16-bit samples plus one colour image. Not from AxioVision.

use std::path::Path;

use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_zvi::{IMAGE_MARKER, ZviReader};

const FREE: u32 = 0xFFFF_FFFF;
const END: u32 = 0xFFFF_FFFE;

/// (path, bytes) streams; storages are implied by the paths.
fn compound_file(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    // directory entries: root, then every storage and stream (depth-first by path)
    #[derive(Clone)]
    struct Node {
        name: String,
        stream: Option<usize>,
        children: Vec<usize>,
    }
    let mut nodes = vec![Node {
        name: "Root Entry".into(),
        stream: None,
        children: vec![],
    }];
    for (si, (path, _)) in streams.iter().enumerate() {
        let mut parent = 0;
        let parts: Vec<&str> = path.split('/').collect();
        for (k, part) in parts.iter().enumerate() {
            let last = k == parts.len() - 1;
            let found = nodes[parent]
                .children
                .iter()
                .copied()
                .find(|&c| nodes[c].name == *part);
            parent = if let Some(c) = found {
                c
            } else {
                nodes.push(Node {
                    name: (*part).into(),
                    stream: last.then_some(si),
                    children: vec![],
                });
                let id = nodes.len() - 1;
                nodes[parent].children.push(id);
                id
            };
        }
    }
    // sector layout: 0 = FAT, then directory sectors, then streams
    let dir_sectors = (nodes.len() * 128).div_ceil(512);
    let mut fat: Vec<u32> = vec![0xFFFF_FFFD];
    let mut body: Vec<u8> = Vec::new();
    let dir_start = 1u32;
    for k in 0..dir_sectors {
        fat.push(if k + 1 == dir_sectors {
            END
        } else {
            dir_start + k as u32 + 1
        });
    }
    let mut starts = vec![END; streams.len()];
    let mut next = 1 + dir_sectors as u32;
    let mut data = Vec::new();
    for (si, (_, bytes)) in streams.iter().enumerate() {
        let n = bytes.len().div_ceil(512).max(1);
        starts[si] = next;
        for k in 0..n {
            fat.push(if k + 1 == n { END } else { next + k as u32 + 1 });
        }
        let mut b = bytes.clone();
        b.resize(n * 512, 0);
        data.extend_from_slice(&b);
        next += n as u32;
    }
    assert!(fat.len() <= 128, "test file too large for one FAT sector");
    fat.resize(128, FREE);
    // directory
    let mut dir = vec![0u8; dir_sectors * 512];
    for (i, n) in nodes.iter().enumerate() {
        let o = i * 128;
        for (k, u) in n.name.encode_utf16().enumerate() {
            dir[o + 2 * k..o + 2 * k + 2].copy_from_slice(&u.to_le_bytes());
        }
        dir[o + 0x40..o + 0x42].copy_from_slice(&((n.name.len() as u16 + 1) * 2).to_le_bytes());
        dir[o + 0x42] = match (i, n.stream) {
            (0, _) => 5,
            (_, Some(_)) => 2,
            _ => 1,
        };
        // siblings: chain the parent's children through the right-sibling pointer
        dir[o + 0x44..o + 0x48].copy_from_slice(&FREE.to_le_bytes());
        dir[o + 0x48..o + 0x4C].copy_from_slice(&FREE.to_le_bytes());
        dir[o + 0x4C..o + 0x50]
            .copy_from_slice(&n.children.first().map_or(FREE, |&c| c as u32).to_le_bytes());
        let (start, size) = n
            .stream
            .map_or((END, 0), |s| (starts[s], streams[s].1.len() as u32));
        dir[o + 0x74..o + 0x78].copy_from_slice(&start.to_le_bytes());
        dir[o + 0x78..o + 0x7C].copy_from_slice(&size.to_le_bytes());
    }
    for n in &nodes {
        for w in n.children.windows(2) {
            let o = w[0] * 128;
            dir[o + 0x48..o + 0x4C].copy_from_slice(&(w[1] as u32).to_le_bytes());
        }
    }
    let mut f = vec![0u8; 512];
    f[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    f[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
    f[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    f[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
    f[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    f[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    f[0x2C..0x30].copy_from_slice(&1u32.to_le_bytes());
    f[0x30..0x34].copy_from_slice(&dir_start.to_le_bytes());
    f[0x38..0x3C].copy_from_slice(&0u32.to_le_bytes());
    f[0x3C..0x40].copy_from_slice(&END.to_le_bytes());
    f[0x44..0x48].copy_from_slice(&END.to_le_bytes());
    for i in 0..109 {
        f[0x4C + 4 * i..0x50 + 4 * i].copy_from_slice(&FREE.to_le_bytes());
    }
    f[0x4C..0x50].copy_from_slice(&0u32.to_le_bytes());
    for v in &fat {
        body.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&body);
    f.extend_from_slice(&dir);
    f.extend_from_slice(&data);
    f
}

fn i4(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&v.to_le_bytes());
}
fn r8(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&5u16.to_le_bytes());
    out.extend_from_slice(&v.to_le_bytes());
}
fn text(out: &mut Vec<u8>, s: &str) {
    let u: Vec<u8> = s
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&(u.len() as u32).to_le_bytes());
    out.extend_from_slice(&u);
}

enum V<'a> {
    I(i32),
    F(f64),
    S(&'a str),
}

fn tag_list(tags: &[(u32, V<'_>)]) -> Vec<u8> {
    let mut b = Vec::new();
    i4(&mut b, 0x2000_1000);
    i4(&mut b, tags.len() as i32);
    for (id, v) in tags {
        match v {
            V::I(x) => i4(&mut b, *x),
            V::F(x) => r8(&mut b, *x),
            V::S(x) => text(&mut b, x),
        }
        i4(&mut b, *id as i32);
        i4(&mut b, 0);
    }
    b
}

fn item_contents(w: u32, h: u32, bpp: u32, fmt: u32, samples: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    i4(&mut b, 0x3000_1003);
    i4(&mut b, w as i32);
    i4(&mut b, h as i32);
    b.extend_from_slice(&[0x41, 0, 4, 0, 0, 0, 1, 2, 3, 4]); // a blob, as real items carry
    for v in [IMAGE_MARKER, w, h, 1, bpp, fmt, 16] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend_from_slice(samples);
    b
}

fn samples16(w: u32, h: u32, seed: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| ((i * 7 + seed * 1000) as u16).to_le_bytes())
        .collect()
}

fn write(path: &Path) {
    let (w, h) = (13u32, 5u32);
    let mut streams: Vec<(String, Vec<u8>)> = vec![(
        "Image/Tags/Contents".into(),
        tag_list(&[
            (515, V::I(w as i32)),
            (516, V::I(h as i32)),
            (769, V::F(0.25)),
            (770, V::I(76)),
            (772, V::F(0.25)),
            (773, V::I(76)),
            (775, V::F(1.5)),
            (776, V::I(76)),
            (1553, V::S("synthetic.zvi")),
        ]),
    )];
    // channel-major like a z stack: items 0,1 = channel 7 z 0,1; items 2,3 = channel 9
    for (n, (c, z)) in [(7, 0), (7, 1), (9, 0), (9, 1)].iter().enumerate() {
        streams.push((
            format!("Image/Item({n})/Contents"),
            item_contents(w, h, 2, 4, &samples16(w, h, n as u32)),
        ));
        streams.push((
            format!("Image/Item({n})/Tags/Contents"),
            tag_list(&[
                (2819, V::I(*z)),
                (2820, V::I(*c)),
                (1284, V::S(if *c == 7 { "GFP" } else { "DAPI" })),
                (1282, V::I(if *c == 7 { 0x0000_FF00 } else { 0x00FF_0000 })),
                (2564, V::F(if *c == 7 { 50.0 } else { 20.0 })),
                (1025, V::F(45_021.5)),
            ]),
        ));
    }
    let refs: Vec<(&str, Vec<u8>)> = streams
        .iter()
        .map(|(p, b)| (p.as_str(), b.clone()))
        .collect();
    std::fs::write(path, compound_file(&refs)).unwrap();
}

#[test]
fn synthetic_zvi_reads_planes_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("synthetic.zvi");
    write(&p);
    let head = std::fs::read(&p).unwrap();
    assert!(ZviReader.sniff(&head[..512], &p).is_some());
    let mut ds = ZviReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!(
        (im.size_x, im.size_y, im.size_z, im.size_c, im.size_t),
        (13, 5, 2, 2, 1)
    );
    assert_eq!(im.dimension_order, "XYZCT");
    assert_eq!(im.physical_size.x, Some(0.25));
    assert_eq!(im.physical_size.z, Some(1.5));
    assert_eq!(im.channels[0].name.as_deref(), Some("GFP"));
    assert_eq!(im.channels[1].color.as_deref(), Some("#0000FF"));
    assert_eq!(im.channels[1].exposure_ms, Some(20.0));
    assert_eq!(im.acquired_at.as_deref(), Some("2023-04-05T12:00:00.000"));
    assert_eq!(im.name.as_deref(), Some("synthetic.zvi"));
    // plane (c=1, z=1) is item 3
    let plane = ds.read_plane(0, PlaneIndex { c: 1, z: 1, t: 0 }).unwrap();
    assert_eq!(plane.data, samples16(13, 5, 3));
    assert!(ds.check().unwrap().ok);
    let (n, frames) = ds.frames(0, None).unwrap();
    assert_eq!(n, 4);
    assert_eq!(frames[3]["exposure_ms"], 20.0);
    // truncated copies: open fails cleanly or check / reads report it, never a panic
    for cut in [300usize, head.len() / 2, head.len() - 600] {
        let q = dir.path().join(format!("cut{cut}.zvi"));
        std::fs::write(&q, &head[..cut]).unwrap();
        match ZviReader.open(&q) {
            Err(e) => assert!(matches!(e.exit_code(), 4..=6), "{e}"),
            Ok(mut ds) => {
                let r = ds.check().unwrap();
                let read = ds.read_plane(0, PlaneIndex { c: 1, z: 1, t: 0 });
                assert!(!r.ok || read.is_err(), "cut at {cut}");
            }
        }
    }
}
