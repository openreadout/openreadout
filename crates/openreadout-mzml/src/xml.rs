//! A small element tree on top of quick-xml, read from a byte offset of a (possibly huge) file.
//! Only the element that is asked for is materialised; everything else is streamed past.

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde_json::{Map, Value};

use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

/// quick-xml reads UTF-8 only. Files that declare a single-byte encoding (ISO-8859-1,
/// windows-1252; the imzML example files) are read through this guard, which replaces every
/// byte above 0x7F by `?`: one byte for one byte, so byte offsets stay valid, at the cost of
/// the non-ASCII characters in free text (names, addresses). Markup is ASCII in both encodings.
///
/// It also stops a single token (a tag, or the text between two tags) longer than
/// [`MAX_TOKEN`]: quick-xml holds a whole token in memory, and a small compressed input can
/// expand into one gigantic token.
#[derive(Debug)]
pub(crate) struct Guard<R> {
    inner: R,
    on: bool,
    /// Bytes since the last `<` or `>`.
    run: usize,
}

/// Longest token read. The largest real tokens are base64 binary arrays of one spectrum or
/// chromatogram, far below this.
const MAX_TOKEN: usize = 256 << 20;

impl<R: std::io::Read> std::io::Read for Guard<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        for b in &mut buf[..n] {
            if self.on && *b > 0x7f {
                *b = b'?';
            }
            if matches!(*b, b'<' | b'>') {
                self.run = 0;
            } else {
                self.run += 1;
            }
        }
        if self.run > MAX_TOKEN {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "an XML tag or text run is longer than {} MiB",
                    MAX_TOKEN >> 20
                ),
            ));
        }
        Ok(n)
    }
}

/// Does the XML declaration at the start of `f` name a single-byte encoding? Restores the
/// file position.
pub(crate) fn single_byte_encoding(f: &mut SourceFile) -> std::io::Result<bool> {
    let pos = f.stream_position()?;
    f.seek(SeekFrom::Start(0))?;
    let mut head = [0u8; 256];
    let mut n = 0;
    while n < head.len() {
        let k = std::io::Read::read(f, &mut head[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    f.seek(SeekFrom::Start(pos))?;
    let text = String::from_utf8_lossy(&head[..n]).to_ascii_lowercase();
    let decl = text
        .trim_start_matches('\u{feff}')
        .strip_prefix("<?xml")
        .and_then(|d| d.split("?>").next())
        .unwrap_or("");
    Ok(["iso-8859", "latin", "windows-125", "cp125"]
        .iter()
        .any(|e| decl.contains(e)))
}

/// A buffered, encoding-guarded reader over `f` from its current position.
pub(crate) fn guarded<R: std::io::Read>(inner: R, on: bool, cap: usize) -> BufReader<Guard<R>> {
    BufReader::with_capacity(cap, Guard { inner, on, run: 0 })
}

/// Deepest element nesting we follow (mzML needs about 8; a hostile file cannot recurse us).
const MAX_DEPTH: usize = 64;

/// One XML element: local tag name, attributes in document order, child elements, text.
#[derive(Debug, Clone, Default)]
pub(crate) struct Node {
    pub(crate) tag: String,
    pub(crate) attrs: Vec<(String, String)>,
    pub(crate) children: Vec<Node>,
    pub(crate) text: String,
    /// The subtree was cut short at a `stop_at` tag or by the end of the input.
    pub(crate) incomplete: bool,
}

impl Node {
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    pub(crate) fn child(&self, tag: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.tag == tag)
    }
    pub(crate) fn children_named<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.tag == tag)
    }
    /// Generic JSON: attributes as `@name`, children by tag (arrays when repeated), `#text`.
    pub(crate) fn to_json(&self) -> Value {
        let mut obj = Map::new();
        for (k, v) in &self.attrs {
            obj.insert(format!("@{k}"), Value::String(v.clone()));
        }
        for c in &self.children {
            let v = c.to_json();
            match obj.get_mut(&c.tag) {
                Some(Value::Array(a)) => a.push(v),
                Some(existing) => {
                    let prev = existing.take();
                    *existing = Value::Array(vec![prev, v]);
                }
                None => {
                    obj.insert(c.tag.clone(), v);
                }
            }
        }
        let t = self.text.trim();
        if !t.is_empty() {
            obj.insert("#text".into(), Value::String(t.to_string()));
        }
        Value::Object(obj)
    }
}

pub(crate) fn local(name: &str) -> &str {
    match name.rfind(':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Attributes of a start tag (entities unescaped; malformed attributes are skipped).
pub(crate) fn attrs_of(e: &BytesStart<'_>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for a in e.attributes().with_checks(false).flatten() {
        let key = local(a.key.as_ref()).to_string();
        let val = a
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_or_else(|_| a.value.to_string(), std::borrow::Cow::into_owned);
        out.push((key, val));
    }
    out
}

pub(crate) fn tag_of(e: &BytesStart<'_>) -> String {
    local(e.name().as_ref()).to_string()
}

/// Build the subtree whose start tag `start` was just read. Parsing stops early (with
/// `incomplete = true`) when a descendant tag in `stop_at` starts.
pub(crate) fn read_subtree<R: BufRead>(
    reader: &mut Reader<R>,
    start: &BytesStart<'_>,
    empty: bool,
    stop_at: &[&str],
    fmt: &'static str,
) -> Result<Node> {
    let root = Node {
        tag: tag_of(start),
        attrs: attrs_of(start),
        ..Node::default()
    };
    if empty {
        return Ok(root);
    }
    let mut stack: Vec<Node> = vec![root];
    let mut buf = Vec::new();
    loop {
        let pos = reader.buffer_position();
        let ev = reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::corrupt_at(fmt, pos, format!("XML: {e}")))?;
        match ev {
            Event::Start(e) => {
                let tag = tag_of(&e);
                if stop_at.contains(&tag.as_str()) {
                    return Ok(close_all(stack));
                }
                if stack.len() >= MAX_DEPTH {
                    return Err(Error::corrupt_at(fmt, pos, "XML nested too deeply"));
                }
                stack.push(Node {
                    tag,
                    attrs: attrs_of(&e),
                    ..Node::default()
                });
            }
            Event::Empty(e) => {
                let tag = tag_of(&e);
                if stop_at.contains(&tag.as_str()) {
                    return Ok(close_all(stack));
                }
                let n = Node {
                    tag,
                    attrs: attrs_of(&e),
                    ..Node::default()
                };
                if let Some(top) = stack.last_mut() {
                    top.children.push(n);
                }
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    let s: &str = t.as_ref();
                    top.text.push_str(s);
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    let s: &str = t.as_ref();
                    top.text.push_str(s);
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    let name: &str = r.as_ref();
                    top.text.push_str(match name {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => "",
                    });
                }
            }
            Event::End(_) => {
                let done = stack.pop().unwrap_or_default();
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => return Ok(done),
                }
            }
            Event::Eof => {
                let mut n = close_all(stack);
                n.incomplete = true;
                return Ok(n);
            }
            _ => {}
        }
        buf.clear();
    }
}

fn close_all(mut stack: Vec<Node>) -> Node {
    while stack.len() > 1 {
        let mut done = stack.pop().unwrap_or_default();
        done.incomplete = true;
        if let Some(p) = stack.last_mut() {
            p.children.push(done);
        }
    }
    let mut root = stack.pop().unwrap_or_default();
    root.incomplete = true;
    root
}

fn configure<R>(r: &mut Reader<R>) {
    let c = r.config_mut();
    c.check_end_names = false;
    c.trim_text(false);
}

/// Read the element starting exactly at `offset` of `path` (whose tag must be `tag`).
pub(crate) fn element_at(
    fs: &Fs,
    path: &Path,
    offset: u64,
    tag: &str,
    stop_at: &[&str],
    small: bool,
    fmt: &'static str,
) -> Result<Node> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    element_in(&mut f, offset, tag, stop_at, small, fmt).map_err(|e| match e {
        Error::Io { source, .. } => Error::io(path, source),
        other => other,
    })
}

/// Like `element_at`, on an open file. `small` uses a 4 KiB buffer (metadata-only reads).
pub(crate) fn element_in(
    f: &mut SourceFile,
    offset: u64,
    tag: &str,
    stop_at: &[&str],
    small: bool,
    fmt: &'static str,
) -> Result<Node> {
    let on = single_byte_encoding(f).map_err(|e| Error::io("<file>", e))?;
    f.seek(SeekFrom::Start(offset))
        .map_err(|e| Error::io("<file>", e))?;
    let cap = if small { 4096 } else { 256 * 1024 };
    let mut r = Reader::from_reader(guarded(f, on, cap));
    configure(&mut r);
    let mut buf = Vec::new();
    loop {
        let pos = r.buffer_position();
        let ev = r
            .read_event_into(&mut buf)
            .map_err(|e| Error::corrupt_at(fmt, offset + pos, format!("XML: {e}")))?;
        match ev {
            Event::Start(e) | Event::Empty(e) if tag_of(&e) != tag => {
                return Err(Error::corrupt_at(
                    fmt,
                    offset,
                    format!("expected <{tag}> at byte {offset}, found <{}>", tag_of(&e)),
                ));
            }
            Event::Start(e) => {
                let e = e.into_owned();
                return read_subtree(&mut r, &e, false, stop_at, fmt);
            }
            Event::Empty(e) => {
                let e = e.into_owned();
                return read_subtree(&mut r, &e, true, stop_at, fmt);
            }
            Event::Eof => {
                return Err(Error::corrupt_at(
                    fmt,
                    offset,
                    format!("end of file where <{tag}> was expected"),
                ));
            }
            Event::Text(t) if t.trim().is_empty() => {}
            Event::Text(_) => {
                return Err(Error::corrupt_at(
                    fmt,
                    offset,
                    format!("text where <{tag}> was expected at byte {offset}"),
                ));
            }
            _ => {}
        }
        buf.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtree_and_stop() {
        let xml = br#"<a x="1"><b>t&amp;u</b><c/><d><e/></d></a>"#;
        let mut r = Reader::from_reader(&xml[..]);
        let mut buf = Vec::new();
        let Event::Start(s) = r.read_event_into(&mut buf).unwrap() else {
            panic!()
        };
        let s = s.into_owned();
        let n = read_subtree(&mut r, &s, false, &[], "t").unwrap();
        assert_eq!(n.attr("x"), Some("1"));
        assert_eq!(n.child("b").unwrap().text, "t&u");
        assert_eq!(n.children.len(), 3);
        assert!(!n.incomplete);
        let mut r = Reader::from_reader(&xml[..]);
        let Event::Start(s) = r.read_event_into(&mut buf).unwrap() else {
            panic!()
        };
        let s = s.into_owned();
        let n = read_subtree(&mut r, &s, false, &["d"], "t").unwrap();
        assert!(n.incomplete);
        assert_eq!(n.children.len(), 2);
    }
}
