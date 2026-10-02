//! Synthetic Thermo Fisher EER movies (docs/formats/tiff.md § EER): pages of electron events
//! coded as run lengths, written by hand with their XML metadata tags, read back as frames;
//! `check` compares every frame's events with its recorded dose.

use std::path::{Path, PathBuf};

use openreadout_core::PixelType;
use openreadout_core::model::Severity;
use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_tiff::TiffReader;

const W: u32 = 16;
const H: u32 = 8;

/// Bit writer, least significant bit first.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, bits: u32) {
        self.acc |= u64::from(v) << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.acc & 0xFF) as u8);
        }
        if self.out.len() % 2 == 1 {
            self.out.push(0);
        }
        self.out
    }
}

/// Code the events at `pixels` (sorted, distinct) of a `total`-pixel frame.
fn encode(pixels: &[u32], total: u32, skip: u32, sub: (u32, u32)) -> Vec<u8> {
    let max = (1u32 << skip) - 1;
    let mut b = Bits::default();
    let mut cur = 0u32;
    let run = |b: &mut Bits, mut gap: u32| {
        while gap >= max {
            b.put(max, skip);
            gap -= max;
        }
        b.put(gap, skip);
    };
    for &p in pixels {
        run(&mut b, p - cur);
        b.put(1, sub.0);
        b.put(0, sub.1);
        cur = p + 1;
    }
    run(&mut b, total - cur);
    b.finish()
}

fn events(t: u32) -> Vec<u32> {
    // frame t: events at every (7 + t)-th pixel, plus a run longer than one code (pixel 0 → 127+)
    let mut v: Vec<u32> = (0..W * H).filter(|p| p % (7 + t) == 3).collect();
    if t == 0 {
        v.retain(|p| !(3..=60).contains(p));
    }
    v
}

fn item(name: &str, unit: Option<&str>, value: &str) -> String {
    match unit {
        Some(u) => format!(r#"<item name="{name}" unit="{u}">{value}</item>"#),
        None => format!(r#"<item name="{name}">{value}</item>"#),
    }
}

/// An EER movie of `frames` pages with compression `compression`; frame `bad_dose` records a
/// wrong dose.
fn eer(frames: u32, compression: u16, bits: (u32, u32, u32), bad_dose: Option<u32>) -> Vec<u8> {
    let mut b = b"II*\0\0\0\0\0".to_vec();
    let mut strips = Vec::new();
    for t in 0..frames {
        let s = encode(&events(t), W * H, bits.0, (bits.1, bits.2));
        strips.push((b.len() as u32, s.len() as u32));
        b.extend(s);
    }
    let acq = format!(
        "<metadata>{}{}{}{}{}{}</metadata>",
        item("commercialName", None, "Falcon 4i"),
        item("cameraName", None, "EF-Falcon"),
        item("exposureTime", Some("s"), "0.5"),
        item("numberOfFrames", None, &frames.to_string()),
        item("sensorPixelSize.width", Some("m"), "7.5e-11"),
        item("timestamp", None, "2025-03-14T17:40:48.482908+01:00"),
    );
    let mut ifd_offsets = Vec::new();
    for t in 0..frames {
        let n = events(t).len() as f64;
        let mut dose = n / f64::from(W * H);
        if bad_dose == Some(t) {
            dose += 0.01;
        }
        let frame = format!(
            "<metadata>{}{}</metadata>",
            item("frameID", None, &t.to_string()),
            item("dose", Some("e/pixel"), &format!("{dose:.6}")),
        );
        while b.len() % 2 == 1 {
            b.push(0);
        }
        let acq_at = b.len() as u32;
        if t == 0 {
            b.extend(acq.as_bytes());
        }
        while b.len() % 2 == 1 {
            b.push(0);
        }
        let frame_at = b.len() as u32;
        b.extend(frame.as_bytes());
        while b.len() % 2 == 1 {
            b.push(0);
        }
        ifd_offsets.push(b.len());
        let mut entries: Vec<(u16, u16, u32, u32)> = vec![
            (256, 4, 1, W),
            (257, 4, 1, H),
            (259, 3, 1, u32::from(compression)),
            (262, 3, 1, 1),
            (273, 4, 1, strips[t as usize].0),
            (274, 3, 1, 2),
            (278, 4, 1, H),
            (279, 4, 1, strips[t as usize].1),
        ];
        if t == 0 {
            entries.push((65001, 7, acq.len() as u32, acq_at));
        }
        entries.push((65002, 7, frame.len() as u32, frame_at));
        if compression == 65002 {
            entries.push((65007, 3, 1, bits.0));
            entries.push((65008, 3, 1, bits.1));
            entries.push((65009, 3, 1, bits.2));
        }
        b.extend((entries.len() as u16).to_le_bytes());
        for (tag, typ, count, value) in entries {
            b.extend(tag.to_le_bytes());
            b.extend(typ.to_le_bytes());
            b.extend(count.to_le_bytes());
            b.extend(value.to_le_bytes());
        }
        b.extend(0u32.to_le_bytes()); // next IFD, patched below
    }
    b[4..8].copy_from_slice(&(ifd_offsets[0] as u32).to_le_bytes());
    for i in 0..ifd_offsets.len() - 1 {
        let o = ifd_offsets[i];
        let n = u16::from_le_bytes([b[o], b[o + 1]]) as usize;
        let next_at = o + 2 + n * 12;
        let next = ifd_offsets[i + 1] as u32;
        b[next_at..next_at + 4].copy_from_slice(&next.to_le_bytes());
    }
    b
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn counts(t: u32) -> Vec<u8> {
    let mut v = vec![0u8; (W * H) as usize];
    for p in events(t) {
        v[p as usize] = 1;
    }
    v
}

#[test]
fn frames_metadata_and_dose_check() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "movie.eer", &eer(3, 65001, (7, 2, 2), None));
    let mut ds = TiffReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_z, im.size_t), (W, H, 1, 3));
    assert_eq!(im.pixel_type, PixelType::Uint8);
    assert!((im.physical_size.x.unwrap() - 7.5e-5).abs() < 1e-12);
    assert!((im.time_increment_s.unwrap() - 0.5 / 3.0).abs() < 1e-12);
    assert_eq!(
        im.instrument.as_ref().unwrap().detector.as_deref(),
        Some("Falcon 4i (EF-Falcon)")
    );
    assert_eq!(im.extra["eer"]["orientation"], 2);
    assert!(info.notes.iter().any(|n| n.contains("Orientation 2")));
    for t in 0..3 {
        let plane = ds.read_plane(0, PlaneIndex { c: 0, z: 0, t }).unwrap();
        assert_eq!(plane.data, counts(t), "frame {t}");
    }
    let (total, frames) = ds.frames(0, None).unwrap();
    assert_eq!(total, 3);
    assert_eq!(frames[2]["frameID"], 2);
    let report = ds.check().unwrap();
    assert!(report.ok, "{:?}", report.findings);
    assert!(
        report
            .findings
            .iter()
            .any(|f| f.code == "eer_events" && f.message.contains("3 frame(s) compared"))
    );
}

#[test]
fn variable_coding_and_a_wrong_dose() {
    let dir = tempfile::tempdir().unwrap();
    // 65002 with 5-bit runs and 1+3 sub-pixel bits (runs longer than 31 need continuations)
    let p = write(dir.path(), "v2.eer", &eer(2, 65002, (5, 1, 3), Some(1)));
    let mut ds = TiffReader.open(&p).unwrap();
    for t in 0..2 {
        let plane = ds.read_plane(0, PlaneIndex { c: 0, z: 0, t }).unwrap();
        assert_eq!(plane.data, counts(t), "frame {t}");
    }
    let report = ds.check().unwrap();
    let bad: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.code == "eer_dose_mismatch")
        .collect();
    assert_eq!(bad.len(), 1);
    assert_eq!(bad[0].severity, Severity::Warning);
}

#[test]
fn a_damaged_stream_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = eer(1, 65001, (7, 2, 2), None);
    // the first run code of frame 0 becomes 126, then every following code is garbage: the
    // runs overrun the 128-pixel frame
    for b in &mut bytes[8..14] {
        *b = 0x7E;
    }
    let p = write(dir.path(), "bad.eer", &bytes);
    let mut ds = TiffReader.open(&p).unwrap();
    let e = ds
        .read_plane(0, PlaneIndex::default())
        .expect_err("overrun");
    assert_eq!(e.exit_code(), 4);
    let report = ds.check().unwrap();
    assert!(report.findings.iter().any(|f| f.code == "eer_bad_frame"));
    assert!(!report.ok);
}
