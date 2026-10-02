//! Clean-room reader for Olympus/Evident cellSens VSI files (cellSens, VS120/VS200 slide
//! scanners): the `.vsi` holds a TIFF preview and a tagged record tree with the metadata; the
//! pixels live in `_<name>_/stack<id>/frame_t*.ets` tile files (raw, JPEG or JPEG 2000 tiles,
//! pyramids for whole slides). A standalone `.ets` can be opened too.
//!
//! Layout and vocabulary: `docs/formats/vsi.md`. Provenance: `docs/provenance/vsi.md`.
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
//! use openreadout_vsi::VsiReader;
//!
//! let format = VsiReader.descriptor();
//! assert_eq!(format.id, openreadout_vsi::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_vsi::VsiReader;
//!
//! let mut dataset = VsiReader.open(Path::new("slide.vsi"))?;
//! let info = dataset.info()?; // headers only: fast even on multi-gigabyte files
//! for image in &info.images {
//!     println!(
//!         "image {}: {} x {} px, {} channel(s), {} plane(s)",
//!         image.index, image.size_x, image.size_y, image.size_c, image.plane_count
//!     );
//! }
//! // Pixels are read one plane at a time: image 0, channel 0, first Z slice, first time point.
//! let plane = dataset.read_plane(0, PlaneIndex { c: 0, z: 0, t: 0 })?;
//! assert_eq!(plane.data.len(), plane.expected_len());
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
pub mod ets;
#[doc(hidden)]
pub mod meta;
#[doc(hidden)]
pub mod tiff;
#[doc(hidden)]
pub mod tree;

use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::Input;
use openreadout_core::{Error, Result};

#[doc(hidden)]
pub use dataset::VsiDataset;
#[doc(hidden)]
pub use ets::{ETS_MAGIC, EtsCompression, EtsFile, EtsHeader, SIS_MAGIC, Tile, looks_like_ets};
#[doc(hidden)]
pub use meta::{
    ChannelMeta, DimKind, DimMeta, ObjectiveMeta, StackMeta, VsiMeta, read_meta, unit_factor,
};
#[doc(hidden)]
pub use tiff::{PreviewInfo, looks_like_vsi, preview_info, preview_jpeg};
#[doc(hidden)]
pub use tree::{TREE_OFFSET, TagRecord, TagTree, TagValue, VOLUME_MAGIC};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "vsi";

/// The Olympus/Evident cellSens VSI reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct VsiReader;

impl FormatReader for VsiReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::VSI)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "Olympus/Evident cellSens VSI".into(),
            vendor: "Evident (Olympus)".into(),
            extensions: vec!["vsi".into(), "ets".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: assurance::VSI.confidence,
            known_gaps: vec![
                "No specification: the .vsi record tree and ETS layout are derived from 27 corpus files (11 with pixel data: cellSens Dimension 1.18/3.2/3.x, VS200 ASW, VS120 dotSlide 2.5, Stream Essentials 1.7; ETS versions 0x00030003/5/6); names, calibration, channels, exposure, objective, camera, times and dimension kinds come from records identified by comparison with Bio-Formats; everything else is only in `vendor` under numeric tags".into(),
                "Dimension kinds other than Z (1), T (2) and channel (4) never occurred in the corpus; one would be exposed as T with a note and a `check` warning".into(),
                "ETS sample types other than 8-bit (2) and 16-bit (4) unsigned, and compressions other than raw (0), JPEG (2) and JPEG 2000 (3), are not decoded (exit 6)".into(),
                "Unstored whole-slide tiles are filled with the ETS background value (as Bio-Formats does); JPEG tiles are decoded with jpeg-decoder and may differ by a few levels from other decoders".into(),
                "Where the tile grid is offset from the image corner (record 2410), coarser pyramid levels place it at the nearest pixel of offset/2^L; Bio-Formats truncates, so such levels can differ from it by one pixel".into(),
                "Planes larger than 4 GiB (full-resolution whole slides) are read by region (`--region`) or at a pyramid level (`--level`), not whole".into(),
                "Stacks without an ETS file (focus maps, sample masks, measurement/ROI layers) and blob_*.ets focus data are listed, not decoded; a stack directory holding several frame_t*.ets files is read from the first only".into(),
                "The excitation wavelength (record 2474) is inferred from plausibility across 9 channels (Bio-Formats reports only emission)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if tiff::looks_like_vsi(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if ets::looks_like_ets(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("cellSens ETS tile file; open the .vsi next to its _<name>_ directory for names and calibration".into()),
            });
        }
        if has_extension(path, &["vsi", "ets"]) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some(
                    "`.vsi`/`.ets` extension, but the content does not match a cellSens header"
                        .into(),
                ),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn reads_any_source(&self) -> bool {
        true
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        let path = input.path();
        if !input.fs().is_file(path) {
            return Err(Error::io(
                path,
                std::io::Error::new(std::io::ErrorKind::NotFound, "not a file"),
            ));
        }
        Ok(Box::new(dataset::VsiDataset::open(input)?))
    }
}
