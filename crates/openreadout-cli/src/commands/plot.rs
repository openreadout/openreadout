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
    /// Write the figure here (.png at 2×, .svg or .pdf), verified after writing. Overrides
    /// the spec's `output`.
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,
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
    let mut spec = openreadout_plot::parse_spec(v)?;
    if let Some(o) = &a.output {
        spec.output = Some(o.display().to_string());
    }
    spec.overwrite |= a.overwrite;
    if spec.output.is_none() {
        return Err(Error::Usage(
            "plot needs somewhere to write the figure: -o FILE (.png, .svg or .pdf)".into(),
        ));
    }
    Ok(spec)
}

/// Run `plot`.
pub fn run(reg: &Registry, a: &PlotArgs) -> i32 {
    let spec = match spec_of(a) {
        Ok(s) => s,
        Err(e) => return fail(a.json, &e),
    };
    match openreadout_plot::run(reg, &spec) {
        Ok(p) => emit(a.json, &p.output, |o| {
            let mut s = String::new();
            if let Some(w) = &o.output {
                let _ = writeln!(
                    s,
                    "wrote {} ({} × {} px, {} bytes, verified)",
                    w.path, o.width, o.height, w.bytes
                );
            }
            for p in &o.panels {
                let _ = writeln!(
                    s,
                    "  {}{} series; x {}; y {}",
                    p.title.as_ref().map(|t| format!("{t}: ")).unwrap_or_default(),
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
