//! OME-TIFF writer: `BigTIFF` container (via the `tiff` crate) plus an OME-XML 2016-06
//! document in the first IFD's `ImageDescription`. Every export is written to a temporary
//! file, read back plane by plane and hash-compared to the source before it is renamed
//! into place. The source file is never opened for writing.
//!
//! OME-TIFF is the open microscopy exchange format: an ordinary (here: Big)TIFF file with one
//! page per plane and the full acquisition metadata as OME-XML, readable by Fiji, napari,
//! QuPath, Bio-Formats and most image software.
//!
//! # Example
//!
//! ```
//! use std::path::Path;
//!
//! use openreadout_core::{Dataset, Result};
//! use openreadout_ometiff::{Codec, ExportOptions, export_ome_tiff};
//!
//! /// Write channel 0 of every image of an opened file as LZW-compressed OME-TIFF.
//! fn channel0_to_ome_tiff(dataset: &mut dyn Dataset, input: &Path) -> Result<()> {
//!     let mut options = ExportOptions::default();
//!     options.select = vec!["c=0".into()];
//!     options.codec = Codec::Lzw;
//!     let report = export_ome_tiff(dataset, input, Path::new("channel0.ome.tiff"), &options)?;
//!     assert!(report.verified); // every plane was read back and hash-compared
//!     Ok(())
//! }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod omexml;
pub mod pyramid;
mod tiled;
mod tiled_export;

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use openreadout_core::parallel::{PlaneRequest, ReadContext, read_in_order};
use openreadout_core::reader::PlaneIndex;
use openreadout_core::select::Selection;
use openreadout_core::{Dataset, Error, PixelType, Plane, Result};
use rayon::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tiff::encoder::{TiffEncoder, TiffKindBig};
use tiff::tags::Tag;

pub use omexml::{WrittenImage, build_ome_xml, build_ome_xml_metadata_only};
pub use pyramid::PyramidMode;
pub use tiled_export::WrittenLevels;

/// Compression codec for the TIFF strips.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Codec {
    /// No compression.
    None,
    /// Deflate (zlib), lossless; the default.
    Deflate,
    /// LZW, lossless; readable by older TIFF software.
    Lzw,
}

/// Export options.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ExportOptions {
    /// Only this image index (default: all).
    pub image: Option<u32>,
    /// Plane selection strings (`c=0`, `z=1-3`, `t=0,2`).
    pub select: Vec<String>,
    /// Compression of the pixel data.
    pub codec: Codec,
    /// Replace an existing output file.
    pub overwrite: bool,
    /// Embed the vendor metadata tree (as JSON) in a `StructuredAnnotation`.
    pub embed_vendor: bool,
    /// Export this source pyramid level as the full resolution (0 = the source's full
    /// resolution).
    pub level: u32,
    /// Export only this rectangle of every plane, in the pixel coordinates of `level`.
    pub region: Option<openreadout_core::Region>,
    /// Reduced-resolution levels (`SubIFDs`): `Auto` copies the source's own levels when the
    /// whole image is exported from level 0 and the source has a pyramid; else none.
    pub pyramid: PyramidMode,
    /// Levels including full resolution: exactly (mean pyramids) or at most (source pyramids).
    pub levels: Option<u32>,
    /// Tile edge of tiled output in pixels (a multiple of 16; default 512). Tiled output is
    /// written for pyramids, regions, levels and planes above 4 GiB.
    pub tile: u32,
}

impl Default for ExportOptions {
    /// Every image and plane, deflate compression, no overwrite, no vendor tree, full
    /// resolution, the source's own pyramid levels when it has them.
    fn default() -> Self {
        ExportOptions {
            image: None,
            select: Vec::new(),
            codec: Codec::Deflate,
            overwrite: false,
            embed_vendor: false,
            level: 0,
            region: None,
            pyramid: PyramidMode::Auto,
            levels: None,
            tile: DEFAULT_TILE,
        }
    }
}

/// Default tile edge of tiled OME-TIFF output.
pub const DEFAULT_TILE: u32 = 512;

/// What `export` reports.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportReport {
    /// The file that was read.
    pub input: String,
    /// The OME-TIFF (or OME-Zarr) written.
    pub output: String,
    /// `ome-tiff` or `ome-zarr`.
    pub format: String,
    /// Images written.
    pub images_written: u32,
    /// Planes written, over all images.
    pub planes_written: u64,
    /// Size of the output in bytes.
    pub bytes_written: u64,
    /// True when every written plane was read back and hashed equal to the source plane.
    pub verified: bool,
    /// Compression used.
    pub codec: Codec,
    /// Size of the embedded OME-XML in bytes.
    pub ome_xml_bytes: u64,
    /// Per image, the resolution levels written (tiled exports: pyramids, regions, levels);
    /// absent for single-resolution exports of whole planes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resolutions: Vec<WrittenLevels>,
    /// OME-Zarr: `plate` when the store is an OME-NGFF high-content screening plate (row,
    /// column and field groups); absent for single images and collections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    /// Images left out because plane files they need are missing (`--skip-incomplete`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images_skipped: Vec<u32>,
}

pub(crate) const CREATOR: &str = concat!("openreadout ", env!("CARGO_PKG_VERSION"));

fn tiff_err(e: tiff::TiffError, path: &Path) -> Error {
    Error::Other(format!("TIFF write error on {}: {e}", path.display()))
}

/// TIFF description of a plane's samples.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SampleLayout {
    bits: u16,
    /// TIFF `SampleFormat`: 1 unsigned integer, 2 signed integer, 3 IEEE floating point.
    format: u16,
    /// TIFF `PhotometricInterpretation`: 1 black is zero, 2 RGB.
    photometric: u16,
}

fn sample_layout(plane: &Plane) -> Result<SampleLayout> {
    layout_of(plane.pixel_type, plane.samples_per_pixel)
}

/// The TIFF sample description of `samples_per_pixel` samples of `pixel_type`.
pub(crate) fn layout_of(pixel_type: PixelType, samples_per_pixel: u32) -> Result<SampleLayout> {
    let (bits, format) = match (pixel_type, samples_per_pixel) {
        (PixelType::Uint8, 1 | 3) => (8, 1),
        (PixelType::Uint16, 1 | 3) => (16, 1),
        (PixelType::Uint32, 1) => (32, 1),
        (PixelType::Int8, 1) => (8, 2),
        (PixelType::Int16, 1) => (16, 2),
        (PixelType::Int32, 1) => (32, 2),
        (PixelType::Float, 1 | 3) => (32, 3),
        (PixelType::Double, 1) => (64, 3),
        (pt, spp) => {
            return Err(Error::unsupported(
                "ome-tiff",
                format!("{} samples of {} per pixel", spp, pt.ome_name()),
                "OME-TIFF export supports 1-sample int8/16/32, uint8/16/32, float and double planes and 3-sample uint8/16/float planes; OME-Zarr export takes the others.",
            ));
        }
    };
    Ok(SampleLayout {
        bits,
        format,
        photometric: if samples_per_pixel == 3 { 2 } else { 1 },
    })
}

/// A plane made ready for the sequential TIFF writer on a worker thread: samples in the file's
/// byte order and, for deflate and LZW, every strip already compressed. Compressing here rather
/// than in the writer is what lets `--threads` speed up compressed exports.
struct EncodedPlane {
    width: u32,
    height: u32,
    samples_per_pixel: u16,
    layout: SampleLayout,
    rows_per_strip: u32,
    strips: Strips,
}

enum Strips {
    /// Uncompressed: the plane's bytes, cut into strips of `strip_bytes` when written.
    Raw { data: Vec<u8>, strip_bytes: usize },
    /// One compressed buffer per strip.
    Compressed(Vec<Vec<u8>>),
}

/// TIFF `Compression` tag value.
pub(crate) fn compression_tag(codec: Codec) -> u16 {
    match codec {
        Codec::None => 1,
        Codec::Lzw => 5,
        Codec::Deflate => 8,
    }
}

/// Strips of at most 1 MiB of whole rows (at least one row).
fn rows_per_strip(plane: &Plane) -> u32 {
    let row_bytes = plane.row_bytes().max(1);
    ((1usize << 20) / row_bytes).clamp(1, plane.height.max(1) as usize) as u32
}

fn encode_plane(plane: Plane, codec: Codec) -> Result<EncodedPlane> {
    let layout = sample_layout(&plane)?;
    let rows = rows_per_strip(&plane);
    let strip_bytes = plane.row_bytes().max(1) * rows as usize;
    let sample_bytes = plane.pixel_type.bytes_per_sample();
    let (width, height, spp) = (plane.width, plane.height, plane.samples_per_pixel);
    let mut data = plane.data;
    // Plane samples are little-endian; the `tiff` encoder writes the host's byte order.
    if cfg!(target_endian = "big") && sample_bytes > 1 {
        for s in data.chunks_mut(sample_bytes) {
            s.reverse();
        }
    }
    let strips = match codec {
        Codec::None => Strips::Raw { data, strip_bytes },
        Codec::Deflate => Strips::Compressed(
            data.chunks(strip_bytes)
                .map(|s| {
                    use std::io::Write;
                    // Same codec and level as the `tiff` crate's default deflate.
                    let mut z = flate2::write::ZlibEncoder::new(
                        Vec::with_capacity(s.len() / 2),
                        flate2::Compression::new(6),
                    );
                    z.write_all(s)?;
                    z.finish()
                })
                .collect::<std::io::Result<_>>()
                .map_err(|e| Error::Other(format!("deflate compression failed: {e}")))?,
        ),
        Codec::Lzw => Strips::Compressed(
            data.chunks(strip_bytes)
                .map(|s| {
                    weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8).encode(s)
                })
                .collect::<std::result::Result<_, _>>()
                .map_err(|e| Error::Other(format!("LZW compression failed: {e}")))?,
        ),
    };
    Ok(EncodedPlane {
        width,
        height,
        samples_per_pixel: spp as u16,
        layout,
        rows_per_strip: rows,
        strips,
    })
}

/// Write one encoded plane as the next IFD. `description` is attached when `Some`. Tags are
/// written in the order the `tiff` crate's `ImageEncoder` uses, so the file layout is the same
/// as before strips were compressed on worker threads.
fn write_plane<W: std::io::Write + std::io::Seek>(
    enc: &mut TiffEncoder<W, TiffKindBig>,
    plane: &EncodedPlane,
    codec: Codec,
    description: Option<&str>,
    out_path: &Path,
) -> Result<()> {
    use tiff::encoder::Rational;
    use tiff::tags::ResolutionUnit;
    let te = |e| tiff_err(e, out_path);
    if plane.width == 0 || plane.height == 0 {
        return Err(Error::Other(format!(
            "cannot write a {}x{} plane to TIFF",
            plane.width, plane.height
        )));
    }
    let spp = usize::from(plane.samples_per_pixel);
    let mut dir = enc.image_directory().map_err(te)?;
    dir.write_tag(Tag::ImageWidth, plane.width).map_err(te)?;
    dir.write_tag(Tag::ImageLength, plane.height).map_err(te)?;
    dir.write_tag(Tag::Compression, compression_tag(codec))
        .map_err(te)?;
    dir.write_tag(Tag::Predictor, 1u16).map_err(te)?;
    dir.write_tag(Tag::PhotometricInterpretation, plane.layout.photometric)
        .map_err(te)?;
    dir.write_tag(Tag::RowsPerStrip, plane.rows_per_strip)
        .map_err(te)?;
    dir.write_tag(Tag::SamplesPerPixel, plane.samples_per_pixel)
        .map_err(te)?;
    dir.write_tag(Tag::XResolution, Rational { n: 1, d: 1 })
        .map_err(te)?;
    dir.write_tag(Tag::YResolution, Rational { n: 1, d: 1 })
        .map_err(te)?;
    dir.write_tag(Tag::ResolutionUnit, ResolutionUnit::None)
        .map_err(te)?;
    dir.write_tag(Tag::Software, CREATOR).map_err(te)?;
    if let Some(d) = description {
        dir.write_tag(Tag::ImageDescription, d).map_err(te)?;
    }
    let mut offsets: Vec<u64> = Vec::new();
    let mut counts: Vec<u64> = Vec::new();
    let mut put = |s: &[u8]| -> Result<()> {
        offsets.push(dir.write_data(s).map_err(te)?);
        counts.push(s.len() as u64);
        Ok(())
    };
    match &plane.strips {
        Strips::Raw { data, strip_bytes } => data.chunks(*strip_bytes).try_for_each(&mut put)?,
        Strips::Compressed(strips) => strips.iter().map(Vec::as_slice).try_for_each(&mut put)?,
    }
    dir.write_tag(Tag::BitsPerSample, &vec![plane.layout.bits; spp][..])
        .map_err(te)?;
    dir.write_tag(Tag::SampleFormat, &vec![plane.layout.format; spp][..])
        .map_err(te)?;
    dir.write_tag(Tag::StripOffsets, &offsets[..]).map_err(te)?;
    dir.write_tag(Tag::StripByteCounts, &counts[..])
        .map_err(te)?;
    dir.finish().map_err(te)
}

/// Read every IFD of the written file back and return the xxh3 of each plane's samples
/// (little-endian bytes, as [`Plane::xxh3_hex`] hashes them). `expected` is the number of
/// planes written. The IFDs are split into contiguous ranges read by parallel workers (the
/// `--threads` pool), each with its own decoder; the last range keeps reading past `expected`
/// if the file has more IFDs, so a surplus shows up as a count mismatch.
fn read_back_hashes(path: &Path, expected: usize, plane_bytes: u64) -> Result<Vec<String>> {
    let expected = expected.max(1);
    // As many workers as planes may be in flight during decoding (threads, memory-bounded).
    let workers = openreadout_core::parallel::window(plane_bytes).clamp(1, expected);
    let per = expected.div_ceil(workers);
    let ranges: Vec<(usize, usize)> = (0..expected)
        .step_by(per)
        .map(|s| (s, (s + per).min(expected)))
        .collect();
    let last = ranges.len() - 1;
    let parts: Vec<Result<Vec<String>>> = ranges
        .par_iter()
        .enumerate()
        .map(|(i, &(start, end))| read_back_range(path, start, end, i == last))
        .collect();
    let mut out = Vec::with_capacity(expected);
    for p in parts {
        out.extend(p?);
    }
    Ok(out)
}

fn read_back_range(path: &Path, start: usize, end: usize, to_last: bool) -> Result<Vec<String>> {
    use tiff::decoder::{Decoder, DecodingResult};
    let tiff_err =
        |e: tiff::TiffError| Error::Other(format!("read-back of {} failed: {e}", path.display()));
    let f = File::open(path).map_err(|e| Error::io(path, e))?;
    // We wrote these planes ourselves from planes already held in memory, so the decoder's
    // default 256 MiB buffer limit (hit by whole-slide RGB scans) protects nothing here.
    let mut dec = Decoder::new(std::io::BufReader::new(f))
        .map_err(tiff_err)?
        .with_limits(tiff::decoder::Limits::unlimited());
    if start > 0 {
        dec.seek_to_image(start).map_err(tiff_err)?;
    }
    let mut out = Vec::with_capacity(end - start);
    let mut i = start;
    loop {
        // A fresh buffer per plane: `read_image_to_buffer` allocates the new one before it
        // drops what the buffer held, so reusing it would keep two planes alive at once.
        let mut buf = DecodingResult::U8(Vec::new());
        dec.read_image_to_buffer(&mut buf).map_err(tiff_err)?;
        out.push(format!("{:032x}", hash_le(&mut buf)));
        i += 1;
        if (i >= end && !to_last) || !dec.more_images() {
            break;
        }
        dec.next_image().map_err(tiff_err)?;
    }
    Ok(out)
}

/// xxh3-128 of a decoded buffer's samples in little-endian byte order, without first copying
/// the plane into a byte vector.
fn hash_le(buf: &mut tiff::decoder::DecodingResult) -> u128 {
    use tiff::decoder::DecodingResult as D;
    let width = match buf {
        D::U8(_) | D::I8(_) => 1,
        D::U16(_) | D::I16(_) | D::F16(_) => 2,
        D::U32(_) | D::I32(_) | D::F32(_) => 4,
        D::U64(_) | D::I64(_) | D::F64(_) => 8,
    };
    let view = buf.as_buffer(0);
    let bytes = view.as_bytes();
    if cfg!(target_endian = "little") || width == 1 {
        return xxhash_rust::xxh3::xxh3_128(bytes);
    }
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    let mut tmp = Vec::with_capacity(1 << 16);
    for chunk in bytes.chunks(1 << 16) {
        tmp.clear();
        tmp.extend_from_slice(chunk);
        for s in tmp.chunks_mut(width) {
            s.reverse();
        }
        h.update(&tmp);
    }
    h.digest128()
}

/// Export `ds` (already opened from `input`) to an OME-TIFF at `output`.
pub fn export_ome_tiff(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &ExportOptions,
) -> Result<ExportReport> {
    export_ome_tiff_with(ds, input, output, opts, &ReadContext::default())
}

/// [`export_ome_tiff`] with parallel plane decoding and progress reporting: planes are
/// decoded by worker threads (extra handles from `ctx.opener`) and written sequentially in
/// the same order, so the file is byte-identical whatever the number of threads.
pub fn export_ome_tiff_with(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &ExportOptions,
    ctx: &ReadContext<'_>,
) -> Result<ExportReport> {
    if output.exists() && !opts.overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let info = ds.info()?;
    let sel = Selection::parse(&opts.select)?;
    let mut written: Vec<WrittenImage<'_>> = Vec::new();
    let mut next_ifd = 0u32;
    for im in &info.images {
        if opts.image.is_some_and(|i| i != im.index) {
            continue;
        }
        // OME-Zarr label images (`extra.label`) have no OME-TIFF counterpart: written only
        // when asked for by `--image`.
        if opts.image.is_none()
            && im.extra.get("label").and_then(serde_json::Value::as_bool) == Some(true)
        {
            continue;
        }
        // Per-plane lists below are sized by the declared dimensions; refuse absurd ones.
        openreadout_core::limits::checked_plane_count("ome-tiff", im.size_c, im.size_z, im.size_t)?;
        let c_map: Vec<u32> = (0..im.size_c)
            .filter(|&c| sel.c.is_empty() || sel.c.contains(&c))
            .collect();
        let z_map: Vec<u32> = (0..im.size_z)
            .filter(|&z| sel.z.is_empty() || sel.z.contains(&z))
            .collect();
        let t_map: Vec<u32> = (0..im.size_t)
            .filter(|&t| sel.t.is_empty() || sel.t.contains(&t))
            .collect();
        if c_map.is_empty() || z_map.is_empty() || t_map.is_empty() {
            continue;
        }
        let mut planes = Vec::new();
        for &t in &t_map {
            for &z in &z_map {
                for &c in &c_map {
                    planes.push((c, z, t));
                }
            }
        }
        let n = planes.len() as u32;
        // Per-frame records feed the `Plane` elements; they are metadata, so a reader that
        // cannot produce them does not stop the export.
        let frames = ds
            .frames(im.index, None)
            .map(|(_, r)| r)
            .unwrap_or_default();
        written.push(WrittenImage {
            info: im,
            first_ifd: next_ifd,
            planes,
            size_c: c_map.len() as u32,
            size_z: z_map.len() as u32,
            size_t: t_map.len() as u32,
            c_map,
            source_planes: Vec::new(),
            frames,
        });
        next_ifd += n;
    }
    if written.is_empty() {
        return Err(Error::Usage("selection matches no planes".into()));
    }
    if written
        .iter()
        .any(|w| tiled_export::needs_tiled(w.info, opts))
    {
        return export_tiled(ds, &info, &written, input, output, opts, ctx);
    }
    // Fail before writing anything when a plane could not be held in memory.
    let mut max_plane = 0u64;
    for w in &written {
        let n = openreadout_core::pixel::plane_bytes_checked(
            "ome-tiff",
            w.info.size_x,
            w.info.size_y,
            w.info.samples_per_pixel.max(1) as usize * w.info.pixel_type.bytes_per_sample(),
        )?;
        max_plane = max_plane.max(n as u64);
    }
    // Re-index the selected planes to 0..n for the OME-XML (TiffData FirstC/Z/T are in the exported image's own index space).
    let xml_images: Vec<WrittenImage<'_>> = written
        .iter()
        .map(|w| {
            let mut w2 = w.clone();
            w2.source_planes.clone_from(&w.planes);
            let cpos = |c: u32| w.c_map.iter().position(|&x| x == c).unwrap_or(0) as u32;
            let zs: Vec<u32> = {
                let mut v: Vec<u32> = w.planes.iter().map(|p| p.1).collect();
                v.sort_unstable();
                v.dedup();
                v
            };
            let ts: Vec<u32> = {
                let mut v: Vec<u32> = w.planes.iter().map(|p| p.2).collect();
                v.sort_unstable();
                v.dedup();
                v
            };
            w2.planes = w
                .planes
                .iter()
                .map(|&(c, z, t)| {
                    (
                        cpos(c),
                        zs.iter().position(|&x| x == z).unwrap_or(0) as u32,
                        ts.iter().position(|&x| x == t).unwrap_or(0) as u32,
                    )
                })
                .collect();
            w2
        })
        .collect();
    let vendor_json = if opts.embed_vendor {
        ds.vendor_metadata()
            .ok()
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::to_string(&v).ok())
            .filter(|s| s.len() < 8 << 20)
    } else {
        None
    };
    let xml =
        build_ome_xml(&info, &xml_images, CREATOR, vendor_json.as_deref()).map_err(Error::Other)?;

    let tmp = output.with_file_name(format!(
        ".{}.partial-{}",
        output
            .file_name()
            .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string()),
        std::process::id()
    ));
    let result = (|| -> Result<(u64, Vec<String>)> {
        let f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let mut enc = TiffEncoder::new_big(BufWriter::new(f)).map_err(|e| tiff_err(e, &tmp))?;
        let mut hashes = Vec::new();
        let mut first = true;
        let mut count = 0u64;
        let mut requests = Vec::new();
        let mut plane_bytes = 0u64;
        for w in &written {
            plane_bytes = plane_bytes.max(
                u64::from(w.info.size_x)
                    * u64::from(w.info.size_y)
                    * u64::from(w.info.samples_per_pixel.max(1))
                    * w.info.pixel_type.bytes_per_sample() as u64,
            );
            for &(c, z, t) in &w.planes {
                requests.push(PlaneRequest {
                    image: w.info.index,
                    index: PlaneIndex { c, z, t },
                    level: 0,
                    region: None,
                });
            }
        }
        let total = requests.len() as u64;
        let format_id: &'static str = Box::leak(info.format.id.clone().into_boxed_str());
        read_in_order(
            ds,
            ctx,
            &requests,
            plane_bytes,
            &|r, plane| {
                if plane.data.len() != plane.expected_len() {
                    let PlaneIndex { c, z, t } = r.index;
                    return Err(Error::corrupt(
                        format_id,
                        format!(
                            "plane c={c} z={z} t={t} has {} bytes, expected {}",
                            plane.data.len(),
                            plane.expected_len()
                        ),
                    ));
                }
                let h = plane.xxh3_hex();
                Ok((encode_plane(plane, opts.codec)?, h))
            },
            &mut |_, (plane, h)| {
                hashes.push(h);
                write_plane(
                    &mut enc,
                    &plane,
                    opts.codec,
                    first.then_some(xml.as_str()),
                    &tmp,
                )?;
                first = false;
                count += 1;
                ctx.report(count, total);
                Ok(())
            },
        )?;
        drop(enc);
        Ok((count, hashes))
    })();
    let (planes_written, source_hashes) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    let back = match read_back_hashes(&tmp, source_hashes.len(), max_plane) {
        Ok(h) => h,
        Err(e) => {
            if std::env::var_os("OPENREADOUT_KEEP_PARTIAL").is_none() {
                let _ = std::fs::remove_file(&tmp);
            }
            return Err(e);
        }
    };
    let verified = back == source_hashes;
    if !verified {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::Other(format!(
            "read-back verification failed: {} of {} planes differ after writing; output discarded",
            back.iter()
                .zip(&source_hashes)
                .filter(|(a, b)| a != b)
                .count()
                + back.len().abs_diff(source_hashes.len()),
            source_hashes.len()
        )));
    }
    let bytes_written = std::fs::metadata(&tmp).map_or(0, |m| m.len());
    if output.exists() {
        std::fs::remove_file(output).map_err(|e| Error::io(output, e))?;
    }
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    Ok(ExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: "ome-tiff".into(),
        images_written: written.len() as u32,
        planes_written,
        bytes_written,
        verified,
        codec: opts.codec,
        ome_xml_bytes: xml.len() as u64,
        resolutions: Vec::new(),
        layout: None,
        images_skipped: Vec::new(),
    })
}

/// The tiled path of [`export_ome_tiff_with`]: temporary file, stream, verify, rename.
fn export_tiled(
    ds: &mut dyn Dataset,
    info: &openreadout_core::FileInfo,
    written: &[WrittenImage<'_>],
    input: &Path,
    output: &Path,
    opts: &ExportOptions,
    ctx: &ReadContext<'_>,
) -> Result<ExportReport> {
    let vendor_json = if opts.embed_vendor {
        ds.vendor_metadata()
            .ok()
            .filter(|v| !v.is_null())
            .and_then(|v| serde_json::to_string(&v).ok())
            .filter(|s| s.len() < 8 << 20)
    } else {
        None
    };
    let tmp = output.with_file_name(format!(
        ".{}.partial-{}",
        output
            .file_name()
            .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string()),
        std::process::id()
    ));
    let res = tiled_export::export(ds, info, written, &tmp, opts, ctx, vendor_json.as_deref());
    let (planes_written, bytes_written, resolutions) = match res {
        Ok(v) => v,
        Err(e) => {
            if std::env::var_os("OPENREADOUT_KEEP_PARTIAL").is_none() {
                let _ = std::fs::remove_file(&tmp);
            }
            return Err(e);
        }
    };
    if output.exists() {
        std::fs::remove_file(output).map_err(|e| Error::io(output, e))?;
    }
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    let xml_bytes = read_description_len(output);
    Ok(ExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: "ome-tiff".into(),
        images_written: written.len() as u32,
        planes_written,
        bytes_written,
        verified: true,
        codec: opts.codec,
        ome_xml_bytes: xml_bytes,
        resolutions,
        layout: None,
        images_skipped: Vec::new(),
    })
}

/// Length of the first IFD's `ImageDescription` (the OME-XML) of a TIFF, or 0.
fn read_description_len(path: &Path) -> u64 {
    use tiff::decoder::Decoder;
    let Ok(f) = File::open(path) else {
        return 0;
    };
    Decoder::new(std::io::BufReader::new(f))
        .ok()
        .and_then(|mut d| d.get_tag_ascii_string(Tag::ImageDescription).ok())
        .map_or(0, |s| s.len() as u64)
}
