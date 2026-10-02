//! Reader for FCS (Flow Cytometry Standard) files, versions 2.0, 3.0, 3.1 and 3.2.
//!
//! FCS is an open ISAC standard. Layout and vocabulary: `docs/formats/fcs.md`.
//! Provenance: `docs/provenance/fcs.md`. A file is one or more data sets, each a HEADER of
//! ASCII offsets, a TEXT segment of delimited keyword-value pairs, a DATA segment of events,
//! and optional supplemental TEXT, ANALYSIS and OTHER segments; `$NEXTDATA` links data sets.
//! Each data set is exposed as one table (rows = events, columns = parameters).
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
//! use openreadout_fcs::FcsReader;
//!
//! let format = FcsReader.descriptor();
//! assert_eq!(format.id, openreadout_fcs::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_fcs::FcsReader;
//!
//! let mut dataset = FcsReader.open(Path::new("sample.fcs"))?;
//! let info = dataset.info()?; // headers only
//! let table = &info.tables[0];
//! println!("{} row(s) x {} column(s)", table.row_count, table.columns.len());
//! // The first 100 rows, column-major (`columns[c][r]`).
//! let rows = dataset.read_table(0, 0, 100)?;
//! for (column, values) in table.columns.iter().zip(&rows.columns) {
//!     println!("{}: {:?}", column.name, values.first());
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

pub mod analysis;
mod assurance;
#[doc(hidden)]
pub mod crc;
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod datetime;
#[doc(hidden)]
pub mod events;
#[doc(hidden)]
pub mod file;
pub mod filter;
pub mod gating;
#[doc(hidden)]
pub mod header;
#[doc(hidden)]
pub mod keywords;
#[doc(hidden)]
pub mod layout;
#[doc(hidden)]
pub mod vendor;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use crc::{Crc16, CrcField, read_crc_field};
#[doc(hidden)]
pub use dataset::{FcsDataset, keyword_prefix, spillover};
#[doc(hidden)]
pub use datetime::{FcsDate, acquisition_span, parse_date, parse_time, parse_timestamp};
pub use events::{file_matrix, is_fluorescence_parameter, is_time_parameter, scale_columns};
#[doc(hidden)]
pub use file::{AuxSegment, DataSet, FcsFile, OffsetSource};
#[doc(hidden)]
pub use header::{
    HEADER_LEN, Header, HeaderError, SegmentRange, VERSIONS, looks_like_fcs, parse_header,
};
#[doc(hidden)]
pub use keywords::{Keyword, KeywordSet, TextParse, parse_float, parse_keywords, parse_uint};
#[doc(hidden)]
pub use layout::{
    ByteOrder, DataType, FieldWidth, Mode, Parameter, SpilloverMatrix, decode_fixed,
    decode_free_ascii, parse_spillover, read_parameters, storage_dtype,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "fcs";

/// The FCS reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct FcsReader;

impl FormatReader for FcsReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::FCS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "FCS (Flow Cytometry Standard)".into(),
            vendor: "ISAC open standard (all cytometer vendors)".into(),
            extensions: vec!["fcs".into(), "lmd".into()],
            family: "flow-cytometry".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::FCS.confidence,
            known_gaps: vec![
                "Histogram data sets ($MODE C/U, deprecated in FCS 3.1) are described but not decoded".into(),
                "Integer fields that are not a whole number of bytes (bit-packed $PnB) are not decoded".into(),
                "$BYTEORD permutations other than 1,2,3,4 and 4,3,2,1 are not decoded".into(),
                "read_table/export return raw values; `table --compensate/--transform` and `analyze gate` apply scale conversion, spillover, transforms and gates".into(),
                "FlowJo workspaces: transforms other than linear, log, logicle, biex and ArcSinh, derived parameters and non-OLS spectral unmixing are reported as unsupported".into(),
                "FCS 1.0 and FCS 4.0 files are detected but not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if looks_like_fcs(head) {
            let version = String::from_utf8_lossy(&head[..6]).into_owned();
            let known = VERSIONS.contains(&version.as_str());
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: (!known)
                    .then(|| format!("version identifier {version} is not supported yet")),
            });
        }
        if has_extension(path, &["fcs", "lmd"]) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("FCS extension but no `FCS` version identifier at byte 0".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(FcsDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(FcsDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
