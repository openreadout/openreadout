//! JPEG pages that the plane API does not expose (pyramid levels, thumbnails, macro images) of
//! the whole-slide corpus files, decoded page by page and compared with tifffile/libjpeg-turbo
//! reference means (a JPEG decoder may differ by one count in some samples). Skipped when the
//! corpus files are not present (`cargo xtask corpus fetch --tier standard --format tiff`).

use std::path::PathBuf;

use openreadout_tiff::{PageLayout, SampleSelect, TiffFile};

fn corpus(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    );
    let p = dir.join(name);
    p.exists().then_some(p)
}

/// (file, page, width, height, tifffile mean over all samples)
const CASES: [(&str, usize, u32, u32, f64); 5] = [
    // photometric YCbCr, one JPEG strip per level → YCbCr-to-RGB path
    (
        "openslide-hamamatsu-CMU-1.ndpi",
        3,
        800,
        596,
        180.904_433_724_832_2,
    ),
    (
        "openslide-hamamatsu-CMU-1.ndpi",
        4,
        1191,
        408,
        174.275_330_913_221_7,
    ),
    // photometric RGB, JPEG with JPEGTables: stripped thumbnail and a tiled pyramid level
    (
        "openslide-aperio-CMU-1.svs",
        1,
        1024,
        732,
        225.926_188_151_041_66,
    ),
    (
        "openslide-aperio-CMU-1.svs",
        3,
        2875,
        2057,
        225.446_918_362_889_53,
    ),
    (
        "ome-qptiff-HandEcompressed_Scan1.qptiff",
        5,
        1920,
        1665,
        198.192_088_234_067_4,
    ),
];

#[test]
fn jpeg_pages_match_reference_means() {
    for (name, page, w, h, mean) in CASES {
        let Some(p) = corpus(name) else {
            continue;
        };
        let (tf, mut src) = TiffFile::open(&p).unwrap();
        let layout = PageLayout::from_ifd(&tf.ifds[page], tf.header.byte_order).unwrap();
        assert_eq!((layout.width, layout.height), (w, h), "{name} page {page}");
        let data = openreadout_tiff::read_page(&mut src, &layout, SampleSelect::All).unwrap();
        assert_eq!(data.len(), (w * h * 3) as usize);
        let ours = data.iter().map(|&v| f64::from(v)).sum::<f64>() / data.len() as f64;
        assert!(
            (ours - mean).abs() < 0.05,
            "{name} page {page}: mean {ours} vs tifffile {mean}"
        );
    }
}
