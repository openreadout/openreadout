//! Imaris `.ims` (HDF5): `DataSet/ResolutionLevel N/TimePoint T/Channel C/Data` volumes,
//! `DataSetInfo` metadata. Layout derived from corpus files browsed with h5py; see
//! `docs/formats/ims.md` and `docs/provenance/ims.md`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::find;
use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, DetectConfidence, FileInfo, FormatDescriptor,
    ImageInfo, InstrumentInfo, LsEntry, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::region::{Region, ResolutionLevel, TileCache};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Finding, PixelType, Plane, Result};
use serde_json::{Value, json};

use crate::h5util::{
    PlaneDataset, PlaneSource, attrs_json, attrs_text, looks_like_hdf5, open_h5_in,
};
use crate::ims_scene::{SceneTable, read_scene_table, scene_tables};

/// Format id used on the command line and in JSON.
pub const IMS_FORMAT_ID: &str = "ims";

/// The Imaris reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImsReader;

impl FormatReader for ImsReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::IMS)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: IMS_FORMAT_ID.into(),
            name: "Imaris IMS (HDF5)".into(),
            vendor: "Oxford Instruments / Bitplane (Imaris)".into(),
            extensions: vec!["ims".into()],
            family: "microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::IMS.confidence,
            known_gaps: vec![
                "Imaris 3 (`.ims` TIFF-based files) is not this format; those files go to the TIFF reader".into(),
                "Scene8 objects: their statistics and numeric record datasets are tables; surface meshes (`SurfaceModel`, `BlockData`), text fields and the legacy `Scene` group are not decoded".into(),
                "Only the first data set is read when a file declares several (NumberOfDataSets > 1)".into(),
                "Chunks compressed with filters other than deflate, shuffle, fletcher32 and LZ4 exit 6".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !looks_like_hdf5(head) {
            return has_extension(path, &["ims"]).then_some(Detection {
                format_id: IMS_FORMAT_ID,
                confidence: DetectConfidence::ExtensionOnly,
                note: Some(
                    "Imaris extension, but no HDF5 signature (Imaris 3 files are TIFF-based)"
                        .into(),
                ),
            });
        }
        let marked = find(head, b"ImarisDataSet").is_some();
        if has_extension(path, &["ims"]) || marked {
            return Some(Detection {
                format_id: IMS_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some(if marked {
                    "HDF5 file with Imaris root attributes".into()
                } else {
                    "HDF5 file with the .ims extension".into()
                }),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ImsDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(ImsDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// One resolution level.
#[derive(Debug, Clone, PartialEq)]
pub struct ImsLevel {
    /// `ResolutionLevel N`.
    pub level: u32,
    /// Image extent (the stored datasets may be padded to whole chunks).
    pub size_x: u64,
    pub size_y: u64,
    pub size_z: u64,
}

/// An opened Imaris file.
pub struct ImsDataset {
    path: PathBuf,
    /// Where the file is read from.
    fs: Fs,
    file: hdf5_pure::File,
    /// Root `ImarisVersion` (the file-format version, e.g. `5.5.0`).
    pub version: Option<String>,
    pub levels: Vec<ImsLevel>,
    pub timepoints: u32,
    pub channels: u32,
    pub pixel_type: PixelType,
    info: ImageInfo,
    /// `DataSetInfo/<group>` attributes as text.
    dataset_info: BTreeMap<String, BTreeMap<String, String>>,
    root_attrs: serde_json::Map<String, Value>,
    other_groups: Vec<String>,
    planes: HashMap<String, PlaneDataset>,
    notes: Vec<String>,
    /// Decoded chunks kept between plane and region reads (a chunk spans several z planes).
    chunks: TileCache<u64, Vec<u8>>,
    /// Tables of the `Scene8` objects.
    scene: Vec<SceneTable>,
}

impl std::fmt::Debug for ImsDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImsDataset")
            .field("path", &self.path)
            .field("levels", &self.levels)
            .finish_non_exhaustive()
    }
}

fn corrupt(d: impl Into<String>) -> Error {
    Error::corrupt(IMS_FORMAT_ID, d.into())
}

/// `N` of `"<prefix> N"` group names, sorted.
fn numbered(names: &[String], prefix: &str) -> Vec<u32> {
    let mut v: Vec<u32> = names
        .iter()
        .filter_map(|n| n.strip_prefix(prefix)?.trim().parse().ok())
        .collect();
    v.sort_unstable();
    v
}

/// Leading number of a text such as `500 nm` or `561 nm nm`.
fn leading_number(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
        .collect();
    t.parse().ok().filter(|v: &f64| v.is_finite())
}

/// Imaris colour `r g b` (0–1 floats) → `#RRGGBB`.
pub fn ims_color(s: &str) -> Option<String> {
    let v: Vec<f64> = s
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    if v.len() < 3 || v.iter().take(3).any(|x| !(0.0..=1.0).contains(x)) {
        return None;
    }
    let b = |x: f64| (x * 255.0).round() as u8;
    Some(format!("#{:02X}{:02X}{:02X}", b(v[0]), b(v[1]), b(v[2])))
}

/// Imaris date text `YYYY-MM-DD HH:MM:SS[.mmm]` → ISO-8601 local time (no zone).
pub fn ims_datetime(s: &str) -> Option<String> {
    let s = s.trim();
    openreadout_core::time::iso8601_to_unix(s)?;
    let (d, t) = s.split_once(' ')?;
    Some(format!("{d}T{t}"))
}

/// Length unit of `DataSetInfo/Image/Unit` → micrometres per unit (empty = µm, Imaris' default).
fn unit_um(u: Option<&str>) -> Option<f64> {
    Some(match u.map_or("", str::trim) {
        "" | "um" | "µm" | "micron" => 1.0,
        "nm" => 1e-3,
        "mm" => 1e3,
        "cm" => 1e4,
        "m" => 1e6,
        _ => return None,
    })
}

impl ImsDataset {
    /// Open an Imaris file: group structure and `DataSetInfo` only.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let file = open_h5_in(fs, path, IMS_FORMAT_ID)?;
        let root_attrs = file
            .root()
            .attrs()
            .map(|a| attrs_json(&a))
            .unwrap_or_default();
        let version = root_attrs
            .get("ImarisVersion")
            .and_then(Value::as_str)
            .map(str::to_string);
        let data = file.group("DataSet").map_err(|_| {
            corrupt("no DataSet group: not an Imaris 5 file, or the file is truncated")
        })?;
        let level_names = data
            .groups()
            .map_err(|e| corrupt(format!("DataSet: {e}")))?;
        let level_ids = numbered(&level_names, "ResolutionLevel");
        if level_ids.is_empty() {
            return Err(corrupt("DataSet holds no `ResolutionLevel N` group"));
        }
        let l0 = format!("DataSet/ResolutionLevel {}", level_ids[0]);
        let tps = numbered(
            &file
                .group(&l0)
                .and_then(|g| g.groups())
                .map_err(|e| corrupt(format!("{l0}: {e}")))?,
            "TimePoint",
        );
        if tps.is_empty() {
            return Err(corrupt(format!("{l0} holds no `TimePoint N` group")));
        }
        let t0 = format!("{l0}/TimePoint {}", tps[0]);
        let chs = numbered(
            &file
                .group(&t0)
                .and_then(|g| g.groups())
                .map_err(|e| corrupt(format!("{t0}: {e}")))?,
            "Channel",
        );
        if chs.is_empty() {
            return Err(corrupt(format!("{t0} holds no `Channel N` group")));
        }
        if tps.iter().enumerate().any(|(i, &t)| t != i as u32)
            || chs.iter().enumerate().any(|(i, &c)| c != i as u32)
        {
            return Err(corrupt(
                "TimePoint / Channel groups are not numbered 0..n without gaps",
            ));
        }
        let mut dataset_info = BTreeMap::new();
        if let Ok(g) = file.group("DataSetInfo") {
            for name in g.groups().unwrap_or_default() {
                if let Ok(a) = file
                    .group(&format!("DataSetInfo/{name}"))
                    .and_then(|x| x.attrs())
                {
                    dataset_info.insert(name, attrs_text(&a));
                }
            }
        }
        let image = dataset_info.get("Image").cloned().unwrap_or_default();
        let num = |m: &BTreeMap<String, String>, k: &str| {
            m.get(k).and_then(|v| v.trim().parse::<f64>().ok())
        };
        let mut levels = Vec::new();
        for &l in &level_ids {
            let g = format!("DataSet/ResolutionLevel {l}/TimePoint 0/Channel 0");
            let a = file
                .group(&g)
                .and_then(|x| x.attrs())
                .map(|a| attrs_text(&a))
                .unwrap_or_default();
            let shape = file
                .dataset(&format!("{g}/Data"))
                .and_then(|d| d.shape())
                .map_err(|e| corrupt(format!("{g}/Data: {e}")))?;
            let stored = |i: usize| shape.get(shape.len().wrapping_sub(3 - i)).copied();
            let pick = |k: &str, i: usize, img: &str| {
                a.get(k)
                    .and_then(|v| v.parse::<u64>().ok())
                    .or_else(|| {
                        (l == level_ids[0])
                            .then(|| image.get(img).and_then(|v| v.parse::<u64>().ok()))
                            .flatten()
                    })
                    .or_else(|| stored(i))
                    .unwrap_or(1)
            };
            levels.push(ImsLevel {
                level: l,
                size_x: pick("ImageSizeX", 2, "X"),
                size_y: pick("ImageSizeY", 1, "Y"),
                size_z: pick("ImageSizeZ", 0, "Z"),
            });
        }
        let first = PlaneDataset::open(&file, &format!("{t0}/Channel 0/Data"), IMS_FORMAT_ID)?;
        let pixel_type = first.pixel_type;
        let lv0 = &levels[0];
        if lv0.size_x > first.shape[2] || lv0.size_y > first.shape[1] || lv0.size_z > first.shape[0]
        {
            return Err(corrupt(format!(
                "image size {}x{}x{} exceeds the stored extent {:?}",
                lv0.size_x, lv0.size_y, lv0.size_z, first.shape
            )));
        }
        let mut info = ImageInfo::new(
            0,
            u32::try_from(lv0.size_x).unwrap_or(u32::MAX),
            u32::try_from(lv0.size_y).unwrap_or(u32::MAX),
            pixel_type,
        );
        info.size_z = u32::try_from(lv0.size_z).unwrap_or(u32::MAX);
        info.size_c = chs.len() as u32;
        info.size_t = tps.len() as u32;
        // one (z, y, x) volume per time point and channel: z varies fastest
        info.dimension_order = "XYZCT".into();
        info.pyramid_levels = levels.len() as u32;
        info.name = image
            .get("Name")
            .map(|n| n.rsplit(['\\', '/']).next().unwrap_or(n).to_string());
        // physical size: extent / pixels, in the file's unit
        let f = unit_um(image.get("Unit").map(String::as_str));
        let px = |a: usize, n: u64| {
            let lo = num(&image, &format!("ExtMin{a}"))?;
            let hi = num(&image, &format!("ExtMax{a}"))?;
            Some((hi - lo) / n as f64 * f?)
        };
        info.physical_size = PhysicalSize::micrometres(
            px(0, lv0.size_x),
            px(1, lv0.size_y),
            if lv0.size_z > 1 {
                px(2, lv0.size_z)
            } else {
                None
            },
        );
        let mut notes = Vec::new();
        if f.is_none() {
            notes.push(format!(
                "unknown length unit {:?}: physical sizes are not reported",
                image.get("Unit")
            ));
        }
        // time points
        let times: Vec<f64> = dataset_info
            .get("TimeInfo")
            .map(|t| {
                (1..=tps.len())
                    .filter_map(|i| t.get(&format!("TimePoint{i}")))
                    .filter_map(|s| openreadout_core::time::iso8601_to_unix(s))
                    .collect()
            })
            .unwrap_or_default();
        if times.len() == tps.len() && times.len() > 1 {
            let dt = (times[times.len() - 1] - times[0]) / (times.len() - 1) as f64;
            if dt > 0.0 {
                info.time_increment_s = Some(dt);
            }
            info.extra.insert(
                "delta_t_s".into(),
                json!(times.iter().map(|t| t - times[0]).collect::<Vec<_>>()),
            );
        }
        info.acquired_at = image.get("RecordingDate").and_then(|s| ims_datetime(s));
        if info.acquired_at.is_some() {
            notes.push(
                "acquired_at is Imaris' RecordingDate, a local wall-clock time without a time zone"
                    .into(),
            );
        }
        // channels
        for c in 0..info.size_c {
            let a = dataset_info
                .get(&format!("Channel {c}"))
                .cloned()
                .unwrap_or_default();
            info.channels.push(ChannelInfo {
                index: c,
                name: a.get("Name").cloned(),
                color: a.get("Color").and_then(|s| ims_color(s)),
                excitation_nm: a
                    .get("LSMExcitationWavelength")
                    .and_then(|s| leading_number(s))
                    .filter(|v| *v > 0.0),
                emission_nm: a
                    .get("LSMEmissionWavelength")
                    .and_then(|s| leading_number(s))
                    .filter(|v| *v > 0.0),
                ..ChannelInfo::default()
            });
        }
        let mag = num(&image, "LensPower").filter(|v| *v > 0.0);
        let na = num(&image, "NumericalAperture").filter(|v| *v > 0.0);
        if mag.is_some() || na.is_some() {
            info.objective = Some(ObjectiveInfo {
                nominal_magnification: mag,
                lens_na: na,
                ..ObjectiveInfo::default()
            });
        }
        let imaris = dataset_info.get("Imaris");
        info.instrument = Some(InstrumentInfo {
            software: Some("Imaris".into()),
            software_version: imaris
                .and_then(|m| m.get("Version").cloned())
                .or_else(|| version.clone()),
            ..InstrumentInfo::default()
        });
        let ex = &mut info.extra;
        if let Some(m) = image.get("MicroscopeMode") {
            ex.insert("microscope_mode".into(), json!(m));
        }
        if let Some(d) = image
            .get("Description")
            .filter(|d| !d.starts_with("(description"))
        {
            ex.insert("description".into(), json!(d));
        }
        if let Some(n) = image.get("Name") {
            ex.insert("source_name".into(), json!(n));
        }
        ex.insert(
            "extent_um".into(),
            json!({
                "min": [num(&image, "ExtMin0"), num(&image, "ExtMin1"), num(&image, "ExtMin2")],
                "max": [num(&image, "ExtMax0"), num(&image, "ExtMax1"), num(&image, "ExtMax2")],
            }),
        );
        if levels.len() > 1 {
            ex.insert(
                "level_sizes".into(),
                json!(
                    levels
                        .iter()
                        .map(|l| [l.size_x, l.size_y, l.size_z])
                        .collect::<Vec<_>>()
                ),
            );
        }
        let n_datasets = root_attrs
            .get("NumberOfDataSets")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .or_else(|| root_attrs.get("NumberOfDataSets"))
            .and_then(Value::as_u64);
        if n_datasets.is_some_and(|n| n > 1) {
            notes.push(format!(
                "the file declares {} data sets; only DataSet is read",
                n_datasets.unwrap_or(0)
            ));
        }
        let other_groups: Vec<String> = file
            .root()
            .groups()
            .unwrap_or_default()
            .into_iter()
            .filter(|g| {
                !matches!(
                    g.as_str(),
                    "DataSet" | "DataSetInfo" | "DataSetTimes" | "Thumbnail"
                )
            })
            .collect();
        let mut scene = match scene_tables(&file) {
            Ok(t) => t,
            Err(e) => {
                notes.push(format!("Scene8 objects could not be read: {e}"));
                Vec::new()
            }
        };
        for (i, t) in scene.iter_mut().enumerate() {
            t.info.index = i as u32;
        }
        if !scene.is_empty() {
            let objects: std::collections::BTreeSet<&str> =
                scene.iter().map(|t| t.object.as_str()).collect();
            notes.push(format!(
                "Imaris scene objects ({}) are tables: statistics per category and record datasets (`table`)",
                objects.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        let listed: Vec<&str> = other_groups
            .iter()
            .map(String::as_str)
            .filter(|g| *g != "Scene8" || scene.is_empty())
            .collect();
        if !listed.is_empty() {
            notes.push(format!(
                "Imaris groups listed, not decoded: {}",
                listed.join(", ")
            ));
        }

        if levels.len() > 1 {
            notes.push("resolution levels are readable with `planes --level N`".into());
        }
        let mut info = info.finish();
        if levels.len() > 1 || first.chunk_shape().is_some() {
            // Tile = the chunk's y/x extent at each level (chunks are opened lazily: level 0's
            // first dataset stands for the others, which Imaris chunks alike).
            let (tw, th) = first
                .chunk_shape()
                .map_or((0, 0), |c| (c[2] as u32, c[1] as u32));
            let (w0, h0) = (info.size_x, info.size_y);
            info.resolution_levels = levels
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let (w, h) = (
                        u32::try_from(l.size_x).unwrap_or(u32::MAX),
                        u32::try_from(l.size_y).unwrap_or(u32::MAX),
                    );
                    let mut r = ResolutionLevel::new(i as u32, w, h, w0, h0).with_tile(tw, th);
                    if l.size_z != u64::from(info.size_z) {
                        r.size_z = Some(u32::try_from(l.size_z).unwrap_or(u32::MAX));
                    }
                    r
                })
                .collect();
        }
        let mut planes = HashMap::new();
        planes.insert(first.path.clone(), first);
        Ok(ImsDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            file,
            version,
            levels,
            timepoints: tps.len() as u32,
            channels: chs.len() as u32,
            pixel_type,
            info,
            dataset_info,
            root_attrs,
            other_groups,
            planes,
            notes,
            chunks: TileCache::new(64 << 20),
            scene,
        })
    }

    fn data_path(level: u32, t: u32, c: u32) -> String {
        format!("DataSet/ResolutionLevel {level}/TimePoint {t}/Channel {c}/Data")
    }

    fn plane_dataset(&mut self, p: &str) -> Result<&PlaneDataset> {
        if !self.planes.contains_key(p) {
            let d = PlaneDataset::open(&self.file, p, IMS_FORMAT_ID)?;
            self.planes.insert(p.to_string(), d);
        }
        self.planes
            .get(p)
            .ok_or_else(|| Error::Other("plane dataset cache".into()))
    }

    fn read(&mut self, image: u32, idx: PlaneIndex, level: u32) -> Result<Plane> {
        self.read_region_at(image, idx, level, None)
    }

    /// Plane `idx` at `level`, or only `region` of it (decoding only the chunks it touches;
    /// decoded chunks, which span several z planes, are cached for the next read).
    fn read_region_at(
        &mut self,
        image: u32,
        idx: PlaneIndex,
        level: u32,
        region: Option<Region>,
    ) -> Result<Plane> {
        if image != 0 {
            return Err(Error::Usage(format!(
                "image index {image} out of range (0..1)"
            )));
        }
        let lv = self.levels.get(level as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "level {level} out of range (0..{})",
                self.levels.len()
            ))
        })?;
        if idx.c >= self.channels || idx.t >= self.timepoints || u64::from(idx.z) >= lv.size_z {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range (c<{}, z<{}, t<{}) at level {level}",
                idx.c, idx.z, idx.t, self.channels, lv.size_z, self.timepoints
            )));
        }
        let (lw, lh) = (lv.size_x as u32, lv.size_y as u32);
        let win = match region {
            Some(r) => {
                r.check_within(lw, lh, &format!("image 0 level {level}"))?;
                r
            }
            None => Region::full(lw, lh),
        };
        let p = Self::data_path(lv.level, idx.t, idx.c);
        let path = self.path.clone();
        let ds = self.plane_dataset(&p)?.clone();
        if ds.pixel_type != self.pixel_type {
            return Err(corrupt(format!(
                "{p} stores {} samples, the image is {}",
                ds.pixel_type.ome_name(),
                self.pixel_type.ome_name()
            )));
        }
        let src = PlaneSource {
            h5: &self.file,
            fs: &self.fs,
            path: &path,
            format: IMS_FORMAT_ID,
        };
        let data = ds.read_plane_region(
            src,
            u64::from(idx.z),
            (
                u64::from(win.x),
                u64::from(win.y),
                u64::from(win.width),
                u64::from(win.height),
            ),
            (lv.size_x, lv.size_y),
            Some(&mut self.chunks),
        )?;
        Ok(Plane {
            width: win.width,
            height: win.height,
            pixel_type: self.pixel_type,
            samples_per_pixel: 1,
            data,
        })
    }
}

impl Dataset for ImsDataset {
    fn assurance_observations(&self) -> openreadout_core::assurance::Observations {
        crate::assurance::hdf5_filters(
            &self
                .levels
                .first()
                .and_then(|l| {
                    self.file
                        .dataset(&format!(
                            "DataSet/ResolutionLevel {}/TimePoint 0/Channel 0/Data",
                            l.level
                        ))
                        .ok()
                })
                .map(|d| d.filters())
                .unwrap_or_default(),
        )
    }

    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.fs.metadata(&self.path).map_or(0, |m| m.len()),
            format: ImsReader.descriptor(),
            format_version: self.version.clone(),
            plane_count: self.info.plane_count,
            images: vec![self.info.clone()],
            tables: self.scene.iter().map(|t| t.info.clone()).collect(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes: self.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({
            "root": self.root_attrs,
            "DataSetInfo": self.dataset_info,
            "other_groups": self.other_groups,
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
            "images[].physical_size",
            "images[].channels",
            "images[].pyramid_levels",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        let i = &self.info;
        for (k, filled) in [
            ("images[].time_increment_s", i.time_increment_s.is_some()),
            ("images[].acquired_at", i.acquired_at.is_some()),
            ("images[].objective", i.objective.is_some()),
            ("images[].name", i.name.is_some()),
        ] {
            if filled {
                p.insert(k.into(), Source::Inferred);
            }
        }
        p.insert("images[].pixel_type".into(), Source::Spec);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for lv in &self.levels {
            let p = Self::data_path(lv.level, 0, 0);
            let d = self
                .planes
                .get(&p)
                .cloned()
                .or_else(|| PlaneDataset::open(&self.file, &p, IMS_FORMAT_ID).ok());
            out.push(LsEntry {
                kind: "pyramid-level".into(),
                name: format!("DataSet/ResolutionLevel {}", lv.level),
                offset: None,
                size: None,
                image: Some(0),
                details: json!({
                    "size": [lv.size_x, lv.size_y, lv.size_z],
                    "stored": d.as_ref().map(|d| [d.shape[2], d.shape[1], d.shape[0]]),
                    "chunk": d.as_ref().and_then(PlaneDataset::chunk_shape).map(|c| [c[2], c[1], c[0]]),
                    "filters": d.as_ref().map(PlaneDataset::filter_names),
                    "timepoints": self.timepoints,
                    "channels": self.channels,
                }),
            });
        }
        for (name, attrs) in &self.dataset_info {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: format!("DataSetInfo/{name}"),
                offset: None,
                size: None,
                image: None,
                details: json!({"attributes": attrs.len()}),
            });
        }
        for g in &self.other_groups {
            out.push(LsEntry {
                kind: "group".into(),
                name: g.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({"decoded": false}),
            });
        }
        for t in &self.scene {
            out.push(LsEntry {
                kind: "table".into(),
                name: t.info.name.clone().unwrap_or_default(),
                offset: None,
                size: None,
                image: None,
                details: json!({"table": t.info.index, "rows": t.info.row_count, "columns": t.info.columns.len(),
                                "object": t.object, "kind": t.info.extra.get("kind")}),
            });
        }
        if self.file.dataset("Thumbnail/Data").is_ok() {
            out.push(LsEntry {
                kind: "attachment".into(),
                name: "Thumbnail/Data".into(),
                offset: None,
                size: None,
                image: None,
                details: Value::Null,
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        self.read(image, index, 0)
    }

    fn read_plane_level(&mut self, image: u32, index: PlaneIndex, level: u32) -> Result<Plane> {
        self.read(image, index, level)
    }

    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        self.read_region_at(image, index, level, Some(region))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), IMS_FORMAT_ID);
        r.performed("HDF5 structure parses; DataSet/ResolutionLevel/TimePoint/Channel groups are numbered without gaps");
        r.performed("every level x time point x channel has a Data dataset of the image's sample type, at least as large as the level's ImageSize");
        r.performed("the last plane of every level-0 volume and the first plane of every other level decode (chunk filters, truncation)");
        let flen = self.fs.metadata(&self.path).map_or(0, |m| m.len());
        let sb = self.file.superblock();
        let eof = sb.base_address.get().saturating_add(sb.eof_address);
        if eof > flen {
            r.push(
                Finding::error(
                    "truncated",
                    format!("file is {flen} bytes but its HDF5 superblock says it ends at {eof}"),
                )
                .at(flen),
            );
        }
        for n in self.notes.clone() {
            if n.starts_with("unknown length unit") {
                r.push(Finding::warning("unit", n));
            }
        }
        for lv in self.levels.clone() {
            for t in 0..self.timepoints {
                for c in 0..self.channels {
                    let p = Self::data_path(lv.level, t, c);
                    let d = match self.plane_dataset(&p) {
                        Ok(d) => d.clone(),
                        Err(e) => {
                            r.push(Finding::error("missing_data", format!("{p}: {e}")));
                            continue;
                        }
                    };
                    if d.shape[2] < lv.size_x || d.shape[1] < lv.size_y || d.shape[0] < lv.size_z {
                        r.push(Finding::error(
                            "extent",
                            format!("{p}: stored {:?} is smaller than the image size", d.shape),
                        ));
                        continue;
                    }
                    let z = if lv.level == self.levels[0].level {
                        lv.size_z.saturating_sub(1) as u32
                    } else {
                        0
                    };
                    let li = self
                        .levels
                        .iter()
                        .position(|x| x.level == lv.level)
                        .unwrap_or(0) as u32;
                    if let Err(e) = self.read(0, PlaneIndex { c, z, t }, li) {
                        r.push(Finding::error("plane_decode", format!("{p} z={z}: {e}")));
                    }
                }
            }
        }
        Ok(r)
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        Ok(Vec::new())
    }

    fn read_table(
        &mut self,
        index: u32,
        first_row: u64,
        max_rows: u64,
    ) -> Result<openreadout_core::model::Table> {
        let t = self.scene.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {index} does not exist ({} tables)",
                self.scene.len()
            ))
        })?;
        let cols = read_scene_table(&self.file, t)?;
        let n = cols.first().map_or(0, Vec::len) as u64;
        let start = first_row.min(n) as usize;
        let end = first_row.saturating_add(max_rows).min(n) as usize;
        Ok(openreadout_core::model::Table {
            table: index,
            first_row,
            columns: cols.into_iter().map(|c| c[start..end].to_vec()).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_fields() {
        assert_eq!(ims_color("1.000 0.000 0.000").as_deref(), Some("#FF0000"));
        assert_eq!(ims_color("0.192 0.878 0.192").as_deref(), Some("#31E031"));
        assert_eq!(ims_color("2 0 0"), None);
        assert_eq!(leading_number("561 nm nm"), Some(561.0));
        assert_eq!(leading_number("nm"), None);
        assert_eq!(
            ims_datetime("2018-10-22 13:09:30.696").as_deref(),
            Some("2018-10-22T13:09:30.696")
        );
        assert_eq!(ims_datetime("yesterday"), None);
        assert_eq!(
            numbered(
                &["Channel 1".into(), "Channel 0".into(), "x".into()],
                "Channel"
            ),
            vec![0, 1]
        );
        assert_eq!(unit_um(Some("nm")), Some(1e-3));
        assert_eq!(unit_um(Some("furlong")), None);
    }
}
