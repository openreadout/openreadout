//! Readers on files built here: DICOM series, NIfTI and Analyze, a ParaVision experiment.

use std::path::Path;

use openreadout_core::{FormatReader, PixelType, PlaneIndex};
use openreadout_medical::{DicomReader, NiftiExportOptions, NiftiReader, ParavisionReader, export_nifti};

// ---------------------------------------------------------------------------------------- DICOM

fn el(tag: u32, vr: &[u8; 2], v: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&((tag >> 16) as u16).to_le_bytes());
    b.extend_from_slice(&((tag & 0xFFFF) as u16).to_le_bytes());
    b.extend_from_slice(vr);
    let long = matches!(vr, b"OB" | b"OW" | b"SQ" | b"UN" | b"UT");
    let mut v = v.to_vec();
    if v.len() % 2 == 1 {
        v.push(if matches!(vr, b"UI" | b"OB") { 0 } else { b' ' });
    }
    if long {
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&(v.len() as u32).to_le_bytes());
    } else {
        b.extend_from_slice(&(v.len() as u16).to_le_bytes());
    }
    b.extend_from_slice(&v);
    b
}

fn us(v: u16) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// A CT slice: 4 x 3 pixels of stored value `base + i`, intercept -1024, at height `z` mm.
fn ct_slice(z: f64, instance: u32, base: u16) -> Vec<u8> {
    let mut f = vec![0u8; 128];
    f.extend_from_slice(b"DICM");
    let mut meta = Vec::new();
    meta.extend(el(0x0002_0001, b"OB", &[0, 1]));
    meta.extend(el(0x0002_0002, b"UI", b"1.2.840.10008.5.1.4.1.1.2"));
    meta.extend(el(0x0002_0010, b"UI", b"1.2.840.10008.1.2.1"));
    f.extend(el(0x0002_0000, b"UL", &(meta.len() as u32).to_le_bytes()));
    f.extend(meta);
    f.extend(el(0x0008_0016, b"UI", b"1.2.840.10008.5.1.4.1.1.2"));
    f.extend(el(0x0008_0060, b"CS", b"CT"));
    f.extend(el(0x0008_103E, b"LO", b"synthetic series"));
    f.extend(el(0x0010_0010, b"PN", b"Doe^Jane"));
    f.extend(el(0x0010_0020, b"LO", b"MRN123456"));
    f.extend(el(0x0010_0030, b"DA", b"19700101"));
    f.extend(el(0x0020_000E, b"UI", b"1.2.3.4.5"));
    f.extend(el(0x0020_0013, b"IS", instance.to_string().as_bytes()));
    f.extend(el(0x0020_0032, b"DS", format!("-10\\-20\\{z}").as_bytes()));
    f.extend(el(0x0020_0037, b"DS", b"1\\0\\0\\0\\1\\0"));
    f.extend(el(0x0028_0002, b"US", &us(1)));
    f.extend(el(0x0028_0004, b"CS", b"MONOCHROME2"));
    f.extend(el(0x0028_0010, b"US", &us(3)));
    f.extend(el(0x0028_0011, b"US", &us(4)));
    f.extend(el(0x0028_0030, b"DS", b"0.5\\0.25"));
    f.extend(el(0x0028_0100, b"US", &us(16)));
    f.extend(el(0x0028_0101, b"US", &us(12)));
    f.extend(el(0x0028_0102, b"US", &us(11)));
    f.extend(el(0x0028_0103, b"US", &us(0)));
    f.extend(el(0x0028_1050, b"DS", b"40"));
    f.extend(el(0x0028_1051, b"DS", b"400"));
    f.extend(el(0x0028_1052, b"DS", b"-1024"));
    f.extend(el(0x0028_1053, b"DS", b"1"));
    let px: Vec<u8> = (0..12u16).flat_map(|i| (base + i).to_le_bytes()).collect();
    f.extend(el(0x7FE0_0010, b"OW", &px));
    f
}

#[test]
fn a_folder_of_slices_is_one_volume_in_hounsfield_units() {
    let dir = tempfile::tempdir().unwrap();
    // written out of order: the reader sorts by position along the normal
    for (name, z, inst, base) in [("b.dcm", 5.0, 2, 1100), ("a.dcm", 2.5, 1, 1000), ("c.dcm", 7.5, 3, 1200)] {
        std::fs::write(dir.path().join(name), ct_slice(z, inst, base)).unwrap();
    }
    let mut ds = DicomReader.open(dir.path()).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.images.len(), 1);
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_z, im.size_t), (4, 3, 3, 1));
    assert_eq!(im.pixel_type, PixelType::Int16);
    assert_eq!(im.physical_size.x, Some(250.0));
    assert_eq!(im.physical_size.y, Some(500.0));
    assert_eq!(im.physical_size.z, Some(2500.0));
    assert_eq!(im.extra["value_unit"], "HU");
    assert_eq!(im.extra["orientation"]["plane"], "axial");
    let p = ds.read_plane(0, PlaneIndex { c: 0, z: 0, t: 0 }).unwrap();
    let v: Vec<i16> = p.data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    assert_eq!(v[0], 1000 - 1024);
    assert_eq!(v[11], 1011 - 1024);
    let top = ds.read_plane(0, PlaneIndex { c: 0, z: 2, t: 0 }).unwrap();
    assert_eq!(i16::from_le_bytes([top.data[0], top.data[1]]), 1200 - 1024);
    assert!(ds.check().unwrap().ok);
}

#[test]
fn patient_identifiers_are_flagged_and_never_printed() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("x.dcm");
    std::fs::write(&f, ct_slice(0.0, 1, 0)).unwrap();
    let ds = DicomReader.open(&f).unwrap();
    let info = serde_json::to_string(&ds.info().unwrap()).unwrap();
    let vendor = serde_json::to_string(&ds.vendor_metadata().unwrap()).unwrap();
    for secret in ["Doe", "Jane", "MRN123456", "19700101"] {
        assert!(!info.contains(secret), "{secret} in info");
        assert!(!vendor.contains(secret), "{secret} in the vendor tree");
    }
    assert!(info.contains("PatientName (0010,0010)") && info.contains("person_name"));
    assert!(info.contains("date_of_birth") && info.contains("patient_id"));
    assert!(vendor.contains("withheld"));
}

#[test]
fn malformed_dicom_is_an_error_not_a_panic() {
    let good = ct_slice(0.0, 1, 0);
    let dir = tempfile::tempdir().unwrap();
    for cut in [0usize, 100, 132, 140, 200, good.len() - 10] {
        let f = dir.path().join(format!("cut{cut}.dcm"));
        std::fs::write(&f, &good[..cut.max(1)]).unwrap();
        if let Ok(mut ds) = DicomReader.open(&f) {
            let _ = ds.info();
            let _ = ds.read_plane(0, PlaneIndex::default());
            let _ = ds.check();
        }
    }
    // a hostile length
    let mut bad = good.clone();
    let n = bad.len();
    bad[n - 30..n - 26].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    let f = dir.path().join("bad.dcm");
    std::fs::write(&f, bad).unwrap();
    if let Ok(mut ds) = DicomReader.open(&f) {
        let _ = ds.read_plane(0, PlaneIndex::default());
    }
}

// ---------------------------------------------------------------------------------------- NIfTI

fn nifti(dims: &[i16], datatype: i16, bitpix: i16, slope: f32, data: &[u8], big: bool) -> Vec<u8> {
    let mut h = vec![0u8; 352];
    let i16b = |v: i16| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    let f32b = |v: f32| if big { v.to_be_bytes() } else { v.to_le_bytes() };
    h[0..4].copy_from_slice(&if big { 348i32.to_be_bytes() } else { 348i32.to_le_bytes() });
    h[40..42].copy_from_slice(&i16b(dims.len() as i16));
    for (i, d) in dims.iter().enumerate() {
        h[42 + 2 * i..44 + 2 * i].copy_from_slice(&i16b(*d));
    }
    h[70..72].copy_from_slice(&i16b(datatype));
    h[72..74].copy_from_slice(&i16b(bitpix));
    for (i, v) in [1.0f32, 2.0, 2.0, 3.0, 1.5].iter().enumerate() {
        h[76 + 4 * i..80 + 4 * i].copy_from_slice(&f32b(*v));
    }
    h[108..112].copy_from_slice(&f32b(352.0));
    h[112..116].copy_from_slice(&f32b(slope));
    h[123] = 2 | 8;
    h[344..348].copy_from_slice(b"n+1\0");
    h.extend_from_slice(data);
    h
}

#[test]
fn nifti_planes_scaling_and_byte_order() {
    let dir = tempfile::tempdir().unwrap();
    // 3 x 2 x 2 x 2 int16, little endian, slope 0.5
    let vals: Vec<i16> = (0..24).collect();
    let data: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
    let f = dir.path().join("a.nii");
    std::fs::write(&f, nifti(&[3, 2, 2, 2], 4, 16, 0.5, &data, false)).unwrap();
    let mut ds = NiftiReader.open(&f).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_z, im.size_t), (3, 2, 2, 2));
    assert_eq!(im.pixel_type, PixelType::Float);
    assert_eq!(im.physical_size.z, Some(3000.0));
    assert_eq!(im.time_increment_s, Some(1.5));
    let p = ds.read_plane(0, PlaneIndex { c: 0, z: 1, t: 1 }).unwrap();
    let v: Vec<f32> = p.data.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    // plane index z + nz * t = 3 → values 18..24, times 0.5
    assert_eq!(v, vec![9.0, 9.5, 10.0, 10.5, 11.0, 11.5]);
    // big-endian copy, no scaling
    let be: Vec<u8> = vals.iter().flat_map(|v| v.to_be_bytes()).collect();
    let g = dir.path().join("b.nii");
    std::fs::write(&g, nifti(&[3, 2, 2, 2], 4, 16, 0.0, &be, true)).unwrap();
    let mut ds = NiftiReader.open(&g).unwrap();
    let p = ds.read_plane(0, PlaneIndex { c: 0, z: 0, t: 1 }).unwrap();
    assert_eq!(i16::from_le_bytes([p.data[0], p.data[1]]), 12);
}

#[test]
fn dicom_exports_to_nifti_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    for (name, z, inst, base) in [("a.dcm", 0.0, 1, 1000), ("b.dcm", 2.5, 2, 1100)] {
        std::fs::write(dir.path().join(name), ct_slice(z, inst, base)).unwrap();
    }
    let mut ds = DicomReader.open(dir.path()).unwrap();
    let out = dir.path().join("out").join("ct.nii.gz");
    let r = export_nifti(ds.as_mut(), dir.path(), &out, &NiftiExportOptions::default()).unwrap();
    assert!(r.verified);
    assert_eq!(r.affine_from, "dicom");
    let mut back = NiftiReader.open(&out).unwrap();
    let im = back.info().unwrap().images.remove(0);
    assert_eq!((im.size_x, im.size_y, im.size_z), (4, 3, 2));
    assert_eq!(im.extra["orientation"]["columns_toward"], "L");
    assert_eq!(im.extra["orientation"]["rows_toward"], "P");
    let p = back.read_plane(0, PlaneIndex { c: 0, z: 1, t: 0 }).unwrap();
    assert_eq!(i16::from_le_bytes([p.data[0], p.data[1]]), 1100 - 1024);
    // the input is refused as output
    assert!(export_nifti(ds.as_mut(), &out, &out, &NiftiExportOptions::default()).is_err());
}

#[test]
fn analyze_pairs_are_read_through_either_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = nifti(&[2, 2, 1], 2, 8, 0.0, &[], false);
    h.truncate(348);
    h[344..348].copy_from_slice(&[0; 4]);
    // vox_offset counts from the start of the .img file
    h[108..112].copy_from_slice(&0f32.to_le_bytes());
    std::fs::write(dir.path().join("x.hdr"), &h).unwrap();
    std::fs::write(dir.path().join("x.img"), [1u8, 2, 3, 4]).unwrap();
    for name in ["x.hdr", "x.img"] {
        let p = dir.path().join(name);
        let head = std::fs::read(&p).unwrap();
        assert!(NiftiReader.sniff(&head, &p).is_some(), "{name}");
        let mut ds = NiftiReader.open(&p).unwrap();
        assert_eq!(ds.info().unwrap().format_version.as_deref(), Some("Analyze 7.5"));
        assert_eq!(ds.read_plane(0, PlaneIndex::default()).unwrap().data, vec![1, 2, 3, 4]);
    }
}

// ----------------------------------------------------------------------------------- ParaVision

#[test]
fn paravision_frame_groups_become_z_and_channels() {
    let dir = tempfile::tempdir().unwrap();
    let e = dir.path().join("7");
    std::fs::create_dir_all(e.join("pdata/1")).unwrap();
    std::fs::write(e.join("acqp"), "##TITLE=Parameter List\n##$ACQ_method=<Bruker:FLASH>\n##END=\n").unwrap();
    std::fs::write(e.join("method"), "##TITLE=Parameter List\n##$Method=<Bruker:FLASH>\n##END=\n").unwrap();
    let visu = "##TITLE=Parameter List, ParaVision 7.0.0\n##$VisuCoreFrameCount=6\n##$VisuCoreDim=2\n##$VisuCoreSize=( 2 )\n2 3\n\
##$VisuCoreExtent=( 2 )\n4 3\n##$VisuCoreUnits=( 2, 65 )\n<mm> <mm>\n##$VisuCoreWordType=_16BIT_SGN_INT\n##$VisuCoreByteOrder=littleEndian\n\
##$VisuCoreDataSlope=( 6 )\n@6*(2)\n##$VisuCoreDataOffs=( 6 )\n@6*(0)\n##$VisuFGOrderDesc=( 2 )\n(2, <FG_ECHO>, <>, 0, 2) (3, <FG_SLICE>, <>, 2, 2)\n\
##$VisuFGElemComment=( 2, 65 )\n<first> <second>\n##$VisuCoreSlicePacksSliceDist=( 1 )\n1.5\n##$VisuSubjectName=( 65 )\n<Mouse Seven>\n##END=\n";
    std::fs::write(e.join("pdata/1/visu_pars"), visu).unwrap();
    // 6 frames of 2 x 3: frame f holds the value f everywhere
    let data: Vec<u8> = (0..6i16).flat_map(|f| std::iter::repeat_n(f, 6)).flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(e.join("pdata/1/2dseq"), data).unwrap();
    assert!(ParavisionReader.sniff(&[], &e).is_some());
    let mut ds = ParavisionReader.open(&e).unwrap();
    let info = ds.info().unwrap();
    let im = &info.images[0];
    assert_eq!((im.size_x, im.size_y, im.size_z, im.size_c), (2, 3, 3, 2));
    assert_eq!(im.channels[1].name.as_deref(), Some("second"));
    assert_eq!(im.physical_size.x, Some(2000.0));
    assert_eq!(im.physical_size.z, Some(1500.0));
    // FG_ECHO varies fastest: (z=2, c=1) is frame 1 + 2*2 = 5, scaled by 2
    let p = ds.read_plane(0, PlaneIndex { c: 1, z: 2, t: 0 }).unwrap();
    assert_eq!(f32::from_le_bytes([p.data[0], p.data[1], p.data[2], p.data[3]]), 10.0);
    let s = serde_json::to_string(&info).unwrap() + &ds.vendor_metadata().unwrap().to_string();
    assert!(!s.contains("Mouse Seven"));
    assert!(ds.check().unwrap().ok);
}

#[test]
fn readers_refuse_what_is_not_theirs() {
    let p = Path::new("x.dcm");
    assert!(DicomReader.sniff(b"PK\x03\x04 not dicom", p).is_some_and(|d| d.confidence == openreadout_core::model::DetectConfidence::ExtensionOnly));
    assert!(NiftiReader.sniff(&[0u8; 400], Path::new("x.nii")).is_none());
}
