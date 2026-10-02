//! String/byte-slice entry points into the LIF parsers, for the cargo-fuzz targets in
//! `fuzz/`. Compiled only with the `fuzzing` feature; not part of the stable API.

use crate::xml::{Axis, parse_images};

/// Parse an XML header into image nodes and evaluate the per-image geometry the reader
/// derives from it. Returns the number of image nodes, or `None` if the XML is rejected.
pub fn fuzz_xml_model(xml: &str) -> Option<usize> {
    let nodes = parse_images(xml).ok()?;
    for n in &nodes {
        for a in [
            Axis::X,
            Axis::Y,
            Axis::Z,
            Axis::T,
            Axis::Mosaic,
            Axis::Lambda,
        ] {
            let _ = (
                n.count(a),
                n.inc(a),
                n.dim(a).and_then(crate::xml::DimensionDesc::step),
            );
        }
        for d in &n.dimensions {
            let _ = d.axis.name();
        }
    }
    Some(nodes.len())
}

/// The signature test `sniff` runs on the first bytes of a file.
pub fn fuzz_sniff(head: &[u8]) -> bool {
    crate::container::looks_like_lif(head)
}
