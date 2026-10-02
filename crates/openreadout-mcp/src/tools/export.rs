//! `openreadout_export`: conversion to open formats, or one embedded attachment.

use std::path::{Path, PathBuf};

use openreadout_core::{Error, Registry};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ProgressNotificationParam};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, schemars, tool, tool_router};
use serde::Deserialize;

use crate::{InstrumentServer, mcp_err, progress, to_value, with_strict};

/// Arguments for `openreadout_export`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExportArgs {
    /// Absolute or working-directory-relative path to the instrument file.
    pub file: String,
    /// `ome-tiff` (default for images; one BigTIFF file), `ome-zarr` (OME-NGFF 0.5 / Zarr v3 directory
    /// with a pyramid), `mzml` (indexed mzML 1.1.0, for mass-spectrometry files; the default for them),
    /// `asm` (Allotrope Simple Model plate-reader JSON; plate-reader exports only), `rdml` (RDML
    /// 1.3; qPCR files: RDML, Applied Biosystems .eds, Rotor-Gene .rex), `parquet` or
    /// `arrow` (Arrow IPC file) for a table, a trace (all sweeps) or mass spectra (`spectra=true`),
    /// `nwb` (NWB 2.x; electrophysiology traces) or `jcamp` (JCAMP-DX 5.01; NMR FIDs and
    /// spectra, other 1-D spectra and chromatograms).
    pub format: Option<String>,
    /// Output path; defaults to the input with `.ome.tiff`, `.ome.zarr`, `.mzML`, `.asm.json`, `.rdml`,
    /// `.parquet`, `.arrow`, `.nwb` or `.jdx`.
    pub output: Option<String>,
    /// Replace an existing output file or directory.
    #[serde(default)]
    pub overwrite: bool,
    /// Write this embedded attachment instead (a CZI's `Thumbnail`, `Label` or `SlidePreview`
    /// image, `TimeStamps`; names from openreadout_info view=structure, kind attachment), or
    /// `#<index>`.
    pub attachment: Option<String>,
    /// Images: only this image index.
    pub image: Option<u32>,
    /// Images: plane selection strings such as `c=0`, `z=2-5`, `t=0,3`.
    #[serde(default)]
    pub select: Vec<String>,
    /// Images: export this source pyramid level as the full resolution (default 0).
    #[serde(default)]
    pub level: u32,
    /// Images: export only this rectangle `{x, y, width, height}` (pixels of `level`) of every
    /// plane, read tile by tile (cut a field out of a whole-slide image).
    pub region: Option<openreadout_core::Region>,
    /// OME-Zarr of a multi-well plate: only the fields of these wells (`C05`).
    #[serde(default)]
    pub wells: Vec<String>,
    /// Parquet, Arrow: this table index (FCS data set, plate read, event or peak table).
    pub table: Option<u32>,
    /// Parquet, Arrow, NWB, JCAMP-DX: this trace index (NWB default: every trace).
    pub trace: Option<u32>,
    /// Parquet, Arrow, NWB, JCAMP-DX: only this sweep (default: every sweep).
    pub sweep: Option<u32>,
    /// Parquet, Arrow, NWB, JCAMP-DX: rows (samples of each sweep for traces) `A-B`, `A-` or
    /// `A`, zero-based and inclusive.
    pub rows: Option<String>,
    /// Parquet, Arrow: export the mass spectra of `run` (one row per point, plus a
    /// `<name>.scans.parquet` per-scan summary) instead of a table or trace.
    #[serde(default)]
    pub spectra: bool,
    /// mzML, Parquet, Arrow: spectra run index (default 0).
    #[serde(default)]
    pub run: u32,
    /// mzML, Parquet, Arrow: write the instrument's centroid lists instead of profiles where a
    /// scan has both.
    #[serde(default)]
    pub centroid: bool,
    /// true: refuse (an error with exit_code 6 and a hint) values this file's assurance does not
    /// validate. Default: the server's setting (OPENREADOUT_STRICT; off).
    #[serde(default)]
    pub strict: Option<bool>,
}

/// `format = "rdml"`: a qPCR file to RDML 1.3.
fn export_rdml(registry: fn() -> Registry, a: &ExportArgs) -> Result<serde_json::Value, McpError> {
    let input = PathBuf::from(&a.file);
    let reg = with_strict(registry(), a.strict);
    let ds = openreadout_qpcr::open_qpcr(&reg, &input).map_err(|e| mcp_err(&e))?;
    let output = a.output.as_ref().map_or_else(
        || openreadout_qpcr::default_rdml_output(&input),
        PathBuf::from,
    );
    let r = openreadout_qpcr::export_rdml(&ds, &output, a.overwrite).map_err(|e| mcp_err(&e))?;
    to_value(&r)
}

/// `format = "asm"`: a plate-reader export to Allotrope Simple Model JSON.
fn export_asm(registry: fn() -> Registry, a: &ExportArgs) -> Result<serde_json::Value, McpError> {
    let input = PathBuf::from(&a.file);
    let reg = with_strict(registry(), a.strict);
    let (_, det) = reg.detect(&input).map_err(|e| mcp_err(&e))?;
    if det.format_id != openreadout_plate::FORMAT_ID {
        return Err(mcp_err(&Error::unsupported(
            "export",
            format!("ASM export of a `{}` file", det.format_id),
            "format=\"asm\" writes Allotrope plate-reader JSON and needs a plate-reader export (Gen5, SoftMax Pro, BMG, EnVision, Tecan, SkanIt, …).",
        )));
    }
    let output = a.output.as_ref().map_or_else(
        || {
            let stem = input
                .file_stem()
                .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
            input.with_file_name(format!("{stem}.asm.json"))
        },
        PathBuf::from,
    );
    let ds = openreadout_plate::PlateDataset::open(&input).map_err(|e| mcp_err(&e))?;
    let r = openreadout_plate::export_asm(&ds, &output, a.overwrite).map_err(|e| mcp_err(&e))?;
    to_value(&r)
}

/// `A-B` (inclusive), `A-` (to the end) or `A` (one row), zero-based.
fn parse_rows(spec: &str) -> Result<(u64, Option<u64>), McpError> {
    let bad = || {
        mcp_err(&Error::Usage(format!(
            "rows {spec:?}: expected A-B, A- or A (zero-based, inclusive)"
        )))
    };
    let s = spec.trim();
    let Some((a, b)) = s.split_once('-') else {
        let a: u64 = s.parse().map_err(|_| bad())?;
        return Ok((a, Some(a)));
    };
    let a: u64 = a.trim().parse().map_err(|_| bad())?;
    let b = b.trim();
    if b.is_empty() {
        return Ok((a, None));
    }
    let b: u64 = b.parse().map_err(|_| bad())?;
    if b < a {
        return Err(bad());
    }
    Ok((a, Some(b)))
}

/// `format = "nwb" | "jcamp" | "parquet" | "arrow"`: traces, tables and spectra to open formats.
fn export_open(
    registry: fn() -> Registry,
    a: &ExportArgs,
    format: &str,
) -> Result<serde_json::Value, McpError> {
    let input = PathBuf::from(&a.file);
    let (_, mut ds) = registry().open(&input).map_err(|e| mcp_err(&e))?;
    let rows = a.rows.as_deref().map(parse_rows).transpose()?;
    let output = a.output.as_ref().map(PathBuf::from);
    let usage = |m: &str| mcp_err(&Error::Usage(m.to_string()));
    let check_out = |o: &PathBuf| {
        if *o == input {
            Err(usage("output must differ from the input"))
        } else {
            Ok(())
        }
    };
    match format {
        "nwb" => {
            if a.table.is_some() || a.spectra {
                return Err(usage(
                    "NWB export writes traces: use trace/sweep, not table/spectra",
                ));
            }
            let mut o = openreadout_hdf5::NwbExportOptions::default();
            o.trace = a.trace;
            o.sweep = a.sweep;
            o.rows = rows;
            o.overwrite = a.overwrite;
            let out = output.unwrap_or_else(|| openreadout_hdf5::default_nwb_output(&input, &o));
            check_out(&out)?;
            let r = openreadout_hdf5::export_nwb(ds.as_mut(), &input, &out, &o)
                .map_err(|e| mcp_err(&e))?;
            to_value(&r)
        }
        "jcamp" => {
            if a.table.is_some() || a.spectra {
                return Err(usage(
                    "JCAMP-DX export writes traces: use trace/sweep, not table/spectra",
                ));
            }
            let mut o = openreadout_nmr::JcampExportOptions::default();
            o.trace = a.trace;
            o.sweep = a.sweep;
            o.rows = rows;
            o.overwrite = a.overwrite;
            o.encoding = openreadout_nmr::JcampEncoding::Difdup;
            let out = output.unwrap_or_else(|| openreadout_nmr::default_jcamp_output(&input, &o));
            check_out(&out)?;
            let r = openreadout_nmr::export_jcamp(ds.as_mut(), &input, &out, &o)
                .map_err(|e| mcp_err(&e))?;
            to_value(&r)
        }
        _ => export_columnar(ds.as_mut(), &input, output, a, format, rows),
    }
}

#[cfg(feature = "parquet")]
fn export_columnar(
    ds: &mut dyn openreadout_core::reader::Dataset,
    input: &Path,
    output: Option<PathBuf>,
    a: &ExportArgs,
    format: &str,
    rows: Option<(u64, Option<u64>)>,
) -> Result<serde_json::Value, McpError> {
    use openreadout_arrow::{ColumnarFormat, ColumnarSelection};
    let usage = |m: String| mcp_err(&Error::Usage(m));
    let mut o = openreadout_arrow::ColumnarOptions::default();
    o.format = if format == "arrow" {
        ColumnarFormat::ArrowIpc
    } else {
        ColumnarFormat::Parquet
    };
    o.select = match (a.table, a.trace, a.spectra) {
        (None, None, false) => ColumnarSelection::Auto,
        (Some(t), None, false) => ColumnarSelection::Table(t),
        (None, Some(t), false) => ColumnarSelection::Trace(t),
        (None, None, true) => ColumnarSelection::Spectra(a.run),
        _ => return Err(usage("give at most one of table, trace and spectra".into())),
    };
    o.sweep = a.sweep;
    o.rows = rows;
    o.centroid = a.centroid;
    o.overwrite = a.overwrite;
    let out = if let Some(p) = output {
        p
    } else {
        let info = ds.info().map_err(|e| mcp_err(&e))?;
        openreadout_arrow::default_output(input, &info, &o)
    };
    if out == input {
        return Err(usage("output must differ from the input".into()));
    }
    let r = openreadout_arrow::export_columnar(ds, input, &out, &o).map_err(|e| mcp_err(&e))?;
    to_value(&r)
}

#[cfg(not(feature = "parquet"))]
fn export_columnar(
    _ds: &mut dyn openreadout_core::reader::Dataset,
    _input: &Path,
    _output: Option<PathBuf>,
    _a: &ExportArgs,
    format: &str,
    _rows: Option<(u64, Option<u64>)>,
) -> Result<serde_json::Value, McpError> {
    Err(mcp_err(&Error::unsupported(
        "export",
        format!("{format} export"),
        "This build was compiled without the `parquet` cargo feature (on by default); use format=\"csv\" through the CLI instead.",
    )))
}

/// The export itself (blocking). `progress`, when given, receives `(done, total)` planes or
/// spectra as they are read.
fn export_blocking(
    registry: fn() -> Registry,
    a: ExportArgs,
    progress: Option<progress::ProgressFn>,
) -> Result<serde_json::Value, McpError> {
    let reg = with_strict(registry(), a.strict);
    let input = PathBuf::from(&a.file);
    if let Some(name) = &a.attachment {
        if a.format.is_some() {
            return Err(mcp_err(&Error::Usage(
                "attachment writes the attachment as stored: leave out format".into(),
            )));
        }
        let (det, mut ds) = reg.open(&input).map_err(|e| mcp_err(&e))?;
        let r = openreadout_ops::extract::extract_attachment(
            ds.as_mut(),
            &input,
            det.format_id,
            name,
            a.output.as_deref().map(Path::new),
            a.overwrite,
        )
        .map_err(|e| mcp_err(&e))?;
        return to_value(&r);
    }
    let requested = a.format.as_deref().map(str::to_ascii_lowercase);
    match requested.as_deref() {
        Some("parquet") => return export_open(registry, &a, "parquet"),
        Some("arrow" | "feather" | "ipc") => return export_open(registry, &a, "arrow"),
        Some("nwb") => return export_open(registry, &a, "nwb"),
        Some("jcamp" | "jcamp-dx" | "jdx") => return export_open(registry, &a, "jcamp"),
        _ => {}
    }
    if a.table.is_some() || a.trace.is_some() || a.sweep.is_some() || a.rows.is_some() || a.spectra
    {
        return Err(mcp_err(&Error::Usage(
            "table, trace, sweep, rows and spectra apply to format parquet, arrow, nwb and jcamp"
                .into(),
        )));
    }
    let is_ms = || {
        reg.detect(&input)
            .is_ok_and(|(r, _)| r.descriptor().family == "mass-spectrometry")
    };
    if requested.as_deref() == Some("mzml") || (requested.is_none() && is_ms()) {
        let (_, mut ds) = reg.open(&input).map_err(|e| mcp_err(&e))?;
        let total = ds
            .info()
            .ok()
            .and_then(|i| i.spectra.first().map(|s| s.scan_count))
            .unwrap_or(0);
        let output = a.output.map_or_else(
            || openreadout_mzml_writer::default_output(&input),
            PathBuf::from,
        );
        let mut wrapped;
        let dsr: &mut dyn openreadout_core::reader::Dataset = match progress {
            Some(p) => {
                wrapped = progress::ProgressDataset::new(ds.as_mut(), total, p);
                &mut wrapped
            }
            None => ds.as_mut(),
        };
        let r = openreadout_mzml_writer::export_mzml(dsr, &input, &output, &{
            let mut mzml_export_options = openreadout_mzml_writer::MzmlExportOptions::default();
            mzml_export_options.run = a.run;
            mzml_export_options.centroid = a.centroid;
            mzml_export_options.overwrite = a.overwrite;
            mzml_export_options.index_range = None;
            mzml_export_options
        })
        .map_err(|e| mcp_err(&e))?;
        return to_value(&r);
    }
    let zarr = match requested.as_deref().unwrap_or("ome-tiff") {
        "ome-tiff" | "ometiff" | "tiff" => false,
        "ome-zarr" | "omezarr" | "zarr" => true,
        "asm" | "allotrope" => return export_asm(registry, &a),
        "rdml" => return export_rdml(registry, &a),
        other => {
            return Err(mcp_err(&Error::Usage(format!(
                "unknown export format '{other}' (ome-tiff|ome-zarr|mzml|asm|rdml|parquet|arrow|nwb|jcamp)"
            ))));
        }
    };
    let codec = openreadout_ometiff::Codec::Deflate;
    let (_, mut ds) = reg.open(&input).map_err(|e| mcp_err(&e))?;
    let info = ds.info().map_err(|e| mcp_err(&e))?;
    if info.images.is_empty() && !info.tables.is_empty() {
        return Err(mcp_err(&Error::unsupported(
            "export",
            format!("image export of a {} file", info.format.name),
            "This file holds tables (events), not images: export with format=\"parquet\" or \"arrow\", read rows with openreadout_table, or run `openreadout export FILE --to csv`.",
        )));
    }
    if info.images.is_empty() && !info.traces.is_empty() {
        return Err(mcp_err(&Error::unsupported(
            "export",
            format!("image export of a {} file", info.format.name),
            "This file holds sampled signals or spectra (traces), not images: export with format=\"parquet\", \"nwb\" (electrophysiology) or \"jcamp\" (spectra), read samples and statistics with openreadout_trace, or run `openreadout export FILE --to csv --sweep N`.",
        )));
    }
    let output = a.output.map_or_else(
        || {
            if zarr {
                openreadout_omezarr::default_output(&input)
            } else {
                let stem = input
                    .file_stem()
                    .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
                input.with_file_name(format!("{stem}.ome.tiff"))
            }
        },
        PathBuf::from,
    );
    if output == input {
        return Err(mcp_err(&Error::Usage(
            "output must differ from the input".into(),
        )));
    }
    let total = progress::planes_to_export(&info, a.image, &a.select);
    let mut wrapped;
    let dsr: &mut dyn openreadout_core::reader::Dataset = match progress {
        Some(p) => {
            wrapped = progress::ProgressDataset::new(ds.as_mut(), total, p);
            &mut wrapped
        }
        None => ds.as_mut(),
    };
    let pyramid: openreadout_ometiff::PyramidMode = "auto".parse().map_err(|e| mcp_err(&e))?;
    let r = if zarr {
        let opts = {
            let mut zarr_export_options = openreadout_omezarr::ZarrExportOptions::default();
            zarr_export_options.image = a.image;
            zarr_export_options.select = a.select;
            zarr_export_options.codec = codec;
            zarr_export_options.overwrite = a.overwrite;
            zarr_export_options.chunk = openreadout_omezarr::DEFAULT_CHUNK;
            zarr_export_options.level = a.level;
            zarr_export_options.region = a.region;
            zarr_export_options.pyramid = pyramid;
            zarr_export_options.wells = a.wells;
            zarr_export_options
        };
        openreadout_omezarr::export_ome_zarr(dsr, &input, &output, &opts)
    } else {
        let opts = {
            let mut export_options = openreadout_ometiff::ExportOptions::default();
            export_options.image = a.image;
            export_options.select = a.select;
            export_options.codec = codec;
            export_options.overwrite = a.overwrite;
            export_options.level = a.level;
            export_options.region = a.region;
            export_options.pyramid = pyramid;
            export_options.tile = openreadout_ometiff::DEFAULT_TILE;
            export_options
        };
        openreadout_ometiff::export_ome_tiff(dsr, &input, &output, &opts)
    }
    .map_err(|e| mcp_err(&e))?;
    to_value(&r)
}

#[tool_router(router = export_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_export",
        annotations(
            title = "Export to an open format",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        ),
        description = "Convert to an open format: a new file, read back and verified; the source is never touched. format: ome-tiff (images, default; pyramidal when the source is), ome-zarr (multiscale; plates as OME-NGFF HCS), parquet or arrow (tables, traces with every sweep, spectra=true for MS points), mzml, nwb (electrophysiology), jcamp (NMR, IR/Raman, chromatograms), asm (plate readers), rdml (qPCR). attachment writes one embedded attachment (thumbnail, label, slide preview) as stored. Sends progress with a progressToken."
    )]
    pub(crate) async fn export(
        &self,
        Parameters(a): Parameters<ExportArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry;
        let token = ctx.meta.get_progress_token();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, u64)>();
        let reporter = token.as_ref().map(|_| {
            progress::throttled(move |done, total| {
                let _ = tx.send((done, total));
            })
        });
        let forward = token.map(|tok| {
            let peer = ctx.peer.clone();
            tokio::spawn(async move {
                while let Some((done, total)) = rx.recv().await {
                    let p = ProgressNotificationParam::new(tok.clone(), done as f64)
                        .with_total(total as f64)
                        .with_message(format!("read {done} of {total} planes/spectra"));
                    let _ = peer.notify_progress(p).await;
                }
            })
        });
        let r = tokio::task::spawn_blocking(move || export_blocking(registry, a, reporter))
            .await
            .map_err(|e| McpError::internal_error(format!("export task failed: {e}"), None))?;
        if let Some(f) = forward {
            let _ = f.await;
        }
        Ok(CallToolResult::structured(r?))
    }
}
