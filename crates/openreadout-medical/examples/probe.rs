//! Development probe: `cargo run -p openreadout-medical --example probe -- FILE [--planes]
//! [--vendor]` prints `info` as JSON and, with `--planes`, the xxh3 of every plane.
use std::path::Path;

use openreadout_core::PlaneIndex;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = Path::new(&args[0]);
    let planes = args.iter().any(|a| a == "--planes");
    let vendor = args.iter().any(|a| a == "--vendor");
    let quiet = args.iter().any(|a| a == "--quiet");
    let reg = openreadout_core::Registry::new()
        .with(Box::new(openreadout_medical::NiftiReader))
        .with(Box::new(openreadout_medical::DicomReader));
    let mut ds = match reg.open(path) {
        Ok((_, d)) => d,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    };
    let info = ds.info().expect("info");
    if !quiet {
        println!("{}", serde_json::to_string_pretty(&info).unwrap());
    }
    if vendor {
        println!(
            "{}",
            serde_json::to_string_pretty(&ds.vendor_metadata().unwrap()).unwrap()
        );
    }
    if planes {
        for im in &info.images {
            for t in 0..im.size_t {
                for z in 0..im.size_z {
                    match ds.read_plane(im.index, PlaneIndex { c: 0, z, t }) {
                        Ok(p) => println!("image {} z {z} t {t}: {}", im.index, p.xxh3_hex()),
                        Err(e) => println!("image {} z {z} t {t}: ERROR {e}", im.index),
                    }
                }
            }
        }
    }
}
