//! `export`: images to OME-TIFF or OME-Zarr, tables and traces to CSV, Parquet or Arrow,
//! spectra to mzML, and the format-specific writers (ASM, NWB, JCAMP-DX, RDML); or one
//! embedded attachment (`extract`).

use std::path::{Path, PathBuf};

use clap::ValueEnum;
use openreadout_core::model::{ExtractOutput, TableExportReport, TraceExportReport};
use openreadout_core::{Error, Registry, Result};

use super::batch::{self, Spec, Stdin};
use super::{nmr, plate, qpcr, wrap};

/// Arguments of `export`.
#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Files, directories or glob patterns; several make a batch (see `--recursive`,
    /// `--jsonl`). `-` reads standard input.
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,
    #[command(flatten)]
    pub batch: batch::BatchArgs,
    /// Target format. Default: `ome-tiff` for image files, `csv` for tabular (FCS) and signal
    /// (ABF, ..., NMR, JCAMP-DX, chromatography) files, `mzml` for mass-spectrometry files;
    /// `asm` writes Allotrope plate-reader JSON; `parquet`/`arrow` write tables, traces and
    /// spectra; `nwb` writes electrophysiology traces; `jcamp` writes NMR and 1-D spectra.
    /// With `-o`, the output's extension sets the format, and `--format` must agree with it.
    #[arg(long = "format", value_enum)]
    pub to: Option<ExportFormat>,
    /// Output path. Defaults to the input name with `.ome.tiff`, `.ome.zarr`, `.csv`, `.mzML`,
    /// `.asm.json`, `.parquet`, `.arrow`, `.nwb`, `.jdx` or `.rdml`. Without `--format`, its
    /// extension picks the format (`.tif` and `.tiff` mean OME-TIFF, `.zarr` OME-Zarr,
    /// `.json` ASM).
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// CSV, Parquet, Arrow: export this table (FCS data set, plate read, event or peak table) index (see `info`). Default 0.
    #[arg(long)]
    pub table: Option<u32>,
    /// CSV, Parquet, Arrow, NWB, JCAMP-DX: row range, zero-based and inclusive: `A-B`, `A-`
    /// (to the end) or `A`. For traces, rows are samples of each sweep.
    #[arg(long)]
    pub rows: Option<String>,
    /// Trace exports: this sweep (see `info` → `traces[].sweep_count`). CSV default 0;
    /// Parquet, Arrow, NWB and JCAMP-DX default: every sweep.
    #[arg(long)]
    pub sweep: Option<u32>,
    /// Trace exports: this trace index (see `info` → `traces[]`). Default 0 (NWB: every trace).
    #[arg(long)]
    pub trace: Option<u32>,
    /// Parquet/Arrow: export the mass spectra of `--run` (long form, one row per point, plus a
    /// `<name>.scans.parquet` per-scan summary) instead of a table or trace.
    #[arg(long)]
    pub spectra: bool,
    /// CSV: add a second header line with column labels (FCS `$PnS`).
    #[arg(long)]
    pub labels: bool,
    /// Export only this image index (see `info --view structure`). Default: all images.
    #[arg(long)]
    pub image: Option<u32>,
    /// Plane selection, e.g. `c=0`, `z=2-5`, `t=0,3`. Repeatable.
    #[arg(long = "select")]
    pub select: Vec<String>,
    /// Compression. Images (default `deflate`): `none`, `deflate`, `lzw` (OME-Zarr stores
    /// `deflate` with the Zarr `gzip` codec and does not support `lzw`). Parquet (default
    /// `snappy`): `none`, `snappy`, `lz4`. Arrow IPC (default `none`): `none`, `lz4`.
    #[arg(long, value_enum)]
    pub compression: Option<Compression>,
    /// Replace an existing output file (or OME-Zarr store).
    #[arg(long)]
    pub overwrite: bool,
    /// Embed the vendor metadata tree (as JSON) in an OME `StructuredAnnotation`
    /// (OME-Zarr: in `OME/METADATA.ome.xml` of multi-image stores).
    #[arg(long)]
    pub embed_vendor: bool,
    /// OME-Zarr: chunk edge in pixels for y and x (chunks are 1x1x1xNxN). OME-TIFF: tile
    /// edge of tiled output (pyramids, regions, levels, planes above 4 GiB; a multiple of 16).
    #[arg(long, value_name = "N", default_value_t = 512)]
    pub chunk_size: u32,
    /// Resolution levels including full resolution (1 = no pyramid): exactly, for mean
    /// pyramids; at most, for the source's own. Default: halve while a level is larger than
    /// 1024 px in either dimension (mean), all of them (source).
    #[arg(long, value_name = "N")]
    pub levels: Option<u32>,
    /// Images: export this source pyramid level as the full resolution (0 = full
    /// resolution; `info` → images[].resolution_levels).
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub level: u32,
    /// Images: export only this rectangle of every plane, `X,Y,WIDTH,HEIGHT` in the pixels
    /// of `--level`, read tile by tile.
    #[arg(long, value_name = "X,Y,W,H")]
    pub region: Option<String>,
    /// Images: lower resolutions to write. `auto` (default): the source's own pyramid when it
    /// has one and the whole image is exported from level 0; otherwise 2x2 mean levels for
    /// OME-Zarr and none for OME-TIFF. `source` copies the source's levels, `mean` computes
    /// them, `none` writes full resolution only. OME-TIFF pyramids go in SubIFDs. Streams
    /// tile by tile, so whole-slide images export in bounded memory.
    #[arg(long, value_name = "MODE", default_value = "auto")]
    pub pyramid: String,
    /// mzML only: write the instrument's centroid lists instead of profiles where a scan has both.
    #[arg(long)]
    pub centroid: bool,
    #[command(flatten)]
    pub plate: plate::PlateExportArgs,
    /// mzML, Parquet, Arrow: run (spectra collection) index.
    #[arg(long, default_value_t = 0)]
    pub run: u32,
    #[command(flatten)]
    pub process: nmr::ProcessArgs,
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum ExportFormat {
    /// `BigTIFF` + OME-XML, one file.
    OmeTiff,
    /// OME-NGFF 0.5 (Zarr v3) directory store with a multiscale pyramid.
    OmeZarr,
    /// Indexed mzML 1.1.0 (mass-spectrometry files).
    Mzml,
    /// Comma-separated values (tables such as FCS events and plate reads, traces such as ABF sweeps or NMR spectra).
    Csv,
    /// Allotrope Simple Model plate-reader JSON (plate-reader exports only).
    Asm,
    /// Apache Parquet: a table, a trace (every sweep, one row per sample) or mass spectra (one row
    /// per point, plus a per-scan summary file).
    Parquet,
    /// Arrow IPC file (Feather v2): the same columns as `parquet`.
    #[value(alias = "feather", alias = "ipc")]
    Arrow,
    /// NWB 2.x (HDF5): electrophysiology traces as `TimeSeries` under `acquisition/`.
    Nwb,
    /// JCAMP-DX 5.01/6.00: NMR FIDs and spectra and other 1-D spectra (`##XYDATA=(X++(Y..Y))`,
    /// DIFDUP with Y checks, or NTUPLES for complex data).
    #[value(alias = "jcamp-dx", alias = "jdx")]
    Jcamp,
    /// RDML 1.3 (Real-time PCR Data Markup Language): qPCR files (RDML, .eds, .rex) with plate
    /// setup, Cq, curves and melt data.
    Rdml,
}

impl ExportFormat {
    /// The `--format` value, as `openreadout_batch::export_format` names formats.
    fn name(self) -> &'static str {
        match self {
            ExportFormat::OmeTiff => "ome-tiff",
            ExportFormat::OmeZarr => "ome-zarr",
            ExportFormat::Mzml => "mzml",
            ExportFormat::Csv => "csv",
            ExportFormat::Asm => "asm",
            ExportFormat::Parquet => "parquet",
            ExportFormat::Arrow => "arrow",
            ExportFormat::Nwb => "nwb",
            ExportFormat::Jcamp => "jcamp",
            ExportFormat::Rdml => "rdml",
        }
    }

    /// The format of `-o`: `--format` when given, else the one the output name implies. An
    /// error (exit 2) when they disagree, or when neither says.
    fn for_output(to: Option<Self>, output: &Path) -> Result<Self> {
        let name =
            openreadout_batch::export_format::resolve(to.map(Self::name), output, "--format")?;
        Self::from_str(name, true)
            .map_err(|_| Error::Other(format!("export format {name} has no --format value")))
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Compression {
    None,
    #[value(alias = "gzip")]
    Deflate,
    Lzw,
    /// Parquet only.
    Snappy,
    /// Parquet (`LZ4_RAW`) and Arrow IPC (`LZ4_FRAME`).
    Lz4,
}

/// Arguments of `extract`.
#[derive(Debug, clap::Args)]
pub struct ExtractArgs {
    /// The file that holds the attachment.
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// The attachment: its name as `info --view structure` shows it (e.g. `Thumbnail`, `Label`,
    /// `SlidePreview`, `TimeStamps`), or `#<index>`.
    #[arg(value_name = "ATTACHMENT")]
    pub attachment: String,
    /// Output path. Default: `<input stem>.<attachment name>.<ext>` next to the input.
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Replace an existing output file.
    #[arg(long)]
    pub overwrite: bool,
    #[arg(long)]
    pub json: bool,
}

/// Run `extract`: write one embedded attachment as stored.
pub fn run_extract(reg: &Registry, a: &ExtractArgs) -> i32 {
    wrap(
        a.json,
        || {
            extract(
                reg,
                &a.file,
                &a.attachment,
                a.output.as_deref(),
                a.overwrite,
            )
        },
        |r| {
            format!(
                "wrote {} ({} `{}`, {} bytes, verified={})",
                r.output, r.attachment.content_type, r.attachment.name, r.bytes_written, r.verified
            )
        },
    )
}

/// Run `export`.
pub fn run(reg: &Registry, a: ExportArgs) -> i32 {
    let ExportArgs {
        files,
        batch,
        to,
        output,
        table,
        rows,
        sweep,
        trace,
        spectra,
        labels,
        image,
        select,
        compression,
        overwrite,
        embed_vendor,
        chunk_size,
        levels,
        level,
        region,
        pyramid,
        centroid,
        plate,
        run,
        process,
        json,
    } = a;
    let req = ExportRequest {
        to,
        output,
        image,
        select,
        compression,
        overwrite,
        embed_vendor,
        chunk_size,
        levels,
        level,
        region,
        pyramid,
        csv: openreadout_batch::csv::CsvOptions {
            table,
            rows,
            labels,
            overwrite,
            sweep,
            trace,
        },
        centroid,
        run,
        spectra,
        process,
        plate,
    };
    batch::run(
        reg,
        &files,
        Spec {
            json,
            batch: &batch,
            stdin: Stdin::Reject,
        },
        &mut |i| export(reg, i, &req),
        &render_export,
    )
}

/// Arguments of `export` after parsing.
struct ExportRequest {
    /// `None`: the default for the file's kind (see `--format`).
    to: Option<ExportFormat>,
    output: Option<PathBuf>,
    image: Option<u32>,
    select: Vec<String>,
    /// `None`: the target format's default.
    compression: Option<Compression>,
    overwrite: bool,
    embed_vendor: bool,
    chunk_size: u32,
    levels: Option<u32>,
    /// Source pyramid level exported as full resolution.
    level: u32,
    /// `X,Y,W,H` of the rectangle to export (parsed per file).
    region: Option<String>,
    /// `auto`, `none`, `source` or `mean`.
    pyramid: String,
    csv: openreadout_batch::csv::CsvOptions,
    centroid: bool,
    run: u32,
    /// Parquet/Arrow: the spectra of `run` rather than a table or trace.
    spectra: bool,
    /// NMR: FID traces exported as processed spectra.
    process: nmr::ProcessArgs,
    /// Multi-well plates: wells, partial copies, one OME-TIFF per field.
    plate: plate::PlateExportArgs,
}

/// Output of `export`: an image export report (OME-TIFF, OME-Zarr), a CSV report for tables or
/// traces, or an mzML report for mass spectra.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ExportOutput {
    Image(openreadout_ometiff::ExportReport),
    Csv(TableExportReport),
    TraceCsv(TraceExportReport),
    Spectra(openreadout_mzml_writer::MzmlExportReport),
    Asm(openreadout_plate::AsmExportReport),
    #[cfg(feature = "parquet")]
    Columnar(openreadout_arrow::ColumnarExportReport),
    Nwb(openreadout_hdf5::NwbExportReport),
    Jcamp(openreadout_nmr::JcampExportReport),
    Rdml(openreadout_qpcr::RdmlExportReport),
    PerImage(plate::PerImageExport),
}

impl batch::Item for ExportOutput {
    fn contents(&self) -> Option<String> {
        let name = |p: &str| {
            Path::new(p)
                .file_name()
                .map_or_else(|| p.to_string(), |n| n.to_string_lossy().to_string())
        };
        Some(match self {
            ExportOutput::Image(r) => format!("{} planes -> {}", r.planes_written, name(&r.output)),
            ExportOutput::Csv(r) => format!("{} rows -> {}", r.rows_written, name(&r.output)),
            ExportOutput::TraceCsv(r) => {
                format!("{} samples -> {}", r.samples_written, name(&r.output))
            }
            ExportOutput::Spectra(r) if r.spectra_written == 0 => format!(
                "{} chromatograms -> {}",
                r.chromatograms_written,
                name(&r.output)
            ),
            ExportOutput::Spectra(r) => {
                format!("{} spectra -> {}", r.spectra_written, name(&r.output))
            }
            ExportOutput::Asm(r) => {
                format!("{} measurements -> {}", r.measurements, name(&r.output))
            }
            #[cfg(feature = "parquet")]
            ExportOutput::Columnar(r) => format!("{} rows -> {}", r.rows_written, name(&r.output)),
            ExportOutput::Nwb(r) => format!(
                "{} series, {} samples -> {}",
                r.series.len(),
                r.samples_written,
                name(&r.output)
            ),
            ExportOutput::Jcamp(r) => {
                format!("{} points -> {}", r.samples_written, name(&r.output))
            }
            ExportOutput::Rdml(r) => {
                format!("{} reactions -> {}", r.reactions, name(&r.output))
            }
            ExportOutput::PerImage(r) => format!("{} files -> {}", r.files.len(), name(&r.output)),
        })
    }
}

fn render_export(r: &ExportOutput) -> String {
    match r {
        ExportOutput::Image(r) => format!(
            "wrote {} ({} images{}, {} planes, {} bytes, verified={}){}",
            r.output,
            r.images_written,
            if r.layout.as_deref() == Some("plate") {
                " as an OME-NGFF HCS plate"
            } else {
                ""
            },
            r.planes_written,
            r.bytes_written,
            r.verified,
            if r.images_skipped.is_empty() {
                String::new()
            } else {
                format!(
                    "; skipped {} image(s) with missing plane files",
                    r.images_skipped.len()
                )
            }
        ),
        ExportOutput::PerImage(r) => plate::render_per_image(r),
        ExportOutput::Csv(r) => format!(
            "wrote {} (table {}, rows {}..{}, {} columns, {} bytes, verified={})",
            r.output,
            r.table,
            r.first_row,
            r.first_row + r.rows_written,
            r.columns_written,
            r.bytes_written,
            r.verified
        ),
        ExportOutput::TraceCsv(r) => format!(
            "wrote {} (trace {}, sweep {}, samples {}..{}, time + {} channels, {} bytes, verified={})",
            r.output,
            r.trace,
            r.sweep,
            r.first_sample,
            r.first_sample + r.samples_written,
            r.channels_written,
            r.bytes_written,
            r.verified
        ),
        ExportOutput::Spectra(r) if r.chromatograms_written > 0 => format!(
            "wrote {} ({} spectra, {} points, {} chromatograms, {} bytes, verified={})",
            r.output,
            r.spectra_written,
            r.points_written,
            r.chromatograms_written,
            r.bytes_written,
            r.verified
        ),
        ExportOutput::Spectra(r) => format!(
            "wrote {} ({} spectra, {} points, {} bytes, verified={})",
            r.output, r.spectra_written, r.points_written, r.bytes_written, r.verified
        ),
        ExportOutput::Asm(r) => format!(
            "wrote {} (ASM plate-reader JSON: {} plate/well documents, {} measurements, {} values, {} errors, {} calculated; {} bytes, verified={}){}",
            r.output,
            r.documents,
            r.measurements,
            r.values,
            r.errors,
            r.calculated,
            r.bytes_written,
            r.verified,
            r.notes.iter().fold(String::new(), |mut acc, n| {
                acc.push_str("\n  note: ");
                acc.push_str(n);
                acc
            })
        ),
        #[cfg(feature = "parquet")]
        ExportOutput::Columnar(r) => {
            let mut s = format!(
                "wrote {} ({} {} {}, {} rows x {} columns, {}, {} bytes, verified={})",
                r.output,
                r.format,
                r.kind,
                r.index,
                r.rows_written,
                r.columns.len(),
                r.compression,
                r.bytes_written,
                r.verified
            );
            if let Some(o) = &r.summary_output {
                s.push_str(&format!(
                    "\nwrote {o} (per-scan summary: {} spectra, {} bytes)",
                    r.spectra_written.unwrap_or(0),
                    r.summary_bytes.unwrap_or(0)
                ));
            }
            s
        }
        ExportOutput::Nwb(r) => {
            let mut s = format!(
                "wrote {} (NWB {}: {} TimeSeries, {} samples, {} bytes, verified={})",
                r.output,
                r.nwb_version,
                r.series.len(),
                r.samples_written,
                r.bytes_written,
                r.verified
            );
            for x in &r.series {
                s.push_str(&format!(
                    "\n  acquisition/{}: trace {}, sweep {}, {} samples x {} channels{}",
                    x.name,
                    x.trace,
                    x.sweep,
                    x.samples,
                    x.channels,
                    x.unit
                        .as_deref()
                        .map(|u| format!(" ({u})"))
                        .unwrap_or_default()
                ));
            }
            s
        }
        ExportOutput::Jcamp(r) => format!(
            "wrote {} (JCAMP-DX {}, trace {}, {} page(s), {} points x {} channel(s), {}, {}, {} bytes, verified={})",
            r.output,
            r.jcamp_version,
            r.trace,
            r.pages,
            r.samples_written,
            r.channels_written,
            r.encoding,
            if r.exact {
                "exact".to_string()
            } else {
                format!("max error {:e}", r.max_abs_error)
            },
            r.bytes_written,
            r.verified
        ),
        ExportOutput::Rdml(r) => format!(
            "wrote {} (RDML {}: {} run(s), {} reactions, {} data elements, {} Cq values, {} amplification and {} melt points; {} bytes, verified={}){}",
            r.output,
            r.rdml_version,
            r.runs,
            r.reactions,
            r.data_elements,
            r.cq_values,
            r.amplification_points,
            r.melt_points,
            r.bytes_written,
            r.verified,
            r.notes.iter().fold(String::new(), |mut acc, n| {
                acc.push_str("\n  note: ");
                acc.push_str(n);
                acc
            })
        ),
    }
}

/// `-o` in batch mode is a directory: outputs mirror the inputs' relative paths under it.
fn export(reg: &Registry, input: &batch::Input<'_>, req: &ExportRequest) -> Result<ExportOutput> {
    let file = input.path;
    // Where default output names are derived from (the input, or its mirror under `-o DIR`).
    let (name_base, explicit) = match (&req.output, input.batch) {
        (Some(dir), true) => {
            let base = dir.join(&input.item.relative);
            if let Some(parent) = base.parent() {
                std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
            }
            (base, None)
        }
        (out, _) => (file.to_path_buf(), out.clone()),
    };
    // `-o` names one file or store, except with `--per-image`, where it is a directory.
    let requested = match &explicit {
        Some(out) if !req.plate.per_image => Some(ExportFormat::for_output(req.to, out)?),
        _ => req.to,
    };
    let opener =
        || -> Result<Box<dyn openreadout_core::Dataset>> { reg.open(file).map(|(_, d)| d) };
    let bar: std::sync::OnceLock<crate::ui::Progress> = std::sync::OnceLock::new();
    let progress = |done: u64, total: u64| {
        if !input.batch {
            bar.get_or_init(|| crate::ui::Progress::new(total, "planes"))
                .set(done, None);
        }
    };
    let ctx = openreadout_core::parallel::ReadContext {
        opener: Some(&opener),
        progress: Some(&progress),
    };
    let codec = match req.compression {
        None | Some(Compression::Deflate) => openreadout_ometiff::Codec::Deflate,
        Some(Compression::None) => openreadout_ometiff::Codec::None,
        Some(Compression::Lzw) => openreadout_ometiff::Codec::Lzw,
        Some(Compression::Snappy | Compression::Lz4) => {
            if matches!(
                requested,
                None | Some(ExportFormat::OmeTiff | ExportFormat::OmeZarr)
            ) {
                return Err(Error::Usage(
                    "--compression snappy and lz4 apply to Parquet and Arrow exports; images take none, deflate or lzw".into(),
                ));
            }
            openreadout_ometiff::Codec::Deflate
        }
    };
    if matches!(requested, Some(ExportFormat::OmeZarr)) && codec == openreadout_ometiff::Codec::Lzw
    {
        return Err(Error::Usage(
            "OME-Zarr export supports --compression none or deflate (gzip), not lzw".into(),
        ));
    }
    if requested == Some(ExportFormat::Rdml) {
        let qds = qpcr::open(reg, file)?;
        let output = explicit.unwrap_or_else(|| openreadout_qpcr::default_rdml_output(&name_base));
        return openreadout_qpcr::export_rdml(&qds, &output, req.overwrite).map(ExportOutput::Rdml);
    }
    let (det, ds) = reg.open(file)?;
    let mut ds = req.process.wrap(ds)?;
    if requested == Some(ExportFormat::Asm) {
        if det.format_id != openreadout_plate::FORMAT_ID {
            return Err(Error::unsupported(
                "export",
                format!("ASM export of a {} file", ds.info()?.format.name),
                "`--format asm` writes Allotrope plate-reader JSON and needs a plate-reader export; use `--format csv`, `--format mzml` or `--format ome-tiff`.",
            ));
        }
        drop(ds);
        let stem = file
            .file_stem()
            .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
        let output = req
            .output
            .clone()
            .unwrap_or_else(|| file.with_file_name(format!("{stem}.asm.json")));
        let plate = openreadout_plate::PlateDataset::open(file)?;
        return openreadout_plate::export_asm(&plate, &output, req.overwrite)
            .map(ExportOutput::Asm);
    }
    let to = if let Some(t) = requested {
        t
    } else {
        match openreadout_batch::csv::default_export_format(&ds.info()?) {
            "mzml" => ExportFormat::Mzml,
            "csv" => ExportFormat::Csv,
            _ => ExportFormat::OmeTiff,
        }
    };
    let stem = name_base
        .file_stem()
        .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
    match to {
        ExportFormat::Parquet | ExportFormat::Arrow => {
            return export_columnar(ds.as_mut(), file, &name_base, explicit, req, to);
        }
        ExportFormat::Nwb => {
            let opts = nwb_options(req)?;
            let output =
                explicit.unwrap_or_else(|| openreadout_hdf5::default_nwb_output(&name_base, &opts));
            if output == file {
                return Err(Error::Usage(
                    "output path must differ from the input; raw files are never modified".into(),
                ));
            }
            return openreadout_hdf5::export_nwb(ds.as_mut(), file, &output, &opts)
                .map(ExportOutput::Nwb);
        }
        ExportFormat::Jcamp => {
            let opts = jcamp_options(req)?;
            let output = explicit
                .unwrap_or_else(|| openreadout_nmr::default_jcamp_output(&name_base, &opts));
            if output == file {
                return Err(Error::Usage(
                    "output path must differ from the input; raw files are never modified".into(),
                ));
            }
            return openreadout_nmr::export_jcamp(ds.as_mut(), file, &output, &opts)
                .map(ExportOutput::Jcamp);
        }
        _ => {}
    }
    if req.spectra {
        return Err(Error::Usage(
            "--spectra applies to --format parquet and --format arrow (use --format mzml for mzML)"
                .into(),
        ));
    }
    let output = explicit.unwrap_or_else(|| match to {
        ExportFormat::OmeTiff => name_base.with_file_name(format!("{stem}.ome.tiff")),
        ExportFormat::OmeZarr => openreadout_omezarr::default_output(&name_base),
        ExportFormat::Mzml => openreadout_mzml_writer::default_output(&name_base),
        ExportFormat::Asm => name_base.with_file_name(format!("{stem}.asm.json")),
        ExportFormat::Rdml => openreadout_qpcr::default_rdml_output(&name_base),
        ExportFormat::Csv => openreadout_batch::csv::default_output(&name_base, &req.csv),
        ExportFormat::Parquet | ExportFormat::Arrow | ExportFormat::Nwb | ExportFormat::Jcamp => {
            unreachable!("handled above")
        }
    });
    if output == file {
        return Err(Error::Usage(
            "output path must differ from the input; raw files are never modified".into(),
        ));
    }
    if to == ExportFormat::Csv {
        return Ok(
            match openreadout_batch::csv::export(ds.as_mut(), file, &output, &req.csv)? {
                openreadout_batch::csv::CsvReport::Table(r) => ExportOutput::Csv(r),
                openreadout_batch::csv::CsvReport::Trace(r) => ExportOutput::TraceCsv(r),
            },
        );
    }
    if to == ExportFormat::Mzml {
        return openreadout_mzml_writer::export_mzml(ds.as_mut(), file, &output, &{
            let mut mzml_export_options = openreadout_mzml_writer::MzmlExportOptions::default();
            mzml_export_options.run = req.run;
            mzml_export_options.centroid = req.centroid;
            mzml_export_options.overwrite = req.overwrite;
            mzml_export_options.index_range = None;
            mzml_export_options
        })
        .map(ExportOutput::Spectra);
    }
    let info = ds.info()?;
    if info.images.is_empty() && (!info.tables.is_empty() || !info.traces.is_empty()) {
        return Err(Error::unsupported(
            "export",
            format!("image export of a {} file", info.format.name),
            if info.tables.is_empty() {
                "This file holds sampled signals or spectra (traces), not images; use `--format csv` (one sweep per file, `--sweep N`) or `openreadout trace`."
            } else {
                "This file holds tables (events), not images; use `--format csv`."
            },
        ));
    }
    if info.images.is_empty() && !info.spectra.is_empty() {
        return Err(Error::unsupported(
            "export",
            format!("image export of a {} file", info.format.name),
            "This file holds mass spectra, not images; use `--format mzml` (or `--format csv` for chromatograms).",
        ));
    }
    let region = req
        .region
        .as_deref()
        .map(openreadout_core::Region::parse)
        .transpose()?;
    let pyramid: openreadout_ometiff::PyramidMode = req.pyramid.parse()?;
    if to == ExportFormat::OmeTiff && req.plate.per_image {
        let mut o = openreadout_ometiff::ExportOptions::default();
        o.image = req.image;
        o.select = req.select.clone();
        o.codec = codec;
        o.overwrite = req.overwrite;
        o.embed_vendor = req.embed_vendor;
        o.level = req.level;
        o.region = region;
        o.pyramid = pyramid;
        o.levels = req.levels;
        o.tile = req.chunk_size;
        let dir = match (&req.output, input.batch) {
            (Some(o), false) => o.clone(),
            _ => plate::default_dir(&name_base),
        };
        return plate::export_per_image(reg, file, ds.as_mut(), &dir, &req.plate, &o, &ctx)
            .map(ExportOutput::PerImage);
    }
    if to == ExportFormat::OmeTiff && (!req.plate.wells.is_empty() || req.plate.skip_incomplete) {
        return Err(Error::Usage(
            "--well and --skip-incomplete apply to --format ome-zarr and to --format ome-tiff --per-image"
                .into(),
        ));
    }
    match to {
        ExportFormat::OmeTiff => openreadout_ometiff::export_ome_tiff_with(
            ds.as_mut(),
            file,
            &output,
            &{
                let mut export_options = openreadout_ometiff::ExportOptions::default();
                export_options.image = req.image;
                export_options.select = req.select.clone();
                export_options.codec = codec;
                export_options.overwrite = req.overwrite;
                export_options.embed_vendor = req.embed_vendor;
                export_options.level = req.level;
                export_options.region = region;
                export_options.pyramid = pyramid;
                export_options.levels = req.levels;
                export_options.tile = req.chunk_size;
                export_options
            },
            &ctx,
        ),
        ExportFormat::OmeZarr => openreadout_omezarr::export_ome_zarr_with(
            ds.as_mut(),
            file,
            &output,
            &{
                let mut zarr_export_options = openreadout_omezarr::ZarrExportOptions::default();
                zarr_export_options.image = req.image;
                zarr_export_options.select = req.select.clone();
                zarr_export_options.codec = codec;
                zarr_export_options.overwrite = req.overwrite;
                zarr_export_options.embed_vendor = req.embed_vendor;
                zarr_export_options.chunk = req.chunk_size;
                zarr_export_options.levels = req.levels;
                zarr_export_options.level = req.level;
                zarr_export_options.region = region;
                zarr_export_options.pyramid = pyramid;
                zarr_export_options.plate = !req.plate.no_plate;
                zarr_export_options.wells = req.plate.wells.clone();
                zarr_export_options.skip_incomplete = req.plate.skip_incomplete;
                zarr_export_options
            },
            &ctx,
        ),
        ExportFormat::Csv
        | ExportFormat::Mzml
        | ExportFormat::Asm
        | ExportFormat::Rdml
        | ExportFormat::Parquet
        | ExportFormat::Arrow
        | ExportFormat::Nwb
        | ExportFormat::Jcamp => unreachable!("handled above"),
    }
    .map(ExportOutput::Image)
}

/// `--rows A-B` parsed, when given.
fn rows(req: &ExportRequest) -> Result<Option<(u64, Option<u64>)>> {
    req.csv
        .rows
        .as_deref()
        .map(openreadout_batch::csv::parse_rows)
        .transpose()
}

/// Reject flags that belong to other export formats.
fn reject_flags(req: &ExportRequest, format: &str) -> Result<()> {
    if req.csv.labels {
        return Err(Error::Usage(format!(
            "--labels applies to CSV, not {format}"
        )));
    }
    if req.csv.table.is_some() {
        return Err(Error::Usage(format!(
            "--table does not apply to {format}: it exports traces (use --trace/--sweep)"
        )));
    }
    if req.spectra {
        return Err(Error::Usage(format!(
            "--spectra does not apply to {format}: it exports traces"
        )));
    }
    if matches!(req.compression, Some(c) if c != Compression::None) {
        return Err(Error::Usage(format!(
            "--compression does not apply to {format}"
        )));
    }
    Ok(())
}

fn nwb_options(req: &ExportRequest) -> Result<openreadout_hdf5::NwbExportOptions> {
    reject_flags(req, "NWB")?;
    let mut o = openreadout_hdf5::NwbExportOptions::default();
    o.trace = req.csv.trace;
    o.sweep = req.csv.sweep;
    o.rows = rows(req)?;
    o.overwrite = req.overwrite;
    Ok(o)
}

fn jcamp_options(req: &ExportRequest) -> Result<openreadout_nmr::JcampExportOptions> {
    reject_flags(req, "JCAMP-DX")?;
    let mut o = openreadout_nmr::JcampExportOptions::default();
    if req.compression == Some(Compression::None) {
        // uncompressed ordinates: AFFN instead of DIFDUP
        o.encoding = openreadout_nmr::JcampEncoding::Affn;
    }
    o.trace = req.csv.trace;
    o.sweep = req.csv.sweep;
    o.rows = rows(req)?;
    o.overwrite = req.overwrite;
    Ok(o)
}

/// `--format parquet|arrow`.
#[cfg(feature = "parquet")]
fn export_columnar(
    ds: &mut dyn openreadout_core::Dataset,
    file: &Path,
    name_base: &Path,
    explicit: Option<PathBuf>,
    req: &ExportRequest,
    to: ExportFormat,
) -> Result<ExportOutput> {
    use openreadout_arrow::{ColumnarCompression, ColumnarFormat, ColumnarSelection};
    if req.csv.labels {
        return Err(Error::Usage(
            "--labels applies to CSV; Parquet/Arrow columns carry their label in the field metadata"
                .into(),
        ));
    }
    let mut o = openreadout_arrow::ColumnarOptions::default();
    o.format = if to == ExportFormat::Arrow {
        ColumnarFormat::ArrowIpc
    } else {
        ColumnarFormat::Parquet
    };
    o.compression = match req.compression {
        None => None,
        Some(Compression::None) => Some(ColumnarCompression::None),
        Some(Compression::Snappy) => Some(ColumnarCompression::Snappy),
        Some(Compression::Lz4) => Some(ColumnarCompression::Lz4),
        Some(Compression::Deflate | Compression::Lzw) => {
            return Err(Error::Usage(
                "Parquet takes --compression none, snappy or lz4; Arrow IPC none or lz4".into(),
            ));
        }
    };
    o.select = match (req.csv.table, req.csv.trace, req.spectra) {
        (None, None, false) => ColumnarSelection::Auto,
        (Some(t), None, false) => ColumnarSelection::Table(t),
        (None, Some(t), false) => ColumnarSelection::Trace(t),
        (None, None, true) => ColumnarSelection::Spectra(req.run),
        _ => {
            return Err(Error::Usage(
                "give at most one of --table, --trace and --spectra".into(),
            ));
        }
    };
    o.sweep = req.csv.sweep;
    o.rows = rows(req)?;
    o.centroid = req.centroid;
    o.overwrite = req.overwrite;
    let output = match explicit {
        Some(p) => p,
        None => openreadout_arrow::default_output(name_base, &ds.info()?, &o),
    };
    if output == file {
        return Err(Error::Usage(
            "output path must differ from the input; raw files are never modified".into(),
        ));
    }
    openreadout_arrow::export_columnar(ds, file, &output, &o).map(ExportOutput::Columnar)
}

#[cfg(not(feature = "parquet"))]
fn export_columnar(
    _ds: &mut dyn openreadout_core::Dataset,
    _file: &Path,
    _name_base: &Path,
    _explicit: Option<PathBuf>,
    _req: &ExportRequest,
    to: ExportFormat,
) -> Result<ExportOutput> {
    Err(Error::unsupported(
        "export",
        format!(
            "{} export",
            if to == ExportFormat::Arrow {
                "Arrow IPC"
            } else {
                "Parquet"
            }
        ),
        "This build was compiled without the `parquet` cargo feature (on by default); rebuild with `--features parquet`, or use `--format csv`.",
    ))
}

fn extract(
    reg: &Registry,
    file: &Path,
    attachment: &str,
    output: Option<&Path>,
    overwrite: bool,
) -> Result<ExtractOutput> {
    let (det, mut ds) = reg.open(file)?;
    openreadout_ops::extract::extract_attachment(
        ds.as_mut(),
        file,
        det.format_id,
        attachment,
        output,
        overwrite,
    )
}
