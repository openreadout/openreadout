//! NeXus NXmx diffraction data (DECTRIS EIGER and JUNGFRAU master files, beamline `.nxs`
//! files): the experiment in `/entry` (beam, detector, goniometer) and the images in data
//! files that `/entry/data/data_NNNNNN` links to, usually bit-shuffled and LZ4-compressed.
//! See `docs/formats/nxmx.md`.

use std::path::{Path, PathBuf};

use openreadout_core::diffraction::{Geometry, Radiation};
use openreadout_core::model::{
    ChannelInfo, CheckReport, DetectConfidence, FileInfo, Finding, FormatDescriptor, ImageInfo,
    InstrumentInfo, LsEntry,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex};
use openreadout_core::region::TileCache;
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use crate::h5util::{PlaneDataset, PlaneSource, attr_text, looks_like_hdf5, open_h5_in};

/// Format id used on the command line and in JSON.
pub const NXMX_FORMAT_ID: &str = "nxmx";
/// Data files read at most.
const MAX_DATA_FILES: usize = 10_000;
/// Bytes of the master file searched for external links.
const MAX_LINK_SCAN: u64 = 64 << 20;
/// Decoded chunks kept between reads (an EIGER 16M frame is 36 MB).
const CHUNK_CACHE_BYTES: usize = 256 << 20;

/// The NXmx reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct NxmxReader;

impl FormatReader for NxmxReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::NXMX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: NXMX_FORMAT_ID.into(),
            name: "NeXus NXmx diffraction (EIGER master/data HDF5)".into(),
            vendor: "open standard (NeXus NXmx; DECTRIS file writer)".into(),
            extensions: vec!["h5".into(), "nxs".into()],
            family: "diffraction".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::NXMX.confidence,
            known_gaps: vec![
                "Images come from the data files the master links to (`/entry/data/data_NNNNNN`); a virtual dataset (`/entry/data/data` in `.nxs` files) is not read, its source files are".into(),
                "Multi-module detectors are read as the stored full frame; per-module geometry (`NXdetector_module` offsets and orientations) is listed raw, not applied".into(),
                "The detector's pixel mask (`pixel_mask`) is reported, not applied: masked pixels keep the value the detector wrote (its maximum when the mask was applied on the detector)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if !looks_like_hdf5(head) {
            return None;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if name.ends_with("_master.h5") || name.ends_with("_master.hdf5") {
            return Some(det(DetectConfidence::Definite, "HDF5 file named like an EIGER master file"));
        }
        if name.ends_with(".nxs") || data_file_prefix(&name).is_some() {
            return Some(det(
                DetectConfidence::Likely,
                "HDF5 file named like a NeXus file or an EIGER data file",
            ));
        }
        None
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let d = self.sniff(head, input.path())?;
        if d.confidence == DetectConfidence::Definite {
            return Some(d);
        }
        // A `.nxs` or data-file name: only claim it when it is NXmx, or a data file whose
        // master is next to it.
        let name = input.path().file_name()?.to_string_lossy().to_string();
        if data_file_prefix(&name.to_ascii_lowercase()).is_some() {
            return find_master(input.fs(), input.path()).map(|_| d);
        }
        let file = open_h5_in(input.fs(), input.path(), NXMX_FORMAT_ID).ok()?;
        is_nxmx(&file).then_some(d)
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(NxmxDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

fn det(confidence: DetectConfidence, note: &str) -> Detection {
    Detection {
        format_id: NXMX_FORMAT_ID,
        confidence,
        note: Some(note.into()),
    }
}

/// `prefix` of a data file named `<prefix>_data_NNNNNN.h5` or `<prefix>_NNNNNN.h5`.
fn data_file_prefix(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".h5").or_else(|| name.strip_suffix(".hdf5"))?;
    let (prefix, num) = stem.rsplit_once('_')?;
    if num.len() != 6 || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(prefix.strip_suffix("_data").unwrap_or(prefix))
}

/// The master (or `.nxs`) file next to a data file.
fn find_master(fs: &Fs, data: &Path) -> Option<PathBuf> {
    let name = data.file_name()?.to_string_lossy().to_string();
    let prefix = data_file_prefix(&name)?.to_string();
    // The prefix was lower-cased for matching: take the original spelling.
    let prefix = &name[..prefix.len()];
    let dir = data.parent().unwrap_or(Path::new(""));
    [
        format!("{prefix}_master.h5"),
        format!("{prefix}_master.hdf5"),
        format!("{prefix}.nxs"),
    ]
    .into_iter()
    .map(|n| dir.join(n))
    .find(|p| fs.metadata(p).is_ok_and(|m| m.is_file()))
}

/// `/entry/definition` is `NXmx`, or `/entry` has an `NXdetector` under `instrument`.
fn is_nxmx(file: &hdf5_pure::File) -> bool {
    if let Some(d) = string_at(file, "/entry/definition") {
        return d.trim().eq_ignore_ascii_case("NXmx");
    }
    file.group("/entry/instrument/detector").is_ok() && file.group("/entry/data").is_ok()
}

fn string_at(file: &hdf5_pure::File, path: &str) -> Option<String> {
    let ds = file.dataset(path).ok()?;
    let s = ds.read_string().ok()?.into_iter().next()?;
    let s: String = s.chars().filter(|&c| c != '\0').collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// The values of a numeric dataset (at most `limit`), and its `units` attribute.
fn numbers_at(file: &hdf5_pure::File, path: &str, limit: u64) -> Option<(Vec<f64>, Option<String>)> {
    let ds = file.dataset(path).ok()?;
    let shape = ds.shape().ok()?;
    let n: u64 = shape.iter().product::<u64>().max(1);
    if n > limit {
        return None;
    }
    let v = ds.read_f64().ok()?;
    let units = ds.attrs().ok().and_then(|a| a.get("units").and_then(attr_text));
    Some((v, units))
}

fn number_at(file: &hdf5_pure::File, path: &str) -> Option<(f64, Option<String>)> {
    let (v, u) = numbers_at(file, path, 16)?;
    v.first().copied().filter(|x| x.is_finite()).map(|x| (x, u))
}

/// A length in millimetres from a value and its units (metres when none are given).
fn to_mm(v: f64, units: Option<&str>) -> f64 {
    match units.map(str::trim).unwrap_or("m") {
        "mm" => v,
        "um" | "µm" | "micron" | "microns" => v * 1e-3,
        "cm" => v * 10.0,
        _ => v * 1e3,
    }
}

/// A wavelength in ångström.
fn to_angstrom(v: f64, units: Option<&str>) -> f64 {
    match units.map(str::trim).unwrap_or("angstrom") {
        "nm" => v * 10.0,
        "m" => v * 1e10,
        _ => v,
    }
}

/// External links named `data_NNNNNN` found in the raw bytes of the master file: their link
/// messages hold the name, a two-byte length, a flags byte, then the file name and the object
/// path, each NUL-terminated (HDF5 link message, external link). `hdf5-pure` resolves no
/// external links, so they are read here.
pub fn external_data_links(bytes: &[u8]) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    let pat = b"data_";
    let mut i = 0;
    while let Some(p) = bytes[i..].windows(pat.len()).position(|w| w == pat) {
        let at = i + p;
        i = at + 1;
        let Some(digits) = bytes.get(at + 5..at + 11) else {
            break;
        };
        if !digits.iter().all(u8::is_ascii_digit) {
            continue;
        }
        let name_end = at + 11;
        let Some(len) = bytes
            .get(name_end..name_end + 2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
        else {
            continue;
        };
        let Some(value) = bytes.get(name_end + 2..name_end + 2 + len) else {
            continue;
        };
        if value.first() != Some(&0) || value.last() != Some(&0) {
            continue;
        }
        let body = &value[1..value.len() - 1];
        let Some(nul) = body.iter().position(|&b| b == 0) else {
            continue;
        };
        let (file, obj) = (&body[..nul], &body[nul + 1..]);
        if file.is_empty() || !obj.starts_with(b"/") || obj.contains(&0) {
            continue;
        }
        let (Ok(file), Ok(obj)) = (std::str::from_utf8(file), std::str::from_utf8(obj)) else {
            continue;
        };
        let name = String::from_utf8_lossy(&bytes[at..name_end]).into_owned();
        if !out.iter().any(|(n, _, _)| *n == name) {
            out.push((name, file.to_string(), obj.to_string()));
        }
    }
    out.sort();
    out
}

/// One data file: its images are the time points `first..first + count`.
struct DataFile {
    link: String,
    path: PathBuf,
    object: String,
    file: hdf5_pure::File,
    plane: PlaneDataset,
    first: u64,
    count: u64,
}

impl std::fmt::Debug for DataFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataFile")
            .field("link", &self.link)
            .field("path", &self.path)
            .field("first", &self.first)
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

/// An opened NXmx master (or `.nxs`) file with its data files.
pub struct NxmxDataset {
    path: PathBuf,
    /// The master file actually read (the input itself, or the master next to a data file).
    master: PathBuf,
    fs: Fs,
    file: hdf5_pure::File,
    data: Vec<DataFile>,
    width: u32,
    height: u32,
    pixel_type: PixelType,
    frames: u64,
    geometry: Geometry,
    /// Per-image rotation angles (`axis`, values) when the scan axis has one per image.
    angles: Option<(String, Vec<f64>)>,
    extra: Map<String, Value>,
    instrument: InstrumentInfo,
    started_at: Option<String>,
    ended_at: Option<String>,
    notes: Vec<String>,
    problems: Vec<(&'static str, String)>,
    chunks: TileCache<u64, Vec<u8>>,
    /// The master was opened without any of its data files.
    data_missing: bool,
}

impl std::fmt::Debug for NxmxDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NxmxDataset")
            .field("path", &self.path)
            .field("data", &self.data)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

impl NxmxDataset {
    /// Open a master file, a `.nxs` file or a data file next to its master.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let fs = input.fs().clone();
        let path = input.path().to_path_buf();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let mut notes = Vec::new();
        let master = if data_file_prefix(&name).is_some() {
            let m = find_master(&fs, &path).ok_or_else(|| {
                Error::unsupported(
                    NXMX_FORMAT_ID,
                    "an EIGER data file without its master file",
                    "open the `<prefix>_master.h5` (or `.nxs`) file next to it; the data files hold only the images, the master holds the geometry",
                )
            })?;
            notes.push(format!(
                "this is a data file: read through its master {}",
                m.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            ));
            m
        } else {
            path.clone()
        };
        let file = open_h5_in(&fs, &master, NXMX_FORMAT_ID)?;
        if !is_nxmx(&file) {
            return Err(Error::corrupt(
                NXMX_FORMAT_ID,
                "no NXmx entry (`/entry/definition` = NXmx, or `/entry/instrument/detector` and `/entry/data`)",
            ));
        }
        let mut ds = NxmxDataset {
            path,
            master: master.clone(),
            fs,
            file,
            data: Vec::new(),
            width: 0,
            height: 0,
            pixel_type: PixelType::Uint16,
            frames: 0,
            geometry: Geometry::default(),
            angles: None,
            extra: Map::new(),
            instrument: InstrumentInfo::default(),
            started_at: None,
            ended_at: None,
            notes,
            problems: Vec::new(),
            chunks: TileCache::new(CHUNK_CACHE_BYTES),
            data_missing: false,
        };
        ds.read_data_files()?;
        ds.read_geometry();
        Ok(ds)
    }

    fn read_data_files(&mut self) -> Result<()> {
        let src = self
            .fs
            .source(&self.master)
            .map_err(|e| Error::io(&self.master, e))?;
        let len = src.size().map_err(|e| Error::io(&self.master, e))?;
        let n = usize::try_from(len.min(MAX_LINK_SCAN)).unwrap_or(0);
        let mut bytes = vec![0u8; n];
        src.read_exact_at(0, &mut bytes)
            .map_err(|e| Error::io(&self.master, e))?;
        let links = external_data_links(&bytes);
        let dir = self.master.parent().unwrap_or(Path::new("")).to_path_buf();
        let mut first = 0u64;
        for (link, fname, object) in links.into_iter().take(MAX_DATA_FILES) {
            let p = dir.join(&fname);
            if self.fs.metadata(&p).is_err() {
                self.problems.push((
                    "missing_file",
                    format!("{link} links to {fname}, which is not next to the master file"),
                ));
                continue;
            }
            let file = open_h5_in(&self.fs, &p, NXMX_FORMAT_ID)?;
            let plane = PlaneDataset::open(&file, &object, NXMX_FORMAT_ID)?;
            let [count, h, w] = plane.shape;
            let (w32, h32) = (
                u32::try_from(w).map_err(|_| Error::corrupt(NXMX_FORMAT_ID, "image too wide"))?,
                u32::try_from(h).map_err(|_| Error::corrupt(NXMX_FORMAT_ID, "image too tall"))?,
            );
            if self.data.is_empty() {
                self.width = w32;
                self.height = h32;
                self.pixel_type = plane.pixel_type;
            } else if (w32, h32, plane.pixel_type) != (self.width, self.height, self.pixel_type) {
                return Err(Error::corrupt(
                    NXMX_FORMAT_ID,
                    format!(
                        "{fname}: images of {w32} × {h32} {} differ from the first data file's {} × {} {}",
                        plane.pixel_type.ome_name(),
                        self.width,
                        self.height,
                        self.pixel_type.ome_name()
                    ),
                ));
            }
            // DECTRIS data files say which images they hold (1-based, inclusive).
            if let Ok(a) = file.dataset(&object).and_then(|d| d.attrs()) {
                let num = |k: &str| a.get(k).and_then(attr_text).and_then(|t| t.parse::<u64>().ok());
                if let (Some(lo), Some(hi)) = (num("image_nr_low"), num("image_nr_high"))
                    && (lo != first + 1 || hi.checked_sub(lo).map(|d| d + 1) != Some(count))
                {
                    self.problems.push((
                        "image_numbers",
                        format!(
                            "{fname}: holds images {lo}–{hi} by its attributes, {} at position {}",
                            count,
                            first + 1
                        ),
                    ));
                }
            }
            self.data.push(DataFile {
                link,
                path: p,
                object,
                file,
                plane,
                first,
                count,
            });
            first += count;
        }
        self.frames = first;
        if self.data.is_empty() {
            // A master on its own: the image's size from the detector description, no pixels.
            let f = &self.file;
            let spec = "/entry/instrument/detector/detectorSpecific";
            let n = |k: &str| number_at(f, &format!("{spec}/{k}")).map(|(v, _)| v);
            let (Some(w), Some(h)) = (n("x_pixels_in_detector"), n("y_pixels_in_detector")) else {
                return Err(Error::unsupported(
                    NXMX_FORMAT_ID,
                    "no readable data file",
                    "the master links its images from `<prefix>_data_NNNNNN.h5` (or `<prefix>_NNNNNN.h5`) files; put them next to the master file",
                ));
            };
            let images = n("nimages").unwrap_or(1.0).max(1.0) * n("ntrigger").unwrap_or(1.0).max(1.0);
            self.width = w as u32;
            self.height = h as u32;
            self.frames = images as u64;
            self.pixel_type = match n("bit_depth_image").map(|b| b as u32) {
                Some(8) => PixelType::Uint8,
                Some(32) => PixelType::Uint32,
                _ => PixelType::Uint16,
            };
            self.data_missing = true;
            self.notes.push("no data file next to the master: geometry and image size only (from detectorSpecific); reading pixels needs the data files".into());
        }
        Ok(())
    }

    fn read_geometry(&mut self) {
        let f = &self.file;
        let det = "/entry/instrument/detector";
        let mut g = Geometry::default();
        g.wavelength_a = ["/entry/instrument/beam/incident_wavelength", "/entry/sample/beam/incident_wavelength"]
            .iter()
            .find_map(|p| number_at(f, p))
            .map(|(v, u)| to_angstrom(v, u.as_deref()))
            .filter(|w| *w > 0.0);
        if g.wavelength_a.is_none()
            && let Some((v, _)) = number_at(f, &format!("{det}/detectorSpecific/wavelength"))
        {
            g.wavelength_a = Some(v).filter(|w| *w > 0.0);
        }
        g.radiation = string_at(f, "/entry/instrument/source/probe")
            .map(|p| {
                if p.to_ascii_lowercase().contains("electron") {
                    Radiation::Electron
                } else {
                    Radiation::Xray
                }
            })
            .or(match g.wavelength_a {
                Some(w) if w < 0.1 => Some(Radiation::Electron),
                Some(_) => Some(Radiation::Xray),
                None => None,
            });
        g.distance_mm = number_at(f, &format!("{det}/detector_distance"))
            .map(|(v, u)| to_mm(v, u.as_deref()))
            .filter(|d| *d > 0.0);
        if let (Some((x, _)), Some((y, _))) = (
            number_at(f, &format!("{det}/beam_center_x")),
            number_at(f, &format!("{det}/beam_center_y")),
        ) {
            g.beam_center_px = Some([x, y]);
        }
        if let (Some((x, ux)), Some((y, uy))) = (
            number_at(f, &format!("{det}/x_pixel_size")),
            number_at(f, &format!("{det}/y_pixel_size")),
        ) {
            g.pixel_size_mm = Some([
                tidy(to_mm(x, ux.as_deref())),
                tidy(to_mm(y, uy.as_deref())),
            ]);
        }
        g.exposure_s = number_at(f, &format!("{det}/count_time")).map(|(v, _)| v);
        let sat = number_at(f, &format!("{det}/saturation_value"))
            .or_else(|| number_at(f, &format!("{det}/detectorSpecific/countrate_correction_count_cutoff")))
            .map(|(v, _)| v);
        let type_max = match self.pixel_type {
            PixelType::Uint8 => Some(f64::from(u8::MAX)),
            PixelType::Uint16 => Some(f64::from(u16::MAX)),
            PixelType::Uint32 => Some(f64::from(u32::MAX)),
            _ => None,
        };
        g.overload = sat.filter(|s| type_max.is_none_or(|m| *s < m));
        // Pixels in the detector's mask are written as the type's maximum.
        if let Some(m) = type_max {
            g.masked_values = vec![m];
        }
        g.negative_is_masked = matches!(self.pixel_type, PixelType::Int32 | PixelType::Int16);
        // The rotation: the goniometer axis whose values change from image to image.
        let frames = self.frames.max(1);
        let groups = ["/entry/sample/transformations", "/entry/sample/goniometer"];
        let mut best: Option<(String, Vec<f64>, Option<f64>)> = None;
        for gp in groups {
            let Ok(group) = f.group(gp) else { continue };
            let Ok(names) = group.datasets() else { continue };
            for n in names {
                if n.ends_with("_end") || n.contains("_range") || n.contains("_increment") {
                    continue;
                }
                let Some((v, _)) = numbers_at(f, &format!("{gp}/{n}"), frames.max(16)) else {
                    continue;
                };
                let width = [
                    format!("{gp}/{n}_increment_set"),
                    format!("{gp}/{n}_range_average"),
                ]
                .iter()
                .find_map(|p| number_at(f, p))
                .map(|(w, _)| w)
                .or_else(|| {
                    numbers_at(f, &format!("{gp}/{n}_end"), frames.max(16))
                        .and_then(|(e, _)| Some(e.first()? - v.first()?))
                });
                let moves = v.len() > 1 && v.windows(2).any(|w| w[0] != w[1]);
                let turns = width.is_some_and(|w| w != 0.0);
                if (moves || turns) && best.is_none() {
                    best = Some((n.clone(), v, width));
                }
            }
            if best.is_some() {
                break;
            }
        }
        if let Some((axis, values, width)) = best {
            g.scan_axis = Some(axis.clone());
            g.oscillation_start_deg = values.first().copied();
            g.oscillation_width_deg = width
                .or_else(|| (values.len() > 1).then(|| values[1] - values[0]))
                .map(tidy);
            if values.len() as u64 == self.frames && self.frames > 1 {
                self.angles = Some((axis, values));
            }
        }
        g.two_theta_deg = number_at(f, &format!("{det}/goniometer/two_theta"))
            .or_else(|| number_at(f, "/entry/sample/goniometer/two_theta"))
            .map(|(v, _)| v)
            .filter(|t| *t != 0.0);
        self.geometry = g;

        let description = string_at(f, &format!("{det}/description"));
        let maker = description.as_deref().and_then(|d| {
            let u = d.to_ascii_uppercase();
            (u.contains("EIGER") || u.contains("PILATUS") || u.contains("JUNGFRAU"))
                .then(|| if u.contains("JUNGFRAU") { "PSI".to_string() } else { "DECTRIS".to_string() })
        });
        self.instrument = InstrumentInfo {
            manufacturer: maker,
            model: description.clone(),
            software: string_at(f, &format!("{det}/detectorSpecific/software_version"))
                .map(|_| "DECTRIS detector firmware".to_string()),
            software_version: string_at(f, &format!("{det}/detectorSpecific/software_version")),
            detector: description,
        };
        self.started_at = string_at(f, "/entry/start_time")
            .or_else(|| string_at(f, &format!("{det}/detectorSpecific/data_collection_date")));
        self.ended_at = string_at(f, "/entry/end_time");
        let mut x = Map::new();
        for (key, path) in [
            ("detector_serial", format!("{det}/detector_number")),
            ("sensor_material", format!("{det}/sensor_material")),
            ("beamline", "/entry/instrument/name".to_string()),
            ("source", "/entry/instrument/source/name".to_string()),
            ("eiger_firmware", format!("{det}/detectorSpecific/eiger_fw_version")),
        ] {
            if let Some(v) = string_at(f, &path) {
                x.insert(key.into(), json!(v));
            }
        }
        if let Some(v) = self.file.group("/entry/instrument").ok().and_then(|g| g.attrs().ok()).and_then(|a| a.get("short_name").and_then(attr_text)) {
            x.entry("beamline").or_insert(json!(v));
        }
        for (key, path, scale) in [
            ("sensor_thickness_mm", format!("{det}/sensor_thickness"), 1e3),
            ("threshold_ev", format!("{det}/threshold_energy"), 1.0),
            ("frame_time_s", format!("{det}/frame_time"), 1.0),
            ("bit_depth", format!("{det}/bit_depth_readout"), 1.0),
            ("transmission", "/entry/instrument/attenuator/attenuator_transmission".to_string(), 1.0),
            ("flux", "/entry/instrument/beam/total_flux".to_string(), 1.0),
        ] {
            if let Some((v, _)) = number_at(f, &path) {
                x.insert(key.into(), json!(tidy(v * scale)));
            }
        }
        for (key, path) in [
            ("pixel_mask_applied", format!("{det}/pixel_mask_applied")),
            ("flatfield_applied", format!("{det}/flatfield_applied")),
            ("countrate_correction_applied", format!("{det}/countrate_correction_applied")),
        ] {
            if let Some((v, _)) = number_at(f, &path) {
                x.insert(key.into(), json!(v != 0.0));
            }
        }
        if let Ok(ds) = f.dataset(&format!("{det}/pixel_mask"))
            .or_else(|_| f.dataset(&format!("{det}/detectorSpecific/pixel_mask")))
            && let Ok(s) = ds.shape()
        {
            x.insert("pixel_mask".into(), json!({"shape": s}));
        }
        x.insert(
            "data_files".into(),
            json!(self.data.iter().map(|d| json!({
                "link": d.link,
                "file": d.path.file_name().map(|n| n.to_string_lossy().to_string()),
                "object": d.object,
                "images": d.count,
                "filters": d.plane.filter_names(),
            })).collect::<Vec<_>>()),
        );
        self.extra = x;
    }

    fn image_info(&self) -> ImageInfo {
        let mut im = ImageInfo::new(0, self.width, self.height, self.pixel_type);
        im.size_t = u32::try_from(self.frames).unwrap_or(u32::MAX).max(1);
        im.name = self.master.file_stem().map(|s| {
            let s = s.to_string_lossy();
            s.strip_suffix("_master").unwrap_or(&s).to_string()
        });
        im.channels = vec![ChannelInfo {
            index: 0,
            ..ChannelInfo::default()
        }];
        im.instrument = Some(self.instrument.clone()).filter(|i| i.model.is_some());
        im.acquired_at = self.started_at.clone();
        if let Some(t) = self.geometry.exposure_s.filter(|t| *t > 0.0) {
            im.time_increment_s = self
                .extra
                .get("frame_time_s")
                .and_then(Value::as_f64)
                .filter(|v| *v > 0.0)
                .or(Some(t));
        }
        self.geometry.store(&mut im);
        im.extra.insert("nxmx".into(), Value::Object(self.extra.clone()));
        im.finish()
    }

    fn locate(&self, t: u64) -> Option<(usize, u64)> {
        self.data
            .iter()
            .position(|d| t >= d.first && t < d.first + d.count)
            .map(|i| (i, t - self.data[i].first))
    }
}

fn tidy(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return v;
    }
    let digits = 11 - v.abs().log10().floor() as i32;
    let scale = 10f64.powi(digits.clamp(-300, 300));
    (v * scale).round() / scale
}

impl Dataset for NxmxDataset {
    fn info(&self) -> Result<FileInfo> {
        let im = self.image_info();
        let mut notes = self.notes.clone();
        notes.extend(self.problems.iter().map(|(_, d)| d.clone()));
        if !self.geometry.masked_values.is_empty() {
            notes.push(format!(
                "pixels the detector masks (module gaps, bad pixels) hold {}; previews draw them as no data",
                self.geometry.masked_values[0]
            ));
        }
        let size = self.fs.metadata(&self.master).map_or(0, |m| m.len())
            + self
                .data
                .iter()
                .map(|d| self.fs.metadata(&d.path).map_or(0, |m| m.len()))
                .sum::<u64>();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: size,
            format: NxmxReader.descriptor(),
            format_version: string_at(&self.file, "/entry/definition"),
            plane_count: im.plane_count,
            images: vec![im],
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn experiment(&self) -> Option<openreadout_core::Experiment> {
        let mut e = openreadout_core::diffraction::experiment(&[self.image_info()])?;
        if let Some(t) = &self.ended_at {
            e.acquisition.get_or_insert_with(Default::default).ended_at = Some(t.clone());
        }
        Some(e)
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let (nodes, truncated) = crate::h5util::walk(&self.file, 2000);
        Ok(json!({
            "master": self.master.display().to_string(),
            "truncated": truncated,
            "nodes": nodes.iter().map(|n| json!({
                "path": n.path,
                "shape": n.shape,
                "dtype": n.dtype,
                "attributes": n.attributes,
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_t", Source::Spec),
            ("images[].pixel_type", Source::Spec),
            ("images[].name", Source::Inferred),
            ("images[].instrument", Source::Spec),
            ("images[].acquired_at", Source::Spec),
            ("images[].time_increment_s", Source::Spec),
            ("images[].extra.nxmx", Source::Spec),
            ("images[].extra.diffraction.wavelength_a", Source::Spec),
            ("images[].extra.diffraction.distance_mm", Source::Spec),
            ("images[].extra.diffraction.beam_center_px", Source::Spec),
            ("images[].extra.diffraction.pixel_size_mm", Source::Spec),
            ("images[].extra.diffraction.exposure_s", Source::Spec),
            ("images[].extra.diffraction.overload", Source::Spec),
            ("images[].extra.diffraction.scan_axis", Source::Inferred),
            ("images[].extra.diffraction.oscillation_start_deg", Source::Spec),
            ("images[].extra.diffraction.oscillation_width_deg", Source::Spec),
            ("images[].extra.diffraction.masked_values", Source::PriorArt),
            ("images[].extra.diffraction.radiation", Source::Inferred),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let mut v = Vec::new();
        if self.master != self.path {
            v.push(self.master.clone());
        }
        v.extend(self.data.iter().map(|d| d.path.clone()));
        v
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = vec![LsEntry {
            kind: "metadata".into(),
            name: "master".into(),
            offset: None,
            size: self.fs.metadata(&self.master).ok().map(|m| m.len()),
            image: None,
            details: json!({"file": self.master.display().to_string()}),
        }];
        for d in &self.data {
            out.push(LsEntry {
                kind: "image".into(),
                name: d.link.clone(),
                offset: None,
                size: self.fs.metadata(&d.path).ok().map(|m| m.len()),
                image: Some(0),
                details: json!({
                    "file": d.path.display().to_string(),
                    "object": d.object,
                    "t": [d.first, d.first + d.count],
                    "chunks": d.plane.chunk_count(),
                    "chunk_shape": d.plane.chunk_shape(),
                    "filters": d.plane.filter_names(),
                }),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, idx: PlaneIndex) -> Result<Plane> {
        if image != 0 || idx.c != 0 || idx.z != 0 || u64::from(idx.t) >= self.frames {
            return Err(Error::Usage(format!(
                "image {image} plane c={} z={} t={} out of range (one image; c<1, z<1, t<{})",
                idx.c, idx.z, idx.t, self.frames
            )));
        }
        if self.data_missing {
            return Err(Error::unsupported(
                NXMX_FORMAT_ID,
                "pixels of a master file without its data files",
                "put the `<prefix>_data_NNNNNN.h5` (or `<prefix>_NNNNNN.h5`) files the master names next to it",
            ));
        }
        let (i, local) = self
            .locate(u64::from(idx.t))
            .ok_or_else(|| Error::Usage(format!("no data file holds image t={}", idx.t)))?;
        let d = &self.data[i];
        let src = PlaneSource {
            h5: &d.file,
            fs: &self.fs,
            path: &d.path,
            format: NXMX_FORMAT_ID,
        };
        let (w, h) = (u64::from(self.width), u64::from(self.height));
        let data = d
            .plane
            .read_plane_region(src, local, (0, 0, w, h), (w, h), Some(&mut self.chunks))?;
        Ok(Plane {
            width: self.width,
            height: self.height,
            pixel_type: self.pixel_type,
            samples_per_pixel: 1,
            data,
        })
    }

    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        if image != 0 {
            return Ok((0, Vec::new()));
        }
        let n = usize::try_from(self.frames).unwrap_or(usize::MAX);
        let take = limit.unwrap_or(n).min(n);
        let w = self.geometry.oscillation_width_deg;
        let rows = (0..take)
            .map(|t| {
                let mut r = Map::new();
                r.insert("t".into(), json!(t));
                if let Some((axis, v)) = &self.angles {
                    r.insert(format!("{axis}_deg"), json!(v[t]));
                } else if let (Some(axis), Some(s), Some(w)) =
                    (&self.geometry.scan_axis, self.geometry.oscillation_start_deg, w)
                {
                    r.insert(format!("{axis}_deg"), json!(tidy(s + w * t as f64)));
                }
                if let Some((i, local)) = self.locate(t as u64) {
                    r.insert("data_file".into(), json!(self.data[i].link));
                    r.insert("index_in_file".into(), json!(local));
                }
                Value::Object(r)
            })
            .collect();
        Ok((self.frames, rows))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), NXMX_FORMAT_ID);
        r.performed("master: an NXmx entry with a detector and data links");
        r.performed("every data file the master links to exists and holds images of one size and type, numbered in order (image_nr_low/high)");
        r.performed("the first and last image of every data file decode (chunk filters, truncation)");
        r.performed("geometry: wavelength, distance, pixel size and beam centre present");
        for (code, d) in &self.problems {
            let f = if *code == "missing_file" {
                Finding::error(code, d.clone())
            } else {
                Finding::warning(code, d.clone())
            };
            r.push(f);
        }
        let g = &self.geometry;
        for (name, missing) in [
            ("wavelength", g.wavelength_a.is_none()),
            ("detector distance", g.distance_mm.is_none()),
            ("pixel size", g.pixel_size_mm.is_none()),
            ("beam centre", g.beam_center_px.is_none()),
        ] {
            if missing {
                r.push(Finding::warning(
                    "missing_geometry",
                    format!("the master records no {name}"),
                ));
            }
        }
        let checks: Vec<(String, u64)> = self
            .data
            .iter()
            .flat_map(|d| {
                let mut v = vec![(d.link.clone(), d.first)];
                if d.count > 1 {
                    v.push((d.link.clone(), d.first + d.count - 1));
                }
                v
            })
            .collect();
        for (link, t) in checks {
            let Ok(t32) = u32::try_from(t) else { continue };
            if let Err(e) = self.read_plane(0, PlaneIndex { c: 0, z: 0, t: t32 }) {
                r.push(Finding::error(
                    "bad_image",
                    format!("{link}: image t={t} does not decode: {e}"),
                ));
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_file_names() {
        assert_eq!(data_file_prefix("therm_6_1_000001.h5"), Some("therm_6_1"));
        assert_eq!(data_file_prefix("x_data_000012.h5"), Some("x"));
        assert_eq!(data_file_prefix("x_master.h5"), None);
        assert_eq!(data_file_prefix("x_00001.h5"), None);
    }

    #[test]
    fn external_links_from_bytes() {
        let mut b = vec![0u8; 7];
        b.extend_from_slice(b"data_000001");
        let value = b"\0Therm_6_1_000001.h5\0/data\0";
        b.extend_from_slice(&(value.len() as u16).to_le_bytes());
        b.extend_from_slice(value);
        b.extend_from_slice(b"data_000002\x05\x00junk");
        let l = external_data_links(&b);
        assert_eq!(
            l,
            vec![("data_000001".into(), "Therm_6_1_000001.h5".into(), "/data".into())]
        );
    }

    #[test]
    fn units() {
        assert!((to_mm(0.199_783, Some("m")) - 199.783).abs() < 1e-9);
        assert_eq!(to_mm(199.0, Some("mm")), 199.0);
        assert_eq!(to_angstrom(0.98, Some("angstrom")), 0.98);
        assert!((to_angstrom(0.098, Some("nm")) - 0.98).abs() < 1e-12);
    }
}
