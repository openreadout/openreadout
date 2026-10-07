//! `export-dataset`: one command turns a slice of an index into an ML-ready dataset.
//!
//! For every data set a query matches: images become OME-Zarr stores, tables, traces and mass
//! spectra become Parquet (or CSV) files, and the metadata (`info` plus the experiment) a JSON
//! file. A datasheet (`datasheet.json` and `DATASHEET.md`, after "Datasheets for Datasets")
//! records the sources, formats, provenance, licences where known, experiment fields and
//! counts.
//!
//! Layout of `OUT/`:
//!
//! ```text
//! DATASHEET.md, datasheet.json
//! data/<id>.json                       info + experiment of the source
//! data/<id>.ome.zarr/                  images
//! data/<id>.table<N>.parquet|csv       tables (FCS events, plate reads, ...)
//! data/<id>.trace<N>.parquet|csv       traces, long form: sweep, abscissa, one column per channel
//! data/<id>.spectra<N>.parquet|csv     spectra, long form, one row per point (+ .scans.parquet)
//! .export/journal.jsonl                progress (a rerun skips what is done)
//! ```
//!
//! `<id>` is the source's stem plus 8 hex digits of its path hash. Parquet files are written by
//! `openreadout-arrow` (typed columns with units and provenance in the field metadata); CSV
//! files by this module. Every output is written under a temporary name, read back and compared
//! (Parquet: schema, metadata and every value; CSV: row count and an xxh3 digest of every value;
//! OME-Zarr stores are verified by their writer), then renamed into place. Sources are never
//! written. The export is resumable: completed data sets are recorded in the journal and
//! skipped on the next run when their outputs are still there with the same sizes.
//!
//! With redaction, values flagged as personal data ([`crate::pii`]) are replaced with stable
//! salted hashes in the metadata JSON, the datasheet and the OME-Zarr metadata files.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use openreadout_core::{Dataset, Error, InfoOutput, Registry, Result};
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::Xxh3;

use crate::crawl::{TOOL_VERSION, now_iso};
use crate::pii::{Finding, Redactor};
use crate::search::{SearchRequest, search};

/// File format for tables, traces and spectra.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TableFormat {
    /// Apache Parquet (Snappy).
    #[default]
    Parquet,
    /// Comma-separated values.
    Csv,
}

/// Options of `export-dataset`.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct ExportDatasetOptions {
    /// The query selecting data sets ([`crate::query`]).
    pub query: String,
    /// Output directory.
    pub output: PathBuf,
    /// Tables, traces and spectra as Parquet (default) or CSV.
    pub tables: TableFormat,
    /// Replace flagged personal data with salted hashes.
    pub redact: Option<Redactor>,
    /// Licence of the whole export (an SPDX id), overriding licence files found next to the
    /// sources.
    pub license: Option<String>,
    /// Most data sets exported (`None`: all matches).
    pub limit: Option<usize>,
    /// Skip images (tables, traces, spectra and metadata only).
    pub no_images: bool,
}

/// Progress callback of [`export_dataset`]: `(data sets done, total, current source path)`.
pub type ExportProgress<'a> = dyn Fn(u64, u64, &str) + Sync + 'a;

/// One written file.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutputFile {
    /// Path relative to the output directory.
    pub file: String,
    /// `metadata`, `image`, `table`, `trace` or `spectra`.
    pub kind: String,
    /// Rows (tables, traces, spectra) or planes (images).
    pub rows: u64,
    /// Columns written (tables, traces, spectra).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<String>,
    /// Bytes on disk.
    pub bytes: u64,
    /// xxh3-64 digest of every value written (rows in order), hex; empty for images.
    #[serde(default)]
    pub digest: String,
    /// Read back and compared.
    pub verified: bool,
}

/// The licence of a data set, when known.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LicenseInfo {
    /// SPDX id, when recognised (`CC-BY-4.0`, `CC0-1.0`, `MIT`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spdx: Option<String>,
    /// Where it came from: `user` (`--license`) or the licence file's path.
    pub source: String,
}

/// One exported data set.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExportedDataset {
    /// Output id (file-name prefix).
    pub id: String,
    /// Source path.
    pub source: String,
    /// Source format id.
    pub format: String,
    /// Source family.
    pub family: String,
    /// Source bytes.
    pub size_bytes: u64,
    /// Source content fingerprint (from the index).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// Images, planes, tables, table rows, traces, trace samples, spectra runs, spectra.
    pub counts: BTreeMap<String, u64>,
    /// Files written.
    pub outputs: Vec<OutputFile>,
    /// The experiment (redacted when asked), with provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<serde_json::Value>")]
    pub experiment: Option<serde_json::Value>,
    /// Licence, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<LicenseInfo>,
    /// Personal-data flags (field, kind, rule).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pii: Vec<crate::record::PiiFlag>,
    /// Values replaced by redaction.
    pub redacted_values: u64,
    /// Things to know (parts that could not be exported).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A data set that could not be exported.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SkippedDataset {
    /// Source path.
    pub source: String,
    /// Why.
    pub reason: String,
}

/// `datasheet.json`, and the payload of `export-dataset --json`.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct ExportDatasetReport {
    /// Datasheet schema version.
    pub schema_version: String,
    /// The output directory.
    pub output: String,
    /// When the export finished.
    pub created_at: String,
    /// Tool and version.
    pub tool: String,
    /// The query.
    pub query: String,
    /// The index it came from.
    pub index_dir: String,
    /// The index's roots.
    pub index_roots: Vec<String>,
    /// When the index was written.
    pub index_updated_at: String,
    /// Tables written as `parquet` or `csv`.
    pub table_format: TableFormat,
    /// Whether personal data was redacted, and how.
    pub redaction: String,
    /// Totals over the exported data sets.
    pub counts: BTreeMap<String, u64>,
    /// Data sets per source format.
    pub formats: BTreeMap<String, u64>,
    /// Data sets per licence (`unknown` when none was found).
    pub licenses: BTreeMap<String, u64>,
    /// Data sets exported in this run (the rest were already done).
    pub exported_now: u64,
    /// Data sets reused from an earlier, interrupted run.
    pub resumed: u64,
    /// Every exported data set.
    pub datasets: Vec<ExportedDataset>,
    /// Data sets that could not be exported.
    pub skipped: Vec<SkippedDataset>,
    /// Every output verified.
    pub verified: bool,
}

fn io(p: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::io(p, e)
}

/// Streams numeric columns to CSV with a digest of every value; verifies on finish. (Parquet
/// outputs are written by `openreadout-arrow`.)
struct Columnar {
    names: Vec<String>,
    ints: Vec<bool>,
    tmp: PathBuf,
    dest: PathBuf,
    rows: u64,
    digest: Xxh3,
    out: BufWriter<File>,
}

fn fmt_num(v: f64, int: bool) -> String {
    if !v.is_finite() {
        return String::new();
    }
    if int {
        format!("{}", v as u64)
    } else {
        format!("{v}")
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

impl Columnar {
    fn create(dest: &Path, names: Vec<String>, ints: Vec<bool>) -> Result<Self> {
        let tmp = dest.with_extension("csv.tmp");
        let file = File::create(&tmp).map_err(io(&tmp))?;
        let mut out = BufWriter::new(file);
        writeln!(
            out,
            "{}",
            names
                .iter()
                .map(|n| csv_escape(n))
                .collect::<Vec<_>>()
                .join(",")
        )
        .map_err(io(&tmp))?;
        Ok(Columnar {
            names,
            ints,
            tmp,
            dest: dest.to_path_buf(),
            rows: 0,
            digest: Xxh3::new(),
            out,
        })
    }

    fn push(&mut self, row: &[f64]) -> Result<()> {
        let mut line = String::new();
        for (i, v) in row.iter().enumerate() {
            let v = if self.ints[i] && v.is_finite() {
                v.trunc()
            } else {
                *v
            };
            self.digest.update(&v.to_bits().to_le_bytes());
            if i > 0 {
                line.push(',');
            }
            line.push_str(&fmt_num(v, self.ints[i]));
        }
        writeln!(self.out, "{line}").map_err(io(&self.tmp))?;
        self.rows += 1;
        Ok(())
    }

    /// Close, read back, compare, rename. Returns the output record (path relative to `base`).
    fn finish(mut self, base: &Path, kind: &str) -> Result<OutputFile> {
        self.out.flush().map_err(io(&self.tmp))?;
        let want = format!("{:016x}", self.digest.digest());
        let (rows, got) = read_back_csv(&self.tmp, self.names.len())?;
        if rows != self.rows || got != want {
            return Err(Error::Other(format!(
                "{}: verification failed ({rows} rows read back of {}, digest {got} vs {want})",
                self.tmp.display(),
                self.rows
            )));
        }
        std::fs::rename(&self.tmp, &self.dest).map_err(io(&self.dest))?;
        let bytes = std::fs::metadata(&self.dest).map_err(io(&self.dest))?.len();
        Ok(OutputFile {
            file: rel(base, &self.dest),
            kind: kind.into(),
            rows: self.rows,
            columns: self.names.clone(),
            bytes,
            digest: want,
            verified: true,
        })
    }
}

fn read_back_csv(p: &Path, ncols: usize) -> Result<(u64, String)> {
    let f = File::open(p).map_err(io(p))?;
    let mut h = Xxh3::new();
    let mut rows = 0u64;
    for (i, line) in BufReader::new(f).lines().enumerate() {
        let line = line.map_err(io(p))?;
        if i == 0 {
            continue;
        }
        let vals: Vec<&str> = line.split(',').collect();
        if vals.len() != ncols {
            return Err(Error::Other(format!(
                "{}: row {i} has {} fields, expected {ncols}",
                p.display(),
                vals.len()
            )));
        }
        for v in vals {
            let x: f64 = if v.is_empty() {
                f64::NAN
            } else {
                v.parse()
                    .map_err(|_| Error::Other(format!("{}: bad number {v}", p.display())))?
            };
            h.update(&x.to_bits().to_le_bytes());
        }
        rows += 1;
    }
    Ok((rows, format!("{:016x}", h.digest())))
}

/// Parquet outputs through `openreadout-arrow` (typed columns, units and provenance in the
/// field metadata, read back and compared value by value): one file per table, per trace (every
/// sweep, with a `sweep` column) and per spectra run (plus its per-scan summary file).
fn export_parquet(
    ds: &mut dyn Dataset,
    info: &openreadout_core::model::FileInfo,
    input: &Path,
    base: &Path,
    stem: &Path,
    counts: &mut BTreeMap<String, u64>,
) -> (Vec<OutputFile>, Vec<String>) {
    use openreadout_arrow::{ColumnarOptions, ColumnarSelection, export_columnar};
    let mut jobs: Vec<(&str, u32, ColumnarSelection)> = Vec::new();
    jobs.extend(
        info.tables
            .iter()
            .map(|t| ("table", t.index, ColumnarSelection::Table(t.index))),
    );
    jobs.extend(
        info.traces
            .iter()
            .map(|t| ("trace", t.index, ColumnarSelection::Trace(t.index))),
    );
    jobs.extend(
        info.spectra
            .iter()
            .map(|s| ("spectra", s.index, ColumnarSelection::Spectra(s.index))),
    );
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for (kind, index, select) in jobs {
        let dest = PathBuf::from(format!("{}.{kind}{index}.parquet", stem.display()));
        let mut opts = ColumnarOptions::default();
        opts.select = select;
        opts.overwrite = true;
        match export_columnar(ds, input, &dest, &opts) {
            Ok(r) => {
                match kind {
                    "table" => {
                        *counts.entry("tables".into()).or_default() += 1;
                        *counts.entry("table_rows".into()).or_default() += r.rows_written;
                    }
                    "trace" => {
                        *counts.entry("traces".into()).or_default() += 1;
                        *counts.entry("trace_samples".into()).or_default() += r.rows_written;
                    }
                    _ => {
                        *counts.entry("spectra_runs".into()).or_default() += 1;
                        *counts.entry("spectra".into()).or_default() +=
                            r.spectra_written.unwrap_or(0);
                    }
                }
                out.push(OutputFile {
                    file: rel(base, Path::new(&r.output)),
                    kind: kind.into(),
                    rows: r.rows_written,
                    columns: r.columns.clone(),
                    bytes: r.bytes_written,
                    digest: String::new(),
                    verified: r.verified,
                });
                if let (Some(s), Some(b)) = (&r.summary_output, r.summary_bytes) {
                    out.push(OutputFile {
                        file: rel(base, Path::new(s)),
                        kind: "spectra-scans".into(),
                        rows: r.spectra_written.unwrap_or(0),
                        columns: Vec::new(),
                        bytes: b,
                        digest: String::new(),
                        verified: r.verified,
                    });
                }
            }
            Err(e) => notes.push(format!("{kind} {index} not exported: {e}")),
        }
    }
    (out, notes)
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn sanitize(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let t = out.trim_matches('_');
    if t.is_empty() {
        "dataset".into()
    } else {
        t.chars().take(60).collect()
    }
}

/// Output id of a source path: its stem (sanitized) and 8 hex digits of its path hash.
pub fn dataset_id(path: &str) -> String {
    let name = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let h = xxhash_rust::xxh3::xxh3_64(path.as_bytes());
    format!(
        "{}-{:08x}",
        sanitize(&crate::formats::stem(&name)),
        h as u32
    )
}

const LICENSE_NAMES: &[&str] = &[
    "license",
    "licence",
    "copying",
    "license.txt",
    "licence.txt",
    "license.md",
    "licence.md",
];

/// Recognise an SPDX id in licence text.
pub fn spdx_of(text: &str) -> Option<String> {
    let t = text.to_lowercase();
    let table: &[(&str, &[&str])] = &[
        (
            "CC0-1.0",
            &["cc0 1.0", "cc0-1.0", "creative commons zero", "cc0"],
        ),
        (
            "CC-BY-4.0",
            &["cc-by-4.0", "cc by 4.0", "creative commons attribution 4.0"],
        ),
        (
            "CC-BY-SA-4.0",
            &["cc-by-sa-4.0", "attribution-sharealike 4.0"],
        ),
        (
            "CC-BY-NC-4.0",
            &["cc-by-nc-4.0", "attribution-noncommercial 4.0"],
        ),
        (
            "CC-BY-3.0",
            &["cc-by-3.0", "creative commons attribution 3.0"],
        ),
        ("ODbL-1.0", &["odbl", "open database license"]),
        (
            "Apache-2.0",
            &[
                "apache license, version 2.0",
                "apache-2.0",
                "apache license 2.0",
            ],
        ),
        (
            "MIT",
            &[
                "mit license",
                "permission is hereby granted, free of charge",
            ],
        ),
        (
            "BSD-3-Clause",
            &["bsd-3-clause", "neither the name of the copyright holder"],
        ),
        (
            "PDDL-1.0",
            &["pddl", "public domain dedication and license"],
        ),
    ];
    let mut best: Option<&str> = None;
    for (id, pats) in table {
        // more specific ids come first in the table except CC0; prefer the first hit
        if pats.iter().any(|p| t.contains(p)) {
            if *id == "CC-BY-4.0" && (t.contains("sharealike") || t.contains("noncommercial")) {
                continue;
            }
            best = Some(id);
            break;
        }
    }
    best.map(str::to_string)
}

/// The licence file nearest to `source` (its directory and ancestors up to `root`).
fn find_license(source: &Path, root: Option<&Path>) -> Option<LicenseInfo> {
    let mut dir = if source.is_dir() {
        Some(source)
    } else {
        source.parent()
    };
    while let Some(d) = dir {
        if let Ok(rd) = std::fs::read_dir(d) {
            let mut hits: Vec<PathBuf> = rd
                .filter_map(std::result::Result::ok)
                .filter(|e| {
                    LICENSE_NAMES.contains(&e.file_name().to_string_lossy().to_lowercase().as_str())
                })
                .map(|e| e.path())
                .collect();
            hits.sort();
            if let Some(p) = hits.first() {
                let text = std::fs::read(p)
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b[..b.len().min(65_536)]).into_owned())
                    .unwrap_or_default();
                return Some(LicenseInfo {
                    spdx: spdx_of(&text),
                    source: p.to_string_lossy().into_owned(),
                });
            }
        }
        if root.is_some_and(|r| d == r) {
            break;
        }
        dir = d.parent();
    }
    None
}

fn export_tables_csv(
    ds: &mut dyn Dataset,
    info: &openreadout_core::model::FileInfo,
    base: &Path,
    stem: &Path,
    counts: &mut BTreeMap<String, u64>,
) -> Result<Vec<OutputFile>> {
    let mut out = Vec::new();
    for t in &info.tables {
        let dest = PathBuf::from(format!("{}.table{}.csv", stem.display(), t.index));
        let mut names: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
        dedupe(&mut names);
        let ints = vec![false; names.len()];
        let mut w = Columnar::create(&dest, names, ints)?;
        let mut first = 0u64;
        while first < t.row_count {
            let chunk = ds.read_table(t.index, first, 65_536)?;
            let n = chunk.columns.first().map_or(0, Vec::len);
            if n == 0 {
                break;
            }
            let mut row = vec![0.0; chunk.columns.len()];
            for r in 0..n {
                for (c, col) in chunk.columns.iter().enumerate() {
                    row[c] = col.get(r).copied().unwrap_or(f64::NAN);
                }
                w.push(&row)?;
            }
            first += n as u64;
        }
        let o = w.finish(base, "table")?;
        *counts.entry("table_rows".into()).or_default() += o.rows;
        *counts.entry("tables".into()).or_default() += 1;
        out.push(o);
    }
    Ok(out)
}

fn dedupe(names: &mut [String]) {
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    for n in names.iter_mut() {
        let c = seen.entry(n.clone()).or_default();
        *c += 1;
        if *c > 1 {
            *n = format!("{n}_{c}");
        }
    }
}

fn export_traces_csv(
    ds: &mut dyn Dataset,
    info: &openreadout_core::model::FileInfo,
    base: &Path,
    stem: &Path,
    counts: &mut BTreeMap<String, u64>,
) -> Result<Vec<OutputFile>> {
    let mut out = Vec::new();
    for t in &info.traces {
        let dest = PathBuf::from(format!("{}.trace{}.csv", stem.display(), t.index));
        let mut names = vec![
            "sweep".to_string(),
            "sample".to_string(),
            "time_s".to_string(),
        ];
        names.extend(t.channels.iter().map(|c| match &c.unit {
            Some(u) if !u.is_empty() => format!("{} [{u}]", c.name),
            _ => c.name.clone(),
        }));
        dedupe(&mut names);
        let mut ints = vec![true, true, false];
        ints.extend(std::iter::repeat_n(false, t.channels.len()));
        let mut w = Columnar::create(&dest, names, ints)?;
        let rate = if t.sample_rate_hz > 0.0 {
            t.sample_rate_hz
        } else {
            f64::NAN
        };
        let start = t.start_s.unwrap_or(0.0);
        for sweep in 0..t.sweep_count.max(1) {
            let mut first = 0u64;
            loop {
                let chunk = ds.read_trace(t.index, sweep, first, 65_536)?;
                let n = chunk.channels.first().map_or(0, Vec::len);
                if n == 0 {
                    break;
                }
                let mut row = vec![f64::NAN; 3 + t.channels.len()];
                for i in 0..n {
                    let s = first + i as u64;
                    row[0] = f64::from(sweep);
                    row[1] = s as f64;
                    row[2] = start + s as f64 / rate;
                    for (c, col) in chunk.channels.iter().take(t.channels.len()).enumerate() {
                        row[3 + c] = col.get(i).copied().unwrap_or(f64::NAN);
                    }
                    w.push(&row)?;
                }
                first += n as u64;
                if first >= t.sample_count {
                    break;
                }
            }
        }
        let o = w.finish(base, "trace")?;
        *counts.entry("trace_samples".into()).or_default() += o.rows;
        *counts.entry("traces".into()).or_default() += 1;
        out.push(o);
    }
    Ok(out)
}

fn export_spectra_csv(
    ds: &mut dyn Dataset,
    info: &openreadout_core::model::FileInfo,
    base: &Path,
    stem: &Path,
    counts: &mut BTreeMap<String, u64>,
) -> Result<Vec<OutputFile>> {
    let mut out = Vec::new();
    for run in &info.spectra {
        let dest = PathBuf::from(format!("{}.spectra{}.csv", stem.display(), run.index));
        let names: Vec<String> = [
            "spectrum",
            "scan",
            "rt_s",
            "ms_level",
            "precursor_mz",
            "mz",
            "intensity",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let ints = vec![true, true, false, true, false, false, false];
        let mut w = Columnar::create(&dest, names, ints)?;
        let n = run.scan_count;
        for i in 0..n {
            let sp = ds.read_spectrum(run.index, i)?;
            for (mz, inten) in sp.mz.iter().zip(&sp.intensity) {
                w.push(&[
                    i as f64,
                    sp.scan_number as f64,
                    sp.rt_s.unwrap_or(f64::NAN),
                    f64::from(sp.ms_level),
                    sp.precursor_mz.unwrap_or(f64::NAN),
                    *mz,
                    f64::from(*inten),
                ])?;
            }
        }
        let o = w.finish(base, "spectra")?;
        *counts.entry("spectra".into()).or_default() += n;
        *counts.entry("spectra_runs".into()).or_default() += 1;
        out.push(o);
    }
    Ok(out)
}

/// Replace flagged values in the text metadata files of an OME-Zarr store. Returns the number
/// of files changed.
fn redact_zarr_metadata(store: &Path, red: &Redactor, findings: &[Finding]) -> Result<u64> {
    let mut changed = 0;
    let values: Vec<&str> = findings
        .iter()
        .map(|f| f.value.trim())
        .filter(|v| v.chars().count() >= 2)
        .collect();
    if values.is_empty() {
        return Ok(0);
    }
    let mut stack = vec![store.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(std::result::Result::ok) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            let is_meta = name == "zarr.json"
                || name == ".zattrs"
                || name == ".zgroup"
                || name == ".zarray"
                || Path::new(&name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("xml"));
            if !is_meta {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let mut out = text.clone();
            for v in &values {
                let rep = red.replacement(v);
                let json_escaped = serde_json::to_string(v).unwrap_or_default();
                let json_inner = json_escaped.trim_matches('"');
                let xml = v
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;")
                    .replace('"', "&quot;");
                for form in [v.to_string(), json_inner.to_string(), xml] {
                    if !form.is_empty() && out.contains(&form) {
                        out = out.replace(&form, &rep);
                    }
                }
            }
            if out != text {
                if Path::new(&name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json"))
                    || name.starts_with(".z")
                {
                    serde_json::from_str::<serde_json::Value>(&out).map_err(|e| {
                        Error::Other(format!("{}: redaction broke the JSON: {e}", p.display()))
                    })?;
                }
                let tmp = p.with_extension("redact.tmp");
                std::fs::write(&tmp, &out).map_err(io(&tmp))?;
                std::fs::rename(&tmp, &p).map_err(io(&p))?;
                changed += 1;
            }
        }
    }
    Ok(changed)
}

fn dir_size(p: &Path) -> u64 {
    if p.is_file() {
        return std::fs::metadata(p).map_or(0, |m| m.len());
    }
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.filter_map(std::result::Result::ok) {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(_) => total += e.metadata().map_or(0, |m| m.len()),
                Err(_) => {}
            }
        }
    }
    total
}

struct Source {
    path: String,
    format: String,
    family: String,
    size: u64,
    fingerprint: Option<String>,
    root: Option<String>,
}

fn export_one(
    reg: &Registry,
    src: &Source,
    out_dir: &Path,
    opts: &ExportDatasetOptions,
) -> Result<ExportedDataset> {
    let id = dataset_id(&src.path);
    let data = out_dir.join("data");
    let stem = data.join(&id);
    let path = Path::new(&src.path);
    let (_, mut ds) = reg.open(path)?;
    let info = ds.info()?;
    let experiment = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
    let mut doc = serde_json::to_value(InfoOutput {
        file: info.clone(),
        experiment: (!experiment.is_empty()).then_some(experiment),
        acquisition: None,
        plate: None,
        images_total: None,
        assurance: Some(openreadout_core::assurance::assess_dataset(
            ds.as_ref(),
            &info,
        )),
    })
    .map_err(|e| Error::Other(e.to_string()))?;
    let findings = crate::pii::scan(&doc);
    let mut redacted = 0u64;
    if let Some(r) = &opts.redact {
        redacted += r.redact(&mut doc, &findings) as u64;
    }
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut outputs = Vec::new();
    let mut notes = Vec::new();
    // Metadata.
    let meta_path = PathBuf::from(format!("{}.json", stem.display()));
    let tmp = meta_path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(&doc).map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(&tmp, &text).map_err(io(&tmp))?;
    let back: serde_json::Value = serde_json::from_slice(&std::fs::read(&tmp).map_err(io(&tmp))?)
        .map_err(|e| Error::Other(e.to_string()))?;
    if back != doc {
        return Err(Error::Other(format!(
            "{}: metadata did not read back",
            tmp.display()
        )));
    }
    std::fs::rename(&tmp, &meta_path).map_err(io(&meta_path))?;
    outputs.push(OutputFile {
        file: rel(out_dir, &meta_path),
        kind: "metadata".into(),
        rows: 1,
        columns: Vec::new(),
        bytes: text.len() as u64,
        digest: format!("{:016x}", xxhash_rust::xxh3::xxh3_64(text.as_bytes())),
        verified: true,
    });
    // Images.
    if !info.images.is_empty() {
        *counts.entry("images".into()).or_default() += info.images.len() as u64;
        *counts.entry("planes".into()).or_default() += info.plane_count;
        if opts.no_images {
            notes.push("images not exported (--no-images)".into());
        } else {
            let store = PathBuf::from(format!("{}.ome.zarr", stem.display()));
            let mut zo = openreadout_omezarr::ZarrExportOptions::default();
            zo.overwrite = true;
            match openreadout_omezarr::export_ome_zarr(ds.as_mut(), path, &store, &zo) {
                Ok(rep) => {
                    if let Some(r) = &opts.redact {
                        let n = redact_zarr_metadata(&store, r, &findings)?;
                        redacted += n;
                    }
                    outputs.push(OutputFile {
                        file: rel(out_dir, &store),
                        kind: "image".into(),
                        rows: rep.planes_written,
                        columns: Vec::new(),
                        bytes: dir_size(&store),
                        digest: String::new(),
                        verified: rep.verified,
                    });
                }
                Err(e) => notes.push(format!("images not exported: {e}")),
            }
        }
    }
    if opts.tables == TableFormat::Parquet {
        let (files, why) = export_parquet(ds.as_mut(), &info, path, out_dir, &stem, &mut counts);
        outputs.extend(files);
        notes.extend(why);
    } else {
        let mut part = |what: &str, r: Result<Vec<OutputFile>>| match r {
            Ok(v) => outputs.extend(v),
            Err(e) => notes.push(format!("{what} not exported: {e}")),
        };
        if !info.tables.is_empty() {
            let r = export_tables_csv(ds.as_mut(), &info, out_dir, &stem, &mut counts);
            part("tables", r);
        }
        if !info.traces.is_empty() {
            let r = export_traces_csv(ds.as_mut(), &info, out_dir, &stem, &mut counts);
            part("traces", r);
        }
        if !info.spectra.is_empty() {
            let r = export_spectra_csv(ds.as_mut(), &info, out_dir, &stem, &mut counts);
            part("spectra", r);
        }
    }
    let license = match &opts.license {
        Some(l) => Some(LicenseInfo {
            spdx: Some(l.clone()),
            source: "user".into(),
        }),
        None => find_license(path, src.root.as_deref().map(Path::new)),
    };
    Ok(ExportedDataset {
        id,
        source: src.path.clone(),
        format: src.format.clone(),
        family: src.family.clone(),
        size_bytes: src.size,
        fingerprint: src.fingerprint.clone(),
        counts,
        outputs,
        experiment: doc.get("experiment").cloned(),
        license,
        pii: findings.iter().map(Finding::flag).collect(),
        redacted_values: redacted,
        notes,
    })
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
enum JLine {
    Done(Box<ExportedDataset>),
    Skipped(SkippedDataset),
}

fn outputs_present(out_dir: &Path, d: &ExportedDataset) -> bool {
    d.outputs.iter().all(|o| {
        let p = out_dir.join(&o.file);
        if o.kind == "image" {
            p.is_dir() && dir_size(&p) == o.bytes
        } else {
            std::fs::metadata(&p).is_ok_and(|m| m.len() == o.bytes)
        }
    })
}

/// Export the data sets of `index_dir` matching `opts.query` into `opts.output`.
pub fn export_dataset(
    reg: &Registry,
    index_dir: &Path,
    opts: &ExportDatasetOptions,
    progress: Option<&ExportProgress<'_>>,
) -> Result<ExportDatasetReport> {
    let manifest = crate::manifest::read_manifest(index_dir)?;
    let mut req = SearchRequest::default();
    req.query.clone_from(&opts.query);
    req.limit = Some(opts.limit.unwrap_or(0));
    req.fields = [
        "path",
        "format",
        "family",
        "size_bytes",
        "fingerprint",
        "root",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    let hits = search(index_dir, &req)?;
    let out_dir = &opts.output;
    if out_dir.starts_with(index_dir) {
        return Err(Error::Usage(
            "the output directory must not be inside the index directory".into(),
        ));
    }
    std::fs::create_dir_all(out_dir.join("data")).map_err(io(out_dir))?;
    let state = out_dir.join(".export");
    std::fs::create_dir_all(&state).map_err(io(&state))?;
    let jpath = state.join("journal.jsonl");
    let mut done: BTreeMap<String, ExportedDataset> = BTreeMap::new();
    if let Ok(f) = File::open(&jpath) {
        for line in BufReader::new(f).lines().map_while(std::result::Result::ok) {
            if let Ok(JLine::Done(d)) = serde_json::from_str::<JLine>(&line) {
                done.insert(d.source.clone(), *d);
            }
        }
    }
    let mut journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&jpath)
        .map_err(io(&jpath))?;
    let str_of = |m: &serde_json::Map<String, serde_json::Value>, k: &str| {
        m.get(k).and_then(|v| v.as_str()).map(str::to_string)
    };
    let total = hits.results.len() as u64;
    let mut datasets = Vec::new();
    let mut skipped = Vec::new();
    let (mut now, mut resumed) = (0u64, 0u64);
    for (i, h) in hits.results.iter().enumerate() {
        let src = Source {
            path: str_of(h, "path").unwrap_or_default(),
            format: str_of(h, "format").unwrap_or_default(),
            family: str_of(h, "family").unwrap_or_default(),
            size: h
                .get("size_bytes")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            fingerprint: str_of(h, "fingerprint"),
            root: str_of(h, "root"),
        };
        if let Some(p) = progress {
            p(i as u64, total, &src.path);
        }
        if let Some(d) = done.get(&src.path)
            && d.fingerprint == src.fingerprint
            && outputs_present(out_dir, d)
        {
            datasets.push(d.clone());
            resumed += 1;
            continue;
        }
        match export_one(reg, &src, out_dir, opts) {
            Ok(d) => {
                let line = serde_json::to_string(&JLine::Done(Box::new(d.clone())))
                    .map_err(|e| Error::Other(e.to_string()))?;
                writeln!(journal, "{line}").map_err(io(&jpath))?;
                journal.sync_data().map_err(io(&jpath))?;
                datasets.push(d);
                now += 1;
            }
            Err(e) => {
                let s = SkippedDataset {
                    source: src.path.clone(),
                    reason: e.to_string(),
                };
                let line = serde_json::to_string(&JLine::Skipped(s.clone()))
                    .map_err(|e| Error::Other(e.to_string()))?;
                writeln!(journal, "{line}").map_err(io(&jpath))?;
                skipped.push(s);
            }
        }
    }
    if let Some(p) = progress {
        p(total, total, "");
    }
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut formats: BTreeMap<String, u64> = BTreeMap::new();
    let mut licenses: BTreeMap<String, u64> = BTreeMap::new();
    let mut verified = true;
    for d in &datasets {
        for (k, v) in &d.counts {
            *counts.entry(k.clone()).or_default() += v;
        }
        *counts.entry("datasets".into()).or_default() += 1;
        *counts.entry("source_bytes".into()).or_default() += d.size_bytes;
        *counts.entry("bytes_written".into()).or_default() +=
            d.outputs.iter().map(|o| o.bytes).sum::<u64>();
        *counts.entry("files_written".into()).or_default() += d.outputs.len() as u64;
        *formats.entry(d.format.clone()).or_default() += 1;
        let l = d.license.as_ref().map_or("unknown".to_string(), |l| {
            l.spdx
                .clone()
                .unwrap_or_else(|| "unrecognised licence file".into())
        });
        *licenses.entry(l).or_default() += 1;
        verified &= d.outputs.iter().all(|o| o.verified);
    }
    let report = ExportDatasetReport {
        schema_version: "1".into(),
        output: out_dir.to_string_lossy().into_owned(),
        created_at: now_iso(),
        tool: format!("openreadout {TOOL_VERSION}"),
        query: opts.query.clone(),
        index_dir: index_dir.to_string_lossy().into_owned(),
        index_roots: manifest.roots.clone(),
        index_updated_at: manifest.updated_at.clone(),
        table_format: opts.tables,
        redaction: if opts.redact.is_some() {
            "personal data replaced by `redacted:` + 16 hex digits of HMAC-SHA256(user salt, value)"
                .into()
        } else {
            "none".into()
        },
        counts,
        formats,
        licenses,
        exported_now: now,
        resumed,
        datasets,
        skipped,
        verified,
    };
    let json = serde_json::to_string_pretty(&report).map_err(|e| Error::Other(e.to_string()))?;
    write_atomic(&out_dir.join("datasheet.json"), json.as_bytes())?;
    write_atomic(
        &out_dir.join("DATASHEET.md"),
        render_datasheet(&report).as_bytes(),
    )?;
    Ok(report)
}

fn write_atomic(p: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(io(&tmp))?;
    if std::fs::read(&tmp).map_err(io(&tmp))? != bytes {
        return Err(Error::Other(format!(
            "{}: did not read back",
            tmp.display()
        )));
    }
    std::fs::rename(&tmp, p).map_err(io(p))
}

fn exp_str(e: Option<&serde_json::Value>, ptr: &str) -> Option<String> {
    e?.pointer(ptr)?.as_str().map(str::to_string)
}

/// `DATASHEET.md`.
pub fn render_datasheet(r: &ExportDatasetReport) -> String {
    use std::fmt::Write as _;
    let mut o = String::new();
    let _ = writeln!(o, "# Datasheet\n");
    let _ = writeln!(
        o,
        "Exported by {} on {} from the index `{}` (updated {}).\n",
        r.tool, r.created_at, r.index_dir, r.index_updated_at
    );
    let _ = writeln!(o, "## Motivation and selection\n");
    let _ = writeln!(
        o,
        "Data sets matching the query `{}` over {}.\n",
        if r.query.is_empty() {
            "(everything)"
        } else {
            &r.query
        },
        r.index_roots
            .iter()
            .map(|x| format!("`{x}`"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(o, "## Composition\n");
    let _ = writeln!(o, "| | |\n| --- | ---: |");
    for (k, v) in &r.counts {
        let _ = writeln!(o, "| {} | {v} |", k.replace('_', " "));
    }
    let _ = writeln!(
        o,
        "\nSource formats: {}.\n",
        r.formats
            .iter()
            .map(|(k, v)| format!("{k} ({v})"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut techniques: BTreeMap<String, u64> = BTreeMap::new();
    let mut instruments: BTreeMap<String, u64> = BTreeMap::new();
    let mut dates: Vec<String> = Vec::new();
    let mut operators = 0u64;
    for d in &r.datasets {
        let e = d.experiment.as_ref();
        if let Some(t) = exp_str(e, "/method/technique/label") {
            *techniques.entry(t).or_default() += 1;
        }
        if let Some(m) =
            exp_str(e, "/instrument/model").or_else(|| exp_str(e, "/instrument/vendor"))
        {
            *instruments.entry(m).or_default() += 1;
        }
        if let Some(s) = exp_str(e, "/acquisition/started_at") {
            dates.push(s);
        }
        if exp_str(e, "/acquisition/operator").is_some() {
            operators += 1;
        }
    }
    dates.sort();
    let _ = writeln!(o, "## Collection\n");
    let list = |m: &BTreeMap<String, u64>| {
        m.iter()
            .map(|(k, v)| format!("{k} ({v})"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(
        o,
        "- Techniques: {}",
        if techniques.is_empty() {
            "not recorded".into()
        } else {
            list(&techniques)
        }
    );
    let _ = writeln!(
        o,
        "- Instruments: {}",
        if instruments.is_empty() {
            "not recorded".into()
        } else {
            list(&instruments)
        }
    );
    match (dates.first(), dates.last()) {
        (Some(a), Some(b)) => {
            let _ = writeln!(
                o,
                "- Acquired: {a} to {b} ({} of {} data sets record a date)",
                dates.len(),
                r.datasets.len()
            );
        }
        _ => {
            let _ = writeln!(o, "- Acquired: not recorded");
        }
    }
    let _ = writeln!(
        o,
        "- Operator recorded in {operators} data sets (see Personal data)\n"
    );
    let _ = writeln!(o, "## Preprocessing\n");
    let _ = writeln!(
        o,
        "Images were converted to OME-Zarr (OME-NGFF 0.5); tables, traces and mass spectra to {} (long form for traces and spectra; values as the reader returns them: raw table values, traces in physical units). No pixel or sample values were changed. Every output was read back and compared before it was renamed into place ({}).\n",
        match r.table_format {
            TableFormat::Parquet => "Parquet",
            TableFormat::Csv => "CSV",
        },
        if r.verified {
            "all verified"
        } else {
            "**some outputs not verified**"
        }
    );
    let _ = writeln!(o, "## Provenance\n");
    let _ = writeln!(
        o,
        "Each data set's `data/<id>.json` holds the source's normalized metadata and experiment, with a `provenance` map naming the field every value came from. `datasheet.json` lists each source path, format, size and content fingerprint.\n"
    );
    let _ = writeln!(
        o,
        "| id | source | format | outputs |\n| --- | --- | --- | --- |"
    );
    for d in &r.datasets {
        let _ = writeln!(
            o,
            "| {} | `{}` | {} | {} |",
            d.id,
            d.source.replace('|', "\\|"),
            d.format,
            d.outputs
                .iter()
                .map(|x| format!("`{}`", x.file))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    let _ = writeln!(o, "\n## Licences\n");
    let _ = writeln!(
        o,
        "{}\n",
        r.licenses
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(" · ")
    );
    let _ = writeln!(
        o,
        "A licence is taken from `--license`, or from the nearest `LICENSE`/`LICENCE`/`COPYING` file next to the source; `unknown` means none was found: ask the data owner before sharing.\n"
    );
    let flagged = r.datasets.iter().filter(|d| !d.pii.is_empty()).count();
    let _ = writeln!(o, "## Personal data\n");
    let _ = writeln!(
        o,
        "{flagged} of {} data sets have fields flagged as possible personal data. Redaction: {}.\n",
        r.datasets.len(),
        r.redaction
    );
    if !r.skipped.is_empty() {
        let _ = writeln!(o, "## Not exported\n");
        for s in &r.skipped {
            let _ = writeln!(o, "- `{}`: {}", s.source, s.reason);
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_licences() {
        let a = dataset_id("/data/run 1/Cells_01.ome.tiff");
        assert!(
            a.starts_with("cells_01-") && a.len() == "cells_01-".len() + 8,
            "{a}"
        );
        assert_ne!(a, dataset_id("/data/run 2/Cells_01.ome.tiff"));
        assert_eq!(
            spdx_of("This work is licensed under CC BY 4.0").as_deref(),
            Some("CC-BY-4.0")
        );
        assert_eq!(spdx_of("MIT License\n\nCopyright").as_deref(), Some("MIT"));
        assert_eq!(spdx_of("all rights reserved"), None);
    }

    #[test]
    fn csv_columns_round_trip() {
        let d = tempfile::tempdir().unwrap();
        {
            let dest = d.path().join("t.csv");
            let mut w =
                Columnar::create(&dest, vec!["i".into(), "x,y".into()], vec![true, false]).unwrap();
            for i in 0..20_000u32 {
                w.push(&[f64::from(i), f64::from(i) * 0.1 + 1e-7]).unwrap();
            }
            w.push(&[20_000.0, f64::NAN]).unwrap();
            let o = w.finish(d.path(), "table").unwrap();
            assert_eq!(o.rows, 20_001);
            assert!(o.verified);
            assert!(dest.exists());
        }
    }
}
