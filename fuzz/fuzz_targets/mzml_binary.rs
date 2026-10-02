//! mzML/mzXML binary arrays: base64, then the value type / compression / MS-Numpress
//! decoder picked by the first byte; the MS-Numpress decoders are also fed raw bytes.
#![no_main]

use openreadout_mzml::{ArrayEncoding, Compression, Outer, ValueType};
use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else {
        return;
    };
    let value_type = match sel & 3 {
        0 => ValueType::Float32,
        1 => ValueType::Float64,
        2 => ValueType::Int32,
        _ => ValueType::Int64,
    };
    let outer = match (sel >> 5) & 3 {
        0 => Outer::Plain,
        1 => Outer::Zlib,
        _ => Outer::Zstd,
    };
    let compression = match (sel >> 2) & 7 {
        0 => Compression::NoCompression,
        1 => Compression::Zlib,
        2 => Compression::Zstd,
        3 => Compression::NumpressLinear(outer),
        4 => Compression::NumpressPic(outer),
        5 => Compression::NumpressSlof(outer),
        _ => Compression::Unsupported("MS:0000000"),
    };
    let enc = ArrayEncoding {
        value_type,
        compression,
        big_endian: sel & 0x80 != 0,
    };
    let _ = openreadout_mzml::decode_array(rest, &enc, None);
    let _ = openreadout_mzml::decode_array(rest, &enc, Some(64));
    let _ = openreadout_mzml::decode_values(rest.to_vec(), &enc, Some(16));
    let _ = openreadout_mzml::decode_linear(rest);
    let _ = openreadout_mzml::decode_pic(rest);
    let _ = openreadout_mzml::decode_slof(rest);
});
