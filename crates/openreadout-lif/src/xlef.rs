//! The XML-only members of the family: XLIF (one image), XLEF (experiment), XLCF (collection)
//! and XLLF (folder view). See `docs/formats/lif.md` § XML containers.
//!
//! These are plain XML files (UTF-8 or UTF-16). XLEF/XLCF/XLLF list other files under
//! `Element/Children/Reference/@File`; an XLIF's `Element/Memory` children name the files that
//! hold the frames of its memory block (`.lof` payloads, or TIFF/JPEG/PNG/BMP images).

use std::path::{Path, PathBuf};

use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};
use roxmltree::Document;

use crate::FORMAT_ID;
use crate::container::utf16le;

/// Which XML container a file is, from its `Element/Data` child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlKind {
    /// `Data/Image`: one image whose pixels live in the files its `Memory` frames name.
    Xlif,
    /// `Data/Experiment`: the root of a LAS X experiment folder.
    Xlef,
    /// `Data/Collection`: a folder of references.
    Xlcf,
    /// Folder view (`.xllf`); handled like a collection (inferred, no corpus file).
    Xllf,
}

impl XmlKind {
    /// Lowercase name used in JSON.
    pub fn name(self) -> &'static str {
        match self {
            XmlKind::Xlif => "xlif",
            XmlKind::Xlef => "xlef",
            XmlKind::Xlcf => "xlcf",
            XmlKind::Xllf => "xllf",
        }
    }
}

/// One frame of an XLIF memory block: `size` bytes at `offset` of the block, stored in `file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRef {
    /// File name as written in the XML (URL-decoded, `\` turned into `/`).
    pub file: String,
    pub offset: u64,
    pub size: u64,
    pub uuid: Option<String>,
}

/// A parsed XML container file.
#[derive(Debug, Clone)]
pub struct XmlContainer {
    pub path: PathBuf,
    pub kind: XmlKind,
    /// `Element/@Name`.
    pub name: String,
    /// The XML text (decoded).
    pub xml: String,
    /// `Element/Children/Reference/@File` values (URL-decoded, `/`-separated), in order.
    pub references: Vec<String>,
}

/// Decode an XML container's bytes: UTF-8 (with or without BOM) or UTF-16 (BOM or `<\0?\0`).
pub fn decode_xml_text(bytes: &[u8]) -> Option<String> {
    let b = bytes;
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8(b[3..].to_vec()).ok();
    }
    if b.starts_with(&[0xFF, 0xFE]) {
        return Some(utf16le(&b[2..]));
    }
    if b.starts_with(&[0xFE, 0xFF]) || b.starts_with(&[0x00, b'<']) {
        let start = usize::from(b.starts_with(&[0xFE, 0xFF])) * 2;
        let units: Vec<u16> = b[start..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return Some(String::from_utf16_lossy(&units));
    }
    if b.starts_with(&[b'<', 0x00]) {
        return Some(utf16le(b));
    }
    std::str::from_utf8(b).ok().map(str::to_string)
}

/// Signature test on the first bytes: an XML file whose root is `LMSDataContainerHeader`.
pub fn looks_like_xml_container(head: &[u8]) -> bool {
    // Decode only whole code units of the head; a truncated multi-byte UTF-8 tail is dropped.
    let text = if let Some(t) = decode_xml_text(head) {
        t
    } else {
        let valid = std::str::from_utf8(head).map_or_else(|e| e.valid_up_to(), str::len);
        match std::str::from_utf8(&head[..valid]) {
            Ok(t) => t.to_string(),
            Err(_) => return false,
        }
    };
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.starts_with('<') && t.contains("<LMSDataContainerHeader")
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Normalize a file reference from the XML: URL-decode, turn `\` into `/`, drop `./`.
pub fn normalize_reference(file: &str) -> String {
    let s = url_decode(file).replace('\\', "/");
    let parts: Vec<&str> = s
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    parts.join("/")
}

/// Resolve a normalized reference relative to `dir`. Falls back to a case-insensitive match of
/// each path component (containers written on Windows are often copied to case-sensitive disks).
pub fn resolve_reference(dir: &Path, reference: &str) -> PathBuf {
    resolve_reference_in(&Fs::local(), dir, reference)
}

/// [`resolve_reference`] in the namespace `fs`.
pub(crate) fn resolve_reference_in(fs: &Fs, dir: &Path, reference: &str) -> PathBuf {
    let direct = dir.join(reference);
    // Walk the reference through directory listings so the result names each component as the
    // file system stores it (a reference `9a.tif` to a stored `9A.tif` resolves to `9A.tif`
    // whether or not the file system ignores case).
    let mut cur = dir.to_path_buf();
    for comp in reference.split('/') {
        if comp == ".." || comp == "." || comp.is_empty() {
            cur = cur.join(comp);
            continue;
        }
        match fs.find_in_dir(&cur, comp) {
            Some(p) => cur = p,
            None => return direct,
        }
    }
    cur
}

/// Largest `.xlif`/`.xlef`/`.xlcf` document read.
const MAX_XML_CONTAINER: u64 = 256 << 20;

impl XmlContainer {
    /// Read and parse an XML container file (header only; referenced files are not opened).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_in(&Fs::local(), path)
    }

    /// Read and parse an XML container file in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        // XML containers are small (KB to a few MB); refuse anything that cannot be one.
        let bytes = openreadout_core::bytes::read_file_capped_in(
            fs,
            path,
            MAX_XML_CONTAINER,
            FORMAT_ID,
            "Leica XML container (.xlif/.xlef/.xlcf)",
        )?;
        let xml = decode_xml_text(&bytes).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, "XML container is neither UTF-8 nor UTF-16 text")
        })?;
        Self::from_xml(path, xml)
    }

    /// Parse already-decoded XML text.
    pub fn from_xml(path: &Path, xml: String) -> Result<Self> {
        let doc = Document::parse(&xml)
            .map_err(|e| Error::corrupt(FORMAT_ID, format!("XML container does not parse: {e}")))?;
        let root = doc.root_element();
        if !root.has_tag_name("LMSDataContainerHeader") {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "XML root is <{}>, expected <LMSDataContainerHeader>",
                    root.tag_name().name()
                ),
            ));
        }
        let element = root
            .children()
            .find(|n| n.is_element() && n.has_tag_name("Element"))
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "XML container has no Element"))?;
        let data = element.children().find(|n| n.has_tag_name("Data"));
        let has = |tag: &str| data.is_some_and(|d| d.children().any(|n| n.has_tag_name(tag)));
        let is_xllf = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xllf"));
        let kind = if has("Image") {
            XmlKind::Xlif
        } else if has("Collection") {
            XmlKind::Xlcf
        } else if has("Experiment") {
            XmlKind::Xlef
        } else if is_xllf {
            XmlKind::Xllf
        } else {
            XmlKind::Xlcf
        };
        let references = element
            .children()
            .filter(|n| n.has_tag_name("Children"))
            .flat_map(|c| c.children().filter(|n| n.has_tag_name("Reference")))
            .filter_map(|r| r.attribute("File"))
            .map(normalize_reference)
            .collect();
        let name = element.attribute("Name").unwrap_or("").to_string();
        drop(doc);
        Ok(XmlContainer {
            path: path.to_path_buf(),
            kind,
            name,
            xml,
            references,
        })
    }
}

/// Frames of an XLIF memory block (`Element/Memory/*[@File]`).
pub fn memory_frames(memory: roxmltree::Node<'_, '_>) -> Vec<FrameRef> {
    memory
        .children()
        .filter(roxmltree::Node::is_element)
        .filter_map(|b| {
            let file = b.attribute("File")?;
            Some(FrameRef {
                file: normalize_reference(file),
                offset: b.attribute("Offset").and_then(|v| v.trim().parse().ok())?,
                size: b.attribute("Size").and_then(|v| v.trim().parse().ok())?,
                uuid: b.attribute("UUID").map(str::to_string),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_encodings() {
        let s = "<?xml version=\"1.0\"?><LMSDataContainerHeader/>";
        assert_eq!(decode_xml_text(s.as_bytes()).unwrap(), s);
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(s.as_bytes());
        assert_eq!(decode_xml_text(&bom).unwrap(), s);
        let le: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_xml_text(&le).unwrap(), s);
        let mut le_bom = vec![0xFF, 0xFE];
        le_bom.extend_from_slice(&le);
        assert_eq!(decode_xml_text(&le_bom).unwrap(), s);
        let be: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(decode_xml_text(&be).unwrap(), s);
        assert!(looks_like_xml_container(&le));
        assert!(looks_like_xml_container(s.as_bytes()));
        assert!(!looks_like_xml_container(b"<html></html>"));
    }

    #[test]
    fn references_and_kinds() {
        let xml = r#"<LMSDataContainerHeader Version="2"><Element Name="Exp"><Data><Experiment/></Data>
            <Children><Reference File=".\Series%20001.xlif"/><Reference File="Sub\Coll.xlcf" UUID="x"/></Children>
            </Element></LMSDataContainerHeader>"#;
        let c = XmlContainer::from_xml(Path::new("a.xlef"), xml.into()).unwrap();
        assert_eq!(c.kind, XmlKind::Xlef);
        assert_eq!(c.name, "Exp");
        assert_eq!(c.references, ["Series 001.xlif", "Sub/Coll.xlcf"]);
        let xml = r#"<LMSDataContainerHeader Version="2"><Element Name="S"><Data><Image/></Data>
            <Memory Size="8" MemoryBlockID="MemBlock_1"><Frame File="S.lof" Offset="0" Size="8" UUID="u"/></Memory>
            </Element></LMSDataContainerHeader>"#;
        let c = XmlContainer::from_xml(Path::new("s.xlif"), xml.into()).unwrap();
        assert_eq!(c.kind, XmlKind::Xlif);
        let doc = Document::parse(xml).unwrap();
        let mem = doc
            .descendants()
            .find(|n| n.has_tag_name("Memory"))
            .unwrap();
        assert_eq!(
            memory_frames(mem),
            [FrameRef {
                file: "S.lof".into(),
                offset: 0,
                size: 8,
                uuid: Some("u".into())
            }]
        );
        assert!(XmlContainer::from_xml(Path::new("x.xlef"), "<Other/>".into()).is_err());
    }
}
