//! `Dataset` over a parsed plate: one image per field of view, planes read lazily from the
//! plate's TIFF files, `check` against the index, the plate layout for `info` → `plate`.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Method, Origin, Sample,
};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, ImageInfo, LsEntry, PhysicalSize,
};
use openreadout_core::plate::{PlateSummary, PlateWell, row_name, well_name};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::{Error, Fs, PixelType, Plane, ProvenanceMap, Result, Source};
use serde_json::{Value, json};

use crate::model::{HcsPlate, PlaneSlot, list_names, mark_missing};
use crate::planes::{blank_plane, check_file, page_shape, read_file_plane};

/// Plates kept parsed for further opens of the same index (parallel reads open one handle per
/// worker thread; the MCP server opens the same plate for every call).
const CACHE_PLATES: usize = 4;

type CacheKey = (
    String,
    PathBuf,
    u64,
    Option<std::time::SystemTime>,
    Option<std::time::SystemTime>,
);

static CACHE: Mutex<Vec<(CacheKey, Arc<HcsPlate>)>> = Mutex::new(Vec::new());

/// A parsed plate from the cache, or `parse()`'s result (then cached). The key covers the
/// index's size and modification time and the plate folder's modification time, so a plate
/// that changes on disk is parsed again. Sources without modification times are not cached.
pub fn cached(
    format: &str,
    fs: &Fs,
    index: &Path,
    root: &Path,
    parse: impl FnOnce() -> Result<HcsPlate>,
) -> Result<Arc<HcsPlate>> {
    let meta = fs.metadata(index).ok();
    let key: Option<CacheKey> = meta.as_ref().and_then(|m| {
        let mt = m.modified().ok()?;
        let dt = fs.metadata(root).ok().and_then(|d| d.modified().ok());
        Some((
            format.to_string(),
            index.to_path_buf(),
            m.len(),
            Some(mt),
            dt,
        ))
    });
    if let Some(k) = &key {
        let c = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, p)) = c.iter().find(|(ck, _)| ck == k) {
            return Ok(Arc::clone(p));
        }
    }
    let p = Arc::new(parse()?);
    if let Some(k) = key {
        let mut c = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        c.retain(|(ck, _)| ck != &k);
        c.push((k, Arc::clone(&p)));
        if c.len() > CACHE_PLATES {
            c.remove(0);
        }
    }
    Ok(p)
}

/// Finish a freshly parsed plate: find which plane files exist (one listing of the plate
/// folder, plus one per sub-folder the index names), and take the sample type from one plane
/// file's header when the index does not say it.
pub fn finish(mut plate: HcsPlate, fs: &Fs, plane_name_test: fn(&str) -> bool) -> HcsPlate {
    // directories the plane names live in
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    for f in &plate.fields {
        for s in &f.planes {
            if let PlaneSlot::File(pf) = s {
                dirs.insert(
                    pf.name
                        .rsplit_once('/')
                        .map_or(String::new(), |(d, _)| d.to_string()),
                );
            }
        }
    }
    let mut present: HashSet<String> = HashSet::new();
    let mut listed_root = HashSet::new();
    for d in &dirs {
        let names = list_names(fs, &plate.path_of(d));
        for n in names {
            if d.is_empty() {
                listed_root.insert(n.clone());
                present.insert(n);
            } else {
                present.insert(format!("{d}/{n}"));
            }
        }
    }
    mark_missing(&mut plate, &present);
    // plane-looking files in the folder that the index does not name
    if plate.unindexed_files.is_empty() && !dirs.is_empty() {
        let named: HashSet<&str> = plate
            .fields
            .iter()
            .flat_map(|f| f.planes.iter())
            .filter_map(PlaneSlot::file_name)
            .collect();
        let mut extra: Vec<String> = listed_root
            .iter()
            .filter(|n| plane_name_test(n) && !named.contains(n.as_str()))
            .cloned()
            .collect();
        extra.sort();
        plate.unindexed_files = extra;
    }
    // sample type and geometry from the first present plane file
    if plate.pixel_type.is_none()
        || plate.size_x == 0
        || plate
            .pixel_source
            .as_deref()
            .is_some_and(|s| !s.starts_with("plane"))
    {
        let first = plate
            .fields
            .iter()
            .flat_map(|f| f.planes.iter())
            .find_map(|s| match s {
                PlaneSlot::File(pf) => Some(pf.clone()),
                _ => None,
            });
        if let Some(pf) = first
            && let Ok(shape) = page_shape(fs, &plate.path_of(&pf.name), pf.page)
        {
            if plate.size_x != 0 && (plate.size_x != shape.width || plate.size_y != shape.height) {
                plate.notes.push(format!(
                    "the index says {}x{} pixels but plane file {} is {}x{}; the file's size is used",
                    plate.size_x, plate.size_y, pf.name, shape.width, shape.height
                ));
            }
            plate.size_x = shape.width;
            plate.size_y = shape.height;
            plate.pixel_type = Some(shape.pixel_type);
            plate.pixel_source = Some(format!("plane file header ({})", pf.name));
        }
    }
    if plate.pixel_type.is_none() {
        plate.pixel_type = Some(PixelType::Uint16);
        plate.pixel_source = Some("assumed".into());
        plate.notes.push(
            "no plane file of the plate is on disk: the sample type is assumed to be uint16".into(),
        );
    }
    plate
}

/// An open plate.
pub struct HcsDataset {
    plate: Arc<HcsPlate>,
    fs: Fs,
    descriptor: FormatDescriptor,
}

impl std::fmt::Debug for HcsDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HcsDataset")
            .field("format", &self.plate.format_id)
            .field("source", &self.plate.source)
            .field("fields", &self.plate.fields.len())
            .finish_non_exhaustive()
    }
}

/// Counts of plane states over the plate.
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    present: u64,
    missing: u64,
    not_recorded: u64,
    not_acquired: u64,
}

impl HcsDataset {
    /// Wrap a parsed plate.
    pub fn new(plate: Arc<HcsPlate>, fs: Fs, descriptor: FormatDescriptor) -> Self {
        HcsDataset {
            plate,
            fs,
            descriptor,
        }
    }

    /// The parsed plate.
    pub fn plate_model(&self) -> &HcsPlate {
        &self.plate
    }

    fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for f in &self.plate.fields {
            for s in &f.planes {
                match s {
                    PlaneSlot::File(_) => c.present += 1,
                    PlaneSlot::Missing { .. } => c.missing += 1,
                    PlaneSlot::NotRecorded => c.not_recorded += 1,
                    PlaneSlot::NotAcquired => c.not_acquired += 1,
                }
            }
        }
        c
    }

    #[allow(clippy::many_single_char_names)] // c, z, t: plane coordinates
    fn image_info(&self, i: usize) -> ImageInfo {
        let p = &*self.plate;
        let f = &p.fields[i];
        let pt = p.pixel_type.unwrap_or(PixelType::Uint16);
        let mut im = ImageInfo::new(i as u32, p.size_x, p.size_y, pt);
        let well = well_name(f.row, f.column);
        im.name = Some(format!("{well} field {}", f.field));
        im.size_c = p.channels.len() as u32;
        im.size_z = p.size_z;
        im.size_t = p.size_t;
        im.channels = p.channels.clone();
        im.physical_size = PhysicalSize::micrometres(
            p.pixel_size_um[0],
            p.pixel_size_um[1],
            p.z_step_um.filter(|_| p.size_z > 1),
        );
        im.time_increment_s = p.time_increment_s.filter(|_| p.size_t > 1);
        im.objective.clone_from(&p.objective);
        im.instrument = Some(p.instrument.clone());
        im.acquired_at = f.acquired_at.clone().or_else(|| p.started_at.clone());
        let mut im = im.finish();
        let ex = &mut im.extra;
        if let Some(id) = &p.id {
            ex.insert("plate".into(), json!(id));
        }
        ex.insert("well".into(), json!(well));
        ex.insert("row".into(), json!(row_name(f.row)));
        ex.insert("column".into(), json!(f.column + 1));
        ex.insert("row_index".into(), json!(f.row));
        ex.insert("column_index".into(), json!(f.column));
        ex.insert("field".into(), json!(f.field));
        ex.insert("field_index".into(), json!(f.field_index));
        if let Some(x) = f.position_um[0] {
            ex.insert("position_x_um".into(), json!(x));
        }
        if let Some(y) = f.position_um[1] {
            ex.insert("position_y_um".into(), json!(y));
        }
        let mut missing = Vec::new();
        let mut absent = Vec::new();
        let mut not_recorded = 0u64;
        for (s, slot) in f.planes.iter().enumerate() {
            let (c, z, t) = p.plane_of(s);
            match slot {
                PlaneSlot::Missing { .. } => missing.push(json!([c, z, t])),
                PlaneSlot::NotRecorded => {
                    not_recorded += 1;
                    absent.push(json!([c, z, t]));
                }
                PlaneSlot::NotAcquired => absent.push(json!([c, z, t])),
                PlaneSlot::File(_) => {}
            }
        }
        if !missing.is_empty() {
            ex.insert("planes_missing".into(), json!(missing.len()));
            if (missing.len() as u64) < im.plane_count {
                ex.insert("missing_planes".into(), Value::Array(missing));
            }
        }
        if !absent.is_empty() {
            ex.insert("absent_planes".into(), Value::Array(absent));
        }
        if not_recorded > 0 {
            ex.insert("planes_not_recorded".into(), json!(not_recorded));
        }
        im
    }

    fn slot(&self, image: u32, index: PlaneIndex) -> Result<&PlaneSlot> {
        let p = &*self.plate;
        let f = p.fields.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image {image} does not exist (the plate has {} fields of view)",
                p.fields.len()
            ))
        })?;
        if index.c >= p.channels.len() as u32 || index.z >= p.size_z || index.t >= p.size_t {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} is out of range (c < {}, z < {}, t < {})",
                index.c,
                index.z,
                index.t,
                p.channels.len(),
                p.size_z,
                p.size_t
            )));
        }
        Ok(&f.planes[p.slot(index.c, index.z, index.t)])
    }

    fn where_is(&self, image: u32, index: PlaneIndex) -> String {
        let f = &self.plate.fields[image as usize];
        format!(
            "well {} field {}, c={} z={} t={}",
            well_name(f.row, f.column),
            f.field,
            index.c,
            index.z,
            index.t
        )
    }
}

impl Dataset for HcsDataset {
    fn info(&self) -> Result<FileInfo> {
        let p = &*self.plate;
        let images: Vec<ImageInfo> = (0..p.fields.len()).map(|i| self.image_info(i)).collect();
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        let mut notes = p.notes.clone();
        let c = self.counts();
        let wells: BTreeSet<(u32, u32)> = p.fields.iter().map(|f| (f.row, f.column)).collect();
        notes.insert(
            0,
            format!(
                "plate {}: {} x {} wells ({}), {} wells imaged, {} fields of view (one image each), {} channel(s), {} plane(s) per field",
                p.id.as_deref().unwrap_or("(no id)"),
                p.rows,
                p.columns,
                p.plate_type.as_deref().unwrap_or("plate type not recorded"),
                wells.len(),
                p.fields.len(),
                p.channels.len(),
                p.planes_per_field()
            ),
        );
        if c.missing > 0 {
            notes.push(format!(
                "{} of {} plane files named by the index are not on disk (the copy is incomplete); reading them fails; `check` lists them",
                c.missing,
                c.missing + c.present
            ));
        }
        if c.not_recorded > 0 {
            notes.push(format!(
                "{} planes are listed by the index without an image (not recorded); they read as blank and are left out of statistics",
                c.not_recorded
            ));
        }
        if c.not_acquired > 0 {
            notes.push(format!(
                "{} planes were never acquired (channels imaged at fewer Z planes or time points than others: images[].extra.absent_planes); they read as blank and are left out of statistics",
                c.not_acquired
            ));
        }
        if !p.unindexed_files.is_empty() {
            notes.push(format!(
                "{} plane-like file(s) in the folder are not named by the index and are not read",
                p.unindexed_files.len()
            ));
        }
        let size_bytes = p.index_bytes;
        Ok(FileInfo {
            path: p.source.display().to_string(),
            size_bytes,
            format: self.descriptor.clone(),
            format_version: p.format_version.clone(),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(self.plate.vendor.clone())
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut m = ProvenanceMap::new();
        for (k, s) in [
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].pixel_type", Source::Spec),
            ("images[].size_c", Source::Inferred),
            ("images[].size_z", Source::Inferred),
            ("images[].size_t", Source::Inferred),
            ("images[].name", Source::Inferred),
            ("images[].channels", Source::Inferred),
            ("images[].physical_size", Source::Inferred),
            ("images[].objective", Source::Inferred),
            ("images[].instrument", Source::Inferred),
            ("images[].acquired_at", Source::Inferred),
            ("images[].extra.well", Source::Inferred),
            ("images[].extra.field", Source::Inferred),
            ("images[].extra.position_x_um", Source::Inferred),
            ("images[].extra.position_y_um", Source::Inferred),
            ("plate", Source::Inferred),
        ] {
            m.insert(k.into(), s);
        }
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let p = &*self.plate;
        let mut out = vec![LsEntry {
            kind: "index".into(),
            name: p.source.display().to_string(),
            offset: None,
            size: Some(p.index_bytes),
            image: None,
            details: json!({"format_version": p.format_version}),
        }];
        for s in &p.sidecars {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: s.display().to_string(),
                offset: None,
                size: self.fs.metadata(s).ok().map(|m| m.len()),
                image: None,
                details: Value::Null,
            });
        }
        let mut wells: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
        for (i, f) in p.fields.iter().enumerate() {
            wells.entry((f.row, f.column)).or_default().push(i as u32);
        }
        for ((r, c), imgs) in &wells {
            out.push(LsEntry {
                kind: "well".into(),
                name: well_name(*r, *c),
                offset: None,
                size: None,
                image: None,
                details: json!({"images": imgs}),
            });
        }
        for (i, f) in p.fields.iter().enumerate() {
            for (s, slot) in f.planes.iter().enumerate() {
                let (c, z, t) = p.plane_of(s);
                if matches!(slot, PlaneSlot::NotAcquired) {
                    continue;
                }
                out.push(LsEntry {
                    kind: "plane-file".into(),
                    name: slot.file_name().unwrap_or("").to_string(),
                    offset: None,
                    size: None,
                    image: Some(i as u32),
                    details: json!({"well": well_name(f.row, f.column), "field": f.field, "c": c, "z": z, "t": t, "state": slot.state()}),
                });
            }
        }
        for u in &p.unindexed_files {
            out.push(LsEntry {
                kind: "unindexed-file".into(),
                name: u.clone(),
                offset: None,
                size: None,
                image: None,
                details: Value::Null,
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let p = Arc::clone(&self.plate);
        let pt = p.pixel_type.unwrap_or(PixelType::Uint16);
        match self.slot(image, index)? {
            PlaneSlot::File(pf) => {
                let path = p.path_of(&pf.name);
                let plane = read_file_plane(&self.fs, &path, pf.page)?;
                if plane.width != p.size_x
                    || plane.height != p.size_y
                    || plane.pixel_type != pt
                    || plane.samples_per_pixel != 1
                {
                    return Err(Error::corrupt(
                        p.format_id,
                        format!(
                            "plane file {} ({}) is {}x{} {} x{}, the plate is {}x{} {}",
                            pf.name,
                            self.where_is(image, index),
                            plane.width,
                            plane.height,
                            plane.pixel_type.ome_name(),
                            plane.samples_per_pixel,
                            p.size_x,
                            p.size_y,
                            pt.ome_name()
                        ),
                    ));
                }
                Ok(plane)
            }
            PlaneSlot::NotAcquired | PlaneSlot::NotRecorded => {
                if p.size_x == 0 || p.size_y == 0 {
                    return Err(Error::unsupported(
                        p.format_id,
                        "a blank plane of a plate whose image size is unknown",
                        "No plane file of this plate is on disk and the index records no image size.",
                    ));
                }
                blank_plane(p.size_x, p.size_y, pt)
            }
            PlaneSlot::Missing { name } => {
                let path = name
                    .as_deref()
                    .map_or_else(|| p.root.clone(), |n| p.path_of(n));
                Err(Error::io(
                    path,
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "the plane file of {} {} is not on disk: this copy of the plate is incomplete (`openreadout check` lists every missing file; images[].extra.missing_planes says which planes of an image can be read)",
                            self.where_is(image, index),
                            name.as_deref().map_or_else(
                                || "(no file found for it)".to_string(),
                                |n| format!("({n})")
                            )
                        ),
                    ),
                ))
            }
        }
    }

    fn check(&mut self) -> Result<CheckReport> {
        self.check_headers()
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let p = Arc::clone(&self.plate);
        let mut r = CheckReport::new(p.source.display().to_string(), p.format_id);
        let c = self.counts();
        r.performed(format!(
            "the plate index parses: {} fields of view in {} wells, {} channel(s), {} Z plane(s), {} time point(s)",
            p.fields.len(),
            p.fields.iter().map(|f| (f.row, f.column)).collect::<BTreeSet<_>>().len(),
            p.channels.len(),
            p.size_z,
            p.size_t
        ));
        r.performed(format!(
            "every plane file the index names exists ({} named, one folder listing)",
            c.present + c.missing
        ));
        r.performed(
            "every plane file present has a readable TIFF header with the plate's image size and sample type, and its strips lie inside the file".to_string(),
        );
        for n in &p.notes {
            r.push(Finding::info("index_note", n.clone()));
        }
        // missing files, grouped by well
        if c.missing > 0 {
            let mut per_well: BTreeMap<(u32, u32), (u64, Vec<String>)> = BTreeMap::new();
            for f in &p.fields {
                for s in &f.planes {
                    if let PlaneSlot::Missing { name } = s {
                        let e = per_well.entry((f.row, f.column)).or_default();
                        e.0 += 1;
                        if e.1.len() < 3 {
                            e.1.push(name.clone().unwrap_or_else(|| format!("field {}", f.field)));
                        }
                    }
                }
            }
            let expected = c.missing + c.present;
            r.push(Finding::error(
                "missing_plane_files",
                format!(
                    "{} of {} plane files named by the index are not on disk, in {} of {} wells: this copy of the plate is incomplete",
                    c.missing,
                    expected,
                    per_well.len(),
                    p.fields.iter().map(|f| (f.row, f.column)).collect::<BTreeSet<_>>().len()
                ),
            ));
            for (i, ((row, col), (n, ex))) in per_well.iter().enumerate() {
                if i >= 24 {
                    r.push(Finding::error(
                        "missing_plane_files",
                        format!(
                            "... and {} more wells with missing files",
                            per_well.len() - 24
                        ),
                    ));
                    break;
                }
                r.push(Finding::error(
                    "missing_plane_files",
                    format!(
                        "well {}: {n} plane file(s) missing (e.g. {})",
                        well_name(*row, *col),
                        ex.join(", ")
                    ),
                ));
            }
        }
        if c.not_recorded > 0 {
            let mut per_well: BTreeMap<(u32, u32), BTreeSet<u32>> = BTreeMap::new();
            for f in &p.fields {
                if f.planes.iter().any(|s| matches!(s, PlaneSlot::NotRecorded)) {
                    per_well
                        .entry((f.row, f.column))
                        .or_default()
                        .insert(f.field);
                }
            }
            let list: Vec<String> = per_well
                .iter()
                .take(12)
                .map(|((r, c), fs)| format!("{} fields {:?}", well_name(*r, *c), fs))
                .collect();
            r.push(Finding::warning(
                "planes_not_recorded",
                format!(
                    "{} planes are listed by the index without an image file (the instrument recorded no image): {}",
                    c.not_recorded,
                    list.join("; ")
                ),
            ));
        }
        if c.not_acquired > 0 {
            r.push(Finding::info(
                "planes_not_acquired",
                format!(
                    "{} (channel, z, t) combinations were never acquired (a channel imaged at fewer Z planes or time points than the others); they read as blank",
                    c.not_acquired
                ),
            ));
        }
        if !p.unindexed_files.is_empty() {
            r.push(Finding::info(
                "unindexed_plane_files",
                format!(
                    "{} plane-like file(s) in the plate folder are not named by the index (e.g. {})",
                    p.unindexed_files.len(),
                    p.unindexed_files.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        // headers of the present files
        let want = p.pixel_type.map(|pt| (p.size_x, p.size_y, pt));
        let mut bad = 0u64;
        for f in &p.fields {
            for s in &f.planes {
                if let PlaneSlot::File(pf) = s {
                    let problems = check_file(&self.fs, &p.path_of(&pf.name), pf.page, want);
                    if !problems.is_empty() {
                        bad += 1;
                        if bad <= 50 {
                            let code = if problems.iter().any(|x| x.contains("truncated")) {
                                "plane_file_truncated"
                            } else if problems.iter().any(|x| x.contains("the plate index says")) {
                                "plane_file_mismatch"
                            } else {
                                "plane_file_unreadable"
                            };
                            r.push(Finding::error(
                                code,
                                format!(
                                    "{} (well {} field {}): {}",
                                    pf.name,
                                    well_name(f.row, f.column),
                                    f.field,
                                    problems.join("; ")
                                ),
                            ));
                        }
                    }
                }
            }
        }
        if bad > 50 {
            r.push(Finding::error(
                "plane_file_unreadable",
                format!("... {} more plane files have problems", bad - 50),
            ));
        }
        Ok(r)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let p = &*self.plate;
        let mut out: Vec<PathBuf> = p.sidecars.clone();
        for f in &p.fields {
            for s in &f.planes {
                if let PlaneSlot::File(pf) = s {
                    out.push(p.path_of(&pf.name));
                }
            }
        }
        if p.source.is_dir() {
            out.retain(|x| x != &p.source);
        }
        out
    }

    #[allow(clippy::many_single_char_names)] // c, z, t: plane coordinates
    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        let p = &*self.plate;
        let Some(f) = p.fields.get(image as usize) else {
            return Ok((0, Vec::new()));
        };
        let total = f
            .planes
            .iter()
            .filter(|s| !matches!(s, PlaneSlot::NotAcquired))
            .count() as u64;
        let mut out = Vec::new();
        for (s, slot) in f.planes.iter().enumerate() {
            if matches!(slot, PlaneSlot::NotAcquired) {
                continue;
            }
            if limit.is_some_and(|l| out.len() >= l) {
                break;
            }
            let (c, z, t) = p.plane_of(s);
            let mut rec = serde_json::Map::new();
            rec.insert("c".into(), json!(c));
            rec.insert("z".into(), json!(z));
            rec.insert("t".into(), json!(t));
            rec.insert("state".into(), json!(slot.state()));
            if let Some(n) = slot.file_name() {
                rec.insert("file".into(), json!(n));
            }
            if let PlaneSlot::File(pf) = slot {
                let (mut at, mut pos, mut exp) =
                    (pf.acquired_at.clone(), pf.position_um, pf.exposure_ms);
                if p.format_id == crate::imagexpress::IMAGEXPRESS_FORMAT_ID
                    && let Some((a, ps, e)) =
                        crate::imagexpress::plane_meta(&self.fs, &p.path_of(&pf.name))
                {
                    at = at.or(a);
                    pos = [pos[0].or(ps[0]), pos[1].or(ps[1]), pos[2].or(ps[2])];
                    exp = exp.or(e);
                }
                if let Some(a) = at {
                    rec.insert("acquired_at".into(), json!(a));
                }
                for (k, v) in ["stage_x_um", "stage_y_um", "stage_z_um"].iter().zip(pos) {
                    if let Some(v) = v {
                        rec.insert((*k).into(), json!(v));
                    }
                }
                if let Some(e) = exp {
                    rec.insert("exposure_ms".into(), json!(e));
                }
            }
            out.push(Value::Object(rec));
        }
        Ok((total, out))
    }

    fn experiment(&self) -> Option<Experiment> {
        let p = &*self.plate;
        let index_name = p.source.file_name().map_or_else(
            || p.source.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let from = |what: &str| format!("{index_name} {what}");
        let mut e = Experiment::default();
        let origin = |e: &mut Experiment, key: &str, what: &str| {
            e.provenance.insert(
                key.into(),
                Origin {
                    source: Source::Inferred,
                    from: from(what),
                },
            );
        };
        if let Some(id) = &p.id {
            e.sample = Some(Sample {
                id: Some(id.clone()),
                name: p.name.clone(),
                well: None,
                barcode: Some(id.clone()),
                sequence_position: None,
                source_field: Some("plate.id".into()),
            });
            origin(&mut e, "sample.id", "plate id / barcode");
            origin(&mut e, "sample.barcode", "plate id / barcode");
            if p.name.is_some() {
                origin(&mut e, "sample.name", "plate name");
            }
        }
        let serial = p
            .plate_extra
            .get("instrument_serial")
            .and_then(Value::as_str)
            .map(str::to_string);
        let inst = ExperimentInstrument {
            vendor: p.instrument.manufacturer.clone(),
            model: p.instrument.model.clone(),
            serial: serial.clone(),
            software: p.instrument.software.clone(),
            software_version: p.instrument.software_version.clone(),
            kind: None,
        };
        if inst != ExperimentInstrument::default() {
            for (k, v) in [
                ("instrument.vendor", &inst.vendor),
                ("instrument.model", &inst.model),
                ("instrument.serial", &inst.serial),
                ("instrument.software", &inst.software),
                ("instrument.software_version", &inst.software_version),
            ] {
                if v.is_some() {
                    origin(&mut e, k, "instrument");
                }
            }
            e.instrument = Some(inst);
        }
        if let Some(m) = &p.method {
            e.method = Some(Method {
                name: Some(m.clone()),
                ..Method::default()
            });
            origin(&mut e, "method.name", "measurement setting");
        }
        let duration = match (
            p.started_at
                .as_deref()
                .and_then(openreadout_core::time::iso8601_to_unix),
            p.ended_at
                .as_deref()
                .and_then(openreadout_core::time::iso8601_to_unix),
        ) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        };
        let acq = Acquisition {
            started_at: p.started_at.clone(),
            ended_at: p.ended_at.clone(),
            operator: p.operator.clone(),
            duration_s: duration,
            comment: p.description.clone(),
            saved_at: None,
        };
        if acq != Acquisition::default() {
            for (k, ok) in [
                ("acquisition.started_at", acq.started_at.is_some()),
                ("acquisition.ended_at", acq.ended_at.is_some()),
                ("acquisition.operator", acq.operator.is_some()),
                ("acquisition.duration_s", acq.duration_s.is_some()),
                ("acquisition.comment", acq.comment.is_some()),
            ] {
                if ok {
                    origin(&mut e, k, "measurement times and user");
                }
            }
            e.acquisition = Some(acq);
        }
        let wells: BTreeSet<(u32, u32)> = p.fields.iter().map(|f| (f.row, f.column)).collect();
        e.notes.push(format!(
            "a multi-well plate: sample.id is the plate id; each image is one field of view of one well (images[].extra.well), {} wells imaged",
            wells.len()
        ));
        Some(e)
    }

    fn plate(&self) -> Option<PlateSummary> {
        let p = &*self.plate;
        let mut wells: BTreeMap<(u32, u32), PlateWell> = BTreeMap::new();
        let mut expected = 0u64;
        let mut absent = 0u64;
        let mut missing = 0u64;
        for (i, f) in p.fields.iter().enumerate() {
            let w = wells.entry((f.row, f.column)).or_insert_with(|| PlateWell {
                well: well_name(f.row, f.column),
                row: row_name(f.row),
                column: f.column + 1,
                row_index: f.row,
                column_index: f.column,
                images: Vec::new(),
                planes_missing: 0,
            });
            w.images.push(i as u32);
            for s in &f.planes {
                expected += 1;
                match s {
                    PlaneSlot::Missing { .. } => {
                        missing += 1;
                        w.planes_missing += 1;
                    }
                    PlaneSlot::NotAcquired | PlaneSlot::NotRecorded => absent += 1,
                    PlaneSlot::File(_) => {}
                }
            }
        }
        let field_count = wells.values().map(|w| w.images.len()).max().unwrap_or(0) as u32;
        let mut extra = p.plate_extra.clone();
        let declared: BTreeSet<(u32, u32)> = p.declared_wells.iter().copied().collect();
        let not_imaged: Vec<String> = declared
            .iter()
            .filter(|k| !wells.contains_key(k))
            .map(|(r, c)| well_name(*r, *c))
            .collect();
        if !not_imaged.is_empty() {
            extra.insert("selected_wells_without_images".into(), json!(not_imaged));
        }
        if !p.unindexed_files.is_empty() {
            extra.insert("unindexed_files".into(), json!(p.unindexed_files.len()));
        }
        if p.channel_extra.iter().any(|c| !c.is_empty()) {
            let chans: Vec<Value> = p
                .channel_extra
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let mut o: serde_json::Map<String, Value> =
                        c.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    o.insert("c".into(), json!(i));
                    Value::Object(o)
                })
                .collect();
            extra.insert("channels".into(), Value::Array(chans));
        }
        let complete_wells = wells.values().filter(|w| w.planes_missing == 0).count();
        extra.insert("wells_complete".into(), json!(complete_wells));
        Some(PlateSummary {
            id: p.id.clone(),
            name: p.name.clone(),
            plate_type: p.plate_type.clone(),
            rows: p.rows,
            columns: p.columns,
            wells: wells.into_values().collect(),
            field_count,
            planes_expected: expected,
            planes_absent: absent,
            planes_missing: missing,
            complete: missing == 0,
            extra,
        })
    }
}
