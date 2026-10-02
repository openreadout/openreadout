//! The HEADER segment: version identifier and ASCII byte offsets (FCS 3.1 §3.1).

/// Length of the fixed part of a HEADER: 6-byte version, 4 spaces, six 8-byte offsets.
pub const HEADER_LEN: usize = 58;

/// Version identifiers this reader understands.
pub const VERSIONS: [&str; 4] = ["FCS2.0", "FCS3.0", "FCS3.1", "FCS3.2"];

/// An inclusive byte range `[begin, end]`, relative to the start of its data set
/// (HEADER values and segment keywords) or absolute (after resolution).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SegmentRange {
    pub begin: u64,
    pub end: u64,
}

impl SegmentRange {
    pub fn new(begin: u64, end: u64) -> Self {
        SegmentRange { begin, end }
    }

    /// Both offsets zero: the writer says "no segment" (or "look in TEXT" for DATA beyond 99,999,999).
    pub fn is_unset(&self) -> bool {
        self.begin == 0 && self.end == 0
    }

    /// Number of bytes, counting both ends; `None` when `end < begin`.
    pub fn byte_len(&self) -> Option<u64> {
        self.end.checked_sub(self.begin).map(|d| d + 1)
    }

    /// Shift by the data set's absolute offset.
    pub fn absolute(&self, base: u64) -> Option<SegmentRange> {
        Some(SegmentRange {
            begin: self.begin.checked_add(base)?,
            end: self.end.checked_add(base)?,
        })
    }
}

/// A parsed HEADER.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    /// Bytes 0–5, e.g. `FCS3.1`.
    pub version: String,
    pub text: SegmentRange,
    pub data: SegmentRange,
    pub analysis: SegmentRange,
    /// Implementor-defined OTHER segments listed after byte 58.
    pub other: Vec<SegmentRange>,
    /// Size of the HEADER as read (58 plus 16 per OTHER pair).
    pub header_len: u64,
    /// Offsets that were blank (all spaces) rather than digits; they read as 0.
    pub blank_fields: Vec<&'static str>,
}

/// Why a HEADER could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderError {
    TooShort(usize),
    NoSignature,
    BadOffset { field: &'static str, raw: String },
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderError::TooShort(n) => {
                write!(f, "only {n} bytes where a 58-byte HEADER is required")
            }
            HeaderError::NoSignature => {
                f.write_str("no `FCS` version identifier at the start of the data set")
            }
            HeaderError::BadOffset { field, raw } => {
                write!(f, "HEADER field {field} is not an ASCII integer: {raw:?}")
            }
        }
    }
}

/// Does this buffer begin like an FCS HEADER (`FCS` + `d.d`)?
pub fn looks_like_fcs(head: &[u8]) -> bool {
    head.len() >= 6
        && &head[..3] == b"FCS"
        && head[3].is_ascii_digit()
        && head[4] == b'.'
        && head[5].is_ascii_digit()
}

fn ascii_offset(
    field: &'static str,
    raw: &[u8],
    blanks: &mut Vec<&'static str>,
) -> Result<u64, HeaderError> {
    let s = String::from_utf8_lossy(raw);
    let t = s.trim_matches(|c: char| c == ' ' || c == '\0');
    if t.is_empty() {
        blanks.push(field);
        return Ok(0);
    }
    t.parse::<u64>().map_err(|_| HeaderError::BadOffset {
        field,
        raw: s.into_owned(),
    })
}

/// Parse a HEADER from the first bytes of a data set. `buf` should hold at least the bytes
/// up to the earliest segment so OTHER offsets can be read; 58 bytes is the minimum.
pub fn parse_header(buf: &[u8]) -> Result<Header, HeaderError> {
    if buf.len() < HEADER_LEN {
        if buf.len() >= 3 && !looks_like_fcs(buf) {
            return Err(HeaderError::NoSignature);
        }
        return Err(HeaderError::TooShort(buf.len()));
    }
    if !looks_like_fcs(buf) {
        return Err(HeaderError::NoSignature);
    }
    let version = String::from_utf8_lossy(&buf[..6]).into_owned();
    let mut blanks = Vec::new();
    let names = [
        "text_begin",
        "text_end",
        "data_begin",
        "data_end",
        "analysis_begin",
        "analysis_end",
    ];
    let mut v = [0u64; 6];
    for (i, name) in names.iter().enumerate() {
        let start = 10 + 8 * i;
        v[i] = ascii_offset(name, &buf[start..start + 8], &mut blanks)?;
    }
    let text = SegmentRange::new(v[0], v[1]);
    let data = SegmentRange::new(v[2], v[3]);
    let analysis = SegmentRange::new(v[4], v[5]);
    // OTHER segment pairs: 16-byte groups after byte 58, before the first segment begins.
    let first_segment = [text.begin, data.begin, analysis.begin]
        .into_iter()
        .filter(|&b| b >= HEADER_LEN as u64)
        .min()
        .unwrap_or(HEADER_LEN as u64);
    let mut other = Vec::new();
    let mut pos = HEADER_LEN;
    let mut header_len = HEADER_LEN as u64;
    while (pos + 16) as u64 <= first_segment && pos + 16 <= buf.len() {
        let chunk = &buf[pos..pos + 16];
        if !chunk.iter().all(|b| b.is_ascii_digit() || *b == b' ') {
            break;
        }
        let mut ignore = Vec::new();
        let (Ok(b), Ok(e)) = (
            ascii_offset("other_begin", &chunk[..8], &mut ignore),
            ascii_offset("other_end", &chunk[8..], &mut ignore),
        ) else {
            break;
        };
        pos += 16;
        header_len = pos as u64;
        if b == 0 && e == 0 {
            continue;
        }
        other.push(SegmentRange::new(b, e));
    }
    Ok(Header {
        version,
        text,
        data,
        analysis,
        other,
        header_len,
        blank_fields: blanks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_example_header() {
        let h =
            parse_header(b"FCS3.1         256    1545    1792  202455       0       0").unwrap();
        assert_eq!(h.version, "FCS3.1");
        assert_eq!(h.text, SegmentRange::new(256, 1545));
        assert_eq!(h.text.byte_len(), Some(1290));
        assert_eq!(h.data.byte_len(), Some(200_664));
        assert!(h.analysis.is_unset());
        assert!(h.other.is_empty());
    }

    #[test]
    fn blank_fields_read_as_zero() {
        let h =
            parse_header(b"FCS3.0         256    2456                                  ").unwrap();
        assert!(h.data.is_unset());
        assert!(h.blank_fields.contains(&"data_begin"));
    }

    #[test]
    fn other_segments() {
        let mut b =
            b"FCS3.0          74    1455    1456   16680       0       0   16681   58392".to_vec();
        b.extend_from_slice(b"/$BEGIN");
        let h = parse_header(&b).unwrap();
        assert_eq!(h.other, vec![SegmentRange::new(16681, 58392)]);
        assert_eq!(h.header_len, 74);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_header(b"oi21j0abcd"), Err(HeaderError::NoSignature));
        assert!(matches!(
            parse_header(b"FCS3.0    12"),
            Err(HeaderError::TooShort(12))
        ));
        assert!(matches!(
            parse_header(b"FCS3.0         2x6    2456       0       0       0       0"),
            Err(HeaderError::BadOffset { .. })
        ));
    }
}
