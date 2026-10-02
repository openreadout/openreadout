//! qPCR readers against independent ground truth (`oracle/qpcr.py` →
//! `corpus/oracle/qpcr/<id>.json`):
//!
//! - RDML: rdmlpython (MIT, the RDML consortium's reference implementation): every reaction's
//!   sample, target, dye, task, Cq, "undetermined", Tm, exclusion, and the count and sum of its
//!   amplification and melt points;
//! - `.eds`: the vendor's own result files read with the Python standard library (Cq, undetermined
//!   calls, Tm, Rn/ΔRn/melt sums, sample/target/task per well, cycles and read temperature, the
//!   vendor's ΔCt and RQ, the vendor's standard curve), and qslib (EUPL, black box) for the
//!   per-dye multicomponent signal;
//! - `.rex`: ElementTree sums of the raw readings;
//! - `.pcrd`: detected, refused with exit 6.
//!
//! Every readable file is also exported to RDML and read back; with
//! `OPENREADOUT_ORACLE_PYTHON` set (e.g. `oracle/.venv/bin/python`), the export is validated
//! against the RDML schema by rdmlpython.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test qpcr -- --nocapture`.
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_core::Registry;
use openreadout_qpcr::{QpcrDataset, QpcrReportRequest, qpcr_report};
use serde::Deserialize;
use serde_json::Value;

mod qpcr_oracle;

const FORMATS: [&str; 4] = [
    "rdml",
    "applied-biosystems-eds",
    "bio-rad-pcrd",
    "rotor-gene-rex",
];

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}
#[derive(Deserialize)]
struct Entry {
    id: String,
    format: String,
    filename: String,
    /// `heldout` entries are for measuring only (docs/benchmark/heldout.md): never checked here.
    #[serde(default)]
    tier: String,
    /// Only inputs are read; `oracle-export` entries are the vendor exports the oracle reads.
    #[serde(default)]
    role: String,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_qpcr::RdmlReader))
        .with(Box::new(openreadout_qpcr::EdsReader))
        .with(Box::new(openreadout_qpcr::PcrdReader))
        .with(Box::new(openreadout_qpcr::RexReader))
        .with(Box::new(openreadout_qpcr::IxoReader))
}

fn rdml_roundtrip(id: &str, ds: &QpcrDataset, dir: &Path, errs: &mut Vec<String>) -> String {
    let out = dir.join(format!("{id}.rdml"));
    let rep = match openreadout_qpcr::export_rdml(ds, &out, true) {
        Ok(r) => r,
        Err(e) => {
            errs.push(format!("{id}: RDML export failed: {e}"));
            return String::new();
        }
    };
    let back = QpcrDataset::open(&out).unwrap();
    let a = qpcr_report(ds, &QpcrReportRequest::default()).unwrap();
    let b = qpcr_report(&back, &QpcrReportRequest::default()).unwrap();
    let cqs = |r: &openreadout_qpcr::QpcrReport| -> Vec<(u32, u32, Option<String>, Option<u64>)> {
        let mut v: Vec<_> = r
            .records
            .iter()
            .filter(|x| x.target.is_some())
            .map(|x| (x.row, x.col, x.target.clone(), x.cq.map(f64::to_bits)))
            .collect();
        v.sort();
        v
    };
    if cqs(&a) != cqs(&b) {
        errs.push(format!("{id}: RDML round trip changed the Cq records"));
    }
    let mut s = format!(
        "; RDML export {} reactions, verified={}",
        rep.reactions, rep.verified
    );
    if let Some(py) = std::env::var_os("OPENREADOUT_ORACLE_PYTHON") {
        let o = std::process::Command::new(py)
            .arg(root().join("oracle/qpcr.py"))
            .arg("--validate-rdml")
            .arg(&out)
            .output()
            .unwrap();
        if o.status.success() {
            s.push_str(", rdmlpython schema-valid");
        } else {
            errs.push(format!(
                "{id}: rdmlpython rejects the export: {}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ));
        }
    }
    s
}

#[test]
fn qpcr_against_oracles() {
    let m: Manifest =
        toml::from_str(&std::fs::read_to_string(root().join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let reg = registry();
    let tmp = tempfile::tempdir().unwrap();
    let mut checked = 0;
    let mut failures = Vec::new();
    for e in m.file.iter().filter(|e| {
        FORMATS.contains(&e.format.as_str())
            && e.tier != "heldout"
            && (e.role.is_empty() || e.role == "input")
    }) {
        let path = corpus_dir().join(&e.filename);
        if !path.exists() {
            println!("skip   {:<32} (not downloaded)", e.id);
            continue;
        }
        let (_, det) = reg.detect(&path).unwrap();
        assert_eq!(
            det.format_id, e.format,
            "{}: detected as {}",
            e.id, det.format_id
        );
        if e.format == "bio-rad-pcrd" {
            let err = reg.open(&path).map(|_| ()).unwrap_err();
            assert_eq!(err.exit_code(), 6, "{}: {err}", e.id);
            println!("pass   {:<32} refused with exit 6", e.id);
            checked += 1;
            continue;
        }
        let oracle_path = root().join(format!("corpus/oracle/qpcr/{}.json", e.id));
        let Ok(text) = std::fs::read_to_string(&oracle_path) else {
            println!("skip   {:<32} (no oracle)", e.id);
            continue;
        };
        let oracle: Value = serde_json::from_str(&text).unwrap();
        if let Some(err) = oracle["error"].as_str() {
            println!("oracle-error {:<26} {err}", e.id);
            continue;
        }
        let mut ds = QpcrDataset::open(&path).unwrap_or_else(|err| panic!("{}: {err}", e.id));
        let mut errs = Vec::new();
        let mut detail =
            qpcr_oracle::compare_dataset(&e.format, &e.id, &mut ds, &oracle, &mut errs);
        detail.push_str(&rdml_roundtrip(&e.id, &ds, tmp.path(), &mut errs));
        checked += 1;
        if errs.is_empty() {
            println!("pass   {:<32} {detail}", e.id);
        } else {
            println!("FAIL   {:<32} {detail}", e.id);
            for x in errs.iter().take(12) {
                println!("         {x}");
            }
            failures.push(format!("{}: {} differences", e.id, errs.len()));
        }
    }
    assert!(
        checked > 0,
        "no qPCR corpus files; run `cargo xtask corpus fetch --format rdml` etc."
    );
    assert!(failures.is_empty(), "{failures:#?}");
}
