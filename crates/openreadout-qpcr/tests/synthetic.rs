//! The readers on small hand-written files (`tests/fixtures/`, also the fuzz seeds): every layout
//! parses into the expected records, RDML export round-trips, and malformed or mutated inputs
//! give clean errors (never a panic).

use std::path::{Path, PathBuf};

use openreadout_core::{Dataset, FormatReader, Registry};
use openreadout_qpcr::{
    EdsReader, PcrdReader, QpcrDataset, QpcrReportRequest, RdmlReader, RexReader, export_rdml,
    qpcr_report,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(RdmlReader))
        .with(Box::new(EdsReader))
        .with(Box::new(PcrdReader))
        .with(Box::new(RexReader))
}

fn report(p: &Path, req: &QpcrReportRequest) -> openreadout_qpcr::QpcrReport {
    qpcr_report(&QpcrDataset::open(p).unwrap(), req).unwrap()
}

#[test]
fn rdml_zip_and_bare_xml() {
    for name in ["small.rdml", "bare.xml"] {
        let p = fixture(name);
        let (_, det) = registry().detect(&p).unwrap();
        assert_eq!(det.format_id, "rdml", "{name}");
        let r = report(&p, &QpcrReportRequest::default());
        assert_eq!(r.records.len(), 3);
        let a1 = r.records.iter().find(|x| x.well == "A1").unwrap();
        assert_eq!(a1.cq, Some(2.5));
        assert_eq!(a1.tm, vec![80.0]);
        assert_eq!(a1.threshold, Some(4.0));
        let b2 = r.records.iter().find(|x| x.well == "B2").unwrap();
        assert!(b2.cq_undetermined && b2.excluded.is_some());
        assert_eq!(b2.task.as_deref(), Some("ntc"));
        assert_eq!(r.cycles, Some(4));
        assert_eq!(r.acquisition_temperature_c, Some(60.0));
    }
}

#[test]
fn eds_layouts() {
    for (name, dialect) in [
        ("sds.eds", "eds-sds"),
        ("7500.eds", "eds-7500"),
        ("json.eds", "eds-json"),
    ] {
        let p = fixture(name);
        let (_, det) = registry().detect(&p).unwrap();
        assert_eq!(det.format_id, "applied-biosystems-eds", "{name}");
        let r = report(&p, &{
            let mut q = QpcrReportRequest::default();
            q.compute_cq = true;
            q.standard_curve = true;
            q
        });
        assert_eq!(r.dialect, dialect);
        let a1 = r.records.iter().find(|x| x.well == "A1").unwrap();
        assert_eq!(a1.target.as_deref(), Some("G"), "{name}");
        assert_eq!(a1.cq, Some(2.5), "{name}");
        let a2 = r.records.iter().find(|x| x.well == "A2").unwrap();
        assert_eq!(a2.task.as_deref(), Some("ntc"), "{name}");
        assert!(a2.cq_undetermined, "{name}");
        assert_eq!(r.cycles, Some(4), "{name}");
        assert_eq!(r.acquisition_temperature_c, Some(60.0), "{name}");
        match dialect {
            "eds-sds" => {
                assert_eq!(a1.tm, vec![80.5, 85.0]);
                assert_eq!(a1.vendor_delta_cq, Some(1.0));
                assert!(a2.excluded.is_some(), "IsOmit");
                assert_eq!(a1.threshold, Some(3.0));
            }
            "eds-json" => {
                assert_eq!(a1.threshold, Some(3.0));
                assert_eq!(a1.calculated_quantity, Some(10.0));
                assert_eq!(a1.quantity, Some(10.0));
            }
            _ => {}
        }
        let mut ds = QpcrDataset::open(&p).unwrap();
        let info = ds.info().unwrap();
        assert!(!info.traces.is_empty(), "{name}");
        let t = ds.read_trace(0, 0, 0, 100).unwrap();
        assert!(!t.channels.is_empty());
        assert!(ds.read_trace(0, 1, 0, 1).is_err());
        let tab = ds.read_table(0, 0, 100).unwrap();
        assert_eq!(tab.columns.len(), openreadout_qpcr::RESULT_COLUMNS.len());
        assert!(
            ds.check()
                .unwrap()
                .findings
                .iter()
                .all(|f| f.severity.as_str() != "error"),
            "{name}"
        );
    }
}

#[test]
fn rex_readings() {
    let p = fixture("small.rex");
    let (_, det) = registry().detect(&p).unwrap();
    assert_eq!(det.format_id, "rotor-gene-rex");
    let r = report(&p, &{
        let mut q = QpcrReportRequest::default();
        q.compute_cq = true;
        q
    });
    assert_eq!(r.records.len(), 2);
    assert_eq!(r.records[0].target.as_deref(), Some("G"));
    assert_eq!(r.records[0].task.as_deref(), Some("standard"));
    assert_eq!(r.records[1].task.as_deref(), Some("ntc"));
    assert_eq!(r.records[0].cycles, 4);
}

#[test]
fn rdml_export_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["small.rdml", "sds.eds", "7500.eds", "json.eds", "small.rex"] {
        let ds = QpcrDataset::open(&fixture(name)).unwrap();
        let out = dir.path().join(format!("{name}.rdml"));
        let rep = export_rdml(&ds, &out, false).unwrap();
        assert!(rep.verified, "{name}");
        assert!(
            export_rdml(&ds, &out, false).is_err(),
            "no overwrite without the flag"
        );
        let back = QpcrDataset::open(&out).unwrap();
        let a = qpcr_report(&ds, &QpcrReportRequest::default()).unwrap();
        let b = qpcr_report(&back, &QpcrReportRequest::default()).unwrap();
        let key = |r: &openreadout_qpcr::QpcrReport| {
            let mut v: Vec<(String, Option<String>, Option<u64>, bool)> = r
                .records
                .iter()
                .filter(|x| x.target.is_some())
                .map(|x| {
                    (
                        x.well.clone(),
                        x.target.clone(),
                        x.cq.map(f64::to_bits),
                        x.cq_undetermined,
                    )
                })
                .collect();
            v.sort();
            v
        };
        assert_eq!(key(&a), key(&b), "{name}");
    }
}

#[test]
fn malformed_inputs_are_clean_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty.rdml", Vec::new()),
        ("garbage.rdml", b"PK\x03\x04not a zip at all".to_vec()),
        ("notxml.rdml", b"<rdml version=\"1.3\"><sample".to_vec()),
        (
            "truncated.eds",
            std::fs::read(fixture("sds.eds")).unwrap()[..1000].to_vec(),
        ),
        (
            "truncated.rdml",
            std::fs::read(fixture("small.rdml")).unwrap()[..500].to_vec(),
        ),
        (
            "wrong.rex",
            b"<Experiment><RexHeader>REX</RexHeader><Samples>".to_vec(),
        ),
    ];
    for (name, bytes) in cases {
        let p = dir.path().join(name);
        std::fs::write(&p, &bytes).unwrap();
        let reader: &dyn FormatReader = match Path::new(name).extension().and_then(|e| e.to_str()) {
            Some("eds") => &EdsReader,
            Some("rex") => &RexReader,
            _ => &RdmlReader,
        };
        let err = reader.open(&p).map(|_| ()).unwrap_err();
        assert!(
            matches!(err.exit_code(), 4 | 5),
            "{name}: {err} ({})",
            err.exit_code()
        );
    }
}

/// A small deterministic mutation fuzzer over the fixtures: flipped bytes, truncations and
/// repeated slices must never panic.
#[test]
fn mutations_never_panic() {
    let dir = tempfile::tempdir().unwrap();
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for name in [
        "small.rdml",
        "bare.xml",
        "sds.eds",
        "7500.eds",
        "json.eds",
        "small.rex",
    ] {
        let base = std::fs::read(fixture(name)).unwrap();
        let ext = Path::new(name).extension().unwrap().to_str().unwrap();
        for i in 0..150 {
            let mut b = base.clone();
            match next() % 3 {
                0 => {
                    for _ in 0..=next() % 8 {
                        let at = (next() as usize) % b.len();
                        b[at] ^= (next() % 255 + 1) as u8;
                    }
                }
                1 => b.truncate((next() as usize) % b.len()),
                _ => {
                    let at = (next() as usize) % b.len();
                    let len = ((next() as usize) % 64).min(b.len() - at);
                    let slice = b[at..at + len].to_vec();
                    b.splice(at..at, slice);
                }
            }
            let p = dir.path().join(format!("m{i}.{ext}"));
            std::fs::write(&p, &b).unwrap();
            let reg = registry();
            if let Ok((_, mut ds)) = reg.open(&p) {
                let _ = ds.info();
                let _ = ds.check();
                let _ = ds.entries();
                let _ = ds.vendor_metadata();
                let _ = ds.read_table(0, 0, 50);
                let _ = ds.read_table(1, 0, 50);
                let _ = ds.read_trace(0, 0, 0, 50);
            }
            if let Ok(q) = QpcrDataset::open(&p) {
                let mut r = QpcrReportRequest::default();
                r.compute_cq = true;
                r.standard_curve = true;
                let _ = qpcr_report(&q, &r);
                let _ = export_rdml(&q, &dir.path().join(format!("m{i}.out.rdml")), true);
            }
        }
    }
}

/// SDS text results write "Undetermined" as `Ct` = the cycle count (`sds-ct-at-cycles.eds` is
/// `sds.eds` with well A2's Ct written as `4.0` of the 4 cycles, Amp Status −1, Cq Conf 0).
#[test]
fn sds_ct_at_cycle_count_is_undetermined() {
    let r = report(&fixture("sds-ct-at-cycles.eds"), &{
        let mut q = QpcrReportRequest::default();
        q.compute_cq = true;
        q
    });
    let a1 = r.records.iter().find(|x| x.well == "A1").unwrap();
    assert_eq!(a1.cq, Some(2.5));
    assert_eq!(a1.cq_status, "determined");
    assert_eq!(a1.amp_status.as_deref(), Some("amplified"));
    let a2 = r.records.iter().find(|x| x.well == "A2").unwrap();
    assert_eq!(a2.cq, None);
    assert!(a2.cq_undetermined);
    assert_eq!(a2.cq_status, "undetermined");
    assert_eq!(a2.cq_stored, Some(4.0));
    assert!(a2.flags.iter().any(|f| f == "ct_at_cycle_count"));
    assert_eq!(a2.amp_status.as_deref(), Some("not amplified"));
    assert_eq!(r.cq_counts.determined, 1);
    assert_eq!(r.cq_counts.undetermined, 1);
    let g = r.targets.iter().find(|t| t.target == "G").unwrap();
    assert_eq!(g.counts.undetermined, 1);
    assert_eq!(g.averaged, 1);
    assert_eq!(g.cq_mean, Some(2.5));
    // A2 is omitted in the file (IsOmit): a substitute Cq does not bring it back
    assert_eq!(g.counts.excluded, 1);
    let r2 = report(&fixture("sds-ct-at-cycles.eds"), &{
        let mut q = QpcrReportRequest::default();
        q.undetermined_cq = Some(4.0);
        q
    });
    let g2 = r2.targets.iter().find(|t| t.target == "G").unwrap();
    assert_eq!((g2.averaged, g2.cq_mean), (1, Some(2.5)));
    assert_eq!(r2.undetermined_cq, Some(4.0));
    // the results table codes the status
    let mut ds = QpcrDataset::open(&fixture("sds-ct-at-cycles.eds")).unwrap();
    let info = ds.info().unwrap();
    let t = &info.tables[0];
    let col = t
        .columns
        .iter()
        .position(|c| c.name == "cq_status")
        .unwrap();
    let tab = ds.read_table(0, 0, t.row_count).unwrap();
    let codes: Vec<f64> = tab.columns[col].clone();
    assert!(codes.contains(&0.0) && codes.contains(&1.0), "{codes:?}");
}
