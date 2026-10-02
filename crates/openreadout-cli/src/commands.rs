//! Command implementations: one module per command (`info`, `check`, `export`, `trace`,
//! `stats`, ...), plus the batch driver and helpers they share.

pub mod assay;
pub mod batch;
pub mod check;
pub mod chromatogram;
pub mod compare;
pub mod doctor;
pub mod doctor_analysis;
pub mod ephys;
pub mod export;
pub mod flow;
pub mod help_heading;
pub mod index;
pub mod info;
pub mod measure;
pub mod nmr;
pub mod peaks;
pub mod plate;
pub mod qpcr;
pub mod report;
pub mod self_cmd;
pub mod sidecar;
pub mod spectra;
pub mod stats;
pub mod summarize;
pub mod tidy;
pub mod trace;
pub mod watch;

use std::path::Path;

use openreadout_core::Result;

use crate::output::{emit, fail};
use crate::{AnalyzeKind, Command};

/// Run one parsed command; the return value is the process exit code.
pub fn run(cmd: Command) -> i32 {
    let reg = crate::registry::registry();
    match cmd {
        Command::Info(a) => info::run(&reg, &a),
        Command::Check(a) => check::run(&reg, &a),
        Command::Export(a) => export::run(&reg, a),
        Command::Trace(a) => trace::run(&reg, &a),
        Command::Stats(a) => stats::run(&reg, &a),
        Command::Spectra(a) => spectra::run(&reg, &a),
        Command::Table(a) => flow::run_table(&reg, &a),
        Command::Analyze(kind) => match kind {
            AnalyzeKind::Peaks(a) => peaks::run(&reg, &a),
            AnalyzeKind::Chromatogram(a) => chromatogram::run(&reg, &a),
            AnalyzeKind::NmrPeaks(a) => nmr::run_nmr_peaks(&reg, &a),
            AnalyzeKind::EphysFeatures(a) => ephys::run_ephys_features(&reg, &a),
            AnalyzeKind::Spikes(a) => ephys::run_spikes(&reg, &a),
            AnalyzeKind::Qpcr(a) => qpcr::run(&reg, &a),
            AnalyzeKind::Assay(a) => assay::run(&reg, &a),
            AnalyzeKind::Gate(a) => flow::run_gate(&reg, &a),
        },
        Command::Batch(a) => measure::run(&reg, &a),
        Command::Link(a) => summarize::run_link(&reg, &a),
        Command::Watch(a) => watch::run(&reg, &a),
        Command::Index(a) => index::index(&reg, &a),
        Command::Search(a) => index::search(&reg, &a),
        Command::Preview(a) => crate::preview_cmd::run(&reg, &a),
        Command::Mcp(a) => crate::mcp_config::run(&a),
        Command::SelfCmd(c) => self_cmd::run(&reg, c),
    }
}

fn wrap<T: serde::Serialize>(
    json: bool,
    f: impl FnOnce() -> Result<T>,
    human: impl FnOnce(&T) -> String,
) -> i32 {
    match f() {
        Ok(v) => emit(json, &v, human),
        Err(e) => fail(json, &e),
    }
}

/// `info` and `check`: a corrupt-looking file modified within the live window
/// says it may still be being written (formats that cannot be read while growing).
fn live_err<T>(file: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    f().map_err(|e| openreadout_core::live::annotate_error(file, e))
}

/// One line for people: the assurance level, painted, and its summary.
pub(crate) fn render_assurance_line(a: &openreadout_core::assurance::Assurance) -> String {
    use openreadout_core::assurance::AssuranceLevel;
    let level = match a.level {
        AssuranceLevel::Validated => crate::ui::paint(crate::ui::OK, "validated"),
        AssuranceLevel::PartiallyValidated => "partially validated".to_string(),
        AssuranceLevel::Unvalidated => crate::ui::paint(crate::ui::ERR, "UNVALIDATED"),
        _ => a.level.as_str().to_string(),
    };
    let detail = a
        .summary
        .split_once(": ")
        .map_or(a.summary.as_str(), |(_, rest)| rest);
    format!("  assurance: {level} - {detail}\n")
}

fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}
