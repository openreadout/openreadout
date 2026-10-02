//! `cargo xtask corpus fetch|verify|status|compress`: the test files listed in
//! corpus/manifest.toml.

use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use serde::Deserialize;

use crate::{compress, corpus_dir, heldout, root, sha256_file, zipmember};

#[derive(Subcommand)]
pub(crate) enum CorpusCmd {
    /// Download files (resumable, checksum-verified).
    Fetch {
        /// smoke | standard | full (cumulative: full includes standard and smoke) | heldout (only
        /// the held-out set, docs/benchmark/heldout.md; never included in the others)
        #[arg(long, default_value = "smoke")]
        tier: String,
        /// Only files of this format id.
        #[arg(long)]
        format: Option<String>,
        /// Only ids containing this substring.
        #[arg(long)]
        only: Option<String>,
        /// Record SHA-256 for entries that have none (trust on first download).
        #[arg(long)]
        record: bool,
    },
    /// Re-hash present files against the manifest.
    Verify,
    /// Show what is present, per tier and format.
    Status,
    /// Store committed ground truth over 1 MiB as `<name>.json.gz` (and smaller files as plain
    /// `<name>.json`) in corpus/oracle, corpus/oracle/heldout and corpus/oracle/flow; stages the
    /// renames in git. Idempotent.
    Compress {
        /// Change nothing; exit 1 if any file needs converting.
        #[arg(long)]
        check: bool,
    },
}

#[derive(Debug, Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Debug, Deserialize, Clone)]
struct Entry {
    id: String,
    format: String,
    tier: String,
    filename: String,
    /// Empty for files that come out of a bundle (see `bundle`) and for synthetic fixtures,
    /// which are regenerated locally instead of downloaded.
    #[serde(default)]
    url: String,
    /// `input`, `corrupt`, `oracle-export`, `analysis` (a gating file paired with inputs, not a
    /// data set: FlowJo workspaces, Gating-ML) or `bundle` (a zip: unpacked into the corpus dir with
    /// `unpack = true`, else extracted into `<corpus>/<id>/`).
    #[serde(default)]
    role: String,
    /// For files inside a bundle: the bundle's id. Such entries are not downloaded themselves;
    /// fetch unpacks the bundle and then checks the file (`filename` is relative to the corpus dir).
    #[serde(default)]
    bundle: String,
    /// For bundles: leading path components dropped when extracting.
    #[serde(default)]
    strip: usize,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    sha256: String,
    #[serde(default)]
    md5: String,
    #[serde(default)]
    license: String,
    /// For zip bundles: extract the archive into the corpus directory itself after download.
    #[serde(default)]
    unpack: bool,
    /// When set, `url` is a zip archive and only this member is fetched (HTTP range requests;
    /// `sha256`/`size` are the member's).
    #[serde(default)]
    zip_member: String,
    /// A directory data set fetched file by file (vendor `.raw`/`.d` directories published as
    /// loose files): `filename` is the directory, `parts_url` the URL of that directory
    /// (ending in `/`), and each part `"<path relative to the directory>  <sha256>"`.
    #[serde(default)]
    parts_url: String,
    #[serde(default)]
    parts: Vec<String>,
}

/// The `(relative path, sha256)` of each `parts` line.
fn parts_of(e: &Entry) -> Result<Vec<(String, String)>> {
    e.parts
        .iter()
        .map(|l| {
            let (rel, sha) = l
                .rsplit_once(char::is_whitespace)
                .with_context(|| format!("{}: part line {l:?} lacks a sha256", e.id))?;
            let rel = rel.trim();
            if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|c| c == "..") {
                bail!("{}: part path {rel:?} must be relative", e.id);
            }
            Ok((rel.to_string(), sha.trim().to_string()))
        })
        .collect()
}

/// Percent-encode the characters of a relative path that URLs cannot carry literally.
fn url_path(rel: &str) -> String {
    let mut out = String::with_capacity(rel.len());
    for c in rel.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '+' => out.push_str("%2B"),
            ',' => out.push_str("%2C"),
            '#' => out.push_str("%23"),
            _ => out.push(c),
        }
    }
    out
}

/// Fetch every part of a directory entry that is missing, checking each file's SHA-256.
fn fetch_parts(e: &Entry, dir: &Path) -> Result<usize> {
    let root_dir = dir.join(&e.filename);
    let mut n = 0;
    for (rel, sha) in parts_of(e)? {
        let dst = root_dir.join(&rel);
        if dst.is_file() && sha256_file(&dst)?.0 == sha {
            continue;
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::remove_file(&dst).ok();
        let got = download(&format!("{}{}", e.parts_url, url_path(&rel)), &dst)
            .with_context(|| format!("download {} part {rel}", e.id))?;
        if got != sha {
            fs::remove_file(&dst).ok();
            bail!(
                "{} part {rel}: SHA-256 mismatch (expected {sha}, got {got})",
                e.id
            );
        }
        n += 1;
    }
    Ok(n)
}

fn load_manifest() -> Result<Manifest> {
    let p = root().join("corpus/manifest.toml");
    Ok(toml::from_str(
        &fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?,
    )?)
}

fn tier_rank(t: &str) -> u8 {
    match t {
        "smoke" => 0,
        "standard" => 1,
        "full" => 2,
        _ => 9,
    }
}

pub(crate) fn run(cmd: CorpusCmd) -> Result<()> {
    let m = load_manifest()?;
    let dir = corpus_dir();
    fs::create_dir_all(&dir)?;
    match cmd {
        CorpusCmd::Fetch {
            tier,
            format,
            only,
            record,
        } => {
            let sel = Selection::new(&tier, format, only)?;
            fetch(&m, &dir, &sel, record)
        }
        CorpusCmd::Verify => verify(&m, &dir),
        CorpusCmd::Status => {
            status(&m, &dir);
            Ok(())
        }
        CorpusCmd::Compress { check } => compress::run(&root(), check),
    }
}

/// Which manifest entries `corpus fetch` takes.
struct Selection {
    /// `tier_rank` of `--tier`.
    want: u8,
    /// `--tier heldout`: only the held-out set.
    heldout_only: bool,
    format: Option<String>,
    only: Option<String>,
}

impl Selection {
    fn new(tier: &str, format: Option<String>, only: Option<String>) -> Result<Self> {
        let want = tier_rank(tier);
        let heldout_only = tier == heldout::TIER;
        if want == 9 && !heldout_only {
            bail!("unknown tier '{tier}' (smoke | standard | full | heldout)");
        }
        Ok(Selection {
            want,
            heldout_only,
            format,
            only,
        })
    }

    /// The entry's tier is wanted and `--only` matches its id.
    fn tier_and_id(&self, e: &Entry) -> bool {
        (if self.heldout_only {
            e.tier == heldout::TIER
        } else {
            tier_rank(&e.tier) <= self.want && e.tier != "hold"
        }) && self.only.as_ref().is_none_or(|o| e.id.contains(o.as_str()))
    }

    fn selects(&self, e: &Entry) -> bool {
        self.tier_and_id(e) && self.format.as_ref().is_none_or(|f| f == &e.format)
    }
}

fn fetch(m: &Manifest, dir: &Path, sel: &Selection, record: bool) -> Result<()> {
    // Bundles are fetched when any of their members is selected.
    let wanted_bundles: BTreeSet<&str> = m
        .file
        .iter()
        .filter(|e| !e.bundle.is_empty() && sel.selects(e))
        .map(|e| e.bundle.as_str())
        .collect();
    // `--format X` also fetches the oracle exports paired with (sharing the id of) X's inputs.
    let paired: BTreeSet<&str> = m
        .file
        .iter()
        .filter(|e| sel.format.as_ref().is_some_and(|f| f == &e.format))
        .map(|e| e.id.as_str())
        .collect();
    let mut recorded = Vec::new();
    for e in &m.file {
        if !e.bundle.is_empty() {
            continue; // members are checked after their bundle is unpacked
        }
        let paired_ok = e.role == "oracle-export"
            && paired.contains(e.id.as_str())
            && tier_rank(&e.tier) <= sel.want
            && e.tier != "hold"
            && sel.tier_and_id(e);
        if sel.selects(e) || paired_ok || wanted_bundles.contains(e.id.as_str()) {
            fetch_entry(e, dir, record, &mut recorded)?;
        }
    }
    for e in m
        .file
        .iter()
        .filter(|e| !e.bundle.is_empty() && sel.selects(e))
    {
        check_bundle_member(e, dir, record, &mut recorded)?;
    }
    if !recorded.is_empty() {
        record_sha256(&recorded)?;
    }
    Ok(())
}

/// Download one entry (or its parts) unless a `.ok` marker says it is complete; extract a
/// bundle. SHA-256 values to write into the manifest (`--record`) go to `recorded`.
fn fetch_entry(
    e: &Entry,
    dir: &Path,
    record: bool,
    recorded: &mut Vec<(String, String)>,
) -> Result<()> {
    let dst = dir.join(&e.filename);
    let ok_marker = dir.join(format!("{}.ok", e.filename));
    if !e.parts.is_empty() {
        if ok_marker.exists() {
            println!("have     {}  (directory of {} files)", e.id, e.parts.len());
            return Ok(());
        }
        println!("fetch    {}  ({} files)", e.id, e.parts.len());
        let n = fetch_parts(e, dir)?;
        fs::write(
            &ok_marker,
            format!("{{\"id\":\"{}\",\"parts\":{}}}\n", e.id, e.parts.len()),
        )?;
        println!("ok       {} ({n} downloaded)", e.id);
        return Ok(());
    }
    if e.url.is_empty() {
        if dst.is_dir() {
            // a directory assembled from its `role = "part"` entries
            println!("have     {}  (directory of parts)", e.id);
        } else if dst.exists() {
            println!("have     {}  (synthetic)", e.id);
        } else {
            let script = match e.format.as_str() {
                "mzml" | "mzxml" => "make_mzml_fixtures.py --oracle",
                "plate" => "make_assay_fixtures.py",
                _ => "make_czi_fixtures.py",
            };
            println!(
                "missing  {}  (synthetic: cd oracle && uv run python {script})",
                e.id
            );
        }
        return Ok(());
    }
    if ok_marker.exists() {
        println!("have     {}", e.id);
        // A download finished earlier but its SHA-256 never reached the manifest.
        if record && e.sha256.is_empty() {
            let marker = fs::read_to_string(&ok_marker).unwrap_or_default();
            if let Some(sha) = marker
                .split("\"sha256\":\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
                .filter(|s| s.len() == 64)
            {
                recorded.push((e.filename.clone(), sha.to_string()));
            }
        }
    } else {
        let sha = download_checked(e, &dst)?;
        if e.sha256.is_empty() && record {
            recorded.push((e.filename.clone(), sha.clone()));
        }
        if e.unpack {
            unzip(&dst, dir).with_context(|| format!("unpack {}", e.id))?;
            println!("unpacked {}", e.id);
        }
        fs::write(
            &ok_marker,
            format!("{{\"id\":\"{}\",\"sha256\":\"{sha}\"}}\n", e.id),
        )?;
        println!("ok       {}", e.id);
    }
    // A bundle without `unpack` is extracted into `<corpus>/<id>/` (minus `strip` leading
    // components) by the checked extractor.
    if e.role == "bundle" && !e.unpack {
        let marker = dir.join(format!("{}.extracted.ok", e.id));
        if marker.exists() {
            return Ok(());
        }
        let out = dir.join(&e.id);
        println!("extract  {} -> {}", e.id, out.display());
        let n = extract_bundle(&dst, &out, e.strip).with_context(|| format!("extract {}", e.id))?;
        fs::write(&marker, format!("{{\"id\":\"{}\",\"files\":{n}}}\n", e.id))?;
        println!("ok       {} ({n} files)", e.id);
    }
    Ok(())
}

/// Download `e` to `dst` (the whole file, or one member of a remote zip) and check its SHA-256
/// and MD5 against the manifest; returns the SHA-256.
fn download_checked(e: &Entry, dst: &Path) -> Result<String> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    println!("fetch    {}  ({})", e.id, human(e.size));
    let sha = if e.zip_member.is_empty() {
        download(&e.url, dst).with_context(|| format!("download {}", e.id))?
    } else {
        zipmember::fetch_member(&e.url, &e.zip_member, dst)
            .with_context(|| format!("download {} from its archive", e.id))?;
        sha256_file(dst)?.0
    };
    if !e.sha256.is_empty() && e.sha256 != sha {
        fs::remove_file(dst).ok();
        bail!(
            "{}: SHA-256 mismatch (expected {}, got {sha})",
            e.id,
            e.sha256
        );
    }
    if !e.md5.is_empty() {
        let got = md5_file(dst)?;
        if got != e.md5 {
            fs::remove_file(dst).ok();
            bail!("{}: MD5 mismatch (expected {}, got {got})", e.id, e.md5);
        }
    }
    Ok(sha)
}

/// A file that comes out of a bundle: present after unpacking, with the manifest's SHA-256.
fn check_bundle_member(
    e: &Entry,
    dir: &Path,
    record: bool,
    recorded: &mut Vec<(String, String)>,
) -> Result<()> {
    let dst = dir.join(&e.filename);
    let ok_marker = dir.join(format!("{}.ok", e.filename));
    if ok_marker.exists() {
        println!("have     {}", e.id);
        return Ok(());
    }
    if !dst.exists() {
        bail!(
            "{}: not found after unpacking bundle {} (expected {})",
            e.id,
            e.bundle,
            dst.display()
        );
    }
    if dst.is_dir() {
        // an experiment directory: covered by its bundle's checksum
        println!("ok       {} (from {})", e.id, e.bundle);
        return Ok(());
    }
    let (sha, _) = sha256_file(&dst)?;
    if !e.sha256.is_empty() && e.sha256 != sha {
        bail!(
            "{}: SHA-256 mismatch (expected {}, got {sha})",
            e.id,
            e.sha256
        );
    }
    if e.sha256.is_empty() && record {
        recorded.push((e.filename.clone(), sha.clone()));
    }
    fs::write(
        &ok_marker,
        format!("{{\"id\":\"{}\",\"sha256\":\"{sha}\"}}\n", e.id),
    )?;
    println!("ok       {} (from {})", e.id, e.bundle);
    Ok(())
}

/// `--record`: write each `(filename, sha256)` into its manifest entry. Keyed by filename
/// because a paired oracle export shares its input's id.
fn record_sha256(recorded: &[(String, String)]) -> Result<()> {
    let p = root().join("corpus/manifest.toml");
    let mut text = fs::read_to_string(&p)?;
    for (filename, sha) in recorded {
        let needle = format!("filename = \"{filename}\"\n");
        if let Some(pos) = text.find(&needle) {
            // after the entry's url line, or after its filename line when it has no url (a
            // file inside a bundle); never past the end of the entry
            let entry_end = text[pos..]
                .find("\n[[file]]")
                .map_or(text.len(), |i| pos + i);
            let line_end = match text[pos..entry_end].find("\nurl = ") {
                Some(url_pos) => {
                    pos + url_pos + 1 + text[pos + url_pos + 1..].find('\n').unwrap_or(0)
                }
                None => pos + needle.len() - 1,
            };
            text.insert_str(line_end + 1, &format!("sha256 = \"{sha}\"\n"));
        }
    }
    fs::write(&p, text)?;
    println!(
        "recorded {} SHA-256 values into the manifest",
        recorded.len()
    );
    Ok(())
}

fn verify(m: &Manifest, dir: &Path) -> Result<()> {
    let mut bad = 0;
    for e in &m.file {
        if !e.bundle.is_empty() {
            continue; // verified through its bundle's zip
        }
        let Some((status, ok)) = verify_entry(e, &m.file, dir)? else {
            continue;
        };
        if !ok {
            bad += 1;
        }
        println!("{status:<22} {}", e.id);
    }
    if bad > 0 {
        bail!("{bad} files failed verification");
    }
    Ok(())
}

/// Files present per tier and format, with their size on disk.
fn status(m: &Manifest, dir: &Path) {
    let is_present = |e: &&&Entry| {
        if e.bundle.is_empty() && !e.url.is_empty() {
            dir.join(format!("{}.ok", e.filename)).exists()
        } else {
            dir.join(&e.filename).exists()
        }
    };
    let formats: BTreeSet<&str> = m.file.iter().map(|e| e.format.as_str()).collect();
    println!(
        "{:<9} {:<11} {:>8} {:>10}",
        "tier", "format", "present", "on disk"
    );
    for tier in ["smoke", "standard", "full", "hold", heldout::TIER] {
        for fmt in &formats {
            let entries: Vec<&Entry> = m
                .file
                .iter()
                .filter(|e| e.tier == tier && e.format == *fmt)
                .collect();
            if entries.is_empty() {
                continue;
            }
            let present: Vec<&&Entry> = entries.iter().filter(is_present).collect();
            let bytes: u64 = present
                .iter()
                .map(|e| path_size(&dir.join(&e.filename)))
                .sum();
            println!(
                "{tier:<9} {fmt:<11} {:>3}/{:<4} {:>10}",
                present.len(),
                entries.len(),
                human(bytes)
            );
        }
    }
    let licenses: BTreeSet<&str> = m.file.iter().map(|e| e.license.as_str()).collect();
    println!(
        "\nlicenses in manifest: {}",
        licenses.into_iter().collect::<Vec<_>>().join(" | ")
    );
    println!("corpus dir: {}", dir.display());
}

/// Verify one manifest entry on disk: `None` when it is not downloaded, else a status line and
/// whether it passed. A directory entry (a data set assembled from its `role = "part"` entries,
/// e.g. a Varian `.fid` folder) has no checksum of its own: its parts are hashed on their own
/// rows, so the directory passes when every part is present.
fn verify_entry(e: &Entry, all: &[Entry], dir: &Path) -> Result<Option<(String, bool)>> {
    let dst = dir.join(&e.filename);
    if !dst.exists() {
        return Ok(None);
    }
    if dst.is_dir() && !e.parts.is_empty() {
        let mut bad = 0usize;
        for (rel, sha) in parts_of(e)? {
            let f = dst.join(&rel);
            if !f.is_file() || sha256_file(&f)?.0 != sha {
                bad += 1;
            }
        }
        return Ok(Some(if bad == 0 {
            (format!("ok (directory, {} files)", e.parts.len()), true)
        } else {
            (
                format!(
                    "INCOMPLETE ({bad}/{} files missing or changed)",
                    e.parts.len()
                ),
                false,
            )
        }));
    }
    if dst.is_dir() {
        let prefix = format!("{}/", e.filename.trim_end_matches('/'));
        let parts: Vec<&Entry> = all
            .iter()
            .filter(|p| p.bundle.is_empty() && p.filename.starts_with(&prefix))
            .collect();
        if !e.sha256.is_empty() {
            return Ok(Some(("DIRECTORY (expected a file)".into(), false)));
        }
        if parts.is_empty() {
            return Ok(Some(("ok (directory, no manifest sha)".into(), true)));
        }
        let missing = parts
            .iter()
            .filter(|p| !dir.join(&p.filename).is_file())
            .count();
        return Ok(Some(if missing == 0 {
            (format!("ok (directory, {} parts)", parts.len()), true)
        } else {
            (
                format!("INCOMPLETE ({missing}/{} parts missing)", parts.len()),
                false,
            )
        }));
    }
    let (sha, n) = sha256_file(&dst)?;
    Ok(Some(if !e.sha256.is_empty() && e.sha256 != sha {
        ("SHA MISMATCH".into(), false)
    } else if e.size != 0 && e.size != n {
        // fetch writes `<file>.ok` after a complete, checked download
        let partial = !e.url.is_empty() && !dir.join(format!("{}.ok", e.filename)).exists();
        let status = if partial {
            format!(
                "SIZE MISMATCH ({n} of {} bytes, an interrupted download: run `cargo xtask corpus fetch` again)",
                e.size
            )
        } else {
            "SIZE MISMATCH".into()
        };
        (status, false)
    } else if e.sha256.is_empty() {
        ("ok (no manifest sha)".into(), true)
    } else {
        ("ok".into(), true)
    }))
}

/// Size of a file, or of every file under a directory.
fn path_size(p: &Path) -> u64 {
    match fs::metadata(p) {
        Ok(m) if m.is_dir() => fs::read_dir(p).map_or(0, |rd| {
            rd.filter_map(std::result::Result::ok)
                .map(|e| path_size(&e.path()))
                .sum()
        }),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// Extract a zip archive into `dir` with the platform's `unzip` (or `tar`, which reads zip
/// on macOS and Windows). Only xtask ever unpacks; the binary never does.
fn unzip(archive: &Path, dir: &Path) -> Result<()> {
    let ok = std::process::Command::new("unzip")
        .arg("-o")
        .arg("-q")
        .arg(archive)
        .arg("-d")
        .arg(dir)
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        return Ok(());
    }
    let status = std::process::Command::new("tar")
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .status()?;
    if !status.success() {
        bail!(
            "could not extract {} (need unzip or tar)",
            archive.display()
        );
    }
    Ok(())
}

fn md5_file(p: &Path) -> Result<String> {
    // md5 is only used to cross-check Zenodo's published digests.
    let out = std::process::Command::new("md5sum").arg(p).output();
    let out = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => {
            let o = std::process::Command::new("md5")
                .arg("-q")
                .arg(p)
                .output()?;
            String::from_utf8_lossy(&o.stdout).to_string()
        }
    };
    Ok(out.split_whitespace().next().unwrap_or("").to_string())
}

/// Download with resume support, retrying dropped connections (some archive FTP-over-HTTP
/// front ends disconnect long transfers); returns the SHA-256 of the complete file.
fn download(url: &str, dst: &Path) -> Result<String> {
    const ATTEMPTS: u32 = 30;
    let mut last = None;
    for attempt in 1..=ATTEMPTS {
        match download_once(url, dst) {
            Ok(()) => return Ok(sha256_file(dst)?.0),
            Err(e) => {
                let have = fs::metadata(dst).map_or(0, |m| m.len());
                eprintln!("  attempt {attempt}/{ATTEMPTS} stopped at {have} bytes: {e:#}");
                last = Some(e);
            }
        }
    }
    Err(last.expect("at least one attempt"))
}

fn download_once(url: &str, dst: &Path) -> Result<()> {
    let existing = fs::metadata(dst).map_or(0, |m| m.len());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent.get(url);
    if existing > 0 {
        req = req.header("Range", &format!("bytes={existing}-"));
    }
    let mut resp = req.call().with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    let mut file = if status == 206 {
        fs::OpenOptions::new().append(true).open(dst)?
    } else if status == 200 {
        fs::File::create(dst)?
    } else if status == 416 && existing > 0 {
        return Ok(()); // range not satisfiable: we already have every byte
    } else {
        bail!("HTTP {status} for {url}");
    };
    let mut reader = resp.body_mut().as_reader();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
    }
    file.flush()?;
    Ok(())
}

/// Extract a zip (stored or deflate entries, zip64 aware) into `out`, dropping `strip` leading
/// path components. Entry names that are absolute or contain `..` are refused. Every entry's
/// size and CRC-32 are verified. Returns the number of files written.
pub(crate) fn extract_bundle(zip: &Path, out: &Path, strip: usize) -> Result<usize> {
    use std::io::{Seek, SeekFrom};
    fn u16at(b: &[u8], i: usize) -> u64 {
        u64::from(u16::from_le_bytes([b[i], b[i + 1]]))
    }
    fn u32at(b: &[u8], i: usize) -> u64 {
        u64::from(u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]))
    }
    fn u64at(b: &[u8], i: usize) -> u64 {
        u64::from_le_bytes(b[i..i + 8].try_into().expect("8 bytes"))
    }
    let mut f = fs::File::open(zip)?;
    let len = f.metadata()?.len();
    let tail_len = len.min(65_536 + 22);
    f.seek(SeekFrom::Start(len - tail_len))?;
    let mut tail = vec![0u8; usize::try_from(tail_len)?];
    f.read_exact(&mut tail)?;
    let eocd = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| tail[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .context("no end-of-central-directory record: not a zip file")?;
    let mut entries = u16at(&tail, eocd + 10);
    let mut cd_size = u32at(&tail, eocd + 12);
    let mut cd_off = u32at(&tail, eocd + 16);
    // zip64: a locator right before the end-of-central-directory record
    if eocd >= 20 && tail[eocd - 20..eocd - 16] == [0x50, 0x4b, 0x06, 0x07] {
        let z64 = u64at(&tail, eocd - 20 + 8);
        f.seek(SeekFrom::Start(z64))?;
        let mut rec = [0u8; 56];
        f.read_exact(&mut rec)?;
        if rec[..4] == [0x50, 0x4b, 0x06, 0x06] {
            entries = u64at(&rec, 32);
            cd_size = u64at(&rec, 40);
            cd_off = u64at(&rec, 48);
        }
    }
    f.seek(SeekFrom::Start(cd_off))?;
    let mut cd = vec![0u8; usize::try_from(cd_size)?];
    f.read_exact(&mut cd)?;
    let mut pos = 0usize;
    let mut written = 0usize;
    fs::create_dir_all(out)?;
    for _ in 0..entries {
        if cd.len() < pos + 46 || cd[pos..pos + 4] != [0x50, 0x4b, 0x01, 0x02] {
            bail!("bad central directory entry at {pos}");
        }
        let method = u16at(&cd, pos + 10);
        let crc = u32::try_from(u32at(&cd, pos + 16))?;
        let mut csize = u32at(&cd, pos + 20);
        let mut usize_ = u32at(&cd, pos + 24);
        let nlen = usize::try_from(u16at(&cd, pos + 28))?;
        let xlen = usize::try_from(u16at(&cd, pos + 30))?;
        let comment_len = usize::try_from(u16at(&cd, pos + 32))?;
        let mut local = u32at(&cd, pos + 42);
        let name = String::from_utf8_lossy(&cd[pos + 46..pos + 46 + nlen]).replace('\\', "/");
        let mut x = pos + 46 + nlen;
        let xend = x + xlen;
        while x + 4 <= xend {
            let id = u16at(&cd, x);
            let sz = usize::try_from(u16at(&cd, x + 2))?;
            if id == 1 {
                // zip64 extended information
                let mut k = x + 4;
                if usize_ == 0xFFFF_FFFF {
                    usize_ = u64at(&cd, k);
                    k += 8;
                }
                if csize == 0xFFFF_FFFF {
                    csize = u64at(&cd, k);
                    k += 8;
                }
                if local == 0xFFFF_FFFF {
                    local = u64at(&cd, k);
                }
            }
            x += 4 + sz;
        }
        pos = xend + comment_len;
        if name.ends_with('/') {
            continue;
        }
        let parts: Vec<&str> = name.split('/').filter(|p| !p.is_empty()).collect();
        if name.starts_with('/') || parts.iter().any(|p| *p == ".." || p.contains(':')) {
            bail!("refusing unsafe entry name {name:?}");
        }
        if parts.len() <= strip {
            continue;
        }
        let rel: PathBuf = parts[strip..].iter().collect();
        let dst = out.join(&rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut hdr = [0u8; 30];
        f.seek(SeekFrom::Start(local))?;
        f.read_exact(&mut hdr)?;
        if hdr[..4] != [0x50, 0x4b, 0x03, 0x04] {
            bail!("bad local header for {name:?}");
        }
        let data_off = local + 30 + u16at(&hdr, 26) + u16at(&hdr, 28);
        f.seek(SeekFrom::Start(data_off))?;
        let raw = (&mut f).take(csize);
        let mut reader: Box<dyn Read + '_> = match method {
            0 => Box::new(raw),
            8 => Box::new(flate2::read::DeflateDecoder::new(raw)),
            other => bail!("{name:?}: compression method {other} is not supported"),
        };
        let mut w = fs::File::create(&dst)?;
        let mut buf = vec![0u8; 1 << 20];
        let mut hasher = flate2::Crc::new();
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            w.write_all(&buf[..n])?;
            total += n as u64;
        }
        drop(reader);
        if total != usize_ || hasher.sum() != crc {
            bail!("{name:?}: size or CRC-32 mismatch after extraction");
        }
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, filename: &str, sha256: &str) -> Entry {
        Entry {
            id: id.into(),
            format: "varian-nmr".into(),
            tier: "smoke".into(),
            filename: filename.into(),
            url: String::new(),
            role: "input".into(),
            bundle: String::new(),
            strip: 0,
            size: 0,
            sha256: sha256.into(),
            md5: String::new(),
            license: String::new(),
            unpack: false,
            zip_member: String::new(),
            parts_url: String::new(),
            parts: Vec::new(),
        }
    }

    #[test]
    fn verify_handles_directory_entries() {
        let dir = std::env::temp_dir().join(format!("xtask-verify-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("x.fid")).unwrap();
        fs::write(dir.join("x.fid/fid"), b"abc").unwrap();
        let sha_abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let all = vec![
            entry("x-data", "x.fid/fid", sha_abc),
            entry("x-procpar", "x.fid/procpar", ""),
            entry("x", "x.fid", ""),
            entry("lone", "lone.fid", ""),
        ];
        // a file part is hashed like any other entry
        assert_eq!(
            verify_entry(&all[0], &all, &dir).unwrap(),
            Some(("ok".to_string(), true))
        );
        // a missing part is skipped on its own row but makes the directory incomplete
        assert_eq!(verify_entry(&all[1], &all, &dir).unwrap(), None);
        let (status, ok) = verify_entry(&all[2], &all, &dir).unwrap().unwrap();
        assert!(!ok && status.starts_with("INCOMPLETE"), "{status}");
        fs::write(dir.join("x.fid/procpar"), b"p").unwrap();
        let (status, ok) = verify_entry(&all[2], &all, &dir).unwrap().unwrap();
        assert!(ok && status == "ok (directory, 2 parts)", "{status}");
        // a directory without parts in the manifest is reported, not hashed
        fs::create_dir_all(dir.join("lone.fid")).unwrap();
        let (status, ok) = verify_entry(&all[3], &all, &dir).unwrap().unwrap();
        assert!(ok && status.contains("no manifest sha"), "{status}");
        // a corrupt file part still fails
        fs::write(dir.join("x.fid/fid"), b"abd").unwrap();
        assert_eq!(
            verify_entry(&all[0], &all, &dir).unwrap(),
            Some(("SHA MISMATCH".to_string(), false))
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
