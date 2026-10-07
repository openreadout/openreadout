//! Streaming OME-Zarr export: every level of every plane is written chunk-aligned block by block
//! ([`openreadout_ometiff::pyramid::stream_plane`]), so images of any size (a stitched slide
//! above 4 GiB, a region, a downsampled level) are exported in bounded memory, with the source's
//! own pyramid copied or 2 × 2 mean levels computed on the way. After writing, every block of
//! every level is read back through `zarrs` and hash-compared.

use std::path::Path;
use std::sync::Arc;

use openreadout_core::model::{FileInfo, ImageInfo};
use openreadout_core::parallel::ReadContext;
use openreadout_core::region::levels_of;
use openreadout_core::{Dataset, Error, Plane, PlaneIndex, Result};
use openreadout_ometiff::WrittenLevels;
use openreadout_ometiff::pyramid::{
    BlockHash, BlockSink, ExportPlan, PyramidMode, exported_info, plan, stream_plane,
};
use zarrs::array::{Array, ArrayBytes, ArrayMetadataOptions, ArraySubset};
use zarrs::filesystem::FilesystemStore;

use crate::ngff::{self, ChannelRange};
use crate::{
    CREATOR, Plan, ZarrExportOptions, le_to_native, pixels, write_collection, write_group,
    zarr_array_builder, zerr,
};

/// The pyramid mode `Auto` stands for, for an OME-Zarr of this image.
pub(crate) fn resolve_mode(im: &ImageInfo, opts: &ZarrExportOptions) -> PyramidMode {
    match opts.pyramid {
        PyramidMode::Auto
            if levels_of(im).len() > 1 && opts.level == 0 && opts.region.is_none() =>
        {
            PyramidMode::Source
        }
        PyramidMode::Auto => PyramidMode::Mean,
        other => other,
    }
}

/// True when this image goes through the streaming writer: a downsampled level, a region, the
/// source's own pyramid, or a plane larger than one streaming block. The in-memory writer holds
/// a plane several times over (decoded, deinterleaved, compressed, read back), so it only takes
/// planes up to that size.
pub(crate) fn needs_tiled(im: &ImageInfo, opts: &ZarrExportOptions) -> bool {
    let whole = u64::from(im.size_x)
        * u64::from(im.size_y)
        * u64::from(im.samples_per_pixel.max(1))
        * im.pixel_type.bytes_per_sample() as u64;
    opts.level > 0
        || opts.region.is_some()
        || resolve_mode(im, opts) == PyramidMode::Source
        || whole > openreadout_ometiff::pyramid::BLOCK_BYTES
}

/// Writes blocks of one plane into the level arrays of one image.
struct ZarrSink<'a> {
    arrays: &'a [Array<FilesystemStore>],
    t: u64,
    z: u64,
    /// First Zarr channel of this plane (`c × samples per pixel`).
    zc: u64,
    spp: usize,
    bps: usize,
    ranges: &'a mut [ChannelRange],
}

impl BlockSink for ZarrSink<'_> {
    fn block(&mut self, level: usize, x: u32, y: u32, p: &Plane) -> Result<()> {
        let a = self
            .arrays
            .get(level)
            .ok_or_else(|| Error::Other(format!("internal error: no level {level}")))?;
        for (s, data) in pixels::deinterleave(&p.data, self.spp, self.bps)
            .into_iter()
            .enumerate()
        {
            let zc = self.zc + s as u64;
            if level == 0
                && let Some(r) = self.ranges.get_mut(zc as usize)
            {
                *r = pixels::union(*r, pixels::range(&data, p.pixel_type));
            }
            let subset = ArraySubset::new_with_ranges(&[
                self.t..self.t + 1,
                zc..zc + 1,
                self.z..self.z + 1,
                u64::from(y)..u64::from(y) + u64::from(p.height),
                u64::from(x)..u64::from(x) + u64::from(p.width),
            ]);
            a.store_array_subset(&subset, ArrayBytes::new_flen(le_to_native(&data, self.bps)))
                .map_err(|e| zerr("chunk write", e))?;
        }
        Ok(())
    }
}

/// Blocks written for one plane: (image, t, z, first Zarr channel) and their hashes.
struct Record {
    image: usize,
    t: u64,
    z: u64,
    zc: u64,
    blocks: Vec<BlockHash>,
}

/// Stream every selected plane of every planned image into a store at `dir`, write the group
/// metadata (multiscales with each level's real scale, omero windows, collection layout), then
/// read every block back. Returns (planes written, OME-XML bytes, levels per image).
#[allow(clippy::too_many_lines)]
pub(crate) fn write_store_tiled(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    plans: &[Plan<'_>],
    dir: &Path,
    opts: &ZarrExportOptions,
    ctx: &ReadContext<'_>,
    layout: Option<&openreadout_core::plate::PlateSummary>,
) -> Result<(u64, u64, Vec<WrittenLevels>)> {
    let pyr: Vec<ExportPlan> = plans
        .iter()
        .map(|p| {
            plan(
                p.info,
                opts.level,
                opts.region,
                resolve_mode(p.info, opts),
                opts.levels,
                opts.chunk,
            )
        })
        .collect::<Result<_>>()?;
    // What the exported images are: region size, level pixel size.
    let exported: Vec<ImageInfo> = plans
        .iter()
        .zip(&pyr)
        .map(|(p, e)| exported_info(p.info, e))
        .collect();
    let total: u64 = plans
        .iter()
        .map(|p| (p.t_map.len() * p.z_map.len() * p.c_map.len()) as u64)
        .sum();
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let store = Arc::new(FilesystemStore::new(dir).map_err(|e| zerr("store", e))?);
    let meta_opts = ArrayMetadataOptions::default().with_include_zarrs_metadata(false);
    let mut records = Vec::new();
    let mut count = 0u64;
    let mut all_arrays = Vec::new();
    for (pi, (plan_, e)) in plans.iter().zip(&pyr).enumerate() {
        let im = plan_.info;
        let spp = plan_.spp();
        let bps = im.pixel_type.bytes_per_sample();
        let arrays: Vec<Array<FilesystemStore>> = e
            .levels
            .iter()
            .enumerate()
            .map(|(l, lv)| {
                let (w, h) = lv.size;
                let shape = vec![
                    plan_.t_map.len() as u64,
                    plan_.zarr_c() as u64,
                    plan_.z_map.len() as u64,
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
                    .build(store.clone(), &plan_.array_path(l))
                    .map_err(|e| zerr("array", e))?;
                a.store_metadata_opt(&meta_opts)
                    .map_err(|e| zerr("array metadata", e))?;
                Ok(a)
            })
            .collect::<Result<_>>()?;
        let mut ranges = vec![ChannelRange::EMPTY; plan_.zarr_c()];
        for (ti, &t) in plan_.t_map.iter().enumerate() {
            for (zi, &z) in plan_.z_map.iter().enumerate() {
                for (ci, &c) in plan_.c_map.iter().enumerate() {
                    let zc = (ci * spp) as u64;
                    let mut sink = ZarrSink {
                        arrays: &arrays,
                        t: ti as u64,
                        z: zi as u64,
                        zc,
                        spp,
                        bps,
                        ranges: &mut ranges,
                    };
                    let blocks = stream_plane(
                        ds,
                        e,
                        PlaneIndex { c, z, t },
                        im.pixel_type,
                        im.samples_per_pixel,
                        &mut sink,
                    )?;
                    records.push(Record {
                        image: pi,
                        t: ti as u64,
                        z: zi as u64,
                        zc,
                        blocks,
                    });
                    count += 1;
                    ctx.report(count, total);
                }
            }
        }
        let default_z = (plan_.z_map.len() / 2) as u32;
        let ex = &exported[pi];
        let factors: Vec<(f64, f64)> = e.levels.iter().map(|l| l.factor).collect();
        let sizes: Vec<(u32, u32)> = e.levels.iter().map(|l| l.size).collect();
        let attrs = ngff::image_attributes(
            ngff::multiscales_with_factors(ex, &sizes, &factors, e.mode, CREATOR),
            ngff::omero(ex, &plan_.c_map, &ranges, default_z),
        );
        write_group(&store, &plan_.group, attrs)?;
        all_arrays.push(arrays);
    }
    // The collection metadata describes the exported geometry.
    let exported_plans: Vec<Plan<'_>> = plans
        .iter()
        .zip(&exported)
        .map(|(p, ex)| Plan {
            info: ex,
            group: p.group.clone(),
            c_map: p.c_map.clone(),
            z_map: p.z_map.clone(),
            t_map: p.t_map.clone(),
            levels: Vec::new(),
            well: p.well.clone(),
        })
        .collect();
    let xml_len = write_collection(ds, info, &exported_plans, dir, &store, opts, layout)?;
    verify(&all_arrays, plans, &records)?;
    let levels = plans
        .iter()
        .zip(&pyr)
        .map(|(p, e)| WrittenLevels {
            image: p.info.index,
            source_level: e.base_level,
            region: opts.region,
            pyramid: if e.levels.len() > 1 {
                e.mode
            } else {
                PyramidMode::None
            },
            sizes: e.levels.iter().map(|l| [l.size.0, l.size.1]).collect(),
            factors: e.levels.iter().map(|l| l.factor.0).collect(),
        })
        .collect();
    Ok((count, xml_len, levels))
}

/// Read every written block back (all samples, all levels) and compare hashes.
fn verify(
    arrays: &[Vec<Array<FilesystemStore>>],
    plans: &[Plan<'_>],
    records: &[Record],
) -> Result<()> {
    let mut bad = 0usize;
    let mut n = 0usize;
    for r in records {
        let spp = plans[r.image].spp();
        let bps = plans[r.image].info.pixel_type.bytes_per_sample();
        for b in &r.blocks {
            n += 1;
            let a = &arrays[r.image][b.level];
            let mut samples = Vec::with_capacity(spp);
            for s in 0..spp as u64 {
                let g = b.region;
                let subset = ArraySubset::new_with_ranges(&[
                    r.t..r.t + 1,
                    r.zc + s..r.zc + s + 1,
                    r.z..r.z + 1,
                    u64::from(g.y)..g.bottom(),
                    u64::from(g.x)..g.right(),
                ]);
                let bytes: ArrayBytes<'static> = a
                    .retrieve_array_subset(&subset)
                    .map_err(|e| zerr("read-back", e))?;
                let raw = bytes.into_fixed().map_err(|e| zerr("read-back", e))?;
                samples.push(le_to_native(&raw, bps).into_owned());
            }
            let data = pixels::interleave(&samples, bps);
            if format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&data)) != b.xxh3 {
                bad += 1;
            }
        }
    }
    if bad > 0 {
        return Err(Error::Other(format!(
            "read-back verification failed: {bad} of {n} blocks differ after writing; output discarded"
        )));
    }
    Ok(())
}
