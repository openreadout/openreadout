//! A synthetic multi-frame Velox-layout EMD file (written with hdf5-pure) to cover what the
//! corpus lacks: frames as T, per-frame metadata columns, chunked storage, the thumbnail.

use openreadout_core::PixelType;
use openreadout_core::reader::{FormatReader, PlaneIndex};
use openreadout_em::EmdReader;

fn velox(path: &std::path::Path, rows: u64, cols: u64, frames: u64) {
    let mut b = hdf5_pure::FileBuilder::new();
    b.create_dataset("Version")
        .with_vlen_strings(&[r#"{"version": "10", "format": "Velox"}"#]);
    let mut data = b.create_group("Data");
    let mut image = data.create_group("Image");
    let mut id = image.create_group("0123456789abcdef0123456789abcdef");
    // element (y, x, t) = 100 y + 10 x + t
    let vals: Vec<u16> = (0..rows)
        .flat_map(|y| {
            (0..cols).flat_map(move |x| (0..frames).map(move |t| (100 * y + 10 * x + t) as u16))
        })
        .collect();
    id.create_dataset("Data")
        .with_u16_data(&vals)
        .with_shape(&[rows, cols, frames])
        .with_chunks(&[1, cols, frames]);
    let doc = br#"{"BinaryResult":{"Detector":"HAADF","PixelSize":{"width":"2e-10","height":"3e-10"},"PixelUnitX":"m","PixelUnitY":"m"},"Optics":{"AccelerationVoltage":"300000"},"Acquisition":{"AcquisitionStartDatetime":{"DateTime":"1600000000"}},"Instrument":{"Manufacturer":"FEI Company","InstrumentModel":"Test"}}"#;
    let n = 400usize;
    let mut md = vec![0u8; n * frames as usize];
    for f in 0..frames as usize {
        for (k, c) in doc.iter().enumerate() {
            md[k * frames as usize + f] = *c;
        }
    }
    id.create_dataset("Metadata")
        .with_u8_data(&md)
        .with_shape(&[n as u64, frames]);
    image.add_group(id.finish());
    data.add_group(image.finish());
    b.add_group(data.finish());
    b.create_dataset("Thumbnail.jpg")
        .with_u8_data(&[0xff, 0xd8, 0xff, 0xd9]);
    b.write(path).unwrap();
}

#[test]
fn multi_frame_velox_image() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("synthetic.emd");
    velox(&p, 3, 4, 5);
    let head = std::fs::read(&p).unwrap();
    assert!(EmdReader.sniff(&head, &p).is_some());
    let mut ds = EmdReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("Velox 10"));
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_t), (4, 3, 5));
    assert_eq!(im.pixel_type, PixelType::Uint16);
    assert_eq!(im.name.as_deref(), Some("HAADF"));
    assert_eq!(im.physical_size.x, Some(2e-4));
    assert_eq!(im.physical_size.y, Some(3e-4));
    assert_eq!(im.acquired_at.as_deref(), Some("2020-09-13T12:26:40.000Z"));
    assert_eq!(im.extra["voltage_kv"], 300.0);
    let plane = ds.read_plane(0, PlaneIndex { c: 0, z: 0, t: 2 }).unwrap();
    let got: Vec<u16> = plane
        .data
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let want: Vec<u16> = (0..3u16)
        .flat_map(|y| (0..4u16).map(move |x| 100 * y + 10 * x + 2))
        .collect();
    assert_eq!(got, want);
    assert!(ds.read_plane(0, PlaneIndex { c: 0, z: 0, t: 5 }).is_err());
    let r = ds.check().unwrap();
    assert!(r.ok, "{:?}", r.findings);
    assert_eq!(ds.attachments().unwrap()[0].extension, "jpg");
    assert_eq!(ds.read_attachment(0).unwrap(), vec![0xff, 0xd8, 0xff, 0xd9]);
}

#[test]
fn hdf5_without_velox_images_is_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("other.emd");
    let mut b = hdf5_pure::FileBuilder::new();
    b.create_dataset("x").with_u8_data(&[1, 2, 3]);
    b.write(&p).unwrap();
    match EmdReader.open(&p) {
        Err(e @ openreadout_core::Error::Unsupported { .. }) => assert_eq!(e.exit_code(), 6),
        other => panic!("{:?}", other.map(|_| ())),
    }
}
