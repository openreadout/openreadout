//! Waters drift-resolved data: a `_funcNNN.ind` drift index and the `_funcNNN.cdt` bytes it
//! points into (LZRW3 sections of positions, intensities and point flags). First 4 bytes: the
//! length of the index part; the rest is the `.cdt`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use openreadout_waters::drift::{decode_drift_scan, lzrw3_decompress, parse_drift_index};

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    if data.len() < 4 {
        return;
    }
    let n = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let rest = &data[4..];
    let (ind, cdt) = rest.split_at(n.min(rest.len()));
    let _ = lzrw3_decompress(cdt, 1 << 20);
    if let Some(ix) = parse_drift_index(ind) {
        for s in ix.scans.iter().take(8) {
            let at = usize::try_from(s.offset).unwrap_or(usize::MAX);
            if let Some(bytes) = cdt.get(at..) {
                let _ = decode_drift_scan(bytes, s);
            }
        }
    }
});
