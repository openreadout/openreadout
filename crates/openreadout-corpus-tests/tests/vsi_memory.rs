//! Memory of reading a whole VSI slide plane (`docs/architecture-memory.md`).
//!
//! The reader decodes the tiles of a plane a batch at a time and pastes each batch into the
//! plane. Before the October 2026 deep pass it decoded every tile first, so it held the plane
//! twice: 3.8 GB of RSS for the 1.9 GB level 0 of `figshare28409411-vsi-dotslide`. The heap
//! high-water mark comes from a counting global allocator.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test vsi_memory`
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_core::{FormatReader, PlaneIndex};
use peak_alloc::PeakAlloc;

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

#[test]
fn a_whole_slide_plane_is_held_once() {
    let path = corpus_dir()
        .join("figshare28409411-vsi-dotslide/144105_13p5555kbDAB msAT8 1-20016-03-23.vsi");
    if !path.exists() {
        eprintln!("skipped: {} is not on disk", path.display());
        return;
    }
    let mut ds = openreadout_vsi::VsiReader.open(&path).unwrap();
    PEAK.reset_peak_usage();
    let before = PEAK.current_usage();
    let plane = ds.read_plane(0, PlaneIndex::default()).unwrap();
    let peak = PEAK.peak_usage().saturating_sub(before);
    let p = plane.data.len();
    assert_eq!(p, 31_740 * 20_970 * 3);
    // the plane, one batch of decoded tiles (64 MiB, or one tile per thread) and their
    // compressed bytes
    let allowance = 256 << 20;
    assert!(
        peak <= p + allowance,
        "peak heap {} MiB for a {} MiB plane",
        peak >> 20,
        p >> 20
    );
}
