//! Experiment facts of a Bruker experiment directory that the normalized model cannot carry
//! (`Dataset::experiment`): the user sample fields `USERA1`…`USERA5` of `acqus`, the free-text
//! sample description the spectrometer software keeps in `pdata/<procno>/title`, and the
//! `##OWNER=` login that wrote `acqus`. See `docs/formats/bruker-nmr.md` § Experiment facts.

use std::io::Read;

use openreadout_core::Source;
use openreadout_core::bytes::latin1;
use openreadout_core::experiment::{Acquisition, Experiment, Origin, Sample};

use crate::bruker_layout::Experiment as Directory;

/// Largest `title` file read (they hold a line or two).
const TITLE_MAX: u64 = 64 * 1024;

/// Values the software writes when a field was never filled in.
fn placeholder(v: &str) -> bool {
    let v = v
        .trim()
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim();
    v.is_empty() || v.eq_ignore_ascii_case("user") || v.eq_ignore_ascii_case("none")
}

/// First non-empty line of `pdata/<procno>/title`, with its relative path.
fn title(dir: &Directory) -> Option<(String, String)> {
    for p in &dir.processing {
        let path = p.dir.join("title");
        let Ok(meta) = dir.fs.metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > TITLE_MAX {
            continue;
        }
        let mut buf = Vec::new();
        let Ok(f) = dir.fs.open(&path) else {
            continue;
        };
        if f.take(TITLE_MAX).read_to_end(&mut buf).is_err() {
            continue;
        }
        let text: String = String::from_utf8(buf).unwrap_or_else(|e| latin1(&e.into_bytes()));
        if let Some(line) = text
            .lines()
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .find(|l| !l.is_empty())
        {
            return Some((line, format!("pdata/{}/title", p.procno)));
        }
    }
    None
}

/// Does a title line read like a sample label (`Q3,3'Br2`, `Ethylene Glycol in D2O+TMS`)
/// rather than a description or an acquisition note (`13C{1H}, ns=16`)?
fn label_like(line: &str) -> bool {
    line.len() <= 64 && line.split_whitespace().count() <= 4 && !line.contains('=')
}

fn origin(from: String) -> Origin {
    Origin {
        source: Source::Inferred,
        from,
    }
}

/// The facts, or `None` when the directory records none.
pub(crate) fn facts(dir: &Directory) -> Option<Experiment> {
    let mut e = Experiment::default();
    let mut s = Sample::default();
    for k in ["USERA1", "USERA2", "USERA3", "USERA4", "USERA5"] {
        if let Some(v) = dir.acqus.text(k).filter(|v| !placeholder(v)) {
            let v = v
                .trim_start_matches('<')
                .trim_end_matches('>')
                .trim()
                .to_string();
            let from = format!("{} ##${k}", dir.acqus.name);
            s.id = Some(v);
            s.source_field = Some(from.clone());
            e.provenance.insert("sample.id".into(), origin(from));
            break;
        }
    }
    if let Some((line, from)) = title(dir) {
        if s.id.is_none() && label_like(&line) {
            s.id = Some(line.clone());
            s.source_field = Some(from.clone());
            e.provenance
                .insert("sample.id".into(), origin(from.clone()));
        }
        if s.id.as_deref() != Some(line.as_str()) {
            s.name = Some(line);
            e.provenance.insert("sample.name".into(), origin(from));
        }
    }
    if s != Sample::default() {
        e.sample = Some(s);
    }
    if let Some((_, owner)) = dir
        .acqus
        .header
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("OWNER"))
        .filter(|(_, v)| !placeholder(v))
    {
        e.acquisition = Some(Acquisition {
            operator: Some(owner.trim().to_string()),
            ..Acquisition::default()
        });
        e.provenance.insert(
            "acquisition.operator".into(),
            origin(format!("{} ##OWNER", dir.acqus.name)),
        );
    }
    (!e.is_empty()).then_some(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_lines() {
        assert!(label_like("Q3,3'Br2"));
        assert!(label_like("Ethylene Glycol in D2O+TMS"));
        assert!(!label_like("13C{1H}, ns=16"));
        assert!(!label_like("Bruker standard tube for water suppression"));
        assert!(placeholder("<user>"));
        assert!(placeholder("<>"));
        assert!(!placeholder("<sample-7>"));
    }
}
