//! `--select` parser (`c=0`, `z=2-5`, `t=0,3,7`), args separated by newlines.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let s = String::from_utf8_lossy(data);
    let args: Vec<String> = s.split('\n').take(64).map(str::to_string).collect();
    if let Ok(sel) = openreadout_core::select::Selection::parse(&args) {
        let _ = (sel.contains(0, 0, 0), sel.is_all());
    }
});
