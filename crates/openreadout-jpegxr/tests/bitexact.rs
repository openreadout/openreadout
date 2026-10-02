//! Bit-exactness against Microsoft's jxrlib and robustness on malformed input.
//!
//! `tests/fixtures/*.jxr` were encoded by jxrlib from synthetic images over a matrix of encoder
//! options (pixel formats, 4:2:0/4:2:2/4:4:4/N-channel, overlap 0-2, spatial/frequency order,
//! soft/hard tiles, dropped subbands, flexbit trimming, scaled/unscaled arithmetic);
//! `expected.txt` holds the FNV-1a hash of the samples jxrlib decodes from each. The file names
//! spell the encoder options. The known-upstream reproducers crashed or overflowed the machine
//! translation this crate replaced (`fuzz/known-upstream/`,
//! `crates/openreadout-codecs/tests/fixtures/malformed/`).

use std::path::{Path, PathBuf};

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn expected() -> Vec<(String, usize, u64)> {
    let text = std::fs::read_to_string(fixtures().join("expected.txt")).unwrap();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut p = l.split_whitespace();
            let name = p.next().unwrap().to_string();
            let len = p.next().unwrap().parse().unwrap();
            let hash = u64::from_str_radix(p.next().unwrap(), 16).unwrap();
            (name, len, hash)
        })
        .collect()
}

#[test]
fn decodes_bit_exact_with_jxrlib() {
    let all = expected();
    assert!(all.len() >= 25);
    for (name, len, hash) in all {
        let data = std::fs::read(fixtures().join(&name)).unwrap();
        let img =
            openreadout_jpegxr::decode(&data, 1 << 30).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(img.data.len(), len, "{name}: decoded size");
        assert_eq!(fnv(&img.data), hash, "{name}: samples differ from jxrlib");
        let info = openreadout_jpegxr::probe(&data).unwrap();
        assert_eq!((info.width, info.height), (img.width, img.height), "{name}");
    }
}

fn no_panic(name: &str, data: &[u8]) {
    let r = std::panic::catch_unwind(|| openreadout_jpegxr::decode(data, 64 << 20));
    assert!(r.is_ok(), "{name}: decoder panicked");
}

#[test]
fn known_upstream_reproducers_fail_cleanly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut n = 0;
    for dir in [
        "fuzz/known-upstream",
        "crates/openreadout-codecs/tests/fixtures/malformed",
    ] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for e in entries {
            let p = e.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("jpegxr") {
                continue;
            }
            let data = std::fs::read(&p).unwrap();
            no_panic(&name, &data);
            n += 1;
        }
    }
    assert!(n >= 4, "expected the JPEG XR reproducers, found {n}");
}

/// splitmix64, for deterministic mutations without a dependency.
fn next(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[test]
fn truncated_and_mutated_fixtures_fail_cleanly() {
    let mut seed = 42u64;
    for (name, _, _) in expected() {
        let data = std::fs::read(fixtures().join(&name)).unwrap();
        for cut in [
            0,
            1,
            8,
            16,
            data.len() / 3,
            data.len() / 2,
            data.len().saturating_sub(1),
        ] {
            no_panic(&name, &data[..cut.min(data.len())]);
        }
        for _ in 0..40 {
            let mut m = data.clone();
            for _ in 0..=(next(&mut seed) % 4) {
                let i = (next(&mut seed) as usize) % m.len();
                m[i] ^= 1 << (next(&mut seed) % 8);
            }
            no_panic(&name, &m);
        }
    }
}
