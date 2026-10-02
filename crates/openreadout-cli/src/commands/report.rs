//! `openreadout check FILE --report`: a privacy-reviewed diagnostic bundle for a new-variant issue
//! (`openreadout_index::report`, docs/maintaining.md). Written to a local file; nothing is sent.

use std::path::{Path, PathBuf};

use openreadout_core::{Error, Registry};
use openreadout_index::report::{self, ReportOptions, ReportOutput};

use crate::output::{emit, fail, json_text};

/// `check --report`: the diagnostic bundle's options.
#[derive(Debug, Clone, Default, clap::Args)]
#[command(next_help_heading = "Diagnostic bundle (--report)")]
pub struct ReportOpts {
    /// Write a privacy-reviewed diagnostic bundle for a file that was refused, failed or is not
    /// `validated`, to attach to a new-variant issue: the assurance fingerprint, every decode
    /// stage with its error, the structure map and the metadata's numbers. No pixel, spectral
    /// or trace values; no free text or path unless `--include-text`. Written to a local file;
    /// nothing is sent (docs/maintaining.md).
    #[arg(long)]
    pub report: bool,
    /// Where to write the bundle. Default: `openreadout-report-<hash>.json` in the current
    /// directory (the file's name is never used).
    #[arg(short, long, value_name = "PATH", requires = "report")]
    pub output: Option<PathBuf>,
    /// Print the bundle exactly as it would be written, and write nothing.
    #[arg(long, requires = "report")]
    pub dry_run: bool,
    /// Replace an existing bundle at the output path.
    #[arg(long, requires = "report")]
    pub overwrite: bool,
    /// Keep free text from the file (sample, image and channel names, comments). The path and
    /// personal data (operators, e-mail addresses, phone numbers, patient ids) are still replaced.
    #[arg(long, requires = "report")]
    pub include_text: bool,
    /// Add hex excerpts of N bytes (at most 256) of the file head and of up to 32 structure
    /// headers; printable text in them is masked unless --include-text.
    #[arg(long, value_name = "N", default_value_t = 0, requires = "report")]
    pub hex: usize,
    /// Run the full integrity check (decodes and checksums data) instead of headers only.
    #[arg(long, requires = "report")]
    pub full_check: bool,
    /// Leave out the file's SHA-256.
    #[arg(long, requires = "report")]
    pub no_hash: bool,
    /// Do not decode a first plane, sweep, spectrum or table rows.
    #[arg(long, requires = "report")]
    pub no_first_read: bool,
}

/// Run `check --report` on `file`.
pub fn run(reg: &Registry, file: &Path, a: &ReportOpts, json: bool) -> i32 {
    let mut opts = ReportOptions::default();
    opts.include_text = a.include_text;
    opts.hex_bytes = a.hex.min(report::MAX_HEX_BYTES);
    opts.full_check = a.full_check;
    opts.hash = !a.no_hash;
    opts.first_read = !a.no_first_read;
    if a.dry_run && a.output.is_some() {
        return fail(
            json,
            &Error::Usage("--dry-run writes nothing; leave out --output".into()),
        );
    }
    // The output path is decided up front so that a reader crash still leaves a report there
    // (the panic handler finishes it).
    let path = if a.dry_run {
        None
    } else {
        Some(
            a.output
                .clone()
                .unwrap_or_else(|| PathBuf::from(report::default_name(file))),
        )
    };
    if let Some(p) = &path
        && p.exists()
        && !a.overwrite
    {
        return fail(
            json,
            &Error::Usage(format!(
                "{} exists (a report of the same input?); pass --overwrite to replace it, or -o to write elsewhere",
                p.display()
            )),
        );
    }
    let r = match report::build(reg, file, &opts, path.as_deref().map(|p| (p, a.overwrite))) {
        Ok(r) => r,
        Err(e) => return fail(json, &e),
    };
    let mut out = ReportOutput {
        output: None,
        bytes: None,
        issue_url: report::ISSUE_URL.into(),
        report: r,
    };
    if a.dry_run {
        if json {
            return emit(true, &out, |_| String::new());
        }
        match json_text(&out.report) {
            Ok(s) => println!("{s}"),
            Err(e) => return fail(false, &Error::Other(e.to_string())),
        }
        return 0;
    }
    let Some(path) = path else {
        return 0;
    };
    let bytes = match serde_json::to_vec_pretty(&out.report) {
        Ok(mut b) => {
            b.push(b'\n');
            b
        }
        Err(e) => return fail(json, &Error::Other(e.to_string())),
    };
    if let Err(e) = report::write_verified(&path, &bytes, a.overwrite) {
        return fail(json, &e);
    }
    out.output = Some(path.display().to_string());
    out.bytes = Some(bytes.len() as u64);
    emit(json, &out, report::render)
}
