//! Clean-room readers for protein-purification (FPLC) chromatography results.
//!
//! - **Cytiva ÄKTA / UNICORN 3-5 result files** (`cytiva-unicorn-res`, `.res`): UV, conductivity,
//!   pH, pressure, temperature, concentration and flow curves, the logbook, fraction and injection
//!   marks, method text.
//! - **UNICORN 6/7 result exports** (`cytiva-unicorn-zip`, the `.zip` UNICORN's *Export result*
//!   writes): the same curves, event lists and UNICORN's own peak tables, with the system,
//!   instrument configuration, column and method.
//!
//! Every curve is a trace sampled at a fixed time interval: channel 0 holds the values, channel 1
//! the retention volume of each sample in ml (`extra.axis` = `{quantity: retention_volume, unit:
//! ml, irregular: true, channel: 1}`), and `sample_rate_hz`/`start_s` give the time after the
//! method start. Event lists and vendor peak tables are tables. Notes:
//! `docs/formats/cytiva-unicorn.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_fplc::UnicornZipReader;
//!
//! let format = UnicornZipReader.descriptor();
//! assert_eq!(format.id, openreadout_fplc::UNICORN_ZIP_FORMAT_ID);
//! println!("{} ({})", format.name, format.family);
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_fplc::UnicornResReader;
//!
//! let mut dataset = UnicornResReader.open(Path::new("run.res"))?;
//! let info = dataset.info()?; // headers only
//! let uv = &info.traces[0];
//! let curve = dataset.read_trace(0, 0, 0, u64::MAX)?;
//! println!("{}: {} samples, last volume {:?} ml", uv.name.as_deref().unwrap_or(""), uv.sample_count, curve.channels[1].last());
//! # Ok::<(), openreadout_core::Error>(())
//! ```
//!
//! # API stability
//!
//! The supported API is what this page documents: the readers, their format ids and
//! [`FplcDataset`] (returned boxed by `open`).
#![forbid(unsafe_code)]
#![warn(missing_docs)]
// member names are lower-cased before their suffix is compared
#![allow(clippy::case_sensitive_file_extension_comparisons)]

mod assurance;
mod dataset;
mod export;
mod model;
mod nrbf;
mod res;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

pub use dataset::FplcDataset;

/// Format id of UNICORN 3-5 result files (`.res`).
pub const UNICORN_RES_FORMAT_ID: &str = res::FORMAT_ID;
/// Format id of UNICORN 6/7 result exports (`.zip`).
pub const UNICORN_ZIP_FORMAT_ID: &str = export::FORMAT_ID;

/// The Cytiva ÄKTA / UNICORN 3-5 result-file reader (`.res`).
#[derive(Debug, Default, Clone, Copy)]
pub struct UnicornResReader;

impl FormatReader for UnicornResReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::CYTIVA_UNICORN_RES)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: UNICORN_RES_FORMAT_ID.into(),
            name: "Cytiva ÄKTA UNICORN result (.res)".into(),
            vendor: "Cytiva (GE Healthcare, Amersham Biosciences)".into(),
            extensions: vec!["res".into()],
            family: "chromatography".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::CYTIVA_UNICORN_RES.confidence,
            known_gaps: vec![
                "Validated on two files (ÄKTAprime 2009, Ettan LC 2017), both with the `UNICORN 3.10` layout text; other UNICORN 3-5 systems (ÄKTA explorer, purifier, FPLC) are expected to share the layout but have no public sample".into(),
                "Times are the sample index × the curve's interval from the method start; a sub-sample start offset some curves carry is read as milliseconds (unvalidated, < 1 sample)".into(),
                "Binary blocks (CONFIG, CALIB, ColumnsChoosen, Template, fraction-collector configuration, the per-run block) are listed by `info --view structure`, not decoded".into(),
                "The run start is derived from the header's end time minus the curves' duration (the logbook's own start text is local time without a zone)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if head.starts_with(&res::MAGIC) {
            return Some(Detection {
                format_id: UNICORN_RES_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        (has_extension(path, &["res"]) && head.len() < 4 && !head.is_empty()).then_some(Detection {
            format_id: UNICORN_RES_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some(".res extension but the file is too short to be a UNICORN result".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let file = input.open()?;
        let len = file.size().map_err(|e| Error::io(input.path(), e))?;
        let parsed = res::parse(&file, input.path(), len)?;
        Ok(Box::new(FplcDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            len,
            dataset::Backing::Res { file, len },
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// The UNICORN 6/7 result-export reader (`.zip`).
#[derive(Debug, Default, Clone, Copy)]
pub struct UnicornZipReader;

/// Member names of a zip's local headers inside `head` (as far as the head reaches).
fn local_names(head: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(h) = head.get(at..at + 30) {
        if h[..4] != *b"PK\x03\x04" {
            break;
        }
        let csize = u32::from_le_bytes([h[18], h[19], h[20], h[21]]) as usize;
        let n = usize::from(u16::from_le_bytes([h[26], h[27]]));
        let e = usize::from(u16::from_le_bytes([h[28], h[29]]));
        let Some(name) = head.get(at + 30..at + 30 + n) else {
            break;
        };
        out.push(String::from_utf8_lossy(name).into_owned());
        // ZIP64 or streamed members hide their size: stop at the first such member
        if csize == 0xFFFF_FFFF || (csize == 0 && h[6] & 8 != 0) {
            break;
        }
        at = at + 30 + n + e + csize;
    }
    out
}

impl FormatReader for UnicornZipReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::CYTIVA_UNICORN_ZIP)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: UNICORN_ZIP_FORMAT_ID.into(),
            name: "Cytiva ÄKTA UNICORN 6/7 result export (.zip)".into(),
            vendor: "Cytiva".into(),
            extensions: vec!["zip".into()],
            family: "chromatography".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::CYTIVA_UNICORN_ZIP.confidence,
            known_gaps: vec![
                "Validated on UNICORN 7.1, 7.3 and 7.7 exports (ÄKTA pure 25, pure 150L, avant); UNICORN 6 exports have no public sample".into(),
                "Curves are read at full resolution; reduced-resolution point files, if an export has them, are not used".into(),
                "Evaluated curves on a volume grid (IsoChroneType Volume) have volumes but no times".into(),
                "Pool tables, method phases and the audit trail are kept in the vendor tree (`info --view full`), not as tables".into(),
                "A UNICORN database backup (`.bak`) or a `.result` file from the UNICORN database is not an export and is not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !openreadout_core::zip::is_zip(head) {
            return None;
        }
        let names = local_names(head);
        let unicorn = names.iter().any(|n| {
            let l = export::logical(n);
            l == "result.xml"
                || (l.starts_with("chrom.") && (l.ends_with(".xml") || l.ends_with("_true")))
                || l == "calibrationsettingdata"
                || l == "systemsettingdata"
        });
        unicorn.then_some(Detection {
            format_id: UNICORN_ZIP_FORMAT_ID,
            confidence: if has_extension(path, &["zip"]) {
                DetectConfidence::Likely
            } else {
                DetectConfidence::ExtensionOnly
            },
            note: None,
        })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        if !openreadout_core::zip::is_zip(head) {
            return None;
        }
        // look at the whole member list: the export's first members vary
        let z =
            openreadout_core::zip::ZipIndex::open(input.fs(), input.path(), UNICORN_ZIP_FORMAT_ID)
                .ok()?;
        export::is_unicorn_export(z.members.iter().map(|m| m.name.as_str())).then_some(Detection {
            format_id: UNICORN_ZIP_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let ex = export::Export::open(input)?;
        if !export::is_unicorn_export(ex.zip.members.iter().map(|m| m.name.as_str())) {
            return Err(Error::unsupported(
                UNICORN_ZIP_FORMAT_ID,
                "a zip without Result.xml and a Chrom.<n>.Xml",
                "This zip is not a UNICORN result export. In UNICORN 6/7 use Evaluation > File > Export > Result to write one.",
            ));
        }
        let parsed = export::parse(&ex)?;
        let len = ex.zip.file_len;
        Ok(Box::new(FplcDataset::new(
            self.descriptor(),
            input.path().to_path_buf(),
            len,
            dataset::Backing::Zip(Box::new(ex)),
            parsed,
        )))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Provenance for the experiment fields every reader of this crate sets from the format itself
/// (vendor, instrument kind, technique) and for each measurement, where the builder did not
/// record one.
pub(crate) fn complete_provenance(e: &mut openreadout_core::experiment::Experiment) {
    use openreadout_core::experiment::Origin;
    use openreadout_core::provenance::Source;
    let o = |from: &str| Origin {
        source: Source::Inferred,
        from: from.into(),
    };
    for k in 0..e.measurements.len() {
        e.provenance
            .entry(format!("measurements[{k}]"))
            .or_insert_with(|| o("the reader's data layout (traces, tables, images)"));
    }
    if let Some(i) = &e.instrument {
        if i.vendor.is_some() {
            e.provenance
                .entry("instrument.vendor".into())
                .or_insert_with(|| o("the file format"));
        }
        if i.kind.is_some() {
            e.provenance
                .entry("instrument.kind".into())
                .or_insert_with(|| o("the file format"));
        }
    }
    if e.method.as_ref().is_some_and(|m| m.technique.is_some()) {
        e.provenance
            .entry("method.technique".into())
            .or_insert_with(|| o("the file format and the recorded mode"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs() {
        let mut head = res::MAGIC.to_vec();
        head.extend_from_slice(&[0u8; 60]);
        assert_eq!(
            UnicornResReader
                .sniff(&head, Path::new("x.res"))
                .unwrap()
                .confidence,
            DetectConfidence::Definite
        );
        assert!(
            UnicornResReader
                .sniff(b"PK\x03\x04", Path::new("x.zip"))
                .is_none()
        );
        let z = openreadout_core::zip::zip_bytes(&[
            ("Result.xml", b"<Result/>"),
            ("Chrom.1.Xml", b"<Chromatogram/>"),
        ])
        .unwrap();
        assert!(UnicornZipReader.sniff(&z, Path::new("run.zip")).is_some());
        let other = openreadout_core::zip::zip_bytes(&[("rdml_data.xml", b"<rdml/>")]).unwrap();
        assert!(
            UnicornZipReader
                .sniff(&other, Path::new("run.zip"))
                .is_none()
        );
    }
}
