//! Synthetic-file tests of the EDF / BDF reader.

use openreadout_core::reader::{Dataset, FormatReader};

use super::{EdfDataset, EdfReader, parse_tal, seconds_ns};

/// A signal for [`build`]: label, unit, physical min/max, digital min/max, samples per record.
struct Sig<'a> {
    label: &'a str,
    unit: &'a str,
    phys: (f64, f64),
    dig: (i64, i64),
    spr: usize,
}

fn pad(s: &str, n: usize) -> Vec<u8> {
    let mut b = s.as_bytes().to_vec();
    b.resize(n, b' ');
    b
}

/// Header + records; `records[k][s]` is signal `s`'s bytes in record `k`.
fn build(
    bdf: bool,
    reserved: &str,
    duration: &str,
    sigs: &[Sig],
    records: &[Vec<Vec<u8>>],
) -> Vec<u8> {
    let ns = sigs.len();
    let mut b = Vec::new();
    if bdf {
        b.push(0xFF);
        b.extend(b"BIOSEMI");
    } else {
        b.extend(pad("0", 8));
    }
    b.extend(pad("X X X X", 80));
    b.extend(pad("Startdate 02-MAR-2002 PSG-1 NN Telemetry03", 80));
    b.extend(pad("02.03.02", 8));
    b.extend(pad("11.25.00", 8));
    b.extend(pad(&(256 * (ns + 1)).to_string(), 8));
    b.extend(pad(reserved, 44));
    b.extend(pad(&records.len().to_string(), 8));
    b.extend(pad(duration, 8));
    b.extend(pad(&ns.to_string(), 4));
    for s in sigs {
        b.extend(pad(s.label, 16));
    }
    for _ in sigs {
        b.extend(pad("AgAgCl electrode", 80));
    }
    for s in sigs {
        b.extend(pad(s.unit, 8));
    }
    for s in sigs {
        b.extend(pad(&s.phys.0.to_string(), 8));
    }
    for s in sigs {
        b.extend(pad(&s.phys.1.to_string(), 8));
    }
    for s in sigs {
        b.extend(pad(&s.dig.0.to_string(), 8));
    }
    for s in sigs {
        b.extend(pad(&s.dig.1.to_string(), 8));
    }
    for _ in sigs {
        b.extend(pad("HP:0.1Hz LP:75Hz", 80));
    }
    for s in sigs {
        b.extend(pad(&s.spr.to_string(), 8));
    }
    for _ in sigs {
        b.extend(pad("", 32));
    }
    for r in records {
        for (s, bytes) in sigs.iter().zip(r) {
            let n = s.spr * if bdf { 3 } else { 2 };
            let mut v = bytes.clone();
            v.resize(n, 0);
            b.extend(v);
        }
    }
    b
}

fn i16s(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn i24s(v: &[i32]) -> Vec<u8> {
    v.iter()
        .flat_map(|x| x.to_le_bytes()[..3].to_vec())
        .collect()
}

fn tal(onset: &str, duration: Option<&str>, texts: &[&str]) -> Vec<u8> {
    let mut b = onset.as_bytes().to_vec();
    if let Some(d) = duration {
        b.push(21);
        b.extend(d.as_bytes());
    }
    b.push(20);
    for t in texts {
        b.extend(t.as_bytes());
        b.push(20);
    }
    b.push(0);
    b
}

fn eeg(spr: usize) -> Sig<'static> {
    Sig {
        label: "EEG Fpz-Cz",
        unit: "uV",
        phys: (-500.0, 500.0),
        dig: (-2048, 2047),
        spr,
    }
}

fn annotations(spr: usize) -> Sig<'static> {
    Sig {
        label: "EDF Annotations",
        unit: "",
        phys: (-1.0, 1.0),
        dig: (-32768, 32767),
        spr,
    }
}

fn open(bytes: &[u8], name: &str) -> (tempfile::TempDir, EdfDataset) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(name);
    std::fs::write(&p, bytes).unwrap();
    assert!(
        EdfReader
            .sniff(&bytes[..bytes.len().min(256)], &p)
            .is_some()
    );
    let ds = EdfDataset::open(&p).unwrap();
    (dir, ds)
}

#[test]
fn edf_plus_two_rates_and_annotations() {
    let temp = Sig {
        label: "Temp rectal",
        unit: "degC",
        phys: (34.4, 40.2),
        dig: (-2048, 2047),
        spr: 2,
    };
    let sigs = [eeg(4), temp, annotations(30)];
    let records: Vec<Vec<Vec<u8>>> = (0..3)
        .map(|k| {
            let mut a = tal(&format!("+{k}"), None, &[""]);
            if k == 1 {
                a.extend(tal("+1.5", Some("0.25"), &["Lights off", "Arousal"]));
            }
            vec![i16s(&[0, 1, -1, 2047]), i16s(&[-2048, 2047]), a]
        })
        .collect();
    let bytes = build(false, "EDF+C", "1", &sigs, &records);
    let (_d, mut ds) = open(&bytes, "x.edf");
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("EDF+C"));
    assert_eq!(info.traces.len(), 2);
    let (t0, t1) = (&info.traces[0], &info.traces[1]);
    assert_eq!(
        (t0.sample_rate_hz, t0.sample_count, t0.sweep_count),
        (4.0, 12, 1)
    );
    assert_eq!((t1.sample_rate_hz, t1.sample_count), (2.0, 6));
    assert_eq!(t0.channels[0].name, "EEG Fpz-Cz");
    assert_eq!(t1.channels[0].unit.as_deref(), Some("degC"));
    assert_eq!(t0.extra["recorded_at"], "2002-03-02T11:25:00");
    assert_eq!(t0.extra["equipment"], "Telemetry03");
    let tr = ds.read_trace(0, 0, 3, 3).unwrap();
    let scale = 1000.0 / 4095.0;
    let want = |d: f64| d * scale + (-500.0 - -2048.0 * scale);
    assert_eq!(tr.channels[0], vec![want(2047.0), want(0.0), want(1.0)]);
    let tr = ds.read_trace(1, 0, 0, 100).unwrap();
    assert_eq!(tr.channels[0].len(), 6);
    assert!((tr.channels[0][0] - 34.4).abs() < 1e-12);
    assert!((tr.channels[0][1] - 40.2).abs() < 1e-12);
    let t = &info.tables[0];
    assert_eq!(t.row_count, 2);
    assert_eq!(
        t.columns[2].extra["categories"],
        serde_json::json!(["Lights off", "Arousal"])
    );
    let rows = ds.read_table(0, 0, 10).unwrap();
    assert_eq!(rows.columns[0], vec![1.5, 1.5]);
    assert_eq!(rows.columns[1], vec![0.25, 0.25]);
    assert_eq!(rows.columns[2], vec![0.0, 1.0]);
    assert!(ds.check().unwrap().ok);
}

#[test]
fn discontinuous_records_are_sweeps() {
    let sigs = [eeg(2), annotations(10)];
    let starts = ["+0", "+1", "+5", "+6", "+7"];
    let records: Vec<Vec<Vec<u8>>> = starts
        .iter()
        .map(|s| vec![i16s(&[1, 2]), tal(s, None, &[""])])
        .collect();
    let bytes = build(false, "EDF+D", "1", &sigs, &records);
    let (_d, mut ds) = open(&bytes, "d.edf");
    let info = ds.info().unwrap();
    let t = &info.traces[0];
    assert_eq!((t.sweep_count, t.sample_count), (2, 6));
    assert_eq!(t.extra["sweep_starts_s"], serde_json::json!([0.0, 5.0]));
    assert_eq!(t.extra["sweep_sample_counts"], serde_json::json!([4, 6]));
    assert_eq!(ds.read_trace(0, 1, 0, 100).unwrap().channels[0].len(), 6);
    assert!(info.tables.is_empty());
}

#[test]
fn bdf_24_bit_samples() {
    let sigs = [
        Sig {
            label: "A1",
            unit: "uV",
            phys: (-262_144.0, 262_143.0),
            dig: (-8_388_608, 8_388_607),
            spr: 3,
        },
        Sig {
            label: "Status",
            unit: "Boolean",
            phys: (-8_388_608.0, 8_388_607.0),
            dig: (-8_388_608, 8_388_607),
            spr: 3,
        },
    ];
    let records = vec![vec![
        i24s(&[-8_388_608, 0, 8_388_607]),
        i24s(&[-1, 5, 65536]),
    ]];
    let bytes = build(true, "24BIT", "1", &sigs, &records);
    let (_d, mut ds) = open(&bytes, "b.bdf");
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("BDF"));
    assert_eq!(info.traces[0].channels[0].dtype, "int24");
    let tr = ds.read_trace(0, 0, 0, 10).unwrap();
    assert!((tr.channels[0][0] + 262_144.0).abs() < 1e-9);
    assert!((tr.channels[0][2] - 262_143.0).abs() < 1e-9);
    assert_eq!(tr.channels[1], vec![-1.0, 5.0, 65536.0]);
}

#[test]
fn reads_page_across_records() {
    let sigs = [eeg(3)];
    let records: Vec<Vec<Vec<u8>>> = (0..4)
        .map(|k| vec![i16s(&[k * 10, k * 10 + 1, k * 10 + 2])])
        .collect();
    let bytes = build(false, "", "0.5", &sigs, &records);
    let (_d, mut ds) = open(&bytes, "p.edf");
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("EDF"));
    assert_eq!(info.traces[0].sample_rate_hz, 6.0);
    let s = info.traces[0].channels[0].scale;
    let o = info.traces[0].channels[0].offset;
    let tr = ds.read_trace(0, 0, 2, 5).unwrap();
    let raw: Vec<f64> = tr.channels[0]
        .iter()
        .map(|v| ((v - o) / s).round())
        .collect();
    assert_eq!(raw, vec![2.0, 10.0, 11.0, 12.0, 20.0]);
}

#[test]
fn tals() {
    let (o, d, t) = parse_tal(b"+993.2\x151.2\x14Limb movement\x14R+L leg\x14").unwrap();
    assert_eq!((o, d), (993_200_000_000, Some(1_200_000_000)));
    assert_eq!(t, vec!["Limb movement", "R+L leg"]);
    let (o, d, t) = parse_tal(b"-0.065\x14\x14").unwrap();
    assert_eq!((o, d), (-65_000_000, None));
    assert_eq!(t, vec![""]);
    assert!(parse_tal(b"12\x14x\x14").is_none());
    assert!(parse_tal(b"+1e3\x14x\x14").is_none());
    assert_eq!(seconds_ns("+.5"), Some(500_000_000));
    assert_eq!(seconds_ns("+0.1234567891"), Some(123_456_789));
}

#[test]
fn subsecond_start() {
    // the first record starts 0.25 s after the header's time; onsets count from there
    let sigs = [eeg(2), annotations(20)];
    let records: Vec<Vec<Vec<u8>>> = (0..2)
        .map(|k| {
            let mut a = tal(&format!("+{k}.25"), None, &[""]);
            if k == 0 {
                a.extend(tal("+1.5", None, &["Spike"]));
            }
            vec![i16s(&[1, 2]), a]
        })
        .collect();
    let bytes = build(false, "EDF+C", "1", &sigs, &records);
    let (_d, mut ds) = open(&bytes, "s.edf");
    let info = ds.info().unwrap();
    assert_eq!(
        info.traces[0].extra["recorded_at"],
        "2002-03-02T11:25:00.25"
    );
    assert_eq!(info.traces[0].start_s, Some(0.0));
    assert_eq!(ds.read_table(0, 0, 1).unwrap().columns[0], vec![1.25]);
}

#[test]
fn damaged_files_fail_cleanly() {
    let sigs = [eeg(4)];
    let records: Vec<Vec<Vec<u8>>> = (0..3).map(|_| vec![i16s(&[1, 2, 3, 4])]).collect();
    let bytes = build(false, "", "1", &sigs, &records);
    // cut in the middle of the last record: two whole records are read, check reports it
    let (_d, mut ds) = open(&bytes[..bytes.len() - 3], "t.edf");
    assert_eq!(ds.info().unwrap().traces[0].sample_count, 8);
    assert!(!ds.check().unwrap().ok);
    // every prefix opens or fails with an error, never a panic
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("cut.edf");
    for n in [0, 10, 255, 256, 300, 511, 512, 513] {
        std::fs::write(&p, &bytes[..n.min(bytes.len())]).unwrap();
        if let Ok(mut ds) = EdfDataset::open(&p) {
            let _ = ds.info();
            let _ = ds.read_trace(0, 0, 0, 100);
            let _ = ds.check();
        }
    }
    // a corrupt signal count
    let mut bad = bytes.clone();
    bad[252..256].copy_from_slice(b"ab  ");
    std::fs::write(&p, &bad).unwrap();
    assert!(EdfDataset::open(&p).is_err());
}
