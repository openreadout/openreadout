//! Length-prefixed strings, bounded whole-file reads, and the date and number formatting shared
//! by the chromatography readers. Every accessor returns `None` instead of panicking when the
//! bytes are not there.

use std::path::Path;

use openreadout_core::bytes::{latin1_field, utf16le_z};
use openreadout_core::{Error, Result};

/// A string stored as one length byte followed by that many single-byte characters, cut at
/// the first NUL and trimmed.
pub(crate) fn pascal8(b: &[u8], off: usize) -> Option<String> {
    let n = *b.get(off)? as usize;
    let s = b.get(off + 1..off + 1 + n)?;
    Some(latin1_field(s))
}

/// A string stored as one length byte (in characters) followed by UTF-16LE code units.
pub(crate) fn pascal16(b: &[u8], off: usize) -> Option<String> {
    let n = *b.get(off)? as usize;
    let s = b.get(off + 1..off + 1 + 2 * n)?;
    Some(utf16le_z(s).trim().to_string())
}

/// Read a whole file of the namespace `fs`, refusing files above `limit` bytes.
pub(crate) fn read_file_in(
    fs: &openreadout_core::source::Fs,
    path: &Path,
    limit: u64,
    format: &'static str,
) -> Result<Vec<u8>> {
    let meta = fs.metadata(path).map_err(|e| Error::io(path, e))?;
    if meta.len() > limit {
        return Err(Error::unsupported(
            format,
            format!("{} bytes in one file", meta.len()),
            format!(
                "Files above {limit} bytes are not loaded whole; report this file so a streaming reader can be added."
            ),
        ));
    }
    fs.read(path).map_err(|e| Error::io(path, e))
}

/// Format a date/time as ISO-8601 (no zone unless `zone_minutes` is given).
pub(crate) fn iso(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    zone_minutes: Option<i32>,
) -> Option<String> {
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut out = format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}");
    if let Some(zone) = zone_minutes {
        let sign = if zone < 0 { '-' } else { '+' };
        let offset = zone.unsigned_abs();
        out.push_str(&format!("{sign}{:02}:{:02}", offset / 60, offset % 60));
    }
    Some(out)
}

/// Round to 12 significant digits so float noise (f32 header times) does not leak into JSON.
pub(crate) fn tidy(v: f64) -> f64 {
    if !v.is_finite() || v == 0.0 {
        return v;
    }
    let mag = v.abs().log10().floor() as i32;
    let p = 10f64.powi(11 - mag);
    if !p.is_finite() || p == 0.0 {
        return v;
    }
    (v * p).round() / p
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn strings() {
        assert_eq!(pascal8(b"\x03abcdef", 0).as_deref(), Some("abc"));
        assert_eq!(pascal8(b"\x09ab", 0), None);
        assert_eq!(pascal16(b"\x02h\x00i\x00x", 0).as_deref(), Some("hi"));
        assert_eq!(
            iso(2013, 6, 28, 10, 59, 0, Some(-300)).as_deref(),
            Some("2013-06-28T10:59:00-05:00")
        );
        assert_eq!(tidy(0.1 + 0.2), 0.3);
    }
}
