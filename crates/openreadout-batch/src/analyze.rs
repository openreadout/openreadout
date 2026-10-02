//! The analyses by name: one entry point, picked by `kind`, configured by `options` (the same
//! options `openreadout_batch` takes for that measure). The MCP tool `openreadout_analyze`, the
//! Python `openreadout.analyze()` and the R `openreadout_analyze()` call it.

use std::path::{Path, PathBuf};

use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Error, Registry, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AnalyzeKind {
    /// chromatographic peaks (detector trace, TIC, XIC, SRM): rt, area, height, widths, tailing, plates, resolution, S/N, area % (purity); compound lists; bands and regions of IR/Raman/UV-Vis/NMR spectra
    Peaks,
    /// TIC, BPC, XIC (mz + ppm), SRM transitions, stored or detector chromatograms: apex time and intensity, integral, thinned arrays ('when does m/z X elute')
    Chromatogram,
    /// NMR peak list (ppm, height, width, S/N) and region integrals, from the processed spectrum or the FID
    NmrPeaks,
    /// patch-clamp: action potentials per sweep, rheobase, f–I slope, input resistance, tau, capacitance, sag; voltage-clamp holding current and access resistance
    EphysFeatures,
    /// extracellular spike detection per channel: counts, rates, times
    Spikes,
    /// qPCR (RDML, .eds, .rex): Cq and Tm per well × target, ΔΔCq fold changes, standard curves
    Qpcr,
    /// plate-reader assays: per-well values, standard curves with back-calculated concentrations, dose-response IC50/EC50, kinetics, growth, Z′
    Assay,
    /// flow-cytometry gating from a FlowJo workspace or Gating-ML file: population counts, percentages and medians
    Gate,
}

impl AnalyzeKind {
    /// The kind's name, as `openreadout_batch` names the measure.
    pub fn name(self) -> &'static str {
        match self {
            Self::Peaks => "peaks",
            Self::Chromatogram => "chromatogram",
            Self::NmrPeaks => "nmr-peaks",
            Self::EphysFeatures => "ephys-features",
            Self::Spikes => "spikes",
            Self::Qpcr => "qpcr",
            Self::Assay => "assay",
            Self::Gate => "gate",
        }
    }

    /// Every kind, in the order the CLI lists them.
    pub const ALL: [AnalyzeKind; 8] = [
        Self::Peaks,
        Self::Chromatogram,
        Self::NmrPeaks,
        Self::EphysFeatures,
        Self::Spikes,
        Self::Qpcr,
        Self::Assay,
        Self::Gate,
    ];

    /// The kind named `name` (`peaks`, `nmr-peaks`, ...); a usage error naming them otherwise.
    pub fn parse(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|k| k.name() == name)
            .ok_or_else(|| {
                let names: Vec<&str> = Self::ALL.iter().map(|k| k.name()).collect();
                Error::Usage(format!(
                    "unknown analysis kind {name:?}: one of {}",
                    names.join(", ")
                ))
            })
    }

    /// Whether the analysis reads one opened data set ([`run_dataset`]); `qpcr`, `assay` and
    /// `gate` open their files themselves and need a path ([`run`]).
    pub fn reads_dataset(self) -> bool {
        !matches!(self, Self::Qpcr | Self::Assay | Self::Gate)
    }
}

/// `options` of kind `qpcr`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct QpcrOptions {
    /// Which records and analyses.
    #[serde(flatten)]
    pub query: crate::measures::QpcrQuery,
    /// At most this many records (default 500).
    pub max_records: Option<usize>,
}

/// `options` of kind `assay`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AssayOptions {
    /// Also return a PNG of the fitted curve (points and fit) as image content (`curve`,
    /// `dose-response`).
    #[serde(default)]
    pub plot: bool,
    /// What to compute and how.
    #[serde(flatten)]
    pub request: openreadout_assay::AssayRequest,
}

/// `options` of kind `gate`. Without `workspace` or `gatingml`, `file` is the gating file and
/// the answer describes it (samples, compensation, transforms, the gate tree).
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct GateOptions {
    /// FlowJo workspace (.wsp); `file` is then the FCS file to count events in.
    pub workspace: Option<String>,
    /// Gating-ML 2.0 document (instead of workspace).
    pub gatingml: Option<String>,
    /// Workspace sample name or id (default: matched by file name, then $FIL, then the only one).
    pub sample: Option<String>,
    /// Report only these populations (paths or unique names) and their descendants.
    #[serde(default)]
    pub populations: Vec<String>,
    /// FCS data set index. Default 0.
    #[serde(default)]
    pub table: u32,
    /// Parameters whose median over each population's events is reported under
    /// populations[].medians (`$PnN` or `$PnS`; `Comp-NAME` for compensated values).
    #[serde(default)]
    pub medians: Vec<String>,
}

/// The options of every kind (for the input schema; each call passes those of its `kind`).
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
enum AnalyzeOptions {
    /// kind=peaks.
    Peaks(openreadout_quant::api::PeaksQuery),
    /// kind=chromatogram.
    Chromatogram(openreadout_quant::api::ChromatogramQuery),
    /// kind=nmr-peaks.
    NmrPeaks(openreadout_signal::api::NmrQuery),
    /// kind=ephys-features.
    EphysFeatures(openreadout_signal::api::EphysQuery),
    /// kind=spikes.
    Spikes(openreadout_signal::api::SpikesQuery),
    /// kind=qpcr.
    Qpcr(QpcrOptions),
    /// kind=assay.
    Assay(AssayOptions),
    /// kind=gate.
    Gate(GateOptions),
}

/// Arguments for `openreadout_analyze`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AnalyzeArgs {
    /// Absolute or working-directory-relative path to the instrument file (kind=gate: the FCS
    /// file, or the gating file itself to describe it).
    pub file: String,
    /// The analysis.
    pub kind: AnalyzeKind,
    /// The options of `kind` (all optional; the schema lists every kind's).
    #[serde(default)]
    #[schemars(with = "Option<AnalyzeOptions>")]
    pub options: Map<String, Value>,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// What `openreadout_analyze` returns, by kind.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum AnalyzeOutput {
    /// kind=peaks.
    Peaks(openreadout_quant::analyze::PeaksOutput),
    /// kind=chromatogram.
    Chromatogram(openreadout_quant::extract::ChromatogramOutput),
    /// kind=nmr-peaks.
    NmrPeaks(openreadout_signal::nmr::NmrReport),
    /// kind=ephys-features.
    EphysFeatures(openreadout_signal::ephys::CellReport),
    /// kind=spikes.
    Spikes(openreadout_signal::ephys::SpikesReport),
    /// kind=qpcr.
    Qpcr(openreadout_qpcr::QpcrReport),
    /// kind=assay.
    Assay(openreadout_assay::AssayOutput),
    /// kind=gate.
    Gate(openreadout_core::flow::GateOutput),
}

fn with_file<T>(
    reg: &Registry,
    file: &str,
    f: impl FnOnce(&mut dyn Dataset, &openreadout_core::FileInfo, &ReadContext<'_>) -> Result<T>,
) -> Result<T> {
    let (_, mut ds) = reg.open(Path::new(file))?;
    let info = ds.info()?;
    let opener = || -> Result<Box<dyn Dataset>> { reg.open(Path::new(file)).map(|(_, d)| d) };
    f(
        ds.as_mut(),
        &info,
        &ReadContext {
            opener: Some(&opener),
            progress: None,
        },
    )
}

fn parse<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    kind: AnalyzeKind,
    options: &Map<String, Value>,
) -> Result<T> {
    crate::measures::parse_options(kind.name(), options)
}

/// Run one analysis; the PNG of an assay fit when `plot` is set.
pub fn run(reg: &Registry, a: &AnalyzeArgs) -> Result<(AnalyzeOutput, Option<Vec<u8>>)> {
    let o = &a.options;
    let out = match a.kind {
        AnalyzeKind::Qpcr => {
            let q: QpcrOptions = parse(a.kind, o)?;
            AnalyzeOutput::Qpcr(qpcr(reg, &a.file, q)?)
        }
        AnalyzeKind::Assay => {
            let mut opts = o.clone();
            let plot = match opts.remove("plot") {
                None => false,
                Some(Value::Bool(b)) => b,
                Some(other) => {
                    return Err(Error::Usage(format!(
                        "`plot` must be true or false, got {other}"
                    )));
                }
            };
            let request: openreadout_assay::AssayRequest = parse(a.kind, &opts)?;
            let out = openreadout_assay::analyze_file(reg, Path::new(&a.file), &request)?;
            let png = if plot {
                openreadout_assay::render_plot(&out)?.map(|(bytes, _)| bytes)
            } else {
                None
            };
            return Ok((AnalyzeOutput::Assay(out), png));
        }
        AnalyzeKind::Gate => {
            let g: GateOptions = parse(a.kind, o)?;
            AnalyzeOutput::Gate(gate(&a.file, g)?)
        }
        kind => with_file(reg, &a.file, |ds, info, ctx| {
            run_dataset(kind, o, ds, info, ctx)
        })?,
    };
    Ok((out, None))
}

/// Run an analysis that reads one opened data set (see [`AnalyzeKind::reads_dataset`]) on `ds`.
pub fn run_dataset(
    kind: AnalyzeKind,
    o: &Map<String, Value>,
    ds: &mut dyn Dataset,
    info: &openreadout_core::FileInfo,
    ctx: &ReadContext<'_>,
) -> Result<AnalyzeOutput> {
    Ok(match kind {
        AnalyzeKind::Peaks => {
            let q: openreadout_quant::api::PeaksQuery = parse(kind, o)?;
            AnalyzeOutput::Peaks(openreadout_quant::api::peaks(ds, info, &q, ctx)?)
        }
        AnalyzeKind::Chromatogram => {
            let q: openreadout_quant::api::ChromatogramQuery = parse(kind, o)?;
            AnalyzeOutput::Chromatogram(openreadout_quant::api::chromatogram(ds, info, &q, ctx)?)
        }
        AnalyzeKind::NmrPeaks => {
            let q: openreadout_signal::api::NmrQuery = parse(kind, o)?;
            AnalyzeOutput::NmrPeaks(openreadout_signal::nmr::analyze(ds, info, &q.request()?)?)
        }
        AnalyzeKind::EphysFeatures => {
            let q: openreadout_signal::api::EphysQuery = parse(kind, o)?;
            AnalyzeOutput::EphysFeatures(openreadout_signal::ephys::analyze_cell(
                ds,
                info,
                &q.request()?,
            )?)
        }
        AnalyzeKind::Spikes => {
            let q: openreadout_signal::api::SpikesQuery = parse(kind, o)?;
            AnalyzeOutput::Spikes(openreadout_signal::ephys::analyze_extracellular(
                ds,
                info,
                &q.request()?,
            )?)
        }
        AnalyzeKind::Qpcr | AnalyzeKind::Assay | AnalyzeKind::Gate => {
            return Err(Error::Usage(format!(
                "analysis {:?} opens its file itself: give it a path",
                kind.name()
            )));
        }
    })
}

fn qpcr(reg: &Registry, file: &str, o: QpcrOptions) -> Result<openreadout_qpcr::QpcrReport> {
    let q = o.query;
    let ds = openreadout_qpcr::open_qpcr(reg, Path::new(file))?;
    let mut req = openreadout_qpcr::QpcrReportRequest::default();
    req.well = q.well;
    req.target = q.target;
    req.sample = q.sample;
    req.run = q.run;
    req.compute_cq = q.compute_cq;
    req.threshold = q.threshold;
    req.baseline = match (q.baseline_start, q.baseline_end) {
        (Some(s), Some(e)) if s >= 1 && e > s => Some((s, e)),
        (None, None) => None,
        _ => {
            return Err(Error::Usage(
                "baseline_start and baseline_end go together, with 1 <= start < end".into(),
            ));
        }
    };
    req.relative = q.ddcq;
    req.reference_targets = q.reference_targets;
    req.control_sample = q.control_sample;
    req.standard_curve = q.standard_curve;
    req.max_records = Some(o.max_records.unwrap_or(500));
    req.undetermined_cq = q.undetermined_cq;
    openreadout_qpcr::qpcr_report(&ds, &req)
}

fn gate(file: &str, g: GateOptions) -> Result<openreadout_core::flow::GateOutput> {
    let (gating, fcs) = match g.workspace.as_ref().or(g.gatingml.as_ref()) {
        Some(w) => (PathBuf::from(w), Some(PathBuf::from(file))),
        None => (PathBuf::from(file), None),
    };
    let mut req = openreadout_fcs::analysis::GateRequest::new(gating, fcs);
    req.sample = g.sample;
    req.table = g.table;
    req.populations = g.populations;
    req.medians = g.medians;
    openreadout_fcs::analysis::gate(&req)
}
