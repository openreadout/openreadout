//! LZMA2 against streams written by a reference encoder (CPython's `lzma` module, liblzma,
//! `FORMAT_RAW` with `FILTER_LZMA2`): `tests/fixtures/lzma2/`.
//!
//! - `text.lzma2`: 2,300,000 bytes of `<Item value="{i % 1013}" name="n{i % 97}"/>` for
//!   i = 0, 1, …, cut to length; preset 6, so several LZMA chunks with state carried over.
//! - `mixed.lzma2`: 70,000 random bytes (Python `random.seed(7)`, `getrandbits(8)`) then
//!   `abc` × 20,000; preset 1 with lc 0, lp 2, pb 0: uncompressed chunks and LZMA chunks.

use openreadout_codecs::lzma2_decode;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/lzma2")
            .join(name),
    )
    .unwrap()
}

fn fnv1a(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &x| {
        (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[test]
fn multi_chunk_text() {
    let mut want = Vec::new();
    for i in 0..90_000u32 {
        want.extend_from_slice(
            format!("<Item value=\"{}\" name=\"n{}\"/>", i % 1013, i % 97).as_bytes(),
        );
    }
    want.truncate(2_300_000);
    let data = fixture("text.lzma2");
    let (out, used) = lzma2_decode(&data, 0).unwrap();
    assert_eq!(used, data.len());
    assert!(out == want, "decoded text differs");
    let (sized, _) = lzma2_decode(&data, want.len()).unwrap();
    assert_eq!(sized.len(), want.len());
    assert!(lzma2_decode(&data, want.len() - 1).is_err());
}

#[test]
fn uncompressed_and_lzma_chunks() {
    let data = fixture("mixed.lzma2");
    let (out, used) = lzma2_decode(&data, 0).unwrap();
    assert_eq!(used, data.len());
    assert_eq!(out.len(), 130_000);
    assert_eq!(&out[70_000..70_006], b"abcabc");
    assert_eq!(fnv1a(&out), 0x771d_3efd_73be_7db3);
}

#[test]
fn every_truncation_fails_cleanly() {
    let data = fixture("text.lzma2");
    for cut in (0..data.len()).step_by(997) {
        assert!(lzma2_decode(&data[..cut], 0).is_err(), "cut at {cut}");
    }
    // flipped bytes decode or fail, never panic
    let mut bad = fixture("mixed.lzma2");
    for i in (0..bad.len()).step_by(4099) {
        bad[i] ^= 0x5a;
        let _ = lzma2_decode(&bad, 0);
    }
}
