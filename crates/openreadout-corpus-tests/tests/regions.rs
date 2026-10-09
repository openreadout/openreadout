//! Region and pyramid-level reads against third-party ground truth
//! (`corpus/oracle/regions/<id>.json`, written by `oracle/gen_regions.py` with czifile,
//! tifffile + zarr, h5py, zarr-python, liffile and Bio-Formats as a black box).
//!
//! For every recorded rectangle, `Dataset::read_region` must return the oracle's pixels:
//! bit-exact (xxh3) for lossless data; for lossy codecs (JPEG tiles decode slightly differently in
//! every implementation) the mean of each cell of an 8 × 8 grid over the region must agree within
//! [`LOSSY_CELL_TOLERANCE`], which still catches a misplaced or missing tile. The level size must
//! equal the oracle's, and the region must equal the same rectangle cropped from a whole-level
//! read wherever the whole level is readable.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test regions -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]

use std::path::{Path, PathBuf};

use openreadout_core::region::{crop, level_size};
use openreadout_core::{Plane, PlaneIndex, Region, Registry};
use serde::Deserialize;

/// Largest difference of an 8 × 8 grid cell mean (sample units) accepted for lossy files.
const LOSSY_CELL_TOLERANCE: f64 = 1.5;
/// Whole levels up to this many pixels are also read to cross-check region = crop(level).
const CROSS_CHECK_PIXELS: u64 = 40_000_000;

#[derive(Deserialize)]
struct OracleFile {
    id: String,
    file: String,
    reader: String,
    lossy: bool,
    regions: Vec<OracleRegion>,
}

#[derive(Deserialize)]
struct OracleRegion {
    image: u32,
    level: u32,
    level_size: [u32; 2],
    c: u32,
    z: u32,
    t: u32,
    region: [u32; 4],
    xxh3: String,
    /// A second reader's hash where it disagrees with the first (see `alt_reader`).
    #[serde(default)]
    xxh3_alt: Option<String>,
    #[serde(default)]
    alt_reader: Option<String>,
    /// Why the oracle's value here is not taken as ground truth (reported, not failed).
    #[serde(default)]
    disputed: Option<String>,
    mean: f64,
    grid: Vec<Vec<Option<f64>>>,
}

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

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_czi::CziReader))
        .with(Box::new(openreadout_lif::LifReader))
        .with(Box::new(openreadout_nd2::Nd2Reader))
        .with(Box::new(openreadout_vsi::VsiReader))
        .with(Box::new(openreadout_wsi::MiraxReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_zarr::ZarrReader))
        .with(Box::new(openreadout_hdf5::ImsReader))
        .with(Box::new(openreadout_hcs::HarmonyReader))
}

/// All samples of a plane as f64, interleaved as stored.
fn values(p: &Plane) -> Vec<f64> {
    use openreadout_core::PixelType as P;
    let d = &p.data;
    match p.pixel_type {
        P::Uint8 => d.iter().map(|&b| f64::from(b)).collect(),
        P::Int8 => d.iter().map(|&b| f64::from(b as i8)).collect(),
        P::Uint16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(u16::from_le_bytes([c[0], c[1]])))
            .collect(),
        P::Int16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(i16::from_le_bytes([c[0], c[1]])))
            .collect(),
        P::Uint32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect(),
        P::Int32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect(),
        P::Float => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
            .collect(),
        P::Double => d
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]))
            .collect(),
        _ => Vec::new(),
    }
}

/// Mean of each cell of an 8 × 8 grid (the oracle's `summary()["grid"]`: cell edges at
/// `linspace(0, n, 9)` truncated to integers, every sample of the cell).
#[allow(clippy::many_single_char_names)] // w, h, s, x, y: image width, height, samples, coordinates
fn grid(p: &Plane) -> Vec<Vec<f64>> {
    let v = values(p);
    let (w, h, s) = (
        p.width as usize,
        p.height as usize,
        p.samples_per_pixel.max(1) as usize,
    );
    let edges = |n: usize| -> Vec<usize> { (0..=8).map(|i| (i * n) / 8).collect() };
    let (ys, xs) = (edges(h), edges(w));
    let mut out = Vec::new();
    for i in 0..8 {
        let mut row = Vec::new();
        for j in 0..8 {
            let (y0, y1) = (ys[i], ys[i + 1].max(ys[i] + 1).min(h));
            let (x0, x1) = (xs[j], xs[j + 1].max(xs[j] + 1).min(w));
            let (mut sum, mut n) = (0.0, 0usize);
            for y in y0..y1 {
                for x in x0..x1 {
                    for k in 0..s {
                        sum += v[(y * w + x) * s + k];
                        n += 1;
                    }
                }
            }
            row.push(if n > 0 { sum / n as f64 } else { f64::NAN });
        }
        out.push(row);
    }
    out
}

#[test]
fn regions_match_oracles() {
    let dir = root().join("corpus/oracle/regions");
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = registry();
    let mut files = 0usize;
    let (mut exact, mut within, mut cross, mut strips_equal) = (0usize, 0usize, 0usize, 0usize);
    let (mut alt_exact, mut disputed) = (0usize, Vec::new());
    let mut worst = 0f64;
    let mut problems = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    entries.sort();
    for p in entries
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let o: OracleFile = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        if only.as_deref().is_some_and(|s| !o.id.contains(s)) {
            continue;
        }
        let path = corpus_dir().join(&o.file);
        if !path.exists() {
            eprintln!("skip {}: {} not in the corpus", o.id, o.file);
            continue;
        }
        files += 1;
        let (_, mut ds) = reg
            .open(&path)
            .unwrap_or_else(|e| panic!("{}: open: {e}", o.id));
        let info = ds.info().unwrap();
        let mut whole: Option<((u32, u32, PlaneIndex), Plane)> = None;
        for r in &o.regions {
            let what = format!(
                "{} image {} level {} c={} z={} t={} region {:?}",
                o.id, r.image, r.level, r.c, r.z, r.t, r.region
            );
            let im = info.images.iter().find(|i| i.index == r.image).unwrap();
            match level_size(im, r.level) {
                Ok(size) if size == (r.level_size[0], r.level_size[1]) => {}
                other => problems.push(format!(
                    "{what}: level size {other:?} != oracle {:?}",
                    r.level_size
                )),
            }
            let idx = PlaneIndex {
                c: r.c,
                z: r.z,
                t: r.t,
            };
            let region = Region::new(r.region[0], r.region[1], r.region[2], r.region[3]);
            let got = match ds.read_region(r.image, idx, r.level, region) {
                Ok(p) => p,
                Err(e) => {
                    problems.push(format!("{what}: read failed: {e}"));
                    continue;
                }
            };
            if (got.width, got.height) != (region.width, region.height) {
                problems.push(format!("{what}: {}x{} returned", got.width, got.height));
                continue;
            }
            // A reader that reads strips itself (`stats` on large CZI planes) returns the same
            // pixels for the region cut into three strips.
            if ds.reads_strips(r.image, r.level) && region.height >= 3 {
                let third = region.height / 3;
                let strips = [
                    Region::new(region.x, region.y, region.width, third),
                    Region::new(region.x, region.y + third, region.width, third),
                    Region::new(
                        region.x,
                        region.y + 2 * third,
                        region.width,
                        region.height - 2 * third,
                    ),
                ];
                let mut data = Vec::with_capacity(got.data.len());
                let read = ds.read_strips(r.image, idx, r.level, &strips, &mut |p| {
                    data.extend_from_slice(&p.data);
                    Ok(())
                });
                match read {
                    Ok(()) if data == got.data => strips_equal += 1,
                    Ok(()) => problems.push(format!("{what}: read_strips differs from the region")),
                    Err(e) => problems.push(format!("{what}: read_strips failed: {e}")),
                }
            }
            let h = got.xxh3_hex();
            if h == r.xxh3 {
                exact += 1;
            } else if r.xxh3_alt.as_deref() == Some(h.as_str()) {
                alt_exact += 1;
                eprintln!(
                    "{what}: equals {} (the first reader differs)",
                    r.alt_reader.as_deref().unwrap_or("the second reader")
                );
            } else {
                let g = grid(&got);
                let mut diff = 0f64;
                for (a, b) in g.iter().flatten().zip(r.grid.iter().flatten()) {
                    if let Some(b) = b {
                        diff = diff.max((a - b).abs());
                    }
                }
                if o.lossy && diff <= LOSSY_CELL_TOLERANCE {
                    within += 1;
                    worst = worst.max(diff);
                } else if let Some(why) = &r.disputed {
                    disputed.push(format!("{what}: {why}"));
                    continue;
                } else {
                    problems.push(format!(
                        "{what}: hash differs (lossy: {}), largest grid-cell difference {diff:.3}, mean {:.3} vs {:.3}",
                        o.lossy,
                        values(&got).iter().sum::<f64>() / values(&got).len().max(1) as f64,
                        r.mean
                    ));
                    continue;
                }
            }
            // Region == crop of the whole level, wherever the whole level can be read.
            let (lw, lh) = (r.level_size[0], r.level_size[1]);
            let bpp =
                (im.samples_per_pixel.max(1) as u64) * im.pixel_type.bytes_per_sample() as u64;
            if u64::from(lw) * u64::from(lh) <= CROSS_CHECK_PIXELS && bpp > 0 {
                let key = (r.image, r.level, idx);
                if whole.as_ref().is_none_or(|(k, _)| *k != key) {
                    match ds.read_plane_level(r.image, idx, r.level) {
                        Ok(pl) => whole = Some((key, pl)),
                        Err(e) => {
                            problems.push(format!("{what}: whole level read failed: {e}"));
                            continue;
                        }
                    }
                }
                let (_, pl) = whole.as_ref().unwrap();
                match crop(pl.clone(), region, "level") {
                    Ok(c) if c.data == got.data => cross += 1,
                    Ok(_) => problems.push(format!(
                        "{what}: region differs from the crop of the whole level"
                    )),
                    Err(e) => problems.push(format!("{what}: crop failed: {e}")),
                }
            }
        }
        eprintln!("{}: {} regions ({})", o.id, o.regions.len(), o.reader);
    }
    eprintln!(
        "regions: {files} files, {exact} bit-exact, {alt_exact} bit-exact with the second reader only, {within} within the lossy tolerance (worst cell {worst:.3}), {cross} equal to the crop of the whole level, {strips_equal} equal to the same region read in strips, {} disputed",
        disputed.len()
    );
    for d in &disputed {
        eprintln!("disputed: {d}");
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}
