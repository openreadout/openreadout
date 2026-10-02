//! Clean-room reader for the TIFF family.
//!
//! Container: TIFF 6.0 and BigTIFF (open specifications). Conventions detected from tags,
//! in priority order: OME-TIFF (OME-XML in `ImageDescription`, multi-file sets, `BinaryOnly`
//! and `*.companion.ome`), Zeiss LSM, MetaMorph STK, Aperio SVS, Hamamatsu NDPI, PerkinElmer
//! QPTIFF, MetaMorph MetaSeries, ImageJ hyperstacks, Micro-Manager, and plain TIFF; Nikon
//! NIS-Elements TIFF exports are grouped with their sibling files. A MetaMorph `.nd` file opens
//! the multi-stage, multi-wavelength time series of TIFF/STK files it names.
//!
//! Layout and vocabulary: `docs/formats/tiff.md`. Provenance: `docs/provenance/tiff.md`.
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
//! use openreadout_tiff::TiffReader;
//!
//! let format = TiffReader.descriptor();
//! assert_eq!(format.id, openreadout_tiff::FORMAT_ID);
//! println!("{} ({}): .{}", format.name, format.family, format.extensions.join(", ."));
//! ```
//!
//! Reading a file:
//!
//! ```no_run
//! use std::path::Path;
//!
//! use openreadout_core::{FormatReader, PlaneIndex};
//! use openreadout_tiff::TiffReader;
//!
//! let mut dataset = TiffReader.open(Path::new("stack.ome.tiff"))?;
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
pub mod bif;
mod check;
mod chunk_codecs;
#[doc(hidden)]
pub mod container;
#[doc(hidden)]
pub mod dataset;
#[doc(hidden)]
pub mod decode;
#[doc(hidden)]
pub mod eer;
mod files;
mod flavors;
mod imagej;
mod jpeg2000;
mod lsm;
mod mdgel;
#[doc(hidden)]
pub mod metamorph;
mod nd;
mod ndpi;
#[doc(hidden)]
pub mod nis;
#[doc(hidden)]
pub mod ome;
#[doc(hidden)]
pub mod philips;
#[doc(hidden)]
pub mod scn;
mod tags;

use std::io::Read;
use std::path::Path;

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Result};

#[doc(hidden)]
pub use container::{
    BIGTIFF_MAGIC, ByteOrder, ByteSource, Field, FieldValue, Ifd, StructureProblem, TIFF_MAGIC,
    TiffFile, TiffHeader, looks_like_tiff,
};
#[doc(hidden)]
pub use dataset::{Flavor, TiffDataset};
#[doc(hidden)]
pub use decode::{MAX_PLANE_BYTES, PageLayout, SampleSelect, compression_name, read_page};
#[doc(hidden)]
pub use metamorph::{MetaSeriesPlane, NdFile, StkInfo, parse_metaseries, parse_nd, parse_stk};
#[doc(hidden)]
pub use nis::{NisPage, SequenceAxis, SequenceName, parse_nis, parse_sequence_name};
#[doc(hidden)]
pub use ome::{
    OmeAnnotation, OmeBinaryOnly, OmeChannel, OmeDetector, OmeDocument, OmeImage, OmeInstrument,
    OmeObjective, OmePixels, OmePlane, OmeTiffData, parse as parse_ome_xml,
};

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "tiff";

/// Extensions of the TIFF family (lowercase, without the dot).
pub const EXTENSIONS: [&str; 15] = [
    "tif", "tiff", "ome.tif", "ome.tiff", "svs", "ndpi", "lsm", "qptiff", "btf", "stk", "nd",
    "eer", "scn", "bif", "gel",
];

/// The TIFF-family reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct TiffReader;

pub(crate) fn is_companion_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().ends_with(".companion.ome"))
}

pub(crate) fn file_starts_like_tiff(fs: &Fs, path: &Path) -> Result<bool> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let mut head = [0u8; 4];
    let n = f.read(&mut head).map_err(|e| Error::io(path, e))?;
    Ok(n == 4 && looks_like_tiff(&head))
}

/// A MetaMorph `.nd` series file: the `.nd` extension and the `"NDInfoFile"` key first.
pub(crate) fn is_nd_path(fs: &Fs, path: &Path) -> bool {
    has_extension(path, &["nd"])
        && fs
            .open(path)
            .and_then(|mut f| {
                let mut head = [0u8; 16];
                let n = f.read(&mut head)?;
                Ok(looks_like_nd(&head[..n]))
            })
            .unwrap_or(false)
}

fn looks_like_nd(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    head.starts_with(b"\"NDInfoFile\"")
}

fn looks_like_ome_xml(head: &[u8]) -> bool {
    let s = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    let s = s.trim_start_matches('\u{feff}').trim_start();
    s.starts_with('<') && (s.contains("<OME ") || s.contains("<OME>") || s.contains(":OME "))
}

impl FormatReader for TiffReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&assurance::TIFF)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "TIFF family (TIFF, BigTIFF, OME-TIFF, ImageJ, SVS, NDPI, LSM, QPTIFF, Leica SCN, Ventana BIF, Micro-Manager, MetaMorph STK/.nd, Thermo Fisher EER)".into(),
            vendor: "open standard (TIFF 6.0 / OME); Leica Aperio, Hamamatsu, Zeiss, PerkinElmer, Molecular Devices MetaMorph, Thermo Fisher (Falcon EER) conventions".into(),
            extensions: EXTENSIONS.iter().map(|s| (*s).to_string()).collect(),
            family: "microscopy".into(),
            can_read: true,
            // `export --to ome-tiff` writes OME-TIFF (openreadout-ometiff).
            can_write: true,
            confidence: assurance::TIFF.confidence,
            known_gaps: vec![
                "Old-style JPEG (6) is decoded only in its tables form (not lossless, not a single JPEGInterchangeFormat stream); LERC tiles exit 6 in default builds (the opt-in `lerc` feature decodes them with lerc-rs, which panics on some malformed input; SECURITY.md). JPEG 2000 tiles (Aperio 33003/33005, 34712) are decoded, irreversible (9/7) ones within 1 grey level of OpenJPEG (the reconstruction is not bit-exact)".into(),
                "Packed 1-31-bit, half and 24-bit float and complex-integer samples are decoded only uncompressed or with LZW, deflate, PackBits or zstd (not inside JPEG, JPEG 2000, WebP, JPEG XL or LERC chunks)".into(),
                "Ventana BIF: tiles are returned on their stored grid; the overlaps recorded in EncodeInfo/TileJointInfo are reported, not stitched (such files are partially decoded for --strict); volumetric BIF (ImageDepth > 1) exits 6".into(),
                "EER (Falcon electron-event movies): frames are decoded to counts on the sensor grid; the sub-pixel (super-resolution) positions of events are not used, the gain reference is not applied, and the TIFF Orientation is reported, not applied".into(),
                "Planes larger than 4 GiB (whole-slide level 0) are read by region (`--region`), not whole; NDPI level 0 is read by its JPEG restart intervals (baseline JPEG only)".into(),
                "NDPI files larger than 4 GiB (offset high bytes in tag 65324) and LSM files larger than 4 GiB (wrapped strip offsets) are not supported".into(),
                "OME Modulo annotations (FLIM/lambda sub-dimensions along C/Z/T) are not expanded; the OME sizes are reported as stored".into(),
                "Micro-Manager multi-file datasets without OME-XML and other TIFF dialects fall back to plain-TIFF behaviour (pages as Z)".into(),
                "MetaMorph STK: only uncompressed stacks; `.nd` series: file names are built from the .nd keys (`_w<i><name>_s<j>_t<k>`), stage positions become separate images, no time increment is derived from the member files".into(),
                "Nikon NIS-Elements TIFF exports: files are grouped by the `xy`/`c` tokens of their names (validated on one public export; `t`/`z` tokens inferred); the LV and XML metadata blocks (objective, channel names, text information) are not decoded; the pixel size tag's meaning is inferred".into(),
                "Palette (indexed-colour) pages return the stored indices; photometric WhiteIsZero pages return stored values (not inverted)".into(),
                "JPEG: YCbCr stored as separate sample planes (PlanarConfiguration 2, e.g. Bio-Formats 8.x from planar input) and three-component streams in grey/palette/CMYK pages are refused (exit 6) rather than decoded to wrong colours; chroma upsampling differs from libjpeg-turbo by up to 3 counts".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if looks_like_tiff(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if has_extension(path, &["nd"]) && looks_like_nd(head) {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some(
                    "MetaMorph .nd series file (the TIFF/STK files it names are read as one data set)"
                        .into(),
                ),
            });
        }
        if (is_companion_path(path) || has_extension(path, &["ome", "xml"]))
            && looks_like_ome_xml(head)
        {
            return Some(Detection {
                format_id: FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some("OME-XML companion file of a multi-file OME-TIFF set".into()),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(TiffDataset::open(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
