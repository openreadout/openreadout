//! Synthetic Biacore `.blr` result files (a minimal compound-file writer below): the reader's
//! traces, tables and experiment facts, and its refusals of damaged or unvalidated layouts.
#![allow(
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::needless_range_loop
)] // exact synthetic values; sector arithmetic

use std::collections::BTreeMap;

use openreadout_biophys::{BIACORE_FORMAT_ID, BiacoreReader};
use openreadout_core::{Error, FormatReader};

const END: u32 = 0xFFFF_FFFE;
const FREE: u32 = 0xFFFF_FFFF;
const NONE: u32 = 0xFFFF_FFFF;

/// A version-3 compound file holding `streams` (`/`-separated paths), every stream in regular
/// 512-byte sectors (mini-stream cutoff 0).
fn compound_file(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    // directory: root, storages, streams
    struct Node {
        name: String,
        kind: u8,
        data: Vec<u8>,
        children: Vec<usize>,
    }
    let mut nodes = vec![Node {
        name: "Root Entry".into(),
        kind: 5,
        data: Vec::new(),
        children: Vec::new(),
    }];
    let mut dirs: BTreeMap<String, usize> = BTreeMap::new();
    for (path, data) in streams {
        let mut parent = 0;
        let parts: Vec<&str> = path.split('/').collect();
        for k in 0..parts.len() - 1 {
            let key = parts[..=k].join("/");
            parent = *dirs.entry(key).or_insert_with(|| {
                nodes.push(Node {
                    name: parts[k].into(),
                    kind: 1,
                    data: Vec::new(),
                    children: Vec::new(),
                });
                let id = nodes.len() - 1;
                nodes[parent].children.push(id);
                id
            });
        }
        nodes.push(Node {
            name: parts[parts.len() - 1].into(),
            kind: 2,
            data: data.clone(),
            children: Vec::new(),
        });
        let id = nodes.len() - 1;
        nodes[parent].children.push(id);
    }
    let sectors = |n: usize| n.div_ceil(512);
    let dir_secs = sectors(nodes.len() * 128);
    let data_secs: usize = nodes.iter().map(|n| sectors(n.data.len())).sum();
    let mut fat_secs = 1;
    while fat_secs * 128 < fat_secs + dir_secs + data_secs {
        fat_secs += 1;
    }
    let total = fat_secs + dir_secs + data_secs;
    let mut fat = vec![FREE; fat_secs * 128];
    for f in fat.iter_mut().take(fat_secs) {
        *f = 0xFFFF_FFFD;
    }
    let chain = |fat: &mut Vec<u32>, first: usize, n: usize| {
        for s in first..first + n {
            fat[s] = if s + 1 < first + n {
                (s + 1) as u32
            } else {
                END
            };
        }
    };
    chain(&mut fat, fat_secs, dir_secs);
    let mut next = fat_secs + dir_secs;
    let mut starts = vec![END; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        let k = sectors(n.data.len());
        if k > 0 {
            starts[i] = next as u32;
            chain(&mut fat, next, k);
            next += k;
        }
    }
    let mut f = vec![0u8; 512 * (1 + total)];
    f[..8].copy_from_slice(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]);
    f[0x18..0x1A].copy_from_slice(&0x3Eu16.to_le_bytes());
    f[0x1A..0x1C].copy_from_slice(&3u16.to_le_bytes());
    f[0x1C..0x1E].copy_from_slice(&0xFFFEu16.to_le_bytes());
    f[0x1E..0x20].copy_from_slice(&9u16.to_le_bytes());
    f[0x20..0x22].copy_from_slice(&6u16.to_le_bytes());
    f[0x2C..0x30].copy_from_slice(&(fat_secs as u32).to_le_bytes());
    f[0x30..0x34].copy_from_slice(&(fat_secs as u32).to_le_bytes());
    f[0x38..0x3C].copy_from_slice(&0u32.to_le_bytes());
    f[0x3C..0x40].copy_from_slice(&END.to_le_bytes());
    f[0x44..0x48].copy_from_slice(&END.to_le_bytes());
    for i in 0..109 {
        let v = if i < fat_secs { i as u32 } else { FREE };
        f[0x4C + 4 * i..0x50 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    for (i, v) in fat.iter().enumerate() {
        f[512 + 4 * i..516 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    let dir0 = 512 * (1 + fat_secs);
    // right-sibling chains under each storage
    let mut right = vec![NONE; nodes.len()];
    for n in &nodes {
        for w in n.children.windows(2) {
            right[w[0]] = w[1] as u32;
        }
    }
    for (i, n) in nodes.iter().enumerate() {
        let o = dir0 + 128 * i;
        let units: Vec<u16> = n.name.encode_utf16().collect();
        for (k, u) in units.iter().enumerate() {
            f[o + 2 * k..o + 2 * k + 2].copy_from_slice(&u.to_le_bytes());
        }
        f[o + 0x40..o + 0x42].copy_from_slice(&((units.len() as u16 + 1) * 2).to_le_bytes());
        f[o + 0x42] = n.kind;
        f[o + 0x43] = 1;
        f[o + 0x44..o + 0x48].copy_from_slice(&NONE.to_le_bytes());
        f[o + 0x48..o + 0x4C].copy_from_slice(&right[i].to_le_bytes());
        let child = n.children.first().map_or(NONE, |&c| c as u32);
        f[o + 0x4C..o + 0x50].copy_from_slice(&child.to_le_bytes());
        f[o + 0x74..o + 0x78].copy_from_slice(&starts[i].to_le_bytes());
        f[o + 0x78..o + 0x7C].copy_from_slice(&(n.data.len() as u32).to_le_bytes());
        if let Ok(s) = usize::try_from(starts[i])
            && s != END as usize
        {
            let at = 512 * (1 + s);
            f[at..at + n.data.len()].copy_from_slice(&n.data);
        }
    }
    // unused directory slots are empty entries
    for i in nodes.len()..dir_secs * 4 {
        let o = dir0 + 128 * i;
        for off in [0x44, 0x48, 0x4C] {
            f[o + off..o + off + 4].copy_from_slice(&NONE.to_le_bytes());
        }
    }
    f
}

fn segment(step: f64, start: f64, values: &[f32]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(1u32.to_le_bytes());
    b.extend(1u32.to_le_bytes());
    for v in [step, start, 0.0, 1.0] {
        b.extend(v.to_le_bytes());
    }
    b.extend((values.len() as u32).to_le_bytes());
    for v in values {
        b.extend(v.to_le_bytes());
    }
    b
}

fn xy(times: &[f32], values: &[f32]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(1u32.to_le_bytes());
    b.extend(1u32.to_le_bytes());
    b.extend((values.len() as u32).to_le_bytes());
    for v in times.iter().chain(values) {
        b.extend(v.to_le_bytes());
    }
    b
}

/// A kinetics run: one cycle, flow cell 2 (segment, 100 samples at 10 Hz) and 2-1 (XY data).
fn run(seg: Vec<u8>, compat: &str) -> Vec<(&'static str, Vec<u8>)> {
    let times: Vec<f32> = (1..=100).map(|i| i as f32 * 0.1).collect();
    let sub: Vec<f32> = (0..100).map(|i| i as f32 * 0.5).collect();
    // report point at 5 s, window 1 s: samples 4.5 .. 5.5 s = indices 44..=54 -> mean 1049
    let rpoint = "Cycle\tFc\tTime\tWindow\tAbsResp\tRelResp\tId\n1\t2\t5\t1\t1049\tN/A\tbaseline\n1\t2-1\t5\t1\t24.5\t3\tbinding\n";
    vec![
        ("\u{3}BIA compability info", compat.as_bytes().to_vec()),
        ("Environment", b"Application=Biacore T200 Control Software\nVersion=2.0.1\nRunType=Kinetics/Affinity\nProcessingUnit=BiacoreT200\nInstrumentId=4242\nTimestamp=43082.5\nUserName=tester\nEndTime=43082.75\n".to_vec()),
        ("Chip", b"Name=CM5\nId=chip-7\nNoFcs=4\n".to_vec()),
        ("RPoint Table", rpoint.as_bytes().to_vec()),
        ("_Cycle 1/EventLog", b"2\nF0;10;d43082.5125\nF1500;33;pR2A1;f30;iSample 1\n".to_vec()),
        ("_Cycle 1/_Window 1/Properties", b"Caption=Sample\nTitle=Sensorgram\n".to_vec()),
        ("_Cycle 1/_Window 1/_Curve 1/Labels", b"Sensorgram Fc=2\nTime\ns\nResponse\nRU\n".to_vec()),
        ("_Cycle 1/_Window 1/_Curve 1/\u{3}Keywords", b"Version=1\nTemp#=25\nSample_1_Sample=B2\nSample_1_Conc#=10\nFc=2\n".to_vec()),
        ("_Cycle 1/_Window 1/_Curve 1/\u{3}RPoints", b"1\n5\t1\t-1\tbaseline\n".to_vec()),
        ("_Cycle 1/_Window 1/_Curve 1/Segment 1", seg),
        ("_Cycle 1/_Window 1/_Curve 2/Labels", b"Subtracted Fc=2-1\nTime\ns\nResp. Diff.\nRU\n".to_vec()),
        ("_Cycle 1/_Window 1/_Curve 2/\u{3}Keywords", b"Version=1\nTemp#=25\nFc=2-1\n".to_vec()),
        ("_Cycle 1/_Window 1/_Curve 2/XYData", xy(&times, &sub)),
    ]
}

fn good_segment() -> Vec<u8> {
    let raw: Vec<f32> = (0..100).map(|i| 1000.0 + i as f32).collect();
    segment(0.1, 0.1, &raw)
}

const COMPAT: &str = "FileType=Result File\nFileTypeVersion=12\n";

fn write(dir: &tempfile::TempDir, name: &str, streams: &[(&str, Vec<u8>)]) -> std::path::PathBuf {
    let p = dir.path().join(name);
    std::fs::write(&p, compound_file(streams)).unwrap();
    p
}

#[test]
fn reads_a_kinetics_run() {
    let dir = tempfile::tempdir().unwrap();
    let p = write(&dir, "run.blr", &run(good_segment(), COMPAT));
    let reg = openreadout_core::Registry::new().with(Box::new(BiacoreReader));
    let (id, mut ds) = reg.open(&p).unwrap();
    assert_eq!(id.format_id, BIACORE_FORMAT_ID);
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 2);
    let t0 = &info.traces[0];
    assert_eq!(t0.name.as_deref(), Some("cycle 1 Sensorgram Fc=2"));
    assert_eq!(t0.sample_count, 100);
    assert!((t0.sample_rate_hz - 10.0).abs() < 1e-12);
    assert_eq!(t0.start_s, Some(0.1));
    assert_eq!(t0.extra["flow_cell"], "2");
    assert_eq!(t0.extra["keywords"]["Sample_1_Sample"], "B2");
    let t1 = &info.traces[1];
    assert_eq!(t1.extra["reference_subtracted"], true);
    assert_eq!(t1.start_s, Some(0.1));
    assert!((t1.sample_rate_hz - 10.0).abs() < 1e-9);
    let tr = ds.read_trace(0, 0, 44, 11).unwrap();
    let mean = tr.channels[0].iter().sum::<f64>() / 11.0;
    assert_eq!(mean, 1049.0);
    let tr = ds.read_trace(1, 0, 0, u64::MAX).unwrap();
    assert_eq!(tr.channels.len(), 1);
    assert_eq!(tr.channels[0][99], 49.5);
    // tables: report points, cycles, event log
    let names: Vec<_> = info
        .tables
        .iter()
        .map(|t| t.name.clone().unwrap())
        .collect();
    assert_eq!(names, ["report_points", "cycles", "event_log"]);
    let rp = ds.read_table(0, 0, u64::MAX).unwrap();
    assert_eq!(rp.columns[4], vec![1049.0, 24.5]);
    assert!(rp.columns[5][0].is_nan());
    assert_eq!(
        info.tables[0].columns[1].extra["categories"],
        serde_json::json!(["2", "2-1"])
    );
    let cy = ds.read_table(1, 0, u64::MAX).unwrap();
    assert!(
        (cy.columns[2][0] - 1080.0).abs() < 1e-6,
        "{:?}",
        cy.columns[2]
    ); // 0.0125 d after the run start
    let ev = ds.read_table(2, 0, u64::MAX).unwrap();
    assert_eq!(ev.columns[1], vec![0.0, 1.5]);
    let e = ds.experiment().unwrap();
    let ins = e.instrument.unwrap();
    assert_eq!(ins.model.as_deref(), Some("Biacore T200"));
    assert_eq!(ins.serial.as_deref(), Some("4242"));
    let acq = e.acquisition.unwrap();
    assert_eq!(acq.started_at.as_deref(), Some("2017-12-13T12:00:00"));
    assert_eq!(acq.duration_s, Some(21_600.0));
    let m = e.method.unwrap();
    assert_eq!(m.parameters["temperature"].value.as_f64(), Some(25.0));
    assert_eq!(m.parameters["flow_cells"].value.as_f64(), Some(4.0));
    let c = ds.check().unwrap();
    assert!(c.ok, "{:?}", c.findings);
}

#[test]
fn refuses_what_is_not_validated() {
    let dir = tempfile::tempdir().unwrap();
    // not a result file
    let p = write(
        &dir,
        "method.blr",
        &run(good_segment(), "FileType=Method\n"),
    );
    let Err(err) = BiacoreReader.open(&p) else {
        panic!("a method file was opened");
    };
    assert!(matches!(err, Error::Unsupported { .. }), "{err}");
    // step and start differ: which is which is not known, the curve is left out with a finding
    let raw: Vec<f32> = (0..100).map(|i| i as f32).collect();
    let p = write(&dir, "odd.blr", &run(segment(0.1, 2.0, &raw), COMPAT));
    let mut ds = BiacoreReader.open(&p).unwrap();
    assert_eq!(ds.info().unwrap().traces.len(), 1);
    let c = ds.check().unwrap();
    assert!(
        c.findings.iter().any(|f| f.code == "unvalidated_time_base"),
        "{:?}",
        c.findings
    );
    // a segment whose size disagrees with its count
    let mut seg = good_segment();
    seg.truncate(seg.len() - 4);
    let p = write(&dir, "short.blr", &run(seg, COMPAT));
    let mut ds = BiacoreReader.open(&p).unwrap();
    let c = ds.check().unwrap();
    assert!(!c.ok);
    assert!(c.findings.iter().any(|f| f.code == "unreadable_curve"));
    // no curve at all
    let streams: Vec<_> = run(good_segment(), COMPAT)
        .into_iter()
        .filter(|(p, _)| !p.contains("_Curve"))
        .collect();
    let p = write(&dir, "empty.blr", &streams);
    assert!(matches!(BiacoreReader.open(&p), Err(Error::Corrupt { .. })));
}

#[test]
fn damaged_files_never_panic() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = compound_file(&run(good_segment(), COMPAT));
    let p = dir.path().join("cut.blr");
    for cut in (0..bytes.len()).step_by(97) {
        std::fs::write(&p, &bytes[..cut]).unwrap();
        if let Ok(mut ds) = BiacoreReader.open(&p) {
            let _ = ds.info();
            let _ = ds.check();
            for t in 0..3 {
                let _ = ds.read_trace(t, 0, 0, u64::MAX);
                let _ = ds.read_table(t, 0, u64::MAX);
            }
        }
    }
    // flipped bytes in the directory and data
    for at in (0..bytes.len()).step_by(211) {
        let mut b = bytes.clone();
        b[at] ^= 0xA5;
        std::fs::write(&p, &b).unwrap();
        if let Ok(mut ds) = BiacoreReader.open(&p) {
            let _ = ds.check();
            let _ = ds.read_trace(0, 0, 0, u64::MAX);
        }
    }
}

/// An evaluation file: the kinetics run under `_DataManager 1/` and an affinity item with a fit.
fn evaluation() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = run(good_segment(), COMPAT)
        .into_iter()
        .filter(|(n, _)| !n.starts_with('\u{3}'))
        .map(|(n, d)| (format!("_DataManager 1/{n}"), d))
        .collect();
    v.push((
        "\u{3}BIA compability info".into(),
        b"FileType=T200 Evaluation File\nFileTypeVersion=4\n".to_vec(),
    ));
    v.push((
        "Environment".into(),
        b"Application=Biacore T200 Evaluation Software\nVersion=3.0\n".to_vec(),
    ));
    let item = "<EvaluationItem3 ClassName=\"AffinityScreen\"><Name>Affinity Screen</Name><modelFits><modelFits><modelFit><curveSet><CurveSet><SampleName>B2</SampleName><Temperature>25</Temperature><Subset0><CurveName>Fc=2-1</CurveName><Curve0><FileIndex>1</FileIndex><CycleNumber>1</CycleNumber><SampleName>B2</SampleName><ConcUnit>\u{b5}M</ConcUnit><Injections><Injection><Concentration>10</Concentration><MolarConcentration>1E-05</MolarConcentration><Response>24.5</Response></Injection></Injections><Included>true</Included></Curve0></Subset0></CurveSet></curveSet><model ModelName=\"Steady State Affinity\"><Model>Conc*Rmax/(Conc+KD)+offset</Model><Chi2>0.5</Chi2><Parameters>0:KD|2E-05|1E-06;0:Rmax|60|2;0:offset|0|0</Parameters><ReportParameters>KD (M)|KD;Rmax (RU)|Rmax;offset (RU)|offset</ReportParameters></model><fitStatus>Cleared</fitStatus></modelFit></modelFits></modelFits></EvaluationItem3>";
    // ISO-8859-1, as the evaluation software writes it
    v.push((
        "Evaluation/EvaluationItem3".into(),
        item.chars().map(|c| c as u8).collect(),
    ));
    v.push((
        "Evaluation/EvaluationItem4".into(),
        b"<EvaluationItem4 ClassName=\"Plot\"><Name>Binding level</Name></EvaluationItem4>"
            .to_vec(),
    ));
    v
}

fn write_owned(
    dir: &tempfile::TempDir,
    name: &str,
    streams: &[(String, Vec<u8>)],
) -> std::path::PathBuf {
    let refs: Vec<(&str, Vec<u8>)> = streams
        .iter()
        .map(|(n, d)| (n.as_str(), d.clone()))
        .collect();
    write(dir, name, &refs)
}

#[test]
fn reads_an_evaluation_file() {
    let dir = tempfile::tempdir().unwrap();
    let p = write_owned(&dir, "eval.bme", &evaluation());
    let reg = openreadout_core::Registry::new()
        .with(Box::new(BiacoreReader))
        .with(Box::new(openreadout_biophys::BiacoreEvaluationReader));
    let (id, mut ds) = reg.open(&p).unwrap();
    assert_eq!(id.format_id, openreadout_biophys::BIACORE_BME_FORMAT_ID);
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 2);
    assert_eq!(info.traces[0].extra["file"], 1);
    let names: Vec<_> = info
        .tables
        .iter()
        .map(|t| t.name.clone().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "report_points",
            "cycles",
            "event_log",
            "evaluation_items",
            "fits",
            "fit_parameters",
            "fit_points"
        ]
    );
    let k = names.iter().position(|n| n == "fits").unwrap();
    let t = &info.tables[k];
    let col = |n: &str| t.columns.iter().position(|c| c.name == n).unwrap();
    let fits = ds.read_table(k as u32, 0, u64::MAX).unwrap();
    assert_eq!(fits.columns[col("KD")], vec![2e-5]);
    assert_eq!(t.columns[col("KD")].unit.as_deref(), Some("M"));
    assert_eq!(fits.columns[col("Rmax_se")], vec![2.0]);
    assert_eq!(fits.columns[col("chi2")], vec![0.5]);
    let pts = names.iter().position(|n| n == "fit_points").unwrap();
    let pt = ds.read_table(pts as u32, 0, u64::MAX).unwrap();
    let pcol = |n: &str| {
        info.tables[pts]
            .columns
            .iter()
            .position(|c| c.name == n)
            .unwrap()
    };
    assert_eq!(pt.columns[pcol("concentration")], vec![1e-5]);
    assert_eq!(pt.columns[pcol("response")], vec![24.5]);
    // the result-file reader leaves an evaluation file to the evaluation reader, and says so
    assert!(matches!(
        BiacoreReader.open(&p).err(),
        Some(Error::Unsupported { .. })
    ));
    // a damaged item is a finding, not a failure
    let mut bad = evaluation();
    bad.iter_mut()
        .find(|(n, _)| n == "Evaluation/EvaluationItem3")
        .unwrap()
        .1
        .truncate(40);
    let p = write_owned(&dir, "bad.bme", &bad);
    let mut ds = openreadout_biophys::BiacoreEvaluationReader
        .open(&p)
        .unwrap();
    let r = ds.check_headers().unwrap();
    assert!(
        r.findings
            .iter()
            .any(|f| f.code == "unreadable_evaluation_item")
    );
}
