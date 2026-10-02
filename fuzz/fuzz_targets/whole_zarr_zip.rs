//! Whole-file OME-Zarr zip store: open -> info -> vendor -> entries -> check -> plane/table/trace/spectrum reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    // Not on macOS: zarrs registers its codecs with `inventory` static initializers, which
    // Apple's linker rejects under sanitizer coverage ("initializer pointer has no target"),
    // and the classic linker then drops libFuzzer's own initializers. Runs on Linux (CI).
    #[cfg(not(target_os = "macos"))]
    openreadout_fuzz::whole_file(
        &openreadout_zarr::ZarrReader,
        "whole_zarr_zip",
        "ome.zarr.zip",
        data,
    );
    #[cfg(target_os = "macos")]
    let _ = data;
});
