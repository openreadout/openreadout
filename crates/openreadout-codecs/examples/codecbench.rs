//! Single-threaded decode throughput of one codec over a directory of encoded chunks:
//! `cargo run --release -p openreadout-codecs --example codecbench -- <codec> <dir> [repeats]`,
//! codec one of `webp`, `jpegxl`, `lerc`, `jpeg2000`, `jpegxr`, `jpeg`. Prints decoded MB/s
//! (decoded bytes / decode time, files read beforehand) and the number of chunks.

use std::time::Instant;

use openreadout_codecs as c;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (Some(codec), Some(dir)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: codecbench <webp|jpegxl|lerc|jpeg2000|jpegxr|jpeg> <dir> [repeats]");
        std::process::exit(2);
    };
    let repeats: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("directory")
        .map(|e| e.expect("entry").path())
        .collect();
    paths.sort();
    let chunks: Vec<Vec<u8>> = paths
        .iter()
        .map(|p| std::fs::read(p).expect("read"))
        .collect();
    let limit = 1 << 30;
    let decode = |d: &[u8]| -> usize {
        let r = match codec.as_str() {
            "webp" => c::webp_decode_limited(d, limit),
            "jpegxl" => c::jpegxl_decode_limited(d, limit),
            "lerc" => c::lerc_decode_limited(d, limit),
            "jpeg2000" => c::jpeg2000_decode_limited(d, limit),
            "jpegxr" => c::jpegxr_decode(d),
            "jpeg" => c::jpeg_decode(d),
            other => panic!("unknown codec {other}"),
        };
        r.expect("decode").data.len()
    };
    let mut best = f64::INFINITY;
    let mut bytes = 0usize;
    for _ in 0..repeats {
        let t = Instant::now();
        bytes = chunks.iter().map(|d| decode(d)).sum();
        best = best.min(t.elapsed().as_secs_f64());
    }
    println!(
        "{codec} {} chunks {:.1} MB decoded {:.1} MB/s",
        chunks.len(),
        bytes as f64 / 1e6,
        bytes as f64 / 1e6 / best
    );
}
