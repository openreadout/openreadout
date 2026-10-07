//! Compare two files (`openreadout compare A B`, MCP `openreadout_check` with `against`): a
//! diff of the normalized metadata as JSON-pointer paths, image geometry, channel names, physical
//! sizes, and every selected plane by xxh3-128 (or, with a tolerance, by the largest absolute
//! sample difference). Typical use: an original and its OME-TIFF export.
//!
//! "ours" is the first file (`a`), "theirs" the second (`b`). Planes are matched by image
//! index and (c, z, t); images whose geometry differs are reported and not compared plane by
//! plane. With a plane selection, a second file holding only the selected planes of an image
//! (an export made with the same `--select`) is compared with that selection of the first:
//! its planes are matched in order, and the metadata diff sees the first file narrowed the
//! same way. Tables and traces are compared through their metadata only.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use openreadout_core::model::{FileInfo, ImageInfo, PhysicalSize};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::select::Selection;
use openreadout_core::{Error, PixelType, Plane, Result};

/// Metadata paths ignored by default: they describe the container, not the data. The
/// dimension order is the order a container stores planes in (OME-Zarr always `t, c, z`);
/// planes are matched by (c, z, t) whatever it is.
pub const DEFAULT_IGNORES: [&str; 6] = [
    "/path",
    "/size_bytes",
    "/format",
    "/format_version",
    "/notes",
    "/images/*/dimension_order",
];

/// Most differences and plane mismatches listed (the counts stay exact).
pub const MAX_LISTED: usize = 1000;

/// What to compare.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct CompareRequest {
    /// Only this image index (in both files).
    pub image: Option<u32>,
    /// Plane selection strings (`c=0`, `z=1-3`, `t=0,2`).
    pub select: Vec<String>,
    /// Pyramid level (0 = full resolution).
    pub level: u32,
    /// Largest absolute sample difference still counted as equal (lossy conversions).
    /// `None`: planes must hash equal.
    pub tolerance: Option<f64>,
    /// Extra JSON pointers (with `*` for any one segment) to leave out of the metadata diff.
    pub ignore: Vec<String>,
    /// Compare `extra` objects too (vendor-specific; usually differ between formats).
    pub include_extra: bool,
    /// Skip the metadata diff entirely (data only).
    pub no_metadata: bool,
    /// Skip reading pixels (metadata and geometry only).
    pub no_pixels: bool,
}

/// One file of the pair.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompareSide {
    /// The file as given.
    pub path: String,
    /// Its format id.
    pub format: String,
    /// Number of images.
    pub images: u32,
    /// Number of tables.
    pub tables: u32,
    /// Number of traces.
    pub traces: u32,
}

/// A value that differs, at an RFC 6901 JSON pointer into `info --json` `data`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Difference {
    /// RFC 6901 JSON pointer to the value, e.g. `/images/0/physical_size/x`.
    pub pointer: String,
    /// Value in the first file; absent when the first file lacks the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ours: Option<Value>,
    /// Value in the second file; absent when the second file lacks the field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theirs: Option<Value>,
}

/// The normalized-metadata diff.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MetadataComparison {
    /// False with `--no-metadata`.
    pub compared: bool,
    /// True when no difference was found.
    pub equal: bool,
    /// Pointers left out (defaults plus `--ignore`); `extra` objects are also left out unless
    /// `include_extra`.
    pub ignored: Vec<String>,
    /// Whether `extra` objects were compared.
    pub include_extra: bool,
    /// Total number of differences (the list holds at most 1000).
    pub difference_count: u64,
    /// The differences, at most 1000.
    pub differences: Vec<Difference>,
}

/// Geometry of one image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Geometry {
    /// Width in pixels.
    pub size_x: u32,
    /// Height in pixels.
    pub size_y: u32,
    /// Number of Z planes.
    pub size_z: u32,
    /// Number of channels.
    pub size_c: u32,
    /// Number of time points.
    pub size_t: u32,
    /// Sample type.
    pub pixel_type: PixelType,
    /// 1 for grayscale, 3 for interleaved RGB.
    pub samples_per_pixel: u32,
}

impl From<&ImageInfo> for Geometry {
    fn from(im: &ImageInfo) -> Self {
        Geometry {
            size_x: im.size_x,
            size_y: im.size_y,
            size_z: im.size_z,
            size_c: im.size_c,
            size_t: im.size_t,
            pixel_type: im.pixel_type,
            samples_per_pixel: im.samples_per_pixel.max(1),
        }
    }
}

/// One image index present in either file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ImageComparison {
    /// Image index.
    pub image: u32,
    /// Absent when the first file has no image with this index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ours: Option<Geometry>,
    /// Absent when the second file has no image with this index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theirs: Option<Geometry>,
    /// Both files have the image and their geometries agree (with `selected`, after the
    /// selection).
    pub geometry_equal: bool,
    /// The second file holds only the selected planes of this image (an export made with the
    /// same selection): planes, channels and the metadata diff were matched in selection
    /// order, and `ours` is the geometry after the selection.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub selected: bool,
    /// Channel names agree.
    pub channel_names_equal: bool,
    /// Channel names of the first file (listed only when they differ).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ours_channels: Vec<Option<String>>,
    /// Channel names of the second file (listed only when they differ).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub theirs_channels: Vec<Option<String>>,
    /// Physical sizes agree to a relative 1e-6 (both absent counts as equal).
    pub physical_size_equal: bool,
    /// Physical pixel size in the first file (listed only when the sizes differ).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ours_physical_size: Option<PhysicalSize>,
    /// Physical pixel size in the second file (listed only when the sizes differ).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theirs_physical_size: Option<PhysicalSize>,
}

/// A plane that differs.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaneMismatch {
    /// Image index.
    pub image: u32,
    /// Channel index.
    pub c: u32,
    /// Z index.
    pub z: u32,
    /// Time index.
    pub t: u32,
    /// xxh3-128 of the plane in the first file.
    pub ours_xxh3: String,
    /// xxh3-128 of the plane in the second file.
    pub theirs_xxh3: String,
    /// Samples that differ (absent when the planes' shapes differ).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub differing_samples: Option<u64>,
    /// Largest absolute difference between corresponding samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_abs_diff: Option<f64>,
    /// With a tolerance: whether `max_abs_diff` is within it (then the plane counts as equal).
    pub within_tolerance: bool,
    /// Why the samples could not be compared (e.g. the plane shapes differ).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Plane-by-plane comparison.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PlaneComparison {
    /// False with `--no-pixels`.
    pub compared: bool,
    /// The `--tolerance` used, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
    /// Planes read from both files.
    pub planes: u64,
    /// Planes with identical xxh3-128.
    pub identical: u64,
    /// Planes that differ but within the tolerance.
    pub within_tolerance: u64,
    /// Planes that differ beyond the tolerance (or at all, without one).
    pub mismatched: u64,
    /// Image indices not compared plane by plane (missing on one side or geometry differs).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped_images: Vec<u32>,
    /// At most 1000 entries, in plane order.
    pub mismatches: Vec<PlaneMismatch>,
}

/// Output of `compare`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompareOutput {
    /// Nothing compared differs: metadata (unless skipped), geometry, channel names,
    /// physical sizes and planes (within the tolerance). The exit code is 0 when true, 1
    /// when false.
    pub identical: bool,
    /// The first file.
    pub ours: CompareSide,
    /// The second file.
    pub theirs: CompareSide,
    /// The normalized-metadata diff.
    pub metadata: MetadataComparison,
    /// Geometry, channel-name and physical-size comparison per image index.
    pub images: Vec<ImageComparison>,
    /// The plane-by-plane comparison.
    pub planes: PlaneComparison,
    /// Caveats (e.g. formats that store different metadata for the same acquisition).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

fn escape(seg: &str) -> String {
    seg.replace('~', "~0").replace('/', "~1")
}

fn pointer_ignored(ptr: &[String], patterns: &[Vec<String>]) -> bool {
    patterns
        .iter()
        .any(|p| p.len() <= ptr.len() && p.iter().zip(ptr).all(|(a, b)| a == "*" || a == b))
}

fn numbers_equal(a: &serde_json::Number, b: &serde_json::Number) -> bool {
    if a == b {
        return true;
    }
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * x.abs().max(y.abs()),
        _ => false,
    }
}

struct Differ<'a> {
    patterns: &'a [Vec<String>],
    include_extra: bool,
    count: u64,
    out: Vec<Difference>,
}

impl Differ<'_> {
    fn push(&mut self, ptr: &[String], ours: Option<&Value>, theirs: Option<&Value>) {
        self.count += 1;
        if self.out.len() < MAX_LISTED {
            let pointer = ptr.iter().fold(String::new(), |mut s, seg| {
                s.push('/');
                s.push_str(&escape(seg));
                s
            });
            self.out.push(Difference {
                pointer,
                ours: ours.cloned(),
                theirs: theirs.cloned(),
            });
        }
    }

    fn diff(&mut self, ptr: &mut Vec<String>, a: Option<&Value>, b: Option<&Value>) {
        if pointer_ignored(ptr, self.patterns) {
            return;
        }
        if !self.include_extra && ptr.last().is_some_and(|s| s == "extra") {
            return;
        }
        match (a, b) {
            (Some(Value::Object(x)), Some(Value::Object(y))) => {
                let mut keys: Vec<&String> = x.keys().collect();
                keys.extend(y.keys().filter(|k| !x.contains_key(*k)));
                for k in keys {
                    ptr.push(k.clone());
                    self.diff(ptr, x.get(k), y.get(k));
                    ptr.pop();
                }
            }
            (Some(Value::Array(x)), Some(Value::Array(y))) => {
                for i in 0..x.len().max(y.len()) {
                    ptr.push(i.to_string());
                    self.diff(ptr, x.get(i), y.get(i));
                    ptr.pop();
                }
            }
            (Some(Value::Number(x)), Some(Value::Number(y))) => {
                if !numbers_equal(x, y) {
                    self.push(ptr, a, b);
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    self.push(ptr, a, b);
                }
            }
            (None, None) => {}
            _ => self.push(ptr, a, b),
        }
    }
}

/// Parse a JSON pointer (`/images/*/name`) into unescaped segments.
fn parse_pointer(p: &str) -> Vec<String> {
    p.trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect()
}

/// Diff two JSON documents. Returns (total differences, the first [`MAX_LISTED`]).
pub fn json_diff(
    a: &Value,
    b: &Value,
    ignore: &[String],
    include_extra: bool,
) -> (u64, Vec<Difference>) {
    let patterns: Vec<Vec<String>> = ignore.iter().map(|p| parse_pointer(p)).collect();
    let mut d = Differ {
        patterns: &patterns,
        include_extra,
        count: 0,
        out: Vec::new(),
    };
    d.diff(&mut Vec::new(), Some(a), Some(b));
    (d.count, d.out)
}

fn physical_equal(a: &PhysicalSize, b: &PhysicalSize) -> bool {
    let eq = |x: Option<f64>, y: Option<f64>| match (x, y) {
        (None, None) => true,
        (Some(x), Some(y)) => (x - y).abs() <= 1e-6 * x.abs().max(y.abs()),
        _ => false,
    };
    eq(a.x, b.x) && eq(a.y, b.y) && eq(a.z, b.z)
}

fn side(info: &FileInfo) -> CompareSide {
    CompareSide {
        path: info.path.clone(),
        format: info.format.id.clone(),
        images: info.images.len() as u32,
        tables: info.tables.len() as u32,
        traces: info.traces.len() as u32,
    }
}

/// Samples of a plane as `f64` (little-endian storage).
fn samples(p: &Plane) -> Vec<f64> {
    let d = &p.data;
    match p.pixel_type {
        PixelType::Uint8 => d.iter().map(|&b| f64::from(b)).collect(),
        PixelType::Int8 => d.iter().map(|&b| f64::from(b as i8)).collect(),
        PixelType::Uint16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(u16::from_le_bytes(*c)))
            .collect(),
        PixelType::Int16 => d
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| f64::from(i16::from_le_bytes(*c)))
            .collect(),
        PixelType::Uint32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(u32::from_le_bytes(*c)))
            .collect(),
        PixelType::Int32 => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(i32::from_le_bytes(*c)))
            .collect(),
        PixelType::Float => d
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f64::from(f32::from_le_bytes(*c)))
            .collect(),
        PixelType::Double => d
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| f64::from_le_bytes(*c))
            .collect(),
        // 64-bit integers; complex samples compare by modulus
        pt => pt.samples_f64(d).collect(),
    }
}

fn diff_planes(a: &Plane, b: &Plane) -> (Option<u64>, Option<f64>, Option<String>) {
    if a.width != b.width
        || a.height != b.height
        || a.samples_per_pixel != b.samples_per_pixel
        || a.pixel_type != b.pixel_type
    {
        return (
            None,
            None,
            Some(format!(
                "plane shapes differ: {}x{} {} x{} vs {}x{} {} x{}",
                a.width,
                a.height,
                a.pixel_type.ome_name(),
                a.samples_per_pixel,
                b.width,
                b.height,
                b.pixel_type.ome_name(),
                b.samples_per_pixel
            )),
        );
    }
    let (ours, theirs) = (samples(a), samples(b));
    let mut differing = 0u64;
    let mut max = 0f64;
    for (p, q) in ours.iter().zip(&theirs) {
        // Bitwise equality (NaN equals NaN here): the planes are compared sample by sample.
        let equal = p.to_bits() == q.to_bits() || (p.is_nan() && q.is_nan());
        if !equal {
            let diff = (p - q).abs();
            if diff == 0.0 {
                // +0.0 and -0.0
                continue;
            }
            differing += 1;
            max = if diff.is_nan() {
                f64::INFINITY
            } else {
                max.max(diff)
            };
        }
    }
    (Some(differing), Some(max), None)
}

/// The indices of `0..n` a selection keeps (all of them for an empty list), in order.
fn pick(n: u32, s: &[u32]) -> Vec<u32> {
    (0..n).filter(|v| s.is_empty() || s.contains(v)).collect()
}

/// Source plane indices of one image, per axis: `(c, z, t)`.
type AxisMaps = (Vec<u32>, Vec<u32>, Vec<u32>);

/// Per axis (c, z, t): pairs of (index in the first file, index in the second).
type AxisPairs = [Vec<(u32, u32)>; 3];

/// An image narrowed to the selected planes, as an export with that selection describes it:
/// sizes and plane count shrink, kept channels are renumbered in order. `None` when the
/// selection keeps every plane or none.
fn project(im: &ImageInfo, sel: &Selection) -> Option<(ImageInfo, AxisMaps)> {
    let (cs, zs, ts) = (
        pick(im.size_c, &sel.c),
        pick(im.size_z, &sel.z),
        pick(im.size_t, &sel.t),
    );
    let full = cs.len() == im.size_c as usize
        && zs.len() == im.size_z as usize
        && ts.len() == im.size_t as usize;
    if full || cs.is_empty() || zs.is_empty() || ts.is_empty() {
        return None;
    }
    let mut narrowed = im.clone();
    narrowed.size_c = cs.len() as u32;
    narrowed.size_z = zs.len() as u32;
    narrowed.size_t = ts.len() as u32;
    narrowed.plane_count = (cs.len() * zs.len() * ts.len()) as u64;
    narrowed.channels = im
        .channels
        .iter()
        .filter_map(|ch| {
            let pos = cs.iter().position(|&v| v == ch.index)?;
            let mut ch = ch.clone();
            ch.index = pos as u32;
            Some(ch)
        })
        .collect();
    Some((narrowed, (cs, zs, ts)))
}

/// Compare two opened files. `a`/`b` are their summaries.
pub fn compare(
    ds_a: &mut dyn Dataset,
    a: &FileInfo,
    ds_b: &mut dyn Dataset,
    b: &FileInfo,
    req: &CompareRequest,
) -> Result<CompareOutput> {
    if let Some(t) = req.tolerance
        && !(t.is_finite() && t >= 0.0)
    {
        return Err(Error::Usage(format!(
            "--tolerance must be a finite number ≥ 0, got {t}"
        )));
    }
    let sel = Selection::parse(&req.select)?;
    let mut notes = Vec::new();

    // Images of `b` that hold exactly the selected planes of the same image of `a`.
    let mut projected: Vec<(u32, AxisMaps)> = Vec::new();
    let mut narrowed = a.clone();
    if !sel.is_all() {
        for im in &mut narrowed.images {
            if req.image.is_some_and(|w| w != im.index) {
                continue;
            }
            let Some(theirs) = b.images.iter().find(|x| x.index == im.index) else {
                continue;
            };
            if Geometry::from(&*im) == Geometry::from(theirs) {
                continue;
            }
            if let Some((p, maps)) = project(im, &sel)
                && Geometry::from(&p) == Geometry::from(theirs)
            {
                narrowed.plane_count = narrowed
                    .plane_count
                    .saturating_sub(im.plane_count)
                    .saturating_add(p.plane_count);
                *im = p;
                projected.push((im.index, maps));
            }
        }
        if !projected.is_empty() {
            notes.push(format!(
                "the second file holds only the selected planes of image(s) {:?}: they were compared with that selection of the first file",
                projected.iter().map(|(i, _)| *i).collect::<Vec<_>>()
            ));
        }
    }
    let a = &narrowed;

    let mut ignored: Vec<String> = DEFAULT_IGNORES.iter().map(|s| (*s).to_string()).collect();
    ignored.extend(req.ignore.iter().cloned());
    let (difference_count, differences) = if req.no_metadata {
        (0, Vec::new())
    } else {
        let va = serde_json::to_value(a).map_err(|e| Error::Other(e.to_string()))?;
        let vb = serde_json::to_value(b).map_err(|e| Error::Other(e.to_string()))?;
        json_diff(&va, &vb, &ignored, req.include_extra)
    };
    let metadata = MetadataComparison {
        compared: !req.no_metadata,
        equal: difference_count == 0,
        ignored,
        include_extra: req.include_extra,
        difference_count,
        differences,
    };

    let mut indices: Vec<u32> = a
        .images
        .iter()
        .chain(&b.images)
        .map(|im| im.index)
        .filter(|i| req.image.is_none_or(|w| w == *i))
        .collect();
    indices.sort_unstable();
    indices.dedup();
    if let Some(i) = req.image
        && indices.is_empty()
    {
        return Err(Error::Usage(format!("image {i} exists in neither file")));
    }
    let mut images = Vec::new();
    // image index and, per axis, (index in `a`, index in `b`) pairs
    let mut to_read: Vec<(u32, AxisPairs)> = Vec::new();
    let mut skipped = Vec::new();
    for i in indices {
        let ia = a.images.iter().find(|im| im.index == i);
        let ib = b.images.iter().find(|im| im.index == i);
        let ga = ia.map(Geometry::from);
        let gb = ib.map(Geometry::from);
        let geometry_equal = ga.is_some() && ga == gb;
        let names = |im: Option<&ImageInfo>| -> Vec<Option<String>> {
            im.map(|im| im.channels.iter().map(|c| c.name.clone()).collect())
                .unwrap_or_default()
        };
        let (na, nb) = (names(ia), names(ib));
        let channel_names_equal = na == nb;
        let (pa, pb) = (
            ia.map(|im| im.physical_size.clone()),
            ib.map(|im| im.physical_size.clone()),
        );
        let physical_size_equal = match (&pa, &pb) {
            (Some(x), Some(y)) => physical_equal(x, y),
            _ => false,
        };
        let maps = projected.iter().find(|(k, _)| *k == i).map(|(_, m)| m);
        if geometry_equal && let Some(g) = &ga {
            let pairs = |v: Vec<u32>| {
                v.into_iter()
                    .enumerate()
                    .map(|(k, x)| (x, k as u32))
                    .collect()
            };
            let same = |n: u32, s: &[u32]| pick(n, s).into_iter().map(|x| (x, x)).collect();
            to_read.push((
                i,
                match maps {
                    Some((cs, zs, ts)) => [pairs(cs.clone()), pairs(zs.clone()), pairs(ts.clone())],
                    None => [
                        same(g.size_c, &sel.c),
                        same(g.size_z, &sel.z),
                        same(g.size_t, &sel.t),
                    ],
                },
            ));
        } else {
            skipped.push(i);
        }
        images.push(ImageComparison {
            image: i,
            ours: ga,
            theirs: gb,
            geometry_equal,
            selected: maps.is_some(),
            channel_names_equal,
            ours_channels: if channel_names_equal { Vec::new() } else { na },
            theirs_channels: if channel_names_equal { Vec::new() } else { nb },
            physical_size_equal,
            ours_physical_size: if physical_size_equal { None } else { pa },
            theirs_physical_size: if physical_size_equal { None } else { pb },
        });
    }

    let mut planes = PlaneComparison {
        compared: !req.no_pixels,
        tolerance: req.tolerance,
        planes: 0,
        identical: 0,
        within_tolerance: 0,
        mismatched: 0,
        skipped_images: skipped,
        mismatches: Vec::new(),
    };
    if !req.no_pixels {
        for (i, [cs, zs, ts]) in &to_read {
            for &(c, bc) in cs {
                for &(z, bz) in zs {
                    for &(t, bt) in ts {
                        let idx = PlaneIndex { c, z, t };
                        let pa = ds_a.read_plane_level(*i, idx, req.level)?;
                        let pb = ds_b.read_plane_level(
                            *i,
                            PlaneIndex {
                                c: bc,
                                z: bz,
                                t: bt,
                            },
                            req.level,
                        )?;
                        planes.planes += 1;
                        let (ha, hb) = (pa.xxh3_hex(), pb.xxh3_hex());
                        if ha == hb {
                            planes.identical += 1;
                            continue;
                        }
                        let (differing, max_abs, note) = diff_planes(&pa, &pb);
                        let within = match (req.tolerance, max_abs) {
                            (Some(tol), Some(m)) => m <= tol,
                            _ => false,
                        };
                        if within {
                            planes.within_tolerance += 1;
                        } else {
                            planes.mismatched += 1;
                        }
                        if planes.mismatches.len() < MAX_LISTED {
                            planes.mismatches.push(PlaneMismatch {
                                image: *i,
                                c,
                                z,
                                t,
                                ours_xxh3: ha,
                                theirs_xxh3: hb,
                                differing_samples: differing,
                                max_abs_diff: max_abs,
                                within_tolerance: within,
                                note,
                            });
                        }
                    }
                }
            }
        }
    }
    if a.images.is_empty() && b.images.is_empty() {
        notes.push(
            "neither file has images: tables and traces are compared through their metadata only"
                .into(),
        );
    }
    let images_equal = images
        .iter()
        .all(|i| i.geometry_equal && i.channel_names_equal && i.physical_size_equal);
    let identical = metadata.equal && images_equal && planes.mismatched == 0;
    Ok(CompareOutput {
        identical,
        ours: side(a),
        theirs: side(b),
        metadata,
        images,
        planes,
        notes,
    })
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diff_reports_pointers_and_honours_ignores() {
        let a = json!({"path": "a", "images": [{"name": "x", "size_x": 4, "extra": {"k": 1}, "px": 0.1}], "m/n": 1});
        let b = json!({"path": "b", "images": [{"name": "y", "size_x": 4, "extra": {"k": 2}, "px": 0.100_000_000_000_1}, {"name": "z"}], "m/n": 2});
        let (n, d) = json_diff(&a, &b, &["/path".into()], false);
        assert_eq!(n, 3, "{d:?}");
        assert_eq!(d[0].pointer, "/images/0/name");
        assert_eq!(d[0].ours, Some(json!("x")));
        assert_eq!(d[1].pointer, "/images/1");
        assert!(d[1].ours.is_none());
        assert_eq!(d[2].pointer, "/m~1n");
        let (n, _) = json_diff(&a, &b, &["/path".into(), "/images/*/name".into()], true);
        assert_eq!(n, 3); // extra/k now counts, names are ignored, /images/1 and /m~1n remain
    }
}
