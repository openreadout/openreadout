//! Repository maintenance tasks: `cargo xtask <command>`.
//!
//! - `corpus fetch|verify|status` — manage test files listed in corpus/manifest.toml
//! - `schema gen` — regenerate docs/schema/*.json from the CLI's types
//! - `vocab-check` — every public identifier in a format crate must appear in `docs/formats/<fmt>.md`
//! - `skill-parity` — the CLI crate's copy of the agent skill must match skills/openreadout/
//! - `mcpb pack` — build a .mcpb bundle around a release binary
//! - `agent-plugins --out DIR` — the openreadout/agent-plugins repository: plugin manifests, skill, licenses
//! - `homebrew-formula`, `scoop-manifest`, `winget-manifest` — package-manager manifests from SHA256SUMS
//! - `heldout-check` — no provenance log, format note or reader cites a held-out corpus file
//! - `version-check` — every version string in the repository agrees with Cargo.toml
//! - `man` — man pages (`man/*.1`) and shell completions (`completions/*`) for release archives
//! - `r-vendor` — copy the crates and vendor the dependencies the R package needs (book/src/guides/r.md)
//! - `publish-order` — the publishable crates in crates.io dependency order (CI and release read it)
//! - `guides [--write]` — every reader crate's MAINTAINING.md: generated facts current, hand-written sections present
//! - `coverage [--crate NAME] [--corpus] [--from lcov.info]` — coverage per crate and the uncovered regions of the readers
//! - `health [--write] [--reports DIR]` — an internal project-health report (target/reports/health.md)
//! - `snapshot review|accept` — golden-output snapshots of every fixture and corpus file (docs/maintaining.md)
//! - `variant intake|status|check` — the new-variant loop: a file or report bundle → corpus entry, intake record, provenance stub (docs/maintaining.md)
#![forbid(unsafe_code)]

mod agent_plugins;
mod assurance;
mod compress;
mod corpus;
mod coverage;
mod guides;
mod health;
mod heldout;
mod mcpb;
mod packaging;
mod publish;
mod repo;
mod rpkg;
mod snapshot;
mod variant;
mod zipmember;

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Test-corpus management.
    Corpus {
        #[command(subcommand)]
        cmd: corpus::CorpusCmd,
    },
    /// Regenerate JSON Schemas under docs/schema/ (runs the built CLI).
    Schema {
        #[command(subcommand)]
        cmd: SchemaCmd,
    },
    /// Check that every public identifier in a format crate is listed in its docs/formats vocabulary.
    VocabCheck,
    /// Check that the CLI crate's copy of the agent skill (crates/openreadout-cli/SKILL.md and references/) matches skills/openreadout/.
    SkillParity {
        /// Overwrite the copy from the canonical skills/openreadout/.
        #[arg(long)]
        fix: bool,
    },
    /// Assemble the openreadout/agent-plugins repository (Claude Code and Codex plugins, Gemini
    /// CLI extension) from packaging/agent-plugins/, the skill and the licenses.
    AgentPlugins {
        /// Output directory; must be missing or empty.
        #[arg(long)]
        out: PathBuf,
    },
    /// Build a .mcpb bundle (zip with manifest.json + binary) for Claude Desktop.
    Mcpb {
        #[command(subcommand)]
        cmd: McpbCmd,
    },
    /// Render packaging/homebrew/openreadout.rb from a release's SHA256SUMS.
    HomebrewFormula(ManifestArgs),
    /// Render packaging/scoop/openreadout.json from a release's SHA256SUMS.
    ScoopManifest(ManifestArgs),
    /// Render the packaging/winget manifests (into `<out>/manifests/o/OpenReadout/OpenReadout/<version>/`).
    WingetManifest(ManifestArgs),
    /// Write man pages (`<out>/man/*.1`) and shell completions (`<out>/completions/`) generated
    /// by the CLI (built for the host with `cargo run`, so it works when cross-compiling).
    Man {
        /// Output directory.
        #[arg(long, default_value = "target/man")]
        out: PathBuf,
    },
    /// Rewrite a finished file into DST step by step in its format's plausible write order
    /// (book/src/guides/lab-shares.md), printing one JSON line per step with its wall-clock time. For testing
    /// `info`/`watch` on growing files without an instrument; DST ends byte-identical to SRC.
    Replay {
        /// The finished file (or OME-Zarr directory). Only read.
        src: PathBuf,
        /// The file or directory to create (must not exist).
        dst: PathBuf,
        /// `ome-tiff`, `ome-zarr`, `nd2`, `czi` or `append` (default: from the extension).
        #[arg(long)]
        pattern: Option<String>,
        /// Seconds between units (planes, frame chunks, subblocks, chunk files, blocks).
        #[arg(long, default_value_t = 0.5)]
        interval: f64,
        /// Speed factor: the interval is divided by it.
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        /// Write every unit in two steps (half, then the rest), exposing a unit in flight.
        #[arg(long)]
        partial: bool,
        /// Stop after this many units (leave the file unfinished).
        #[arg(long)]
        stop_after: Option<u64>,
    },
    /// Check (or with --write regenerate) every reader's validated-variant table and confidence
    /// level from the development-corpus evidence (corpus/assurance/evidence.json); `--write`
    /// also writes the internal per-format report target/reports/evidence.md. `refresh` rebuilds
    /// that evidence (docs/assurance.md).
    AssuranceAudit {
        #[command(subcommand)]
        cmd: Option<AssuranceCmd>,
        /// Rewrite the generated tables instead of failing on drift, and write
        /// target/reports/evidence.md.
        #[arg(long)]
        write: bool,
    },
    /// Check that no provenance log, format note or reader source cites a held-out corpus file,
    /// and that no development-corpus file shares a source record with one (docs/benchmark/heldout.md).
    HeldoutCheck,
    /// Make r/openreadout self-contained for `R CMD build` / CRAN: copy the crates it needs into
    /// src/rust/ and vendor their dependencies into src/rust/vendor.tar.xz (book/src/guides/r.md).
    RVendor {
        /// Output directory (default r/openreadout/src/rust).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Also keep the unpacked vendor/ directory.
        #[arg(long)]
        keep_dir: bool,
    },
    /// Print the publishable crates in the order crates.io needs them (dependencies first).
    PublishOrder {
        /// Print `-p NAME` arguments for `cargo publish` on one line instead of one name per line.
        #[arg(long)]
        flags: bool,
    },
    /// Check (or with --write create and update) every reader crate's maintainer guide,
    /// `crates/openreadout-<name>/MAINTAINING.md`: its generated facts (formats, evidence, source
    /// map, where variants branch, pinning corpus files, tests) must be current and its
    /// hand-written sections present.
    Guides {
        /// Create missing guides and rewrite the generated parts.
        #[arg(long)]
        write: bool,
    },
    /// Line and function coverage per crate with cargo-llvm-cov (optional tool), and the largest
    /// uncovered regions of every reader crate (`--corpus`: with the corpus tests too);
    /// `--from lcov.info` summarizes an existing run. Writes target/coverage/summary.md.
    Coverage(coverage::CoverageArgs),
    /// Print (or with --write save) an internal project-health report: confidence distribution,
    /// corpus, evidence, held-out results, open findings and safety nets, computed from the
    /// repository.
    Health {
        /// Write target/reports/health.md instead of printing the report.
        #[arg(long)]
        write: bool,
        /// Folder holding the benchmark reports (the latest heldout-*.json and
        /// second-opinions.md); defaults to $OPENREADOUT_REPORTS. Without it those parts are left out.
        #[arg(long)]
        reports: Option<PathBuf>,
    },
    /// Golden-output snapshots (docs/maintaining.md § Regression snapshots): `review` shows how
    /// the last `golden`/`snapshots` test run differs from the committed records; `accept` takes it.
    Snapshot {
        #[command(subcommand)]
        cmd: snapshot::SnapshotCmd,
    },
    /// The new-variant loop (docs/maintaining.md): `intake` turns a file or a report bundle from
    /// an issue into a corpus entry, an intake record with a failing expectation, a provenance
    /// stub and the next steps; `status` lists open intakes; `check` (CI) keeps records consistent.
    Variant {
        #[command(subcommand)]
        cmd: variant::VariantCmd,
    },
    /// Check that every version string (Cargo, server.json, mcpb, plugins, Python, npm) agrees with Cargo.toml.
    VersionCheck {
        /// Release tag (e.g. v0.1.0): must match too, and CHANGELOG.md must have its heading.
        #[arg(long)]
        tag: Option<String>,
    },
}

#[derive(clap::Args)]
struct ManifestArgs {
    /// Release version (a leading `v` is ignored). Defaults to the Cargo.toml workspace version.
    #[arg(long)]
    version: Option<String>,
    /// The release's SHA256SUMS file.
    #[arg(long)]
    sums: PathBuf,
    /// Output file (directory for winget-manifest).
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Subcommand)]
enum AssuranceCmd {
    /// Run the CLI's `info` on every development-corpus input and join the corpus test's oracle
    /// results into corpus/assurance/evidence.json.
    Refresh {
        /// The `openreadout` binary to run (build it first: `cargo build --release -p openreadout`).
        #[arg(long, default_value = "target/release/openreadout")]
        bin: PathBuf,
        /// Per-file oracle results: the JSON lines `CORPUS_RESULTS=<path> cargo test -p
        /// openreadout-corpus-tests --features corpus --profile corpus --test corpus corpus_matches_oracle` writes;
        /// repeat the flag to add the lines `MZ_RESULTS=<path>` makes the `mz_agreement` test write.
        #[arg(long, required = true)]
        results: Vec<PathBuf>,
        /// Held-out agreement (the JSON lines `HELDOUT_REPORT=<path>` makes the held-out corpus
        /// test write); only per-format counts are kept. Default: keep the previous counts.
        #[arg(long)]
        heldout: Option<PathBuf>,
        /// Refresh only the files of this format (repeat for several); the evidence of every
        /// other format is kept as it is. The results then need to cover only these formats.
        #[arg(long = "format")]
        formats: Vec<String>,
    },
    /// Leave-one-depositor-out cross-validation of the assurance signal on the development
    /// corpus: how often `--strict` would refuse correct files of a depositor it has not seen,
    /// and why (docs/assurance.md).
    Cv {
        /// Write every file's verdict as JSON.
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum SchemaCmd {
    Gen,
}

#[derive(Subcommand)]
enum McpbCmd {
    Pack {
        /// Path to the built `openreadout` binary.
        #[arg(long)]
        binary: PathBuf,
        /// Target triple label for the bundle name (e.g. aarch64-apple-darwin).
        #[arg(long)]
        target: String,
        /// Output directory.
        #[arg(long, default_value = "target/mcpb")]
        out: PathBuf,
    },
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("repo root")
}

fn corpus_dir() -> PathBuf {
    std::env::var_os("OPENREADOUT_CORPUS_DIR")
        .map_or_else(|| root().join("corpus/files"), PathBuf::from)
}

fn sha256_file(p: &Path) -> Result<(String, u64)> {
    let mut file = fs::File::open(p)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let k = file.read(&mut buf)?;
        if k == 0 {
            break;
        }
        hasher.update(&buf[..k]);
        n += k as u64;
    }
    Ok((hex::encode(hasher.finalize()), n))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Corpus { cmd } => corpus::run(cmd),
        Cmd::Schema {
            cmd: SchemaCmd::Gen,
        } => schema_gen(),
        Cmd::VocabCheck => vocab_check(),
        Cmd::SkillParity { fix } => skill_parity(fix),
        Cmd::AgentPlugins { out } => agent_plugins::run(&out),
        Cmd::Mcpb {
            cmd:
                McpbCmd::Pack {
                    binary,
                    target,
                    out,
                },
        } => mcpb::pack(&binary, &target, &out),
        Cmd::HomebrewFormula(a) => packaging::render_file(
            "homebrew/openreadout.rb",
            a.version,
            &a.sums,
            &a.out
                .unwrap_or_else(|| root().join("target/packaging/openreadout.rb")),
        ),
        Cmd::ScoopManifest(a) => packaging::render_file(
            "scoop/openreadout.json",
            a.version,
            &a.sums,
            &a.out
                .unwrap_or_else(|| root().join("target/packaging/openreadout.json")),
        ),
        Cmd::WingetManifest(a) => packaging::winget(
            a.version,
            &a.sums,
            &a.out
                .unwrap_or_else(|| root().join("target/packaging/winget")),
        ),
        Cmd::HeldoutCheck => heldout::check(&root()),
        Cmd::AssuranceAudit {
            cmd:
                Some(AssuranceCmd::Refresh {
                    bin,
                    results,
                    heldout,
                    formats,
                }),
            ..
        } => assurance::refresh(
            &root(),
            &corpus_dir(),
            &if bin.is_absolute() {
                bin
            } else {
                root().join(bin)
            },
            &results,
            heldout.as_deref(),
            &formats,
        ),
        Cmd::AssuranceAudit {
            cmd: Some(AssuranceCmd::Cv { json }),
            ..
        } => assurance::cross_validate(&root(), json.as_deref()),
        Cmd::AssuranceAudit { cmd: None, write } => assurance::audit(&root(), write),
        Cmd::RVendor { out, keep_dir } => rpkg::vendor(&root(), out, keep_dir),
        Cmd::VersionCheck { tag } => packaging::version_check(tag.as_deref()),
        Cmd::Variant { cmd } => variant::run(cmd),
        Cmd::Guides { write } => guides::run(write),
        Cmd::Health { write, reports } => health::run(write, reports),
        Cmd::Coverage(a) => coverage::run(&a),
        Cmd::Snapshot { cmd } => snapshot::run(cmd),
        Cmd::PublishOrder { flags } => publish::run(flags),
        Cmd::Man { out } => man(&out),
        Cmd::Replay {
            src,
            dst,
            pattern,
            interval,
            speed,
            partial,
            stop_after,
        } => replay(
            &src,
            &dst,
            pattern.as_deref(),
            interval,
            speed,
            partial,
            stop_after,
        ),
    }
}

fn replay(
    src: &Path,
    dst: &Path,
    pattern: Option<&str>,
    interval: f64,
    speed: f64,
    partial: bool,
    stop_after: Option<u64>,
) -> Result<()> {
    use openreadout_live::replay::{Pattern, PatternStatus, Replayer, plan};
    let pat = match pattern {
        Some(p) => Pattern::parse(p).with_context(|| format!("unknown pattern {p}"))?,
        None => Pattern::for_path(src).context("cannot pick a pattern; pass --pattern")?,
    };
    let status = match pat.status() {
        PatternStatus::Documented => "documented",
        _ => "assumed",
    };
    eprintln!(
        "replay {} -> {} with the {} pattern ({status}; see book/src/guides/lab-shares.md)",
        src.display(),
        dst.display(),
        pat.name()
    );
    let p = plan(src, pat, partial).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut r = Replayer::new(p, dst).map_err(|e| anyhow::anyhow!("{e}"))?;
    let pause = std::time::Duration::from_secs_f64((interval / speed.max(1e-6)).max(0.0));
    let mut units = 0u64;
    while let Some((kind, unit)) = r.plan().steps.get(r.done()).map(|s| (s.kind, s.unit)) {
        if stop_after.is_some_and(|n| units >= n && unit.is_some()) {
            eprintln!(
                "stopped after {units} units; {} is unfinished",
                dst.display()
            );
            return Ok(());
        }
        if (unit.is_some() || kind == "partial") && r.done() > 0 {
            std::thread::sleep(pause);
        }
        let i = r.done();
        r.step().map_err(|e| anyhow::anyhow!("{e}"))?;
        if unit.is_some() {
            units += 1;
        }
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        println!(
            "{}",
            serde_json::json!({"step": i, "kind": kind, "unit": unit, "units_done": units, "t_ms": ms})
        );
    }
    Ok(())
}

fn schema_gen() -> Result<()> {
    let out = root().join("docs/schema");
    fs::create_dir_all(&out)?;
    let bin = root().join("target/debug/openreadout");
    if !bin.exists() {
        bail!("build the CLI first: cargo build -p openreadout");
    }
    for name in [
        "info-format",
        "info",
        "info-full",
        "info-explain",
        "info-structure",
        "check",
        "export",
        "formats",
        "planes",
        "extract",
        "trace",
        "spectrum",
        "scans",
        "preview",
        "stats",
        "compare",
        "doctor",
        "sidecar",
        "sidecar-file",
        "envelope",
        "index",
        "search",
        "health",
        "export-dataset",
        "watch",
        "gate",
        "table",
        "qpcr",
        "nmr-peaks",
        "ephys-features",
        "spikes",
        "chromatogram",
        "peaks",
        "assay",
        "stats-wells",
        "batch-table",
        "summarize",
        "link",
        "report",
    ] {
        let o = std::process::Command::new(&bin)
            .args(["self", "schema", name])
            .output()?;
        if !o.status.success() {
            bail!(
                "self schema {name} failed: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        }
        let p = out.join(format!("{name}.schema.json"));
        fs::write(&p, &o.stdout)?;
        println!("wrote {}", p.display());
    }
    Ok(())
}

/// Completion scripts written by `cargo xtask man`: (shell, file name).
const COMPLETIONS: [(&str, &str); 5] = [
    ("bash", "openreadout.bash"),
    ("zsh", "_openreadout"),
    ("fish", "openreadout.fish"),
    ("powershell", "_openreadout.ps1"),
    ("elvish", "openreadout.elv"),
];

fn man(out: &Path) -> Result<()> {
    let run = |args: &[&str]| -> Result<Vec<u8>> {
        let o =
            std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
                .current_dir(root())
                .args(["run", "--quiet", "-p", "openreadout", "--"])
                .args(args)
                .output()
                .context("running cargo")?;
        if !o.status.success() {
            bail!(
                "openreadout {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&o.stderr)
            );
        }
        Ok(o.stdout)
    };
    let out = if out.is_absolute() {
        out.to_path_buf()
    } else {
        std::env::current_dir()?.join(out)
    };
    let man_dir = out.join("man");
    let man_arg = man_dir.to_string_lossy().to_string();
    run(&["--quiet", "self", "man", "--out", &man_arg])?;
    let pages = fs::read_dir(&man_dir)?
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "1"))
        .count();
    if pages == 0 {
        bail!("no man pages were written to {}", man_dir.display());
    }
    println!("wrote {pages} man pages to {}", man_dir.display());
    let comp = out.join("completions");
    fs::create_dir_all(&comp)?;
    for (shell, name) in COMPLETIONS {
        let text = run(&["self", "completions", shell])?;
        let p = comp.join(name);
        fs::write(&p, text)?;
        println!("wrote {}", p.display());
    }
    Ok(())
}

/// Public identifiers declared in one source file: items, and `pub name:` fields.
fn public_idents(src: &str) -> BTreeSet<String> {
    let mut idents = BTreeSet::new();
    for line in src.lines() {
        let l = line.trim_start();
        for kw in [
            "pub struct ",
            "pub enum ",
            "pub fn ",
            "pub const ",
            "pub trait ",
            "pub type ",
        ] {
            if let Some(rest) = l.strip_prefix(kw) {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    idents.insert(name);
                }
            }
        }
        // public struct fields: `pub name:` inside structs
        if let Some(rest) = l.strip_prefix("pub ")
            && rest.contains(':')
            && !rest.starts_with("fn ")
            && !rest.starts_with("struct ")
            && !rest.starts_with("enum ")
            && !rest.starts_with("const ")
            && !rest.starts_with("use ")
            && !rest.starts_with("mod ")
            && !rest.starts_with("type ")
        {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && name != "crate" {
                idents.insert(name);
            }
        }
    }
    idents
}

/// Format crates and the vocabulary documents their public identifiers must appear in. A crate
/// may implement several formats: a source file whose name starts with a listed prefix is
/// checked against that prefix's document; other files against any of the crate's documents.
const VOCAB: &[(&str, &[(&str, &str)])] = &[
    ("czi", &[("", "czi")]),
    ("nd2", &[("", "nd2")]),
    ("lif", &[("", "lif")]),
    ("tiff", &[("", "tiff")]),
    ("fcs", &[("gating", "flowjo-wsp"), ("", "fcs")]),
    ("oir", &[("", "oir")]),
    ("vsi", &[("", "vsi")]),
    // whole-slide formats stored as many images: one module directory per format
    ("wsi", &[("mirax", "mirax")]),
    ("abf", &[("", "abf")]),
    ("neuralynx", &[("", "neuralynx")]),
    ("blackrock", &[("", "blackrock")]),
    ("spikeglx", &[("", "spikeglx")]),
    ("intan", &[("", "intan")]),
    ("plexon", &[("", "plexon")]),
    // one module directory per format; lib.rs/assurance.rs are checked against all documents
    (
        "ephys",
        &[
            ("heka", "heka-patchmaster"),
            ("spike2", "ced-spike2"),
            ("openephys", "open-ephys"),
            ("winwcp", "winwcp"),
        ],
    ),
    (
        "nmr",
        &[
            ("bruker", "bruker-nmr"),
            ("jcamp", "jcamp-dx"),
            ("varian", "varian-nmr"),
            ("jeol", "jeol-jdf"),
            ("spinsolve", "magritek-spinsolve"),
        ],
    ),
    ("thermo", &[("", "thermo-raw")]),
    ("agilent-ms", &[("", "agilent-masshunter")]),
    ("waters", &[("", "waters-raw")]),
    ("sciex", &[("", "sciex-wiff")]),
    ("mzml", &[("", "mzml")]),
    ("bruker-tims", &[("", "bruker-tdf")]),
    // file-name prefixes per format; lib.rs/binary.rs are checked against all four documents
    (
        "chrom",
        &[
            ("chemstation", "chemstation"),
            ("andi", "andi-chrom"),
            ("netcdf", "andi-chrom"),
            ("shimadzu", "shimadzu"),
            ("openlab", "openlab-cds"),
            ("chromeleon", "chromeleon"),
            ("empower", "empower-arw"),
        ],
    ),
    ("plate", &[("", "plate-readers")]),
    ("qpcr", &[("", "qpcr")]),
    // analysis crate: nmr/ and ephys/ modules; lib.rs/fft.rs are checked against both documents
    (
        "signal",
        &[("nmr", "nmr-processing"), ("ephys", "ephys-analysis")],
    ),
    ("zarr", &[("", "ome-zarr")]),
    ("zvi", &[("", "zvi")]),
    ("oif", &[("", "oif")]),
    ("dcimg", &[("", "dcimg")]),
    // high-content screening plates: one module per format; lib/model/dataset/planes are
    // checked against all four documents (hcs.md holds the shared plate model)
    (
        "hcs",
        &[
            ("harmony", "opera-harmony"),
            ("imagexpress", "imagexpress"),
            ("cellvoyager", "cellvoyager"),
            ("", "hcs"),
        ],
    ),
    // one module per format; lib.rs/common.rs are checked against every document
    (
        "spectro",
        &[
            ("opus", "bruker-opus"),
            ("omnic", "thermo-omnic"),
            ("wdf", "renishaw-wdf"),
            ("pesp", "perkinelmer-sp"),
            ("spc", "galactic-spc"),
            ("witec", "witec-project"),
            ("agilent", "agilent-fpa"),
            ("fsm", "perkinelmer-fsm"),
            ("jasco", "jasco-jws"),
            ("cary", "agilent-cary"),
        ],
    ),
    // ÄKTA/UNICORN .res and result exports share one note
    ("fplc", &[("", "cytiva-unicorn")]),
    (
        "biophys",
        &[
            ("itc", "microcal-itc"),
            ("biacore", "cytiva-biacore"),
            ("seahorse", "agilent-seahorse"),
            ("octet", "sartorius-octet"),
            ("zetasizer", "malvern-zetasizer"),
            ("gpr", "genepix-gpr"),
        ],
    ),
    ("gel", &[("", "biorad-scn")]),
    // BES3T and ESP/WinEPR share one note
    ("epr", &[("", "bruker-epr")]),
    // one module per format; lib.rs/common.rs are checked against the shared note
    ("xrd", &[("", "xrd")]),
    // one module per vendor; lib.rs is checked against every document
    (
        "echem",
        &[
            ("arbin", "arbin-res"),
            ("eclab", "biologic-eclab"),
            ("gamry", "gamry-dta"),
            ("neware", "neware"),
        ],
    ),
    (
        "thermal",
        &[
            ("ngb", "netzsch-ngb"),
            ("trios", "ta-trios"),
            ("ta", "ta-universal-analysis"),
        ],
    ),
    // Imaris, NWB and generic HDF5 share h5util.rs/lib.rs (checked against both documents)
    (
        "hdf5",
        &[("ims", "ims"), ("nwb", "hdf5"), ("generic", "hdf5")],
    ),
    // one module directory per format; lib.rs/util.rs are checked against all four documents
    (
        "em",
        &[("mrc", "mrc"), ("dm", "dm"), ("ser", "ser"), ("emd", "emd")],
    ),
];

/// `.rs` files under `dir`, recursively, with their path relative to `dir` (`/`-separated, no
/// extension): `lib`, `mrc/dataset`, ...
fn rust_sources(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d)? {
            let p = entry?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "rs") {
                let rel = p
                    .strip_prefix(dir)
                    .unwrap_or(&p)
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, p));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Public identifiers in `crates/openreadout-<crate>/src/**/*.rs` must appear in `docs/formats/<fmt>.md`.
fn vocab_check() -> Result<()> {
    let mut failures = 0;
    for (krate, docs) in VOCAB {
        let texts: Vec<(&str, String)> = docs
            .iter()
            .map(|(prefix, doc)| {
                fs::read_to_string(root().join(format!("docs/formats/{doc}.md")))
                    .map(|t| (*prefix, t))
                    .with_context(|| format!("read docs/formats/{doc}.md"))
            })
            .collect::<Result<_>>()?;
        let names: Vec<String> = docs
            .iter()
            .map(|(_, d)| format!("docs/formats/{d}.md"))
            .collect();
        let src_dir = root().join(format!("crates/openreadout-{krate}/src"));
        let mut count = 0usize;
        let mut missing: BTreeSet<String> = BTreeSet::new();
        for (stem, p) in rust_sources(&src_dir)? {
            let mut owners: Vec<&String> = texts
                .iter()
                .filter(|(prefix, _)| !prefix.is_empty() && stem.starts_with(prefix))
                .map(|(_, t)| t)
                .collect();
            if owners.is_empty() {
                owners = texts.iter().map(|(_, t)| t).collect();
            }
            for ident in public_idents(&fs::read_to_string(&p)?) {
                count += 1;
                if !owners.iter().any(|doc| doc.contains(ident.as_str())) {
                    missing.insert(format!(
                        "{ident}  ({})",
                        p.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
            }
        }
        if missing.is_empty() {
            println!(
                "{krate}: {count} public identifiers, all in {}",
                names.join(" / ")
            );
        } else {
            failures += missing.len();
            println!(
                "{krate}: {} identifiers missing from {}:",
                missing.len(),
                names.join(" / ")
            );
            for m in missing {
                println!("    {m}");
            }
        }
    }
    if failures > 0 {
        bail!(
            "{failures} identifiers are not in their format's vocabulary table (see docs/legal/clean-room-policy.md rule 5)"
        );
    }
    Ok(())
}

fn skill_parity(fix: bool) -> Result<()> {
    let skill = root().join("skills/openreadout");
    // The CLI crate embeds its own copy (SKILL.md and references/) so `cargo publish` can
    // package it and `openreadout self skill --install` can write it.
    let copies = [root().join("crates/openreadout-cli/SKILL.md")];
    let reference_copies = [root().join("crates/openreadout-cli/references")];
    let mut wanted: Vec<(PathBuf, String)> = Vec::new();
    let src = fs::read_to_string(skill.join("SKILL.md"))?;
    for c in &copies {
        wanted.push((c.clone(), src.clone()));
    }
    let mut refs: Vec<PathBuf> = fs::read_dir(skill.join("references"))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .collect();
    refs.sort();
    for dir in &reference_copies {
        for r in &refs {
            let name = r.file_name().context("reference file name")?;
            wanted.push((dir.join(name), fs::read_to_string(r)?));
        }
    }
    let mut bad = 0;
    for (c, text) in &wanted {
        let same = fs::read_to_string(c).is_ok_and(|s| &s == text);
        if same {
            println!("ok      {}", c.strip_prefix(root()).unwrap_or(c).display());
        } else if fix {
            fs::create_dir_all(c.parent().unwrap())?;
            fs::write(c, text)?;
            println!("fixed   {}", c.strip_prefix(root()).unwrap_or(c).display());
        } else {
            bad += 1;
            println!("DIFFERS {}", c.strip_prefix(root()).unwrap_or(c).display());
        }
    }
    // A reference file removed from the skill is removed from the copies.
    for dir in &reference_copies {
        let Ok(rd) = fs::read_dir(dir) else { continue };
        for e in rd.filter_map(std::result::Result::ok) {
            let p = e.path();
            if wanted.iter().any(|(w, _)| *w == p) {
                continue;
            }
            if fix {
                fs::remove_file(&p)?;
                println!("removed {}", p.strip_prefix(root()).unwrap_or(&p).display());
            } else {
                bad += 1;
                println!("EXTRA   {}", p.strip_prefix(root()).unwrap_or(&p).display());
            }
        }
    }
    if bad > 0 {
        bail!("{bad} skill copies differ; run `cargo xtask skill-parity --fix`");
    }
    Ok(())
}
