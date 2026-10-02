//! OME-XML builder. The input is a JSON `FileInfo` (the `info --json` payload, so seeds
//! are real reader output) plus an optional vendor-JSON string after a NUL byte. The
//! document must always come out well-formed.
#![no_main]

use libfuzzer_sys::fuzz_target;

use openreadout_core::model::FileInfo;
use openreadout_ometiff::omexml::{build_ome_xml, build_ome_xml_metadata_only, WrittenImage};

fuzz_target!(|data: &[u8]| {
    let (json, vendor) = match data.iter().position(|&b| b == 0) {
        Some(i) => (
            &data[..i],
            Some(String::from_utf8_lossy(&data[i + 1..]).into_owned()),
        ),
        None => (data, None),
    };
    let Ok(info) = serde_json::from_slice::<FileInfo>(json) else {
        return;
    };
    let mut images = Vec::new();
    let mut ifd = 0u32;
    for im in info.images.iter().take(16) {
        // Bound the plane list: the builder's cost is linear in it.
        let (c, z, t) = (im.size_c.min(8), im.size_z.min(8), im.size_t.min(8));
        let mut planes = Vec::new();
        for ti in 0..t {
            for zi in 0..z {
                for ci in 0..c {
                    planes.push((ci, zi, ti));
                }
            }
        }
        let n = planes.len() as u32;
        images.push(WrittenImage {
            info: im,
            first_ifd: ifd,
            planes,
            size_c: c,
            size_z: z,
            size_t: t,
            c_map: (0..c).collect(),
            source_planes: Vec::new(),
            frames: Vec::new(),
        });
        ifd = ifd.saturating_add(n);
    }
    for doc in [
        build_ome_xml(&info, &images, "fuzz", vendor.as_deref()),
        build_ome_xml_metadata_only(&info, &images, "fuzz", vendor.as_deref()),
    ] {
        if let Ok(xml) = doc {
            assert!(
                xml.is_ascii(),
                "OME-XML must be 7-bit for the TIFF ImageDescription"
            );
            if let Err(e) = roxmltree::Document::parse(&xml) {
                panic!("builder produced malformed XML: {e}\n{xml}");
            }
        }
    }
});
