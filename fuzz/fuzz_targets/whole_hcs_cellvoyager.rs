//! Whole plate, Yokogawa CellVoyager measurement: `MeasurementData.mlf`, `MeasurementDetail.mrf`, the `.mes` setting and one plane TIFF; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_hcs::CellVoyagerReader,
        "whole_hcs_cellvoyager",
        "m",
        &[
            "m/MeasurementData.mlf",
            "m/MeasurementDetail.mrf",
            "m/CellPainting_20x_2bin_6FoV.mes",
            "m/1053601756_A01_T0001F001L01A01Z01C01.tif",
        ],
        Some(0),
        data,
    );
});
