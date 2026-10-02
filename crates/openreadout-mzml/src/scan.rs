//! Locating elements in a large XML file: the trailing offset index (`indexedmzML`, mzXML
//! `index`) and, when it is missing or wrong, one streaming pass over the whole file.

use std::io::{BufRead, Read, Seek, SeekFrom};
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;

use openreadout_core::source::{Fs, SourceFile};
use openreadout_core::{Error, Result};

use crate::xml::{Node, attrs_of, read_subtree, tag_of};

/// Where one spectrum (or chromatogram, or mzXML scan) starts, and its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Located {
    pub(crate) offset: u64,
    pub(crate) id: String,
}

/// The offsets written in the file's own index.
#[derive(Debug, Clone, Default)]
pub(crate) struct WrittenIndex {
    /// Where the index element starts (from `indexListOffset` / `indexOffset`).
    pub(crate) at: u64,
    /// `(index name, entries)` in file order, e.g. `spectrum`, `chromatogram`, `scan`.
    pub(crate) lists: Vec<(String, Vec<Located>)>,
}

impl WrittenIndex {
    pub(crate) fn list(&self, name: &str) -> Option<&[Located]> {
        self.lists
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_slice())
    }
}

/// Last `n` bytes of the file as text.
fn tail(file: &mut SourceFile, len: u64, n: u64) -> std::io::Result<String> {
    let start = len.saturating_sub(n);
    file.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    file.take(n).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Find `<{tag}>N</{tag}>` near the end of the file and return N.
pub(crate) fn trailer_offset(fs: &Fs, path: &Path, len: u64, tag: &str) -> Result<Option<u64>> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let t = tail(&mut f, len, 64 * 1024).map_err(|e| Error::io(path, e))?;
    let open = format!("<{tag}>");
    let Some(i) = t.rfind(&open) else {
        return Ok(None);
    };
    let rest = &t[i + open.len()..];
    let end = rest.find('<').unwrap_or(rest.len());
    Ok(rest[..end].trim().parse::<u64>().ok())
}

/// Parse the index element starting at `at` (`indexList` in mzML, `index` in mzXML). `entry`
/// is the offset element (`offset`), `id_attr` its id attribute (`idRef` / `id`).
pub(crate) fn read_written_index(
    fs: &Fs,
    path: &Path,
    at: u64,
    id_attr: &str,
    fmt: &'static str,
) -> Result<WrittenIndex> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let on = crate::xml::single_byte_encoding(&mut f).map_err(|e| Error::io(path, e))?;
    f.seek(SeekFrom::Start(at))
        .map_err(|e| Error::io(path, e))?;
    let mut r = Reader::from_reader(crate::xml::guarded(f, on, 64 * 1024));
    r.config_mut().check_end_names = false;
    let mut out = WrittenIndex {
        at,
        lists: Vec::new(),
    };
    let mut buf = Vec::new();
    let mut current: Option<usize> = None;
    let mut pending_id: Option<String> = None;
    let mut text = String::new();
    let mut first = true;
    loop {
        let pos = r.buffer_position();
        let ev = r
            .read_event_into(&mut buf)
            .map_err(|e| Error::corrupt_at(fmt, at + pos, format!("index: {e}")))?;
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let tag = tag_of(&e);
                if first {
                    first = false;
                    if tag != "indexList" && tag != "index" {
                        return Err(Error::corrupt_at(
                            fmt,
                            at,
                            format!("index offset {at} points at <{tag}>, not the index"),
                        ));
                    }
                }
                match tag.as_str() {
                    "index" => {
                        let name = attrs_of(&e)
                            .into_iter()
                            .find(|(k, _)| k == "name")
                            .map(|(_, v)| v)
                            .unwrap_or_default();
                        out.lists.push((name, Vec::new()));
                        current = Some(out.lists.len() - 1);
                    }
                    "offset" => {
                        pending_id = attrs_of(&e)
                            .into_iter()
                            .find(|(k, _)| k == id_attr)
                            .map(|(_, v)| v);
                        text.clear();
                    }
                    _ => {}
                }
            }
            Event::Text(t) if pending_id.is_some() => text.push_str(&t),
            Event::End(e) => {
                let tag = tag_of_end(e.name().as_ref()).to_string();
                if tag == "offset" {
                    if let (Some(id), Some(k)) = (pending_id.take(), current) {
                        let offset = text.trim().parse::<u64>().map_err(|_| {
                            Error::corrupt_at(
                                fmt,
                                at + pos,
                                format!("index entry for {id:?} is not a byte offset"),
                            )
                        })?;
                        out.lists[k].1.push(Located { offset, id });
                    }
                } else if tag == "index" && out.lists.len() == 1 && fmt == "mzxml" {
                    // mzXML has exactly one index; what follows is indexOffset/sha1
                    break;
                } else if tag == "indexList" {
                    break;
                }
            }
            Event::Eof => {
                if first {
                    return Err(Error::corrupt_at(
                        fmt,
                        at,
                        "index offset is past the end of the file",
                    ));
                }
                break;
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(out)
}

fn tag_of_end(name: &str) -> &str {
    crate::xml::local(name)
}

/// A start tag: local name and attributes.
pub(crate) type StartTag = (String, Vec<(String, String)>);

/// The start tag found at `offset`.
pub(crate) fn start_tag_at(
    file: &mut SourceFile,
    offset: u64,
) -> std::io::Result<Option<StartTag>> {
    let on = crate::xml::single_byte_encoding(file)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut r = Reader::from_reader(crate::xml::guarded(&mut *file, on, 1024));
    r.config_mut().check_end_names = false;
    let mut buf = Vec::new();
    loop {
        match r.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => return Ok(Some((tag_of(&e), attrs_of(&e)))),
            Ok(Event::Text(t)) if t.trim().is_empty() => {}
            Ok(_) | Err(_) => return Ok(None),
        }
        buf.clear();
    }
}

/// Check that every entry of `list` lands on a `<{tag} …>` with the listed id.
/// Returns the entries that do not.
pub(crate) fn verify_offsets(
    fs: &Fs,
    path: &Path,
    list: &[Located],
    tag: &str,
    id_attr: &str,
    sample: bool,
) -> Result<Vec<Located>> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let mut bad = Vec::new();
    let picks: Vec<usize> = if sample && list.len() > 16 {
        let n = list.len();
        (0..16).map(|k| k * (n - 1) / 15).collect()
    } else {
        (0..list.len()).collect()
    };
    for i in picks {
        let e = &list[i];
        let ok = match start_tag_at(&mut f, e.offset).map_err(|x| Error::io(path, x))? {
            Some((t, attrs)) => t == tag && attrs.iter().any(|(k, v)| k == id_attr && *v == e.id),
            None => false,
        };
        if !ok {
            bad.push(e.clone());
        }
    }
    Ok(bad)
}

/// What one streaming pass over the file found.
#[derive(Debug, Default)]
pub(crate) struct Walk {
    /// Header subtrees, in order (children of the root element before the data lists).
    pub(crate) header: Vec<Node>,
    /// Root element attributes (`mzML` or `mzXML`/`msRun`).
    pub(crate) root_attrs: Vec<(String, String)>,
    /// `indexedmzML` wrapper seen.
    pub(crate) indexed_wrapper: bool,
    /// Attributes and direct params of `run` (mzML) or `msRun` (mzXML).
    pub(crate) run: Node,
    /// `spectrumList` / `chromatogramList` start tags (mzML).
    pub(crate) lists: Vec<Node>,
    /// Located elements by tag (`spectrum`, `chromatogram`, `scan`).
    pub(crate) found: Vec<(String, Vec<Located>)>,
    /// The root element was closed (the file is complete).
    pub(crate) complete: bool,
    /// The pass stopped early at the first data element (header-only walk).
    pub(crate) stopped_early: bool,
    /// XML error that ended the pass, if any (with its byte offset).
    pub(crate) error: Option<(u64, String)>,
}

impl Walk {
    pub(crate) fn located(&self, tag: &str) -> Vec<Located> {
        self.found
            .iter()
            .find(|(t, _)| t == tag)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

/// Stream the file. Header subtrees (`header_tags`) directly under the root are materialised;
/// every start of a tag in `data_tags` is located. With `stop_at_data`, the pass ends at the
/// first data element (the header walk used when the index is good).
pub(crate) fn walk(
    fs: &Fs,
    path: &Path,
    root_tags: &[&str],
    run_tag: &str,
    header_tags: &[&str],
    list_tags: &[&str],
    data_tags: &[&str],
    stop_at_data: bool,
) -> Result<Walk> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let on = crate::xml::single_byte_encoding(&mut f).map_err(|e| Error::io(path, e))?;
    Ok(walk_reader(
        crate::xml::guarded(f, on, 256 * 1024),
        root_tags,
        run_tag,
        header_tags,
        list_tags,
        data_tags,
        stop_at_data,
    ))
}

pub(crate) fn walk_reader<R: BufRead>(
    input: R,
    root_tags: &[&str],
    run_tag: &str,
    header_tags: &[&str],
    list_tags: &[&str],
    data_tags: &[&str],
    stop_at_data: bool,
) -> Walk {
    let mut r = Reader::from_reader(input);
    r.config_mut().check_end_names = false;
    let mut w = Walk::default();
    for t in data_tags {
        w.found.push(((*t).to_string(), Vec::new()));
    }
    let mut buf = Vec::new();
    // open elements (local names), outermost first
    let mut open: Vec<String> = Vec::new();
    let mut root_depth: Option<usize> = None;
    let mut run_depth: Option<usize> = None;
    loop {
        buf.clear();
        let pos = r.buffer_position();
        let ev = match r.read_event_into(&mut buf) {
            Ok(ev) => ev,
            Err(e) => {
                w.error = Some((pos, e.to_string()));
                break;
            }
        };
        let (e, empty) = match ev {
            Event::Start(e) => (e.into_owned(), false),
            Event::Empty(e) => (e.into_owned(), true),
            Event::End(e) => {
                let tag = crate::xml::local(e.name().as_ref()).to_string();
                if root_depth == Some(open.len().saturating_sub(1))
                    && root_tags.contains(&tag.as_str())
                {
                    w.complete = true;
                }
                if run_depth == Some(open.len().saturating_sub(1)) && tag == run_tag {
                    run_depth = None;
                }
                open.pop();
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        let tag = tag_of(&e);
        let depth = open.len();
        if tag == "indexedmzML" {
            w.indexed_wrapper = true;
        }
        if root_depth.is_none() && root_tags.contains(&tag.as_str()) {
            w.root_attrs = attrs_of(&e);
            root_depth = Some(depth);
        } else if let Some(k) = data_tags.iter().position(|t| *t == tag) {
            let attrs = attrs_of(&e);
            let id = attrs
                .iter()
                .find(|(a, _)| a == "id" || a == "num")
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            w.found[k].1.push(Located { offset: pos, id });
            if stop_at_data {
                w.stopped_early = true;
                return w;
            }
        } else {
            let under_root = root_depth.is_some_and(|d| depth == d + 1);
            let under_run = run_depth.is_some_and(|d| depth == d + 1);
            if (under_root || under_run) && header_tags.contains(&tag.as_str()) {
                match read_subtree(&mut r, &e, empty, &[], "xml") {
                    Ok(n) => w.header.push(n),
                    Err(err) => {
                        w.error = Some((pos, err.to_string()));
                        break;
                    }
                }
                continue;
            }
            if (under_root || run_tag == "msRun") && tag == run_tag && run_depth.is_none() {
                w.run = Node {
                    tag: tag.clone(),
                    attrs: attrs_of(&e),
                    ..Node::default()
                };
                run_depth = Some(depth);
            } else if under_run && list_tags.contains(&tag.as_str()) {
                w.lists.push(Node {
                    tag: tag.clone(),
                    attrs: attrs_of(&e),
                    ..Node::default()
                });
            } else if under_run
                && matches!(
                    tag.as_str(),
                    "cvParam" | "userParam" | "referenceableParamGroupRef"
                )
            {
                w.run.children.push(Node {
                    tag: tag.clone(),
                    attrs: attrs_of(&e),
                    ..Node::default()
                });
            }
        }
        if !empty {
            open.push(tag);
        }
    }
    w
}
