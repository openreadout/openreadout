//! Small helpers over `roxmltree` (namespace-agnostic: elements are matched by local name).

use roxmltree::{Document, Node, ParsingOptions};

pub(crate) use openreadout_core::xml::{child, children};
use openreadout_core::{Error, Result};

/// Parse an XML document (no DTDs; a node budget large enough for any qPCR file).
pub(crate) fn parse<'a>(text: &'a str, format: &'static str, what: &str) -> Result<Document<'a>> {
    let opts = ParsingOptions {
        allow_dtd: false,
        nodes_limit: 50_000_000,
        ..ParsingOptions::default()
    };
    Document::parse_with_options(text, opts)
        .map_err(|e| Error::corrupt(format, format!("{what}: XML does not parse: {e}")))
}

/// Text of a node, trimmed; `None` when empty.
pub(crate) fn node_text(n: Node<'_, '_>) -> Option<String> {
    let t: String = n
        .children()
        .filter(roxmltree::Node::is_text)
        .filter_map(|c| c.text())
        .collect();
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Trimmed text of the first child element `name`; `None` when absent or empty.
pub(crate) fn text(n: Node<'_, '_>, name: &str) -> Option<String> {
    child(n, name).and_then(node_text)
}

/// Text of the child `name` as a finite number.
pub(crate) fn number(n: Node<'_, '_>, name: &str) -> Option<f64> {
    text(n, name).and_then(|t| crate::model::num(&t))
}

/// The `id` attribute, trimmed.
pub(crate) fn id(n: Node<'_, '_>) -> Option<String> {
    n.attribute("id")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Escape text for XML element content and attribute values.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // Characters XML 1.0 does not allow are dropped.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        let d = parse(
            r#"<r xmlns="http://x"><a id=" 1 "><b> 2.5 </b><b>x</b></a></r>"#,
            "t",
            "doc",
        )
        .unwrap();
        let a = child(d.root_element(), "a").unwrap();
        assert_eq!(id(a).as_deref(), Some("1"));
        assert_eq!(number(a, "b"), Some(2.5));
        assert_eq!(children(a, "b").count(), 2);
        assert!(parse("<a>", "t", "doc").is_err());
        assert!(parse("<!DOCTYPE a [<!ENTITY x 'y'>]><a>&x;</a>", "t", "doc").is_err());
        assert_eq!(escape("a<&\u{1}\"b"), "a&lt;&amp;&quot;b");
    }
}
