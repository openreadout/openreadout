//! Fixtures written with the `tiff` crate's encoder, read back with our reader, and
//! cross-checked against the `tiff` crate's decoder.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_tiff::TiffReader;
use tiff::decoder::{Decoder, DecodingResult};
use tiff::encoder::compression::DeflateLevel;
use tiff::encoder::{Compression, TiffEncoder, colortype};
use tiff::tags::{Predictor, Tag};

fn tmp(name: &str) -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(name);
    (d, p)
}

fn ramp16(w: u32, h: u32, seed: u16) -> Vec<u16> {
    (0..w * h)
        .map(|i| (i as u16).wrapping_mul(7).wrapping_add(seed))
        .collect()
}

fn le16(v: &[u16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// Our plane bytes must equal the tiff crate's decoding of the same page.
fn tiff_crate_page(path: &Path, page: usize) -> Vec<u8> {
    let mut d = Decoder::new(std::io::BufReader::new(File::open(path).unwrap())).unwrap();
    d.seek_to_image(page).unwrap();
    match d.read_image().unwrap() {
        DecodingResult::U8(v) => v,
        DecodingResult::U16(v) => le16(&v),
        DecodingResult::F32(v) => v.iter().flat_map(|x| x.to_le_bytes()).collect(),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn multipage_u16_every_compression_and_predictor() {
    for (name, comp, pred) in [
        ("none.tif", Compression::Uncompressed, Predictor::None),
        ("lzw.tif", Compression::Lzw, Predictor::None),
        ("lzw-pred.tif", Compression::Lzw, Predictor::Horizontal),
        (
            "deflate.tif",
            Compression::Deflate(DeflateLevel::default()),
            Predictor::None,
        ),
        (
            "deflate-pred.tif",
            Compression::Deflate(DeflateLevel::default()),
            Predictor::Horizontal,
        ),
        ("packbits.tif", Compression::Packbits, Predictor::None),
    ] {
        let (_d, p) = tmp(name);
        let (w, h) = (37, 23);
        {
            let mut enc = TiffEncoder::new(BufWriter::new(File::create(&p).unwrap()))
                .unwrap()
                .with_compression(comp)
                .with_predictor(pred);
            for z in 0..3 {
                let mut img = enc.new_image::<colortype::Gray16>(w, h).unwrap();
                img.rows_per_strip(5).unwrap();
                img.write_data(&ramp16(w, h, z * 1000)).unwrap();
            }
        }
        let mut ds = TiffReader.open(&p).unwrap();
        let info = ds.info().unwrap();
        assert_eq!(info.images.len(), 1, "{name}");
        let im = &info.images[0];
        assert_eq!(
            (im.size_x, im.size_y, im.size_z, im.size_c, im.size_t),
            (w, h, 3, 1, 1)
        );
        assert!(
            info.notes.iter().any(|n| n.contains("Z planes")),
            "{name}: {:?}",
            info.notes
        );
        for z in 0..3u32 {
            let plane = ds.read_plane(0, PlaneIndex { c: 0, z, t: 0 }).unwrap();
            assert_eq!(
                plane.data,
                le16(&ramp16(w, h, z as u16 * 1000)),
                "{name} z={z}"
            );
            assert_eq!(
                plane.data,
                tiff_crate_page(&p, z as usize),
                "{name} vs tiff crate"
            );
        }
        assert!(ds.check().unwrap().ok, "{name}");
    }
}

#[test]
fn bigtiff_rgb8() {
    let (_d, p) = tmp("rgb.btf");
    let (w, h) = (19, 11);
    let data: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
    {
        let mut enc = TiffEncoder::new_big(BufWriter::new(File::create(&p).unwrap())).unwrap();
        enc.write_image::<colortype::RGB8>(w, h, &data).unwrap();
    }
    let mut ds = TiffReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!(im.samples_per_pixel, 3);
    assert_eq!(info.format_version.as_deref(), Some("6.0+BigTIFF"));
    let plane = ds.read_plane(0, PlaneIndex::default()).unwrap();
    assert_eq!(plane.data, data);
    assert_eq!(plane.data, tiff_crate_page(&p, 0));
}

#[test]
#[allow(clippy::many_single_char_names)]
fn imagej_hyperstack_order_and_calibration() {
    let (_d, p) = tmp("hyper.tif");
    let (w, h) = (8, 6);
    let (c, z, t) = (2u32, 3u32, 2u32);
    {
        let mut enc = TiffEncoder::new(BufWriter::new(File::create(&p).unwrap())).unwrap();
        // ImageJ default order: c fastest, then z, then t; page k holds seed k.
        for k in 0..c * z * t {
            let mut img = enc.new_image::<colortype::Gray16>(w, h).unwrap();
            if k == 0 {
                img.encoder()
                    .write_tag(
                        Tag::ImageDescription,
                        "ImageJ=1.54f\nimages=12\nchannels=2\nslices=3\nframes=2\nhyperstack=true\nunit=micron\nspacing=0.5\nfinterval=2.5\n",
                    )
                    .unwrap();
                img.resolution(
                    tiff::tags::ResolutionUnit::None,
                    tiff::encoder::Rational { n: 4, d: 1 },
                );
            }
            img.write_data(&vec![k as u16; (w * h) as usize]).unwrap();
        }
    }
    let mut ds = TiffReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_c, im.size_z, im.size_t), (c, z, t));
    assert_eq!(im.physical_size.x, Some(0.25));
    assert_eq!(im.physical_size.z, Some(0.5));
    assert_eq!(im.time_increment_s, Some(2.5));
    let plane = ds.read_plane(0, PlaneIndex { c: 1, z: 2, t: 1 }).unwrap();
    let k = 1 + c * (2 + z);
    assert_eq!(plane.data, le16(&vec![k as u16; (w * h) as usize]));
}

#[test]
fn ome_xml_plane_map_and_units() {
    let (_d, p) = tmp("x.ome.tif");
    let (w, h) = (4, 4);
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?><OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06" UUID="urn:uuid:1"><Image ID="Image:0" Name="img"><Pixels ID="Pixels:0" DimensionOrder="XYZTC" Type="uint16" SizeX="4" SizeY="4" SizeZ="2" SizeC="2" SizeT="1" PhysicalSizeX="250" PhysicalSizeXUnit="nm" PhysicalSizeY="0.25" PhysicalSizeZ="1.5"><Channel ID="Channel:0:0" Name="DAPI" SamplesPerPixel="1" Color="65535"/><Channel ID="Channel:0:1" Name="GFP" SamplesPerPixel="1"/><TiffData/></Pixels></Image></OME>"#;
    {
        let mut enc = TiffEncoder::new(BufWriter::new(File::create(&p).unwrap())).unwrap();
        for k in 0..4u16 {
            let mut img = enc.new_image::<colortype::Gray16>(w, h).unwrap();
            if k == 0 {
                img.encoder().write_tag(Tag::ImageDescription, xml).unwrap();
            }
            img.write_data(&[k; 16]).unwrap();
        }
    }
    let mut ds = TiffReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_c, im.size_z), (2, 2));
    assert_eq!(im.physical_size.x, Some(0.25));
    assert_eq!(im.physical_size.z, Some(1.5));
    assert_eq!(im.channels[0].name.as_deref(), Some("DAPI"));
    assert_eq!(im.channels[0].color.as_deref(), Some("#0000FF"));
    // XYZTC: z fastest → IFD 1 is (c=0, z=1), IFD 2 is (c=1, z=0).
    let plane = ds.read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
    assert_eq!(plane.data, le16(&[2u16; 16]));
    let plane = ds.read_plane(0, PlaneIndex { c: 0, z: 1, t: 0 }).unwrap();
    assert_eq!(plane.data, le16(&[1u16; 16]));
}

#[test]
#[allow(clippy::many_single_char_names)]
fn truncated_file_fails_check_and_reads_cleanly() {
    let (d, p) = tmp("t.tif");
    let (w, h) = (64, 64);
    {
        let mut enc = TiffEncoder::new(BufWriter::new(File::create(&p).unwrap())).unwrap();
        for z in 0..4 {
            enc.write_image::<colortype::Gray16>(w, h, &ramp16(w, h, z))
                .unwrap();
        }
    }
    let full = std::fs::read(&p).unwrap();
    let t = d.path().join("trunc.tif");
    std::fs::write(&t, &full[..full.len() * 6 / 10]).unwrap();
    let mut ds = TiffReader.open(&t).unwrap();
    let report = ds.check().unwrap();
    assert!(!report.ok, "{:?}", report.findings);
    assert!(
        report.findings.iter().any(|f| f.code == "truncated"),
        "{:?}",
        report.findings
    );
    // Garbage and empty files are clean errors, never panics.
    let g = d.path().join("garbage.tif");
    std::fs::write(&g, b"II*\0\xff\xff\xff\x7f").unwrap();
    assert!(TiffReader.open(&g).is_err());
    std::fs::write(&g, b"II*\0").unwrap();
    assert!(TiffReader.open(&g).is_err());
}

#[test]
fn float32_pages() {
    let (_d, p) = tmp("f.tif");
    let data: Vec<f32> = (0..30).map(|i| i as f32 * 0.5 - 3.0).collect();
    {
        let mut enc = TiffEncoder::new(BufWriter::new(File::create(&p).unwrap()))
            .unwrap()
            .with_compression(Compression::Deflate(DeflateLevel::default()));
        enc.write_image::<colortype::Gray32Float>(6, 5, &data)
            .unwrap();
    }
    let mut ds = TiffReader.open(&p).unwrap();
    let plane = ds.read_plane(0, PlaneIndex::default()).unwrap();
    assert_eq!(plane.pixel_type, openreadout_core::PixelType::Float);
    assert_eq!(plane.data, tiff_crate_page(&p, 0));
}

#[test]
fn renamed_single_file_ome_tiff_still_reads() {
    let (_d, p) = tmp("renamed.ome.tif");
    let xml = r#"<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2015-01"><Image ID="Image:0"><Pixels ID="Pixels:0" DimensionOrder="XYCZT" Type="uint8" SizeX="3" SizeY="2" SizeZ="1" SizeC="2" SizeT="1"><Channel ID="Channel:0:0"/><Channel ID="Channel:0:1"/><TiffData IFD="0" PlaneCount="1"><UUID FileName="original.ome.tif">urn:uuid:abc</UUID></TiffData><TiffData FirstC="1" IFD="1" PlaneCount="1"><UUID FileName="original.ome.tif">urn:uuid:abc</UUID></TiffData></Pixels></Image></OME>"#;
    {
        let mut enc = TiffEncoder::new(BufWriter::new(File::create(&p).unwrap())).unwrap();
        for k in 0..2u8 {
            let mut img = enc.new_image::<colortype::Gray8>(3, 2).unwrap();
            if k == 0 {
                img.encoder().write_tag(Tag::ImageDescription, xml).unwrap();
            }
            img.write_data(&[k + 10; 6]).unwrap();
        }
    }
    let mut ds = TiffReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert!(
        info.notes.iter().any(|n| n.contains("renamed")),
        "{:?}",
        info.notes
    );
    let plane = ds.read_plane(0, PlaneIndex { c: 1, z: 0, t: 0 }).unwrap();
    assert_eq!(plane.data, vec![11u8; 6]);
    assert!(ds.check().unwrap().ok);
}

/// Every truncation point and single-byte corruption of a small file: open/info/ls/check
/// and plane reads return values or clean errors, never panic.
#[test]
fn malformed_inputs_never_panic() {
    let (d, p) = tmp("m.tif");
    {
        let mut enc = TiffEncoder::new(BufWriter::new(File::create(&p).unwrap()))
            .unwrap()
            .with_compression(Compression::Lzw)
            .with_predictor(Predictor::Horizontal);
        for z in 0..2 {
            let mut img = enc.new_image::<colortype::Gray16>(9, 7).unwrap();
            img.rows_per_strip(3).unwrap();
            img.write_data(&ramp16(9, 7, z)).unwrap();
        }
    }
    let full = std::fs::read(&p).unwrap();
    let q = d.path().join("mut.tif");
    let exercise = |bytes: &[u8]| {
        std::fs::write(&q, bytes).unwrap();
        if let Ok(mut ds) = TiffReader.open(&q) {
            let _ = ds.entries();
            let _ = ds.vendor_metadata();
            let _ = ds.check();
            if let Ok(info) = ds.info() {
                for im in &info.images {
                    for z in 0..im.size_z.min(4) {
                        let _ = ds.read_plane(im.index, PlaneIndex { c: 0, z, t: 0 });
                    }
                }
            }
        }
    };
    for n in (0..full.len()).step_by(3) {
        exercise(&full[..n]);
    }
    for i in 0..full.len() {
        for flip in [0xFFu8, 0x80, 0x01] {
            let mut b = full.clone();
            b[i] ^= flip;
            exercise(&b);
        }
    }
}
