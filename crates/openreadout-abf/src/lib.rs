//! Reader for Axon Binary Format (ABF 1 and ABF 2) electrophysiology recordings.
//!
//! Layout and vocabulary: `docs/formats/abf.md`. Provenance: `docs/provenance/abf.md` (derived
//! from the public, MIT-licensed pyABF documentation and the corpus; the Axon SDK was never
//! consulted). A file is exposed as one trace: `sweep_count` sweeps × input channels, samples
//! scaled to physical units with the per-channel gain chain recorded in the header.
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
//! use openreadout_abf::AbfReader;
//!
//! let format = AbfReader.descriptor();
//! assert_eq!(format.id, openreadout_abf::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_abf::AbfReader;
//!
//! let mut dataset = AbfReader.open(Path::new("cell.abf"))?;
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
pub mod atf;
#[doc(hidden)]
pub mod command;
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod file;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

pub use atf::{ATF_FORMAT_ID, AtfReader};
#[doc(hidden)]
pub use atf::{
    AtfColumn, AtfDataset, MAX_ATF_LEN, MAX_COLUMNS, MAX_HEADER_RECORDS, looks_like_atf,
};
#[doc(hidden)]
pub use command::{CommandPlan, command_plan, command_sweep};
#[doc(hidden)]
pub use dataset::{AbfDataset, MAX_READ_SAMPLES, command_trace_info, trace_info};
#[doc(hidden)]
pub use file::{
    ABF1_BASIC_LEN, ABF1_EXTENDED_LEN, AbfFile, AcquisitionMode, BLOCK_LEN, DigitalEpoch, Epoch,
    EpochKind, Generation, InputChannel, MAX_RECORDS, MAX_STRINGS_LEN, OutputChannel,
    SECTION_ENTRY_LEN, SECTION_MAP_OFFSET, SECTION_NAMES, SampleFormat, SectionEntry, Sweep, Tag,
    Telegraph, channel_scaling, guid_text, iso_datetime, parse_string_table, usable_factor,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "abf";

/// The ABF reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct AbfReader;

impl FormatReader for AbfReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::ABF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Axon Binary Format (ABF)".into(),
            vendor: "Molecular Devices (Axon Instruments) pCLAMP".into(),
            extensions: vec!["abf".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::ABF.confidence,
            known_gaps: vec![
                "Command (DAC) waveforms are synthesized (trace 1) for ABF 2 episodic files with step, ramp and pulse-train epochs only; stimulus files, user lists, alternating outputs, conditioning trains, triangle/cosine/biphasic epochs and ABF 1 files keep the epoch table only".into(),
                "User lists, statistics, math, scope and voice-tag sections are listed but not decoded".into(),
                "Pre-1.6 ABF 1 files: telegraph, protocol path and 20-entry epoch tables are not in the header and are not reported".into(),
                "Old pCLAMP/Axotape files without an `ABF ` signature (CLPX, FTCX) are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        match head.get(..4) {
            Some(b"ABF2") => Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            }),
            Some(b"ABF ") => Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            }),
            _ if has_extension(path, &["abf"]) => Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("ABF extension but no `ABF ` or `ABF2` signature at byte 0".into()),
            }),
            _ => None,
        }
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(AbfDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(AbfDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
