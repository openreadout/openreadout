//! MRC files built byte by byte from the MRC2014 header layout, covering what the corpus lacks:
//! mode 101 and 12, big-endian files, volume stacks, IMOD unsigned bytes, FEI and SerialEM
//! extended headers, complex data and truncation.

use std::path::{Path, PathBuf};

use openreadout_core::model::{DetectConfidence, Severity};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::{Error, PixelType};
use openreadout_em::MrcReader;

struct Spec {
    nx: i32,
    ny: i32,
    nz: i32,
    mode: i32,
    big: bool,
    ispg: i32,
    mz: i32,
    cell: [f32; 3],
    exttyp: &'static [u8; 4],
    ext: Vec<u8>,
    nint: i16,
    nreal: i16,
    imod_flags: Option<i32>,
    labels: Vec<&'static str>,
    map_id: bool,
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            nx: 4,
            ny: 3,
            nz: 2,
            mode: 2,
            big: false,
            ispg: 1,
            mz: 2,
            cell: [8.0, 6.0, 4.0],
            exttyp: b"\0\0\0\0",
            ext: Vec::new(),
            nint: 0,
            nreal: 0,
            imod_flags: None,
            labels: vec!["synthetic"],
            map_id: true,
        }
    }
}

fn header(s: &Spec) -> Vec<u8> {
    let mut b = vec![0u8; 1024];
    let put_i = |b: &mut Vec<u8>, o: usize, v: i32| {
        let x = if s.big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        };
        b[o..o + 4].copy_from_slice(&x);
    };
    let put_f = |b: &mut Vec<u8>, o: usize, v: f32| {
        let x = if s.big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        };
        b[o..o + 4].copy_from_slice(&x);
    };
    for (o, v) in [
        (0, s.nx),
        (4, s.ny),
        (8, s.nz),
        (12, s.mode),
        (28, s.nx),
        (32, s.ny),
        (36, s.mz),
    ] {
        put_i(&mut b, o, v);
    }
    for (k, v) in s.cell.iter().enumerate() {
        put_f(&mut b, 40 + 4 * k, *v);
    }
    for k in 0..3 {
        put_f(&mut b, 52 + 4 * k, 90.0);
        put_i(&mut b, 64 + 4 * k, k as i32 + 1);
    }
    put_f(&mut b, 76, 0.0);
    put_f(&mut b, 80, 1.0);
    put_f(&mut b, 84, 0.5);
    put_i(&mut b, 88, s.ispg);
    put_i(&mut b, 92, s.ext.len() as i32);
    b[104..108].copy_from_slice(s.exttyp);
    put_i(&mut b, 108, 20141);
    let short = |v: i16| {
        if s.big {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    b[128..130].copy_from_slice(&short(s.nint));
    b[130..132].copy_from_slice(&short(s.nreal));
    if let Some(f) = s.imod_flags {
        put_i(&mut b, 152, 1_146_047_817);
        put_i(&mut b, 156, f);
    }
    if s.map_id {
        b[208..212].copy_from_slice(b"MAP ");
    }
    b[212..216].copy_from_slice(if s.big {
        &[0x11, 0x11, 0, 0]
    } else {
        &[0x44, 0x44, 0, 0]
    });
    put_f(&mut b, 216, 0.25);
    put_i(&mut b, 220, s.labels.len() as i32);
    for (k, l) in s.labels.iter().enumerate() {
        b[224 + 80 * k..224 + 80 * k + l.len()].copy_from_slice(l.as_bytes());
    }
    b
}

fn write(dir: &Path, name: &str, s: &Spec, data: &[u8]) -> PathBuf {
    let mut bytes = header(s);
    bytes.extend_from_slice(&s.ext);
    bytes.extend_from_slice(data);
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn open(p: &Path) -> Box<dyn Dataset> {
    MrcReader.open(p).unwrap()
}

fn plane(ds: &mut Box<dyn Dataset>, z: u32, t: u32) -> Vec<u8> {
    ds.read_plane(0, PlaneIndex { c: 0, z, t }).unwrap().data
}

#[test]
fn float_volume_geometry_and_physical_size() {
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..24).flat_map(|v| (v as f32).to_le_bytes()).collect();
    let p = write(dir.path(), "v.mrc", &Spec::default(), &data);
    let mut ds = open(&p);
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_z, im.size_t), (4, 3, 2, 1));
    assert_eq!(im.pixel_type, PixelType::Float);
    // 8 Å / 4 = 2 Å = 0.0002 µm
    assert_eq!(im.physical_size.x, Some(2.0 * 1e-4));
    assert_eq!(im.physical_size.y, Some(2.0 * 1e-4));
    assert_eq!(im.physical_size.z, Some(2.0 * 1e-4));
    assert_eq!(info.format_version.as_deref(), Some("20141"));
    assert_eq!(plane(&mut ds, 1, 0), data[48..].to_vec());
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
}

#[test]
fn big_endian_int16_is_returned_little_endian() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spec {
        mode: 1,
        big: true,
        nz: 1,
        mz: 1,
        ..Spec::default()
    };
    let data: Vec<u8> = (0..12i16).flat_map(|v| (v - 5).to_be_bytes()).collect();
    let p = write(dir.path(), "be.mrc", &s, &data);
    let mut ds = open(&p);
    let want: Vec<u8> = (0..12i16).flat_map(|v| (v - 5).to_le_bytes()).collect();
    assert_eq!(plane(&mut ds, 0, 0), want);
    assert_eq!(
        ds.info().unwrap().images[0].extra["byte_order"],
        "big-endian"
    );
}

#[test]
fn half_floats_are_widened() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spec {
        mode: 12,
        nz: 1,
        mz: 1,
        nx: 2,
        ny: 1,
        ..Spec::default()
    };
    // 1.0 and -2.0 as binary16
    let p = write(dir.path(), "h.mrc", &s, &[0x00, 0x3c, 0x00, 0xc0]);
    let mut ds = open(&p);
    assert_eq!(ds.info().unwrap().images[0].pixel_type, PixelType::Float);
    let got = plane(&mut ds, 0, 0);
    assert_eq!(
        got,
        [1.0f32.to_le_bytes(), (-2.0f32).to_le_bytes()].concat()
    );
}

#[test]
fn packed_4_bit_is_unpacked_with_row_padding() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spec {
        mode: 101,
        nx: 3,
        ny: 2,
        nz: 1,
        mz: 1,
        ..Spec::default()
    };
    let p = write(dir.path(), "p.mrc", &s, &[0x21, 0x03, 0x54, 0x06]);
    let mut ds = open(&p);
    assert_eq!(ds.info().unwrap().images[0].pixel_type, PixelType::Uint8);
    assert_eq!(plane(&mut ds, 0, 0), vec![1, 2, 3, 4, 5, 6]);
    assert!(ds.check().unwrap().ok);
}

#[test]
fn imod_unsigned_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spec {
        mode: 0,
        nz: 1,
        mz: 1,
        imod_flags: Some(0),
        ..Spec::default()
    };
    let p = write(dir.path(), "u.mrc", &s, &[200u8; 12]);
    let ds = open(&p);
    let info = ds.info().unwrap();
    assert_eq!(info.images[0].pixel_type, PixelType::Uint8);
    assert_eq!(info.images[0].extra["bytes_signed"], false);
    let s = Spec {
        mode: 0,
        nz: 1,
        mz: 1,
        imod_flags: Some(1),
        ..Spec::default()
    };
    let p = write(dir.path(), "s.mrc", &s, &[200u8; 12]);
    assert_eq!(
        open(&p).info().unwrap().images[0].pixel_type,
        PixelType::Int8
    );
}

#[test]
fn image_stack_and_volume_stack_layouts() {
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..72).flat_map(|v| (v as f32).to_le_bytes()).collect();
    // ISPG 0: 6 images as T
    let s = Spec {
        ispg: 0,
        nz: 6,
        mz: 6,
        ..Spec::default()
    };
    let p = write(dir.path(), "stack.mrcs", &s, &data);
    let im = open(&p).info().unwrap().images.remove(0);
    assert_eq!((im.size_z, im.size_t), (1, 6));
    assert_eq!(im.physical_size.z, None);
    // the same header in a .map is a volume by convention
    let p = write(dir.path(), "stack.map", &s, &data);
    let im = open(&p).info().unwrap().images.remove(0);
    assert_eq!((im.size_z, im.size_t), (6, 1));
    // ISPG 401: 3 volumes of 2 sections
    let s = Spec {
        ispg: 401,
        nz: 6,
        mz: 2,
        ..Spec::default()
    };
    let p = write(dir.path(), "vs.mrc", &s, &data);
    let mut ds = open(&p);
    let im = ds.info().unwrap().images.remove(0);
    assert_eq!((im.size_z, im.size_t), (2, 3));
    // section t*2+z = 2*2+1 = 5
    assert_eq!(plane(&mut ds, 1, 2), data[5 * 48..6 * 48].to_vec());
    // NZ not a multiple of MZ: check reports it
    let s = Spec {
        ispg: 401,
        nz: 5,
        mz: 2,
        ..Spec::default()
    };
    let p = write(dir.path(), "bad.mrc", &s, &data[..5 * 48]);
    let r = open(&p).check().unwrap();
    assert!(r.findings.iter().any(|f| f.code == "bad_volume_stack"));
}

#[test]
fn fei1_extended_header_is_decoded() {
    let dir = tempfile::tempdir().unwrap();
    let mut block = vec![0u8; 768];
    block[0..4].copy_from_slice(&768i32.to_le_bytes());
    block[12..20].copy_from_slice(&43_831.5f64.to_le_bytes()); // 2020-01-01T12:00Z
    block[20..27].copy_from_slice(b"Krios42");
    block[52..55].copy_from_slice(b"EPU");
    block[84..92].copy_from_slice(&300_000f64.to_le_bytes());
    block[156..164].copy_from_slice(&2e-10f64.to_le_bytes());
    block[419..427].copy_from_slice(&1.5f64.to_le_bytes());
    block[435..441].copy_from_slice(b"Falcon");
    let mut ext = block.clone();
    ext.extend_from_slice(&block);
    let s = Spec {
        mode: 6,
        ispg: 0,
        nz: 2,
        mz: 1,
        exttyp: b"FEI1",
        ext,
        ..Spec::default()
    };
    let data = vec![1u8; 4 * 3 * 2 * 2];
    let p = write(dir.path(), "fei.mrc", &s, &data);
    let ds = open(&p);
    let info = ds.info().unwrap();
    let im = &info.images[0];
    let ins = im.instrument.as_ref().unwrap();
    assert_eq!(ins.model.as_deref(), Some("Krios42"));
    assert_eq!(ins.software.as_deref(), Some("EPU"));
    assert_eq!(ins.detector.as_deref(), Some("Falcon"));
    assert_eq!(im.acquired_at.as_deref(), Some("2020-01-01T12:00:00.000Z"));
    assert_eq!(im.extra["high_tension_kv"], 300.0);
    assert_eq!(im.channels[0].exposure_ms, Some(1500.0));
    let (n, recs) = ds.frames(0, None).unwrap();
    assert_eq!(n, 2);
    assert_eq!(recs[1]["section"], 1);
    assert_eq!(recs[1]["camera_name"], "Falcon");
    // CELLA/MX = 2 Å, FEI pixel size 2e-10 m = 2 Å: no mismatch; the attachment is the raw block pair
    let mut ds = ds;
    let r = ds.check().unwrap();
    assert!(
        !r.findings.iter().any(|f| f.code == "pixel_size_mismatch"),
        "{:?}",
        r.findings
    );
    assert_eq!(ds.attachments().unwrap()[0].size, 1536);
    assert_eq!(ds.read_attachment(0).unwrap().len(), 1536);
}

#[test]
fn serialem_extended_header_gives_tilt_angles() {
    let dir = tempfile::tempdir().unwrap();
    let mut ext = Vec::new();
    for tilt in [-6000i16, 0, 6000] {
        ext.extend_from_slice(&tilt.to_le_bytes());
        ext.extend_from_slice(&(290i16).to_le_bytes());
    }
    let s = Spec {
        mode: 1,
        ispg: 0,
        nz: 3,
        mz: 3,
        exttyp: b"SERI",
        ext,
        nint: 4,
        nreal: 1 | 8,
        ..Spec::default()
    };
    let p = write(dir.path(), "ts.st", &s, &[0u8; 4 * 3 * 2 * 3]);
    let ds = open(&p);
    let (n, recs) = ds.frames(0, None).unwrap();
    assert_eq!(n, 3);
    assert_eq!(recs[0]["tilt_angle_deg"], -60.0);
    assert_eq!(recs[2]["magnification"], 29_000.0);
}

#[test]
fn complex_float_pairs_are_returned_as_stored() {
    let dir = tempfile::tempdir().unwrap();
    for big in [false, true] {
        let s = Spec {
            mode: 4,
            nz: 1,
            mz: 1,
            big,
            ..Spec::default()
        };
        // 4 x 3 complex values: (k, -k / 2)
        let values: Vec<f32> = (0..12u8)
            .flat_map(|k| [f32::from(k), -f32::from(k) / 2.0])
            .collect();
        let data: Vec<u8> = values
            .iter()
            .flat_map(|v| {
                if big {
                    v.to_be_bytes()
                } else {
                    v.to_le_bytes()
                }
            })
            .collect();
        let p = write(dir.path(), "fft.mrc", &s, &data);
        let mut ds = open(&p);
        let info = ds.info().unwrap();
        assert_eq!(info.images[0].extra["complex"], true);
        assert_eq!(info.images[0].pixel_type, PixelType::ComplexFloat);
        let got = plane(&mut ds, 0, 0);
        let want: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(got, want, "big-endian {big}");
    }
}

#[test]
fn complex_int16_pairs_are_widened_to_complex_float() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spec {
        mode: 3,
        nz: 1,
        mz: 1,
        ..Spec::default()
    };
    let parts: Vec<i16> = (0..24i16).map(|k| (k - 12) * 1000).collect();
    let data: Vec<u8> = parts.iter().flat_map(|v| v.to_le_bytes()).collect();
    let p = write(dir.path(), "fft3.mrc", &s, &data);
    let mut ds = open(&p);
    assert_eq!(
        ds.info().unwrap().images[0].pixel_type,
        PixelType::ComplexFloat
    );
    let want: Vec<u8> = parts
        .iter()
        .flat_map(|v| f32::from(*v).to_le_bytes())
        .collect();
    assert_eq!(plane(&mut ds, 0, 0), want);
}

#[test]
fn gzip_compressed_map_reads_like_the_plain_one() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..24).flat_map(|v| (v as f32).to_le_bytes()).collect();
    let plain = write(dir.path(), "v.map", &Spec::default(), &data);
    let bytes = std::fs::read(&plain).unwrap();
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&bytes).unwrap();
    let gz = dir.path().join("emd_1.map.gz");
    std::fs::write(&gz, enc.finish().unwrap()).unwrap();

    let head = std::fs::read(&gz).unwrap();
    let det = MrcReader.sniff(&head, &gz).expect("gzip MRC is detected");
    assert_eq!(det.confidence, DetectConfidence::Definite);

    let mut a = open(&plain);
    let mut b = open(&gz);
    let (ia, ib) = (a.info().unwrap(), b.info().unwrap());
    assert_eq!(ia.images[0].size_x, ib.images[0].size_x);
    assert_eq!(ia.images[0].size_z, ib.images[0].size_z);
    assert!(ib.notes.iter().any(|n| n.contains("gzip-compressed")));
    for z in 0..2 {
        assert_eq!(plane(&mut a, z, 0), plane(&mut b, z, 0));
    }

    // a gzip file that is not an MRC inside is not claimed
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&[7u8; 2048]).unwrap();
    let other = dir.path().join("x.map.gz");
    let bytes = enc.finish().unwrap();
    std::fs::write(&other, &bytes).unwrap();
    assert!(MrcReader.sniff(&bytes, &other).is_none());
}

#[test]
fn truncation_is_an_error_in_check_and_on_read() {
    let dir = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..24).flat_map(|v| (v as f32).to_le_bytes()).collect();
    let p = write(dir.path(), "t.mrc", &Spec::default(), &data[..60]);
    let mut ds = open(&p);
    assert!(
        ds.info()
            .unwrap()
            .notes
            .iter()
            .any(|n| n.contains("truncated"))
    );
    let r = ds.check().unwrap();
    assert!(!r.ok);
    let f = r.findings.iter().find(|f| f.code == "truncated").unwrap();
    assert_eq!(f.severity, Severity::Error);
    assert!(ds.read_plane(0, PlaneIndex::default()).is_ok());
    match ds.read_plane(0, PlaneIndex { c: 0, z: 1, t: 0 }) {
        Err(e @ Error::Corrupt { .. }) => assert_eq!(e.exit_code(), 4),
        other => panic!("{:?}", other.map(|p| p.data.len())),
    }
    // a file shorter than the header does not open
    std::fs::write(dir.path().join("short.mrc"), [0u8; 100]).unwrap();
    assert!(matches!(
        MrcReader.open(&dir.path().join("short.mrc")),
        Err(Error::Corrupt { .. })
    ));
}

#[test]
fn header_inconsistencies_are_findings() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spec {
        labels: vec!["a", "", "c"],
        map_id: false,
        ..Spec::default()
    };
    let mut bytes = header(&s);
    bytes[220..224].copy_from_slice(&5i32.to_le_bytes()); // NLABL lies
    bytes.extend_from_slice(&[0u8; 96]);
    bytes.extend_from_slice(&[9u8; 7]); // trailing bytes
    let p = dir.path().join("h.mrc");
    std::fs::write(&p, bytes).unwrap();
    let mut ds = open(&p);
    let codes: Vec<String> = ds
        .check()
        .unwrap()
        .findings
        .into_iter()
        .map(|f| f.code)
        .collect();
    for c in [
        "missing_map_id",
        "label_count",
        "label_gap",
        "trailing_bytes",
    ] {
        assert!(codes.iter().any(|x| x == c), "{c} missing from {codes:?}");
    }
}

#[test]
fn sniffing() {
    let dir = tempfile::tempdir().unwrap();
    let with = header(&Spec::default());
    let without = header(&Spec {
        map_id: false,
        ..Spec::default()
    });
    let d = MrcReader.sniff(&with, &dir.path().join("x.bin")).unwrap();
    assert_eq!(d.confidence, DetectConfidence::Definite);
    let d = MrcReader
        .sniff(&without, &dir.path().join("x.rec"))
        .unwrap();
    assert_eq!(d.confidence, DetectConfidence::Likely);
    assert!(
        MrcReader
            .sniff(&without, &dir.path().join("x.bin"))
            .is_none()
    );
    assert!(
        MrcReader
            .sniff(&[0u8; 2000], &dir.path().join("x.map"))
            .is_none()
    );
}
