//! Attachment payloads we interpret: `CZTIMS` (time stamps) and `CZEVL` (event list).
//! Layouts derived from hex dumps (docs/provenance/czi.md, 2026-09-22) with czifile's
//! documented schemas as prior art. See `docs/formats/czi.md`.

use openreadout_core::bytes::{le_f64, le_i32, le_u32};
use openreadout_core::{Error, Result};

use crate::FORMAT_ID;

/// Largest attachment payload we read into memory for interpretation (not for
/// `export --attachment`).
pub const MAX_INTERPRETED_ATTACHMENT: u64 = 64 * 1024 * 1024;

/// One `CZEVL` record.
#[derive(Debug, Clone, PartialEq)]
pub struct EventRecord {
    /// Seconds on the same relative clock as the time stamps.
    pub time_s: f64,
    /// Raw event type code.
    pub event_code: i32,
    pub description: String,
}

impl EventRecord {
    /// Our name for the event type code.
    pub fn event_kind(&self) -> &'static str {
        match self.event_code {
            0 => "marker",
            1 => "interval_change",
            2 => "bleach_start",
            3 => "bleach_stop",
            4 => "trigger",
            _ => "unknown",
        }
    }
}

/// `CZTIMS`: u32 size (unreliable), u32 count, `count` × f64 seconds.
pub fn parse_time_stamps(b: &[u8]) -> Result<Vec<f64>> {
    let count = le_u32(b, 4)
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "TimeStamps attachment shorter than 8 bytes"))?
        as usize;
    let need = count
        .checked_mul(8)
        .and_then(|n| n.checked_add(8))
        .ok_or_else(|| Error::corrupt(FORMAT_ID, "TimeStamps count overflows"))?;
    if b.len() < need {
        return Err(Error::corrupt(
            FORMAT_ID,
            format!(
                "TimeStamps attachment declares {count} values but holds {} bytes",
                b.len()
            ),
        ));
    }
    Ok((0..count).filter_map(|i| le_f64(b, 8 + 8 * i)).collect())
}

/// `CZEVL`: u32 size, u32 count, then records of i32 `entry_size`, f64 time, i32 type,
/// i32 `description_size`, description bytes (NUL-terminated text).
pub fn parse_event_list(b: &[u8]) -> Result<Vec<EventRecord>> {
    let bad = |what: &str| Error::corrupt(FORMAT_ID, format!("EventList attachment: {what}"));
    let count = le_u32(b, 4).ok_or_else(|| bad("shorter than 8 bytes"))? as usize;
    let mut out = Vec::with_capacity(count.min(4096));
    let mut pos = 8usize;
    for i in 0..count {
        let entry_size = le_i32(b, pos).ok_or_else(|| bad(&format!("record {i} truncated")))?;
        let time_s = le_f64(b, pos + 4).ok_or_else(|| bad(&format!("record {i} truncated")))?;
        let event_code =
            le_i32(b, pos + 12).ok_or_else(|| bad(&format!("record {i} truncated")))?;
        let dsize = le_i32(b, pos + 16).ok_or_else(|| bad(&format!("record {i} truncated")))?;
        let dsize =
            usize::try_from(dsize).map_err(|_| bad(&format!("record {i}: negative text size")))?;
        let text = b
            .get(pos + 20..pos + 20 + dsize)
            .ok_or_else(|| bad(&format!("record {i} text runs past the payload")))?;
        let description = String::from_utf8_lossy(text)
            .trim_end_matches('\0')
            .to_string();
        out.push(EventRecord {
            time_s,
            event_code,
            description,
        });
        let step = usize::try_from(entry_size)
            .ok()
            .filter(|s| *s >= 20)
            .unwrap_or(20 + dsize);
        pos = pos
            .checked_add(step)
            .ok_or_else(|| bad("record size overflows"))?;
    }
    Ok(out)
}

/// File extension for an attachment's content type (for `export --attachment`).
pub fn extension_for(content_file_type: &str) -> &'static str {
    match content_file_type.to_ascii_uppercase().as_str() {
        "JPG" | "JPEG" => "jpg",
        "CZI" | "ZISRAW" => "czi",
        "CZEXP" | "CZHWS" | "CZMVM" | "CZFBMX" | "XML" => "xml",
        "ZIP-COMP" | "ZIP" => "gz",
        "PNG" => "png",
        "TIF" | "TIFF" => "tif",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_stamps_from_corpus_bytes() {
        // zenodo7015307-T-3-CH-2, TimeStamps attachment (32 bytes)
        let b = [
            0x1c, 0, 0, 0, 3, 0, 0, 0, 0xeb, 0xf2, 0xf7, 0x9c, 0x05, 0xeb, 0xcd, 0x3f, 0x91, 0x4c,
            0xe2, 0x07, 0x42, 0x4d, 0xe2, 0x3f, 0xe7, 0xd1, 0x43, 0x23, 0x8e, 0x2b, 0xed, 0x3f,
        ];
        let t = parse_time_stamps(&b).unwrap();
        assert_eq!(t.len(), 3);
        assert!((t[1] - t[0] - 0.338).abs() < 0.001, "{t:?}");
        assert!(parse_time_stamps(&b[..20]).is_err());
        assert!(parse_time_stamps(&[1, 2]).is_err());
        assert!(parse_time_stamps(&[8, 0, 0, 0, 0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn event_list_from_corpus_bytes() {
        let mut b = vec![0x30, 0, 0, 0, 1, 0, 0, 0, 0x28, 0, 0, 0];
        b.extend_from_slice(&[0x97, 0x19, 0x36, 0xca, 0xfa, 0xcd, 0x84, 0x3f]);
        b.extend_from_slice(&[0, 0, 0, 0, 0x14, 0, 0, 0]);
        b.extend_from_slice(b"IncubationRecording\0");
        let e = parse_event_list(&b).unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].description, "IncubationRecording");
        assert_eq!(e[0].event_kind(), "marker");
        assert!((e[0].time_s - 0.0102).abs() < 1e-3);
        assert!(
            parse_event_list(&[8, 0, 0, 0, 0, 0, 0, 0])
                .unwrap()
                .is_empty()
        );
        assert!(parse_event_list(&b[..30]).is_err());
        assert!(parse_event_list(&[8, 0, 0, 0, 9, 0, 0, 0]).is_err());
    }

    #[test]
    fn extensions() {
        assert_eq!(extension_for("JPG"), "jpg");
        assert_eq!(extension_for("CZI"), "czi");
        assert_eq!(extension_for("CZTIMS"), "bin");
        assert_eq!(extension_for("Zip-Comp"), "gz");
    }
}
