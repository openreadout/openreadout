//! `Dataset` for Zeiss AxioVision `.zvi` files: an MS-CFB compound file whose `Image/Item(n)`
//! storages each hold one plane (`Contents`: typed values, a 28-byte raw-image header, the
//! samples) and its tag list (`Tags/Contents`). Layout: `docs/formats/zvi.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{find, le_u32};
use openreadout_core::cfb::{Cfb, CfbEntry};
use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Value, json};

use crate::tags::{TagList, parse_tags};
use crate::{FORMAT_ID, ZviReader};

/// Marks the raw-image header that precedes the samples in an item's `Contents` stream.
pub const IMAGE_MARKER: u32 = 0x1000_2000;
/// Bytes of the raw-image header: marker, width, height, depth, bytes per pixel, pixel format,
/// valid bits (seven little-endian u32).
pub const IMAGE_HEADER_LEN: usize = 28;
/// How far into an item's `Contents` stream the header is searched for.
const HEADER_SEARCH: u64 = 64 << 10;
/// Largest tag or header stream read.
const MAX_SMALL_STREAM: u64 = 16 << 20;

// Tag ids, identified by comparing tag values across corpus files and with Bio-Formats' output
// (black box); names are ours (docs/formats/zvi.md).
pub const TAG_WIDTH: u32 = 515;
pub const TAG_HEIGHT: u32 = 516;
pub const TAG_PIXEL_FORMAT: u32 = 518;
pub const TAG_Z_INDEX: u32 = 2819;
pub const TAG_C_INDEX: u32 = 2820;
pub const TAG_T_INDEX: u32 = 2821;
/// Further per-item indices (0 in every corpus file); items that differ in them become
/// separate images.
pub const POSITION_TAGS: [u32; 3] = [2822, 2823, 2827];
pub const TAG_CHANNEL_NAME: u32 = 1284;
pub const TAG_CHANNEL_COLOR: u32 = 1282;
pub const TAG_EXPOSURE_MS: u32 = 2564;
pub const TAG_EXCITATION_NM: u32 = 0x0100_0110;
pub const TAG_EMISSION_NM: u32 = 0x0100_0111;
pub const TAG_SCALE_X: u32 = 769;
pub const TAG_UNIT_X: u32 = 770;
pub const TAG_SCALE_Y: u32 = 772;
pub const TAG_UNIT_Y: u32 = 773;
pub const TAG_SCALE_Z: u32 = 775;
pub const TAG_UNIT_Z: u32 = 776;
/// Scale unit code meaning micrometres (the only one seen with a calibrated scale).
pub const UNIT_MICROMETRE: i64 = 76;
pub const TAG_ACQUIRED: u32 = 1025;
pub const TAG_RELATIVE_TIME: u32 = 300;
pub const TAG_OBJECTIVE_NAME: u32 = 2049;
pub const TAG_OBJECTIVE_MAGNIFICATION: u32 = 2076;
pub const TAG_OBJECTIVE_NA: u32 = 2077;
pub const TAG_MICROSCOPE: u32 = 2075;
pub const TAG_CAMERA: u32 = 1042;
pub const TAG_FILE_NAME: u32 = 1553;
pub const TAG_CENTER_X: u32 = 2073;
pub const TAG_CENTER_Y: u32 = 2074;

/// One stored plane (`Image/Item(n)`).
#[derive(Debug, Clone, PartialEq)]
pub struct ZviItem {
    /// `n` of `Item(n)`.
    pub number: u32,
    pub width: u32,
    pub height: u32,
    pub bytes_per_pixel: u32,
    pub pixel_format: u32,
    pub valid_bits: u32,
    /// Offset of the samples in the item's `Contents` stream.
    pub data_offset: u64,
    /// Size of the item's `Contents` stream.
    pub stream_size: u64,
    /// Raw z, c, t indices as the tags store them.
    pub z: i64,
    pub c: i64,
    pub t: i64,
    /// Values of [`POSITION_TAGS`].
    pub position: [i64; 3],
    pub tags: TagList,
}

/// How samples are laid out, from the header's bytes per pixel and pixel format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleLayout {
    pub pixel_type: PixelType,
    pub samples_per_pixel: u32,
    /// Colour samples are stored blue, green, red (swapped to red, green, blue on read).
    pub bgr: bool,
}

/// Sample layout for (bytes per pixel, pixel format). Formats 4 (16-bit grey) and 8 (3 × 16-bit
/// colour) are seen in the corpus; 8-bit grey and 3 × 8-bit colour are inferred by analogy.
pub fn sample_layout(bytes_per_pixel: u32, pixel_format: u32) -> Option<SampleLayout> {
    let (pixel_type, spp) = match (bytes_per_pixel, pixel_format) {
        (2, _) => (PixelType::Uint16, 1),
        (1, _) => (PixelType::Uint8, 1),
        (6, _) => (PixelType::Uint16, 3),
        (3, _) => (PixelType::Uint8, 3),
        _ => return None,
    };
    Some(SampleLayout {
        pixel_type,
        samples_per_pixel: spp,
        bgr: spp == 3,
    })
}

/// One image: the items sharing [`POSITION_TAGS`] values.
#[derive(Debug, Clone)]
pub struct ZviImage {
    pub position: [i64; 3],
    /// (c, z, t) ordinal → item number.
    pub planes: BTreeMap<(u32, u32, u32), u32>,
    pub layout: Option<SampleLayout>,
    pub info: ImageInfo,
}

/// An opened ZVI file.
pub struct ZviDataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    cfb: Cfb,
    pub items: Vec<ZviItem>,
    pub images: Vec<ZviImage>,
    image_tags: TagList,
    root_tags: Option<TagList>,
    thumbnail: Option<(CfbEntry, u64, u64)>,
    notes: Vec<String>,
}

impl std::fmt::Debug for ZviDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZviDataset")
            .field("path", &self.path)
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

fn corrupt(d: impl Into<String>) -> Error {
    Error::corrupt(FORMAT_ID, d.into())
}

/// ZVI colour (`0x00BBGGRR`) → `#RRGGBB`.
pub fn zvi_color(v: i64) -> Option<String> {
    let v = u32::try_from(v).ok()?;
    (v <= 0x00FF_FFFF).then(|| {
        format!(
            "#{:02X}{:02X}{:02X}",
            v & 0xFF,
            v >> 8 & 0xFF,
            v >> 16 & 0xFF
        )
    })
}

/// OLE date (days since 1899-12-30, local time) → ISO-8601 without a zone.
pub fn ole_date(days: f64) -> Option<String> {
    if !(1.0..=2_958_465.0).contains(&days) {
        return None;
    }
    let ms_total = (days * 86_400_000.0).round() as i64;
    let unix_ms = ms_total - 25_569 * 86_400_000;
    let iso = openreadout_core::time::unix_to_iso8601(
        unix_ms.div_euclid(1000),
        unix_ms.rem_euclid(1000) as u32,
    );
    Some(iso.trim_end_matches('Z').to_string())
}

/// Find the raw-image header in the first bytes of an item's `Contents` stream.
fn find_header(head: &[u8], stream_size: u64) -> Option<(u64, [u32; 6])> {
    let marker = IMAGE_MARKER.to_le_bytes();
    let mut at = 0usize;
    while let Some(found) = find(head.get(at..)?, &marker) {
        let start = at + found;
        let fields: Option<Vec<u32>> = (1..7).map(|i| le_u32(head, start + 4 * i)).collect();
        if let Some(v) = fields {
            let (width, height, depth, bpp) = (
                u64::from(v[0]),
                u64::from(v[1]),
                u64::from(v[2].max(1)),
                u64::from(v[3]),
            );
            let data = (start + IMAGE_HEADER_LEN) as u64;
            if width > 0
                && height > 0
                && bpp > 0
                && width
                    .checked_mul(height)
                    .and_then(|n| n.checked_mul(depth))
                    .and_then(|n| n.checked_mul(bpp))
                    .and_then(|n| n.checked_add(data))
                    .is_some_and(|end| end <= stream_size)
            {
                return Some((data, [v[0], v[1], v[2], v[3], v[4], v[5]]));
            }
        }
        at = start + 1;
    }
    None
}

impl ZviDataset {
    /// Open a ZVI file: the compound-file directory, every item's tags and raw-image header.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
        let cfb = Cfb::open(&mut f, path, FORMAT_ID)?;
        let read_small = |f: &mut SourceFile, e: &CfbEntry| cfb.read(f, path, e, MAX_SMALL_STREAM);
        let image_tags = cfb
            .stream("Image/Tags/Contents")
            .cloned()
            .and_then(|e| read_small(&mut f, &e).ok())
            .and_then(|b| parse_tags(&b))
            .unwrap_or_default();
        let root_tags = cfb
            .stream("Tags")
            .cloned()
            .and_then(|e| read_small(&mut f, &e).ok())
            .and_then(|b| b.get(16..).and_then(parse_tags));
        let mut numbers: Vec<u32> = cfb
            .entries
            .iter()
            .filter(|e| e.is_stream)
            .filter_map(|e| {
                let rest = e.path.strip_prefix("Image/Item(")?;
                let (n, tail) = rest.split_once(')')?;
                (tail == "/Contents").then(|| n.parse().ok())?
            })
            .collect();
        numbers.sort_unstable();
        if numbers.is_empty() {
            return Err(if cfb.stream("Image/Contents").is_some() {
                corrupt("the Image storage holds no Item(n)/Contents stream (no planes)")
            } else {
                Error::unsupported(
                    FORMAT_ID,
                    "a compound file without an Image storage",
                    "This is an OLE2 compound file but not a ZVI image; `info --view structure` on it is not available.",
                )
            });
        }
        let mut notes = Vec::new();
        let mut items = Vec::new();
        for n in numbers {
            let ce = cfb
                .stream(&format!("Image/Item({n})/Contents"))
                .cloned()
                .ok_or_else(|| corrupt(format!("Item({n}) disappeared")))?;
            let head = cfb.read(&mut f, path, &ce, HEADER_SEARCH)?;
            let Some((data_offset, hdr)) = find_header(&head, ce.size) else {
                notes.push(format!(
                    "Image/Item({n}) holds no raw image header (or its samples are cut short); skipped"
                ));
                continue;
            };
            let tags = cfb
                .stream(&format!("Image/Item({n})/Tags/Contents"))
                .cloned()
                .and_then(|e| read_small(&mut f, &e).ok())
                .and_then(|b| parse_tags(&b))
                .unwrap_or_default();
            let idx = |id| tags.i64(id).unwrap_or(0);
            items.push(ZviItem {
                number: n,
                width: hdr[0],
                height: hdr[1],
                bytes_per_pixel: hdr[3],
                pixel_format: hdr[4],
                valid_bits: hdr[5],
                data_offset,
                stream_size: ce.size,
                z: idx(TAG_Z_INDEX),
                c: idx(TAG_C_INDEX),
                t: idx(TAG_T_INDEX),
                position: POSITION_TAGS.map(idx),
                tags,
            });
        }
        if items.is_empty() {
            return Err(corrupt("no Image/Item(n) stream holds a readable plane"));
        }
        // group by position indices → images
        let positions: BTreeSet<[i64; 3]> = items.iter().map(|i| i.position).collect();
        let mut ds = ZviDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            cfb,
            items,
            images: Vec::new(),
            image_tags,
            root_tags,
            thumbnail: None,
            notes,
        };
        for (k, pos) in positions.iter().enumerate() {
            let img = ds.build_image(k as u32, *pos);
            ds.images.push(img);
        }
        if positions.len() > 1 {
            ds.notes.push(format!(
                "items differ in index tags {POSITION_TAGS:?}; each combination is exposed as a separate image (inferred, no corpus file has this)"
            ));
        }
        if ds.items.iter().any(|i| matches!(i.bytes_per_pixel, 1 | 3)) {
            ds.notes.push(
                "8-bit samples (grey, or 3 x 8-bit colour stored B, G, R): this layout is inferred by analogy with the validated 16-bit ones; no public file had it"
                    .into(),
            );
        }
        if ds.images.iter().any(|i| i.info.size_t > 1) {
            ds.notes.push(
                "the time index (tag 2821) is inferred: no validated file has a time series; check frames[].delta_t_s"
                    .into(),
            );
        }
        if ds.images.iter().any(|i| i.info.acquired_at.is_some()) {
            ds.notes.push(
                "acquired_at is the ZVI acquisition date, a local wall-clock time without a time zone"
                    .into(),
            );
        }
        if let Some(e) = ds.cfb.stream("Thumbnail").cloned()
            && let Ok(b) = ds.cfb.read(&mut f, path, &e, 64)
            && b.get(26..28) == Some(b"BM")
            && let Some(n) = le_u32(&b, 28)
            && 26 + u64::from(n) <= e.size
        {
            ds.thumbnail = Some((e, 26, u64::from(n)));
        }
        Ok(ds)
    }

    fn build_image(&mut self, index: u32, pos: [i64; 3]) -> ZviImage {
        let items: Vec<&ZviItem> = self.items.iter().filter(|i| i.position == pos).collect();
        let ord = |f: &dyn Fn(&ZviItem) -> i64| -> BTreeMap<i64, u32> {
            let set: BTreeSet<i64> = items.iter().map(|i| f(i)).collect();
            set.into_iter()
                .enumerate()
                .map(|(k, v)| (v, k as u32))
                .collect()
        };
        let zs = ord(&|i| i.z);
        let cs = ord(&|i| i.c);
        let ts = ord(&|i| i.t);
        let mut planes = BTreeMap::new();
        let mut duplicates = 0;
        for it in &items {
            let key = (cs[&it.c], zs[&it.z], ts[&it.t]);
            if planes.insert(key, it.number).is_some() {
                duplicates += 1;
            }
        }
        let first = items[0];
        let layout = sample_layout(first.bytes_per_pixel, first.pixel_format);
        let mut info = ImageInfo::new(
            index,
            first.width,
            first.height,
            layout.map_or(PixelType::Uint8, |l| l.pixel_type),
        );
        info.size_z = zs.len() as u32;
        info.size_c = cs.len() as u32;
        info.size_t = ts.len() as u32;
        info.samples_per_pixel = layout.map_or(1, |l| l.samples_per_pixel);
        // storage order: the axis that changes first between consecutive items is fastest
        let change = |f: &dyn Fn(&ZviItem) -> i64| {
            items
                .iter()
                .position(|i| f(i) != f(first))
                .unwrap_or(usize::MAX)
        };
        let mut axes = [
            (change(&|i| i.z), 'Z'),
            (change(&|i| i.c), 'C'),
            (change(&|i| i.t), 'T'),
        ];
        axes.sort_by_key(|(k, a)| (*k, "CZT".find(*a).unwrap_or(0)));
        info.dimension_order = format!("XY{}", axes.iter().map(|(_, a)| a).collect::<String>());
        let it = &self.image_tags;
        let scale = |s: u32, u: u32| {
            (it.i64(u) == Some(UNIT_MICROMETRE))
                .then(|| it.f64(s))
                .flatten()
        };
        info.physical_size = PhysicalSize::micrometres(
            scale(TAG_SCALE_X, TAG_UNIT_X),
            scale(TAG_SCALE_Y, TAG_UNIT_Y),
            if info.size_z > 1 {
                scale(TAG_SCALE_Z, TAG_UNIT_Z)
            } else {
                None
            },
        );
        for (u, axis) in [(TAG_UNIT_X, "x"), (TAG_UNIT_Y, "y"), (TAG_UNIT_Z, "z")] {
            if let Some(code) = it.i64(u).filter(|c| *c != 0 && *c != UNIT_MICROMETRE)
                && !self.notes.iter().any(|n| n.contains("scale unit code"))
            {
                self.notes.push(format!(
                    "scale unit code {code} on the {axis} axis is not known: that physical size is not reported"
                ));
            }
        }
        // channels, in ascending order of the stored channel index
        for (raw, &ci) in &cs {
            let rep = items.iter().find(|i| i.c == *raw).copied().unwrap_or(first);
            let t = &rep.tags;
            // Item tags first; single-channel files (colour snapshots) keep some values only
            // in the image tags.
            let single = cs.len() == 1;
            let f64_of = |id: u32| t.f64(id).or_else(|| single.then(|| it.f64(id)).flatten());
            info.channels.push(ChannelInfo {
                index: ci,
                name: t
                    .text(TAG_CHANNEL_NAME)
                    .or_else(|| single.then(|| it.text(TAG_CHANNEL_NAME)).flatten()),
                color: t.i64(TAG_CHANNEL_COLOR).and_then(zvi_color),
                exposure_ms: f64_of(TAG_EXPOSURE_MS).filter(|v| *v >= 0.0),
                excitation_nm: f64_of(TAG_EXCITATION_NM).filter(|v| *v > 0.0),
                emission_nm: f64_of(TAG_EMISSION_NM).filter(|v| *v > 0.0),
                ..ChannelInfo::default()
            });
        }
        let anyt = |id: u32| {
            items
                .iter()
                .find_map(|i| i.tags.get(id))
                .or_else(|| it.get(id))
        };
        info.acquired_at = anyt(TAG_ACQUIRED)
            .and_then(crate::tags::TagValue::as_f64)
            .and_then(ole_date);
        let obj_name = anyt(TAG_OBJECTIVE_NAME)
            .and_then(|v| v.as_text())
            .map(str::to_string);
        let mag = anyt(TAG_OBJECTIVE_MAGNIFICATION)
            .and_then(crate::tags::TagValue::as_f64)
            .filter(|v| *v > 0.0);
        let na = anyt(TAG_OBJECTIVE_NA)
            .and_then(crate::tags::TagValue::as_f64)
            .filter(|v| *v > 0.0);
        if obj_name.is_some() || mag.is_some() || na.is_some() {
            info.objective = Some(ObjectiveInfo {
                model: obj_name,
                nominal_magnification: mag,
                lens_na: na,
                ..ObjectiveInfo::default()
            });
        }
        let scope = anyt(TAG_MICROSCOPE)
            .and_then(|v| v.as_text())
            .map(str::to_string);
        let camera = anyt(TAG_CAMERA)
            .and_then(|v| v.as_text())
            .map(str::to_string);
        if scope.is_some() || camera.is_some() {
            info.instrument = Some(InstrumentInfo {
                manufacturer: Some("Carl Zeiss".into()),
                model: scope,
                software: Some("AxioVision".into()),
                detector: camera,
                ..InstrumentInfo::default()
            });
        }
        info.name = it.text(TAG_FILE_NAME);
        // time increment from the relative times of the first channel and z
        if info.size_t > 1 {
            let mut times: Vec<(u32, f64)> = items
                .iter()
                .filter(|i| cs[&i.c] == 0 && zs[&i.z] == 0)
                .filter_map(|i| Some((ts[&i.t], i.tags.f64(TAG_RELATIVE_TIME)? * 86_400.0)))
                .collect();
            times.sort_by_key(|x| x.0);
            if times.len() > 1 {
                let (a, b) = (times[0], times[times.len() - 1]);
                let dt = (b.1 - a.1) / f64::from(b.0 - a.0);
                if dt > 0.0 && dt.is_finite() {
                    info.time_increment_s = Some(dt);
                }
            }
        }
        let ex = &mut info.extra;
        ex.insert("bytes_per_pixel".into(), json!(first.bytes_per_pixel));
        ex.insert("pixel_format".into(), json!(first.pixel_format));
        ex.insert("bits_significant".into(), json!(first.valid_bits));
        if pos != [0, 0, 0] {
            ex.insert("position_indices".into(), json!(pos));
        }
        if let (Some(x), Some(y)) = (first.tags.f64(TAG_CENTER_X), first.tags.f64(TAG_CENTER_Y)) {
            ex.insert("center_um".into(), json!({"x": x, "y": y}));
        }
        ex.insert(
            "stored_indices".into(),
            json!({
                "z": zs.keys().collect::<Vec<_>>(),
                "c": cs.keys().collect::<Vec<_>>(),
                "t": ts.keys().collect::<Vec<_>>(),
            }),
        );
        if layout.is_none() {
            ex.insert("unsupported_sample_layout".into(), json!(true));
        }
        let expected = u64::from(info.size_z) * u64::from(info.size_c) * u64::from(info.size_t);
        if duplicates > 0 || (planes.len() as u64) < expected {
            self.notes.push(format!(
                "image {index}: {} items for {expected} (c, z, t) positions ({duplicates} duplicates); missing planes read as errors",
                items.len()
            ));
        }
        ZviImage {
            position: pos,
            planes,
            layout,
            info: info.finish(),
        }
    }

    fn item(&self, number: u32) -> Result<&ZviItem> {
        self.items
            .iter()
            .find(|i| i.number == number)
            .ok_or_else(|| corrupt(format!("Item({number}) is not indexed")))
    }

    fn read_item(&self, f: &mut SourceFile, it: &ZviItem) -> Result<Vec<u8>> {
        let e = self
            .cfb
            .stream(&format!("Image/Item({})/Contents", it.number))
            .cloned()
            .ok_or_else(|| corrupt(format!("Item({}) stream is missing", it.number)))?;
        let n = u64::from(it.width) * u64::from(it.height) * u64::from(it.bytes_per_pixel);
        let all = self.cfb.read(f, &self.path, &e, it.data_offset + n)?;
        let start = usize::try_from(it.data_offset).map_err(|_| corrupt("offset too large"))?;
        all.get(start..)
            .filter(|d| d.len() as u64 == n)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                corrupt(format!(
                    "Item({}): {} sample bytes expected, stream holds {}",
                    it.number,
                    n,
                    all.len().saturating_sub(start)
                ))
            })
    }
}

impl Dataset for ZviDataset {
    fn info(&self) -> Result<FileInfo> {
        let images: Vec<ImageInfo> = self.images.iter().map(|i| i.info.clone()).collect();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: ZviReader.descriptor(),
            format_version: None,
            plane_count: images.iter().map(|i| i.plane_count).sum(),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes: self.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let items: Vec<Value> = self
            .items
            .iter()
            .take(1000)
            .map(|i| json!({"item": i.number, "tags": i.tags.to_json()}))
            .collect();
        Ok(json!({
            "image_tags": self.image_tags.to_json(),
            "document_tags": self.root_tags.as_ref().map(TagList::to_json),
            "items": items,
            "items_total": self.items.len(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "images[].size_x",
            "images[].size_y",
            "images[].size_z",
            "images[].size_c",
            "images[].size_t",
            "images[].pixel_type",
            "images[].samples_per_pixel",
            "images[].dimension_order",
            "images[].channels",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        let any = |f: &dyn Fn(&ImageInfo) -> bool| self.images.iter().any(|i| f(&i.info));
        for (k, filled) in [
            (
                "images[].physical_size",
                any(&|i| !i.physical_size.is_empty()),
            ),
            ("images[].acquired_at", any(&|i| i.acquired_at.is_some())),
            ("images[].objective", any(&|i| i.objective.is_some())),
            ("images[].instrument", any(&|i| i.instrument.is_some())),
            (
                "images[].time_increment_s",
                any(&|i| i.time_increment_s.is_some()),
            ),
            ("images[].name", any(&|i| i.name.is_some())),
        ] {
            if filled {
                p.insert(k.into(), Source::Inferred);
            }
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (k, im) in self.images.iter().enumerate() {
            for (&(c, z, t), &n) in &im.planes {
                let it = self.item(n)?;
                out.push(LsEntry {
                    kind: "plane".into(),
                    name: format!("Image/Item({n})"),
                    offset: None,
                    size: Some(it.stream_size),
                    image: Some(k as u32),
                    details: json!({"c": c, "z": z, "t": t, "stored": {"z": it.z, "c": it.c, "t": it.t}, "width": it.width, "height": it.height, "bytes_per_pixel": it.bytes_per_pixel, "pixel_format": it.pixel_format}),
                });
            }
        }
        for e in self.cfb.entries.iter().filter(|e| e.is_stream) {
            if e.path.starts_with("Image/Item(") && e.path.ends_with(")/Contents") {
                continue;
            }
            out.push(LsEntry {
                kind: "stream".into(),
                name: e.path.clone(),
                offset: None,
                size: Some(e.size),
                image: None,
                details: Value::Null,
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let im = self.images.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {image} out of range (0..{})",
                self.images.len()
            ))
        })?;
        let (sc, sz, st) = (im.info.size_c, im.info.size_z, im.info.size_t);
        if index.c >= sc || index.z >= sz || index.t >= st {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range (c<{sc}, z<{sz}, t<{st})",
                index.c, index.z, index.t
            )));
        }
        let Some(layout) = im.layout else {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!(
                    "ZVI samples of {} bytes per pixel (format {})",
                    im.info.extra["bytes_per_pixel"], im.info.extra["pixel_format"]
                ),
                "Only 8/16-bit grey and 3 × 8/16-bit colour ZVI planes are decoded.",
            ));
        };
        let n = *im.planes.get(&(index.c, index.z, index.t)).ok_or_else(|| {
            corrupt(format!(
                "no Item holds plane c={} z={} t={}",
                index.c, index.z, index.t
            ))
        })?;
        let it = self.item(n)?.clone();
        if (it.width, it.height) != (im.info.size_x, im.info.size_y) {
            return Err(corrupt(format!(
                "Item({n}) is {}x{}, the image is {}x{}",
                it.width, it.height, im.info.size_x, im.info.size_y
            )));
        }
        let mut f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        let mut data = self.read_item(&mut f, &it)?;
        if layout.bgr {
            let bps = layout.pixel_type.bytes_per_sample();
            for px in data.chunks_exact_mut(3 * bps) {
                let (b, rest) = px.split_at_mut(bps);
                let r = &mut rest[bps..];
                b.swap_with_slice(r);
            }
        }
        Ok(Plane {
            width: it.width,
            height: it.height,
            pixel_type: layout.pixel_type,
            samples_per_pixel: layout.samples_per_pixel,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("compound-file header, FAT and directory parse (MS-CFB)");
        r.performed("every Image/Item(n)/Contents stream has a raw-image header and its full sample block, reachable through its sector chain");
        r.performed("every (c, z, t) position of every image has exactly one item; item sizes match the image");
        for p in &self.cfb.problems {
            r.push(Finding::error("compound_file", p.clone()));
        }
        for n in &self.notes {
            if n.contains("skipped") || n.contains("missing planes") {
                r.push(Finding::error("planes", n.clone()));
            }
        }
        let mut f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        for it in self.items.clone() {
            if it.tags.unparsed > 0 {
                r.push(Finding::warning(
                    "tags",
                    format!(
                        "Item({}): {} tag entries could not be parsed",
                        it.number, it.tags.unparsed
                    ),
                ));
            }
            if let Err(e) = self.read_item(&mut f, &it) {
                r.push(Finding::error("truncated", e.to_string()));
            }
        }
        Ok(r)
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        Ok(self
            .thumbnail
            .iter()
            .map(|(_, off, n)| AttachmentInfo {
                index: 0,
                name: "Thumbnail".into(),
                content_type: "BMP".into(),
                extension: "bmp".into(),
                offset: Some(*off),
                size: *n,
                extra: BTreeMap::new(),
            })
            .collect())
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let Some((e, off, n)) = self.thumbnail.clone().filter(|_| index == 0) else {
            return Err(Error::Usage(format!(
                "attachment #{index} does not exist; `info --view structure` lists them"
            )));
        };
        let mut f = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        let b = self.cfb.read(&mut f, &self.path, &e, off + n)?;
        b.get(off as usize..)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| corrupt("thumbnail stream is short"))
    }

    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        let Some(im) = self.images.get(image as usize) else {
            return Ok((0, Vec::new()));
        };
        let total = im.planes.len() as u64;
        let mut out = Vec::new();
        for (&(c, z, t), &n) in im.planes.iter().take(limit.unwrap_or(usize::MAX)) {
            let it = self.item(n)?;
            let mut rec = serde_json::Map::new();
            rec.insert("c".into(), json!(c));
            rec.insert("z".into(), json!(z));
            rec.insert("t".into(), json!(t));
            rec.insert("frame".into(), json!(n));
            if let Some(d) = it.tags.f64(TAG_RELATIVE_TIME).filter(|d| *d >= 0.0) {
                rec.insert("delta_t_s".into(), json!(d * 86_400.0));
            }
            if let Some(e) = it.tags.f64(TAG_EXPOSURE_MS).filter(|e| *e >= 0.0) {
                rec.insert("exposure_ms".into(), json!(e));
            }
            out.push(Value::Object(rec));
        }
        Ok((total, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(zvi_color(65280).as_deref(), Some("#00FF00"));
        assert_eq!(zvi_color(16_711_680).as_deref(), Some("#0000FF"));
        assert_eq!(zvi_color(-1), None);
        assert_eq!(
            ole_date(45_021.5).as_deref(),
            Some("2023-04-05T12:00:00.000")
        );
        assert_eq!(ole_date(0.0), None);
        let mut s = vec![0u8; 40];
        s[8..12].copy_from_slice(&IMAGE_MARKER.to_le_bytes());
        for (i, v) in [2u32, 2, 1, 1, 3, 8].iter().enumerate() {
            s[12 + 4 * i..16 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        assert_eq!(find_header(&s, 36 + 4), Some((36, [2, 2, 1, 1, 3, 8])));
        assert_eq!(find_header(&s, 39), None);
        assert_eq!(sample_layout(6, 8).unwrap().samples_per_pixel, 3);
        assert_eq!(sample_layout(4, 9), None);
    }
}
