//! `self`: supported formats, self-test, JSON Schemas, the agent skill, shell completions
//! and man pages.

use std::path::{Path, PathBuf};

use clap::{CommandFactory, Subcommand, ValueEnum};
use openreadout_core::model::{
    CheckReport, DetectOutput, Dump, ExtractOutput, FormatsOutput, Listing, PlanesOutput,
    SpectrumOutput,
};
use openreadout_core::{InfoOutput, Registry};
use openreadout_ops::explain::Explanation;

use super::{doctor, export::ExportOutput, sidecar, summarize, wrap};

/// The canonical skill text, embedded so `openreadout self skill` works offline.
pub const SKILL_MD: &str = include_str!("../../SKILL.md");

/// The skill's reference files (`references/<name>`), which SKILL.md links to.
pub const SKILL_REFERENCES: &[(&str, &str)] =
    include!(concat!(env!("OUT_DIR"), "/skill_references.rs"));

/// `openreadout self <command>`.
#[derive(Debug, Subcommand)]
pub enum SelfCommand {
    /// List supported formats, read/write status, confidence and known gaps.
    Formats {
        #[arg(long)]
        json: bool,
    },
    /// Check this installation: version, build, formats, MCP, and a self-test on built-in
    /// synthetic files through the readers and the analyses (no corpus needed).
    Doctor(doctor::DoctorArgs),
    /// Print the JSON Schema of a command's `data` payload.
    Schema {
        #[arg(value_enum)]
        of: SchemaOf,
    },
    /// Print the agent skill (SKILL.md), or install it into agent skill directories.
    Skill {
        /// Install to: `claude` (~/.claude/skills), `agents` (~/.agents/skills, read by Codex, Cursor, Copilot, Gemini), or `all`.
        #[arg(long, value_enum)]
        install: Option<SkillTarget>,
    },
    /// Print a shell completion script (bash, zsh, fish, powershell, elvish) to stdout.
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Write man pages (`openreadout.1` and one per command) into a directory.
    Man {
        /// Output directory (created if missing).
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SchemaOf {
    /// `info`.
    Info,
    /// `info --view full`.
    InfoFull,
    /// `info --view structure`.
    InfoStructure,
    /// `info --view explain` (and `--ask`).
    InfoExplain,
    /// `info --view format`.
    InfoFormat,
    /// `info --view full --sidecar`: the report (one per input).
    Sidecar,
    /// The content of a `<file>.openreadout.json` sidecar.
    SidecarFile,
    /// `check`.
    Check,
    /// `planes`.
    Planes,
    /// `compare`.
    Compare,
    /// `report`: the diagnostic bundle for a new-variant issue (`--json` prints it with
    /// the output path).
    Report,
    /// `export`.
    Export,
    /// `extract`.
    Extract,
    Preview,
    Stats,
    /// `stats --per well|field`: per-well rows of a multi-well plate.
    StatsWells,
    Trace,
    /// `table`: rows of a table, with the processing record for FCS.
    Table,
    /// `scans`: scan headers of a mass-spectrometry run.
    Scans,
    /// `spectrum`: one mass spectrum.
    Spectrum,
    /// `analyze peaks`: peak tables, targeted peaks and compound rows.
    Peaks,
    /// `analyze chromatogram`: TIC/BPC/XIC/SRM chromatograms with summaries.
    Chromatogram,
    /// `analyze nmr-peaks`: peak list, integrals and the processing record of an NMR spectrum.
    NmrPeaks,
    /// `analyze ephys-features`: per-cell, per-sweep and per-spike patch-clamp features.
    EphysFeatures,
    /// `analyze spikes`: extracellular spike counts, rates and times per channel.
    Spikes,
    /// `analyze qpcr`: named well × target records and qPCR analyses.
    Qpcr,
    /// `analyze assay-wells`, `assay-curve`, `dose-response`, `kinetics`, `growth`, `assay-qc`:
    /// plate-reader analyses.
    Assay,
    /// `analyze gate`: populations of a FlowJo workspace or Gating-ML file with event counts.
    Gate,
    /// `batch`, and the table mode of info, stats, trace, table and analyze gate (`--tidy --json`).
    BatchTable,
    /// `summarize`.
    Summarize,
    /// `link`.
    Link,
    /// `index.json` (also `index --json`); lists the columns of every table.
    Index,
    Search,
    /// `health`.
    Health,
    /// `export-dataset`.
    ExportDataset,
    /// One `watch` event (the `data` of each line).
    Watch,
    /// `self formats`.
    Formats,
    /// `self doctor`.
    Doctor,
    /// The JSON envelope every command prints with `--json`.
    Envelope,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SkillTarget {
    Claude,
    Agents,
    All,
}

pub fn run(reg: &Registry, c: SelfCommand) -> i32 {
    match c {
        SelfCommand::Formats { json } => wrap(
            json,
            || {
                Ok(FormatsOutput {
                    formats: reg.descriptors(),
                })
            },
            render_formats,
        ),
        SelfCommand::Doctor(a) => doctor::run(reg, &a),
        SelfCommand::Schema { of } => {
            println!("{}", schema(of));
            0
        }
        SelfCommand::Skill { install } => skill(install),
        SelfCommand::Completions { shell } => completions(shell),
        SelfCommand::Man { out } => man(&out),
    }
}

fn schema(of: SchemaOf) -> String {
    let s = match of {
        SchemaOf::Info => schemars::schema_for!(InfoOutput),
        SchemaOf::InfoFull => schemars::schema_for!(Dump),
        SchemaOf::InfoStructure => schemars::schema_for!(Listing),
        SchemaOf::InfoExplain => schemars::schema_for!(Explanation),
        SchemaOf::InfoFormat => schemars::schema_for!(DetectOutput),
        SchemaOf::Sidecar => schemars::schema_for!(sidecar::SidecarReport),
        SchemaOf::SidecarFile => schemars::schema_for!(sidecar::SidecarFile),
        SchemaOf::Check => schemars::schema_for!(CheckReport),
        SchemaOf::Planes => schemars::schema_for!(PlanesOutput),
        SchemaOf::Compare => schemars::schema_for!(openreadout_ops::compare::CompareOutput),
        SchemaOf::Report => schemars::schema_for!(openreadout_index::report::ReportOutput),
        SchemaOf::Export => schemars::schema_for!(ExportOutput),
        SchemaOf::Extract => schemars::schema_for!(ExtractOutput),
        SchemaOf::Preview => schemars::schema_for!(openreadout_preview::PreviewOutput),
        SchemaOf::Stats => schemars::schema_for!(openreadout_core::stats::StatsOutput),
        SchemaOf::StatsWells => schemars::schema_for!(openreadout_core::plate::WellStatsOutput),
        SchemaOf::Trace => schemars::schema_for!(openreadout_core::trace::TraceSlice),
        SchemaOf::Table => schemars::schema_for!(openreadout_core::model::TableSlice),
        SchemaOf::Scans => schemars::schema_for!(openreadout_core::ScanList),
        SchemaOf::Spectrum => schemars::schema_for!(SpectrumOutput),
        SchemaOf::Peaks => schemars::schema_for!(openreadout_quant::analyze::PeaksOutput),
        SchemaOf::Chromatogram => {
            schemars::schema_for!(openreadout_quant::extract::ChromatogramOutput)
        }
        SchemaOf::NmrPeaks => schemars::schema_for!(openreadout_signal::nmr::NmrReport),
        SchemaOf::EphysFeatures => schemars::schema_for!(openreadout_signal::ephys::CellReport),
        SchemaOf::Spikes => schemars::schema_for!(openreadout_signal::ephys::SpikesReport),
        SchemaOf::Qpcr => schemars::schema_for!(openreadout_qpcr::QpcrReport),
        SchemaOf::Assay => schemars::schema_for!(openreadout_assay::AssayOutput),
        SchemaOf::Gate => schemars::schema_for!(openreadout_core::flow::GateOutput),
        SchemaOf::BatchTable => schemars::schema_for!(openreadout_batch::BatchOutput),
        SchemaOf::Summarize => schemars::schema_for!(summarize::SummarizeOutput),
        SchemaOf::Link => schemars::schema_for!(openreadout_batch::LinkOutput),
        SchemaOf::Index => schemars::schema_for!(openreadout_index::IndexManifest),
        SchemaOf::Search => schemars::schema_for!(openreadout_index::SearchOutput),
        SchemaOf::Health => schemars::schema_for!(openreadout_index::HealthReport),
        SchemaOf::ExportDataset => {
            schemars::schema_for!(openreadout_index::ExportDatasetReport)
        }
        SchemaOf::Watch => schemars::schema_for!(openreadout_live::watch::WatchEvent),
        SchemaOf::Formats => schemars::schema_for!(FormatsOutput),
        SchemaOf::Doctor => schemars::schema_for!(doctor::DoctorReport),
        SchemaOf::Envelope => {
            schemars::schema_for!(openreadout_core::Envelope<serde_json::Value>)
        }
    };
    serde_json::to_string_pretty(&s).expect("schema serializes")
}

fn skill(install: Option<SkillTarget>) -> i32 {
    let Some(target) = install else {
        print!("{SKILL_MD}");
        return 0;
    };
    let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) else {
        eprintln!("error: cannot determine home directory");
        return 1;
    };
    let home = PathBuf::from(home);
    let mut dirs = Vec::new();
    if matches!(target, SkillTarget::Claude | SkillTarget::All) {
        dirs.push(home.join(".claude").join("skills").join("openreadout"));
    }
    if matches!(target, SkillTarget::Agents | SkillTarget::All) {
        dirs.push(home.join(".agents").join("skills").join("openreadout"));
    }
    for d in dirs {
        let refs = d.join("references");
        let res = std::fs::create_dir_all(&refs)
            .and_then(|()| std::fs::write(d.join("SKILL.md"), SKILL_MD))
            .and_then(|()| {
                SKILL_REFERENCES
                    .iter()
                    .try_for_each(|(name, text)| std::fs::write(refs.join(name), text))
            });
        if let Err(e) = res {
            eprintln!("error: could not write {}: {e}", d.display());
            return 5;
        }
        println!("installed {}", d.join("SKILL.md").display());
    }
    0
}

fn render_formats(f: &FormatsOutput) -> String {
    let mut s = String::from("id     name              read  write  confidence  extensions\n");
    for d in &f.formats {
        s.push_str(&format!(
            "{:<6} {:<17} {:<5} {:<6} {:<11} {}\n",
            d.id,
            d.name,
            if d.can_read { "yes" } else { "no" },
            if d.can_write { "yes" } else { "no" },
            format!("{:?}", d.confidence).to_lowercase(),
            d.extensions.join(", ")
        ));
        for g in &d.known_gaps {
            s.push_str(&format!("         gap: {g}\n"));
        }
    }
    s.trim_end().to_string()
}

/// Print the completion script for `shell` on stdout.
pub fn completions(shell: clap_complete::Shell) -> i32 {
    let mut cmd = crate::Cli::command();
    clap_complete::generate(shell, &mut cmd, "openreadout", &mut std::io::stdout());
    0
}

/// Write `openreadout.1` and `openreadout-<command>.1` for every command into `out`.
pub fn man(out: &Path) -> i32 {
    let res = std::fs::create_dir_all(out)
        .and_then(|()| clap_mangen::generate_to(crate::Cli::command(), out));
    match res {
        Ok(()) => {
            if !crate::ui::quiet() {
                let mut pages: Vec<String> = std::fs::read_dir(out)
                    .map(|rd| {
                        rd.filter_map(std::result::Result::ok)
                            .map(|e| e.file_name().to_string_lossy().to_string())
                            .filter(|n| n.starts_with("openreadout") && n.ends_with(".1"))
                            .collect()
                    })
                    .unwrap_or_default();
                pages.sort();
                println!("wrote {} man pages to {}", pages.len(), out.display());
            }
            0
        }
        Err(e) => {
            crate::output::print_error(None, &openreadout_core::Error::io(out, e));
            5
        }
    }
}
