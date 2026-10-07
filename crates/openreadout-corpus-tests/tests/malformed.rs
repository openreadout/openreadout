//! Malformed-file matrix: every smoke-tier input of every format present on disk is damaged in
//! several ways and must still produce a clean answer.
//!
//! Each input is copied to a scratch directory as a unit: a directory data set (Bruker NMR
//! experiment, ChemStation `.D`, MassLynx `.raw`) whole; a file inside a subdirectory
//! (`x.d/analysis.tdf`, a VSI with its `_x_/` tree, an OME-TIFF file set) with that
//! subdirectory; a top-level file with its top-level companions (same stem: `.imzML` + `.ibd`,
//! SpikeGLX `.meta` + `.bin`, TIA `.ser` + `.emi`, Blackrock `.nsx` + `.nev`). The *victim* —
//! the input file, or the largest file of a directory data set — is then damaged.
//!
//! Variants per input: the victim under a wrong extension (files only), 8 single-byte flips at
//! seeded positions in its first 64 KiB, and truncation to 90 %, 50 % and 10 % of its length.
//! For each variant:
//! - in-process, with every reader the registry picks: `open`, `info`, `vendor_metadata`,
//!   `entries`, `check`, `read_plane(0, c0 z0 t0)` and its statistics, the first rows of table
//!   0, the first samples of trace 0, spectrum 0 and frame records return `Ok` or a clean
//!   `Err` (a panic fails the test); for the image formats whose `check` verifies the data
//!   extent (CZI, ND2, LIF), a truncated file never passes `check`;
//! - through the CLI binary: `info`, `info --view structure`, `check`, one-plane `planes`
//!   and one-plane `stats`
//!   exit with 0, 4 (corrupt), 5 (I/O) or 6 (unsupported) — 3 (unknown format) only when the
//!   registry no longer recognises the damaged input — never 1 (internal error or panic), 101
//!   or a signal, and `--json` stdout is always JSON. Exception: the JPEG XR decoder's known
//!   panics (SECURITY.md) end the release binary in its panic handler (exit 1); those runs are
//!   listed as known upstream issues and do not fail the matrix.
//!
//! Run: `cargo test -p openreadout-corpus-tests --features corpus --profile corpus --test malformed -- --nocapture`
//! Env: `OPENREADOUT_CORPUS_DIR` overrides `corpus/files`; `CORPUS_ONLY=<substring>` filters
//! ids; `CORPUS_FORMAT=<id>[,<id>...]` filters formats.
//! The CLI binary is taken from the same target directory (built on demand).
#![cfg(feature = "corpus")]

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::Command;

use openreadout_core::Registry;
use openreadout_core::reader::PlaneIndex;
use rayon::prelude::*;
use serde::Deserialize;

#[path = "../../openreadout-cli/src/registry.rs"]
mod registry;

#[derive(Deserialize)]
struct Manifest {
    file: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    format: String,
    filename: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    tier: String,
}

/// Formats whose `check` compares the data extent with the header, so any truncation is found.
const EXTENT_CHECKED: [&str; 3] = ["czi", "nd2", "lif"];
const FLIPS: usize = 8;
const FLIP_WINDOW: u64 = 64 * 1024;
/// Largest directory copied along with an input in a subdirectory.
const MAX_UNIT_BYTES: u64 = 256 << 20;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// splitmix64: deterministic positions and values without a `rand` dependency.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn seed_for(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// The CLI binary next to this test executable (`target/<profile>/openreadout`). Always
/// (re)built first: a stale binary left in `target/` from an older checkout would otherwise be
/// tested instead of the current code (cargo makes this a no-op when it is up to date).
fn cli_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().and_then(Path::parent).unwrap().to_path_buf();
    let bin = profile_dir.join(format!("openreadout{}", std::env::consts::EXE_SUFFIX));
    {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut cmd = Command::new(cargo);
        cmd.current_dir(root()).args(["build", "-p", "openreadout"]);
        // `target/debug` is the dev profile; every other directory is named after its profile.
        match profile_dir.file_name().and_then(|n| n.to_str()) {
            Some("debug") | None => {}
            Some(profile) => {
                cmd.args(["--profile", profile]);
            }
        }
        let status = cmd.status().expect("run cargo build for the CLI");
        assert!(status.success(), "building openreadout failed");
    }
    assert!(bin.exists(), "CLI binary not found at {}", bin.display());
    bin
}

/// Files the harness writes next to corpus files (download receipts, oracle sidecars).
fn is_harness_file(p: &Path) -> bool {
    let n = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    [".ok", ".oracle", ".oracle.json"]
        .iter()
        .any(|suffix| n.ends_with(suffix))
}

fn tree_size(p: &Path) -> u64 {
    if p.is_dir() {
        std::fs::read_dir(p).map_or(0, |rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| !is_harness_file(p))
                .map(|p| tree_size(&p))
                .sum()
        })
    } else {
        std::fs::metadata(p).map_or(0, |m| m.len())
    }
}

fn copy_tree(src: &Path, dst: &Path) {
    if src.is_dir() {
        std::fs::create_dir_all(dst).unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let p = e.unwrap().path();
            if !is_harness_file(&p) {
                copy_tree(&p, &dst.join(p.file_name().unwrap()));
            }
        }
    } else {
        std::fs::copy(src, dst).unwrap();
    }
}

fn largest_file(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()? {
        let p = e.ok()?.path();
        if is_harness_file(&p) {
            continue;
        }
        let cand = if p.is_dir() {
            largest_file(&p).map(|f| (tree_size(&f), f))
        } else {
            Some((tree_size(&p), p))
        };
        if let Some((n, f)) = cand
            && best.as_ref().is_none_or(|(b, _)| n > *b)
        {
            best = Some((n, f));
        }
    }
    best.map(|(_, p)| p)
}

/// Stem shared by a file and its companions: the name up to the last extension, with a
/// trailing `_<digits>` removed (TIA: `x_1.ser` belongs to `x.emi`).
fn companion_stem(name: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    match stem.rsplit_once('_') {
        Some((s, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => s.to_string(),
        _ => stem.to_string(),
    }
}

/// Copy the unit of `src` into `work`; return (path to open, victim file).
fn working_copy(files_dir: &Path, e: &Entry, work: &Path) -> (PathBuf, PathBuf) {
    let src = files_dir.join(&e.filename);
    let dst = work.join(&e.filename);
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
    if src.is_dir() {
        copy_tree(&src, &dst);
        let victim = largest_file(&dst).unwrap_or_else(|| dst.clone());
        return (dst, victim);
    }
    let parent = src.parent().unwrap();
    if parent != files_dir && tree_size(parent) <= MAX_UNIT_BYTES {
        copy_tree(parent, dst.parent().unwrap());
        return (dst.clone(), dst);
    }
    std::fs::copy(&src, &dst).unwrap();
    if parent == files_dir {
        let name = src.file_name().unwrap().to_string_lossy().into_owned();
        let stem = companion_stem(&name).to_ascii_lowercase();
        for sib in std::fs::read_dir(files_dir).unwrap().filter_map(Result::ok) {
            let p = sib.path();
            let n = p.file_name().unwrap().to_string_lossy().into_owned();
            if n != name
                && p.is_file()
                && !is_harness_file(&p)
                && n.to_ascii_lowercase().starts_with(&stem)
                && n.as_bytes()
                    .get(stem.len())
                    .is_some_and(|b| matches!(b, b'.' | b'_'))
            {
                std::fs::copy(&p, work.join(&n)).unwrap();
            }
        }
    }
    (dst.clone(), dst)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    WrongExtension,
    Flip,
    Truncated,
}

/// Run the library sequence under `catch_unwind`. Returns a failure description, if any.
fn library_checks(reg: &Registry, path: &Path, kind: Kind, format: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let r = catch_unwind(AssertUnwindSafe(|| {
        let Ok((_, mut ds)) = reg.open(path) else {
            return None;
        };
        if let Ok(info) = ds.info() {
            // The assurance profile must describe damaged files without panicking.
            let a = openreadout_core::assurance::assess_dataset(ds.as_ref(), &info);
            let _ = serde_json::to_string(&a);
        }
        let _ = ds.vendor_metadata();
        let _ = ds.entries();
        let report = ds.check();
        if let Ok(plane) = ds.read_plane(0, PlaneIndex::default()) {
            let acc = openreadout_core::stats::Accumulator::from_plane(&plane);
            let _ = acc.finish(64, openreadout_core::stats::HistogramScale::Linear);
        }
        let _ = ds.read_table(0, 0, 64);
        let _ = ds.read_trace(0, 0, 0, 4096);
        let _ = ds.read_spectrum(0, 0);
        let _ = ds.frames(0, Some(16));
        // Strict mode gates every read on the assessment; it must not panic either.
        if let Ok(mut strict) = openreadout_core::strict::StrictDataset::wrap(ds) {
            let _ = strict.file_assurance();
            let _ = strict.info();
            let _ = strict.read_plane(0, PlaneIndex::default());
            let _ = strict.read_table(0, 0, 8);
            let _ = strict.read_trace(0, 0, 0, 64);
            let _ = strict.read_spectrum(0, 0);
        }
        Some(report)
    }));
    match r {
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_default();
            failures.push(format!("library panicked: {msg}"));
        }
        Ok(Some(Ok(report)))
            if kind == Kind::Truncated && report.ok && EXTENT_CHECKED.contains(&format) =>
        {
            failures.push("check passed a truncated file".into());
        }
        Ok(_) => {}
    }
    failures
}

fn cli_checks(bin: &Path, path: &Path, recognised: bool) -> Vec<String> {
    let p = path.to_str().unwrap();
    let one_plane = ["--select", "c=0", "--select", "z=0", "--select", "t=0"];
    let runs: Vec<Vec<&str>> = vec![
        vec!["info", p, "--json"],
        vec!["info", "--view", "structure", p, "--json"],
        vec!["check", p, "--json"],
        [&["planes", p, "--image", "0"][..], &one_plane, &["--json"]].concat(),
        [&["stats", p, "--image", "0"][..], &one_plane, &["--json"]].concat(),
        // strict mode: refusals are exit 6, never an internal error
        vec!["--strict", "info", p, "--json"],
        vec!["--strict", "check", p, "--json"],
        [
            &["--strict", "stats", p, "--image", "0"][..],
            &one_plane,
            &["--json"],
        ]
        .concat(),
    ];
    let mut failures = Vec::new();
    for args in runs {
        let out = Command::new(bin).args(&args).output().unwrap();
        let code = out.status.code();
        let allowed = match code {
            Some(0 | 4 | 5 | 6) => true,
            Some(3) => !recognised,
            _ => false,
        };
        if !allowed {
            failures.push(format!(
                "`{}` exited {code:?}: {}",
                args.join(" ").replace(p, "FILE"),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
            continue;
        }
        if serde_json::from_slice::<serde_json::Value>(&out.stdout).is_err() {
            failures.push(format!("`{}` --json stdout is not JSON", args[0]));
        }
    }
    failures
}

/// Damages one input in every variant and returns its failures.
fn check_input(
    reg: &Registry,
    bin: &Path,
    files_dir: &Path,
    work_root: &Path,
    e: &Entry,
) -> Vec<String> {
    let work = work_root.join(&e.id);
    let _ = std::fs::remove_dir_all(&work);
    let (open, victim) = working_copy(files_dir, e, &work);
    let len = std::fs::metadata(&victim).map_or(0, |m| m.len());
    let mut failures = Vec::new();
    let mut report = |variant: &str, kind: Kind, path: &Path| {
        let recognised = reg.detect(path).is_ok();
        for f in library_checks(reg, path, kind, &e.format)
            .into_iter()
            .chain(cli_checks(bin, path, recognised))
        {
            failures.push(format!("{} [{variant}]: {f}", e.id));
        }
    };

    if !open.is_dir() {
        let ext = if e.format == "czi" { "nd2" } else { "czi" };
        let renamed = open.with_extension(ext);
        std::fs::rename(&open, &renamed).unwrap();
        report("wrong-extension", Kind::WrongExtension, &renamed);
        std::fs::rename(&renamed, &open).unwrap();
    }

    let mut rng = seed_for(&e.id);
    let window = len.min(FLIP_WINDOW);
    for i in 0..FLIPS {
        if window == 0 {
            break;
        }
        // Half the flips land in the first KiB, where the headers live.
        let span = if i % 2 == 0 { window.min(1024) } else { window };
        let pos = splitmix(&mut rng) % span;
        let xor = (splitmix(&mut rng) % 255 + 1) as u8;
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&victim)
            .unwrap();
        let mut orig = [0u8; 1];
        f.seek(SeekFrom::Start(pos)).unwrap();
        std::io::Read::read_exact(&mut f, &mut orig).unwrap();
        f.seek(SeekFrom::Start(pos)).unwrap();
        f.write_all(&[orig[0] ^ xor]).unwrap();
        drop(f);
        report(&format!("flip@{pos}^{xor:#04x}"), Kind::Flip, &open);
        let mut f = OpenOptions::new().write(true).open(&victim).unwrap();
        f.seek(SeekFrom::Start(pos)).unwrap();
        f.write_all(&orig).unwrap();
    }

    for pct in [90u64, 50, 10] {
        let cut = len * pct / 100;
        OpenOptions::new()
            .write(true)
            .open(&victim)
            .unwrap()
            .set_len(cut)
            .unwrap();
        report(&format!("truncated-{pct}%"), Kind::Truncated, &open);
    }
    let _ = std::fs::remove_dir_all(&work);
    println!("checked {:<60} ({len} bytes damaged)", e.id);
    failures
}

#[test]
fn malformed_matrix() {
    let root = root();
    let manifest: Manifest =
        toml::from_str(&std::fs::read_to_string(root.join("corpus/manifest.toml")).unwrap())
            .unwrap();
    let files_dir = std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root.join("corpus/files"), PathBuf::from);
    let only = std::env::var("CORPUS_ONLY").ok();
    let formats: Option<Vec<String>> = std::env::var("CORPUS_FORMAT")
        .ok()
        .map(|s| s.split(',').map(str::to_string).collect());
    let reg = registry::registry();
    let bin = cli_binary();
    let work_root =
        std::env::temp_dir().join(format!("openreadout-malformed-{}", std::process::id()));

    // Inputs are independent (each has its own scratch copy), so they are checked in parallel.
    let inputs: Vec<&Entry> = manifest
        .file
        .iter()
        .filter(|e| e.tier == "smoke" && (e.role.is_empty() || e.role == "input"))
        .filter(|e| {
            only.as_ref().is_none_or(|o| e.id.contains(o.as_str()))
                && formats.as_ref().is_none_or(|f| f.contains(&e.format))
                && files_dir.join(&e.filename).exists()
        })
        .collect();
    let checked = inputs.len();
    let failures: Vec<String> = inputs
        .par_iter()
        .flat_map_iter(|e| check_input(&reg, &bin, &files_dir, &work_root, e))
        .collect();
    let _ = std::fs::remove_dir_all(&work_root);
    assert!(
        checked > 0,
        "no smoke-tier corpus files found; run `cargo xtask corpus fetch --tier smoke`"
    );
    println!("{checked} inputs checked");
    assert!(
        failures.is_empty(),
        "{} malformed-file failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
