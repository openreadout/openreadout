//! Exports (`openreadout_export()`) and the qPCR request (`openreadout_qpcr()`): JSON options in, report out.

use std::path::Path;

use extendr_api::prelude::*;
use openreadout_core::{Error, Region, Result};
use serde::Deserialize;

use crate::with_ds;

/// Options of every export target; each target reads the ones it knows (as the CLI's `export`).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportArgs {
    image: Option<u32>,
    #[serde(default)]
    select: Vec<String>,
    compression: Option<String>,
    #[serde(default)]
    overwrite: bool,
    #[serde(default)]
    embed_vendor: bool,
    #[serde(default)]
    level: u32,
    region: Option<[u32; 4]>,
    pyramid: Option<String>,
    levels: Option<u32>,
    tile: Option<u32>,
    run: Option<u32>,
    #[serde(default)]
    centroid: bool,
    table: Option<u32>,
    trace: Option<u32>,
    sweep: Option<u32>,
    #[serde(default)]
    spectra: bool,
    rows: Option<(u64, Option<u64>)>,
}

fn codec(c: Option<&str>, zarr: bool) -> Result<openreadout_ometiff::Codec> {
    Ok(match c {
        None | Some("deflate") => openreadout_ometiff::Codec::Deflate,
        Some("gzip") if zarr => openreadout_ometiff::Codec::Deflate,
        Some("none") => openreadout_ometiff::Codec::None,
        Some("lzw") if !zarr => openreadout_ometiff::Codec::Lzw,
        Some(o) => {
            return Err(Error::Usage(format!(
                "unknown compression {o:?} (expected {})",
                if zarr {
                    "none or deflate"
                } else {
                    "none, deflate or lzw"
                }
            )));
        }
    })
}

fn region(r: Option<[u32; 4]>) -> Option<Region> {
    r.map(|[x, y, w, h]| Region::new(x, y, w, h))
}

/// Run one export; returns the target's report as a JSON value.
pub fn export(h: &Robj, to: &str, output: &str, options: &str) -> Result<serde_json::Value> {
    let a: ExportArgs = crate::parse("export options", options)?;
    let out = Path::new(output);
    let value = |r: std::result::Result<serde_json::Value, serde_json::Error>| {
        r.map_err(|e| Error::Other(format!("JSON serialization failed: {e}")))
    };
    match to {
        "ome-tiff" => {
            let mut o = openreadout_ometiff::ExportOptions::default();
            o.image = a.image;
            o.select = a.select;
            o.codec = codec(a.compression.as_deref(), false)?;
            o.overwrite = a.overwrite;
            o.embed_vendor = a.embed_vendor;
            o.level = a.level;
            o.region = region(a.region);
            o.pyramid = a.pyramid.as_deref().unwrap_or("auto").parse()?;
            o.levels = a.levels;
            if let Some(t) = a.tile {
                o.tile = t;
            }
            let r = with_ds(h, |ds, path| {
                openreadout_ometiff::export_ome_tiff(ds, Path::new(path), out, &o)
            })?;
            value(serde_json::to_value(r))
        }
        "ome-zarr" => {
            let mut o = openreadout_omezarr::ZarrExportOptions::default();
            o.image = a.image;
            o.select = a.select;
            o.codec = codec(a.compression.as_deref(), true)?;
            o.overwrite = a.overwrite;
            o.embed_vendor = a.embed_vendor;
            o.level = a.level;
            o.region = region(a.region);
            o.pyramid = a.pyramid.as_deref().unwrap_or("auto").parse()?;
            o.levels = a.levels;
            if let Some(t) = a.tile {
                o.chunk = t;
            }
            let r = with_ds(h, |ds, path| {
                openreadout_omezarr::export_ome_zarr(ds, Path::new(path), out, &o)
            })?;
            value(serde_json::to_value(r))
        }
        "mzml" => {
            let mut o = openreadout_mzml_writer::MzmlExportOptions::default();
            o.run = a.run.unwrap_or(0);
            o.centroid = a.centroid;
            o.overwrite = a.overwrite;
            let r = with_ds(h, |ds, path| {
                openreadout_mzml_writer::export_mzml(ds, Path::new(path), out, &o)
            })?;
            value(serde_json::to_value(r))
        }
        "parquet" | "arrow" => {
            use openreadout_arrow::{
                ColumnarCompression, ColumnarFormat, ColumnarOptions, ColumnarSelection,
            };
            let mut o = ColumnarOptions::default();
            o.format = if to == "arrow" {
                ColumnarFormat::ArrowIpc
            } else {
                ColumnarFormat::Parquet
            };
            o.compression = match a.compression.as_deref() {
                None => None,
                Some("none") => Some(ColumnarCompression::None),
                Some("snappy") => Some(ColumnarCompression::Snappy),
                Some("lz4") => Some(ColumnarCompression::Lz4),
                Some(c) => {
                    return Err(Error::Usage(format!(
                        "unknown compression {c:?} (expected none, snappy or lz4)"
                    )));
                }
            };
            o.select = match (a.table, a.trace, a.spectra) {
                (None, None, false) => ColumnarSelection::Auto,
                (Some(t), None, false) => ColumnarSelection::Table(t),
                (None, Some(t), false) => ColumnarSelection::Trace(t),
                (None, None, true) => ColumnarSelection::Spectra(a.run.unwrap_or(0)),
                _ => {
                    return Err(Error::Usage(
                        "give at most one of table, trace and spectra".into(),
                    ));
                }
            };
            o.sweep = a.sweep;
            o.rows = a.rows;
            o.centroid = a.centroid;
            o.overwrite = a.overwrite;
            let r = with_ds(h, |ds, path| {
                openreadout_arrow::export_columnar(ds, Path::new(path), out, &o)
            })?;
            value(serde_json::to_value(r))
        }
        "rdml" => {
            let input = with_ds(h, |_, path| Ok(path.to_string()))?;
            let ds = openreadout_qpcr::open_qpcr(&crate::registry(), Path::new(&input))?;
            let r = openreadout_qpcr::export_rdml(&ds, out, a.overwrite)?;
            value(serde_json::to_value(r))
        }
        other => Err(Error::Usage(format!(
            "unknown export target {other:?} (expected ome-tiff, ome-zarr, mzml, parquet, arrow or rdml)"
        ))),
    }
}

/// `openreadout_qpcr()` options (the Python `qpcr()` keywords).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QpcrArgs {
    well: Option<String>,
    target: Option<String>,
    sample: Option<String>,
    run: Option<String>,
    #[serde(default)]
    compute_cq: bool,
    cq_method: Option<openreadout_qpcr::CqMethod>,
    threshold: Option<f64>,
    baseline: Option<(u32, u32)>,
    #[serde(default)]
    ddcq: bool,
    #[serde(default)]
    reference_targets: Vec<String>,
    control_sample: Option<String>,
    #[serde(default)]
    standard_curve: bool,
    max_records: Option<usize>,
    undetermined_cq: Option<f64>,
}

impl QpcrArgs {
    pub fn request(self) -> openreadout_qpcr::QpcrReportRequest {
        let mut r = openreadout_qpcr::QpcrReportRequest::default();
        r.well = self.well;
        r.target = self.target;
        r.sample = self.sample;
        r.run = self.run;
        r.compute_cq = self.compute_cq;
        r.cq_method = self.cq_method;
        r.threshold = self.threshold;
        r.baseline = self.baseline;
        r.relative = self.ddcq;
        r.reference_targets = self.reference_targets;
        r.control_sample = self.control_sample;
        r.standard_curve = self.standard_curve;
        r.max_records = self.max_records;
        r.undetermined_cq = self.undetermined_cq;
        r
    }
}
