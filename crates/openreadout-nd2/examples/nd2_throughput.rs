//! Plane-read throughput of the ND2 reader: reads every plane of every image of the given
//! files (c fastest, then z, then t) and prints planes/s, MB/s and the time per plane.
//!
//! `cargo run --release -p openreadout-nd2 --example nd2_throughput -- FILE.nd2 [...]`
//! `--memory` reads the file into memory first and opens it through a byte source instead.

use std::time::Instant;

use openreadout_core::{FormatReader, Input, PlaneIndex};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let memory = args.iter().any(|a| a == "--memory");
    for path in args.iter().filter(|a| !a.starts_with("--")) {
        let p = std::path::Path::new(path);
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let input = if memory {
            Input::from_bytes(&name, std::fs::read(p).expect("read file"))
        } else {
            Input::local(p)
        };
        let mut ds = openreadout_nd2::Nd2Reader.open_input(&input).expect("open");
        let info = ds.info().expect("info");
        for im in &info.images {
            // The page cache and the reader warm up on the first pass; the best of three counts.
            let mut best = f64::INFINITY;
            let (mut n, mut bytes) = (0u64, 0u64);
            for _ in 0..3 {
                let (mut pn, mut pb) = (0u64, 0u64);
                let t0 = Instant::now();
                for t in 0..im.size_t {
                    for z in 0..im.size_z {
                        for c in 0..im.size_c {
                            let pl = ds
                                .read_plane(im.index, PlaneIndex { c, z, t })
                                .expect("read plane");
                            pb += pl.data.len() as u64;
                            pn += 1;
                        }
                    }
                }
                best = best.min(t0.elapsed().as_secs_f64());
                (n, bytes) = (pn, pb);
            }
            println!(
                "{name} image {}: {}x{} c={} z={} t={} spp={} {:?}: {n} planes, {:.1} MB in {:.3} s = {:.0} MB/s, {:.3} ms/plane",
                im.index,
                im.size_x,
                im.size_y,
                im.size_c,
                im.size_z,
                im.size_t,
                im.samples_per_pixel,
                im.pixel_type,
                bytes as f64 / 1e6,
                best,
                bytes as f64 / 1e6 / best,
                best * 1e3 / n as f64
            );
        }
    }
}
