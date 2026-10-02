//! Decode JPEG XR files and print `name width height channels fnv1a64` per file (or the error),
//! for differential testing against another decoder. `--raw DIR` also writes the samples.
#![forbid(unsafe_code)]

use std::io::Write;

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn main() {
    let mut raw_dir = None;
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--raw" {
            raw_dir = args.next();
        } else {
            files.push(a);
        }
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut total_bytes = 0usize;
    let mut secs = 0.0f64;
    for f in files {
        let name = std::path::Path::new(&f)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let data = match std::fs::read(&f) {
            Ok(d) => d,
            Err(e) => {
                let _ = writeln!(out, "{name} IOERROR {e}");
                continue;
            }
        };
        let t = std::time::Instant::now();
        let result = openreadout_jpegxr::decode(&data, 4 << 30);
        secs += t.elapsed().as_secs_f64();
        match result {
            Ok(img) => {
                total_bytes += img.data.len();
                let _ = writeln!(
                    out,
                    "{name} {} {} {} {:016x}",
                    img.width,
                    img.height,
                    img.channels,
                    fnv(&img.data)
                );
                if let Some(dir) = &raw_dir {
                    let _ = std::fs::write(
                        std::path::Path::new(dir).join(format!("{name}.raw")),
                        &img.data,
                    );
                }
            }
            Err(e) => {
                let _ = writeln!(out, "{name} ERROR {e}");
            }
        }
    }
    eprintln!(
        "decoded {:.1} MB in {secs:.2} s of decoding ({:.1} MB/s)",
        total_bytes as f64 / 1e6,
        total_bytes as f64 / 1e6 / secs.max(1e-9)
    );
}
