//! cellSens VSI slides checked against the depositor's own full-resolution export (made by the
//! vendor software, independent of Bio-Formats): where Bio-Formats 8.5.0 places tiles wrongly
//! (a Stream "MIA" stitch with negative tile indices, `docs/provenance/vsi.md` 2026-09-25) the
//! export is the only reference, and where it agrees with Bio-Formats (a slide whose tile grid
//! starts 42 px left of the image) it confirms the placement independently.
//!
//! For each pair the level-0 plane we read must (a) correlate with the export (Pearson r over
//! every 4th row; exports carry lossy compression and a display transform) and (b) differ least
//! from it at zero shift: moving our plane by one pixel in any of the 8 directions must increase
//! the mean absolute difference. (b) is what catches a misplaced tile grid.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test vsi_exports -- --nocapture`
#![cfg(feature = "corpus")]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use std::path::{Path, PathBuf};

use openreadout_core::{FormatReader, PlaneIndex};

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    )
}

/// A grey image as f64 samples (colour images: the mean of R, G, B).
struct Grey {
    w: usize,
    h: usize,
    v: Vec<f64>,
}

fn grey(w: usize, h: usize, spp: usize, bytes: &[u8]) -> Grey {
    let v = bytes
        .chunks_exact(spp)
        .map(|p| p.iter().map(|&b| f64::from(b)).sum::<f64>() / spp as f64)
        .collect();
    Grey { w, h, v }
}

fn decode_export(p: &Path) -> Grey {
    let data = std::fs::read(p).unwrap();
    if data.starts_with(b"\x89PNG") {
        let dec = png::Decoder::new(std::io::Cursor::new(&data));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0u8; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!(info.bit_depth, png::BitDepth::Eight);
        let spp = info.color_type.samples();
        grey(
            info.width as usize,
            info.height as usize,
            spp,
            &buf[..info.buffer_size()],
        )
    } else {
        let r = openreadout_codecs::jpeg_decode(&data).unwrap();
        grey(
            r.width as usize,
            r.height as usize,
            r.channels as usize,
            &r.data,
        )
    }
}

/// Mean |ours(x, y) - export(x + dx, y + dy)| over every 4th row, 8 px away from the edges.
fn mean_abs(a: &Grey, b: &Grey, dx: i64, dy: i64) -> f64 {
    let (mut s, mut n) = (0.0, 0u64);
    for y in (8..a.h - 8).step_by(4) {
        let yb = (y as i64 + dy) as usize;
        for x in 8..a.w - 8 {
            let xb = (x as i64 + dx) as usize;
            s += (a.v[y * a.w + x] - b.v[yb * b.w + xb]).abs();
            n += 1;
        }
    }
    s / n as f64
}

fn pearson(a: &Grey, b: &Grey) -> f64 {
    let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for y in (0..a.h).step_by(4) {
        for x in 0..a.w {
            let (u, v) = (a.v[y * a.w + x], b.v[y * b.w + x]);
            sa += u;
            sb += v;
            saa += u * u;
            sbb += v * v;
            sab += u * v;
            n += 1.0;
        }
    }
    let cov = sab / n - sa / n * sb / n;
    cov / ((saa / n - (sa / n).powi(2)).sqrt() * (sbb / n - (sb / n).powi(2)).sqrt())
}

#[test]
fn vsi_level0_matches_depositor_export() {
    let dir = corpus_dir();
    // (dataset, export, lowest accepted correlation)
    let pairs = [
        (
            "figshare30384007-vsi-spleen/MBF_EXP_spleen_prussian_717.vsi",
            "figshare30384007-vsi-spleen/MBF_EXP_spleen_prussian_717.png",
            0.9,
        ),
        (
            "figshare27677802-vsi-stitch/stitch.vsi",
            "figshare27677802-vsi-stitch/stitch.jpg",
            0.9,
        ),
    ];
    let mut checked = 0;
    for (vsi, export, min_r) in pairs {
        let (vp, ep) = (dir.join(vsi), dir.join(export));
        if !vp.exists() || !ep.exists() {
            println!("skip  {vsi} (not downloaded)");
            continue;
        }
        let mut ds = openreadout_vsi::VsiReader.open(&vp).unwrap();
        let p = ds.read_plane(0, PlaneIndex::default()).unwrap();
        let ours = grey(
            p.width as usize,
            p.height as usize,
            p.samples_per_pixel as usize,
            &p.data,
        );
        let theirs = decode_export(&ep);
        assert_eq!(
            (ours.w, ours.h),
            (theirs.w, theirs.h),
            "{vsi}: size differs from the export"
        );
        let r = pearson(&ours, &theirs);
        let at0 = mean_abs(&ours, &theirs, 0, 0);
        let mut worst_neighbour = f64::INFINITY;
        for dy in -1..=1 {
            for dx in -1..=1 {
                if (dx, dy) != (0, 0) {
                    worst_neighbour = worst_neighbour.min(mean_abs(&ours, &theirs, dx, dy));
                }
            }
        }
        println!(
            "{vsi}: r = {r:.4}, mean |diff| {at0:.3} at zero shift, >= {worst_neighbour:.3} one pixel off"
        );
        assert!(r >= min_r, "{vsi}: correlation {r} with the export");
        assert!(
            at0 < worst_neighbour,
            "{vsi}: the export aligns better one pixel away ({worst_neighbour} < {at0}): tiles are misplaced"
        );
        checked += 1;
    }
    println!("{checked} slide(s) checked against their exports");
}
