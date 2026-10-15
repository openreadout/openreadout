//! `plot`: a figure from lab files, described by the same JSON spec as the `openreadout_plot`
//! MCP tool.

use std::fmt::Write as _;
use std::path::PathBuf;

use openreadout_core::{Error, Registry, Result};

use crate::output::{emit, fail};

/// Arguments of `plot`.
#[derive(Debug, clap::Args)]
pub struct PlotArgs {
    /// The figure as JSON (the arguments of the `openreadout_plot` MCP tool): a .json file, or
    /// the JSON text itself, e.g. `'{"data": {"file": "run.mzML", "peaks": 1}}'`.
    #[arg(value_name = "SPEC")]
    pub spec: String,
    /// Write the figure here: .png, .jpg, .tiff (at `--scale`), .svg or .pdf, verified after
    /// writing.
    #[arg(short, long, value_name = "FILE")]
    pub output: PathBuf,
    /// Pixels per figure unit of .png, .jpg and .tiff files (0.25 to 8).
    #[arg(long, default_value_t = openreadout_plot::DEFAULT_SCALE)]
    pub scale: f32,
    /// Print resolution: recorded in raster files, and the page size of .svg and .pdf (figure
    /// units × scale ÷ dpi inches).
    #[arg(long)]
    pub dpi: Option<u32>,
    /// Replace an existing output file.
    #[arg(long)]
    pub overwrite: bool,
    /// `light` (default) or `dark`.
    #[arg(long, value_name = "THEME")]
    pub theme: Option<String>,
    #[arg(long)]
    pub json: bool,
}

fn spec_of(a: &PlotArgs) -> Result<openreadout_plot::PlotSpec> {
    let text = if a.spec.trim_start().starts_with('{') {
        a.spec.clone()
    } else {
        std::fs::read_to_string(&a.spec).map_err(|e| Error::io(&a.spec, e))?
    };
    let mut v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Error::Usage(format!("plot spec is not valid JSON: {e}")))?;
    if let Some(t) = &a.theme
        && let serde_json::Value::Object(m) = &mut v
    {
        m.insert("theme".into(), serde_json::Value::String(t.clone()));
    }
    openreadout_plot::parse_spec(v)
}

/// What `plot` prints: what was drawn and the file written.
#[derive(serde::Serialize, schemars::JsonSchema)]
pub struct PlotReport {
    /// What was drawn.
    #[serde(flatten)]
    pub plot: openreadout_plot::PlotOutput,
    /// The file written.
    pub written: openreadout_plot::WrittenFile,
}

fn run_plot(reg: &Registry, a: &PlotArgs) -> Result<PlotReport> {
    let spec = spec_of(a)?;
    let opts = openreadout_plot::SaveOptions {
        scale: a.scale,
        dpi: a.dpi,
        overwrite: a.overwrite,
    };
    // check the file name before reading any data
    openreadout_plot::FileFormat::from_path(&a.output)?;
    let mut inputs = spec.input_files();
    if !a.spec.trim_start().starts_with('{') {
        inputs.push(PathBuf::from(&a.spec));
    }
    let inputs: Vec<&std::path::Path> = inputs.iter().map(PathBuf::as_path).collect();
    openreadout_core::output::refuse_inputs(&a.output, inputs.iter().copied())?;
    let p = openreadout_plot::plot(reg, &spec)?;
    let written = openreadout_plot::write(&p.figure, &a.output, &opts, &inputs)?;
    Ok(PlotReport {
        plot: p.output,
        written,
    })
}

/// Run `plot`.
pub fn run(reg: &Registry, a: &PlotArgs) -> i32 {
    match run_plot(reg, a) {
        Ok(report) => emit(a.json, &report, |report| {
            let (o, w) = (&report.plot, &report.written);
            let mut s = String::new();
            let unit = if matches!(w.format.as_str(), "svg" | "pdf") {
                "units"
            } else {
                "px"
            };
            let _ = writeln!(
                s,
                "wrote {} ({} × {} {unit}, {} bytes, verified)",
                w.path, w.width, w.height, w.bytes
            );
            for p in &o.panels {
                let _ = writeln!(
                    s,
                    "  {}{} series; x {}; y {}",
                    p.title
                        .as_ref()
                        .map(|t| format!("{t}: "))
                        .unwrap_or_default(),
                    p.series.iter().map(|x| x.count.unwrap_or(1)).sum::<usize>(),
                    p.x.label,
                    p.y.label
                );
            }
            for n in &o.notes {
                let _ = writeln!(s, "  note: {n}");
            }
            for w in &o.warnings {
                let _ = writeln!(s, "  warning: {w}");
            }
            s.trim_end().to_string()
        }),
        Err(e) => fail(a.json, &e),
    }
}
