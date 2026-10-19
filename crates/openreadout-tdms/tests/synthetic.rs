//! The TDMS reader on `tests/fixtures/mini.tdms` (`oracle/make_tdms_fixtures.py`): two segments,
//! two groups, float64 channels with a sample interval, a linear scale, int16, timestamp and
//! string channels. Values known by construction.
#![allow(clippy::float_cmp, clippy::many_single_char_names)] // values known exactly by construction

use std::path::Path;

use openreadout_core::source::Input;
use openreadout_core::{Dataset, FormatReader};
use openreadout_tdms::TdmsReader;

fn fixture() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mini.tdms"
    ))
}

fn values(ds: &mut dyn Dataset, trace: u32, ch: usize) -> Vec<f64> {
    ds.read_trace(trace, 0, 0, 100).unwrap().channels[ch].clone()
}

#[test]
fn reads_groups_channels_and_segments() {
    let head = std::fs::read(fixture()).unwrap();
    assert!(TdmsReader.sniff(&head, fixture()).is_some());
    let mut ds = TdmsReader.open(fixture()).unwrap();
    let info = ds.info().unwrap();
    assert_eq!(info.traces.len(), 2);
    let s = &info.traces[0];
    assert_eq!(s.name.as_deref(), Some("Sensors"));
    assert_eq!(s.sample_count, 8);
    assert_eq!(s.sample_rate_hz, 2.0);
    assert_eq!(s.channels[0].unit.as_deref(), Some("degC"));
    assert_eq!(s.channels[1].scale, 2.0);
    assert_eq!(s.channels[1].offset, 1.0);
    assert_eq!(s.extra["segments"], 2);
    assert_eq!(s.extra["writer"], "openreadout fixture");
    assert_eq!(
        values(&mut *ds, 0, 0),
        (0..8).map(|i| f64::from(i) * 0.25).collect::<Vec<_>>()
    );
    // stored 0..8, scaled 2x + 1
    assert_eq!(
        values(&mut *ds, 0, 1),
        (0..8).map(|i| 2.0 * f64::from(i) + 1.0).collect::<Vec<_>>()
    );
    let l = &info.traces[1];
    assert_eq!(l.channels[0].dtype, "int16");
    assert_eq!(values(&mut *ds, 1, 0)[7], 65.0);
    // 2024-01-02T03:04:05.5Z
    let t = values(&mut *ds, 1, 1);
    assert!((t[0] - 1_704_164_645.5).abs() < 1e-6, "{t:?}");
    let v = ds.vendor_metadata().unwrap();
    assert_eq!(v["objects"]["/'Log'/'notes'"]["value_count"], 8);
    assert_eq!(v["objects"]["/'Log'/'notes'"]["first_values"][5], "note 5");
    let rep = ds.check().unwrap();
    assert!(rep.findings.is_empty(), "{:?}", rep.findings);
    // a window inside the second segment
    let w = ds.read_trace(0, 0, 5, 2).unwrap();
    assert_eq!(w.channels[0], vec![1.25, 1.5]);
}

#[test]
fn damage_is_an_error_not_a_panic() {
    let good = std::fs::read(fixture()).unwrap();
    for cut in (1..good.len()).step_by(5) {
        let input = Input::from_bytes("cut.tdms", good[..cut].to_vec());
        if let Ok(mut ds) = TdmsReader.open_input(&input) {
            if let Ok(info) = ds.info() {
                for t in &info.traces {
                    let _ = ds.read_trace(t.index, 0, 0, t.sample_count);
                }
            }
            let _ = ds.check();
        }
    }
    for i in 0..good.len() {
        let mut b = good.clone();
        b[i] ^= 0xa5;
        let input = Input::from_bytes("flip.tdms", b);
        if let Ok(mut ds) = TdmsReader.open_input(&input) {
            if let Ok(info) = ds.info() {
                for t in &info.traces {
                    let _ = ds.read_trace(t.index, 0, 0, t.sample_count.min(1 << 16));
                }
            }
            let _ = ds.check();
        }
    }
}
