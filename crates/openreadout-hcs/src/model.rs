//! The plate model the three readers fill: channels, fields of view (one image each) and, per
//! field, one slot per (c, z, t) plane saying where its pixels are. Vocabulary:
//! `docs/formats/hcs.md` (shared) and the per-format notes.
//!
//! Readers produce [`RawPlane`] records in any order; [`PlateBuilder`] turns them into the
//! sorted model: wells row by row, fields by their number, Z and T by their recorded index.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use openreadout_core::model::{ChannelInfo, InstrumentInfo, ObjectiveInfo};
use openreadout_core::{Error, PixelType, Result};
use serde_json::Value;

/// Largest number of planes one plate may declare (a 1536-well plate with 100 fields and 10
/// channels is 1.5 million); a hostile index cannot make us allocate more.
pub const MAX_PLATE_PLANES: u64 = 20_000_000;
/// Largest plate geometry accepted (1536 wells are 32 x 48).
pub const MAX_PLATE_ROWS: u32 = 256;
/// See [`MAX_PLATE_ROWS`].
pub const MAX_PLATE_COLUMNS: u32 = 256;

/// Where one plane's pixels are.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaneSlot {
    /// Not part of the acquisition: no record exists (a channel acquired at fewer Z planes or
    /// time points than the others). Reads as a blank plane.
    NotAcquired,
    /// The index lists the plane but records no image for it (an empty file name). Reads as a
    /// blank plane.
    NotRecorded,
    /// Expected by the index, but its file is not on disk. `name` is the file name when the
    /// index records one.
    Missing {
        /// File name relative to the plate folder, when known.
        name: Option<String>,
    },
    /// On disk.
    File(PlaneFile),
}

impl PlaneSlot {
    /// The plane's state as a word for JSON (`present`, `missing`, `not_recorded`,
    /// `not_acquired`).
    pub fn state(&self) -> &'static str {
        match self {
            PlaneSlot::NotAcquired => "not_acquired",
            PlaneSlot::NotRecorded => "not_recorded",
            PlaneSlot::Missing { .. } => "missing",
            PlaneSlot::File(_) => "present",
        }
    }
    /// The file name, when one is known.
    pub fn file_name(&self) -> Option<&str> {
        match self {
            PlaneSlot::File(f) => Some(&f.name),
            PlaneSlot::Missing { name } => name.as_deref(),
            _ => None,
        }
    }
}

/// A plane stored in a TIFF file of the plate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaneFile {
    /// Path relative to the plate folder (`/`-separated).
    pub name: String,
    /// Zero-based page of the TIFF (0 for the single-page files of every corpus plate).
    pub page: u32,
    /// Acquisition time of the plane (ISO 8601), when the index records one.
    pub acquired_at: Option<String>,
    /// Stage position of the plane in µm (x, y, z), when the index records one.
    pub position_um: [Option<f64>; 3],
    /// Exposure time in ms, when the index records one per plane.
    pub exposure_ms: Option<f64>,
}

/// One plane record as a reader parsed it, before sorting.
#[derive(Debug, Clone)]
pub struct RawPlane {
    /// Zero-based plate row.
    pub row: u32,
    /// Zero-based plate column.
    pub column: u32,
    /// Field (site) number as the vendor counts it.
    pub field: u32,
    /// Index into the channel list.
    pub channel: usize,
    /// Z index as recorded (any base; sorted).
    pub z: u32,
    /// Time index as recorded (any base; sorted).
    pub t: u32,
    /// Where the pixels are.
    pub slot: PlaneSlot,
    /// Stage position of the field in µm (x, y), when recorded (offset from the well centre).
    pub field_position_um: [Option<f64>; 2],
}

/// One field of view of one well: one image of the dataset.
#[derive(Debug, Clone)]
pub struct HcsField {
    /// Zero-based plate row.
    pub row: u32,
    /// Zero-based plate column.
    pub column: u32,
    /// Field number as the vendor counts it.
    pub field: u32,
    /// Zero-based position of this field among the well's fields.
    pub field_index: u32,
    /// Stage position of the field in µm (x, y), when recorded.
    pub position_um: [Option<f64>; 2],
    /// Earliest acquisition time of the field's planes.
    pub acquired_at: Option<String>,
    /// One slot per plane, index `c + size_c * (z + size_z * t)`.
    pub planes: Vec<PlaneSlot>,
}

/// A parsed plate.
#[derive(Debug, Clone, Default)]
pub struct HcsPlate {
    /// Format id of the reader that parsed it.
    pub format_id: &'static str,
    /// The index file (or folder) that was opened.
    pub source: PathBuf,
    /// The folder the plane file names are relative to.
    pub root: PathBuf,
    /// Metadata files read besides the index (`.mrf`, `.mes`, `.wpi`, ...), existing ones only.
    pub sidecars: Vec<PathBuf>,
    /// Bytes of the index and the sidecars read.
    pub index_bytes: u64,
    /// Version of the index as recorded (`HarmonyV5`, `HTSInfoFile 1.0`, ...).
    pub format_version: Option<String>,
    /// Plate id or barcode.
    pub id: Option<String>,
    /// Plate name when recorded besides the id.
    pub name: Option<String>,
    /// Plate type (product) as recorded.
    pub plate_type: Option<String>,
    /// Plate rows.
    pub rows: u32,
    /// Plate columns.
    pub columns: u32,
    /// Wells the index says were selected for imaging (zero-based row, column).
    pub declared_wells: Vec<(u32, u32)>,
    /// Channels, in `c` order.
    pub channels: Vec<ChannelInfo>,
    /// Per-channel facts with no normalized field (`extra` of each channel entry).
    pub channel_extra: Vec<BTreeMap<String, Value>>,
    /// Z planes per field.
    pub size_z: u32,
    /// Time points per field.
    pub size_t: u32,
    /// Plane width in pixels (0 when unknown).
    pub size_x: u32,
    /// Plane height in pixels (0 when unknown).
    pub size_y: u32,
    /// Sample type, from a plane file's header or the index.
    pub pixel_type: Option<PixelType>,
    /// Where `pixel_type` and the plane size came from (`index`, `tiff:<file>`).
    pub pixel_source: Option<String>,
    /// Pixel width and height in µm.
    pub pixel_size_um: [Option<f64>; 2],
    /// Z step in µm.
    pub z_step_um: Option<f64>,
    /// Interval between time points in seconds.
    pub time_increment_s: Option<f64>,
    /// Objective.
    pub objective: Option<ObjectiveInfo>,
    /// Instrument and software.
    pub instrument: InstrumentInfo,
    /// Operator or user.
    pub operator: Option<String>,
    /// Measurement start (ISO 8601).
    pub started_at: Option<String>,
    /// Measurement end (ISO 8601).
    pub ended_at: Option<String>,
    /// Name of the acquisition protocol or setting file.
    pub method: Option<String>,
    /// Free-text description of the plate or measurement.
    pub description: Option<String>,
    /// Fields of view, wells row by row then field number: one image each.
    pub fields: Vec<HcsField>,
    /// Notes for `info`.
    pub notes: Vec<String>,
    /// Plate-level facts for `info` → `plate.extra`.
    pub plate_extra: BTreeMap<String, Value>,
    /// The vendor metadata for `info --view full` (names as the files spell them).
    pub vendor: Value,
    /// Plane-looking files in the plate folder that the index does not name.
    pub unindexed_files: Vec<String>,
}

impl HcsPlate {
    /// Number of (c, z, t) slots per field.
    pub fn planes_per_field(&self) -> usize {
        self.channels.len() * self.size_z as usize * self.size_t as usize
    }

    /// Slot index of (c, z, t).
    pub fn slot(&self, c: u32, z: u32, t: u32) -> usize {
        let (sc, sz) = (self.channels.len(), self.size_z as usize);
        c as usize + sc * (z as usize + sz * t as usize)
    }

    /// (c, z, t) of a slot index.
    pub fn plane_of(&self, slot: usize) -> (u32, u32, u32) {
        let sc = self.channels.len().max(1);
        let sz = self.size_z.max(1) as usize;
        (
            (slot % sc) as u32,
            ((slot / sc) % sz) as u32,
            (slot / (sc * sz)) as u32,
        )
    }

    /// Absolute path of a plane file.
    pub fn path_of(&self, name: &str) -> PathBuf {
        let mut p = self.root.clone();
        for part in name
            .split(['/', '\\'])
            .filter(|s| !s.is_empty() && *s != "..")
        {
            p.push(part);
        }
        p
    }
}

/// Collects [`RawPlane`]s and produces an [`HcsPlate`].
#[derive(Debug)]
pub struct PlateBuilder {
    format: &'static str,
    planes: Vec<RawPlane>,
}

impl PlateBuilder {
    /// An empty builder for format `format`.
    pub fn new(format: &'static str) -> Self {
        PlateBuilder {
            format,
            planes: Vec::new(),
        }
    }

    /// Add one plane record.
    pub fn push(&mut self, p: RawPlane) -> Result<()> {
        if self.planes.len() as u64 >= MAX_PLATE_PLANES {
            return Err(Error::unsupported(
                self.format,
                format!("an index with more than {MAX_PLATE_PLANES} planes"),
                "Plates this large are not supported; split the index or report the file.",
            ));
        }
        self.planes.push(p);
        Ok(())
    }

    /// Number of records collected.
    pub fn len(&self) -> usize {
        self.planes.len()
    }

    /// True when no record was collected.
    pub fn is_empty(&self) -> bool {
        self.planes.is_empty()
    }

    /// Sort the records into fields and slots. `plate.channels` must already be filled; the
    /// fields, `size_z` and `size_t` are set here. Duplicate records for one slot keep the
    /// first one that has a file and are counted in a note.
    pub fn finish(self, plate: &mut HcsPlate) -> Result<()> {
        let nc = plate.channels.len();
        if nc == 0 {
            return Err(Error::corrupt(
                self.format,
                "the index lists no channels (no plane records)",
            ));
        }
        let zs: BTreeSet<u32> = self.planes.iter().map(|p| p.z).collect();
        let ts: BTreeSet<u32> = self.planes.iter().map(|p| p.t).collect();
        let z_of: HashMap<u32, u32> = zs.iter().enumerate().map(|(i, &z)| (z, i as u32)).collect();
        let t_of: HashMap<u32, u32> = ts.iter().enumerate().map(|(i, &t)| (t, i as u32)).collect();
        plate.size_z = (zs.len() as u32).max(1);
        plate.size_t = (ts.len() as u32).max(1);
        let per_field = nc
            .checked_mul(plate.size_z as usize)
            .and_then(|v| v.checked_mul(plate.size_t as usize))
            .filter(|&v| v as u64 <= MAX_PLATE_PLANES)
            .ok_or_else(|| Error::corrupt(self.format, "too many planes per field"))?;
        let mut keys: BTreeMap<(u32, u32, u32), usize> = BTreeMap::new();
        for p in &self.planes {
            let n = keys.len();
            keys.entry((p.row, p.column, p.field)).or_insert(n);
        }
        let total = (keys.len() as u64).saturating_mul(per_field as u64);
        if total > MAX_PLATE_PLANES {
            return Err(Error::unsupported(
                self.format,
                format!("{} fields x {per_field} planes", keys.len()),
                "Plates this large are not supported; report the file.",
            ));
        }
        let mut fields: Vec<HcsField> = Vec::with_capacity(keys.len());
        let mut order: HashMap<(u32, u32, u32), usize> = HashMap::with_capacity(keys.len());
        for (i, &(row, column, field)) in keys.keys().enumerate() {
            order.insert((row, column, field), i);
            fields.push(HcsField {
                row,
                column,
                field,
                field_index: 0,
                position_um: [None, None],
                acquired_at: None,
                planes: vec![PlaneSlot::NotAcquired; per_field],
            });
        }
        let mut duplicates = 0u64;
        for p in self.planes {
            let Some(&fi) = order.get(&(p.row, p.column, p.field)) else {
                continue;
            };
            if p.channel >= nc {
                continue;
            }
            let (z, t) = (z_of[&p.z], t_of[&p.t]);
            let slot = p.channel + nc * (z as usize + plate.size_z as usize * t as usize);
            let f = &mut fields[fi];
            if f.position_um[0].is_none() {
                f.position_um = p.field_position_um;
            }
            if let PlaneSlot::File(pf) = &p.slot
                && let Some(ts) = &pf.acquired_at
                && f.acquired_at.as_ref().is_none_or(|cur| earlier(ts, cur))
            {
                f.acquired_at = Some(ts.clone());
            }
            let cur = &mut f.planes[slot];
            match cur {
                PlaneSlot::NotAcquired => *cur = p.slot,
                PlaneSlot::File(_) => duplicates += 1,
                _ => {
                    duplicates += 1;
                    if matches!(p.slot, PlaneSlot::File(_)) {
                        *cur = p.slot;
                    }
                }
            }
        }
        // field_index: position among the well's fields
        let mut last = (u32::MAX, u32::MAX);
        let mut k = 0u32;
        for f in &mut fields {
            if (f.row, f.column) != last {
                last = (f.row, f.column);
                k = 0;
            }
            f.field_index = k;
            k += 1;
        }
        if duplicates > 0 {
            plate.notes.push(format!(
                "{duplicates} plane records repeat a (well, field, channel, z, t) already listed; the first record with a file is used"
            ));
        }
        plate.fields = fields;
        Ok(())
    }
}

/// Is ISO timestamp `a` earlier than `b`? Compared as instants when both parse, else as text.
pub fn earlier(a: &str, b: &str) -> bool {
    match (
        openreadout_core::time::iso8601_to_unix(a),
        openreadout_core::time::iso8601_to_unix(b),
    ) {
        (Some(x), Some(y)) => x < y,
        _ => a < b,
    }
}

/// Mark every `File` slot whose file is not in `present` (names relative to the root,
/// compared case-sensitively, `/`-separated) as `Missing`. Returns the number marked.
pub fn mark_missing<S: std::hash::BuildHasher>(
    plate: &mut HcsPlate,
    present: &HashSet<String, S>,
) -> u64 {
    let mut n = 0;
    for f in &mut plate.fields {
        for s in &mut f.planes {
            if let PlaneSlot::File(pf) = s
                && !present.contains(&pf.name)
            {
                *s = PlaneSlot::Missing {
                    name: Some(pf.name.clone()),
                };
                n += 1;
            }
        }
    }
    n
}

/// Names of the regular files directly in `dir` (one directory listing), or an empty set
/// when the directory cannot be listed.
pub fn list_names(fs: &openreadout_core::Fs, dir: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Ok(rd) = fs.read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_file()) {
                out.insert(e.file_name().to_string_lossy().into_owned());
            }
        }
    }
    out
}

/// Parse a number that may be written with a decimal comma.
pub fn number(s: &str) -> Option<f64> {
    let t = s.trim();
    t.parse::<f64>()
        .ok()
        .or_else(|| t.replace(',', ".").parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

/// A value converted to µm from `unit` (`m`, `mm`, `um`, `µm`, `nm`); `None` for unknown units.
pub fn to_micrometres(v: f64, unit: &str) -> Option<f64> {
    let f = match unit.trim() {
        "m" => 1e6,
        "mm" => 1e3,
        "um" | "µm" | "μm" | "micron" | "microns" => 1.0,
        "nm" => 1e-3,
        _ => return None,
    };
    Some(v * f)
}

/// Does `name` end in one of `exts` (without the dot, any letter case)?
pub(crate) fn has_extension(name: &str, exts: &[&str]) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(name: &str) -> ChannelInfo {
        ChannelInfo {
            name: Some(name.into()),
            ..ChannelInfo::default()
        }
    }

    fn file(name: &str, t: &str) -> PlaneSlot {
        PlaneSlot::File(PlaneFile {
            name: name.into(),
            acquired_at: Some(t.into()),
            ..PlaneFile::default()
        })
    }

    #[test]
    fn builder_sorts_wells_fields_and_planes() {
        let mut plate = HcsPlate {
            channels: vec![ch("a"), ch("b")],
            ..HcsPlate::default()
        };
        let mut b = PlateBuilder::new("test");
        // well B01 field 2, then A02 field 1 (two channels, two z)
        for (row, col, field, c, z, name) in [
            (1, 0, 2, 0, 5, "b1"),
            (0, 1, 1, 1, 6, "a2c1z6"),
            (0, 1, 1, 0, 5, "a2c0z5"),
            (0, 1, 1, 0, 6, "a2c0z6"),
        ] {
            b.push(RawPlane {
                row,
                column: col,
                field,
                channel: c,
                z,
                t: 0,
                slot: file(name, "2020-01-01T00:00:00Z"),
                field_position_um: [Some(1.0), None],
            })
            .unwrap();
        }
        b.finish(&mut plate).unwrap();
        assert_eq!((plate.size_z, plate.size_t), (2, 1));
        assert_eq!(plate.fields.len(), 2);
        let f0 = &plate.fields[0];
        assert_eq!((f0.row, f0.column, f0.field, f0.field_index), (0, 1, 1, 0));
        assert_eq!(f0.planes[plate.slot(0, 0, 0)].file_name(), Some("a2c0z5"));
        assert_eq!(f0.planes[plate.slot(1, 1, 0)].file_name(), Some("a2c1z6"));
        assert_eq!(f0.planes[plate.slot(1, 0, 0)], PlaneSlot::NotAcquired);
        assert_eq!(plate.plane_of(plate.slot(1, 1, 0)), (1, 1, 0));
        let present: HashSet<String> = ["a2c0z5".to_string()].into();
        assert_eq!(mark_missing(&mut plate, &present), 3);
    }

    #[test]
    fn builder_rejects_empty_channel_list() {
        let mut plate = HcsPlate::default();
        assert!(PlateBuilder::new("test").finish(&mut plate).is_err());
    }

    #[test]
    fn units_and_numbers() {
        assert_eq!(to_micrometres(1.5e-6, "m"), Some(1.5));
        assert_eq!(to_micrometres(2.0, "furlong"), None);
        assert_eq!(number(" 0,5 "), Some(0.5));
        assert_eq!(number("NaN"), None);
        assert!(earlier("2020-01-01T00:00:00+01:00", "2020-01-01T00:30:00Z"));
    }
}
