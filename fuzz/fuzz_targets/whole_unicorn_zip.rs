//! Whole-file UNICORN 6/7 result export (.zip): Result.xml, Chrom.1.Xml, nested zips padded after their end record, MS-NRBF strings and float arrays: open -> info -> vendor -> entries -> check -> table/trace reads.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    openreadout_fuzz::whole_file(&openreadout_fplc::UnicornZipReader, "whole_unicorn_zip", "zip", data);
});
