//! Namespace-tolerant helpers over `roxmltree`: elements and attributes are matched by local
//! name, since writers differ in which prefixes they bind (and FlowJo mixes prefixed and
//! unprefixed names).

use std::path::Path;

use openreadout_core::bytes::latin1;
use openreadout_core::limits::MAX_METADATA_BYTES;
pub use openreadout_core::xml::{child, children};
use openreadout_core::{Error, Result};
use roxmltree::Node;

use super::WSP_FORMAT_ID;

/// Read a gating XML file as text (UTF-8; a UTF-8 BOM is dropped), bounded in size.
pub fn read_text(path: &Path) -> Result<String> {
    let meta = std::fs::metadata(path).map_err(|e| Error::io(path, e))?;
    if meta.is_dir() {
        return Err(Error::Usage(format!(
            "{} is a directory, not a workspace or Gating-ML file",
            path.display()
        )));
    }
    if meta.len() > MAX_METADATA_BYTES {
        return Err(Error::unsupported(
            WSP_FORMAT_ID,
            format!("{} bytes of XML", meta.len()),
            "Gating files larger than 512 MiB are not read.",
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok(s.to_string()),
        // Some writers declare UTF-8 but store Latin-1 in keyword values.
        Err(_) => Ok(latin1(bytes)),
    }
}

/// Local name of the first element, found without building a tree.
pub fn root_local_name(text: &str) -> Option<String> {
    let mut rest = text;
    loop {
        let i = rest.find('<')?;
        rest = &rest[i + 1..];
        if rest.starts_with('?') || rest.starts_with('!') {
            continue;
        }
        let name: String = rest
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
            .collect();
        return Some(name.rsplit(':').next().unwrap_or(&name).to_string());
    }
}

/// Parse a document, mapping syntax errors to a corrupt-file error of `format`.
pub fn parse<'a>(text: &'a str, format: &'static str) -> Result<roxmltree::Document<'a>> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    roxmltree::Document::parse_with_options(text, opts)
        .map_err(|e| Error::corrupt(format, format!("XML: {e}")))
}

/// Attribute by local name, whatever its namespace.
pub fn attr<'a>(n: Node<'a, '_>, local: &str) -> Option<&'a str> {
    n.attributes()
        .find(|a| a.name() == local)
        .map(|a| a.value())
}

/// Element children, any name.
pub fn elements<'a, 'i>(n: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(Node::is_element)
}

/// A number attribute (surrounding spaces allowed).
pub fn num_attr(n: Node<'_, '_>, local: &str) -> Option<f64> {
    attr(n, local).and_then(|v| v.trim().parse::<f64>().ok())
}

/// Line number of a node, for error messages.
pub fn line(n: Node<'_, '_>) -> u32 {
    n.document().text_pos_at(n.range().start).row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_names() {
        assert_eq!(
            root_local_name(
                "<?xml version=\"1.0\"?>\n<!-- x -->\n<gating:Gating-ML xmlns:gating=\"u\">"
            )
            .as_deref(),
            Some("Gating-ML")
        );
        assert_eq!(
            root_local_name("<?xml version=\"1.0\"?> <Workspace version=\"20.0\">").as_deref(),
            Some("Workspace")
        );
    }
}
