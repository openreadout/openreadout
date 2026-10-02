//! Reader for SpikeGLX recordings: a `.meta` text file beside a headerless interleaved int16
//! `.bin` (Neuropixels imec AP/LF streams, NI-DAQ and OneBox streams).
//!
//! Layout and vocabulary: `docs/formats/spikeglx.md`. Provenance: `docs/provenance/spikeglx.md`
//! (the SpikeGLX authors' public metadata documentation). One stream (one `.bin`) is one trace
//! with a single sweep; channels are scaled to µV (imec) or V (NI, OneBox) as the documentation
//! specifies; status (SY) and digital (XD) words are returned raw.
//!
//! # Example
//!
//! Every reader implements [`openreadout_core::FormatReader`]: `descriptor`
//! says what it reads, `sniff` recognises a file from its first bytes, and `open` returns a
//! [`openreadout_core::Dataset`] with a header-only `info()` summary and lazy data
//! access.
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_spikeglx::SpikeGlxReader;
//!
//! let format = SpikeGlxReader.descriptor();
//! assert_eq!(format.id, openreadout_spikeglx::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_spikeglx::SpikeGlxReader;
//!
//! let mut dataset = SpikeGlxReader.open(Path::new("run_g0_t0.imec0.ap.bin"))?;
//! let info = dataset.info()?; // headers only
//! let trace = &info.traces[0];
//! println!("{} sweep(s) at {} Hz, {} channel(s)", trace.sweep_count, trace.sample_rate_hz, trace.channels.len());
//! // The first 1000 samples of sweep 0, every channel, scaled to physical units.
//! let samples = dataset.read_trace(0, 0, 0, 1000)?;
//! for (channel, values) in trace.channels.iter().zip(&samples.channels) {
//!     println!("{} ({}): {:?}", channel.name, channel.unit.as_deref().unwrap_or("?"), values.first());
//! }
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! Applications that accept any instrument file usually register every reader in an
//! [`openreadout_core::Registry`] and let it detect the format; that is what the
//! `openreadout` command-line tool does.
//!
//! # API stability
//!
//! The supported API is what this page documents. The parser modules are public only so that
//! tests and fuzz targets can reach them: they are hidden from this documentation and may change
//! in any release.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod assurance;
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod meta;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use dataset::{MAX_META_LEN, SpikeGlxDataset, stream_paths};
#[doc(hidden)]
pub use meta::{
    ChannelKind, Meta, SavedChannel, StreamKind, default_max_int, fixed_gain, has_selectable_gain,
    imec_scale, list_elements, ni_scale, parse_meta, parse_subset, saved_channels,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "spikeglx";

/// True when `.meta` text looks like SpikeGLX metadata.
pub fn looks_like_meta(text: &[u8]) -> bool {
    let t = String::from_utf8_lossy(&text[..text.len().min(1 << 16)]);
    t.contains("nSavedChans=") && (t.contains("typeThis=") || t.contains("SampRate="))
}

/// Files a SpikeGLX run directory may hold besides the stream pairs: `.meta` files (read with
/// their `.bin`), logs, notes and text outputs.
pub const SESSION_COMPANIONS: [&str; 6] = ["meta", "txt", "log", "json", "md", "csv"];

/// A probe subdirectory of a run: its name ends in `_imec` and a probe number.
fn is_probe_dir_name(name: &str) -> bool {
    name.rsplit_once("_imec")
        .is_some_and(|(_, n)| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The stream `.bin` files (each with its `.meta`) of a SpikeGLX run directory, or `None` when
/// `dir` is not one (see [`openreadout_core::session::session_files`]). The run directory
/// `<run>_g0/` holds the NI-DAQ and OneBox streams; each probe's streams are in a subdirectory
/// named `…_imec<N>` (normally `<run>_g0_imec<N>/`), which is read too. Other subdirectories
/// (post-processing outputs) are ignored, and a directory that only wraps a run directory is not
/// a run itself.
pub fn session_files(dir: &Path) -> Option<Vec<std::path::PathBuf>> {
    session_files_in(&Input::local(dir))
}

fn session_files_in(input: &Input) -> Option<Vec<std::path::PathBuf>> {
    let dir = input.path();
    let fs = input.fs();
    let data = |p: &Path| has_extension(p, &["bin"]) && fs.is_file(&p.with_extension("meta"));
    let own = |d: &Path| {
        openreadout_core::session::session_files_in(fs, d, &data, &SESSION_COMPANIONS, 0)
    };
    let mut files = Vec::new();
    // the run directory's own files; a directory with only probe subdirectories is fine
    if let Some(v) = own(dir) {
        files.extend(v);
    } else {
        let foreign = fs
            .read_dir(dir)
            .ok()?
            .filter_map(std::result::Result::ok)
            .any(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                e.file_type().is_ok_and(|t| t.is_file())
                    && !n.starts_with('.')
                    && !has_extension(&e.path(), &["ok"])
                    && !has_extension(&e.path(), &SESSION_COMPANIONS)
            });
        if foreign {
            return None;
        }
    }
    let mut probes: Vec<std::path::PathBuf> = fs
        .read_dir(dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .filter(|e| {
            e.file_type().is_ok_and(|t| t.is_dir())
                && is_probe_dir_name(&e.file_name().to_string_lossy())
        })
        .map(|e| e.path())
        .collect();
    probes.sort();
    for p in probes {
        files.extend(own(&p)?);
    }
    files.sort();
    (!files.is_empty()).then_some(files)
}

/// Open a SpikeGLX run directory: every stream (imec AP and LF bands, NI-DAQ, OneBox) is a
/// trace of its own, with its own rate and sample clock; nothing is resampled or realigned.
pub fn open_session(dir: &Path) -> Result<openreadout_core::session::SessionDataset> {
    open_session_input(&Input::local(dir))
}

fn open_session_input(input: &Input) -> Result<openreadout_core::session::SessionDataset> {
    let dir = input.path();
    let files = session_files_in(input).ok_or_else(|| {
        openreadout_core::Error::unsupported(
            FORMAT_ID,
            "a directory that is not a SpikeGLX run",
            "A SpikeGLX run directory holds .bin/.meta stream pairs (probe streams may sit one level down); open a single .bin or .meta otherwise.",
        )
    })?;
    let mut members: Vec<Box<dyn Dataset>> = Vec::new();
    for f in &files {
        members.push(Box::new(SpikeGlxDataset::open_input(&input.with_path(f))?));
    }
    openreadout_core::session::SessionDataset::new(
        FORMAT_ID,
        dir,
        members,
        openreadout_core::session::Combine::Separate,
    )
}

/// The SpikeGLX reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct SpikeGlxReader;

impl FormatReader for SpikeGlxReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::SPIKEGLX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "SpikeGLX (.bin + .meta)".into(),
            vendor: "SpikeGLX (HHMI Janelia), Neuropixels / NI-DAQ / OneBox".into(),
            extensions: vec!["bin".into(), "meta".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::SPIKEGLX.confidence,
            known_gaps: vec![
                "A run directory is one session with one trace per stream (AP, LF, NI, OneBox), each on its own clock; streams are not aligned on their sync pulses".into(),
                "Probe sites come from ~snsGeomMap (µm) or ~snsShankMap (grid column/row, not converted to µm); imro bank/reference settings are reported in `info --view full`, not normalized".into(),
                "imMaxInt defaults for probe types other than NP 1.0 and 2.0 (types 21/24) are inferred from the ProbeTable when the metadata omits it".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    /// A `.bin` is recognised by the `.meta` next to it, looked up in the input's namespace.
    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if input.is_dir() {
            // Definite once a stream's .meta reads as SpikeGLX metadata, so that a run directory
            // holding its probe subdirectory stays one data set (not walked into by `index`).
            let files = session_files_in(input)?;
            let signed = files.iter().take(8).any(|f| {
                dataset::read_meta(input.fs(), &f.with_extension("meta"))
                    .is_ok_and(|t| t.len() as u64 <= MAX_META_LEN && looks_like_meta(&t))
            });
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: if signed {
                    DetectConfidence::Definite
                } else {
                    DetectConfidence::Likely
                },
                note: Some("SpikeGLX run directory (every stream read as one session)".into()),
            });
        }
        if has_extension(path, &["meta"]) && looks_like_meta(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if has_extension(path, &["bin"]) {
            let (_, meta) = stream_paths(path);
            if let Ok(text) = dataset::read_meta(input.fs(), &meta)
                && text.len() as u64 <= MAX_META_LEN
                && looks_like_meta(&text)
            {
                return Some(Detection {
                    format_id: FORMAT_ID,
                    confidence: DetectConfidence::Definite,
                    note: None,
                });
            }
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        if input.is_dir() {
            return Ok(Box::new(open_session_input(input)?));
        }
        Ok(Box::new(SpikeGlxDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
