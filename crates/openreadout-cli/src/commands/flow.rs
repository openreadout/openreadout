//! `analyze gate` (FlowJo workspace / Gating-ML populations and their event counts) and `table` (rows of
//! a table, for FCS optionally compensated, transformed and with population columns).

use std::fmt::Write as _;
use std::path::PathBuf;

use openreadout_core::flow::{GateOutput, PopulationNode};
use openreadout_core::model::TableSlice;
use openreadout_core::{Error, Registry, Result};
use openreadout_fcs::analysis::{
    CompensationChoice, GateRequest, TableOptions, TransformChoice, gate, parse_transform_spec,
    table_slice,
};

use crate::output::{emit, fail};

/// Most rows `table` prints in one call.
pub const MAX_TABLE_ROWS: u64 = 1_000_000;

/// Arguments of `gate`.
#[derive(Debug, clap::Args)]
pub struct GateArgs {
    /// FCS file to count events in. Without it, the gating file is described (samples,
    /// compensation, transforms, the gate tree) without counts. Several files, a directory or
    /// a glob give one table: a row per file and population.
    #[arg(value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: super::batch::BatchArgs,
    #[command(flatten)]
    pub tidy: super::tidy::TidyArgs,
    /// Report each population's median of this parameter (`$PnN` or `$PnS`; `Comp-NAME` for
    /// the compensated values). Repeatable or comma-separated.
    #[arg(long = "median", value_name = "PARAMETER", value_delimiter = ',')]
    pub medians: Vec<String>,
    /// FlowJo 10 workspace (`.wsp`).
    #[arg(
        long,
        value_name = "WSP",
        conflicts_with = "gatingml",
        required_unless_present = "gatingml"
    )]
    pub workspace: Option<PathBuf>,
    /// Gating-ML 2.0 document.
    #[arg(long, value_name = "XML")]
    pub gatingml: Option<PathBuf>,
    /// Workspace sample (name or id). Default: the sample whose name or file matches FILE, then
    /// its `$FIL`, then the only sample.
    #[arg(long, value_name = "NAME")]
    pub sample: Option<String>,
    /// Report only this population (path like `/Lymphocytes/Singlets` or a unique name) and its
    /// descendants. Repeatable.
    #[arg(long = "population", value_name = "PATH")]
    pub populations: Vec<String>,
    /// FCS data set index (files with several data sets).
    #[arg(long, default_value_t = 0)]
    pub table: u32,
    #[arg(long)]
    pub json: bool,
}

/// Which compensation `table --compensate` applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum CompensateArg {
    /// The gating file's matrix when `--workspace`/`--gatingml` names one, else the FCS file's.
    Auto,
    /// The FCS file's `$SPILLOVER` / `$SPILL` / `SPILL`.
    Fcs,
    /// The gating file's matrix (the workspace sample's, or the only Gating-ML matrix).
    Gating,
}

/// Arguments of `table`.
#[derive(Debug, Clone, clap::Args)]
pub struct TableArgs {
    /// The file (FCS, plate-reader export, spike or peak table; see `info` → `tables[]`).
    /// Several files, a directory or a glob give one summary table: per FCS file and
    /// parameter (events, median, mean, sd, min, max), per plate well and read.
    #[arg(required_unless_present = "from_index", value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: super::batch::BatchArgs,
    #[command(flatten)]
    pub tidy: super::tidy::TidyArgs,
    /// Table index (FCS data set, plate read, …). Default 0 (batch table: every table).
    #[arg(long)]
    pub table: Option<u32>,
    /// Zero-based first row.
    #[arg(long, default_value_t = 0)]
    pub first_row: u64,
    /// Rows to return (at most 1000000).
    #[arg(long, default_value_t = 100)]
    pub max_rows: u64,
    /// FCS: compensate first (values become scale values, then compensated). Without a value:
    /// `auto`.
    #[arg(long, value_enum, num_args = 0..=1, default_missing_value = "auto", value_name = "FROM")]
    pub compensate: Option<CompensateArg>,
    /// FCS: transform after compensation: `logicle`, `arcsinh`, `hyperlog`, `log`, `linear`,
    /// `biex`, `flowjo-log`, `arcsinh-cofactor` (parameters as `NAME:K=V,…`, e.g.
    /// `logicle:T=262144,W=0.5,M=4.5,A=0`, `arcsinh-cofactor:5`), or `workspace` for the
    /// FlowJo sample's own per-parameter transforms.
    #[arg(long, value_name = "SPEC")]
    pub transform: Option<String>,
    /// Parameters (`$PnN`) to transform. Default: every fluorescence parameter (not FSC, SSC,
    /// Time). Repeatable or comma-separated.
    #[arg(long = "parameter", value_name = "NAME", value_delimiter = ',')]
    pub parameters: Vec<String>,
    /// FCS: FlowJo workspace for `--compensate gating`, `--transform workspace` and `--population`.
    #[arg(long, value_name = "WSP", conflicts_with = "gatingml")]
    pub workspace: Option<PathBuf>,
    /// FCS: Gating-ML 2.0 document (same uses).
    #[arg(long, value_name = "XML")]
    pub gatingml: Option<PathBuf>,
    /// Workspace sample (name or id).
    #[arg(long, value_name = "NAME")]
    pub sample: Option<String>,
    /// Append a 0/1 column `gate:<path>` with this population's membership. Repeatable.
    #[arg(long = "population", value_name = "PATH")]
    pub populations: Vec<String>,
    /// Only rows meeting CONDITION: `COLUMN OP NUMBER` with OP one of > >= < <= == != and
    /// COLUMN a name ($PnN), label ($PnS) or `gate:<path>` column, e.g. `--filter 'FITC-A >
    /// 1000'`. Repeatable or joined with `&&` (every one must hold). Tested on the values
    /// returned (after --compensate/--transform/--population); `filter.matched_rows` counts
    /// every match and --first-row/--max-rows page through them.
    #[arg(long = "filter", value_name = "CONDITION")]
    pub row_filters: Vec<String>,
    /// Count only: no rows, just `filter.matched_rows` of `total_rows` (and `percent`).
    #[arg(long)]
    pub count: bool,
    #[arg(long)]
    pub json: bool,
}

pub fn run_gate(reg: &Registry, a: &GateArgs) -> i32 {
    let gating = a.workspace.clone().or_else(|| a.gatingml.clone());
    if !a.files.is_empty() && super::tidy::wanted(reg, &a.tidy, &a.batch, &a.files, true) {
        let Some(g) = gating else {
            return fail(
                a.json,
                &Error::Usage("pass --workspace W.wsp or --gatingml G.xml".into()),
            );
        };
        let m = openreadout_batch::measures::GateMeasure {
            gating_file: g.clone(),
            sample: a.sample.clone(),
            populations: a.populations.clone(),
            medians: a.medians.clone(),
            table: a.table,
        };
        // the gating file often sits next to the FCS files: never an input
        let mut files = a.files.clone();
        files.retain(|f| f != &g);
        return super::tidy::run(reg, &m, &files, &a.batch, &a.tidy, a.json);
    }
    let result = (|| -> Result<GateOutput> {
        let gating = gating
            .ok_or_else(|| Error::Usage("pass --workspace W.wsp or --gatingml G.xml".into()))?;
        let mut req = GateRequest::new(gating, a.files.first().cloned());
        req.sample = a.sample.clone();
        req.table = a.table;
        req.populations = a.populations.clone();
        req.medians = a.medians.clone();
        gate(&req)
    })();
    match result {
        Ok(v) => emit(a.json, &v, render_gate),
        Err(e) => fail(a.json, &e),
    }
}

fn render_node(out: &mut String, n: &PopulationNode, depth: usize, rows: &GateOutput) {
    let row = rows.populations.iter().find(|r| r.path == n.path);
    let count = n.count.map(|c| c.to_string()).unwrap_or_default();
    let pct = row
        .and_then(|r| r.percent_of_parent)
        .map(|p| format!("  {p:.2}% of parent"))
        .unwrap_or_default();
    let stored = row
        .and_then(|r| {
            r.stored_count
                .filter(|s| Some(*s) != r.count)
                .map(|s| format!("  (stored {s})"))
        })
        .unwrap_or_default();
    let kind = if n.gate_type.is_empty() {
        String::new()
    } else {
        format!(" [{}]", n.gate_type)
    };
    let _ = writeln!(
        out,
        "{}{}{kind}  {count}{pct}{stored}",
        "  ".repeat(depth),
        n.name
    );
    for c in &n.children {
        render_node(out, c, depth + 1, rows);
    }
}

fn render_gate(g: &GateOutput) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{} ({})", g.gating_file, g.gating_format);
    if let Some(w) = &g.workspace {
        let _ = writeln!(
            s,
            "FlowJo {} workspace {}: {} sample(s), {} group(s)",
            w.flowjo_version.as_deref().unwrap_or("?"),
            w.version.as_deref().unwrap_or("?"),
            w.samples.len(),
            w.groups.len()
        );
    }
    if let Some(sm) = &g.sample {
        let _ = writeln!(s, "sample: {} (id {})", sm.name, sm.id);
    }
    if let Some(p) = &g.path {
        let _ = writeln!(s, "events: {} in {p}", g.event_count.unwrap_or(0));
    }
    for c in &g.compensation {
        let _ = writeln!(
            s,
            "compensation {} ({}, {} detectors{}{})",
            c.name,
            c.source,
            c.detectors.len(),
            if c.spectral { ", spectral" } else { "" },
            if c.used { ", used" } else { "" }
        );
    }
    for n in &g.tree {
        render_node(&mut s, n, 0, g);
    }
    for n in &g.notes {
        let _ = writeln!(s, "note: {n}");
    }
    s.trim_end().to_string()
}

fn table_options(a: &TableArgs) -> Result<TableOptions> {
    let mut o = TableOptions::default();
    o.gating_file = a.workspace.clone().or_else(|| a.gatingml.clone());
    o.sample = a.sample.clone();
    o.populations = a.populations.clone();
    o.compensation = a.compensate.map(|c| match c {
        CompensateArg::Auto => CompensationChoice::Auto,
        CompensateArg::Fcs => CompensationChoice::File,
        CompensateArg::Gating => CompensationChoice::GatingFile,
    });
    o.transform = match a.transform.as_deref() {
        None => {
            if !a.parameters.is_empty() {
                return Err(Error::Usage(
                    "--parameter selects the columns --transform applies to; pass --transform"
                        .into(),
                ));
            }
            None
        }
        Some("workspace" | "gating") => Some(TransformChoice::GatingFile),
        Some(spec) => Some(TransformChoice::Uniform {
            transform: parse_transform_spec(spec)?,
            parameters: a.parameters.clone(),
        }),
    };
    Ok(o)
}

pub fn run_table(reg: &Registry, a: &TableArgs) -> i32 {
    // `--where` filters batch-table rows (COLUMN=VALUE); a comparison belongs in --filter
    if a.row_filters.is_empty()
        && !a.count
        && a.files.len() <= 1
        && let Some(w) = a.tidy.filters.iter().find(|w| w.contains(['<', '>']))
    {
        return fail(
            a.json,
            &Error::Usage(format!(
                "--where filters batch-table rows (COLUMN=VALUE); to test events use --filter '{w}' (add --count to count them)"
            )),
        );
    }
    let filtering = !a.row_filters.is_empty() || a.count;
    if filtering && super::tidy::wanted(reg, &a.tidy, &a.batch, &a.files, true) {
        return fail(
            a.json,
            &Error::Usage(
                "--filter/--count read one file; for many files use `table FILES --tidy` (per-parameter statistics) or `analyze gate FILES --workspace W.wsp` (population counts)".into(),
            ),
        );
    }
    if filtering {
        let Some(file) = a.files.first() else {
            return fail(a.json, &Error::Usage("no input given".into()));
        };
        let result = table_options(a).and_then(|o| {
            let conds = openreadout_fcs::filter::parse_conditions(&a.row_filters)?;
            openreadout_fcs::filter::filtered_slice(
                reg,
                file,
                a.table.unwrap_or(0),
                a.first_row,
                if a.count {
                    0
                } else {
                    a.max_rows.min(MAX_TABLE_ROWS)
                },
                &o,
                &conds,
            )
        });
        return match result {
            Ok(v) => emit(a.json, &v, render_table),
            Err(e) => fail(a.json, &e),
        };
    }
    if super::tidy::wanted(reg, &a.tidy, &a.batch, &a.files, true) {
        // --parameter narrows the summary here; it names transform targets only with --transform
        let mut for_options = a.clone();
        if for_options.transform.is_none() {
            for_options.parameters.clear();
        }
        let o = match table_options(&for_options) {
            Ok(o) if !o.populations.is_empty() => {
                return fail(
                    a.json,
                    &Error::Usage(
                        "--population in a batch table: use `analyze gate FILES --workspace W.wsp` for population counts and medians".into(),
                    ),
                );
            }
            Ok(o) => o,
            Err(e) => return fail(a.json, &e),
        };
        let m = openreadout_batch::measures::TableMeasure {
            table: a.table,
            parameters: a.parameters.clone(),
            options_text: format!(
                "{:?}{:?}{:?}{:?}",
                a.compensate, a.transform, a.workspace, a.gatingml
            ),
            fcs: o,
        };
        let mut files = a.files.clone();
        files.retain(|f| Some(f) != a.workspace.as_ref() && Some(f) != a.gatingml.as_ref());
        return super::tidy::run(reg, &m, &files, &a.batch, &a.tidy, a.json);
    }
    let Some(file) = a.files.first() else {
        return fail(a.json, &Error::Usage("no input given".into()));
    };
    let result = table_options(a).and_then(|o| {
        table_slice(
            reg,
            file,
            a.table.unwrap_or(0),
            a.first_row,
            a.max_rows.min(MAX_TABLE_ROWS),
            &o,
        )
    });
    match result {
        Ok(v) => emit(a.json, &v, render_table),
        Err(e) => fail(a.json, &e),
    }
}

fn render_table(t: &TableSlice) -> String {
    let mut s = String::new();
    if let Some(f) = &t.filter {
        let _ = writeln!(
            s,
            "# {} of {} rows ({:.4}%) meet {}",
            f.matched_rows,
            f.total_rows,
            f.percent,
            if f.conditions.is_empty() {
                "no condition".to_string()
            } else {
                f.conditions.join(" && ")
            }
        );
        if t.rows.is_empty() {
            return s.trim_end().to_string();
        }
    }
    let _ = writeln!(s, "{}", t.columns.join("\t"));
    for r in &t.rows {
        let cells: Vec<String> = r.iter().map(|v| format!("{v}")).collect();
        let _ = writeln!(s, "{}", cells.join("\t"));
    }
    let shown = t.rows.len() as u64;
    let _ = write!(
        s,
        "# {}rows {}..{} of {} (table {}){}",
        if t.filter.is_some() { "matching " } else { "" },
        t.first_row,
        t.first_row + shown,
        t.filter.as_ref().map_or(t.total_rows, |f| f.matched_rows),
        t.table,
        if t.processing.is_some() {
            "; processed, see --json → processing"
        } else {
            ""
        }
    );
    s
}
