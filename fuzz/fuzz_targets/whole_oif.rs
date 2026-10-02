//! Whole data set, Olympus FluoView OIF: the `.oif` settings file, then one plane's `.pty` and `.tif` in its `.oif.files` folder; parts separated by `openreadout_fuzz::BUNDLE_SEP`.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_bundle(
        &openreadout_oif::OifReader,
        "whole_oif",
        "",
        &["50x.oif", "50x.oif.files/s_C001.pty", "50x.oif.files/s_C001.tif"],
        Some(0),
        data,
    );
});
