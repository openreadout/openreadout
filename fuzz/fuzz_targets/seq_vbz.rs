//! VBZ signal decoding: the POD5 row decoder (zstd + 16-bit variable-byte integers), the FAST5
//! chunk decoder (size header + zstd + StreamVByte) and both variable-byte layers on raw bytes.
//! The first two bytes give the sample count (little-endian u16).
#![no_main]

use libfuzzer_sys::fuzz_target;
use openreadout_seq::nanopore::vbz;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let n = usize::from(u16::from_le_bytes([data[0], data[1]]));
    let body = &data[2..];
    let _ = vbz::pod5_row(body, n);
    let _ = vbz::svb16_decode(body, n);
    let _ = vbz::streamvbyte_decode(body, n);
    let _ = vbz::fast5_chunk(body, &[1, 2, 1, 1]);
    let _ = vbz::fast5_chunk(body, &[1, 2, 0, 1]);
});
