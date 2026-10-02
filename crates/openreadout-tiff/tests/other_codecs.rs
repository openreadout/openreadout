//! WebP, JPEG XL, LERC and old-style JPEG pages.
//!
//! GDAL's autotest samples (`tests/fixtures/codecs/*.tif`, MIT, see the README there) are
//! decoded and compared sample by sample with `<name>.expected`, written by tifffile +
//! imagecodecs (libwebp, libjxl, Esri's lerc): all must match exactly.
//!
//! The old-style JPEG sample (corpus id `gdal-ojpeg-zackthecat`, not committed; skipped when
//! absent) is compared with Pillow + libtiff through per-channel means and the mean absolute
//! difference measured when this test was written (1.02; 9.5 when ReferenceBlackWhite is
//! ignored). libtiff converts YCbCr itself from libjpeg's sub-sampled chroma, jpeg-decoder
//! interpolates chroma first, so colour edges differ by up to 20 levels.

use std::path::{Path, PathBuf};

use openreadout_tiff::{PageLayout, SampleSelect, TiffFile};

fn decode(p: &Path) -> Result<Vec<u8>, String> {
    let (tf, mut src) = TiffFile::open(p).map_err(|e| e.to_string())?;
    let layout =
        PageLayout::from_ifd(&tf.ifds[0], tf.header.byte_order).map_err(|e| e.to_string())?;
    openreadout_tiff::read_page(&mut src, &layout, SampleSelect::All).map_err(|e| e.to_string())
}

#[test]
fn gdal_samples_match_reference_decoders() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/codecs");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().to_string_lossy().into_owned();
            Path::new(&n)
                .extension()
                .is_some_and(|x| x == "tif")
                .then_some(n)
        })
        .collect();
    names.sort();
    assert!(names.len() >= 20, "fixtures missing");
    let mut failures = Vec::new();
    for name in &names {
        let p = dir.join(name);
        let want = std::fs::read(p.with_extension("expected")).unwrap();
        match decode(&p) {
            Ok(got) if got.len() != want.len() => failures.push(format!(
                "{name}: {} bytes, expected {}",
                got.len(),
                want.len()
            )),
            // LERC is off by default (lerc-rs is not robust; SECURITY.md): an unsupported-feature
            // error unless the `lerc` feature is on.
            Err(e) if name.to_ascii_lowercase().contains("lerc") && !cfg!(feature = "lerc") => {
                assert!(e.contains("unsupported"), "{name}: {e}");
            }
            Err(e) => failures.push(format!("{name}: {e}")),
            Ok(got) if got != want => {
                let n = got.iter().zip(&want).filter(|(a, b)| a != b).count();
                failures.push(format!("{name}: {n} of {} bytes differ", want.len()));
            }
            Ok(_) => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn old_style_jpeg_matches_libtiff() {
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    );
    let p = dir.join("gdal-zackthecat.tif");
    if !p.exists() {
        return;
    }
    let got = decode(&p).unwrap();
    assert_eq!(got.len(), 234 * 213 * 3);
    // Pillow 12.3 + libtiff 4.7: per-channel means of the RGB image (ours: 58.25, 58.54, 58.99;
    // libtiff's channels average 0.3-1.0 levels lower).
    let want = [57.941_555_31, 58.114_702_46, 58.032_041_25];
    for (c, w) in want.iter().enumerate() {
        let s: f64 = got.iter().skip(c).step_by(3).map(|&v| f64::from(v)).sum();
        let mean = s / (234.0 * 213.0);
        eprintln!("channel {c}: mean {mean:.4}, libtiff {w:.4}");
        assert!(
            (mean - w).abs() < 1.2,
            "channel {c}: mean {mean} vs libtiff {w}"
        );
    }
}
