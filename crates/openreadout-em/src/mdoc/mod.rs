//! SerialEM `.mdoc` metadata files: `key = value` lines in sections (`[ZValue = N]` for each
//! image of a tilt series or stack, `[FrameSet = N]` for a saved movie, `[MontSection = N]`
//! for a montage piece), after file-wide keys and `[T = ...]` title lines. Read on their own
//! (a table of sections) and as the sidecar of an MRC stack (`stack.mrc.mdoc`), whose
//! per-section records they extend. See `docs/formats/mdoc.md`.

mod dataset;

use std::path::{Path, PathBuf};

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

pub use dataset::MdocDataset;

/// Format id used on the command line and in JSON.
pub const FORMAT_ID: &str = "mdoc";
/// Sections read at most.
pub const MAX_SECTIONS: usize = 100_000;

/// The SerialEM mdoc reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct MdocReader;

impl FormatReader for MdocReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::MDOC)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: FORMAT_ID.into(),
            name: "SerialEM metadata (mdoc)".into(),
            vendor: "SerialEM (open text format)".into(),
            extensions: vec!["mdoc".into()],
            family: "electron-microscopy".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::MDOC.confidence,
            known_gaps: vec![
                "Metadata only: the images live in the stack or movie files the mdoc names; open those (an MRC stack next to its mdoc gets the mdoc's tilt angles and doses as frame records)".into(),
                "Navigator (`.nav`) files, which use the same syntax, are not read".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        let text = String::from_utf8_lossy(&head[..head.len().min(4096)]);
        let has_section = text.contains("[ZValue")
            || text.contains("[FrameSet")
            || text.contains("[MontSection")
            || text.contains("[T = SerialEM");
        let ext = has_extension(path, &["mdoc"]);
        match (ext, has_section) {
            (true, true) => Some(det(DetectConfidence::Definite, None)),
            (true, false) => Some(det(
                DetectConfidence::ExtensionOnly,
                Some("mdoc extension, but no SerialEM section in the first 4 KiB"),
            )),
            (false, true) if text.trim_start().starts_with("PixelSpacing")
                || text.contains("\nPixelSpacing")
                || text.contains("[T = SerialEM") =>
            {
                Some(det(DetectConfidence::Likely, None))
            }
            _ => None,
        }
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(MdocDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

fn det(confidence: DetectConfidence, note: Option<&str>) -> Detection {
    Detection {
        format_id: FORMAT_ID,
        confidence,
        note: note.map(str::to_string),
    }
}

/// One `[Kind = N]` section.
#[derive(Debug, Clone, PartialEq)]
pub struct MdocSection {
    /// `ZValue`, `FrameSet`, `MontSection`, ...
    pub kind: String,
    /// The number after `=`.
    pub index: i64,
    /// `key = value` lines in file order.
    pub fields: Vec<(String, String)>,
}

impl MdocSection {
    /// The first value of `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn num(&self, key: &str) -> Option<f64> {
        self.get(key)?
            .split_whitespace()
            .next()?
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
    }

    fn nums(&self, key: &str) -> Option<Vec<f64>> {
        let v: Vec<f64> = self
            .get(key)?
            .split_whitespace()
            .filter_map(|w| w.parse::<f64>().ok())
            .collect();
        (!v.is_empty()).then_some(v)
    }
}

/// A parsed mdoc file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mdoc {
    /// File-wide `key = value` lines before the first section.
    pub globals: Vec<(String, String)>,
    /// `[T = ...]` title lines (SerialEM version, microscope, date; tilt axis angle).
    pub titles: Vec<String>,
    /// Sections in file order.
    pub sections: Vec<MdocSection>,
}

impl Mdoc {
    /// Parse mdoc text.
    pub fn parse(text: &str) -> Result<Mdoc> {
        let mut m = Mdoc::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let (k, v) = inner.split_once('=').unwrap_or((inner, ""));
                let (k, v) = (k.trim(), v.trim());
                if k == "T" {
                    m.titles.push(v.to_string());
                    continue;
                }
                if m.sections.len() >= MAX_SECTIONS {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("more than {MAX_SECTIONS} sections"),
                    ));
                }
                let index = v.parse::<i64>().map_err(|_| {
                    Error::corrupt(FORMAT_ID, format!("section header `{line}` has no number"))
                })?;
                m.sections.push(MdocSection {
                    kind: k.to_string(),
                    index,
                    fields: Vec::new(),
                });
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim().to_string(), v.trim().to_string());
            if k == "T" {
                m.titles.push(v);
            } else if let Some(s) = m.sections.last_mut() {
                s.fields.push((k, v));
            } else {
                m.globals.push((k, v));
            }
        }
        if m.sections.is_empty() && m.globals.is_empty() {
            return Err(Error::corrupt(
                FORMAT_ID,
                "no `key = value` line and no section: not a SerialEM mdoc file",
            ));
        }
        Ok(m)
    }

    /// The first file-wide value of `key`.
    pub fn global(&self, key: &str) -> Option<&str> {
        self.globals
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The tilt axis angle from a `[T = Tilt axis angle = 85.6, ...]` title, in degrees.
    pub fn tilt_axis_deg(&self) -> Option<f64> {
        self.titles.iter().find_map(|t| {
            let rest = t.split("Tilt axis angle =").nth(1)?;
            rest.split([',', ' ']).find(|w| !w.is_empty())?.parse().ok()
        })
    }

    /// The acquisition order of the sections (by `DateTime`, then file order); `None` when a
    /// section has no readable time.
    pub fn acquisition_order(&self) -> Option<Vec<usize>> {
        let times: Option<Vec<String>> = self
            .sections
            .iter()
            .map(|s| s.get("DateTime").and_then(iso_datetime))
            .collect();
        let times = times?;
        let mut order: Vec<usize> = (0..self.sections.len()).collect();
        order.sort_by(|&a, &b| times[a].cmp(&times[b]).then(a.cmp(&b)));
        Some(order)
    }

    /// One record per section in our vocabulary (see `docs/formats/mdoc.md`), with the dose
    /// accumulated in acquisition order.
    pub fn records(&self) -> Vec<Map<String, Value>> {
        let order = self.acquisition_order();
        let mut prior = vec![None; self.sections.len()];
        if let Some(order) = &order {
            let mut acc = 0.0;
            let mut ok = true;
            for &i in order {
                prior[i] = ok.then_some(acc);
                match self.sections[i].num("ExposureDose") {
                    Some(d) => acc += d,
                    None => ok = false,
                }
            }
        }
        self.sections
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let mut r = Map::new();
                r.insert("section_kind".into(), json!(s.kind));
                r.insert("section".into(), json!(s.index));
                for (key, ours) in [
                    ("TiltAngle", "tilt_angle_deg"),
                    ("ExposureDose", "exposure_dose_e_per_a2"),
                    ("ExposureTime", "exposure_time_s"),
                    ("Defocus", "defocus_um"),
                    ("TargetDefocus", "target_defocus_um"),
                    ("Magnification", "magnification"),
                    ("PixelSpacing", "pixel_spacing_a"),
                    ("StageZ", "stage_z_um"),
                    ("RotationAngle", "rotation_angle_deg"),
                    ("Intensity", "intensity"),
                    ("SpotSize", "spot_size"),
                    ("Binning", "binning"),
                    ("NumSubFrames", "subframes"),
                    ("CountsPerElectron", "counts_per_electron"),
                    ("DoseRate", "dose_rate"),
                    ("PriorRecordDose", "prior_record_dose_e_per_a2"),
                ] {
                    if let Some(v) = s.num(key) {
                        r.insert(ours.into(), json!(v));
                    }
                }
                if let Some(v) = s.nums("StagePosition").filter(|v| v.len() >= 2) {
                    r.insert("stage_x_um".into(), json!(v[0]));
                    r.insert("stage_y_um".into(), json!(v[1]));
                }
                if let Some(v) = s.nums("ImageShift").filter(|v| v.len() >= 2) {
                    r.insert("image_shift".into(), json!([v[0], v[1]]));
                }
                if let Some(v) = s.nums("MinMaxMean").filter(|v| v.len() >= 3) {
                    r.insert("min".into(), json!(v[0]));
                    r.insert("max".into(), json!(v[1]));
                    r.insert("mean".into(), json!(v[2]));
                }
                if let Some(v) = s.nums("FrameDosesAndNumber").filter(|v| v.len() >= 2) {
                    r.insert("frame_dose_e_per_a2".into(), json!(v[0]));
                }
                if let Some(p) = s.get("SubFramePath") {
                    r.insert("subframe_file".into(), json!(file_name(p)));
                }
                for (key, ours) in [("GainReference", "gain_reference"), ("DefectFile", "defect_file")] {
                    if let Some(p) = s.get(key) {
                        r.insert(ours.into(), json!(file_name(p)));
                    }
                }
                if let Some(t) = s.get("DateTime") {
                    match iso_datetime(t) {
                        Some(iso) => r.insert("acquired_at".into(), json!(iso)),
                        None => r.insert("date_time".into(), json!(t)),
                    };
                }
                if let Some(Some(p)) = prior.get(i) {
                    r.insert("prior_dose_e_per_a2".into(), json!(round6(*p)));
                    if let Some(d) = s.num("ExposureDose") {
                        r.insert("cumulative_dose_e_per_a2".into(), json!(round6(p + d)));
                    }
                }
                if let Some(order) = &order
                    && let Some(k) = order.iter().position(|&j| j == i)
                {
                    r.insert("acquisition_order".into(), json!(k));
                }
                r
            })
            .collect()
    }

    /// A short summary: section kinds and count, tilt range, total dose.
    pub fn summary(&self) -> Map<String, Value> {
        let mut s = Map::new();
        s.insert("sections".into(), json!(self.sections.len()));
        if let Some(k) = self.sections.first() {
            s.insert("section_kind".into(), json!(k.kind));
        }
        let tilts: Vec<f64> = self.sections.iter().filter_map(|x| x.num("TiltAngle")).collect();
        if tilts.len() > 1 {
            let lo = tilts.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = tilts.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            s.insert("tilt_range_deg".into(), json!([round6(lo), round6(hi)]));
        }
        let doses: Vec<f64> = self.sections.iter().filter_map(|x| x.num("ExposureDose")).collect();
        if doses.len() == self.sections.len() && doses.iter().any(|d| *d > 0.0) {
            s.insert(
                "total_dose_e_per_a2".into(),
                json!(round6(doses.iter().sum::<f64>())),
            );
        }
        if let Some(a) = self.tilt_axis_deg() {
            s.insert("tilt_axis_deg".into(), json!(a));
        }
        if let Some(v) = self.global("PixelSpacing").and_then(|v| v.parse::<f64>().ok()) {
            s.insert("pixel_spacing_a".into(), json!(v));
        }
        if let Some(v) = self.global("Voltage").and_then(|v| v.parse::<f64>().ok()) {
            s.insert("voltage_kv".into(), json!(v));
        }
        if let Some(v) = self.global("ImageFile") {
            s.insert("image_file".into(), json!(v));
        }
        s
    }
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// The file name of a Windows or Unix path as SerialEM records it.
pub fn file_name(p: &str) -> String {
    p.rsplit(['\\', '/']).next().unwrap_or(p).trim().to_string()
}

/// SerialEM's `11-Oct-19  14:25:01` (or with a four-digit year) as ISO 8601 without a zone.
pub fn iso_datetime(s: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let mut it = s.split_whitespace();
    let date = it.next()?;
    let time = it.next()?;
    let mut d = date.split('-');
    let day: u32 = d.next()?.parse().ok()?;
    let mon = d.next()?.to_ascii_lowercase();
    let year: u32 = d.next()?.parse().ok()?;
    let m = MONTHS.iter().position(|x| mon.starts_with(x))? + 1;
    let year = if year < 100 { 2000 + year } else { year };
    let t: Vec<u32> = time.split(':').filter_map(|x| x.parse().ok()).collect();
    if t.len() != 3 || day == 0 || day > 31 || t[0] > 23 || t[1] > 59 || t[2] > 60 {
        return None;
    }
    Some(format!(
        "{year:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}",
        t[0], t[1], t[2]
    ))
}

/// The mdoc next to an image file: `<name>.mdoc` (SerialEM appends `.mdoc` to the image file's
/// full name).
pub fn sidecar_of(fs: &Fs, image: &Path) -> Option<PathBuf> {
    let name = image.file_name()?.to_string_lossy().to_string();
    let p = image.with_file_name(format!("{name}.mdoc"));
    fs.metadata(&p).is_ok_and(|m| m.is_file()).then_some(p)
}

/// Read and parse the mdoc at `path`.
pub fn read(fs: &Fs, path: &Path) -> Result<Mdoc> {
    let text = openreadout_core::limits::read_metadata_text(FORMAT_ID, fs, path)?;
    Mdoc::parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = "PixelSpacing = 1.6938\nVoltage = 300\nImageFile = G3_TS_001.mrc\nImageSize = 3708 3838\nDataMode = 1\n\n[T = SerialEM: Digitized on EMBL Krios   11-Oct-19  14:22:41    ]\n\n[T =     Tilt axis angle = 85.6, binning = 1  spot = 7  camera = 0]\n\n[ZValue = 0]\nTiltAngle = 0\nStagePosition = 244.187 -320.332\nExposureDose = 2.5\nSubFramePath = X:\\frames\\a_0.0.tif\nFrameDosesAndNumber = 0.31142 8\nDateTime = 11-Oct-19  14:25:01\n\n[ZValue = 1]\nTiltAngle = 3.00013\nExposureDose = 2.5\nDateTime = 11-Oct-19  14:24:01\n";

    #[test]
    fn parses_globals_titles_sections_and_doses() {
        let m = Mdoc::parse(TS).unwrap();
        assert_eq!(m.global("Voltage"), Some("300"));
        assert_eq!(m.titles.len(), 2);
        assert_eq!(m.tilt_axis_deg(), Some(85.6));
        assert_eq!(m.sections.len(), 2);
        let r = m.records();
        assert_eq!(r[0]["tilt_angle_deg"], json!(0.0));
        assert_eq!(r[0]["stage_y_um"], json!(-320.332));
        assert_eq!(r[0]["subframe_file"], json!("a_0.0.tif"));
        assert_eq!(r[0]["acquired_at"], json!("2019-10-11T14:25:01"));
        // section 1 was taken first: section 0 has 2.5 e/Å² before it
        assert_eq!(r[1]["prior_dose_e_per_a2"], json!(0.0));
        assert_eq!(r[0]["prior_dose_e_per_a2"], json!(2.5));
        assert_eq!(r[0]["cumulative_dose_e_per_a2"], json!(5.0));
        assert_eq!(r[0]["acquisition_order"], json!(1));
        let s = m.summary();
        assert_eq!(s["total_dose_e_per_a2"], json!(5.0));
        assert_eq!(s["tilt_range_deg"], json!([0.0, 3.00013]));
    }

    #[test]
    fn bad_text_and_dates() {
        assert!(Mdoc::parse("hello\nworld").is_err());
        assert!(Mdoc::parse("[ZValue = x]\n").is_err());
        assert_eq!(iso_datetime("15-Jul-2019 13:54:01").as_deref(), Some("2019-07-15T13:54:01"));
        assert_eq!(iso_datetime("bad"), None);
        assert_eq!(file_name("W:\\rawdata\\stack_0000.tif"), "stack_0000.tif");
    }
}
