//! Clean-room readers for high-content screening (HCS) plates.
//!
//! Three plate-imager families, each an index file plus one TIFF per plane:
//!
//! - **Revvity/PerkinElmer Harmony** exports of the Opera Phenix, Operetta and Operetta CLS
//!   (format id `opera-harmony`): `Images/Index.idx.xml` (also `Index.xml`, `Index.ref.xml`), and
//!   Columbus exports (`ImageIndex.ColumbusIDX.xml` with multi-page TIFF or `.flex` planes).
//! - **Molecular Devices ImageXpress / MetaXpress** (format id `imagexpress`): an `.HTD` plate
//!   description, or a plate folder without one.
//! - **Yokogawa CellVoyager** CV7000/CV8000/CQ1 (format id `cellvoyager`):
//!   `MeasurementData.mlf` with `MeasurementDetail.mrf`, the `.mes` setting and `.wpi`/`.wpp`.
//!
//! Every field of view of every well is one image (`images[i].extra.well`, `field`); the plate
//! (id, type, wells, completeness of the copy on disk) is reported by
//! [`openreadout_core::Dataset::plate`]. `info` reads the index and lists the plate folder
//! once; it opens at most one plane file (for the sample type), however many the plate has.
//!
//! Layout and vocabulary: `docs/formats/hcs.md`, `docs/formats/opera-harmony.md`,
//! `docs/formats/imagexpress.md`, `docs/formats/cellvoyager.md`. Provenance:
//! `docs/provenance/opera-harmony.md`, `imagexpress.md`, `cellvoyager.md`.
//!
//! # Example
//!
//! ```
//! use openreadout_core::FormatReader;
//! use openreadout_hcs::HarmonyReader;
//!
//! let format = HarmonyReader.descriptor();
//! assert_eq!(format.id, openreadout_hcs::HARMONY_FORMAT_ID);
//! println!("{} ({}): {}", format.name, format.family, format.extensions.join(", "));
//! ```
//!
//! Reading a plate:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_hcs::HarmonyReader;
//!
//! let mut dataset = HarmonyReader.open(Path::new("Measurement 1/Images/Index.idx.xml"))?;
//! let plate = dataset.plate().expect("a plate");
//! println!("{} wells imaged, {} planes missing", plate.wells.len(), plate.planes_missing);
//! let well = plate.well("C05").expect("C05 was imaged");
//! // First field of view of well C05, channel 0.
//! let plane = dataset.read_plane(well.images[0], PlaneIndex { c: 0, z: 0, t: 0 })?;
//! assert_eq!(plane.data.len(), plane.expected_len());
//! # Ok::<(), openreadout_core::Error>(())
//! ```
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
pub mod cellvoyager;
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod flex;
#[doc(hidden)]
pub mod harmony;
#[doc(hidden)]
pub mod imagexpress;
#[doc(hidden)]
pub mod model;
#[doc(hidden)]
pub mod planes;

use std::path::{Path, PathBuf};

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Result};

pub use cellvoyager::CELLVOYAGER_FORMAT_ID;
#[doc(hidden)]
pub use dataset::HcsDataset;
pub use harmony::HARMONY_FORMAT_ID;
pub use imagexpress::IMAGEXPRESS_FORMAT_ID;

/// Format family of the three readers.
pub const FAMILY: &str = "microscopy";

fn small_head(fs: &Fs, path: &Path) -> Vec<u8> {
    use std::io::Read;
    let mut head = vec![0u8; 4096];
    let n = fs
        .open(path)
        .and_then(|mut f| f.read(&mut head))
        .unwrap_or(0);
    head.truncate(n);
    head
}

fn definite(format_id: &'static str, note: Option<String>) -> Detection {
    Detection {
        format_id,
        confidence: DetectConfidence::Definite,
        note,
    }
}

/// Revvity/PerkinElmer Harmony exports (Opera Phenix, Operetta, Operetta CLS).
#[derive(Debug, Default, Clone, Copy)]
pub struct HarmonyReader;

impl HarmonyReader {
    fn index_of(input: &Input) -> Option<PathBuf> {
        let fs = input.fs();
        let p = input.path();
        if fs.is_dir(p) {
            harmony::find_index(fs, p)
        } else {
            Some(p.to_path_buf())
        }
    }
}

impl FormatReader for HarmonyReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::OPERA_HARMONY)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: HARMONY_FORMAT_ID.into(),
            name: "Revvity/PerkinElmer Harmony and Columbus exports (Opera Phenix, Operetta, Opera)".into(),
            vendor: "Revvity (PerkinElmer)".into(),
            extensions: vec!["idx.xml".into(), "xml".into(), "flex".into()],
            family: FAMILY.into(),
            can_read: true,
            can_write: false,
            confidence: assurance::OPERA_HARMONY.confidence,
            known_gaps: vec![
                "Harmony 4/5 `Index.idx.xml` with TIFF planes, Columbus `ImageIndex.ColumbusIDX.xml` with multi-page TIFF or `.flex` planes, and standalone Opera `.flex` files or measurement folders are read; the older `Index.ref.xml` layout with gzip-compressed planes (`.tiff.gz`) is not".into(),
                "Standalone `.flex`: time series (Kinetic) and compressed pages are not validated on a public file; one well per file, pages as the Flex XML's Image@BufferNo say".into(),
                "FLIM planes (several FlimID values) and sequence/fk/fl variants in file names are not told apart".into(),
                "Flat-field profiles and skew-crop parameters are listed by `info --view full`, not applied".into(),
                "The Z step is the PositionZ difference of the first field's first two planes".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if flex::is_flex_name(path) && openreadout_tiff::looks_like_tiff(head) {
            return Some(definite(
                HARMONY_FORMAT_ID,
                Some("Opera .flex file (one well; the Flex XML describes its pages); open the measurement folder for the whole plate".into()),
            ));
        }
        (has_extension(path, &["xml"]) && harmony::looks_like_harmony(head)).then(|| {
            definite(
                HARMONY_FORMAT_ID,
                Some("Harmony plate index; its TIFF planes are read as one plate".into()),
            )
        })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let fs = input.fs();
        let p = input.path();
        if fs.is_dir(p) {
            let Some(idx) = harmony::find_index(fs, p) else {
                let n = flex::flex_files(fs, p).len();
                return (n > 0).then(|| {
                    definite(
                        HARMONY_FORMAT_ID,
                        Some(format!("Opera measurement folder of {n} .flex file(s)")),
                    )
                });
            };
            return harmony::looks_like_harmony(&small_head(fs, &idx)).then(|| {
                definite(
                    HARMONY_FORMAT_ID,
                    Some(format!("Harmony measurement folder ({})", idx.display())),
                )
            });
        }
        self.sniff(head, p)
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let fs = input.fs().clone();
        let p = input.path();
        if flex::is_flex_name(p) || (fs.is_dir(p) && harmony::find_index(&fs, p).is_none()) {
            let root = if fs.is_dir(p) {
                p.to_path_buf()
            } else {
                p.parent().map(Path::to_path_buf).unwrap_or_default()
            };
            let plate = dataset::cached(HARMONY_FORMAT_ID, &fs, p, &root, || {
                flex::parse(&fs, p).map(|pl| dataset::finish(pl, &fs, |_| false))
            })?;
            return Ok(Box::new(HcsDataset::new(plate, fs, self.descriptor())));
        }
        let idx = Self::index_of(input).ok_or_else(|| {
            Error::unsupported(
                HARMONY_FORMAT_ID,
                "a folder without Index.idx.xml",
                "Open the Harmony measurement folder, its Images folder or the Index.idx.xml inside it.",
            )
        })?;
        let root = idx.parent().map(Path::to_path_buf).unwrap_or_default();
        let plate = dataset::cached(HARMONY_FORMAT_ID, &fs, &idx, &root, || {
            harmony::parse(&fs, &idx).map(|p| dataset::finish(p, &fs, harmony::is_plane_name))
        })?;
        Ok(Box::new(HcsDataset::new(plate, fs, self.descriptor())))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Molecular Devices ImageXpress / MetaXpress plates.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImageXpressReader;

impl FormatReader for ImageXpressReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::IMAGEXPRESS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: IMAGEXPRESS_FORMAT_ID.into(),
            name: "Molecular Devices ImageXpress / MetaXpress plate (HTD + TIFF)".into(),
            vendor: "Molecular Devices".into(),
            extensions: vec!["htd".into()],
            family: FAMILY.into(),
            can_read: true,
            can_write: false,
            confidence: assurance::IMAGEXPRESS.confidence,
            known_gaps: vec![
                "Z series (ZStep_<n> folders) and time series (TimePoint_<n> folders) follow the folder names; Z projections and stitched montages are not handled specially".into(),
                "A plate folder without its HTD is read from the file names: missing planes cannot be detected there".into(),
                "Excitation wavelengths and per-channel colours are not recorded in the files read; channel names come from the HTD (WaveName) or the plane files' illumination setting".into(),
                "Cellomics/CellWorX variants of the HTD (`.pnl`) are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (has_extension(path, &["htd"]) || imagexpress::looks_like_htd(head))
            .then_some(())
            .filter(|()| imagexpress::looks_like_htd(head))
            .map(|()| {
                definite(
                    IMAGEXPRESS_FORMAT_ID,
                    Some(
                        "MetaXpress plate description; its TIFF planes are read as one plate"
                            .into(),
                    ),
                )
            })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let fs = input.fs();
        let p = input.path();
        if fs.is_dir(p) {
            if let Some(h) = imagexpress::find_htd(fs, p) {
                return imagexpress::looks_like_htd(&small_head(fs, &h)).then(|| {
                    definite(
                        IMAGEXPRESS_FORMAT_ID,
                        Some(format!("ImageXpress plate folder ({})", h.display())),
                    )
                });
            }
            // a plate folder without its HTD: MetaXpress file names and plane headers
            return imagexpress::plate_folder_without_htd(fs, p).map(|n| Detection {
                format_id: IMAGEXPRESS_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some(format!(
                    "folder of {n} MetaXpress plane files (<plate>_<well>_s<site>_w<wave>.tif with MetaMorph headers) without an HTD"
                )),
            });
        }
        self.sniff(head, p)
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let fs = input.fs().clone();
        let p = input.path().to_path_buf();
        let root = if fs.is_dir(&p) {
            p.clone()
        } else {
            p.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        let plate = dataset::cached(IMAGEXPRESS_FORMAT_ID, &fs, &p, &root, || {
            imagexpress::parse(&fs, &p).map(|pl| dataset::finish(pl, &fs, |_| false))
        })?;
        Ok(Box::new(HcsDataset::new(plate, fs, self.descriptor())))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// Yokogawa CellVoyager measurements (CV7000, CV8000, CQ1).
#[derive(Debug, Default, Clone, Copy)]
pub struct CellVoyagerReader;

impl CellVoyagerReader {
    fn mlf_of(input: &Input) -> Option<PathBuf> {
        let fs = input.fs();
        let p = input.path();
        if fs.is_dir(p) {
            return cellvoyager::find_index(fs, p);
        }
        let name = p.file_name()?.to_string_lossy().to_ascii_lowercase();
        if model::has_extension(&name, &["mlf"]) {
            return Some(p.to_path_buf());
        }
        cellvoyager::find_index(fs, p.parent()?)
    }
}

impl FormatReader for CellVoyagerReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::CELLVOYAGER)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: CELLVOYAGER_FORMAT_ID.into(),
            name: "Yokogawa CellVoyager measurement (CV7000, CV8000, CQ1)".into(),
            vendor: "Yokogawa".into(),
            extensions: vec!["mlf".into(), "mrf".into(), "wpi".into(), "mes".into()],
            family: FAMILY.into(),
            can_read: true,
            can_write: false,
            confidence: assurance::CELLVOYAGER.confidence,
            known_gaps: vec![
                "Tiled fields (several PartialTileIndex values) are not stitched".into(),
                "The objective's numerical aperture is not recorded in the files read".into(),
                "Excitation is reported only for channels with a single laser; the emission band comes from the filter name (BP<centre>/<width>)".into(),
                "Correction files (shading, geometry, crosstalk) are listed, not applied".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        (has_extension(path, &["mlf", "mrf", "wpi", "mes"]) && cellvoyager::looks_like_cellvoyager(head))
            .then(|| definite(CELLVOYAGER_FORMAT_ID, Some("CellVoyager measurement; MeasurementData.mlf and its TIFF planes are read as one plate".into())))
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let fs = input.fs();
        let p = input.path();
        if fs.is_dir(p) {
            let mlf = cellvoyager::find_index(fs, p)?;
            return cellvoyager::looks_like_cellvoyager(&small_head(fs, &mlf)).then(|| {
                definite(
                    CELLVOYAGER_FORMAT_ID,
                    Some(format!(
                        "CellVoyager measurement folder ({})",
                        mlf.display()
                    )),
                )
            });
        }
        let d = self.sniff(head, p)?;
        // `.mrf`/`.wpi`/`.mes` stand for the measurement only when its record file is there
        if has_extension(p, &["mlf"]) || Self::mlf_of(input).is_some() {
            Some(d)
        } else {
            None
        }
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let fs = input.fs().clone();
        let mlf = Self::mlf_of(input).ok_or_else(|| {
            Error::unsupported(
                CELLVOYAGER_FORMAT_ID,
                "a CellVoyager file without MeasurementData.mlf next to it",
                "Open the measurement folder or its MeasurementData.mlf (the record of every image).",
            )
        })?;
        let root = mlf.parent().map(Path::to_path_buf).unwrap_or_default();
        let plate = dataset::cached(CELLVOYAGER_FORMAT_ID, &fs, &mlf, &root, || {
            cellvoyager::parse(&fs, &mlf)
                .map(|p| dataset::finish(p, &fs, cellvoyager::is_plane_name))
        })?;
        Ok(Box::new(HcsDataset::new(plate, fs, self.descriptor())))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
