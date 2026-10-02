//! Synthetic UNICORN files built from the layouts in `docs/formats/cytiva-unicorn.md`: a `.res`
//! with two curves, a logbook, fractions and an injection; a UNICORN 7 result export with two
//! curves (one padded nested zip), events and a peak table. Values, volumes, times, tables and
//! experiment facts are checked; every truncation and a few corruptions must fail cleanly.

use openreadout_core::source::Input;
use openreadout_core::{Dataset, FormatReader};
use openreadout_fplc::{UnicornResReader, UnicornZipReader};

// ---------------------------------------------------------------------------------- .res

fn desc(flags: u16, name: &str, unit: &str, storage: u16, factor: f64, second: f64) -> Vec<u8> {
    let mut d = Vec::with_capacity(78);
    d.extend_from_slice(&78u16.to_le_bytes());
    d.extend_from_slice(&flags.to_le_bytes());
    let mut n = name.as_bytes().to_vec();
    n.resize(40, 0);
    d.extend(n);
    let mut u = unit.as_bytes().to_vec();
    u.resize(16, 0);
    d.extend(u);
    d.extend_from_slice(&storage.to_le_bytes());
    d.extend_from_slice(&factor.to_le_bytes());
    d.extend_from_slice(&second.to_le_bytes());
    d
}

fn curve_block(unit: &str, pairs: &[(i32, i32)], value_factor: f64) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [6u16, 2, 1] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend(desc(0x8001, "Acc. Time", "min", 0x0104, 0.5, 0.0));
    b.extend(desc(0x8002, "Acc. Volume", "ml", 0x0104, 0.01, 0.0));
    b.extend(desc(0x4001, "0", unit, 0x0104, value_factor, 0.0));
    assert_eq!(b.len(), 240);
    for (v, y) in pairs {
        b.extend_from_slice(&v.to_le_bytes());
        b.extend_from_slice(&y.to_le_bytes());
    }
    b
}

fn event_block(label: &str, events: &[(f64, f64, &str)]) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [6u16, 6, 1] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.extend(desc(0x8001, "", "", 0x0104, 1.0, 0.0));
    b.extend(desc(0x8001, "Time", "min", 0x0308, 1.0, 0.0));
    b.extend(desc(0x8002, "Volume", "ml", 0x0308, 1.0, 0.0));
    b.extend(desc(0x4000, label, "", 0x044c, 1.0, 0.0));
    b.extend(desc(0x4000, "Add. Text", "", 0x044c, 1.0, 0.0));
    b.extend(desc(0x4000, "Value", "", 0x0308, 1.0, 0.0));
    b.extend(desc(0x4000, "Flags", "", 0x0104, 1.0, 0.0));
    assert_eq!(b.len(), 552);
    for (t, v, text) in events {
        b.extend_from_slice(&t.to_le_bytes());
        b.extend_from_slice(&v.to_le_bytes());
        let mut s = text.as_bytes().to_vec();
        s.resize(76, 0);
        b.extend(s);
        b.extend(vec![0u8; 76]);
        b.extend_from_slice(&1.0f64.to_le_bytes());
        b.extend_from_slice(&2i32.to_le_bytes());
    }
    b
}

/// A `.res` with blocks (name, type word, header bytes, data).
fn res_file(blocks: &[(&str, [u8; 6], u32, Vec<u8>)]) -> Vec<u8> {
    let dir = 0x2b0usize;
    let data_start = dir + 344 * (blocks.len() + 1);
    let mut f = vec![0u8; data_start];
    f[..4].copy_from_slice(&[0x11, 0x47, 0x11, 0x47]);
    f[4..8].copy_from_slice(&0x18u32.to_le_bytes());
    f[8..12].copy_from_slice(&(dir as u32).to_le_bytes());
    f[0x18..0x24].copy_from_slice(b"UNICORN 3.10");
    f[0x68..0x6c].copy_from_slice(&1_700_000_000u32.to_le_bytes());
    f[0x6c..0x70].copy_from_slice(&1_700_000_600u32.to_le_bytes());
    f[0x76..0x7c].copy_from_slice(b"tester");
    for (i, (name, kind, hdr, data)) in blocks.iter().enumerate() {
        let at = f.len();
        f.extend_from_slice(data);
        let e = dir + 344 * i;
        f[e..e + 6].copy_from_slice(kind);
        f[e + 6..e + 6 + name.len()].copy_from_slice(name.as_bytes());
        f[e + 302..e + 306].copy_from_slice(&(data.len() as u32).to_le_bytes());
        f[e + 306..e + 310].copy_from_slice(&(data.len() as u32).to_le_bytes());
        f[e + 310..e + 314].copy_from_slice(&(at as u32).to_le_bytes());
        f[e + 314..e + 318].copy_from_slice(&hdr.to_le_bytes());
    }
    let len = f.len() as u32;
    f[16..20].copy_from_slice(&len.to_le_bytes());
    f
}

fn sample_res() -> Vec<u8> {
    // 5 samples at 0.5 min, 1 ml/min: volumes 0, 0.5, 1, 1.5, 2 ml
    let uv: Vec<(i32, i32)> = vec![(0, 100), (50, 2500), (100, 15328), (150, 2500), (200, 100)];
    let cond: Vec<(i32, i32)> = (0..5).map(|i| (i * 50, 15_000 + i)).collect();
    res_file(&[
        (
            "Methods",
            [1, 0, 2, 0, 1, 2],
            0,
            b"METHOD\n0.00 Base Volume, 24.0 {ml}, Superdex 200 10/300\n".to_vec(),
        ),
        (
            "Techniques",
            [1, 0, 2, 0, 0x54, 3],
            0,
            b"START_TECHNIQUES\nSize_Exclusion\nEND_TECHNIQUES\n".to_vec(),
        ),
        (
            "run1:1_Logbook",
            [1, 0, 4, 0, 0x48, 4],
            552,
            event_block(
                "Inject",
                &[
                    (
                        0.0,
                        0.0,
                        "Method Run 14.11.2023, 23:03:20, Method : SEC test",
                    ),
                    (0.5, 0.5, "Injection Valve Inj"),
                ],
            ),
        ),
        (
            "run1:1_UV1_280nm",
            [1, 0, 4, 0, 1, 0x14],
            240,
            curve_block(" mAU", &uv, 0.001),
        ),
        (
            "run1:1_Cond",
            [1, 0, 4, 0, 1, 0x14],
            240,
            curve_block(" mS/cm", &cond, 0.001),
        ),
        (
            "run1:1_Fractions",
            [1, 0, 4, 0, 0x44, 4],
            552,
            event_block(
                "TubeNo",
                &[(1.0, 1.0, "A1"), (1.5, 1.5, "A2"), (2.0, 2.0, "Waste")],
            ),
        ),
        (
            "run1:1_Inject",
            [1, 0, 4, 0, 0x46, 4],
            552,
            event_block("Inject", &[(0.5, 0.5, "1")]),
        ),
    ])
}

fn open_res(bytes: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    UnicornResReader.open_input(&Input::from_bytes("run.res", bytes))
}

#[test]
fn res_curves_events_and_facts() {
    let mut ds = open_res(sample_res()).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format.id, "cytiva-unicorn-res");
    assert_eq!(info.format_version.as_deref(), Some("3.10"));
    assert_eq!(info.traces.len(), 2);
    let uv = &info.traces[0];
    assert_eq!(uv.name.as_deref(), Some("UV1_280nm"));
    assert_eq!(uv.sample_count, 5);
    assert!((uv.sample_rate_hz - 1.0 / 30.0).abs() < 1e-12);
    assert_eq!(uv.extra["kind"], "uv");
    assert_eq!(uv.extra["wavelength_nm"], 280.0);
    assert_eq!(uv.extra["injection_volume_ml"], 0.5);
    assert_eq!(uv.channels[0].unit.as_deref(), Some("mAU"));
    assert_eq!(uv.channels[1].name, "volume");
    let t = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(t.channels[0], vec![0.1, 2.5, 15.328, 2.5, 0.1]);
    assert_eq!(t.channels[1], vec![0.0, 0.5, 1.0, 1.5, 2.0]);
    let window = ds.read_trace(0, 0, 3, 10).unwrap();
    assert_eq!(window.first_sample, 3);
    assert_eq!(window.channels[0], vec![2.5, 0.1]);
    assert!(ds.read_trace(0, 1, 0, 1).is_err());
    assert!(ds.read_trace(9, 0, 0, 1).is_err());
    assert_eq!(info.traces[1].extra["kind"], "conductivity");
    // tables: logbook, fractions, injections
    let names: Vec<_> = info
        .tables
        .iter()
        .map(|t| t.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["logbook", "fractions", "injections"]);
    let fr = ds.read_table(1, 0, 10).unwrap();
    assert_eq!(fr.columns[1], vec![1.0, 1.5, 2.0]);
    assert_eq!(
        info.tables[1].columns[2].extra["categories"],
        serde_json::json!(["A1", "A2", "Waste"])
    );
    // facts
    let e = ds.experiment().unwrap();
    assert_eq!(e.method.as_ref().unwrap().name.as_deref(), Some("SEC test"));
    assert_eq!(
        e.method.as_ref().unwrap().technique.as_ref().unwrap().id,
        "CHMO:0001013"
    );
    assert_eq!(
        e.method.as_ref().unwrap().parameters["column"].value,
        "Superdex 200 10/300"
    );
    let a = e.acquisition.unwrap();
    assert_eq!(a.operator.as_deref(), Some("tester"));
    assert_eq!(a.ended_at.as_deref(), Some("2023-11-14T22:23:20.000Z"));
    assert_eq!(a.duration_s, Some(120.0));
    let check = ds.check().unwrap();
    assert!(check.ok, "{check:?}");
}

#[test]
fn res_damage_is_clean() {
    let good = sample_res();
    for cut in [
        0,
        3,
        100,
        0x2b0,
        0x2b0 + 200,
        good.len() / 2,
        good.len() - 5,
    ] {
        let r = open_res(good[..cut].to_vec());
        if let Ok(mut ds) = r {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, 10);
            let _ = ds.read_table(0, 0, 10);
        }
    }
    // a block pointing past the end is a finding, a curve read an error
    let mut bad = good.clone();
    let e = 0x2b0 + 344 * 3; // the UV curve's entry
    bad[e + 310..e + 314].copy_from_slice(&0x7fff_fff0u32.to_le_bytes());
    let mut ds = open_res(bad).unwrap();
    let r = ds.check().unwrap();
    assert!(!r.ok);
    // garbage descriptors: the curve is left out with a warning
    let mut bad = good;
    let at = u32::from_le_bytes(
        bad[0x2b0 + 344 * 3 + 310..0x2b0 + 344 * 3 + 314]
            .try_into()
            .unwrap(),
    ) as usize;
    bad[at + 6] = 0x99;
    let mut ds = open_res(bad).unwrap();
    assert_eq!(ds.info().unwrap().traces.len(), 1);
    assert!(
        ds.check()
            .unwrap()
            .findings
            .iter()
            .any(|f| f.code == "unreadable_curve_header")
    );
}

// ---------------------------------------------------------------------------------- .zip

fn nrbf_floats(v: &[f32]) -> Vec<u8> {
    let mut b = vec![
        0u8, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 1, 0, 0, 0, 0, 0, 0, 0,
    ];
    b.push(15);
    b.extend_from_slice(&1i32.to_le_bytes());
    b.extend_from_slice(&(v.len() as i32).to_le_bytes());
    b.push(11);
    for x in v {
        b.extend_from_slice(&x.to_le_bytes());
    }
    b.push(11);
    b
}

fn nrbf_string(s: &str) -> Vec<u8> {
    let mut b = vec![
        0u8, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 1, 0, 0, 0, 0, 0, 0, 0,
    ];
    b.push(6);
    b.extend_from_slice(&1i32.to_le_bytes());
    let mut n = s.len();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            b.push(byte);
            break;
        }
        b.push(byte | 0x80);
    }
    b.extend_from_slice(s.as_bytes());
    b.push(11);
    b
}

fn curve_zip(vol: &[f32], amp: &[f32], pad: usize) -> Vec<u8> {
    let mut z = openreadout_core::zip::zip_bytes(&[
        ("CoordinateData.AmplitudesDataType", b"System.Single[]\r\n"),
        ("CoordinateData.Amplitudes", &nrbf_floats(amp)),
        ("CoordinateData.VolumesDataType", b"System.Single[]\r\n"),
        ("CoordinateData.Volumes", &nrbf_floats(vol)),
    ])
    .unwrap();
    z.extend(std::iter::repeat_n(0u8, pad));
    z
}

fn curve_xml(name: &str, kind: &str, unit: &str, num: u32, dt: f64, t0: f64) -> String {
    format!(
        "<Curve CurveDataType=\"{kind}\"><Name>{name}</Name><IsoChroneType>Time</IsoChroneType><DistanceBetweenPoints>{dt}</DistanceBetweenPoints><DistanceToStartPoint>{t0}</DistanceToStartPoint><TimeUnit>min</TimeUnit><MethodStartTime>2024-02-14T18:13:47.412</MethodStartTime><MethodStartTimeUtcOffsetMinutes>60</MethodStartTimeUtcOffsetMinutes><AmplitudeUnit>{unit}</AmplitudeUnit><IsOriginalData>true</IsOriginalData><CurveNumber>{num}</CurveNumber><ColumnVolume>24</ColumnVolume><CurvePoints><CurvePoint><IsFullResolution>true</IsFullResolution><BinaryCurvePointsFileName>Chrom.1_{num}_True</BinaryCurvePointsFileName></CurvePoint></CurvePoints></Curve>"
    )
}

fn sample_zip(uv_member: Option<Vec<u8>>) -> Vec<u8> {
    let result = "<Result FormatVersion=\"5\" UNICORNVersion=\"7.3.0.473\"><SystemName>Pure25#1</SystemName><Name>SEC 001</Name><CreatedBy>alice</CreatedBy><ResultSearchCriterias><ResultSearchCriteria><Name>VariableValue</Name><Keyword1>Flow rate</Keyword1><Keyword2>0.500</Keyword2><ExtraDisplayInformation>ml/min</ExtraDisplayInformation></ResultSearchCriteria><ResultSearchCriteria><Name>VariableValue</Name><Keyword1>Sample_ID</Keyword1><Keyword2>lysate-7</Keyword2></ResultSearchCriteria></ResultSearchCriterias></Result>";
    let chrom = format!(
        "<Chromatogram FormatVersion=\"9\" UNICORNVersion=\"7.3.0.473\"><ChromatogramName>Chrom.1</ChromatogramName><TimeUnit>min</TimeUnit><Curves>{}{}</Curves><EventCurves><EventCurve EventCurveType=\"Fraction\"><Events><Event><EventTime>1</EventTime><EventVolume>0.5</EventVolume><EventText>A1</EventText></Event></Events></EventCurve><EventCurve EventCurveType=\"Injection\"><Events><Event><EventTime>0.5</EventTime><EventVolume>0.25</EventVolume><EventText></EventText></Event></Events></EventCurve></EventCurves><PeakTables><PeakTable><DataCurve><CurveNumber>1</CurveNumber></DataCurve><CalculationRetention>Volume</CalculationRetention><Name>UV 1_280@01,PEAK</Name><ZeroAdjustedToInjectionNumber>1</ZeroAdjustedToInjectionNumber><Peaks><Peak><MaxPeakRetention>0.75</MaxPeakRetention><StartPeakRetention>0.5</StartPeakRetention><EndPeakRetention>1.25</EndPeakRetention><Height>9</Height><Area>2.5</Area></Peak></Peaks></PeakTable></PeakTables></Chromatogram>",
        curve_xml("UV 1_280", "UV", "mAU", 1, 0.5, 0.5),
        curve_xml("Cond", "Conduction", "mS/cm", 4, 1.0, 1.0)
    );
    let uv = uv_member.unwrap_or_else(|| {
        curve_zip(
            &[0.0, 0.25, 0.5, 0.75, 1.0, 1.25],
            &[1.0, 2.0, 5.0, 9.0, 4.0, 1.0],
            70_000,
        )
    });
    let cond = curve_zip(&[0.0, 0.5, 1.0], &[10.0, 10.5, 11.0], 0);
    let instrument = openreadout_core::zip::zip_bytes(&[
        ("XmlDataType", b"System.String\r\n"),
        (
            "Xml",
            &nrbf_string("<InstrumentConfigurationDesc><Description>AKTA pure 25</Description><FirmwareName>AKTA pure</FirmwareName><FirmwareVersion>4.17</FirmwareVersion></InstrumentConfigurationDesc>"),
        ),
    ])
    .unwrap();
    openreadout_core::zip::zip_bytes(&[
        ("Result.xml", result.as_bytes()),
        ("Chrom.1.Xml", chrom.as_bytes()),
        ("Chrom.1_1_True", &uv),
        ("Chrom.1_4_True", &cond),
        ("InstrumentConfigurationData", &instrument),
        ("NextFracData", b""),
    ])
    .unwrap()
}

fn open_zip(bytes: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    UnicornZipReader.open_input(&Input::from_bytes("run.zip", bytes))
}

#[test]
fn zip_curves_events_peaks_and_facts() {
    let mut ds = open_zip(sample_zip(None)).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format.id, "cytiva-unicorn-zip");
    assert_eq!(info.traces.len(), 2);
    let uv = &info.traces[0];
    assert_eq!(uv.sample_count, 6);
    assert_eq!(uv.start_s, Some(30.0));
    assert!((uv.sample_rate_hz - 1.0 / 30.0).abs() < 1e-12);
    assert_eq!(uv.extra["wavelength_nm"], 280.0);
    assert_eq!(uv.extra["curve_number"], 1.0);
    assert_eq!(uv.extra["injection_volume_ml"], 0.25);
    let t = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(t.channels[0], vec![1.0, 2.0, 5.0, 9.0, 4.0, 1.0]);
    assert_eq!(t.channels[1], vec![0.0, 0.25, 0.5, 0.75, 1.0, 1.25]);
    assert_eq!(info.traces[1].extra["kind"], "conductivity");
    let names: Vec<_> = info
        .tables
        .iter()
        .map(|t| t.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["fractions", "injections", "UV 1_280@01,PEAK"]);
    let peaks = ds.read_table(2, 0, 10).unwrap();
    assert_eq!(peaks.columns[0], vec![0.75]); // retention
    assert_eq!(peaks.columns[3], vec![9.0]); // height
    assert_eq!(info.tables[2].extra["trace"], 0);
    assert_eq!(info.tables[2].columns[4].unit.as_deref(), Some("mAU·ml"));
    let e = ds.experiment().unwrap();
    assert_eq!(e.sample.unwrap().id.as_deref(), Some("lysate-7"));
    let ins = e.instrument.unwrap();
    assert_eq!(ins.model.as_deref(), Some("AKTA pure 25"));
    assert_eq!(ins.software_version.as_deref(), Some("7.3.0.473"));
    let m = e.method.unwrap();
    assert_eq!(m.parameters["flow_rate"].value, 0.5);
    let a = e.acquisition.unwrap();
    assert_eq!(a.operator.as_deref(), Some("alice"));
    assert_eq!(
        a.started_at.as_deref(),
        Some("2024-02-14T18:13:47.412+01:00")
    );
    assert!(ds.check().unwrap().ok);
}

#[test]
fn zip_damaged_curves_are_refused() {
    // an array that claims more floats than its member holds
    let mut short = nrbf_floats(&[1.0, 2.0, 3.0]);
    short[22..26].copy_from_slice(&1000i32.to_le_bytes());
    let bad = openreadout_core::zip::zip_bytes(&[
        ("CoordinateData.Amplitudes", &short),
        ("CoordinateData.Volumes", &nrbf_floats(&[0.0, 1.0, 2.0])),
    ])
    .unwrap();
    let mut ds = open_zip(sample_zip(Some(bad))).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 1, "the damaged curve is left out");
    assert!(!ds.check().unwrap().ok);
    // bare floats (no serialization header)
    let bare: Vec<u8> = [1.0f32, 2.0].iter().flat_map(|x| x.to_le_bytes()).collect();
    let bad = openreadout_core::zip::zip_bytes(&[
        ("CoordinateData.Amplitudes", &bare),
        ("CoordinateData.Volumes", &bare),
    ])
    .unwrap();
    let ds = open_zip(sample_zip(Some(bad))).unwrap();
    assert_eq!(ds.info().unwrap().traces.len(), 1);
    // truncations of the whole export: clean results only
    let good = sample_zip(None);
    for cut in [
        0,
        10,
        30,
        good.len() / 3,
        good.len() / 2,
        good.len() - 30,
        good.len() - 1,
    ] {
        if let Ok(mut ds) = open_zip(good[..cut].to_vec()) {
            let _ = ds.info();
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, 10);
        }
    }
    // not an export
    let other = openreadout_core::zip::zip_bytes(&[("readme.txt", b"hello")]).unwrap();
    assert!(matches!(
        open_zip(other),
        Err(openreadout_core::Error::Unsupported { .. })
    ));
}
