//! The PatchMaster "bundle" header: a signature, the writing program's version, and up to twelve
//! items, each the start, length and extension of an embedded file (`.dat` samples, `.pul` tree,
//! `.pgf` stimulus tree, ...). Layout from HEKA's published `DataFile_v9.txt`
//! (`docs/formats/heka-patchmaster.md`).

use openreadout_core::bytes::Block;
use openreadout_core::{Error, Result};

use super::HEKA_FORMAT_ID;

/// Bytes of the (v9 / v1000) bundle header.
pub const BUNDLE_HEADER_LEN: u64 = 256;
/// Items in a bundle header.
pub const BUNDLE_ITEMS: usize = 12;

/// One embedded file of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleItem {
    /// Zero-based slot in the header.
    pub slot: usize,
    /// Byte offset of the embedded file.
    pub start: u64,
    /// Its length in bytes.
    pub length: u64,
    /// Its extension (`.dat`, `.pul`, `.pgf`, `.amp`, `.sol`, `.mrk`, `.mth`, `.onl`, `.txt`).
    pub extension: String,
}

impl BundleItem {
    /// One past its last byte.
    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.length)
    }
}

/// The parsed bundle header.
#[derive(Debug, Clone)]
pub struct Bundle {
    /// `DAT2` (a filled bundle header).
    pub signature: String,
    /// The writing program's version text (`v2x90.2, 22-Nov-2016`).
    pub version: String,
    /// Time of the last modification, in PatchMaster seconds.
    pub modified: f64,
    /// Number of valid items the header claims.
    pub item_count: i32,
    /// The header's endian flag (true on Windows and Intel Macs).
    pub little_endian: bool,
    /// Items with a non-zero length or start, in slot order.
    pub items: Vec<BundleItem>,
}

impl Bundle {
    /// The item with this extension (`.pul`).
    pub fn item(&self, ext: &str) -> Option<&BundleItem> {
        self.items
            .iter()
            .find(|i| i.extension.eq_ignore_ascii_case(ext))
    }
}

/// True when `head` starts like a PatchMaster bundle (`DAT1`/`DAT2` followed by four zero bytes).
pub fn looks_like_bundle(head: &[u8]) -> bool {
    matches!(head.get(..4), Some(b"DAT1" | b"DAT2")) && head.get(4..8) == Some(&[0, 0, 0, 0][..])
}

/// Parse the 256-byte header held in `b` (origin 0).
pub fn parse_bundle(b: &Block, file_len: u64) -> Result<Bundle> {
    let sig = b
        .slice(0, 4)
        .ok_or_else(|| Error::corrupt(HEKA_FORMAT_ID, "file shorter than a bundle header"))?;
    match sig {
        b"DAT2" => {}
        b"DAT1" | b"DATA" => {
            return Err(Error::unsupported(
                HEKA_FORMAT_ID,
                "PatchMaster files without a bundle (DAT1 or DATA signature)",
                "this .dat holds only samples; its trees are in separate .pul/.pgf files next to it, which this reader does not open yet. Re-save the file as a bundle in PatchMaster (Replay → Save as bundle), or read it with Stimfit or load-heka-python.",
            ));
        }
        _ => {
            return Err(Error::corrupt(
                HEKA_FORMAT_ID,
                "no DAT2 bundle signature at byte 0",
            ));
        }
    }
    if b.len() < BUNDLE_HEADER_LEN as usize {
        return Err(Error::corrupt(
            HEKA_FORMAT_ID,
            format!(
                "the file is {} bytes long, shorter than the {BUNDLE_HEADER_LEN}-byte bundle header",
                b.len()
            ),
        ));
    }
    let version = b.text_at(8, 32).unwrap_or_default();
    let modified = b.f64_at(40).unwrap_or(f64::NAN);
    let item_count = b.i32_at(48).unwrap_or(0);
    let little_endian = b.u8_at(52).unwrap_or(0) != 0;
    // PatchMaster Next's v2000 layout keeps the version text but widens the header to 352 bytes
    // with 64-bit item offsets and writes its numeric format version at byte 56.
    if b.i32_at(56) == Some(2000) {
        return Err(Error::unsupported(
            HEKA_FORMAT_ID,
            "PatchMaster Next v2000 bundles (64-bit offsets)",
            "this bundle uses the v2000 layout of PatchMaster Next 1.6 or later, which no development file of this reader has; export the series to ABF or ASCII from PatchMaster, or read it with load-heka-python.",
        ));
    }
    if !little_endian {
        return Err(Error::unsupported(
            HEKA_FORMAT_ID,
            "big-endian (PowerPC Macintosh) PatchMaster bundles",
            "this bundle was written big-endian (a PowerPC Mac); no development file of this reader is, so its values are not read. Open it in PatchMaster on a current computer and save it again, or read it with load-heka-python.",
        ));
    }
    let mut items = Vec::new();
    for slot in 0..BUNDLE_ITEMS {
        let at = 64 + 16 * slot as u64;
        let start = b.i32_at(at).unwrap_or(0);
        let length = b.i32_at(at + 4).unwrap_or(0);
        let extension = b.text_at(at + 8, 8).unwrap_or_default();
        if start == 0 && length == 0 {
            continue;
        }
        if start < 0 || length < 0 {
            return Err(Error::corrupt_at(
                HEKA_FORMAT_ID,
                at,
                format!("bundle item {slot} ({extension}) has a negative start or length"),
            ));
        }
        let item = BundleItem {
            slot,
            start: start as u64,
            length: length as u64,
            extension,
        };
        if item.start > file_len {
            return Err(Error::corrupt_at(
                HEKA_FORMAT_ID,
                at,
                format!(
                    "bundle item {slot} ({}) starts at byte {} past the end of the file ({file_len} bytes)",
                    item.extension, item.start
                ),
            ));
        }
        items.push(item);
    }
    Ok(Bundle {
        signature: "DAT2".into(),
        version,
        modified,
        item_count,
        little_endian,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(sig: [u8; 4]) -> Vec<u8> {
        let mut b = vec![0u8; 256];
        b[..4].copy_from_slice(&sig);
        b[8..28].copy_from_slice(b"v2x90.2, 22-Nov-2016");
        b[48] = 2;
        b[52] = 1;
        // .dat at 256, 100 bytes; .pul at 356, 50 bytes
        b[64..68].copy_from_slice(&256i32.to_le_bytes());
        b[68..72].copy_from_slice(&100i32.to_le_bytes());
        b[72..76].copy_from_slice(b".dat");
        b[80..84].copy_from_slice(&356i32.to_le_bytes());
        b[84..88].copy_from_slice(&50i32.to_le_bytes());
        b[88..92].copy_from_slice(b".pul");
        b
    }

    #[test]
    fn parses_items() {
        let bytes = header(*b"DAT2");
        assert!(looks_like_bundle(&bytes));
        let b = parse_bundle(
            &Block {
                origin: 0,
                bytes: bytes.clone(),
            },
            406,
        )
        .unwrap();
        assert_eq!(b.version, "v2x90.2, 22-Nov-2016");
        assert_eq!(b.items.len(), 2);
        assert_eq!(b.item(".pul").unwrap().start, 356);
        assert_eq!(b.item(".pul").unwrap().end(), 406);
    }

    #[test]
    fn refuses_dat1_v2000_and_big_endian() {
        let bytes = header(*b"DAT1");
        let e = parse_bundle(&Block { origin: 0, bytes }, 406).unwrap_err();
        assert!(e.to_string().contains("bundle"), "{e}");
        let mut bytes = header(*b"DAT2");
        bytes[56..60].copy_from_slice(&2000i32.to_le_bytes());
        assert!(parse_bundle(&Block { origin: 0, bytes }, 406).is_err());
        let mut bytes = header(*b"DAT2");
        bytes[52] = 0;
        assert!(parse_bundle(&Block { origin: 0, bytes }, 406).is_err());
    }

    #[test]
    fn rejects_items_past_the_end() {
        let bytes = header(*b"DAT2");
        assert!(parse_bundle(&Block { origin: 0, bytes }, 300).is_err());
    }
}
