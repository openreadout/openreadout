//! Synthetic Seahorse `.asyr` files (gzip-compressed assay XML built from
//! `docs/formats/agilent-seahorse.md`): tables, traces and experiment facts, the refusal of a
//! plate not listed row by row, and damage (clean errors, never a panic).
#![allow(clippy::float_cmp)] // exact synthetic values

use std::fmt::Write as _;
use std::io::Write as _;

use openreadout_biophys::{SEAHORSE_FORMAT_ID, SeahorseReader};
use openreadout_core::source::Input;
use openreadout_core::{Dataset, Error, FormatReader};

fn gz(xml: &str) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(xml.as_bytes()).unwrap();
    e.finish().unwrap()
}

/// A 2 × 2 plate (A1 background), 3 readings, 1 measurement of 3 readings, one injection port.
fn assay(order: &[(u32, u32)]) -> String {
    let mut wells = String::new();
    for (k, (r, c)) in order.iter().enumerate() {
        let group = if k == 0 {
            "<GroupName>Background</GroupName><IsBackground>true</IsBackground>".to_string()
        } else {
            "<GroupName>cells</GroupName><IsBackground>false</IsBackground><InjectionCondition><Injections><PortInjection><PortContents><ReagentName>Oligomycin</ReagentName><PortConcentrationUnit>µM</PortConcentrationUnit><PortConcentration>1.5</PortConcentration><PortLocation>A</PortLocation></PortContents><Volume>25</Volume></PortInjection></Injections></InjectionCondition>".to_string()
        };
        let _ = write!(
            wells,
            "<Well><ParentGroup>{group}</ParentGroup><RowIndex>{r}</RowIndex><ColumnIndex>{c}</ColumnIndex><Flag>false</Flag></Well>"
        );
    }
    let mut ticks = String::new();
    for t in 0..3 {
        let mut analytes = String::new();
        for an in ["O2", "pH"] {
            let arr = |name: &str, v: f64| {
                let mut s = format!("<{name}>");
                for w in 0..4 {
                    let _ = write!(
                        s,
                        "<double>{}</double>",
                        v + f64::from(w) * 10.0 + f64::from(t)
                    );
                }
                s + &format!("</{name}>")
            };
            let _ = write!(
                analytes,
                "<Item><Key><string>{an}</string></Key><Value><AnalyteDataSet><AnalyteName>{an}</AnalyteName><WellTemperature>37</WellTemperature><IsValid>true</IsValid>{}{}{}{}{}{}</AnalyteDataSet></Value></Item>",
                arr("LedOnEmissionValues", 1000.0),
                arr("LedOffEmissionValues", 10.0),
                arr("LedOnReferenceValues", 2000.0),
                arr("LedOffReferenceValues", 20.0),
                arr("LedUseValues", 0.0),
                arr("CorrectedEmissionValues", 500.0),
            );
        }
        let _ = write!(
            ticks,
            "<PlateTickDataSet><TrayTemperature>37.0</TrayTemperature><EnvironmentalTemperature>30.5</EnvironmentalTemperature><AnalyteDataSetsByAnalyteName>{analytes}</AnalyteDataSetsByAnalyteName><TimeStamp>PT{}M</TimeStamp></PlateTickDataSet>",
            30 + t
        );
    }
    format!(
        "<?xml version=\"1.0\"?>\r\n<XfeAssay><Instrument>x</Instrument><AssayDataSet><RateSpans><RateSpan><StartTickIndex>0</StartTickIndex><EndTickIndex>2</EndTickIndex><ValideRate>true</ValideRate></RateSpan></RateSpans><PlateTickDataSets>{ticks}</PlateTickDataSets><CommandHistory><CommandHistory><InstructionName>Baseline</InstructionName><CommandName>Measure</CommandName><StartTime>2023-11-21T12:00:00+08:00</StartTime><EndTime>2023-11-21T12:03:00+08:00</EndTime><CompletionStatus>Success</CompletionStatus></CommandHistory></CommandHistory></AssayDataSet><Plate><RowCount>2</RowCount><ColumnCount>2</ColumnCount><Wells>{wells}</Wells></Plate><Name>Mito test</Name><InstrumentSerialNumber>00422084</InstrumentSerialNumber><SWVersion>2.6.1.56</SWVersion><LastRunBy>tester</LastRunBy><VersionStamp>4</VersionStamp></XfeAssay>"
    )
}

const ROW_MAJOR: [(u32, u32); 4] = [(0, 0), (0, 1), (1, 0), (1, 1)];

fn open(bytes: Vec<u8>) -> openreadout_core::Result<Box<dyn Dataset>> {
    SeahorseReader.open_input(&Input::from_bytes("run.asyr", bytes))
}

#[test]
fn reads_an_assay() {
    let bytes = gz(&assay(&ROW_MAJOR));
    let det = SeahorseReader
        .sniff(&bytes, std::path::Path::new("x.bin"))
        .unwrap();
    assert_eq!(det.format_id, SEAHORSE_FORMAT_ID);
    let mut ds = open(bytes).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 3);
    assert_eq!(info.traces[0].extra["analyte"], "O2");
    assert_eq!(info.traces[0].channels[1].name, "A1");
    assert_eq!(info.traces[0].channels[4].name, "B2");
    let tr = ds.read_trace(0, 0, 0, u64::MAX).unwrap();
    assert_eq!(tr.channels[0], vec![1800.0, 1860.0, 1920.0]);
    assert_eq!(tr.channels[4][2], 532.0); // well B2 (index 3), reading 2
    let wells = ds.read_table(0, 0, u64::MAX).unwrap();
    assert_eq!(wells.columns[4], vec![1.0, 0.0, 0.0, 0.0]);
    let inj = ds.read_table(2, 0, u64::MAX).unwrap();
    assert_eq!(inj.columns[3], vec![1.5]);
    assert_eq!(
        info.tables[2].columns[2].extra["categories"],
        serde_json::json!(["Oligomycin"])
    );
    let meas = ds.read_table(1, 0, u64::MAX).unwrap();
    assert_eq!(meas.columns[3], vec![1800.0]);
    let e = ds.experiment().unwrap();
    assert_eq!(e.instrument.unwrap().serial.as_deref(), Some("00422084"));
    assert_eq!(e.acquisition.unwrap().duration_s, Some(180.0));
    assert!(ds.check().unwrap().ok);
}

#[test]
fn refusals_and_damage() {
    let col_major = [(0, 0), (1, 0), (0, 1), (1, 1)];
    assert!(matches!(
        open(gz(&assay(&col_major))),
        Err(Error::Unsupported { .. })
    ));
    assert!(open(gz("<XfeAssay><Instrument/></XfeAssay>")).is_err());
    assert!(open(gz("<Other/>")).is_err());
    let bytes = gz(&assay(&ROW_MAJOR));
    for cut in (0..bytes.len()).step_by(13) {
        if let Ok(mut ds) = open(bytes[..cut].to_vec()) {
            let _ = ds.info();
            let _ = ds.check();
        }
    }
    let xml = assay(&ROW_MAJOR);
    for cut in (0..xml.len()).step_by(97) {
        if let Some(s) = xml.get(..cut)
            && let Ok(mut ds) = open(gz(s))
        {
            let _ = ds.info();
            for t in 0..3 {
                let _ = ds.read_trace(t, 0, 0, u64::MAX);
                let _ = ds.read_table(t, 0, u64::MAX);
            }
        }
    }
}
