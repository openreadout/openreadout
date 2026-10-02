//! MetaMorph `.nd` series: the TIFF/STK files a `.nd` file names, opened as one data set (stage
//! positions as images, wavelengths as channels). See `docs/formats/tiff.md` § MetaMorph.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::{ChannelInfo, ImageInfo, ObjectiveInfo, PhysicalSize};
use openreadout_core::provenance::Source;
use openreadout_core::{Error, Fs, PixelType, Result};
use serde_json::{Map, Value, json};

use crate::container::Ifd;
use crate::dataset::{
    Flavor, PlaneSrc, Series, TiffDataset, check_plane_count, dir_of, instrument_from_tags,
    resolution_um,
};
use crate::decode::PageLayout;
use crate::files::{FileSetMember, file_name_of};
use crate::metamorph::{MetaSeriesPlane, NdFile, StkInfo, parse_metaseries, parse_stk};
use crate::{FORMAT_ID, tags};

/// Extensions tried, in order, for the files of a `.nd` series (matched case-insensitively).
const ND_EXTENSIONS: [&str; 3] = ["tif", "stk", "tiff"];

/// The first member file that opens: its index, page 0 and layout, and its MetaMorph
/// metadata (STK tags, MetaSeries description).
struct FirstMember {
    file: usize,
    page0: Ifd,
    layout: PageLayout,
    stk: Option<StkInfo>,
    metaseries: Option<MetaSeriesPlane>,
}

/// What every image of the series shares.
struct NdGeometry {
    pixel_type: PixelType,
    z_count: u32,
    samples_per_pixel: u32,
    physical_size: PhysicalSize,
}

impl TiffDataset {
    /// A MetaMorph `.nd` series: one image per stage position, wavelengths as channels, time
    /// points as T, the Z planes of each file as Z.
    pub(crate) fn open_nd(&mut self) -> Result<()> {
        let text = openreadout_core::limits::read_metadata_text(FORMAT_ID, &self.fs, &self.path)?;
        let nd = crate::metamorph::parse_nd(&text).ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                "not a MetaMorph .nd file (no \"NDInfoFile\" key)",
            )
        })?;
        self.flavor = Flavor::MetamorphNd;
        self.files.push(FileSetMember {
            path: self.path.clone(),
            name: file_name_of(&self.path),
            uuid: None,
            metadata_only: true,
            opened: None,
            error: None,
        });
        let stem = self
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let listing = dir_listing(&self.fs, &dir_of(&self.path));
        let (n_s, n_c, n_t) = (nd.image_count(), nd.channel_count(), nd.time_points);
        check_plane_count(
            u64::from(nd.z_steps)
                .saturating_mul(n_c as u64)
                .saturating_mul(u64::from(n_t))
                .saturating_mul(n_s as u64),
        )?;
        let (member_of, missing) = self.add_nd_members(&nd, &stem, &listing);
        let first = self.first_nd_member(missing)?;
        let geometry = self.nd_geometry(&nd, &first)?;
        let mut repeated = Vec::new();
        for s in 0..n_s {
            let mut info = nd_image_info(&nd, &first, &geometry, s, &stem);
            let mut planes = vec![None; geometry.z_count as usize * n_c * n_t as usize];
            for t in 0..n_t {
                for z in 0..geometry.z_count {
                    for c in 0..n_c {
                        let file = member_of[&(s, c, t)];
                        let has_z = nd.wave_do_z.get(c).copied().unwrap_or(true);
                        let index = if has_z { z } else { 0 };
                        if !has_z && geometry.z_count > 1 && s == 0 && t == 0 && z == 0 {
                            repeated.push(c);
                        }
                        let slot = (t as usize * geometry.z_count as usize + z as usize) * n_c + c;
                        planes[slot] = Some(PlaneSrc::Member { file, index });
                    }
                }
            }
            let names: Vec<String> = (0..n_t)
                .flat_map(|t| (0..n_c).map(move |c| (c, t)))
                .take(64)
                .map(|(c, t)| self.files[member_of[&(s, c, t)]].name.clone())
                .collect();
            info.extra.insert("files".into(), json!(names));
            self.series.push(Series {
                info: info.finish(),
                planes,
                levels: Vec::new(),
            });
        }
        for c in repeated {
            self.notes.push(format!(
                "wavelength {} was acquired without Z: its single plane is repeated at every Z",
                c + 1
            ));
        }
        if missing > 0 {
            self.notes.push(format!(
                "{missing} of the {} files this .nd names are missing; reads of their planes fail (see `check`)",
                self.files.len() - 1
            ));
        }
        let extra = nd_extra_files(&listing, &stem, &nd);
        if extra > 0 {
            self.notes.push(format!(
                "{extra} more file(s) named like this series lie past the {n_t} time point(s) the .nd declares; they are not read"
            ));
        }
        self.notes.push(format!(
            "MetaMorph .nd series: {} file(s), {n_s} stage position(s) as images, {n_c} wavelength(s) as channels, {n_t} time point(s), {} Z plane(s) per file",
            self.files.len() - 1,
            geometry.z_count
        ));
        if nd.start_time.is_some() {
            self.notes.push("acquired_at is the .nd StartTime1 in the acquisition computer's local time (no zone recorded)".into());
        }
        let mut v = Map::new();
        v.insert("nd".into(), Value::Object(nd.keys.clone()));
        if let Some(s) = &first.stk {
            v.insert("first_file_stk".into(), s.to_json());
        }
        if let Some(m) = &first.metaseries {
            v.insert(
                "first_file_metaseries".into(),
                Value::Object(m.props.clone()),
            );
        }
        self.vendor = Value::Object(v);
        self.nd = Some(nd);
        self.set_provenance(&[
            ("images[].size_x", Source::Spec),
            ("images[].size_y", Source::Spec),
            ("images[].size_z", Source::Inferred),
            ("images[].size_c", Source::Inferred),
            ("images[].size_t", Source::Inferred),
            ("images[].pixel_type", Source::Spec),
            ("images[].physical_size", Source::PriorArt),
            ("images[].name", Source::Inferred),
            ("images[].acquired_at", Source::Inferred),
            ("images[].channels[].name", Source::Inferred),
            ("images[].instrument", Source::Spec),
            ("images[].objective", Source::PriorArt),
            ("images[].extra.description", Source::Inferred),
            ("images[].extra.stage_index", Source::Inferred),
            ("images[].extra.files", Source::Inferred),
        ]);
        Ok(())
    }

    /// One member per (stage, wavelength, time point), named from the `.nd` keys and found
    /// in `listing` (a name not there is a missing member). Returns the member of each
    /// (stage, wavelength, time point) and the number missing.
    fn add_nd_members(
        &mut self,
        nd: &NdFile,
        stem: &str,
        listing: &HashMap<String, PathBuf>,
    ) -> (HashMap<(usize, usize, u32), usize>, usize) {
        let dir = dir_of(&self.path);
        let mut member_of = HashMap::new();
        let mut missing = 0usize;
        for s in 0..nd.image_count() {
            for w in 0..nd.channel_count() {
                for t in 0..nd.time_points {
                    let base = nd.file_stem(stem, s, w, t);
                    let found = ND_EXTENSIONS
                        .iter()
                        .find_map(|e| listing.get(&format!("{base}.{e}").to_lowercase()));
                    let (path, name) = if let Some(p) = found {
                        (p.clone(), file_name_of(p))
                    } else {
                        missing += 1;
                        let name = format!("{base}.TIF");
                        (dir.join(&name), name)
                    };
                    self.files.push(FileSetMember::pending(path, name, None));
                    member_of.insert((s, w, t), self.files.len() - 1);
                }
            }
        }
        (member_of, missing)
    }

    /// The first member that opens as a TIFF or STK with a decodable page 0: the geometry of
    /// the series.
    fn first_nd_member(&mut self, missing: usize) -> Result<FirstMember> {
        let mut found = None;
        for i in 1..self.files.len() {
            if self.open_member(i).is_ok()
                && let Some(t) = self.opened(i)
                && let Some(p0) = t.ifds.first()
                && let Ok(l) = PageLayout::from_ifd(p0, t.header.byte_order)
                && l.pixel_type().is_ok()
            {
                found = Some((i, p0.clone(), l));
                break;
            }
        }
        let Some((file, page0, layout)) = found else {
            return Err(Error::corrupt(
                FORMAT_ID,
                format!(
                    "none of the {} files this .nd names could be opened as a TIFF or STK ({missing} are missing)",
                    self.files.len() - 1
                ),
            ));
        };
        let stk = if crate::metamorph::is_stk(&page0) {
            self.files[file]
                .opened
                .as_mut()
                .and_then(|(_, src)| parse_stk(&page0, src))
        } else {
            None
        };
        let metaseries = page0
            .text(tags::IMAGE_DESCRIPTION)
            .and_then(parse_metaseries);
        Ok(FirstMember {
            file,
            page0,
            layout,
            stk,
            metaseries,
        })
    }

    /// Pixel type, Z planes per file (the `.nd` Z series, else the first file's planes) and
    /// calibration (STK, then MetaSeries, then the TIFF resolution tags).
    fn nd_geometry(&mut self, nd: &NdFile, first: &FirstMember) -> Result<NdGeometry> {
        let l = &first.layout;
        let pixel_type = l.pixel_type()?;
        let file_planes = first.stk.as_ref().map_or_else(
            || self.opened(first.file).map_or(1, |t| t.ifds.len() as u32),
            |s| s.plane_count,
        );
        let mut z_count = nd.z_steps;
        if z_count == 1 && file_planes > 1 {
            z_count = file_planes;
            self.notes.push(format!(
                "the .nd declares no Z series but '{}' holds {file_planes} planes; they are exposed as Z",
                self.files[first.file].name
            ));
        }
        let (mut px, mut py) = first
            .stk
            .as_ref()
            .map_or((None, None), StkInfo::pixel_size_um);
        if let Some(m) = &first.metaseries
            && px.is_none()
            && py.is_none()
        {
            (px, py) = m.pixel_size_um;
        }
        if px.is_none() && py.is_none() {
            (px, py) = resolution_um(&first.page0);
        }
        let pz = if z_count > 1 {
            nd.z_step_um
                .or_else(|| first.stk.as_ref().and_then(StkInfo::z_step_um))
        } else {
            None
        };
        Ok(NdGeometry {
            pixel_type,
            z_count,
            samples_per_pixel: if l.planar == 2 {
                1
            } else {
                u32::from(l.samples_per_pixel)
            },
            physical_size: PhysicalSize::micrometres(px, py, pz),
        })
    }
}

/// The image of stage position `s`: sizes, calibration, wavelength names as channels, and
/// the instrument and objective from the first member.
fn nd_image_info(
    nd: &NdFile,
    first: &FirstMember,
    geometry: &NdGeometry,
    s: usize,
    stem: &str,
) -> ImageInfo {
    let l = &first.layout;
    let n_c = nd.channel_count();
    let mut info = ImageInfo::new(s as u32, l.width, l.height, geometry.pixel_type);
    info.size_c = n_c as u32;
    info.size_z = geometry.z_count;
    info.size_t = nd.time_points;
    info.samples_per_pixel = geometry.samples_per_pixel;
    info.physical_size = geometry.physical_size.clone();
    info.name = nd
        .stages
        .get(s)
        .filter(|n| !n.is_empty())
        .cloned()
        .or_else(|| Some(stem.to_string()));
    info.acquired_at.clone_from(&nd.start_time);
    info.channels = (0..n_c)
        .map(|c| ChannelInfo {
            index: c as u32,
            name: nd.waves.get(c).filter(|n| !n.is_empty()).cloned(),
            ..ChannelInfo::default()
        })
        .collect();
    info.instrument = instrument_from_tags(&first.page0);
    if let Some(m) = &first.metaseries {
        if m.application.is_some() {
            let mut inst = info.instrument.clone().unwrap_or_default();
            inst.software.clone_from(&m.application);
            inst.software_version.clone_from(&m.application_version);
            info.instrument = Some(inst);
        }
        if m.objective.is_some() || m.objective_na.is_some() {
            info.objective = Some(ObjectiveInfo {
                model: m.objective.clone(),
                nominal_magnification: m
                    .objective
                    .as_deref()
                    .and_then(crate::metamorph::magnification),
                lens_na: m.objective_na,
                ..ObjectiveInfo::default()
            });
        }
    }
    if let Some(d) = &nd.description {
        info.extra.insert("description".into(), json!(d));
    }
    if !nd.stages.is_empty() {
        info.extra.insert("stage_index".into(), json!(s + 1));
    }
    info
}

/// The files of a directory by lower-case name (writers differ in the case of extensions).
fn dir_listing(fs: &Fs, dir: &Path) -> HashMap<String, PathBuf> {
    let mut listing = HashMap::new();
    if let Ok(rd) = fs.read_dir(dir) {
        for e in rd.flatten() {
            if e.metadata().is_ok_and(|m| m.is_file()) {
                let p = e.path();
                listing.insert(file_name_of(&p).to_lowercase(), p);
            }
        }
    }
    listing
}

/// Files in the listing that follow the series' naming but lie past its declared time points
/// (a time point written after the `.nd` was saved).
fn nd_extra_files(listing: &HashMap<String, PathBuf>, stem: &str, nd: &NdFile) -> usize {
    if !nd.timelapse {
        return 0;
    }
    let mut n = 0;
    for s in 0..nd.image_count() {
        for w in 0..nd.channel_count() {
            let base = nd.file_stem(stem, s, w, nd.time_points);
            if ND_EXTENSIONS
                .iter()
                .any(|e| listing.contains_key(&format!("{base}.{e}").to_lowercase()))
            {
                n += 1;
            }
        }
    }
    n
}
