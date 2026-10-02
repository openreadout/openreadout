//! Batch mode for the read-only commands (`info` in every view, `check`, `check --planes`,
//! `stats`) and `export`.
//!
//! One plain file argument gives one envelope (or one human report) and that file's exit code.
//! Several arguments, a directory, a glob pattern, `--recursive` or `--jsonl` switch to batch
//! mode:
//!
//! - inputs are expanded by [`openreadout_core::batch::expand`] (directory data sets are one
//!   item; hidden entries are skipped; globs are expanded for shells that do not);
//! - every input is processed even when one fails (`--continue-on-error`, the default), or
//!   processing stops at the first failure with `--fail-fast`;
//! - `--jsonl` prints one compact envelope per input and line, each with its `path`;
//!   `--json` prints one JSON array of those envelopes; human output prints each report under a
//!   `==> path <==` header, then a summary table (path, format, contents, status);
//! - files found in a directory or by a glob that no reader recognises but that are a lab's own
//!   companions — sample sheets, sequences, plate maps, compound or region lists, READMEs
//!   ([`openreadout_batch::companion`]) — are skipped and reported as skipped (a note on
//!   stderr, a row of the summary table), not as unknown-format errors;
//! - the exit code is the highest exit code of any input (0 when all succeeded).
//!
//! Standard input (`-`) is spooled to a temporary file for commands that open files (its size
//! is capped by `OPENREADOUT_STDIN_MAX_BYTES`, default 4 GiB); `info --view format` sniffs it directly.

use std::io::Read;
use std::path::{Path, PathBuf};

use openreadout_core::InfoOutput;
use openreadout_core::batch::{BatchInput, ExpandOptions, expand};
use openreadout_core::model::{CheckReport, DetectOutput, Dump, FileInfo, Listing, PlanesOutput};
use openreadout_core::stats::StatsOutput;
use openreadout_core::{Envelope, Error, Registry, Result};
use openreadout_ops::explain::Explanation;
use serde::Serialize;

use crate::output::{emit, fail, print_error, tool_id};
use crate::ui::{self, Align, Progress};

/// Batch flags shared by the batch-capable commands.
#[derive(Debug, Clone, Default, clap::Args)]
#[command(next_help_heading = "Several inputs")]
pub struct BatchArgs {
    /// Walk sub-directories of directory arguments (a directory a reader recognizes as a
    /// data set is one input and is not walked into).
    #[arg(short = 'r', long)]
    pub recursive: bool,
    /// Batch output as JSON Lines: one compact envelope per input, each with its `path`.
    #[arg(long)]
    pub jsonl: bool,
    /// Keep going after an input fails (the default in batch mode).
    #[arg(long, overrides_with = "fail_fast")]
    pub continue_on_error: bool,
    /// Stop at the first input that fails (non-zero exit code).
    #[arg(long, overrides_with = "continue_on_error")]
    pub fail_fast: bool,
    /// Leave out inputs that are not instrument files (unknown format, exit 3) instead of
    /// reporting them.
    #[arg(long)]
    pub skip_unknown: bool,
    // `--help` layout: the command's own flags that follow stay under "Options"
    #[command(flatten)]
    pub end_heading: super::help_heading::EndHeading,
}

/// Whether a command accepts `-` (standard input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stdin {
    /// Not supported: `-` is a usage error.
    Reject,
    /// Spool standard input to a temporary file and open that.
    Spool,
    /// The command reads standard input itself (`info --view format`).
    Pass,
}

/// How one command runs in batch mode.
#[derive(Debug, Clone, Copy)]
pub struct Spec<'a> {
    pub json: bool,
    pub batch: &'a BatchArgs,
    pub stdin: Stdin,
}

/// What a command's closure gets for one input.
#[derive(Debug)]
pub struct Input<'a> {
    pub item: &'a BatchInput,
    /// The path to open (a temporary spool file for `-`).
    pub path: &'a Path,
    /// True in batch mode.
    pub batch: bool,
}

/// A command result that batch mode can summarize.
pub trait Item: Serialize {
    /// Exit code of a successful result (e.g. `check` of a corrupt file is 4).
    fn exit_code(&self) -> i32 {
        0
    }
    fn format(&self) -> Option<String> {
        None
    }
    /// What the input holds, in a few words.
    fn contents(&self) -> Option<String> {
        None
    }
    /// Replace the (spool file) path in the result with `to` (`-` for standard input).
    fn relabel(&mut self, _to: &str) {}
}

pub fn file_contents(i: &FileInfo) -> String {
    let mut parts = Vec::new();
    let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
    if !i.images.is_empty() {
        parts.push(format!(
            "{}, {}",
            plural(i.images.len(), "image"),
            plural(i.plane_count as usize, "plane")
        ));
    }
    if !i.tables.is_empty() {
        parts.push(plural(i.tables.len(), "table"));
    }
    if !i.traces.is_empty() {
        parts.push(plural(i.traces.len(), "trace"));
    }
    if !i.spectra.is_empty() {
        parts.push(plural(i.spectra.len(), "spectrum run"));
    }
    if parts.is_empty() {
        "empty".into()
    } else {
        parts.join(", ")
    }
}

impl Item for DetectOutput {
    fn format(&self) -> Option<String> {
        Some(self.format.clone())
    }
    fn contents(&self) -> Option<String> {
        Some(format!("{:?}", self.confidence).to_lowercase())
    }
    fn relabel(&mut self, to: &str) {
        self.path = to.into();
    }
}

impl Item for FileInfo {
    fn format(&self) -> Option<String> {
        Some(self.format.id.clone())
    }
    fn contents(&self) -> Option<String> {
        Some(file_contents(self))
    }
    fn relabel(&mut self, to: &str) {
        self.path = to.into();
    }
}

impl Item for InfoOutput {
    fn format(&self) -> Option<String> {
        self.file.format()
    }
    fn contents(&self) -> Option<String> {
        self.file.contents()
    }
    fn relabel(&mut self, to: &str) {
        self.file.relabel(to);
    }
}

impl Item for Dump {
    fn format(&self) -> Option<String> {
        self.file.format()
    }
    fn contents(&self) -> Option<String> {
        self.file.contents()
    }
    fn relabel(&mut self, to: &str) {
        self.file.relabel(to);
    }
}

impl Item for Explanation {
    fn contents(&self) -> Option<String> {
        let s: String = self.summary.chars().take(60).collect();
        Some(if s.len() < self.summary.len() {
            format!("{s}…")
        } else {
            s
        })
    }
}

impl Item for Listing {
    fn format(&self) -> Option<String> {
        Some(self.format.clone())
    }
    fn contents(&self) -> Option<String> {
        Some(format!("{} entries", self.entries.len()))
    }
    fn relabel(&mut self, to: &str) {
        self.path = to.into();
    }
}

impl Item for CheckReport {
    fn exit_code(&self) -> i32 {
        if self.ok { 0 } else { 4 }
    }
    fn format(&self) -> Option<String> {
        Some(self.format.clone())
    }
    fn contents(&self) -> Option<String> {
        Some(if self.findings.is_empty() {
            "intact".into()
        } else {
            match self.findings.len() {
                1 => "1 finding".into(),
                n => format!("{n} findings"),
            }
        })
    }
    fn relabel(&mut self, to: &str) {
        self.path = to.into();
    }
}

impl Item for PlanesOutput {
    fn format(&self) -> Option<String> {
        Some(self.format.clone())
    }
    fn contents(&self) -> Option<String> {
        Some(format!("{} planes", self.planes.len()))
    }
    fn relabel(&mut self, to: &str) {
        self.path = to.into();
    }
}

impl Item for StatsOutput {
    fn format(&self) -> Option<String> {
        Some(self.format.clone())
    }
    fn contents(&self) -> Option<String> {
        let planes: u64 = self.images.iter().map(|i| i.planes).sum();
        Some(format!("{planes} planes"))
    }
    fn relabel(&mut self, to: &str) {
        self.path = to.into();
    }
}

/// A temporary copy of standard input, deleted on drop.
#[derive(Debug)]
pub struct Spool {
    pub path: PathBuf,
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Default cap on spooled standard input.
pub const STDIN_MAX_BYTES: u64 = 4 << 30;

fn stdin_cap() -> u64 {
    std::env::var("OPENREADOUT_STDIN_MAX_BYTES")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(STDIN_MAX_BYTES)
}

/// Copy standard input to a temporary file (at most `OPENREADOUT_STDIN_MAX_BYTES`).
pub fn spool_stdin() -> Result<Spool> {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "openreadout-stdin-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let spool = Spool { path };
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&spool.path)
        .map_err(|e| Error::io(&spool.path, e))?;
    let cap = stdin_cap();
    let n = std::io::copy(&mut std::io::stdin().lock().take(cap + 1), &mut f)
        .map_err(|e| Error::io("<stdin>", e))?;
    if n > cap {
        return Err(Error::Usage(format!(
            "standard input is larger than {cap} bytes; pass the file by path, or raise OPENREADOUT_STDIN_MAX_BYTES"
        )));
    }
    Ok(spool)
}

/// The first bytes of standard input (for `info --view format -`).
pub fn stdin_head() -> Result<Vec<u8>> {
    let mut head = Vec::new();
    std::io::stdin()
        .lock()
        .take(openreadout_core::reader::SNIFF_LEN as u64)
        .read_to_end(&mut head)
        .map_err(|e| Error::io("<stdin>", e))?;
    Ok(head)
}

fn process<T: Item>(
    item: &BatchInput,
    batch: bool,
    stdin: Stdin,
    f: &mut dyn FnMut(&Input<'_>) -> Result<T>,
) -> Result<T> {
    if item.path.as_os_str() == "-" {
        match stdin {
            Stdin::Reject => {
                return Err(Error::Usage(
                    "this command does not read standard input (`-`); pass a file path".into(),
                ));
            }
            Stdin::Spool => {
                let spool = spool_stdin()?;
                let mut v = f(&Input {
                    item,
                    path: &spool.path,
                    batch,
                })?;
                v.relabel("-");
                return Ok(v);
            }
            Stdin::Pass => {}
        }
    }
    f(&Input {
        item,
        path: &item.path,
        batch,
    })
}

fn status_name(code: i32) -> &'static str {
    match code {
        0 => "ok",
        2 => "usage",
        3 => "unknown format",
        4 => "corrupt",
        5 => "i/o error",
        6 => "unsupported",
        _ => "error",
    }
}

/// Run `f` over `files`: one input as a single result, several as a batch (see the module docs).
pub fn run<T: Item>(
    reg: &Registry,
    files: &[PathBuf],
    spec: Spec<'_>,
    f: &mut dyn FnMut(&Input<'_>) -> Result<T>,
    human: &dyn Fn(&T) -> String,
) -> i32 {
    let b = spec.batch;
    let exp = expand(reg, files, {
        let mut expand_options = ExpandOptions::default();
        expand_options.recursive = b.recursive;
        expand_options.glob = true;
        expand_options
    });
    let batch = b.jsonl || b.recursive || files.len() > 1 || exp.expanded;
    if !batch {
        let Some(item) = exp.items.into_iter().next() else {
            return fail(spec.json, &Error::Usage("no input given".into()));
        };
        if let Some(e) = item.error {
            return fail(spec.json, &e);
        }
        return match process(&item, false, spec.stdin, f) {
            Ok(v) => {
                let code = v.exit_code();
                // An error while printing (a `--only` pointer that fails) is the exit code.
                match emit(spec.json, &v, human) {
                    0 => code,
                    rc => rc,
                }
            }
            Err(e) => fail(spec.json, &e),
        };
    }

    let array = (spec.json || crate::output::only_set()) && !b.jsonl;
    let as_json = spec.json || b.jsonl || crate::output::only_set();
    let quiet = ui::quiet();
    let total = exp.items.len() as u64;
    let progress = Progress::new(total, "files");
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut worst = 0;
    let (mut ok, mut failed, mut skipped) = (0u64, 0u64, 0u64);
    let mut companions = 0u64;
    let mut first = true;
    if array {
        println!("[");
    }
    for (i, mut item) in exp.items.into_iter().enumerate() {
        let shown = item.path.display().to_string();
        progress.set(i as u64, Some(&shown));
        let res = match item.error.take() {
            Some(e) => Err(e),
            None => process(&item, true, spec.stdin, f),
        };
        if b.skip_unknown && matches!(res, Err(Error::UnknownFormat { .. })) {
            skipped += 1;
            continue;
        }
        // a sample sheet, plate map or README found in a folder is not data
        if matches!(res, Err(Error::UnknownFormat { .. }))
            && !files.iter().any(|f| f == &item.path)
            && let Some(kind) = openreadout_batch::companion::classify(&item.path)
        {
            companions += 1;
            if !quiet {
                progress.suspend(|| {
                    eprintln!(
                        "note: skipped {shown}: {} (not instrument data)",
                        kind.describe()
                    );
                });
            }
            rows.push(vec![
                shown,
                "-".into(),
                kind.describe().into(),
                ui::paint(ui::DIM, "skipped"),
            ]);
            continue;
        }
        let (code, format, contents) = match &res {
            Ok(v) => (v.exit_code(), v.format(), v.contents()),
            Err(e) => (e.exit_code(), None, None),
        };
        progress.suspend(|| {
            if as_json {
                let owned_err;
                let outcome: std::result::Result<serde_json::Value, &Error> = match &res {
                    Ok(v) => match crate::output::data_value(v) {
                        Ok(x) => Ok(x),
                        Err(e) => {
                            owned_err = e;
                            Err(&owned_err)
                        }
                    },
                    Err(e) => Err(e),
                };
                let text = match outcome {
                    Ok(v) => {
                        let env = Envelope::ok(tool_id(), v).with_path(shown.clone());
                        if b.jsonl {
                            serde_json::to_string(&env)
                        } else {
                            crate::output::json_text(&env)
                        }
                    }
                    Err(e) => {
                        let env: Envelope<()> =
                            Envelope::err(tool_id(), e).with_path(shown.clone());
                        if b.jsonl {
                            serde_json::to_string(&env)
                        } else {
                            crate::output::json_text(&env)
                        }
                    }
                }
                .expect("serializable");
                if b.jsonl {
                    println!("{text}");
                } else {
                    if !first {
                        println!(",");
                    }
                    print!("{text}");
                }
                first = false;
            } else {
                match &res {
                    Ok(v) if !quiet => {
                        anstream::println!("{}", ui::paint(ui::BOLD, format!("==> {shown} <==")));
                        anstream::println!("{}\n", human(v));
                    }
                    Ok(_) => {}
                    Err(e) => print_error(Some(&shown), e),
                }
            }
        });
        if code == 0 {
            ok += 1;
        } else {
            failed += 1;
        }
        worst = worst.max(code);
        let format = format.or_else(|| {
            (item.path.as_os_str() != "-")
                .then(|| {
                    reg.detect(&item.path)
                        .ok()
                        .map(|(_, d)| d.format_id.to_string())
                })
                .flatten()
        });
        let status = if code == 0 {
            ui::paint(ui::OK, "ok")
        } else {
            ui::paint(ui::ERR, format!("{} ({code})", status_name(code)))
        };
        rows.push(vec![
            shown,
            format.unwrap_or_else(|| "-".into()),
            contents.unwrap_or_else(|| "-".into()),
            status,
        ]);
        if code != 0 && b.fail_fast {
            break;
        }
    }
    progress.finish();
    if array {
        println!("\n]");
    }
    if !as_json && !quiet {
        anstream::println!(
            "{}",
            ui::table(
                &["path", "format", "contents", "status"],
                &[Align::Left, Align::Left, Align::Left, Align::Left],
                &rows
            )
        );
        let mut tail = format!("{} inputs: {ok} ok, {failed} failed", ok + failed);
        if companions > 0 {
            tail.push_str(&format!(
                ", {companions} skipped (sample sheets, plate maps, documents)"
            ));
        }
        if skipped > 0 {
            tail.push_str(&format!(", {skipped} skipped (unknown format)"));
        }
        if b.fail_fast && (ok + failed + skipped + companions) < total {
            tail.push_str(&format!(
                ", stopped after the first failure ({} not processed)",
                total - ok - failed - skipped - companions
            ));
        }
        anstream::println!("{}", ui::paint(ui::DIM, tail));
    }
    worst
}
