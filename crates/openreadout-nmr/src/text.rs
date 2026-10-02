//! Text decoding shared by both readers: parameter files and JCAMP-DX files are ASCII in
//! principle, UTF-8 or Latin-1 in practice.

use openreadout_core::bytes::latin1;

/// Decode bytes as UTF-8 (dropping a leading byte-order mark), falling back to Latin-1
/// byte-for-byte. The flag is true when the fallback was used.
pub fn decode_text(bytes: &[u8]) -> (String, bool) {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), false),
        Err(_) => (latin1(bytes), true),
    }
}

/// Split a line at the first `$$` (comment marker). Returns the content and the comment text.
pub(crate) fn split_comment(line: &str) -> (&str, Option<&str>) {
    match line.find("$$") {
        Some(i) => (&line[..i], Some(line[i + 2..].trim())),
        None => (line, None),
    }
}

/// Lines with their byte offsets, without the line terminator (`\n`, `\r\n` or a lone `\r`).
pub(crate) fn lines_with_offsets(text: &str) -> Vec<(u64, &str)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push((start as u64, &text[start..i]));
                start = i + 1;
            }
            b'\r' => {
                out.push((start as u64, &text[start..i]));
                if bytes.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push((start as u64, &text[start..]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding_and_lines() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFab").0, "ab");
        let (s, latin) = decode_text(b"na\xEFve");
        assert!(latin);
        assert_eq!(s, "naïve");
        let l = lines_with_offsets("a\r\nbc\rd\n\ne");
        assert_eq!(l, vec![(0, "a"), (3, "bc"), (6, "d"), (8, ""), (9, "e")]);
        assert_eq!(split_comment("12 34 $$ note"), ("12 34 ", Some("note")));
    }
}
