//! `trace`: samples and statistics of one sweep, or a per-sweep table over many files.

use std::path::PathBuf;

use openreadout_core::trace::{TraceRequest, TraceSlice, slice_trace};
use openreadout_core::{Error, Registry};

use super::{batch, nmr, peaks, tidy, truncate, wrap};

/// Arguments of `trace`.
#[derive(Debug, clap::Args)]
pub struct TraceArgs {
    /// The file; several files, directories or globs make a batch table.
    #[arg(required_unless_present = "from_index", value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: batch::BatchArgs,
    #[command(flatten)]
    pub tidy: tidy::TidyArgs,
    /// Trace index (see `info` → `traces[]`). Default 0 (batch table: every trace).
    #[arg(long)]
    pub trace: Option<u32>,
    /// Sweep (episode or segment) index. Default 0 (batch table: every sweep).
    #[arg(long)]
    pub sweep: Option<u32>,
    /// Channel index; repeatable. Default: all channels.
    #[arg(long = "channel")]
    pub channels: Vec<u32>,
    /// First sample of the window (zero-based, within the sweep).
    #[arg(long, default_value_t = 0)]
    pub first: u64,
    /// Window length in samples. Default: to the end of the sweep.
    #[arg(long)]
    pub count: Option<u64>,
    /// Window on the trace's own axis instead of --first/--count, `A:B` (either order): cm⁻¹,
    /// nm, ppm or a chromatogram's retention time in its axis unit (see `info` →
    /// traces[].extra.axis); seconds for signals without an axis.
    #[arg(long = "x-range", value_name = "A:B", allow_hyphen_values = true, conflicts_with_all = ["first", "count"])]
    pub x_range: Option<String>,
    /// Samples returned per channel (statistics always cover the whole window). Max 100000.
    #[arg(long, default_value_t = 1000)]
    pub max_samples: u64,
    #[command(flatten)]
    pub process: nmr::ProcessArgs,
    #[arg(long)]
    pub json: bool,
}

pub fn run(reg: &Registry, a: &TraceArgs) -> i32 {
    if tidy::wanted(reg, &a.tidy, &a.batch, &a.files, true) {
        let x_range = match a.x_range.as_deref().map(peaks::parse_x_range).transpose() {
            Ok(r) => r,
            Err(e) => return crate::output::fail(a.json, &e),
        };
        let m = openreadout_batch::measures::TraceMeasure {
            trace: a.trace,
            sweep: a.sweep,
            channels: a.channels.clone(),
            first: a.first,
            count: a.count,
            x_range,
        };
        return tidy::run(reg, &m, &a.files, &a.batch, &a.tidy, a.json);
    }
    wrap(
        a.json,
        || {
            let file = a
                .files
                .first()
                .ok_or_else(|| Error::Usage("no input given".into()))?;
            let (_, ds) = reg.open(file)?;
            let mut ds = a.process.wrap(ds)?;
            let info = ds.info()?;
            let mut request = TraceRequest::default();
            request.trace = a.trace.unwrap_or(0);
            request.sweep = a.sweep.unwrap_or(0);
            request.channels = a.channels.clone();
            request.first_sample = a.first;
            request.count = a.count;
            request.x_range = a.x_range.as_deref().map(peaks::parse_x_range).transpose()?;
            request.max_samples = a.max_samples;
            slice_trace(ds.as_mut(), &info, &request)
        },
        render_trace,
    )
}

fn fmt_num(v: Option<f64>) -> String {
    v.map_or_else(|| "-".into(), |x| format!("{x:.6}"))
}

fn render_trace(t: &TraceSlice) -> String {
    let mut s = format!(
        "{} ({}): trace {} sweep {}/{}  samples {}..{} of {}  {} Hz\n",
        t.path,
        t.format,
        t.trace,
        t.sweep,
        t.sweep_count,
        t.first_sample,
        t.first_sample + t.sample_count,
        t.sweep_sample_count,
        t.sample_rate_hz
    );
    if let Some(a) = &t.axis {
        let num = |k: &str| a.get(k).and_then(serde_json::Value::as_f64);
        if let (Some(first), Some(step)) = (num("first"), num("step")) {
            let x0 = first + step * t.first_sample as f64;
            let x1 = first + step * (t.first_sample + t.sample_count.saturating_sub(1)) as f64;
            s.push_str(&format!(
                "  axis: {} {x0:.6} .. {x1:.6} {} (step {step:.6})\n",
                a.get("quantity").and_then(|v| v.as_str()).unwrap_or("x"),
                a.get("unit").and_then(|v| v.as_str()).unwrap_or("")
            ));
        }
    }
    for c in &t.channels {
        let unit = c.unit.as_deref().unwrap_or("");
        s.push_str(&format!(
            "  ch{} {:<16} {:<6} min {}  max {}  mean {}  std {}\n",
            c.index,
            truncate(&c.name, 16),
            unit,
            fmt_num(c.stats.min),
            fmt_num(c.stats.max),
            fmt_num(c.stats.mean),
            fmt_num(c.stats.std)
        ));
        if let Some(i) = c.stats.argmax {
            let at = match (c.stats.argmax_axis_value, &t.axis, c.stats.argmax_time_s) {
                (Some(x), Some(a), _) => format!(
                    " = {} {x:.6} {}",
                    a.get("quantity").and_then(|v| v.as_str()).unwrap_or("x"),
                    a.get("unit").and_then(|v| v.as_str()).unwrap_or("")
                ),
                (_, _, Some(s)) => format!(" = {s:.6} s"),
                _ => String::new(),
            };
            s.push_str(&format!("      max at sample {i}{}\n", at.trim_end()));
        }
        let head: Vec<String> = c
            .samples
            .iter()
            .take(8)
            .map(|v| format!("{v:.6}"))
            .collect();
        s.push_str(&format!(
            "      first: {}{}\n",
            head.join(", "),
            if c.samples.len() > 8 { ", …" } else { "" }
        ));
    }
    if t.truncated {
        s.push_str("  (samples truncated; use --json with --max-samples, or `export --to csv`)\n");
    }
    s.trim_end().to_string()
}
