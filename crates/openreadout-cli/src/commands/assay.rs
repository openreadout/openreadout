//! `analyze assay-wells`, `assay-curve`, `dose-response`, `kinetics`, `growth`, `assay-qc`:
//! plate-reader analysis after reading a plate — layouts, blank subtraction and
//! replicates, standard curves, dose-response, kinetics, growth curves and assay quality
//! (crate `openreadout-assay`, book/src/guides/plate-analysis.md).

use std::path::PathBuf;

use openreadout_assay::fit::{Model, Weighting};
use openreadout_assay::{
    Analysis, AssayOutput, AssayRequest, BlankMode, FitOn, Normalize, OutlierRule, Reduce,
};
use openreadout_core::{Error, Registry, Result};

use crate::output::{emit, fail};

/// The plate-reader analyses, each its own `analyze` subcommand.
#[derive(Debug, clap::Subcommand)]
pub enum AssayCommand {
    /// Per-well values with roles, blank subtraction, and per-replicate-group mean, SD, CV % and
    /// outlier flags (plus Z′ when the layout has positive and negative controls).
    #[command(name = "assay-wells")]
    Wells(WellsArgs),
    /// Fit a standard curve (linear, 4PL, 5PL) to the standard wells and back-calculate the
    /// concentration of every well, with range flags, LLOQ/ULOQ, R² and residuals.
    #[command(name = "assay-curve")]
    Curve(CurveArgs),
    /// Fit a dose-response curve (4PL/5PL) per compound: IC50/EC50 with confidence interval,
    /// Hill slope, top and bottom; optionally normalised to controls (percent effect).
    DoseResponse(DoseArgs),
    /// Per-well kinetic metrics of a kinetic read: max slope (Vmax) over a sliding window, lag
    /// time, time to max, mean slope, AUC.
    Kinetics(TimeArgs),
    /// Per-well growth curves (OD over time): maximum growth rate and doubling time from the
    /// exponential phase, lag time, and a logistic fit (K, r, N0).
    Growth(GrowthArgs),
    /// Assay quality from control wells: Z′, signal/background, signal/noise, SSMD, CVs.
    #[command(name = "assay-qc")]
    Qc(WellsArgs),
}

/// Options every analysis takes.
#[derive(Debug, clap::Args)]
pub struct CommonArgs {
    /// Plate-reader export (Gen5, SoftMax Pro, BMG, EnVision, Kaleido, Tecan, SkanIt, plate
    /// matrices) or a long CSV with `well` and `value` columns (optionally `time_s`/`time_min`,
    /// and layout columns).
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// Plate layout CSV/TSV: a plate-map grid (blocks whose corner cell names the field: role,
    /// sample, compound, concentration, dilution) or a long table with a `well` column.
    #[arg(long, value_name = "CSV")]
    pub layout: Option<PathBuf>,
    /// Ignore the layout the export embeds (Gen5 Well ID and Conc/Dil, SkanIt Sample, BMG
    /// Content/Concentration).
    #[arg(long)]
    pub no_embedded_layout: bool,
    /// Mark wells as blanks (`H1,H2`, `H1:H12`).
    #[arg(long, value_name = "WELLS")]
    pub blank_wells: Option<String>,
    /// Mark wells as positive controls (full effect / maximum signal).
    #[arg(long, value_name = "WELLS")]
    pub positive_wells: Option<String>,
    /// Mark wells as negative controls (no effect / vehicle).
    #[arg(long, value_name = "WELLS")]
    pub negative_wells: Option<String>,
    /// Mark wells as empty (ignored).
    #[arg(long, value_name = "WELLS")]
    pub empty_wells: Option<String>,
    /// Role of the wells a layout names NAME (role text or sample name, with or without a
    /// trailing index), repeatable: `--role DMSO=negative --role STAU=positive`. For names the
    /// layout cannot place (vehicle/DMSO wells are controls of unknown sign until mapped).
    #[arg(long = "role", value_name = "NAME=ROLE")]
    pub roles: Vec<String>,
    /// Table (plate) index (see `info` → tables[]).
    #[arg(long, default_value_t = 0)]
    pub table: u32,
    /// Read to analyse: 1-based number or label. Default: the first measured read.
    #[arg(long, value_name = "READ")]
    pub read: Option<String>,
    /// Spectral reads: the wavelength (nm).
    #[arg(long, value_name = "NM")]
    pub wavelength_nm: Option<f64>,
    /// Blank subtraction: `auto` (mean of the blank wells when there are any), `mean`,
    /// `median`, `none`.
    #[arg(long, value_enum, default_value = "auto")]
    pub blank_subtraction: BlankArg,
    /// Outliers within replicate groups: `grubbs` (≥ 3 wells, alpha 0.05), `mad` (≥ 5 wells,
    /// modified z-score > 3.5), `none`.
    #[arg(long, value_enum, default_value = "grubbs")]
    pub outliers: OutlierArg,
    /// Outlier cut-off: alpha (grubbs) or modified z-score (mad).
    #[arg(long, value_name = "X")]
    pub outlier_threshold: Option<f64>,
    /// Leave flagged outliers out of means, fits and controls.
    #[arg(long)]
    pub exclude_outliers: bool,
    /// Write tidy CSV tables to PREFIX.wells.csv, .samples.csv, .compounds.csv, .kinetics.csv,
    /// .growth.csv, .curve.csv.
    #[arg(long, value_name = "PREFIX")]
    pub csv: Option<PathBuf>,
    /// Replace existing output files.
    #[arg(long)]
    pub overwrite: bool,
    #[arg(long)]
    pub json: bool,
}

/// `assay-wells` / `assay-qc`.
#[derive(Debug, clap::Args)]
pub struct WellsArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Kinetic reads: how each well's time course becomes one value.
    #[arg(long, value_enum)]
    pub reduce: Option<ReduceArg>,
    /// Points per window for `--reduce max-slope`.
    #[arg(long, value_name = "N")]
    pub window: Option<usize>,
    /// `controls`: every well's percent effect, 0 % = negative control mean, 100 % = positive
    /// control mean (percent activity of a screen, as Gen5 `Norm…` reads compute it).
    #[arg(long, value_enum, default_value = "none")]
    pub normalize: NormalizeArg,
}

/// Fit options.
#[derive(Debug, clap::Args)]
pub struct FitArgs {
    /// Curve model: `linear`, `4pl`, `5pl`.
    #[arg(long, default_value = "4pl", value_parser = parse_model)]
    pub model: Model,
    /// Weighting: `none`, `1/y`, `1/y2`, `1/x`, `1/x2`.
    #[arg(long, default_value = "none", value_parser = parse_weighting)]
    pub weighting: Weighting,
    /// Confidence level of the intervals.
    #[arg(long, default_value_t = 0.95)]
    pub confidence: f64,
    /// Kinetic reads: how each well's time course becomes one value.
    #[arg(long, value_enum)]
    pub reduce: Option<ReduceArg>,
    /// Points per window for `--reduce max-slope`.
    #[arg(long, value_name = "N")]
    pub window: Option<usize>,
    /// Render the curve (points and fit) as a PNG.
    #[arg(long, value_name = "PNG")]
    pub plot: Option<PathBuf>,
}

/// `assay-curve`.
#[derive(Debug, clap::Args)]
pub struct CurveArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[command(flatten)]
    pub fit: FitArgs,
    /// Standards: `WELLS=CONC`, repeatable (`--standard A1,A2=100 --standard B1:B2=50`).
    #[arg(long = "standard", value_name = "WELLS=CONC")]
    pub standards: Vec<String>,
    /// Fit every standard replicate (`replicates`) or the level means (`means`).
    #[arg(long, value_enum, default_value = "replicates")]
    pub fit_on: FitOnArg,
    /// Lower limit of quantification (default: the recovery rule).
    #[arg(long)]
    pub lloq: Option<f64>,
    /// Upper limit of quantification (default: the recovery rule).
    #[arg(long)]
    pub uloq: Option<f64>,
}

/// `dose-response`.
#[derive(Debug, clap::Args)]
pub struct DoseArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    #[command(flatten)]
    pub fit: FitArgs,
    /// `controls`: fit percent effect, 0 % = negative control mean, 100 % = positive control
    /// mean.
    #[arg(long, value_enum, default_value = "none")]
    pub normalize: NormalizeArg,
}

/// `kinetics`.
#[derive(Debug, clap::Args)]
pub struct TimeArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Points per sliding window. Default: 5 or a tenth of the time points, whichever is
    /// larger.
    #[arg(long, value_name = "N")]
    pub window: Option<usize>,
    /// Only these wells (`A1:H6`).
    #[arg(long, value_name = "WELLS")]
    pub wells: Option<String>,
}

/// `growth`.
#[derive(Debug, clap::Args)]
pub struct GrowthArgs {
    #[command(flatten)]
    pub time: TimeArgs,
    /// Values at or below this are left out of the log-scale fit (default 5 % of each well's
    /// maximum).
    #[arg(long, value_name = "OD")]
    pub growth_threshold: Option<f64>,
}

fn parse_model(s: &str) -> std::result::Result<Model, String> {
    Model::parse(s).ok_or_else(|| format!("unknown model `{s}`: use linear, 4pl or 5pl"))
}

fn parse_weighting(s: &str) -> std::result::Result<Weighting, String> {
    Weighting::parse(s)
        .ok_or_else(|| format!("unknown weighting `{s}`: use none, 1/y, 1/y2, 1/x or 1/x2"))
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum BlankArg {
    Auto,
    Mean,
    Median,
    None,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OutlierArg {
    Mad,
    Grubbs,
    None,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ReduceArg {
    First,
    Last,
    Max,
    Min,
    Mean,
    MaxSlope,
    MeanSlope,
    Auc,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum FitOnArg {
    Replicates,
    Means,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum NormalizeArg {
    None,
    Controls,
}

fn reduce_of(r: Option<ReduceArg>) -> Option<Reduce> {
    r.map(|r| match r {
        ReduceArg::First => Reduce::First,
        ReduceArg::Last => Reduce::Last,
        ReduceArg::Max => Reduce::Max,
        ReduceArg::Min => Reduce::Min,
        ReduceArg::Mean => Reduce::Mean,
        ReduceArg::MaxSlope => Reduce::MaxSlope,
        ReduceArg::MeanSlope => Reduce::MeanSlope,
        ReduceArg::Auc => Reduce::Auc,
    })
}

fn role_map(
    c: &CommonArgs,
) -> std::result::Result<std::collections::BTreeMap<String, String>, String> {
    c.roles
        .iter()
        .map(|item| {
            item.rsplit_once('=')
                .map(|(n, r)| (n.trim().to_string(), r.trim().to_string()))
                .filter(|(n, r)| !n.is_empty() && !r.is_empty())
                .ok_or_else(|| format!("--role `{item}`: expected NAME=ROLE (e.g. DMSO=negative)"))
        })
        .collect()
}

fn base_request(c: &CommonArgs, analysis: Analysis) -> AssayRequest {
    AssayRequest {
        analysis,
        table: c.table,
        read: c.read.clone(),
        wavelength_nm: c.wavelength_nm,
        layout: c.layout.as_ref().map(|p| p.display().to_string()),
        embedded_layout: !c.no_embedded_layout,
        blank_wells: c.blank_wells.clone(),
        positive_wells: c.positive_wells.clone(),
        negative_wells: c.negative_wells.clone(),
        empty_wells: c.empty_wells.clone(),
        blank_subtraction: match c.blank_subtraction {
            BlankArg::Auto => BlankMode::Auto,
            BlankArg::Mean => BlankMode::Mean,
            BlankArg::Median => BlankMode::Median,
            BlankArg::None => BlankMode::None,
        },
        outliers: match c.outliers {
            OutlierArg::Mad => OutlierRule::Mad,
            OutlierArg::Grubbs => OutlierRule::Grubbs,
            OutlierArg::None => OutlierRule::None,
        },
        outlier_threshold: c.outlier_threshold,
        exclude_outliers: c.exclude_outliers,
        ..AssayRequest::default()
    }
}

fn apply_fit(req: &mut AssayRequest, f: &FitArgs) {
    req.model = Some(f.model);
    req.weighting = f.weighting;
    req.confidence = f.confidence;
    req.reduce = reduce_of(f.reduce);
    req.window = f.window;
}

/// Run `analyze assay-wells`, `assay-curve`, `dose-response`, `kinetics`, `growth` or
/// `assay-qc`.
pub fn run(reg: &Registry, command: &AssayCommand) -> i32 {
    let (common, req, plot) = match command {
        AssayCommand::Wells(w) | AssayCommand::Qc(w) => {
            let analysis = if matches!(command, AssayCommand::Qc(_)) {
                Analysis::Qc
            } else {
                Analysis::Wells
            };
            let mut r = base_request(&w.common, analysis);
            r.reduce = reduce_of(w.reduce);
            r.window = w.window;
            r.normalize = match w.normalize {
                NormalizeArg::None => Normalize::None,
                NormalizeArg::Controls => Normalize::Controls,
            };
            (&w.common, r, None)
        }
        AssayCommand::Curve(c) => {
            let mut r = base_request(&c.common, Analysis::Curve);
            apply_fit(&mut r, &c.fit);
            if !c.standards.is_empty() {
                r.standards = Some(c.standards.join(";"));
            }
            r.fit_on = match c.fit_on {
                FitOnArg::Replicates => FitOn::Replicates,
                FitOnArg::Means => FitOn::Means,
            };
            r.lloq = c.lloq;
            r.uloq = c.uloq;
            (&c.common, r, c.fit.plot.clone())
        }
        AssayCommand::DoseResponse(d) => {
            let mut r = base_request(&d.common, Analysis::DoseResponse);
            apply_fit(&mut r, &d.fit);
            r.normalize = match d.normalize {
                NormalizeArg::None => Normalize::None,
                NormalizeArg::Controls => Normalize::Controls,
            };
            (&d.common, r, d.fit.plot.clone())
        }
        AssayCommand::Kinetics(t) => {
            let mut r = base_request(&t.common, Analysis::Kinetics);
            r.window = t.window;
            r.wells = t.wells.clone();
            (&t.common, r, None)
        }
        AssayCommand::Growth(g) => {
            let mut r = base_request(&g.time.common, Analysis::Growth);
            r.window = g.time.window;
            r.wells = g.time.wells.clone();
            r.growth_threshold = g.growth_threshold;
            (&g.time.common, r, None)
        }
    };
    let result = (|| -> Result<AssayOutput> {
        let mut req = req;
        req.roles = role_map(common).map_err(Error::Usage)?;
        let mut out = openreadout_assay::analyze_file(reg, &common.file, &req)?;
        if let Some(prefix) = &common.csv {
            for p in openreadout_assay::write_tables(&out, prefix, common.overwrite)? {
                out.written.push(p.display().to_string());
            }
        }
        if let Some(png) = &plot {
            match openreadout_assay::render_plot(&out)? {
                Some((bytes, notes)) => {
                    openreadout_assay::write_verified(png, &bytes, common.overwrite)?;
                    out.written.push(png.display().to_string());
                    out.notes.extend(notes);
                }
                None => {
                    return Err(Error::Usage("nothing to draw: no curve was fitted".into()));
                }
            }
        }
        Ok(out)
    })();
    match result {
        Ok(v) => emit(common.json, &v, openreadout_assay::render_text),
        Err(e) => fail(common.json, &e),
    }
}
