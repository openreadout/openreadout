//! `openreadout` command-line entry point.
//!
//! One static binary that reads raw lab-instrument files (microscopy, mass spectrometry, flow
//! cytometry, electrophysiology, chromatography, NMR, plate readers) without vendor software:
//! JSON metadata (`info`), integrity checks (`check`), exports to open formats (`export`),
//! analyses (`analyze`), previews and an MCP server (`mcp`). See `openreadout --help` and `book/src/reference/commands/`.
//!
//! Cargo features: `mcp` (default) builds the `mcp` subcommand; `mcp-http` adds
//! `openreadout mcp --http`, a loopback Streamable HTTP transport, and is off by default
//! because the shipped binary does no networking.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod commands;
mod csv;
mod mcp_config;
mod output;
mod panic;
mod preview_cmd;
mod registry;
mod ui;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// `OpenReadout`: read raw lab-instrument files without the vendor software.
///
/// Every command accepts `--json` and returns a stable envelope
/// `{"ok":true,"schema_version":"1","tool":{...},"data":{...}}`; errors carry a
/// machine-readable `code`, a `hint`, and the process exit code
/// (0 ok, 1 error, 2 usage, 3 unknown format, 4 corrupt, 5 I/O, 6 unsupported feature).
#[derive(Debug, Parser)]
#[command(name = "openreadout", version, about, long_about = None, propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[command(flatten)]
    global: GlobalArgs,
}

/// Flags accepted by every command.
#[derive(Debug, clap::Args)]
#[command(next_help_heading = "Global options")]
struct GlobalArgs {
    /// Worker threads for plane decoding (export, check --planes, stats). Default: the number of CPUs.
    /// Output is identical whatever the count.
    #[arg(long, global = true, value_name = "N")]
    threads: Option<usize>,
    /// Colour in human-readable output: `auto` (terminals only; honours NO_COLOR and
    /// CLICOLOR_FORCE), `always` or `never`. JSON is never coloured.
    #[arg(long, global = true, value_enum, default_value_t, value_name = "WHEN")]
    color: ui::ColorWhen,
    /// Show progress on stderr even when it is not a terminal (as plain lines). Default: only
    /// on a terminal.
    #[arg(long, global = true, overrides_with = "no_progress")]
    progress: bool,
    /// Never show progress.
    #[arg(long, global = true, overrides_with = "progress")]
    no_progress: bool,
    /// No human-readable output on success, no progress, no batch summary. Errors still go to
    /// stderr and `--json`/`--jsonl` output is unchanged.
    #[arg(short, long, global = true)]
    quiet: bool,
    /// Live window in seconds: an incomplete file modified less than this long ago is
    /// reported as `acquisition.state: in_progress` (still being written) instead of
    /// interrupted. Default 300, or OPENREADOUT_LIVE_WINDOW; 0 turns it off. See https://openreadout.github.io/openreadout/guides/lab-shares.html.
    #[arg(long, global = true, value_name = "SECONDS")]
    live_window: Option<f64>,
    /// JSON output: keep only these values of `data`, as JSON pointers (`/images/0/physical_size`;
    /// `*` maps over every element: `/images/*/name`). Comma-separated or repeated; implies JSON.
    /// `data` becomes `{pointer: value}`; pointers that match nothing are listed in `missing`.
    #[arg(long, global = true, value_name = "POINTERS")]
    only: Vec<String>,
    /// JSON output on one line instead of indented (about half the bytes; same content).
    #[arg(long, global = true)]
    compact: bool,
    /// Refuse (exit 6, with a hint) to return values this file's assurance does not validate:
    /// outputs that depend on a variant (format version, writer, codec, layout) no development
    /// file confirmed against an independent reader, on a structure left undecoded, or on a
    /// vendor calibration not applied. `info --json` → `assurance.strict_refuses` lists them;
    /// `check` and `info --view structure` still run. Also OPENREADOUT_STRICT=1. See https://openreadout.github.io/openreadout/reference/assurance.html.
    #[arg(long, global = true)]
    strict: bool,
    /// Panic on purpose (tests the panic handler).
    #[arg(long, global = true, hide = true)]
    self_panic: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// What a file holds: format, images, tables, traces, spectra, plate, experiment. Reads headers only.
    ///
    /// `--view full` adds per-frame records, per-field provenance and the vendor's metadata tree;
    /// `structure` lists the container (images, blocks, attachments, pyramid levels); `explain`
    /// (or `--ask`) explains the file in plain words; `format` only identifies the format from
    /// its signature. With `--tidy` (or `--fields`, `--sample-sheet`, `-o`): one metadata row
    /// per data set.
    Info(commands::info::InfoArgs),
    /// Validate a file's integrity (exit 4 if corrupt or truncated); hash its planes, compare it
    /// with a second file, or write a diagnostic bundle for a new-variant issue.
    Check(commands::check::CheckArgs),
    /// Render a PNG/JPEG preview: an image plane, channel composite or max projection; a trace
    /// sparkline (sweeps, chromatograms, NMR spectra); a mass spectrum; or a plate heat map.
    Preview(preview_cmd::PreviewArgs),
    /// Pixel statistics per plane, channel and image (min, max, mean, std, percentiles,
    /// histogram, zero and saturated fractions), or per well of a plate (`--per well`).
    ///
    /// With `--tidy` (or `--sample-sheet`, `--by`, `-o`): one table over many files.
    Stats(commands::stats::StatsArgs),
    /// Samples and statistics of one sweep of a sampled signal: electrophysiology, chromatogram
    /// detector traces, NMR FIDs and spectra, IR/Raman/UV-Vis spectra, qPCR curves and other
    /// 1-D data (see `info` → `traces[]`).
    ///
    /// Several files, a directory or `--tidy` give one table of per-sweep, per-channel statistics
    /// (every trace and sweep unless --trace/--sweep narrow it).
    Trace(commands::trace::TraceArgs),
    /// Rows of a table (FCS events, plate-reader values, spike, event or peak tables). FCS:
    /// optionally compensated, transformed and with population membership from a FlowJo
    /// workspace or Gating-ML file.
    Table(commands::flow::TableArgs),
    /// Mass spectra: the scan headers of a run (filters, counts, paging, CSV), or one
    /// spectrum's m/z and intensity arrays with `--scan`, `--index` or `--ms-level L --nth K`.
    Spectra(commands::spectra::SpectraArgs),
    /// Analyses with documented methods: chromatographic peaks, chromatograms, NMR peaks,
    /// patch-clamp features, extracellular spikes, qPCR, plate assays, flow-cytometry gating.
    #[command(subcommand)]
    Analyze(AnalyzeKind),
    /// Export to an open format (OME-TIFF, OME-Zarr, CSV, Parquet, Arrow, mzML, NWB,
    /// JCAMP-DX, Allotrope ASM, RDML), or write one embedded attachment (`--attachment`).
    ///
    /// Images go to OME-TIFF or OME-Zarr; tables (FCS events, spike and event tables, plate
    /// reads, peak tables) and traces (electrophysiology sweeps, NMR FIDs and spectra, JCAMP-DX
    /// spectra, chromatograms) to CSV, Parquet or Arrow IPC; mass spectra to indexed mzML,
    /// Parquet or Arrow IPC; electrophysiology traces to NWB; NMR and 1-D spectra to JCAMP-DX.
    /// Never modifies the source; the output is read back and verified before it is renamed
    /// into place.
    Export(commands::export::ExportArgs),
    /// Any measure over many files as one tidy table, with sample sheets (`--sample-sheet`) and
    /// group summaries (`--by`); `batch summarize TABLE` summarizes a saved table.
    ///
    /// Measures: stats, trace, table, info, spectra (one row per MS scan header), or an analysis
    /// — peaks, chromatogram, assay, nmr-peaks, ephys-features, spikes, qpcr, gate — configured
    /// with the MCP options' names (`--set mz=[195.0877] --set ppm=10`). Rows are exactly the
    /// single-file command's records (one per peak, compound, well, sweep, …).
    Batch(commands::measure::MeasureArgs),
    /// Group files that measured the same sample across instruments and formats (sample ids,
    /// barcodes, plate wells, conversions naming their source, the same acquisition), with the
    /// evidence and a confidence for every link. Headers only.
    Link(commands::summarize::LinkArgs),
    /// Catalog every instrument data set under one or more directories into an index of open
    /// files (Parquet tables + index.json): headers only, parallel, resumable, incremental.
    Index(commands::index::IndexArgs),
    /// Search an index (`search INDEX_DIR "objective=63x channel~GFP acquired<2020"`), report
    /// its storage health (`--health`), or export what a query selects as a dataset (`--export`).
    Search(commands::index::SearchArgs),
    /// Watch directories for instrument data being written: one JSON line per new data set,
    /// plane, scan or sweep, per completed or stalled data set, per QC finding (`--qc`) and
    /// per error. Polls, opens files read-only and never locks them.
    Watch(commands::watch::WatchArgs),
    /// Run as an MCP (Model Context Protocol) server (stdio, or Streamable HTTP with `--http`),
    /// print a client config snippet (`--config`), or write it into a client's config (`--install`).
    Mcp(mcp_config::McpArgs),
    /// This installation: supported formats, self-test, JSON Schemas, agent skill, shell
    /// completions, man pages.
    #[command(subcommand, name = "self")]
    SelfCmd(commands::self_cmd::SelfCommand),
}

/// `openreadout analyze <kind>`.
#[derive(Debug, Subcommand)]
pub(crate) enum AnalyzeKind {
    /// Detect and integrate chromatographic peaks (detector trace, TIC, XIC, SRM): retention
    /// time, area, height, widths, tailing, plates, resolution, S/N, area % (purity); spectral
    /// bands and regions on IR/Raman/UV-Vis/NMR axes.
    ///
    /// `--rt` reports the peak near a retention time, `--targets` a compound list (one row per
    /// compound per file), `--plot` draws it.
    Peaks(commands::peaks::PeaksArgs),
    /// Chromatograms from mass spectra and detector signals: TIC, BPC, extracted-ion (XIC) at
    /// one or many m/z, SRM/MRM transitions, stored chromatograms and UV/DAD/FID/TCD traces.
    ///
    /// Filters: MS level, polarity, scan filter, precursor, RT range. Streams the spectra once
    /// (in parallel); JSON, or `-o` CSV/Parquet/Arrow/PNG.
    Chromatogram(commands::chromatogram::ChromatogramArgs),
    /// NMR peak list and integrals, on the vendor's processed spectrum or a spectrum processed
    /// from the FID; `--integrate A:B` regions.
    NmrPeaks(commands::nmr::NmrPeaksArgs),
    /// Patch-clamp features: action potentials per sweep, rheobase and f–I curve, passive
    /// properties; voltage clamp: holding current, access/membrane resistance, capacitance.
    EphysFeatures(commands::ephys::EphysArgs),
    /// Extracellular spike detection: band-pass, threshold at K × MAD noise, spike counts, rates
    /// and times per channel.
    Spikes(commands::ephys::SpikesArgs),
    /// Real-time PCR results with names: one record per well × target (sample, target, task, Cq,
    /// Tm); `--cq` recomputes Cq, `--ddcq` computes 2^-ΔΔCq, `--standard-curve` fits efficiency.
    Qpcr(commands::qpcr::QpcrArgs),
    /// Plate-reader assays: layouts, blank subtraction and replicate statistics, standard curves
    /// (linear/4PL/5PL), dose-response IC50/EC50, kinetics, growth curves, Z′.
    Assay(commands::assay::AssayArgs),
    /// Flow-cytometry gating from a FlowJo workspace (`.wsp`) or Gating-ML 2.0 file: the gate
    /// hierarchy and, given FCS files, the event count of every population.
    Gate(commands::flow::GateArgs),
}

fn main() -> ExitCode {
    panic::install();
    let cli = Cli::parse();
    let g = &cli.global;
    panic::self_test(g.self_panic);
    let progress = if g.progress {
        Some(true)
    } else if g.no_progress {
        Some(false)
    } else {
        None
    };
    ui::init(g.color, g.quiet, progress);
    output::set_only(openreadout_ops::project::parse_list(&g.only));
    output::set_compact(g.compact);
    if let Some(n) = g.threads {
        if n == 0 {
            output::print_error(
                None,
                &openreadout_core::Error::Usage("--threads must be at least 1".into()),
            );
            return ExitCode::from(2);
        }
        // Fails only if a pool already exists, which cannot happen this early.
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global();
    }
    if let Some(w) = g.live_window {
        if !(w.is_finite() && w >= 0.0) {
            output::print_error(
                None,
                &openreadout_core::Error::Usage(
                    "--live-window must be a number of seconds >= 0".into(),
                ),
            );
            return ExitCode::from(2);
        }
        openreadout_core::live::set_window(std::time::Duration::from_secs_f64(w));
    }
    if g.strict {
        openreadout_core::assurance::set_default_strict(true);
    }
    let code = commands::run(cli.command);
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
