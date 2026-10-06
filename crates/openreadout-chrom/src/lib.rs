//! Readers for chromatography data.
//!
//! - Agilent ChemStation `.D` directories and their `.ch`/`.uv`/`.ms` files (`chemstation`):
//!   `docs/formats/chemstation.md`.
//! - Agilent OpenLab CDS injections (`.dx` containers with their `.rx` result packages,
//!   `openlab-cds`): `docs/formats/openlab-cds.md`.
//! - AIA/ANDI netCDF (`.cdf`), chromatography and mass-spectrometry templates (`andi-chrom`):
//!   `docs/formats/andi-chrom.md`; includes a netCDF-3 classic reader.
//! - Shimadzu LabSolutions `.lcd`/`.gcd` (`shimadzu`): `docs/formats/shimadzu.md`; includes a
//!   compound-file (OLE2) reader.
//! - Thermo Scientific Chromeleon 7 archives (`.cmbx`, `chromeleon`): `docs/formats/chromeleon.md`.
//! - Waters Empower ASCII raw-data exports (`.arw`, `empower-arw`): `docs/formats/empower-arw.md`.
//!
//! Provenance of every layout: `docs/provenance/<format>.md`.
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
//! use openreadout_chrom::ChemStationReader;
//!
//! let format = ChemStationReader.descriptor();
//! assert_eq!(format.id, openreadout_chrom::CHEMSTATION_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::FormatReader;
//! use openreadout_chrom::ChemStationReader;
//!
//! let mut dataset = ChemStationReader.open(Path::new("sample.D"))?;
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

#[doc(hidden)]
pub mod andi_dataset;
mod assurance;
mod binary;
#[doc(hidden)]
pub mod chemstation_dataset;
#[doc(hidden)]
pub mod chemstation_decode;
#[doc(hidden)]
pub mod chemstation_header;
mod chemstation_method;
#[doc(hidden)]
pub mod chemstation_report;
#[doc(hidden)]
pub mod chromeleon;
#[doc(hidden)]
pub mod chromeleon_dataset;
#[doc(hidden)]
pub mod chromeleon_results;
#[doc(hidden)]
pub mod empower_arw;
#[doc(hidden)]
pub mod netcdf;
#[doc(hidden)]
pub mod openlab_dataset;
#[doc(hidden)]
pub mod openlab_xml;
#[doc(hidden)]
pub mod shimadzu_channels;
#[doc(hidden)]
pub mod shimadzu_dataset;
#[doc(hidden)]
pub mod shimadzu_peaks;
#[doc(hidden)]
pub mod shimadzu_signal;
#[doc(hidden)]
pub mod shimadzu_tlm;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::zip;

#[doc(hidden)]
pub use andi_dataset::{AndiDataset, AndiTemplate, andi_template, andi_timestamp};
#[doc(hidden)]
pub use chemstation_dataset::{ChemStationDataset, DirFile, SignalFile, is_chemstation_dir};
#[doc(hidden)]
pub use chemstation_decode::{
    BodyEnd, DecodedSignal, MS_RECORD_HEADER, MS_RECORD_TRAILER, ScanRecord, UV_RECORD_HEADER,
    decode_delta_records, decode_float64, decode_second_difference, ms_intensity, ms_pairs,
    ms_record, uv_record, uv_values,
};
#[doc(hidden)]
pub use chemstation_header::{
    BodyEncoding, DECODED_VERSIONS, SignalHeader, SignalKind, looks_like_chemstation, parse_date,
    version_of,
};
#[doc(hidden)]
pub use chromeleon_dataset::ChromeleonDataset;
#[doc(hidden)]
pub use netcdf::{
    AttrValue, Attribute, Dimension, HeaderError, NcType, NetCdf, Variable, read_header,
};
#[doc(hidden)]
pub use openlab_dataset::{OpenLabDataset, SignalMapping, decode_pairs};
#[doc(hidden)]
pub use openreadout_core::cfb::{CFB_MAGIC, Cfb, CfbEntry};
#[doc(hidden)]
pub use shimadzu_dataset::{
    ShimadzuChannel, ShimadzuDataset, ShimadzuSignal, ShimadzuSignalKind, property_fields,
    text_runs,
};

/// Format id of the Agilent ChemStation reader.
pub const CHEMSTATION_ID: &str = "chemstation";

pub(crate) fn has_ext(path: &Path, exts: &[&str]) -> bool {
    has_extension(path, exts)
}

/// Agilent ChemStation `.D` directories and `.ch`/`.uv`/`.ms` signal files.
#[derive(Debug, Default, Clone, Copy)]
pub struct ChemStationReader;

impl FormatReader for ChemStationReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::CHEMSTATION)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: CHEMSTATION_ID.into(),
            name: "Agilent ChemStation data directory (.D: .ch, .uv, .ms)".into(),
            vendor: "Agilent".into(),
            extensions: vec!["d".into(), "ch".into(), "uv".into(), "ms".into()],
            family: "chromatography".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::CHEMSTATION.confidence,
            known_gaps: vec![
                "Signal-file versions other than 2 (.ms), 30, 81, 130, 131 (.uv), 179 and 181 are listed but not decoded".into(),
                "Signal files are read whole; .ch files above 512 MiB are refused (exit 6)".into(),
                ".reg register files, RUN.LOG and method (.M) files are listed with sizes, not decoded".into(),
                "Vendor peak reports are tables[0] vendor_peaks: Result.xml (limits, baselines, named compounds and amounts), else Report.TXT (the printed report), and RESULTS.CSV (MSD ChemStation TIC integration); check finds every reported peak on our decoded signal, Result.xml areas recomputed between the vendor's limits (median ratio 0.9999 on 66 peaks) (2 + 4 GC runs, 144 peaks; 12 MSD runs, 46 peaks). Binary reports (REPORT.REG, .rpt) and calibration curves/levels are not read".into(),
                "MS scans are reported as stored (thresholded m/z–intensity pairs at 0.05 m/z resolution), MS level 1, polarity unknown".into(),
                "Agilent MassHunter (.d with AcqData/) and OpenLab CDS (.dx) are different formats: agilent-masshunter and openlab-cds read them".into(),
                ".ch headers hold no point count: a version 81/179/181 body cut exactly at a value boundary cannot be told from a complete one (check reports the interval the header time range implies)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    /// A `.D` directory is recognised by its signal files, read through the namespace.
    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if input.is_dir() {
            return chemstation_dataset::is_chemstation_dir_in(input.fs(), path).then_some(
                Detection {
                    format_id: CHEMSTATION_ID,
                    confidence: DetectConfidence::Definite,
                    note: None,
                },
            );
        }
        if let Some(v) = looks_like_chemstation(head) {
            let known = DECODED_VERSIONS.contains(&v.as_str());
            return Some(Detection {
                format_id: CHEMSTATION_ID,
                confidence: DetectConfidence::Definite,
                note: (!known)
                    .then(|| format!("ChemStation file version {v} is listed but not decoded")),
            });
        }
        if has_extension(path, &["ch", "uv"]) {
            return Some(Detection {
                format_id: CHEMSTATION_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("ChemStation extension but no version string at byte 0".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ChemStationDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ChemStationDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Format id of the Agilent OpenLab CDS reader.
pub const OPENLAB_ID: &str = "openlab-cds";

/// The first member name of a zip head, lower case.
fn first_member(head: &[u8]) -> Option<String> {
    Some(zip::first_member_name(head)?.to_ascii_lowercase())
}

/// Does `head` start like an OpenLab CDS container: a zip whose first member is the manifest,
/// the results document or a GUID-named signal part? (A first `[Content_Types].xml` or
/// `_rels/.rels` is any Open Packaging Conventions file, such as `.xlsx`: it counts only with a
/// `.dx`/`.rx` extension, in [`OpenLabReader`]'s sniff.)
pub fn looks_like_openlab(head: &[u8]) -> bool {
    let Some(lower) = first_member(head) else {
        return false;
    };
    if matches!(lower.as_str(), "injection.acmd" | "base/injectionacaml") {
        return true;
    }
    // `<guid>.CH` / `<guid>.IT` / `<guid>.UV`
    let (stem, ext) = lower.rsplit_once('.').unwrap_or((&lower, ""));
    stem.len() == 36
        && stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && stem.matches('-').count() == 4
        && matches!(ext, "ch" | "it" | "uv" | "uvd")
}

/// Agilent OpenLab CDS injections: `.dx` containers (with the `.rx` result package of the same
/// name and the result set's `.acaml` sequence file when present) and `.rx` packages.
#[derive(Debug, Default, Clone, Copy)]
pub struct OpenLabReader;

impl FormatReader for OpenLabReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::OPENLAB_CDS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: OPENLAB_ID.into(),
            name: "Agilent OpenLab CDS injection (.dx, with its .rx results)".into(),
            vendor: "Agilent".into(),
            extensions: vec!["dx".into(), "rx".into()],
            family: "chromatography".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::OPENLAB_CDS.confidence,
            known_gaps: vec![
                "Detector signals (.CH, Signal179), instrument curves (.IT: pressure, flow, solvent ratios, temperatures) and DAD spectra parts (.UV, Spectra131: one trace, one channel per wavelength; bit-identical to rainbow-api, unit checked against the DAD's own channels) are traces; spectra directories (.UVD) and parts of other content types are listed, not decoded".into(),
                "Vendor peaks (.rx) are a table with the vendor's retention times, areas, heights, area %, limits and baseline codes; compound amounts, calibration and custom fields (e.g. GPC results) are not read, and every corpus file has unnamed compounds".into(),
                "Without the result set's .acaml sequence file, vendor peaks are matched to a signal by their baseline values (the vendor's baseline at a BB peak limit is the signal sample there); tables[0].extra.signal_mapping says how".into(),
                "A result-set folder (.rslt) is not one data set: open its .dx files (openreadout batch 'set.rslt/*.dx')".into(),
                "Validated on GC-FID (54 injections, OpenLab CDS 2.6 and a YADG GC injection), one LC-DAD/FLD result set whose detector signals were cut to 4 values by its publisher and two LC-DAD injections with spectra; no LC-MS .dx in the corpus".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if input.is_dir() {
            // A result-set folder is claimed only by its extension, so that opening it gives
            // the hint to open its injections while recursive walks still enter it and take
            // each `.dx` as its own data set.
            let dx = openlab_dataset::injections_in(input.fs(), path);
            return (has_extension(path, &["rslt"]) && !dx.is_empty()).then_some(Detection {
                format_id: OPENLAB_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some("an OpenLab CDS result-set folder: open one of its .dx files".into()),
            });
        }
        if !zip::is_zip(head) {
            return None;
        }
        let ours = has_extension(path, &["dx", "rx"]);
        let packaging = first_member(head)
            .is_some_and(|n| matches!(n.as_str(), "[content_types].xml" | "_rels/.rels"));
        if looks_like_openlab(head) || (ours && packaging) {
            return Some(Detection {
                format_id: OPENLAB_ID,
                confidence: if ours {
                    DetectConfidence::Definite
                } else {
                    DetectConfidence::Likely
                },
                note: None,
            });
        }
        ours.then(|| Detection {
            format_id: OPENLAB_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some(
                "a zip archive with an OpenLab CDS extension but an unexpected first member".into(),
            ),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(OpenLabDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(OpenLabDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Format id of the Shimadzu LabSolutions reader.
pub const SHIMADZU_ID: &str = "shimadzu";

/// Shimadzu LabSolutions `.lcd` (LC) and `.gcd` (GC) data files.
#[derive(Debug, Default, Clone, Copy)]
pub struct ShimadzuReader;

impl FormatReader for ShimadzuReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::SHIMADZU)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: SHIMADZU_ID.into(),
            name: "Shimadzu LabSolutions data file (.lcd, .gcd)".into(),
            vendor: "Shimadzu".into(),
            extensions: vec!["lcd".into(), "gcd".into()],
            family: "chromatography".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::SHIMADZU.confidence,
            known_gaps: vec![
                "Read: chromatograms (older layout in mV; newer layout and .gcd in the unit of their Chromatogram Status record, e.g. µV), status traces (pump pressures, oven/cooler temperatures in the record's unit), the PDA field (stored µAU, shown in mAU) and LabSolutions' own peak tables (tables[0] vendor_peaks). Validated point for point against LabSolutions ASCII exports of an older-layout (LCsolution 1.25) and a newer-layout (5.109) file (25,038 trace points, 8 peaks in every column), bit-exact against chromConverter on 2 files, and against the vendor's own peak areas on 4 files (LC-UV, PDA, GC)".into(),
                "Newer-layout LC chromatograms (LSS Raw Data/Chromatogram ChN) are named by their 2D Data Item title (DT 52 or 48) and scaled by their status record; the unit rule is the one validated on older-layout exports, a 5.109 export and a GC file's vendor peak areas, but no export exists of a file-version-5.01 (DT 48) chromatogram".into(),
                "PDA channel chromatograms (a wavelength and bandwidth) are not extracted: their vendor peak tables are read and labelled with the channel's wavelength (channel 0 has none recorded); newer-layout peak records are read in their first 64 bytes only (no k', plates, tailing, resolution)".into(),
                "Older-layout status traces are named by module model and quantity (LC-20AD pressure 1): the file stores no names; LabSolutions' export names (Pump A Pressure, Oven Temp.) are its own".into(),
                "LC-MS (TLM Raw Data): MRM and SIM records are spectra (run 0), equal to msconvert's on 610 spectra of 3 CC0 files; full-scan and product-ion-scan spectra are refused (exit 6: the vendor's reader returns other values), their retention time, TIC and precursor listed; polarity unknown. MS quantitative results, compound and calibration tables and method streams are listed, not decoded".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !has_extension(path, &["lcd", "gcd"]) {
            return None;
        }
        Some(Detection {
            format_id: SHIMADZU_ID,
            confidence: if head.starts_with(&CFB_MAGIC) {
                DetectConfidence::Definite
            } else {
                DetectConfidence::ExtensionOnly
            },
            note: (!head.starts_with(&CFB_MAGIC))
                .then(|| "LabSolutions extension but no compound-file signature".into()),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ShimadzuDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ShimadzuDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Format id of the AIA/ANDI netCDF reader.
pub const ANDI_ID: &str = "andi-chrom";

/// Does `head` look like an ANDI netCDF file (netCDF magic plus a template attribute or
/// variable name in the header bytes)?
pub fn looks_like_andi(head: &[u8]) -> bool {
    if head.len() < 4 || &head[..3] != b"CDF" || !matches!(head[3], 1 | 2) {
        return false;
    }
    let has = |needle: &[u8]| head.windows(needle.len()).any(|w| w == needle);
    has(b"aia_template_revision")
        || has(b"ms_template_revision")
        || has(b"ordinate_values")
        || has(b"mass_values")
}

/// AIA/ANDI netCDF chromatography and mass-spectrometry files.
#[derive(Debug, Default, Clone, Copy)]
pub struct AndiReader;

impl FormatReader for AndiReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::ANDI_CHROM)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: ANDI_ID.into(),
            name: "AIA/ANDI netCDF (chromatography and mass spectrometry)".into(),
            vendor: "ASTM E1947 / E2077 open standard (exported by most chromatography data systems)".into(),
            extensions: vec!["cdf".into(), "nc".into()],
            family: "chromatography".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::ANDI_CHROM.confidence,
            known_gaps: vec![
                "netCDF CDF-5 (64-bit data) files are recognised but not read (ANDI files are CDF-1)".into(),
                "Non-uniform chromatography sampling (raw_data_retention) is reported, but traces assume the regular actual_sampling_interval grid".into(),
                "ANDI/MS: MS level is reported as 1 (the template has no MS level); library/SIM extensions (group variables) are listed, not mapped".into(),
                "ANDI/MS: the total-ion chromatogram is a trace only when scan times are evenly spaced (within 1 %)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if looks_like_andi(head) {
            return Some(Detection {
                format_id: ANDI_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if head.len() >= 4 && &head[..3] == b"CDF" && has_extension(path, &["cdf"]) {
            return Some(Detection {
                format_id: ANDI_ID,
                confidence: DetectConfidence::Likely,
                note: Some("netCDF file with a .cdf extension but no ANDI template marker in its first bytes".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(AndiDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(AndiDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Format id of the Chromeleon archive reader.
pub const CHROMELEON_ID: &str = "chromeleon";

/// Thermo Scientific Chromeleon 7 archives (`.cmbx`): a sequence's injections and their 2D
/// signals.
#[derive(Debug, Default, Clone, Copy)]
pub struct ChromeleonReader;

impl FormatReader for ChromeleonReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::CHROMELEON)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: CHROMELEON_ID.into(),
            name: "Thermo Scientific Chromeleon 7 archive (.cmbx)".into(),
            vendor: "Thermo Fisher Scientific".into(),
            extensions: vec!["cmbx".into()],
            family: "chromatography".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::CHROMELEON.confidence,
            known_gaps: vec![
                "Read: every archived 2D signal (detector channels, PDA-extracted channels, MS extracted-ion chromatograms, pump pressure and flow; encodings PtsLDiff, PtsLL2Df, PtsDDCmp) and every 3D field (PDA spectra: one trace, one channel per wavelength) of every injection, and the calibration standards a processing method holds, as traces with their unit, device and recorded range from the sequence file. Validated point for point against Chromeleon's own ASCII exports (6 HPAEC-PAD injections, 55,446 points, Chromeleon 7.3.1) and a Chromeleon 7.2.10 PDF report (57 GC-FID injections); every signal and 3D field of 10 archives equals the sequence file's recorded point count, times, minimum and maximum; Chromeleon's extracted channels equal our 3D fields bit for bit".into(),
                "Chromeleon's own integration results, as it last saved them, are tables[0] vendor_peaks (retention time, limits, baseline, area, height, component, 50/10/5 % points; widths, asymmetry, tailing, plates and resolution computed from those points): equal to the PDF report's printed values, and every one of 5,360 stored peaks of 9 archives reproduced by our integration on our decoded signal within 1e-6. Injection details (position, volume, inject time, type, status, level, methods) are tables[1] injections and traces[].extra, equal to the vendor's exports and report. Amounts are not stored with the peaks and are not computed; injections Chromeleon saved no results for have none".into(),
                "Audit trails, instrument methods (the script: stages, times, property settings such as gradients and oven programs) and processing-method settings are decompressed (LZMA2) and offered as attachments and in dump; report definitions, layouts and custom raw items are listed only".into(),
                "Mass-spectrometry data (MSRawItem) is an embedded Thermo .raw file: listed with a hint to extract it and open it as thermo-raw; MS channels Chromeleon derives on demand have no stored points and are not traces. Unknown section tags, block layouts or result values never seen are refused (exit 6)".into(),
                "Chromeleon 6 backups (.cmb) are a different format and are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !zip::is_zip(head) {
            return None;
        }
        let ours = has_extension(path, &["cmbx"]);
        let seq = first_member(head).is_some_and(|n| chromeleon::first_member_is_sequence(&n));
        if !(seq || ours) {
            return None;
        }
        Some(Detection {
            format_id: CHROMELEON_ID,
            confidence: match (seq, ours) {
                (true, true) => DetectConfidence::Definite,
                (true, false) => DetectConfidence::Likely,
                _ => DetectConfidence::ExtensionOnly,
            },
            note: (!seq).then(|| {
                "a zip archive with the .cmbx extension but no sequence file as its first member"
                    .into()
            }),
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ChromeleonDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ChromeleonDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Format id of the Waters Empower ASCII export reader.
pub const EMPOWER_ARW_ID: &str = "empower-arw";

/// Waters Empower ASCII raw-data exports (`.arw`): one chromatogram with the header fields the
/// export method chose.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmpowerArwReader;

impl FormatReader for EmpowerArwReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::EMPOWER_ARW)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: EMPOWER_ARW_ID.into(),
            name: "Waters Empower ASCII raw-data export (.arw)".into(),
            vendor: "Waters".into(),
            extensions: vec!["arw".into()],
            family: "chromatography".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::EMPOWER_ARW.confidence,
            known_gaps: vec![
                "Read: 2D exports (a row of quoted field names, a row of their values, then time/value rows) as one trace with every exported field; equal to Appia's reading of the same files. Empower keeps its native raw data in its database: exports (this ASCII export and AIA/netCDF, read by andi-chrom) are how data leaves it".into(),
                "Multi-column (3D PDA) exports are refused (exit 6): no public example with a licence; export PDA data as AIA/netCDF or as single-wavelength channels".into(),
                "The export does not state the value's unit, and times are assumed to be minutes (Empower's export unit)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let text = empower_arw::looks_like_arw(head);
        let ext = has_extension(path, &["arw"]);
        if !text {
            // an export without its header rows: rows of two numbers, named `.arw`
            return (ext && empower_arw::looks_like_headerless_arw(head)).then_some(Detection {
                format_id: EMPOWER_ARW_ID,
                confidence: DetectConfidence::Likely,
                note: Some(
                    "rows of two numbers and no header: an Empower export whose method selected no fields"
                        .into(),
                ),
            });
        }
        Some(Detection {
            format_id: EMPOWER_ARW_ID,
            confidence: if ext {
                DetectConfidence::Definite
            } else {
                DetectConfidence::Likely
            },
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(empower_arw::EmpowerArwDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(empower_arw::EmpowerArwDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
