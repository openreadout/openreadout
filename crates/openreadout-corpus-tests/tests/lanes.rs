//! Lanes and bands (`stats` lanes) on real gels and blots, against what the pictures show: the
//! number of lanes, the bands of a loading-control blot, and the order of band volumes where it
//! is plain to see. No reference program measures these files, so the expected values were read
//! off the images (`preview --marks lanes`); the test catches a change that finds other lanes or
//! bands.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --test lanes`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`.
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_core::measure::{Lanes, LanesRequest, MeasureRequest, measure};

#[path = "support/registry.rs"]
mod registry;

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

/// Lanes of a corpus file, or `None` when it is not on disk.
fn lanes(name: &str, req: LanesRequest) -> Option<Lanes> {
    let path = corpus_dir().join(name);
    if !path.exists() {
        eprintln!("skip {name}: not on disk");
        return None;
    }
    let reg = registry::registry();
    let (_, mut ds) = reg.open(&path).expect("opens");
    let info = ds.info().expect("info");
    let m = MeasureRequest {
        lanes: Some(req),
        ..MeasureRequest::default()
    };
    Some(
        measure(ds.as_mut(), &info, &m)
            .expect("measures")
            .lanes
            .expect("lanes"),
    )
}

#[test]
fn a_tubulin_loading_control_has_one_band_per_lane() {
    // four lanes of tubulin, one band each at the same height, of similar strength
    let Some(l) = lanes("scn-zenodo6723849-tubulin.scn", LanesRequest::default()) else {
        return;
    };
    assert_eq!(l.count, 4, "{l:#?}");
    for lane in &l.lanes {
        assert_eq!(
            lane.bands.len(),
            1,
            "lane {}: {:?}",
            lane.number,
            lane.bands
        );
        assert!(
            (lane.bands[0].y - 177.0).abs() < 10.0,
            "{:?}",
            lane.bands[0]
        );
        assert!(lane.bands[0].relative > 0.8, "{:?}", lane.bands[0]);
    }
}

#[test]
fn a_membrane_strip_has_five_lanes_and_no_edge_bands() {
    // five lanes on a strip of membrane; its bright edges (rows 190 and 340) are not bands
    let Some(l) = lanes("scn-zenodo21792158-porcelli.scn", LanesRequest::default()) else {
        return;
    };
    assert_eq!(l.count, 5, "{l:#?}");
    let strongest = l
        .lanes
        .iter()
        .flat_map(|x| &x.bands)
        .max_by(|a, b| a.volume.total_cmp(&b.volume))
        .expect("bands");
    assert!((strongest.y - 258.0).abs() < 10.0, "{strongest:?}");
    for lane in &l.lanes {
        assert!(
            lane.bands.iter().all(|b| b.y > 192.0),
            "lane {}: {:?}",
            lane.number,
            lane.bands
        );
    }
}

#[test]
fn a_dna_gel_leaves_out_its_wells() {
    // nine loaded lanes; the wells at the top glow in every lane and are not bands
    let Some(l) = lanes("scn-zenodo11355396-geldoc.scn", LanesRequest::default()) else {
        return;
    };
    assert_eq!(l.count, 9, "{l:#?}");
    assert!(
        l.lanes.iter().flat_map(|x| &x.bands).all(|b| b.y > 60.0),
        "{l:#?}"
    );
}

#[test]
fn a_lane_count_is_respected() {
    let req = LanesRequest {
        count: Some(3),
        ..LanesRequest::default()
    };
    let Some(l) = lanes("scn-zenodo7977915-ncbp2.scn", req) else {
        return;
    };
    assert_eq!(l.count, 3, "{l:#?}");
}
