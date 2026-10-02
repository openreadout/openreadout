//! Small synthetic files for tests and demos (no corpus needed). Every value is invented.

/// A classic little-endian OME-TIFF: `pages` uint16 `w`×`h` planes along T (sample values
/// `page * 1000 + i`), page 0's ImageDescription (the OME-XML) stored after the last page, as
/// in the corpus OME-TIFFs. Replay it with [`crate::replay::Pattern::OmeTiff`].
pub fn ome_tiff(w: u32, h: u32, pages: u32) -> Vec<u8> {
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06"><Image ID="Image:0" Name="synthetic"><Pixels ID="Pixels:0" DimensionOrder="XYZCT" Type="uint16" SizeX="{w}" SizeY="{h}" SizeZ="1" SizeC="1" SizeT="{pages}"><Channel ID="Channel:0:0" SamplesPerPixel="1"/><TiffData IFD="0" PlaneCount="{pages}"/></Pixels></Image></OME>"#
    );
    let page_len = w * h * 2;
    let mut out = Vec::new();
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&8u32.to_le_bytes());
    let ifd_len = |n: u32| 2 + 12 * n + 4;
    // Layout: [header][IFD0][data0][IFD1][data1]...[XML]
    let mut offsets = Vec::new();
    let mut pos = 8u32;
    for p in 0..pages {
        let n = if p == 0 { 11 } else { 10 };
        offsets.push(pos);
        pos += ifd_len(n) + page_len;
    }
    let xml_at = pos;
    for p in 0..pages {
        let n: u16 = if p == 0 { 11 } else { 10 };
        let ifd = offsets[p as usize];
        let data = ifd + ifd_len(u32::from(n));
        let next = offsets.get(p as usize + 1).copied().unwrap_or(0);
        out.extend_from_slice(&n.to_le_bytes());
        let mut entry = |tag: u16, typ: u16, count: u32, value: u32| {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            if typ == 3 {
                out.extend_from_slice(&(value as u16).to_le_bytes());
                out.extend_from_slice(&[0, 0]);
            } else {
                out.extend_from_slice(&value.to_le_bytes());
            }
        };
        entry(256, 3, 1, w);
        entry(257, 3, 1, h);
        entry(258, 3, 1, 16);
        entry(259, 3, 1, 1);
        entry(262, 3, 1, 1);
        if p == 0 {
            entry(270, 2, xml.len() as u32 + 1, xml_at);
        }
        entry(273, 4, 1, data);
        entry(277, 3, 1, 1);
        entry(278, 3, 1, h);
        entry(279, 4, 1, page_len);
        entry(339, 3, 1, 1);
        out.extend_from_slice(&next.to_le_bytes());
        for i in 0..(w * h) {
            out.extend_from_slice(&((p * 1000 + i) as u16).to_le_bytes());
        }
    }
    out.extend_from_slice(xml.as_bytes());
    out.push(0);
    out
}
