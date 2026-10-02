//! Compound-file (MS-CFB) reader of the Shimadzu reader: header, FAT, mini FAT, directory
//! tree, then every stream read back (at most 1 MiB each).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(init: openreadout_fuzz::init(), |data: &[u8]| {
    let path = openreadout_fuzz::scratch_file("chrom_cfb", "lcd", data);
    let Ok(mut f) = std::fs::File::open(&path) else {
        return;
    };
    let Ok(cfb) = openreadout_chrom::Cfb::open(&mut f, &path, "shimadzu") else {
        return;
    };
    for e in cfb.entries.iter().filter(|e| e.is_stream) {
        let _ = cfb.stream(&e.path);
        let _ = cfb.read(&mut f, &path, e, 1 << 20);
    }
});
