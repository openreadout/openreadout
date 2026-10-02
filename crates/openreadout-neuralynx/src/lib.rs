//! Reader for Neuralynx Cheetah/Pegasus files: NCS continuous channels, NEV events, and
//! NSE/NST/NTT spike waveforms.
//!
//! Layout and vocabulary: `docs/formats/neuralynx.md`. Provenance: `docs/provenance/neuralynx.md`
//! (Neuralynx's public "Data File Formats" document; Neo, BSD-3, read as documentation). Every
//! file is a 16 KiB text header followed by fixed-size records. An NCS file is one trace whose
//! sweeps are its gap-free segments; event and spike files are tables.
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
//! use openreadout_neuralynx::NeuralynxReader;
//!
//! let format = NeuralynxReader.descriptor();
//! assert_eq!(format.id, openreadout_neuralynx::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_neuralynx::NeuralynxReader;
//!
//! let mut dataset = NeuralynxReader.open(Path::new("CSC1.ncs"))?;
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
pub mod header;
#[doc(hidden)]
pub mod records;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use dataset::{MAX_LABELS, MAX_TABLE_READ, NeuralynxDataset};
#[doc(hidden)]
pub use header::{HEADER_LEN, HEADER_MAGIC, HeaderEntry, TextHeader, decode_text, parse_header};
#[doc(hidden)]
pub use records::{
    ContinuousIndex, EventRecord, FileKind, NCS_RECORD_LEN, NCS_SAMPLES, NEV_RECORD_LEN,
    NVT_POINTS, NVT_RECORD_LEN, NVT_RECORD_START, NVT_TARGETS, RecordHead, SPIKE_FEATURES,
    SPIKE_PREFIX_LEN, SPIKE_SAMPLES, Segment, SpikeRecord, VideoRecord, event_record,
    index_continuous, record_head, segments_from, spike_record, video_record,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "neuralynx";

/// Extensions this reader opens.
pub const EXTENSIONS: [&str; 6] = ["ncs", "nev", "nse", "nst", "ntt", "nvt"];

/// Files next to the recording that a session directory may hold besides the data files: logs,
/// settings, and the raw-data files this reader does not decode.
pub const SESSION_COMPANIONS: [&str; 8] = ["txt", "log", "cfg", "xml", "ini", "json", "md", "nrd"];

/// The data files of a Neuralynx recording directory, or `None` when `dir` is not one (see
/// [`openreadout_core::session::session_files`]). Continuous files holding only the text header
/// (no records) are left out.
pub fn session_files(dir: &Path) -> Option<Vec<std::path::PathBuf>> {
    session_files_in(&Input::local(dir))
}

fn session_files_in(input: &Input) -> Option<Vec<std::path::PathBuf>> {
    let dir = input.path();
    let fs = input.fs();
    let files = openreadout_core::session::session_files_in(
        fs,
        dir,
        &|p| has_extension(p, &EXTENSIONS),
        &SESSION_COMPANIONS,
        0,
    )?;
    let kept: Vec<_> = files
        .into_iter()
        .filter(|p| {
            !has_extension(p, &["ncs"]) || fs.metadata(p).is_ok_and(|m| m.len() > HEADER_LEN)
        })
        .collect();
    (!kept.is_empty()).then_some(kept)
}

/// Open a Neuralynx recording directory: every `.ncs` channel on the same sample grid (rate,
/// segments and their start stamps) joins one multi-channel trace, and each `.nev` / `.nse` /
/// `.nst` / `.ntt` file is a table.
pub fn open_session(dir: &Path) -> Result<openreadout_core::session::SessionDataset> {
    open_session_input(&Input::local(dir))
}

fn open_session_input(input: &Input) -> Result<openreadout_core::session::SessionDataset> {
    let dir = input.path();
    let files = session_files_in(input).ok_or_else(|| {
        openreadout_core::Error::unsupported(
            FORMAT_ID,
            "a directory that is not a Neuralynx recording",
            "A Neuralynx session directory holds .ncs/.nev/.nse/.nst/.ntt files (plus logs); open a single file otherwise.",
        )
    })?;
    let mut members: Vec<Box<dyn Dataset>> = Vec::new();
    for f in &files {
        members.push(Box::new(NeuralynxDataset::open_input(&input.with_path(f))?));
    }
    let mut s = openreadout_core::session::SessionDataset::new(
        FORMAT_ID,
        dir,
        members,
        openreadout_core::session::Combine::SameGrid,
    )?;
    for t in &mut s.info_mut().traces {
        if t.channels.len() > 1 {
            t.name = Some(format!("continuous_{}Hz", t.sample_rate_hz));
        }
    }
    Ok(s)
}

/// The Neuralynx reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct NeuralynxReader;

/// True when the head looks like a Neuralynx text header: the header line, or `-FileType` /
/// `-RecordSize` / `NLX_Base_Class` keys in the first kilobytes.
pub fn looks_like_neuralynx(head: &[u8]) -> bool {
    let n = head.len().min(4096);
    let text = decode_text(&head[..n]);
    text.trim_start().starts_with(HEADER_MAGIC)
}

fn has_header_keys(head: &[u8]) -> bool {
    let n = head.len().min(4096);
    let text = decode_text(&head[..n]);
    [
        "-RecordSize",
        "-FileType",
        "NLX_Base_Class_Type",
        "-ADBitVolts",
    ]
    .iter()
    .any(|k| text.contains(k))
}

impl FormatReader for NeuralynxReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::NEURALYNX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Neuralynx (NCS/NEV/NSE/NTT)".into(),
            vendor: "Neuralynx (Cheetah, Pegasus)".into(),
            extensions: EXTENSIONS.iter().map(|e| (*e).to_string()).collect(),
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::NEURALYNX.confidence,
            known_gaps: vec![
                "A recording directory is one session: channels on different sample grids (rate, segment boundaries) are separate traces, never resampled; event and spike files are separate tables".into(),
                "Raw data (.nrd) files are not read; video-tracker (.nvt) records give the extracted position, angle and target count, not the colour-transition points or target bitfields".into(),
                "Sampling rate is the header value; when timestamps imply another rate (pre-Cheetah-5 1 MHz clock) it is reported in extra.timestamp_rate_hz, not applied".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if input.is_dir() {
            let files = session_files_in(input)?;
            let signed = files.iter().take(8).any(|f| {
                let mut head = [0u8; 1024];
                input
                    .fs()
                    .open(f)
                    .and_then(|mut h| std::io::Read::read(&mut h, &mut head))
                    .is_ok_and(|n| looks_like_neuralynx(&head[..n]))
            });
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: if signed {
                    DetectConfidence::Definite
                } else {
                    DetectConfidence::Likely
                },
                note: Some("Neuralynx recording directory (read as one session)".into()),
            });
        }
        let ext = has_extension(path, &EXTENSIONS);
        if looks_like_neuralynx(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if ext && has_header_keys(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("Neuralynx header keys without the header line".into()),
            });
        }
        if ext {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("Neuralynx extension but no Neuralynx text header".into()),
            });
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
        Ok(Box::new(NeuralynxDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
