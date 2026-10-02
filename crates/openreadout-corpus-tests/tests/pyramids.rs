//! Pyramid-level invariants over the whole corpus: for every image a reader reports with more
//! than one resolution level (CZI, SVS/NDPI/OME-TIFF SubIFDs and other TIFF pyramids, VSI/ETS,
//! Imaris, OME-Zarr), the levels must describe one pyramid:
//!
//! - `resolution_levels` lists `pyramid_levels` levels numbered 0, 1, ...; level 0 is the image
//!   (`size_x` × `size_y`, downsampling 1);
//! - sizes never grow and downsampling factors never shrink from one level to the next, and no
//!   level repeats the one before it;
//! - each level's size is at most the level-0 size over its downsampling factors (rounded up,
//!   plus one pixel), and at least 90 % of it (slide scanners leave empty margins out of the
//!   coarser levels);
//! - the downsampling factors map level pixels onto level 0: textured windows of a level (on
//!   the middle Z and T plane) correlate (Pearson r >= 0.8) with the level-0 window at
//!   `position × factor`, block-averaged by the factor, at the best origin within one level
//!   pixel of it (scanners' downsampling may shift a level by part of a pixel);
//! - the coarsest level reads back at exactly its reported size.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test pyramids -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters ids.
#![cfg(feature = "corpus")]
#![allow(
    clippy::float_cmp, // level 0 is downsampled exactly 1
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::path::{Path, PathBuf};

use openreadout_core::{Dataset, Plane, PlaneIndex, Region, Registry};
use serde::Deserialize;

/// Formats whose readers can report pyramids.
const FORMATS: &[&str] = &["czi", "tiff", "vsi", "ims", "ome-zarr", "zip", "lif", "nd2"];
/// Coarsest levels up to this many pixels are read back.
const READ_BACK_PIXELS: u64 = 16_000_000;
/// Side of the level windows correlated with level 0.
const WINDOW: u32 = 16;
/// Lowest accepted correlation between a level window and its block-averaged level-0 window.
const MIN_CORRELATION: f64 = 0.8;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    format: String,
    filename: String,
    #[serde(default)]
    role: String,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
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
}

/// The invariant violations of one image's levels (empty when it has none or they hold).
fn check_levels(im: &openreadout_core::ImageInfo) -> Vec<String> {
    let mut bad = Vec::new();
    let lv = &im.resolution_levels;
    if lv.is_empty() {
        if im.pyramid_levels > 1 {
            bad.push(format!(
                "{} pyramid levels but no resolution_levels",
                im.pyramid_levels
            ));
        }
        return bad;
    }
    if lv.len() != im.pyramid_levels as usize {
        bad.push(format!(
            "{} resolution_levels but pyramid_levels = {}",
            lv.len(),
            im.pyramid_levels
        ));
    }
    let l0 = &lv[0];
    if (l0.size_x, l0.size_y) != (im.size_x, im.size_y)
        || l0.downsample_x != 1.0
        || l0.downsample_y != 1.0
    {
        bad.push(format!(
            "level 0 is {}x{} (downsampling {}, {}), image is {}x{}",
            l0.size_x, l0.size_y, l0.downsample_x, l0.downsample_y, im.size_x, im.size_y
        ));
    }
    for (i, l) in lv.iter().enumerate() {
        if l.level as usize != i {
            bad.push(format!("level at position {i} is numbered {}", l.level));
        }
        if i == 0 {
            continue;
        }
        let p = &lv[i - 1];
        if l.size_x > p.size_x || l.size_y > p.size_y {
            bad.push(format!(
                "level {i} ({}x{}) is larger than level {} ({}x{})",
                l.size_x,
                l.size_y,
                i - 1,
                p.size_x,
                p.size_y
            ));
        }
        if l.downsample_x < p.downsample_x || l.downsample_y < p.downsample_y {
            bad.push(format!(
                "level {i} downsampling ({}, {}) below level {}'s ({}, {})",
                l.downsample_x,
                l.downsample_y,
                i - 1,
                p.downsample_x,
                p.downsample_y
            ));
        }
        if (l.size_x, l.size_y) == (p.size_x, p.size_y) && l.size_z == p.size_z {
            bad.push(format!(
                "level {i} repeats level {} ({}x{})",
                i - 1,
                l.size_x,
                l.size_y
            ));
        }
        for (axis, size0, size, ds) in [
            ("x", im.size_x, l.size_x, l.downsample_x),
            ("y", im.size_y, l.size_y, l.downsample_y),
        ] {
            if !(ds.is_finite() && ds >= 1.0) {
                bad.push(format!("level {i} downsample_{axis} = {ds}"));
                continue;
            }
            let expect = f64::from(size0) / ds;
            if f64::from(size) > expect.ceil() + 1.0 || f64::from(size) < 0.9 * expect {
                bad.push(format!(
                    "level {i} {axis}: {size} px, but level 0 ({size0}) / {ds} = {expect:.1}"
                ));
            }
        }
    }
    bad
}

/// Samples of a plane as `f64`, the samples of a pixel averaged (`None` for other types).
fn gray(p: &Plane) -> Option<Vec<f64>> {
    use openreadout_core::PixelType as P;
    let d = &p.data;
    let v: Vec<f64> = match p.pixel_type {
        P::Uint8 => d.iter().map(|&b| f64::from(b)).collect(),
        P::Uint16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(u16::from_le_bytes(*c)))
            .collect(),
        P::Int16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(i16::from_le_bytes(*c)))
            .collect(),
        P::Float => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(f32::from_le_bytes(*c)))
            .collect(),
        _ => return None,
    };
    let spp = p.samples_per_pixel.max(1) as usize;
    Some(
        v.chunks_exact(spp)
            .map(|c| c.iter().sum::<f64>() / spp as f64)
            .collect(),
    )
}

fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (ma, mb) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        sab += (x - ma) * (y - mb);
        saa += (x - ma) * (x - ma);
        sbb += (y - mb) * (y - mb);
    }
    sab / (saa * sbb).sqrt()
}

/// The highest correlation of a level window `v` (`WINDOW` × `WINDOW`) with the block means
/// (`k` × `k` level-0 pixels) of `b` (`size`, row-major), over block origins within one level
/// pixel of `at`: `(r, dx, dy)`, `None` when no origin fits.
fn best_phase(
    v: &[f64],
    b: &[f64],
    size: (u32, u32),
    at: (u32, u32),
    k: (u32, u32),
) -> Option<(f64, i64, i64)> {
    let (w, h) = (size.0 as usize, size.1 as usize);
    if b.len() < w * h {
        return None;
    }
    // summed-area table, (w + 1) x (h + 1)
    let mut sat = vec![0.0; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0;
        for x in 0..w {
            row += b[y * w + x];
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let sum = |x0: usize, y0: usize, x1: usize, y1: usize| {
        sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0]
            + sat[y0 * (w + 1) + x0]
    };
    let (kx, ky) = (i64::from(k.0), i64::from(k.1));
    let n = i64::from(WINDOW);
    let mut best: Option<(f64, i64, i64)> = None;
    let mut down = vec![0.0; (WINDOW * WINDOW) as usize];
    for dy in -ky..=ky {
        for dx in -kx..=kx {
            let (ox, oy) = (i64::from(at.0) + dx, i64::from(at.1) + dy);
            if ox < 0 || oy < 0 || ox + n * kx > w as i64 || oy + n * ky > h as i64 {
                continue;
            }
            for j in 0..n {
                for i in 0..n {
                    let (x0, y0) = ((ox + i * kx) as usize, (oy + j * ky) as usize);
                    down[(j * n + i) as usize] = sum(x0, y0, x0 + kx as usize, y0 + ky as usize);
                }
            }
            let r = pearson(v, &down);
            if best.is_none_or(|b| r > b.0 || b.0.is_nan()) {
                best = Some((r, dx, dy));
            }
        }
    }
    best
}

/// Do the downsampling factors of a level map it onto level 0? The first level downsampled at
/// least 4 times (else the coarsest): up to three of its most textured `WINDOW`-pixel windows
/// on a 5 × 5 grid are compared with the level-0 window at `position × factor`, block-averaged
/// by the rounded factor, at the best block origin within one level pixel ([`best_phase`]). `Ok(Some((level, lowest r)))`, `Ok(None)` when no window is textured,
/// `Err` when a correlation is below `MIN_CORRELATION`.
fn check_factors(
    ds: &mut dyn Dataset,
    im: &openreadout_core::ImageInfo,
) -> Result<Option<(u32, f64)>, String> {
    let lv = &im.resolution_levels;
    let Some(l) = lv
        .iter()
        .skip(1)
        .find(|l| l.downsample_x >= 4.0)
        .or_else(|| lv.last().filter(|l| l.level > 0))
    else {
        return Ok(None);
    };
    let (kx, ky) = (l.downsample_x.round() as u32, l.downsample_y.round() as u32);
    if kx == 0 || ky == 0 || l.size_x < WINDOW * 2 || l.size_y < WINDOW * 2 || kx > 128 || ky > 128
    {
        return Ok(None);
    }
    // The middle plane of a stack: the first focal plane of a confocal stack is often outside
    // the specimen, camera noise that no downsampling preserves (zenodo14641597-idr6001240,
    // z = 0: mean 10, sd 8, r = 0.29 with the block means of the whole plane).
    // Imaris levels are downsampled in Z too (fewer planes than level 0): those keep plane 0,
    // where both levels start.
    let one = Region {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    };
    let last_z = PlaneIndex {
        c: 0,
        z: im.size_z.saturating_sub(1),
        t: 0,
    };
    let same_z = ds.read_region(im.index, last_z, l.level, one).is_ok();
    let idx = PlaneIndex {
        c: 0,
        z: if same_z { im.size_z / 2 } else { 0 },
        t: im.size_t / 2,
    };
    let mut windows = Vec::new();
    for fy in [0.2, 0.35, 0.5, 0.65, 0.8] {
        for fx in [0.1, 0.3, 0.5, 0.7, 0.9] {
            let x = ((f64::from(l.size_x - WINDOW)) * fx) as u32;
            let y = ((f64::from(l.size_y - WINDOW)) * fy) as u32;
            let r = Region {
                x,
                y,
                width: WINDOW,
                height: WINDOW,
            };
            let p = ds
                .read_region(im.index, idx, l.level, r)
                .map_err(|e| format!("level {} region {r:?}: {e}", l.level))?;
            let Some(v) = gray(&p) else {
                return Ok(None);
            };
            let n = v.len() as f64;
            let m = v.iter().sum::<f64>() / n;
            let sd = (v.iter().map(|a| (a - m) * (a - m)).sum::<f64>() / n).sqrt();
            windows.push((sd, x, y, v));
        }
    }
    windows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut lowest: Option<f64> = None;
    for (sd, x, y, v) in windows.into_iter().take(3) {
        if sd < 4.0 {
            break;
        }
        let x0 = (f64::from(x) * l.downsample_x).round() as u32;
        let y0 = (f64::from(y) * l.downsample_y).round() as u32;
        let (w0, h0) = (WINDOW * kx, WINDOW * ky);
        if x0 + w0 > im.size_x || y0 + h0 > im.size_y {
            continue;
        }
        // Level 0 around the window, one level pixel wider on each side: a scanner's
        // downsampling may shift a level by a fraction of its pixel (Leica SCN400F level 1 of
        // openslide-leica-fluorescence-1 sits 3 level-0 pixels to the left), so the best
        // correlation within one level pixel counts. A wrong factor misplaces the window by
        // its position times the error, far more than one level pixel.
        let (ax, ay) = (x0.saturating_sub(kx), y0.saturating_sub(ky));
        let (bx, by) = ((x0 + w0 + kx).min(im.size_x), (y0 + h0 + ky).min(im.size_y));
        let r = Region {
            x: ax,
            y: ay,
            width: bx - ax,
            height: by - ay,
        };
        let big = ds
            .read_region(im.index, idx, 0, r)
            .map_err(|e| format!("level 0 region {r:?}: {e}"))?;
        let Some(b) = gray(&big) else {
            return Ok(None);
        };
        let best = best_phase(&v, &b, (r.width, r.height), (x0 - ax, y0 - ay), (kx, ky));
        let Some((r, dx, dy)) = best else { continue };
        lowest = Some(lowest.map_or(r, |m: f64| m.min(r)));
        if r.is_nan() || r < MIN_CORRELATION {
            return Err(format!(
                "level {} window at ({x}, {y}) correlates at best r = {r:.3} with level 0 at ({x0}{dx:+}, {y0}{dy:+}) under downsampling ({}, {})",
                l.level, l.downsample_x, l.downsample_y
            ));
        }
    }
    Ok(lowest.map(|r| (l.level, r)))
}

#[test]
fn pyramid_levels_are_consistent() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let reg = registry();
    let (mut files, mut pyramids, mut levels) = (0usize, 0usize, 0usize);
    let mut problems = Vec::new();
    for e in &manifest.file {
        // Held-out files are never development inputs (docs/benchmark/heldout.md).
        if !(e.role.is_empty() || e.role == "input") || !FORMATS.contains(&e.format.as_str()) {
            continue;
        }
        if only.as_ref().is_some_and(|o| !e.id.contains(o.as_str())) {
            continue;
        }
        let path = dir.join(&e.filename);
        if !path.exists() {
            continue;
        }
        let Ok((_, mut ds)) = reg.open(&path) else {
            continue;
        };
        files += 1;
        let Ok(info) = ds.info() else {
            continue;
        };
        let images = info.images;
        for im in &images {
            if im.pyramid_levels <= 1 && im.resolution_levels.len() <= 1 {
                continue;
            }
            pyramids += 1;
            levels += im.resolution_levels.len();
            let sizes: Vec<String> = im
                .resolution_levels
                .iter()
                .map(|l| format!("{}x{}/{}", l.size_x, l.size_y, l.downsample_x))
                .collect();
            println!("  {} image {}: {}", e.id, im.index, sizes.join(" "));
            for b in check_levels(im) {
                problems.push(format!("{} image {}: {b}", e.id, im.index));
            }
            match check_factors(ds.as_mut(), im) {
                Ok(Some((level, r))) => {
                    println!("    level {level} vs level 0: lowest window correlation {r:.3}");
                }
                Ok(None) => println!("    (no textured window to correlate)"),
                Err(b) => problems.push(format!("{} image {}: {b}", e.id, im.index)),
            }
            // The coarsest level reads back at its reported size.
            if let Some(last) = im.resolution_levels.last().filter(|l| l.level > 0)
                && u64::from(last.size_x) * u64::from(last.size_y) <= READ_BACK_PIXELS
            {
                match ds.read_plane_level(im.index, PlaneIndex::default(), last.level) {
                    Ok(p) if (p.width, p.height) == (last.size_x, last.size_y) => {}
                    Ok(p) => problems.push(format!(
                        "{} image {} level {}: read {}x{}, reported {}x{}",
                        e.id, im.index, last.level, p.width, p.height, last.size_x, last.size_y
                    )),
                    Err(err) => problems.push(format!(
                        "{} image {} level {}: read failed: {err}",
                        e.id, im.index, last.level
                    )),
                }
            }
        }
    }
    println!(
        "pyramids: {files} files opened, {pyramids} pyramidal images, {levels} levels checked"
    );
    for p in &problems {
        println!("  {p}");
    }
    assert!(
        problems.is_empty(),
        "{} pyramid invariant violations",
        problems.len()
    );
}
