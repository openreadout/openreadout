//! `table --filter … --count` against FlowIO + NumPy (`oracle/fcs_filter.py` →
//! `corpus/oracle/flow/filter-counts.json`): raw, compensated and arcsinh-transformed values.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test fcs_filter -- --nocapture`
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_fcs::analysis::{
    CompensationChoice, TableOptions, TransformChoice, parse_transform_spec,
};
use openreadout_fcs::filter::{filtered_slice, parse_conditions};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

#[test]
fn filter_counts_match_flowio_and_numpy() {
    let oracle: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("corpus/oracle/flow/filter-counts.json")).unwrap(),
    )
    .unwrap();
    let reg = openreadout_core::Registry::new().with(Box::new(openreadout_fcs::FcsReader));
    let mut compared = 0;
    for j in oracle["jobs"].as_array().unwrap() {
        let id = j["id"].as_str().unwrap();
        let path = corpus_dir().join(format!("{id}.fcs"));
        if !path.exists() {
            eprintln!("skip: {} missing", path.display());
            continue;
        }
        let conds: Vec<String> = j["conditions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let mut o = TableOptions::default();
        match j["values"].as_str().unwrap() {
            "raw" => {}
            "compensated" => o.compensation = Some(CompensationChoice::File),
            "arcsinh" => {
                o.compensation = Some(CompensationChoice::File);
                let k = j["cofactor"].as_f64().unwrap();
                o.transform = Some(TransformChoice::Uniform {
                    transform: parse_transform_spec(&format!("arcsinh-cofactor:{k}")).unwrap(),
                    parameters: Vec::new(),
                });
            }
            other => panic!("values {other}"),
        }
        let s =
            filtered_slice(&reg, &path, 0, 0, 0, &o, &parse_conditions(&conds).unwrap()).unwrap();
        let f = s.filter.unwrap();
        println!(
            "{id} {} {conds:?}: {} of {} (FlowIO + NumPy: {} of {})",
            j["values"], f.matched_rows, f.total_rows, j["matched"], j["total"]
        );
        assert_eq!(
            f.matched_rows,
            j["matched"].as_u64().unwrap(),
            "{id} {conds:?}"
        );
        assert_eq!(f.total_rows, j["total"].as_u64().unwrap());
        assert!(s.rows.is_empty(), "count only");
        compared += 1;
    }
    if compared == 0 {
        eprintln!("skip: no FCS corpus files");
    }
}

#[test]
fn filtered_rows_page_through_the_matches() {
    let path = corpus_dir().join("flowio-3fitc-4pe-004.fcs");
    if !path.exists() {
        return;
    }
    let reg = openreadout_core::Registry::new().with(Box::new(openreadout_fcs::FcsReader));
    let conds = parse_conditions(&["FL1-H > 100 && FL2-H <= 50".into()]).unwrap();
    let o = TableOptions::default();
    let all = filtered_slice(&reg, &path, 0, 0, 1000, &o, &conds).unwrap();
    let n = all.filter.as_ref().unwrap().matched_rows;
    assert_eq!(all.rows.len() as u64, n);
    assert!(!all.truncated);
    for r in &all.rows {
        assert!(r[2] > 100.0 && r[3] <= 50.0);
    }
    let page = filtered_slice(&reg, &path, 0, 10, 5, &o, &conds).unwrap();
    assert_eq!(page.rows, all.rows[10..15].to_vec());
    assert!(page.truncated);
    assert!(parse_conditions(&["NOPE > 1".into()]).is_ok());
    assert!(
        filtered_slice(
            &reg,
            &path,
            0,
            0,
            1,
            &o,
            &parse_conditions(&["NOPE > 1".into()]).unwrap()
        )
        .is_err()
    );
}
