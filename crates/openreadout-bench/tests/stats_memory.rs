//! Memory ceiling of `stats` on a whole-slide plane (`docs/architecture-memory.md`).
//!
//! `stats` reads a large plane of a tiled level in full-width strips and accumulates each
//! strip as it arrives, so its peak heap depends on the strip size and the thread count, not
//! on the plane. This test computes statistics of a synthetic 256 MiB plane stored in tiles
//! (a stand-in for a whole-slide level 0, which can be gigabytes) and asserts that
//! 1. the peak heap stays far below the plane (at most 96 MiB on 1 and 4 threads), and
//! 2. the result equals the statistics of the same plane read whole.
//!
//! The heap high-water mark comes from a counting global allocator ([`peak_alloc`]), as in
//! `memory_ceiling.rs`.

use openreadout_core::model::{CheckReport, FileInfo, ImageInfo, LsEntry};
use openreadout_core::parallel::ReadContext;
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::region::{Region, ResolutionLevel};
use openreadout_core::stats::{Accumulator, HistogramScale, StatsRequest, compute_stats};
use openreadout_core::{Error, PixelType, Plane, ProvenanceMap, Result};
use peak_alloc::PeakAlloc;

#[global_allocator]
static PEAK: PeakAlloc = PeakAlloc;

/// 16384 x 16384 uint8 = 256 MiB, stored in 512 x 512 tiles, with a half-size level so it
/// reads as a pyramid.
const SIDE: u32 = 16_384;
const TILE: u32 = 512;

/// The sample at (x, y): varied enough that every byte value occurs.
fn sample(x: u32, y: u32) -> u8 {
    (x.wrapping_mul(7) ^ y.wrapping_mul(13)).wrapping_add(x / 97) as u8
}

#[derive(Clone)]
struct TiledSlide;

impl Dataset for TiledSlide {
    fn info(&self) -> Result<FileInfo> {
        let mut im = ImageInfo::new(0, SIDE, SIDE, PixelType::Uint8);
        im.pyramid_levels = 2;
        im.resolution_levels = vec![
            ResolutionLevel::new(0, SIDE, SIDE, SIDE, SIDE).with_tile(TILE, TILE),
            ResolutionLevel::new(1, SIDE / 2, SIDE / 2, SIDE, SIDE).with_tile(TILE, TILE),
        ];
        let im = im.finish();
        let mut format = openreadout_bench::SyntheticImage::new(1, 1, 1, 1, 1)
            .info()?
            .format;
        format.id = "synthetic-slide".into();
        Ok(FileInfo {
            path: "synthetic-slide".into(),
            size_bytes: u64::from(SIDE) * u64::from(SIDE),
            format,
            format_version: None,
            plane_count: im.plane_count,
            images: vec![im],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes: Vec::new(),
        })
    }
    fn vendor_metadata(&self) -> Result<serde_json::Value> {
        Ok(serde_json::Value::Null)
    }
    fn provenance(&self) -> ProvenanceMap {
        ProvenanceMap::new()
    }
    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(Vec::new())
    }
    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        self.read_region(image, index, 0, Region::full(SIDE, SIDE))
    }
    fn read_region(
        &mut self,
        _image: u32,
        _index: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        if level != 0 {
            return Err(Error::Other("only level 0 is synthesized".into()));
        }
        let mut data = Vec::with_capacity(region.width as usize * region.height as usize);
        for y in region.y..region.y + region.height {
            data.extend((region.x..region.x + region.width).map(|x| sample(x, y)));
        }
        Ok(Plane {
            width: region.width,
            height: region.height,
            pixel_type: PixelType::Uint8,
            samples_per_pixel: 1,
            data,
        })
    }
    fn check(&mut self) -> Result<CheckReport> {
        Ok(CheckReport::new("synthetic", "synthetic"))
    }
}

fn mib(b: usize) -> f64 {
    b as f64 / f64::from(1u32 << 20)
}

/// One test function: the allocator counters are process-wide.
#[test]
fn stats_memory_is_bounded_by_strips_not_plane() {
    let plane = SIDE as usize * SIDE as usize;
    let mut ds = TiledSlide;
    let info = ds.info().unwrap();
    let mut req = StatsRequest::default();
    req.bins = 16;
    let mut peaks = Vec::new();
    let mut results = Vec::new();
    for threads in [1usize, 4] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        let opener = || -> Result<Box<dyn Dataset>> { Ok(Box::new(TiledSlide)) };
        let out = pool.install(|| {
            let ctx = ReadContext {
                opener: Some(&opener),
                progress: None,
            };
            PEAK.reset_peak_usage();
            let base = PEAK.current_usage();
            let out = compute_stats(&mut ds, &info, &req, &ctx).unwrap();
            peaks.push(PEAK.peak_usage().saturating_sub(base));
            out
        });
        eprintln!(
            "stats, {threads} thread(s): peak heap {:.1} MiB for a {:.0} MiB plane",
            mib(*peaks.last().unwrap()),
            mib(plane)
        );
        results.push(serde_json::to_string(&out.images[0].stats).unwrap());
    }
    for (p, threads) in peaks.iter().zip([1, 4]) {
        assert!(
            *p <= 96 << 20,
            "stats peak heap {p} B on {threads} thread(s) exceeds 96 MiB (plane {plane} B)"
        );
    }
    // The same plane read whole gives the same statistics.
    let whole = ds.read_plane(0, PlaneIndex::default()).unwrap();
    let expected =
        serde_json::to_string(&Accumulator::from_plane(&whole).finish(16, HistogramScale::Linear))
            .unwrap();
    for r in &results {
        assert_eq!(r, &expected);
    }
}
