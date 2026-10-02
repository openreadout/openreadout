//! Reader for Blackrock Neurotech NSx continuous files (spec 2.1, 2.2/2.3, 3.0 including PTP)
//! and NEV event files.
//!
//! Layout and vocabulary: `docs/formats/blackrock.md`. Provenance: `docs/provenance/blackrock.md`
//! (Blackrock's public NEV/NSx specification LB-0023; Neo, BSD-3, read as documentation for
//! spec 2.1). An NSx file is one trace whose sweeps are its data packets (recording pauses); a
//! NEV file is one table of data packets (spikes with waveforms, digital events, comments).
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
//! use openreadout_blackrock::BlackrockReader;
//!
//! let format = BlackrockReader.descriptor();
//! assert_eq!(format.id, openreadout_blackrock::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_blackrock::BlackrockReader;
//!
//! let mut dataset = BlackrockReader.open(Path::new("session.ns5"))?;
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
pub mod nev;
#[doc(hidden)]
pub mod nsx;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use dataset::{
    KIND_COMMENT, KIND_DIGITAL, KIND_OTHER, KIND_SPIKE, MAX_COMMENTS, MAX_TABLE_READ, NevDataset,
    NsxDataset,
};
#[doc(hidden)]
pub use nev::{
    COMMENT_PACKET, ExtHeader, MAX_ELECTRODE_ID, NEV_BASIC_LEN, NEV_EXT_LEN, NevFile, NevPacket,
    WaveformHeader, nev_packet, parse_nev,
};
#[doc(hidden)]
pub use nsx::{
    BASIC_HEADER_LEN, CC_LEN, MAX_CHANNELS, NsxChannel, NsxFile, NsxPacket, NsxSpec, NsxSweep,
    PERIOD_CLOCK_HZ, PTP_RESOLUTION, SPEC21_HEADER_LEN, cc_scaling, parse_nsx, spec21_factor,
    spec21_label, systemtime,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "blackrock";

/// Extensions this reader opens.
pub const EXTENSIONS: [&str; 7] = ["ns1", "ns2", "ns3", "ns4", "ns5", "ns6", "nev"];

/// Leading bytes of the file kinds.
pub const SIGNATURES: [&[u8; 8]; 5] = [
    b"NEURALSG",
    b"NEURALCD",
    b"BRSMPGRP",
    b"NEURALEV",
    b"BREVENTS",
];

/// Files a Blackrock recording directory may hold besides NSx/NEV data: the Central
/// configuration (`.ccf`), logs and notes.
pub const SESSION_COMPANIONS: [&str; 6] = ["ccf", "txt", "log", "xml", "json", "md"];

/// The NSx and NEV files of a Blackrock recording directory, or `None` when `dir` is not one
/// (see [`openreadout_core::session::session_files`]).
pub fn session_files(dir: &Path) -> Option<Vec<std::path::PathBuf>> {
    session_files_in(&Input::local(dir))
}

fn session_files_in(input: &Input) -> Option<Vec<std::path::PathBuf>> {
    let dir = input.path();
    let fs = input.fs();
    openreadout_core::session::session_files_in(
        fs,
        dir,
        &|p| has_extension(p, &EXTENSIONS),
        &SESSION_COMPANIONS,
        0,
    )
}

/// Open a Blackrock recording directory: each NSx file (`.ns1`–`.ns6`, one sampling group each)
/// is a trace with its own rate and clock, each NEV file a table. Spec 2.1 NSx files take their
/// scaling from the NEV of the same name, as when opened alone.
pub fn open_session(dir: &Path) -> Result<openreadout_core::session::SessionDataset> {
    open_session_input(&Input::local(dir))
}

fn open_session_input(input: &Input) -> Result<openreadout_core::session::SessionDataset> {
    let dir = input.path();
    let files = session_files_in(input).ok_or_else(|| {
        openreadout_core::Error::unsupported(
            FORMAT_ID,
            "a directory that is not a Blackrock recording",
            "A Blackrock session directory holds .ns1-.ns6 and .nev files (plus .ccf/logs); open a single file otherwise.",
        )
    })?;
    let mut members = Vec::new();
    for f in &files {
        members.push(BlackrockReader.open_input(&input.with_path(f))?);
    }
    openreadout_core::session::SessionDataset::new(
        FORMAT_ID,
        dir,
        members,
        openreadout_core::session::Combine::Separate,
    )
}

/// The Blackrock reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct BlackrockReader;

impl FormatReader for BlackrockReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BLACKROCK)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Blackrock NSx/NEV".into(),
            vendor: "Blackrock Neurotech (Cerebus, NeuroPort)".into(),
            extensions: EXTENSIONS.iter().map(|e| (*e).to_string()).collect(),
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BLACKROCK.confidence,
            known_gaps: vec![
                "A recording directory is one session (each NSx file a trace, each NEV a table, clocks kept); NEV rows are not assigned to NSx sweeps".into(),
                "NEV video-sync, tracking, button, configuration and log packets are counted (kind 3), not decoded; comment text is in `info --view full`".into(),
                "Spec 2.1 NSx scaling needs the .nev with the same name next to it".into(),
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
                let mut head = [0u8; 8];
                input
                    .fs()
                    .open(f)
                    .and_then(|mut h| std::io::Read::read(&mut h, &mut head))
                    .is_ok_and(|n| n == 8 && SIGNATURES.iter().any(|s| head == **s))
            });
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: if signed {
                    DetectConfidence::Definite
                } else {
                    DetectConfidence::Likely
                },
                note: Some("Blackrock recording directory (NSx + NEV, read as one session)".into()),
            });
        }
        if head.len() >= 8 && SIGNATURES.iter().any(|s| &head[..8] == *s) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &EXTENSIONS).then(|| Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("Blackrock extension but no NSx/NEV signature at byte 0".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let path = input.path();
        if input.is_dir() {
            return Ok(Box::new(open_session_input(input)?));
        }
        let mut head = [0u8; 8];
        let mut f = input.open()?;
        let n = std::io::Read::read(&mut f, &mut head)
            .map_err(|e| openreadout_core::Error::io(path, e))?;
        if n == 8 && (&head == b"NEURALEV" || &head == b"BREVENTS") {
            Ok(Box::new(NevDataset::open_input(input)?))
        } else {
            Ok(Box::new(NsxDataset::open_input(input)?))
        }
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
