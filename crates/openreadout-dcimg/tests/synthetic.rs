//! Synthetic DCIMG files in both layouts (docs/formats/dcimg.md): geometry, the stored-pixel
//! correction, frame records, and truncation (`check` errors, reads fail cleanly).

use std::path::{Path, PathBuf};

use openreadout_core::model::Severity;
use openreadout_core::{Error, FormatReader, PlaneIndex};
use openreadout_dcimg::DcimgReader;

const W: u32 = 16;
const H: u32 = 6;
const ROW: u32 = W * 2;
const FRAME: u32 = ROW * H;
/// Overwritten pixels: row 3, columns 0..4.
const JUNK_ROW: u32 = 3;

fn sample(t: u32, x: u32, y: u32) -> u16 {
    (t * 1000 + y * 100 + x) as u16
}

/// Frame `t` as stored: the first four pixels of `JUNK_ROW` hold 0, 65535, 0, 65535.
fn stored_frame(t: u32) -> Vec<u8> {
    let mut b = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let v = if y == JUNK_ROW && x < 4 {
                if x % 2 == 0 { 0 } else { 65535 }
            } else {
                sample(t, x, y)
            };
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    b
}

fn real_pixels(t: u32) -> Vec<u8> {
    (0..4)
        .flat_map(|x| sample(t, x, JUNK_ROW).to_le_bytes())
        .collect()
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

const STAMP0: u32 = 1_725_555_197;

/// Version 7: header 0x78, frames back to back, footer with counter, stamp and pixel tables.
fn packed(frames: u32) -> Vec<u8> {
    let hs = 0x78usize;
    let data_rel = 0x70u32;
    let session_data = u64::from(data_rel) + u64::from(frames * FRAME);
    let mut b = vec![0u8; hs + data_rel as usize];
    b[..8].copy_from_slice(b"DCIMG\0\0\0");
    put32(&mut b, 8, 7);
    put32(&mut b, 0x20, 1);
    put32(&mut b, 0x24, frames);
    put32(&mut b, 0x28, hs as u32);
    let s = hs;
    put32(&mut b, s + 0x20, frames);
    put32(&mut b, s + 0x24, 2);
    put32(&mut b, s + 0x28, 1);
    put32(&mut b, s + 0x2c, W);
    put32(&mut b, s + 0x30, ROW);
    put32(&mut b, s + 0x34, H);
    put32(&mut b, s + 0x38, FRAME);
    put32(&mut b, s + 0x44, data_rel);
    put64(&mut b, s + 0x48, session_data);
    for t in 0..frames {
        b.extend(stored_frame(t));
    }
    // footer
    let f = b.len();
    let n = frames as usize;
    let (counters, stamps, pixels) = (0x110usize, 0x110 + 4 * n, 0x110 + 12 * n);
    let size = pixels + 8 * n;
    let mut foot = vec![0u8; size];
    put32(&mut foot, 0, 7);
    put64(&mut foot, 8, 0xa0);
    put32(&mut foot, 0x28, size as u32);
    put64(&mut foot, 0xa0 + 0x30, counters as u64);
    put64(&mut foot, 0xa0 + 0x40, stamps as u64);
    put64(&mut foot, 0xa0 + 0x58, pixels as u64);
    put32(&mut foot, 0xa0 + 0x64, JUNK_ROW * ROW);
    put64(&mut foot, 0xa0 + 0x68, 8);
    for t in 0..n {
        // a dropped frame: the counter skips 5
        let counter = if t >= 5 { t as u32 + 1 } else { t as u32 };
        put32(&mut foot, counters + 4 * t, counter);
        put32(&mut foot, stamps + 8 * t, STAMP0 + t as u32);
        put32(&mut foot, stamps + 8 * t + 4, 250_000);
        foot[pixels + 8 * t..pixels + 8 * t + 8].copy_from_slice(&real_pixels(t as u32));
    }
    b.extend(foot);
    let session_bytes = (b.len() - s) as u64;
    put64(&mut b, s, session_bytes);
    let len = b.len() as u64;
    put64(&mut b, 0x30, len);
    put64(&mut b, 0x40, len);
    let _ = f;
    b
}

/// Version 0x1000000: header 0x70, a block table (camera text, sub-array, stored pixels when
/// `trailer` is 32), frames each followed by a trailer.
fn framed(frames: u32, trailer: u32) -> Vec<u8> {
    let hs = 0x70usize;
    let s = hs;
    let base = s + 0xa0;
    // blocks: kind 1 (16 bytes), kind 4 (56, only with stored pixels), camera block (248)
    let mut blocks: Vec<(u32, u32)> = vec![(1, 0x10)];
    if trailer == 32 {
        blocks.push((4, 0x38));
    }
    blocks.push((3, 0xf8));
    let table = s + 0xf0;
    let first_block = table + 16 * blocks.len();
    let mut off = first_block;
    let mut placed = Vec::new();
    for &(kind, size) in &blocks {
        placed.push((kind, size, off));
        off += size as usize;
    }
    let data = off.next_multiple_of(16);
    let mut b = vec![0u8; data];
    b[..8].copy_from_slice(b"DCIMG\0\0\0");
    put32(&mut b, 8, 0x0100_0000);
    put32(&mut b, 0x20, 1);
    put32(&mut b, 0x24, frames);
    put32(&mut b, 0x28, hs as u32);
    put32(&mut b, s + 0x3c, frames);
    put32(&mut b, s + 0x40, 2);
    put32(&mut b, s + 0x44, 1);
    put32(&mut b, s + 0x48, W);
    put32(&mut b, s + 0x4c, H);
    put32(&mut b, s + 0x50, ROW);
    put32(&mut b, s + 0x54, FRAME);
    put64(&mut b, s + 0x60, (data - s) as u64);
    put32(&mut b, s + 0x74, FRAME + trailer);
    put32(&mut b, s + 0x7c, trailer);
    for (i, &(kind, size, at)) in placed.iter().enumerate() {
        put32(&mut b, table + 16 * i, kind);
        put32(&mut b, table + 16 * i + 4, size);
        put64(&mut b, table + 16 * i + 8, (at - base) as u64);
        if kind == 4 {
            put32(&mut b, at + 12, 8);
            put32(&mut b, at + 16, JUNK_ROW * ROW);
        }
        if size == 0xf8 {
            b[at..at + 4].copy_from_slice(b"1.23");
            b[at + 0x40..at + 0x40 + 10].copy_from_slice(b"C11440-22C");
            b[at + 0x90..at + 0x90 + 11].copy_from_slice(b"S/N: 001234");
            let sub = [0u16, W as u16, 1020, H as u16];
            for (k, v) in sub.iter().enumerate() {
                b[at + 0xc8 + 2 * k..at + 0xca + 2 * k].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
    for t in 0..frames {
        if trailer == 32 {
            b.extend(stored_frame(t));
        } else {
            b.extend((0..H).flat_map(|y| (0..W).flat_map(move |x| sample(t, x, y).to_le_bytes())));
        }
        let mut tr = vec![0u8; trailer as usize];
        put32(&mut tr, 0, t);
        put32(&mut tr, 4, STAMP0 + t);
        put32(&mut tr, 8, 500_000);
        if trailer == 32 {
            tr[12..20].copy_from_slice(&real_pixels(t));
        }
        b.extend(tr);
    }
    b.extend(vec![0u8; 64]);
    let session_bytes = (b.len() - s) as u64;
    put64(&mut b, s, session_bytes);
    let len = b.len() as u64;
    put64(&mut b, 0x30, len);
    put64(&mut b, 0x40, len);
    b
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn expected(t: u32) -> Vec<u8> {
    (0..H)
        .flat_map(|y| (0..W).flat_map(move |x| sample(t, x, y).to_le_bytes()))
        .collect()
}

#[test]
fn packed_layout_reads_frames_with_stored_pixels_put_back() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "a.dcimg", &packed(8));
    assert!(
        DcimgReader
            .sniff(&std::fs::read(&p).unwrap()[..64], &p)
            .is_some()
    );
    let mut ds = DcimgReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_t, im.size_z), (W, H, 8, 1));
    assert_eq!(
        im.acquired_at.as_deref(),
        Some("2024-09-05T16:53:17.250000Z")
    );
    assert!((im.time_increment_s.unwrap() - 1.0).abs() < 1e-9);
    assert_eq!(im.extra["stored_pixels"]["row"], JUNK_ROW);
    for t in 0..8 {
        let plane = ds.read_plane(0, PlaneIndex { c: 0, z: 0, t }).unwrap();
        assert_eq!(plane.data, expected(t), "frame {t}");
    }
    let (total, recs) = ds.frames(0, Some(3)).unwrap();
    assert_eq!((total, recs.len()), (8, 3));
    assert_eq!(recs[2]["frame"], 2);
    assert_eq!(recs[2]["delta_t_s"], 2.0);
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    assert!(r.findings.iter().any(|f| f.code == "frame_counter_gap"));
}

#[test]
fn framed_layouts_with_and_without_stored_pixels() {
    let dir = tempfile::tempdir().unwrap();
    for trailer in [16u32, 32] {
        let p = write(
            dir.path(),
            &format!("f{trailer}.dcimg"),
            &framed(3, trailer),
        );
        let mut ds = DcimgReader.open(&p).unwrap();
        let info = ds.info().unwrap();
        let im = &info.images[0];
        assert_eq!((im.size_x, im.size_y, im.size_t), (W, H, 3));
        assert_eq!(
            im.instrument.as_ref().and_then(|i| i.model.as_deref()),
            Some("C11440-22C")
        );
        assert_eq!(im.extra["camera_serial"], "001234");
        assert_eq!(im.extra.contains_key("stored_pixels"), trailer == 32);
        for t in 0..3 {
            let plane = ds.read_plane(0, PlaneIndex { c: 0, z: 0, t }).unwrap();
            assert_eq!(plane.data, expected(t), "trailer {trailer} frame {t}");
        }
        let (_, recs) = ds.frames(0, None).unwrap();
        assert_eq!(recs[1]["acquired_at"], "2024-09-05T16:53:18.500000Z");
        let r = ds.check().unwrap();
        assert!(r.ok, "{:?}", r.findings);
    }
}

#[test]
fn truncated_files_fail_check_and_reads_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    for (name, full) in [("p.dcimg", packed(4)), ("f.dcimg", framed(4, 32))] {
        // cut in the middle of the third frame
        let cut = full.len() - (FRAME as usize) * 2 - 200;
        let p = write(dir.path(), name, &full[..cut]);
        let mut ds = DcimgReader.open(&p).unwrap();
        let r = ds.check().unwrap();
        assert!(!r.ok, "{name}");
        assert!(
            r.findings
                .iter()
                .any(|f| f.code == "truncated" && f.severity == Severity::Error),
            "{name}: {:?}",
            r.findings
        );
        assert!(ds.read_plane(0, PlaneIndex { c: 0, z: 0, t: 0 }).is_ok());
        let e = ds
            .read_plane(0, PlaneIndex { c: 0, z: 0, t: 3 })
            .unwrap_err();
        assert!(matches!(e, Error::Corrupt { .. }), "{name}: {e}");
        assert_eq!(e.exit_code(), 4);
        // a file cut inside its header does not open (exit 4)
        let p = write(dir.path(), &format!("h{name}"), &full[..0x60]);
        let e = DcimgReader.open(&p).err().expect("header cut");
        assert_eq!(e.exit_code(), 4, "{name}: {e}");
    }
}

#[test]
fn malformed_headers_never_panic() {
    let dir = tempfile::tempdir().unwrap();
    for (i, full) in [packed(3), framed(2, 32), framed(2, 16)]
        .into_iter()
        .enumerate()
    {
        for step in [1usize, 7, 13] {
            for at in (8..full.len().min(0x400)).step_by(step) {
                let mut b = full.clone();
                b[at] ^= 0xff;
                let p = write(dir.path(), &format!("m{i}.dcimg"), &b);
                if let Ok(mut ds) = DcimgReader.open(&p) {
                    let _ = ds.info();
                    let _ = ds.check();
                    let _ = ds.frames(0, None);
                    let _ = ds.read_plane(0, PlaneIndex::default());
                }
            }
        }
    }
}
