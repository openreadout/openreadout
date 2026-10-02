//! Readers for NMR data.
//!
//! - **Bruker TopSpin / XWIN-NMR experiment directories** (`bruker-nmr`): a directory `<expno>/`
//!   with JCAMP-DX-style parameter files (`acqus`, `acqu2s`, …), the time-domain samples (`fid`
//!   or `ser`) and processed spectra under `pdata/<procno>/`. Notes and vocabulary:
//!   `docs/formats/bruker-nmr.md`; provenance: `docs/provenance/bruker-nmr.md`.
//! - **Varian/Agilent VnmrJ data directories** (`varian-nmr`): a `<name>.fid/` directory with
//!   the binary `fid` (file header, block headers, int16/int32/float32 samples) and the
//!   `procpar` parameter text. Notes and vocabulary: `docs/formats/varian-nmr.md`; provenance:
//!   `docs/provenance/varian-nmr.md`.
//! - **JEOL Delta `.jdf` files** (`jeol-jdf`): a binary header, a parameter section and 1D or 2D
//!   data sections. Notes and vocabulary: `docs/formats/jeol-jdf.md`; provenance:
//!   `docs/provenance/jeol-jdf.md`.
//! - **Magritek Spinsolve benchtop NMR experiment directories** (`magritek-spinsolve`):
//!   `acqu.par`/`proc.par` parameter text and Prospa `.1d`/`.2d` data files (FID first). Notes
//!   and vocabulary: `docs/formats/magritek-spinsolve.md`; provenance:
//!   `docs/provenance/magritek-spinsolve.md`.
//! - **JCAMP-DX** (`jcamp-dx`): the IUPAC text format for spectra (NMR, IR, UV-Vis, MS), with
//!   ASDF-compressed tables, NTUPLES and compound (LINK) files. Notes and vocabulary:
//!   `docs/formats/jcamp-dx.md`; provenance: `docs/provenance/jcamp-dx.md`.
//!
//! All five expose sampled data as core traces (`TraceInfo` / `Dataset::read_trace`).
//!
//! [`export_jcamp`] writes any evenly sampled trace (an NMR FID or spectrum, a JCAMP-DX
//! spectrum, a chromatogram) as JCAMP-DX 5.01: `##XYDATA=(X++(Y..Y))` in DIFDUP (with Y-value
//! checks) or AFFN, NTUPLES pages for complex or multi-sweep data. The file is re-read with the
//! JCAMP-DX reader and verified before it is renamed into place.
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
//! use openreadout_nmr::BrukerReader;
//!
//! let format = BrukerReader.descriptor();
//! assert_eq!(format.id, openreadout_nmr::BRUKER_FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_nmr::BrukerReader;
//!
//! let mut dataset = BrukerReader.open(Path::new("experiment/1"))?;
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
pub mod bruker_dataset;
mod bruker_experiment;
#[doc(hidden)]
pub mod bruker_layout;
#[doc(hidden)]
pub mod bruker_params;
#[doc(hidden)]
pub mod jcamp_asdf;
#[doc(hidden)]
pub mod jcamp_dataset;
#[doc(hidden)]
pub mod jcamp_parse;
#[doc(hidden)]
pub mod jcamp_write;
#[doc(hidden)]
pub mod jeol_dataset;
#[doc(hidden)]
pub mod jeol_header;
#[doc(hidden)]
pub mod spinsolve;
#[doc(hidden)]
pub mod text;
#[doc(hidden)]
pub mod varian_dataset;
#[doc(hidden)]
pub mod varian_layout;
#[doc(hidden)]
pub mod varian_procpar;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use bruker_dataset::{BrukerDataset, group_delay};
#[doc(hidden)]
pub use bruker_layout::{
    ByteOrder, DSP_GROUP_DELAY, Experiment, ProcLayout, Processing, RawLayout, SampleType,
    decode_samples, find_experiments, resolve_experiment_dir,
};
#[doc(hidden)]
pub use bruker_params::{ParamFile, ParamValue, parse_param_file};
#[doc(hidden)]
pub use jcamp_asdf::{AsdfLine, AsdfTable, YToken, decode_asdf, decode_groups, lex_asdf_line};
#[doc(hidden)]
pub use jcamp_dataset::JcampDataset;
#[doc(hidden)]
pub use jcamp_parse::{Block, JcampFile, Ldr, normalize_label, parse_affn, parse_jcamp};
#[doc(hidden)]
pub use jeol_dataset::{JeolDataset, jeol_nucleus};
#[doc(hidden)]
pub use jeol_header::{JdfAxisType, JdfHeader, JdfParam, JdfUnit, JdfValue, parse_jdf_params};
#[doc(hidden)]
pub use spinsolve::{
    ParFile, ProspaHeader, SpinsolveDataset, find_spinsolve_experiments_in, parse_par,
    resolve_spinsolve_dir_in,
};
#[doc(hidden)]
pub use text::decode_text;
#[doc(hidden)]
pub use varian_dataset::VarianDataset;
#[doc(hidden)]
pub use varian_layout::{
    BlockHeader, FidHeader, VarianSampleType, decode_varian, find_varian_experiments,
    read_fid_header, resolve_varian_dir,
};
#[doc(hidden)]
pub use varian_procpar::{Procpar, ProcparParam, ProcparValues, parse_procpar};

pub use jcamp_write::{
    JcampEncoding, JcampExportOptions, JcampExportReport, default_jcamp_output, export_jcamp,
};

/// Format id of Bruker TopSpin experiment directories.
pub const BRUKER_FORMAT_ID: &str = "bruker-nmr";
/// Format id of JCAMP-DX files.
pub const JCAMP_FORMAT_ID: &str = "jcamp-dx";
/// Format id of Varian/Agilent VnmrJ data directories.
pub const VARIAN_FORMAT_ID: &str = "varian-nmr";
/// Format id of JEOL Delta `.jdf` files.
pub const JEOL_FORMAT_ID: &str = "jeol-jdf";
/// Format id of Magritek Spinsolve experiment directories.
pub const SPINSOLVE_FORMAT_ID: &str = "magritek-spinsolve";

/// The Bruker TopSpin reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrukerReader;

impl FormatReader for BrukerReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::BRUKER_NMR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BRUKER_FORMAT_ID.into(),
            name: "Bruker TopSpin NMR".into(),
            vendor: "Bruker".into(),
            extensions: Vec::new(),
            family: "nmr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::BRUKER_NMR.confidence,
            known_gaps: vec![
                "Input is an experiment directory (or its acqus/fid/ser, or a pdata/<n> directory), not a single file".into(),
                "Processed data: 1D, 2D and 3D components are read; 4D+ processing is not decoded".into(),
                "Time-domain values are scaled by 2^NC by analogy with nmrglue's NC_proc rule (inferred); the digital-filter group delay is reported, and removed only when the FID is processed (`analyze nmr-peaks`, `trace --process`; 1-D rows only)".into(),
                "Non-uniformly sampled ser files are returned as acquired rows; nuslist indices are available as a table; NUS reconstruction is out of scope".into(),
                "DTYPA/DTYPP values other than 0 (int32) and 2 (float64) are not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    /// Experiment directories are recognised by their parameter and data files, looked up
    /// through the namespace.
    fn sniff_input(&self, _head: &[u8], input: &Input) -> Option<Detection> {
        let (path, fs) = (input.path(), input.fs());
        if bruker_layout::resolve_experiment_dir_in(fs, path).is_some() {
            return Some(Detection {
                format_id: BRUKER_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if fs.is_dir(path) {
            let exps = bruker_layout::find_experiments_in(fs, path);
            if !exps.is_empty() {
                return Some(Detection {
                    format_id: BRUKER_FORMAT_ID,
                    confidence: DetectConfidence::Likely,
                    note: Some(format!(
                        "directory holds {} Bruker experiment directories; open one of them (e.g. {})",
                        exps.len(),
                        exps[0].display()
                    )),
                });
            }
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(BrukerDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(BrukerDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The JCAMP-DX reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct JcampReader;

impl FormatReader for JcampReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::JCAMP_DX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: JCAMP_FORMAT_ID.into(),
            name: "JCAMP-DX".into(),
            vendor: "IUPAC open standard (NMR, IR, UV-Vis, MS spectra)".into(),
            extensions: vec!["jdx".into(), "dx".into(), "jcamp".into(), "jcm".into()],
            family: "spectroscopy".into(),
            can_read: true,
            can_write: true,
            confidence: assurance::JCAMP_DX.confidence,
            known_gaps: vec![
                "##PEAK ASSIGNMENTS= tables and JCAMP-CS structure blocks are listed, not decoded"
                    .into(),
                "ASDF compression inside (XY..XY) peak tables is not decoded (AFFN only)".into(),
                "##RADATA= (interferograms) is listed, not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if jcamp_parse::looks_like_jcamp(head) {
            return Some(Detection {
                format_id: JCAMP_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if has_extension(path, &["jdx", "dx", "jcamp", "jcm"]) {
            return Some(Detection {
                format_id: JCAMP_FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("JCAMP-DX extension but the file does not start with ##TITLE=".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(JcampDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(JcampDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Varian/Agilent VnmrJ reader (`<name>.fid/` directories with `fid` and `procpar`).
#[derive(Debug, Default, Clone, Copy)]
pub struct VarianReader;

impl FormatReader for VarianReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::VARIAN_NMR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: VARIAN_FORMAT_ID.into(),
            name: "Varian/Agilent VnmrJ NMR".into(),
            vendor: "Agilent (Varian)".into(),
            extensions: Vec::new(),
            family: "nmr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::VARIAN_NMR.confidence,
            known_gaps: vec![
                "Input is a data directory (`<name>.fid/` with fid and procpar), or its fid/procpar/text/log".into(),
                "Sweeps are the fid's traces in disk order: arrayed and multidimensional data are not reordered by phase or array parameters (nmrglue's as_2d order)".into(),
                "Block scale factors and drift corrections are reported, not applied (as nmrglue)".into(),
                "Processed data (datdir/phasefile, datdir/data) are listed, not decoded: no public file or permissive prior art".into(),
                "int16 samples (dp='n') are decoded from the header flags but covered by synthetic tests only".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    /// Data directories are recognised by their `fid` header, read through the namespace.
    fn sniff_input(&self, _head: &[u8], input: &Input) -> Option<Detection> {
        let (path, fs) = (input.path(), input.fs());
        if varian_layout::resolve_varian_dir_in(fs, path).is_some() {
            return Some(Detection {
                format_id: VARIAN_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if fs.is_dir(path) {
            let exps = varian_layout::find_varian_experiments_in(fs, path);
            if !exps.is_empty() {
                return Some(Detection {
                    format_id: VARIAN_FORMAT_ID,
                    confidence: DetectConfidence::Likely,
                    note: Some(format!(
                        "directory holds {} VnmrJ data directories; open one of them (e.g. {})",
                        exps.len(),
                        exps[0].display()
                    )),
                });
            }
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(VarianDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(VarianDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The JEOL Delta `.jdf` reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct JeolReader;

impl FormatReader for JeolReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::JEOL_JDF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: JEOL_FORMAT_ID.into(),
            name: "JEOL Delta NMR".into(),
            vendor: "JEOL".into(),
            extensions: vec!["jdf".into()],
            family: "nmr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::JEOL_JDF.confidence,
            known_gaps: vec![
                "1D and 2D (32-point submatrix) data with real/complex axes are decoded; 3D+ and small-submatrix layouts, TPPI and envelope axes are unsupported (exit 6)".into(),
                "Values are returned as stored; nmrglue returns the complex conjugate".into(),
                "String parameters hold at most 16 characters (longer values are cut in the file)".into(),
                "Acquisition start (ACTUAL_START_TIME, seconds since 1990-01-01 UTC) and header dates are inferred from corpus files".into(),
                "Context, annotation and history sections are not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(b"JEOL.NMR") {
            return Some(Detection {
                format_id: JEOL_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if has_extension(path, &["jdf"]) {
            return Some(Detection {
                format_id: JEOL_FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("JEOL extension but the file does not start with JEOL.NMR".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(JeolDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(JeolDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The Magritek Spinsolve reader (experiment directories with `acqu.par` and Prospa `.1d`/`.2d`
/// data files).
#[derive(Debug, Default, Clone, Copy)]
pub struct SpinsolveReader;

impl FormatReader for SpinsolveReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::MAGRITEK_SPINSOLVE)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: SPINSOLVE_FORMAT_ID.into(),
            name: "Magritek Spinsolve benchtop NMR".into(),
            vendor: "Magritek".into(),
            extensions: vec!["1d".into(), "2d".into()],
            family: "nmr".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::MAGRITEK_SPINSOLVE.confidence,
            known_gaps: vec![
                "Input is an experiment directory (acqu.par and data.1d/data.2d), or any file in it; a folder of experiments lists them".into(),
                "Prospa data types 501 (complex), 503 (x + real) and 504 (x + complex) are decoded; other type codes and multi-row files with an x block are listed, not decoded".into(),
                "Plot files (.pt1/.pt2), MestReNova documents and nmr_fid.dx are listed, not decoded (open nmr_fid.dx with the JCAMP-DX reader)".into(),
                "The x block's unit is not stored: it is named (ms or ppm) only when its spacing matches the dwell time or the spectral width".into(),
                "Sub-experiment folders (e.g. 1D_T1IRT2/0000/) are opened one by one".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    /// Prospa data files by their magic; directories and parameter files through the namespace.
    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let (path, fs) = (input.path(), input.fs());
        if head.starts_with(spinsolve::DATA_MAGIC)
            || spinsolve::resolve_spinsolve_dir_in(fs, path).is_some()
        {
            return Some(Detection {
                format_id: SPINSOLVE_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if fs.is_dir(path) {
            let exps = spinsolve::find_spinsolve_experiments_in(fs, path);
            if !exps.is_empty() {
                return Some(Detection {
                    format_id: SPINSOLVE_FORMAT_ID,
                    confidence: DetectConfidence::Likely,
                    note: Some(format!(
                        "directory holds {} Spinsolve experiments; open one of them (e.g. {})",
                        exps.len(),
                        exps[0].display()
                    )),
                });
            }
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(SpinsolveDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(SpinsolveDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
