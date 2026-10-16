//! Whole Illumina run folder with BCL base calls: `RunInfo.xml`, `RunParameters.xml`, the InterOp tile, Q, error, extraction, corrected-intensity, extended-tile and index metrics, a filter file, two cycles' BCL files and a locations file; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_seq::IlluminaRunReader,
        "whole_illumina_bcl",
        "",
        &[
            "RunInfo.xml",
            "RunParameters.xml",
            "InterOp/TileMetricsOut.bin",
            "InterOp/QMetricsOut.bin",
            "InterOp/ErrorMetricsOut.bin",
            "InterOp/ExtractionMetricsOut.bin",
            "InterOp/CorrectedIntMetricsOut.bin",
            "InterOp/ExtendedTileMetricsOut.bin",
            "InterOp/IndexMetricsOut.bin",
            "Data/Intensities/BaseCalls/L001/s_1_1101.filter",
            "Data/Intensities/BaseCalls/L001/C1.1/s_1_1101.bcl",
            "Data/Intensities/BaseCalls/L001/C2.1/s_1_1101.bcl",
            "Data/Intensities/L001/s_1_1101.locs",
        ],
        None,
        data,
    );
});
