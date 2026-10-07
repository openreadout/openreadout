//! Native half of the `openreadout` Python package. Returns JSON strings and NumPy arrays;
//! the pure-Python wrapper (`python/openreadout/`) turns them into dicts and shaped arrays.
//!
//! Pixels, table rows, trace samples and spectra are decoded into a Rust `Vec` and handed to
//! NumPy without a copy (`rust-numpy` takes ownership of the buffer).
//!
//! Threading: a `NativeFile` may be shared between Python threads (dask's threaded scheduler
//! does this). It keeps a small pool of dataset handles on the same input: a pixel read takes
//! a free handle, or opens another one (up to [`max_handles`]) when every handle is busy, and
//! the GIL is released while it decodes, so concurrent reads of one file run in parallel.
#![forbid(unsafe_code)]
#![allow(clippy::needless_pass_by_value)]
// c, z, t, x, y, w, h are the natural names of plane indices and rectangles; the NumPy return
// tuples are what the Python side unpacks.
#![allow(clippy::many_single_char_names, clippy::type_complexity)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use numpy::{IntoPyArray, PyArray1};
use openreadout_core::model::DetectOutput;
use openreadout_core::reader::PlaneIndex;
use openreadout_core::source::{ByteSource, CachedSource, CallbackSource, Input, MemSource};
use openreadout_core::{Dataset, Error, Region, Registry};
use pyo3::exceptions::{PyIOError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedBytes;
use pyo3::types::PyBytes;

fn registry() -> Registry {
    Registry::new()
        .with(Box::new(openreadout_czi::CziReader))
        .with(Box::new(openreadout_lif::LifReader))
        .with(Box::new(openreadout_nd2::Nd2Reader))
        .with(Box::new(openreadout_thermo::ThermoRawReader))
        .with(Box::new(openreadout_fcs::FcsReader))
        .with(Box::new(openreadout_mzml::ImzmlReader))
        .with(Box::new(openreadout_mzml::MzmlReader))
        .with(Box::new(openreadout_mzml::MzxmlReader))
        .with(Box::new(openreadout_mzml::MzmlbReader))
        .with(Box::new(openreadout_bruker_tims::BrukerTimsReader))
        .with(Box::new(openreadout_oir::OirReader))
        .with(Box::new(openreadout_vsi::VsiReader))
        .with(Box::new(openreadout_wsi::MiraxReader))
        .with(Box::new(openreadout_zvi::ZviReader))
        .with(Box::new(openreadout_oif::OibReader))
        .with(Box::new(openreadout_oif::OifReader))
        .with(Box::new(openreadout_dcimg::DcimgReader))
        .with(Box::new(openreadout_qpcr::RdmlReader))
        .with(Box::new(openreadout_qpcr::EdsReader))
        .with(Box::new(openreadout_qpcr::PcrdReader))
        .with(Box::new(openreadout_qpcr::RexReader))
        .with(Box::new(openreadout_qpcr::IxoReader))
        .with(Box::new(openreadout_qpcr::ExportReader))
        .with(Box::new(openreadout_abf::AbfReader))
        .with(Box::new(openreadout_abf::AtfReader))
        .with(Box::new(openreadout_neuralynx::NeuralynxReader))
        .with(Box::new(openreadout_blackrock::BlackrockReader))
        .with(Box::new(openreadout_spikeglx::SpikeGlxReader))
        .with(Box::new(openreadout_intan::IntanReader))
        .with(Box::new(openreadout_plexon::PlexonReader))
        .with(Box::new(openreadout_ephys::HekaReader))
        .with(Box::new(openreadout_ephys::Spike2Reader))
        .with(Box::new(openreadout_ephys::WinWcpReader))
        .with(Box::new(openreadout_ephys::OpenEphysReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_chrom::OpenLabReader))
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_chrom::ShimadzuReader))
        .with(Box::new(openreadout_chrom::ChromeleonReader))
        .with(Box::new(openreadout_chrom::EmpowerArwReader))
        .with(Box::new(openreadout_hcs::HarmonyReader))
        .with(Box::new(openreadout_hcs::ImageXpressReader))
        .with(Box::new(openreadout_hcs::CellVoyagerReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_zarr::ZarrReader))
        .with(Box::new(openreadout_em::MrcReader))
        .with(Box::new(openreadout_em::DmReader))
        .with(Box::new(openreadout_em::SerReader))
        .with(Box::new(openreadout_em::EmdReader))
        .with(Box::new(openreadout_hdf5::ImsReader))
        .with(Box::new(openreadout_hdf5::NwbReader))
        .with(Box::new(openreadout_nmr::BrukerReader))
        .with(Box::new(openreadout_nmr::JcampReader))
        .with(Box::new(openreadout_nmr::VarianReader))
        .with(Box::new(openreadout_nmr::JeolReader))
        .with(Box::new(openreadout_nmr::SpinsolveReader))
        .with(Box::new(openreadout_spectro::OpusReader))
        .with(Box::new(openreadout_spectro::OmnicReader))
        .with(Box::new(openreadout_spectro::WdfReader))
        .with(Box::new(openreadout_spectro::PeSpReader))
        .with(Box::new(openreadout_spectro::JwsReader))
        .with(Box::new(openreadout_spectro::SpcReader))
        .with(Box::new(openreadout_spectro::WitecReader))
        .with(Box::new(openreadout_spectro::AgilentFpaReader))
        .with(Box::new(openreadout_spectro::FsmReader))
        .with(Box::new(openreadout_spectro::CaryReader))
        .with(Box::new(openreadout_biophys::ItcReader))
        .with(Box::new(openreadout_biophys::BiacoreReader))
        .with(Box::new(openreadout_biophys::BiacoreEvaluationReader))
        .with(Box::new(openreadout_biophys::SeahorseReader))
        .with(Box::new(openreadout_biophys::OctetReader))
        .with(Box::new(openreadout_biophys::ZetasizerReader))
        .with(Box::new(openreadout_biophys::GprReader))
        .with(Box::new(openreadout_gel::ImageLabReader))
        .with(Box::new(openreadout_fplc::UnicornResReader))
        .with(Box::new(openreadout_fplc::UnicornZipReader))
        .with(Box::new(openreadout_epr::Bes3tReader))
        .with(Box::new(openreadout_epr::EspReader))
        .with(Box::new(openreadout_xrd::XrdmlReader))
        .with(Box::new(openreadout_xrd::BrukerRawReader))
        .with(Box::new(openreadout_xrd::BrmlReader))
        .with(Box::new(openreadout_xrd::RasReader))
        .with(Box::new(openreadout_xrd::RasxReader))
        .with(Box::new(openreadout_echem::MprReader))
        .with(Box::new(openreadout_echem::MptReader))
        .with(Box::new(openreadout_echem::GamryReader))
        .with(Box::new(openreadout_echem::NdaReader))
        .with(Box::new(openreadout_echem::NdaxReader))
        .with(Box::new(openreadout_echem::ArbinReader))
        .with(Box::new(openreadout_thermal::NgbReader))
        .with(Box::new(openreadout_thermal::TaReader))
        .with(Box::new(openreadout_thermal::TriosReader))
        // loose: any other HDF5 file (after EMD, Imaris and NWB, which claim theirs definitely)
        .with(Box::new(openreadout_hdf5::Hdf5Reader))
        // last: text/CSV/XLSX exports; the generic plate-matrix fallback is the loosest sniff
        .with(Box::new(openreadout_plate::PlateReader))
}

/// Python exception class (defined in `openreadout/_errors.py`) for each core error.
fn error_class(e: &Error) -> &'static str {
    match e {
        Error::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            "InstrumentFileNotFoundError"
        }
        Error::Io { .. } => "InstrumentIOError",
        Error::UnknownFormat { .. } => "UnknownFormatError",
        Error::Usage(_) => "UsageError",
        Error::Corrupt { .. } => "CorruptFileError",
        Error::Unsupported { .. } => "UnsupportedFeatureError",
        _ => "OpenReadoutError",
    }
}

/// Convert a core error into the matching `openreadout` exception, carrying `code`,
/// `exit_code` and `hint` as attributes. Falls back to built-in exceptions if the pure-Python
/// half is not importable (e.g. `_native` loaded on its own).
fn to_py(py: Python<'_>, e: Error) -> PyErr {
    let hint = e.hint();
    let msg = match &hint {
        Some(h) => format!("{e} (hint: {h})"),
        None => e.to_string(),
    };
    let built = (|| -> PyResult<PyErr> {
        let module = py.import("openreadout._errors")?;
        let cls = module.getattr(error_class(&e))?;
        let exc = cls.call1((msg.clone(),))?;
        exc.setattr("code", e.code())?;
        exc.setattr("exit_code", e.exit_code())?;
        exc.setattr("hint", hint.clone())?;
        if let Error::Io { path, .. } | Error::UnknownFormat { path } = &e {
            exc.setattr("filename", path.display().to_string())?;
        }
        Ok(PyErr::from_value(exc))
    })();
    built.unwrap_or_else(|_| match e {
        Error::Io { .. } => PyIOError::new_err(msg),
        Error::Usage(_) | Error::UnknownFormat { .. } => PyValueError::new_err(msg),
        _ => PyRuntimeError::new_err(msg),
    })
}

fn json_err(e: serde_json::Error) -> PyErr {
    PyRuntimeError::new_err(format!("JSON serialization failed: {e}"))
}

/// Most dataset handles one `NativeFile` opens for concurrent reads: `OPENREADOUT_PY_HANDLES`,
/// else the number of CPUs (at most 4: readers decode tiles in parallel already).
fn max_handles() -> usize {
    std::env::var("OPENREADOUT_PY_HANDLES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(4)
        })
}

/// A handle taken from a [`NativeFile`]'s pool; returned when dropped, or discarded (and the
/// handle count lowered) when a panic unwinds through the read.
struct Lease<'a> {
    file: &'a NativeFile,
    ds: Option<Box<dyn Dataset>>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(ds) = self.ds.take() {
            if std::thread::panicking() {
                drop(ds);
                self.file.handles.fetch_sub(1, Ordering::AcqRel);
                self.file.ready.notify_all();
            } else {
                self.file.give_back(ds);
            }
        }
    }
}

/// An opened instrument file.
#[pyclass(name = "NativeFile", module = "openreadout._native")]
struct NativeFile {
    path: String,
    format: String,
    /// What was opened: reopened for extra handles.
    input: Input,
    closed: AtomicBool,
    /// Keep no handle open between calls: every call opens the input and closes it again
    /// (for hosts that must not hold file descriptors, such as bioio's plugin contract).
    transient: AtomicBool,
    /// Idle handles; `ready` is signalled when one is returned.
    pool: Mutex<Vec<Box<dyn Dataset>>>,
    ready: Condvar,
    /// Handles in existence (idle or in use).
    handles: AtomicUsize,
    max_handles: usize,
}

impl NativeFile {
    fn new(path: String, format: String, input: Input, ds: Box<dyn Dataset>) -> Self {
        NativeFile {
            path,
            format,
            input,
            closed: AtomicBool::new(false),
            transient: AtomicBool::new(false),
            pool: Mutex::new(vec![ds]),
            ready: Condvar::new(),
            handles: AtomicUsize::new(1),
            max_handles: max_handles(),
        }
    }

    /// A handle to read with: an idle one, else a new one when `grow` and below the limit,
    /// else the next one returned.
    fn take(&self, mut grow: bool) -> Result<Box<dyn Dataset>, String> {
        let closed = || format!("I/O operation on closed file: {}", self.path);
        let mut pool = self
            .pool
            .lock()
            .map_err(|_| format!("openreadout: dataset pool poisoned for {}", self.path))?;
        loop {
            if self.closed.load(Ordering::Acquire) {
                return Err(closed());
            }
            if let Some(ds) = pool.pop() {
                return Ok(ds);
            }
            let n = self.handles.load(Ordering::Acquire);
            // No handle left at all (transient mode, or one was discarded after a panic):
            // always open one.
            if (grow && n < self.max_handles) || n == 0 || self.transient.load(Ordering::Acquire) {
                self.handles.fetch_add(1, Ordering::AcqRel);
                drop(pool);
                let reg = registry();
                let opened = reg
                    .by_id(&self.format)
                    .ok_or_else(|| format!("format {} is not registered", self.format))
                    .and_then(|r| r.open_input(&self.input).map_err(|e| e.to_string()));
                let err = match opened {
                    Ok(ds) => return Ok(ds),
                    Err(e) => e,
                };
                self.handles.fetch_sub(1, Ordering::AcqRel);
                if n == 0 {
                    return Err(err);
                }
                // Could not open another handle (file gone, descriptors exhausted): wait for
                // one of the existing ones instead.
                grow = false;
                pool = self
                    .pool
                    .lock()
                    .map_err(|_| format!("openreadout: dataset pool poisoned for {}", self.path))?;
                continue;
            }
            pool = self
                .ready
                .wait(pool)
                .map_err(|_| format!("openreadout: dataset pool poisoned for {}", self.path))?;
        }
    }

    /// Return a handle to the pool (dropped when the file was closed meanwhile).
    fn give_back(&self, ds: Box<dyn Dataset>) {
        if let Ok(mut pool) = self.pool.lock() {
            if self.closed.load(Ordering::Acquire) || self.transient.load(Ordering::Acquire) {
                self.handles.fetch_sub(1, Ordering::AcqRel);
            } else {
                pool.push(ds);
            }
        }
        self.ready.notify_one();
    }

    /// Run `f` on a dataset handle with the GIL released. Handles are only taken with the GIL
    /// released, so a thread waiting for another thread's read never blocks the interpreter.
    /// Pixel reads pass `grow = true` to open another handle when all are busy.
    fn with_ds_opt<T: Send>(
        &self,
        py: Python<'_>,
        grow: bool,
        f: impl FnOnce(&mut dyn Dataset) -> openreadout_core::Result<T> + Send,
    ) -> PyResult<T> {
        let result = py.detach(|| -> Result<openreadout_core::Result<T>, String> {
            let mut lease = Lease {
                file: self,
                ds: Some(self.take(grow)?),
            };
            let ds = lease
                .ds
                .as_mut()
                .ok_or_else(|| "openreadout: dataset lease is empty".to_string())?;
            Ok(f(ds.as_mut()))
        });
        match result {
            Ok(r) => r.map_err(|e| to_py(py, e)),
            Err(msg) => Err(PyValueError::new_err(msg)),
        }
    }

    /// [`Self::with_ds_opt`] on an existing handle (metadata and whole-file operations).
    fn with_ds<T: Send>(
        &self,
        py: Python<'_>,
        f: impl FnOnce(&mut dyn Dataset) -> openreadout_core::Result<T> + Send,
    ) -> PyResult<T> {
        self.with_ds_opt(py, false, f)
    }
}

#[pymethods]
impl NativeFile {
    #[getter]
    fn path(&self) -> &str {
        &self.path
    }
    #[getter]
    fn format(&self) -> &str {
        &self.format
    }
    /// True after `close()`.
    #[getter]
    fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    /// Release the file handles. Idempotent; a read in flight on another thread finishes and
    /// its handle is released when it returns.
    fn close(&self, py: Python<'_>) {
        self.closed.store(true, Ordering::Release);
        py.detach(|| {
            if let Ok(mut pool) = self.pool.lock() {
                let n = pool.len();
                pool.clear();
                self.handles.fetch_sub(n, Ordering::AcqRel);
            }
            self.ready.notify_all();
        });
    }
    /// Transient mode: keep no handle open between calls (each call reopens the input). Setting
    /// it closes the idle handles now.
    fn set_transient(&self, py: Python<'_>, on: bool) {
        self.transient.store(on, Ordering::Release);
        if on {
            py.detach(|| {
                if let Ok(mut pool) = self.pool.lock() {
                    let n = pool.len();
                    pool.clear();
                    self.handles.fetch_sub(n, Ordering::AcqRel);
                }
                self.ready.notify_all();
            });
        }
    }
    /// Dataset handles open right now (idle or reading).
    #[getter]
    fn handle_count(&self) -> usize {
        self.handles.load(Ordering::Acquire)
    }
    /// `info --json` data (`FileInfo` plus the derived `experiment`) as a JSON string.
    fn info_json(&self, py: Python<'_>) -> PyResult<String> {
        let out = self.with_ds(py, |ds| {
            let info = ds.info()?;
            Ok(openreadout_core::InfoOutput::new(ds, info))
        })?;
        serde_json::to_string(&out).map_err(json_err)
    }
    /// Per-well statistics of a multi-well plate (`stats --per well --json` data) as a JSON string.
    #[pyo3(signature = (select=Vec::new(), wells=Vec::new(), per_field=false))]
    fn well_stats_json(
        &self,
        py: Python<'_>,
        select: Vec<String>,
        wells: Vec<String>,
        per_field: bool,
    ) -> PyResult<String> {
        let out = self.with_ds(py, |ds| {
            let info = ds.info()?;
            let mut req = openreadout_core::plate::WellStatsRequest::default();
            req.select = select;
            req.wells = wells;
            req.per_field = per_field;
            openreadout_core::plate::well_stats(
                ds,
                &info,
                &req,
                &openreadout_core::parallel::ReadContext::default(),
            )
        })?;
        serde_json::to_string(&out).map_err(json_err)
    }
    /// Pixel statistics (`stats --json` data) as a JSON string: per plane, per image × channel
    /// and per image, with image and channel names. `mip`: `"z"` or `"t"` projects first.
    #[pyo3(signature = (image=None, select=Vec::new(), level=0, region=None, bins=0, log_bins=false, per_plane=true, mip=None))]
    #[allow(clippy::too_many_arguments)]
    fn stats_json(
        &self,
        py: Python<'_>,
        image: Option<u32>,
        select: Vec<String>,
        level: u32,
        region: Option<(u32, u32, u32, u32)>,
        bins: u32,
        log_bins: bool,
        per_plane: bool,
        mip: Option<String>,
    ) -> PyResult<String> {
        let mip = match mip.as_deref() {
            None => None,
            Some("z" | "Z") => Some(openreadout_core::stats::Projection::Z),
            Some("t" | "T") => Some(openreadout_core::stats::Projection::T),
            Some(other) => {
                return Err(PyValueError::new_err(format!(
                    "mip must be 'z' or 't', not {other:?}"
                )));
            }
        };
        let out = self.with_ds(py, |ds| {
            let info = ds.info()?;
            let mut req = openreadout_core::stats::StatsRequest::default();
            req.image = image;
            req.select = select;
            req.level = level;
            req.region = region.map(|(x, y, w, h)| Region::new(x, y, w, h));
            req.bins = bins;
            req.scale = if log_bins {
                openreadout_core::stats::HistogramScale::Log
            } else {
                openreadout_core::stats::HistogramScale::Linear
            };
            req.per_plane = per_plane;
            req.mip = mip;
            openreadout_core::stats::compute_stats(
                ds,
                &info,
                &req,
                &openreadout_core::parallel::ReadContext::default(),
            )
        })?;
        serde_json::to_string(&out).map_err(json_err)
    }
    /// `info --view full --json` data: the summary with per-frame records, the vendor metadata
    /// tree (with `vendor`) and the provenance map, as a JSON string.
    fn full_json(&self, py: Python<'_>, vendor: bool) -> PyResult<String> {
        let out = self.with_ds(py, |ds| {
            let mut info = ds.info()?;
            openreadout_core::reader::attach_frames(
                ds,
                &mut info,
                Some(openreadout_core::reader::DEFAULT_FRAME_RECORDS),
            )?;
            Ok(openreadout_core::model::Dump {
                file: openreadout_core::InfoOutput::new(ds, info),
                vendor: if vendor {
                    Some(ds.vendor_metadata()?)
                } else {
                    None
                },
                provenance: ds.provenance(),
            })
        })?;
        serde_json::to_string(&out).map_err(json_err)
    }
    /// Vendor metadata tree as a JSON string.
    fn vendor_json(&self, py: Python<'_>) -> PyResult<String> {
        let v = self.with_ds(py, |ds| ds.vendor_metadata())?;
        serde_json::to_string(&v).map_err(json_err)
    }
    /// Provenance map as a JSON string.
    fn provenance_json(&self, py: Python<'_>) -> PyResult<String> {
        let p = self.with_ds(py, |ds| Ok(ds.provenance()))?;
        serde_json::to_string(&p).map_err(json_err)
    }
    /// `LsEntry` list as a JSON string.
    fn entries_json(&self, py: Python<'_>) -> PyResult<String> {
        let e = self.with_ds(py, |ds| ds.entries())?;
        serde_json::to_string(&e).map_err(json_err)
    }
    /// `CheckReport` as a JSON string.
    fn check_json(&self, py: Python<'_>) -> PyResult<String> {
        let r = self.with_ds(py, |ds| ds.check())?;
        serde_json::to_string(&r).map_err(json_err)
    }
    /// OME-XML (2016-06) describing every image, with `MetadataOnly` pixels.
    fn ome_xml(&self, py: Python<'_>) -> PyResult<String> {
        let info = self.with_ds(py, |ds| ds.info())?;
        let images: Vec<openreadout_ometiff::WrittenImage<'_>> = info
            .images
            .iter()
            .map(openreadout_ometiff::WrittenImage::whole)
            .collect();
        let creator = concat!("openreadout ", env!("CARGO_PKG_VERSION"));
        openreadout_ometiff::build_ome_xml_metadata_only(&info, &images, creator, None)
            .map_err(PyRuntimeError::new_err)
    }
    /// One plane, or the rectangle `region = (x, y, width, height)` of it, at pyramid `level`:
    /// `(width, height, samples_per_pixel, numpy_dtype, data)` where `data` is a 1-D uint8
    /// array owning the decoded buffer (no copy): little-endian samples, contiguous rows,
    /// interleaved RGB.
    #[pyo3(signature = (image, c, z, t, level=0, region=None))]
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn read_plane<'py>(
        &self,
        py: Python<'py>,
        image: u32,
        c: u32,
        z: u32,
        t: u32,
        level: u32,
        region: Option<(u32, u32, u32, u32)>,
    ) -> PyResult<(u32, u32, u32, &'static str, Bound<'py, PyArray1<u8>>)> {
        let idx = PlaneIndex { c, z, t };
        let p = self.with_ds_opt(py, true, move |ds| match region {
            Some((x, y, w, h)) => ds.read_region(image, idx, level, Region::new(x, y, w, h)),
            None => ds.read_plane_level(image, idx, level),
        })?;
        if p.data.len() != p.expected_len() {
            return Err(PyRuntimeError::new_err(format!(
                "plane has {} bytes, expected {}",
                p.data.len(),
                p.expected_len()
            )));
        }
        let dtype = p.pixel_type.numpy_dtype();
        Ok((
            p.width,
            p.height,
            p.samples_per_pixel,
            dtype,
            p.data.into_pyarray(py),
        ))
    }
    /// Rows of a table (FCS data set): `(rows, columns, values)` where `values` is a float64
    /// array, column-major (all of column 0, then column 1, ...).
    #[pyo3(signature = (table=0, first_row=0, max_rows=None))]
    fn read_table<'py>(
        &self,
        py: Python<'py>,
        table: u32,
        first_row: u64,
        max_rows: Option<u64>,
    ) -> PyResult<(u64, u32, Bound<'py, PyArray1<f64>>)> {
        let (rows, cols, values) = self.with_ds(py, |ds| {
            let t = ds.read_table(table, first_row, max_rows.unwrap_or(u64::MAX))?;
            let rows = t.columns.first().map_or(0, Vec::len);
            let mut values = Vec::with_capacity(rows * t.columns.len());
            for c in &t.columns {
                values.extend_from_slice(c);
            }
            Ok((rows as u64, t.columns.len() as u32, values))
        })?;
        Ok((rows, cols, values.into_pyarray(py)))
    }
    /// Samples of one sweep of a trace: `(channels, samples, values)` where `values` is a float64
    /// array of the scaled values, channel-major (all of channel 0, then channel 1, ...).
    #[pyo3(signature = (trace=0, sweep=0, first_sample=0, max_samples=None))]
    fn read_trace<'py>(
        &self,
        py: Python<'py>,
        trace: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: Option<u64>,
    ) -> PyResult<(u32, u64, Bound<'py, PyArray1<f64>>)> {
        let (chans, n, values) = self.with_ds(py, |ds| {
            let t = ds.read_trace(trace, sweep, first_sample, max_samples.unwrap_or(u64::MAX))?;
            let n = t.channels.first().map_or(0, Vec::len);
            let mut values = Vec::with_capacity(n * t.channels.len());
            for c in &t.channels {
                if c.len() != n {
                    return Err(Error::Other(format!(
                        "reader returned channels of unequal length ({} vs {n})",
                        c.len()
                    )));
                }
                values.extend_from_slice(c);
            }
            Ok((t.channels.len() as u32, n as u64, values))
        })?;
        Ok((chans, n, values.into_pyarray(py)))
    }
    /// `analyze KIND` on this file (`options`: the JSON object of the arguments of that kind's
    /// MCP tool) for the kinds that read one data set (peaks, chromatogram,
    /// nmr-peaks, ephys-features, spikes) → the analysis output as JSON.
    fn analyze_json(&self, py: Python<'_>, kind: String, options: String) -> PyResult<String> {
        let kind = analyze_kind(py, &kind)?;
        let o = options_map(&options)?;
        let out = self.with_ds(py, move |ds| {
            let info = ds.info()?;
            openreadout_batch::analyze::run_dataset(kind, &o, ds, &info, &Default::default())
        })?;
        serde_json::to_string(&out).map_err(json_err)
    }
    /// `info --view explain` data: what the file is, in sentences, answering `ask` when given.
    #[pyo3(signature = (ask=None))]
    fn explain_json(&self, py: Python<'_>, ask: Option<String>) -> PyResult<String> {
        let out = self.with_ds(py, move |ds| {
            let mut info = ds.info()?;
            let experiment = openreadout_core::experiment::of_dataset(ds, &info);
            let _ = openreadout_core::reader::attach_frames(ds, &mut info, Some(1));
            let assurance = openreadout_core::assurance::assess_dataset(ds, &info);
            Ok(
                openreadout_ops::explain::explain_with(&info, Some(&experiment), ask.as_deref())
                    .with_assurance(assurance),
            )
        })?;
        serde_json::to_string(&out).map_err(json_err)
    }
    /// Scan headers of run `run` without decoding peaks: `filter` is a `ScanFilter` as JSON
    /// (`{}` for every scan) → `ScanList` JSON (every match counted, `limit` listed from the
    /// `offset`-th).
    #[pyo3(signature = (run, filter, offset=0, limit=u64::MAX))]
    fn scans_json(
        &self,
        py: Python<'_>,
        run: u32,
        filter: String,
        offset: u64,
        limit: u64,
    ) -> PyResult<String> {
        let f: openreadout_core::ScanFilter = serde_json::from_str(&filter)
            .map_err(|e| PyValueError::new_err(format!("scan filter: {e}")))?;
        let (path, format) = (self.path.clone(), self.format.clone());
        let out = self.with_ds(py, move |ds| {
            openreadout_core::scans::scan_list(ds, &path, &format, run, &f, offset, limit)
        })?;
        serde_json::to_string(&out).map_err(json_err)
    }
    /// Like `read_spectrum`, addressed by the instrument's scan number (through the reader's
    /// scan-number map when it has one: mzML/mzXML native ids, timsTOF spectrum numbers).
    #[pyo3(signature = (run, scan, centroid=false))]
    fn read_spectrum_scan<'py>(
        &self,
        py: Python<'py>,
        run: u32,
        scan: u64,
        centroid: bool,
    ) -> PyResult<(String, Bound<'py, PyArray1<f64>>, Bound<'py, PyArray1<f32>>)> {
        let view = if centroid {
            openreadout_core::SpectrumView::Centroid
        } else {
            openreadout_core::SpectrumView::Primary
        };
        let mut sp = self.with_ds(py, move |ds| {
            openreadout_core::reader::spectrum_by_scan(ds, run, scan, view)
        })?;
        let mz = std::mem::take(&mut sp.mz);
        let it = std::mem::take(&mut sp.intensity);
        let meta = serde_json::to_string(&sp).map_err(json_err)?;
        Ok((meta, mz.into_pyarray(py), it.into_pyarray(py)))
    }
    /// Export to OME-TIFF (pyramidal and tiled when asked for or when the source has a pyramid);
    /// returns the `ExportReport` as JSON.
    #[pyo3(signature = (output, image=None, select=None, compression="deflate", overwrite=false, embed_vendor=false, level=0, region=None, pyramid="auto", levels=None, tile=512))]
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    fn export_ome_tiff(
        &self,
        py: Python<'_>,
        output: String,
        image: Option<u32>,
        select: Option<Vec<String>>,
        compression: &str,
        overwrite: bool,
        embed_vendor: bool,
        level: u32,
        region: Option<(u32, u32, u32, u32)>,
        pyramid: &str,
        levels: Option<u32>,
        tile: u32,
    ) -> PyResult<String> {
        let codec = match compression {
            "none" => openreadout_ometiff::Codec::None,
            "deflate" => openreadout_ometiff::Codec::Deflate,
            "lzw" => openreadout_ometiff::Codec::Lzw,
            other => {
                return Err(to_py(
                    py,
                    Error::Usage(format!(
                        "unknown compression '{other}' (expected none, deflate or lzw)"
                    )),
                ));
            }
        };
        let opts = {
            let mut export_options = openreadout_ometiff::ExportOptions::default();
            export_options.image = image;
            export_options.select = select.unwrap_or_default();
            export_options.codec = codec;
            export_options.overwrite = overwrite;
            export_options.embed_vendor = embed_vendor;
            export_options.level = level;
            export_options.region = region.map(|(x, y, w, h)| Region::new(x, y, w, h));
            export_options.pyramid = pyramid.parse().map_err(|e| to_py(py, e))?;
            export_options.levels = levels;
            export_options.tile = tile;
            export_options
        };
        let input = self.path.clone();
        let r = self.with_ds(py, move |ds| {
            openreadout_ometiff::export_ome_tiff(ds, Path::new(&input), Path::new(&output), &opts)
        })?;
        serde_json::to_string(&r).map_err(json_err)
    }
    /// Export to an OME-Zarr store (NGFF 0.5, Zarr v3) with a pyramid; returns the
    /// `ExportReport` as JSON.
    #[pyo3(signature = (output, image=None, select=None, compression="deflate", overwrite=false, embed_vendor=false, level=0, region=None, pyramid="auto", levels=None, chunk=512))]
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    fn export_ome_zarr(
        &self,
        py: Python<'_>,
        output: String,
        image: Option<u32>,
        select: Option<Vec<String>>,
        compression: &str,
        overwrite: bool,
        embed_vendor: bool,
        level: u32,
        region: Option<(u32, u32, u32, u32)>,
        pyramid: &str,
        levels: Option<u32>,
        chunk: u32,
    ) -> PyResult<String> {
        let codec = match compression {
            "none" => openreadout_ometiff::Codec::None,
            "deflate" | "gzip" => openreadout_ometiff::Codec::Deflate,
            other => {
                return Err(to_py(
                    py,
                    Error::Usage(format!(
                        "unknown compression '{other}' (expected none or deflate)"
                    )),
                ));
            }
        };
        let opts = {
            let mut o = openreadout_omezarr::ZarrExportOptions::default();
            o.image = image;
            o.select = select.unwrap_or_default();
            o.codec = codec;
            o.overwrite = overwrite;
            o.embed_vendor = embed_vendor;
            o.level = level;
            o.region = region.map(|(x, y, w, h)| Region::new(x, y, w, h));
            o.pyramid = pyramid.parse().map_err(|e| to_py(py, e))?;
            o.levels = levels;
            o.chunk = chunk;
            o
        };
        let input = self.path.clone();
        let r = self.with_ds(py, move |ds| {
            openreadout_omezarr::export_ome_zarr(ds, Path::new(&input), Path::new(&output), &opts)
        })?;
        serde_json::to_string(&r).map_err(json_err)
    }
    /// One mass spectrum: `(metadata_json, mz, intensity)` where the arrays are float64 (m/z)
    /// and float32 (intensity), handed over without a copy; `metadata_json` is the `Spectrum`
    /// without its arrays.
    #[pyo3(signature = (run, index, centroid=false))]
    fn read_spectrum<'py>(
        &self,
        py: Python<'py>,
        run: u32,
        index: u64,
        centroid: bool,
    ) -> PyResult<(String, Bound<'py, PyArray1<f64>>, Bound<'py, PyArray1<f32>>)> {
        let view = if centroid {
            openreadout_core::SpectrumView::Centroid
        } else {
            openreadout_core::SpectrumView::Primary
        };
        let mut sp = self.with_ds(py, move |ds| ds.read_spectrum_view(run, index, view))?;
        let mz = std::mem::take(&mut sp.mz);
        let it = std::mem::take(&mut sp.intensity);
        let meta = serde_json::to_string(&sp).map_err(json_err)?;
        Ok((meta, mz.into_pyarray(py), it.into_pyarray(py)))
    }
    /// Export the spectra to indexed mzML; returns the `MzmlExportReport` as JSON.
    #[pyo3(signature = (output, run=0, centroid=false, overwrite=false))]
    fn export_mzml(
        &self,
        py: Python<'_>,
        output: String,
        run: u32,
        centroid: bool,
        overwrite: bool,
    ) -> PyResult<String> {
        let opts = {
            let mut mzml_export_options = openreadout_mzml_writer::MzmlExportOptions::default();
            mzml_export_options.run = run;
            mzml_export_options.centroid = centroid;
            mzml_export_options.overwrite = overwrite;
            mzml_export_options.index_range = None;
            mzml_export_options
        };
        let input = self.path.clone();
        let r = self.with_ds(py, move |ds| {
            openreadout_mzml_writer::export_mzml(ds, Path::new(&input), Path::new(&output), &opts)
        })?;
        serde_json::to_string(&r).map_err(json_err)
    }
    /// A table, a trace (every sweep, or one) or the spectra of a run as a `pyarrow.Table`,
    /// with the same columns and field/schema metadata as `export --format parquet`. The record
    /// batches are handed over through the Arrow C data interface (no copy). `per_scan=True`
    /// (with `spectra=True`) returns the per-scan summary instead of the points. `max_rows`
    /// caps the rows read (an error beyond it).
    #[pyo3(signature = (table=None, trace=None, sweep=None, first_row=None, last_row=None, spectra=false, run=0, centroid=false, per_scan=false, max_rows=None))]
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    fn to_arrow<'py>(
        &self,
        py: Python<'py>,
        table: Option<u32>,
        trace: Option<u32>,
        sweep: Option<u32>,
        first_row: Option<u64>,
        last_row: Option<u64>,
        spectra: bool,
        run: u32,
        centroid: bool,
        per_scan: bool,
        max_rows: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        use arrow_pyarrow::IntoPyArrow;
        use openreadout_arrow::ColumnarSelection;
        let select = match (table, trace, spectra) {
            (None, None, false) => ColumnarSelection::Auto,
            (Some(t), None, false) => ColumnarSelection::Table(t),
            (None, Some(t), false) => ColumnarSelection::Trace(t),
            (None, None, true) => ColumnarSelection::Spectra(run),
            _ => {
                return Err(to_py(
                    py,
                    Error::Usage("give at most one of table, trace and spectra".into()),
                ));
            }
        };
        let mut opts = openreadout_arrow::ColumnarOptions::default();
        opts.select = select;
        opts.sweep = sweep;
        opts.rows = match (first_row, last_row) {
            (None, None) => None,
            (a, b) => Some((a.unwrap_or(0), b)),
        };
        opts.centroid = centroid;
        let input = self.path.clone();
        let data = self.with_ds(py, move |ds| {
            openreadout_arrow::read_columnar(ds, Path::new(&input), &opts, max_rows)
        })?;
        let (batches, schema) = if per_scan {
            let s = data.summary.ok_or_else(|| {
                to_py(py, Error::Usage("per_scan=True needs spectra=True".into()))
            })?;
            let schema = s.schema();
            (vec![s], schema)
        } else {
            (data.batches, data.schema)
        };
        let t = arrow_pyarrow::Table::try_new(batches, schema)
            .map_err(|e| PyRuntimeError::new_err(e.to_string()))?;
        t.into_pyarrow(py)
    }
    fn __repr__(&self) -> String {
        format!("NativeFile({:?}, format={:?})", self.path, self.format)
    }
}

/// Open a file (format detected from its signature).
#[pyfunction]
fn open(py: Python<'_>, path: String) -> PyResult<NativeFile> {
    let input = Input::local(&path);
    let opened = py.detach(|| registry().open_input(&input));
    let (det, ds) = opened.map_err(|e| to_py(py, e))?;
    Ok(NativeFile::new(path, det.format_id.to_string(), input, ds))
}

/// The bytes of a `bytes` object, shared without copying (a `bytearray` is copied once).
struct PyBuffer(PyBackedBytes);

impl AsRef<[u8]> for PyBuffer {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Seek + read on a binary file-like object, one call at a time. Reads re-acquire the GIL; the
/// lock keeps a `seek` and its `read` together when several threads read one object.
fn fileobj_source(
    py: Python<'_>,
    obj: Py<PyAny>,
    name: &str,
    size: Option<u64>,
) -> PyResult<Input> {
    let size = if let Some(s) = size {
        s
    } else {
        let o = obj.bind(py);
        let end: u64 = o.call_method1("seek", (0, 2))?.extract()?;
        o.call_method1("seek", (0,))?;
        end
    };
    let lock = Mutex::new(());
    let read = move |offset: u64, buf: &mut [u8]| -> std::io::Result<usize> {
        let _one_at_a_time = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Python::attach(|py| -> PyResult<usize> {
            let o = obj.bind(py);
            o.call_method1("seek", (offset,))?;
            let got = o.call_method1("read", (buf.len(),))?;
            let bytes: PyBackedBytes = got.extract()?;
            let n = bytes.len().min(buf.len());
            buf[..n].copy_from_slice(&bytes[..n]);
            Ok(n)
        })
        .map_err(|e| std::io::Error::other(format!("reading the file object failed: {e}")))
    };
    let src: Arc<dyn ByteSource> = Arc::new(CallbackSource::new(name, size, read));
    let cached: Arc<dyn ByteSource> = Arc::new(CachedSource::with_defaults(src));
    Ok(Input::from_source(name, cached))
}

fn bytes_input(data: PyBackedBytes, name: &str) -> Input {
    Input::from_source(name, Arc::new(MemSource::new(name, PyBuffer(data))))
}

fn open_input(py: Python<'_>, input: Input) -> PyResult<NativeFile> {
    let opened = py.detach(|| registry().open_input(&input));
    let (det, ds) = opened.map_err(|e| to_py(py, e))?;
    Ok(NativeFile::new(
        input.path().display().to_string(),
        det.format_id.to_string(),
        input,
        ds,
    ))
}

/// Open a file held in memory (`bytes` or `bytearray`); `name` stands in for the path (its
/// extension helps detection).
#[pyfunction]
fn open_bytes(py: Python<'_>, data: PyBackedBytes, name: String) -> PyResult<NativeFile> {
    open_input(py, bytes_input(data, &name))
}

/// Open a binary file-like object (`seek` and `read`), read on demand through a block cache.
#[pyfunction]
#[pyo3(signature = (fileobj, name, size=None))]
fn open_fileobj(
    py: Python<'_>,
    fileobj: Py<PyAny>,
    name: String,
    size: Option<u64>,
) -> PyResult<NativeFile> {
    let input = fileobj_source(py, fileobj, &name, size)?;
    open_input(py, input)
}

fn detect_input_json(py: Python<'_>, input: &Input) -> PyResult<String> {
    let reg = registry();
    let found = py.detach(|| {
        reg.detect_input(input)
            .map(|(r, d)| (r.descriptor().name, d.format_id, d.confidence, d.note))
    });
    let (name, format, confidence, note) = found.map_err(|e| to_py(py, e))?;
    serde_json::to_string(&DetectOutput {
        path: input.path().display().to_string(),
        format: format.to_string(),
        name,
        confidence,
        note,
    })
    .map_err(json_err)
}

/// [`detect_json`] for bytes held in memory.
#[pyfunction]
fn detect_bytes_json(py: Python<'_>, data: PyBackedBytes, name: String) -> PyResult<String> {
    detect_input_json(py, &bytes_input(data, &name))
}

/// [`detect_json`] for a binary file-like object.
#[pyfunction]
#[pyo3(signature = (fileobj, name, size=None))]
fn detect_fileobj_json(
    py: Python<'_>,
    fileobj: Py<PyAny>,
    name: String,
    size: Option<u64>,
) -> PyResult<String> {
    let input = fileobj_source(py, fileobj, &name, size)?;
    detect_input_json(py, &input)
}

/// Detect the format from the file's signature without opening it; `DetectOutput` as JSON.
#[pyfunction]
fn detect_json(py: Python<'_>, path: String) -> PyResult<String> {
    let reg = registry();
    let found = py.detach(|| {
        reg.detect(Path::new(&path))
            .map(|(r, d)| (r.descriptor().name, d.format_id, d.confidence, d.note))
    });
    let (name, format, confidence, note) = found.map_err(|e| to_py(py, e))?;
    serde_json::to_string(&DetectOutput {
        path,
        format: format.to_string(),
        name,
        confidence,
        note,
    })
    .map_err(json_err)
}

fn analyze_kind(py: Python<'_>, name: &str) -> PyResult<openreadout_batch::analyze::AnalyzeKind> {
    openreadout_batch::analyze::AnalyzeKind::parse(name).map_err(|e| to_py(py, e))
}

fn options_map(json: &str) -> PyResult<serde_json::Map<String, serde_json::Value>> {
    serde_json::from_str(json).map_err(|e| PyValueError::new_err(format!("analysis options: {e}")))
}

/// `analyze KIND FILE` (`options`: the JSON object of the arguments of that
/// kind's MCP tool) → the analysis output as JSON and, for an assay with `"plot": true`, the curve as
/// PNG bytes (`None` when nothing was fitted).
#[pyfunction]
fn analyze_json(
    py: Python<'_>,
    path: String,
    kind: String,
    options: String,
) -> PyResult<(String, Option<Bound<'_, PyBytes>>)> {
    let args = openreadout_batch::analyze::AnalyzeArgs {
        file: path,
        kind: analyze_kind(py, &kind)?,
        options: options_map(&options)?,
        strict: None,
    };
    let (out, png) = py
        .detach(|| openreadout_batch::analyze::run(&registry(), &args))
        .map_err(|e| to_py(py, e))?;
    Ok((
        serde_json::to_string(&out).map_err(json_err)?,
        png.map(|b| PyBytes::new(py, &b)),
    ))
}

/// Export a qPCR file (RDML, `.eds`, `.rex`) as RDML 1.3; the `RdmlExportReport` as JSON.
#[pyfunction]
#[pyo3(signature = (path, output=None, overwrite=false))]
fn export_rdml_json(
    py: Python<'_>,
    path: String,
    output: Option<String>,
    overwrite: bool,
) -> PyResult<String> {
    let reg = registry();
    let out = py.detach(|| {
        let input = Path::new(&path);
        let ds = openreadout_qpcr::open_qpcr(&reg, input)?;
        let output = output.map_or_else(
            || openreadout_qpcr::default_rdml_output(input),
            std::path::PathBuf::from,
        );
        openreadout_qpcr::export_rdml(&ds, &output, overwrite)
    });
    serde_json::to_string(&out.map_err(|e| to_py(py, e))?).map_err(json_err)
}

/// Format identifiers this build supports.
#[pyfunction]
fn formats_json() -> PyResult<String> {
    serde_json::to_string(&openreadout_core::model::FormatsOutput {
        formats: registry().descriptors(),
    })
    .map_err(json_err)
}

fn request<T: serde::de::DeserializeOwned>(json: &str) -> PyResult<T> {
    serde_json::from_str(json).map_err(|e| PyValueError::new_err(format!("bad request: {e}")))
}

/// A batch table (`openreadout_batch::api::BatchToolArgs` as JSON in, `BatchOutput` as JSON
/// out, every row).
#[pyfunction]
fn batch_json(py: Python<'_>, request_json: String) -> PyResult<String> {
    let a: openreadout_batch::api::BatchToolArgs = request(&request_json)?;
    let out = py
        .detach(|| openreadout_batch::api::run_batch(&registry(), a, false))
        .map_err(|e| to_py(py, e))?;
    serde_json::to_string(&out).map_err(json_err)
}

/// Group summary of a table file (`SummarizeToolArgs` JSON in, `SummarizeToolOutput` out).
#[pyfunction]
fn summarize_json(py: Python<'_>, request_json: String) -> PyResult<String> {
    let a: openreadout_batch::api::SummarizeToolArgs = request(&request_json)?;
    let out = py
        .detach(|| openreadout_batch::api::run_summarize(a))
        .map_err(|e| to_py(py, e))?;
    serde_json::to_string(&out).map_err(json_err)
}

/// Files of the same sample (`LinkToolArgs` JSON in, `LinkOutput` out).
#[pyfunction]
fn link_json(py: Python<'_>, request_json: String) -> PyResult<String> {
    let a: openreadout_batch::api::LinkToolArgs = request(&request_json)?;
    let out = py
        .detach(|| openreadout_batch::api::run_link(&registry(), a))
        .map_err(|e| to_py(py, e))?;
    serde_json::to_string(&out).map_err(json_err)
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(batch_json, m)?)?;
    m.add_function(wrap_pyfunction!(summarize_json, m)?)?;
    m.add_function(wrap_pyfunction!(link_json, m)?)?;
    m.add_class::<NativeFile>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(open_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(open_fileobj, m)?)?;
    m.add_function(wrap_pyfunction!(detect_bytes_json, m)?)?;
    m.add_function(wrap_pyfunction!(detect_fileobj_json, m)?)?;
    m.add_function(wrap_pyfunction!(detect_json, m)?)?;
    m.add_function(wrap_pyfunction!(formats_json, m)?)?;
    m.add_function(wrap_pyfunction!(analyze_json, m)?)?;
    m.add_function(wrap_pyfunction!(export_rdml_json, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
