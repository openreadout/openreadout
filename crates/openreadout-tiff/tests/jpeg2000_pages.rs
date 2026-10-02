//! JPEG 2000 pyramid levels of the Aperio corpus slides (33005 Kakadu RGB, 33003 Matrox YCbCr),
//! decoded page by page and compared with tifffile + imagecodecs (OpenJPEG): the page mean, and
//! three 256 x 256 crops per page (`tests/fixtures/jpeg2000/*.rgb.zst`, OpenJPEG's samples) that
//! must agree within 2 grey levels with at most 0.5 % of samples differing — irreversible 9/7
//! JPEG 2000 is not bit-exact across decoders. Skipped when the corpus files are not present
//! (`cargo xtask corpus fetch --tier standard --only aperio`).

use std::path::{Path, PathBuf};

use openreadout_tiff::{PageLayout, SampleSelect, TiffFile};

fn corpus(name: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("OPENREADOUT_CORPUS_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/files"),
        PathBuf::from,
    );
    let p = dir.join(name);
    p.exists().then_some(p)
}

/// (file, page, width, height, tifffile + imagecodecs mean over all samples)
const CASES: [(&str, usize, u32, u32, f64); 4] = [
    (
        "openslide-aperio-CMU-1-JP2K-33005.svs",
        2,
        11500,
        8223,
        226.569_824_560_661_43,
    ),
    (
        "openslide-aperio-CMU-1-JP2K-33005.svs",
        3,
        2875,
        2055,
        226.414_009_520_787_06,
    ),
    (
        "openslide-aperio-JP2K-33003-1.svs",
        2,
        3843,
        4374,
        226.669_772_073_945,
    ),
    (
        "openslide-aperio-JP2K-33003-1.svs",
        3,
        1921,
        2187,
        226.358_228_441_357_74,
    ),
];

fn crops(file: &str, page: usize) -> Vec<(usize, usize, Vec<u8>)> {
    let stem = file
        .strip_prefix("openslide-")
        .and_then(|s| s.strip_suffix(".svs"))
        .unwrap();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/jpeg2000");
    let prefix = format!("{stem}-p{page}-");
    let mut v = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let rest = rest.strip_suffix(".rgb.zst").unwrap();
        let (y, x) = rest.split_once("-x").unwrap();
        let y: usize = y.strip_prefix('y').unwrap().parse().unwrap();
        let x: usize = x.parse().unwrap();
        let raw =
            openreadout_codecs::zstd_decode(&std::fs::read(&p).unwrap(), 256 * 256 * 3).unwrap();
        v.push((y, x, raw));
    }
    v
}

#[test]
fn aperio_jpeg2000_levels_match_openjpeg() {
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
            (ours - mean).abs() < 0.02,
            "{name} page {page}: mean {ours} vs OpenJPEG {mean}"
        );
        let crops = crops(name, page);
        assert_eq!(crops.len(), 3, "{name} page {page}: reference crops");
        for (y, x, want) in crops {
            let (mut worst, mut differing) = (0u8, 0usize);
            for r in 0..256 {
                let row = &data[((y + r) * w as usize + x) * 3..][..256 * 3];
                for (a, b) in row.iter().zip(&want[r * 256 * 3..(r + 1) * 256 * 3]) {
                    let d = a.abs_diff(*b);
                    worst = worst.max(d);
                    differing += usize::from(d > 0);
                }
            }
            assert!(
                worst <= 2 && differing * 200 <= want.len(),
                "{name} page {page} crop ({y},{x}): max diff {worst}, {differing} samples differ"
            );
        }
    }
}
