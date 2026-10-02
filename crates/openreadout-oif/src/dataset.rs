//! `Dataset` for Olympus FluoView OIF/OIB data sets: one TIFF per plane (`s_C001Z002T003.tif`),
//! the plane's axis indices in its file name, a `.pty` property file per plane and the main
//! `.oif` settings file for the whole acquisition. Layout: `docs/formats/oif.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    AttachmentInfo, ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, InstrumentInfo,
    LsEntry, ObjectiveInfo, PhysicalSize,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::Input;
use openreadout_core::{Error, PixelType, Plane, Result};
use openreadout_tiff::{ByteSource, PageLayout, SampleSelect, TiffFile, read_page};
use serde_json::{Value, json};

use crate::settings::Settings;
use crate::store::{Member, Store, StoreKind};
use crate::{OibReader, OifReader};

/// Largest TIFF member read into memory (one plane plus its header).
const MAX_TIFF_BYTES: u64 = 4 << 30;

/// Axis letters of plane file names that map onto the normalized model.
pub const AXIS_CHANNEL: char = 'C';
/// Focal plane index.
pub const AXIS_Z: char = 'Z';
/// Time point index.
pub const AXIS_TIME: char = 'T';
/// Emission wavelength (lambda) index; folded into the channel axis.
pub const AXIS_LAMBDA: char = 'L';
/// Suffix letter of reference images (`s_C001-R001.tif`).
pub const REFERENCE_MARK: char = 'R';

/// The indices a plane file name carries: `s_C001Z002.tif` → `[('C', 1), ('Z', 2)]`;
/// `s_C001-R001.tif` → `[('C', 1)]` with `reference = Some(1)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaneName {
    /// `(axis letter, one-based index)` in name order.
    pub axes: Vec<(char, u32)>,
    /// Number of the `-R###` suffix of reference images.
    pub reference: Option<u32>,
}

fn parse_indices(s: &str) -> Option<Vec<(char, u32)>> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if !c.is_ascii_alphabetic() {
            return None;
        }
        let start = i + 1;
        let mut end = start;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        if end == start {
            return None;
        }
        out.push((
            char::from(c.to_ascii_uppercase()),
            s[start..end].parse().ok()?,
        ));
        i = end;
    }
    (!out.is_empty()).then_some(out)
}

/// Does the file name end in one of these extensions (case-insensitive)?
fn ext_is(name: &str, exts: &[&str]) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, e)| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Parse a plane file name (`s_` prefix, axis letters with numbers, optional `-R###`).
pub fn parse_plane_name(name: &str) -> Option<PlaneName> {
    if !ext_is(name, &["tif", "tiff"]) {
        return None;
    }
    let stem = &name[..name.rfind('.')?];
    let body = stem.split_once('_').map_or(stem, |(_, rest)| rest);
    let mut parts = body.split('-');
    let axes = parse_indices(parts.next()?)?;
    let mut reference = None;
    for p in parts {
        let idx = parse_indices(p)?;
        match idx.as_slice() {
            [(REFERENCE_MARK, n)] => reference = Some(*n),
            _ => return None,
        }
    }
    Some(PlaneName { axes, reference })
}

/// One image: the plane files that share their axis letters (and reference mark).
#[derive(Debug, Clone)]
pub struct FvImage {
    /// Normalized description.
    pub info: ImageInfo,
    /// True for reference images (`-R###` files).
    pub reference: bool,
    /// `(c, z, t)` → member name.
    pub planes: BTreeMap<(u32, u32, u32), String>,
    /// Plane files whose name repeats another plane's indices.
    pub duplicates: usize,
}

/// An opened OIF or OIB data set.
pub struct FvDataset {
    /// The container.
    pub store: Store,
    /// Images: the main acquisition(s) first, then reference images.
    pub images: Vec<FvImage>,
    notes: Vec<String>,
    first_properties: Option<Settings>,
}

impl std::fmt::Debug for FvDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FvDataset")
            .field("path", &self.store.path)
            .field("images", &self.images.len())
            .finish_non_exhaustive()
    }
}

/// Grouping key of a plane file: reference mark, axis letters, values of letters we do not map.
type GroupKey = (bool, String, Vec<(char, u32)>);

/// FluoView date `2020-02-10 13:07:52` (+ milliseconds) → ISO-8601 without a zone.
pub fn fluoview_date(date: &str, millis: Option<f64>) -> Option<String> {
    let d = date.trim();
    let (day, time) = d.split_once(' ')?;
    let ok = day.len() == 10
        && time.len() == 8
        && day.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        })
        && time.bytes().enumerate().all(|(i, b)| {
            if i == 2 || i == 5 {
                b == b':'
            } else {
                b.is_ascii_digit()
            }
        });
    if !ok {
        return None;
    }
    let ms = millis
        .filter(|m| (0.0..1000.0).contains(m))
        .map_or(0, |m| m as u32);
    Some(format!("{day}T{time}.{ms:03}"))
}

fn um_from(value: Option<f64>, unit: Option<&str>) -> Option<f64> {
    let v = value?;
    match unit.map(str::trim) {
        Some("um" | "µm" | "micron") => Some(v),
        Some("nm") => Some(v / 1000.0),
        Some("mm") => Some(v * 1000.0),
        _ => None,
    }
}

impl FvDataset {
    /// Open an OIB compound file or an OIF settings file with its `.oif.files` folder.
    pub fn open(path: &Path, kind: StoreKind) -> Result<Self> {
        Self::open_input(&Input::local(path), kind)
    }

    /// [`FvDataset::open`] for an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input, kind: StoreKind) -> Result<Self> {
        let store = Store::open_in(input.fs(), input.path(), kind)?;
        let mut ds = FvDataset {
            store,
            images: Vec::new(),
            notes: Vec::new(),
            first_properties: None,
        };
        ds.build()?;
        Ok(ds)
    }

    fn format_id(&self) -> &'static str {
        self.store.kind.format_id()
    }

    fn corrupt(&self, d: impl Into<String>) -> Error {
        Error::corrupt(self.format_id(), d.into())
    }

    fn build(&mut self) -> Result<()> {
        let mut groups: BTreeMap<GroupKey, Vec<(String, PlaneName)>> = BTreeMap::new();
        let mut ignored = 0usize;
        for m in self.store.members.values() {
            let Some(pn) = parse_plane_name(&m.name) else {
                if ext_is(&m.name, &["tif", "tiff"]) {
                    ignored += 1;
                }
                continue;
            };
            let letters: String = pn.axes.iter().map(|(a, _)| *a).collect();
            let others: Vec<(char, u32)> = pn
                .axes
                .iter()
                .filter(|(a, _)| ![AXIS_CHANNEL, AXIS_Z, AXIS_TIME, AXIS_LAMBDA].contains(a))
                .copied()
                .collect();
            groups
                .entry((pn.reference.is_some(), letters, others))
                .or_default()
                .push((m.name.clone(), pn));
        }
        if ignored > 0 {
            self.notes.push(format!(
                "{ignored} TIFF file(s) whose names carry no axis indices are listed by `info --view structure` but not exposed as planes"
            ));
        }
        if groups.is_empty() && !self.store.members.is_empty() {
            self.notes
                .push("the data set holds no plane TIFF files (s_C001….tif)".into());
        }
        let n_groups = groups.len();
        for (k, ((reference, letters, others), files)) in groups.into_iter().enumerate() {
            let img = self.build_image(k as u32, reference, &letters, &others, &files)?;
            self.images.push(img);
        }
        if n_groups > 1 {
            let refs = self.images.iter().filter(|i| i.reference).count();
            if refs > 0 {
                self.notes.push(
                    "reference images (s_C###-R###.tif, the overview frame of a line or point scan) are exposed as a separate image".into(),
                );
            }
            if n_groups - refs > 1 {
                self.notes.push(
                    "plane files with different axis letters or unmapped axis indices are exposed as separate images (inferred)".into(),
                );
            }
        }
        if self.images.iter().any(|i| i.info.acquired_at.is_some()) {
            self.notes.push(
                "acquired_at is FluoView's ImageCaputreDate, a local wall-clock time without a time zone".into(),
            );
        }
        Ok(())
    }

    fn build_image(
        &mut self,
        index: u32,
        reference: bool,
        letters: &str,
        others: &[(char, u32)],
        files: &[(String, PlaneName)],
    ) -> Result<FvImage> {
        let get =
            |pn: &PlaneName, a: char| pn.axes.iter().find(|(x, _)| *x == a).map_or(1, |(_, v)| *v);
        let ordinals = |a: char| -> BTreeMap<u32, u32> {
            let set: BTreeSet<u32> = files.iter().map(|(_, pn)| get(pn, a)).collect();
            set.into_iter()
                .enumerate()
                .map(|(k, v)| (v, k as u32))
                .collect()
        };
        let cs = ordinals(AXIS_CHANNEL);
        let zs = ordinals(AXIS_Z);
        let ts = ordinals(AXIS_TIME);
        let ls = ordinals(AXIS_LAMBDA);
        let n_l = ls.len() as u32;
        let mut planes = BTreeMap::new();
        let mut duplicates = 0;
        for (name, pn) in files {
            let c = cs[&get(pn, AXIS_CHANNEL)] * n_l + ls[&get(pn, AXIS_LAMBDA)];
            let key = (c, zs[&get(pn, AXIS_Z)], ts[&get(pn, AXIS_TIME)]);
            if planes.insert(key, name.clone()).is_some() {
                duplicates += 1;
            }
        }
        // geometry from the first plane's TIFF header
        let first_name = planes.values().next().cloned().unwrap_or_default();
        let first = self
            .store
            .member(&first_name)
            .cloned()
            .ok_or_else(|| self.corrupt(format!("{first_name} disappeared")))?;
        let (tf, _src) = self.tiff(&first)?;
        let ifd = tf.ifds.first().ok_or_else(|| {
            self.corrupt(format!("{}: the TIFF has no image directory", first.name))
        })?;
        let layout = PageLayout::from_ifd(ifd, tf.header.byte_order)
            .map_err(|e| self.retag(&first.name, e))?;
        let pixel_type = layout.pixel_type().unwrap_or(PixelType::Uint16);
        let mut info = ImageInfo::new(index, layout.width, layout.height, pixel_type);
        info.samples_per_pixel = if layout.is_rgb() {
            u32::from(layout.samples_per_pixel)
        } else {
            1
        };
        info.size_c = cs.len() as u32 * n_l;
        info.size_z = zs.len() as u32;
        info.size_t = ts.len() as u32;
        let main = &self.store.main;
        let axis_order = main
            .text("Axis Parameter Common", "AxisOrder")
            .unwrap_or_default();
        info.dimension_order = dimension_order(&axis_order);
        // properties of the first plane: pixel calibration, objective
        let first_pty = first_name
            .rsplit_once('.')
            .map(|(stem, _)| format!("{stem}.pty"))
            .and_then(|p| self.store.settings(&p));
        let ip = |key: &str| {
            first_pty
                .as_ref()
                .and_then(|p| p.text("Image Parameters", key))
        };
        let ipn = |key: &str| {
            first_pty
                .as_ref()
                .and_then(|p| p.number("Image Parameters", key))
        };
        let refp = |key: &str| main.text("Reference Image Parameter", key);
        let refn = |key: &str| main.number("Reference Image Parameter", key);
        let (x, y) = if first_pty.is_some() {
            (
                um_from(ipn("WidthConvertValue"), ip("WidthUnit").as_deref()),
                um_from(ipn("HeightConvertValue"), ip("HeightUnit").as_deref()),
            )
        } else {
            let y_is_space = reference || axis_order.chars().nth(1) == Some('Y');
            (
                um_from(refn("WidthConvertValue"), refp("WidthUnit").as_deref()),
                if y_is_space {
                    um_from(refn("HeightConvertValue"), refp("HeightUnit").as_deref())
                } else {
                    None
                },
            )
        };
        let z_axis = axis_section(main, 'Z');
        let z = (info.size_z > 1)
            .then(|| {
                let s = main.section(z_axis.as_deref()?)?;
                um_from(
                    s.number("Interval").map(f64::abs),
                    s.text("PixUnit").as_deref(),
                )
            })
            .flatten();
        info.physical_size = PhysicalSize::micrometres(x, y, z);
        // channels: [Channel n Parameters] for the file's C number n, times lambda bands
        let lambda = axis_section(main, 'L').and_then(|s| main.section(&s).cloned());
        for (&c_raw, &ci) in &cs {
            let sec = main.section(&format!("Channel {c_raw} Parameters"));
            for (&l_raw, &li) in &ls {
                let mut ch = ChannelInfo {
                    index: ci * n_l + li,
                    ..ChannelInfo::default()
                };
                if let Some(s) = sec {
                    ch.name = s.text("CH Name");
                    ch.fluorophore = s.text("DyeName").filter(|d| d != "None");
                    ch.excitation_nm = s.number("ExcitationWavelength").filter(|v| *v > 0.0);
                    ch.emission_nm = s.number("EmissionWavelength").filter(|v| *v > 0.0);
                    ch.acquisition_mode = s.text("LightType");
                }
                if letters.contains(AXIS_LAMBDA)
                    && let Some(l) = &lambda
                    && let (Some(start), Some(step)) =
                        (l.number("StartPosition"), l.number("Interval"))
                {
                    // band i starts at StartPosition + i × Interval and is Resolution wide
                    let lo = start + step * f64::from(l_raw.saturating_sub(1));
                    let width = l.number("Resolution").filter(|w| *w > 0.0).unwrap_or(step);
                    ch.emission_nm = Some(lo + width / 2.0);
                    ch.emission_range_nm = Some([lo, lo + width]);
                }
                info.channels.push(ch);
            }
        }
        info.channels.sort_by_key(|c| c.index);
        // objective and scan timing from the first plane's properties
        if let Some(p) = &first_pty {
            let acq = "Acquisition Parameters Common";
            let model = p.text(acq, "ObjectiveLens Name");
            let mag = p.number(acq, "Magnification").filter(|v| *v > 0.0);
            let na = p
                .number(acq, "ObjectiveLens NAValue")
                .filter(|v| *v > 0.0 && *v < 9999.0);
            if model.is_some() || mag.is_some() || na.is_some() {
                info.objective = Some(ObjectiveInfo {
                    model: model.map(|m| m.split_whitespace().collect::<Vec<_>>().join(" ")),
                    nominal_magnification: mag,
                    lens_na: na,
                    ..ObjectiveInfo::default()
                });
            }
            if let Some(v) = p.number(acq, "Time Per Line").filter(|v| *v > 0.0) {
                info.extra.insert("line_time_us".into(), json!(v));
            }
            if let Some(v) = p.number(acq, "Time Per Pixel").filter(|v| *v > 0.0) {
                info.extra.insert("pixel_dwell_us".into(), json!(v));
            }
        }
        let version = |k: &str| main.text("Version Info", k);
        let device = main.text("Acquisition Parameters Common", "Acquisition Device");
        info.instrument = Some(InstrumentInfo {
            manufacturer: Some("Olympus".into()),
            model: version("SystemName").or(device),
            software: Some("FluoView".into()),
            software_version: version("SystemVersion"),
            ..InstrumentInfo::default()
        });
        info.acquired_at = main
            .text("Acquisition Parameters Common", "ImageCaputreDate")
            .and_then(|d| {
                fluoview_date(
                    &d,
                    main.number("Acquisition Parameters Common", "ImageCaputreDate+MilliSec"),
                )
            });
        let data_name = main.text("File Info", "DataName");
        info.name = if reference {
            Some(format!(
                "{} [reference]",
                data_name.as_deref().unwrap_or("image")
            ))
        } else {
            data_name
        };
        // time increment from the first and last time point's properties
        if info.size_t > 1 {
            let time_of = |t: u32| -> Option<f64> {
                let name = planes.get(&(0, 0, t))?;
                let pty = format!("{}.pty", name.rsplit_once('.')?.0);
                self.store.settings(&pty)?.number(
                    &format!("Axis {} Parameters", axis_number(main, 'T')?),
                    "AbsPositionValue",
                )
            };
            if let (Some(a), Some(b)) = (time_of(0), time_of(info.size_t - 1)) {
                let dt = (b - a) / 1000.0 / f64::from(info.size_t - 1);
                if dt > 0.0 && dt.is_finite() {
                    info.time_increment_s = Some(dt);
                }
            }
        }
        let ex = &mut info.extra;
        if reference {
            ex.insert("kind".into(), json!("reference"));
        }
        if !axis_order.is_empty() {
            ex.insert("axis_order".into(), json!(axis_order));
        }
        if let Some(m) = main.text("Acquisition Parameters Common", "ScanMode") {
            ex.insert("scan_mode".into(), json!(m));
        }
        if let Some(b) = ipn("ValidBitCounts").or_else(|| refn("ValidBitCounts")) {
            ex.insert("bits_significant".into(), json!(b as u32));
        }
        if !others.is_empty() {
            ex.insert(
                "other_indices".into(),
                json!(
                    others
                        .iter()
                        .map(|(a, v)| (a.to_string(), *v))
                        .collect::<BTreeMap<_, _>>()
                ),
            );
        }
        if n_l > 1 || letters.contains(AXIS_LAMBDA) {
            ex.insert(
                "lambda_index".into(),
                json!(ls.keys().copied().collect::<Vec<_>>()),
            );
        }
        ex.insert("compression".into(), json!(layout.compression_name()));
        let expected = u64::from(info.size_z) * u64::from(info.size_c) * u64::from(info.size_t);
        if duplicates > 0 || (planes.len() as u64) < expected {
            self.notes.push(format!(
                "image {index}: {} plane files for {expected} (c, z, t) positions ({duplicates} duplicates); missing planes read as errors",
                files.len()
            ));
        }
        if self.first_properties.is_none() {
            self.first_properties.clone_from(&first_pty);
        }
        Ok(FvImage {
            info: info.finish(),
            reference,
            planes,
            duplicates,
        })
    }

    /// Parse a TIFF member (in memory for OIB, from disk for OIF).
    fn tiff(&self, m: &Member) -> Result<(TiffFile, ByteSource)> {
        let r = match &m.source {
            crate::store::MemberSource::File(p) => TiffFile::open_in(&self.store.fs, p),
            crate::store::MemberSource::Stream(_) => {
                let bytes = self.store.read(m, MAX_TIFF_BYTES)?;
                TiffFile::from_bytes(&self.store.label(m), bytes)
            }
        };
        r.map_err(|e| self.retag(&m.name, e))
    }

    /// TIFF-level errors are reported under this format's id, naming the member.
    fn retag(&self, member: &str, e: Error) -> Error {
        match e {
            Error::Corrupt { detail, .. } => {
                Error::corrupt(self.format_id(), format!("{member}: {detail}"))
            }
            Error::Unsupported { feature, hint, .. } => Error::Unsupported {
                format: self.format_id(),
                feature: format!("{member}: {feature}"),
                hint,
            },
            other => other,
        }
    }

    /// Decode one plane member into little-endian samples.
    fn read_member(&self, name: &str) -> Result<(PageLayout, Vec<u8>)> {
        let m = self
            .store
            .member(name)
            .cloned()
            .ok_or_else(|| self.corrupt(format!("plane file {name} is missing")))?;
        let (tf, mut src) = self.tiff(&m)?;
        if let Some(p) = tf.problems.iter().find(|p| p.code == "truncated") {
            return Err(self.corrupt(format!("{}: {}", m.name, p.detail)));
        }
        let ifd = tf
            .ifds
            .first()
            .ok_or_else(|| self.corrupt(format!("{}: no image directory", m.name)))?;
        let layout =
            PageLayout::from_ifd(ifd, tf.header.byte_order).map_err(|e| self.retag(&m.name, e))?;
        let data =
            read_page(&mut src, &layout, SampleSelect::All).map_err(|e| self.retag(&m.name, e))?;
        Ok((layout, data))
    }

    fn descriptor(&self) -> openreadout_core::model::FormatDescriptor {
        match self.store.kind {
            StoreKind::Oib => OibReader.descriptor(),
            StoreKind::Oif => OifReader.descriptor(),
        }
    }

    fn pty_of(&self, plane: &str) -> Option<Settings> {
        let stem = plane.rsplit_once('.')?.0;
        self.store.settings(&format!("{stem}.pty"))
    }

    fn size_bytes(&self) -> u64 {
        let own = self
            .store
            .fs
            .metadata(&self.store.path)
            .map_or(0, |m| m.len());
        match self.store.kind {
            StoreKind::Oib => own,
            StoreKind::Oif => own + self.store.members.values().map(|m| m.size).sum::<u64>(),
        }
    }
}

/// `[Axis n Parameters Common]` section whose `AxisCode` is `code`.
fn axis_section(main: &Settings, code: char) -> Option<String> {
    axis_number(main, code).map(|n| format!("Axis {n} Parameters Common"))
}

/// Number `n` of the axis section whose `AxisCode` is `code`.
fn axis_number(main: &Settings, code: char) -> Option<u32> {
    (0..16).find(|n| {
        main.text(&format!("Axis {n} Parameters Common"), "AxisCode")
            .is_some_and(|c| c.eq_ignore_ascii_case(&code.to_string()))
    })
}

/// `XYCZ` / `XYCL` / `XTC` → `XYCZT`-style order of C, Z and T (lambda counts as C; axes the
/// order does not name are appended in C, Z, T order).
pub fn dimension_order(axis_order: &str) -> String {
    let mut out = String::from("XY");
    for a in axis_order.chars().skip(2) {
        let a = match a.to_ascii_uppercase() {
            AXIS_LAMBDA => AXIS_CHANNEL,
            other => other,
        };
        if [AXIS_CHANNEL, AXIS_Z, AXIS_TIME].contains(&a) && !out.contains(a) {
            out.push(a);
        }
    }
    for a in [AXIS_CHANNEL, AXIS_Z, AXIS_TIME] {
        if !out.contains(a) {
            out.push(a);
        }
    }
    out
}

impl Dataset for FvDataset {
    fn member_files(&self) -> Vec<PathBuf> {
        self.files()
            .into_iter()
            .filter(|p| *p != self.store.path && self.store.fs.is_file(p))
            .collect()
    }

    fn info(&self) -> Result<FileInfo> {
        let images: Vec<ImageInfo> = self.images.iter().map(|i| i.info.clone()).collect();
        let mut notes = self.notes.clone();
        notes.extend(self.store.problems.iter().cloned());
        Ok(FileInfo {
            path: self.store.path.display().to_string(),
            size_bytes: self.size_bytes(),
            format: self.descriptor(),
            format_version: self
                .store
                .main
                .text("Version Info", "FileVersion")
                .or_else(|| self.store.main.text("ProfileSaveInfo", "Version")),
            plane_count: images.iter().map(|i| i.plane_count).sum(),
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: Vec::new(),
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({
            "main_file": self.store.main_name,
            "main": self.store.main.to_json(),
            "oib_info": self.store.oib_info.as_ref().map(Settings::to_json),
            "first_plane_properties": self.first_properties.as_ref().map(Settings::to_json),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        // plane geometry and sample type come from each plane's TIFF header (TIFF 6.0)
        for k in [
            "images[].size_x",
            "images[].size_y",
            "images[].pixel_type",
            "images[].samples_per_pixel",
        ] {
            p.insert(k.into(), Source::Spec);
        }
        // axis indices in plane file names: oiffile's documented convention, checked on corpus files
        for k in [
            "images[].size_z",
            "images[].size_c",
            "images[].size_t",
            "images[].dimension_order",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        p.insert("images[].channels".into(), Source::Inferred);
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
        let mut used = BTreeSet::new();
        for (k, im) in self.images.iter().enumerate() {
            for (&(c, z, t), name) in &im.planes {
                let size = self.store.member(name).map(|m| m.size);
                used.insert(name.to_ascii_lowercase());
                out.push(LsEntry {
                    kind: "plane".into(),
                    name: name.clone(),
                    offset: None,
                    size,
                    image: Some(k as u32),
                    details: json!({"c": c, "z": z, "t": t}),
                });
            }
        }
        out.push(LsEntry {
            kind: "settings".into(),
            name: self.store.main_name.clone(),
            offset: None,
            size: None,
            image: None,
            details: json!({"role": "main"}),
        });
        for (key, m) in &self.store.members {
            if used.contains(key) {
                continue;
            }
            let ext = m
                .name
                .rsplit_once('.')
                .map(|(_, e)| e.to_ascii_lowercase())
                .unwrap_or_default();
            let kind = match ext.as_str() {
                "pty" => "properties",
                "roi" => "roi",
                "lut" => "lut",
                "bmp" => "thumbnail",
                "txt" => "text",
                _ => "file",
            };
            out.push(LsEntry {
                kind: kind.into(),
                name: m.name.clone(),
                offset: None,
                size: Some(m.size),
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
        let name = im
            .planes
            .get(&(index.c, index.z, index.t))
            .cloned()
            .ok_or_else(|| {
                self.corrupt(format!(
                    "no plane file holds c={} z={} t={}",
                    index.c, index.z, index.t
                ))
            })?;
        let (w, h, pt, spp) = (
            im.info.size_x,
            im.info.size_y,
            im.info.pixel_type,
            im.info.samples_per_pixel,
        );
        let (layout, data) = self.read_member(&name)?;
        if (layout.width, layout.height) != (w, h) {
            return Err(self.corrupt(format!(
                "{name} is {}x{}, the image is {w}x{h}",
                layout.width, layout.height
            )));
        }
        let got = layout.pixel_type().map_err(|e| self.retag(&name, e))?;
        if got != pt {
            return Err(self.corrupt(format!("{name} holds {got:?} samples, the image is {pt:?}")));
        }
        Ok(Plane {
            width: w,
            height: h,
            pixel_type: pt,
            samples_per_pixel: spp,
            data,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.store.path.display().to_string(), self.format_id());
        match self.store.kind {
            StoreKind::Oib => {
                r.performed("compound-file header, FAT and directory parse (MS-CFB)");
                r.performed("every stream OibInfo.txt lists exists");
            }
            StoreKind::Oif => r.performed("the .oif.files data folder exists"),
        }
        r.performed("main settings file parses");
        r.performed("every plane TIFF parses and decodes to the image's size and sample type");
        r.performed("every (c, z, t) position of every image has exactly one plane file");
        r.performed("every plane file has its .pty property file");
        for p in &self.store.problems {
            r.push(Finding::error("container", p.clone()));
        }
        if self.images.is_empty() {
            r.push(Finding::error(
                "planes",
                "no plane TIFF files: the data set holds no image data".to_string(),
            ));
        }
        for n in &self.notes {
            if n.contains("missing planes") {
                r.push(Finding::error("planes", n.clone()));
            }
        }
        for k in 0..self.images.len() {
            let im = &self.images[k];
            let (w, h) = (im.info.size_x, im.info.size_y);
            let names: Vec<String> = im.planes.values().cloned().collect();
            for name in names {
                match self.read_member(&name) {
                    Ok((layout, _)) if (layout.width, layout.height) != (w, h) => {
                        r.push(Finding::error(
                            "geometry",
                            format!(
                                "{name} is {}x{}, image {k} is {w}x{h}",
                                layout.width, layout.height
                            ),
                        ));
                    }
                    Ok(_) => {}
                    Err(e @ Error::Unsupported { .. }) => {
                        r.push(Finding::warning("unsupported", e.to_string()));
                    }
                    Err(e) => r.push(Finding::error("truncated", e.to_string())),
                }
                let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
                if self.store.member(&format!("{stem}.pty")).is_none() {
                    r.push(Finding::warning(
                        "properties",
                        format!("{name} has no {stem}.pty property file"),
                    ));
                }
            }
        }
        Ok(r)
    }

    fn attachments(&self) -> Result<Vec<AttachmentInfo>> {
        Ok(self
            .thumbnail()
            .map(|m| AttachmentInfo {
                index: 0,
                name: m.name.clone(),
                content_type: "BMP".into(),
                extension: "bmp".into(),
                offset: None,
                size: m.size,
                extra: BTreeMap::new(),
            })
            .into_iter()
            .collect())
    }

    fn read_attachment(&mut self, index: u32) -> Result<Vec<u8>> {
        let m = self
            .thumbnail()
            .filter(|_| index == 0)
            .cloned()
            .ok_or_else(|| {
                Error::Usage(format!(
                    "attachment #{index} does not exist; `info --view structure` lists them"
                ))
            })?;
        self.store.read(&m, m.size)
    }

    fn frames(&self, image: u32, limit: Option<usize>) -> Result<(u64, Vec<Value>)> {
        let Some(im) = self.images.get(image as usize) else {
            return Ok((0, Vec::new()));
        };
        let main = &self.store.main;
        let t_axis = axis_number(main, 'T');
        let z_axis = axis_number(main, 'Z');
        let total = im.planes.len() as u64;
        let mut out = Vec::new();
        for (&(c, z, t), name) in im.planes.iter().take(limit.unwrap_or(usize::MAX)) {
            let mut rec = serde_json::Map::new();
            rec.insert("c".into(), json!(c));
            rec.insert("z".into(), json!(z));
            rec.insert("t".into(), json!(t));
            rec.insert("file".into(), json!(name));
            if let Some(p) = self.pty_of(name) {
                let abs = |n: Option<u32>| {
                    p.number(&format!("Axis {} Parameters", n?), "AbsPositionValue")
                };
                if let Some(ms) = abs(t_axis) {
                    rec.insert("delta_t_s".into(), json!(ms / 1000.0));
                }
                if let Some(nm) = abs(z_axis) {
                    rec.insert("stage_z_um".into(), json!(nm / 1000.0));
                }
                if let Some(v) = p.number("Acquisition Parameters Common", "PMTVoltage") {
                    rec.insert("detector_voltage".into(), json!(v));
                }
            }
            out.push(Value::Object(rec));
        }
        Ok((total, out))
    }
}

impl FvDataset {
    fn thumbnail(&self) -> Option<&Member> {
        self.store
            .members
            .values()
            .find(|m| ext_is(&m.name, &["bmp"]) && m.name.to_ascii_lowercase().contains("thumb"))
            .or_else(|| {
                self.store
                    .members
                    .values()
                    .find(|m| ext_is(&m.name, &["bmp"]))
            })
    }

    /// Paths of the files this data set is made of (the `.oib`, or the `.oif` and its folder's
    /// files), for tools that copy or checksum a data set.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut out = vec![self.store.path.clone()];
        if self.store.kind == StoreKind::Oif {
            out.extend(self.store.members.values().filter_map(|m| match &m.source {
                crate::store::MemberSource::File(p) => Some(p.clone()),
                crate::store::MemberSource::Stream(_) => None,
            }));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plane_names() {
        assert_eq!(
            parse_plane_name("s_C001Z002T010.tif"),
            Some(PlaneName {
                axes: vec![('C', 1), ('Z', 2), ('T', 10)],
                reference: None
            })
        );
        assert_eq!(
            parse_plane_name("s_C002-R002.tif"),
            Some(PlaneName {
                axes: vec![('C', 2)],
                reference: Some(2)
            })
        );
        assert_eq!(parse_plane_name("s_Thumb.bmp"), None);
        assert_eq!(parse_plane_name("s_LUT1.lut"), None);
        assert_eq!(parse_plane_name("s_Thumb.tif"), None);
        assert_eq!(parse_plane_name("s_C001-X.tif"), None);
    }

    #[test]
    fn orders_and_dates() {
        assert_eq!(dimension_order("XYCZ"), "XYCZT");
        assert_eq!(dimension_order("XYCT"), "XYCTZ");
        assert_eq!(dimension_order("XYCL"), "XYCZT");
        assert_eq!(dimension_order("XTC"), "XYCZT");
        assert_eq!(dimension_order("XYZCT"), "XYZCT");
        assert_eq!(
            fluoview_date("2020-02-10 13:07:52", Some(469.0)).as_deref(),
            Some("2020-02-10T13:07:52.469")
        );
        assert_eq!(fluoview_date("garbage", None), None);
    }
}
