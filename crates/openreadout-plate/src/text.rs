//! Text decoding and splitting for delimited exports: byte-order marks, UTF-16, UTF-8 with a
//! Windows-1252 fallback, CR/LF/CRLF line endings, tab or comma fields with CSV quoting.

use openreadout_core::bytes::{Endian, utf16, windows1252};

/// How the bytes of a text export were decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Windows1252,
}

impl Encoding {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf-8",
            Encoding::Utf8Bom => "utf-8-bom",
            Encoding::Utf16Le => "utf-16le",
            Encoding::Utf16Be => "utf-16be",
            Encoding::Windows1252 => "windows-1252",
        }
    }
}

/// Heuristic for UTF-16 without a BOM: many NUL bytes at odd (LE) or even (BE) positions.
fn bomless_utf16(bytes: &[u8]) -> Option<Endian> {
    let n = bytes.len().min(4096) & !1;
    if n < 8 {
        return None;
    }
    let (mut even, mut odd) = (0usize, 0usize);
    for (i, b) in bytes[..n].iter().enumerate() {
        if *b == 0 {
            if i % 2 == 0 {
                even += 1;
            } else {
                odd += 1;
            }
        }
    }
    let half = n / 2;
    if odd * 10 > half * 4 && even * 10 < half {
        Some(Endian::Little)
    } else if even * 10 > half * 4 && odd * 10 < half {
        Some(Endian::Big)
    } else {
        None
    }
}

/// Decode a whole export. Never fails: undecodable bytes become U+FFFD or Windows-1252 characters.
pub(crate) fn decode(bytes: &[u8]) -> (String, Encoding) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (
            String::from_utf8_lossy(rest).into_owned(),
            Encoding::Utf8Bom,
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (utf16(rest, Endian::Little), Encoding::Utf16Le);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (utf16(rest, Endian::Big), Encoding::Utf16Be);
    }
    if let Some(order) = bomless_utf16(bytes) {
        return (
            utf16(bytes, order),
            if order == Endian::Little {
                Encoding::Utf16Le
            } else {
                Encoding::Utf16Be
            },
        );
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), Encoding::Utf8),
        // A UTF-8 file cut in the middle of a character (only possible for a sniffed head).
        Err(e) if e.error_len().is_none() => (
            String::from_utf8_lossy(&bytes[..e.valid_up_to()]).into_owned(),
            Encoding::Utf8,
        ),
        Err(_) => (windows1252(bytes), Encoding::Windows1252),
    }
}

/// Split into lines on CRLF, LF or a bare CR (all three occur, sometimes in one file).
pub(crate) fn lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\n' => {
                out.push(&text[start..i]);
                start = i + 1;
            }
            b'\r' => {
                out.push(&text[start..i]);
                if b.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < b.len() {
        out.push(&text[start..]);
    }
    out
}

/// Field separator of a delimited export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delimiter {
    Tab,
    Comma,
    Semicolon,
}

impl Delimiter {
    pub(crate) fn byte(self) -> u8 {
        match self {
            Delimiter::Tab => b'\t',
            Delimiter::Comma => b',',
            Delimiter::Semicolon => b';',
        }
    }
    pub(crate) fn name(self) -> &'static str {
        match self {
            Delimiter::Tab => "tab",
            Delimiter::Comma => "comma",
            Delimiter::Semicolon => "semicolon",
        }
    }
}

/// The delimiter that occurs in the most of the first lines (tab wins ties: exports that use
/// tabs often contain commas inside values).
pub(crate) fn sniff_delimiter(lines: &[&str]) -> Delimiter {
    let mut score = [0usize; 3];
    for l in lines.iter().take(200) {
        score[0] += usize::from(l.contains('\t'));
        score[1] += usize::from(l.contains(','));
        score[2] += usize::from(l.contains(';'));
    }
    if score[0] > 0 && score[0] * 2 >= score[1] {
        Delimiter::Tab
    } else if score[2] > score[1] {
        Delimiter::Semicolon
    } else {
        Delimiter::Comma
    }
}

/// Split one line into fields. Tabs split plainly (Gen5, SoftMax Pro and i-control do not quote);
/// comma/semicolon fields follow CSV quoting (`"a,b"`, doubled quotes).
pub(crate) fn split(line: &str, d: Delimiter) -> Vec<String> {
    if d == Delimiter::Tab {
        return line.split('\t').map(str::to_string).collect();
    }
    let sep = char::from(d.byte());
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    let mut at_start = true;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' && at_start {
            quoted = true;
            at_start = false;
        } else if c == sep {
            out.push(std::mem::take(&mut cur));
            at_start = true;
        } else {
            cur.push(c);
            at_start = false;
        }
    }
    out.push(cur);
    out
}

/// Excel's formula quoting of text that looks numeric: `="0012"` → `0012`.
pub(crate) fn unformula(s: &str) -> &str {
    let t = s.trim();
    t.strip_prefix("=\"")
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings() {
        assert_eq!(decode(b"abc").1, Encoding::Utf8);
        assert_eq!(decode(b"\xEF\xBB\xBFabc").0, "abc");
        let (s, e) = decode(b"\xFF\xFEa\0b\0");
        assert_eq!((s.as_str(), e), ("ab", Encoding::Utf16Le));
        let (s, e) = decode(b"T\xB0 600 \x80");
        assert_eq!((s.as_str(), e), ("T° 600 €", Encoding::Windows1252));
        let (s, e) = decode(b"P\0l\0a\0t\0e\0:\0\t\0x\0");
        assert_eq!((s.as_str(), e), ("Plate:\tx", Encoding::Utf16Le));
    }

    #[test]
    fn line_endings() {
        assert_eq!(lines("a\r\nb\rc\nd"), vec!["a", "b", "c", "d"]);
        assert_eq!(lines("a\n\nb\n"), vec!["a", "", "b"]);
    }

    #[test]
    fn csv_split() {
        assert_eq!(
            split("a,\"b,c\",d", Delimiter::Comma),
            vec!["a", "b,c", "d"]
        );
        assert_eq!(
            split("1,\"=\"\"LoP\"\"\",x", Delimiter::Comma),
            vec!["1", "=\"LoP\"", "x"]
        );
        assert_eq!(unformula("=\"LoP\""), "LoP");
        assert_eq!(split("a\tb\t", Delimiter::Tab), vec!["a", "b", ""]);
        assert_eq!(sniff_delimiter(&["a\tb,c", "d\te"]), Delimiter::Tab);
        assert_eq!(sniff_delimiter(&["a,b", "c,d"]), Delimiter::Comma);
    }
}
