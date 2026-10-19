//! Bruker flex series MALDI-TOF spectra (flexControl: autoflex, ultraflex, microflex, MALDI
//! Biotyper, rapifleX): one spectrum per acquisition directory, `fid` (32-bit integers, `TD`
//! points) beside its `acqus` parameter file, in a tree `<run>/<spot>/<n>/<1SLin|1SRef|1Ref|
//! <mass>.LIFT/1SRef>/`. m/z comes from the time-of-flight calibration in `acqus`
//! ([`params::Calibration`]). Notes: `docs/formats/bruker-flex.md`; provenance:
//! `docs/provenance/bruker-flex.md`.
//!
//! The input is a spot directory (or its `fid` / `acqus`), or any directory above spot
//! directories: every spot below it becomes a spectrum of one run.

pub(crate) mod params;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use openreadout_core::model::{
    CheckReport, DetectConfidence, FileInfo, Finding, FormatDescriptor, InstrumentInfo, LsEntry,
    SpectraInfo, Spectrum,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};

use params::{Calibration, Params};

/// Format id of Bruker flex MALDI-TOF spectra.
pub const FLEX_FORMAT_ID: &str = "bruker-flex";

/// Directory levels searched below the input for spot directories.
const MAX_DEPTH: usize = 7;
/// Spot directories kept at most.
const MAX_SPOTS: usize = 200_000;
/// Largest `fid` read (points).
const MAX_POINTS: u64 = 1 << 26;
/// Largest parameter file read (bytes).
const MAX_PARAMS: u64 = 4 << 20;

fn bad(detail: impl Into<String>) -> Error {
    Error::corrupt(FLEX_FORMAT_ID, detail)
}

/// The parameter file of a spot directory: `acqus`, else `acqu`.
fn params_path(fs: &Fs, dir: &Path) -> Option<PathBuf> {
    ["acqus", "acqu"]
        .iter()
        .map(|n| dir.join(n))
        .find(|p| fs.is_file(p))
}

fn read_params_text(fs: &Fs, p: &Path) -> Option<String> {
    let m = fs.metadata(p).ok()?;
    if m.len() > MAX_PARAMS {
        return None;
    }
    fs.read(p)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Whether `dir` is a flex spot directory: a `fid` beside an XMASS parameter file.
pub(crate) fn is_spot(fs: &Fs, dir: &Path) -> bool {
    fs.is_file(&dir.join("fid"))
        && params_path(fs, dir)
            .and_then(|p| read_params_text(fs, &p))
            .is_some_and(|t| Params::is_xmass(&t))
}

/// Subdirectories of a directory above spots that need not hold spots.
fn ignored_dir(name: &str) -> bool {
    name == "pdata" || name == "__MACOSX" || name.starts_with('.')
}

/// The spot directories at or below `path` (a spot, its `fid` or parameter file, or a
/// directory above spots), in path order; at most `limit`, looking into at most `budget`
/// directories. A directory above spots counts only when each of its subdirectories (up to
/// 64 are checked) holds a spot: a folder that also holds other data is not a flex run.
pub(crate) fn find_spots(fs: &Fs, path: &Path, limit: usize, budget: usize) -> Vec<PathBuf> {
    let spots = walk_spots(fs, path, limit, budget);
    if spots.is_empty() || !fs.is_dir(path) || is_spot(fs, path) {
        return spots;
    }
    let Ok(rd) = fs.read_dir(path) else {
        return spots;
    };
    let mut subs: Vec<PathBuf> = rd
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| !ignored_dir(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .take(10_000)
        .collect();
    subs.sort();
    let every = subs
        .iter()
        .take(64)
        .all(|d| spots.iter().any(|s| s.starts_with(d)) || !walk_spots(fs, d, 1, 400).is_empty());
    if every { spots } else { Vec::new() }
}

fn walk_spots(fs: &Fs, path: &Path, limit: usize, budget: usize) -> Vec<PathBuf> {
    if !fs.is_dir(path) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let parent = path.parent().unwrap_or(Path::new("."));
        return if matches!(name, "fid" | "acqus" | "acqu") && is_spot(fs, parent) {
            vec![parent.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    let mut out = Vec::new();
    let mut stack = vec![(path.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((d, depth)) = stack.pop() {
        visited += 1;
        if out.len() >= limit || visited > budget {
            break;
        }
        if is_spot(fs, &d) {
            out.push(d.clone());
        }
        if depth >= MAX_DEPTH {
            continue;
        }
        let Ok(rd) = fs.read_dir(&d) else {
            continue;
        };
        let mut subs: Vec<PathBuf> = rd
            .filter_map(std::result::Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_none_or(|n| !ignored_dir(&n.to_string_lossy()))
            })
            .take(100_000)
            .collect();
        subs.sort();
        for s in subs.into_iter().rev() {
            stack.push((s, depth + 1));
        }
    }
    out.sort();
    out
}

/// How the spectrum was acquired: from its directory names (`1SLin`, `1SRef`, `1Ref`,
/// `<mass>.LIFT/1SRef`), else from the names in the `PATH` the instrument recorded, else from
/// `AQ_mod` (1 linear, 2 reflector in the files we have).
fn mode_of(dir: &Path, p: &Params) -> &'static str {
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let parent = dir
        .parent()
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let by_name = mode_from_names(name, parent, p);
    if by_name != "unknown" {
        return by_name;
    }
    if let Some(path) = p.get("PATH") {
        let parts: Vec<&str> = path.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
        if let [.., parent, name] = parts.as_slice() {
            let by_path = mode_from_names(name, parent, p);
            if by_path != "unknown" {
                return by_path;
            }
        }
    }
    match p.get("AQ_mod") {
        Some("1") => "linear",
        Some("2") => "reflector",
        _ => "unknown",
    }
}

fn mode_from_names(name: &str, parent: &str, p: &Params) -> &'static str {
    if parent.to_ascii_uppercase().ends_with(".LIFT") || p.get("SPType") == Some("2") {
        "LIFT"
    } else if name.to_ascii_lowercase().contains("lin") {
        "linear"
    } else if name.to_ascii_lowercase().contains("ref") {
        "reflector"
    } else {
        "unknown"
    }
}

/// One spot directory, parsed.
#[derive(Debug, Clone)]
struct Spot {
    dir: PathBuf,
    rel: String,
    params: Params,
    calibration: std::result::Result<Calibration, String>,
    points: u64,
    big_endian: bool,
    mode: &'static str,
}

/// An open run of Bruker flex spectra.
#[derive(Debug)]
pub struct FlexDataset {
    path: PathBuf,
    fs: Fs,
    spots: Vec<Spot>,
    truncated: bool,
    notes: Vec<String>,
}

fn spot_name(s: &Spot) -> String {
    s.params
        .get("SPOTNO")
        .or_else(|| s.params.get("PATCHNO"))
        .map_or_else(|| s.rel.clone(), str::to_string)
}

impl FlexDataset {
    fn open_in(fs: &Fs, path: &Path) -> Result<Self> {
        let dirs = find_spots(fs, path, MAX_SPOTS + 1, usize::MAX);
        if dirs.is_empty() {
            return Err(Error::Usage(format!(
                "{} holds no Bruker flex spectrum (a directory with fid and an XMASS acqus file)",
                path.display()
            )));
        }
        let truncated = dirs.len() > MAX_SPOTS;
        let base = if fs.is_dir(path) {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(Path::new(".")).to_path_buf()
        };
        let mut spots = Vec::with_capacity(dirs.len().min(MAX_SPOTS));
        let mut notes = Vec::new();
        for d in dirs.into_iter().take(MAX_SPOTS) {
            let pp = params_path(fs, &d).ok_or_else(|| bad("a spot without its parameter file"))?;
            let text = read_params_text(fs, &pp)
                .ok_or_else(|| bad(format!("{} cannot be read", pp.display())))?;
            let params = Params::parse(&text);
            let fid_len = fs
                .metadata(&d.join("fid"))
                .map_err(|e| Error::io(d.join("fid"), e))?
                .len();
            let points = match params.f64("TD") {
                Some(td) if td >= 0.0 && td <= MAX_POINTS as f64 => td as u64,
                _ => fid_len / 4,
            };
            if fid_len < points.saturating_mul(4) {
                notes.push(format!(
                    "{}: fid holds {} points, acqus TD says {points}",
                    d.display(),
                    fid_len / 4
                ));
            }
            // `/` on every platform, so the spot names in the output don't depend on the OS
            let rel = d.strip_prefix(&base).map_or_else(
                |_| d.display().to_string(),
                |p| {
                    p.components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/")
                },
            );
            let mode = mode_of(&d, &params);
            spots.push(Spot {
                calibration: Calibration::from(&params),
                big_endian: params.get("BYTORDA") == Some("1"),
                rel: if rel.is_empty() { ".".into() } else { rel },
                dir: d,
                params,
                points,
                mode,
            });
        }
        if truncated {
            notes.push(format!(
                "only the first {MAX_SPOTS} spot directories are read"
            ));
        }
        for s in &spots {
            if let Err(e) = &s.calibration {
                notes.push(format!("{}: {e}; its m/z cannot be computed", s.rel));
            }
        }
        let lift = spots.iter().filter(|s| s.mode == "LIFT").count();
        if lift > 0 {
            notes.push(format!(
                "{lift} LIFT fragment spectra: their m/z comes from the ML1 to ML3 constants in acqus, as for MS spectra. flexControl's fragment calibration (NTBCal) is not applied, so fragment m/z can differ from flexAnalysis's"
            ));
        }
        Ok(FlexDataset {
            path: path.to_path_buf(),
            fs: fs.clone(),
            spots,
            truncated,
            notes,
        })
    }

    fn header(&self, i: usize) -> Result<Spectrum> {
        let s = self.spots.get(i).ok_or_else(|| {
            Error::Usage(format!(
                "spectrum {i} out of range (the run has {} spectra)",
                self.spots.len()
            ))
        })?;
        let p = &s.params;
        let polarity = match p.get("IONIZATION MODE").map(str::trim) {
            Some(m) if m.ends_with('+') => "positive",
            Some(m) if m.ends_with('-') => "negative",
            _ => "unknown",
        };
        let mut sp = Spectrum {
            index: i as u64,
            scan_number: i as u64 + 1,
            native_id: Some(s.rel.clone()),
            ms_level: if s.mode == "LIFT" { 2 } else { 1 },
            polarity: polarity.into(),
            centroided: false,
            ..Spectrum::default()
        };
        if s.mode == "LIFT" {
            sp.precursor_mz = p.f64("Parent").filter(|v| *v > 0.0);
            sp.extra.insert(
                "mz_note".into(),
                json!("m/z from the acqus ML1 to ML3 constants; the fragment calibration (NTBCal) is not applied"),
            );
        }
        let e = &mut sp.extra;
        e.insert("spot".into(), json!(spot_name(s)));
        e.insert("mode".into(), json!(s.mode));
        e.insert("points".into(), json!(s.points));
        for (k, key) in [
            ("shots", "NoSHOTS"),
            ("instrument", "INSTRUM"),
            ("instrument_type", "InstTyp"),
            ("software_version", "FCVer"),
            ("acquired", "AQ_DATE"),
            ("comment", "CMT1"),
        ] {
            if let Some(v) = p.get(key) {
                e.insert(
                    k.into(),
                    p.f64(key)
                        .filter(|_| matches!(key, "NoSHOTS" | "InstTyp"))
                        .map_or_else(|| json!(v), |n| json!(n)),
                );
            }
        }
        if let Ok(c) = &s.calibration {
            e.insert(
                "calibration".into(),
                json!({"DW": c.dw, "DELAY": c.delay, "ML1": c.ml1, "ML2": c.ml2, "ML3": c.ml3}),
            );
            if s.points > 0 {
                sp.scan_window_mz = Some([c.mz(0), c.mz(s.points as usize - 1)]);
            }
        }
        if self.fs.is_file(&s.dir.join("pdata/1/1r")) {
            e.insert("processed".into(), json!("pdata/1/1r"));
        }
        Ok(sp)
    }

    /// Spectrum `i` with its points.
    pub fn spectrum(&self, i: usize) -> Result<Spectrum> {
        let mut sp = self.header(i)?;
        let spot = &self.spots[i];
        let cal = spot
            .calibration
            .as_ref()
            .map_err(|e| bad(format!("{}: {e}", spot.rel)))?;
        let fid = spot.dir.join("fid");
        let file = self.fs.open(&fid).map_err(|e| Error::io(&fid, e))?;
        let len = file.size().map_err(|e| Error::io(&fid, e))?;
        let points = spot.points.min(len / 4).min(MAX_POINTS) as usize;
        let mut bytes = vec![0u8; points * 4];
        file.read_exact_at(0, &mut bytes)
            .map_err(|e| Error::io(&fid, e))?;
        sp.mz = (0..points).map(|k| cal.mz(k)).collect();
        let big_endian = spot.big_endian;
        sp.intensity = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&word| {
                (if big_endian {
                    i32::from_be_bytes(word)
                } else {
                    i32::from_le_bytes(word)
                }) as f32
            })
            .collect();
        Ok(sp)
    }

    fn spectra_info(&self) -> SpectraInfo {
        let mut levels = BTreeSet::new();
        let mut modes: BTreeMap<&str, u64> = BTreeMap::new();
        let mut instruments = BTreeSet::new();
        let mut versions = BTreeSet::new();
        let mut types = BTreeSet::new();
        let mut points = 0u64;
        for s in &self.spots {
            levels.insert(if s.mode == "LIFT" { 2 } else { 1 });
            *modes.entry(s.mode).or_default() += 1;
            points += s.points;
            if let Some(v) = s.params.get("INSTRUM") {
                instruments.insert(v.to_string());
            }
            if let Some(v) = s.params.get("FCVer") {
                versions.insert(v.to_string());
            }
            if let Some(v) = s.params.get("InstTyp") {
                types.insert(v.to_string());
            }
        }
        let mut extra = BTreeMap::new();
        extra.insert("modes".into(), json!(modes));
        extra.insert("total_points".into(), json!(points));
        extra.insert("instruments".into(), json!(instruments));
        extra.insert("instrument_types".into(), json!(types));
        extra.insert("software_versions".into(), json!(versions));
        let spots: Vec<Value> = self
            .spots
            .iter()
            .take(1000)
            .map(|s| json!({"path": s.rel, "spot": spot_name(s), "mode": s.mode, "points": s.points}))
            .collect();
        extra.insert("spots".into(), json!(spots));
        let first = self.spots.first().map(|s| &s.params);
        SpectraInfo {
            index: 0,
            name: self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
            scan_count: self.spots.len() as u64,
            ms_levels: levels.into_iter().collect(),
            rt_range_s: None,
            instrument: Some(InstrumentInfo {
                manufacturer: Some("Bruker".into()),
                model: None,
                software: Some("flexControl".into()),
                software_version: first.and_then(|p| p.get("FCVer")).map(str::to_string),
                detector: None,
            }),
            extra,
        }
    }
}

impl Dataset for FlexDataset {
    fn info(&self) -> Result<FileInfo> {
        let size = self
            .spots
            .iter()
            .filter_map(|s| self.fs.metadata(&s.dir.join("fid")).ok())
            .map(|m| m.len())
            .sum();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: size,
            format: flex_descriptor(),
            format_version: None,
            images: Vec::new(),
            tables: Vec::new(),
            spectra: vec![self.spectra_info()],
            traces: Vec::new(),
            plane_count: 0,
            notes: self.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let spots: Vec<Value> = self
            .spots
            .iter()
            .take(200)
            .map(|s| json!({"path": s.rel, "acqus": s.params.map}))
            .collect();
        Ok(json!({"spots": spots, "spot_count": self.spots.len()}))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut m = ProvenanceMap::new();
        m.insert("spectra[0].scan_count".into(), Source::Inferred);
        m.insert("spectra[0].ms_levels".into(), Source::Inferred);
        m.insert(
            "spectra[0].instrument.software_version".into(),
            Source::Inferred,
        );
        m.insert("spectra[0].extra.modes".into(), Source::Inferred);
        m.insert("spectra[0].extra.total_points".into(), Source::PriorArt);
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .spots
            .iter()
            .take(2000)
            .enumerate()
            .map(|(i, s)| LsEntry {
                kind: "spectrum".into(),
                name: s.rel.clone(),
                offset: None,
                size: Some(s.points * 4),
                image: None,
                details: json!({"index": i, "spot": spot_name(s), "mode": s.mode}),
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FLEX_FORMAT_ID,
            "image planes",
            "Bruker flex runs hold MALDI-TOF spectra: use `openreadout spectrum` or `export --format mzml`.",
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut rep = self.check_headers()?;
        rep.performed("read every fid and computed its m/z axis");
        for i in 0..self.spots.len() {
            match self.spectrum(i) {
                Ok(sp) if sp.mz.len() as u64 == self.spots[i].points => {}
                Ok(sp) => rep.push(Finding::error(
                    "short_fid",
                    format!(
                        "{}: {} points, TD says {}",
                        self.spots[i].rel,
                        sp.mz.len(),
                        self.spots[i].points
                    ),
                )),
                Err(e) => rep.push(Finding::error(
                    "bad_spectrum",
                    format!("{}: {e}", self.spots[i].rel),
                )),
            }
        }
        Ok(rep)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.path.display().to_string(), FLEX_FORMAT_ID);
        rep.performed("found every spot directory and parsed its acqus");
        if self.truncated {
            rep.push(Finding::warning(
                "too_many_spots",
                format!("only the first {MAX_SPOTS} spots are read"),
            ));
        }
        for s in &self.spots {
            if let Err(e) = &s.calibration {
                rep.push(Finding::error("no_calibration", format!("{}: {e}", s.rel)));
            }
        }
        Ok(rep)
    }

    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        Ok((run == 0).then(|| {
            self.spots
                .iter()
                .map(|s| if s.mode == "LIFT" { 2 } else { 1 })
                .collect()
        }))
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        if run != 0 {
            return Err(Error::Usage(format!(
                "run {run} out of range (a flex run is one list of spectra)"
            )));
        }
        let start = usize::try_from(first).unwrap_or(usize::MAX);
        for i in start..self.spots.len() {
            let mut h = openreadout_core::ScanHeader::from(self.header(i)?);
            h.point_count = Some(self.spots[i].points);
            if !visit(h) {
                break;
            }
        }
        Ok(true)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "run {index} out of range (a flex run is one list of spectra)"
            )));
        }
        let i = usize::try_from(spectrum)
            .map_err(|_| Error::Usage("spectrum index too large".into()))?;
        self.spectrum(i)
    }

    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        if run != 0 {
            return Ok(None);
        }
        let i = scan_number
            .checked_sub(1)
            .filter(|i| *i < self.spots.len() as u64);
        i.map(Some).ok_or_else(|| {
            Error::Usage(format!(
                "no spectrum {scan_number}: the run has {} (numbered from 1)",
                self.spots.len()
            ))
        })
    }
}

pub(crate) fn flex_descriptor() -> FormatDescriptor {
    FormatDescriptor {
        id: FLEX_FORMAT_ID.into(),
        name: "Bruker flex MALDI-TOF (fid + acqus)".into(),
        vendor: "Bruker (flexControl: autoflex, ultraflex, microflex, MALDI Biotyper, rapifleX)"
            .into(),
        extensions: Vec::new(),
        family: "mass-spectrometry".into(),
        can_read: true,
        can_write: false,
        confidence: crate::assurance::BRUKER_FLEX.confidence,
        known_gaps: vec![
            "Input is a spot directory (or its fid / acqus) or a directory above spot directories, not a single file".into(),
            "The raw fid is read; flexAnalysis's processed spectrum (pdata/1/1r) and peak lists are not".into(),
            "Intensities are returned as 32-bit floats, exact up to 2^24".into(),
            "LIFT fragment spectra get m/z from the ML1 to ML3 constants in acqus; flexControl's fragment calibration (NTBCal) is not applied".into(),
            "The instrument model is not in the files (only the host name INSTRUM and a numeric InstTyp)".into(),
        ],
    }
}

/// The Bruker flex MALDI-TOF reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrukerFlexReader;

impl FormatReader for BrukerFlexReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::BRUKER_FLEX)
    }

    fn descriptor(&self) -> FormatDescriptor {
        flex_descriptor()
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    fn sniff_input(&self, _head: &[u8], input: &Input) -> Option<Detection> {
        // a bounded look: detection runs on every directory a user passes
        let spots = find_spots(input.fs(), input.path(), 1, 400);
        (!spots.is_empty()).then_some(Detection {
            format_id: FLEX_FORMAT_ID,
            confidence: DetectConfidence::Definite,
            note: None,
        })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        self.open_input(&Input::local(path))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(FlexDataset::open_in(input.fs(), input.path())?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
