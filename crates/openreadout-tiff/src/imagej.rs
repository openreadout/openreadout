//! ImageJ `ImageDescription` (`ImageJ=…` key=value lines) and the ImageJ binary metadata
//! tags. See `docs/formats/tiff.md` § ImageJ.

use openreadout_core::bytes::{Endian, be_u32, utf16};
use openreadout_core::model::{ChannelInfo, ImageInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::container::Ifd;
use crate::dataset::{PlaneSrc, Series, TiffDataset, check_plane_count, instrument_from_tags};
use crate::decode::PageLayout;
use crate::{FORMAT_ID, tags};

/// Parsed ImageJ description. Absent keys keep ImageJ's defaults (1).
#[derive(Debug, Clone, Default)]
pub struct ImageJInfo {
    /// Every key as written, values typed (int, float, bool, string).
    pub keys: Map<String, Value>,
    pub images: u32,
    pub channels: u32,
    pub slices: u32,
    pub frames: u32,
    pub hyperstack: bool,
    /// Plane order of the pages, `czt` (default) or another permutation.
    pub order: String,
    pub unit: Option<String>,
    pub spacing: Option<f64>,
    pub finterval: Option<f64>,
    pub mode: Option<String>,
}

/// Parse the description; `None` when it is not an ImageJ description.
pub fn parse_description(desc: &str) -> Option<ImageJInfo> {
    let mut keys = Map::new();
    for line in desc.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        let val = if let Ok(i) = v.parse::<i64>() {
            Value::from(i)
        } else if let Some(f) = v.parse::<f64>().ok().filter(|f| f.is_finite()) {
            Value::from(f)
        } else if v.eq_ignore_ascii_case("true") {
            Value::Bool(true)
        } else if v.eq_ignore_ascii_case("false") {
            Value::Bool(false)
        } else {
            Value::String(v.to_string())
        };
        keys.insert(k.to_string(), val);
    }
    if !keys.contains_key("ImageJ") && !keys.contains_key("SCIFIO") {
        return None;
    }
    let int = |k: &str| {
        keys.get(k)
            .and_then(Value::as_i64)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|&v| v > 0)
    };
    let float = |k: &str| keys.get(k).and_then(Value::as_f64);
    let text = |k: &str| keys.get(k).and_then(Value::as_str).map(str::to_string);
    Some(ImageJInfo {
        images: int("images").unwrap_or(1),
        channels: int("channels").unwrap_or(1),
        slices: int("slices").unwrap_or(1),
        frames: int("frames").unwrap_or(1),
        hyperstack: keys
            .get("hyperstack")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        order: text("order")
            .unwrap_or_else(|| "czt".into())
            .to_ascii_lowercase(),
        unit: text("unit").map(|u| unescape_unit(&u)),
        spacing: float("spacing").filter(|v| *v > 0.0),
        finterval: float("finterval")
            .filter(|v| *v > 0.0)
            .or_else(|| float("fps").filter(|v| *v > 0.0).map(|f| 1.0 / f)),
        mode: text("mode"),
        keys,
    })
}

/// ImageJ writes `µm` for µm.
fn unescape_unit(u: &str) -> String {
    u.replace("\\u00B5", "µ").replace("\\u00b5", "µ")
}

/// Factor from an ImageJ unit name to micrometres (`None` for `pixel` or unknown units).
pub fn unit_to_um(unit: &str) -> Option<f64> {
    Some(match unit.trim().to_ascii_lowercase().as_str() {
        "micron" | "microns" | "um" | "µm" | "μm" | "micrometer" | "micrometre" => 1.0,
        "nm" | "nanometer" | "nanometre" => 1e-3,
        "mm" | "millimeter" | "millimetre" => 1e3,
        "cm" | "centimeter" | "centimetre" => 1e4,
        "m" | "meter" | "metre" => 1e6,
        "a" | "å" | "angstrom" => 1e-4,
        "inch" | "in" => 25_400.0,
        _ => return None,
    })
}

impl ImageJInfo {
    /// Page index of plane (c, z, t) for the declared `order` (default `czt`: c fastest).
    pub fn page_of(&self, c: u32, z: u32, t: u32) -> u64 {
        let (cs, zs, ts) = (
            u64::from(self.channels),
            u64::from(self.slices),
            u64::from(self.frames),
        );
        let (c, z, t) = (u64::from(c), u64::from(z), u64::from(t));
        match self.order.as_str() {
            "ctz" => c + cs * (t + ts * z),
            "zct" => z + zs * (c + cs * t),
            "ztc" => z + zs * (t + ts * c),
            "tcz" => t + ts * (c + cs * z),
            "tzc" => t + ts * (z + zs * c),
            _ => c + cs * (z + zs * t),
        }
    }
}

/// Decode the ImageJ binary metadata (tags 50838/50839): `info` text and slice `labels`.
/// Layout: a header block (`IJIJ` magic, then (type, count) pairs), followed by the entries
/// whose byte lengths are listed in tag 50838; strings are UTF-16 big-endian.
pub fn parse_binary(counts: &[u64], data: &[u8]) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(&header_len) = counts.first() else {
        return out;
    };
    let header_len = header_len as usize;
    if header_len < 4 || data.len() < header_len || &data[..4] != b"IJIJ" {
        return out;
    }
    let mut types = Vec::new();
    let mut i = 4;
    while i + 8 <= header_len {
        types.push((
            be_u32(data, i).unwrap_or(0),
            be_u32(data, i + 4).unwrap_or(0),
        ));
        i += 8;
    }
    let mut pos = header_len;
    let mut entry = 1usize;
    for (ty, n) in types {
        let name = match ty {
            0x696e_666f => "info",
            0x6c61_626c => "labels",
            0x7261_6e67 => "ranges",
            0x6c75_7473 => "luts",
            0x706c_6f74 => "plot",
            0x726f_6920 => "roi",
            0x6f76_6572 => "overlays",
            0x7072_6f70 => "properties",
            _ => "unknown",
        };
        let mut items = Vec::new();
        for _ in 0..n {
            let Some(&len) = counts.get(entry) else {
                return out;
            };
            entry += 1;
            let len = len as usize;
            let Some(b) = data.get(pos..pos + len) else {
                return out;
            };
            pos += len;
            match name {
                "info" | "labels" | "properties" => {
                    items.push(Value::String(utf16(b, Endian::Big)));
                }
                "ranges" => {
                    for c in b.as_chunks::<8>().0 {
                        items.push(Value::from(f64::from_be_bytes(*c)));
                    }
                }
                _ => items.push(Value::from(len)),
            }
        }
        out.insert(name.into(), Value::Array(items));
    }
    out
}

/// The stack an ImageJ description and page 0 describe.
struct ImageJStack {
    size_c: u32,
    size_z: u32,
    size_t: u32,
    samples_per_pixel: u32,
    /// Planar multi-sample pages: each sample is a channel.
    sample_channels: bool,
}

impl ImageJStack {
    /// Channels from the description (or the samples of planar pages); a plain stack
    /// (`images=N` without hyperstack sizes) is exposed as Z, and extra images over the
    /// declared sizes multiply Z.
    fn of(ij: &ImageJInfo, l: &PageLayout, pages: usize) -> Self {
        let (mut c, mut z, t) = (ij.channels, ij.slices, ij.frames);
        let mut samples_per_pixel = 1u32;
        let mut sample_channels = false;
        if l.samples_per_pixel > 1 {
            if l.planar == 2 {
                sample_channels = true;
                c = u32::from(l.samples_per_pixel);
            } else {
                samples_per_pixel = u32::from(l.samples_per_pixel);
                if c == samples_per_pixel {
                    c = 1;
                }
            }
        }
        let declared = u64::from(c)
            .saturating_mul(u64::from(z))
            .saturating_mul(u64::from(t));
        let per_page = if sample_channels { u64::from(c) } else { 1 };
        let images = if ij.images > 1 {
            u64::from(ij.images)
        } else {
            pages as u64 * per_page
        };
        if declared == 1 && images > 1 {
            z = (images / per_page) as u32;
        } else if declared < images && images % declared == 0 && !sample_channels {
            z = z.saturating_mul(u32::try_from(images / declared).unwrap_or(u32::MAX));
        }
        ImageJStack {
            size_c: c.max(1),
            size_z: z.max(1),
            size_t: t.max(1),
            samples_per_pixel,
            sample_channels,
        }
    }

    /// Planes stored per page.
    fn per_page(&self) -> u64 {
        if self.sample_channels {
            u64::from(self.size_c)
        } else {
            1
        }
    }
}

impl TiffDataset {
    pub(crate) fn build_imagej(&mut self, ij: &ImageJInfo, page0: &Ifd) -> Result<()> {
        let order = self.main().header.byte_order;
        let pages = self.main().ifds.len();
        let l = PageLayout::from_ifd(page0, order)?;
        let pt = l.pixel_type()?;
        let stack = ImageJStack::of(ij, &l, pages);
        let mut info = ImageInfo::new(0, l.width, l.height, pt);
        info.size_c = stack.size_c;
        info.size_z = stack.size_z;
        info.size_t = stack.size_t;
        info.samples_per_pixel = stack.samples_per_pixel;
        imagej_metadata(&mut info, ij, page0);
        let n_planes = u64::from(info.size_c)
            .saturating_mul(u64::from(info.size_z))
            .saturating_mul(u64::from(info.size_t));
        let contiguous = pages as u64 * stack.per_page() < n_planes;
        if contiguous {
            let data_ok = l.compression == 1
                && l.offsets
                    .windows(2)
                    .zip(&l.byte_counts)
                    .all(|(w, &n)| w[0] + n == w[1]);
            if !data_ok {
                return Err(Error::corrupt(
                    FORMAT_ID,
                    format!(
                        "ImageJ description declares {n_planes} planes but the file has {pages} pages and page 0's data is not a contiguous uncompressed block"
                    ),
                ));
            }
            self.notes.push("ImageJ stack stored contiguously after the first page (pages beyond the first are not written)".into());
        }
        check_plane_count(n_planes)?;
        let planes = imagej_planes(ij, &stack, contiguous);
        self.series.push(Series {
            info: info.finish(),
            planes,
            levels: Vec::new(),
        });
        self.set_provenance(&[
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_z", Source::PriorArt),
            ("images[].size_c", Source::PriorArt),
            ("images[].size_t", Source::PriorArt),
            ("images[].dimension_order", Source::PriorArt),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::PriorArt),
            ("images[].time_increment_s", Source::PriorArt),
            ("images[].channels[].name", Source::PriorArt),
        ]);
        Ok(())
    }
}

/// Dimension order, calibration, frame interval, channel labels (from the binary metadata)
/// and the ImageJ keys kept in `extra`.
fn imagej_metadata(info: &mut ImageInfo, ij: &ImageJInfo, page0: &Ifd) {
    info.dimension_order = match ij.order.as_str() {
        "ctz" => "XYCTZ",
        "zct" => "XYZCT",
        "ztc" => "XYZTC",
        "tcz" => "XYTCZ",
        "tzc" => "XYTZC",
        _ => "XYCZT",
    }
    .into();
    let unit_f = ij.unit.as_deref().and_then(unit_to_um);
    let (rx, ry) = (
        page0.float(tags::X_RESOLUTION).filter(|r| *r > 0.0),
        page0.float(tags::Y_RESOLUTION).filter(|r| *r > 0.0),
    );
    info.physical_size = PhysicalSize::micrometres(
        unit_f.and_then(|f| rx.map(|r| f / r)),
        unit_f.and_then(|f| ry.map(|r| f / r)),
        unit_f.and_then(|f| ij.spacing.map(|s| s * f)),
    );
    let tunit = ij
        .keys
        .get("tunit")
        .and_then(Value::as_str)
        .and_then(|u| {
            crate::ome::time_to_s(Some(match u {
                "sec" => "s",
                "msec" => "ms",
                other => other,
            }))
        })
        .unwrap_or(1.0);
    info.time_increment_s = ij.finterval.map(|f| f * tunit);
    let binary = match (
        page0.uints(tags::IMAGEJ_META_COUNTS),
        page0.bytes(tags::IMAGEJ_META),
    ) {
        (Some(counts), Some(data)) => parse_binary(&counts, data),
        _ => Map::new(),
    };
    let labels: Vec<String> = binary
        .get("labels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    info.channels = (0..info.size_c)
        .map(|ci| ChannelInfo {
            index: ci,
            name: (labels.len() == info.size_c as usize)
                .then(|| labels[ci as usize].clone())
                .filter(|s| !s.is_empty()),
            ..ChannelInfo::default()
        })
        .collect();
    info.instrument = instrument_from_tags(page0);
    if let Some(m) = &ij.mode {
        info.extra.insert("imagej_mode".into(), json!(m));
    }
    info.extra
        .insert("imagej_hyperstack".into(), json!(ij.hyperstack));
    if let Some(v) = ij.keys.get("ImageJ") {
        info.extra.insert("imagej_version".into(), v.clone());
    }
}

/// The plane table: pages in the description's order, a sample of a page per channel for
/// planar pages, or planes back to back after page 0 for a contiguous stack.
fn imagej_planes(ij: &ImageJInfo, stack: &ImageJStack, contiguous: bool) -> Vec<Option<PlaneSrc>> {
    let ijc = ImageJInfo {
        channels: if stack.sample_channels {
            1
        } else {
            stack.size_c
        },
        slices: stack.size_z,
        frames: stack.size_t,
        ..ij.clone()
    };
    let (nc, nz, nt) = (stack.size_c, stack.size_z, stack.size_t);
    let mut planes = vec![None; (nc as usize) * (nz as usize) * (nt as usize)];
    for tt in 0..nt {
        for zz in 0..nz {
            for cc in 0..nc {
                let slot = ((tt as usize * nz as usize) + zz as usize) * nc as usize + cc as usize;
                planes[slot] = Some(if contiguous {
                    PlaneSrc::Contiguous {
                        file: 0,
                        index: ijc.page_of(if stack.sample_channels { 0 } else { cc }, zz, tt),
                    }
                } else if stack.sample_channels {
                    PlaneSrc::Page {
                        file: 0,
                        page: ijc.page_of(0, zz, tt) as usize,
                        sample: Some(cc as u16),
                    }
                } else {
                    PlaneSrc::Page {
                        file: 0,
                        page: ijc.page_of(cc, zz, tt) as usize,
                        sample: None,
                    }
                });
            }
        }
    }
    planes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyperstack_description() {
        let d = parse_description(
            "ImageJ=1.52o\nimages=30\nchannels=3\nframes=10\nhyperstack=true\nunit=micron\nfinterval=0.742\n",
        )
        .unwrap();
        assert_eq!((d.channels, d.slices, d.frames), (3, 1, 10));
        assert!(d.hyperstack);
        assert_eq!(d.unit.as_deref(), Some("micron"));
        assert_eq!(d.page_of(2, 0, 1), 5);
        assert!(parse_description("not imagej").is_none());
    }
}
