//! The `.d` dataset: SQLite metadata, frame blobs, spectra, chromatograms.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use openreadout_core::model::{
    CheckReport, FileInfo, Finding, InstrumentInfo, LsEntry, SignalChannelInfo, SpectraInfo,
    Spectrum, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex, SpectrumView};
use openreadout_core::source::{EntryMeta, Fs, Input};
use openreadout_core::{Error, Plane, Result};

use crate::FORMAT_ID;
use crate::calibration::{self, MobilityModel, MzCalibrationRow, MzModel, TimsCalibrationRow};
use crate::frame::{
    TimsFrame, decode_tdf_frame, decode_tdf_frame_lzf, decode_tsf_profile, decode_tsf_spectrum,
    group_and_sum, read_blob,
};
use crate::sqlite::{SqlTable, SqliteDb, SqliteError};

/// Which flavour of `.d` this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimsKind {
    /// `analysis.tdf`: frames of mobility scans.
    Tdf,
    /// `analysis.tsf`: one line spectrum per frame, no mobility.
    Tsf,
}

/// One row of the `Frames` table (the columns we use).
#[derive(Debug, Clone, Default)]
pub struct FrameRecord {
    pub id: i64,
    pub time_s: f64,
    pub polarity: String,
    pub scan_mode: i64,
    pub msms_type: i64,
    pub blob_offset: u64,
    pub max_intensity: i64,
    pub summed_intensity: i64,
    pub scans: u32,
    pub peaks: u64,
    pub accumulation_ms: f64,
    pub ramp_ms: f64,
    /// `Frames.MzCalibration` / `Frames.TimsCalibration`: the calibration rows this frame uses.
    pub mz_calibration: Option<i64>,
    pub tims_calibration: Option<i64>,
    /// `Frames.T1` / `Frames.T2`: the temperatures (°C) the m/z model is compensated for.
    pub t1: Option<f64>,
    pub t2: Option<f64>,
}

/// One PASEF MS/MS selection (`PasefFrameMsMsInfo` row) or DIA window (`DiaFrameMsMsWindows` row).
#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub frame: i64,
    pub scan_begin: usize,
    pub scan_end: usize,
    pub isolation_mz: f64,
    pub isolation_width: f64,
    pub collision_energy: f64,
}

/// A DDA precursor (`Precursors` row).
#[derive(Debug, Clone, Default)]
pub struct PrecursorRecord {
    pub id: i64,
    pub monoisotopic_mz: Option<f64>,
    pub largest_peak_mz: Option<f64>,
    pub charge: Option<i64>,
    pub scan_number: Option<f64>,
    pub intensity: Option<f64>,
    pub parent_frame: Option<i64>,
}

/// A MALDI frame's spot (`MaldiFrameInfo` row).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MaldiSpot {
    pub spot: Option<String>,
    pub region: Option<i64>,
    pub x_index: Option<i64>,
    pub y_index: Option<i64>,
    /// Stage position `[x, y, z]` in µm.
    pub motor_um: [Option<f64>; 3],
    pub laser_power: Option<f64>,
    pub laser_shots: Option<i64>,
}

/// How m/z and 1/K0 are computed for this dataset, decided once at open.
#[derive(Debug, Clone, Default, PartialEq)]
struct CalibrationStatus {
    /// Every frame's MzCalibration row evaluates (else those frames use the approximation).
    mz_ok: bool,
    /// Every frame's TimsCalibration row evaluates (TDF only).
    mobility_ok: bool,
    /// Parts of the models no corpus file validated (listed once).
    unvalidated: Vec<String>,
    /// Why a model could not be evaluated, per distinct reason.
    failures: Vec<String>,
}

impl CalibrationStatus {
    fn assess(
        frames: &[FrameRecord],
        mz: &BTreeMap<i64, MzCalibrationRow>,
        tims: &BTreeMap<i64, TimsCalibrationRow>,
        kind: TimsKind,
    ) -> Self {
        let mut st = CalibrationStatus {
            mz_ok: !frames.is_empty(),
            mobility_ok: kind == TimsKind::Tdf && !frames.is_empty(),
            ..Self::default()
        };
        let mut seen = BTreeSet::new();
        for f in frames {
            let key = (
                f.mz_calibration,
                f.t1.map(f64::to_bits),
                f.t2.map(f64::to_bits),
            );
            if !seen.insert(key) {
                continue;
            }
            match f.mz_calibration.and_then(|id| mz.get(&id)) {
                None => {
                    st.mz_ok = false;
                    push_unique(
                        &mut st.failures,
                        match f.mz_calibration {
                            _ if mz.is_empty() => "the database has no MzCalibration rows".into(),
                            None => "Frames names no MzCalibration row".into(),
                            Some(id) => {
                                format!("frames name MzCalibration {id}, which the table lacks")
                            }
                        },
                    );
                }
                Some(row) => match row.model(f.t1.unwrap_or(f64::NAN), f.t2.unwrap_or(f64::NAN)) {
                    Ok(m) => {
                        if let Some(u) = m.unvalidated {
                            push_unique(&mut st.unvalidated, u);
                        }
                    }
                    Err(e) => {
                        st.mz_ok = false;
                        push_unique(&mut st.failures, e);
                    }
                },
            }
        }
        if kind == TimsKind::Tdf {
            let ids: BTreeSet<Option<i64>> = frames.iter().map(|f| f.tims_calibration).collect();
            for id in ids {
                match id.and_then(|id| tims.get(&id)) {
                    None => {
                        st.mobility_ok = false;
                        push_unique(
                            &mut st.failures,
                            match id {
                                _ if tims.is_empty() => {
                                    "the database has no TimsCalibration rows".into()
                                }
                                None => "Frames names no TimsCalibration row".into(),
                                Some(id) => format!(
                                    "frames name TimsCalibration {id}, which the table lacks"
                                ),
                            },
                        );
                    }
                    Some(row) => {
                        if let Err(e) = row.model() {
                            st.mobility_ok = false;
                            push_unique(&mut st.failures, e);
                        }
                    }
                }
            }
        }
        st
    }

    fn notes(&self, approx: bool) -> Vec<String> {
        let mut n = Vec::new();
        for u in &self.unvalidated {
            n.push(format!("m/z calibration uses {u}, which no vendor-calibrated reference file has validated: m/z values are the model's reading and may be off"));
        }
        if !self.failures.is_empty() {
            n.push(format!(
                "calibration models not applied ({}): the affected {} the acquisition-range approximation (typically tens of ppm off){}",
                self.failures.join("; "),
                match (self.mz_ok, self.mobility_ok) {
                    (false, false) => "m/z and 1/K0 values use",
                    (false, true) => "m/z values use",
                    _ => "1/K0 values use",
                },
                if approx {
                    ""
                } else {
                    " (unavailable: GlobalMetadata lacks the acquisition range)"
                }
            ));
        }
        n
    }
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

/// What one exposed spectrum is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// A whole frame, summed over its scans (MS1 and other non-PASEF frames; TSF frames).
    Frame(usize),
    /// A DDA-PASEF precursor: its selections summed across frames.
    Precursor(usize),
    /// One DIA-PASEF window of a frame: `(frame index, window index in its group)`.
    Window(usize, usize),
}

/// m/z and 1/K0 conversions (the acquisition-range approximations timsrust uses).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conversions {
    pub mz_intercept: f64,
    pub mz_slope: f64,
    pub mobility_intercept: Option<f64>,
    pub mobility_slope: Option<f64>,
}

impl Conversions {
    pub fn mz(&self, tof: f64) -> f64 {
        let r = self.mz_intercept + self.mz_slope * tof;
        r * r
    }
    pub fn mobility(&self, scan: f64) -> Option<f64> {
        Some(self.mobility_intercept? + self.mobility_slope? * scan)
    }
}

fn sql_err(path: &Path, e: SqliteError) -> Error {
    match e {
        SqliteError::Io(m) => Error::io(path, std::io::Error::other(m)),
        SqliteError::Corrupt(m) => Error::corrupt(FORMAT_ID, format!("{}: {m}", path.display())),
        SqliteError::Unsupported(m) => Error::unsupported(
            FORMAT_ID,
            m,
            "The SQLite database uses a feature this reader does not implement; please report the file.",
        ),
    }
}

fn opt_f64(t: &SqlTable, row: usize, col: &str) -> Option<f64> {
    t.value(row, col).as_f64().filter(|v| v.is_finite())
}

fn opt_i64(t: &SqlTable, row: usize, col: &str) -> Option<i64> {
    t.value(row, col).as_i64()
}

/// An open timsTOF `.d` directory.
#[derive(Debug)]
pub struct TimsDataset {
    dir: PathBuf,
    /// Where the `.d` directory is read from.
    fs: Fs,
    kind: TimsKind,
    db_path: PathBuf,
    bin_path: PathBuf,
    bin_len: u64,
    size_bytes: u64,
    metadata: BTreeMap<String, String>,
    frames: Vec<FrameRecord>,
    frame_index: HashMap<i64, usize>,
    precursors: Vec<PrecursorRecord>,
    /// Selections per precursor (same order as `precursors`), in table order.
    selections: Vec<Vec<Selection>>,
    /// DIA window group per frame id, and the windows of each group.
    dia_group: HashMap<i64, i64>,
    dia_windows: BTreeMap<i64, Vec<Selection>>,
    /// `FrameMsMsInfo` (MS/MS frames without PASEF): frame id → (trigger m/z, width, charge, energy).
    frame_msms: HashMap<i64, (f64, f64, Option<i64>, f64)>,
    spectra: Vec<Part>,
    /// The acquisition-range approximations (used only where no calibration row applies).
    conv: Option<Conversions>,
    mz_cals: BTreeMap<i64, MzCalibrationRow>,
    tims_cals: BTreeMap<i64, TimsCalibrationRow>,
    calibration: CalibrationStatus,
    maldi: HashMap<i64, MaldiSpot>,
    /// TSF: which spectra the file stores (`HasLineSpectra`, `HasProfileSpectra`).
    line_spectra: bool,
    profile_spectra: bool,
    digitizer_samples: usize,
    /// Bound on one decompressed type-1 scan.
    max_scan_bytes: usize,
    compression: i64,
    wal_commits: u32,
    table_rows: BTreeMap<String, usize>,
    vendor_tables: BTreeMap<String, Value>,
    notes: Vec<String>,
    /// Most recently decoded frame (PASEF precursors read several selections of one frame).
    cache: Option<(usize, TimsFrame)>,
}

/// The `.d` directory for `path` (the directory itself, or a file inside it).
pub fn dataset_dir(path: &Path) -> Option<PathBuf> {
    dataset_dir_in(&Fs::local(), path)
}

/// [`dataset_dir`] in the namespace `fs`.
pub(crate) fn dataset_dir_in(fs: &Fs, path: &Path) -> Option<PathBuf> {
    if fs.is_dir(path) {
        return Some(path.to_path_buf());
    }
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "analysis.tdf" | "analysis.tdf_bin" | "analysis.tsf" | "analysis.tsf_bin"
    ) {
        return path.parent().map(Path::to_path_buf);
    }
    None
}

/// Which analysis file a `.d` directory holds.
pub fn kind_of(dir: &Path) -> Option<TimsKind> {
    kind_of_in(&Fs::local(), dir)
}

/// [`kind_of`] in the namespace `fs`.
pub(crate) fn kind_of_in(fs: &Fs, dir: &Path) -> Option<TimsKind> {
    if fs.is_file(&dir.join("analysis.tdf")) {
        Some(TimsKind::Tdf)
    } else if fs.is_file(&dir.join("analysis.tsf")) {
        Some(TimsKind::Tsf)
    } else {
        None
    }
}

impl TimsDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, or a `.d` directory held in memory).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let dir = dataset_dir_in(fs, path).ok_or_else(|| {
            Error::Usage(format!("{} is not a timsTOF .d directory", path.display()))
        })?;
        let kind = kind_of_in(fs, &dir).ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                "a .d directory without analysis.tdf or analysis.tsf",
                "Only timsTOF TDF/TSF acquisitions are read; other Bruker .d data (BAF, YEP, FID) are not supported.",
            )
        })?;
        let (db_name, bin_name) = match kind {
            TimsKind::Tdf => ("analysis.tdf", "analysis.tdf_bin"),
            TimsKind::Tsf => ("analysis.tsf", "analysis.tsf_bin"),
        };
        let db_path = dir.join(db_name);
        let bin_path = dir.join(bin_name);
        let bin_len = fs
            .metadata(&bin_path)
            .map_err(|e| Error::io(&bin_path, e))?
            .len();
        let mut db = SqliteDb::open_in(fs, &db_path).map_err(|e| sql_err(&db_path, e))?;
        let mut size_bytes = bin_len;
        for f in [db_name.to_string(), format!("{db_name}-wal")] {
            size_bytes += fs.metadata(&dir.join(f)).map_or(0, |m| m.len());
        }
        let mut notes = Vec::new();
        let mut table_rows = BTreeMap::new();
        let mut read = |db: &mut SqliteDb, name: &str| -> Result<Option<SqlTable>> {
            if !db.has_table(name) {
                return Ok(None);
            }
            let t = db.read_table(name).map_err(|e| sql_err(&db_path, e))?;
            table_rows.insert(t.name.clone(), t.rows.len());
            Ok(Some(t))
        };
        let gm = read(&mut db, "GlobalMetadata")?.ok_or_else(|| {
            Error::corrupt(
                FORMAT_ID,
                format!("{} has no GlobalMetadata table", db_path.display()),
            )
        })?;
        let mut metadata = BTreeMap::new();
        for r in 0..gm.rows.len() {
            let k = gm.value(r, "Key").as_text().unwrap_or_default().to_string();
            let v = match gm.value(r, "Value") {
                crate::sqlite::SqlValue::Text(t) => t.clone(),
                crate::sqlite::SqlValue::Integer(i) => i.to_string(),
                crate::sqlite::SqlValue::Real(f) => f.to_string(),
                _ => String::new(),
            };
            metadata.insert(k, v);
        }
        let ft =
            read(&mut db, "Frames")?.ok_or_else(|| Error::corrupt(FORMAT_ID, "no Frames table"))?;
        let mut frames = Vec::with_capacity(ft.rows.len());
        for r in 0..ft.rows.len() {
            frames.push(FrameRecord {
                id: opt_i64(&ft, r, "Id").unwrap_or(r as i64 + 1),
                time_s: opt_f64(&ft, r, "Time").unwrap_or(0.0),
                polarity: ft.value(r, "Polarity").as_text().unwrap_or("").to_string(),
                scan_mode: opt_i64(&ft, r, "ScanMode").unwrap_or(-1),
                msms_type: opt_i64(&ft, r, "MsMsType").unwrap_or(0),
                blob_offset: opt_i64(&ft, r, "TimsId")
                    .and_then(|v| u64::try_from(v).ok())
                    .unwrap_or(u64::MAX),
                max_intensity: opt_i64(&ft, r, "MaxIntensity").unwrap_or(0),
                summed_intensity: opt_i64(&ft, r, "SummedIntensities").unwrap_or(0),
                scans: opt_i64(&ft, r, "NumScans")
                    .and_then(|v| u32::try_from(v).ok())
                    .unwrap_or(1),
                peaks: opt_i64(&ft, r, "NumPeaks")
                    .and_then(|v| u64::try_from(v).ok())
                    .unwrap_or(0),
                accumulation_ms: opt_f64(&ft, r, "AccumulationTime").unwrap_or(0.0),
                ramp_ms: opt_f64(&ft, r, "RampTime").unwrap_or(0.0),
                mz_calibration: opt_i64(&ft, r, "MzCalibration"),
                tims_calibration: opt_i64(&ft, r, "TimsCalibration"),
                t1: opt_f64(&ft, r, "T1"),
                t2: opt_f64(&ft, r, "T2"),
            });
        }
        frames.sort_by_key(|f| f.id);
        let frame_index: HashMap<i64, usize> =
            frames.iter().enumerate().map(|(i, f)| (f.id, i)).collect();
        // DDA-PASEF
        let mut precursors = Vec::new();
        let mut selections: Vec<Vec<Selection>> = Vec::new();
        if let Some(pt) = read(&mut db, "Precursors")? {
            for r in 0..pt.rows.len() {
                precursors.push(PrecursorRecord {
                    id: opt_i64(&pt, r, "Id").unwrap_or(r as i64 + 1),
                    monoisotopic_mz: opt_f64(&pt, r, "MonoisotopicMz"),
                    largest_peak_mz: opt_f64(&pt, r, "LargestPeakMz"),
                    charge: opt_i64(&pt, r, "Charge"),
                    scan_number: opt_f64(&pt, r, "ScanNumber"),
                    intensity: opt_f64(&pt, r, "Intensity"),
                    parent_frame: opt_i64(&pt, r, "Parent"),
                });
            }
        }
        let pidx: HashMap<i64, usize> = precursors
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id, i))
            .collect();
        selections.resize(precursors.len(), Vec::new());
        if let Some(st) = read(&mut db, "PasefFrameMsMsInfo")? {
            let mut orphans = 0usize;
            for r in 0..st.rows.len() {
                let sel = selection(&st, r, "Frame");
                match opt_i64(&st, r, "Precursor").and_then(|p| pidx.get(&p)) {
                    Some(&i) => selections[i].push(sel),
                    None => orphans += 1,
                }
            }
            for s in &mut selections {
                s.sort_by_key(|x| (x.frame, x.scan_begin));
            }
            if orphans > 0 {
                notes.push(format!(
                    "{orphans} PASEF selections name no known precursor and are ignored"
                ));
            }
        }
        // DIA-PASEF
        let mut dia_group = HashMap::new();
        if let Some(t) = read(&mut db, "DiaFrameMsMsInfo")? {
            for r in 0..t.rows.len() {
                if let (Some(f), Some(g)) = (opt_i64(&t, r, "Frame"), opt_i64(&t, r, "WindowGroup"))
                {
                    dia_group.insert(f, g);
                }
            }
        }
        let mut dia_windows: BTreeMap<i64, Vec<Selection>> = BTreeMap::new();
        if let Some(t) = read(&mut db, "DiaFrameMsMsWindows")? {
            for r in 0..t.rows.len() {
                let g = opt_i64(&t, r, "WindowGroup").unwrap_or(0);
                let mut s = selection(&t, r, "WindowGroup");
                s.frame = 0;
                dia_windows.entry(g).or_default().push(s);
            }
            for w in dia_windows.values_mut() {
                w.sort_by_key(|s| s.scan_begin);
            }
        }
        let mut frame_msms = HashMap::new();
        if let Some(t) = read(&mut db, "FrameMsMsInfo")? {
            for r in 0..t.rows.len() {
                if let Some(f) = opt_i64(&t, r, "Frame") {
                    frame_msms.insert(
                        f,
                        (
                            opt_f64(&t, r, "TriggerMass").unwrap_or(0.0),
                            opt_f64(&t, r, "IsolationWidth").unwrap_or(0.0),
                            opt_i64(&t, r, "PrecursorCharge"),
                            opt_f64(&t, r, "CollisionEnergy").unwrap_or(0.0),
                        ),
                    );
                }
            }
        }
        // tables reported verbatim under `vendor`
        let mut vendor_tables = BTreeMap::new();
        let mut mz_cals = BTreeMap::new();
        let mut tims_cals = BTreeMap::new();
        if let Some(t) = read(&mut db, "MzCalibration")? {
            for r in calibration::mz_rows(&t) {
                mz_cals.insert(r.id, r);
            }
            vendor_tables.insert(t.name.clone(), table_json(&t, 200));
        }
        if let Some(t) = read(&mut db, "TimsCalibration")? {
            for r in calibration::tims_rows(&t) {
                tims_cals.insert(r.id, r);
            }
            vendor_tables.insert(t.name.clone(), table_json(&t, 200));
        }
        // MALDI: one spot (target position, raster index, stage position) per frame.
        let mut maldi = HashMap::new();
        if let Some(t) = read(&mut db, "MaldiFrameInfo")? {
            for r in 0..t.rows.len() {
                if let Some(f) = opt_i64(&t, r, "Frame") {
                    maldi.insert(
                        f,
                        MaldiSpot {
                            spot: t.value(r, "SpotName").as_text().map(str::to_string),
                            region: opt_i64(&t, r, "RegionNumber"),
                            x_index: opt_i64(&t, r, "XIndexPos"),
                            y_index: opt_i64(&t, r, "YIndexPos"),
                            motor_um: [
                                opt_f64(&t, r, "MotorPositionX"),
                                opt_f64(&t, r, "MotorPositionY"),
                                opt_f64(&t, r, "MotorPositionZ"),
                            ],
                            laser_power: opt_f64(&t, r, "LaserPower"),
                            laser_shots: opt_i64(&t, r, "NumLaserShots"),
                        },
                    );
                }
            }
        }
        for name in [
            "Segments",
            "CalibrationInfo",
            "MaldiFrameLaserInfo",
            "PrmTargets",
        ] {
            if let Some(t) = read(&mut db, name)? {
                vendor_tables.insert(t.name.clone(), table_json(&t, 200));
            }
        }
        for name in db.tables() {
            if !table_rows.contains_key(&name)
                && let Ok(n) = db.count_rows(&name)
            {
                table_rows.insert(name, usize::try_from(n).unwrap_or(usize::MAX));
            }
        }
        // spectra in acquisition order
        let mut first_frame_of: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        for (i, sels) in selections.iter().enumerate() {
            if let Some(f) = sels.iter().map(|s| s.frame).min() {
                first_frame_of.entry(f).or_default().push(i);
            }
        }
        let mut spectra = Vec::new();
        for (fi, f) in frames.iter().enumerate() {
            if kind == TimsKind::Tsf {
                spectra.push(Part::Frame(fi));
            } else if f.msms_type == 8 {
                if let Some(ps) = first_frame_of.get(&f.id) {
                    spectra.extend(ps.iter().map(|&p| Part::Precursor(p)));
                }
            } else if f.msms_type == 9 {
                let n = dia_group
                    .get(&f.id)
                    .and_then(|g| dia_windows.get(g))
                    .map_or(0, Vec::len);
                spectra.extend((0..n).map(|w| Part::Window(fi, w)));
            } else {
                spectra.push(Part::Frame(fi));
            }
        }
        let compression = metadata
            .get("TimsCompressionType")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(if kind == TimsKind::Tsf { 0 } else { 2 });
        if kind == TimsKind::Tdf && !matches!(compression, 1 | 2) {
            notes.push(format!(
                "TimsCompressionType {compression}: frames of this compression type are not decoded (types 1 and 2 are)"
            ));
        }
        if kind == TimsKind::Tdf && compression == 1 {
            notes.push("TimsCompressionType 1 (first-generation timsTOF, LZF): a frame's blob holds more scans than Frames.NumScans; every stored scan is read (Frames.NumPeaks and SummedIntensities count them all)".into());
        }
        let flag = |k: &str| metadata.get(k).map(|v| v.trim() == "1");
        let line_spectra = flag("HasLineSpectra").unwrap_or(kind == TimsKind::Tsf);
        let profile_spectra = flag("HasProfileSpectra").unwrap_or(false);
        let digitizer_samples = metadata
            .get("DigitizerNumSamples")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let max_scan_bytes = metadata
            .get("MaxNumPeaksPerScan")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .map_or(64 << 20, |n| n.saturating_mul(8).clamp(1 << 16, 64 << 20));
        if frames.iter().any(|f| f.msms_type == 10) {
            notes.push("prm-PASEF frames (MsMsType 10) are exposed whole, all scans summed: their per-target scan ranges (PrmFrameMsMsInfo) are not split into separate spectra".into());
        }
        if metadata.get("ClosedProperly").map(String::as_str) == Some("0") {
            notes.push("the acquisition was not closed properly (ClosedProperly = 0)".into());
        }
        let wal_commits = db.wal_frames();
        if wal_commits > 0 {
            notes.push(format!(
                "{wal_commits} committed transactions were read from {db_name}-wal (the acquisition database was not checkpointed)"
            ));
        }
        let conv = conversions(&metadata, &frames);
        let calibration = CalibrationStatus::assess(&frames, &mz_cals, &tims_cals, kind);
        notes.extend(calibration.notes(conv.is_some()));
        Ok(TimsDataset {
            fs: fs.clone(),
            dir,
            kind,
            db_path,
            bin_path,
            bin_len,
            size_bytes,
            metadata,
            frames,
            frame_index,
            precursors,
            selections,
            dia_group,
            dia_windows,
            frame_msms,
            spectra,
            conv,
            mz_cals,
            tims_cals,
            calibration,
            maldi,
            line_spectra,
            profile_spectra,
            digitizer_samples,
            max_scan_bytes,
            compression,
            wal_commits,
            table_rows,
            vendor_tables,
            notes,
            cache: None,
        })
    }

    pub fn kind(&self) -> TimsKind {
        self.kind
    }

    pub fn frame_records(&self) -> &[FrameRecord] {
        &self.frames
    }

    pub fn conversions(&self) -> Option<Conversions> {
        self.conv
    }

    fn meta(&self, k: &str) -> Option<&str> {
        self.metadata
            .get(k)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// Decode frame `i` (zero-based position in `Frames`, ordered by `Id`).
    pub fn read_frame(&mut self, i: usize) -> Result<TimsFrame> {
        if let Some((c, f)) = &self.cache
            && *c == i
        {
            return Ok(f.clone());
        }
        let f = self.decode_frame(i)?;
        self.cache = Some((i, f.clone()));
        Ok(f)
    }

    fn decode_frame(&self, i: usize) -> Result<TimsFrame> {
        if self.kind != TimsKind::Tdf {
            return Err(Error::unsupported(
                FORMAT_ID,
                "mobility frames of a TSF dataset",
                "TSF data have one line spectrum per frame; read spectra instead.",
            ));
        }
        let rec = self.frames.get(i).ok_or_else(|| {
            Error::Usage(format!(
                "frame {i} out of range ({} frames)",
                self.frames.len()
            ))
        })?;
        if !matches!(self.compression, 1 | 2) {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("TDF compression type {}", self.compression),
                "Compression types 1 (LZF, first-generation timsTOF) and 2 (zstd, timsTOF Pro and later) are decoded; please report the file.",
            ));
        }
        let mut bin = self
            .fs
            .open(&self.bin_path)
            .map_err(|e| Error::io(&self.bin_path, e))?;
        let (scans_hdr, data) =
            read_blob(&mut bin, self.bin_len, rec.blob_offset).map_err(|e| {
                Error::corrupt_at(FORMAT_ID, rec.blob_offset, format!("frame {}: {e}", rec.id))
            })?;
        let frame = if self.compression == 1 {
            decode_tdf_frame_lzf(&data, scans_hdr, self.max_scan_bytes)
        } else {
            decode_tdf_frame(&data, scans_hdr.max(rec.scans))
        }
        .map_err(|e| {
            Error::corrupt_at(FORMAT_ID, rec.blob_offset, format!("frame {}: {e}", rec.id))
        })?;
        Ok(frame)
    }

    /// The m/z model of frame `fi` (its MzCalibration row at its temperatures), or `None` when
    /// the row is missing or cannot be evaluated (the acquisition-range approximation applies).
    pub fn mz_model(&self, fi: usize) -> Option<MzModel> {
        let f = self.frames.get(fi)?;
        let row = self.mz_cals.get(&f.mz_calibration?)?;
        row.model(f.t1.unwrap_or(f64::NAN), f.t2.unwrap_or(f64::NAN))
            .ok()
    }

    /// The 1/K0 model of frame `fi`, or `None` (approximation, or TSF data).
    pub fn mobility_model(&self, fi: usize) -> Option<MobilityModel> {
        let f = self.frames.get(fi)?;
        self.tims_cals.get(&f.tims_calibration?)?.model().ok()
    }

    /// 1/K0 of a (fractional) scan position of frame `fi`: the TimsCalibration model, else the
    /// acquisition-range approximation.
    fn mobility_at(&self, fi: usize, scan: f64) -> Option<f64> {
        if self.kind != TimsKind::Tdf {
            return None;
        }
        match self.mobility_model(fi) {
            Some(m) => Some(m.inverse_mobility(scan)).filter(|v| v.is_finite()),
            None => self.conv.as_ref().and_then(|c| c.mobility(scan.floor())),
        }
    }

    /// Convert TOF indices of frame `fi` to m/z, recording how in `sp.extra.mz_calibration`.
    fn to_mz(&self, fi: usize, tofs: &[f64], sp: &mut Spectrum) -> Result<Vec<f64>> {
        if let Some(m) = self.mz_model(fi) {
            sp.extra.insert(
                "mz_calibration".into(),
                json!(format!(
                    "MzCalibration {} (model type {}), frame {} temperatures applied{}",
                    m.calibration_id,
                    m.model_type,
                    self.frames[fi].id,
                    if m.unvalidated.is_some() {
                        " (unvalidated model variant)"
                    } else {
                        ""
                    }
                )),
            );
            return Ok(tofs.iter().map(|&t| m.mz(t)).collect());
        }
        let conv = self.conv.ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                "m/z conversion without a calibration row or the acquisition range",
                "The frame's MzCalibration row is missing or unreadable and GlobalMetadata lacks MzAcqRangeLower/Upper and DigitizerNumSamples; frames are still readable.",
            )
        })?;
        sp.extra.insert(
            "mz_calibration".into(),
            json!("acquisition-range approximation (no usable MzCalibration row; typically tens of ppm off)"),
        );
        Ok(tofs.iter().map(|&t| conv.mz(t)).collect())
    }

    /// Metadata of spectrum `index` from the SQLite tables alone (no frame is read): RT and
    /// polarity of its (first) frame, level, precursor, isolation, collision energy; the TIC of
    /// a whole MS frame (`Frames.SummedIntensities`); 1/K0 when the mobility calibration is
    /// known.
    fn spectrum_meta(&self, index: usize) -> Result<(Part, Spectrum)> {
        let src = *self.spectra.get(index).ok_or_else(|| {
            Error::Usage(format!(
                "spectrum index {index} out of range ({} spectra)",
                self.spectra.len()
            ))
        })?;
        let mut sp = Spectrum {
            index: index as u64,
            scan_number: index as u64 + 1,
            centroided: false,
            ..Spectrum::default()
        };
        let frame_meta = |sp: &mut Spectrum, f: &FrameRecord| {
            sp.rt_s = Some(f.time_s);
            sp.polarity = match f.polarity.as_str() {
                "+" => "positive",
                "-" => "negative",
                _ => "unknown",
            }
            .into();
            sp.extra.insert("frame".into(), json!(f.id));
            sp.extra.insert("msms_type".into(), json!(f.msms_type));
            sp.extra
                .insert("accumulation_time_ms".into(), json!(f.accumulation_ms));
            if let Some(m) = self.maldi.get(&f.id) {
                if let Some(s) = &m.spot {
                    sp.extra.insert("maldi_spot".into(), json!(s));
                }
                if let (Some(x), Some(y)) = (m.x_index, m.y_index) {
                    sp.extra.insert("position".into(), json!([x, y]));
                }
                if m.motor_um.iter().any(Option::is_some) {
                    sp.extra
                        .insert("stage_position_um".into(), json!(m.motor_um));
                }
                if let Some(r) = m.region {
                    sp.extra.insert("maldi_region".into(), json!(r));
                }
                if let Some(p) = m.laser_power {
                    sp.extra.insert("laser_power".into(), json!(p));
                }
                if let Some(n) = m.laser_shots {
                    sp.extra.insert("laser_shots".into(), json!(n));
                }
            }
        };
        match src {
            Part::Frame(fi) => {
                let f = &self.frames[fi];
                frame_meta(&mut sp, f);
                sp.native_id = Some(format!("frame={}", f.id));
                sp.ms_level = if f.msms_type == 0 { 1 } else { 2 };
                sp.total_ion_current = Some(f.summed_intensity as f64);
                if let Some(&(mz, w, z, ce)) = self.frame_msms.get(&f.id) {
                    sp.precursor_mz = Some(mz);
                    sp.isolation_window_mz = Some([mz - w / 2.0, mz + w / 2.0]);
                    sp.precursor_charge = z.and_then(|z| i32::try_from(z).ok()).filter(|z| *z != 0);
                    sp.collision_energy = Some(ce);
                    sp.activation = Some("CID".into());
                }
                if self.kind == TimsKind::Tsf {
                    sp.centroided = !self.profile_spectra;
                }
            }
            Part::Precursor(pi) => {
                let p = &self.precursors[pi];
                let sels = &self.selections[pi];
                let first = sels.iter().map(|s| s.frame).min().unwrap_or(0);
                if let Some(&fi) = self.frame_index.get(&first) {
                    frame_meta(&mut sp, &self.frames[fi]);
                }
                sp.native_id = Some(format!("precursor={}", p.id));
                sp.ms_level = 2;
                sp.precursor_mz = p.monoisotopic_mz.or(p.largest_peak_mz);
                sp.precursor_charge = p
                    .charge
                    .and_then(|z| i32::try_from(z).ok())
                    .filter(|z| *z != 0);
                sp.precursor_intensity = p.intensity;
                sp.activation = Some("CID".into());
                if let Some(s) = sels.first() {
                    sp.isolation_window_mz = Some([
                        s.isolation_mz - s.isolation_width / 2.0,
                        s.isolation_mz + s.isolation_width / 2.0,
                    ]);
                    sp.collision_energy = Some(s.collision_energy);
                    sp.extra
                        .insert("isolation_target_mz".into(), json!(s.isolation_mz));
                }
                // The precursor's (fractional) apex scan, in its parent frame's calibration.
                let cal_frame = p
                    .parent_frame
                    .or(Some(first))
                    .and_then(|id| self.frame_index.get(&id).copied());
                sp.inverse_reduced_mobility = match (p.scan_number, cal_frame) {
                    (Some(s), Some(fi)) if s.is_finite() && s >= 0.0 => self.mobility_at(fi, s),
                    _ => None,
                };
                sp.extra.insert("precursor".into(), json!(p.id));
                if let Some(parent) = p.parent_frame {
                    sp.extra.insert("parent_frame".into(), json!(parent));
                }
                sp.extra.insert(
                    "pasef_frames".into(),
                    json!(sels.iter().map(|s| s.frame).collect::<BTreeSet<_>>()),
                );
            }
            Part::Window(fi, wi) => {
                let f = &self.frames[fi];
                frame_meta(&mut sp, f);
                let w = self
                    .dia_group
                    .get(&f.id)
                    .and_then(|g| self.dia_windows.get(g))
                    .and_then(|ws| ws.get(wi))
                    .cloned()
                    .unwrap_or_default();
                sp.native_id = Some(format!("frame={} window={}", f.id, wi + 1));
                sp.ms_level = 2;
                sp.activation = Some("CID".into());
                sp.isolation_window_mz = Some([
                    w.isolation_mz - w.isolation_width / 2.0,
                    w.isolation_mz + w.isolation_width / 2.0,
                ]);
                sp.collision_energy = Some(w.collision_energy);
                sp.extra
                    .insert("isolation_target_mz".into(), json!(w.isolation_mz));
                sp.extra
                    .insert("window_group".into(), json!(self.dia_group.get(&f.id)));
                sp.extra
                    .insert("scan_range".into(), json!([w.scan_begin, w.scan_end]));
                if let (Some(a), Some(b)) = (
                    self.mobility_at(fi, w.scan_begin as f64),
                    self.mobility_at(fi, w.scan_end.saturating_sub(1) as f64),
                ) {
                    sp.extra.insert(
                        "inverse_reduced_mobility_range".into(),
                        json!([a.min(b), a.max(b)]),
                    );
                }
            }
        }
        Ok((src, sp))
    }

    /// Read a TSF frame's blob: the line spectrum (fractional TOF indices) or, with
    /// `profile`, the profile (every digitizer sample; zero runs dropped except the samples
    /// next to signal and the two ends of the range).
    fn tsf_frame(&self, fi: usize, profile: bool) -> Result<(Vec<f64>, Vec<f32>)> {
        let f = &self.frames[fi];
        let mut bin = self
            .fs
            .open(&self.bin_path)
            .map_err(|e| Error::io(&self.bin_path, e))?;
        let bad = |e: crate::frame::FrameError| {
            Error::corrupt_at(FORMAT_ID, f.blob_offset, format!("frame {}: {e}", f.id))
        };
        let (clen, data) = read_blob(&mut bin, self.bin_len, f.blob_offset).map_err(bad)?;
        if !profile {
            let peaks = usize::try_from(f.peaks).unwrap_or(0);
            return decode_tsf_spectrum(clen, &data, peaks).map_err(bad);
        }
        let samples = decode_tsf_profile(clen, &data, self.digitizer_samples).map_err(bad)?;
        let n = samples.len();
        let mut tofs = Vec::new();
        let mut ints = Vec::new();
        for (i, &v) in samples.iter().enumerate() {
            let keep = v != 0
                || i == 0
                || i + 1 == n
                || samples.get(i + 1).is_some_and(|&w| w != 0)
                || i.checked_sub(1).is_some_and(|j| samples[j] != 0);
            if keep {
                tofs.push(i as f64);
                ints.push(v as f32);
            }
        }
        Ok((tofs, ints))
    }

    fn spectrum_at(&mut self, index: usize, view: SpectrumView) -> Result<Spectrum> {
        let (src, mut sp) = self.spectrum_meta(index)?;
        let tofs: Vec<f64>;
        let ints: Vec<f32>;
        let cal_frame: usize;
        match src {
            Part::Frame(fi) => {
                cal_frame = fi;
                if self.kind == TimsKind::Tsf {
                    // Primary is the profile when the file stores one; Centroid (or a file
                    // with line spectra only) the instrument's line spectrum.
                    let profile = self.profile_spectra
                        && (view == SpectrumView::Primary || !self.line_spectra);
                    (tofs, ints) = self.tsf_frame(fi, profile)?;
                    sp.centroided = !profile;
                    sp.extra.insert(
                        "tsf_spectrum".into(),
                        json!(if profile { "profile" } else { "line" }),
                    );
                } else {
                    let fr = self.read_frame(fi)?;
                    let (t, s) =
                        group_and_sum([(fr.tof_indices.as_slice(), fr.intensities.as_slice())]);
                    tofs = t.iter().map(|&x| f64::from(x)).collect();
                    ints = s.iter().map(|&x| x as f32).collect();
                }
            }
            Part::Precursor(pi) => {
                let p = self.precursors[pi].clone();
                let sels = self.selections[pi].clone();
                let mut parts: Vec<(Vec<u32>, Vec<u32>)> = Vec::new();
                let mut first_fi = None;
                for s in &sels {
                    let Some(&fi) = self.frame_index.get(&s.frame) else {
                        return Err(Error::corrupt(
                            FORMAT_ID,
                            format!(
                                "precursor {} names frame {} which is not in Frames",
                                p.id, s.frame
                            ),
                        ));
                    };
                    first_fi.get_or_insert(fi);
                    let fr = self.read_frame(fi)?;
                    let (t, i) = fr.scan_range(s.scan_begin, s.scan_end);
                    parts.push((t.to_vec(), i.to_vec()));
                }
                // Summed per TOF index across the PASEF frames, converted with the first
                // frame's calibration (frames seconds apart differ by far less than 0.1 ppm).
                cal_frame = first_fi.unwrap_or(0);
                let (bins, sums) =
                    group_and_sum(parts.iter().map(|(t, i)| (t.as_slice(), i.as_slice())));
                tofs = bins.iter().map(|&x| f64::from(x)).collect();
                ints = sums.iter().map(|&x| x as f32).collect();
            }
            Part::Window(fi, wi) => {
                cal_frame = fi;
                let f = self.frames[fi].clone();
                let w = self
                    .dia_group
                    .get(&f.id)
                    .and_then(|g| self.dia_windows.get(g))
                    .and_then(|ws| ws.get(wi))
                    .cloned()
                    .unwrap_or_default();
                let fr = self.read_frame(fi)?;
                let (bins, sums) = group_and_sum([fr.scan_range(w.scan_begin, w.scan_end)]);
                tofs = bins.iter().map(|&x| f64::from(x)).collect();
                ints = sums.iter().map(|&x| x as f32).collect();
            }
        }
        sp.mz = self.to_mz(cal_frame, &tofs, &mut sp)?;
        sp.intensity = ints;
        if !matches!(src, Part::Frame(_)) || self.kind == TimsKind::Tsf {
            let tic: f64 = sp.intensity.iter().map(|&v| f64::from(v)).sum();
            if sp.total_ion_current.is_none() {
                sp.total_ion_current = Some(tic);
            }
        }
        Ok(sp)
    }

    fn acquisition(&self) -> &'static str {
        if self.kind == TimsKind::Tsf {
            "line spectra (TSF)"
        } else if self.frames.iter().any(|f| f.msms_type == 8) {
            "DDA-PASEF"
        } else if self.frames.iter().any(|f| f.msms_type == 9) {
            "DIA-PASEF"
        } else if self.frames.iter().all(|f| f.msms_type == 0) {
            "MS1 only"
        } else {
            "other"
        }
    }

    fn spectra_info(&self) -> SpectraInfo {
        let mut levels = BTreeSet::new();
        let mut counts: BTreeMap<u32, u64> = BTreeMap::new();
        for s in &self.spectra {
            let l = match s {
                Part::Frame(fi) if self.frames[*fi].msms_type == 0 => 1,
                _ => 2,
            };
            levels.insert(l);
            *counts.entry(l).or_default() += 1;
        }
        let rt = match (self.frames.first(), self.frames.last()) {
            (Some(a), Some(b)) => Some([a.time_s, b.time_s]),
            _ => None,
        };
        let mut extra = BTreeMap::new();
        let mut put = |k: &str, v: Value| {
            if !v.is_null() {
                extra.insert(k.to_string(), v);
            }
        };
        put("acquisition", json!(self.acquisition()));
        put(
            "data_kind",
            json!(match self.kind {
                TimsKind::Tdf => "tdf",
                TimsKind::Tsf => "tsf",
            }),
        );
        put("frame_count", json!(self.frames.len()));
        put(
            "ms1_frames",
            json!(self.frames.iter().filter(|f| f.msms_type == 0).count()),
        );
        put(
            "pasef_frames",
            json!(self.frames.iter().filter(|f| f.msms_type == 8).count()),
        );
        put(
            "dia_frames",
            json!(self.frames.iter().filter(|f| f.msms_type == 9).count()),
        );
        put("precursor_count", json!(self.precursors.len()));
        put("dia_window_groups", json!(self.dia_windows.len()));
        // `Frames.Polarity` (`+`/`-`) of every frame; other codes are not named
        let pols: std::collections::BTreeSet<&str> = self
            .frames
            .iter()
            .filter_map(|f| match f.polarity.trim() {
                "+" => Some("positive"),
                "-" => Some("negative"),
                _ => None,
            })
            .collect();
        if !pols.is_empty() {
            put("polarities", json!(pols));
        }
        put("ms_level_counts", json!(counts));
        put("acquired_at", json!(self.meta("AcquisitionDateTime")));
        put("sample_name", json!(self.meta("SampleName")));
        put("method", json!(self.meta("MethodName")));
        put("operator", json!(self.meta("OperatorName")));
        put(
            "instrument_serial",
            json!(self.meta("InstrumentSerialNumber")),
        );
        put(
            "schema",
            json!(format!(
                "{} {}.{}",
                self.meta("SchemaType").unwrap_or("?"),
                self.meta("SchemaVersionMajor").unwrap_or("?"),
                self.meta("SchemaVersionMinor").unwrap_or("?")
            )),
        );
        put("compression_type", json!(self.compression));
        put(
            "closed_properly",
            json!(self.meta("ClosedProperly").map(|v| v == "1")),
        );
        put("wal_commits_applied", json!(self.wal_commits));
        let mzr = (
            self.meta("MzAcqRangeLower")
                .and_then(|v| v.parse::<f64>().ok()),
            self.meta("MzAcqRangeUpper")
                .and_then(|v| v.parse::<f64>().ok()),
        );
        if let (Some(a), Some(b)) = mzr {
            put("mz_acquisition_range", json!([a, b]));
        }
        let imr = (
            self.meta("OneOverK0AcqRangeLower")
                .and_then(|v| v.parse::<f64>().ok()),
            self.meta("OneOverK0AcqRangeUpper")
                .and_then(|v| v.parse::<f64>().ok()),
        );
        if let (Some(a), Some(b)) = imr {
            put("inverse_reduced_mobility_range", json!([a, b]));
        }
        put(
            "scans_per_frame",
            json!(self.frames.iter().map(|f| f.scans).max()),
        );
        put(
            "mz_conversion",
            json!(if !self.calibration.mz_ok {
                "acquisition-range approximation (MzCalibration not applicable to every frame; see notes)"
            } else if self.calibration.unvalidated.is_empty() {
                "MzCalibration model per frame, temperature-compensated (validated)"
            } else {
                "MzCalibration model per frame, temperature-compensated (unvalidated model variant; see notes)"
            }),
        );
        if self.kind == TimsKind::Tdf {
            put(
                "mobility_conversion",
                json!(if self.calibration.mobility_ok {
                    "TimsCalibration model per frame (validated)"
                } else {
                    "acquisition-range approximation (TimsCalibration not applicable; see notes)"
                }),
            );
        }
        if let Some(m) = self.mz_model(0) {
            put("mz_calibration", m.describe());
        }
        if let Some(m) = self.mobility_model(0) {
            put("mobility_calibration", m.describe());
        }
        if self.kind == TimsKind::Tsf {
            put(
                "tsf_spectra",
                json!({"line": self.line_spectra, "profile": self.profile_spectra}),
            );
        }
        if !self.maldi.is_empty() {
            put("maldi_spots", json!(self.maldi.len()));
            put(
                "maldi_application",
                json!(self.meta("MaldiApplicationType")),
            );
        }
        SpectraInfo {
            index: 0,
            name: self.meta("SampleName").map(str::to_string).or_else(|| {
                self.dir
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
            }),
            scan_count: self.spectra.len() as u64,
            ms_levels: levels.into_iter().collect(),
            rt_range_s: rt,
            instrument: Some(InstrumentInfo {
                manufacturer: self.meta("InstrumentVendor").map(str::to_string),
                model: self.meta("InstrumentName").map(str::to_string),
                software: self.meta("AcquisitionSoftware").map(str::to_string),
                software_version: self.meta("AcquisitionSoftwareVersion").map(str::to_string),
                detector: None,
            }),
            extra,
        }
    }

    fn ms1_frames(&self) -> Vec<&FrameRecord> {
        self.frames.iter().filter(|f| f.msms_type == 0).collect()
    }

    fn traces(&self) -> Vec<TraceInfo> {
        let n = self.ms1_frames().len() as u64;
        if n == 0 {
            return Vec::new();
        }
        ["TIC", "BPC"]
            .iter()
            .enumerate()
            .map(|(i, name)| TraceInfo {
                index: i as u32,
                name: Some((*name).to_string()),
                sample_rate_hz: 0.0,
                sample_count: n,
                sweep_count: 1,
                channels: vec![
                    SignalChannelInfo {
                        index: 0,
                        name: "time".into(),
                        unit: Some("s".into()),
                        dtype: "float64".into(),
                        scale: 1.0,
                        offset: 0.0,
                        extra: BTreeMap::new(),
                    },
                    SignalChannelInfo {
                        index: 1,
                        name: "intensity".into(),
                        unit: Some("counts".into()),
                        dtype: "float64".into(),
                        scale: 1.0,
                        offset: 0.0,
                        extra: BTreeMap::new(),
                    },
                ],
                start_s: self.ms1_frames().first().map(|f| f.time_s),
                extra: {
                    let mut e = BTreeMap::new();
                    e.insert(
                        "chromatogram_type".into(),
                        json!(if i == 0 {
                            "total ion current (MS1 frames, Frames.SummedIntensities)"
                        } else {
                            "base peak (MS1 frames, Frames.MaxIntensity)"
                        }),
                    );
                    e.insert(
                        "irregular_sampling".into(),
                        json!("sample_rate_hz is 0: channel `time` holds each sample's time in seconds"),
                    );
                    e
                },
            })
            .collect()
    }

    /// Warnings for m/z or 1/K0 values that do not come from a validated calibration model.
    fn calibration_findings(&self, rep: &mut CheckReport) {
        rep.performed("evaluated every frame's MzCalibration and TimsCalibration model");
        for u in &self.calibration.unvalidated {
            rep.push(Finding::warning(
                "calibration_unvalidated",
                format!("the m/z calibration uses {u}, which no vendor-calibrated reference file has validated; m/z values may be off"),
            ));
        }
        if !self.calibration.failures.is_empty() {
            rep.push(Finding::warning(
                "calibration_not_applied",
                format!(
                    "{}: the acquisition-range approximation is used instead (typically tens of ppm off in m/z)",
                    self.calibration.failures.join("; ")
                ),
            ));
        }
    }

    fn run_check(&mut self) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.dir.display().to_string(), FORMAT_ID);
        rep.performed("read the SQLite database (and its write-ahead log, if any)");
        if self.meta("ClosedProperly") == Some("0") {
            rep.push(Finding::warning(
                "not_closed_properly",
                "GlobalMetadata says ClosedProperly = 0: the acquisition was interrupted or the data were copied while open",
            ));
        }
        self.calibration_findings(&mut rep);
        if self.kind == TimsKind::Tdf && !matches!(self.compression, 1 | 2) {
            rep.push(Finding::warning(
                "unsupported_compression",
                format!(
                    "TimsCompressionType {}: frames were not decoded",
                    self.compression
                ),
            ));
            return Ok(rep);
        }
        rep.performed(
            "decoded every frame blob and compared its scan and peak counts with the Frames table",
        );
        let mut bad = 0usize;
        for i in 0..self.frames.len() {
            let rec = self.frames[i].clone();
            let res: Result<(u64, u64, u64)> = if self.kind == TimsKind::Tdf {
                self.decode_frame(i).and_then(|f| {
                    if self.compression == 2 && f.scan_count() != rec.scans as usize {
                        return Err(Error::corrupt(
                            FORMAT_ID,
                            format!(
                                "{} scans, Frames.NumScans says {}",
                                f.scan_count(),
                                rec.scans
                            ),
                        ));
                    }
                    let s: u64 = f.intensities.iter().map(|&v| u64::from(v)).sum();
                    let m = f.intensities.iter().copied().max().map_or(0, u64::from);
                    Ok((f.intensities.len() as u64, s, m))
                })
            } else {
                let mut bin = self
                    .fs
                    .open(&self.bin_path)
                    .map_err(|e| Error::io(&self.bin_path, e))?;
                read_blob(&mut bin, self.bin_len, rec.blob_offset)
                    .map_err(|e| Error::corrupt(FORMAT_ID, e.to_string()))
                    .and_then(|(c, d)| {
                        decode_tsf_spectrum(c, &d, usize::try_from(rec.peaks).unwrap_or(0))
                            .map_err(|e| Error::corrupt(FORMAT_ID, e.to_string()))
                    })
                    .map(|(t, _)| (t.len() as u64, rec.summed_intensity.max(0) as u64, 0))
            };
            match res {
                Ok((peaks, sum, _max)) => {
                    if peaks != rec.peaks {
                        bad += 1;
                        if bad == 1 {
                            rep.push(
                                Finding::error(
                                    "peak_count_mismatch",
                                    format!(
                                        "frame {}: {peaks} peaks decoded, Frames.NumPeaks says {}",
                                        rec.id, rec.peaks
                                    ),
                                )
                                .at(rec.blob_offset),
                            );
                        }
                    }
                    let _ = sum;
                }
                Err(e) => {
                    bad += 1;
                    if bad <= 3 {
                        rep.push(
                            Finding::error("bad_frame", format!("frame {}: {e}", rec.id))
                                .at(rec.blob_offset),
                        );
                    }
                }
            }
        }
        if bad > 3 {
            rep.push(Finding::error(
                "bad_frame",
                format!("{bad} frames failed in total"),
            ));
        }
        rep.performed("checked that every PASEF selection and DIA window names an existing frame and scan range");
        let mut dangling = 0usize;
        for sels in &self.selections {
            for s in sels {
                match self.frame_index.get(&s.frame) {
                    Some(&fi)
                        if s.scan_begin <= s.scan_end
                            && s.scan_end <= self.frames[fi].scans as usize => {}
                    _ => dangling += 1,
                }
            }
        }
        for (f, g) in &self.dia_group {
            if !self.frame_index.contains_key(f) || !self.dia_windows.contains_key(g) {
                dangling += 1;
            }
        }
        if dangling > 0 {
            rep.push(Finding::error(
                "dangling_reference",
                format!("{dangling} PASEF selections or DIA window assignments point at missing frames, windows or scans"),
            ));
        }
        Ok(rep)
    }
}

fn selection(t: &SqlTable, r: usize, frame_col: &str) -> Selection {
    let u = |c: &str| {
        t.value(r, c)
            .as_i64()
            .and_then(|v| usize::try_from(v).ok())
            .unwrap_or(0)
    };
    Selection {
        frame: t.value(r, frame_col).as_i64().unwrap_or(0),
        scan_begin: u("ScanNumBegin"),
        scan_end: u("ScanNumEnd"),
        isolation_mz: t.value(r, "IsolationMz").as_f64().unwrap_or(0.0),
        isolation_width: t.value(r, "IsolationWidth").as_f64().unwrap_or(0.0),
        collision_energy: t.value(r, "CollisionEnergy").as_f64().unwrap_or(0.0),
    }
}

fn table_json(t: &SqlTable, max_rows: usize) -> Value {
    let rows: Vec<Value> = t
        .rows
        .iter()
        .take(max_rows)
        .map(|r| {
            let mut m = serde_json::Map::new();
            for (c, v) in t.columns.iter().zip(r) {
                m.insert(c.clone(), v.to_json());
            }
            Value::Object(m)
        })
        .collect();
    json!({"columns": t.columns, "row_count": t.rows.len(), "rows": rows})
}

/// The conversions timsrust applies (see `docs/formats/bruker-tdf.md`).
fn conversions(meta: &BTreeMap<String, String>, frames: &[FrameRecord]) -> Option<Conversions> {
    let f = |k: &str| meta.get(k).and_then(|v| v.trim().parse::<f64>().ok());
    let mut lo = f("MzAcqRangeLower")?;
    let mut hi = f("MzAcqRangeUpper")?;
    let samples = f("DigitizerNumSamples")?;
    if meta.get("AcquisitionSoftware").map(String::as_str) == Some("Bruker otofControl") {
        lo -= 5.0;
        hi += 5.0;
    }
    if !(lo >= 0.0 && hi > lo && samples > 0.0) {
        return None;
    }
    let intercept = lo.sqrt();
    let slope = (hi.sqrt() - intercept) / samples;
    let (mut mi, mut ms) = (None, None);
    if let (Some(im_lo), Some(im_hi)) = (f("OneOverK0AcqRangeLower"), f("OneOverK0AcqRangeUpper")) {
        let max_scan = frames.iter().map(|f| f.scans).max().unwrap_or(0);
        if max_scan > 1 {
            mi = Some(im_hi);
            ms = Some((im_lo - im_hi) / f64::from(max_scan - 1));
        }
    }
    Some(Conversions {
        mz_intercept: intercept,
        mz_slope: slope,
        mobility_intercept: mi,
        mobility_slope: ms,
    })
}

impl Dataset for TimsDataset {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.dir.display().to_string(),
            size_bytes: self.size_bytes,
            format: crate::BrukerTimsReader.descriptor(),
            format_version: match (
                self.meta("SchemaVersionMajor"),
                self.meta("SchemaVersionMinor"),
            ) {
                (Some(a), Some(b)) => Some(format!(
                    "{} {a}.{b}",
                    self.meta("SchemaType").unwrap_or(match self.kind {
                        TimsKind::Tdf => "TDF",
                        TimsKind::Tsf => "TSF",
                    })
                )),
                _ => None,
            },
            images: Vec::new(),
            tables: Vec::new(),
            spectra: vec![self.spectra_info()],
            traces: self.traces(),
            plane_count: 0,
            notes: self.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut m = serde_json::Map::new();
        m.insert("GlobalMetadata".into(), json!(self.metadata));
        for (k, v) in &self.vendor_tables {
            m.insert(k.clone(), v.clone());
        }
        m.insert("table_rows".into(), json!(self.table_rows));
        Ok(Value::Object(m))
    }

    fn provenance(&self) -> ProvenanceMap {
        // Keys are paths into `info`. Spectrum arrays (m/z, 1/K0 from the acquisition-range
        // approximations of timsrust) are PriorArt; see docs/formats/bruker-tdf.md.
        let mut m = ProvenanceMap::new();
        for k in [
            "spectra[0].scan_count",
            "spectra[0].ms_levels",
            "spectra[0].extra.acquisition",
            "spectra[0].extra.ms_level_counts",
        ] {
            m.insert(k.into(), Source::PriorArt);
        }
        for k in [
            "spectra[0].rt_range_s",
            "spectra[0].instrument.model",
            "spectra[0].extra.acquired_at",
            "spectra[0].extra.mz_acquisition_range",
            "traces[].name",
        ] {
            m.insert(k.into(), Source::Inferred);
        }
        m
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        if let Ok(rd) = self.fs.read_dir(&self.dir) {
            let mut files: Vec<(String, u64, bool)> = rd
                .flatten()
                .map(|e| {
                    let md = e.metadata().ok();
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        md.as_ref().map_or(0, EntryMeta::len),
                        md.is_some_and(|m| m.is_dir()),
                    )
                })
                .collect();
            files.sort();
            for (name, size, is_dir) in files {
                out.push(LsEntry {
                    kind: if is_dir { "directory" } else { "file" }.into(),
                    name,
                    offset: None,
                    size: (!is_dir).then_some(size),
                    image: None,
                    details: Value::Null,
                });
            }
        }
        for (t, n) in &self.table_rows {
            out.push(LsEntry {
                kind: "table".into(),
                name: t.clone(),
                offset: None,
                size: None,
                image: None,
                details: json!({"rows": n}),
            });
        }
        for f in self.frames.iter().take(1000) {
            out.push(LsEntry {
                kind: "frame".into(),
                name: format!("frame {}", f.id),
                offset: Some(f.blob_offset),
                size: None,
                image: None,
                details: json!({"time_s": f.time_s, "msms_type": f.msms_type, "scans": f.scans, "peaks": f.peaks}),
            });
        }
        if self.frames.len() > 1000 {
            out.push(LsEntry {
                kind: "frame".into(),
                name: format!("… {} more frames", self.frames.len() - 1000),
                offset: None,
                size: None,
                image: None,
                details: json!({"listed": 1000, "total": self.frames.len()}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "timsTOF data are mass spectra: use `openreadout spectra` or `export --format mzml`.",
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        self.run_check()
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut rep = CheckReport::new(self.dir.display().to_string(), FORMAT_ID);
        rep.performed("read the SQLite database (and its write-ahead log, if any)");
        if self.meta("ClosedProperly") == Some("0") {
            rep.push(Finding::warning(
                "not_closed_properly",
                "GlobalMetadata says ClosedProperly = 0: the acquisition was interrupted or the data were copied while open",
            ));
        }
        self.calibration_findings(&mut rep);
        rep.performed(
            "every frame's blob offset lies inside the binary file (frames not decoded: headers-only check)",
        );
        let past: Vec<usize> = self
            .frames
            .iter()
            .enumerate()
            .filter(|(_, f)| f.blob_offset >= self.bin_len)
            .map(|(i, _)| i)
            .collect();
        if let Some(first) = past.first() {
            rep.push(Finding::error(
                "truncated",
                format!(
                    "{} of {} frames start past the end of {} ({} bytes; first: frame {first})",
                    past.len(),
                    self.frames.len(),
                    self.bin_path.display(),
                    self.bin_len
                ),
            ));
        }
        Ok(rep)
    }

    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        Ok((run == 0).then(|| {
            self.spectra
                .iter()
                .map(|p| match p {
                    Part::Frame(fi) => {
                        if self.frames.get(*fi).is_some_and(|f| f.msms_type == 0) {
                            1
                        } else {
                            2
                        }
                    }
                    Part::Precursor(_) | Part::Window(..) => 2,
                })
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
                "run {run} out of range (a .d holds one run)"
            )));
        }
        for i in usize::try_from(first).unwrap_or(usize::MAX)..self.spectra.len() {
            let (src, sp) = self.spectrum_meta(i)?;
            let mut h = openreadout_core::ScanHeader::from(sp);
            // The stored TIC covers a whole MS frame; a PASEF precursor or DIA window sums
            // part of one (computed from the peaks by `spectra`, so not listed here).
            if let Part::Frame(fi) = src {
                let f = &self.frames[fi];
                h.base_peak_intensity = Some(f.max_intensity as f64).filter(|v| *v > 0.0);
                if self.kind == TimsKind::Tsf && !self.profile_spectra {
                    h.point_count = Some(f.peaks);
                }
            }
            if !visit(h) {
                break;
            }
        }
        Ok(true)
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        self.read_spectrum_view(index, spectrum, SpectrumView::Primary)
    }

    fn read_spectrum_view(
        &mut self,
        index: u32,
        spectrum: u64,
        view: SpectrumView,
    ) -> Result<Spectrum> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "run {index} out of range (a .d holds one run)"
            )));
        }
        self.spectrum_at(usize::try_from(spectrum).unwrap_or(usize::MAX), view)
    }

    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        if run != 0 || scan_number == 0 || scan_number > self.spectra.len() as u64 {
            return Err(Error::Usage(format!(
                "no spectrum {scan_number}: spectra are numbered 1..{} (see `info --view structure`)",
                self.spectra.len()
            )));
        }
        Ok(Some(scan_number - 1))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if sweep != 0 || index > 1 || self.ms1_frames().is_empty() {
            return Err(Error::Usage(format!(
                "trace {index} sweep {sweep} does not exist (traces 0 TIC, 1 BPC; one sweep)"
            )));
        }
        let start = usize::try_from(first_sample).unwrap_or(usize::MAX);
        let take = usize::try_from(max_samples).unwrap_or(usize::MAX);
        let ms1 = self.ms1_frames();
        let rows: Vec<&&FrameRecord> = ms1.iter().skip(start).take(take).collect();
        Ok(Trace {
            trace: index,
            sweep,
            first_sample,
            channels: vec![
                rows.iter().map(|f| f.time_s).collect(),
                rows.iter()
                    .map(|f| {
                        if index == 0 {
                            f.summed_intensity as f64
                        } else {
                            f.max_intensity as f64
                        }
                    })
                    .collect(),
            ],
        })
    }
}

impl TimsDataset {
    /// Path of the SQLite database.
    pub fn database_path(&self) -> &Path {
        &self.db_path
    }
}
