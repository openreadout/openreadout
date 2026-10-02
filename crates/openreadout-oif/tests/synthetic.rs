//! SYNTHETIC FluoView data sets written below (not from FluoView), following the layout in
//! `docs/formats/oif.md`: an OIB compound file (MS-CFB version 3, every stream in regular sectors)
//! and the same files as an OIF folder. Two channels × two z planes of 16-bit samples, a lambda
//! variant, and truncated / incomplete copies for `check`.

use std::path::Path;

use openreadout_core::Error;
use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_oif::{OibReader, OifReader};

const FREE: u32 = 0xFFFF_FFFF;
const END: u32 = 0xFFFF_FFFE;

/// (path, bytes) streams; storages are implied by the paths.
fn compound_file(streams: &[(String, Vec<u8>)]) -> Vec<u8> {
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
    let dir_sectors = (nodes.len() * 128).div_ceil(512);
    let mut fat: Vec<u32> = vec![0xFFFF_FFFD];
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
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(&dir);
    f.extend_from_slice(&data);
    f
}

/// A little-endian, uncompressed, single-strip 16-bit grey TIFF.
fn tiff16(w: u32, h: u32, samples: &[u8]) -> Vec<u8> {
    let entries: [(u16, u16, u32, u32); 8] = [
        (256, 4, 1, w),
        (257, 4, 1, h),
        (258, 3, 1, 16),
        (259, 3, 1, 1),
        (262, 3, 1, 1),
        (273, 4, 1, 0), // patched below
        (278, 4, 1, h),
        (279, 4, 1, samples.len() as u32),
    ];
    let ifd_len = 2 + entries.len() * 12 + 4;
    let data_at = 8 + ifd_len as u32;
    let mut b = b"II*\0".to_vec();
    b.extend_from_slice(&8u32.to_le_bytes());
    b.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, ty, n, v) in entries {
        let v = if tag == 273 { data_at } else { v };
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&ty.to_le_bytes());
        b.extend_from_slice(&n.to_le_bytes());
        if ty == 3 {
            b.extend_from_slice(&(v as u16).to_le_bytes());
            b.extend_from_slice(&[0, 0]);
        } else {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(samples);
    b
}

fn utf16(s: &str) -> Vec<u8> {
    let mut b = vec![0xFF, 0xFE];
    b.extend(
        s.replace('\n', "\r\n")
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    b
}

fn samples(w: u32, h: u32, seed: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| ((i * 3 + seed * 1000) as u16).to_le_bytes())
        .collect()
}

const W: u32 = 11;
const H: u32 = 7;

fn main_settings(axis_order: &str, lambda: bool) -> String {
    let mut s = format!(
        "[Acquisition Parameters Common]\nImageCaputreDate='2021-03-04 05:06:07'\nImageCaputreDate+MilliSec=89\nScanMode=\"XY\"\n\
[Axis 0 Parameters Common]\nAxisCode=\"X\"\nMaxSize={W}\n\
[Axis 3 Parameters Common]\nAxisCode=\"Z\"\nInterval=1500\nPixUnit=\"nm\"\nMaxSize=2\n\
[Axis 4 Parameters Common]\nAxisCode=\"T\"\nMaxSize=0\n\
[Axis Parameter Common]\nAxisOrder=\"{axis_order}\"\n\
[Channel 1 Parameters]\nCH Name=\"CH1\"\nDyeName=\"DAPI\"\nEmissionWavelength=461\nExcitationWavelength=405\nLightType=\"Fluorescence\"\n\
[Channel 2 Parameters]\nCH Name=\"CH2\"\nDyeName=\"None\"\nEmissionWavelength=0\nExcitationWavelength=559\n\
[File Info]\nDataName=\"synthetic.oib\"\n\
[Reference Image Parameter]\nWidthConvertValue=0,207.0\nWidthUnit=\"um\"\nHeightConvertValue=0,207.0\nHeightUnit=\"um\"\nValidBitCounts=12\n\
[Version Info]\nFileVersion=\"1.2.6.0\"\nSystemName=\"FLUOVIEW FV1000\"\nSystemVersion=\"4.2.2.9\"\n"
    );
    if lambda {
        s.push_str("[Axis 6 Parameters Common]\nAxisCode=\"L\"\nStartPosition=500.0\nInterval=10\nResolution=10\nMaxSize=2\n");
    }
    s
}

fn pty(t_ms: f64, z_nm: f64) -> String {
    format!(
        "[Acquisition Parameters Common]\nMagnification=60.0\nObjectiveLens NAValue=1.35\nObjectiveLens Name=\"UPLSAPO   60X O  NA:1.35\"\nPMTVoltage=600\n\
[Axis 3 Parameters]\nAbsPositionValue={z_nm}\n[Axis 4 Parameters]\nAbsPositionValue={t_ms}\n\
[Image Parameters]\nWidthConvertValue=0.207\nWidthUnit=\"um\"\nHeightConvertValue=0.207\nHeightUnit=\"um\"\nValidBitCounts=12\n"
    )
}

/// The member files: `(file name, bytes)`, main settings excluded.
fn members(lambda: bool) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let second = if lambda { 'L' } else { 'Z' };
    for c in 1..=2u32 {
        for k in 1..=2u32 {
            let stem = if lambda {
                format!("s_C001{second}{c:03}")
            } else {
                format!("s_C{c:03}{second}{k:03}")
            };
            if lambda && k == 2 {
                continue;
            }
            let seed = c * 10 + k;
            out.push((format!("{stem}.tif"), tiff16(W, H, &samples(W, H, seed))));
            out.push((
                format!("{stem}.pty"),
                utf16(&pty(f64::from(k - 1) * 250.0, 1000.0 * f64::from(k))),
            ));
        }
    }
    out.push(("s_Thumb.bmp".into(), b"BM\0\0\0\0".to_vec()));
    out
}

fn write_oib(path: &Path, lambda: bool) {
    let files = members(lambda);
    let mut info = String::from(
        "[OibSaveInfo]\nVersion=2.0.0.0\nName=COFAImages\nMainFileName=Stream00000\nStream00000=synthetic.oif\nStorage00001=synthetic.oif.files\n",
    );
    let mut streams = vec![
        ("OibInfo.txt".to_string(), Vec::new()),
        (
            "Stream00000".to_string(),
            utf16(&main_settings(if lambda { "XYCL" } else { "XYCZ" }, lambda)),
        ),
    ];
    for (k, (name, bytes)) in files.into_iter().enumerate() {
        let key = format!("Stream{:05}", k + 1);
        info.push_str(&format!("{key}=Storage00001/{name}\n"));
        streams.push((format!("Storage00001/{key}"), bytes));
    }
    streams[0].1 = utf16(&info);
    std::fs::write(path, compound_file(&streams)).unwrap();
}

fn write_oif(dir: &Path) -> std::path::PathBuf {
    let main = dir.join("synthetic.oif");
    std::fs::write(&main, utf16(&main_settings("XYCZ", false))).unwrap();
    let folder = dir.join("synthetic.oif.files");
    std::fs::create_dir_all(&folder).unwrap();
    for (name, bytes) in members(false) {
        std::fs::write(folder.join(name), bytes).unwrap();
    }
    main
}

#[test]
fn oib_geometry_planes_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("synthetic.oib");
    write_oib(&p, false);
    let head = std::fs::read(&p).unwrap();
    assert!(OibReader.sniff(&head[..64], &p).is_some());
    let mut ds = OibReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 1);
    let im = &info.images[0];
    assert_eq!(
        (im.size_x, im.size_y, im.size_z, im.size_c, im.size_t),
        (W, H, 2, 2, 1)
    );
    assert_eq!(im.dimension_order, "XYCZT");
    assert_eq!(im.physical_size.x, Some(0.207));
    assert_eq!(im.physical_size.z, Some(1.5));
    assert_eq!(im.channels[0].name.as_deref(), Some("CH1"));
    assert_eq!(im.channels[0].fluorophore.as_deref(), Some("DAPI"));
    assert_eq!(im.channels[1].emission_nm, None);
    assert_eq!(im.acquired_at.as_deref(), Some("2021-03-04T05:06:07.089"));
    let obj = im.objective.as_ref().unwrap();
    assert_eq!(obj.lens_na, Some(1.35));
    assert_eq!(obj.model.as_deref(), Some("UPLSAPO 60X O NA:1.35"));
    for c in 0..2 {
        for z in 0..2 {
            let plane = ds.read_plane(0, PlaneIndex { c, z, t: 0 }).unwrap();
            assert_eq!(plane.data, samples(W, H, (c + 1) * 10 + z + 1));
        }
    }
    let (n, frames) = ds.frames(0, None).unwrap();
    assert_eq!(n, 4);
    assert_eq!(frames[1]["stage_z_um"], 2.0);
    assert!(ds.check().unwrap().ok);
    assert_eq!(ds.attachments().unwrap().len(), 1);
    assert_eq!(ds.read_attachment(0).unwrap(), b"BM\0\0\0\0");
    assert!(ds.read_plane(0, PlaneIndex { c: 2, z: 0, t: 0 }).is_err());
}

#[test]
fn lambda_bands_are_channels() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("lambda.oib");
    write_oib(&p, true);
    let mut ds = OibReader.open(&p).unwrap();
    let im = ds.info().unwrap().images.remove(0);
    assert_eq!((im.size_c, im.size_z), (2, 1));
    assert_eq!(im.channels[1].emission_range_nm, Some([510.0, 520.0]));
    let plane = ds.read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
    assert_eq!(plane.data, samples(W, H, 21));
}

#[test]
fn truncated_oib_is_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t.oib");
    write_oib(&p, false);
    let full = std::fs::read(&p).unwrap();
    // cut into the last plane TIFF: the thumbnail (1 sector) and the last .pty (2 sectors) follow it
    std::fs::write(&p, &full[..full.len() - 1950]).unwrap();
    match OibReader.open(&p) {
        Err(e) => assert_eq!(e.exit_code(), 4, "{e}"),
        Ok(mut ds) => {
            let r = ds.check().unwrap();
            assert!(!r.ok, "check must report the truncation");
            let err = ds
                .read_plane(0, PlaneIndex { c: 1, z: 1, t: 0 })
                .unwrap_err();
            assert!(matches!(err, Error::Corrupt { .. }), "{err}");
            assert_eq!(err.exit_code(), 4);
        }
    }
    // a file cut inside the directory cannot be opened at all
    std::fs::write(&p, &full[..900]).unwrap();
    let err = OibReader
        .open(&p)
        .err()
        .expect("a cut directory must not open");
    assert_eq!(err.exit_code(), 4, "{err}");
}

#[test]
fn oif_folder_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let main = write_oif(dir.path());
    let head = std::fs::read(&main).unwrap();
    assert!(OifReader.sniff(&head, &main).is_some());
    let mut ds = OifReader.open(&main).unwrap();
    assert_eq!(ds.info().unwrap().images[0].plane_count, 4);
    assert_eq!(
        ds.read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 })
            .unwrap()
            .data,
        samples(W, H, 21)
    );
    assert!(ds.check().unwrap().ok);
    // a plane file cut short
    let tif = dir.path().join("synthetic.oif.files/s_C002Z002.tif");
    let b = std::fs::read(&tif).unwrap();
    std::fs::write(&tif, &b[..b.len() - 10]).unwrap();
    let mut ds = OifReader.open(&main).unwrap();
    assert!(!ds.check().unwrap().ok);
    let err = ds
        .read_plane(0, PlaneIndex { c: 1, z: 1, t: 0 })
        .unwrap_err();
    assert_eq!(err.exit_code(), 4, "{err}");
    // a plane file missing: the grid has a hole
    std::fs::remove_file(&tif).unwrap();
    let mut ds = OifReader.open(&main).unwrap();
    assert!(!ds.check().unwrap().ok);
    // the whole data folder missing: opens, but check fails
    std::fs::remove_dir_all(dir.path().join("synthetic.oif.files")).unwrap();
    let mut ds = OifReader.open(&main).unwrap();
    assert!(ds.info().unwrap().images.is_empty());
    assert!(!ds.check().unwrap().ok);
}
