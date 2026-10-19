//! The synthetic QuantaSoft plate (`oracle/make_qlp_fixture.py`): setup, droplet classes,
//! amplitudes, and clean errors on every truncation.

use std::path::Path;

use openreadout_benchtop::QlpReader;
use openreadout_core::FormatReader;
use openreadout_core::source::Input;

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/qlp-synthetic.qlp");

#[test]
fn reads_the_synthetic_plate() {
    let head = std::fs::read(FIXTURE).unwrap();
    assert!(QlpReader.sniff(&head, Path::new(FIXTURE)).is_some());
    let mut ds = QlpReader.open(Path::new(FIXTURE)).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.format_version.as_deref(), Some("QuantaSoft 1.7.4.0917"));
    let wells = &info.tables[0];
    assert_eq!(wells.row_count, 2, "A01 on both channels; A02 was not read");
    let names: Vec<&str> = wells.columns.iter().map(|c| c.name.as_str()).collect();
    let t = ds.read_table(0, 0, u64::MAX).unwrap();
    let col = |n: &str| &t.columns[names.iter().position(|x| *x == n).unwrap()];
    assert_eq!(col("droplets"), &vec![6.0, 6.0]);
    assert_eq!(col("accepted_droplets"), &vec![5.0, 5.0]);
    // channel 1: classes 2 and 3 are positive; channel 2: classes 3 and 4
    assert_eq!(col("positive_droplets"), &vec![2.0, 2.0]);
    assert_eq!(col("droplets_ch1_neg_ch2_neg"), &vec![2.0, 2.0]);
    let target = names.iter().position(|x| *x == "target").unwrap();
    let cats = wells.columns[target].extra["categories"].as_array().unwrap();
    assert_eq!(cats[t.columns[target][1] as usize], "GeneB");
    let d = ds.read_table(1, 0, u64::MAX).unwrap();
    assert_eq!(d.columns[2], vec![900.0, 9000.0, 9100.0, 950.0, 400.0, 920.0]);
    assert_eq!(d.columns[5], vec![1.0, 2.0, 3.0, 4.0, 0.0, 1.0]);
    // a slice of the droplets
    let s = ds.read_table(1, 2, 2).unwrap();
    assert_eq!(s.columns[5], vec![3.0, 4.0]);
    assert!(ds.check().unwrap().ok);
}

#[test]
fn truncations_give_clean_errors() {
    let full = std::fs::read(FIXTURE).unwrap();
    for n in (0..full.len()).step_by(7) {
        let input = Input::from_bytes("cut.qlp", full[..n].to_vec());
        if let Ok(mut ds) = QlpReader.open_input(&input) {
            let _ = ds.info();
            let _ = ds.read_table(0, 0, u64::MAX);
            let _ = ds.read_table(1, 0, u64::MAX);
            let _ = ds.check();
        }
    }
}
