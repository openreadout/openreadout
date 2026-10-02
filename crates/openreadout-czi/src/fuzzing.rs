//! Byte-slice entry points into the container parsers, for the cargo-fuzz targets in
//! `fuzz/`. Compiled only with the `fuzzing` feature; not part of the stable API.

use openreadout_core::Result;

use crate::container::{parse_attachment_entry, parse_entry, parse_subblock_header};

/// Parse consecutive subblock directory entries from `data` (as in a `ZISRAWDIRECTORY`
/// payload after its 128-byte header). Returns how many parsed.
pub fn fuzz_directory_entries(data: &[u8]) -> usize {
    let mut off = 0;
    let mut n = 0;
    while let Ok((_, len)) = parse_entry(data, off) {
        off += len;
        n += 1;
    }
    n
}

/// Parse a `ZISRAWSUBBLOCK` payload header (sizes plus the embedded directory entry) and
/// run the geometry helpers the reader uses on it.
pub fn fuzz_subblock_header(data: &[u8]) -> Result<()> {
    let sb = parse_subblock_header(data)?;
    let e = &sb.entry;
    let _ = (
        e.is_level0(),
        e.upsample_factor(),
        e.scale(),
        e.encoded_len(),
    );
    for d in ['X', 'Y', 'Z', 'C', 'T', 'S', 'M'] {
        let _ = (e.index(d), e.covers(d, 0), e.covers(d, i32::MAX));
    }
    let _ = (
        e.pixel_type.name(),
        e.pixel_type.bytes_per_pixel(),
        e.compression.name(),
    );
    Ok(())
}

/// Parse attachment directory entries (128 bytes each) from `data`.
pub fn fuzz_attachment_entries(data: &[u8]) -> usize {
    (0..data.len() / 128)
        .filter_map(|i| parse_attachment_entry(data, i * 128))
        .count()
}

/// Parse the `ImageDocument` metadata XML the way `open` does.
pub fn fuzz_metadata_xml(xml: &str) -> bool {
    crate::xml::parse_image_xml(xml).is_some()
}
