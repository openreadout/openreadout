//! Readers for the open mass-spectrometry exchange formats mzML 1.1 (HUPO-PSI) and
//! mzXML 2.x/3.x (ISB).
//!
//! Both are XML with base64 binary arrays; files run to many gigabytes, so nothing is loaded
//! whole: the trailing offset index (`indexedmzML`, mzXML `index`) is read and spot-checked at
//! open, and each spectrum is parsed from its byte offset on demand. Without a usable index
//! the file is scanned once to locate every spectrum. Layout and vocabulary:
//! `docs/formats/mzml.md`. Provenance: `docs/provenance/mzml.md`.
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
//! use openreadout_mzml::MzmlReader;
//!
//! let format = MzmlReader.descriptor();
//! assert_eq!(format.id, openreadout_mzml::MZML_FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_mzml::MzmlReader;
//!
//! let mut dataset = MzmlReader.open(Path::new("run.mzML"))?;
//! let info = dataset.info()?; // headers only
//! let run = &info.spectra[0];
//! println!("{} spectra, MS levels {:?}", run.scan_count, run.ms_levels);
//! // Spectrum 0 of run 0: parallel m/z and intensity arrays.
//! let spectrum = dataset.read_spectrum(0, 0)?;
//! println!("MS{} at {} s: {} points", spectrum.ms_level, spectrum.rt_s, spectrum.mz.len());
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
pub mod binary;
mod cv;
mod experiment;
#[doc(hidden)]
pub mod gz;
#[doc(hidden)]
pub mod mzml;
#[doc(hidden)]
pub mod mzmlb;
#[doc(hidden)]
pub mod mzxml;
#[doc(hidden)]
pub mod numpress;
mod scan;
mod xml;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;

#[doc(hidden)]
pub use binary::{
    ArrayEncoding, ArrayError, Compression, Outer, ValueType, decode_array, decode_base64,
    decode_values,
};
#[doc(hidden)]
pub use mzml::MzmlDataset;
#[doc(hidden)]
pub use mzxml::{MzxmlDataset, parse_duration};
#[doc(hidden)]
pub use numpress::{NumpressError, decode_linear, decode_pic, decode_slof};

/// Format id of mzML on the command line and in JSON.
pub const MZML_FORMAT_ID: &str = mzml::FMT;
/// Format id of imzML on the command line and in JSON.
pub const IMZML_FORMAT_ID: &str = "imzml";
/// Format id of mzXML on the command line and in JSON.
pub const MZXML_FORMAT_ID: &str = mzxml::FMT;

/// Does the head of the file look like the XML root `root` (after the prolog)?
fn root_is(head: &[u8], roots: &[&str]) -> bool {
    let text = String::from_utf8_lossy(&head[..head.len().min(8192)]);
    let mut rest = text.trim_start_matches('\u{feff}');
    // skip the XML declaration, processing instructions and comments
    loop {
        rest = rest.trim_start();
        if let Some(r) = rest.strip_prefix("<?") {
            match r.find("?>") {
                Some(i) => rest = &r[i + 2..],
                None => return false,
            }
        } else if let Some(r) = rest.strip_prefix("<!--") {
            match r.find("-->") {
                Some(i) => rest = &r[i + 3..],
                None => return false,
            }
        } else {
            break;
        }
    }
    let Some(tag) = rest.strip_prefix('<') else {
        return false;
    };
    let name: String = tag
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
        .collect();
    let local = name.rsplit(':').next().unwrap_or(&name);
    roots.contains(&local)
}

/// Detection of a gzip-compressed file from its decompressed head `inner`: definite when the
/// document root matches, extension-only when only the name (`.<ext>.gz`) does.
fn sniff_gz(
    inner: &[u8],
    path: &Path,
    roots: &[&str],
    ext: &str,
    format_id: &'static str,
) -> Option<Detection> {
    if root_is(inner, roots) {
        return Some(Detection {
            format_id,
            confidence: DetectConfidence::Definite,
            note: Some("gzip-compressed".into()),
        });
    }
    if gz::has_gz_extension(path, ext) {
        return Some(Detection {
            format_id,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some(format!(
                "gzip-compressed .{ext}.gz, but the decompressed start is not the expected root element"
            )),
        });
    }
    None
}

/// The mzML reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct MzmlReader;

impl FormatReader for MzmlReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::MZML)
    }

    fn descriptor(&self) -> FormatDescriptor {
        mzml::descriptor()
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if has_extension(path, &["imzml"]) {
            return None; // the imzML reader's
        }
        if let Some(inner) = gz::peek(head) {
            return sniff_gz(
                &inner,
                path,
                &["mzML", "indexedmzML"],
                "mzml",
                MZML_FORMAT_ID,
            );
        }
        if root_is(head, &["mzML", "indexedmzML"]) {
            return Some(Detection {
                format_id: MZML_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if has_extension(path, &["mzml"]) && !head.is_empty() {
            return Some(Detection {
                format_id: MZML_FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some(
                    "mzML extension but the root element is not <mzML>/<indexedmzML>".into(),
                ),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        if gz::input_is_gzip(input) {
            return gz::open_gz(input, MZML_FORMAT_ID, |fs, p| {
                Ok(Box::new(MzmlDataset::open_in(fs, p)?))
            });
        }
        Ok(Box::new(MzmlDataset::open_in(input.fs(), input.path())?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The imzML reader (mass spectrometry imaging: mzML metadata, arrays in a `.ibd` file).
#[derive(Debug, Default, Clone, Copy)]
pub struct ImzmlReader;

impl FormatReader for ImzmlReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::IMZML)
    }

    fn descriptor(&self) -> FormatDescriptor {
        mzml::imzml_descriptor()
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !has_extension(path, &["imzml"]) || head.is_empty() {
            return None;
        }
        Some(Detection {
            format_id: IMZML_FORMAT_ID,
            confidence: if root_is(head, &["mzML", "indexedmzML"]) {
                DetectConfidence::Definite
            } else {
                DetectConfidence::ExtensionOnly
            },
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(MzmlDataset::open_imzml_in(
            input.fs(),
            input.path(),
        )?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The mzXML reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct MzxmlReader;

impl FormatReader for MzxmlReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::MZXML)
    }

    fn descriptor(&self) -> FormatDescriptor {
        mzxml::descriptor()
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if let Some(inner) = gz::peek(head) {
            return sniff_gz(&inner, path, &["mzXML"], "mzxml", MZXML_FORMAT_ID);
        }
        if root_is(head, &["mzXML"]) {
            return Some(Detection {
                format_id: MZXML_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if has_extension(path, &["mzxml"]) && !head.is_empty() {
            return Some(Detection {
                format_id: MZXML_FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("mzXML extension but the root element is not <mzXML>".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        if gz::input_is_gzip(input) {
            return gz::open_gz(input, MZXML_FORMAT_ID, |fs, p| {
                Ok(Box::new(MzxmlDataset::open_in(fs, p)?))
            });
        }
        Ok(Box::new(MzxmlDataset::open_in(input.fs(), input.path())?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Format id of mzMLb on the command line and in JSON.
pub const MZMLB_FORMAT_ID: &str = mzmlb::FMT;

/// The mzMLb reader (mzML with its arrays in HDF5 datasets).
#[derive(Debug, Default, Clone, Copy)]
pub struct MzmlbReader;

impl FormatReader for MzmlbReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::MZMLB)
    }

    fn descriptor(&self) -> FormatDescriptor {
        mzmlb::descriptor()
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let named = mzmlb::sniff(head, path)?;
        Some(Detection {
            format_id: MZMLB_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: (!named).then(|| {
                "HDF5 file named .mzMLb; mzMLb dataset names not seen in the first 64 KiB".into()
            }),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let size_bytes = input.metadata()?.len();
        let (ds, container) = mzmlb::open(input)?;
        Ok(Box::new(gz::ContainerDataset {
            inner: Box::new(ds),
            path: input.path().to_path_buf(),
            size_bytes,
            format: Some(mzmlb::descriptor()),
            key: "container",
            facts: container.facts,
            notes: vec![
                "mzMLb: the mzML document is the HDF5 dataset `mzML`; its binary arrays are slices of HDF5 datasets (byte offsets in this output refer to the `mzML` dataset)".into(),
            ],
            performed: vec![
                "opened the HDF5 container and read the mzML document and the index from its datasets".into(),
            ],
            findings: Vec::new(),
        }))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::root_is;

    #[test]
    fn sniff_roots() {
        assert!(root_is(
            b"<?xml version=\"1.0\"?>\n<indexedmzML xmlns=\"x\">",
            &["mzML", "indexedmzML"]
        ));
        assert!(root_is(b"\xef\xbb\xbf<mzML>", &["mzML"]));
        assert!(root_is(b"<!-- c --><mzXML a='1'>", &["mzXML"]));
        assert!(!root_is(b"<html>", &["mzML"]));
        assert!(!root_is(b"", &["mzML"]));
    }
}
