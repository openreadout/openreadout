//! Element lookups over a parsed `roxmltree` document, matching elements by local name (any
//! namespace), shared by the readers of XML metadata.

use roxmltree::Node;

/// First child element with this local name.
pub fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// Child elements with this local name, in document order.
pub fn children<'a, 'i: 'a>(
    n: Node<'a, 'i>,
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// The element reached from `n` through the first child of each name in `path`.
pub fn path<'a, 'i>(n: Node<'a, 'i>, path: &[&str]) -> Option<Node<'a, 'i>> {
    path.iter().try_fold(n, |cur, name| child(cur, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_names() {
        let doc = roxmltree::Document::parse(
            r#"<r xmlns:a="urn:a"><a:x n="1"><y/></a:x><x n="2"/>text<z/></r>"#,
        )
        .unwrap();
        let r = doc.root_element();
        assert_eq!(child(r, "x").and_then(|x| x.attribute("n")), Some("1"));
        assert_eq!(children(r, "x").count(), 2);
        assert!(path(r, &["x", "y"]).is_some());
        assert!(path(r, &["x", "z"]).is_none());
        assert!(child(r, "w").is_none());
    }
}
