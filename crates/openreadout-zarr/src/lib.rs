//! OME-Zarr (OME-NGFF) reader: NGFF 0.1–0.5 on Zarr v2 and v3, directory and zip stores.
//!
//! Images come from `multiscales` groups, HCS plates (one image per field of view) and
//! `bioformats2raw.layout` collections (one image per series, with `OME/METADATA.ome.xml` parsed
//! by the TIFF crate's OME-XML parser). Arrays are read through the pure-Rust `zarrs` crate;
//! the `blosc`, `zstd` and numcodecs `lz4` codecs are decoded by `openreadout-codecs` (see
//! [`codecs`]) so that no C library is needed.
//!
//! Layout and vocabulary: `docs/formats/ome-zarr.md`. Provenance: `docs/provenance/ome-zarr.md`.
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
//! use openreadout_zarr::ZarrReader;
//!
//! let format = ZarrReader.descriptor();
//! assert_eq!(format.id, openreadout_zarr::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_zarr::ZarrReader;
//!
//! let mut dataset = ZarrReader.open(Path::new("plate.ome.zarr"))?;
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
pub mod codecs;
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod ngff;
#[doc(hidden)]
pub mod store;

use std::path::Path;

use openreadout_core::Result;
use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::{Fs, Input};

#[doc(hidden)]
pub use dataset::{ArrayMeta, AxisRole, ZarrDataset, ZarrImage, array_meta, axis_roles};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "ome-zarr";

/// Group metadata keys that make a Zarr group an OME-NGFF entry point.
const NGFF_KEYS: [&str; 4] = [
    "\"multiscales\"",
    "\"plate\"",
    "\"bioformats2raw.layout\"",
    "\"well\"",
];

/// The OME-Zarr reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct ZarrReader;

fn ngff_text(s: &str) -> bool {
    NGFF_KEYS.iter().any(|k| s.contains(k))
}

/// Does this directory hold a Zarr hierarchy root, and does its metadata name NGFF keys?
fn sniff_dir(fs: &Fs, dir: &Path) -> Option<DetectConfidence> {
    let mut any = false;
    for doc in ["zarr.json", ".zattrs", ".zgroup"] {
        let p = dir.join(doc);
        let Ok(meta) = fs.metadata(&p) else {
            continue;
        };
        any = true;
        if meta.len() > 16 << 20 {
            continue;
        }
        if let Ok(text) = fs.read_to_string(&p)
            && ngff_text(&text)
        {
            return Some(DetectConfidence::Definite);
        }
    }
    any.then_some(DetectConfidence::Likely)
}

impl FormatReader for ZarrReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::OME_ZARR)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "OME-Zarr (OME-NGFF)".into(),
            vendor: "open standard (OME-NGFF, Zarr)".into(),
            extensions: vec!["zarr".into(), "ome.zarr".into(), "zip".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: true,
            confidence: assurance::OME_ZARR.confidence,
            known_gaps: vec![
                "Label images are exposed as images after the others (label ids as stored); their colours and properties are copied, not interpreted".into(),
                "Codecs other than blosc (blosclz/lz4/snappy/zlib/zstd, byte and bit shuffle)/zstd/gzip/zlib/lz4/crc32c/sharding/transpose are not decoded (exit 4 on read, reported by `check`)".into(),
                "String, structured and datetime data types are not read (exit 6); float16 is widened to float32, bool is uint8 0/1".into(),
                "Scale and translation transformations are composed (physical sizes, `extra.origin_um`); NGFF 0.6 coordinate systems and affine/rotation transformations are reported, not applied".into(),
                "Axes other than t/c/z/y/x are read at index 0".into(),
                "Remote (HTTP/S3) stores are not opened: the binary has no network access".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    /// Directory stores are recognised by their root metadata, read through the namespace.
    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let (path, fs) = (input.path(), input.fs());
        if fs.is_dir(path) {
            return sniff_dir(fs, path).map(|confidence| Detection {
                format_id: FORMAT_ID,
                confidence,
                note: Some(if confidence == DetectConfidence::Definite {
                    "Zarr directory store with OME-NGFF metadata".into()
                } else {
                    "Zarr directory store (no OME-NGFF keys at the root)".into()
                }),
            });
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if matches!(name, "zarr.json" | ".zattrs" | ".zgroup")
            && ngff_text(&String::from_utf8_lossy(head))
        {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("metadata document at the root of a Zarr directory store".into()),
            });
        }
        if head.starts_with(b"PK\x03\x04") && has_extension(path, &["zip"]) {
            let names_zarr = [
                b"zarr.json".as_slice(),
                b".zgroup".as_slice(),
                b".zattrs".as_slice(),
                b".zarray".as_slice(),
            ]
            .iter()
            .any(|n| head.windows(n.len()).any(|w| w == *n));
            if names_zarr {
                return Some(Detection {
                    format_id: FORMAT_ID,
                    confidence: DetectConfidence::Definite,
                    note: Some("zip archive holding a Zarr store".into()),
                });
            }
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ZarrDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ZarrDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
