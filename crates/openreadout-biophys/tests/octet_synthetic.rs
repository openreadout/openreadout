//! Synthetic Sartorius Octet `.frd` documents built from `docs/formats/sartorius-octet.md`: the
//! sensorgram, the step table and the experiment facts read back; damage ends in clean errors.
#![allow(clippy::float_cmp)] // exact synthetic values

use base64::Engine as _;
use openreadout_biophys::{OCTET_FORMAT_ID, OctetReader};
use openreadout_core::source::Input;
use openreadout_core::{Dataset, Error, FormatReader};

fn b64(v: &[f32]) -> String {
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn step(name: &str, kind: &str, conc: f64, x: &[f32], y: &[f32], points: usize) -> String {
    format!(
        "<Step><CommonData><SampleLocation>3</SampleLocation><SampleID>ab</SampleID><SampleRow>A</SampleRow><WellType>SAMPLE</WellType><Concentration>{conc}</Concentration><ConcentrationUnits>µg/ml</ConcentrationUnits><MolarConcentration>-1</MolarConcentration><MolarConcUnits>nM</MolarConcUnits><MolecularWeight>-1</MolecularWeight><Temperature>25.5</Temperature><StartTime>0</StartTime><AssayTime>0.4</AssayTime></CommonData><FlowRate>1000</FlowRate><StepType>{kind}</StepType><StepName>{name}</StepName><StepStatus>OK</StepStatus><ActualTime>0.4</ActualTime><CycleTime>0.2</CycleTime><AssayXData Points=\"{points}\">{}</AssayXData><AssayYData Points=\"{points}\">{}</AssayYData></Step>",
        b64(x),
        b64(y)
    )
}

fn doc(second_points: usize) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ExperimentResults><ExperimentInfo Name=\"Experiment_1\"><RTDVersion>2.0</RTDVersion><ExperimentType>KINETICS</ExperimentType><ExperimentSubType>KBASIC</ExperimentSubType><StartDateTime>2021-01-22T17:21:55</StartDateTime><UserName>Labuser</UserName><SensorName>A1</SensorName><SensorType>SAX</SensorType><SensorRole>LIGAND</SensorRole><WritingSW>DataAcquisition.exe 11.1.2.24</WritingSW><InstrumentType>OctetRED96e</InstrumentType><InstrumentSerial>FB-1</InstrumentSerial></ExperimentInfo><KineticsData>{}{}</KineticsData></ExperimentResults>",
        step("Baseline", "BASELINE", -1.0, &[0.0, 0.2], &[0.0, 0.01], 2),
        step(
            "Association",
            "ASSOC",
            50.0,
            &[0.4, 0.6],
            &[0.5, 0.75],
            second_points
        )
    )
}

fn open(xml: String) -> openreadout_core::Result<Box<dyn Dataset>> {
    OctetReader.open_input(&Input::from_bytes("a.frd", xml.into_bytes()))
}

#[test]
fn reads_a_sensor() {
    let xml = doc(2);
    let det = OctetReader
        .sniff(xml.as_bytes(), std::path::Path::new("a.frd"))
        .unwrap();
    assert_eq!(det.format_id, OCTET_FORMAT_ID);
    let mut ds = open(xml).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces[0].name.as_deref(), Some("sensor A1"));
    let tr = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(
        tr.channels[0],
        [
            0.0,
            f64::from(0.2_f32),
            f64::from(0.4_f32),
            f64::from(0.6_f32)
        ]
    );
    assert_eq!(tr.channels[1][3], 0.75);
    assert_eq!(tr.channels[2], [1.0, 1.0, 2.0, 2.0]);
    let t = ds.read_table(0, 0, u64::MAX).unwrap();
    let col = |n: &str| {
        info.tables[0]
            .columns
            .iter()
            .position(|c| c.name == n)
            .unwrap()
    };
    assert!(t.columns[col("concentration")][0].is_nan());
    assert_eq!(t.columns[col("concentration")][1], 50.0);
    assert_eq!(t.columns[col("points")], [2.0, 2.0]);
    let e = ds.experiment().unwrap();
    assert_eq!(e.instrument.unwrap().model.as_deref(), Some("OctetRED96e"));
    assert_eq!(
        e.acquisition.unwrap().started_at.as_deref(),
        Some("2021-01-22T17:21:55")
    );
}

#[test]
fn damage_is_a_clean_error() {
    // a Points attribute that disagrees with the data
    assert!(matches!(open(doc(3)), Err(Error::Corrupt { .. })));
    let xml = doc(2);
    for cut in [10, 200, xml.len() / 2, xml.len() - 5] {
        let r = OctetReader.open_input(&Input::from_bytes("a.frd", xml.as_bytes()[..cut].to_vec()));
        assert!(r.is_err());
    }
    // no kinetics data: refused
    let other = xml.replace("KineticsData", "QuantData");
    assert!(matches!(open(other), Err(Error::Unsupported { .. })));
    // bad base64
    let bad = xml.replacen(
        "<AssayYData Points=\"2\">",
        "<AssayYData Points=\"2\">%%",
        1,
    );
    assert!(matches!(open(bad), Err(Error::Corrupt { .. })));
}
