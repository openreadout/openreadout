//! A tiny synthetic MassHunter directory built byte by byte from the layouts in
//! `docs/formats/agilent-masshunter.md`: two Q-TOF scans (centroid flight times calibrated
//! through `DefaultMassCal.xml`, one MS/MS scan) and one pump signal.

use std::path::{Path, PathBuf};

use openreadout_agilent_ms::{AgilentMsReader, FORMAT_ID};
use openreadout_core::{FormatReader, Registry, SpectrumView};

const XSD: &str = r#"<?xml version="1.0"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
<xs:complexType name="ScanRecordType"><xs:sequence>
<xs:element name="ScanID" type="xs:int"/><xs:element name="ScanTime" type="xs:double"/>
<xs:element name="MSLevel" type="xs:int"/><xs:element name="TIC" type="xs:double"/>
<xs:element name="CalibrationID" type="xs:int"/><xs:element name="IonPolarity" type="xs:int"/>
<xs:element name="CollisionEnergy" type="xs:double"/><xs:element name="MzOfInterest" type="xs:double"/>
<!-- <xs:element name="Commented" type="xs:double"/> -->
<xs:element name="SpectrumParamValues" type="SpectrumParamsType" maxOccurs="unbounded"/>
</xs:sequence></xs:complexType>
<xs:complexType name="SpectrumParamsType"><xs:sequence>
<xs:element name="SpectrumFormatID" type="xs:int"/><xs:element name="SpectrumOffset" type="xs:long"/>
<xs:element name="ByteCount" type="xs:int"/><xs:element name="PointCount" type="xs:int"/>
<xs:element name="MinX" type="xs:double"/><xs:element name="MaxX" type="xs:double"/>
</xs:sequence></xs:complexType>
</xs:schema>"#;

const CAL: &str = r#"<DefaultMassCalibration><DefaultCalibrations>
<DefaultCalibration DefaultCalibrationID="1">
<Step Number="1"><CalibrationFormula>Traditional</CalibrationFormula><ValueUseFlags>0</ValueUseFlags>
<Values><Value Number="1">0.0005</Value><Value Number="2">1000</Value></Values></Step>
<Step Number="2"><CalibrationFormula>Polynomial</CalibrationFormula><ValueUseFlags>2</ValueUseFlags>
<Values><Value Number="1">20000</Value><Value Number="2">40000</Value><Value Number="3">1e-6</Value></Values></Step>
</DefaultCalibration></DefaultCalibrations></DefaultMassCalibration>"#;

fn header(tag: u16) -> Vec<u8> {
    let mut b = vec![0u8; 0x44];
    b[..2].copy_from_slice(&tag.to_le_bytes());
    b
}

fn mz(t: f64) -> f64 {
    let r = 0.0005 * (t - 1000.0);
    r * r - 1e-6 * t.clamp(20000.0, 40000.0)
}

fn build(root: &Path) -> PathBuf {
    let d = root.join("synthetic.d");
    let acq = d.join("AcqData");
    std::fs::create_dir_all(&acq).unwrap();
    // peaks: n f64 flight times, then n f32 abundances
    let mut peak = header(0x0103);
    let scans: [(&[f64], &[f32]); 2] = [
        (&[21000.0, 30000.0, 41000.0], &[10.0, 20.0, 5.0]),
        (&[25000.0], &[7.0]),
    ];
    let mut blocks = Vec::new();
    for (x, y) in scans {
        let off = peak.len() as i64;
        for v in x {
            peak.extend_from_slice(&v.to_le_bytes());
        }
        for v in y {
            peak.extend_from_slice(&v.to_le_bytes());
        }
        blocks.push((
            off,
            (x.len() * 12) as i32,
            x.len() as i32,
            mz(x[0]),
            mz(*x.last().unwrap()),
        ));
    }
    std::fs::write(acq.join("MSPeak.bin"), &peak).unwrap();
    let mut scan = header(0x0101);
    scan.resize(0x5C, 0);
    for (at, v) in [(0x44usize, 1i32), (0x48, 6), (0x4C, 1), (0x58, 0x5C)] {
        scan[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    for (k, (off, bytes, n, lo, hi)) in blocks.iter().enumerate() {
        scan.extend_from_slice(&(100 + k as i32).to_le_bytes()); // ScanID
        scan.extend_from_slice(&(0.5 * (k + 1) as f64).to_le_bytes()); // ScanTime (min)
        scan.extend_from_slice(&(1 + k as i32).to_le_bytes()); // MSLevel
        scan.extend_from_slice(&35.0f64.to_le_bytes()); // TIC
        scan.extend_from_slice(&1i32.to_le_bytes()); // CalibrationID
        scan.extend_from_slice(&0i32.to_le_bytes()); // IonPolarity: positive
        scan.extend_from_slice(&(20.0 * k as f64).to_le_bytes()); // CollisionEnergy
        scan.extend_from_slice(&(if k == 1 { 150.5f64 } else { 0.0 }).to_le_bytes());
        scan.extend_from_slice(&2i32.to_le_bytes()); // centroid block
        scan.extend_from_slice(&off.to_le_bytes());
        scan.extend_from_slice(&bytes.to_le_bytes());
        scan.extend_from_slice(&n.to_le_bytes());
        scan.extend_from_slice(&lo.to_le_bytes());
        scan.extend_from_slice(&hi.to_le_bytes());
    }
    std::fs::write(acq.join("MSScan.bin"), &scan).unwrap();
    std::fs::write(acq.join("MSScan.xsd"), XSD).unwrap();
    std::fs::write(acq.join("DefaultMassCal.xml"), CAL).unwrap();
    std::fs::write(
        acq.join("Devices.xml"),
        "<Devices><Device DeviceID=\"1\"><Name>QTOF</Name><ModelNumber>G0000X</ModelNumber><SerialNumber>SN1</SerialNumber></Device><Device DeviceID=\"3\"><Name>BinPump</Name></Device></Devices>",
    )
    .unwrap();
    std::fs::write(acq.join("Contents.xml"), "<Contents><AcquiredTime>2020-01-02T03:04:05Z</AcquiredTime><AcqSoftwareVersion>test 1.0</AcqSoftwareVersion></Contents>").unwrap();
    // one pump signal: descriptor + data block (start 0 min, step 0.01 min, 3 values)
    let mut cd = header(0x0200);
    for v in [1i32, 3, 1] {
        cd.extend_from_slice(&v.to_le_bytes());
    }
    cd.extend_from_slice(&[1, b'A', 9]);
    cd.extend_from_slice(b" Pressure");
    cd.extend_from_slice(&2i32.to_le_bytes());
    cd.extend_from_slice(&0x44i64.to_le_bytes());
    cd.extend_from_slice(&3i32.to_le_bytes());
    cd.extend_from_slice(&[0u8; 40]);
    cd.extend_from_slice(&[3, b'b', b'a', b'r']);
    cd.extend_from_slice(&1.0f64.to_le_bytes());
    cd.extend_from_slice(&[0u8; 8]);
    std::fs::write(acq.join("BinPump1.cd"), &cd).unwrap();
    let mut cg = header(0x0201);
    for v in [0.0f64, 0.01, 100.0, 101.0, 102.0] {
        cg.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(acq.join("BinPump1.cg"), &cg).unwrap();
    d
}

fn scratch(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("openreadout-agilent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn reads_scans_spectra_and_signals() {
    let root = scratch("read");
    let d = build(&root);
    let reg = Registry::new().with(Box::new(AgilentMsReader));
    let (det, mut ds) = reg.open(&d).unwrap();
    assert_eq!(det.format_id, FORMAT_ID);
    let info = ds.info().unwrap();
    let run = &info.spectra[0];
    assert_eq!(run.scan_count, 2);
    assert_eq!(run.ms_levels, [1, 2]);
    assert_eq!(
        run.instrument.as_ref().unwrap().model.as_deref(),
        Some("G0000X")
    );
    assert_eq!(run.extra["instrument_serial"], "SN1");
    let sp = ds.read_spectrum_view(0, 0, SpectrumView::Centroid).unwrap();
    assert_eq!(sp.scan_number, 100);
    assert_eq!(sp.polarity, "positive");
    assert!(sp.centroided);
    assert_eq!(sp.mz.len(), 3);
    for (got, t) in sp.mz.iter().zip([21000.0, 30000.0, 41000.0]) {
        assert!((got - mz(t)).abs() < 1e-12, "{got} vs {}", mz(t));
    }
    let ms2 = ds.read_spectrum(0, 1).unwrap();
    assert_eq!(ms2.ms_level, 2);
    assert_eq!(ms2.precursor_mz, Some(150.5));
    assert_eq!(ms2.collision_energy, Some(20.0));
    assert!((ms2.rt_s.unwrap() - 60.0).abs() < 1e-9);
    // TIC, BPC, then the pump signal
    assert_eq!(info.traces.len(), 3);
    let tr = ds.read_trace(2, 0, 0, 10).unwrap();
    assert_eq!(tr.channels[1], [100.0, 101.0, 102.0]);
    assert!((tr.channels[0][2] - 1.2).abs() < 1e-9);
    assert!(ds.check().unwrap().ok);
    // a file inside AcqData opens the same directory
    assert!(
        AgilentMsReader
            .sniff(&[], &d.join("AcqData").join("MSScan.bin"))
            .is_some()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn truncated_index_and_data_are_reported() {
    let root = scratch("trunc");
    let d = build(&root);
    let scan = d.join("AcqData").join("MSScan.bin");
    let len = std::fs::metadata(&scan).unwrap().len();
    let f = std::fs::OpenOptions::new().write(true).open(&scan).unwrap();
    f.set_len(len - 10).unwrap();
    let peak = d.join("AcqData").join("MSPeak.bin");
    let f = std::fs::OpenOptions::new().write(true).open(&peak).unwrap();
    f.set_len(0x44 + 20).unwrap();
    let mut ds = AgilentMsReader.open(&d).unwrap();
    assert_eq!(ds.info().unwrap().spectra[0].scan_count, 1);
    let rep = ds.check().unwrap();
    assert!(!rep.ok);
    assert!(rep.findings.iter().any(|f| f.code == "truncated"));
    assert!(ds.read_spectrum(0, 0).is_err());
    let _ = std::fs::remove_dir_all(root);
}
