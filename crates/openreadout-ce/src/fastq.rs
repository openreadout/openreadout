//! FASTQ export of base calls: one record per base-call table (the calls of a Sanger read),
//! qualities as Phred+33. Works on any data set whose table has `extra.kind = "base_calls"` with
//! `base` (text) and `quality` columns.

use std::path::{Path, PathBuf};

use openreadout_core::model::{FileInfo, TableInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Output of `export --format fastq`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct FastqExportReport {
    /// The input file.
    pub input: String,
    /// The FASTQ file written.
    pub output: String,
    /// Always `fastq`.
    pub format: String,
    /// Records written (one per read).
    pub reads: u32,
    /// Bases written.
    pub bases: u64,
    /// The record names.
    pub names: Vec<String>,
    /// True once the file was read back and matched.
    pub verified: bool,
    /// Anything the caller should know.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// `<input stem>.fastq` next to `input`.
pub fn default_fastq_output(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
    input.with_file_name(format!("{stem}.fastq"))
}

/// The base-call tables of a file.
pub fn base_call_tables(info: &FileInfo) -> Vec<&TableInfo> {
    info.tables
        .iter()
        .filter(|t| t.extra.get("kind").and_then(Value::as_str) == Some("base_calls"))
        .collect()
}

/// A record name: no whitespace (FASTQ ends the name at the first blank).
fn record_name(s: &str) -> String {
    let n: String = s
        .trim()
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect();
    if n.is_empty() { "read".into() } else { n }
}

/// Write the base calls of `ds` to `output` as FASTQ (Phred+33), verified by reading back.
/// Only the basecaller's calls (the first base-call table) are written.
pub fn export_fastq(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    overwrite: bool,
) -> Result<FastqExportReport> {
    let info = ds.info()?;
    let tables = base_call_tables(&info);
    let Some(t) = tables.first() else {
        return Err(Error::unsupported(
            "export",
            format!(
                "FASTQ export of a {} file without base calls",
                info.format.name
            ),
            "FASTQ holds base calls: export a Sanger read (.ab1). Fragment-analysis runs and traces export as CSV or Parquet.",
        ));
    };
    let rows = ds.read_table(t.index, 0, t.row_count)?;
    let col = |name: &str| t.columns.iter().position(|c| c.name == name);
    let b =
        col("base").ok_or_else(|| Error::Other("base-call table without a base column".into()))?;
    let cats: Vec<String> = t.columns[b]
        .extra
        .get("categories")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let seq: String = rows.columns[b]
        .iter()
        .map(|c| cats.get(*c as usize).map_or("N", String::as_str))
        .collect();
    let mut notes = Vec::new();
    let qual: String = match col("quality") {
        Some(q) => rows.columns[q]
            .iter()
            .map(|v| {
                let v = if v.is_finite() {
                    v.clamp(0.0, 93.0)
                } else {
                    0.0
                };
                char::from(33 + v.round() as u8)
            })
            .collect(),
        None => {
            notes.push(
                "the file has no quality values; every base is written with quality 0".into(),
            );
            "!".repeat(seq.chars().count())
        }
    };
    if qual.bytes().all(|c| c == b'!') && !seq.is_empty() {
        notes.push("every quality value is 0 (the basecaller gave none)".into());
    }
    let e = openreadout_core::experiment::of_dataset(ds, &info);
    let name = record_name(
        &e.sample
            .as_ref()
            .and_then(|s| s.name.clone())
            .or_else(|| input.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_default(),
    );
    let text = format!("@{name}\n{seq}\n+\n{qual}\n");
    openreadout_core::output::write_verified(output, text.as_bytes(), overwrite, &[input])?;
    if tables.len() > 1 {
        notes.push(
            "the file also holds edited base calls; the basecaller's calls were written".into(),
        );
    }
    Ok(FastqExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: "fastq".into(),
        reads: 1,
        bases: seq.chars().count() as u64,
        names: vec![name],
        verified: true,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_have_no_blanks() {
        assert_eq!(record_name(" my read 1 "), "my_read_1");
        assert_eq!(record_name(""), "read");
        assert_eq!(
            default_fastq_output(Path::new("/a/b/x.ab1")),
            PathBuf::from("/a/b/x.fastq")
        );
    }
}
