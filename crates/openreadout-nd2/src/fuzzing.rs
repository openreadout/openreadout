//! Byte-slice entry points into the ND2 parsers, for the cargo-fuzz targets in `fuzz/`.
//! Compiled only with the `fuzzing` feature; not part of the stable API.

use serde_json::Value;

use crate::container::parse_map;
use crate::meta::{Attributes, FrameLayout, loops_from_lv, planes_from_lv};

/// Parse a chunk-map payload. Returns the number of entries.
pub fn fuzz_chunk_map(payload: &[u8]) -> usize {
    parse_map(payload).len()
}

/// Run the normalizers the reader applies to a decoded metadata tree (LV or variant XML):
/// attributes, the experiment loop tree, the frame layout and picture planes.
pub fn fuzz_normalize(v: &Value) {
    let attrs = Attributes::from_lv(v);
    let loops = loops_from_lv(v);
    let frames = attrs.as_ref().map_or(1, |a| a.frame_count);
    let layout = FrameLayout::from_loops(&loops, frames);
    let _ = layout.frame_index(
        layout.p_count.saturating_sub(1),
        layout.t_count.saturating_sub(1),
        layout.z_count.saturating_sub(1),
    );
    let _ = planes_from_lv(v);
}
