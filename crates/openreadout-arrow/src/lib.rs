//! Parquet and Arrow IPC export of the non-image data of any OpenReadout dataset.
//!
//! - **Tables** (FCS events, spike and event tables, plate reads, chromatography peak tables):
//!   one column per table column in its stored type (`uint16`, `float32`, ...); plate `well`
//!   columns become a dictionary of well names.
//! - **Traces** (electrophysiology sweeps, NMR FIDs and spectra, chromatograms): one row per
//!   sample, a `sweep` column, the abscissa (`time_s`, or the spectrum's axis such as
//!   `chemical_shift_ppm`) and one `float64` column per channel in physical units.
//! - **Spectra** (mass spectrometry): long form, one row per point
//!   (`scan, rt_s, ms_level, precursor_mz, mz, intensity`), plus a per-scan summary written to a
//!   second file (`<name>.scans.parquet`).
//!
//! Every column carries its unit (`unit`), label and the provenance of the normalized fields it
//! comes from in its field metadata; the file carries the source format, the instrument and a
//! JSON copy of `info` (keys in [`meta`]). The file is written under a temporary name, read back
//! through the Parquet or Arrow IPC reader and compared column by column (schema, metadata and
//! an xxh3 digest of every value), and only then renamed into place.
//!
//! Compression is pure Rust: Snappy (Parquet default) or LZ4; Arrow IPC files are uncompressed
//! unless LZ4 is asked for.
//!
//! # Example
//!
//! ```
//! use std::path::Path;
//!
//! use openreadout_arrow::{ColumnarFormat, ColumnarOptions, export_columnar};
//! use openreadout_core::{Dataset, Result};
//!
//! /// Write table 0 (e.g. the events of an FCS file) as Parquet.
//! fn to_parquet(dataset: &mut dyn Dataset, input: &Path, output: &Path) -> Result<()> {
//!     let mut options = ColumnarOptions::default();
//!     options.format = ColumnarFormat::Parquet;
//!     let report = export_columnar(dataset, input, output, &options)?;
//!     println!("{} rows, {} columns, verified={}", report.rows_written, report.columns.len(), report.verified);
//!     Ok(())
//! }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod meta;
mod source;
mod verify;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

pub use arrow_array;
use arrow_array::RecordBatch;
pub use arrow_schema;
use arrow_schema::SchemaRef;
use openreadout_core::model::FileInfo;
use openreadout_core::{Dataset, Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use source::{BatchSource, SpectraSource, TableSource, TraceSource};
use verify::Digest;

/// The container written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ColumnarFormat {
    /// Apache Parquet (`.parquet`).
    #[default]
    Parquet,
    /// Arrow IPC file format, also known as Feather v2 (`.arrow`).
    ArrowIpc,
}

impl ColumnarFormat {
    /// The name used in reports and on the command line: `parquet` or `arrow`.
    pub fn id(self) -> &'static str {
        match self {
            ColumnarFormat::Parquet => "parquet",
            ColumnarFormat::ArrowIpc => "arrow",
        }
    }
    /// File extension without the dot.
    pub fn extension(self) -> &'static str {
        self.id()
    }
}

/// Column compression. Every codec here is implemented in pure Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ColumnarCompression {
    /// No compression.
    None,
    /// Snappy (Parquet only; the Parquet default).
    Snappy,
    /// LZ4 (Parquet `LZ4_RAW`, Arrow IPC `LZ4_FRAME`).
    Lz4,
}

impl ColumnarCompression {
    /// Lowercase name for reports.
    pub fn id(self) -> &'static str {
        match self {
            ColumnarCompression::None => "none",
            ColumnarCompression::Snappy => "snappy",
            ColumnarCompression::Lz4 => "lz4",
        }
    }
}

/// Which part of the file to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ColumnarSelection {
    /// The spectra of run 0 when the file holds spectra and no images, else table 0, else
    /// trace 0.
    #[default]
    Auto,
    /// Table `N` (see `info` → `tables[]`).
    Table(u32),
    /// Trace `N` (see `info` → `traces[]`).
    Trace(u32),
    /// The spectra of run `N` (see `info` → `spectra[]`).
    Spectra(u32),
}

/// What to write.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ColumnarOptions {
    /// Parquet or Arrow IPC.
    pub format: ColumnarFormat,
    /// `None`: Snappy for Parquet, no compression for Arrow IPC.
    pub compression: Option<ColumnarCompression>,
    /// The table, trace or spectra run.
    pub select: ColumnarSelection,
    /// Traces: only this sweep (default: every sweep, with a `sweep` column).
    pub sweep: Option<u32>,
    /// Tables: rows `[first, last]`; traces: samples `[first, last]` of each sweep (zero-based,
    /// inclusive; `None` = to the end).
    pub rows: Option<(u64, Option<u64>)>,
    /// Spectra: the instrument's centroid lists instead of profiles where a scan has both.
    pub centroid: bool,
    /// Replace an existing output file.
    pub overwrite: bool,
}

/// Output of a Parquet or Arrow IPC export.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ColumnarExportReport {
    /// The input file.
    pub input: String,
    /// The file written.
    pub output: String,
    /// `parquet` or `arrow`.
    pub format: String,
    /// What was exported: `table`, `trace` or `spectra`.
    pub kind: String,
    /// Table, trace or spectra-run index.
    pub index: u32,
    /// Rows written (table rows, trace samples over all sweeps, or spectrum points).
    pub rows_written: u64,
    /// Column names, in order.
    pub columns: Vec<String>,
    /// `none`, `snappy` or `lz4`.
    pub compression: String,
    /// Size of the written file in bytes.
    pub bytes_written: u64,
    /// True when the file was read back and its schema, metadata and every value matched.
    pub verified: bool,
    /// Traces: the sweeps written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweeps: Option<Vec<u32>>,
    /// Tables and traces: the first row (sample of each sweep) written, zero-based.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_row: Option<u64>,
    /// Spectra: spectra (scans) written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spectra_written: Option<u64>,
    /// Spectra: the per-scan summary file (`<name>.scans.parquet`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_output: Option<String>,
    /// Spectra: size of the summary file in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_bytes: Option<u64>,
}

/// The resolved export target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Table(u32),
    Trace(u32),
    Spectra(u32),
}

fn resolve(info: &FileInfo, sel: ColumnarSelection) -> Result<Target> {
    let unsupported = |what: &str| {
        Error::unsupported(
            "export",
            format!("{what} of a {} file", info.format.name),
            "Parquet/Arrow export writes tables, traces and mass spectra; this file holds images: use `--to ome-tiff` or `--to ome-zarr`.",
        )
    };
    Ok(match sel {
        ColumnarSelection::Auto => {
            if info.images.is_empty() && !info.spectra.is_empty() {
                Target::Spectra(info.spectra[0].index)
            } else if let Some(t) = info.tables.first() {
                Target::Table(t.index)
            } else if let Some(t) = info.traces.first() {
                Target::Trace(t.index)
            } else if !info.spectra.is_empty() {
                Target::Spectra(info.spectra[0].index)
            } else {
                return Err(unsupported("Parquet/Arrow export"));
            }
        }
        ColumnarSelection::Table(i) => {
            if info.tables.is_empty() {
                return Err(if info.traces.is_empty() && info.spectra.is_empty() {
                    unsupported("table export")
                } else {
                    Error::Usage(format!(
                        "this file has no tables; it holds {} (use {})",
                        if info.traces.is_empty() {
                            "spectra"
                        } else {
                            "traces"
                        },
                        if info.traces.is_empty() {
                            "--spectra"
                        } else {
                            "--trace N"
                        }
                    ))
                });
            }
            if !info.tables.iter().any(|t| t.index == i) {
                return Err(Error::Usage(format!(
                    "--table {i} out of range (file has {} tables)",
                    info.tables.len()
                )));
            }
            Target::Table(i)
        }
        ColumnarSelection::Trace(i) => {
            if info.traces.is_empty() {
                return Err(if info.tables.is_empty() && info.spectra.is_empty() {
                    unsupported("trace export")
                } else {
                    Error::Usage(format!(
                        "this file has no traces; it holds {} (use {})",
                        if info.tables.is_empty() {
                            "spectra"
                        } else {
                            "tables"
                        },
                        if info.tables.is_empty() {
                            "--spectra"
                        } else {
                            "--table N"
                        }
                    ))
                });
            }
            if !info.traces.iter().any(|t| t.index == i) {
                return Err(Error::Usage(format!(
                    "--trace {i} out of range (file has {} traces)",
                    info.traces.len()
                )));
            }
            Target::Trace(i)
        }
        ColumnarSelection::Spectra(r) => {
            if info.spectra.is_empty() {
                return Err(if info.images.is_empty() {
                    Error::Usage(
                        "this file holds no mass spectra; export a table (--table N) or a trace (--trace N)".into(),
                    )
                } else {
                    unsupported("spectra export")
                });
            }
            Target::Spectra(r)
        }
    })
}

/// Default output path next to `input` (or `base`, its mirror under a batch output directory):
/// `<stem>.parquet`, `<stem>.table1.parquet`, `<stem>.trace2.sweep0.parquet`,
/// `<stem>.run1.parquet`, ... (`.arrow` for Arrow IPC).
pub fn default_output(base: &Path, info: &FileInfo, opts: &ColumnarOptions) -> PathBuf {
    let stem = base
        .file_stem()
        .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
    let ext = opts.format.extension();
    let mid = match resolve(info, opts.select) {
        Ok(Target::Table(t)) if t > 0 => format!(".table{t}"),
        Ok(Target::Trace(t)) => match (t, opts.sweep) {
            (0, None) => String::new(),
            (0, Some(s)) => format!(".sweep{s}"),
            (t, None) => format!(".trace{t}"),
            (t, Some(s)) => format!(".trace{t}.sweep{s}"),
        },
        Ok(Target::Spectra(r)) if r > 0 => format!(".run{r}"),
        _ => String::new(),
    };
    base.with_file_name(format!("{stem}{mid}.{ext}"))
}

/// The per-scan summary written beside a spectra export: `X.parquet` → `X.scans.parquet`.
pub fn summary_output(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .map_or_else(|| "export".into(), |n| n.to_string_lossy().to_string());
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.clone(), String::new()),
    };
    output.with_file_name(format!("{stem}.scans{ext}"))
}

fn source_name(input: &Path) -> String {
    input.file_name().map_or_else(
        || input.display().to_string(),
        |n| n.to_string_lossy().to_string(),
    )
}

/// The batch source of one export.
enum Src {
    Table(TableSource),
    Trace(TraceSource),
    Spectra(Box<SpectraSource>),
}

impl Src {
    fn get(&mut self) -> &mut dyn BatchSource {
        match self {
            Src::Table(s) => s,
            Src::Trace(s) => s,
            Src::Spectra(s) => s.as_mut(),
        }
    }
    fn spectra(&self) -> Option<&SpectraSource> {
        match self {
            Src::Spectra(s) => Some(s),
            _ => None,
        }
    }
}

/// Build the batch source of an export.
fn build(ds: &mut dyn Dataset, input: &Path, opts: &ColumnarOptions) -> Result<(Src, Target)> {
    let mut file_info = ds.info()?;
    let prov = ds.provenance();
    let name = source_name(input);
    // The embedded `info` names the source by its file name, not the local path it was read from.
    file_info.path.clone_from(&name);
    let info = openreadout_core::experiment::InfoOutput::new(&*ds, file_info);
    let target = resolve(&info, opts.select)?;
    Ok(match target {
        Target::Table(i) => {
            if opts.sweep.is_some() {
                return Err(Error::Usage(
                    "--sweep applies to traces; a table is selected with --table and --rows".into(),
                ));
            }
            let t = info
                .tables
                .iter()
                .find(|t| t.index == i)
                .cloned()
                .unwrap_or_default();
            let total = t.row_count;
            let (first, last) = opts.rows.unwrap_or((0, None));
            if first > total || (first == total && total > 0) {
                return Err(Error::Usage(format!(
                    "--rows starts at {first} but table {i} has {total} rows"
                )));
            }
            let end = last.map_or(total, |l| l.saturating_add(1).min(total));
            (
                Src::Table(TableSource::new(&info, &prov, &t, (first, end), &name)),
                target,
            )
        }
        Target::Trace(i) => {
            let t = info
                .traces
                .iter()
                .find(|t| t.index == i)
                .cloned()
                .unwrap_or_default();
            let sweeps: Vec<u32> = match opts.sweep {
                Some(s) if s >= t.sweep_count => {
                    return Err(Error::Usage(format!(
                        "--sweep {s} out of range (trace {i} has {} sweeps)",
                        t.sweep_count
                    )));
                }
                Some(s) => vec![s],
                None => (0..t.sweep_count).collect(),
            };
            let rows = opts.rows.unwrap_or((0, None));
            (
                Src::Trace(TraceSource::new(&info, &prov, &t, &sweeps, rows, &name)),
                target,
            )
        }
        Target::Spectra(r) => {
            if opts.sweep.is_some() || opts.rows.is_some() {
                return Err(Error::Usage(
                    "--sweep and --rows do not apply to spectra; every scan of the run is written"
                        .into(),
                ));
            }
            let s = SpectraSource::new(&info, &prov, r, opts.centroid, &name)?;
            (Src::Spectra(Box::new(s)), target)
        }
    })
}

enum Sink {
    Parquet(parquet::arrow::ArrowWriter<BufWriter<File>>),
    Ipc(arrow_ipc::writer::FileWriter<BufWriter<File>>),
}

impl Sink {
    fn create(
        path: &Path,
        format: ColumnarFormat,
        compression: ColumnarCompression,
        schema: &SchemaRef,
    ) -> Result<Self> {
        let f = BufWriter::new(File::create(path).map_err(|e| Error::io(path, e))?);
        let err = |e: String| Error::Other(format!("{}: {e}", path.display()));
        Ok(match format {
            ColumnarFormat::Parquet => {
                use parquet::basic::Compression as C;
                use parquet::file::metadata::KeyValue;
                // Non-Arrow Parquet readers see the file-level metadata as key/value pairs too.
                let kv: Vec<KeyValue> = schema
                    .metadata()
                    .iter()
                    .map(|(k, v)| KeyValue::new(k.clone(), v.clone()))
                    .collect();
                let props = parquet::file::properties::WriterProperties::builder()
                    .set_compression(match compression {
                        ColumnarCompression::None => C::UNCOMPRESSED,
                        ColumnarCompression::Snappy => C::SNAPPY,
                        ColumnarCompression::Lz4 => C::LZ4_RAW,
                    })
                    .set_max_row_group_row_count(Some(1 << 20))
                    .set_max_row_group_bytes(Some(128 << 20))
                    .set_key_value_metadata(Some(kv))
                    .build();
                Sink::Parquet(
                    parquet::arrow::ArrowWriter::try_new(f, schema.clone(), Some(props))
                        .map_err(|e| err(e.to_string()))?,
                )
            }
            ColumnarFormat::ArrowIpc => {
                let c = match compression {
                    ColumnarCompression::None => None,
                    ColumnarCompression::Lz4 => Some(arrow_ipc::CompressionType::LZ4_FRAME),
                    ColumnarCompression::Snappy => {
                        return Err(Error::Usage(
                            "Arrow IPC files support --compression none or lz4, not snappy".into(),
                        ));
                    }
                };
                let o = arrow_ipc::writer::IpcWriteOptions::default()
                    .try_with_compression(c)
                    .map_err(|e| err(e.to_string()))?;
                Sink::Ipc(
                    arrow_ipc::writer::FileWriter::try_new_with_options(f, schema, o)
                        .map_err(|e| err(e.to_string()))?,
                )
            }
        })
    }

    fn write(&mut self, b: &RecordBatch) -> Result<()> {
        match self {
            Sink::Parquet(w) => w.write(b).map_err(|e| Error::Other(e.to_string())),
            Sink::Ipc(w) => w.write(b).map_err(|e| Error::Other(e.to_string())),
        }
    }

    fn close(self) -> Result<()> {
        use std::io::Write as _;
        let mut inner = match self {
            Sink::Parquet(w) => w.into_inner().map_err(|e| Error::Other(e.to_string()))?,
            Sink::Ipc(w) => w.into_inner().map_err(|e| Error::Other(e.to_string()))?,
        };
        inner
            .flush()
            .map_err(|e| Error::Other(format!("flush: {e}")))?;
        let f = inner
            .into_inner()
            .map_err(|e| Error::Other(format!("flush: {e}")))?;
        f.sync_all().map_err(|e| Error::Other(format!("sync: {e}")))
    }
}

fn temp_path(output: &Path) -> PathBuf {
    let name = output
        .file_name()
        .map_or_else(|| "export".into(), |n| n.to_string_lossy().to_string());
    output.with_file_name(format!(".{name}.partial-{}", std::process::id()))
}

/// Write `batches` (already hashed into `digest`) to `tmp`, then read it back and compare.
fn write_one(
    tmp: &Path,
    format: ColumnarFormat,
    compression: ColumnarCompression,
    schema: &SchemaRef,
    produce: &mut dyn FnMut(&mut Sink, &mut Digest) -> Result<()>,
) -> Result<(Digest, u64)> {
    let mut sink = Sink::create(tmp, format, compression, schema)?;
    let mut digest = Digest::default();
    produce(&mut sink, &mut digest)?;
    sink.close()?;
    verify::read_back(tmp, format, schema, &digest)?;
    let bytes = std::fs::metadata(tmp).map_err(|e| Error::io(tmp, e))?.len();
    Ok((digest, bytes))
}

/// Export one table, trace or spectra run of `ds` to `output` as Parquet or Arrow IPC. Spectra
/// also write the per-scan summary to [`summary_output`]`(output)`. Everything is written under
/// temporary names, read back and verified, and only then renamed into place.
pub fn export_columnar(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &ColumnarOptions,
) -> Result<ColumnarExportReport> {
    let compression = opts.compression.unwrap_or(match opts.format {
        ColumnarFormat::Parquet => ColumnarCompression::Snappy,
        ColumnarFormat::ArrowIpc => ColumnarCompression::None,
    });
    let (mut src, target) = build(ds, input, opts)?;
    let summary = src.spectra().map(|_| summary_output(output));
    for out in std::iter::once(output).chain(summary.as_deref()) {
        if out == input {
            return Err(Error::Usage(
                "output path must differ from the input; raw files are never modified".into(),
            ));
        }
        if out.exists() && !opts.overwrite {
            return Err(Error::Usage(format!(
                "{} exists; pass --overwrite to replace it",
                out.display()
            )));
        }
    }
    let schema = src.get().schema();
    let tmp = temp_path(output);
    let tmp_summary = summary.as_deref().map(temp_path);
    let cleanup = |paths: &[Option<&Path>]| {
        if std::env::var_os("OPENREADOUT_KEEP_PARTIAL").is_none() {
            for p in paths.iter().flatten() {
                std::fs::remove_file(p).ok();
            }
        }
    };
    let mut summary_batch = None;
    let result = (|| -> Result<(Digest, u64, Option<u64>)> {
        let (digest, bytes) =
            write_one(&tmp, opts.format, compression, &schema, &mut |sink, d| {
                while let Some(b) = src.get().next_batch(ds)? {
                    d.update(&b)?;
                    sink.write(&b)?;
                }
                Ok(())
            })?;
        let mut summary_bytes = None;
        if let Some(tmp_s) = &tmp_summary {
            // The summary rows were gathered by the spectra source while it streamed.
            let s = src
                .spectra()
                .ok_or_else(|| Error::Other("internal: not a spectra export".into()))?;
            let batch = s.summary_batch()?;
            let sschema = s.summary_schema();
            let (_, b) = write_one(tmp_s, opts.format, compression, &sschema, &mut |sink, d| {
                d.update(&batch)?;
                sink.write(&batch)
            })?;
            summary_bytes = Some(b);
            summary_batch = Some(s.spectra_read());
        }
        Ok((digest, bytes, summary_bytes))
    })();
    let (digest, bytes, summary_bytes) = match result {
        Ok(v) => v,
        Err(e) => {
            cleanup(&[Some(&tmp), tmp_summary.as_deref()]);
            return Err(e);
        }
    };
    if let (Some(tmp_s), Some(out_s)) = (&tmp_summary, &summary) {
        std::fs::rename(tmp_s, out_s).map_err(|e| Error::io(out_s, e))?;
    }
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    let (kind, index) = match target {
        Target::Table(i) => ("table", i),
        Target::Trace(i) => ("trace", i),
        Target::Spectra(i) => ("spectra", i),
    };
    let sweeps = match target {
        Target::Trace(_) => {
            let info = ds.info()?;
            let t = info.traces.iter().find(|t| t.index == index);
            Some(match opts.sweep {
                Some(s) => vec![s],
                None => (0..t.map_or(0, |t| t.sweep_count)).collect(),
            })
        }
        _ => None,
    };
    Ok(ColumnarExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: opts.format.id().into(),
        kind: kind.into(),
        index,
        rows_written: digest.rows,
        columns: schema.fields().iter().map(|f| f.name().clone()).collect(),
        compression: compression.id().into(),
        bytes_written: bytes,
        verified: true,
        sweeps,
        first_row: match target {
            Target::Spectra(_) => None,
            _ => Some(opts.rows.map_or(0, |r| r.0)),
        },
        spectra_written: summary_batch,
        summary_output: summary.map(|p| p.display().to_string()),
        summary_bytes,
    })
}

/// An export read into memory: the schema and its record batches (and, for spectra, the
/// per-scan summary). For in-process consumers such as the Python bindings (`File.to_arrow()`).
#[derive(Debug, Clone)]
pub struct ColumnarData {
    /// Schema with the same field and file metadata as the exported files.
    pub schema: SchemaRef,
    /// The rows, in batches of at most 65,536 rows (262,144 points for spectra).
    pub batches: Vec<RecordBatch>,
    /// Spectra only: the per-scan summary.
    pub summary: Option<RecordBatch>,
}

/// Read one table, trace or spectra run into Arrow record batches, exactly as
/// [`export_columnar`] would write them. `max_rows` caps the rows read (an error when the
/// selection is larger: pass `rows`/`sweep` to narrow it).
pub fn read_columnar(
    ds: &mut dyn Dataset,
    input: &Path,
    opts: &ColumnarOptions,
    max_rows: Option<u64>,
) -> Result<ColumnarData> {
    let (mut src, _) = build(ds, input, opts)?;
    let is_spectra = src.spectra().is_some();
    if let Some(cap) = max_rows
        && !is_spectra
        && src.get().total_rows() > cap
    {
        return Err(Error::Usage(format!(
            "the selection has {} rows, more than the limit of {cap}; narrow it with rows or sweep",
            src.get().total_rows()
        )));
    }
    let mut batches = Vec::new();
    let mut rows = 0u64;
    while let Some(b) = src.get().next_batch(ds)? {
        rows += b.num_rows() as u64;
        if let Some(cap) = max_rows
            && rows > cap
        {
            return Err(Error::Usage(format!(
                "more than {cap} points; export to a file instead (`openreadout export --to parquet`)"
            )));
        }
        batches.push(b);
    }
    let summary = match src.spectra() {
        Some(s) => Some(s.summary_batch()?),
        None => None,
    };
    Ok(ColumnarData {
        schema: src.get().schema(),
        batches,
        summary,
    })
}

/// Serialize record batches as an Arrow IPC *stream* (for consumers that take bytes, e.g.
/// `pyarrow.ipc.open_stream`).
pub fn ipc_stream_bytes(schema: &SchemaRef, batches: &[RecordBatch]) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut buf, schema)
            .map_err(|e| Error::Other(e.to_string()))?;
        for b in batches {
            w.write(b).map_err(|e| Error::Other(e.to_string()))?;
        }
        w.finish().map_err(|e| Error::Other(e.to_string()))?;
    }
    Ok(buf)
}

/// Read a Parquet or Arrow IPC file (e.g. one this crate wrote) into memory.
pub fn read_file(path: &Path) -> Result<(SchemaRef, Vec<RecordBatch>)> {
    let head = {
        use std::io::Read as _;
        let mut f = File::open(path).map_err(|e| Error::io(path, e))?;
        let mut b = [0u8; 6];
        f.read_exact(&mut b).map_err(|e| Error::io(path, e))?;
        b
    };
    let open = || File::open(path).map_err(|e| Error::io(path, e));
    let err = |e: String| Error::corrupt("parquet", format!("{}: {e}", path.display()));
    if &head[..4] == b"PAR1" {
        let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(open()?)
            .map_err(|e| err(e.to_string()))?;
        let schema = b.schema().clone();
        let batches = b
            .build()
            .map_err(|e| err(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| err(e.to_string()))?;
        Ok((schema, batches))
    } else if &head == b"ARROW1" {
        let r = arrow_ipc::reader::FileReader::try_new_buffered(open()?, None)
            .map_err(|e| err(e.to_string()))?;
        let schema = r.schema();
        let batches = r
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| err(e.to_string()))?;
        Ok((schema, batches))
    } else {
        Err(Error::UnknownFormat {
            path: path.to_path_buf(),
        })
    }
}
