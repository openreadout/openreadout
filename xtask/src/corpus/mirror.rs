//! `cargo xtask corpus mirror key|pack` and `corpus fetch --mirror`: a copy of the smoke-tier
//! corpus published as a GitHub release, so CI does not depend on each origin being up.
//!
//! Only files whose license allows verbatim redistribution and whose SHA-256 is in the manifest
//! go into the mirror. Fetch checks every file it takes from the mirror against the manifest, so
//! the mirror never has to be trusted on its own, and an older mirror still supplies the files
//! that have not changed since it was packed.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use sha2::{Digest, Sha256};

use super::{Entry, Manifest, download_once, human, parts_of, url_path};
use crate::{heldout, sha256_file};

/// Where `corpus fetch --mirror` looks for releases unless `--mirror-url` says otherwise.
pub(crate) const DEFAULT_BASE: &str =
    "https://github.com/openreadout/openreadout/releases/download";

/// The GitHub API listing used to find the newest mirror when the exact one is not published.
const RELEASES_API: &str =
    "https://api.github.com/repos/openreadout/openreadout/releases?per_page=100";

/// Release tags and file names start with this, followed by the mirror key.
const PREFIX: &str = "corpus-smoke-";

/// The name of the notice inside the tarball.
const NOTICE_IN_TAR: &str = "MIRROR-NOTICE.md";

/// Licenses that allow us to publish a verbatim copy. A license string must match exactly;
/// anything else (GPL and LGPL, EMBL-EBI terms, CeCILL, EUPL, Artistic, custom notices) keeps
/// coming from its origin.
const LICENSES: &[&str] = &[
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "CC0-1.0",
    "CC0-1.0 (MassIVE public dataset)",
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "MIT OR Apache-2.0",
    "LicenseRef-public-domain",
];

#[derive(Subcommand)]
pub(crate) enum MirrorCmd {
    /// Print the mirror key: a hash of the path and SHA-256 of every mirrored file.
    Key,
    /// Check the smoke-tier files in the corpus directory against the manifest and write
    /// `corpus-smoke-<key>.tar.gz`, its `.sha256` and `NOTICE.md` into `--out`.
    Pack {
        #[arg(long)]
        out: PathBuf,
    },
}

pub(crate) fn run(cmd: MirrorCmd, m: &Manifest, corpus: &Path) -> Result<()> {
    let files = mirror_files(&m.file)?;
    match cmd {
        MirrorCmd::Key => {
            println!("{}", key(&files));
            Ok(())
        }
        MirrorCmd::Pack { out } => pack(&files, corpus, &out),
    }
}

/// One file in the mirror.
#[derive(Debug, Clone)]
pub(crate) struct MirrorFile<'a> {
    /// Relative to the corpus directory, with `/` separators.
    pub(crate) path: String,
    pub(crate) sha256: String,
    /// Where the file comes from, for the notice.
    pub(crate) url: String,
    pub(crate) entry: &'a Entry,
}

fn license_allows_mirror(license: &str) -> bool {
    LICENSES.contains(&license)
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The files of one entry that go into the mirror, or none when the entry is not eligible: it
/// must be on the smoke tier, downloaded from a URL (not a bundle member or a synthetic
/// fixture), under an allowed license, and carry a SHA-256 for every file.
fn entry_files(e: &Entry) -> Result<Vec<MirrorFile<'_>>> {
    if e.tier != "smoke"
        || !e.bundle.is_empty()
        || (e.url.is_empty() && e.parts.is_empty())
        || !license_allows_mirror(&e.license)
    {
        return Ok(Vec::new());
    }
    if e.parts.is_empty() {
        if !is_sha256(&e.sha256) {
            return Ok(Vec::new());
        }
        let url = if e.zip_member.is_empty() {
            e.url.clone()
        } else {
            format!("{} (zip member {})", e.url, e.zip_member)
        };
        return Ok(vec![MirrorFile {
            path: e.filename.clone(),
            sha256: e.sha256.clone(),
            url,
            entry: e,
        }]);
    }
    let parts = parts_of(e)?;
    if parts.iter().any(|(_, sha)| !is_sha256(sha)) {
        return Ok(Vec::new());
    }
    let dir = e.filename.trim_end_matches('/');
    Ok(parts
        .into_iter()
        .map(|(rel, sha256)| MirrorFile {
            path: format!("{dir}/{rel}"),
            sha256,
            url: format!("{}{}", e.parts_url, url_path(&rel)),
            entry: e,
        })
        .collect())
}

/// Every file that goes into the mirror, sorted by path. Fails on a held-out entry or path, an
/// unsafe path, or two entries that name the same path with different checksums.
pub(crate) fn mirror_files(all: &[Entry]) -> Result<Vec<MirrorFile<'_>>> {
    let mut by_path: BTreeMap<String, MirrorFile<'_>> = BTreeMap::new();
    for e in all {
        for f in entry_files(e)? {
            if e.tier == heldout::TIER || f.path.starts_with("heldout/") {
                bail!("{}: held-out files must never be mirrored", e.id);
            }
            if f.path.starts_with('/')
                || f.path.starts_with('.')
                || f.path
                    .split('/')
                    .any(|c| c.is_empty() || c == "." || c == "..")
                || f.path.contains('\n')
            {
                bail!("{}: path {:?} cannot go into the mirror", e.id, f.path);
            }
            if let Some(prev) = by_path.get(&f.path) {
                if prev.sha256 != f.sha256 {
                    bail!(
                        "{} and {} name the same path {} with different SHA-256",
                        prev.entry.id,
                        e.id,
                        f.path
                    );
                }
                continue;
            }
            by_path.insert(f.path.clone(), f);
        }
    }
    Ok(by_path.into_values().collect())
}

/// The first 16 hex digits of a SHA-256 over `<path>  <sha256>\n` for each file, sorted by path.
pub(crate) fn key(files: &[MirrorFile<'_>]) -> String {
    let mut lines: Vec<String> = files
        .iter()
        .map(|f| format!("{}  {}\n", f.path, f.sha256))
        .collect();
    lines.sort();
    let mut h = Sha256::new();
    for l in &lines {
        h.update(l.as_bytes());
    }
    hex::encode(h.finalize())[..16].to_string()
}

fn cell(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.is_empty() {
        "-".into()
    } else {
        s.replace('|', "\\|")
    }
}

fn notice(files: &[MirrorFile<'_>], key: &str) -> String {
    let mut s = format!(
        "# OpenReadout smoke-corpus mirror {key}\n\n\
         This archive holds verbatim copies of public test files, taken unchanged from the sources \
         listed below so that OpenReadout's continuous integration does not depend on every \
         origin being reachable. Each file stays under its own license, named in its row, and \
         belongs to the people credited there. The SHA-256 of every file is recorded in \
         `corpus/manifest.toml` in the OpenReadout repository.\n\n\
         | id | path | license | attribution | source | URL |\n\
         |---|---|---|---|---|---|\n"
    );
    for f in files {
        let e = f.entry;
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {} |",
            cell(&e.id),
            cell(&f.path),
            cell(&e.license),
            cell(&e.attribution),
            cell(&e.source),
            cell(&f.url)
        );
    }
    s
}

fn pack(files: &[MirrorFile<'_>], corpus: &Path, out: &Path) -> Result<()> {
    let key = key(files);
    let name = format!("{PREFIX}{key}.tar.gz");
    let mut bad = Vec::new();
    let mut bytes = 0u64;
    for f in files {
        let p = corpus.join(&f.path);
        let regular = fs::symlink_metadata(&p).is_ok_and(|m| m.is_file());
        if !regular {
            bad.push(format!("missing    {}", f.path));
            continue;
        }
        let (sha, n) = sha256_file(&p)?;
        if sha != f.sha256 {
            bad.push(format!("SHA-256    {} (got {sha})", f.path));
        }
        bytes += n;
    }
    if !bad.is_empty() {
        for b in &bad {
            eprintln!("{b}");
        }
        bail!(
            "{} of {} files are missing or differ from the manifest in {} (run `cargo xtask corpus fetch`)",
            bad.len(),
            files.len(),
            corpus.display()
        );
    }
    println!(
        "packing {} files ({}) from {}",
        files.len(),
        human(bytes),
        corpus.display()
    );
    fs::create_dir_all(out)?;
    // tar changes directory, so the paths it gets must be absolute
    let out = &out.canonicalize()?;
    let work = out.join(format!(".pack-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let result = (|| -> Result<()> {
        let text = notice(files, &key);
        fs::write(work.join(NOTICE_IN_TAR), &text)?;
        // `./` keeps a name that starts with `-` from being read as an option.
        let mut list = String::new();
        for f in files {
            let _ = writeln!(list, "./{}", f.path);
        }
        fs::write(work.join("files.txt"), list)?;
        // BSD tar takes no option after the first file name and reads a `-T` list from the
        // last `-C` directory, so the notice and the files go in with two runs of tar.
        let body = work.join("body.tar");
        tar(
            &work,
            &["-cf".as_ref(), body.as_os_str(), NOTICE_IN_TAR.as_ref()],
        )?;
        tar(
            &corpus.canonicalize()?,
            &[
                "-rhf".as_ref(),
                body.as_os_str(),
                "-T".as_ref(),
                work.join("files.txt").as_os_str(),
            ],
        )?;
        let partial = work.join(&name);
        let mut gz = flate2::write::GzEncoder::new(
            fs::File::create(&partial)?,
            flate2::Compression::default(),
        );
        std::io::copy(&mut fs::File::open(&body)?, &mut gz)?;
        gz.finish()?.sync_all()?;
        fs::remove_file(&body)?;
        let (sha, size) = sha256_file(&partial)?;
        fs::rename(&partial, out.join(&name))?;
        fs::write(
            out.join(format!("{name}.sha256")),
            format!("{sha}  {name}\n"),
        )?;
        fs::write(out.join("NOTICE.md"), text)?;
        println!(
            "wrote {} ({}, sha256 {sha})",
            out.join(&name).display(),
            human(size)
        );
        Ok(())
    })();
    let _ = fs::remove_dir_all(&work);
    result
}

/// Run tar in `dir`.
fn tar(dir: &Path, args: &[&std::ffi::OsStr]) -> Result<()> {
    let status = Command::new("tar")
        .current_dir(dir)
        // macOS tar would otherwise add `._` files for extended attributes
        .env("COPYFILE_DISABLE", "1")
        .args(args)
        .status()
        .context("run tar")?;
    if !status.success() {
        bail!("tar {args:?} failed ({status})");
    }
    Ok(())
}

/// The staging directory `corpus fetch --mirror` extracts the mirror into.
pub(crate) fn stage_dir(corpus: &Path) -> PathBuf {
    corpus.join(".mirror")
}

/// Download and unpack the smoke mirror into `<corpus>/.mirror/files`. Returns the directory
/// to look up files in, or `None` (after printing one line) when the mirror cannot be used.
/// Never fails: fetch then takes every file from its origin, as without `--mirror`.
pub(crate) fn stage(all: &[Entry], corpus: &Path, base: &str) -> Option<PathBuf> {
    match try_stage(all, corpus, base) {
        Ok(dir) => dir,
        Err(e) => {
            println!("mirror   not used ({e:#}); fetching from the origins");
            None
        }
    }
}

fn try_stage(all: &[Entry], corpus: &Path, base: &str) -> Result<Option<PathBuf>> {
    let files = mirror_files(all)?;
    let need = files
        .iter()
        .filter(|f| {
            !corpus
                .join(".receipts")
                .join(format!("{}.ok", f.entry.filename))
                .exists()
        })
        .count();
    if need == 0 {
        println!("mirror   not needed (every mirrored file is present)");
        return Ok(None);
    }
    let key = key(&files);
    let base = base.trim_end_matches('/');
    let stage = stage_dir(corpus);
    fs::create_dir_all(&stage)?;
    let exact = format!("{PREFIX}{key}");
    let (tag, sums) = match fetch_text(&format!("{base}/{exact}/{exact}.tar.gz.sha256"), &stage) {
        Ok(text) => (exact, text),
        Err(e) if e.downcast_ref::<NotFound>().is_some() => {
            let newest = newest_tag()?
                .with_context(|| format!("no release {exact} and no older {PREFIX}* release"))?;
            println!("mirror   no release for key {key}; using {newest}");
            let text = fetch_text(&format!("{base}/{newest}/{newest}.tar.gz.sha256"), &stage)?;
            (newest, text)
        }
        Err(e) => return Err(e),
    };
    let want = sums
        .split_whitespace()
        .next()
        .filter(|s| is_sha256(s))
        .with_context(|| format!("{tag}.tar.gz.sha256 holds no SHA-256"))?
        .to_ascii_lowercase();
    let tarball = stage.join(format!("{tag}.tar.gz"));
    println!("mirror   downloading {tag}.tar.gz");
    download_with_retries(&format!("{base}/{tag}/{tag}.tar.gz"), &tarball)?;
    let got = sha256_file(&tarball)?.0;
    if got != want {
        fs::remove_file(&tarball).ok();
        bail!("{tag}.tar.gz has SHA-256 {got}, expected {want}");
    }
    let files_dir = stage.join("files");
    let _ = fs::remove_dir_all(&files_dir);
    fs::create_dir_all(&files_dir)?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&files_dir)
        .status()
        .context("run tar")?;
    if !status.success() {
        bail!("could not extract {tag}.tar.gz ({status})");
    }
    fs::remove_file(&tarball).ok();
    println!("mirror   unpacked {tag}; each file is checked against the manifest before use");
    Ok(Some(files_dir))
}

/// Remove the staging directory, whatever is left in it.
pub(crate) fn cleanup(corpus: &Path) {
    let _ = fs::remove_dir_all(stage_dir(corpus));
}

/// Move the staged copy of `rel` to `dst` when its SHA-256 is `sha256`. Returns false, leaving
/// `dst` alone, when there is no stage, no such regular file, or the checksum differs.
pub(crate) fn take_staged(stage: Option<&Path>, rel: &str, sha256: &str, dst: &Path) -> bool {
    let Some(stage) = stage else {
        return false;
    };
    if !is_sha256(sha256) {
        return false;
    }
    let src = stage.join(rel);
    if !fs::symlink_metadata(&src).is_ok_and(|m| m.is_file()) {
        return false;
    }
    if sha256_file(&src).map(|(s, _)| s).ok().as_deref() != Some(sha256) {
        return false;
    }
    if let Some(parent) = dst.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    fs::rename(&src, dst).is_ok() || fs::copy(&src, dst).is_ok()
}

/// An HTTP 404: the mirror for this key was never published.
#[derive(Debug)]
struct NotFound;

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTP 404")
    }
}

impl std::error::Error for NotFound {}

/// Download `url` to `dst`, resuming and retrying with backoff. A 404 ends it at once.
fn download_with_retries(url: &str, dst: &Path) -> Result<()> {
    const ATTEMPTS: u32 = 5;
    let mut last = None;
    for attempt in 1..=ATTEMPTS {
        match download_once(url, dst) {
            Ok(()) => return Ok(()),
            Err(e) if e.to_string().starts_with("HTTP 404") => {
                return Err(anyhow::Error::new(NotFound).context(url.to_string()));
            }
            Err(e) => {
                eprintln!("  mirror attempt {attempt}/{ATTEMPTS}: {e:#}");
                last = Some(e);
                if attempt < ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_secs(2u64 << attempt));
                }
            }
        }
    }
    Err(last.expect("at least one attempt"))
}

/// A small text file, downloaded with retries.
fn fetch_text(url: &str, stage: &Path) -> Result<String> {
    let tmp = stage.join("download.txt");
    fs::remove_file(&tmp).ok();
    download_with_retries(url, &tmp)?;
    let text = fs::read_to_string(&tmp)?;
    fs::remove_file(&tmp).ok();
    Ok(text)
}

/// The tag of the newest published `corpus-smoke-*` release, from the GitHub API. Uses
/// `GITHUB_TOKEN` when it is set, which raises the rate limit.
fn newest_tag() -> Result<Option<String>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "openreadout-xtask");
    if let Ok(token) = std::env::var("GITHUB_TOKEN")
        && !token.is_empty()
    {
        req = req.header("Authorization", &format!("Bearer {token}"));
    }
    let mut resp = req.call().context("GitHub releases API")?;
    let status = resp.status().as_u16();
    if status != 200 {
        bail!("GitHub releases API answered HTTP {status}");
    }
    let releases: serde_json::Value = serde_json::from_str(&resp.body_mut().read_to_string()?)?;
    Ok(newest_mirror_tag(&releases))
}

/// The newest non-draft release whose tag starts with `corpus-smoke-`, by publication date.
fn newest_mirror_tag(releases: &serde_json::Value) -> Option<String> {
    releases
        .as_array()?
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            let when = r["published_at"].as_str().unwrap_or("");
            tag.starts_with(PREFIX)
                .then(|| (when.to_string(), tag.to_string()))
        })
        .max()
        .map(|(_, tag)| tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, filename: &str, license: &str, sha256: &str) -> Entry {
        Entry {
            id: id.into(),
            format: "abf".into(),
            tier: "smoke".into(),
            filename: filename.into(),
            url: format!("https://example.org/{filename}"),
            license: license.into(),
            sha256: sha256.into(),
            role: "input".into(),
            ..Entry::default()
        }
    }

    const A: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const B: &str = "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532";

    #[test]
    fn only_allowed_licenses_are_mirrored() {
        let all = vec![
            entry("a", "a.abf", "CC-BY-4.0", A),
            entry("b", "b.abf", "GPL-3.0-only", A),
            entry("c", "c.abf", "EMBL-EBI-Terms-of-Use", A),
            entry("d", "d.abf", "", A),
            entry("e", "e.abf", "MIT OR Apache-2.0", A),
            entry("f", "f.abf", "cc-by-4.0", A),
        ];
        let ids: Vec<&str> = mirror_files(&all)
            .unwrap()
            .iter()
            .map(|f| f.entry.id.as_str())
            .collect();
        assert_eq!(ids, ["a", "e"]);
    }

    #[test]
    fn needs_url_checksum_and_smoke_tier() {
        let no_sha = entry("a", "a.abf", "MIT", "");
        let mut member = entry("b", "b/x.abf", "MIT", A);
        member.bundle = "bz".into();
        member.url.clear();
        let mut synthetic = entry("c", "c.abf", "MIT", A);
        synthetic.url.clear();
        let mut standard = entry("d", "d.abf", "MIT", A);
        standard.tier = "standard".into();
        let mut bundle = entry("bz", "bz.zip", "MIT", B);
        bundle.role = "bundle".into();
        let mut parts = entry("p", "p.raw", "MIT", "");
        parts.url.clear();
        parts.parts_url = "https://example.org/p.raw/".into();
        parts.parts = vec![format!("_FUNC001.DAT  {A}"), format!("sub/x y  {B}")];
        let mut half = parts.clone();
        half.id = "half".into();
        half.filename = "half.raw".into();
        half.parts = vec![format!("one  {A}"), "two".into()];
        let all = vec![no_sha, member, synthetic, standard, bundle, parts];
        let got: Vec<(String, String)> = mirror_files(&all)
            .unwrap()
            .iter()
            .map(|f| (f.path.clone(), f.url.clone()))
            .collect();
        assert_eq!(
            got,
            [
                ("bz.zip".into(), "https://example.org/bz.zip".into()),
                (
                    "p.raw/_FUNC001.DAT".into(),
                    "https://example.org/p.raw/_FUNC001.DAT".into()
                ),
                (
                    "p.raw/sub/x y".into(),
                    "https://example.org/p.raw/sub/x%20y".into()
                ),
            ]
        );
        // a part without a checksum is an error in the manifest itself
        assert!(mirror_files(&[half]).is_err());
    }

    #[test]
    fn held_out_files_are_refused() {
        let mut held = entry("h", "heldout/h.abf", "MIT", A);
        held.tier = heldout::TIER.into();
        // a held-out tier entry is simply not part of the smoke mirror
        assert!(
            mirror_files(std::slice::from_ref(&held))
                .unwrap()
                .is_empty()
        );
        // a smoke entry under heldout/ is a manifest mistake and stops the mirror
        held.tier = "smoke".into();
        assert!(mirror_files(&[held]).is_err());
        let mut parts = entry("p", "heldout/p.raw", "MIT", "");
        parts.url.clear();
        parts.parts_url = "https://example.org/p.raw/".into();
        parts.parts = vec![format!("x  {A}")];
        assert!(mirror_files(&[parts]).is_err());
    }

    #[test]
    fn key_ignores_manifest_order() {
        let x = vec![
            entry("a", "a.abf", "MIT", A),
            entry("b", "b.abf", "CC0-1.0", B),
            entry("c", "c.abf", "GPL-3.0", A),
        ];
        let mut y = x.clone();
        y.reverse();
        let kx = key(&mirror_files(&x).unwrap());
        assert_eq!(kx, key(&mirror_files(&y).unwrap()));
        assert_eq!(kx.len(), 16);
        // the excluded GPL entry does not count
        assert_eq!(kx, key(&mirror_files(&x[..2]).unwrap()));
        // a changed checksum changes the key
        let mut z = x.clone();
        z[0].sha256 = B.into();
        assert_ne!(kx, key(&mirror_files(&z).unwrap()));
        // so does reordering the files given to `key` directly
        let mut files = mirror_files(&x).unwrap();
        files.reverse();
        assert_eq!(kx, key(&files));
    }

    #[test]
    fn staged_file_with_wrong_checksum_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let stage = tmp.path().join("stage");
        fs::create_dir_all(stage.join("d")).unwrap();
        fs::write(stage.join("d/good"), b"abc").unwrap();
        fs::write(stage.join("d/bad"), b"abd").unwrap();
        let dst = tmp.path().join("corpus");
        assert!(!take_staged(None, "d/good", A, &dst.join("d/good")));
        assert!(!take_staged(Some(&stage), "d/bad", A, &dst.join("d/bad")));
        assert!(!dst.join("d/bad").exists());
        assert!(stage.join("d/bad").exists());
        assert!(!take_staged(
            Some(&stage),
            "d/missing",
            A,
            &dst.join("d/missing")
        ));
        assert!(take_staged(Some(&stage), "d/good", A, &dst.join("d/good")));
        assert_eq!(fs::read(dst.join("d/good")).unwrap(), b"abc");
        assert!(!stage.join("d/good").exists());
    }

    #[test]
    fn newest_mirror_release_is_chosen() {
        let v = serde_json::json!([
            {"tag_name": "v0.3.0", "draft": false, "published_at": "2026-10-13T00:00:00Z"},
            {"tag_name": "corpus-smoke-aaaa", "draft": false, "published_at": "2026-10-01T00:00:00Z"},
            {"tag_name": "corpus-smoke-bbbb", "draft": false, "published_at": "2026-10-10T00:00:00Z"},
            {"tag_name": "corpus-smoke-cccc", "draft": true, "published_at": null}
        ]);
        assert_eq!(newest_mirror_tag(&v).as_deref(), Some("corpus-smoke-bbbb"));
        assert_eq!(newest_mirror_tag(&serde_json::json!([])), None);
    }

    #[test]
    fn notice_escapes_table_cells() {
        let mut e = entry("a", "a.abf", "MIT", A);
        e.attribution = "Jane | Doe\nLab".into();
        let files = mirror_files(std::slice::from_ref(&e)).unwrap();
        let n = notice(&files, "0123456789abcdef");
        assert!(
            n.contains("| a | a.abf | MIT | Jane \\| Doe Lab | - | https://example.org/a.abf |")
        );
    }
}
