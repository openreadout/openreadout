//! Synthetic diffraction files: layouts read back exactly, malformed files end in clean errors.

use std::path::Path;

use openreadout_core::{Error, FormatReader};
use openreadout_xrd::{BrmlReader, BrukerRawReader, RasReader, RasxReader, XrdmlReader};

fn write(dir: &Path, name: &str, data: &[u8]) -> std::path::PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, data).unwrap();
    p
}

const XRDML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xrdMeasurements xmlns="http://www.xrdml.com/XRDMeasurement/2.1" status="Completed">
 <sample type="To be analyzed"><id>S1</id><name>quartz</name></sample>
 <xrdMeasurement measurementType="Scan" status="Completed" sampleMode="Reflection">
  <usedWavelength intended="K-Alpha 1"><kAlpha1 unit="Angstrom">1.5405980</kAlpha1><kAlpha2 unit="Angstrom">1.5444260</kAlpha2><ratioKAlpha2KAlpha1>0.5</ratioKAlpha2KAlpha1></usedWavelength>
  <incidentBeamPath><xRayTube id="1" name="t"><tension unit="kV">45</tension><current unit="mA">40</current><anodeMaterial>Cu</anodeMaterial></xRayTube></incidentBeamPath>
  <scan appendNumber="0" mode="Continuous" scanAxis="Gonio" status="Completed">
   <header><startTimeStamp>2024-09-19T15:10:10+02:00</startTimeStamp><author><name>XRD2</name></author></header>
   <dataPoints>
    <positions axis="2Theta" unit="deg"><startPosition>10</startPosition><endPosition>11</endPosition></positions>
    <positions axis="Omega" unit="deg"><startPosition>5</startPosition><endPosition>5.5</endPosition></positions>
    <commonCountingTime unit="seconds">2.0</commonCountingTime>
    ATTEN
    <counts unit="counts">10 20 30</counts>
   </dataPoints>
  </scan>
 </xrdMeasurement>
</xrdMeasurements>"#;

#[test]
fn xrdml_counts_and_attenuation() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "a.xrdml", XRDML.replace("ATTEN", "").as_bytes());
    let mut ds = XrdmlReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].extra["axis"]["first"], 10.0);
    assert_eq!(info.traces[0].extra["axis"]["step"], 0.5);
    assert_eq!(
        ds.read_trace(0, 0, 0, 9).unwrap().channels[0],
        vec![10.0, 20.0, 30.0]
    );
    let exp = ds.experiment().unwrap();
    assert_eq!(exp.method.unwrap().parameters["anode"].value, "Cu");
    // raw counts with attenuation factors are multiplied (schema 2.x); raw counts kept
    let p = write(
        dir.path(),
        "b.xrdml",
        XRDML
            .replace(
                "ATTEN",
                "<beamAttenuationFactors>1 2 4</beamAttenuationFactors>",
            )
            .as_bytes(),
    );
    let mut ds = XrdmlReader.open(&p).unwrap();
    let t = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(t.channels[0], vec![10.0, 40.0, 120.0]);
    assert_eq!(t.channels[1], vec![10.0, 20.0, 30.0]);
    // divergence corrections on raw counts: refused
    let p = write(
        dir.path(),
        "c.xrdml",
        XRDML
            .replace(
                "ATTEN",
                "<commonDivergenceCorrection>1.1</commonDivergenceCorrection>",
            )
            .as_bytes(),
    );
    assert!(matches!(
        XrdmlReader.open(&p),
        Err(Error::Unsupported { .. })
    ));
    // truncated XML and a count mismatch
    let p = write(dir.path(), "d.xrdml", &XRDML.as_bytes()[..400]);
    assert!(XrdmlReader.open(&p).is_err());
    let p = write(
        dir.path(),
        "e.xrdml",
        XRDML
            .replace(
                "ATTEN",
                "<beamAttenuationFactors>1 2</beamAttenuationFactors>",
            )
            .as_bytes(),
    );
    assert!(matches!(XrdmlReader.open(&p), Err(Error::Corrupt { .. })));
}

/// A minimal RAW4.00 file: header, one USER text record, one range with 3 float32 records.
fn raw4() -> Vec<u8> {
    let mut b = vec![0u8; 61];
    b[..8].copy_from_slice(b"RAW4.00\0");
    b[12..22].copy_from_slice(b"06/30/2023");
    b[24..32].copy_from_slice(b"12:41:34");
    // text record
    let mut rec = vec![0u8; 36];
    rec[..4].copy_from_slice(&10u32.to_le_bytes());
    let v = b"Lab\n";
    rec[4..8].copy_from_slice(&((36 + v.len()) as u32).to_le_bytes());
    rec[12..16].copy_from_slice(b"USER");
    rec.extend_from_slice(v);
    b.extend_from_slice(&rec);
    // range header (kind 0)
    let mut h = vec![0u8; 160];
    h[32..46].copy_from_slice(b"Locked Coupled");
    h[72..80].copy_from_slice(&5.0f64.to_le_bytes());
    h[80..88].copy_from_slice(&0.5f64.to_le_bytes());
    h[88..92].copy_from_slice(&3u32.to_le_bytes());
    h[100..104].copy_from_slice(&40f32.to_le_bytes());
    h[104..108].copy_from_slice(&30f32.to_le_bytes());
    h[136..140].copy_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&h);
    for v in [100f32, 200., 150.] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

#[test]
fn bruker_raw4_and_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "a.raw", &raw4());
    let mut ds = BrukerRawReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].extra["axis"]["first"], 5.0);
    assert_eq!(
        ds.read_trace(0, 0, 0, 9).unwrap().channels[0],
        vec![100.0, 200.0, 150.0]
    );
    let exp = ds.experiment().unwrap();
    assert_eq!(exp.acquisition.unwrap().operator.as_deref(), Some("Lab"));
    // every truncation is an error, never a panic
    let full = raw4();
    for n in [8, 40, 61, 70, 130, 250, full.len() - 1] {
        let p = write(dir.path(), "t.raw", &full[..n]);
        assert!(BrukerRawReader.open(&p).is_err(), "cut at {n}");
    }
    // version 2: refused
    let p = write(dir.path(), "v2.raw", b"RAW2\0\0\0\0\0\0\0\0");
    assert!(matches!(
        BrukerRawReader.open(&p),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn rigaku_ras_rasx_and_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let ras = "*RAS_DATA_START\n*RAS_HEADER_START\n*MEAS_SCAN_AXIS_X \"TwoThetaTheta\"\n*MEAS_SCAN_UNIT_Y \"counts\"\n*MEAS_DATA_COUNT \"3\"\n*HW_XG_TARGET_NAME \"Cu\"\n*MEAS_SCAN_START_TIME \"07/16/25 08:43:20\"\n*RAS_HEADER_END\n*RAS_INT_START\n10.00 5 1\n10.01 6 1\n10.02 7 2\n*RAS_INT_END\n*RAS_DATA_END\n";
    let p = write(dir.path(), "a.ras", ras.as_bytes());
    let mut ds = RasReader.open(&p).unwrap();
    let t = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(t.channels[0], vec![5.0, 6.0, 7.0]);
    assert!(
        ds.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "attenuation")
    );
    let exp = ds.experiment().unwrap();
    assert_eq!(
        exp.acquisition.unwrap().started_at.as_deref(),
        Some("2025-07-16T08:43:20")
    );
    let p = write(
        dir.path(),
        "b.ras",
        &ras.as_bytes()[..ras.find("10.02").unwrap()],
    );
    assert!(matches!(RasReader.open(&p), Err(Error::Corrupt { .. })));
    // RASX: a zip with a profile and its conditions
    let cond = "<MeasurementConditions><ScanInformation><AxisName>TwoThetaTheta</AxisName><IntensityUnit>cps</IntensityUnit></ScanInformation></MeasurementConditions>";
    let archive = openreadout_core::zip::zip_bytes(&[
        (
            "Data0/Profile0.txt",
            "\u{feff}10.0\t1.5\t1\n10.1\t2.5\t1\n".as_bytes(),
        ),
        ("Data0/MesurementConditions0.xml", cond.as_bytes()),
    ])
    .unwrap();
    let p = write(dir.path(), "c.rasx", &archive);
    let mut ds = RasxReader.open(&p).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].channels[0].unit.as_deref(), Some("cps"));
    assert_eq!(
        ds.read_trace(0, 0, 0, 9).unwrap().channels[0],
        vec![1.5, 2.5]
    );
    let p = write(dir.path(), "d.rasx", &archive[..archive.len() / 2]);
    assert!(RasxReader.open(&p).is_err());
}

#[test]
fn brml_minimal_and_malformed() {
    let dir = tempfile::tempdir().unwrap();
    let raw = r#"<?xml version="1.0"?><RawData xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><TimeStampStarted>2024-03-12T15:02:00+02:00</TimeStampStarted><DataRoutes><DataRoute RouteFlag="Measured"><ScanInformation VisibleName="Coupled TwoTheta/Theta"><TimePerStep>0.5</TimePerStep><ScanAxes><ScanAxisInfo AxisId="TwoTheta" UIVisibility="Primary" Unit="°"><Start>3</Start><Increment>0.02</Increment></ScanAxisInfo></ScanAxes></ScanInformation><Datum>96,1,3.00,1.50,1082</Datum><Datum>96,1,3.02,1.51,1052</Datum><DataViews><RawDataView xsi:type="FixedRawDataView" Start="0" Length="1" LogicName="MeasuredTime"/><RawDataView xsi:type="FixedRawDataView" Start="1" Length="1" LogicName="AbsorptionFactor"/><RawDataView xsi:type="VaryingRawDataView" Start="2" Length="2"><Varying><FieldDefinitions FieldName="TwoTheta" AxisId="TwoTheta"/><FieldDefinitions FieldName="Theta" AxisId="Theta"/></Varying></RawDataView><RawDataView xsi:type="RecordedRawDataView" Start="4" Length="1"><Recording VisibleName="det"/></RawDataView></DataViews></DataRoute></DataRoutes></RawData>"#;
    let archive = openreadout_core::zip::zip_bytes(&[
        ("experimentCollection.xml", b"<x/>".as_slice()),
        ("Experiment0/RawData0.xml", raw.as_bytes()),
    ])
    .unwrap();
    let p = write(dir.path(), "a.brml", &archive);
    let mut ds = BrmlReader.open(&p).unwrap();
    let t = ds.read_trace(0, 0, 0, 9).unwrap();
    assert_eq!(t.channels[0], vec![3.0, 3.02]);
    assert_eq!(t.channels[1], vec![1082.0, 1052.0]);
    // a Datum row too short for the views: corrupt
    let bad = raw.replace("<Datum>96,1,3.02,1.51,1052</Datum>", "<Datum>96,1</Datum>");
    let archive = openreadout_core::zip::zip_bytes(&[
        ("experimentCollection.xml", b"<x/>".as_slice()),
        ("Experiment0/RawData0.xml", bad.as_bytes()),
    ])
    .unwrap();
    let p = write(dir.path(), "b.brml", &archive);
    assert!(matches!(BrmlReader.open(&p), Err(Error::Corrupt { .. })));
}
