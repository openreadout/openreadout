//! `Dataset` for OME-Zarr stores: images from `multiscales`, HCS plates (one image per field)
//! and `bioformats2raw.layout` collections (one image per series, OME-XML from
//! `OME/METADATA.ome.xml`), pixels through `zarrs`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, LsEntry, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::region::{Region, ResolutionLevel};
use openreadout_core::{Error, PixelType, Plane, Result};
use openreadout_tiff::{OmeDocument, OmeImage, parse_ome_xml};
use serde_json::{Value, json};
use zarrs::array::{Array, ArrayBytes, ArraySubset};
use zarrs::storage::ReadableStorageTraits;

use crate::ngff::{self, GroupAttrs, Multiscale, ZarrFormat};
use crate::store::{Store, join};
use crate::{FORMAT_ID, ZarrReader};

/// Largest plane read into memory.
const MAX_PLANE_BYTES: u64 = 4 << 30;
/// Chunks whose presence `check` verifies per array (beyond this it samples evenly).
const MAX_CHUNKS_CHECKED: u64 = 100_000;

type ZArray = Array<dyn ReadableStorageTraits>;

/// What each array dimension of an image is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisRole {
    X,
    Y,
    Z,
    C,
    T,
    /// An axis NGFF allows but the normalized model has no place for (read at index 0).
    Other,
}

/// Array-level facts read from `.zarray` / `zarr.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayMeta {
    pub path: String,
    pub shape: Vec<u64>,
    pub chunks: Vec<u64>,
    /// Data type as the metadata spells it (`<u2`, `uint16`, ...).
    pub dtype: String,
    pub pixel_type: Option<PixelType>,
    /// Codec / compressor names in pipeline order.
    pub codecs: Vec<String>,
    pub format: ZarrFormat,
}

/// One image of the store.
#[derive(Debug, Clone)]
pub struct ZarrImage {
    pub multiscale: Multiscale,
    pub roles: Vec<AxisRole>,
    pub levels: Vec<ArrayMeta>,
    /// Group-level `omero` block.
    pub omero: Option<Value>,
    /// Label images listed under `<group>/labels`.
    pub labels: Vec<String>,
    pub info: ImageInfo,
}

/// An opened OME-Zarr store.
pub struct ZarrDataset {
    path: PathBuf,
    store: Store,
    /// `image`, `plate` or `bioformats2raw`.
    layout: &'static str,
    ngff_version: Option<String>,
    zarr_format: ZarrFormat,
    root: GroupAttrs,
    images: Vec<ZarrImage>,
    /// Root-level label images.
    labels: Vec<String>,
    ome_xml: Option<(OmeDocument, usize)>,
    arrays: HashMap<String, Arc<ZArray>>,
    notes: Vec<String>,
}

impl std::fmt::Debug for ZarrDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZarrDataset")
            .field("path", &self.path)
            .field("layout", &self.layout)
            .field("images", &self.images.len())
            .finish_non_exhaustive()
    }
}

/// The pixel type an array's samples are returned as. Half floats are widened to `float`,
/// booleans are `uint8` 0/1 (`docs/formats/ome-zarr.md` § Data types).
fn pixel_type_of(dtype: &str) -> Option<PixelType> {
    let d = dtype.trim_start_matches(['<', '>', '|', '=']);
    Some(match d {
        "u1" | "uint8" | "b1" | "bool" => PixelType::Uint8,
        "u2" | "uint16" => PixelType::Uint16,
        "u4" | "uint32" => PixelType::Uint32,
        "u8" | "uint64" => PixelType::Uint64,
        "i1" | "int8" => PixelType::Int8,
        "i2" | "int16" => PixelType::Int16,
        "i4" | "int32" => PixelType::Int32,
        "i8" | "int64" => PixelType::Int64,
        "f2" | "float16" | "f4" | "float32" => PixelType::Float,
        "f8" | "float64" => PixelType::Double,
        "c8" | "complex64" => PixelType::ComplexFloat,
        "c16" | "complex128" => PixelType::ComplexDouble,
        _ => return None,
    })
}

/// Half-float arrays: stored 2 bytes, returned as 4-byte floats.
fn is_half(dtype: &str) -> bool {
    matches!(
        dtype.trim_start_matches(['<', '>', '|', '=']),
        "f2" | "float16"
    )
}

use openreadout_core::pixel::half_to_f32;

fn u64_list(v: Option<&Value>) -> Vec<u64> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
}

/// Read an array's metadata document (`zarr.json` with `node_type: array`, or `.zarray`).
pub fn array_meta(store: &Store, path: &str) -> Result<Option<ArrayMeta>> {
    if let Some(doc) = store.json(&join(path, "zarr.json"))? {
        if doc.get("node_type").and_then(Value::as_str) != Some("array") {
            return Ok(None);
        }
        let dtype = match doc.get("data_type") {
            Some(Value::String(s)) => s.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        let chunks = u64_list(
            doc.pointer("/chunk_grid/configuration/chunk_shape")
                .or_else(|| doc.get("chunk_shape")),
        );
        let mut codecs = Vec::new();
        for c in doc
            .get("codecs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let name = c.get("name").and_then(Value::as_str).unwrap_or("?");
            codecs.push(name.to_string());
            if name == "sharding_indexed" {
                for inner in c
                    .pointer("/configuration/codecs")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    codecs.push(format!(
                        "sharding_indexed/{}",
                        inner.get("name").and_then(Value::as_str).unwrap_or("?")
                    ));
                }
            }
        }
        return Ok(Some(ArrayMeta {
            path: path.to_string(),
            shape: u64_list(doc.get("shape")),
            chunks,
            pixel_type: pixel_type_of(&dtype),
            dtype,
            codecs,
            format: ZarrFormat::V3,
        }));
    }
    let Some(doc) = store.json(&join(path, ".zarray"))? else {
        return Ok(None);
    };
    let dtype = match doc.get("dtype") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    let mut codecs: Vec<String> = doc
        .get("filters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|f| f.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    if let Some(c) = doc.get("compressor").filter(|c| !c.is_null()) {
        let id = c.get("id").and_then(Value::as_str).unwrap_or("?");
        codecs.push(match c.get("cname").and_then(Value::as_str) {
            Some(cn) => format!("{id}/{cn}"),
            None => id.to_string(),
        });
    }
    Ok(Some(ArrayMeta {
        path: path.to_string(),
        shape: u64_list(doc.get("shape")),
        chunks: u64_list(doc.get("chunks")),
        pixel_type: pixel_type_of(&dtype),
        dtype,
        codecs,
        format: ZarrFormat::V2,
    }))
}

/// Map axes to roles: by name (`x`, `y`, `z`, `c`, `t`), else by type (`channel`, `time`, and
/// the last two / three `space` axes as y, x / z, y, x).
pub fn axis_roles(axes: &[ngff::Axis]) -> Vec<AxisRole> {
    let mut roles: Vec<AxisRole> = axes
        .iter()
        .map(|a| match a.name.to_ascii_lowercase().as_str() {
            "x" => AxisRole::X,
            "y" => AxisRole::Y,
            "z" => AxisRole::Z,
            "c" | "ch" | "channel" => AxisRole::C,
            "t" | "time" => AxisRole::T,
            _ => match a.kind.as_deref() {
                Some("channel") if !axes.iter().any(|b| b.name == "c") => AxisRole::C,
                Some("time") if !axes.iter().any(|b| b.name == "t") => AxisRole::T,
                _ => AxisRole::Other,
            },
        })
        .collect();
    // space axes with other names: the last is x, then y, then z
    if !roles.contains(&AxisRole::X) {
        let space: Vec<usize> = axes
            .iter()
            .enumerate()
            .filter(|(i, a)| a.kind.as_deref() == Some("space") && roles[*i] == AxisRole::Other)
            .map(|(i, _)| i)
            .collect();
        for (k, &i) in space.iter().rev().enumerate() {
            roles[i] = [AxisRole::X, AxisRole::Y, AxisRole::Z]
                .get(k)
                .copied()
                .unwrap_or(AxisRole::Other);
        }
    }
    roles
}

fn role_index(roles: &[AxisRole], r: AxisRole) -> Option<usize> {
    roles.iter().position(|&x| x == r)
}

fn channel_from_omero(i: u32, c: &Value) -> ChannelInfo {
    ChannelInfo {
        index: i,
        name: c
            .get("label")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        color: ngff::color(c.get("color")),
        ..ChannelInfo::default()
    }
}

fn ome_color(c: i32) -> String {
    let v = c as u32;
    format!(
        "#{:02X}{:02X}{:02X}",
        v >> 24 & 0xFF,
        v >> 16 & 0xFF,
        v >> 8 & 0xFF
    )
}

/// Fill image metadata from the matching OME-XML `Image` (bioformats2raw collections, and
/// images at the root with an `OME/METADATA.ome.xml`).
fn apply_ome(info: &mut ImageInfo, img: &OmeImage, doc: &OmeDocument) {
    if img.name.is_some() {
        info.name.clone_from(&img.name);
    }
    info.acquired_at = doc.acquired_at(img);
    let px = &img.pixels;
    if info.physical_size.is_empty() {
        info.physical_size =
            PhysicalSize::micrometres(px.physical_size_x, px.physical_size_y, px.physical_size_z);
    }
    if info.time_increment_s.is_none() {
        info.time_increment_s = px.time_increment;
    }
    let inst = doc.instrument_of(img);
    // OpenReadout's writer invents an `omero` label and colour for every channel; its OME-XML
    // says what the source recorded (when channels map one to one: not for split RGB samples)
    let ours = doc
        .creator
        .as_deref()
        .is_some_and(|c| c.starts_with("openreadout "))
        && px.channels.len() == info.channels.len();
    for ch in &mut info.channels {
        if let Some(o) = px.channels.get(ch.index as usize) {
            if o.name.is_some() || ours {
                ch.name.clone_from(&o.name);
            }
            ch.fluorophore.clone_from(&o.fluor);
            ch.excitation_nm = o.excitation_wavelength;
            ch.emission_nm = o.emission_wavelength;
            if let Some(c) = o.color {
                ch.color = Some(ome_color(c));
            } else if ours {
                ch.color = None;
            }
            ch.acquisition_mode = doc.channel_mode(o);
            // as the OME-TIFF reader: the first plane of the channel with an exposure
            if let Some(s) = px
                .planes
                .iter()
                .find(|p| p.the_c == ch.index && p.exposure_time.is_some())
                .and_then(|p| p.exposure_time)
            {
                ch.exposure_ms = Some(s * 1e3);
            }
            if let Some([a, b]) = doc.channel_band(inst, o) {
                ch.set_band(a, b);
            }
        }
    }
    let (objective, instrument) = doc.objective_and_instrument(img);
    if objective.is_some() {
        info.objective = objective;
    }
    if instrument.is_some() {
        info.instrument = instrument;
    }
    if let Some(c) = &doc.creator {
        info.extra.insert("ome_creator".into(), json!(c));
    }
}

/// Build the normalized image from its multiscales entry and level-0 array.
#[allow(clippy::many_single_char_names)]
fn image_info(
    index: u32,
    m: &Multiscale,
    roles: &[AxisRole],
    levels: &[ArrayMeta],
    omero: Option<&Value>,
) -> ImageInfo {
    let l0 = &levels[0];
    let size = |r: AxisRole| {
        role_index(roles, r)
            .and_then(|i| l0.shape.get(i).copied())
            .map_or(1, |v| u32::try_from(v).unwrap_or(u32::MAX))
    };
    let mut im = ImageInfo::new(
        index,
        size(AxisRole::X),
        size(AxisRole::Y),
        l0.pixel_type.unwrap_or(PixelType::Uint8),
    );
    im.size_z = size(AxisRole::Z);
    im.size_c = size(AxisRole::C);
    im.size_t = size(AxisRole::T);
    // Planes are stored with the last axis fastest: XY, then the remaining roles reversed.
    let mut order = String::from("XY");
    for r in roles.iter().rev() {
        let ch = match r {
            AxisRole::Z => 'Z',
            AxisRole::C => 'C',
            AxisRole::T => 'T',
            _ => continue,
        };
        order.push(ch);
    }
    for ch in ['Z', 'C', 'T'] {
        if !order.contains(ch) {
            order.push(ch);
        }
    }
    im.dimension_order = order;
    im.name.clone_from(&m.name);
    let scale0 = &m.levels[0].scale;
    let len = |r: AxisRole| {
        let i = role_index(roles, r)?;
        let unit = m.axes.get(i)?.unit.as_deref()?;
        Some(scale0.get(i)? * ngff::length_um(unit)?)
    };
    im.physical_size =
        PhysicalSize::micrometres(len(AxisRole::X), len(AxisRole::Y), len(AxisRole::Z));
    im.time_increment_s = role_index(roles, AxisRole::T).and_then(|i| {
        let unit = m.axes.get(i)?.unit.as_deref()?;
        Some(scale0.get(i)? * ngff::time_s(unit)?).filter(|v| *v > 0.0)
    });
    let omero_channels: Vec<Value> = omero
        .and_then(|o| o.get("channels"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    im.channels = (0..im.size_c)
        .map(|c| {
            omero_channels.get(c as usize).map_or_else(
                || ChannelInfo {
                    index: c,
                    ..ChannelInfo::default()
                },
                |v| channel_from_omero(c, v),
            )
        })
        .collect();
    im.pyramid_levels = levels.len() as u32;
    // Extent of `v` (a shape or chunk shape) along role `r`; `absent` when there is no such axis.
    let along = |v: &[u64], r: AxisRole, absent: u32| {
        role_index(roles, r)
            .and_then(|i| v.get(i).copied())
            .map_or(absent, |v| u32::try_from(v).unwrap_or(u32::MAX))
    };
    let (w0, h0, z0) = (im.size_x, im.size_y, size(AxisRole::Z));
    let tiled = along(&l0.chunks, AxisRole::X, w0) < w0 || along(&l0.chunks, AxisRole::Y, h0) < h0;
    if levels.len() > 1 || tiled {
        im.resolution_levels = levels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let (w, h) = (
                    along(&l.shape, AxisRole::X, 1),
                    along(&l.shape, AxisRole::Y, 1),
                );
                let mut r = ResolutionLevel::new(i as u32, w, h, w0, h0).with_tile(
                    along(&l.chunks, AxisRole::X, w),
                    along(&l.chunks, AxisRole::Y, h),
                );
                let z = along(&l.shape, AxisRole::Z, 1);
                if z != z0 {
                    r.size_z = Some(z);
                }
                r
            })
            .collect();
    }
    let ex = &mut im.extra;
    ex.insert(
        "group".into(),
        json!(if m.group.is_empty() {
            "/"
        } else {
            m.group.as_str()
        }),
    );
    ex.insert(
        "axes".into(),
        json!(m.axes.iter().map(|a| a.name.clone()).collect::<Vec<_>>()),
    );
    if let Some(v) = &m.version {
        ex.insert("ngff_version".into(), json!(v));
    }
    ex.insert("dtype".into(), json!(l0.dtype));
    ex.insert("scale".into(), json!(scale0));
    if let Some(t) = &m.levels[0].translation {
        ex.insert("translation".into(), json!(t));
        // the physical coordinate of the first sample along each spatial axis, in µm
        let mut origin = serde_json::Map::new();
        for (r, key) in [(AxisRole::X, "x"), (AxisRole::Y, "y"), (AxisRole::Z, "z")] {
            if let Some(i) = role_index(roles, r)
                && let Some(f) = m
                    .axes
                    .get(i)
                    .and_then(|a| a.unit.as_deref())
                    .and_then(ngff::length_um)
                && let Some(v) = t.get(i)
            {
                origin.insert(key.into(), json!(v * f));
            }
        }
        if !origin.is_empty() {
            ex.insert("origin_um".into(), Value::Object(origin));
        }
    }
    if let Some(mt) = &m.method {
        ex.insert("downsampling".into(), json!(mt));
    }
    if levels.len() > 1 {
        let sizes: Vec<Value> = levels
            .iter()
            .map(|l| {
                let g = |r| role_index(roles, r).and_then(|i| l.shape.get(i).copied());
                json!([g(AxisRole::X), g(AxisRole::Y)])
            })
            .collect();
        ex.insert("level_sizes".into(), Value::Array(sizes));
    }
    let windows: Vec<Value> = omero_channels
        .iter()
        .map(|c| c.get("window").cloned().unwrap_or(Value::Null))
        .collect();
    if windows.iter().any(|w| !w.is_null()) {
        ex.insert("channel_windows".into(), Value::Array(windows));
    }
    if roles.contains(&AxisRole::Other) {
        let others: Vec<String> = roles
            .iter()
            .zip(&m.axes)
            .filter(|(r, _)| **r == AxisRole::Other)
            .map(|(_, a)| a.name.clone())
            .collect();
        ex.insert("unmapped_axes".into(), json!(others));
    }
    im.finish()
}

fn label_names(store: &Store, group: &str) -> Result<Vec<String>> {
    Ok(ngff::group_attrs(store, &join(group, "labels"))?
        .and_then(|a| a.ngff.get("labels").and_then(Value::as_array).cloned())
        .map(|l| {
            let mut names: Vec<String> = Vec::new();
            for n in l.iter().filter_map(Value::as_str) {
                // a name listed twice is one label image
                if !names.iter().any(|m| m == n) {
                    names.push(n.to_string());
                }
            }
            names
        })
        .unwrap_or_default())
}

impl ZarrDataset {
    /// Open a store and read every image's metadata (no chunk is read).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&openreadout_core::source::Input::local(path))
    }

    /// Open an [`Input`](openreadout_core::source::Input) (a local path, a buffer, a host
    /// source, a folder held in memory).
    pub(crate) fn open_input(input: &openreadout_core::source::Input) -> Result<Self> {
        let path = input.path();
        crate::codecs::register();
        let store = Store::open_in(input.fs(), path)?;
        let root = ngff::group_attrs(&store, "")?.ok_or_else(|| {
            if array_meta(&store, "").ok().flatten().is_some() {
                Error::unsupported(
                    FORMAT_ID,
                    "a bare Zarr array (no OME-NGFF group)",
                    "Only OME-Zarr (NGFF) images, plates and bioformats2raw collections are read; a plain Zarr array has no axes or scale metadata.",
                )
            } else {
                Error::corrupt(FORMAT_ID, "no zarr.json, .zgroup or .zattrs at the store root")
            }
        })?;
        let mut ds = ZarrDataset {
            path: path.to_path_buf(),
            zarr_format: root.format,
            ngff_version: ngff::version_of(&root.ngff),
            store,
            layout: "image",
            root,
            images: Vec::new(),
            labels: Vec::new(),
            ome_xml: None,
            arrays: HashMap::new(),
            notes: Vec::new(),
        };
        let ngff_root = ds.root.ngff.clone();
        let mut groups: Vec<(String, Option<String>, BTreeMap<String, Value>)> = Vec::new();
        if ngff_root.get("multiscales").is_some() {
            groups.push((String::new(), None, BTreeMap::new()));
            // An image at the root may keep its OME-XML in `OME/` too (OpenReadout exports do).
            ds.load_ome_xml()?;
        } else if let Some(plate) = ngff_root.get("plate") {
            ds.layout = "plate";
            let rows: Vec<String> = plate
                .get("rows")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.get("name").and_then(Value::as_str).map(str::to_string))
                .collect();
            let cols: Vec<String> = plate
                .get("columns")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|r| r.get("name").and_then(Value::as_str).map(str::to_string))
                .collect();
            for w in plate
                .get("wells")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(wp) = w.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let Some(wa) = ngff::group_attrs(&ds.store, wp)? else {
                    ds.notes
                        .push(format!("well {wp} is listed in the plate but missing"));
                    continue;
                };
                if ds.ngff_version.is_none() {
                    ds.ngff_version = ngff::version_of(&wa.ngff);
                }
                for f in wa
                    .ngff
                    .pointer("/well/images")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(fp) = f.get("path").and_then(Value::as_str) else {
                        continue;
                    };
                    let mut extra = BTreeMap::new();
                    extra.insert("well".into(), json!(wp));
                    extra.insert("field".into(), json!(fp));
                    let ri = w.get("rowIndex").and_then(Value::as_u64);
                    let ci = w.get("columnIndex").and_then(Value::as_u64);
                    if let Some(r) = ri.and_then(|r| rows.get(r as usize)) {
                        extra.insert("row".into(), json!(r));
                    }
                    if let Some(c) = ci.and_then(|c| cols.get(c as usize)) {
                        extra.insert("column".into(), json!(c));
                    }
                    if let Some(a) = f.get("acquisition") {
                        extra.insert("acquisition".into(), a.clone());
                    }
                    groups.push((join(wp, fp), Some(format!("{wp}/{fp}")), extra));
                }
            }
        } else if ngff_root.get("bioformats2raw.layout").is_some() {
            ds.layout = "bioformats2raw";
            let listed: Option<Vec<String>> = ngff::group_attrs(&ds.store, "OME")?.and_then(|a| {
                a.ngff.get("series").and_then(Value::as_array).map(|s| {
                    s.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
            });
            let series = if let Some(s) = listed {
                s
            } else {
                let mut s = Vec::new();
                while ngff::group_attrs(&ds.store, &s.len().to_string())?.is_some() {
                    s.push(s.len().to_string());
                    if s.len() > 1_000_000 {
                        break;
                    }
                }
                s
            };
            for s in &series {
                let mut extra = BTreeMap::new();
                extra.insert("series".into(), json!(s));
                groups.push((s.clone(), Some(s.clone()), extra));
            }
            ds.load_ome_xml()?;
        } else {
            return Err(Error::unsupported(
                FORMAT_ID,
                "a Zarr group without OME-NGFF image metadata (no multiscales, plate or bioformats2raw.layout)",
                "Only OME-Zarr images, HCS plates and bioformats2raw collections are read; `info --view structure` on a sub-group path may find the image.",
            ));
        }
        ds.labels = label_names(&ds.store, "")?;
        for (series_index, (group, name, extra)) in groups.into_iter().enumerate() {
            let attrs = if group.is_empty() {
                Some(ds.root.clone())
            } else {
                ngff::group_attrs(&ds.store, &group)?
            };
            let Some(attrs) = attrs else {
                ds.notes.push(format!("image group {group} is missing"));
                continue;
            };
            let omero = attrs.ngff.get("omero").cloned();
            let labels = if group.is_empty() {
                Vec::new()
            } else {
                label_names(&ds.store, &group)?
            };
            let ms = ngff::multiscales(&group, &attrs.ngff);
            if ms.is_empty() {
                ds.notes
                    .push(format!("group {group} has no multiscales metadata"));
                continue;
            }
            if ds.ngff_version.is_none() {
                ds.ngff_version = ms[0].version.clone();
            }
            for m in ms {
                if m.levels.is_empty() {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("multiscales of {group:?} lists no datasets"),
                    ));
                }
                let mut levels = Vec::new();
                for l in &m.levels {
                    let meta = array_meta(&ds.store, &l.path)?.ok_or_else(|| {
                        Error::corrupt(
                            FORMAT_ID,
                            format!(
                                "dataset {} is listed but its array metadata is missing",
                                l.path
                            ),
                        )
                    })?;
                    levels.push(meta);
                }
                let mut roles = axis_roles(&m.axes);
                let rank = levels[0].shape.len();
                if m.implied_axes && rank < roles.len() {
                    // NGFF 0.1/0.2 arrays are 5-D; tolerate lower ranks as trailing axes.
                    roles = roles[roles.len() - rank..].to_vec();
                }
                if roles.len() != rank {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "{}: {} axes in the metadata but the array has {rank} dimensions",
                            levels[0].path,
                            roles.len()
                        ),
                    ));
                }
                if !roles.contains(&AxisRole::X) || !roles.contains(&AxisRole::Y) {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        format!(
                            "an image without x and y axes ({:?})",
                            m.axes.iter().map(|a| &a.name).collect::<Vec<_>>()
                        ),
                        "Planes are read along the x and y axes; this multiscales names neither.",
                    ));
                }
                let index = ds.images.len() as u32;
                let mut info = image_info(index, &m, &roles, &levels, omero.as_ref());
                if let Some(n) = &name
                    && info.name.is_none()
                {
                    info.name = Some(n.clone());
                }
                if ds.layout == "plate"
                    && let Some(n) = &name
                {
                    info.name = Some(n.clone());
                }
                for (k, v) in &extra {
                    info.extra.insert(k.clone(), v.clone());
                }
                if !labels.is_empty() {
                    info.extra.insert("labels".into(), json!(labels));
                }
                // bioformats2raw series `N` is OME `Image` N of the metadata document
                let ome_index = extra
                    .get("series")
                    .and_then(Value::as_str)
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(series_index);
                if let Some((doc, _)) = &ds.ome_xml
                    && let Some(oi) = doc.images.get(ome_index)
                {
                    apply_ome(&mut info, oi, doc);
                }
                if levels[0].pixel_type.is_none() {
                    info.extra
                        .insert("unsupported_dtype".into(), json!(levels[0].dtype));
                }
                ds.images.push(ZarrImage {
                    multiscale: m,
                    roles,
                    levels,
                    omero: omero.clone(),
                    labels: labels.clone(),
                    info,
                });
            }
        }
        if ds.images.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!("the {} holds no readable image", ds.layout),
            ));
        }
        let zoned = |a: &str| {
            a.ends_with('Z')
                || a.get(19..)
                    .is_some_and(|r| r.contains('+') || r.contains('-'))
        };
        if ds
            .images
            .iter()
            .filter_map(|i| i.info.acquired_at.as_deref())
            .any(|a| !zoned(a))
        {
            ds.notes.push(
                "acquired_at comes from the OME-XML AcquisitionDate, written without a time zone"
                    .into(),
            );
        }
        ds.add_label_images()?;
        if ds.images.iter().any(|i| i.levels[0].pixel_type.is_none()) {
            ds.notes.push("some arrays have a data type that is not read (strings, structured or other types); their planes exit 6".into());
        }
        if ds.images.iter().any(|i| is_half(&i.levels[0].dtype)) {
            ds.notes
                .push("float16 arrays are returned as 32-bit floats (exactly)".into());
        }
        Ok(ds)
    }

    /// Label images (`<group>/labels/<name>`, NGFF `image-label`) as images after the others:
    /// one per label multiscales, named `<group>/labels/<name>`, `extra.label_of` = the image
    /// index they annotate (absent for root-level labels of a plate or collection).
    fn add_label_images(&mut self) -> Result<()> {
        let mut todo: Vec<(String, Option<u32>, String)> = Vec::new();
        for (i, im) in self.images.iter().enumerate() {
            for l in &im.labels {
                todo.push((im.multiscale.group.clone(), Some(i as u32), l.clone()));
            }
        }
        if self.images.len() == 1 && self.images[0].multiscale.group.is_empty() {
            // a single root image: its labels are the root labels
            for l in &self.labels {
                todo.push((String::new(), Some(0), l.clone()));
            }
        } else {
            for l in &self.labels {
                todo.push((String::new(), None, l.clone()));
            }
        }
        for (parent, of, name) in todo {
            let group = join(&join(&parent, "labels"), &name);
            let Some(attrs) = ngff::group_attrs(&self.store, &group)? else {
                self.notes
                    .push(format!("label image {group} is listed but missing"));
                continue;
            };
            for m in ngff::multiscales(&group, &attrs.ngff) {
                let mut levels = Vec::new();
                for l in &m.levels {
                    match array_meta(&self.store, &l.path)? {
                        Some(meta) => levels.push(meta),
                        None => break,
                    }
                }
                if levels.is_empty() || levels.len() != m.levels.len() {
                    self.notes.push(format!(
                        "label image {group}: a listed level has no array metadata; not exposed"
                    ));
                    continue;
                }
                let roles = axis_roles(&m.axes);
                if roles.len() != levels[0].shape.len()
                    || !roles.contains(&AxisRole::X)
                    || !roles.contains(&AxisRole::Y)
                {
                    self.notes.push(format!(
                        "label image {group}: axes do not match its array; not exposed"
                    ));
                    continue;
                }
                let index = self.images.len() as u32;
                let mut info = image_info(index, &m, &roles, &levels, None);
                info.name = Some(group.clone());
                info.extra.insert("label".into(), Value::Bool(true));
                if let Some(of) = of {
                    info.extra.insert("label_of".into(), json!(of));
                }
                if let Some(il) = attrs.ngff.get("image-label") {
                    info.extra.insert("image_label".into(), il.clone());
                }
                self.images.push(ZarrImage {
                    multiscale: m,
                    roles,
                    levels,
                    omero: None,
                    labels: Vec::new(),
                    info,
                });
            }
        }
        Ok(())
    }

    /// The images found in the store.
    pub fn images(&self) -> &[ZarrImage] {
        &self.images
    }

    fn image(&self, i: u32) -> Result<&ZarrImage> {
        self.images.get(i as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {i} out of range (0..{})",
                self.images.len()
            ))
        })
    }

    fn array(&mut self, path: &str) -> Result<Arc<ZArray>> {
        if let Some(a) = self.arrays.get(path) {
            return Ok(a.clone());
        }
        let a = Array::open(self.store.storage.clone(), &format!("/{path}"))
            .map_err(|e| Error::corrupt(FORMAT_ID, format!("array {path}: {e}")))?;
        let a = Arc::new(a);
        self.arrays.insert(path.to_string(), a.clone());
        Ok(a)
    }

    fn read_level(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Option<Region>,
    ) -> Result<Plane> {
        let im = self.image(image)?;
        let meta = im.levels.get(level as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "level {level} out of range for image {image} (0..{})",
                im.levels.len()
            ))
        })?;
        let roles = im.roles.clone();
        let Some(pixel_type) = meta.pixel_type else {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("Zarr data type {}", meta.dtype),
                "Read: bool, 8/16/32/64-bit integers, 16/32/64-bit floats and complex64/128 arrays; this data type is not.",
            ));
        };
        let half = is_half(&meta.dtype);
        let mut ranges = Vec::with_capacity(roles.len());
        let (mut w, mut h) = (1u64, 1u64);
        let (mut full_w, mut full_h) = (1u64, 1u64);
        for (d, r) in roles.iter().enumerate() {
            match r {
                AxisRole::X => full_w = meta.shape.get(d).copied().unwrap_or(1),
                AxisRole::Y => full_h = meta.shape.get(d).copied().unwrap_or(1),
                _ => {}
            }
        }
        if let Some(r) = region {
            r.check_within(
                u32::try_from(full_w).unwrap_or(u32::MAX),
                u32::try_from(full_h).unwrap_or(u32::MAX),
                &format!("image {image} level {level}"),
            )?;
        }
        for (d, r) in roles.iter().enumerate() {
            let n = meta.shape.get(d).copied().unwrap_or(1);
            let pick = |v: u32, axis: &str| -> Result<std::ops::Range<u64>> {
                if u64::from(v) >= n {
                    return Err(Error::Usage(format!(
                        "{axis}={v} out of range for image {image} level {level} (size {n})"
                    )));
                }
                Ok(u64::from(v)..u64::from(v) + 1)
            };
            ranges.push(match (r, region) {
                (AxisRole::X, Some(g)) => {
                    w = u64::from(g.width);
                    u64::from(g.x)..g.right()
                }
                (AxisRole::Y, Some(g)) => {
                    h = u64::from(g.height);
                    u64::from(g.y)..g.bottom()
                }
                (AxisRole::X, None) => {
                    w = n;
                    0..n
                }
                (AxisRole::Y, None) => {
                    h = n;
                    0..n
                }
                (AxisRole::C, _) => pick(index.c, "c")?,
                (AxisRole::Z, _) => pick(index.z, "z")?,
                (AxisRole::T, _) => pick(index.t, "t")?,
                (AxisRole::Other, _) => 0..1.min(n),
            });
        }
        if roles.iter().all(|r| *r != AxisRole::C) && index.c > 0
            || roles.iter().all(|r| *r != AxisRole::Z) && index.z > 0
            || roles.iter().all(|r| *r != AxisRole::T) && index.t > 0
        {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image}",
                index.c, index.z, index.t
            )));
        }
        let bps = pixel_type.bytes_per_sample() as u64;
        if w.saturating_mul(h).saturating_mul(bps) > MAX_PLANE_BYTES {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("a {w}x{h} plane larger than 4 GiB"),
                "Read a region of it (`--region X,Y,W,H`) or a downsampled pyramid level (`--level N`) instead.",
            ));
        }
        let arr = self.array(&meta.path)?;
        let subset = ArraySubset::new_with_ranges(&ranges);
        let bytes: ArrayBytes<'static> = arr.retrieve_array_subset(&subset).map_err(|e| {
            Error::corrupt(
                FORMAT_ID,
                format!(
                    "{}: cannot read c={} z={} t={}: {e}",
                    meta.path, index.c, index.z, index.t
                ),
            )
        })?;
        let raw = bytes
            .into_fixed()
            .map_err(|e| Error::corrupt(FORMAT_ID, format!("{}: {e}", meta.path)))?;
        let mut data = raw.into_owned();
        let unit = if half {
            2
        } else if pixel_type.is_complex() {
            bps as usize / 2
        } else {
            bps as usize
        };
        if cfg!(target_endian = "big") && unit > 1 {
            for s in data.chunks_exact_mut(unit) {
                s.reverse();
            }
        }
        if half {
            data = data
                .as_chunks::<2>()
                .0
                .iter()
                .flat_map(|c| half_to_f32(u16::from_le_bytes(*c)).to_le_bytes())
                .collect();
        }
        if meta.dtype.trim_start_matches(['<', '>', '|', '=']) == "b1" || meta.dtype == "bool" {
            for v in &mut data {
                *v = u8::from(*v != 0);
            }
        }
        // x before y in the array: transpose to row-major y, x
        let (xi, yi) = (
            role_index(&roles, AxisRole::X).unwrap_or(0),
            role_index(&roles, AxisRole::Y).unwrap_or(0),
        );
        if xi < yi {
            let b = bps as usize;
            let (wu, hu) = (w as usize, h as usize);
            let mut t = vec![0u8; data.len()];
            for x in 0..wu {
                for y in 0..hu {
                    let src = (x * hu + y) * b;
                    let dst = (y * wu + x) * b;
                    t[dst..dst + b].copy_from_slice(&data[src..src + b]);
                }
            }
            data = t;
        }
        let plane = Plane {
            width: u32::try_from(w).unwrap_or(u32::MAX),
            height: u32::try_from(h).unwrap_or(u32::MAX),
            pixel_type,
            samples_per_pixel: 1,
            data,
        };
        if plane.data.len() != plane.expected_len() {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "{}: read {} bytes, expected {}",
                    meta.path,
                    plane.data.len(),
                    plane.expected_len()
                ),
            ));
        }
        Ok(plane)
    }

    fn check_array(&mut self, meta: &ArrayMeta, r: &mut CheckReport) {
        let arr = match self.array(&meta.path) {
            Ok(a) => a,
            Err(e) => {
                r.push(Finding::error("array_metadata", e.to_string()));
                return;
            }
        };
        if arr.shape() != meta.shape.as_slice() {
            r.push(Finding::error(
                "array_metadata",
                format!(
                    "{}: zarrs reads shape {:?}, metadata says {:?}",
                    meta.path,
                    arr.shape(),
                    meta.shape
                ),
            ));
        }
        let grid: Vec<u64> = arr.chunk_grid_shape().to_vec();
        let total: u64 = grid.iter().product();
        let step = total.div_ceil(MAX_CHUNKS_CHECKED).max(1);
        let mut present = 0u64;
        let mut checked = 0u64;
        let mut first: Option<Vec<u64>> = None;
        let mut last: Option<Vec<u64>> = None;
        let mut k = 0u64;
        while k < total {
            let mut idx = vec![0u64; grid.len()];
            let mut rem = k;
            for d in (0..grid.len()).rev() {
                idx[d] = rem % grid[d].max(1);
                rem /= grid[d].max(1);
            }
            let key = arr.chunk_key(&idx);
            checked += 1;
            if self.store.size(key.as_str()).is_some() {
                present += 1;
                if first.is_none() {
                    first = Some(idx.clone());
                }
                last = Some(idx);
            }
            k += step;
        }
        if present < checked {
            r.push(Finding::info(
                "missing_chunks",
                format!(
                    "{}: {} of {checked} checked chunks are not stored (read as the fill value)",
                    meta.path,
                    checked - present
                ),
            ));
        }
        for idx in [first, last].into_iter().flatten() {
            let res: std::result::Result<ArrayBytes<'static>, _> = arr.retrieve_chunk(&idx);
            if let Err(e) = res {
                r.push(Finding::error(
                    "chunk_decode",
                    format!("{}: chunk {idx:?} cannot be decoded: {e}", meta.path),
                ));
                break;
            }
        }
    }
}

impl ZarrDataset {
    /// Parse `OME/METADATA.ome.xml` when the store has one.
    fn load_ome_xml(&mut self) -> Result<()> {
        if let Some(xml) = self.store.get("OME/METADATA.ome.xml")? {
            let text = String::from_utf8_lossy(&xml);
            match parse_ome_xml(&text) {
                Some(doc) => self.ome_xml = Some((doc, xml.len())),
                None => self
                    .notes
                    .push("OME/METADATA.ome.xml is present but is not OME-XML".into()),
            }
        }
        Ok(())
    }

    /// Growing-file evidence (see [`Dataset::write_state`]) for a directory store: the
    /// metadata documents are written first and chunk files appear one by one, so a level-0
    /// array whose stored chunks form a strict prefix of the chunk grid in C order (the order
    /// an acquisition fills `t, c, z`) is unfinished. Chunks missing in the middle mean a
    /// sparse array read as the fill value (finished): `None`. A plane is complete when every
    /// chunk it touches is stored.
    fn growing_state(&self) -> Option<openreadout_core::live::WriteState> {
        if self.store.kind != crate::store::StoreKind::Directory || self.images.is_empty() {
            return None;
        }
        let mut ws = openreadout_core::live::WriteState::new();
        let mut newest: Option<std::time::SystemTime> = None;
        let mut incomplete = false;
        let mut expected = 0u64;
        let stat = |key: &str| -> Option<openreadout_core::source::EntryMeta> {
            self.store
                .fs
                .metadata(&self.store.path.join(key.trim_start_matches('/')))
                .ok()
        };
        for (image, im) in self.images.iter().enumerate() {
            let meta = im.levels.first()?;
            let arr: ZArray =
                Array::open(self.store.storage.clone(), &format!("/{}", meta.path)).ok()?;
            let grid: Vec<u64> = arr.chunk_grid_shape().to_vec();
            let total: u64 = grid.iter().product();
            if grid.len() != im.roles.len() || total == 0 || total > 4_000_000 {
                return None;
            }
            // Walk the chunk grid in C order; stored chunks must form a prefix.
            let mut stored = Vec::with_capacity(total as usize);
            let mut gap = false;
            for k in 0..total {
                let mut idx = vec![0u64; grid.len()];
                let mut rem = k;
                for d in (0..grid.len()).rev() {
                    idx[d] = rem % grid[d].max(1);
                    rem /= grid[d].max(1);
                }
                let key = arr.chunk_key(&idx);
                let m = stat(key.as_str());
                if let Some(m) = &m {
                    if gap {
                        return None; // a hole before a stored chunk: sparse, not growing
                    }
                    if let Ok(t) = m.modified() {
                        newest = Some(newest.map_or(t, |n| n.max(t)));
                    }
                    if ws.unit_bytes.is_none() {
                        ws.unit_bytes = Some(m.len());
                    }
                } else {
                    gap = true;
                }
                stored.push(m.is_some());
            }
            let (nc, nz, nt) = (im.info.size_c, im.info.size_z, im.info.size_t);
            expected += u64::from(nc) * u64::from(nz) * u64::from(nt);
            if stored.iter().all(|s| *s) {
                // A finished image (an earlier well of a plate): all of its planes are complete.
                for t in 0..nt {
                    for c in 0..nc {
                        for z in 0..nz {
                            ws.complete.push((image as u32, PlaneIndex { c, z, t }));
                        }
                    }
                }
                continue;
            }
            // An image with no chunk yet has not started (a plate acquired well by well).
            incomplete = true;
            let chunks = arr.chunk_shape(&vec![0; grid.len()]).ok()?;
            let chunk_len: Vec<u64> = chunks.iter().map(|c| c.get()).collect();
            // Every plane (in array axis order) whose chunks are all stored.
            let mut planes: Vec<(Vec<u64>, PlaneIndex)> = Vec::new();
            for t in 0..nt {
                for c in 0..nc {
                    for z in 0..nz {
                        let mut pos = Vec::with_capacity(grid.len());
                        for r in &im.roles {
                            pos.push(match r {
                                AxisRole::T => u64::from(t),
                                AxisRole::C => u64::from(c),
                                AxisRole::Z => u64::from(z),
                                _ => 0,
                            });
                        }
                        planes.push((pos, PlaneIndex { c, z, t }));
                    }
                }
            }
            planes.sort_by(|a, b| a.0.cmp(&b.0));
            for (pos, idx) in planes {
                let complete = all_chunks_stored(&grid, &chunk_len, &im.roles, &pos, &stored);
                if complete {
                    ws.complete.push((image as u32, idx));
                }
            }
        }
        if !incomplete {
            return None;
        }
        ws.missing.push("chunk files".into());
        ws.expected_planes = Some(expected);
        ws.modified = newest;
        ws.evidence.push(
            "the stored level-0 chunks are a prefix of the chunk grid in acquisition order; the rest are not written yet".into(),
        );
        Some(ws)
    }
}

/// Are all chunks touched by the plane at array position `pos` (X and Y axes: every chunk)
/// stored? `stored` is indexed in C order of `grid`.
fn all_chunks_stored(
    grid: &[u64],
    chunk_len: &[u64],
    roles: &[AxisRole],
    pos: &[u64],
    stored: &[bool],
) -> bool {
    let mut ranges: Vec<(u64, u64)> = Vec::with_capacity(grid.len());
    for (d, r) in roles.iter().enumerate() {
        if matches!(r, AxisRole::X | AxisRole::Y) {
            ranges.push((0, grid[d]));
        } else {
            let i = pos[d] / chunk_len[d].max(1);
            ranges.push((i, i + 1));
        }
    }
    let mut idx: Vec<u64> = ranges.iter().map(|r| r.0).collect();
    loop {
        let mut k = 0u64;
        for d in 0..grid.len() {
            k = k * grid[d].max(1) + idx[d];
        }
        if !stored.get(k as usize).copied().unwrap_or(false) {
            return false;
        }
        let mut d = grid.len();
        loop {
            if d == 0 {
                return true;
            }
            d -= 1;
            idx[d] += 1;
            if idx[d] < ranges[d].1 {
                break;
            }
            idx[d] = ranges[d].0;
        }
    }
}

impl Dataset for ZarrDataset {
    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        crate::assurance::internal(self.images())
    }

    fn write_state(&self) -> Option<openreadout_core::live::WriteState> {
        self.growing_state()
    }

    fn info(&self) -> Result<FileInfo> {
        let images: Vec<ImageInfo> = self.images.iter().map(|i| i.info.clone()).collect();
        let mut notes = self.notes.clone();
        let nl = self
            .images
            .iter()
            .filter(|i| i.info.extra.contains_key("label"))
            .count();
        if nl > 0 {
            notes.push(format!(
                "{nl} label image(s) are exposed after the images (extra.label, extra.label_of; values are label ids)"
            ));
        }
        if self.images.iter().any(|i| i.levels.len() > 1) {
            notes.push("pyramid levels are readable with `planes --level N`".into());
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        let size_bytes = match self.store.kind {
            crate::store::StoreKind::Zip => {
                self.store.fs.metadata(&self.path).map_or(0, |m| m.len())
            }
            crate::store::StoreKind::Directory => dir_size(&self.store.fs, &self.store.path),
        };
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes,
            format: ZarrReader.descriptor(),
            format_version: Some(format!(
                "{} (NGFF, Zarr v{}, {} store)",
                self.ngff_version.as_deref().unwrap_or("unknown"),
                self.zarr_format.number(),
                self.store.kind.name()
            )),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let images: Vec<Value> = self
            .images
            .iter()
            .map(|i| {
                json!({
                    "group": i.multiscale.group,
                    "omero": i.omero,
                    "levels": i.levels.iter().map(|l| json!({"path": l.path, "shape": l.shape, "chunks": l.chunks, "dtype": l.dtype, "codecs": l.codecs})).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({
            "root_attributes": self.root.raw,
            "layout": self.layout,
            "images": images,
            "labels": self.labels,
            "ome_xml_bytes": self.ome_xml.as_ref().map(|(_, n)| *n),
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
            "images[].physical_size",
            "images[].channels",
            "images[].pyramid_levels",
        ] {
            p.insert(k.into(), Source::Spec);
        }
        let any = |f: &dyn Fn(&ImageInfo) -> bool| self.images.iter().any(|i| f(&i.info));
        if any(&|i| i.time_increment_s.is_some()) {
            p.insert("images[].time_increment_s".into(), Source::Spec);
        }
        if any(&|i| i.name.is_some()) {
            p.insert("images[].name".into(), Source::Spec);
        }
        if self.ome_xml.is_some() {
            for k in [
                "images[].objective",
                "images[].instrument",
                "images[].acquired_at",
            ] {
                p.insert(k.into(), Source::Spec);
            }
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        out.push(LsEntry {
            kind: "store".into(),
            name: self.path.display().to_string(),
            offset: None,
            size: None,
            image: None,
            details: json!({
                "store": self.store.kind.name(),
                "zarr_format": self.zarr_format.number(),
                "ngff_version": self.ngff_version,
                "layout": self.layout,
                "prefix": self.store.prefix,
                "members": self.store.zip_summary().map(|(n, _)| n),
            }),
        });
        if let Some(plate) = self.root.ngff.get("plate") {
            out.push(LsEntry {
                kind: "plate".into(),
                name: plate
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("plate")
                    .to_string(),
                offset: None,
                size: None,
                image: None,
                details: json!({
                    "rows": plate.get("rows").and_then(Value::as_array).map(Vec::len),
                    "columns": plate.get("columns").and_then(Value::as_array).map(Vec::len),
                    "wells": plate.get("wells").and_then(Value::as_array).map(Vec::len),
                    "field_count": plate.get("field_count"),
                    "acquisitions": plate.get("acquisitions"),
                }),
            });
        }
        if let Some((doc, n)) = &self.ome_xml {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: "OME/METADATA.ome.xml".into(),
                offset: None,
                size: Some(*n as u64),
                image: None,
                details: json!({"images": doc.images.len(), "creator": doc.creator}),
            });
        }
        for (i, im) in self.images.iter().enumerate() {
            let g = if im.multiscale.group.is_empty() {
                "/".to_string()
            } else {
                im.multiscale.group.clone()
            };
            out.push(LsEntry {
                kind: "image".into(),
                name: g,
                offset: None,
                size: None,
                image: Some(i as u32),
                details: json!({
                    "name": im.info.name,
                    "axes": im.multiscale.axes.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
                    "levels": im.levels.len(),
                    "labels": im.labels,
                }),
            });
            for (l, meta) in im.levels.iter().enumerate() {
                out.push(LsEntry {
                    kind: "pyramid-level".into(),
                    name: meta.path.clone(),
                    offset: None,
                    size: None,
                    image: Some(i as u32),
                    details: json!({
                        "level": l,
                        "shape": meta.shape,
                        "chunks": meta.chunks,
                        "dtype": meta.dtype,
                        "codecs": meta.codecs,
                        "scale": im.multiscale.levels.get(l).map(|x| x.scale.clone()),
                    }),
                });
            }
            for lab in &im.labels {
                out.push(LsEntry {
                    kind: "label".into(),
                    name: join(&join(&im.multiscale.group, "labels"), lab),
                    offset: None,
                    size: None,
                    image: Some(i as u32),
                    details: Value::Null,
                });
            }
        }
        for lab in &self.labels {
            out.push(LsEntry {
                kind: "label".into(),
                name: join("labels", lab),
                offset: None,
                size: None,
                image: None,
                details: Value::Null,
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        self.read_level(image, index, 0, None)
    }

    fn read_plane_level(&mut self, image: u32, index: PlaneIndex, level: u32) -> Result<Plane> {
        self.read_level(image, index, level, None)
    }

    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        self.read_level(image, index, level, Some(region))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("store opens; the NGFF group metadata (multiscales / plate / well / bioformats2raw) parses");
        r.performed("every listed dataset has array metadata whose rank matches the axes");
        r.performed("pyramid levels do not grow in x or y; omero channel count matches the c axis");
        r.performed(format!(
            "stored chunks are counted (up to {MAX_CHUNKS_CHECKED} per array, evenly sampled beyond) and the first and last stored chunk of every array decodes"
        ));
        for n in &self.notes {
            r.push(Finding::warning("metadata", n.clone()));
        }
        let images = self.images.clone();
        for im in &images {
            let xi = role_index(&im.roles, AxisRole::X).unwrap_or(0);
            let yi = role_index(&im.roles, AxisRole::Y).unwrap_or(0);
            for pair in im.levels.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                if b.shape.len() != a.shape.len() {
                    r.push(Finding::error(
                        "pyramid",
                        format!("{} and {} have different ranks", a.path, b.path),
                    ));
                } else if b.shape.get(xi) > a.shape.get(xi) || b.shape.get(yi) > a.shape.get(yi) {
                    r.push(Finding::warning(
                        "pyramid",
                        format!("{} is larger than {} in x or y", b.path, a.path),
                    ));
                }
            }
            if let Some(n) = im
                .omero
                .as_ref()
                .and_then(|o| o.get("channels"))
                .and_then(Value::as_array)
                .map(Vec::len)
                && n as u32 != im.info.size_c
            {
                r.push(Finding::warning(
                    "omero_channels",
                    format!(
                        "{}: omero lists {n} channels, the c axis has {}",
                        im.multiscale.group, im.info.size_c
                    ),
                ));
            }
            if im.levels[0].pixel_type.is_none() {
                r.push(Finding::info(
                    "unsupported_dtype",
                    format!(
                        "{}: data type {} is not read",
                        im.levels[0].path, im.levels[0].dtype
                    ),
                ));
            }
            for meta in &im.levels {
                self.check_array(meta, &mut r);
            }
        }
        Ok(r)
    }
}

fn dir_size(fs: &openreadout_core::source::Fs, dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    let mut n = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs.read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            n += 1;
            if n > 2_000_000 {
                return total;
            }
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => total += e.metadata().map_or(0, |m| m.len()),
                _ => {}
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ngff::Axis;

    fn ax(n: &str, k: Option<&str>) -> Axis {
        Axis {
            name: n.into(),
            kind: k.map(str::to_string),
            unit: None,
        }
    }

    #[test]
    fn roles_by_name_and_type() {
        let r = axis_roles(&[
            ax("t", None),
            ax("c", None),
            ax("z", None),
            ax("y", None),
            ax("x", None),
        ]);
        assert_eq!(
            r,
            vec![
                AxisRole::T,
                AxisRole::C,
                AxisRole::Z,
                AxisRole::Y,
                AxisRole::X
            ]
        );
        let r = axis_roles(&[
            ax("channel", Some("channel")),
            ax("row", Some("space")),
            ax("col", Some("space")),
        ]);
        assert_eq!(r, vec![AxisRole::C, AxisRole::Y, AxisRole::X]);
        let r = axis_roles(&[ax("angle", Some("angle")), ax("y", None), ax("x", None)]);
        assert_eq!(r[0], AxisRole::Other);
        assert_eq!(pixel_type_of("<u2"), Some(PixelType::Uint16));
        assert_eq!(pixel_type_of("float32"), Some(PixelType::Float));
        assert_eq!(pixel_type_of("<i8"), Some(PixelType::Int64));
        assert_eq!(pixel_type_of("|b1"), Some(PixelType::Uint8));
        assert_eq!(pixel_type_of("<f2"), Some(PixelType::Float));
        assert_eq!(pixel_type_of("<U3"), None);
        assert_eq!(ome_color(-16_776_961), "#FF0000");
    }
}
