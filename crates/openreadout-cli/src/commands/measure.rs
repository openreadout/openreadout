//! `batch MEASURE PATHS…`: any measure — `stats`, `trace`, `table`, `info`, `scans`, or an
//! analysis (`peaks`, `chromatogram`, `assay`, `nmr-peaks`, `ephys-features`, `spikes`, `qpcr`,
//! `gate`) — over many files as one tidy table, with sample sheets and `--by` summaries.
//! Options are named as in MCP (`--set mz=[195.0877] --set ppm=10`), exactly as
//! `openreadout_batch` takes them.

use std::path::PathBuf;

use openreadout_batch::measures::{build, spec_from_options};
use openreadout_core::{Error, Registry, Result};
use serde_json::{Map, Value};

use super::batch::BatchArgs;
use super::tidy::TidyArgs;

/// Arguments of `batch`.
#[derive(Debug, clap::Args)]
pub struct MeasureArgs {
    /// What to compute per data set: stats (`--set per=well` for plates), trace, table, info,
    /// scans (one row per MS scan header), or an analysis: peaks, chromatogram, assay,
    /// nmr-peaks, ephys-features, spikes, qpcr, gate. To summarize a saved table, use
    /// `summarize`.
    #[arg(value_name = "MEASURE")]
    pub measure: String,
    /// Files, directories (with -r) or glob patterns.
    #[arg(required_unless_present = "from_index", value_name = "PATH")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: BatchArgs,
    /// A measure option, `KEY=VALUE`, repeatable: the MCP tools' argument names (`mz=[195.0877]`,
    /// `ppm=10`, `analysis=curve`, `from=fid`, `rows=compound`). VALUE is read as JSON when it
    /// parses (numbers, true, [..], {..}), else as text.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    pub set: Vec<String>,
    /// All options as one JSON object (merged under --set).
    #[arg(long, value_name = "JSON")]
    pub options: Option<String>,
    #[arg(long)]
    pub json: bool,
    #[command(flatten)]
    pub tidy: TidyArgs,
}

/// The options object from `--options` and `--set`.
pub fn options(a: &MeasureArgs) -> Result<Map<String, Value>> {
    let mut m = match &a.options {
        None => Map::new(),
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => m,
            Ok(_) => return Err(Error::Usage("--options must be a JSON object".into())),
            Err(e) => return Err(Error::Usage(format!("--options: {e}"))),
        },
    };
    for s in &a.set {
        let Some((k, v)) = s.split_once('=') else {
            return Err(Error::Usage(format!("--set {s:?}: write KEY=VALUE")));
        };
        let k = k.trim();
        if k.is_empty() {
            return Err(Error::Usage(format!("--set {s:?}: empty key")));
        }
        let value = serde_json::from_str::<Value>(v.trim())
            .unwrap_or_else(|_| Value::String(v.to_string()));
        m.insert(k.to_string(), value);
    }
    Ok(m)
}

pub fn run(reg: &Registry, a: &MeasureArgs) -> i32 {
    let measure = options(a)
        .and_then(|o| spec_from_options(&a.measure, o))
        .and_then(|spec| build(&spec));
    let m = match measure {
        Ok(m) => m,
        Err(e) => return crate::output::fail(a.json, &e),
    };
    let mut tidy = a.tidy.clone();
    tidy.tidy = true;
    super::tidy::run(reg, m.as_ref(), &a.files, &a.batch, &tidy, a.json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(flatten)]
        a: MeasureArgs,
    }

    #[test]
    fn set_values_are_json_or_text() {
        let t = T::parse_from([
            "x",
            "peaks",
            "dir",
            "--set",
            "mz=[195.0877]",
            "--set",
            "baseline=valley",
            "--set",
            "ppm=10",
            "--options",
            r#"{"area_seconds": true}"#,
        ]);
        let o = options(&t.a).unwrap();
        assert_eq!(o["mz"], serde_json::json!([195.0877]));
        assert_eq!(o["baseline"], serde_json::json!("valley"));
        assert_eq!(o["ppm"], serde_json::json!(10));
        assert_eq!(o["area_seconds"], serde_json::json!(true));
    }
}
