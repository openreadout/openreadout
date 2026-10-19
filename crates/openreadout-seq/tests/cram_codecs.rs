//! The CRAM codec test vectors of hts-specs (`test/cram/codecs`, Apache-2.0 / BSD), when
//! `CRAM_CODEC_VECTORS` names a copy of that directory: every rANS 4x8, rANS Nx16, arithmetic,
//! fqzcomp and name-tokeniser file must decode to its gzip original (qualities: the first
//! column of each line, joined; names: one per line). Without the variable the test does
//! nothing:
//! `CRAM_CODEC_VECTORS=path/to/hts-specs/test/cram/codecs cargo test -p openreadout-seq --test cram_codecs`.

use std::io::Read;
use std::path::Path;

fn original(dir: &Path, stem: &str) -> Vec<u8> {
    let f = std::fs::File::open(dir.join("gzip").join(format!("{stem}.gz"))).unwrap();
    let mut out = Vec::new();
    flate2::read::MultiGzDecoder::new(f)
        .read_to_end(&mut out)
        .unwrap();
    out
}

fn first_columns(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in text.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        out.extend(
            line.split(|b| *b == b' ' || *b == b'\t')
                .next()
                .unwrap_or(&[]),
        );
    }
    out
}

#[test]
fn hts_specs_codec_vectors() {
    let Some(dir) = std::env::var_os("CRAM_CODEC_VECTORS") else {
        return;
    };
    let dir = Path::new(&dir);
    let mut checked = 0;
    let mut failed = Vec::new();
    for (sub, method) in [
        ("rans4x8", 4u8),
        ("ransNx16", 5),
        ("range", 6),
        ("fqzcomp", 7),
        ("tok3", 8),
    ] {
        let mut entries: Vec<_> = std::fs::read_dir(dir.join(sub))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            let stem = name.rsplit_once('.').unwrap().0.to_string();
            let data = std::fs::read(&p).unwrap();
            let raw = original(dir, &stem);
            let want = if method == 8 || stem == "u32" {
                raw.clone()
            } else {
                first_columns(&raw)
            };
            match openreadout_seq::cram::decode_codec(method, &data) {
                Ok(mut got) => {
                    if method == 7 {
                        // fqzcomp returns Phred values; the originals are FASTQ characters
                        for b in &mut got {
                            *b = b.wrapping_add(33);
                        }
                    }
                    if method == 8 {
                        for b in &mut got {
                            if *b == 0 {
                                *b = b'\n';
                            }
                        }
                    }
                    if got != want && got != raw {
                        failed.push(format!(
                            "{sub}/{name}: {} bytes, want {}",
                            got.len(),
                            want.len()
                        ));
                    }
                }
                Err(e) => failed.push(format!("{sub}/{name}: {e}")),
            }
            checked += 1;
        }
    }
    assert!(
        failed.is_empty(),
        "{checked} files, failures:\n{}",
        failed.join("\n")
    );
    eprintln!("{checked} codec test vectors decode to their originals");
}
