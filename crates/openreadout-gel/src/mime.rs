//! A MIME multipart walker (RFC 2046) over a byte source: part headers are read and parsed,
//! bodies are only located (offset and length), so a large image part is never loaded to parse
//! the file.

use std::collections::BTreeMap;

use openreadout_core::bytes::find;

/// One part: its headers and where its body is.
#[derive(Debug, Clone)]
pub(crate) struct Part {
    /// Header names lower-cased → values.
    pub(crate) headers: BTreeMap<String, String>,
    /// Absolute offset of the body.
    pub(crate) offset: u64,
    /// Body length.
    pub(crate) len: u64,
    /// Nesting depth (0 = a part of the file's top-level multipart).
    pub(crate) depth: u32,
    /// Index of the enclosing part in the flat list, if any.
    pub(crate) parent: Option<usize>,
}

impl Part {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
    /// `Content-Description`, the Image Lab part name.
    pub(crate) fn description(&self) -> &str {
        self.header("content-description").unwrap_or("")
    }
}

/// Reads `len` bytes at `offset` (fewer at the end of the file).
pub(crate) trait Bytes {
    fn read(&self, offset: u64, len: u64) -> Result<Vec<u8>, String>;
    fn len(&self) -> u64;
}

/// Most parts walked.
const MAX_PARTS: usize = 10_000;
/// Deepest nesting followed.
const MAX_DEPTH: u32 = 8;
/// Longest header block read.
const MAX_HEADER: u64 = 64 << 10;

/// Header block at `at`: `(headers, offset after the blank line)`.
fn header_block(src: &dyn Bytes, at: u64) -> Result<(BTreeMap<String, String>, u64), String> {
    let buf = src.read(at, MAX_HEADER.min(src.len().saturating_sub(at)))?;
    let end = find(&buf, b"\r\n\r\n")
        .ok_or_else(|| format!("no end of the header block at byte {at}"))?;
    let text = String::from_utf8_lossy(&buf[..end]);
    let mut h = BTreeMap::new();
    for line in text.split("\r\n") {
        if let Some((k, v)) = line.split_once(':') {
            h.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    Ok((h, at + end as u64 + 4))
}

/// The boundary parameter of a `Content-Type` value.
pub(crate) fn boundary(content_type: &str) -> Option<String> {
    let i = content_type.to_ascii_lowercase().find("boundary=")?;
    let rest = &content_type[i + 9..];
    let b = if let Some(r) = rest.strip_prefix('"') {
        r.split('"').next()?
    } else {
        rest.split(';').next()?.trim()
    };
    (!b.is_empty() && b.len() <= 200).then(|| b.to_string())
}

/// Offset of the next `--boundary` delimiter at or after `at` (at a line start: at `at` itself or
/// after CRLF), scanning forward in chunks.
fn find_delimiter(src: &dyn Bytes, at: u64, boundary: &str) -> Result<Option<u64>, String> {
    let needle = format!("--{boundary}").into_bytes();
    let mut pos = at;
    let chunk = 1u64 << 20;
    while pos < src.len() {
        let buf = src.read(pos, chunk.min(src.len() - pos))?;
        if let Some(i) = find(&buf, &needle) {
            return Ok(Some(pos + i as u64));
        }
        if buf.len() < needle.len() {
            break;
        }
        pos += (buf.len() - needle.len() + 1) as u64;
    }
    Ok(None)
}

/// Walk the multipart body that starts at `at` with `boundary`. Returns the offset after its
/// closing delimiter.
fn walk(
    src: &dyn Bytes,
    at: u64,
    boundary: &str,
    depth: u32,
    parent: Option<usize>,
    parts: &mut Vec<Part>,
) -> Result<u64, String> {
    if depth > MAX_DEPTH {
        return Err("multipart nesting deeper than 8".into());
    }
    let delim_len = boundary.len() as u64 + 2;
    let mut pos = find_delimiter(src, at, boundary)?
        .ok_or_else(|| format!("no `--{boundary}` delimiter after byte {at}"))?;
    loop {
        if parts.len() >= MAX_PARTS {
            return Err("more than 10000 parts".into());
        }
        let after = pos + delim_len;
        let tail = src.read(after, 2)?;
        if tail == b"--" {
            return Ok(after + 2);
        }
        let start = if tail == b"\r\n" { after + 2 } else { after };
        // Image Lab ends a multipart with a plain delimiter, not `--B--`: what follows it is
        // then the enclosing multipart's delimiter, or the end of the file
        let next = src.read(start, 2)?;
        if next.is_empty() || next == b"--" {
            return Ok(start);
        }
        let (headers, body) = header_block(src, start)?;
        let ct = headers.get("content-type").cloned().unwrap_or_default();
        let index = parts.len();
        parts.push(Part {
            headers: headers.clone(),
            offset: body,
            len: 0,
            depth,
            parent,
        });
        let end = if let Some(b) = boundary_of(&ct) {
            let e = walk(src, body, &b, depth + 1, Some(index), parts)?;
            parts[index].len = e.saturating_sub(body);
            e
        } else if let Some(n) = headers
            .get("content-length")
            .and_then(|v| v.trim().parse::<u64>().ok())
        {
            let e = body
                .checked_add(n)
                .filter(|e| *e <= src.len())
                .ok_or_else(|| {
                    format!(
                        "part `{}` declares {n} bytes at {body}, past the end of the {}-byte file (truncated)",
                        headers.get("content-description").map_or("", String::as_str),
                        src.len()
                    )
                })?;
            parts[index].len = n;
            e
        } else {
            // no length: the body runs to the next delimiter (minus its CRLF)
            let next = find_delimiter(src, body, boundary)?
                .ok_or_else(|| format!("part at {body} has no closing delimiter"))?;
            let e = next.saturating_sub(2).max(body);
            parts[index].len = e - body;
            e
        };
        pos = find_delimiter(src, end, boundary)?.ok_or_else(|| {
            format!("no `--{boundary}` delimiter after the part ending at byte {end} (truncated)")
        })?;
    }
}

fn boundary_of(content_type: &str) -> Option<String> {
    content_type
        .to_ascii_lowercase()
        .starts_with("multipart/")
        .then(|| boundary(content_type))
        .flatten()
}

/// The file's top-level headers and every part, depth first in file order.
pub(crate) fn parse(src: &dyn Bytes) -> Result<(BTreeMap<String, String>, Vec<Part>), String> {
    let (top, body) = header_block(src, 0)?;
    let ct = top.get("content-type").cloned().unwrap_or_default();
    let b =
        boundary_of(&ct).ok_or_else(|| "the file is not a MIME multipart document".to_string())?;
    let mut parts = Vec::new();
    walk(src, body, &b, 0, None, &mut parts)?;
    Ok((top, parts))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Mem(Vec<u8>);
    impl Bytes for Mem {
        fn read(&self, offset: u64, len: u64) -> Result<Vec<u8>, String> {
            let a = (offset as usize).min(self.0.len());
            let b = (offset.saturating_add(len) as usize).min(self.0.len());
            Ok(self.0[a..b].to_vec())
        }
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
    }

    #[test]
    fn nested_parts_with_lengths() {
        let doc = b"MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"A\"\r\n\r\n--A\r\nContent-Type: text/xml\r\nContent-Length: 3\r\nContent-Description: X\r\n\r\n<a>\r\n--A\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\nContent-Description: S\r\n\r\n--B\r\nContent-Type: application/octet-stream\r\nContent-Length: 4\r\nContent-Description: D\r\n\r\n--B-\r\n--B--\r\n--A--\r\n";
        let (top, parts) = parse(&Mem(doc.to_vec())).unwrap();
        assert_eq!(top["mime-version"], "1.0");
        let names: Vec<_> = parts.iter().map(Part::description).collect();
        assert_eq!(names, ["X", "S", "D"]);
        assert_eq!(
            parts[2].len, 4,
            "a body that looks like a delimiter is taken by its length"
        );
        assert_eq!(parts[2].parent, Some(1));
        // truncated: clean errors
        for cut in [10, 60, 120, doc.len() - 8] {
            assert!(parse(&Mem(doc[..cut].to_vec())).is_err(), "{cut}");
        }
        assert_eq!(
            boundary("multipart/mixed; boundary=xyz"),
            Some("xyz".into())
        );
    }
}
