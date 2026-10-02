//! Valid-pixel masks carried in a subblock's attachment. Layout from the ZEISS libCZI
//! documentation page `valid_pixel_mask_concept` (chunk container, GUID, header, MSB-first bits)
//! and czifile's `CziSubBlockSegmentData.mask` (BSD-3). See `docs/formats/czi.md`.

use openreadout_core::bytes::{le_i32, le_u32};

/// Chunk-type GUID of the valid-pixel mask, as stored (little-endian GUID
/// `{CBE3EA67-5BFC-492B-A16A-ECE378031448}`).
pub const MASK_CHUNK_GUID: [u8; 16] = [
    0x67, 0xEA, 0xE3, 0xCB, 0xFC, 0x5B, 0x2B, 0x49, 0xA1, 0x6A, 0xEC, 0xE3, 0x78, 0x03, 0x14, 0x48,
];

/// A decoded valid-pixel mask: one bit per pixel, 1 = valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidMask {
    pub mask_width: u32,
    pub mask_height: u32,
    /// Bytes per mask row.
    pub stride: u32,
    pub bits: Vec<u8>,
}

impl ValidMask {
    /// Is pixel `(x, y)` valid? Pixels outside the mask count as valid.
    pub fn is_valid(&self, x: u32, y: u32) -> bool {
        if x >= self.mask_width || y >= self.mask_height {
            return true;
        }
        let i = y as usize * self.stride as usize + (x / 8) as usize;
        self.bits.get(i).is_none_or(|b| b & (0x80 >> (x % 8)) != 0)
    }
    /// True when every pixel is valid (the mask can be ignored).
    pub fn all_valid(&self) -> bool {
        (0..self.mask_height).all(|y| (0..self.mask_width).all(|x| self.is_valid(x, y)))
    }
}

fn parse_payload(p: &[u8]) -> Option<ValidMask> {
    let (w, h, repr, stride) = (le_u32(p, 0)?, le_u32(p, 4)?, le_u32(p, 8)?, le_u32(p, 12)?);
    if repr != 0
        || w == 0
        || h == 0
        || w > 65_536
        || h > 65_536
        || u64::from(stride) * 8 < u64::from(w)
    {
        return None;
    }
    let need = (stride as usize).checked_mul(h as usize)?;
    let bits = p.get(16..16usize.checked_add(need)?)?.to_vec();
    Some(ValidMask {
        mask_width: w,
        mask_height: h,
        stride,
        bits,
    })
}

/// Find and decode the valid-pixel mask in a subblock attachment, if there is one.
/// Walks the chunk container (`GUID`, i32 size, payload)*; also accepts the layout czifile
/// handles where the GUID follows 16 leading bytes. Anything malformed yields `None`.
pub fn parse_valid_mask(att: &[u8]) -> Option<ValidMask> {
    let mut pos = 0usize;
    while pos + 20 <= att.len() {
        let guid = att.get(pos..pos + 16)?;
        let size = usize::try_from(le_i32(att, pos + 16)?).ok()?;
        let payload = att.get(pos + 20..pos.checked_add(20)?.checked_add(size)?)?;
        if guid == MASK_CHUNK_GUID {
            return parse_payload(payload);
        }
        pos = pos + 20 + size;
    }
    if att.get(16..32) == Some(&MASK_CHUNK_GUID[..]) {
        return parse_payload(att.get(32..)?);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(w: u32, h: u32, stride: u32, bits: &[u8]) -> Vec<u8> {
        let mut p = Vec::new();
        for v in [w, h, 0, stride] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        p.extend_from_slice(bits);
        let mut c = MASK_CHUNK_GUID.to_vec();
        c.extend_from_slice(&(p.len() as i32).to_le_bytes());
        c.extend_from_slice(&p);
        c
    }

    #[test]
    fn msb_first_bits_one_is_valid() {
        let m =
            parse_valid_mask(&chunk(10, 2, 2, &[0b1000_0001, 0b1100_0000, 0xFF, 0x00])).unwrap();
        assert!(m.is_valid(0, 0));
        assert!(!m.is_valid(1, 0));
        assert!(m.is_valid(7, 0));
        assert!(m.is_valid(8, 0) && m.is_valid(9, 0));
        assert!(m.is_valid(3, 1) && !m.is_valid(8, 1));
        assert!(m.is_valid(20, 20), "outside the mask counts as valid");
        assert!(!m.all_valid());
    }

    #[test]
    fn mask_after_another_chunk_and_malformed_input() {
        let mut att = vec![0xAA; 16];
        att.extend_from_slice(&4i32.to_le_bytes());
        att.extend_from_slice(&[1, 2, 3, 4]);
        att.extend(chunk(8, 1, 1, &[0xFF]));
        assert!(parse_valid_mask(&att).unwrap().all_valid());
        assert_eq!(parse_valid_mask(&[]), None);
        assert_eq!(parse_valid_mask(&[0; 40]), None);
        let mut bad = chunk(8, 1, 1, &[0xFF]);
        bad[16] = 0xFF; // negative / huge size
        bad[19] = 0x7F;
        assert_eq!(parse_valid_mask(&bad), None);
        assert_eq!(
            parse_valid_mask(&chunk(64, 1, 1, &[0xFF])),
            None,
            "stride too small"
        );
    }
}
