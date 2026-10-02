//! Shimadzu LabSolutions signals against chromConverter (GPL-3.0, run as a black box by
//! `oracle/shimadzu_oracle.py`; committed results in `corpus/oracle/shimadzu/`): the
//! chromatogram it returns (a `Chromatogram ChN` stream, or the PDA max plot) must equal our
//! trace of the same name value for value, and the PDA field must peak at the max plot
//! (`check`).
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test shimadzu_signals -- --nocapture`
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_core::FormatReader;
use openreadout_core::model::Severity;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

#[test]
#[allow(clippy::many_single_char_names)] // p = path, o = oracle, c = chromatogram, t = trace, n = points
fn chromatograms_match_chromconverter() {
    let mut compared = 0;
    for entry in std::fs::read_dir(root().join("corpus/oracle/shimadzu")).unwrap() {
        let p = entry.unwrap().path();
        let o: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let file = corpus_dir().join(o["file"].as_str().unwrap());
        if !file.exists() {
            eprintln!("skip: {} missing", file.display());
            continue;
        }
        let Some(c) = o["chromatogram"].as_object() else {
            continue;
        };
        let mut ds = openreadout_chrom::ShimadzuReader.open(&file).unwrap();
        let info = ds.info().unwrap();
        let name = c["name"].as_str().unwrap();
        let t = info
            .traces
            .iter()
            .find(|t| {
                t.name.as_deref() == Some(name)
                    || t.extra.get("stream_label").and_then(|v| v.as_str()) == Some(name)
            })
            .unwrap_or_else(|| panic!("{}: no trace named {name}", p.display()));
        let n = c["n"].as_u64().unwrap();
        // chromatograms carry the vendor's leading t = 0 point, which chromConverter does not,
        // and are scaled to the export's units (`extra.raw_scale`)
        let stored = t
            .extra
            .get("stored_points")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(t.sample_count);
        assert_eq!(stored, n, "{name}: point count");
        let raw_scale = t.extra.get("raw_scale").and_then(serde_json::Value::as_f64);
        let tr = ds
            .read_trace(t.index, 0, t.sample_count - stored, n)
            .unwrap();
        let bytes: Vec<u8> = tr.channels[0]
            .iter()
            .map(|v| raw_scale.map_or(*v, |k| (v / k).round()))
            .flat_map(f64::to_le_bytes)
            .collect();
        let hash = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
        assert_eq!(
            hash,
            c["xxh3"].as_str().unwrap(),
            "{}: {name} values differ",
            p.display()
        );
        let rep = ds.check().unwrap();
        assert!(
            !rep.findings.iter().any(|f| f.severity == Severity::Error),
            "{}: {:?}",
            p.display(),
            rep.findings
        );
        println!(
            "{:<45} {name}: {n} points bit-exact against chromConverter; check ok",
            o["id"].as_str().unwrap()
        );
        compared += 1;
    }
    assert!(
        compared > 0
            || !corpus_dir()
                .join("zenodo17868549-gp070190p-hplc.lcd")
                .exists()
    );
}
