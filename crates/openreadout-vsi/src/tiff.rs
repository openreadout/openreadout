//! The `.vsi` file's own TIFF part: one IFD with a small JPEG preview.

use openreadout_core::bytes::{le_u16, le_u32};

/// What we read from the first IFD.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewInfo {
    pub ifd_offset: u32,
    pub width: u32,
    pub height: u32,
    /// TIFF compression code (7 = JPEG).
    pub compression: u16,
    pub strip_offset: u32,
    pub strip_len: u32,
    /// `JPEGTables` byte range, if present.
    pub tables: Option<(u32, u32)>,
    /// `Make` and `Model` (the camera, in fluorescence files).
    pub make: Option<String>,
    pub model: Option<String>,
}

/// Does `head` start like a `.vsi`: little-endian TIFF, then the record tree at offset 8?
pub fn looks_like_vsi(head: &[u8]) -> bool {
    head.len() >= 12 && &head[..4] == b"II*\0" && head[8..12] == crate::tree::VOLUME_MAGIC
}

/// Parse the first IFD (single-strip preview); `None` when it is not a little-endian TIFF.
#[allow(clippy::many_single_char_names)]
pub fn preview_info(d: &[u8]) -> Option<PreviewInfo> {
    if d.get(..4)? != b"II*\0" {
        return None;
    }
    let ifd = le_u32(d, 4)?;
    let n = le_u16(d, ifd as usize)? as usize;
    let mut p = PreviewInfo {
        ifd_offset: ifd,
        ..PreviewInfo::default()
    };
    let text = |count: u32, value: u32| -> Option<String> {
        let (s, e) = if count <= 4 {
            return None;
        } else {
            (value as usize, value as usize + count as usize)
        };
        let b = d.get(s..e)?;
        Some(
            String::from_utf8_lossy(b)
                .trim_end_matches('\0')
                .trim()
                .to_string(),
        )
    };
    for k in 0..n.min(512) {
        let e = ifd as usize + 2 + 12 * k;
        let tag = le_u16(d, e)?;
        let ty = le_u16(d, e + 2)?;
        let count = le_u32(d, e + 4)?;
        let value = if ty == 3 && count == 1 {
            u32::from(le_u16(d, e + 8)?)
        } else {
            le_u32(d, e + 8)?
        };
        match tag {
            256 => p.width = value,
            257 => p.height = value,
            259 => p.compression = value as u16,
            273 => p.strip_offset = value,
            279 => p.strip_len = value,
            347 => p.tables = Some((value, count)),
            271 => p.make = text(count, value),
            272 => p.model = text(count, value),
            _ => {}
        }
    }
    Some(p)
}

/// The preview as a standalone JPEG: `JPEGTables` without its end marker + the strip without
/// its start marker (the usual way to complete an abbreviated TIFF JPEG strip).
pub fn preview_jpeg(d: &[u8], p: &PreviewInfo) -> Option<Vec<u8>> {
    let s = p.strip_offset as usize;
    let strip = d.get(s..s.checked_add(p.strip_len as usize)?)?;
    let Some((to, tl)) = p.tables else {
        return Some(strip.to_vec());
    };
    let tables = d.get(to as usize..(to as usize).checked_add(tl as usize)?)?;
    if tables.len() < 4
        || strip.len() < 2
        || !tables.ends_with(&[0xFF, 0xD9])
        || strip[..2] != [0xFF, 0xD8]
    {
        return Some(strip.to_vec());
    }
    let mut out = tables[..tables.len() - 2].to_vec();
    out.extend_from_slice(&strip[2..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_and_reads_ifd() {
        let mut d = b"II*\0".to_vec();
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&crate::tree::VOLUME_MAGIC);
        assert!(looks_like_vsi(&d));
        d.resize(40, 0);
        // IFD with width, height, strip offset/len
        let entries: [(u16, u16, u32, u32); 4] = [
            (256, 4, 1, 7),
            (257, 3, 1, 5),
            (273, 4, 1, 100),
            (279, 4, 1, 4),
        ];
        d.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, ty, count, value) in entries {
            d.extend_from_slice(&tag.to_le_bytes());
            d.extend_from_slice(&ty.to_le_bytes());
            d.extend_from_slice(&count.to_le_bytes());
            d.extend_from_slice(&value.to_le_bytes());
        }
        d.resize(100, 0);
        d.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
        let p = preview_info(&d).unwrap();
        assert_eq!(
            (p.width, p.height, p.strip_offset, p.strip_len),
            (7, 5, 100, 4)
        );
        assert_eq!(preview_jpeg(&d, &p).unwrap(), vec![0xFF, 0xD8, 0xFF, 0xD9]);
    }
}
