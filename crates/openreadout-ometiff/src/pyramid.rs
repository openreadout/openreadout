//! Streaming multi-resolution export: a plane (or a rectangle of it) is read block by block and
//! handed to a [`BlockSink`] level by level, so an image of any size is written in bounded memory.
//!
//! Two ways to get the lower resolutions:
//!
//! - [`PyramidMode::Source`]: copy the source's own pyramid levels (CZI pyramid subblocks, the
//!   reduced pages of a whole-slide TIFF, VSI/ETS levels, Imaris resolution levels, OME-Zarr
//!   multiscales). Each level is read with [`Dataset::read_region`] in blocks.
//! - [`PyramidMode::Mean`]: compute 2 × 2 mean levels (odd edges average the pixels present,
//!   integers round half away from zero — the same rule as the OME-Zarr writer). The base level
//!   is read in blocks of `tile × 2^k` pixels; levels `1..=k` are computed inside each block
//!   (exact, because block origins are multiples of `2^k`), and the coarser levels from one
//!   buffer holding level `k` whole, which is small by then.
//!
//! Memory: one base block (at most [`BLOCK_BYTES`]) plus its downsampled copies, plus the level-`k`
//! buffer (at most [`TAIL_BYTES`]) in mean mode; the source's own tile caches come on top.

use openreadout_core::model::ImageInfo;
use openreadout_core::region::{PlacedTile, Region, ResolutionLevel, levels_of, paste_into_region};
use openreadout_core::{Dataset, Error, PixelType, Plane, PlaneIndex, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Largest base block read at once (bytes of decoded samples).
pub const BLOCK_BYTES: u64 = 256 << 20;
/// Largest level held whole to compute the coarsest mean levels from.
pub const TAIL_BYTES: u64 = 1 << 30;
/// Levels are added while a level is larger than this in either dimension (as the OME-Zarr
/// writer does) when no level count is given.
pub const AUTO_PYRAMID_THRESHOLD: u32 = 1024;

/// Where the lower resolutions of an export come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum PyramidMode {
    /// The writer's default: the source's own levels when it has them and the whole image is
    /// exported from level 0; else 2 × 2 mean levels for OME-Zarr and none for OME-TIFF.
    #[default]
    Auto,
    /// Full resolution only.
    None,
    /// The source's own pyramid levels, copied.
    Source,
    /// 2 × 2 mean levels computed from the exported resolution.
    Mean,
}

impl std::str::FromStr for PyramidMode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "auto" => PyramidMode::Auto,
            "none" => PyramidMode::None,
            "source" => PyramidMode::Source,
            "mean" => PyramidMode::Mean,
            other => {
                return Err(Error::Usage(format!(
                    "unknown pyramid mode '{other}' (expected auto, none, source or mean)"
                )));
            }
        })
    }
}

/// One output resolution level.
#[derive(Debug, Clone, PartialEq)]
pub struct OutLevel {
    /// Width and height in pixels.
    pub size: (u32, u32),
    /// Output level-0 size over this level's size (x, y).
    pub factor: (f64, f64),
    /// Source level read for it (source mode), with the rectangle of that level.
    pub source: Option<(u32, Region)>,
}

/// How one image is exported: which source level and rectangle is the base, and its levels.
#[derive(Debug, Clone)]
pub struct ExportPlan {
    /// Source image index.
    pub image: u32,
    /// Source level the base (output level 0) is read from.
    pub base_level: u32,
    /// Rectangle of the base level that is exported.
    pub region: Region,
    /// Output levels, full resolution first.
    pub levels: Vec<OutLevel>,
    /// `Source` or `Mean` (or `None` with a single level).
    pub mode: PyramidMode,
    /// Tile (chunk) edge of the output, in pixels.
    pub tile: u32,
    /// Base block edge (a multiple of `tile`).
    pub block: u32,
    /// Mean mode: levels `1..=stream_levels` are computed block by block.
    pub stream_levels: usize,
    /// Bytes per pixel (all samples).
    pub bytes_per_pixel: usize,
}

/// Sizes of 2 × 2 mean levels below `(w, h)`: halving (rounding up) `levels - 1` times, or
/// while a level exceeds [`AUTO_PYRAMID_THRESHOLD`] when `levels` is `None`.
pub fn mean_level_sizes(w: u32, h: u32, levels: Option<u32>) -> Vec<(u32, u32)> {
    let mut out = vec![(w.max(1), h.max(1))];
    loop {
        let &(lw, lh) = out.last().unwrap_or(&(1, 1));
        let more = match levels {
            None => lw > AUTO_PYRAMID_THRESHOLD || lh > AUTO_PYRAMID_THRESHOLD,
            Some(n) => (out.len() as u32) < n.max(1) && (lw > 1 || lh > 1),
        };
        if !more {
            return out;
        }
        out.push((lw.div_ceil(2), lh.div_ceil(2)));
    }
}

/// Plan the export of `im`: base level `level`, rectangle `region` of it (default: all of it),
/// lower levels from `mode` (already resolved from `Auto`), `levels` output levels at most
/// (mean mode: exactly, when given), tiles of `tile` pixels.
pub fn plan(
    im: &ImageInfo,
    level: u32,
    region: Option<Region>,
    mode: PyramidMode,
    levels: Option<u32>,
    tile: u32,
) -> Result<ExportPlan> {
    if tile == 0 {
        return Err(Error::Usage("tile size must be at least 1".into()));
    }
    let src_levels: Vec<ResolutionLevel> = levels_of(im);
    let base = src_levels
        .iter()
        .find(|l| l.level == level)
        .cloned()
        .ok_or_else(|| {
            Error::Usage(format!(
                "pyramid level {level} out of range for image {} (0..{})",
                im.index,
                src_levels.len()
            ))
        })?;
    let region = region.unwrap_or(Region::full(base.size_x, base.size_y));
    region.check_within(
        base.size_x,
        base.size_y,
        &format!("image {} level {level}", im.index),
    )?;
    let spp = im.samples_per_pixel.max(1) as usize;
    let bpp = spp * im.pixel_type.bytes_per_sample();
    let (w, h) = (region.width, region.height);
    let mut out = ExportPlan {
        image: im.index,
        base_level: level,
        region,
        levels: vec![OutLevel {
            size: (w, h),
            factor: (1.0, 1.0),
            source: Some((level, region)),
        }],
        mode,
        tile,
        block: tile,
        stream_levels: 0,
        bytes_per_pixel: bpp,
    };
    match mode {
        PyramidMode::Source => {
            let max = levels.map_or(usize::MAX, |n| n.max(1) as usize);
            // A level with another number of z planes (Imaris downsamples z too) cannot be a
            // sub-resolution of the same planes; it and the levels after it are not copied.
            for l in src_levels
                .iter()
                .filter(|l| l.level > level)
                .take_while(|l| l.size_z == base.size_z)
            {
                if out.levels.len() >= max {
                    break;
                }
                let r = region.rescale((base.size_x, base.size_y), (l.size_x, l.size_y));
                out.levels.push(OutLevel {
                    size: (r.width, r.height),
                    // The source's own factors relative to the base level (a region's size
                    // ratio is only approximately the factor).
                    factor: (
                        l.downsample_x / base.downsample_x.max(f64::MIN_POSITIVE),
                        l.downsample_y / base.downsample_y.max(f64::MIN_POSITIVE),
                    ),
                    source: Some((l.level, r)),
                });
            }
        }
        PyramidMode::Mean => {
            for (i, &(lw, lh)) in mean_level_sizes(w, h, levels).iter().enumerate().skip(1) {
                let f = f64::from(1u32 << i.min(31));
                out.levels.push(OutLevel {
                    size: (lw, lh),
                    factor: (f, f),
                    source: None,
                });
            }
        }
        PyramidMode::None | PyramidMode::Auto => {}
    }
    // Block size: a multiple of the tile, within BLOCK_BYTES; in mean mode tile × 2^k with k
    // as large as the budget allows (up to the number of levels below the base).
    let tile64 = u64::from(tile);
    let fits = |b: u64| b * b * bpp as u64 <= BLOCK_BYTES;
    if out.mode == PyramidMode::Mean && out.levels.len() > 1 {
        let mut k = 0usize;
        while k + 1 < out.levels.len() && fits(tile64 << (k + 1)) && (tile64 << (k + 1)) <= 1 << 16
        {
            k += 1;
        }
        // The coarser levels are made from level k held whole.
        if k + 1 < out.levels.len() {
            let (tw, th) = out.levels[k].size;
            let tail = u64::from(tw) * u64::from(th) * bpp as u64;
            if tail > TAIL_BYTES {
                return Err(Error::unsupported(
                    "export",
                    format!(
                        "a mean pyramid of a {w} x {h} image (level {k} alone is {:.1} GiB)",
                        tail as f64 / f64::from(1u32 << 30)
                    ),
                    "Export the source's own levels (--pyramid source), a region (--region), or from a coarser level (--level N).",
                ));
            }
        }
        out.stream_levels = k;
        out.block = u32::try_from(tile64 << k).unwrap_or(u32::MAX);
    } else {
        let mut b = tile64;
        while fits(b * 2) && b * 2 <= 8192 {
            b *= 2;
        }
        out.block = u32::try_from(b).unwrap_or(tile);
    }
    Ok(out)
}

/// The image as exported: its size is the region's, its pixel size the base level's.
pub fn exported_info(im: &ImageInfo, p: &ExportPlan) -> ImageInfo {
    let mut out = im.clone();
    let lv = levels_of(im).into_iter().find(|l| l.level == p.base_level);
    let (fx, fy) = lv.map_or((1.0, 1.0), |l| (l.downsample_x, l.downsample_y));
    out.size_x = p.region.width;
    out.size_y = p.region.height;
    if p.base_level > 0 {
        out.physical_size.x = out.physical_size.x.map(|v| v * fx);
        out.physical_size.y = out.physical_size.y.map(|v| v * fy);
    }
    out.pyramid_levels = p.levels.len() as u32;
    out.resolution_levels = Vec::new();
    out.mosaic = None;
    out
}

/// Receives the output pixels. `level` is the output level, `(x, y)` the block's top-left pixel
/// in that level (a multiple of the tile edge), `plane` its samples.
pub trait BlockSink {
    /// Take one block of level `level`.
    fn block(&mut self, level: usize, x: u32, y: u32, plane: &Plane) -> Result<()>;
}

/// One block handed to the sink, with the xxh3-128 of its samples (for read-back verification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHash {
    /// Output level.
    pub level: usize,
    /// Rectangle in that level.
    pub region: Region,
    /// xxh3-128 hex of the samples.
    pub xxh3: String,
}

fn emit(
    sink: &mut dyn BlockSink,
    hashes: &mut Vec<BlockHash>,
    level: usize,
    x: u32,
    y: u32,
    p: &Plane,
) -> Result<()> {
    sink.block(level, x, y, p)?;
    hashes.push(BlockHash {
        level,
        region: Region::new(x, y, p.width, p.height),
        xxh3: p.xxh3_hex(),
    });
    Ok(())
}

fn check_block(p: &Plane, want: (u32, u32), pt: PixelType, spp: u32) -> Result<()> {
    if (p.width, p.height) != want
        || p.pixel_type != pt
        || p.samples_per_pixel.max(1) != spp.max(1)
        || p.data.len() != p.expected_len()
    {
        return Err(Error::corrupt(
            "export",
            format!(
                "the source returned a {}x{} {} x{} block ({} bytes) where {}x{} {} x{} was asked for",
                p.width,
                p.height,
                p.pixel_type.ome_name(),
                p.samples_per_pixel,
                p.data.len(),
                want.0,
                want.1,
                pt.ome_name(),
                spp
            ),
        ));
    }
    Ok(())
}

/// Read plane `idx` of the plan's image block by block and hand every output level to `sink`.
/// Returns the hash of every block emitted, in emission order.
#[allow(clippy::many_single_char_names)]
pub fn stream_plane(
    ds: &mut dyn Dataset,
    plan: &ExportPlan,
    idx: PlaneIndex,
    pixel_type: PixelType,
    spp: u32,
    sink: &mut dyn BlockSink,
) -> Result<Vec<BlockHash>> {
    let mut hashes = Vec::new();
    let b = plan.block;
    if plan.mode == PyramidMode::Mean && plan.levels.len() > 1 {
        let k = plan.stream_levels;
        let (w, h) = plan.levels[0].size;
        let tail_size = plan.levels[k].size;
        let mut tail: Option<Plane> = (k + 1 < plan.levels.len()).then(|| Plane {
            width: tail_size.0,
            height: tail_size.1,
            pixel_type,
            samples_per_pixel: spp,
            data: vec![0u8; tail_size.0 as usize * tail_size.1 as usize * plan.bytes_per_pixel],
        });
        for by in (0..h).step_by(b as usize) {
            for bx in (0..w).step_by(b as usize) {
                let r = Region::new(
                    plan.region.x + bx,
                    plan.region.y + by,
                    b.min(w - bx),
                    b.min(h - by),
                );
                let mut cur = ds.read_region(plan.image, idx, plan.base_level, r)?;
                check_block(&cur, (r.width, r.height), pixel_type, spp)?;
                emit(sink, &mut hashes, 0, bx, by, &cur)?;
                for l in 1..=k {
                    cur = downsample_2x(&cur);
                    emit(sink, &mut hashes, l, bx >> l, by >> l, &cur)?;
                }
                if let Some(t) = tail.as_mut() {
                    let bpp = plan.bytes_per_pixel;
                    let whole = Region::full(t.width, t.height);
                    // Paste the level-k block into the whole level-k buffer.
                    let mut buf = std::mem::take(&mut t.data);
                    paste_into_region(
                        &mut buf,
                        whole,
                        &PlacedTile {
                            data: &cur.data,
                            row_bytes: cur.width as usize * bpp,
                            x: i64::from(bx >> k),
                            y: i64::from(by >> k),
                            width: cur.width,
                            height: cur.height,
                        },
                        bpp,
                    );
                    t.data = buf;
                }
            }
        }
        if let Some(mut cur) = tail {
            for l in k + 1..plan.levels.len() {
                cur = downsample_2x(&cur);
                emit(sink, &mut hashes, l, 0, 0, &cur)?;
            }
        }
        return Ok(hashes);
    }
    for (l, out) in plan.levels.iter().enumerate() {
        let (src_level, src) = out.source.unwrap_or((plan.base_level, plan.region));
        for by in (0..src.height).step_by(b as usize) {
            for bx in (0..src.width).step_by(b as usize) {
                let r = Region::new(
                    src.x + bx,
                    src.y + by,
                    b.min(src.width - bx),
                    b.min(src.height - by),
                );
                let p = ds.read_region(plan.image, idx, src_level, r)?;
                check_block(&p, (r.width, r.height), pixel_type, spp)?;
                emit(sink, &mut hashes, l, bx, by, &p)?;
            }
        }
    }
    Ok(hashes)
}

/// Halve a plane in both dimensions (output `ceil(w/2) × ceil(h/2)`): each output sample is the
/// mean of the (up to) 2 × 2 input samples it covers, per sample of interleaved pixels; integers
/// round half away from zero.
pub fn downsample_2x(p: &Plane) -> Plane {
    let (w, h) = (p.width as usize, p.height as usize);
    let spp = p.samples_per_pixel.max(1) as usize;
    let (ow, oh) = (w.div_ceil(2), h.div_ceil(2));
    macro_rules! go {
        ($t:ty, $n:expr, $from:expr, $to:expr) => {{
            // Samples are decoded where they are read: no widened copy of the block.
            let src = p.data.as_chunks::<$n>().0;
            let mut out = Vec::with_capacity(ow * oh * spp * $n);
            for oy in 0..oh {
                let ys = if oy * 2 + 1 < h { 2 } else { 1 };
                for ox in 0..ow {
                    let xs = if ox * 2 + 1 < w { 2 } else { 1 };
                    for s in 0..spp {
                        let mut sum = 0.0f64;
                        for dy in 0..ys {
                            for dx in 0..xs {
                                sum += src
                                    .get(((oy * 2 + dy) * w + ox * 2 + dx) * spp + s)
                                    .map_or(0.0, |c| $from(<$t>::from_le_bytes(*c)));
                            }
                        }
                        let m = sum / (ys * xs) as f64;
                        let v: $t = $to(m);
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                }
            }
            out
        }};
    }
    let data = if w == 0 || h == 0 {
        Vec::new()
    } else {
        match p.pixel_type {
            PixelType::Uint8 => go!(u8, 1, f64::from, |m: f64| m.round().clamp(0.0, 255.0) as u8),
            PixelType::Int8 => go!(i8, 1, f64::from, |m: f64| m.round().clamp(-128.0, 127.0)
                as i8),
            PixelType::Uint16 => go!(u16, 2, f64::from, |m: f64| m.round().clamp(0.0, 65535.0)
                as u16),
            PixelType::Int16 => go!(
                i16,
                2,
                f64::from,
                |m: f64| m.round().clamp(-32768.0, 32767.0) as i16
            ),
            PixelType::Uint32 => go!(u32, 4, f64::from, |m: f64| m
                .round()
                .clamp(0.0, f64::from(u32::MAX))
                as u32),
            PixelType::Int32 => go!(i32, 4, f64::from, |m: f64| m
                .round()
                .clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                as i32),
            PixelType::Float => go!(f32, 4, f64::from, |m: f64| m as f32),
            PixelType::Double => go!(f64, 8, |v: f64| v, |m: f64| m),
            _ => Vec::new(),
        }
    };
    Plane {
        width: ow as u32,
        height: oh as u32,
        pixel_type: p.pixel_type,
        samples_per_pixel: p.samples_per_pixel,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(w: u32, h: u32, spp: u32, f: impl Fn(u32, u32, u32) -> u16) -> Plane {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                for s in 0..spp {
                    data.extend_from_slice(&f(x, y, s).to_le_bytes());
                }
            }
        }
        Plane {
            width: w,
            height: h,
            pixel_type: PixelType::Uint16,
            samples_per_pixel: spp,
            data,
        }
    }

    #[test]
    fn downsample_matches_the_rule() {
        // 3x2 → 2x1: [(1+2+4+5)/4 = 3, (3+6)/2 = 4.5 → 5]
        let p = plane(3, 2, 1, |x, y, _| (y * 3 + x + 1) as u16);
        let d = downsample_2x(&p);
        assert_eq!((d.width, d.height), (2, 1));
        let v: Vec<u16> = d
            .data
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(v, [3, 5]);
        // Samples of RGB pixels are averaged separately.
        let rgb = plane(2, 2, 3, |_, _, s| (s * 10) as u16);
        let v: Vec<u16> = downsample_2x(&rgb)
            .data
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(v, [0, 10, 20]);
    }

    /// A plane of known pixels that serves any region at level 0.
    struct Src(Plane);
    impl Dataset for Src {
        fn info(&self) -> Result<openreadout_core::FileInfo> {
            Err(Error::Other("unused".into()))
        }
        fn vendor_metadata(&self) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn provenance(&self) -> openreadout_core::ProvenanceMap {
            openreadout_core::ProvenanceMap::new()
        }
        fn entries(&self) -> Result<Vec<openreadout_core::LsEntry>> {
            Ok(vec![])
        }
        fn read_plane(&mut self, _: u32, _: PlaneIndex) -> Result<Plane> {
            Ok(self.0.clone())
        }
        fn check(&mut self) -> Result<openreadout_core::CheckReport> {
            Ok(openreadout_core::CheckReport::new("t", "t"))
        }
    }

    /// Collects every level into whole planes.
    struct Collect(Vec<Plane>, usize);
    impl BlockSink for Collect {
        fn block(&mut self, level: usize, x: u32, y: u32, p: &Plane) -> Result<()> {
            let t = &mut self.0[level];
            let whole = Region::full(t.width, t.height);
            paste_into_region(
                &mut t.data,
                whole,
                &PlacedTile {
                    data: &p.data,
                    row_bytes: p.width as usize * self.1,
                    x: i64::from(x),
                    y: i64::from(y),
                    width: p.width,
                    height: p.height,
                },
                self.1,
            );
            Ok(())
        }
    }

    #[test]
    fn streamed_mean_pyramid_equals_the_global_one() {
        // 37 x 23, tiles of 4: several blocks, odd edges, block-local and tail levels.
        let src = plane(37, 23, 1, |x, y, _| ((x * 7 + y * 13) % 251) as u16);
        let mut im = ImageInfo::new(0, 37, 23, PixelType::Uint16);
        im.samples_per_pixel = 1;
        let mut p = plan(&im, 0, None, PyramidMode::Mean, Some(6), 4).unwrap();
        // Force small blocks: two block-local levels, the rest from the tail.
        p.stream_levels = 2;
        p.block = 16;
        let mut want = vec![src.clone()];
        while want.len() < p.levels.len() {
            let d = downsample_2x(want.last().unwrap());
            want.push(d);
        }
        let mut sink = Collect(
            want.iter()
                .map(|w| Plane {
                    data: vec![0; w.data.len()],
                    ..w.clone()
                })
                .collect(),
            2,
        );
        let mut ds = Src(src.clone());
        let hashes = stream_plane(
            &mut ds,
            &p,
            PlaneIndex::default(),
            PixelType::Uint16,
            1,
            &mut sink,
        )
        .unwrap();
        for (l, (got, w)) in sink.0.iter().zip(&want).enumerate() {
            assert_eq!(got.data, w.data, "level {l}");
            assert_eq!((got.width, got.height), p.levels[l].size);
        }
        assert!(hashes.iter().any(|h| h.level == 5));
    }

    #[test]
    fn plans_levels_regions_and_blocks() {
        let mut im = ImageInfo::new(0, 10_000, 6_000, PixelType::Uint8);
        im.samples_per_pixel = 3;
        im.pyramid_levels = 3;
        im.resolution_levels = vec![
            ResolutionLevel::new(0, 10_000, 6_000, 10_000, 6_000),
            ResolutionLevel::new(1, 2_500, 1_500, 10_000, 6_000),
            ResolutionLevel::new(2, 625, 375, 10_000, 6_000),
        ];
        let p = plan(
            &im,
            0,
            Some(Region::new(1000, 1000, 4000, 2000)),
            PyramidMode::Source,
            None,
            512,
        )
        .unwrap();
        assert_eq!(p.levels.len(), 3);
        assert_eq!(
            p.levels[1].source,
            Some((1, Region::new(250, 250, 1000, 500)))
        );
        assert_eq!(p.levels[1].factor, (4.0, 4.0));
        let m = plan(&im, 1, None, PyramidMode::Mean, None, 256).unwrap();
        assert_eq!(
            m.levels.iter().map(|l| l.size).collect::<Vec<_>>(),
            [(2500, 1500), (1250, 750), (625, 375)]
        );
        assert_eq!(m.block % 256, 0);
        assert!(plan(&im, 3, None, PyramidMode::None, None, 512).is_err());
        assert!(
            plan(
                &im,
                0,
                Some(Region::new(9000, 0, 2000, 10)),
                PyramidMode::None,
                None,
                512
            )
            .is_err()
        );
        assert_eq!("mean".parse::<PyramidMode>().unwrap(), PyramidMode::Mean);
        assert!("median".parse::<PyramidMode>().is_err());
    }

    /// Imaris halves z at its lower levels (zenodo4433202-ovule-732: 219 planes, then 109):
    /// such a level is not copied, since the export's planes are level-0 planes.
    #[test]
    fn source_levels_with_fewer_z_planes_are_not_copied() {
        let mut im = ImageInfo::new(0, 256, 256, PixelType::Uint8);
        im.size_z = 219;
        im.pyramid_levels = 2;
        let mut l1 = ResolutionLevel::new(1, 128, 128, 256, 256);
        l1.size_z = Some(109);
        im.resolution_levels = vec![ResolutionLevel::new(0, 256, 256, 256, 256), l1];
        let p = plan(&im, 0, None, PyramidMode::Source, None, 512).unwrap();
        assert_eq!(p.levels.len(), 1);
    }
}
