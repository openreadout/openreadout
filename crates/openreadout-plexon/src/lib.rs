//! Reader for Plexon PLX ("Plexon 1") and PL2 electrophysiology recordings.
//!
//! Layout and vocabulary: `docs/formats/plexon.md`. Provenance: `docs/provenance/plexon.md` (PLX
//! from Neo, BSD-3, read as documentation; PL2 from hex dumps of corpus files only — Plexon's own
//! PL2 reader is a DLL that is neither opened nor run). Continuous channels are traces (channels
//! sampled on one grid share a trace; recording pauses split sweeps); spike waveforms and events
//! are tables.
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
//! use openreadout_plexon::PlexonReader;
//!
//! let format = PlexonReader.descriptor();
//! assert_eq!(format.id, openreadout_plexon::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_plexon::PlexonReader;
//!
//! let mut dataset = PlexonReader.open(Path::new("session.plx"))?;
//! let info = dataset.info()?;
//! for t in &info.traces {
//!     println!("trace {}: {} channel(s) at {} Hz", t.index, t.channels.len(), t.sample_rate_hz);
//! }
//! // The first 100 spikes: time, channel, unit and waveform samples in mV.
//! let spikes = dataset.read_table(0, 0, 100)?;
//! println!("{} spike rows", spikes.columns[0].len());
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
pub mod pl2;
#[doc(hidden)]
pub mod pl2_dataset;
#[doc(hidden)]
pub mod plx;

use openreadout_core::source::Input;
use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};

#[doc(hidden)]
pub use dataset::{MAX_TABLE_READ, PlxDataset, PlxTrace};
#[doc(hidden)]
pub use pl2::{
    EventRun, MAX_PL2_CHANNELS, PL2_FILE_HEADER_LEN, PL2_MAGIC, PL2_MAGIC_AT, Pl2Channel, Pl2File,
    Pl2Header, Pl2Index, REC_ANALOG, REC_ANALOG_HEADER, REC_DIGITAL_HEADER, REC_END, REC_EVENTS,
    REC_SPIKE_HEADER, REC_SPIKES, RECORD_HEADER_LEN, SpikeRun, UNIT_SLOTS, looks_like_pl2,
    parse_pl2, parse_pl2_header, record_len, walk_records,
};
#[doc(hidden)]
pub use pl2_dataset::{Pl2Dataset, Pl2Trace};
#[doc(hidden)]
pub use plx::{
    BLOCK_CONTINUOUS, BLOCK_EVENT, BLOCK_HEADER_LEN, BLOCK_SPIKE, CONTINUOUS_HEADER_LEN,
    ContinuousChannel, EVENT_HEADER_LEN, EventChannel, EventRecord, FILE_HEADER_LEN,
    MAX_CHANNEL_HEADERS, PLX_MAGIC, PlxFile, PlxHeader, PlxIndex, Run, SPIKE_HEADER_LEN,
    SampleBlock, SpikeChannel, block_header, parse_file_header, parse_plx, runs, walk_blocks,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "plexon";

/// Extensions this reader opens.
pub const EXTENSIONS: [&str; 2] = ["plx", "pl2"];

/// The Plexon reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlexonReader;

impl FormatReader for PlexonReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::PLEXON)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Plexon PLX/PL2".into(),
            vendor: "Plexon (MAP, OmniPlex)".into(),
            extensions: EXTENSIONS.iter().map(|e| (*e).to_string()).collect(),
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::PLEXON.confidence,
            known_gaps: vec![
                "PLX has no index: `info` walks every data-block header (fast, but proportional to the file size)".into(),
                "PLX values are in mV by the gain formulas Neo documents; version 100/101 files assume a preamplifier gain of 1000".into(),
                "PL2 is derived from hex dumps of public files (Plexon's reader is a DLL); spike waveforms and events are confirmed against Neo on a PLX of the same recording, PL2 continuous records only for internal consistency".into(),
                "PLX version 107 (and continuous channels of unequal length) are read but have no oracle-confirmed continuous data".into(),
                "PL2 footer index and device settings are not read; `info` walks the data-record headers".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.get(..4) == Some(&PLX_MAGIC[..]) || looks_like_pl2(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &EXTENSIONS).then(|| Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("Plexon extension but no PLX/PL2 signature".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn reads_any_source(&self) -> bool {
        true
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let path = input.path();
        let mut head = [0u8; 16];
        let n = input
            .fs()
            .open(path)
            .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
            .map_err(|e| openreadout_core::Error::io(path, e))?;
        if looks_like_pl2(&head[..n]) {
            return Ok(Box::new(Pl2Dataset::open_input(input)?));
        }
        Ok(Box::new(PlxDataset::open_input(input)?))
    }
}
