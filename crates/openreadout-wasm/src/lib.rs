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
//!   range reader the page implements). Operations that need bytes not yet provided fail and
//!   say what they need ([`InstrumentFile::pending`]); the host reads it, calls
//!   [`InstrumentFile::provide`] and repeats the operation. The npm wrapper
//!   (`packaging/wasm/index.js`) runs that loop.
//!
//! Data sets spread over several files, or that are a directory, come in the same ways:
//! [`Files`] holds files in memory, [`InstrumentFile::lazy_files`] names files whose bytes are
//! provided as the readers ask for them (a dropped folder), and [`InstrumentFile::lazy_store`]
//! opens a directory whose files the host looks up one by one (an OME-Zarr image the page
//! reads over HTTP). Every operation returns the `--json` envelope of the CLI command of the same name
//! as a string: `{"ok":true,"schema_version":"2","tool":{…},"data":{…}}` or `{"ok":false,…,
//! "error":{code,message,hint,exit_code}}`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use openreadout_core::envelope::ToolId;
use openreadout_core::model::{DetectOutput, Dump, FormatsOutput, Listing, TableSlice};
use openreadout_core::reader::{DEFAULT_FRAME_RECORDS, SpectrumView, attach_frames};
use openreadout_core::source::{
    ByteSource, DirEntry, DirSource, EntryMeta, Fs, Input, MemFs, MemSource,
};
use openreadout_core::trace::TraceRequest;
use openreadout_core::{Dataset, Detection, Envelope, Error, InfoOutput, Registry, SpectrumOutput};
use openreadout_preview::{Axes, Encoding, PreviewOptions, PreviewRequest};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Every reader this build carries, in the CLI's detection order.
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
        .with(Box::new(openreadout_mzml::MgfReader))
        .with(Box::new(openreadout_bruker_tims::BrukerTimsReader))
        .with(Box::new(openreadout_bruker_tims::BrukerBafReader))
        .with(Box::new(openreadout_bruker_tims::BrukerFlexReader))
        .with(Box::new(openreadout_oir::OirReader))
        .with(Box::new(openreadout_vsi::VsiReader))
        .with(Box::new(openreadout_wsi::MiraxReader))
        .with(Box::new(openreadout_wsi::IsyntaxReader))
        .with(Box::new(openreadout_wsi::VmsReader))
        .with(Box::new(openreadout_zvi::ZviReader))
        .with(Box::new(openreadout_oif::OibReader))
        .with(Box::new(openreadout_oif::OifReader))
        .with(Box::new(openreadout_abf::AbfReader))
        .with(Box::new(openreadout_abf::AtfReader))
        .with(Box::new(openreadout_plexon::PlexonReader))
        .with(Box::new(openreadout_ephys::HekaReader))
        .with(Box::new(openreadout_ephys::Spike2Reader))
        .with(Box::new(openreadout_ephys::WinWcpReader))
        .with(Box::new(openreadout_ephys::TdtReader))
        .with(Box::new(openreadout_ephys::AxonaReader))
        .with(Box::new(openreadout_ephys::McsRawReader))
        .with(Box::new(openreadout_ephys::EdfReader))
        .with(Box::new(openreadout_ephys::BrainVisionReader))
        .with(Box::new(openreadout_ephys::EeglabReader))
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
        .with(Box::new(openreadout_qpcr::ExportReader))
        .with(Box::new(openreadout_diffraction::CbfReader))
        .with(Box::new(openreadout_diffraction::SmvReader))
        // medical imaging before TIFF: DICOM whole-slide files can be dual-personality TIFF,
        // and ParaVision folders hold DICOM exports
        .with(Box::new(openreadout_medical::ParavisionReader))
        .with(Box::new(openreadout_medical::DicomReader))
        .with(Box::new(openreadout_medical::NiftiReader))
        .with(Box::new(openreadout_tiff::TiffReader))
        .with(Box::new(openreadout_zarr::ZarrReader))
        .with(Box::new(openreadout_em::MrcReader))
        .with(Box::new(openreadout_em::MdocReader))
        .with(Box::new(openreadout_em::DmReader))
        .with(Box::new(openreadout_em::SerReader))
        .with(Box::new(openreadout_em::EmdReader))
        .with(Box::new(openreadout_hdf5::ImsReader))
        .with(Box::new(openreadout_hdf5::NxmxReader))
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
        .with(Box::new(openreadout_spectro::UvProbeReader))
        .with(Box::new(openreadout_spectro::CaryReader))
        .with(Box::new(openreadout_biophys::ItcReader))
        .with(Box::new(openreadout_biophys::BiacoreReader))
        .with(Box::new(openreadout_biophys::BiacoreEvaluationReader))
        .with(Box::new(openreadout_biophys::SeahorseReader))
        .with(Box::new(openreadout_biophys::OctetReader))
        .with(Box::new(openreadout_biophys::ZetasizerReader))
        .with(Box::new(openreadout_biophys::GprReader))
        .with(Box::new(openreadout_gel::ImageLabReader))
        .with(Box::new(openreadout_gel::SyngeneReader))
        .with(Box::new(openreadout_igor::IgorReader))
        .with(Box::new(openreadout_sif::SifReader))
        .with(Box::new(openreadout_dv::DvReader))
        .with(Box::new(openreadout_benchtop::NanoDropReader))
        .with(Box::new(openreadout_benchtop::QubitReader))
        .with(Box::new(openreadout_fplc::UnicornResReader))
        .with(Box::new(openreadout_fplc::UnicornZipReader))
        .with(Box::new(openreadout_epr::Bes3tReader))
        .with(Box::new(openreadout_epr::EspReader))
        .with(Box::new(openreadout_xrd::XrdmlReader))
        .with(Box::new(openreadout_xrd::BrukerRawReader))
        .with(Box::new(openreadout_xrd::BrmlReader))
        .with(Box::new(openreadout_xrd::RasReader))
        .with(Box::new(openreadout_xrd::RasxReader))
        .with(Box::new(openreadout_ce::AbifReader))
        .with(Box::new(openreadout_ce::BioanalyzerReader))
        .with(Box::new(openreadout_ce::TapeStationReader))
        .with(Box::new(openreadout_echem::MprReader))
        .with(Box::new(openreadout_echem::MptReader))
        .with(Box::new(openreadout_echem::GamryReader))
        .with(Box::new(openreadout_echem::NdaReader))
        .with(Box::new(openreadout_echem::NdaxReader))
        .with(Box::new(openreadout_echem::ArbinReader))
        .with(Box::new(openreadout_tdms::TdmsReader))
        .with(Box::new(openreadout_thermal::NgbReader))
        .with(Box::new(openreadout_thermal::TaReader))
        .with(Box::new(openreadout_thermal::TriosReader))
        // DNA sequencers: Illumina run folders, nanopore signal (FAST5 before generic HDF5),
        // FASTQ and BAM read files
        .with(Box::new(openreadout_seq::IlluminaRunReader))
        .with(Box::new(openreadout_seq::Pod5Reader))
        .with(Box::new(openreadout_seq::Fast5Reader))
        .with(Box::new(openreadout_seq::FastqReader))
        .with(Box::new(openreadout_seq::BamReader))
        .with(Box::new(openreadout_seq::CramReader))
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

/// Default `max_size` of a preview: the demo page shows it at about this width.
pub const DEFAULT_WASM_PREVIEW_SIZE: u32 = 768;

/// Block size of lazily provided files (1 MiB).
pub const LAZY_BLOCK: u64 = 1 << 20;

/// What an operation needed from the host and did not have. Each is recorded once, and the
/// operation is repeated once the host has provided them. A file that isn't there doesn't stop
/// a reader that only checks for it, so one pass can ask for many lookups, which the host
/// answers together.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Need {
    /// Does this file exist, and how large is it?
    Lookup(PathBuf),
    /// The block of this file starting at `block * LAZY_BLOCK`.
    Block(PathBuf, u64),
}

type NeedSlot = Arc<Mutex<Vec<Need>>>;

fn record(slot: &NeedSlot, need: Need) {
    let mut needs = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if !needs.contains(&need) {
        needs.push(need);
    }
}

fn would_block(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        format!("{what} has not been provided yet"),
    )
}

/// A file whose bytes arrive asynchronously: reads of blocks not yet provided fail and record
/// the block; the host provides it and repeats the operation.
#[derive(Debug)]
struct LazySource {
    path: PathBuf,
    size: u64,
    blocks: Mutex<BTreeMap<u64, Arc<Vec<u8>>>>,
    need: NeedSlot,
}

impl LazySource {
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
            record(&self.need, Need::Block(self.path.clone(), index));
            return Err(would_block("this block"));
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
        self.path.display().to_string()
    }
}

/// Files whose sizes and bytes the host provides on request: one large file, a dropped folder
/// whose files are all named up front, or an open-ended store (a Zarr hierarchy the page reads
/// over HTTP), whose files are looked up one by one as the readers ask for them.
#[derive(Debug)]
struct LazyFs {
    known: Mutex<MemFs>,
    sources: Mutex<Vec<Arc<LazySource>>>,
    /// An open-ended store: a path not yet known may exist (the host looks it up), and
    /// directories other than the root cannot be listed.
    open_ended: Option<PathBuf>,
    absent: Mutex<std::collections::BTreeSet<PathBuf>>,
    need: NeedSlot,
}

impl LazyFs {
    fn new(open_ended: Option<PathBuf>) -> LazyFs {
        LazyFs {
            known: Mutex::new(MemFs::new()),
            sources: Mutex::new(Vec::new()),
            open_ended,
            absent: Mutex::new(std::collections::BTreeSet::new()),
            need: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The file at `path` exists and holds `size` bytes.
    fn found(&self, path: &Path, size: u64) {
        let src = Arc::new(LazySource {
            path: path.to_path_buf(),
            size,
            blocks: Mutex::new(BTreeMap::new()),
            need: self.need.clone(),
        });
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path, src.clone());
        self.sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(src);
    }

    fn absent(&self, path: &Path) {
        self.absent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_path_buf());
    }

    fn provide(&self, path: &Path, offset: u64, data: &[u8]) {
        let sources = self.sources.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(src) = sources.iter().find(|s| s.path == path) {
            src.provide(offset, data);
        }
    }

    fn bytes_held(&self) -> u64 {
        self.sources
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|s| s.bytes_held())
            .sum()
    }

    fn clear_needs(&self) {
        self.need
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    fn needs(&self) -> Vec<Need> {
        self.need
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// For a path the known files do not answer: in an open-ended store, ask the host to look
    /// it up (unless it already said the file is absent).
    fn unknown(&self, path: &Path, err: io::Error) -> io::Error {
        let Some(root) = &self.open_ended else {
            return err;
        };
        if err.kind() != io::ErrorKind::NotFound
            || !path.starts_with(root)
            || self
                .absent
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(path)
        {
            return err;
        }
        record(&self.need, Need::Lookup(path.to_path_buf()));
        would_block(&path.display().to_string())
    }
}

impl DirSource for LazyFs {
    fn open(&self, path: &Path) -> io::Result<Arc<dyn ByteSource>> {
        let known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        known.open(path).map_err(|e| self.unknown(path, e))
    }

    fn metadata(&self, path: &Path) -> io::Result<EntryMeta> {
        if self.open_ended.as_deref() == Some(path) {
            return Ok(EntryMeta::dir());
        }
        let known = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        known.metadata(path).map_err(|e| self.unknown(path, e))
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        if self.open_ended.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the files of this store are looked up one by one and cannot be listed",
            ));
        }
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .list(dir)
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
    lazy: Option<Arc<LazyFs>>,
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
    fn new(input: Input, lazy: Option<Arc<LazyFs>>) -> InstrumentFile {
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
    /// operation, [`pending`](InstrumentFile::pending) names the block it still needs.
    pub fn lazy(name: &str, size: f64) -> Result<InstrumentFile, JsError> {
        let fs = LazyFs::new(None);
        fs.found(Path::new(name), size_arg(size)?);
        Ok(InstrumentFile::lazy_in(name, fs))
    }

    /// The file or folder at `open` among files whose paths and sizes are known up front (a
    /// dropped folder) and whose bytes are provided as the readers ask for them.
    #[wasm_bindgen(js_name = lazyFiles)]
    pub fn lazy_files(
        paths: Vec<String>,
        sizes: Vec<f64>,
        open: &str,
    ) -> Result<InstrumentFile, JsError> {
        if paths.len() != sizes.len() {
            return Err(JsError::new("paths and sizes must have the same length"));
        }
        let fs = LazyFs::new(None);
        for (p, size) in paths.iter().zip(sizes) {
            fs.found(Path::new(p), size_arg(size)?);
        }
        Ok(InstrumentFile::lazy_in(open, fs))
    }

    /// A store whose files the host looks up as the readers ask for them (a folder the page
    /// reads over HTTP): nothing in it is known up front, and it cannot be listed. Opens the
    /// directory `root` itself (a Zarr hierarchy), or the file `open` inside it, whose
    /// companion files are looked up the same way.
    #[wasm_bindgen(js_name = lazyStore)]
    pub fn lazy_store(root: &str, open: Option<String>) -> Result<InstrumentFile, JsError> {
        let path = open.unwrap_or_else(|| root.to_string());
        if !Path::new(&path).starts_with(root) {
            return Err(JsError::new("the file to open must be inside the store"));
        }
        Ok(InstrumentFile::lazy_in(
            &path,
            LazyFs::new(Some(PathBuf::from(root))),
        ))
    }

    /// Block size of lazy files, in bytes.
    #[wasm_bindgen(js_name = blockSize)]
    pub fn block_size() -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let b = LAZY_BLOCK as f64;
        b
    }

    /// Lazy files: what the last operation needed and did not have (its result is to be
    /// discarded; provide all of it and repeat the operation), or `undefined`. Each item is
    /// either `{ path, size, offset, length }`, a block of a file of `size` bytes, or
    /// `{ path, lookup: true }`, a file of an open-ended store to look up (answer with
    /// [`found`](InstrumentFile::found) or [`absent`](InstrumentFile::absent)).
    pub fn pending(&self) -> Option<Vec<js_sys::Object>> {
        let needs = self.lazy.as_ref()?.needs();
        if needs.is_empty() {
            return None;
        }
        Some(needs.into_iter().map(|n| self.need_object(n)).collect())
    }

    /// Lazy files: bytes of the file at `path` starting at `offset` (a multiple of the block
    /// size); any number of whole blocks, or up to the end of the file.
    pub fn provide(&mut self, path: &str, offset: f64, data: &[u8]) -> Result<(), JsError> {
        let offset = size_arg(offset)?;
        if let Some(l) = &self.lazy {
            l.provide(Path::new(path), offset, data);
        }
        Ok(())
    }

    /// Open-ended stores: the file at `path` exists and holds `size` bytes.
    pub fn found(&mut self, path: &str, size: f64) -> Result<(), JsError> {
        let size = size_arg(size)?;
        if let Some(l) = &self.lazy {
            l.found(Path::new(path), size);
        }
        Ok(())
    }

    /// Open-ended stores: there is no file at `path`.
    pub fn absent(&mut self, path: &str) {
        if let Some(l) = &self.lazy {
            l.absent(Path::new(path));
        }
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
                    ..Default::default()
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

    /// `preview --json`: draws the picture (fetch it with [`image`](InstrumentFile::image)).
    /// `options` is a JSON object with the arguments of the `openreadout_preview` MCP tool:
    /// `image`, `select` (e.g. `["c=1","z=3"]`), `mip`, `composite`, `level`, `region`,
    /// `max_size` (default 768), `axes` (`rulers`, `grid`, `scale-bar` or `none`; default
    /// `none` here, the bare plane), `title`, `caption`, `contrast`, `lut`, `trace`, `sweep`, `channels`, `run`, `spectrum`, `scan`,
    /// `centroid`, `table`, `columns`, `marks`, `annotations`, plus `dark`, `format`
    /// (`png`, `jpeg` or `svg`) and `scale` (1 to 4: PNG and JPEG pixels per figure pixel, so a
    /// page can draw the picture at `max_size` CSS pixels and keep it sharp on a high-density
    /// screen; the positions in the output stay in figure pixels).
    pub fn preview(&mut self, options: Option<String>) -> String {
        let (req, encoding, scale) = match preview_request(options.as_deref()) {
            Ok(r) => r,
            Err(e) => return envelope::<()>(Err(e)),
        };
        let mut bytes = Vec::new();
        let r = self.with_dataset(|ds, _| {
            let info = ds.info()?;
            let rendered = render_scaled(ds, &info, &req, encoding, scale)?;
            let (out, b) = openreadout_preview::finish(&rendered, encoding, 90)?;
            bytes = b;
            Ok(out)
        });
        self.image = if r.is_ok() { bytes } else { Vec::new() };
        envelope(r)
    }

    /// The picture from the last successful [`preview`](InstrumentFile::preview), encoded as it
    /// says in `encoding` (empty otherwise).
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
            let mut slice = TableSlice {
                path: path.clone(),
                format: format.into(),
                table,
                first_row,
                total_rows: t.row_count,
                columns: t.columns.iter().map(|c| c.name.clone()).collect(),
                labels: t.columns.iter().map(|c| c.label.clone()).collect(),
                texts: Default::default(),
                truncated: first_row + (n as u64) < t.row_count,
                rows,
                processing: None,
                filter: None,
            };
            slice.attach_texts(&t);
            Ok(slice)
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

    /// `scans --json`: the scan headers of run `run`, without decoding peaks, filtered by
    /// `ms_level`, `polarity`, `rt_range` (`[start, end]` minutes), `precursor` (with
    /// `precursor_tol` or `precursor_ppm`), `charge`, `activation` and `scan_filter`, every match
    /// counted and `limit` (default 100) listed from the `offset`-th. `options` is a JSON object
    /// with those keys and `run` (default 0).
    pub fn scans(&mut self, options: Option<String>) -> String {
        let o = match parse_options::<ScansOptions>("scans", options.as_deref()) {
            Ok(o) => o,
            Err(e) => return envelope::<()>(Err(e)),
        };
        let path = self.name();
        let filter = openreadout_core::ScanFilter {
            ms_level: o.ms_level,
            polarity: o.polarity.map(|p| p.to_ascii_lowercase()),
            rt_min_s: o.rt_range.map(|r| r[0] * 60.0),
            rt_max_s: o.rt_range.map(|r| r[1] * 60.0),
            precursor_mz: o.precursor,
            precursor_tol_mz: o.precursor_tol,
            precursor_tol_ppm: o.precursor_ppm,
            charge: o.charge,
            activation: o.activation,
            filter_contains: o.scan_filter,
        };
        envelope(self.with_dataset(|ds, format| {
            openreadout_core::scans::scan_list(
                ds,
                &path,
                format,
                o.run,
                &filter,
                o.offset,
                o.limit.unwrap_or(100),
            )
        }))
    }

    /// `spectrum --json`: one spectrum of run `run`, by `spectrum` (the zero-based index) or
    /// `scan` (the instrument's scan number), the instrument's centroid list with `centroid`,
    /// its arrays cut to `max_points` points (default 100000). `options` is a JSON object with
    /// those keys.
    pub fn spectrum(&mut self, options: Option<String>) -> String {
        let o = match parse_options::<SpectrumOptions>("spectrum", options.as_deref()) {
            Ok(o) => o,
            Err(e) => return envelope::<()>(Err(e)),
        };
        let path = self.name();
        let run = o.run;
        let view = if o.centroid {
            SpectrumView::Centroid
        } else {
            SpectrumView::Primary
        };
        let r = self.with_dataset(|ds, format| {
            let mut sp = match (o.spectrum, o.scan) {
                (Some(i), None) => ds.read_spectrum_view(run, i, view)?,
                (None, Some(n)) => openreadout_core::reader::spectrum_by_scan(ds, run, n, view)?,
                _ => return Err(Error::Usage("give one of spectrum and scan".into())),
            };
            let point_count = sp.mz.len() as u64;
            let keep = usize::try_from(o.max_points.unwrap_or(100_000)).unwrap_or(usize::MAX);
            let truncated = sp.mz.len() > keep;
            let peaks = openreadout_core::scans::top_peaks(&sp, 20);
            openreadout_core::scans::keep_most_intense(&mut sp, keep);
            Ok(SpectrumOutput {
                path: path.clone(),
                format: format.into(),
                run,
                view: if o.centroid { "centroid" } else { "primary" }.into(),
                point_count,
                truncated,
                peaks,
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

/// `options` of [`InstrumentFile::scans`].
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ScansOptions {
    run: u32,
    ms_level: Option<u32>,
    polarity: Option<String>,
    rt_range: Option<[f64; 2]>,
    precursor: Option<f64>,
    precursor_tol: Option<f64>,
    precursor_ppm: Option<f64>,
    charge: Option<i32>,
    activation: Option<String>,
    scan_filter: Option<String>,
    offset: u64,
    limit: Option<u64>,
}

/// `options` of [`InstrumentFile::spectrum`].
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct SpectrumOptions {
    run: u32,
    spectrum: Option<u64>,
    scan: Option<u64>,
    centroid: bool,
    max_points: Option<u64>,
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
    fn lazy_in(path: &str, fs: LazyFs) -> InstrumentFile {
        let fs = Arc::new(fs);
        let input = Input::new(path, Fs::new(fs.clone() as Arc<dyn DirSource>));
        InstrumentFile::new(input, Some(fs))
    }

    fn need_object(&self, need: Need) -> js_sys::Object {
        let o = js_sys::Object::new();
        let set = |k: &str, v: JsValue| {
            let _ = js_sys::Reflect::set(&o, &JsValue::from_str(k), &v);
        };
        match need {
            Need::Lookup(path) => {
                set("path", path.display().to_string().into());
                set("lookup", JsValue::TRUE);
            }
            Need::Block(path, block) => {
                let size = self.lazy_size(&path).unwrap_or(0);
                let at = block * LAZY_BLOCK;
                #[allow(clippy::cast_precision_loss)]
                {
                    set("path", path.display().to_string().into());
                    set("size", JsValue::from_f64(size as f64));
                    set("offset", JsValue::from_f64(at as f64));
                    set(
                        "length",
                        JsValue::from_f64(size.saturating_sub(at).min(LAZY_BLOCK) as f64),
                    );
                }
            }
        }
        o
    }

    fn lazy_size(&self, path: &Path) -> Option<u64> {
        let l = self.lazy.as_ref()?;
        let sources = l.sources.lock().unwrap_or_else(PoisonError::into_inner);
        sources.iter().find(|s| s.path == path).map(|s| s.size)
    }

    /// Run `op`; for lazy files, a result computed while something was missing is replaced by
    /// an error (the host provides what [`pending`](InstrumentFile::pending) names and repeats
    /// the call).
    fn attempt<T>(
        &mut self,
        op: impl FnOnce(&mut Self, &Input) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if let Some(l) = &self.lazy {
            l.clear_needs();
        }
        let input = self.input.clone();
        let r = op(self, &input);
        let Some(need) = self
            .lazy
            .as_ref()
            .and_then(|l| l.needs().into_iter().next())
        else {
            return r;
        };
        // Whatever the reader made of the missing bytes is discarded, including an opened data
        // set whose state may reflect them.
        self.opened = None;
        let message = match need {
            Need::Lookup(path) => {
                format!("{} must be looked up: answer and repeat", path.display())
            }
            Need::Block(path, block) => format!(
                "bytes from {} of {} are needed: provide them and repeat",
                block * LAZY_BLOCK,
                path.display()
            ),
        };
        Err(Error::Io {
            path: PathBuf::from(self.name()),
            source: io::Error::new(io::ErrorKind::WouldBlock, message),
        })
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

/// Render a preview, drawn at `scale` PNG or JPEG pixels per figure pixel. An image is read
/// again at the larger size so that its pixels are real, as `preview --scale` does in the CLI.
fn render_scaled(
    ds: &mut dyn Dataset,
    info: &openreadout_core::model::FileInfo,
    req: &PreviewRequest,
    encoding: Encoding,
    scale: f32,
) -> Result<openreadout_preview::Rendered, Error> {
    let mut rendered = openreadout_preview::render(ds, info, req)?;
    if scale <= 1.0 || encoding == Encoding::Svg {
        return Ok(rendered);
    }
    if rendered.output.kind == "image" {
        let mut big = req.clone();
        big.max_size = ((req.max_size as f32 * scale).round() as u32)
            .clamp(16, openreadout_preview::MAX_PREVIEW_SIZE);
        let b = openreadout_preview::render(ds, info, &big)?;
        if !rendered.take_pixels(&b, scale) {
            return Ok(b);
        }
    }
    rendered.canvas = rendered.figure.to_raster(scale);
    Ok(rendered)
}

fn preview_request(options: Option<&str>) -> Result<(PreviewRequest, Encoding, f32), Error> {
    let text = options.filter(|t| !t.trim().is_empty()).unwrap_or("{}");
    let mut v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| Error::Usage(format!("preview options are not JSON: {e}")))?;
    let obj = v
        .as_object_mut()
        .ok_or_else(|| Error::Usage("preview options must be a JSON object".into()))?;
    let encoding = match obj.remove("format") {
        None => Encoding::Png,
        Some(serde_json::Value::String(f)) => f.parse()?,
        Some(_) => {
            return Err(Error::Usage(
                "preview option `format` is png, jpeg or svg".into(),
            ));
        }
    };
    let scale = match obj.remove("scale") {
        None => 1.0,
        Some(serde_json::Value::Number(n)) => match n.as_f64() {
            Some(x) if (1.0..=4.0).contains(&x) => x as f32,
            _ => {
                return Err(Error::Usage(format!(
                    "preview option `scale` is a number from 1 to 4, not {n}"
                )));
            }
        },
        Some(_) => {
            return Err(Error::Usage(
                "preview option `scale` is a number from 1 to 4".into(),
            ));
        }
    };
    let opts: PreviewOptions =
        serde_json::from_value(v).map_err(|e| Error::Usage(format!("preview options: {e}")))?;
    let mut req = opts.request(DEFAULT_WASM_PREVIEW_SIZE, Axes::None)?;
    req.max_size = req.max_size.clamp(16, 4096);
    Ok((req, encoding, scale))
}

/// Detect, open and summarise in one call (the demo page's first request): the `info`
/// envelope for `data` named `name`.
#[wasm_bindgen]
pub fn info(name: &str, data: Vec<u8>) -> String {
    InstrumentFile::from_bytes(name, data).info(None, None)
}
