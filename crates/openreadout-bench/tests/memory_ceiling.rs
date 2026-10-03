//! Memory ceiling of image export (book/src/project/performance.md, "Memory ceiling").
//!
//! Export holds a bounded number of decoded planes in memory whatever the size of the file:
//! peak heap is a function of the plane size and the thread count, not of the plane count.
//! This test exports synthetic images of 16 and 64 planes (the larger one is 4x the data, and
//! both span several decode windows, so both reach the steady state) and asserts that
//! 1. the peak heap of the large export exceeds the small one's by at most one decode window
//!    (`2 * threads` planes): how full the window gets in the small export depends on thread
//!    timing, but an export that kept every plane would hold all 64, and
//! 2. the peak heap stays below the documented formula, `(2 * threads + 4) * plane_bytes`
//!    plus a fixed allowance for metadata and I/O buffers.
//!
//! The heap high-water mark comes from a counting global allocator ([`peak_alloc`]); it
//! measures anonymous memory the process allocates, which is what a large export could run
//! out of (file pages read through the OS cache are reclaimable and not counted).

use std::path::Path;

use openreadout_bench::{SyntheticImage, tiff_options, zarr_options};
use openreadout_core::Result;
use openreadout_core::parallel::ReadContext;
use openreadout_core::reader::Dataset;
use peak_alloc::PeakAlloc;

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

/// Metadata, OME-XML, TIFF/Zarr writer state, I/O buffers.
const FIXED_ALLOWANCE: usize = 16 << 20;

fn export_peak(ds: &SyntheticImage, threads: usize, zarr: bool, dir: &Path) -> usize {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .unwrap();
    let out = dir.join(if zarr { "out.ome.zarr" } else { "out.ome.tiff" });
    let template = ds.clone();
    let opener = move || -> Result<Box<dyn Dataset>> { Ok(Box::new(template.clone())) };
    let mut primary = ds.clone();
    pool.install(|| {
        let ctx = ReadContext {
            opener: Some(&opener),
            progress: None,
        };
        PEAK.reset_peak_usage();
        let base = PEAK.current_usage();
        if zarr {
            openreadout_omezarr::export_ome_zarr_with(
                &mut primary,
                Path::new("synthetic"),
                &out,
                &zarr_options(openreadout_ometiff::Codec::None, Some(1)),
                &ctx,
            )
            .unwrap();
        } else {
            openreadout_ometiff::export_ome_tiff_with(
                &mut primary,
                Path::new("synthetic"),
                &out,
                &tiff_options(openreadout_ometiff::Codec::None),
                &ctx,
            )
            .unwrap();
        }
        PEAK.peak_usage().saturating_sub(base)
    })
}

fn mib(b: usize) -> f64 {
    b as f64 / f64::from(1u32 << 20)
}

fn check(zarr: bool) {
    let dir = tempfile::tempdir().unwrap();
    // 1536 x 1536 uint16 = 4.5 MiB per plane; 64 planes = 288 MiB of pixels.
    let small = SyntheticImage::new(1536, 1536, 4, 4, 1);
    let large = SyntheticImage::new(1536, 1536, 4, 4, 4);
    let plane = small.plane_bytes() as usize;
    for threads in [1usize, 4] {
        let p_small = export_peak(&small, threads, zarr, dir.path());
        let p_large = export_peak(&large, threads, zarr, dir.path());
        let ceiling = (2 * threads + 4) * plane + FIXED_ALLOWANCE;
        let what = if zarr { "OME-Zarr" } else { "OME-TIFF" };
        eprintln!(
            "{what}, {threads} thread(s): peak heap {:.1} MiB for 16 planes, {:.1} MiB for 64 \
             planes (plane {:.1} MiB, ceiling {:.1} MiB)",
            mib(p_small),
            mib(p_large),
            mib(plane),
            mib(ceiling)
        );
        assert!(
            p_large <= p_small + 2 * threads * plane,
            "{what} peak heap grows with the plane count: {p_small} B for 16 planes, {p_large} B for 64"
        );
        assert!(
            p_large <= ceiling,
            "{what} peak heap {p_large} B exceeds the documented ceiling {ceiling} B"
        );
    }
}

/// One test function: the allocator counters are process-wide, so the exports must not run
/// concurrently with anything else in this binary.
#[test]
fn export_memory_is_bounded_by_plane_size() {
    check(false);
    check(true);
}
