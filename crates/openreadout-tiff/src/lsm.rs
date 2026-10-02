//! Zeiss LSM: the info record in private tag 34412 and its sub-records.
//! Layout derived from the public documentation of tifffile (BSD-3-Clause); see
//! `docs/formats/tiff.md` § Zeiss LSM and `docs/provenance/tiff.md`.

use openreadout_core::bytes::{le_f64, le_u16, le_u32, until_nul};
use openreadout_core::model::{ChannelInfo, ImageInfo, InstrumentInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use openreadout_core::{Error, PixelType, Result};
use serde_json::json;

use crate::container::{ByteOrder, ByteSource, FieldValue, Ifd};
use crate::dataset::{Attachment, PlaneSrc, Series, TiffDataset, check_plane_count};
use crate::decode::PageLayout;
use crate::files::FileSetMember;
use crate::{FORMAT_ID, tags};

/// The two magic numbers seen at the start of the record (LSM 1.3 and LSM 2.0+).
pub const LSM_INFO_MAGICS: [u32; 2] = [0x0300_494C, 0x0400_494C];

/// The parts of the LSM info record we use. Sizes are voxel counts; lengths in metres as stored.
#[derive(Debug, Clone, Default)]
pub struct LsmInfo {
    pub magic: u32,
    pub record_size: u32,
    pub dim_x: u32,
    pub dim_y: u32,
    pub dim_z: u32,
    pub dim_channels: u32,
    pub dim_time: u32,
    /// 1 = 8-bit, 2 = 12-bit, 5 = 32-bit float, 0 = varies per channel.
    pub data_type: u32,
    pub voxel_size_x_m: f64,
    pub voxel_size_y_m: f64,
    pub voxel_size_z_m: f64,
    /// Acquisition mode: 0 = xyz stack, 3 = time series xy, 6 = time series xyz, ...
    pub scan_type: u16,
    pub spectral_scan: u16,
    pub time_interval_s: f64,
    /// Positions and mosaic tiles (LSM 4.2+; 0 when absent).
    pub dim_positions: u32,
    pub dim_tiles: u32,
    pub channel_names: Vec<String>,
    /// `[r, g, b]` per channel.
    pub channel_colors: Vec<[u8; 3]>,
    /// Seconds, one per time point.
    pub time_stamps: Vec<f64>,
    /// `[start, end]` in metres per channel, for spectral scans.
    pub channel_wavelengths_m: Vec<[f64; 2]>,
}

/// Parse the record (the raw bytes of tag 34412, or the bytes at its offset). Sub-records
/// are read from `src`. Returns `None` when the magic number does not match.
pub fn parse(record: &[u8], src: &mut ByteSource) -> Option<LsmInfo> {
    let magic = le_u32(record, 0)?;
    if !LSM_INFO_MAGICS.contains(&magic) {
        return None;
    }
    let record_size = le_u32(record, 4)?;
    let i32f = |at: usize| le_u32(record, at).map(|v| (v as i32).max(0) as u32);
    let mut info = LsmInfo {
        magic,
        record_size,
        dim_x: i32f(8)?,
        dim_y: i32f(12)?,
        dim_z: i32f(16)?,
        dim_channels: i32f(20)?,
        dim_time: i32f(24)?,
        data_type: i32f(28)?,
        voxel_size_x_m: le_f64(record, 40)?,
        voxel_size_y_m: le_f64(record, 48)?,
        voxel_size_z_m: le_f64(record, 56)?,
        scan_type: le_u16(record, 88)?,
        spectral_scan: le_u16(record, 90)?,
        time_interval_s: le_f64(record, 112).unwrap_or(0.0),
        ..Default::default()
    };
    let size = record_size as usize;
    if size >= 272 {
        info.dim_positions = i32f(264).unwrap_or(0);
        info.dim_tiles = i32f(268).unwrap_or(0);
    }
    let offset = |at: usize| {
        (at + 4 <= size)
            .then(|| le_u32(record, at))
            .flatten()
            .filter(|&o| o >= 8)
            .map(u64::from)
    };
    if let Some(off) = offset(108) {
        read_channel_colors(src, off, &mut info);
    }
    if let Some(off) = offset(132)
        && let Ok(h) = src.read_at(off, 8)
    {
        let count = le_u32(&h, 4).unwrap_or(0) as u64;
        if count > 0
            && count < 10_000_000
            && le_u32(&h, 0).map(u64::from) == Some(8 + 8 * count)
            && let Ok(b) = src.read_at(off + 8, 8 * count)
        {
            info.time_stamps = b
                .as_chunks::<8>()
                .0
                .iter()
                .filter_map(|c| le_f64(c, 0))
                .collect();
        }
    }
    if let Some(off) = offset(204)
        && let Ok(h) = src.read_at(off, 4)
    {
        let n = u64::from(le_u32(&h, 0).unwrap_or(0));
        if n > 0
            && n < 4096
            && let Ok(b) = src.read_at(off + 4, 16 * n)
        {
            info.channel_wavelengths_m = b
                .as_chunks::<16>()
                .0
                .iter()
                .filter_map(|c| Some([le_f64(c, 0)?, le_f64(c, 8)?]))
                .collect();
        }
    }
    Some(info)
}

/// Channel colours and names: a 24-byte header (size, colour count, name count, colour
/// offset, name offset, mono flag), RGBA colours, then length-prefixed names.
fn read_channel_colors(src: &mut ByteSource, off: u64, info: &mut LsmInfo) {
    let Ok(h) = src.read_at(off, 24) else {
        return;
    };
    let (Some(size), Some(ncolors), Some(nnames), Some(coff), Some(noff)) = (
        le_u32(&h, 0),
        le_u32(&h, 4),
        le_u32(&h, 8),
        le_u32(&h, 12),
        le_u32(&h, 16),
    ) else {
        return;
    };
    if ncolors != nnames || ncolors > 4096 || size > 1 << 20 {
        return;
    }
    let Ok(block) = src.read_at(off, u64::from(size)) else {
        return;
    };
    for i in 0..ncolors as usize {
        if let Some(c) = block.get(coff as usize + 4 * i..coff as usize + 4 * i + 4) {
            info.channel_colors.push([c[0], c[1], c[2]]);
        }
    }
    let mut p = noff as usize;
    while info.channel_names.len() < nnames as usize {
        let Some(len) = le_u32(&block, p) else { break };
        let len = len as usize;
        let Some(raw) = block.get(p + 4..p + 4 + len) else {
            break;
        };
        info.channel_names
            .push(String::from_utf8_lossy(until_nul(raw)).trim().to_string());
        p += 4 + len;
    }
}

impl LsmInfo {
    /// Axis letters of the stored data for this scan type, slowest first, without X/Y
    /// (`Z`, `C`, `T`); line scans are not image stacks and return `None`.
    pub fn stack_axes(&self) -> Option<&'static str> {
        Some(match self.scan_type {
            0 => "ZC",
            3 => "TC",
            6 | 7 => "TZC",
            _ => return None,
        })
    }
}

/// How the planes of an LSM file map onto images: one image per position (or tile), each
/// with Z and T from the record and channels from the page's samples.
struct LsmStack {
    axes: &'static str,
    pixel_type: PixelType,
    size_z: u32,
    size_t: u32,
    size_c: u32,
    samples: u32,
    /// Each sample is a channel (planar pages).
    sample_channels: bool,
    positions: u32,
    /// Data pages per position.
    per_position: usize,
}

impl TiffDataset {
    pub(crate) fn build_lsm(&mut self, lsm: &LsmInfo) -> Result<()> {
        let order = self.main().header.byte_order;
        let ifds = self.main().ifds.clone();
        let data_pages: Vec<usize> = ifds
            .iter()
            .enumerate()
            .filter(|(_, i)| i.uint(tags::NEW_SUBFILE_TYPE).unwrap_or(0) & 1 == 0)
            .map(|(i, _)| i)
            .collect();
        let thumbs: Vec<usize> = (0..ifds.len())
            .filter(|i| !data_pages.contains(i))
            .collect();
        let Some(&p0) = data_pages.first() else {
            return Err(Error::corrupt(FORMAT_ID, "LSM file without data pages"));
        };
        let l = PageLayout::from_ifd(&ifds[p0], order)?;
        if l.compression != 1 {
            self.fix_lsm_byte_counts(&ifds, &data_pages, &thumbs);
        }
        let stack = self.lsm_stack(lsm, &l)?;
        if (data_pages.len() as u64) < stack.per_position as u64 * u64::from(stack.positions) {
            self.notes.push(format!(
                "LSM record declares {} planes per position × {} but the file has {} data pages (acquisition stopped early?)",
                stack.per_position,
                stack.positions,
                data_pages.len()
            ));
        }
        for pos in 0..stack.positions {
            let info = lsm_image_info(
                lsm,
                &stack,
                pos,
                ImageInfo::new(pos, l.width, l.height, stack.pixel_type),
            );
            let planes = lsm_planes(&stack, pos, &data_pages)?;
            self.series.push(Series {
                info: info.finish(),
                planes,
                levels: Vec::new(),
            });
        }
        for p in thumbs {
            self.attachments.push(Attachment {
                role: "thumbnail".into(),
                page: p,
            });
        }
        self.set_provenance(&[
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_z", Source::PriorArt),
            ("images[].size_c", Source::PriorArt),
            ("images[].size_t", Source::PriorArt),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::PriorArt),
            ("images[].time_increment_s", Source::PriorArt),
            ("images[].channels[].name", Source::PriorArt),
            ("images[].channels[].color", Source::PriorArt),
        ]);
        Ok(())
    }

    /// Compressed LSM: `StripByteCounts` hold uncompressed sizes; a strip's stored size is the
    /// gap to the next strip of any page (or twice the count, up to the end of the file).
    fn fix_lsm_byte_counts(&mut self, ifds: &[Ifd], data_pages: &[usize], thumbs: &[usize]) {
        let len = self.main().file_len;
        let mut all: Vec<u64> = data_pages
            .iter()
            .chain(thumbs)
            .flat_map(|&p| ifds[p].uints(tags::STRIP_OFFSETS).unwrap_or_default())
            .collect();
        all.sort_unstable();
        all.dedup();
        for &p in data_pages {
            let offs = ifds[p].uints(tags::STRIP_OFFSETS).unwrap_or_default();
            let counts = ifds[p].uints(tags::STRIP_BYTE_COUNTS).unwrap_or_default();
            let fixed: Vec<u64> =
                offs.iter()
                    .enumerate()
                    .map(|(k, &o)| {
                        let next = all.iter().find(|&&x| x > o).copied().unwrap_or_else(|| {
                            (o + 2 * counts.get(k).copied().unwrap_or(0)).min(len)
                        });
                        next - o
                    })
                    .collect();
            self.byte_count_fix.insert(p, fixed);
        }
    }

    /// The stack layout of the record (x-y stacks and time series only).
    fn lsm_stack(&mut self, lsm: &LsmInfo, l: &PageLayout) -> Result<LsmStack> {
        let axes = lsm.stack_axes().ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!(
                    "LSM scan type {} (line, point or spline scans)",
                    lsm.scan_type
                ),
                "Only x-y stacks and x-y time series (scan types 0, 3, 6) are exposed as images.",
            )
        })?;
        let pixel_type = l.pixel_type()?;
        let size_z = if axes.contains('Z') {
            lsm.dim_z.max(1)
        } else {
            1
        };
        let size_t = if axes.contains('T') {
            lsm.dim_time.max(1)
        } else {
            1
        };
        let spp = u32::from(l.samples_per_pixel);
        let (size_c, samples, sample_channels) = if spp > 1 && l.planar == 2 {
            (spp, 1, true)
        } else if spp > 1 {
            (1, spp, false)
        } else {
            (1, 1, false)
        };
        if lsm.dim_channels > 1 && spp == 1 {
            self.notes.push(format!(
                "LSM record declares {} channels but pages hold 1 sample; exposing 1 channel",
                lsm.dim_channels
            ));
        }
        let positions = lsm
            .dim_positions
            .max(1)
            .saturating_mul(lsm.dim_tiles.max(1));
        check_plane_count(u64::from(positions))?;
        let per_position = u64::from(size_z).saturating_mul(u64::from(size_t));
        check_plane_count(per_position)?;
        Ok(LsmStack {
            axes,
            pixel_type,
            size_z,
            size_t,
            size_c,
            samples,
            sample_channels,
            positions,
            per_position: per_position as usize,
        })
    }
}

/// The image of position `pos`: sizes, voxel size and frame interval, channels and the record.
fn lsm_image_info(lsm: &LsmInfo, stack: &LsmStack, pos: u32, mut info: ImageInfo) -> ImageInfo {
    let (zc, tc, size_c, samples) = (stack.size_z, stack.size_t, stack.size_c, stack.samples);
    info.size_z = zc;
    info.size_t = tc;
    info.size_c = size_c;
    info.samples_per_pixel = samples;
    info.dimension_order = if stack.axes == "TZC" {
        "XYCZT"
    } else if stack.axes == "TC" {
        "XYCTZ"
    } else {
        "XYCZT"
    }
    .into();
    if stack.positions > 1 {
        info.name = Some(format!("position {pos}"));
    }
    let um = |m: f64| (m.is_finite() && m > 0.0).then_some(m * 1e6);
    info.physical_size = PhysicalSize::micrometres(
        um(lsm.voxel_size_x_m),
        um(lsm.voxel_size_y_m),
        if zc > 1 { um(lsm.voxel_size_z_m) } else { None },
    );
    info.time_increment_s = if tc > 1 && lsm.time_stamps.len() == tc as usize {
        Some((lsm.time_stamps[tc as usize - 1] - lsm.time_stamps[0]) / f64::from(tc - 1))
    } else if tc > 1 && lsm.time_interval_s > 0.0 {
        Some(lsm.time_interval_s)
    } else {
        None
    };
    info.channels = (0..size_c.max(samples.min(1)))
        .map(|ci| ChannelInfo {
            index: ci,
            name: lsm
                .channel_names
                .get(ci as usize)
                .cloned()
                .filter(|s| !s.is_empty()),
            color: lsm
                .channel_colors
                .get(ci as usize)
                .map(|c| format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])),
            emission_range_nm: lsm
                .channel_wavelengths_m
                .get(ci as usize)
                .filter(|w| w[0] > 0.0 && w[1] > 0.0)
                .map(|w| [w[0] * 1e9, w[1] * 1e9]),
            ..ChannelInfo::default()
        })
        .collect();
    info.instrument = Some(InstrumentInfo {
        manufacturer: Some("Carl Zeiss".into()),
        ..InstrumentInfo::default()
    });
    info.extra
        .insert("lsm_scan_type".into(), json!(lsm.scan_type));
    info.extra.insert(
        "lsm_record".into(),
        json!({
            "magic": format!("{:#010x}", lsm.magic),
            "record_size": lsm.record_size,
            "dimensions_xyzct": [lsm.dim_x, lsm.dim_y, lsm.dim_z, lsm.dim_channels, lsm.dim_time],
            "positions": lsm.dim_positions,
            "tiles": lsm.dim_tiles,
        }),
    );
    info.extra
        .insert("lsm_data_type".into(), json!(lsm.data_type));
    if lsm.spectral_scan != 0 {
        info.extra.insert("lsm_spectral_scan".into(), json!(true));
    }
    info
}

/// The plane table of position `pos`: its data pages in (t, z) order, a sample of each page
/// per channel for planar pages.
fn lsm_planes(stack: &LsmStack, pos: u32, data_pages: &[usize]) -> Result<Vec<Option<PlaneSrc>>> {
    let (zc, tc, size_c) = (stack.size_z, stack.size_t, stack.size_c);
    let n = u64::from(size_c)
        .saturating_mul(u64::from(zc))
        .saturating_mul(u64::from(tc));
    check_plane_count(n)?;
    let mut planes = vec![None; n as usize];
    for t_ in 0..tc {
        for z_ in 0..zc {
            let k = pos as usize * stack.per_position + (t_ * zc + z_) as usize;
            let Some(&page) = data_pages.get(k) else {
                continue;
            };
            for c_ in 0..size_c {
                let slot = ((t_ * zc + z_) * size_c + c_) as usize;
                planes[slot] = Some(PlaneSrc::Page {
                    file: 0,
                    page,
                    sample: stack.sample_channels.then_some(c_ as u16),
                });
            }
        }
    }
    Ok(planes)
}

/// Find and parse the LSM record on page 0.
pub(crate) fn lsm_record(member: &mut FileSetMember, page0: &Ifd) -> Option<LsmInfo> {
    let f = page0.field(tags::LSM_INFO)?;
    let (t, src) = member.opened.as_mut()?;
    if t.header.byte_order != ByteOrder::Little {
        return None;
    }
    let record = match &f.value {
        FieldValue::Bytes(b) => b.clone(),
        FieldValue::Unsigned(v) if v.len() == 1 => src
            .read_at(v[0], 512.min(src.len.saturating_sub(v[0])))
            .ok()?,
        _ => return None,
    };
    let record = if record.len() < 96 {
        // Some writers store the tag as a 4-byte offset.
        let off = u64::from(openreadout_core::bytes::le_u32(&record, 0)?);
        src.read_at(off, 512.min(src.len.saturating_sub(off)))
            .ok()?
    } else {
        record
    };
    parse(&record, src)
}
