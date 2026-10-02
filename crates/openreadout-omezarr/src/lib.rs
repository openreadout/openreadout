//! OME-Zarr writer: OME-NGFF 0.5 on Zarr v3 (via the pure-Rust `zarrs` crate).
//!
//! Every image becomes a `multiscales` group with 5-D arrays in `t, c, z, y, x` order, one
//! array per resolution level (2×2 mean pyramid), plus `omero` channel display metadata.
//! A single exported image is written at the root of the store; several images become a
//! `bioformats2raw.layout` collection (`0/`, `1/`, ... plus an `OME/` group listing the
//! series and holding `METADATA.ome.xml`). A multi-well plate (a high-content screening
//! plate, [`openreadout_core::plate`]) becomes an OME-NGFF HCS plate: `<row>/<column>/<field>`
//! image groups under `plate` and `well` metadata.
//!
//! The store is written to a temporary directory next to the output, every level-0 plane is
//! read back through `zarrs` and hash-compared with the source plane, and only then is the
//! directory renamed into place. The source file is never opened for writing.
//!
//! OME-Zarr (OME-NGFF) is the cloud-friendly successor of OME-TIFF: a directory of small
//! compressed chunks plus JSON metadata, readable by napari, `ome-zarr-py`, bioio and web
//! viewers.
//!
//! # Example
//!
//! ```
//! use std::path::Path;
//!
//! use openreadout_core::{Dataset, Result};
//! use openreadout_omezarr::{ZarrExportOptions, default_output, export_ome_zarr};
//!
//! /// Write an opened file next to its input as `<stem>.ome.zarr`, with 256-pixel chunks.
//! fn to_ome_zarr(dataset: &mut dyn Dataset, input: &Path) -> Result<()> {
//!     let mut options = ZarrExportOptions::default();
//!     options.chunk = 256;
//!     let report = export_ome_zarr(dataset, input, &default_output(input), &options)?;
//!     println!("{} planes in {}", report.planes_written, report.output);
//!     Ok(())
//! }
//! ```
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod ngff;
mod pixels;
mod tiled;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::model::{FileInfo, ImageInfo};
use openreadout_core::parallel::{PlaneRequest, ReadContext, read_in_order};
use openreadout_core::plate::PlateSummary;
use openreadout_core::reader::PlaneIndex;
use openreadout_core::select::Selection;
use openreadout_core::{Dataset, Error, PixelType, Result};
pub use openreadout_ometiff::PyramidMode;
use openreadout_ometiff::{Codec, ExportReport, WrittenImage, build_ome_xml_metadata_only};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use zarrs::array::codec::GzipCodec;
use zarrs::array::{
    Array, ArrayBuilder, ArrayBytes, ArrayMetadataOptions, ArraySubset, BytesToBytesCodecTraits,
    DataType, data_type,
};
use zarrs::filesystem::FilesystemStore;
use zarrs::group::GroupBuilder;

use crate::ngff::ChannelRange;

/// Default chunk edge (y and x) in pixels.
pub const DEFAULT_CHUNK: u32 = 512;
/// gzip level used for the `deflate` codec.
const GZIP_LEVEL: u32 = 5;

const CREATOR: &str = concat!("openreadout ", env!("CARGO_PKG_VERSION"));

/// OME-Zarr export options.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[non_exhaustive]
pub struct ZarrExportOptions {
    /// Only this image index (default: all).
    pub image: Option<u32>,
    /// Plane selection strings (`c=0`, `z=1-3`, `t=0,2`).
    pub select: Vec<String>,
    /// `none` or `deflate` (stored with the Zarr `gzip` codec). `lzw` is rejected.
    pub codec: Codec,
    /// Replace an existing output directory.
    pub overwrite: bool,
    /// Embed the vendor metadata tree (as JSON) in `OME/METADATA.ome.xml` (collections only).
    pub embed_vendor: bool,
    /// Chunk edge in pixels for y and x (chunks are `1×1×1×chunk×chunk`).
    pub chunk: u32,
    /// Resolution levels including full resolution. `None`: add 2× levels while a level is
    /// larger than 1024 px in either dimension (mean pyramids; source pyramids: all of them).
    pub levels: Option<u32>,
    /// Export this source pyramid level as the full resolution (0 = the source's full
    /// resolution).
    pub level: u32,
    /// Export only this rectangle of every plane, in the pixel coordinates of `level`.
    pub region: Option<openreadout_core::Region>,
    /// Where the lower levels come from. `Auto`: the source's own pyramid when it has one and
    /// the whole image is exported from level 0, else 2 × 2 means.
    pub pyramid: PyramidMode,
    /// Multi-well plates: write the OME-NGFF HCS layout (`plate`, `well` and field groups)
    /// when every image is exported. `false` writes a `bioformats2raw.layout` collection.
    pub plate: bool,
    /// Multi-well plates: only the images of these wells (`C05`, `c5`); empty = every well.
    pub wells: Vec<String>,
    /// Leave out images whose selected planes include missing files (a partial copy of a
    /// plate) instead of failing; they are listed in `images_skipped`.
    pub skip_incomplete: bool,
}

impl Default for ZarrExportOptions {
    fn default() -> Self {
        ZarrExportOptions {
            image: None,
            select: Vec::new(),
            codec: Codec::Deflate,
            overwrite: false,
            embed_vendor: false,
            chunk: DEFAULT_CHUNK,
            levels: None,
            level: 0,
            region: None,
            pyramid: PyramidMode::Auto,
            plate: true,
            wells: Vec::new(),
            skip_incomplete: false,
        }
    }
}

fn zerr(what: &str, e: impl std::fmt::Display) -> Error {
    Error::Other(format!("OME-Zarr {what}: {e}"))
}

fn zarr_data_type(p: PixelType) -> Result<DataType> {
    Ok(match p {
        PixelType::Int8 => data_type::int8(),
        PixelType::Int16 => data_type::int16(),
        PixelType::Int32 => data_type::int32(),
        PixelType::Uint8 => data_type::uint8(),
        PixelType::Uint16 => data_type::uint16(),
        PixelType::Uint32 => data_type::uint32(),
        PixelType::Float => data_type::float32(),
        PixelType::Double => data_type::float64(),
        other => return Err(unsupported_pixel_type(other)),
    })
}

fn unsupported_pixel_type(p: PixelType) -> Error {
    Error::unsupported(
        "ome-zarr export",
        format!("pixel type {}", p.ome_name()),
        "This writer predates the pixel type; export to OME-TIFF or update openreadout.",
    )
}

fn zarr_array_builder(
    shape: Vec<u64>,
    chunks: Vec<u64>,
    p: PixelType,
    codec: Codec,
) -> Result<ArrayBuilder> {
    let mut b = match p {
        PixelType::Int8 => ArrayBuilder::new(shape, chunks, zarr_data_type(p)?, 0i8),
        PixelType::Int16 => ArrayBuilder::new(shape, chunks, zarr_data_type(p)?, 0i16),
        PixelType::Int32 => ArrayBuilder::new(shape, chunks, zarr_data_type(p)?, 0i32),
        PixelType::Uint8 => ArrayBuilder::new(shape, chunks, zarr_data_type(p)?, 0u8),
        PixelType::Uint16 => ArrayBuilder::new(shape, chunks, zarr_data_type(p)?, 0u16),
        PixelType::Uint32 => ArrayBuilder::new(shape, chunks, zarr_data_type(p)?, 0u32),
        PixelType::Float => ArrayBuilder::new(shape, chunks, zarr_data_type(p)?, 0.0f32),
        PixelType::Double => ArrayBuilder::new(shape, chunks, zarr_data_type(p)?, 0.0f64),
        other => return Err(unsupported_pixel_type(other)),
    };
    b.dimension_names(Some(ngff::AXES));
    match codec {
        Codec::None => {}
        Codec::Deflate => {
            let gz: Arc<dyn BytesToBytesCodecTraits> =
                Arc::new(GzipCodec::new(GZIP_LEVEL).map_err(|e| zerr("gzip codec", e))?);
            b.bytes_to_bytes_codecs(vec![gz]);
        }
        // `Lzw`, and any codec added to the OME-TIFF writer later.
        other => {
            return Err(Error::Usage(format!(
                "OME-Zarr export supports compression none or deflate (written as the Zarr gzip codec), not {}",
                format!("{other:?}").to_lowercase()
            )));
        }
    }
    Ok(b)
}

/// Plane bytes are little-endian; `zarrs` array bytes are native-endian. Swapping is its
/// own inverse, so this also converts native bytes back to little-endian.
fn le_to_native(data: &[u8], bytes_per_sample: usize) -> std::borrow::Cow<'_, [u8]> {
    if cfg!(target_endian = "big") && bytes_per_sample > 1 {
        let mut v = data.to_vec();
        for s in v.chunks_exact_mut(bytes_per_sample) {
            s.reverse();
        }
        std::borrow::Cow::Owned(v)
    } else {
        std::borrow::Cow::Borrowed(data)
    }
}

/// One image as it is laid out in the store.
struct Plan<'a> {
    info: &'a ImageInfo,
    /// Group path inside the store (`/` or `/<n>`).
    group: String,
    c_map: Vec<u32>,
    z_map: Vec<u32>,
    t_map: Vec<u32>,
    levels: Vec<(u32, u32)>,
    /// Plate layout: (row name, column name) of the image's well.
    well: Option<(String, String)>,
}

impl Plan<'_> {
    fn spp(&self) -> usize {
        self.info.samples_per_pixel.max(1) as usize
    }
    fn zarr_c(&self) -> usize {
        self.c_map.len() * self.spp()
    }
    fn array_path(&self, level: usize) -> String {
        if self.group == "/" {
            format!("/{level}")
        } else {
            format!("{}/{level}", self.group)
        }
    }
}

/// The plans, and the images left out because their selected planes include missing files.
fn plans<'a>(
    info: &'a FileInfo,
    opts: &ZarrExportOptions,
    layout: Option<&PlateSummary>,
) -> Result<(Vec<Plan<'a>>, Vec<u32>)> {
    let sel = Selection::parse(&opts.select)?;
    // images of the requested wells, and each image's well
    let mut well_of: std::collections::HashMap<u32, (String, String)> =
        std::collections::HashMap::new();
    if let Some(l) = layout {
        for w in &l.wells {
            for &i in &w.images {
                well_of.insert(i, (w.row.clone(), w.column.to_string()));
            }
        }
    }
    let mut wanted_wells = std::collections::HashSet::new();
    for w in &opts.wells {
        let Some(l) = layout else {
            return Err(Error::Usage(
                "--well applies to multi-well plates; this file is not one".into(),
            ));
        };
        let pw = l
            .well(w)
            .ok_or_else(|| Error::Usage(format!("well '{w}' was not imaged on this plate")))?;
        wanted_wells.extend(pw.images.iter().copied());
    }
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut incomplete = Vec::new();
    for im in &info.images {
        if opts.image.is_some_and(|i| i != im.index) {
            continue;
        }
        if !opts.wells.is_empty() && !wanted_wells.contains(&im.index) {
            continue;
        }
        let (missing, all_missing) = openreadout_core::plate::missing_planes(im);
        let needs_missing = all_missing || missing.iter().any(|&(c, z, t)| sel.contains(c, z, t));
        if needs_missing {
            if opts.skip_incomplete {
                skipped.push(im.index);
                continue;
            }
            incomplete.push(im.index);
        }
        // Per-plane lists below are sized by the declared dimensions; refuse absurd ones.
        openreadout_core::limits::checked_plane_count("ome-zarr", im.size_c, im.size_z, im.size_t)?;
        let pick = |n: u32, s: &[u32]| -> Vec<u32> {
            (0..n).filter(|v| s.is_empty() || s.contains(v)).collect()
        };
        let c_map = pick(im.size_c, &sel.c);
        let z_map = pick(im.size_z, &sel.z);
        let t_map = pick(im.size_t, &sel.t);
        if c_map.is_empty() || z_map.is_empty() || t_map.is_empty() {
            continue;
        }
        // Fail before writing anything when a plane could not be held in memory (the
        // streaming writer, used for such planes, reads them block by block).
        if !tiled::needs_tiled(im, opts) {
            openreadout_core::pixel::plane_bytes_checked(
                "ome-zarr",
                im.size_x,
                im.size_y,
                im.samples_per_pixel.max(1) as usize * im.pixel_type.bytes_per_sample(),
            )?;
        }
        out.push(Plan {
            info: im,
            group: String::new(),
            c_map,
            z_map,
            t_map,
            levels: ngff::plan_levels(
                im.size_x,
                im.size_y,
                if opts.pyramid == PyramidMode::None {
                    Some(1)
                } else {
                    opts.levels
                },
            ),
            well: well_of.get(&im.index).cloned(),
        });
    }
    if !incomplete.is_empty() {
        let names: Vec<String> = incomplete
            .iter()
            .take(6)
            .map(|i| {
                info.images
                    .iter()
                    .find(|im| im.index == *i)
                    .and_then(|im| im.name.clone())
                    .unwrap_or_else(|| format!("image {i}"))
            })
            .collect();
        return Err(Error::Unsupported {
            format: "ome-zarr export",
            feature: format!(
                "{} image(s) whose plane files are missing ({}{})",
                incomplete.len(),
                names.join(", "),
                if incomplete.len() > 6 { ", ..." } else { "" }
            ),
            hint: Some(
                "This copy of the plate is incomplete (`openreadout check` lists the missing files). Pass --skip-incomplete to export only the complete fields, or choose wells with --well.".into(),
            ),
        });
    }
    if out.is_empty() && !skipped.is_empty() {
        return Err(Error::Usage(format!(
            "every selected image has missing plane files ({} skipped); nothing to export",
            skipped.len()
        )));
    }
    if out.is_empty() {
        return Err(Error::Usage(match opts.image {
            Some(i) if !info.images.iter().any(|im| im.index == i) => format!(
                "image {i} does not exist (the file has {} images)",
                info.images.len()
            ),
            _ => "selection matches no planes".into(),
        }));
    }
    let plate_mode = opts.plate && opts.image.is_none() && out.iter().all(|p| p.well.is_some());
    if plate_mode {
        let mut k: std::collections::HashMap<(String, String), usize> =
            std::collections::HashMap::new();
        for p in &mut out {
            if let Some((r, c)) = &p.well {
                let n = k.entry((r.clone(), c.clone())).or_insert(0);
                p.group = format!("/{r}/{c}/{n}");
                *n += 1;
            }
        }
    } else {
        let single = out.len() == 1;
        for (n, p) in out.iter_mut().enumerate() {
            p.well = None;
            p.group = if single { "/".into() } else { format!("/{n}") };
        }
    }
    Ok((out, skipped))
}

/// Total size of the regular files under `dir`.
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    for e in rd.flatten() {
        match e.file_type() {
            Ok(t) if t.is_dir() => total += dir_size(&e.path()),
            Ok(t) if t.is_file() => total += e.metadata().map_or(0, |m| m.len()),
            _ => {}
        }
    }
    total
}

/// Refuse outputs that would clobber the input or something that is not a Zarr store.
fn check_output(input: &Path, output: &Path, overwrite: bool) -> Result<()> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let inp = canon(input);
    if output.exists() {
        let out = canon(output);
        if inp == out || inp.starts_with(&out) {
            return Err(Error::Usage(format!(
                "{} contains the input file; raw files are never modified",
                output.display()
            )));
        }
        if !overwrite {
            return Err(Error::Usage(format!(
                "{} exists; pass --overwrite to replace it",
                output.display()
            )));
        }
        if output.is_dir() && !output.join("zarr.json").is_file() {
            return Err(Error::Usage(format!(
                "{} is a directory that is not a Zarr v3 store (no zarr.json); refusing to replace it",
                output.display()
            )));
        }
    }
    Ok(())
}

/// Export `ds` (already opened from `input`) to an OME-Zarr store (a directory) at `output`.
pub fn export_ome_zarr(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &ZarrExportOptions,
) -> Result<ExportReport> {
    export_ome_zarr_with(ds, input, output, opts, &ReadContext::default())
}

/// [`export_ome_zarr`] with parallel plane decoding and progress reporting (planes are
/// decoded by worker threads and written in the same order whatever the thread count).
pub fn export_ome_zarr_with(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &ZarrExportOptions,
    ctx: &ReadContext<'_>,
) -> Result<ExportReport> {
    if opts.chunk == 0 {
        return Err(Error::Usage("chunk size must be at least 1".into()));
    }
    if opts.levels == Some(0) {
        return Err(Error::Usage(
            "levels must be at least 1 (1 = full resolution only)".into(),
        ));
    }
    if opts.codec == Codec::Lzw {
        zarr_array_builder(vec![1], vec![1], PixelType::Uint8, opts.codec)?;
    }
    check_output(input, output, opts.overwrite)?;
    let info = ds.info()?;
    let layout = openreadout_core::plate::plate_layout(ds, &info);
    let (plans, images_skipped) = plans(&info, opts, layout.as_ref())?;

    let tmp = output.with_file_name(format!(
        ".{}.partial-{}",
        output
            .file_name()
            .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string()),
        std::process::id()
    ));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).map_err(|e| Error::io(&tmp, e))?;
    }
    let keep_partial = std::env::var_os("OPENREADOUT_KEEP_PARTIAL").is_some();
    let streamed = plans.iter().any(|p| tiled::needs_tiled(p.info, opts));
    let result = if streamed {
        tiled::write_store_tiled(ds, &info, &plans, &tmp, opts, ctx, layout.as_ref())
    } else {
        write_store(ds, &info, &plans, &tmp, opts, ctx, layout.as_ref()).and_then(
            |(planes, hashes, xml)| {
                verify(&tmp, &plans, &hashes)?;
                Ok((planes, xml, Vec::new()))
            },
        )
    };
    let (planes_written, ome_xml_bytes, resolutions) = match result {
        Ok(v) => v,
        Err(e) => {
            if !keep_partial {
                let _ = std::fs::remove_dir_all(&tmp);
            }
            return Err(e);
        }
    };
    let bytes_written = dir_size(&tmp);
    if output.exists() {
        if output.is_dir() {
            std::fs::remove_dir_all(output).map_err(|e| Error::io(output, e))?;
        } else {
            std::fs::remove_file(output).map_err(|e| Error::io(output, e))?;
        }
    }
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    Ok(ExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: "ome-zarr".into(),
        images_written: plans.len() as u32,
        planes_written,
        bytes_written,
        verified: true,
        codec: opts.codec,
        ome_xml_bytes,
        resolutions,
        layout: plans
            .first()
            .is_some_and(|p| p.well.is_some())
            .then(|| "plate".to_string()),
        images_skipped,
    })
}

/// Source hash of one exported plane, keyed by its position in the store.
struct PlaneRecord {
    image: usize,
    t: u64,
    c: u64,
    z: u64,
    xxh3: String,
}

type Written = (u64, Vec<PlaneRecord>, u64);

fn write_store(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    plans: &[Plan<'_>],
    dir: &Path,
    opts: &ZarrExportOptions,
    ctx: &ReadContext<'_>,
    layout: Option<&PlateSummary>,
) -> Result<Written> {
    let total: u64 = plans
        .iter()
        .map(|p| (p.t_map.len() * p.z_map.len() * p.c_map.len()) as u64)
        .sum();
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let store = Arc::new(FilesystemStore::new(dir).map_err(|e| zerr("store", e))?);
    let meta_opts = ArrayMetadataOptions::default().with_include_zarrs_metadata(false);
    let mut records = Vec::new();
    let mut count = 0u64;
    let format_id: &'static str = match info.format.id.as_str() {
        "czi" => "czi",
        "nd2" => "nd2",
        "lif" => "lif",
        _ => "source",
    };

    for (pi, plan) in plans.iter().enumerate() {
        let im = plan.info;
        let spp = plan.spp();
        let bps = im.pixel_type.bytes_per_sample();
        let arrays: Vec<Array<FilesystemStore>> = plan
            .levels
            .iter()
            .enumerate()
            .map(|(l, &(w, h))| {
                let shape = vec![
                    plan.t_map.len() as u64,
                    plan.zarr_c() as u64,
                    plan.z_map.len() as u64,
                    u64::from(h),
                    u64::from(w),
                ];
                let chunks = vec![
                    1,
                    1,
                    1,
                    u64::from(opts.chunk.min(h).max(1)),
                    u64::from(opts.chunk.min(w).max(1)),
                ];
                let a = zarr_array_builder(shape, chunks, im.pixel_type, opts.codec)?
                    .build(store.clone(), &plan.array_path(l))
                    .map_err(|e| zerr("array", e))?;
                a.store_metadata_opt(&meta_opts)
                    .map_err(|e| zerr("array metadata", e))?;
                Ok(a)
            })
            .collect::<Result<_>>()?;
        let mut ranges = vec![ChannelRange::EMPTY; plan.zarr_c()];
        // (ti, zi, ci) of each request, in the store's t, z, c order.
        let mut slots = Vec::new();
        let mut requests = Vec::new();
        for (ti, &t) in plan.t_map.iter().enumerate() {
            for (zi, &z) in plan.z_map.iter().enumerate() {
                for (ci, &c) in plan.c_map.iter().enumerate() {
                    slots.push((ti, zi, ci));
                    requests.push(PlaneRequest {
                        image: im.index,
                        index: PlaneIndex { c, z, t },
                        level: 0,
                        region: None,
                    });
                }
            }
        }
        let plane_bytes = u64::from(im.size_x)
            * u64::from(im.size_y)
            * spp as u64
            * im.pixel_type.bytes_per_sample() as u64;
        read_in_order(
            ds,
            ctx,
            &requests,
            plane_bytes,
            &|r, plane| {
                let PlaneIndex { c, z, t } = r.index;
                if plane.data.len() != plane.expected_len()
                    || plane.width != im.size_x
                    || plane.height != im.size_y
                    || plane.pixel_type != im.pixel_type
                    || plane.samples_per_pixel.max(1) as usize != spp
                {
                    return Err(Error::corrupt(
                        format_id,
                        format!(
                            "image {} plane c={c} z={z} t={t} is {}x{} {} x{} ({} bytes), expected {}x{} {} x{}",
                            im.index,
                            plane.width,
                            plane.height,
                            plane.pixel_type.ome_name(),
                            plane.samples_per_pixel,
                            plane.data.len(),
                            im.size_x,
                            im.size_y,
                            im.pixel_type.ome_name(),
                            spp
                        ),
                    ));
                }
                let h = plane.xxh3_hex();
                Ok((plane, h))
            },
            &mut |i, (plane, xxh3)| {
                let (ti, zi, ci) = slots[i];
                records.push(PlaneRecord {
                    image: pi,
                    t: ti as u64,
                    c: (ci * spp) as u64,
                    z: zi as u64,
                    xxh3,
                });
                for (s, mut data) in pixels::deinterleave(&plane.data, spp, bps)
                    .into_iter()
                    .enumerate()
                {
                    let zc = (ci * spp + s) as u64;
                    ranges[ci * spp + s] =
                        pixels::union(ranges[ci * spp + s], pixels::range(&data, im.pixel_type));
                    for (l, a) in arrays.iter().enumerate() {
                        let (w, h) = plan.levels[l];
                        if l > 0 {
                            let (pw, ph) = plan.levels[l - 1];
                            data = pixels::downsample_2x(&data, pw, ph, im.pixel_type);
                        }
                        let subset = ArraySubset::new_with_ranges(&[
                            ti as u64..ti as u64 + 1,
                            zc..zc + 1,
                            zi as u64..zi as u64 + 1,
                            0..u64::from(h),
                            0..u64::from(w),
                        ]);
                        a.store_array_subset(
                            &subset,
                            ArrayBytes::new_flen(le_to_native(&data, bps)),
                        )
                        .map_err(|e| zerr("chunk write", e))?;
                    }
                }
                count += 1;
                ctx.report(count, total);
                Ok(())
            },
        )?;
        let default_z = (plan.z_map.len() / 2) as u32;
        let attrs = ngff::image_attributes(
            ngff::multiscales(im, &plan.levels, CREATOR),
            ngff::omero(im, &plan.c_map, &ranges, default_z),
        );
        write_group(&store, &plan.group, attrs)?;
    }

    let xml_len = write_collection(ds, info, plans, dir, &store, opts, layout)?;
    Ok((count, records, xml_len))
}

/// The metadata of a multi-image store (collection root, `OME` group listing the series,
/// `OME/METADATA.ome.xml`); returns the XML's size (0 for single-image stores).
fn write_collection(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    plans: &[Plan<'_>],
    dir: &Path,
    store: &Arc<FilesystemStore>,
    opts: &ZarrExportOptions,
    layout: Option<&PlateSummary>,
) -> Result<u64> {
    let mut xml_len = 0u64;
    if plans.first().is_some_and(|p| p.well.is_some()) {
        write_plate_groups(store, plans, layout)?;
    } else if plans.len() > 1 {
        write_group(store, "/", ngff::collection_root_attributes())?;
        let series: Vec<String> = plans
            .iter()
            .map(|p| p.group.trim_start_matches('/').to_string())
            .collect();
        write_group(store, "/OME", ngff::series_attributes(&series))?;
        let written: Vec<WrittenImage<'_>> = plans
            .iter()
            .map(|p| {
                // Describe the selected planes (exported index space) and where each came
                // from, so the metadata document carries per-plane `Plane` elements.
                let mut planes = Vec::new();
                let mut source_planes = Vec::new();
                for (ti, &t) in p.t_map.iter().enumerate() {
                    for (zi, &z) in p.z_map.iter().enumerate() {
                        for (ci, &c) in p.c_map.iter().enumerate() {
                            planes.push((ci as u32, zi as u32, ti as u32));
                            source_planes.push((c, z, t));
                        }
                    }
                }
                WrittenImage {
                    info: p.info,
                    first_ifd: 0,
                    planes,
                    size_c: p.c_map.len() as u32,
                    size_z: p.z_map.len() as u32,
                    size_t: p.t_map.len() as u32,
                    c_map: p.c_map.clone(),
                    source_planes,
                    frames: ds
                        .frames(p.info.index, None)
                        .map(|(_, r)| r)
                        .unwrap_or_default(),
                }
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
        let xml = build_ome_xml_metadata_only(info, &written, CREATOR, vendor_json.as_deref())
            .map_err(Error::Other)?;
        let p = dir.join("OME").join("METADATA.ome.xml");
        std::fs::write(&p, xml.as_bytes()).map_err(|e| Error::io(&p, e))?;
        xml_len = xml.len() as u64;
    }
    Ok(xml_len)
}

/// The `plate` root, one group per row and the `well` groups of an HCS plate store.
fn write_plate_groups(
    store: &Arc<FilesystemStore>,
    plans: &[Plan<'_>],
    layout: Option<&PlateSummary>,
) -> Result<()> {
    // wells in plate order with their field paths
    let mut wells: std::collections::BTreeMap<(u32, u32), (String, String, Vec<String>)> =
        std::collections::BTreeMap::new();
    for p in plans {
        let Some((r, c)) = &p.well else { continue };
        let (ri, ci) = openreadout_core::plate::parse_well(&format!("{r}{c}"))
            .ok_or_else(|| Error::Other(format!("plate export: bad well {r}{c}")))?;
        let field = p.group.rsplit('/').next().unwrap_or("0").to_string();
        wells
            .entry((ri, ci))
            .or_insert_with(|| (r.clone(), c.clone(), Vec::new()))
            .2
            .push(field);
    }
    let rows = layout
        .map_or(0, |l| l.rows)
        .max(wells.keys().map(|k| k.0 + 1).max().unwrap_or(0));
    let cols = layout
        .map_or(0, |l| l.columns)
        .max(wells.keys().map(|k| k.1 + 1).max().unwrap_or(0));
    let row_names: Vec<String> = (0..rows).map(openreadout_core::plate::row_name).collect();
    let col_names: Vec<String> = (1..=cols).map(|c| c.to_string()).collect();
    let name = layout.and_then(|l| l.id.clone());
    let field_count = wells.values().map(|w| w.2.len()).max().unwrap_or(0);
    let attrs = ngff::plate_attributes(
        name.as_deref(),
        &row_names,
        &col_names,
        &wells
            .iter()
            .map(|(&(ri, ci), (r, c, _))| (format!("{r}/{c}"), ri, ci))
            .collect::<Vec<_>>(),
        field_count,
    );
    write_group(store, "/", attrs)?;
    let used_rows: std::collections::BTreeSet<&String> = wells.values().map(|w| &w.0).collect();
    for r in used_rows {
        write_group(store, &format!("/{r}"), serde_json::json!({}))?;
    }
    for (r, c, fields) in wells.values() {
        write_group(store, &format!("/{r}/{c}"), ngff::well_attributes(fields))?;
    }
    Ok(())
}

fn write_group(store: &Arc<FilesystemStore>, path: &str, attrs: serde_json::Value) -> Result<()> {
    let serde_json::Value::Object(map) = attrs else {
        return Err(Error::Other("group attributes must be an object".into()));
    };
    let g = GroupBuilder::new()
        .attributes(map)
        .build(store.clone(), path)
        .map_err(|e| zerr("group", e))?;
    g.store_metadata().map_err(|e| zerr("group metadata", e))
}

/// Re-open the written store from disk and compare every level-0 plane with its source hash.
fn verify(dir: &Path, plans: &[Plan<'_>], records: &[PlaneRecord]) -> Result<()> {
    let store = Arc::new(FilesystemStore::new(dir).map_err(|e| zerr("store", e))?);
    let arrays: Vec<Array<FilesystemStore>> = plans
        .iter()
        .map(|p| Array::open(store.clone(), &p.array_path(0)).map_err(|e| zerr("read-back", e)))
        .collect::<Result<_>>()?;
    let mut bad = 0usize;
    for r in records {
        let plan = &plans[r.image];
        let spp = plan.spp() as u64;
        let bps = plan.info.pixel_type.bytes_per_sample();
        let (w, h) = plan.levels[0];
        let mut samples = Vec::with_capacity(spp as usize);
        for s in 0..spp {
            let subset = ArraySubset::new_with_ranges(&[
                r.t..r.t + 1,
                r.c + s..r.c + s + 1,
                r.z..r.z + 1,
                0..u64::from(h),
                0..u64::from(w),
            ]);
            let bytes: ArrayBytes<'static> = arrays[r.image]
                .retrieve_array_subset(&subset)
                .map_err(|e| zerr("read-back", e))?;
            let raw = bytes.into_fixed().map_err(|e| zerr("read-back", e))?;
            samples.push(le_to_native(&raw, bps).into_owned());
        }
        let data = pixels::interleave(&samples, bps);
        if format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&data)) != r.xxh3 {
            bad += 1;
        }
    }
    if bad > 0 {
        return Err(Error::Other(format!(
            "read-back verification failed: {bad} of {} planes differ after writing; output discarded",
            records.len()
        )));
    }
    Ok(())
}

/// Default output path for `input`: same directory, `<stem>.ome.zarr`.
pub fn default_output(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
    input.with_file_name(format!("{stem}.ome.zarr"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openreadout_core::model::{CheckReport, FormatDescriptor, LsEntry};
    use openreadout_core::{Confidence, Plane, ProvenanceMap};

    struct Ramp(ImageInfo);

    impl Dataset for Ramp {
        fn info(&self) -> Result<FileInfo> {
            Ok(FileInfo {
                path: "ramp".into(),
                size_bytes: 0,
                format: FormatDescriptor {
                    id: "ramp".into(),
                    name: "Ramp".into(),
                    vendor: String::new(),
                    extensions: vec![],
                    family: "microscopy".into(),
                    can_read: true,
                    can_write: false,
                    confidence: Confidence::Low,
                    known_gaps: vec![],
                },
                format_version: None,
                images: vec![self.0.clone()],
                tables: vec![],
                spectra: vec![],
                traces: Vec::new(),
                plane_count: 1,
                notes: vec![],
            })
        }
        fn vendor_metadata(&self) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn provenance(&self) -> ProvenanceMap {
            ProvenanceMap::new()
        }
        fn entries(&self) -> Result<Vec<LsEntry>> {
            Ok(vec![])
        }
        fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
            Ok(Plane {
                width: 4,
                height: 2,
                pixel_type: PixelType::Uint8,
                samples_per_pixel: 1,
                data: (1..=8).collect(),
            })
        }
        fn check(&mut self) -> Result<CheckReport> {
            Ok(CheckReport::new("ramp", "ramp"))
        }
    }

    #[test]
    fn verification_detects_a_changed_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("s");
        let mut ds = Ramp(ImageInfo::new(0, 4, 2, PixelType::Uint8).finish());
        let info = ds.info().unwrap();
        let opts = ZarrExportOptions {
            codec: Codec::None,
            ..Default::default()
        };
        let (plans, _) = plans(&info, &opts, None).unwrap();
        let (n, records, _) = write_store(
            &mut ds,
            &info,
            &plans,
            &store,
            &opts,
            &ReadContext::default(),
            None,
        )
        .unwrap();
        assert_eq!(n, 1);
        verify(&store, &plans, &records).unwrap();
        // flip one sample in the (uncompressed) level-0 chunk
        let chunk = store.join("0/c/0/0/0/0/0");
        let mut bytes = std::fs::read(&chunk).unwrap();
        bytes[3] ^= 0xFF;
        std::fs::write(&chunk, bytes).unwrap();
        let e = verify(&store, &plans, &records).unwrap_err();
        assert!(e.to_string().contains("1 of 1 planes differ"), "{e}");
    }
}
