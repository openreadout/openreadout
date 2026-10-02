//! TEXT (and supplemental TEXT / ANALYSIS) keyword-value parsing (FCS 3.1 §3.2).
//!
//! The first byte of the primary TEXT is the delimiter. A delimiter inside a keyword or value is
//! written twice. Keywords are case-insensitive; values are UTF-8 in 3.1 (we fall back to
//! Latin-1, byte for byte, when a value is not valid UTF-8, which older writers produce).

use std::collections::HashMap;

/// One keyword-value pair, spelled exactly as in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyword {
    pub name: String,
    pub value: String,
}

/// An ordered keyword list with case-insensitive lookup.
#[derive(Debug, Clone, Default)]
pub struct KeywordSet {
    pub entries: Vec<Keyword>,
    index: HashMap<String, usize>,
    /// Keywords that appeared more than once (the last occurrence wins for lookups).
    pub duplicates: Vec<String>,
}

impl KeywordSet {
    pub fn push(&mut self, name: String, value: String) {
        let key = name.to_ascii_uppercase();
        if self.index.contains_key(&key) {
            self.duplicates.push(name.clone());
        }
        self.index.insert(key, self.entries.len());
        self.entries.push(Keyword { name, value });
    }

    /// Value of `name`, ignoring case.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.index
            .get(&name.to_ascii_uppercase())
            .map(|&i| self.entries[i].value.as_str())
    }

    pub fn contains(&self, name: &str) -> bool {
        self.index.contains_key(&name.to_ascii_uppercase())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Result of parsing a keyword segment.
#[derive(Debug, Clone, Default)]
pub struct TextParse {
    pub keywords: KeywordSet,
    /// Count of zero-length values (written as two delimiters in a row where a value must start).
    pub empty_values: usize,
    /// A keyword with no value at the end of the segment.
    pub dangling_keyword: Option<String>,
    /// Keywords containing bytes outside printable ASCII (32–126).
    pub nonprintable_keywords: usize,
    /// Values that were not valid UTF-8 and were read as Latin-1.
    pub latin1_values: usize,
    /// The segment did not end with the delimiter (after trailing spaces/NULs were ignored).
    pub unterminated: bool,
}

fn decode(bytes: &[u8], latin1: &mut usize) -> String {
    if let Ok(s) = std::str::from_utf8(bytes) {
        s.to_string()
    } else {
        *latin1 += 1;
        openreadout_core::bytes::latin1(bytes)
    }
}

/// Parse `seg` as delimited keyword-value pairs. When `seg` starts with `delimiter` that byte is
/// the opening delimiter and is skipped (always the case for primary TEXT).
pub fn parse_keywords(seg: &[u8], delimiter: u8) -> TextParse {
    let mut out = TextParse::default();
    let mut i = usize::from(seg.first() == Some(&delimiter));
    // Ignore trailing padding (spaces or NULs) after the final delimiter; some writers include it.
    let mut end = seg.len();
    while end > i
        && (seg[end - 1] == b' ' || seg[end - 1] == 0)
        && delimiter != b' '
        && delimiter != 0
    {
        end -= 1;
    }
    let seg = &seg[..end];
    out.unterminated = seg.last() != Some(&delimiter);
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    while i < seg.len() {
        // A delimiter where a token must start ends an empty token (values may not begin with the delimiter).
        if seg[i] == delimiter {
            tokens.push(Vec::new());
            i += 1;
            continue;
        }
        let mut tok = Vec::new();
        let mut closed = false;
        while i < seg.len() {
            let b = seg[i];
            if b == delimiter {
                // A doubled delimiter is an escape, except as the segment's last two bytes, where
                // it can only be a terminator followed by an empty final value.
                if seg.get(i + 1) == Some(&delimiter) && i + 2 < seg.len() {
                    tok.push(delimiter);
                    i += 2;
                    continue;
                }
                i += 1;
                closed = true;
                break;
            }
            tok.push(b);
            i += 1;
        }
        tokens.push(tok);
        if !closed {
            break;
        }
    }
    let mut it = tokens.into_iter();
    while let Some(k) = it.next() {
        let name_bytes = k;
        match it.next() {
            Some(v) => {
                if name_bytes.is_empty() {
                    // An empty keyword cannot be recorded meaningfully; count it as an empty value.
                    out.empty_values += 1;
                    continue;
                }
                if v.is_empty() {
                    out.empty_values += 1;
                }
                if !name_bytes.iter().all(|b| (32..=126).contains(b)) {
                    out.nonprintable_keywords += 1;
                }
                let name = decode(&name_bytes, &mut out.latin1_values);
                let value = decode(&v, &mut out.latin1_values);
                out.keywords.push(name, value);
            }
            None => {
                if !name_bytes.is_empty() {
                    out.dangling_keyword = Some(decode(&name_bytes, &mut out.latin1_values));
                }
            }
        }
    }
    out
}

/// Parse a numeric keyword value: surrounding whitespace (seen in real files, forbidden by
/// FCS 3.1 §3.2.17) and leading zeros are accepted.
pub fn parse_uint(v: &str) -> Option<u64> {
    let t = v.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<u64>().ok()
}

/// Parse a floating-point keyword value (FCS 3.1 §3.2.20 lexical form, plus surrounding spaces).
pub fn parse_float(v: &str) -> Option<f64> {
    let t = v.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().filter(|x| x.is_finite())
}

/// True when a numeric keyword value carries padding the spec forbids.
pub fn is_padded(v: &str) -> bool {
    v.trim() != v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_escape_example() {
        let p = parse_keywords(b"/$SYS/RSX-11//M/$TOT/5000/", b'/');
        assert_eq!(p.keywords.get("$sys"), Some("RSX-11/M"));
        assert_eq!(p.keywords.get("$TOT"), Some("5000"));
        assert!(!p.unterminated);
        assert_eq!(p.empty_values, 0);
    }

    #[test]
    fn escaped_delimiter_at_end_of_value() {
        let p = parse_keywords(b"/A/x///B/y/", b'/');
        assert_eq!(p.keywords.get("A"), Some("x/"));
        assert_eq!(p.keywords.get("B"), Some("y"));
    }

    #[test]
    fn control_character_delimiter_and_padding() {
        let p = parse_keywords(b"\x0c$TOT\x0c83411   \x0cX\x0c1\x0c   ", 0x0c);
        assert_eq!(p.keywords.get("$TOT"), Some("83411   "));
        assert_eq!(parse_uint(p.keywords.get("$TOT").unwrap()), Some(83411));
        assert!(is_padded(p.keywords.get("$TOT").unwrap()));
        assert!(!p.unterminated);
    }

    #[test]
    fn supplemental_without_leading_delimiter() {
        let p = parse_keywords(b"SORTSTATS|abc|NEXT|1|", b'|');
        assert_eq!(p.keywords.get("SORTSTATS"), Some("abc"));
        assert_eq!(p.keywords.len(), 2);
    }

    #[test]
    fn latin1_fallback_and_duplicates() {
        let p = parse_keywords(b"/CREATOR/CELLQuest\xaa 3.3/creator/again/", b'/');
        assert_eq!(p.latin1_values, 1);
        assert_eq!(p.keywords.get("CREATOR"), Some("again"));
        assert_eq!(p.keywords.duplicates, vec!["creator".to_string()]);
        assert_eq!(p.keywords.entries[0].value, "CELLQuest\u{aa} 3.3");
    }

    #[test]
    fn empty_final_value() {
        let p = parse_keywords(b"\\A\\1\\Doc.\\\\", b'\\');
        assert_eq!(p.keywords.get("Doc."), Some(""));
        assert!(p.dangling_keyword.is_none());
        assert_eq!(p.empty_values, 1);
    }

    #[test]
    fn dangling_and_unterminated() {
        let p = parse_keywords(b"/A/1/B", b'/');
        assert_eq!(p.dangling_keyword.as_deref(), Some("B"));
        assert!(p.unterminated);
    }

    #[test]
    fn numbers() {
        assert_eq!(parse_uint("0000000000004161"), Some(4161));
        assert_eq!(parse_uint("  "), None);
        assert_eq!(parse_float(" 3.6e-2"), Some(0.036));
        assert_eq!(parse_float("xxxx"), None);
    }
}
