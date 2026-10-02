//! Experiment facts of an mzML/imzML header that the normalized model cannot carry
//! (`Dataset::experiment`): the sample the run measured, from `sampleList/sample`
//! (`@name`, else `@id`). See `docs/formats/mzml.md` § Experiment facts.

use openreadout_core::Source;
use openreadout_core::experiment::{Experiment, Origin, Sample};

use crate::xml::Node;

/// `_x0032_` → `2`: mzML ids escape characters XML ids may not start with.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("_x") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 2..];
        if let Some(c) = tail
            .get(..5)
            .filter(|h| h.ends_with('_'))
            .and_then(|h| u32::from_str_radix(&h[..4], 16).ok())
            .and_then(char::from_u32)
        {
            out.push(c);
            rest = &tail[5..];
        } else {
            out.push_str("_x");
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// The facts, or `None` when the header lists no named sample.
pub(crate) fn facts(header: &[Node]) -> Option<Experiment> {
    let list = header.iter().find(|n| n.tag == "sampleList")?;
    let samples: Vec<&Node> = list.children_named("sample").collect();
    let first = samples.first()?;
    let (value, attr) = [("name", first.attr("name")), ("id", first.attr("id"))]
        .into_iter()
        .find_map(|(k, v)| {
            v.map(unescape)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty() && v.chars().any(char::is_alphanumeric))
                .map(|v| (v, k))
        })?;
    let from = format!("sampleList/sample[0]/@{attr}");
    let mut e = Experiment {
        sample: Some(Sample {
            id: Some(value),
            source_field: Some(from.clone()),
            ..Sample::default()
        }),
        ..Experiment::default()
    };
    e.provenance.insert(
        "sample.id".into(),
        Origin {
            source: Source::Spec,
            from,
        },
    );
    if samples.len() > 1 {
        e.notes.push(format!(
            "The mzML sampleList names {} samples; `sample.id` is the first.",
            samples.len()
        ));
    }
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_ids() {
        assert_eq!(unescape("_x0031_"), "1");
        assert_eq!(
            unescape("_x0032_0090101_x0020_-_x0020_Sample_x0020_1"),
            "20090101 - Sample 1"
        );
        assert_eq!(unescape("plain_x"), "plain_x");
    }
}
