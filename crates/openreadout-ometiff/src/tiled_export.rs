//! Tiled (and pyramidal) OME-TIFF export: used when a pyramid is written, a region or a
//! downsampled level is exported, or a plane is too large to hold whole. Every plane is streamed
//! block by block ([`crate::pyramid::stream_plane`]), written as 512 × 512 (`tile`) tiles with
//! its reduced levels in `SubIFDs`, then the file is read back with OpenReadout's TIFF reader
//! and every block of every level is hash-compared with what was written.

use std::path::Path;

use openreadout_core::model::{FileInfo, ImageInfo};
use openreadout_core::parallel::ReadContext;
use openreadout_core::region::levels_of;
use openreadout_core::source::Input;
use openreadout_core::{Dataset, Error, FormatReader, PlaneIndex, Region, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::pyramid::{BlockHash, PyramidMode, exported_info, plan, stream_plane};
use crate::tiled::TiledTiff;
use crate::{CREATOR, ExportOptions, WrittenImage, build_ome_xml, layout_of};

/// Resolution levels written for one image by a tiled export.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WrittenLevels {
    /// Source image index.
    pub image: u32,
    /// Source pyramid level the exported full resolution was read from.
    pub source_level: u32,
    /// Rectangle of that level that was exported (absent: all of it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<Region>,
    /// Where the lower levels came from: `source` (copied), `mean` (2 x 2 means) or `none`.
    pub pyramid: PyramidMode,
    /// `[width, height]` of every level written, full resolution first.
    pub sizes: Vec<[u32; 2]>,
    /// Downsampling factor of every level relative to the exported full resolution (x).
    pub factors: Vec<f64>,
}

/// True when the export of this image needs the tiled writer.
pub(crate) fn needs_tiled(im: &ImageInfo, opts: &ExportOptions) -> bool {
    let whole = u64::from(im.size_x)
        * u64::from(im.size_y)
        * u64::from(im.samples_per_pixel.max(1))
        * im.pixel_type.bytes_per_sample() as u64;
    opts.level > 0
        || opts.region.is_some()
        || matches!(opts.pyramid, PyramidMode::Source | PyramidMode::Mean)
        || (opts.pyramid == PyramidMode::Auto && levels_of(im).len() > 1)
        || whole > openreadout_core::pixel::MAX_PLANE_BYTES
}

/// The pyramid mode `Auto` stands for, for an OME-TIFF of this image.
pub(crate) fn resolve_mode(im: &ImageInfo, opts: &ExportOptions) -> PyramidMode {
    match opts.pyramid {
        PyramidMode::Auto
            if levels_of(im).len() > 1 && opts.level == 0 && opts.region.is_none() =>
        {
            PyramidMode::Source
        }
        PyramidMode::Auto => PyramidMode::None,
        other => other,
    }
}

/// One plane written: output image, its (c, z, t) in the exported index space, block hashes.
struct Written {
    image: u32,
    index: PlaneIndex,
    blocks: Vec<BlockHash>,
}

/// Export with the tiled writer. `written` are the images and selected planes as the strip
/// writer would write them (their `info` is the source's).
#[allow(clippy::too_many_lines)]
pub(crate) fn export(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    written: &[WrittenImage<'_>],
    output_tmp: &Path,
    opts: &ExportOptions,
    ctx: &ReadContext<'_>,
    vendor_json: Option<&str>,
) -> Result<(u64, u64, Vec<WrittenLevels>)> {
    let mut plans = Vec::new();
    for w in written {
        let mode = resolve_mode(w.info, opts);
        let p = plan(
            w.info,
            opts.level,
            opts.region,
            mode,
            opts.levels,
            opts.tile,
        )?;
        plans.push(p);
    }
    let first_layout = layout_of(
        written[0].info.pixel_type,
        written[0].info.samples_per_pixel,
    )?;
    for w in written {
        let l = layout_of(w.info.pixel_type, w.info.samples_per_pixel)?;
        if (l.bits, l.format, l.photometric)
            != (
                first_layout.bits,
                first_layout.format,
                first_layout.photometric,
            )
        {
            return Err(Error::unsupported(
                "ome-tiff",
                "a tiled export of images with different sample types",
                "Export one image at a time (--image N).",
            ));
        }
    }
    let infos: Vec<ImageInfo> = written
        .iter()
        .zip(&plans)
        .map(|(w, p)| exported_info(w.info, p))
        .collect();
    // OME-XML: the same plane re-indexing as the strip writer, on the exported geometry.
    let xml_images: Vec<WrittenImage<'_>> = written
        .iter()
        .zip(&infos)
        .map(|(w, im)| {
            let mut w2 = w.clone();
            w2.info = im;
            w2.source_planes.clone_from(&w.planes);
            w2.planes = reindex(&w.planes, &w.c_map);
            w2
        })
        .collect();
    let xml = build_ome_xml(info, &xml_images, CREATOR, vendor_json).map_err(Error::Other)?;
    let bps = first_layout.bits as usize / 8;
    let spp = written[0].info.samples_per_pixel.max(1);
    let mut tw = TiledTiff::create(
        output_tmp,
        opts.tile,
        opts.codec,
        first_layout,
        spp as u16,
        bps,
    )?;
    let total: u64 = written.iter().map(|w| w.planes.len() as u64).sum();
    let mut done = 0u64;
    let mut records = Vec::new();
    let mut first = true;
    for (oi, ((w, p), xw)) in written.iter().zip(&plans).zip(&xml_images).enumerate() {
        let sizes: Vec<(u32, u32)> = p.levels.iter().map(|l| l.size).collect();
        for (k, &(c, z, t)) in w.planes.iter().enumerate() {
            tw.begin_plane(&sizes);
            let blocks = stream_plane(
                ds,
                p,
                PlaneIndex { c, z, t },
                w.info.pixel_type,
                w.info.samples_per_pixel,
                &mut tw,
            )?;
            tw.end_plane(first.then_some(xml.as_str()), CREATOR)?;
            first = false;
            let (ci, zi, ti) = xw.planes[k];
            records.push(Written {
                image: oi as u32,
                index: PlaneIndex {
                    c: ci,
                    z: zi,
                    t: ti,
                },
                blocks,
            });
            done += 1;
            ctx.report(done, total);
        }
    }
    let bytes = tw.finish()?;
    verify(output_tmp, &records)?;
    let levels = written
        .iter()
        .zip(&plans)
        .map(|(w, p)| WrittenLevels {
            image: w.info.index,
            source_level: p.base_level,
            region: opts.region,
            pyramid: if p.levels.len() > 1 {
                p.mode
            } else {
                PyramidMode::None
            },
            sizes: p.levels.iter().map(|l| [l.size.0, l.size.1]).collect(),
            factors: p.levels.iter().map(|l| l.factor.0).collect(),
        })
        .collect();
    Ok((done, bytes, levels))
}

/// (c, z, t) of the selected planes in the exported image's own index space.
fn reindex(planes: &[(u32, u32, u32)], c_map: &[u32]) -> Vec<(u32, u32, u32)> {
    let axis = |f: fn(&(u32, u32, u32)) -> u32| {
        let mut v: Vec<u32> = planes.iter().map(f).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let (zs, ts) = (axis(|p| p.1), axis(|p| p.2));
    let pos = |v: &[u32], x: u32| v.iter().position(|&y| y == x).unwrap_or(0) as u32;
    planes
        .iter()
        .map(|&(c, z, t)| (pos(c_map, c), pos(&zs, z), pos(&ts, t)))
        .collect()
}

/// Read every written block back through the TIFF reader (OME-XML, `SubIFDs`, tile tables and
/// compression as another program would meet them) and compare hashes.
fn verify(path: &Path, records: &[Written]) -> Result<()> {
    let reader = openreadout_tiff::TiffReader;
    let mut back = reader.open_input(&Input::local(path))?;
    let mut bad = Vec::new();
    let mut n = 0usize;
    for r in records {
        for b in &r.blocks {
            n += 1;
            let got = back.read_region(r.image, r.index, b.level as u32, b.region);
            match got {
                Ok(p) if p.xxh3_hex() == b.xxh3 => {}
                Ok(_) => bad.push(format!(
                    "image {} c={} z={} t={} level {} block {}: pixels differ",
                    r.image, r.index.c, r.index.z, r.index.t, b.level, b.region
                )),
                Err(e) => bad.push(format!(
                    "image {} c={} z={} t={} level {} block {}: {e}",
                    r.image, r.index.c, r.index.z, r.index.t, b.level, b.region
                )),
            }
            if bad.len() > 5 {
                break;
            }
        }
    }
    if !bad.is_empty() {
        return Err(Error::Other(format!(
            "read-back verification failed ({} of {n} blocks checked so far differ); output discarded: {}",
            bad.len(),
            bad.join("; ")
        )));
    }
    Ok(())
}
