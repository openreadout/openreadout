//! Illumina InterOp metric parsers (tile, Q, error, extraction, corrected intensity, extended
//! tile, index) and the CBCL header, fed the same bytes, with a three-read layout.
#![no_main]

use libfuzzer_sys::fuzz_target;
use openreadout_seq::illumina::{basecalls, interop::InterOp};

fuzz_target!(|data: &[u8]| {
    let layout = [(1, 1, 36), (2, 37, 46), (3, 47, 56)];
    let mut io = InterOp::default();
    io.add_tile("t", data, &layout);
    io.add_q("q", data, &layout);
    io.add_error("e", data, &layout);
    io.add_extraction("x", data);
    io.add_corrected("c", data);
    io.add_extended_tile("et", data);
    io.add_index("i", data);
    let _ = basecalls::parse_cbcl_header(data, "fuzz");
    let _ = basecalls::parse_filter(data, "fuzz");
    let _ = basecalls::parse_locs(data, "fuzz");
});
