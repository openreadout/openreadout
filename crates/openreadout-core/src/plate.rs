//! Multi-well plates (high-content screening): the plate layout `info` reports under `plate`, well
//! names, and per-well pixel statistics (`stats --per well`, MCP `openreadout_stats` with
//! `per: "well"`).
//!
//! A plate reader exposes one image per (well, field of view) and describes the layout through
//! [`Dataset::plate`]: which wells were imaged, which images belong to each, and how complete
//! the copy on disk is. Readers without that hook (OME-Zarr plates) are described from their
//! images' `extra.row` / `extra.column` keys ([`plate_layout`]).
//!
//! Planes that cannot be read carry two lists in the image's `extra` (`docs/formats/hcs.md`):
//!
//! - `absent_planes`: `[[c, z, t], ...]` planes the instrument never acquired or recorded
//!   (a channel imaged at fewer Z planes than the others; a field the index lists without an
//!   image). They read as blank planes and are left out of every statistic.
//! - `missing_planes`: `[[c, z, t], ...]` planes whose files the index names but that are not
//!   on disk (a partial copy). Reading them fails; [`well_stats`] skips and counts them.
//!   `planes_missing` holds the count; when every plane of an image is missing the list is
//!   left out and `planes_missing` equals `plane_count`.

use std::collections::{BTreeMap, HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{FileInfo, ImageInfo};
use crate::parallel::{PlaneRequest, ReadContext, read_in_order};
use crate::reader::{Dataset, PlaneIndex};
use crate::select::Selection;
use crate::stats::{Accumulator, HistogramScale};
use crate::{Error, PixelType, Result};

/// Name of a plate row: 0 → `A`, 25 → `Z`, 26 → `AA`, 31 → `AF` (1536-well plates).
pub fn row_name(index: u32) -> String {
    let mut n = index;
    let mut out = Vec::new();
    loop {
        out.push(b'A' + (n % 26) as u8);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// Well name: row letters and the 1-based column zero-padded to two digits (`C05`, `AF48`).
pub fn well_name(row_index: u32, column_index: u32) -> String {
    format!(
        "{}{:02}",
        row_name(row_index),
        column_index.saturating_add(1)
    )
}

/// Parse a well name (`C05`, `c5`, `AA12`, `C-05`, `C/5`) into zero-based (row, column).
pub fn parse_well(name: &str) -> Option<(u32, u32)> {
    let s = name.trim();
    let letters: String = s.chars().take_while(char::is_ascii_alphabetic).collect();
    if letters.is_empty() || letters.len() > 2 {
        return None;
    }
    let rest = s[letters.len()..].trim_start_matches(['-', '/', '_', ' ']);
    if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let col: u32 = rest.parse().ok()?;
    if col == 0 {
        return None;
    }
    let mut row: u32 = 0;
    for (i, ch) in letters.to_ascii_uppercase().bytes().enumerate() {
        let v = u32::from(ch - b'A');
        row = if i == 0 { v } else { (row + 1) * 26 + v };
    }
    Some((row, col - 1))
}

/// One imaged well of a plate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlateWell {
    /// Well name, e.g. `C05` (row letters, column zero-padded to two digits).
    pub well: String,
    /// Row letters (`C`).
    pub row: String,
    /// Column number, 1-based (`5`).
    pub column: u32,
    /// Zero-based row index (`C` = 2).
    pub row_index: u32,
    /// Zero-based column index.
    pub column_index: u32,
    /// Indices of the images (fields of view) of this well, in field order.
    pub images: Vec<u32>,
    /// Planes of this well whose files are not on disk (0 when the well is complete).
    #[serde(default)]
    pub planes_missing: u64,
}

/// Layout of a multi-well plate (high-content screening): `info` → `plate`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlateSummary {
    /// Plate identifier or barcode as the acquisition software recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Plate name, when recorded besides the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Plate type (product) as recorded, e.g. `384 PerkinElmer CellCarrier Ultra`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plate_type: Option<String>,
    /// Number of rows of the plate (8 for a 96-well plate).
    pub rows: u32,
    /// Number of columns of the plate (12 for a 96-well plate).
    pub columns: u32,
    /// The imaged wells, row by row.
    pub wells: Vec<PlateWell>,
    /// Most fields of view in one well.
    pub field_count: u32,
    /// Planes the plate's index lists (every image's `plane_count`).
    pub planes_expected: u64,
    /// Planes the instrument never acquired or recorded (they read as blank and are left out
    /// of statistics; `images[].extra.absent_planes`).
    #[serde(default)]
    pub planes_absent: u64,
    /// Planes whose files the index names but that are not on disk (`check` lists them).
    #[serde(default)]
    pub planes_missing: u64,
    /// True when every plane the index names is on disk.
    pub complete: bool,
    /// Format-specific plate facts in the reader's documented vocabulary.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

impl PlateSummary {
    /// The well with this name (`C05`, `c5`), if it was imaged.
    pub fn well(&self, name: &str) -> Option<&PlateWell> {
        let (r, c) = parse_well(name)?;
        self.wells
            .iter()
            .find(|w| w.row_index == r && w.column_index == c)
    }
}

/// `(c, z, t)` triples listed under `extra[key]`.
fn plane_list(im: &ImageInfo, key: &str) -> HashSet<(u32, u32, u32)> {
    let mut out = HashSet::new();
    if let Some(Value::Array(a)) = im.extra.get(key) {
        for p in a {
            if let Some([c, z, t]) = p.as_array().and_then(|v| {
                let n: Vec<u32> = v
                    .iter()
                    .filter_map(|x| x.as_u64().and_then(|x| u32::try_from(x).ok()))
                    .collect();
                <[u32; 3]>::try_from(n).ok()
            }) {
                out.insert((c, z, t));
            }
        }
    }
    out
}

/// Planes of `im` that hold no data: listed in `extra.absent_planes` (never acquired).
pub fn absent_planes(im: &ImageInfo) -> HashSet<(u32, u32, u32)> {
    plane_list(im, "absent_planes")
}

/// Planes of `im` whose files are missing (`extra.missing_planes`), and whether every plane is
/// (`extra.planes_missing == plane_count`).
pub fn missing_planes(im: &ImageInfo) -> (HashSet<(u32, u32, u32)>, bool) {
    let n = im
        .extra
        .get("planes_missing")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    (
        plane_list(im, "missing_planes"),
        n > 0 && n >= im.plane_count,
    )
}

fn extra_str(im: &ImageInfo, key: &str) -> Option<String> {
    match im.extra.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The plate layout of an open dataset: the reader's own ([`Dataset::plate`]) or, for images
/// that carry `extra.row` and `extra.column` (OME-Zarr plates), one built from those keys.
/// `None` when the file is not a plate.
pub fn plate_layout(ds: &dyn Dataset, info: &FileInfo) -> Option<PlateSummary> {
    ds.plate().or_else(|| layout_from_images(info))
}

/// A layout from `extra.row` / `extra.column` (strings or numbers) of every image.
pub(crate) fn layout_from_images(info: &FileInfo) -> Option<PlateSummary> {
    if info.images.is_empty() {
        return None;
    }
    let mut wells: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
    for im in &info.images {
        let row = extra_str(im, "row")?;
        let col = extra_str(im, "column")?;
        let (r, c) = parse_well(&format!("{row}{col}"))?;
        wells.entry((r, c)).or_default().push(im.index);
    }
    let rows = wells.keys().map(|k| k.0 + 1).max().unwrap_or(0);
    let columns = wells.keys().map(|k| k.1 + 1).max().unwrap_or(0);
    let (rows, columns) = standard_dimensions(rows, columns);
    let field_count = wells.values().map(Vec::len).max().unwrap_or(0) as u32;
    let planes_expected = info.images.iter().map(|i| i.plane_count).sum();
    Some(PlateSummary {
        id: None,
        name: None,
        plate_type: None,
        rows,
        columns,
        wells: wells
            .into_iter()
            .map(|((r, c), images)| PlateWell {
                well: well_name(r, c),
                row: row_name(r),
                column: c + 1,
                row_index: r,
                column_index: c,
                images,
                planes_missing: 0,
            })
            .collect(),
        field_count,
        planes_expected,
        planes_absent: 0,
        planes_missing: 0,
        complete: true,
        extra: BTreeMap::new(),
    })
}

/// The smallest standard plate (6, 12, 24, 48, 96, 384, 1536 wells) that holds `rows` x
/// `columns`; the size itself when none does.
pub fn standard_dimensions(rows: u32, columns: u32) -> (u32, u32) {
    for (r, c) in [(2, 3), (3, 4), (4, 6), (6, 8), (8, 12), (16, 24), (32, 48)] {
        if rows <= r && columns <= c {
            return (r, c);
        }
    }
    (rows, columns)
}

// ---------------------------------------------------------------------------------------------
// per-well statistics
// ---------------------------------------------------------------------------------------------

/// What [`well_stats`] computes.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WellStatsRequest {
    /// Plane selection strings (`c=0`, `z=1-3`, `t=0`); empty = every plane.
    pub select: Vec<String>,
    /// Only these wells (`C05`, `c5`); empty = every imaged well.
    pub wells: Vec<String>,
    /// One row per (well, field, channel) instead of per (well, channel).
    pub per_field: bool,
    /// Pyramid level (0 = full resolution).
    pub level: u32,
}

/// One tidy row of [`WellStatsOutput`]: the statistics of one channel over the selected planes
/// of one well (or one field of it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WellStatsRow {
    /// Plate id or barcode, when recorded (the same on every row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plate: Option<String>,
    /// Well name (`C05`).
    pub well: String,
    /// Row letters.
    pub row: String,
    /// Column number, 1-based.
    pub column: u32,
    /// Per-field rows only: the image index of the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<u32>,
    /// Per-field rows only: the field number as the acquisition software counts it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<u32>,
    /// Channel index.
    pub c: u32,
    /// Channel name, if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// Fields of view whose planes were read into this row.
    pub fields: u32,
    /// Planes aggregated.
    pub planes: u64,
    /// Selected planes whose files are missing (skipped).
    pub planes_missing: u64,
    /// Finite samples.
    pub count: u64,
    /// Mean intensity (absent when no plane was read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
    /// Population standard deviation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub std: Option<f64>,
    /// Smallest sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Largest sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// 1st percentile (NumPy `linear` interpolation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p1: Option<f64>,
    /// Median.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median: Option<f64>,
    /// 99th percentile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p99: Option<f64>,
    /// Fraction of samples equal to 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zero_fraction: Option<f64>,
    /// Integer data: fraction of samples at the type's maximum (detector saturation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturated_fraction: Option<f64>,
}

/// Output of `stats --per well` and MCP `openreadout_stats` with `per: "well"`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WellStatsOutput {
    /// The input file.
    pub path: String,
    /// Format id of the input file.
    pub format: String,
    /// Plate id or barcode, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plate: Option<String>,
    /// Pyramid level read (0 = full resolution); omitted when 0.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub level: u32,
    /// The selection as given; empty = every plane.
    pub select: Vec<String>,
    /// True when rows are per field of view.
    pub per_field: bool,
    /// Column names of `rows`, in order (for tables and CSV).
    pub columns: Vec<String>,
    /// One row per (well, channel), or per (well, field, channel) with `per_field`, in plate
    /// order (row by row, then field, then channel).
    pub rows: Vec<WellStatsRow>,
    /// Selected wells without a single readable plane (every file missing).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wells_without_data: Vec<String>,
    /// Things to know when reading the rows (absent planes left out, missing files skipped).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// Column names of [`WellStatsRow`] in serialization order.
pub const WELL_STATS_COLUMNS: [&str; 21] = [
    "plate",
    "well",
    "row",
    "column",
    "image",
    "field",
    "c",
    "channel",
    "fields",
    "planes",
    "planes_missing",
    "count",
    "mean",
    "std",
    "min",
    "max",
    "p1",
    "median",
    "p99",
    "zero_fraction",
    "saturated_fraction",
];

/// Per-well (or per-field) statistics of every selected channel of a plate. Planes the
/// instrument never acquired (`extra.absent_planes`) are left out; planes whose files are
/// missing are skipped and counted in `planes_missing`. Fails for files that are not plates.
pub fn well_stats(
    ds: &mut dyn Dataset,
    info: &FileInfo,
    req: &WellStatsRequest,
    ctx: &ReadContext<'_>,
) -> Result<WellStatsOutput> {
    type Acc = (u64, HashSet<u32>, Accumulator);
    let Some(layout) = plate_layout(ds, info) else {
        return Err(Error::unsupported(
            "stats",
            format!("per-well statistics of a {} file", info.format.name),
            "This file is not a multi-well plate (no well of any image is known); use `openreadout stats FILE` for per-image statistics.",
        ));
    };
    let sel = Selection::parse(&req.select)?;
    let mut wanted: Vec<&PlateWell> = Vec::new();
    for w in &req.wells {
        let found = layout.well(w).ok_or_else(|| {
            let imaged: Vec<&str> = layout
                .wells
                .iter()
                .map(|w| w.well.as_str())
                .take(12)
                .collect();
            Error::Usage(format!(
                "well '{w}' was not imaged on this plate (imaged: {}{})",
                imaged.join(", "),
                if layout.wells.len() > 12 { ", ..." } else { "" }
            ))
        })?;
        if !wanted.iter().any(|x| x.well == found.well) {
            wanted.push(found);
        }
    }
    if req.wells.is_empty() {
        wanted = layout.wells.iter().collect();
    }
    wanted.sort_by_key(|w| (w.row_index, w.column_index));
    let by_index: HashMap<u32, &ImageInfo> = info.images.iter().map(|i| (i.index, i)).collect();

    // (well slot, image, c) of each request; missing planes per (well slot, image, c).
    let mut requests = Vec::new();
    let mut slots: Vec<(usize, u32, u32)> = Vec::new();
    let mut missing: BTreeMap<(usize, u32, u32), u64> = BTreeMap::new();
    let mut absent_total = 0u64;
    let mut plane_bytes = 0u64;
    for (wi, w) in wanted.iter().enumerate() {
        for &ii in &w.images {
            let Some(im) = by_index.get(&ii) else {
                continue;
            };
            let absent = absent_planes(im);
            let (miss, all_missing) = missing_planes(im);
            plane_bytes = plane_bytes.max(
                u64::from(im.size_x)
                    * u64::from(im.size_y)
                    * u64::from(im.samples_per_pixel.max(1))
                    * im.pixel_type.bytes_per_sample() as u64,
            );
            for c in 0..im.size_c {
                for z in 0..im.size_z {
                    for t in 0..im.size_t {
                        if !sel.contains(c, z, t) {
                            continue;
                        }
                        if absent.contains(&(c, z, t)) {
                            absent_total += 1;
                            continue;
                        }
                        if all_missing || miss.contains(&(c, z, t)) {
                            *missing.entry((wi, ii, c)).or_default() += 1;
                            continue;
                        }
                        slots.push((wi, ii, c));
                        requests.push(PlaneRequest {
                            image: ii,
                            index: PlaneIndex { c, z, t },
                            level: req.level,
                            region: None,
                        });
                    }
                }
            }
        }
    }
    let mut per_well: BTreeMap<(usize, u32), Acc> = BTreeMap::new();
    let mut per_field: BTreeMap<(usize, u32, u32), Acc> = BTreeMap::new();
    let total = requests.len() as u64;
    let want_fields = req.per_field;
    read_in_order(
        ds,
        ctx,
        &requests,
        plane_bytes,
        &|_, p| Ok(Accumulator::from_plane(&p)),
        &mut |i, acc| {
            let (wi, ii, c) = slots[i];
            let pt = info
                .images
                .iter()
                .find(|im| im.index == ii)
                .map_or(PixelType::Uint16, |im| im.pixel_type);
            let e = per_well
                .entry((wi, c))
                .or_insert_with(|| (0, HashSet::new(), Accumulator::new(pt)));
            e.0 += 1;
            e.1.insert(ii);
            e.2.merge(&acc);
            if want_fields {
                let f = per_field
                    .entry((wi, ii, c))
                    .or_insert_with(|| (0, HashSet::new(), Accumulator::new(pt)));
                f.0 += 1;
                f.1.insert(ii);
                f.2.merge(&acc);
            }
            ctx.report(i as u64 + 1, total);
            Ok(())
        },
    )?;

    let channel_name = |ii: u32, c: u32| {
        by_index
            .get(&ii)
            .and_then(|im| im.channels.iter().find(|ch| ch.index == c))
            .and_then(|ch| ch.name.clone())
    };
    let field_number = |ii: u32| {
        by_index
            .get(&ii)
            .and_then(|im| im.extra.get("field"))
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .and_then(|v| u32::try_from(v).ok())
    };
    let make_row = |w: &PlateWell, image: Option<u32>, c: u32, acc: Option<&Acc>, miss: u64| {
        let stats = acc.map(|a| a.2.finish(0, HistogramScale::Linear));
        let first = image.or_else(|| w.images.first().copied()).unwrap_or(0);
        WellStatsRow {
            plate: layout.id.clone(),
            well: w.well.clone(),
            row: w.row.clone(),
            column: w.column,
            image,
            field: image.and_then(field_number),
            c,
            channel: channel_name(first, c),
            fields: acc.map_or(0, |a| a.1.len() as u32),
            planes: acc.map_or(0, |a| a.0),
            planes_missing: miss,
            count: stats.as_ref().map_or(0, |s| s.count),
            mean: stats.as_ref().and_then(|s| s.mean),
            std: stats.as_ref().and_then(|s| s.std),
            min: stats.as_ref().and_then(|s| s.min),
            max: stats.as_ref().and_then(|s| s.max),
            p1: stats
                .as_ref()
                .and_then(|s| s.percentiles.as_ref().map(|p| p.p1)),
            median: stats
                .as_ref()
                .and_then(|s| s.percentiles.as_ref().map(|p| p.p50)),
            p99: stats
                .as_ref()
                .and_then(|s| s.percentiles.as_ref().map(|p| p.p99)),
            zero_fraction: stats
                .as_ref()
                .filter(|s| s.count > 0)
                .map(|s| s.zero_fraction),
            saturated_fraction: stats.as_ref().and_then(|s| s.saturated_fraction),
        }
    };
    let mut rows = Vec::new();
    let mut wells_without_data = Vec::new();
    for (wi, w) in wanted.iter().enumerate() {
        let channels: Vec<u32> = {
            let mut cs: Vec<u32> = w
                .images
                .iter()
                .filter_map(|i| by_index.get(i))
                .flat_map(|im| 0..im.size_c)
                .filter(|c| sel.c.is_empty() || sel.c.contains(c))
                .collect();
            cs.sort_unstable();
            cs.dedup();
            cs
        };
        if !per_well.keys().any(|k| k.0 == wi) {
            wells_without_data.push(w.well.clone());
        }
        if want_fields {
            for &ii in &w.images {
                for &c in &channels {
                    let miss = missing.get(&(wi, ii, c)).copied().unwrap_or(0);
                    rows.push(make_row(w, Some(ii), c, per_field.get(&(wi, ii, c)), miss));
                }
            }
        } else {
            for &c in &channels {
                let miss: u64 = missing
                    .iter()
                    .filter(|((mw, _, mc), _)| *mw == wi && *mc == c)
                    .map(|(_, n)| n)
                    .sum();
                rows.push(make_row(w, None, c, per_well.get(&(wi, c)), miss));
            }
        }
    }
    let mut notes = Vec::new();
    if absent_total > 0 {
        notes.push(format!(
            "{absent_total} selected planes were never acquired or recorded by the instrument (images[].extra.absent_planes) and are left out"
        ));
    }
    let missing_total: u64 = missing.values().sum();
    if missing_total > 0 {
        notes.push(format!(
            "{missing_total} selected planes have no file on disk (a partial copy) and are skipped; `openreadout check` lists them"
        ));
    }
    Ok(WellStatsOutput {
        path: info.path.clone(),
        format: info.format.id.clone(),
        plate: layout.id.clone(),
        level: req.level,
        select: req.select.clone(),
        per_field: req.per_field,
        columns: WELL_STATS_COLUMNS
            .iter()
            .filter(|c| req.per_field || !matches!(**c, "image" | "field"))
            .map(|c| (*c).to_string())
            .collect(),
        rows,
        wells_without_data,
        notes,
    })
}

/// The rows as CSV (header line first; empty cells for absent values).
pub fn well_stats_csv(out: &WellStatsOutput) -> String {
    let mut s = out.columns.join(",");
    s.push('\n');
    let num = |v: Option<f64>| v.map(|x| format!("{x}")).unwrap_or_default();
    let text = |v: &str| {
        if v.contains([',', '"', '\n']) {
            format!("\"{}\"", v.replace('"', "\"\""))
        } else {
            v.to_string()
        }
    };
    for r in &out.rows {
        let mut cells: Vec<String> = Vec::new();
        for c in &out.columns {
            cells.push(match c.as_str() {
                "plate" => text(r.plate.as_deref().unwrap_or("")),
                "well" => text(&r.well),
                "row" => text(&r.row),
                "column" => r.column.to_string(),
                "image" => r.image.map(|v| v.to_string()).unwrap_or_default(),
                "field" => r.field.map(|v| v.to_string()).unwrap_or_default(),
                "c" => r.c.to_string(),
                "channel" => text(r.channel.as_deref().unwrap_or("")),
                "fields" => r.fields.to_string(),
                "planes" => r.planes.to_string(),
                "planes_missing" => r.planes_missing.to_string(),
                "count" => r.count.to_string(),
                "mean" => num(r.mean),
                "std" => num(r.std),
                "min" => num(r.min),
                "max" => num(r.max),
                "p1" => num(r.p1),
                "median" => num(r.median),
                "p99" => num(r.p99),
                "zero_fraction" => num(r.zero_fraction),
                "saturated_fraction" => num(r.saturated_fraction),
                _ => String::new(),
            });
        }
        s.push_str(&cells.join(","));
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_and_well_names_round_trip() {
        assert_eq!(row_name(0), "A");
        assert_eq!(row_name(25), "Z");
        assert_eq!(row_name(26), "AA");
        assert_eq!(row_name(31), "AF");
        assert_eq!(well_name(2, 4), "C05");
        assert_eq!(well_name(31, 47), "AF48");
        for (r, c) in [(0, 0), (2, 4), (15, 23), (26, 0), (31, 47)] {
            assert_eq!(parse_well(&well_name(r, c)), Some((r, c)));
        }
        assert_eq!(parse_well("c5"), Some((2, 4)));
        assert_eq!(parse_well("C/5"), Some((2, 4)));
        assert_eq!(parse_well("C0"), None);
        assert_eq!(parse_well("5"), None);
        assert_eq!(parse_well("ABC1"), None);
        assert_eq!(parse_well("A1x"), None);
    }

    #[test]
    fn standard_plates() {
        assert_eq!(standard_dimensions(3, 7), (6, 8));
        assert_eq!(standard_dimensions(7, 7), (8, 12));
        assert_eq!(standard_dimensions(9, 2), (16, 24));
        assert_eq!(standard_dimensions(17, 1), (32, 48));
        assert_eq!(standard_dimensions(40, 1), (40, 1));
    }

    #[test]
    fn plane_lists_are_read_from_extra() {
        let mut im = ImageInfo::new(0, 2, 2, PixelType::Uint8);
        im.size_c = 2;
        let mut im = im.finish();
        im.extra.insert(
            "absent_planes".into(),
            serde_json::json!([[1, 0, 0], [7, "x", 0]]),
        );
        im.extra
            .insert("planes_missing".into(), serde_json::json!(2));
        assert_eq!(absent_planes(&im), HashSet::from([(1, 0, 0)]));
        assert!(missing_planes(&im).1);
    }
}
