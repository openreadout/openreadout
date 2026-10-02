//! Whole-file Bio-Rad Image Lab .scn: MIME multipart walk (nested parts, lengths, delimiters), XML headers, image data: open -> info -> vendor -> entries -> check -> plane/table reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_gel::ImageLabReader, "whole_scn", "scn", data);
});
