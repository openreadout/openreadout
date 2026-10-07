//! `analyze chromatogram`: TIC, BPC, XIC and SRM chromatograms from mass spectra, and stored
//! chromatograms / detector signals, as JSON, CSV, Parquet, Arrow or a PNG plot
//! (`openreadout_quant::extract`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use openreadout_core::parallel::ReadContext;
use openreadout_core::{Dataset, Error, Registry, Result};
use openreadout_quant::extract::{
    Aggregate, ChromRequest, ChromatogramOutput, Target, Tolerance, extract,
};

use super::batch::{self, BatchArgs, Item, Spec, Stdin};
use crate::ui::{self, Align, Progress};

/// How the points inside an XIC window are combined.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum AggregateArg {
    /// Sum of the intensities in the window.
    #[default]
    Sum,
    /// Largest intensity in the window.
    Max,
}

/// Which chromatograms, from which scans (shared by `chromatogram` and `peaks`).
#[derive(Debug, Clone, Default, clap::Args)]
pub struct SourceArgs {
    /// Total-ion chromatogram (the default for mass-spectrometry files when nothing else is
    /// chosen): each scan's recorded TIC, or the sum of its intensities.
    #[arg(long)]
    pub tic: bool,
    /// Base-peak chromatogram.
    #[arg(long)]
    pub bpc: bool,
    /// Extracted-ion chromatogram at this m/z (repeatable, or comma-separated): the sum of the
    /// intensities within ±tolerance in each scan, one pass for all of them.
    #[arg(long = "mz", value_name = "MZ", value_delimiter = ',')]
    pub mz: Vec<f64>,
    /// XIC tolerance (half-width) in ppm. Default 10.
    #[arg(long, value_name = "PPM", conflicts_with = "da")]
    pub ppm: Option<f64>,
    /// XIC tolerance (half-width) in m/z units instead of ppm.
    #[arg(long, value_name = "DA")]
    pub da: Option<f64>,
    /// SRM/MRM transition `Q1>Q3` (or `Q1/Q3`), repeatable: from MS/MS scans, stored SRM
    /// chromatograms (mzML) or MRM table columns (Waters).
    #[arg(long = "transition", value_name = "Q1>Q3")]
    pub transitions: Vec<String>,
    /// Q1 and Q3 tolerance of `--transition`, in m/z units. Default 0.5 (unit resolution; the
    /// nearest distinct precursor and product within it are used).
    #[arg(long, value_name = "DA")]
    pub transition_tol: Option<f64>,
    /// A stored chromatogram or detector signal (UV/DAD, FID, TCD, a stored TIC or SRM trace):
    /// trace index as `info` → `traces[]` lists it; repeatable. The default for files with
    /// traces and no spectra.
    #[arg(long = "trace", value_name = "N")]
    pub traces: Vec<u32>,
    /// Channel of `--trace` (e.g. one wavelength of a DAD spectrum trace). Default: its first
    /// signal channel.
    #[arg(long, value_name = "C")]
    pub channel: Option<u32>,
    /// Sweep of `--trace`.
    #[arg(long, default_value_t = 0)]
    pub sweep: u32,
    /// Spectra run index.
    #[arg(long, default_value_t = 0)]
    pub run: u32,
    /// MS level of the scans. Default 1 (2 with `--precursor`; SRM: any MS/MS level).
    #[arg(long, value_name = "N")]
    pub ms_level: Option<u32>,
    /// Only scans of this polarity: `positive` or `negative`.
    #[arg(long, value_name = "POLARITY")]
    pub polarity: Option<String>,
    /// Only scans whose scan filter/description contains this text (case-insensitive), e.g.
    /// `"FTMS + p"` or `"Full ms"`.
    #[arg(long, value_name = "TEXT")]
    pub scan_filter: Option<String>,
    /// Only MS/MS scans of this precursor m/z (product-ion XIC with `--mz`).
    #[arg(long, value_name = "MZ")]
    pub precursor: Option<f64>,
    /// Precursor tolerance in m/z units. Default 0.5.
    #[arg(long, value_name = "DA")]
    pub precursor_tol: Option<f64>,
    /// Retention-time range in minutes, `A-B`.
    #[arg(long, value_name = "A-B")]
    pub rt_range: Option<String>,
    /// TIC/BPC over this m/z range only, `A-B`.
    #[arg(long, value_name = "A-B")]
    pub mz_range: Option<String>,
    /// Use profile data where a scan stores both profile and centroids (default: the
    /// instrument's centroids; profile points are summed within the XIC window).
    #[arg(long)]
    pub profile: bool,
    /// How points inside an XIC window are combined.
    #[arg(long, value_enum, default_value_t)]
    pub aggregate: AggregateArg,
}

/// Parse `A-B` (numbers, `A < B`).
pub fn parse_range(s: &str, what: &str) -> Result<[f64; 2]> {
    let bad = || {
        Error::Usage(format!(
            "{what} {s:?}: expected A-B with A < B, e.g. 5.1-5.6"
        ))
    };
    let t = s.trim();
    // the separator is the first '-' that is not a leading sign
    let pos = t
        .char_indices()
        .skip(1)
        .find(|&(i, c)| (c == '-' || c == ':') && !t[..i].ends_with(['e', 'E']))
        .map(|(i, _)| i)
        .ok_or_else(bad)?;
    let a: f64 = t[..pos].trim().parse().map_err(|_| bad())?;
    let b: f64 = t[pos + 1..].trim().parse().map_err(|_| bad())?;
    if !(a.is_finite() && b.is_finite() && a < b) {
        return Err(bad());
    }
    Ok([a, b])
}

/// Parse `Q1>Q3` or `Q1/Q3`.
pub fn parse_transition(s: &str) -> Result<(f64, f64)> {
    let bad = || {
        Error::Usage(format!(
            "--transition {s:?}: expected Q1>Q3 or Q1/Q3, e.g. 279.2>179.2"
        ))
    };
    let (a, b) = s
        .split_once('>')
        .or_else(|| s.split_once('/'))
        .ok_or_else(bad)?;
    let a: f64 = a.trim().parse().map_err(|_| bad())?;
    let b: f64 = b.trim().parse().map_err(|_| bad())?;
    Ok((a, b))
}

impl SourceArgs {
    /// The extraction request these flags describe.
    pub fn request(&self) -> Result<ChromRequest> {
        let mut r = ChromRequest::default();
        if self.tic {
            r.targets.push(Target::Tic);
        }
        if self.bpc {
            r.targets.push(Target::Bpc);
        }
        for &mz in &self.mz {
            r.targets.push(Target::Xic { mz });
        }
        for t in &self.transitions {
            let (q1, q3) = parse_transition(t)?;
            r.targets.push(Target::Srm { q1, q3 });
        }
        for &trace in &self.traces {
            r.targets.push(Target::Trace {
                trace,
                channel: self.channel,
            });
        }
        if self.channel.is_some() && self.traces.is_empty() {
            r.targets.push(Target::Trace {
                trace: 0,
                channel: self.channel,
            });
        }
        if let Some(p) = self.ppm {
            r.tolerance = Tolerance::Ppm(p);
        }
        if let Some(d) = self.da {
            r.tolerance = Tolerance::Da(d);
        }
        if let Some(t) = self.transition_tol {
            r.transition_tolerance_da = t;
        }
        r.run = self.run;
        r.ms_level = self.ms_level;
        r.polarity = self.polarity.as_deref().map(str::parse).transpose()?;
        r.scan_filter.clone_from(&self.scan_filter);
        r.precursor_mz = self.precursor;
        if let Some(t) = self.precursor_tol {
            r.precursor_tolerance_da = t;
        }
        r.rt_range_min = self
            .rt_range
            .as_deref()
            .map(|s| parse_range(s, "--rt-range"))
            .transpose()?;
        r.mz_range = self
            .mz_range
            .as_deref()
            .map(|s| parse_range(s, "--mz-range"))
            .transpose()?;
        r.profile = self.profile;
        r.aggregate = match self.aggregate {
            AggregateArg::Sum => Aggregate::Sum,
            AggregateArg::Max => Aggregate::Max,
        };
        r.sweep = self.sweep;
        Ok(r)
    }
}

/// Arguments of `chromatogram`.
#[derive(Debug, clap::Args)]
pub struct ChromatogramArgs {
    /// Files, directories or glob patterns (several make a batch); `-` reads standard input.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: BatchArgs,
    #[command(flatten)]
    pub source: SourceArgs,
    /// Return at most this many points per chromatogram (the most intense point of each of N
    /// equal slices; the summary still covers every point). Default: all.
    #[arg(long, value_name = "N")]
    pub max_points: Option<usize>,
    /// Write the chromatograms to a file: `.csv`, `.parquet`, `.arrow` (a table: `rt_min` and
    /// one column per chromatogram, or long form `chromatogram, rt_min, intensity` when their
    /// times differ) or `.png`/`.jpg` (a plot). One input only.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
    /// Plot width in pixels for `.png`/`.jpg` output.
    #[arg(long, default_value_t = 1200)]
    pub width: u32,
    /// Replace an existing output file.
    #[arg(long)]
    pub overwrite: bool,
    #[arg(long)]
    pub json: bool,
}

impl Item for ChromatogramOutput {
    fn format(&self) -> Option<String> {
        Some(self.format.clone())
    }
    fn contents(&self) -> Option<String> {
        Some(
            self.chromatograms
                .iter()
                .map(|c| c.label.clone())
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
    fn relabel(&mut self, to: &str) {
        self.path = to.into();
    }
}

/// Open `path` and extract, with parallel spectrum reads and a progress bar.
pub fn extract_file(
    reg: &Registry,
    path: &Path,
    req: &ChromRequest,
    show_progress: bool,
) -> Result<(ChromatogramOutput, Box<dyn Dataset>)> {
    let (_, mut ds) = reg.open(path)?;
    let info = ds.info()?;
    let opener = || -> Result<Box<dyn Dataset>> { reg.open(path).map(|(_, d)| d) };
    let bar: OnceLock<Progress> = OnceLock::new();
    let progress = |done: u64, total: u64| {
        if show_progress {
            bar.get_or_init(|| Progress::new(total, "scans"))
                .set(done, None);
        }
    };
    let out = extract(
        ds.as_mut(),
        &info,
        req,
        &ReadContext {
            opener: Some(&opener),
            progress: Some(&progress),
        },
    )?;
    if let Some(b) = bar.get() {
        b.finish();
    }
    Ok((out, ds))
}

fn write_output(
    input: &Path,
    output: &Path,
    out: &ChromatogramOutput,
    width: u32,
    overwrite: bool,
) -> Result<()> {
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" => {
            let canvas = openreadout_quant::plot::render(
                &out.chromatograms,
                &vec![None; out.chromatograms.len()],
                width,
            );
            let enc = if ext == "png" {
                openreadout_preview::Encoding::Png
            } else {
                openreadout_preview::Encoding::Jpeg
            };
            let bytes = openreadout_preview::encode(
                &canvas,
                enc,
                openreadout_preview::DEFAULT_JPEG_QUALITY,
            )?;
            openreadout_preview::write_verified(input, output, &bytes, overwrite)?;
        }
        "csv" => {
            let mut t = openreadout_quant::dataset::ChromatogramTable::new(out);
            openreadout_batch::csv::export_csv(
                &mut t,
                input,
                output,
                &openreadout_batch::csv::CsvOptions {
                    table: Some(0),
                    rows: None,
                    labels: false,
                    overwrite,
                    sweep: None,
                    trace: None,
                },
            )?;
        }
        #[cfg(feature = "parquet")]
        "parquet" | "arrow" | "feather" => {
            let mut t = openreadout_quant::dataset::ChromatogramTable::new(out);
            let mut o = openreadout_arrow::ColumnarOptions::default();
            o.format = if ext == "parquet" {
                openreadout_arrow::ColumnarFormat::Parquet
            } else {
                openreadout_arrow::ColumnarFormat::ArrowIpc
            };
            o.select = openreadout_arrow::ColumnarSelection::Table(0);
            o.overwrite = overwrite;
            openreadout_arrow::export_columnar(&mut t, input, output, &o)?;
        }
        _ => {
            return Err(Error::Usage(format!(
                "-o {}: use a .csv, .parquet, .arrow, .png or .jpg file name",
                output.display()
            )));
        }
    }
    Ok(())
}

pub fn run(reg: &Registry, a: &ChromatogramArgs) -> i32 {
    let req = match a.source.request() {
        Ok(r) => r,
        Err(e) => return crate::output::fail(a.json, &e),
    };
    if a.output.is_some() && (a.files.len() > 1 || a.batch.recursive) {
        return crate::output::fail(
            a.json,
            &Error::Usage("-o writes one input's chromatograms; give one file".into()),
        );
    }
    batch::run(
        reg,
        &a.files,
        Spec {
            json: a.json,
            batch: &a.batch,
            stdin: Stdin::Spool,
        },
        &mut |input| {
            let (mut out, _) = extract_file(reg, input.path, &req, !input.batch)?;
            if let Some(o) = &a.output {
                write_output(input.path, o, &out, a.width, a.overwrite)?;
                out.output = Some(o.display().to_string());
            }
            if let Some(m) = a.max_points {
                for c in &mut out.chromatograms {
                    c.decimate(m);
                }
            }
            Ok(out)
        },
        &render,
    )
}

fn num(v: Option<f64>) -> String {
    match v {
        None => "-".into(),
        Some(x) if x != 0.0 && (x.abs() >= 1e6 || x.abs() < 1e-3) => format!("{x:.3e}"),
        Some(x) => format!("{x:.4}"),
    }
}

fn render(o: &ChromatogramOutput) -> String {
    let rows: Vec<Vec<String>> = o
        .chromatograms
        .iter()
        .map(|c| {
            vec![
                c.label.clone(),
                c.source.clone(),
                c.points.to_string(),
                num(c.apex_rt_min),
                num(c.apex_intensity),
                num(c.integral),
            ]
        })
        .collect();
    let mut s = format!("{} ({})\n", o.path, o.format);
    s.push_str(&ui::table(
        &[
            "chromatogram",
            "source",
            "points",
            "apex min",
            "apex",
            "integral",
        ],
        &[
            Align::Left,
            Align::Left,
            Align::Right,
            Align::Right,
            Align::Right,
            Align::Right,
        ],
        &rows,
    ));
    for c in &o.chromatograms {
        for n in &c.notes {
            s.push_str(&format!("\n{}: {n}", c.label));
        }
    }
    if let Some(p) = &o.output {
        s.push_str(&format!("\nwrote {p}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)] // parsed literals compare exactly
    fn ranges_and_transitions() {
        assert_eq!(parse_range("5.1-5.6", "x").unwrap(), [5.1, 5.6]);
        assert_eq!(parse_range("1e-1-2", "x").unwrap(), [0.1, 2.0]);
        assert_eq!(parse_range("100:200", "x").unwrap(), [100.0, 200.0]);
        assert!(parse_range("5", "x").is_err());
        assert!(parse_range("6-5", "x").is_err());
        assert_eq!(parse_transition("279.2>179.2").unwrap(), (279.2, 179.2));
        assert_eq!(parse_transition("279.2/179.2").unwrap(), (279.2, 179.2));
        assert!(parse_transition("279.2").is_err());
    }
}
