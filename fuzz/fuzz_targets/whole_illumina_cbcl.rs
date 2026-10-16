//! Whole Illumina run folder with CBCL base calls: `RunInfo.xml`, InterOp tile v3 and Q v7 metrics, a filter file, two cycles' CBCL files and the flow cell's `s.locs`; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_seq::IlluminaRunReader,
        "whole_illumina_cbcl",
        "",
        &[
            "RunInfo.xml",
            "InterOp/TileMetricsOut.bin",
            "InterOp/QMetricsOut.bin",
            "Data/Intensities/BaseCalls/L001/s_1_2101.filter",
            "Data/Intensities/BaseCalls/L001/C1.1/L001_2.cbcl",
            "Data/Intensities/BaseCalls/L001/C2.1/L001_2.cbcl",
            "Data/Intensities/s.locs",
        ],
        None,
        data,
    );
});
