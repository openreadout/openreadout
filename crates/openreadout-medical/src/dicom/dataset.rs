//! `Dataset` for DICOM files and folders: images, metadata, plane and region reads, checks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo, LsEntry, ObjectiveInfo,
    PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::region::{
    PlacedTile, Region, ResolutionLevel, TileCache, paste_into_region, region_buffer,
};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, PixelType, Plane, Result};
use serde_json::{Map, Value, json};

use super::dictionary::{DICOMDIR_CLASS, WSI_CLASS, sop_class_name};
use super::element::Lookup;
use super::file::{DicomFile, PixelLoc};
use super::pixels::{FrameTable, PixelDesc, decode_frame};
use super::series::{Arrangement, FrameRec, ImagePlan, plan};
use super::tags as t;
use super::values::{Modality, apply};
use super::wsi::{Slide, TileRef, flavour, slides};
use super::{DicomReader, FORMAT_ID, dicom_files_in, phi, vendor};
use crate::util::{lps_letter, num, plane_of_normal};

/// Most files read from one folder.
const MAX_FILES: usize = 200_000;

enum Img {
    Planes(Box<ImagePlan>),
    Slide(Slide),
}

/// An opened DICOM file or folder.
pub(crate) struct DicomDataset {
    path: PathBuf,
    is_dir: bool,
    files: Vec<DicomFile>,
    descs: Vec<Option<PixelDesc>>,
    tables: Vec<Option<FrameTable>>,
    images: Vec<Img>,
    /// Files without pixel data (structured reports, RT objects, presentation states).
    others: Vec<usize>,
    /// Files found but not read, with why.
    unreadable: Vec<(PathBuf, String)>,
    /// Files a DICOMDIR names that are missing.
    missing: Vec<PathBuf>,
    dicomdir: Option<PathBuf>,
    notes: Vec<String>,
    tiles: TileCache<(usize, u32), Vec<u8>>,
}

impl std::fmt::Debug for DicomDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DicomDataset")
            .field("path", &self.path)
            .field("files", &self.files.len())
            .field("images", &self.images.len())
            .finish_non_exhaustive()
    }
}

/// The referenced files of a DICOMDIR (PS3.3 F.3: Referenced File ID components relative to
/// the DICOMDIR's folder).
fn dicomdir_files(fs: &Fs, dicomdir: &DicomFile) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let base = dicomdir
        .path
        .parent()
        .unwrap_or(Path::new(""))
        .to_path_buf();
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for rec in dicomdir.ds.items(t::DIRECTORY_RECORD_SEQUENCE) {
        let Some(e) = rec.get(t::REFERENCED_FILE_ID) else {
            continue;
        };
        let parts = e.strings();
        if parts.is_empty()
            || parts
                .iter()
                .any(|p| p.is_empty() || p == ".." || p.contains('/'))
        {
            continue;
        }
        // resolve each component case-insensitively (media written in upper case)
        let mut p = base.clone();
        let mut ok = true;
        for part in &parts {
            if let Some(q) = fs.find_in_dir(&p, part) {
                p = q;
            } else {
                p = p.join(part);
                ok = false;
            }
        }
        if found.contains(&p) || missing.contains(&p) {
            continue;
        }
        if ok && fs.is_file(&p) {
            found.push(p);
        } else {
            missing.push(p);
        }
    }
    (found, missing)
}

fn is_dicomdir(f: &DicomFile) -> bool {
    f.sop_class() == DICOMDIR_CLASS || f.ds.get(t::DIRECTORY_RECORD_SEQUENCE).is_some()
}

/// Seconds of a TM value and a DA value as ISO-8601.
fn iso(date: Option<String>, time: Option<String>) -> Option<String> {
    let d = date?;
    let d = d.trim();
    if d.len() < 8 || !d.as_bytes()[..8].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut s = format!("{}-{}-{}", &d[0..4], &d[4..6], &d[6..8]);
    if let Some(tm) = time {
        let tm = tm.trim().replace(':', "");
        if tm.len() >= 4 && tm.as_bytes()[..4].iter().all(u8::is_ascii_digit) {
            let sec = tm.get(4..6).filter(|v| v.len() == 2).unwrap_or("00");
            s.push_str(&format!("T{}:{}:{sec}", &tm[0..2], &tm[2..4]));
            if let Some(frac) = tm.get(6..).filter(|f| f.starts_with('.') && f.len() > 1) {
                s.push_str(frac);
            }
        }
    }
    Some(s)
}

/// ISO-8601 from a DT value (`YYYYMMDDHHMMSS.FFFFFF&ZZXX`).
fn iso_dt(dt: &str) -> Option<String> {
    let dt = dt.trim();
    let (main, _) = dt.split_once(['+', '-']).unwrap_or((dt, ""));
    iso(
        main.get(0..8).map(str::to_string),
        main.get(8..).map(str::to_string),
    )
}

impl DicomDataset {
    pub(crate) fn open(input: &Input) -> Result<Self> {
        let fs = input.fs();
        let path = input.path().to_path_buf();
        let is_dir = fs.is_dir(&path);
        let mut notes = Vec::new();
        let mut missing = Vec::new();
        let mut dicomdir = None;
        let mut first: Option<DicomFile> = None;
        let paths: Vec<PathBuf> = if is_dir {
            // A DICOMDIR names the files of the media, but some writers list only part of
            // the folder: every DICOM file found is read, and the DICOMDIR's missing files
            // are reported.
            let dd = fs.find_in_dir(&path, "DICOMDIR");
            let mut found = dicom_files_in(fs, &path, MAX_FILES);
            if let Some(Ok(d)) = dd.as_ref().map(|p| DicomFile::open(fs, p))
                && is_dicomdir(&d)
            {
                let (listed, miss) = dicomdir_files(fs, &d);
                missing = miss;
                dicomdir = Some(d.path.clone());
                for p in listed {
                    if !found.contains(&p) {
                        found.push(p);
                    }
                }
            }
            found
        } else {
            let f = DicomFile::open(fs, &path)?;
            if is_dicomdir(&f) {
                let (listed, miss) = dicomdir_files(fs, &f);
                missing = miss;
                dicomdir = Some(f.path.clone());
                let dir = match path.parent() {
                    Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
                    _ => PathBuf::from("."),
                };
                let mut found = dicom_files_in(fs, &dir, MAX_FILES);
                for p in listed {
                    if !found.contains(&p) {
                        found.push(p);
                    }
                }
                found
            } else if f.sop_class() == WSI_CLASS {
                let series = f.ds.text(t::SERIES_INSTANCE_UID).unwrap_or_default();
                let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
                let dir = if dir.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    dir
                };
                let mut v = vec![path.clone()];
                if !series.is_empty() {
                    for p in dicom_files_in(fs, &dir, 4096) {
                        if p == path
                            || p.parent() != Some(dir.as_path()) && !dir.as_os_str().eq(".")
                        {
                            continue;
                        }
                        if let Ok(o) = DicomFile::open(fs, &p)
                            && o.ds.text(t::SERIES_INSTANCE_UID).as_deref() == Some(&series)
                            && o.sop_class() == WSI_CLASS
                        {
                            v.push(p);
                        }
                    }
                }
                first = Some(f);
                v
            } else {
                first = Some(f);
                vec![path.clone()]
            }
        };
        if paths.len() >= MAX_FILES {
            notes.push(format!(
                "only the first {MAX_FILES} DICOM files of the folder are read"
            ));
        }
        let mut files = Vec::new();
        let mut unreadable = Vec::new();
        for p in paths {
            let parsed = match first.take() {
                Some(f) if f.path == p => Ok(f),
                other => {
                    first = other;
                    DicomFile::open(fs, &p)
                }
            };
            match parsed {
                Ok(f) if is_dicomdir(&f) => {}
                Ok(f) => files.push(f),
                Err(e) => {
                    if !is_dir && dicomdir.is_none() && files.is_empty() && unreadable.is_empty() {
                        // the one file asked for
                        return Err(e);
                    }
                    unreadable.push((p, e.to_string()));
                }
            }
        }
        if files.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                if unreadable.is_empty() {
                    format!("{} holds no readable DICOM files", path.display())
                } else {
                    format!(
                        "none of the {} DICOM files could be read; first problem: {}",
                        unreadable.len(),
                        unreadable[0].1
                    )
                },
            ));
        }
        let mut descs = Vec::with_capacity(files.len());
        let mut others = Vec::new();
        for (i, f) in files.iter().enumerate() {
            if f.pixels.is_none() {
                others.push(i);
                descs.push(None);
                continue;
            }
            match PixelDesc::of(&f.ds, f.pixel_tag) {
                Ok(d) => descs.push(Some(d)),
                Err(e) => {
                    notes.push(format!("{}: {e}", f.path.display()));
                    others.push(i);
                    descs.push(None);
                }
            }
        }
        // whole-slide instances become pyramids; everything else is grouped into series
        let pairs: Vec<(DicomFile, PixelDesc)> = files
            .iter()
            .zip(&descs)
            .filter_map(|(f, d)| d.clone().map(|d| (f.clone(), d)))
            .collect();
        let index: Vec<usize> = descs
            .iter()
            .enumerate()
            .filter_map(|(i, d)| d.as_ref().map(|_| i))
            .collect();
        let wsi_local: Vec<usize> = pairs
            .iter()
            .enumerate()
            .filter(|(_, (f, _))| f.sop_class() == WSI_CLASS)
            .map(|(i, _)| i)
            .collect();
        let mut images = Vec::new();
        for mut s in slides(&pairs, &wsi_local) {
            s.first_file = index[s.first_file];
            for l in &mut s.levels {
                for f in &mut l.files {
                    *f = index[*f];
                }
                for tref in l.tiles.values_mut() {
                    tref.file = index[tref.file];
                }
            }
            images.push(Img::Slide(s));
        }
        let rest: Vec<(usize, (DicomFile, PixelDesc))> = pairs
            .into_iter()
            .enumerate()
            .filter(|(i, (f, _))| !(wsi_local.contains(i) && flavour(f) == "VOLUME"))
            .collect();
        let rest_index: Vec<usize> = rest.iter().map(|(i, _)| index[*i]).collect();
        let rest_pairs: Vec<(DicomFile, PixelDesc)> = rest.into_iter().map(|(_, p)| p).collect();
        for mut p in plan(&rest_pairs) {
            p.first_file = rest_index[p.first_file];
            for fr in p.planes.iter_mut().flatten() {
                fr.file = rest_index[fr.file];
            }
            images.push(Img::Planes(Box::new(p)));
        }
        if !missing.is_empty() {
            notes.push(format!(
                "the DICOMDIR names {} files that are missing (first: {})",
                missing.len(),
                missing[0].display()
            ));
        }
        if !unreadable.is_empty() {
            notes.push(format!(
                "{} files could not be read (first: {}: {})",
                unreadable.len(),
                unreadable[0].0.display(),
                unreadable[0].1
            ));
        }
        let tables = vec![None; files.len()];
        Ok(DicomDataset {
            path,
            is_dir,
            files,
            descs,
            tables,
            images,
            others,
            unreadable,
            missing,
            dicomdir,
            notes,
            tiles: TileCache::default(),
        })
    }

    fn rel(&self, p: &Path) -> String {
        let base = if self.is_dir {
            self.path.as_path()
        } else {
            self.path.parent().unwrap_or(Path::new(""))
        };
        p.strip_prefix(base)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Decode frame `frame` of file `fi` in the image's output type.
    fn frame(
        &mut self,
        fi: usize,
        frame: u32,
        modality: &Modality,
        out: PixelType,
        img: usize,
    ) -> Result<Vec<u8>> {
        let desc = self.descs[fi]
            .clone()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "file has no image pixel module"))?;
        let d = decode_frame(&self.files[fi], &desc, &mut self.tables[fi], frame)?;
        let palette = match &self.images[img] {
            Img::Planes(p) => p.palette.as_ref(),
            Img::Slide(_) => None,
        };
        if let Some(pal) = palette {
            return Ok(pal.apply(&d));
        }
        if d.spp == 3 {
            return Ok(d.data);
        }
        Ok(apply(&d, modality, out))
    }

    fn plane_type(&self, img: usize) -> (PixelType, u32) {
        match &self.images[img] {
            Img::Planes(p) => match &p.palette {
                Some(pal) => (
                    if pal.eight_bit {
                        PixelType::Uint8
                    } else {
                        PixelType::Uint16
                    },
                    3,
                ),
                None => (p.out_type, p.desc.spp),
            },
            Img::Slide(s) => (s.desc.stored_type(), s.desc.spp),
        }
    }

    fn image_info(&self, index: usize) -> ImageInfo {
        match &self.images[index] {
            Img::Planes(p) => self.planes_info(index, p),
            Img::Slide(s) => self.slide_info(index, s),
        }
    }

    /// Fields every image shares: names, instrument, acquisition, patient, privacy.
    fn common(&self, im: &mut ImageInfo, fi: usize) {
        let f = &self.files[fi];
        let ds = &f.ds;
        let modality = ds.text(t::MODALITY);
        let series = ds.text(t::SERIES_DESCRIPTION);
        let number = ds.number(t::SERIES_NUMBER);
        im.name = series.clone().or_else(|| {
            modality
                .as_ref()
                .map(|m| {
                    format!(
                        "{m} series {}",
                        number.map_or(String::new(), |n| n.to_string())
                    )
                })
                .map(|s| s.trim().to_string())
        });
        let software = ds
            .get(t::SOFTWARE_VERSIONS)
            .map(|e| e.strings().join(" "))
            .filter(|s| !s.trim().is_empty());
        im.instrument = Some(InstrumentInfo {
            manufacturer: ds.text(t::MANUFACTURER),
            model: ds.text(t::MANUFACTURER_MODEL_NAME),
            software: None,
            software_version: software,
            detector: None,
        })
        .filter(|i| i.manufacturer.is_some() || i.model.is_some() || i.software_version.is_some());
        im.acquired_at = ds
            .text(t::ACQUISITION_DATE_TIME)
            .and_then(|d| iso_dt(&d))
            .or_else(|| iso(ds.text(t::ACQUISITION_DATE), ds.text(t::ACQUISITION_TIME)))
            .or_else(|| iso(ds.text(t::SERIES_DATE), ds.text(t::SERIES_TIME)))
            .or_else(|| iso(ds.text(t::STUDY_DATE), ds.text(t::STUDY_TIME)))
            .or_else(|| iso(ds.text(t::CONTENT_DATE), ds.text(t::CONTENT_TIME)));
        let ex = &mut im.extra;
        let mut put = |k: &str, v: Option<Value>| {
            if let Some(v) = v.filter(|v| !v.is_null()) {
                ex.insert(k.into(), v);
            }
        };
        put("modality", modality.clone().map(Value::from));
        let class = f.sop_class();
        put(
            "sop_class",
            Some(Value::from(
                sop_class_name(&class).map_or(class.clone(), str::to_string),
            )),
        );
        put("series_description", series.map(Value::from));
        put("series_number", number.map(num));
        put(
            "study_description",
            ds.text(t::STUDY_DESCRIPTION).map(Value::from),
        );
        put("protocol_name", ds.text(t::PROTOCOL_NAME).map(Value::from));
        put("body_part", ds.text(t::BODY_PART_EXAMINED).map(Value::from));
        put(
            "transfer_syntax",
            Some(json!({"uid": f.syntax_uid, "name": f.syntax.name, "lossy": f.syntax.lossy})),
        );
        if ds.text(t::LOSSY_IMAGE_COMPRESSION).as_deref() == Some("01") {
            put("lossy_compression", Some(Value::Bool(true)));
        }
        // acquisition settings, in our units
        let mut acq = Map::new();
        for (k, tag, scale) in [
            ("kvp", t::KVP, 1.0),
            ("tube_current_ma", t::X_RAY_TUBE_CURRENT, 1.0),
            ("exposure_time_ms", t::EXPOSURE_TIME, 1.0),
            ("repetition_time_ms", t::REPETITION_TIME, 1.0),
            ("echo_time_ms", t::ECHO_TIME, 1.0),
            ("inversion_time_ms", t::INVERSION_TIME, 1.0),
            ("flip_angle_deg", t::FLIP_ANGLE, 1.0),
            ("field_strength_t", t::MAGNETIC_FIELD_STRENGTH, 1.0),
            ("slice_thickness_mm", t::SLICE_THICKNESS, 1.0),
        ] {
            if let Some(v) = ds.number(tag) {
                acq.insert(k.into(), num(v * scale));
            }
        }
        for (k, tag) in [
            ("convolution_kernel", t::CONVOLUTION_KERNEL),
            ("scanning_sequence", t::SCANNING_SEQUENCE),
        ] {
            if let Some(v) = ds
                .get(tag)
                .map(|e| e.strings().join("\\"))
                .filter(|s| !s.is_empty())
            {
                acq.insert(k.into(), Value::from(v));
            }
        }
        if let Some(r) = ds
            .first(t::RADIOPHARMACEUTICAL_INFORMATION_SEQUENCE)
            .and_then(|i| i.text(t::RADIOPHARMACEUTICAL))
        {
            acq.insert("radiopharmaceutical".into(), Value::from(r));
        }
        if !acq.is_empty() {
            ex.insert("acquisition".into(), Value::Object(acq));
        }
        // the subject, without identifying values
        let mut subject = Map::new();
        if let Some(s) = ds.text(t::PATIENT_SEX) {
            subject.insert("sex".into(), Value::from(s));
        }
        if let Some(a) = ds.text(t::PATIENT_AGE) {
            subject.insert("age".into(), Value::from(a));
        }
        if let Some(w) = ds.number(t::PATIENT_WEIGHT).filter(|v| *v > 0.0) {
            subject.insert("weight_kg".into(), num(w));
        }
        if let Some(h) = ds.number(t::PATIENT_SIZE).filter(|v| *v > 0.0) {
            subject.insert("height_m".into(), num(h));
        }
        if let Some(s) = ds.text(t::PATIENT_SPECIES_DESCRIPTION) {
            subject.insert("species".into(), Value::from(s));
        }
        if !subject.is_empty() {
            ex.insert("subject".into(), Value::Object(subject));
        }
        let flags = phi::flags(ds);
        if !flags.is_empty() {
            ex.insert("personal_data".into(), Value::Array(flags));
        }
        if phi::deidentified(ds) {
            ex.insert("deidentified".into(), Value::Bool(true));
        }
    }

    #[allow(clippy::float_cmp)]
    fn planes_info(&self, index: usize, p: &ImagePlan) -> ImageInfo {
        let (ty, spp) = self.plane_type(index);
        let mut im = ImageInfo::new(index as u32, p.desc.cols, p.desc.rows, ty);
        im.samples_per_pixel = spp;
        im.size_z = p.size_z;
        im.size_t = p.size_t;
        im.size_c = p.size_c;
        let first = p.planes.iter().flatten().next();
        let spacing = first.and_then(|f| f.spacing);
        let mut z = p.z_step_mm;
        if p.size_z <= 1 {
            z = None;
        }
        let ds = &self.files[p.first_file].ds;
        let mut spacing_source = "PixelSpacing";
        let spacing = spacing.or_else(|| {
            let v = ds.numbers(t::IMAGER_PIXEL_SPACING);
            (v.len() >= 2 && v[0] > 0.0 && v[1] > 0.0).then(|| {
                spacing_source = "ImagerPixelSpacing";
                [v[0], v[1]]
            })
        });
        let spacing = spacing.or_else(|| {
            // ultrasound: the first region with cm units (3) in both directions
            ds.items(t::SEQUENCE_OF_ULTRASOUND_REGIONS)
                .iter()
                .find_map(|r| {
                    let ux = r.number(t::PHYSICAL_UNITS_X_DIRECTION)?;
                    let uy = r.number(t::PHYSICAL_UNITS_Y_DIRECTION)?;
                    let dx = r.number(t::PHYSICAL_DELTA_X)?;
                    let dy = r.number(t::PHYSICAL_DELTA_Y)?;
                    (ux == 3.0 && uy == 3.0 && dx > 0.0 && dy > 0.0).then(|| {
                        spacing_source = "SequenceOfUltrasoundRegions";
                        [dy * 10.0, dx * 10.0]
                    })
                })
        });
        im.physical_size = PhysicalSize::micrometres(
            spacing.map(|s| s[1] * 1000.0),
            spacing.map(|s| s[0] * 1000.0),
            z.map(|v| v * 1000.0),
        );
        im.time_increment_s = p.time_step_s.filter(|v| *v > 0.0 && p.size_t > 1);
        let modality = ds.text(t::MODALITY).unwrap_or_default();
        im.channels = if p.channel_names.is_empty() {
            vec![ChannelInfo {
                index: 0,
                name: Some(if p.desc.is_colour() {
                    "RGB".into()
                } else {
                    modality.clone()
                })
                .filter(|s| !s.is_empty()),
                ..ChannelInfo::default()
            }]
        } else {
            p.channel_names
                .iter()
                .enumerate()
                .map(|(i, n)| ChannelInfo {
                    index: i as u32,
                    name: Some(n.clone()),
                    ..ChannelInfo::default()
                })
                .collect()
        };
        self.common(&mut im, p.first_file);
        if self.files[p.first_file].sop_class() == WSI_CLASS {
            // a slide's label, overview or thumbnail image
            im.name = Some(flavour(&self.files[p.first_file]).to_ascii_lowercase());
        }
        let ex = &mut im.extra;
        ex.insert("arrangement".into(), Value::from(p.arrangement.label()));
        let nfiles = {
            let mut v: Vec<usize> = p.planes.iter().flatten().map(|f| f.file).collect();
            v.sort_unstable();
            v.dedup();
            v.len()
        };
        ex.insert("files".into(), Value::from(nfiles));
        ex.insert(
            "photometric_interpretation".into(),
            Value::from(p.desc.photometric.clone()),
        );
        ex.insert(
            "stored_pixel_type".into(),
            Value::from(p.desc.stored_type().ome_name()),
        );
        ex.insert("bits_stored".into(), Value::from(p.desc.bits_stored));
        if spacing.is_some() && spacing_source != "PixelSpacing" {
            ex.insert("pixel_spacing_from".into(), Value::from(spacing_source));
        }
        if let Some(src) = p.z_step_source.filter(|_| im.physical_size.z.is_some()) {
            ex.insert("z_step_from".into(), Value::from(src));
        }
        // value transform
        let mods: Vec<&Modality> = p.planes.iter().flatten().map(|f| &f.modality).collect();
        let distinct = {
            let mut d: Vec<&Modality> = Vec::new();
            for m in &mods {
                if !d.contains(m) {
                    d.push(m);
                }
            }
            d
        };
        let rescale_type = ds.text(t::RESCALE_TYPE);
        match distinct.as_slice() {
            [Modality::Linear { slope, intercept }] => {
                ex.insert(
                    "rescale".into(),
                    json!({"slope": num(*slope), "intercept": num(*intercept), "applied": true}),
                );
            }
            [Modality::Lut { first, table, bits }] => {
                ex.insert(
                    "rescale".into(),
                    json!({"lut_entries": table.len(), "lut_first_value": first, "lut_bits": bits, "applied": true}),
                );
            }
            [Modality::Identity] | [] => {}
            many => {
                ex.insert(
                    "rescale".into(),
                    json!({"varies": true, "transforms": many.len(), "applied": true}),
                );
            }
        }
        let unit = match modality.as_str() {
            "CT" => Some("HU".to_string()),
            "PT" => ds.text(t::UNITS),
            "RTDOSE" => ds.text(t::DOSE_UNITS),
            _ => None,
        }
        .or(rescale_type.filter(|r| r != "US"));
        if let Some(u) = unit.filter(|_| !p.desc.is_colour()) {
            ex.insert("value_unit".into(), Value::from(u));
        }
        // display windows (in output units)
        let mut windows = Vec::new();
        let centres = ds.numbers(t::WINDOW_CENTER);
        let widths = ds.numbers(t::WINDOW_WIDTH);
        let names = ds
            .get(t::WINDOW_CENTER_WIDTH_EXPLANATION)
            .map(super::parse::Element::strings)
            .unwrap_or_default();
        for (i, (c, w)) in centres.iter().zip(&widths).enumerate() {
            if *w > 0.0 {
                let mut o = json!({"center": num(*c), "width": num(*w)});
                if let Some(n) = names.get(i).filter(|n| !n.is_empty()) {
                    o["name"] = Value::from(n.clone());
                }
                windows.push(o);
            }
        }
        if windows.is_empty()
            && let Some((c, w)) = first.and_then(|f| f.window)
        {
            windows.push(json!({"center": num(c), "width": num(w)}));
        }
        if !windows.is_empty() {
            ex.insert("windows".into(), Value::Array(windows));
        }
        if p.desc.photometric == "MONOCHROME1" {
            ex.insert("display_inverted".into(), Value::Bool(true));
        }
        if let Some(o) = first.and_then(|f| f.orientation) {
            let row = [o[0], o[1], o[2]];
            let col = [o[3], o[4], o[5]];
            let nrm = p.normal.unwrap_or([0.0, 0.0, 1.0]);
            let mut orient = json!({
                "row_direction": [num(o[0]), num(o[1]), num(o[2])],
                "column_direction": [num(o[3]), num(o[4]), num(o[5])],
                "plane": plane_of_normal(nrm),
                "columns_toward": lps_letter(row).to_string(),
                "rows_toward": lps_letter(col).to_string(),
            });
            if p.size_z > 1 {
                orient["slices_toward"] = Value::from(lps_letter(nrm).to_string());
            }
            if let Some(pos) = first.and_then(|f| f.position) {
                orient["first_position_mm"] = json!([num(pos[0]), num(pos[1]), num(pos[2])]);
            }
            ex.insert("orientation".into(), orient);
        }
        im.finish()
    }

    fn slide_info(&self, index: usize, s: &Slide) -> ImageInfo {
        let (ty, spp) = self.plane_type(index);
        let l0 = &s.levels[0];
        let mut im = ImageInfo::new(index as u32, l0.width, l0.height, ty);
        im.samples_per_pixel = spp;
        im.physical_size = PhysicalSize::micrometres(
            l0.spacing.map(|v| v[1] * 1000.0),
            l0.spacing.map(|v| v[0] * 1000.0),
            None,
        );
        im.pyramid_levels = s.levels.len() as u32;
        im.resolution_levels = s
            .levels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                ResolutionLevel::new(i as u32, l.width, l.height, l0.width, l0.height)
                    .with_tile(l.tile_w, l.tile_h)
            })
            .collect();
        im.channels = vec![ChannelInfo {
            index: 0,
            name: Some(if spp == 3 {
                "RGB".into()
            } else {
                "gray".into()
            }),
            acquisition_mode: Some("Brightfield".into()),
            ..ChannelInfo::default()
        }];
        self.common(&mut im, s.first_file);
        let ds = &self.files[s.first_file].ds;
        im.name = ds
            .text(t::SERIES_DESCRIPTION)
            .or_else(|| ds.text(t::CONTAINER_IDENTIFIER))
            .or_else(|| Some("slide".into()));
        if let Some(op) = ds.first(t::OPTICAL_PATH_SEQUENCE) {
            let power = op.number(t::OBJECTIVE_LENS_POWER);
            let na = op.number(t::OBJECTIVE_LENS_NUMERICAL_APERTURE);
            if power.is_some() || na.is_some() {
                im.objective = Some(ObjectiveInfo {
                    nominal_magnification: power,
                    lens_na: na,
                    ..ObjectiveInfo::default()
                });
            }
            if let Some(w) = op.number(t::ILLUMINATION_WAVE_LENGTH)
                && let Some(c) = im.channels.first_mut()
            {
                c.excitation_nm = Some(w);
            }
        }
        let ex = &mut im.extra;
        ex.insert("arrangement".into(), Value::from("slide pyramid"));
        ex.insert(
            "dimension_organization".into(),
            Value::from(l0.organization.clone()),
        );
        ex.insert(
            "photometric_interpretation".into(),
            Value::from(s.desc.photometric.clone()),
        );
        if let Some(c) = ds.text(t::CONTAINER_IDENTIFIER) {
            ex.insert("container_identifier".into(), Value::from(c));
        }
        if let Some(sp) = ds.first(t::SPECIMEN_DESCRIPTION_SEQUENCE) {
            let mut m = Map::new();
            if let Some(v) = sp.text(t::SPECIMEN_IDENTIFIER) {
                m.insert("identifier".into(), Value::from(v));
            }
            if let Some(v) = sp.text(t::SPECIMEN_SHORT_DESCRIPTION) {
                m.insert("description".into(), Value::from(v));
            }
            if !m.is_empty() {
                ex.insert("specimen".into(), Value::Object(m));
            }
        }
        let levels: Vec<Value> = s
            .levels
            .iter()
            .map(|l| {
                json!({
                    "files": l.files.iter().map(|f| Value::from(self.rel(&self.files[*f].path))).collect::<Vec<_>>(),
                    "tiles_stored": l.tiles.len(),
                    "tiles_in_grid": u64::from(l.grid().0) * u64::from(l.grid().1),
                })
            })
            .collect();
        ex.insert("level_files".into(), Value::Array(levels));
        im.finish()
    }

    fn plan_at(&self, image: u32) -> Result<&Img> {
        self.images.get(image as usize).ok_or_else(|| {
            Error::Usage(format!(
                "image index {image} out of range ({} images; `openreadout info` lists them)",
                self.images.len()
            ))
        })
    }

    fn plane_frame(&self, image: u32, idx: PlaneIndex) -> Result<(Option<FrameRec>, PixelType)> {
        match self.plan_at(image)? {
            Img::Planes(p) => {
                if idx.c >= p.size_c || idx.z >= p.size_z || idx.t >= p.size_t {
                    return Err(Error::Usage(format!(
                        "plane c={} z={} t={} out of range for image {image} (c<{}, z<{}, t<{})",
                        idx.c, idx.z, idx.t, p.size_c, p.size_z, p.size_t
                    )));
                }
                let i = (idx.c as usize * p.size_t as usize + idx.t as usize) * p.size_z as usize
                    + idx.z as usize;
                Ok((
                    p.planes.get(i).cloned().flatten(),
                    self.plane_type(image as usize).0,
                ))
            }
            Img::Slide(_) => Err(Error::Other("slide".into())),
        }
    }

    /// Decode (or take from the cache) one tile of a slide.
    fn tile(&mut self, image: usize, tref: TileRef) -> Result<Arc<Vec<u8>>> {
        if let Some(v) = self.tiles.get(&(tref.file, tref.frame)) {
            return Ok(v);
        }
        let ty = self.plane_type(image).0;
        let v = Arc::new(self.frame(tref.file, tref.frame, &Modality::Identity, ty, image)?);
        let n = v.len();
        self.tiles.put((tref.file, tref.frame), v.clone(), n);
        Ok(v)
    }

    fn slide_region(&mut self, image: u32, level: u32, region: Region) -> Result<Plane> {
        let Img::Slide(s) = self.plan_at(image)? else {
            return Err(Error::Other("not a slide".into()));
        };
        let l = s.levels.get(level as usize).ok_or_else(|| {
            Error::Usage(format!(
                "level {level} out of range (image {image} has {} levels)",
                s.levels.len()
            ))
        })?;
        region.check_within(l.width, l.height, &format!("image {image} level {level}"))?;
        let (tw, th) = (l.tile_w, l.tile_h);
        let (gx, gy) = l.grid();
        let x0 = region.x / tw;
        let y0 = region.y / th;
        let x1 = ((region.right() - 1) / u64::from(tw)).min(u64::from(gx) - 1) as u32;
        let y1 = ((region.bottom() - 1) / u64::from(th)).min(u64::from(gy) - 1) as u32;
        let mut wanted = Vec::new();
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                if let Some(r) = l.tiles.get(&(tx, ty)) {
                    wanted.push((tx, ty, *r));
                }
            }
        }
        let (pt, spp) = self.plane_type(image as usize);
        let bpp = pt.bytes_per_sample() * spp as usize;
        let mut out = region_buffer(FORMAT_ID, region, bpp)?;
        for (tx, ty, r) in wanted {
            let data = self.tile(image as usize, r)?;
            paste_into_region(
                &mut out,
                region,
                &PlacedTile {
                    data: &data,
                    row_bytes: tw as usize * bpp,
                    x: i64::from(tx) * i64::from(tw),
                    y: i64::from(ty) * i64::from(th),
                    width: tw,
                    height: th,
                },
                bpp,
            );
        }
        Ok(Plane {
            width: region.width,
            height: region.height,
            pixel_type: pt,
            samples_per_pixel: spp,
            data: out,
        })
    }

    fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.file_len).sum()
    }
}

impl Dataset for DicomDataset {
    fn info(&self) -> Result<FileInfo> {
        let images: Vec<ImageInfo> = (0..self.images.len()).map(|i| self.image_info(i)).collect();
        let mut notes = self.notes.clone();
        for img in &self.images {
            match img {
                Img::Planes(p) => notes.extend(p.notes.iter().cloned()),
                Img::Slide(s) => notes.extend(s.notes.iter().cloned()),
            }
        }
        if !self.others.is_empty() {
            let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
            for &i in &self.others {
                let c = self.files[i].sop_class();
                *kinds
                    .entry(sop_class_name(&c).map_or(c, str::to_string))
                    .or_default() += 1;
            }
            let list: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} x{n}")).collect();
            notes.push(format!(
                "{} files hold no image pixel data ({}); their headers are in `info --view full`",
                self.others.len(),
                list.join(", ")
            ));
        }
        if images.iter().any(|i| i.extra.contains_key("personal_data")) {
            notes.push("patient-identifying values (names, IDs, birth dates, addresses) are withheld; extra.personal_data lists which elements hold them".into());
        }
        if images.iter().any(|i| i.extra.contains_key("rescale")) {
            notes.push("pixel values have the modality rescale applied (stored x slope + intercept): CT values are Hounsfield units; extra.stored_pixel_type is the type as stored".into());
        }
        let problems: usize = self.files.iter().map(|f| f.problems.len()).sum();
        if problems > 0
            && let Some(p) = self.files.iter().flat_map(|f| f.problems.iter()).next()
        {
            notes.push(format!(
                "{problems} encoding problems were tolerated (first: {p}); `check` lists them"
            ));
        }
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.total_size(),
            format: DicomReader.descriptor(),
            format_version: None,
            plane_count,
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let tree_of = |f: &DicomFile| {
            json!({
                "file": self.rel(&f.path),
                "meta": vendor::tree(&f.meta),
                "dataset": vendor::tree(&f.ds),
            })
        };
        if self.files.len() == 1 {
            return Ok(tree_of(&self.files[0]));
        }
        // one header per image (its first file) and per file without pixels
        let mut list = Vec::new();
        for img in &self.images {
            let fi = match img {
                Img::Planes(p) => p.first_file,
                Img::Slide(s) => s.first_file,
            };
            list.push(tree_of(&self.files[fi]));
        }
        for &i in self.others.iter().take(32) {
            list.push(tree_of(&self.files[i]));
        }
        Ok(json!({"files": self.files.len(), "headers": list}))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "images[].size_x",
            "images[].size_y",
            "images[].pixel_type",
            "images[].samples_per_pixel",
            "images[].physical_size",
            "images[].acquired_at",
            "images[].instrument",
            "images[].name",
            "images[].objective",
            "images[].resolution_levels",
            "images[].extra.modality",
            "images[].extra.rescale",
            "images[].extra.windows",
            "images[].extra.orientation",
            "images[].extra.acquisition",
            "images[].extra.personal_data",
            "images[].extra.transfer_syntax",
        ] {
            p.insert(k.into(), Source::Spec);
        }
        for k in [
            "images[].size_z",
            "images[].size_t",
            "images[].time_increment_s",
            "images[].extra.arrangement",
            "images[].extra.value_unit",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        if let Some(d) = &self.dicomdir {
            out.push(LsEntry {
                kind: "metadata".into(),
                name: self.rel(d),
                offset: None,
                size: None,
                image: None,
                details: json!({"role": "DICOMDIR"}),
            });
        }
        let image_of = |fi: usize| -> Option<u32> {
            self.images
                .iter()
                .position(|img| match img {
                    Img::Planes(p) => p.planes.iter().flatten().any(|f| f.file == fi),
                    Img::Slide(s) => s.levels.iter().any(|l| l.files.contains(&fi)),
                })
                .map(|i| i as u32)
        };
        for (i, f) in self.files.iter().enumerate() {
            let class = f.sop_class();
            let mut details = json!({
                "sop_class": sop_class_name(&class).unwrap_or(class.as_str()),
                "transfer_syntax": f.syntax.name,
            });
            if let Some(d) = &self.descs[i] {
                details["rows"] = Value::from(d.rows);
                details["columns"] = Value::from(d.cols);
                details["frames"] = Value::from(d.frames);
            }
            out.push(LsEntry {
                kind: "file".into(),
                name: self.rel(&f.path),
                offset: None,
                size: Some(f.file_len),
                image: image_of(i),
                details,
            });
            if let Some(px) = f.pixels {
                let (offset, size, enc) = match px {
                    PixelLoc::Native { offset, len } => (offset, Some(len), false),
                    PixelLoc::Encapsulated { offset } => (offset, None, true),
                };
                out.push(LsEntry {
                    kind: "image".into(),
                    name: format!("{} pixel data", self.rel(&f.path)),
                    offset: (!f.syntax.deflated).then_some(offset),
                    size,
                    image: image_of(i),
                    details: json!({"encapsulated": enc}),
                });
            }
        }
        for (p, why) in &self.unreadable {
            out.push(LsEntry {
                kind: "unreadable".into(),
                name: self.rel(p),
                offset: None,
                size: None,
                image: None,
                details: json!({"error": why}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        if let Img::Slide(s) = self.plan_at(image)? {
            if index != PlaneIndex::default() {
                return Err(Error::Usage(format!(
                    "plane c={} z={} t={} out of range: a slide has one plane",
                    index.c, index.z, index.t
                )));
            }
            let (w, h) = (s.levels[0].width, s.levels[0].height);
            return self.slide_region(image, 0, Region::full(w, h));
        }
        let (fr, ty) = self.plane_frame(image, index)?;
        let (_, spp) = self.plane_type(image as usize);
        let Img::Planes(p) = &self.images[image as usize] else {
            return Err(Error::Other("not a plane image".into()));
        };
        let (w, h) = (p.desc.cols, p.desc.rows);
        let Some(fr) = fr else {
            // a segment with no frame at this slice
            let n = openreadout_core::pixel::plane_bytes_checked(
                FORMAT_ID,
                w,
                h,
                ty.bytes_per_sample() * spp as usize,
            )?;
            return Ok(Plane {
                width: w,
                height: h,
                pixel_type: ty,
                samples_per_pixel: spp,
                data: vec![0; n],
            });
        };
        let data = self.frame(fr.file, fr.frame, &fr.modality, ty, image as usize)?;
        Ok(Plane {
            width: w,
            height: h,
            pixel_type: ty,
            samples_per_pixel: spp,
            data,
        })
    }

    fn read_plane_level(&mut self, image: u32, index: PlaneIndex, level: u32) -> Result<Plane> {
        match self.plan_at(image)? {
            Img::Slide(s) => {
                let l = s.levels.get(level as usize).ok_or_else(|| {
                    Error::Usage(format!(
                        "level {level} out of range ({} levels)",
                        s.levels.len()
                    ))
                })?;
                let (w, h) = (l.width, l.height);
                if index != PlaneIndex::default() {
                    return Err(Error::Usage("a slide has one plane (c=0, z=0, t=0)".into()));
                }
                self.slide_region(image, level, Region::full(w, h))
            }
            Img::Planes(_) if level == 0 => self.read_plane(image, index),
            Img::Planes(_) => Err(Error::Usage(format!(
                "level {level} out of range: image {image} has one resolution level"
            ))),
        }
    }

    fn read_region(
        &mut self,
        image: u32,
        index: PlaneIndex,
        level: u32,
        region: Region,
    ) -> Result<Plane> {
        if matches!(self.plan_at(image)?, Img::Slide(_)) {
            if index != PlaneIndex::default() {
                return Err(Error::Usage("a slide has one plane (c=0, z=0, t=0)".into()));
            }
            return self.slide_region(image, level, region);
        }
        let plane = self.read_plane_level(image, index, level)?;
        openreadout_core::region::crop(plane, region, &format!("image {image} level {level}"))
    }

    #[allow(clippy::many_single_char_names)]
    fn stored_areas(
        &mut self,
        image: u32,
        _index: PlaneIndex,
        level: u32,
        region: Option<Region>,
    ) -> Result<Option<Vec<Region>>> {
        let Img::Slide(s) = self.plan_at(image)? else {
            return Ok(None);
        };
        let Some(l) = s.levels.get(level as usize) else {
            return Ok(None);
        };
        let (gx, gy) = l.grid();
        if l.tiles.len() as u64 == u64::from(gx) * u64::from(gy) {
            return Ok(None);
        }
        let r = region.unwrap_or(Region::full(l.width, l.height));
        let mut out = Vec::new();
        for &(tx, ty) in l.tiles.keys() {
            let (x, y) = (
                i64::from(tx) * i64::from(l.tile_w),
                i64::from(ty) * i64::from(l.tile_h),
            );
            if let Some((x0, y0, x1, y1)) =
                r.overlap(x, y, u64::from(l.tile_w), u64::from(l.tile_h))
            {
                out.push(Region::new(
                    (x0 - i64::from(r.x)) as u32,
                    (y0 - i64::from(r.y)) as u32,
                    (x1 - x0) as u32,
                    (y1 - y0) as u32,
                ));
            }
        }
        out.sort_by_key(|r| (r.y, r.x));
        Ok(Some(out))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        r.performed("the first and last frame of every image decode");
        for i in 0..self.images.len() as u32 {
            let planes: Vec<PlaneIndex> = match &self.images[i as usize] {
                Img::Planes(p) => {
                    let mut v = vec![PlaneIndex::default()];
                    if p.size_z * p.size_t > 1 {
                        v.push(PlaneIndex {
                            c: 0,
                            z: p.size_z - 1,
                            t: p.size_t - 1,
                        });
                    }
                    v
                }
                Img::Slide(_) => Vec::new(),
            };
            for idx in planes {
                if let Err(e) = self.read_plane(i, idx) {
                    r.push(Finding::error(
                        "undecodable_frame",
                        format!("image {i} plane z={} t={}: {e}", idx.z, idx.t),
                    ));
                }
            }
            if let Img::Slide(s) = &self.images[i as usize] {
                let l = s
                    .levels
                    .last()
                    .map(|l| (l.width, l.height, s.levels.len() as u32 - 1));
                if let Some((w, h, lv)) = l
                    && let Err(e) = self.slide_region(i, lv, Region::full(w, h))
                {
                    r.push(Finding::error(
                        "undecodable_frame",
                        format!("image {i} level {lv}: {e}"),
                    ));
                }
            }
        }
        Ok(r)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("preamble and DICM prefix, file meta group and transfer syntax");
        r.performed("every data element and sequence parses to the pixel data");
        r.performed("native pixel data holds every frame; encapsulated fragments are complete");
        r.performed("slices of a series are evenly spaced (no missing slices)");
        for (p, why) in &self.unreadable {
            r.push(Finding::error(
                "unreadable_file",
                format!("{}: {why}", self.rel(p)),
            ));
        }
        for p in &self.missing {
            r.push(Finding::error(
                "missing_file",
                format!("the DICOMDIR names {}, which is missing", self.rel(p)),
            ));
        }
        for (i, f) in self.files.iter().enumerate() {
            let name = self.rel(&f.path);
            if !f.preamble {
                r.push(Finding::warning(
                    "no_preamble",
                    format!("{name}: no 128-byte preamble and 'DICM' (not a Part 10 file)"),
                ));
            }
            for p in &f.problems {
                if !p.starts_with("no 128-byte preamble") {
                    r.push(Finding::warning("encoding", format!("{name}: {p}")));
                }
            }
            if f.syntax.uid.is_empty() {
                r.push(Finding::warning(
                    "unknown_transfer_syntax",
                    format!(
                        "{name}: transfer syntax {} is not one PS3.6 lists",
                        f.syntax_uid
                    ),
                ));
            }
            let Some(desc) = &self.descs[i] else { continue };
            match f.pixels {
                Some(PixelLoc::Native { len, .. }) => {
                    let need = (desc.native_frame_bits() * u64::from(desc.frames)).div_ceil(8);
                    if len < need {
                        r.push(Finding::error(
                            "truncated",
                            format!(
                                "{name}: pixel data holds {len} bytes, {} frames need {need}",
                                desc.frames
                            ),
                        ));
                    } else if len > need + 1 {
                        r.push(Finding::warning(
                            "trailing_bytes",
                            format!(
                                "{name}: pixel data holds {} bytes more than {} frames need",
                                len - need,
                                desc.frames
                            ),
                        ));
                    }
                }
                Some(PixelLoc::Encapsulated { .. }) => match FrameTable::build(f, desc.frames) {
                    Ok(tb) if !tb.complete => r.push(Finding::error(
                        "truncated",
                        format!(
                            "{name}: encapsulated pixel data has no sequence delimiter (truncated?)"
                        ),
                    )),
                    Ok(_) => {}
                    Err(e) => r.push(Finding::error("bad_fragments", format!("{name}: {e}"))),
                },
                None => {}
            }
        }
        for (i, img) in self.images.iter().enumerate() {
            if let Img::Planes(p) = img
                && p.arrangement == Arrangement::Volume
                && let Some(step) = p.z_step_mm
                && let Some(n) = p.normal
            {
                let d: Vec<f64> = p
                    .planes
                    .iter()
                    .flatten()
                    .filter_map(|f| f.position.map(|q| crate::util::dot(q, n)))
                    .collect();
                let gaps = d
                    .windows(2)
                    .filter(|w| (w[1] - w[0]).abs() > 1.5 * step)
                    .count();
                if gaps > 0 {
                    r.push(Finding::warning(
                        "missing_slices",
                        format!("image {i}: {gaps} gaps between slices wider than 1.5 x the {step:.3} mm step (missing files?)"),
                    ));
                }
            }
        }
        Ok(r)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self
            .files
            .iter()
            .map(|f| f.path.clone())
            .filter(|p| *p != self.path)
            .collect();
        if let Some(d) = &self.dicomdir
            && *d != self.path
        {
            v.push(d.clone());
        }
        v
    }

    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        let Some(Img::Planes(p)) = self.images.get(image as usize) else {
            return Ok((0, Vec::new()));
        };
        let total = p.planes.len() as u64;
        let take = limit.map_or(p.planes.len(), |l| l.min(p.planes.len()));
        let sz = p.size_z.max(1) as usize;
        let st = p.size_t.max(1) as usize;
        let recs = p.planes[..take]
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.as_ref().map(|f| (i, f)))
            .map(|(i, f)| {
                let mut m = Map::new();
                m.insert("z".into(), Value::from(i % sz));
                m.insert("t".into(), Value::from((i / sz) % st));
                if p.size_c > 1 {
                    m.insert("c".into(), Value::from(i / sz / st));
                }
                m.insert(
                    "file".into(),
                    Value::from(self.rel(&self.files[f.file].path)),
                );
                if self.descs[f.file].as_ref().is_some_and(|d| d.frames > 1) {
                    m.insert("frame".into(), Value::from(f.frame));
                }
                if let Some(n) = f.instance {
                    m.insert("instance_number".into(), num(n));
                }
                if let Some(pos) = f.position {
                    m.insert(
                        "position_mm".into(),
                        json!([num(pos[0]), num(pos[1]), num(pos[2])]),
                    );
                }
                if let Some(s) = f.slice_location {
                    m.insert("slice_location_mm".into(), num(s));
                }
                if let Some(ts) = f.time_s {
                    m.insert("acquisition_time_s".into(), num(ts));
                }
                if let Some(tt) = f.trigger_ms {
                    m.insert("trigger_time_ms".into(), num(tt));
                }
                if let Modality::Linear { slope, intercept } = f.modality {
                    m.insert("rescale_slope".into(), num(slope));
                    m.insert("rescale_intercept".into(), num(intercept));
                }
                Value::Object(m)
            })
            .collect();
        Ok((total, recs))
    }
}
