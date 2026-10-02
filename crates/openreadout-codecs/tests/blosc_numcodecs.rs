//! Blosc chunks written by numcodecs (MIT test fixtures in `tests/fixtures/numcodecs-blosc/`):
//! bit shuffle with lz4, blosclz and snappy, and byte shuffle with small blocks, each decoding
//! to the array it was made from. Array 03 (1000 booleans) is left out: c-blosc 1.21
//! (numcodecs 0.17) does not decode its stored chunks to the stored array either.

use std::path::Path;

#[test]
fn numcodecs_blosc_fixtures_decode_to_their_arrays() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/numcodecs-blosc");
    let mut checked = 0;
    for codec in ["05", "08", "09", "11"] {
        for a in 0..13 {
            let enc = dir.join(format!("codec.{codec}-encoded.{a:02}.dat"));
            if a == 3 {
                continue;
            }
            let Ok(bytes) = std::fs::read(&enc) else {
                continue;
            };
            let want = std::fs::read(dir.join(format!("array.{a:02}.bin"))).unwrap();
            let got = openreadout_codecs::blosc_decode(&bytes)
                .unwrap_or_else(|e| panic!("codec {codec} array {a}: {e}"));
            if got != want {
                let first = got.iter().zip(&want).position(|(x, y)| x != y);
                panic!(
                    "codec {codec} array {a}: lengths {} {}, first difference at {first:?}",
                    got.len(),
                    want.len()
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 40, "only {checked} fixtures found");
}
