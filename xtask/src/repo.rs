//! What the maintenance tasks (`variant`, `guides`, `snapshot`, `health`) read about the
//! repository: format crates and their format ids, format notes and provenance logs, the corpus
//! manifest and the assurance evidence. Everything here is derived from files in the repository,
//! so the generated pages stay current by construction.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// One `[[file]]` of `corpus/manifest.toml` (the fields these tasks need).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Entry {
    pub id: String,
    pub format: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

/// Every manifest entry.
pub fn manifest(root: &Path) -> Result<Vec<Entry>> {
    let p = root.join("corpus/manifest.toml");
    let text = fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    Ok(toml::from_str::<Manifest>(&text)
        .with_context(|| format!("parse {}", p.display()))?
        .file)
}

/// The development inputs (what the corpus test compares and the assurance evidence covers):
/// inputs and depositor mzML/mzXML exports, never a held-out entry.
pub fn is_development_input(e: &Entry) -> bool {
    e.tier != crate::heldout::TIER
        && e.role != crate::heldout::ROLE
        && (e.role.is_empty()
            || e.role == "input"
            || (e.role == "oracle-export" && (e.format == "mzml" || e.format == "mzxml")))
}

/// One file of `corpus/assurance/evidence.json`.
#[derive(Debug, Clone, Deserialize)]
pub struct FileEvidence {
    pub id: String,
    pub format: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub synthetic: bool,
    pub oracle: String,
    #[serde(default)]
    pub independent: bool,
    #[serde(default)]
    pub compared: Vec<String>,
    #[serde(default)]
    pub features: Vec<(String, String, Vec<String>)>,
}

/// Held-out pass/fail counts of one format.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HeldoutCounts {
    pub pass: u32,
    pub fail: u32,
}

/// `corpus/assurance/evidence.json`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Evidence {
    pub files: Vec<FileEvidence>,
    #[serde(default)]
    pub heldout: BTreeMap<String, HeldoutCounts>,
}

/// The committed assurance evidence (empty when absent).
pub fn evidence(root: &Path) -> Result<Evidence> {
    let p = root.join("corpus/assurance/evidence.json");
    if !p.exists() {
        return Ok(Evidence::default());
    }
    let text = fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", p.display()))
}

/// A format crate: `crates/openreadout-<name>` with a `src/assurance.rs` (a reader crate).
#[derive(Debug, Clone)]
pub struct ReaderCrate {
    /// `czi`, `chrom`, `agilent-ms`.
    pub name: String,
    /// `crates/openreadout-<name>`.
    pub dir: PathBuf,
    /// Format ids its assurance profiles declare, in file order.
    pub formats: Vec<String>,
    /// Format notes (`docs/formats/<doc>.md`) of the crate, from the vocabulary table.
    pub docs: Vec<String>,
}

/// Every reader crate, sorted by name.
pub fn reader_crates(root: &Path) -> Result<Vec<ReaderCrate>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(root.join("crates"))? {
        let dir = entry?.path();
        let a = dir.join("src/assurance.rs");
        // openreadout-core holds the assurance machinery, not a reader
        if !a.exists() || dir.ends_with("openreadout-core") {
            continue;
        }
        let Some(name) = dir
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("openreadout-"))
            .map(str::to_string)
        else {
            continue;
        };
        let text = fs::read_to_string(&a)?;
        let formats = text
            .lines()
            .filter_map(|l| l.trim().strip_prefix("format_id: \""))
            .filter_map(|r| r.split('"').next())
            .map(str::to_string)
            .collect();
        let docs = crate::VOCAB
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, d)| {
                let mut v: Vec<String> = Vec::new();
                for (_, doc) in *d {
                    if !v.iter().any(|x| x == doc) {
                        v.push((*doc).to_string());
                    }
                }
                v
            })
            .unwrap_or_default();
        out.push(ReaderCrate {
            name,
            dir,
            formats,
            docs,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The crate that reads `format`.
pub fn crate_of<'a>(crates: &'a [ReaderCrate], format: &str) -> Option<&'a ReaderCrate> {
    crates
        .iter()
        .find(|c| c.formats.iter().any(|f| f == format))
}

/// The note (`docs/formats/<doc>.md`, also the provenance log's name) of `format`: the note named
/// like the format id, else the crate's note whose name the id starts with (or that starts with
/// the id), else the crate's first note.
pub fn doc_of(root: &Path, crates: &[ReaderCrate], format: &str) -> Option<String> {
    if root.join(format!("docs/formats/{format}.md")).exists() {
        return Some(format.to_string());
    }
    let c = crate_of(crates, format)?;
    // a module prefix of the vocabulary table named like the format (`nwb` → `hdf5`)
    let by_prefix = crate::VOCAB
        .iter()
        .find(|(k, _)| *k == c.name)
        .and_then(|(_, d)| {
            d.iter()
                .find(|(prefix, _)| !prefix.is_empty() && format.starts_with(prefix))
                .map(|(_, doc)| (*doc).to_string())
        });
    if by_prefix.is_some() {
        return by_prefix;
    }
    c.docs
        .iter()
        .find(|d| format.starts_with(d.as_str()) || d.starts_with(format))
        .or_else(|| c.docs.first())
        .cloned()
}

/// Today's date as `YYYY-MM-DD` (UTC), from the system clock.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Replace the text between `<!-- BEGIN GENERATED {tag} -->` and `<!-- END GENERATED {tag} -->`
/// (markers kept), or append a new block when the markers are absent.
pub fn replace_generated(text: &str, tag: &str, body: &str) -> String {
    let begin = format!("<!-- BEGIN GENERATED {tag} -->");
    let end = format!("<!-- END GENERATED {tag} -->");
    let block = format!("{begin}\n{}\n{end}", body.trim_end());
    match (text.find(&begin), text.find(&end)) {
        (Some(b), Some(e)) if e > b => format!("{}{}{}", &text[..b], block, &text[e + end.len()..]),
        _ => format!("{}\n\n{block}\n", text.trim_end()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_is_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"), "{t}");
    }

    #[test]
    fn generated_blocks_are_replaced_in_place() {
        let t = "a\n<!-- BEGIN GENERATED x -->\nold\n<!-- END GENERATED x -->\nb\n";
        assert_eq!(
            replace_generated(t, "x", "new"),
            "a\n<!-- BEGIN GENERATED x -->\nnew\n<!-- END GENERATED x -->\nb\n"
        );
        assert!(replace_generated("a", "x", "n").ends_with("<!-- END GENERATED x -->\n"));
    }

    #[test]
    fn every_reader_crate_maps_to_notes_and_formats() {
        let root = crate::root();
        let crates = reader_crates(&root).unwrap();
        assert!(crates.len() > 30);
        for c in &crates {
            assert!(
                !c.formats.is_empty(),
                "{}: no format_id in assurance.rs",
                c.name
            );
            for f in &c.formats {
                let doc = doc_of(&root, &crates, f);
                assert!(
                    doc.as_ref()
                        .is_some_and(|d| root.join(format!("docs/formats/{d}.md")).exists()),
                    "{f} ({}): no format note",
                    c.name
                );
            }
        }
    }
}
