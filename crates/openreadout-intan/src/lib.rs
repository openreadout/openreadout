//! Reader for Intan Technologies RHD2000 (`.rhd`) and RHS2000 (`.rhs`) recordings in the
//! traditional single-file format.
//!
//! Layout and vocabulary: `docs/formats/intan.md`. Provenance: `docs/provenance/intan.md` (Intan's
//! public data-file-format application notes). The header lists signal groups and channels;
//! samples follow in data blocks of 60 or 128 samples. Each stored signal kind (amplifier,
//! auxiliary, supply, temperature, board ADC/DAC, digital, DC amplifier, stimulation) is one
//! trace with a single sweep, scaled as the application notes specify.
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
//! use openreadout_intan::IntanReader;
//!
//! let format = IntanReader.descriptor();
//! assert_eq!(format.id, openreadout_intan::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_intan::IntanReader;
//!
//! let mut dataset = IntanReader.open(Path::new("recording.rhd"))?;
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
pub mod split;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::bytes::le_u32;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use dataset::{IntanDataset, MAX_HEADER_LEN};
#[doc(hidden)]
pub use header::{
    BlockPart, Cursor, Family, IntanChannel, IntanHeader, RHD_MAGIC, RHS_MAGIC, SignalKind,
    block_layout, parse_header, scaling, stim_steps,
};
#[doc(hidden)]
pub use split::{
    SplitDataset, SplitLayout, SplitStream, StreamFiles, channel_file, info_file, signal_file,
    split_scaling,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "intan";

/// Files a split-layout directory may hold besides `info.rhd`/`info.rhs` and the `.dat` files:
/// the acquisition settings and notes.
pub const SESSION_COMPANIONS: [&str; 5] = ["xml", "txt", "log", "json", "md"];

/// The files of a "one file per signal type" or "one file per channel" Intan directory (its
/// `info.rhd` / `info.rhs` and `.dat` files), or `None` when `dir` is not one.
pub fn session_files(dir: &Path) -> Option<Vec<std::path::PathBuf>> {
    session_files_in(&Input::local(dir))
}

fn session_files_in(input: &Input) -> Option<Vec<std::path::PathBuf>> {
    let dir = input.path();
    split::info_file_in(input.fs(), dir)?;
    openreadout_core::session::session_files_in(
        input.fs(),
        dir,
        &|p| has_extension(p, &["dat", "rhd", "rhs"]),
        &SESSION_COMPANIONS,
        0,
    )
}

/// True for an `info.rhd` / `info.rhs` header file that has `time.dat` or other `.dat` files next
/// to it and no data blocks of its own: opening it opens the split recording.
pub fn is_split_header(path: &Path) -> bool {
    is_split_header_input(&Input::local(path))
}

fn is_split_header_input(input: &Input) -> bool {
    let path = input.path();
    let fs = input.fs();
    let named_info = path
        .file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("info"));
    named_info
        && path.parent().is_some_and(|d| {
            fs.is_file(&d.join("time.dat"))
                || split::info_file_in(fs, d).is_some_and(|_| {
                    fs.read_dir(d).is_ok_and(|mut rd| {
                        rd.any(|e| e.is_ok_and(|e| has_extension(&e.path(), &["dat"])))
                    })
                })
        })
}

/// The Intan reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct IntanReader;

impl FormatReader for IntanReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::INTAN)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Intan RHD2000/RHS2000".into(),
            vendor: "Intan Technologies (RHD/RHS recording and stimulation systems)".into(),
            extensions: vec!["rhd".into(), "rhs".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::INTAN.confidence,
            known_gaps: vec![
                "\"One file per signal type\" and \"one file per channel\" directories (info.rhd/info.rhs + .dat) are read as one recording; temperature sensors are not saved in those layouts".into(),
                "Stimulation words return the signed current; the amp-settle, charge-recovery and compliance bits are not exposed".into(),
                "Split recordings (one file per N minutes) are read file by file".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if input.is_dir() {
            return session_files_in(input).map(|_| Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("Intan split-layout directory (info.rhd/info.rhs + .dat files)".into()),
            });
        }
        let magic = le_u32(head, 0);
        if matches!(magic, Some(RHD_MAGIC | RHS_MAGIC)) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        has_extension(path, &["rhd", "rhs"]).then(|| Detection {
            format_id: FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("Intan extension but no RHD/RHS magic number".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        if input.is_dir() || is_split_header_input(input) {
            return Ok(Box::new(SplitDataset::open_input(input)?));
        }
        Ok(Box::new(IntanDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
