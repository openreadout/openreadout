//! `cargo xtask guides [--write]`: every reader crate has a maintainer guide,
//! `crates/openreadout-<name>/MAINTAINING.md` (docs/maintaining.md).
//!
//! A guide has a hand-written part (the decode pipeline, invariants and checks, how to debug a new
//! file, fragile spots) and a generated part between `<!-- BEGIN GENERATED guide -->` and
//! `<!-- END GENERATED guide -->`: the crate's formats with their confidence and corpus evidence,
//! its source files with the first line of each module's documentation, where its assurance
//! profile branches on variants (every feature it observes, every structure it reports as
//! undecoded, every value it assumes), the validated variants with the corpus files that confirm
//! them, its tests, fixtures, fuzz targets and snapshot files, and open intakes. The generated
//! part is recomputed from the repository, so it stays current: without `--write` (CI) the task
//! fails when a guide is missing, lacks a required section, or its generated part is stale.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Result, bail};

use crate::repo::{self, ReaderCrate};

/// Sections every hand-written part has.
pub const SECTIONS: [&str; 4] = [
    "## Decode pipeline",
    "## Invariants and checks",
    "## Debugging a new file",
    "## Fragile spots",
];

const TAG: &str = "guide";

/// Outputs, confirmed files, files read and example ids of one (format, kind, value).
type VariantRow = (BTreeSet<String>, u32, u32, Vec<String>);

/// Rows of the validated-variant table listed per crate (the profile has them all).
const MAX_VARIANT_ROWS: usize = 80;

pub fn run(write: bool) -> Result<()> {
    let root = crate::root();
    let crates = repo::reader_crates(&root)?;
    let ev = repo::evidence(&root)?;
    let manifest = repo::manifest(&root)?;
    let mut stale = Vec::new();
    for c in &crates {
        let path = c.dir.join("MAINTAINING.md");
        let facts = generated(&root, c, &crates, &ev, &manifest)?;
        let current = fs::read_to_string(&path).ok();
        let new = match &current {
            Some(t) => repo::replace_generated(t, TAG, &facts),
            None => repo::replace_generated(&skeleton(c), TAG, &facts),
        };
        let missing: Vec<&str> = SECTIONS
            .iter()
            .filter(|s| !new.lines().any(|l| l.trim_end() == **s))
            .copied()
            .collect();
        let rel = format!("crates/openreadout-{}/MAINTAINING.md", c.name);
        if !missing.is_empty() {
            stale.push(format!("{rel}: missing sections {missing:?}"));
        }
        let hand = new.split("<!-- BEGIN GENERATED").next().unwrap_or_default();
        if hand.contains("TODO") {
            stale.push(format!(
                "{rel}: the hand-written part still has TODO placeholders (write the decode pipeline, invariants, debugging steps and fragile spots)"
            ));
        }
        if current.as_deref() != Some(new.as_str()) {
            if write {
                fs::write(&path, &new)?;
                println!(
                    "{} {rel}",
                    if current.is_some() {
                        "updated"
                    } else {
                        "created"
                    }
                );
            } else {
                stale.push(format!(
                    "{rel}: {}",
                    if current.is_some() {
                        "generated part is stale"
                    } else {
                        "missing"
                    }
                ));
            }
        }
    }
    if !stale.is_empty() {
        bail!(
            "maintainer guides (run `cargo xtask guides --write`, then fill in any new hand-written section):\n  {}",
            stale.join("\n  ")
        );
    }
    println!("guides: {} reader crates, all current", crates.len());
    Ok(())
}

/// The hand-written part of a new guide: headings to fill in.
fn skeleton(c: &ReaderCrate) -> String {
    format!(
        "# Maintaining `openreadout-{}`\n\nFormats: {}. Start with [docs/maintaining.md](../../docs/maintaining.md) for the project-wide process.\n\n{}\n\nTODO: entry points, key structs, and where versions and variants branch.\n\n{}\n\nTODO: what the reader checks, and what it refuses (exit 4, exit 6).\n\n{}\n\nTODO: which commands and fixtures to start from.\n\n{}\n\nTODO: what is inferred, assumed or has broken before.\n",
        c.name,
        c.formats
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", "),
        SECTIONS[0],
        SECTIONS[1],
        SECTIONS[2],
        SECTIONS[3]
    )
}

/// First line of a Rust file's module documentation (`//! ...`).
fn module_doc(text: &str) -> String {
    let mut out = String::new();
    for l in text.lines() {
        let t = l.trim_start();
        if let Some(d) = t.strip_prefix("//!") {
            let d = d.trim();
            if d.is_empty() {
                if out.is_empty() {
                    continue;
                }
                break;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(d);
            if out.ends_with('.') || out.len() > 160 {
                break;
            }
        } else if out.is_empty() && (t.is_empty() || t.starts_with("#!")) {
            // inner attributes and blank lines before the documentation
        } else {
            break;
        }
    }
    // first sentence, at most 180 characters
    let first = out
        .split(". ")
        .next()
        .unwrap_or_default()
        .trim_end_matches('.');
    let mut s: String = first.chars().take(180).collect();
    if first.chars().count() > 180 {
        s.push('…');
    }
    s.replace('|', "\\|")
}

/// `(call, kind or first string argument, text)` of every `feature(`, `context(`, `undecoded(`,
/// `assumed(` and `calibration(` call in a profile.
fn profile_calls(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for call in ["feature", "context", "undecoded", "assumed", "calibration"] {
        let needle = format!(".{call}(");
        let mut rest = text;
        while let Some(i) = rest.find(&needle) {
            let after = &rest[i + needle.len()..];
            // the argument list, up to the matching parenthesis
            let mut depth = 1;
            let mut end = after.len();
            for (j, ch) in after.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = j;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let args: String = after[..end]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            rest = &after[end..];
            let label = match call {
                "feature" | "context" => {
                    let kind = args
                        .split("K::")
                        .nth(1)
                        .or_else(|| args.split("FeatureKind::").nth(1))
                        .map(|k| {
                            k.chars()
                                .take_while(char::is_ascii_alphanumeric)
                                .collect::<String>()
                        })
                        .unwrap_or_default();
                    if kind.is_empty() {
                        continue; // the definition in core, or a helper forwarding a kind
                    }
                    let value = args
                        .split_once(',')
                        .map(|(_, v)| v.rsplit_once(", &[").map_or(v, |(x, _)| x).trim())
                        .unwrap_or_default();
                    format!(
                        "{} `{}`{}",
                        snake(&kind),
                        short(value, 70),
                        if call == "context" {
                            " (descriptive)"
                        } else {
                            ""
                        }
                    )
                }
                _ => {
                    let first = args
                        .split('"')
                        .nth(1)
                        .map_or_else(|| short(&args, 60), |s| format!("\"{s}\""));
                    format!("{call} {first}")
                }
            };
            if !out.iter().any(|(_, l)| *l == label) {
                out.push((call.to_string(), label));
            }
        }
    }
    out
}

fn snake(kind: &str) -> String {
    let mut s = String::new();
    for (i, c) in kind.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            s.push('_');
        }
        s.push(c.to_ascii_lowercase());
    }
    s
}

fn short(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    let t = if s.chars().count() > n { t + "…" } else { t };
    t.replace('|', "\\|").replace('`', "'")
}

/// The generated confidence of `format` from its profile's generated block.
fn confidence(profile: &str, format: &str) -> String {
    let ident = format.to_ascii_uppercase().replace('-', "_");
    let needle = format!("const {ident}_CONFIDENCE: Confidence = Confidence::");
    profile.find(&needle).map_or_else(
        || "?".into(),
        |i| {
            profile[i + needle.len()..]
                .chars()
                .take_while(char::is_ascii_alphabetic)
                .collect::<String>()
                .to_ascii_lowercase()
        },
    )
}

fn basis(profile: &str, format: &str) -> String {
    let needle = format!("format_id: \"{format}\"");
    let Some(i) = profile.find(&needle) else {
        return "?".into();
    };
    let rest = &profile[i..];
    rest.find("basis: Basis::").map_or_else(
        || "?".into(),
        |j| {
            snake(
                &rest[j + 14..]
                    .chars()
                    .take_while(char::is_ascii_alphanumeric)
                    .collect::<String>(),
            )
            .replace('_', " ")
        },
    )
}

fn rust_files(dir: &Path) -> Vec<(String, std::path::PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.filter_map(std::result::Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let rel = p
                    .strip_prefix(dir)
                    .unwrap_or(&p)
                    .display()
                    .to_string()
                    .replace('\\', "/");
                out.push((rel, p));
            }
        }
    }
    out.sort();
    out
}

fn generated(
    root: &Path,
    c: &ReaderCrate,
    crates: &[ReaderCrate],
    ev: &repo::Evidence,
    manifest: &[repo::Entry],
) -> Result<String> {
    let profile = fs::read_to_string(c.dir.join("src/assurance.rs"))?;
    let mut s = String::from(
        "## Facts (generated)\n\n*Generated by `cargo xtask guides --write` from the sources, `corpus/manifest.toml`, `corpus/assurance/evidence.json` and `corpus/intake/`; do not edit. CI fails when it is stale.*\n\n",
    );
    formats_table(&mut s, root, c, crates, ev, &profile);
    source_map(&mut s, c);
    branches(&mut s, &profile);
    validated_variants(&mut s, c, ev);
    tests_and_fixtures(&mut s, root, c, manifest)?;
    open_intakes(&mut s, root, c)?;
    Ok(s)
}

/// Each format's notes, confidence, basis and corpus evidence.
fn formats_table(
    s: &mut String,
    root: &Path,
    c: &ReaderCrate,
    crates: &[ReaderCrate],
    ev: &repo::Evidence,
    profile: &str,
) {
    s.push_str("### Formats\n\n| format id | notes and provenance | confidence | basis | development files: read / confirmed | depositors | held-out pass / fail |\n| --- | --- | --- | --- | --- | --- | --- |\n");
    for f in &c.formats {
        let doc = repo::doc_of(root, crates, f).unwrap_or_default();
        let files: Vec<&repo::FileEvidence> = ev
            .files
            .iter()
            .filter(|x| &x.format == f && x.oracle != "unreadable")
            .collect();
        let confirmed = files
            .iter()
            .filter(|x| x.oracle == "pass" && x.independent)
            .count();
        let sources: BTreeSet<&str> = files
            .iter()
            .filter(|x| x.oracle == "pass" && x.independent && !x.synthetic)
            .map(|x| x.source.as_str())
            .collect();
        let h = ev
            .heldout
            .get(f)
            .map_or_else(|| "-".to_string(), |h| format!("{} / {}", h.pass, h.fail));
        let _ = writeln!(
            s,
            "| `{f}` | [format note](../../docs/formats/{doc}.md), [provenance log](../../docs/provenance/{doc}.md) | {} | {} | {} / {confirmed} | {} | {h} |",
            confidence(profile, f),
            basis(profile, f),
            files.len(),
            sources.len()
        );
    }
}

/// Every source file with the first sentence of its module documentation.
fn source_map(s: &mut String, c: &ReaderCrate) {
    s.push_str(
        "\n### Source map\n\n| file | what it does (its module documentation) |\n| --- | --- |\n",
    );
    for (rel, p) in rust_files(&c.dir.join("src")) {
        let doc = module_doc(&fs::read_to_string(&p).unwrap_or_default());
        let _ = writeln!(s, "| [`src/{rel}`](src/{rel}) | {doc} |");
    }
}

/// The features and structures the assurance profile reports.
fn branches(s: &mut String, profile: &str) {
    let calls = profile_calls(profile);
    s.push_str("\n### Where variants branch\n\nThe assurance profile ([`src/assurance.rs`](src/assurance.rs)) observes these features (each value is looked up in the validated table below; a value never confirmed makes the file `unvalidated` for the feature's outputs) and reports these structures and assumptions:\n\n");
    for (call, label) in &calls {
        let _ = writeln!(
            s,
            "- {}{label}",
            match call.as_str() {
                "feature" | "context" => "feature ",
                _ => "",
            }
        );
    }
    if calls.is_empty() {
        s.push_str(
            "- (the profile uses only the shared helpers of `openreadout_core::assurance`)\n",
        );
    }
}

/// Variant-feature values seen in the corpus, with the files that confirm them.
fn validated_variants(s: &mut String, c: &ReaderCrate, ev: &repo::Evidence) {
    s.push_str("\n### Validated variants and the corpus files that pin them\n\n| format | kind | value | outputs | confirmed files | read | example corpus files |\n| --- | --- | --- | --- | --- | --- | --- |\n");
    let mut rows: BTreeMap<(String, String, String), VariantRow> = BTreeMap::new();
    for x in ev.files.iter().filter(|x| c.formats.contains(&x.format)) {
        let validates = x.oracle == "pass" && x.independent;
        for (kind, value, scope) in &x.features {
            let r = rows
                .entry((x.format.clone(), kind.clone(), value.clone()))
                .or_default();
            r.0.extend(scope.iter().cloned());
            r.2 += 1;
            let counts =
                validates && (scope.is_empty() || scope.iter().any(|sc| x.compared.contains(sc)));
            if counts {
                r.1 += 1;
                if r.3.len() < 3 {
                    r.3.push(x.id.clone());
                }
            }
        }
    }
    let total = rows.len();
    for ((format, kind, value), (scope, confirmed, read, examples)) in
        rows.into_iter().take(MAX_VARIANT_ROWS)
    {
        let _ = writeln!(
            s,
            "| `{format}` | {kind} | `{}` | {} | {confirmed} | {read} | {} |",
            short(&value, 60),
            if scope.is_empty() {
                "descriptive".to_string()
            } else {
                scope.into_iter().collect::<Vec<_>>().join(", ")
            },
            examples
                .iter()
                .map(|e| format!("`{e}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if total > MAX_VARIANT_ROWS {
        let _ = writeln!(
            s,
            "\n… {} more values: the generated table in `src/assurance.rs` has all of them.",
            total - MAX_VARIANT_ROWS
        );
    }
}

/// Integration tests, fixtures, fuzz targets, corpus inputs and snapshots.
fn tests_and_fixtures(
    s: &mut String,
    root: &Path,
    c: &ReaderCrate,
    manifest: &[repo::Entry],
) -> Result<()> {
    s.push_str("\n### Tests, fixtures, fuzz targets, snapshots\n\n");
    let tests: Vec<String> = rust_files(&c.dir.join("tests"))
        .into_iter()
        .filter(|(rel, _)| !rel.contains('/'))
        .map(|(rel, _)| format!("[`tests/{rel}`](tests/{rel})"))
        .collect();
    let _ = writeln!(
        s,
        "- integration tests: {}",
        if tests.is_empty() {
            "none (unit tests in `src/`)".to_string()
        } else {
            tests.join(", ")
        }
    );
    let fixtures = c.dir.join("tests/fixtures");
    if fixtures.is_dir() {
        let mut n = 0;
        let mut stack = vec![fixtures.clone()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d)?.filter_map(std::result::Result::ok) {
                if e.path().is_dir() {
                    stack.push(e.path());
                } else {
                    n += 1;
                }
            }
        }
        let _ = writeln!(
            s,
            "- committed fixtures: {n} files in [`tests/fixtures/`](tests/fixtures) (malformed ones are replayed through every reader by `openreadout`'s `tests/fuzz_regressions.rs`; all are snapshotted by its `tests/golden.rs`)"
        );
    }
    let krate_ident = format!("openreadout_{}", c.name.replace('-', "_"));
    let mut fuzz: Vec<String> = Vec::new();
    if let Ok(rd) = fs::read_dir(root.join("fuzz/fuzz_targets")) {
        for e in rd.filter_map(std::result::Result::ok) {
            let t = fs::read_to_string(e.path()).unwrap_or_default();
            if t.contains(&krate_ident) {
                fuzz.push(format!(
                    "`{}`",
                    e.path().file_stem().unwrap_or_default().to_string_lossy()
                ));
            }
        }
    }
    fuzz.sort();
    let _ = writeln!(
        s,
        "- fuzz targets (`fuzz/fuzz_targets/`): {}",
        if fuzz.is_empty() {
            "none name this crate directly (the `whole_*` targets go through the registry)".into()
        } else {
            fuzz.join(", ")
        }
    );
    let mut tiers: BTreeMap<&str, u32> = BTreeMap::new();
    for e in manifest.iter().filter(|e| c.formats.contains(&e.format)) {
        if e.role.is_empty() || e.role == "input" || e.role == "heldout" {
            *tiers.entry(e.tier.as_str()).or_default() += 1;
        }
    }
    let _ = writeln!(
        s,
        "- corpus inputs by tier: {}",
        if tiers.is_empty() {
            "none".to_string()
        } else {
            tiers
                .iter()
                .map(|(t, n)| format!("{t} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    let snaps: Vec<String> = c
        .formats
        .iter()
        .filter(|f| root.join(format!("corpus/snapshots/{f}.jsonl")).exists())
        .map(|f| format!("[`corpus/snapshots/{f}.jsonl`](../../corpus/snapshots/{f}.jsonl)"))
        .collect();
    if !snaps.is_empty() {
        let _ = writeln!(s, "- golden snapshots: {}", snaps.join(", "));
    }
    Ok(())
}

/// New-variant intakes of the crate's formats still open.
fn open_intakes(s: &mut String, root: &Path, c: &ReaderCrate) -> Result<()> {
    let intake_dir = root.join("corpus/intake");
    let mut open = Vec::new();
    if intake_dir.is_dir() {
        let mut ps: Vec<_> = fs::read_dir(&intake_dir)?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        ps.sort();
        for p in ps {
            let t = fs::read_to_string(&p)?;
            let r: crate::variant::Record = toml::from_str(&t)?;
            if c.formats.contains(&r.format)
                && matches!(r.status.as_str(), "open" | "awaiting-file")
            {
                open.push(format!(
                    "- [`{}`](../../corpus/intake/{}.toml) ({}): {}",
                    r.id, r.id, r.status, r.level_at_intake
                ));
            }
        }
    }
    s.push_str("\n### Open new-variant intakes\n\n");
    if open.is_empty() {
        s.push_str("None.\n");
    } else {
        for o in open {
            s.push_str(&o);
            s.push('\n');
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_docs_are_first_sentences() {
        assert_eq!(
            module_doc("//! Reads the chunk map. Then more.\n//! next\nuse x;"),
            "Reads the chunk map"
        );
        assert_eq!(module_doc("#![forbid(unsafe_code)]\n//! Doc.\n"), "Doc");
        assert_eq!(module_doc("use x;\n"), "");
    }

    #[test]
    fn profile_calls_name_kinds_values_and_structures() {
        let src = r#"
            o.feature(K::Codec, &codec, &[Scope::Pixels]);
            o.feature(
                K::Layout,
                format!("loop {k}"),
                &[Scope::Pixels, Scope::Metadata],
            );
            o.undecoded("damaged container index", &[Scope::Pixels], "x");
            o.assumed("images[].size_t", n.to_string());
        "#;
        let calls: Vec<String> = profile_calls(src).into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            calls,
            [
                "codec `&codec`",
                "layout `format!(\"loop {k}\")`",
                "undecoded \"damaged container index\"",
                "assumed \"images[].size_t\""
            ]
        );
    }

    #[test]
    fn every_reader_crate_has_a_current_guide() {
        run(false).unwrap();
    }
}
