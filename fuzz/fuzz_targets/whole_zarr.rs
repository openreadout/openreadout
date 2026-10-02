//! Whole data set, OME-Zarr directory store: `.zgroup`, `.zattrs`, `0/.zarray`, `0/0.0.0.0.0` (v2), then `zarr.json`, `0/zarr.json`, `0/c/0/0/0/0/0` (v3); parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    // Not on macOS: see whole_zarr_zip.rs (zarrs' `inventory` initializers do not link under
    // sanitizer coverage with Apple's linker). Runs on Linux (CI).
    #[cfg(not(target_os = "macos"))]
    openreadout_fuzz::whole_bundle(
        &openreadout_zarr::ZarrReader,
        "whole_zarr",
        "s.zarr",
        &[
            "s.zarr/.zgroup",
            "s.zarr/.zattrs",
            "s.zarr/0/.zarray",
            "s.zarr/0/0.0.0.0.0",
            "s.zarr/zarr.json",
            "s.zarr/0/zarr.json",
            "s.zarr/0/c/0/0/0/0/0",
        ],
        None,
        data,
    );
    #[cfg(target_os = "macos")]
    let _ = data;
});
