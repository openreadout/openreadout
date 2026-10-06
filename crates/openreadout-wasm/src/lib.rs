//! OpenReadout compiled to WebAssembly (`wasm32-unknown-unknown` + `wasm-bindgen`): the same
//! readers and the same JSON as the `openreadout` binary, running in a browser tab or in Node,
//! on bytes the page already has. Nothing is uploaded and nothing is fetched: this crate has no
//! network code, and the host (the page) decides where bytes come from.
//!
//! Three ways to hand a file over:
//!
//! - [`InstrumentFile::from_bytes`]: the whole file (a `Uint8Array`), e.g. from
//!   `await blob.arrayBuffer()`.
//! - [`InstrumentFile::from_callback`]: a synchronous `read(offset, length) → Uint8Array`
//!   function, e.g. `FileReaderSync` in a Web Worker or `fs.readSync` in Node. Only the bytes
//!   the readers ask for are read.
//! - [`InstrumentFile::lazy`]: for asynchronous hosts (a `Blob` on the main thread, an HTTP
//!   range reader the page implements). Operations that need bytes not yet provided fail with a
//!   pending range ([`InstrumentFile::pending`]); the host reads it, calls
//!   [`InstrumentFile::provide`] and repeats the operation. The npm wrapper
//!   (`packaging/wasm/index.js`) runs that loop.
//!
//! [`Files`] holds several files (a dropped folder) so multi-file data sets resolve their
//! siblings. Every operation returns the `--json` envelope of the CLI command of the same name
//! as a string: `{"ok":true,"schema_version":"2","tool":{…},"data":{…}}` or `{"ok":false,…,
//! "error":{code,message,hint,exit_code}}`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use openreadout_core::envelope::ToolId;
use openreadout_core::model::{DetectOutput, Dump, FormatsOutput, Listing, TableSlice};
use openreadout_core::reader::{DEFAULT_FRAME_RECORDS, SpectrumView, attach_frames};
use openreadout_core::source::{ByteSource, Fs, Input, MemFs, MemSource};
use openreadout_core::trace::TraceRequest;
use openreadout_core::{Dataset, Detection, Envelope, Error, InfoOutput, Registry, SpectrumOutput};
use openreadout_preview::{Encoding, PreviewRequest};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Every reader this build carries, in the CLI's detection order. The OME-Zarr reader is left
/// out: its `zarrs` array types are not `Send` on `wasm32`.
pub fn registry() -> Registry {
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
        .with(Box::new(openreadout_abf::AbfReader))
        .with(Box::new(openreadout_abf::AtfReader))
        .with(Box::new(openreadout_plexon::PlexonReader))
        .with(Box::new(openreadout_ephys::HekaReader))
        .with(Box::new(openreadout_ephys::Spike2Reader))
        .with(Box::new(openreadout_ephys::WinWcpReader))
        .with(Box::new(openreadout_ephys::OpenEphysReader))
        .with(Box::new(openreadout_neuralynx::NeuralynxReader))
        .with(Box::new(openreadout_blackrock::BlackrockReader))
        .with(Box::new(openreadout_spikeglx::SpikeGlxReader))
        .with(Box::new(openreadout_intan::IntanReader))
        .with(Box::new(openreadout_agilent_ms::AgilentMsReader))
        .with(Box::new(openreadout_sciex::SciexWiffReader))
        .with(Box::new(openreadout_chrom::OpenLabReader))
        .with(Box::new(openreadout_chrom::ChemStationReader))
        .with(Box::new(openreadout_chrom::AndiReader))
        .with(Box::new(openreadout_waters::WatersRawReader))
        .with(Box::new(openreadout_chrom::ShimadzuReader))
        .with(Box::new(openreadout_chrom::ChromeleonReader))
        .with(Box::new(openreadout_chrom::EmpowerArwReader))
        .with(Box::new(openreadout_qpcr::RdmlReader))
        .with(Box::new(openreadout_qpcr::EdsReader))
        .with(Box::new(openreadout_qpcr::PcrdReader))
        .with(Box::new(openreadout_qpcr::RexReader))
        .with(Box::new(openreadout_qpcr::IxoReader))
        .with(Box::new(openreadout_tiff::TiffReader))
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
        .with(Box::new(openreadout_hdf5::Hdf5Reader))
        .with(Box::new(openreadout_plate::PlateReader))
}

fn tool_id() -> ToolId {
    ToolId {
        name: "openreadout".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

fn envelope<T: Serialize>(r: Result<T, Error>) -> String {
    let s = match &r {
        Ok(v) => serde_json::to_string(&Envelope::ok(tool_id(), v)),
        Err(e) => serde_json::to_string(&Envelope::<()>::err(tool_id(), e)),
    };
    s.unwrap_or_else(|e| {
        format!(
            r#"{{"ok":false,"schema_version":"2","error":{{"code":"error","message":"JSON serialization failed: {e}","exit_code":1}}}}"#
        )
    })
}

/// The version of this build (the crate version, like `openreadout --version`).
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").into()
}

/// `openreadout self formats --json`: the formats this build reads.
#[wasm_bindgen]
pub fn formats() -> String {
    envelope(Ok(FormatsOutput {
        formats: registry().descriptors(),
    }))
}

// ---------------------------------------------------------------------------------------------
// Byte sources the host provides
// ---------------------------------------------------------------------------------------------

thread_local! {
    /// Synchronous read callbacks, by id. `wasm32-unknown-unknown` has one thread; a JS function
    /// is not `Send`, so sources keep only its id and look it up here.
    static CALLBACKS: RefCell<BTreeMap<u32, js_sys::Function>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_CALLBACK: RefCell<u32> = const { RefCell::new(0) };
}

/// A file read through a JS `read(offset, length) → Uint8Array` function.
#[derive(Debug)]
struct JsCallbackSource {
    id: u32,
    size: u64,
    name: String,
}

impl ByteSource for JsCallbackSource {
    fn size(&self) -> io::Result<u64> {
        Ok(self.size)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let want = usize::try_from(self.size - offset)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        CALLBACKS.with(|c| {
            let c = c.borrow();
            let f = c
                .get(&self.id)
                .ok_or_else(|| io::Error::other("the read callback was released"))?;
            #[allow(clippy::cast_precision_loss)] // offsets stay below 2^53 (9 PB)
            let got = f
                .call2(
                    &JsValue::NULL,
                    &JsValue::from_f64(offset as f64),
                    &JsValue::from_f64(want as f64),
                )
                .map_err(|e| io::Error::other(format!("read callback threw: {e:?}")))?;
            let bytes = js_sys::Uint8Array::new(&got);
            let n = (bytes.length() as usize).min(want);
            bytes.subarray(0, n as u32).copy_to(&mut buf[..n]);
            Ok(n)
        })
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

/// Default block size of a [`InstrumentFile::lazy`] file (1 MiB).
pub const LAZY_BLOCK: u64 = 1 << 20;

/// A file whose bytes arrive asynchronously: reads of blocks not yet provided fail and record
/// the first missing block; the host provides it and repeats the operation.
#[derive(Debug)]
struct LazySource {
    size: u64,
    name: String,
    blocks: Mutex<BTreeMap<u64, Arc<Vec<u8>>>>,
    /// First block missed since the last `take_miss`.
    miss: Mutex<Option<u64>>,
    missed: AtomicBool,
}

impl LazySource {
    fn take_miss(&self) -> Option<u64> {
        self.missed.store(false, Ordering::SeqCst);
        self.miss
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn provide(&self, offset: u64, data: &[u8]) {
        let mut blocks = self.blocks.lock().unwrap_or_else(PoisonError::into_inner);
        // Only whole blocks (or the file's last, short block) are kept.
        let mut at = offset.next_multiple_of(LAZY_BLOCK);
        while at < self.size {
            let Some(rel) = usize::try_from(at - offset).ok() else {
                break;
            };
            let want = usize::try_from((self.size - at).min(LAZY_BLOCK)).unwrap_or(usize::MAX);
            let Some(chunk) = data.get(rel..rel.saturating_add(want)) else {
                break;
            };
            blocks
                .entry(at / LAZY_BLOCK)
                .or_insert_with(|| Arc::new(chunk.to_vec()));
            at += LAZY_BLOCK;
        }
    }

    fn bytes_held(&self) -> u64 {
        self.blocks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|b| b.len() as u64)
            .sum()
    }
}

impl ByteSource for LazySource {
    fn size(&self) -> io::Result<u64> {
        Ok(self.size)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let index = offset / LAZY_BLOCK;
        let block = self
            .blocks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&index)
            .cloned();
        let Some(block) = block else {
            // After a first miss every read fails fast: the operation is repeated anyway.
            if !self.missed.swap(true, Ordering::SeqCst) {
                *self.miss.lock().unwrap_or_else(PoisonError::into_inner) = Some(index);
            }
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "these bytes have not been provided yet",
            ));
        };
        let within = usize::try_from(offset - index * LAZY_BLOCK).unwrap_or(usize::MAX);
        let Some(avail) = block.get(within..) else {
            return Ok(0);
        };
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        Ok(n)
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

// ---------------------------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------------------------

/// Several files held by the page (a dropped folder, a file with its companions), so a data
/// set spread over files (an OIF folder, an OME-TIFF set, an imzML with its `.ibd`) resolves
/// its siblings.
#[wasm_bindgen]
#[derive(Debug, Default)]
pub struct Files {
    fs: MemFs,
}

#[wasm_bindgen]
impl Files {
    /// An empty set.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Files {
        Files::default()
    }

    /// Add a file at `path` (`/`-separated, relative to the dropped folder's parent).
    pub fn add(&mut self, path: &str, data: Vec<u8>) {
        let src = MemSource::new(path, data);
        self.fs.insert(path, Arc::new(src));
    }

    /// Paths of every file added, sorted.
    pub fn paths(&self) -> Vec<String> {
        self.fs
            .paths()
            .into_iter()
            .map(|p| p.display().to_string())
            .collect()
    }

    /// Open the file (or directory) at `path` among these files.
    pub fn open(&self, path: &str) -> InstrumentFile {
        InstrumentFile::new(Input::new(path, Fs::new(Arc::new(self.fs.clone()))), None)
    }
}

/// One instrument file (or directory data set) and the operations of the CLI on it.
#[wasm_bindgen]
pub struct InstrumentFile {
    input: Input,
    lazy: Option<Arc<LazySource>>,
    callback: Option<u32>,
    opened: Option<(Detection, &'static str, Box<dyn Dataset>)>,
    image: Vec<u8>,
}

impl std::fmt::Debug for InstrumentFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstrumentFile")
            .field("input", &self.input.path())
            .field("lazy", &self.lazy.is_some())
            .field("opened", &self.opened.is_some())
            .finish_non_exhaustive()
    }
}

impl Drop for InstrumentFile {
    fn drop(&mut self) {
        if let Some(id) = self.callback.take() {
            CALLBACKS.with(|c| c.borrow_mut().remove(&id));
        }
    }
}

fn size_arg(size: f64) -> Result<u64, JsError> {
    if size.is_finite() && size >= 0.0 && size.fract() == 0.0 && size <= 9_007_199_254_740_992.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
        Ok(size as u64)
    } else {
        Err(JsError::new("size must be a non-negative integer"))
    }
}

/// Offsets and counts from JS numbers (non-negative integers; anything else is a usage error).
fn count_arg(v: f64, what: &str) -> Result<u64, Error> {
    if v.is_finite() && v >= 0.0 && v.fract() == 0.0 && v <= 9_007_199_254_740_992.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
        Ok(v as u64)
    } else {
        Err(Error::Usage(format!(
            "{what} must be a non-negative integer, got {v}"
        )))
    }
}

#[wasm_bindgen]
impl InstrumentFile {
    fn new(input: Input, lazy: Option<Arc<LazySource>>) -> InstrumentFile {
        InstrumentFile {
            input,
            lazy,
            callback: None,
            opened: None,
            image: Vec::new(),
        }
    }

    /// A file held in memory: `name` is its file name (the extension helps detection), `data`
    /// its bytes (copied into WebAssembly memory).
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(name: &str, data: Vec<u8>) -> InstrumentFile {
        InstrumentFile::new(Input::from_bytes(name, data), None)
    }

    /// A file of `size` bytes read on demand through `read(offset, length)`, which must return
    /// a `Uint8Array` synchronously (`FileReaderSync` in a Worker, `fs.readSync` in Node).
    #[wasm_bindgen(js_name = fromCallback)]
    pub fn from_callback(
        name: &str,
        size: f64,
        read: js_sys::Function,
    ) -> Result<InstrumentFile, JsError> {
        let size = size_arg(size)?;
        let id = NEXT_CALLBACK.with(|n| {
            let mut n = n.borrow_mut();
            *n = n.wrapping_add(1);
            *n
        });
        CALLBACKS.with(|c| c.borrow_mut().insert(id, read));
        let src = JsCallbackSource {
            id,
            size,
            name: name.to_string(),
        };
        let cached = openreadout_core::source::CachedSource::with_defaults(
            Arc::new(src) as Arc<dyn ByteSource>
        );
        let mut f = InstrumentFile::new(Input::from_source(name, Arc::new(cached)), None);
        f.callback = Some(id);
        Ok(f)
    }

    /// A file of `size` bytes provided asynchronously in blocks of [`LAZY_BLOCK`] bytes: after an
    /// operation, [`pending`](InstrumentFile::pending) names the range it still needs.
    pub fn lazy(name: &str, size: f64) -> Result<InstrumentFile, JsError> {
        let src = Arc::new(LazySource {
            size: size_arg(size)?,
            name: name.to_string(),
            blocks: Mutex::new(BTreeMap::new()),
            miss: Mutex::new(None),
            missed: AtomicBool::new(false),
        });
        let input = Input::from_source(name, src.clone());
        Ok(InstrumentFile::new(input, Some(src)))
    }

    /// Block size of lazy files, in bytes.
    #[wasm_bindgen(js_name = blockSize)]
    pub fn block_size() -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let b = LAZY_BLOCK as f64;
        b
    }

    /// Lazy files: `[offset, length]` of the block the last operation needed and did not have
    /// (its result is to be discarded; provide the bytes and repeat it), or `undefined`.
    pub fn pending(&self) -> Option<Vec<f64>> {
        let lazy = self.lazy.as_ref()?;
        let block = *lazy.miss.lock().unwrap_or_else(PoisonError::into_inner);
        let at = block? * LAZY_BLOCK;
        let len = (lazy.size - at).min(LAZY_BLOCK);
        #[allow(clippy::cast_precision_loss)]
        Some(vec![at as f64, len as f64])
    }

    /// Lazy files: bytes of the file starting at `offset` (a multiple of the block size); any
    /// number of whole blocks, or up to the end of the file.
    pub fn provide(&mut self, offset: f64, data: &[u8]) -> Result<(), JsError> {
        let offset = size_arg(offset)?;
        if let Some(l) = &self.lazy {
            l.provide(offset, data);
        }
        Ok(())
    }

    /// Lazy files: bytes provided so far.
    #[wasm_bindgen(js_name = bytesHeld)]
    pub fn bytes_held(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let n = self.lazy.as_ref().map_or(0, |l| l.bytes_held()) as f64;
        n
    }

    /// The name the file was given.
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.input.path().display().to_string()
    }

    /// Release the opened data set (it is reopened on the next operation).
    pub fn close(&mut self) {
        self.opened = None;
    }

    /// `info --view VIEW --json`. `view`: `summary` (default: the header-only summary),
    /// `full` (plus per-frame records, the vendor metadata tree and field provenance),
    /// `structure` (the container's elements), `explain` (what the file is, in sentences) or
    /// `format` (the format, from the first bytes). `options` is a JSON object with any of
    /// `vendor` and `provenance` (`full`; default true) and `ask` (`explain`: a question to
    /// answer in words).
    pub fn info(&mut self, view: Option<String>, options: Option<String>) -> String {
        let opts = match parse_options::<InfoOptions>("info", options.as_deref()) {
            Ok(o) => o,
            Err(e) => return envelope::<()>(Err(e)),
        };
        let path = self.name();
        match view.as_deref().unwrap_or("summary") {
            "summary" => envelope(self.with_dataset(|ds, _| {
                let info = ds.info()?;
                Ok(InfoOutput::new(ds, info))
            })),
            "full" => envelope(self.with_dataset(|ds, _| {
                let mut info = ds.info()?;
                attach_frames(ds, &mut info, Some(DEFAULT_FRAME_RECORDS))?;
                Ok(Dump {
                    file: InfoOutput::new(ds, info),
                    vendor: if opts.vendor {
                        Some(ds.vendor_metadata()?)
                    } else {
                        None
                    },
                    provenance: if opts.provenance {
                        ds.provenance()
                    } else {
                        Default::default()
                    },
                })
            })),
            "structure" => envelope(self.with_dataset(|ds, format| {
                Ok(Listing {
                    path: path.clone(),
                    format: format.into(),
                    entries: ds.entries()?,
                    acquisition: None,
                })
            })),
            "explain" => envelope(self.with_dataset(|ds, _| {
                let mut info = ds.info()?;
                let experiment = openreadout_core::experiment::of_dataset(ds, &info);
                let _ = attach_frames(ds, &mut info, Some(1));
                Ok(openreadout_ops::explain::explain_with(
                    &info,
                    Some(&experiment),
                    opts.ask.as_deref(),
                ))
            })),
            "format" => envelope(self.attempt(|_, input| {
                let reg = registry();
                let (reader, det) = reg.detect_input(input)?;
                Ok(DetectOutput {
                    path: path.clone(),
                    format: det.format_id.into(),
                    name: reader.descriptor().name,
                    confidence: det.confidence,
                    note: det.note,
                })
            })),
            other => envelope::<()>(Err(Error::Usage(format!(
                "view must be summary, full, structure, explain or format, not {other:?}"
            )))),
        }
    }

    /// `check --json [--headers-only]`: integrity report.
    pub fn check(&mut self, headers_only: bool) -> String {
        let r = self.with_dataset(|ds, _| {
            if headers_only {
                ds.check_headers()
            } else {
                ds.check()
            }
        });
        envelope(r)
    }

    /// `preview --json`: renders a PNG (fetch it with [`image`](InstrumentFile::image)).
    /// `options` is a JSON object with any of `image`, `select` (e.g. `["c=1","z=3"]`),
    /// `composite`, `max_size`, `trace`, `sweep`, `run`, `spectrum`, `table`, `axes` (coordinate
    /// rulers and scale bar around image previews; default false here, unlike the CLI) and
    /// `grid`.
    pub fn preview(&mut self, options: Option<String>) -> String {
        let req = match preview_request(options.as_deref()) {
            Ok(r) => r,
            Err(e) => return envelope::<()>(Err(e)),
        };
        let mut png = Vec::new();
        let r = self.with_dataset(|ds, _| {
            let info = ds.info()?;
            let rendered = openreadout_preview::render(ds, &info, &req)?;
            let (out, bytes) = openreadout_preview::finish(&rendered, Encoding::Png, 90)?;
            png = bytes;
            Ok(out)
        });
        self.image = if r.is_ok() { png } else { Vec::new() };
        envelope(r)
    }

    /// The PNG from the last successful [`preview`](InstrumentFile::preview) (empty otherwise).
    pub fn image(&self) -> Vec<u8> {
        self.image.clone()
    }

    /// Rows `[first_row, first_row + max_rows)` of table `table` (like the MCP
    /// `openreadout_table` tool): `{columns, labels, rows, total_rows, truncated}`.
    pub fn table(&mut self, table: u32, first_row: f64, max_rows: f64) -> String {
        let path = self.name();
        let r = self.with_dataset(|ds, format| {
            let first_row = count_arg(first_row, "first_row")?;
            let want = count_arg(max_rows, "max_rows")?.min(100_000);
            let info = ds.info()?;
            let t = info
                .tables
                .iter()
                .find(|t| t.index == table)
                .cloned()
                .ok_or_else(|| {
                    Error::Usage(format!(
                        "table {table} not found (file has {} tables)",
                        info.tables.len()
                    ))
                })?;
            let tab = ds.read_table(table, first_row, want)?;
            let n = tab.columns.first().map_or(0, Vec::len);
            let rows: Vec<Vec<f64>> = (0..n)
                .map(|r| tab.columns.iter().map(|c| c[r]).collect())
                .collect();
            Ok(TableSlice {
                path: path.clone(),
                format: format.into(),
                table,
                first_row,
                total_rows: t.row_count,
                columns: t.columns.iter().map(|c| c.name.clone()).collect(),
                labels: t.columns.iter().map(|c| c.label.clone()).collect(),
                truncated: first_row + (n as u64) < t.row_count,
                rows,
                processing: None,
                filter: None,
            })
        });
        envelope(r)
    }

    /// `trace --json`: statistics over a window of one sweep and its first `max_samples`
    /// values per channel. `count` < 0 means to the end of the sweep.
    pub fn trace(
        &mut self,
        trace: u32,
        sweep: u32,
        first_sample: f64,
        count: f64,
        max_samples: f64,
    ) -> String {
        let r = self.with_dataset(|ds, _| {
            let info = ds.info()?;
            let mut req = TraceRequest::default();
            req.trace = trace;
            req.sweep = sweep;
            req.first_sample = count_arg(first_sample, "first_sample")?;
            req.count = if count < 0.0 {
                None
            } else {
                Some(count_arg(count, "count")?)
            };
            req.max_samples = count_arg(max_samples, "max_samples")?.min(100_000);
            openreadout_core::trace::slice_trace(ds, &info, &req)
        });
        envelope(r)
    }

    /// `spectra --json`. Without `index` and `scan`: the scan headers of run `run`, without
    /// decoding peaks, filtered by `ms_level`, `polarity`, `rt_range` (`[start, end]` minutes),
    /// `precursor_mz` (with `ppm`), `charge`, `activation` and `scan_filter`, every match
    /// counted and `limit` (default 100) listed from the `offset`-th. With `index` (zero-based)
    /// or `scan` (the instrument's scan number): that spectrum (the instrument's centroid list
    /// with `centroid`), its arrays cut to `max_points` points (default 100000). `options` is a
    /// JSON object with those keys and `run` (default 0).
    pub fn spectra(&mut self, options: Option<String>) -> String {
        let o = match parse_options::<SpectraOptions>("spectra", options.as_deref()) {
            Ok(o) => o,
            Err(e) => return envelope::<()>(Err(e)),
        };
        let path = self.name();
        let run = o.run;
        if o.index.is_none() && o.scan.is_none() {
            let filter = openreadout_core::ScanFilter {
                ms_level: o.ms_level,
                polarity: o.polarity.map(|p| p.to_ascii_lowercase()),
                rt_min_s: o.rt_range.map(|r| r[0] * 60.0),
                rt_max_s: o.rt_range.map(|r| r[1] * 60.0),
                precursor_mz: o.precursor_mz,
                precursor_tol_mz: None,
                precursor_tol_ppm: o.ppm,
                charge: o.charge,
                activation: o.activation,
                filter_contains: o.scan_filter,
            };
            return envelope(self.with_dataset(|ds, format| {
                openreadout_core::scans::scan_list(
                    ds,
                    &path,
                    format,
                    run,
                    &filter,
                    o.offset,
                    o.limit.unwrap_or(100),
                )
            }));
        }
        let view = if o.centroid {
            SpectrumView::Centroid
        } else {
            SpectrumView::Primary
        };
        let r = self.with_dataset(|ds, format| {
            let mut sp = match (o.index, o.scan) {
                (Some(i), None) => ds.read_spectrum_view(run, i, view)?,
                (None, Some(n)) => openreadout_core::reader::spectrum_by_scan(ds, run, n, view)?,
                _ => return Err(Error::Usage("give at most one of index and scan".into())),
            };
            let point_count = sp.mz.len() as u64;
            let keep = usize::try_from(o.max_points.unwrap_or(100_000)).unwrap_or(usize::MAX);
            let truncated = sp.mz.len() > keep;
            sp.mz.truncate(keep);
            sp.intensity.truncate(keep);
            Ok(SpectrumOutput {
                path: path.clone(),
                format: format.into(),
                run,
                view: if o.centroid { "centroid" } else { "primary" }.into(),
                point_count,
                truncated,
                spectrum: sp,
            })
        });
        envelope(r)
    }
}

/// `options` of [`InstrumentFile::info`].
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct InfoOptions {
    vendor: bool,
    provenance: bool,
    ask: Option<String>,
}

impl Default for InfoOptions {
    fn default() -> Self {
        Self {
            vendor: true,
            provenance: true,
            ask: None,
        }
    }
}

/// `options` of [`InstrumentFile::spectra`].
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SpectraOptions {
    run: u32,
    index: Option<u64>,
    scan: Option<u64>,
    centroid: bool,
    max_points: Option<u64>,
    ms_level: Option<u32>,
    polarity: Option<String>,
    rt_range: Option<[f64; 2]>,
    precursor_mz: Option<f64>,
    ppm: Option<f64>,
    charge: Option<i32>,
    activation: Option<String>,
    scan_filter: Option<String>,
    offset: u64,
    limit: Option<u64>,
}

/// A JSON object of options (absent or blank: the defaults).
fn parse_options<T: serde::de::DeserializeOwned + Default>(
    what: &str,
    text: Option<&str>,
) -> Result<T, Error> {
    match text.filter(|t| !t.trim().is_empty()) {
        None => Ok(T::default()),
        Some(t) => {
            serde_json::from_str(t).map_err(|e| Error::Usage(format!("{what} options: {e}")))
        }
    }
}

impl InstrumentFile {
    /// Run `op`; for lazy files, a result computed while bytes were missing is replaced by an
    /// error (the host provides the pending range and repeats the call).
    fn attempt<T>(
        &mut self,
        op: impl FnOnce(&mut Self, &Input) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if let Some(l) = &self.lazy {
            let _ = l.take_miss();
        }
        let input = self.input.clone();
        let r = op(self, &input);
        let missed = self.lazy.as_ref().is_some_and(|l| {
            l.miss
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_some()
        });
        if missed {
            // Whatever the reader made of the missing bytes is discarded, including an opened
            // data set whose state may reflect them.
            self.opened = None;
            let (at, len) = self.pending().map_or((0.0, 0.0), |p| (p[0], p[1]));
            return Err(Error::Io {
                path: PathBuf::from(self.name()),
                source: io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!(
                        "bytes {at}..{} are needed: provide them and repeat",
                        at + len
                    ),
                ),
            });
        }
        r
    }

    fn with_dataset<T>(
        &mut self,
        op: impl FnOnce(&mut dyn Dataset, &'static str) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.attempt(|me, input| {
            if me.opened.is_none() {
                let reg = registry();
                let (reader, det) = reg.detect_input(input)?;
                let ds = reader.open_input(input)?;
                me.opened = Some((det.clone(), det.format_id, ds));
            }
            let (_, format, ds) = me.opened.as_mut().expect("opened above");
            op(ds.as_mut(), format)
        })
    }
}

fn preview_request(options: Option<&str>) -> Result<PreviewRequest, Error> {
    let mut req = PreviewRequest::default();
    // The demo page shows the bare plane unless asked for rulers (`"axes": true`).
    req.axes = false;
    let Some(text) = options.filter(|t| !t.trim().is_empty()) else {
        return Ok(req);
    };
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| Error::Usage(format!("preview options are not JSON: {e}")))?;
    let obj = v
        .as_object()
        .ok_or_else(|| Error::Usage("preview options must be a JSON object".into()))?;
    let u32_of = |k: &str| -> Result<Option<u32>, Error> {
        obj.get(k)
            .map(|x| {
                x.as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| {
                        Error::Usage(format!(
                            "preview option `{k}` must be a small non-negative integer"
                        ))
                    })
            })
            .transpose()
    };
    req.image = u32_of("image")?;
    req.trace = u32_of("trace")?;
    req.sweep = u32_of("sweep")?;
    req.run = u32_of("run")?;
    req.table = u32_of("table")?;
    if let Some(n) = u32_of("max_size")? {
        req.max_size = n.clamp(16, 4096);
    }
    if let Some(n) = obj.get("spectrum").and_then(serde_json::Value::as_u64) {
        req.spectrum = Some(n);
    }
    if let Some(b) = obj.get("composite").and_then(serde_json::Value::as_bool) {
        req.composite = b;
    }
    if let Some(b) = obj.get("axes").and_then(serde_json::Value::as_bool) {
        req.axes = b;
    }
    if let Some(b) = obj.get("grid").and_then(serde_json::Value::as_bool) {
        req.grid = b;
    }
    if let Some(sel) = obj.get("select").and_then(serde_json::Value::as_array) {
        req.select = sel
            .iter()
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect();
    }
    Ok(req)
}

/// Detect, open and summarise in one call (the demo page's first request): the `info`
/// envelope for `data` named `name`.
#[wasm_bindgen]
pub fn info(name: &str, data: Vec<u8>) -> String {
    InstrumentFile::from_bytes(name, data).info(None, None)
}
