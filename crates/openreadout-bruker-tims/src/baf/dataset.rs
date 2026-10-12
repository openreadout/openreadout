//! `Dataset` for a BAF `.d` directory: one run of spectra (profile and centroid views), m/z from
//! the spectrum's calibration with temperature drift compensation, or from the recalibration in
//! `Calibrator.ami` when the directory holds one.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::le_u32;
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, InstrumentInfo, LsEntry, Spectrum, SpectraInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex, SpectrumView};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use super::calibration::{
    Calibration, DriftSettings, ReferenceCalibration, TofCalibration, find_recalibration,
    parse_payload, parse_reference,
};
use super::records::{
    AcquisitionHeader, Pointers, Segment, SummaryEntry, TAG_ACQUISITION, TAG_COMPRESSOR,
    TAG_LINES, TAG_POINTERS, TAG_PROFILE, TAG_SEGMENT, TAG_SPECTRUM, TAG_SUMMARY, TAG_TRAILER,
    TAG_WRITER, bad, bad_at, cstr, parse_acquisition, parse_pointers, parse_segment,
    parse_summary, parse_trailer,
};
use super::spectrum::{SpectrumRecord, parse_lines, parse_profile, parse_spectrum, variable};
use super::{BrukerBafReader, FORMAT_ID};

/// Largest record read whole (header records, spectrum records, blocks).
const MAX_RECORD: u64 = 1 << 30;
/// Largest `Calibrator.ami` read.
const MAX_AMI: u64 = 64 << 20;
/// Most top-level records walked when the trailer index cannot be used.
const MAX_RECORDS: usize = 1 << 26;

/// Where a spectrum's records are.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpectrumEntry {
    /// The parsed spectrum record.
    pub record: SpectrumRecord,
    /// Offset of the spectrum record.
    pub offset: u64,
    /// Offset of its centroid block.
    pub lines: Option<u64>,
    /// Offset of its profile block.
    pub profile: Option<u64>,
    /// Its summary-table entry.
    pub summary: Option<SummaryEntry>,
}

/// Which calibration gives m/z.
#[derive(Debug, Clone, PartialEq)]
enum MzSource {
    /// `Calibrator.ami`'s payload scaled to each spectrum's temperatures (or K ratio).
    Recalibration(TofCalibration),
    /// Each spectrum's own calibration, K and Q recomputed from the header reference.
    Acquisition,
}

/// An opened BAF directory.
pub struct BafDataset {
    fs: Fs,
    dir: PathBuf,
    baf: PathBuf,
    file: SourceFile,
    len: u64,
    writer: Option<String>,
    compressor: Option<String>,
    header: AcquisitionHeader,
    segments: Vec<Segment>,
    spectra: Vec<SpectrumEntry>,
    references: Vec<ReferenceCalibration>,
    drift: Option<DriftSettings>,
    mz_source: MzSource,
    index_source: &'static str,
    has_sqlite: bool,
    profile_coding: Option<&'static str>,
    element_code: Option<u32>,
    notes: Vec<String>,
    problems: Vec<String>,
    by_id: HashMap<u32, usize>,
}

impl std::fmt::Debug for BafDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BafDataset")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

/// The `.d` directory and `analysis.baf` an input names (the directory or the file).
pub fn locate(fs: &Fs, path: &Path) -> Option<(PathBuf, PathBuf)> {
    if fs.is_dir(path) {
        let baf = fs.find_in_dir(path, "analysis.baf")?;
        return fs.is_file(&baf).then(|| (path.to_path_buf(), baf));
    }
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    (name == "analysis.baf").then(|| {
        let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
        (dir, path.to_path_buf())
    })
}

fn read_exact(file: &SourceFile, path: &Path, at: u64, n: u64) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; usize::try_from(n).map_err(|_| bad("record too large"))?];
    file.read_exact_at(at, &mut buf).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            bad_at(at, format!("{}: record runs past the end of the file", path.display()))
        } else {
            Error::io(path, e)
        }
    })?;
    Ok(buf)
}

/// The length and tag of the record at `at`.
fn record_head(file: &SourceFile, path: &Path, at: u64, file_len: u64) -> Result<(u64, u32)> {
    let h = read_exact(file, path, at, 8)?;
    let len = u64::from(le_u32(&h, 0).unwrap_or(0));
    let tag = le_u32(&h, 4).unwrap_or(0);
    if len < 8 || at + len > file_len || tag & 0xFFFF_0000 != super::records::TAG_BASE {
        return Err(bad_at(
            at,
            format!("not a BAF record (length {len}, tag {tag:#010x})"),
        ));
    }
    if len > MAX_RECORD {
        return Err(bad_at(at, format!("record of {len} bytes")));
    }
    Ok((len, tag))
}

fn read_record(file: &SourceFile, path: &Path, at: u64, file_len: u64) -> Result<(u32, Vec<u8>)> {
    let (len, tag) = record_head(file, path, at, file_len)?;
    Ok((tag, read_exact(file, path, at, len)?))
}

impl BafDataset {
    /// Open the `.d` directory (or its `analysis.baf`) named by `input`.
    #[allow(clippy::too_many_lines)]
    pub fn open(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let (dir, baf) = locate(fs, path).ok_or_else(|| {
            Error::Usage(format!(
                "{} is not a Bruker .d directory with analysis.baf",
                path.display()
            ))
        })?;
        let file = fs.open(&baf).map_err(|e| Error::io(&baf, e))?;
        let len = file.size().map_err(|e| Error::io(&baf, e))?;
        let mut notes = Vec::new();
        let mut problems = Vec::new();
        // Leading records: the pointer record, the writer, the compressor, the header.
        let (tag, first) = read_record(&file, &baf, 0, len)?;
        if tag != TAG_POINTERS {
            return Err(bad_at(0, format!("the first record is {tag:#010x}, not the BAF pointer record")));
        }
        let pointers = parse_pointers(&first);
        let mut writer = None;
        let mut compressor = None;
        let mut header = None;
        let mut at = first.len() as u64;
        let mut body_start = None;
        while at < len {
            let (rlen, tag) = record_head(&file, &baf, at, len)?;
            match tag {
                TAG_WRITER => {
                    let r = read_exact(&file, &baf, at, rlen)?;
                    writer = Some(cstr(&r, 8, 40)).filter(|s| !s.is_empty());
                }
                TAG_COMPRESSOR => {
                    let r = read_exact(&file, &baf, at, rlen)?;
                    if r.len() > 36 {
                        compressor = Some(cstr(&r, 36, r.len())).filter(|s| !s.is_empty());
                    }
                }
                TAG_ACQUISITION => {
                    let r = read_exact(&file, &baf, at, rlen)?;
                    header = Some(parse_acquisition(&r)?);
                }
                TAG_SEGMENT | TAG_SPECTRUM | TAG_PROFILE | TAG_LINES | TAG_SUMMARY | TAG_TRAILER => {
                    body_start = Some(at);
                    break;
                }
                _ => {}
            }
            at += rlen;
        }
        let header = header.ok_or_else(|| bad("no acquisition header record"))?;
        // Record offsets: from the trailer index when the pointer record leads to one,
        // otherwise by walking the chain.
        let mut offsets: BTreeMap<u32, BTreeMap<u32, u64>> = BTreeMap::new();
        let mut summary: Vec<SummaryEntry> = Vec::new();
        let mut index_source = "trailer index";
        let trailer_ok = match pointers {
            Some(Pointers { trailer, summary: sum, .. }) if trailer > 0 && trailer < len => {
                match read_record(&file, &baf, trailer, len) {
                    Ok((TAG_TRAILER, rec)) => {
                        for (id, off) in parse_trailer(&rec) {
                            if off + 8 <= len
                                && let Ok((_, t)) = record_head(&file, &baf, off, len)
                            {
                                offsets.entry(t).or_default().insert(id, off);
                            }
                        }
                        if let Ok((TAG_SUMMARY, srec)) = read_record(&file, &baf, sum, len) {
                            match parse_summary(&srec) {
                                Ok(s) => summary = s,
                                Err(e) => problems.push(format!("summary table: {e}")),
                            }
                        }
                        !offsets.get(&TAG_SPECTRUM).is_none_or(BTreeMap::is_empty)
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !trailer_ok {
            index_source = "record walk";
            offsets.clear();
            let mut at = body_start.unwrap_or(len);
            let mut seen = 0usize;
            while at < len {
                let (rlen, tag) = record_head(&file, &baf, at, len)?;
                if matches!(tag, TAG_SEGMENT | TAG_SPECTRUM | TAG_PROFILE | TAG_LINES) {
                    let id_at = if tag == TAG_SEGMENT { 16 } else { 12 };
                    let r = read_exact(&file, &baf, at + id_at, 4)?;
                    offsets
                        .entry(tag)
                        .or_default()
                        .insert(le_u32(&r, 0).unwrap_or(0), at);
                } else if tag == TAG_SUMMARY {
                    let r = read_exact(&file, &baf, at, rlen)?;
                    summary = parse_summary(&r).unwrap_or_default();
                }
                at += rlen;
                seen += 1;
                if seen > MAX_RECORDS {
                    return Err(bad("more records than accepted"));
                }
            }
            notes.push("no usable trailer index (an acquisition that did not finish?): the records were walked one by one".into());
        }
        let mut segments = Vec::new();
        for (_, &off) in offsets.get(&TAG_SEGMENT).into_iter().flatten() {
            let (_, r) = read_record(&file, &baf, off, len)?;
            segments.push(parse_segment(&r)?);
        }
        let by_summary: HashMap<u32, SummaryEntry> =
            summary.into_iter().map(|e| (e.id, e)).collect();
        let lines = offsets.get(&TAG_LINES).cloned().unwrap_or_default();
        let profiles = offsets.get(&TAG_PROFILE).cloned().unwrap_or_default();
        let mut spectra = Vec::new();
        for (&id, &off) in offsets.get(&TAG_SPECTRUM).into_iter().flatten() {
            let (_, r) = read_record(&file, &baf, off, len)?;
            let record = parse_spectrum(&r)?;
            if record.id != id {
                problems.push(format!("the index names spectrum {id} at {off}, the record says {}", record.id));
            }
            spectra.push(SpectrumEntry {
                lines: lines.get(&record.id).copied(),
                profile: profiles.get(&record.id).copied(),
                summary: by_summary.get(&record.id).cloned(),
                offset: off,
                record,
            });
        }
        spectra.sort_by_key(|s| s.record.id);
        let by_id = spectra
            .iter()
            .enumerate()
            .map(|(k, s)| (s.record.id, k))
            .collect();
        let references: Vec<ReferenceCalibration> = header
            .reference_calibrations
            .iter()
            .filter_map(|p| parse_reference(p))
            .collect();
        let mut ds = BafDataset {
            fs: fs.clone(),
            has_sqlite: fs.find_in_dir(&dir, "analysis.sqlite").is_some(),
            dir,
            baf,
            file,
            len,
            writer,
            compressor,
            header,
            segments,
            spectra,
            references,
            drift: None,
            mz_source: MzSource::Acquisition,
            index_source,
            profile_coding: None,
            element_code: None,
            notes,
            problems,
            by_id,
        };
        // The first profile block's coding and the first centroid block's element code.
        if let Some(off) = ds.spectra.iter().find_map(|s| s.profile) {
            let h = read_exact(&ds.file, &ds.baf, off, 24)?;
            ds.profile_coding = Some(match h.get(21) {
                Some(4) => "float32",
                Some(1) => "packed",
                _ => "unknown",
            });
        }
        if let Some(off) = ds.spectra.iter().find_map(|s| s.lines) {
            let h = read_exact(&ds.file, &ds.baf, off, 24)?;
            ds.element_code = le_u32(&h, 20);
        }
        ds.drift = ds.drift_settings();
        ds.mz_source = ds.recalibration(fs);
        match &ds.mz_source {
            MzSource::Recalibration(_) => ds.notes.push("m/z: the recalibration stored in Calibrator.ami (applied by the vendor's processing software), scaled to each spectrum's detector temperatures; vendor exports of recalibrated runs use it".into()),
            MzSource::Acquisition => ds.notes.push("m/z: each spectrum's acquisition calibration, with K and Q recomputed from the header's reference calibration and the spectrum's detector temperatures".into()),
        }
        Ok(ds)
    }

    fn first_tof(&self) -> Option<TofCalibration> {
        let p = self.spectra.first()?.record.calibration.as_deref()?;
        match parse_payload(p) {
            Ok(Calibration::Tof(c)) => Some(c),
            _ => None,
        }
    }

    /// Drift compensation settings: the header reference whose t0 matches the first
    /// spectrum's, then the segment variables for what it lacks.
    fn drift_settings(&self) -> Option<DriftSettings> {
        let first = self.first_tof()?;
        let mut s = DriftSettings::default();
        let mut have = false;
        if let Some(r) = self.references.iter().find(|r| r.t0_ns == first.t0_ns) {
            s.reference = r.reference;
            s.gradient = r.gradient;
            have = true;
        }
        let h = &self.header;
        let seg_value = |name: &str| -> Option<f64> {
            let id = h.variable_id(name)?;
            let def = h.variables.get(&id)?;
            self.segments
                .iter()
                .flat_map(|g| &g.lists)
                .find_map(|l| l.values.get(&id).and_then(|raw| def.decode(*raw)))
        };
        if !have {
            match (
                seg_value("TOF_DeviceReferenceTemp1"),
                seg_value("TOF_DeviceReferenceTemp2"),
                seg_value("TOF_DeviceTempGradient1"),
                seg_value("TOF_DeviceTempGradient2"),
            ) {
                (Some(r1), Some(r2), Some(g1), Some(g2)) => {
                    s.reference = (r1, r2);
                    s.gradient = (g1, g2);
                }
                _ => return None,
            }
        }
        s.enabled = seg_value("TOF_SwitchTempCompensation").is_some_and(|v| v != 0.0);
        s.one_sensor = seg_value("TOF_UseOnlyOneTemperature").is_some_and(|v| v != 0.0);
        Some(s)
    }

    fn recalibration(&mut self, fs: &Fs) -> MzSource {
        let Some(p) = fs.find_in_dir(&self.dir, "Calibrator.ami") else {
            return MzSource::Acquisition;
        };
        let Some(first) = self.first_tof() else {
            return MzSource::Acquisition;
        };
        let size = fs.metadata(&p).map(|m| m.len()).unwrap_or(0);
        if size > MAX_AMI {
            self.problems
                .push(format!("Calibrator.ami of {size} bytes was not read"));
            return MzSource::Acquisition;
        }
        match fs.read(&p) {
            Ok(a) => match find_recalibration(&a, &first) {
                Some(c) => MzSource::Recalibration(c),
                None => {
                    self.notes.push("Calibrator.ami holds no calibration matching the spectra's; the acquisition calibration is used".into());
                    MzSource::Acquisition
                }
            },
            Err(e) => {
                self.problems.push(format!("Calibrator.ami: {e}"));
                MzSource::Acquisition
            }
        }
    }

    /// The calibration that gives spectrum `k`'s m/z.
    fn calibration_of(&self, k: usize) -> Result<Calibration> {
        let s = &self.spectra[k].record;
        let payload = s.calibration.as_deref().ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("spectrum {} without a calibration", s.id),
                "Its m/z cannot be computed; intensities are in `info --view full` only.",
            )
        })?;
        let own = parse_payload(payload).map_err(|e| {
            Error::unsupported(
                FORMAT_ID,
                format!("spectrum {}: {e}", s.id),
                "Only time-of-flight calibrations of kinds 3 and 10 and the FT-ICR calibration are known; send the file so its calibration can be checked.",
            )
        })?;
        let Calibration::Tof(own) = own else {
            return Ok(own);
        };
        match &self.mz_source {
            MzSource::Recalibration(cal) => {
                let r = match (self.drift, own.temperatures, cal.temperatures) {
                    (Some(d), Some(ti), Some(ta)) => d.factor(ti) / d.factor(ta),
                    _ => {
                        let first = self.first_tof().map_or(own.k, |f| f.k);
                        own.k / first
                    }
                };
                let mut c = cal.clone();
                c.k = cal.k * r;
                c.q = cal.q / r;
                Ok(Calibration::Tof(c))
            }
            MzSource::Acquisition => {
                let mut c = own;
                if let (Some(d), Some(t)) = (self.drift, c.temperatures)
                    && let Some(r) = self.references.iter().find(|r| r.t0_ns == c.t0_ns)
                {
                    let f = d.factor(t);
                    c.k = r.k * f;
                    c.q = r.q / f;
                }
                Ok(Calibration::Tof(c))
            }
        }
    }

    fn defs_value(&self, k: usize, name: &str) -> Option<f64> {
        let h = &self.header;
        variable(&self.spectra[k].record.variables, &h.variables, h.variable_id(name))
    }

    fn scan_window(&self, k: usize) -> Option<[f64; 2]> {
        let s = &self.spectra[k].record;
        let h = &self.header;
        let (b, e) = (h.variable_id("Mode_ScanBegin")?, h.variable_id("Mode_ScanEnd")?);
        let seg = self.segments.iter().find(|g| g.number == s.segment)?;
        let list = seg
            .lists
            .iter()
            .find(|l| l.scan_type == s.scan_type)
            .or_else(|| seg.lists.first())?;
        let lo = h.variables.get(&b)?.decode(*list.values.get(&b)?)?;
        let hi = h.variables.get(&e)?.decode(*list.values.get(&e)?)?;
        Some([lo, hi])
    }

    /// The header fields of spectrum `k` (no peaks).
    fn header_of(&self, k: usize) -> Spectrum {
        let e = &self.spectra[k];
        let s = &e.record;
        let mut sp = Spectrum {
            index: k as u64,
            scan_number: u64::from(s.id),
            ms_level: s.ms_level(),
            rt_s: Some(f64::from(s.rt_ms) / 1000.0),
            polarity: match s.scan_type[0] {
                0 => "positive",
                1 => "negative",
                _ => "unknown",
            }
            .into(),
            native_id: Some(format!("scan={}", s.id)),
            ..Spectrum::default()
        };
        if let Some(&p) = s.precursors.last() {
            let p = f64::from(p);
            sp.precursor_mz = Some(p);
            let res = self.defs_value(k, "Quadrupole_IsolationResolution_Act");
            let half = res.filter(|v| *v > 0.0).map_or(1.0, |v| v / 2.0);
            sp.isolation_window_mz = Some([p - half, p + half]);
            sp.collision_energy = self.defs_value(k, "Collision_Energy_Act");
            let h = &self.header;
            if let Some(id) = h.variable_id("MSMS_FragmentationMode_Act")
                && let (Some(def), Some(raw)) = (h.variables.get(&id), s.variables.get(&id))
                && let Some(v) = def.decode(*raw)
            {
                sp.activation = def.text_of(v).map(|t| t.to_ascii_uppercase());
            }
            // 0 = not determined; the vendor library reports any other value as the charge.
            if let Some(c) = self.defs_value(k, "MSMS_PreCursorChargeState")
                && c > 0.0
                && c.fract() == 0.0
                && c <= f64::from(i32::MAX)
            {
                sp.precursor_charge = Some(c as i32);
            }
            if s.precursors.len() > 1 {
                sp.extra.insert(
                    "precursors".into(),
                    json!(s.precursors.iter().map(|&v| f64::from(v)).collect::<Vec<_>>()),
                );
            }
        }
        sp.scan_window_mz = self.scan_window(k);
        if let Some(sm) = &e.summary {
            sp.total_ion_current = Some(f64::from(sm.stats[4]));
            sp.base_peak_intensity = Some(f64::from(sm.stats[3]));
        }
        sp.centroided = e.profile.is_none();
        sp.extra.insert("segment".into(), json!(s.segment));
        sp.extra.insert("scan_mode_code".into(), json!(s.scan_type[2]));
        sp
    }

    fn spectrum_of(&mut self, k: usize, view: SpectrumView) -> Result<Spectrum> {
        let mut sp = self.header_of(k);
        let e = self.spectra[k].clone();
        let cal = self.calibration_of(k)?;
        let use_profile = view == SpectrumView::Primary && e.profile.is_some();
        if use_profile {
            let off = e.profile.unwrap_or_default();
            let (_, r) = read_record(&self.file, &self.baf, off, self.len)?;
            let (id, v) = parse_profile(&r)?;
            if id != e.record.id {
                return Err(bad_at(off, format!("profile block of spectrum {id} where {} was expected", e.record.id)));
            }
            sp.mz = (0..v.len()).map(|i| cal.mz(i as f64)).collect();
            sp.intensity = v.iter().map(|&x| x as f32).collect();
            sp.centroided = false;
        } else if let Some(off) = e.lines {
            let (_, r) = read_record(&self.file, &self.baf, off, self.len)?;
            let b = parse_lines(&r)?;
            if b.id != e.record.id {
                return Err(bad_at(off, format!("centroid block of spectrum {} where {} was expected", b.id, e.record.id)));
            }
            sp.mz = b.position.iter().map(|&x| cal.mz(x)).collect();
            sp.intensity = b.intensity.iter().map(|&x| x as f32).collect();
            sp.centroided = true;
            if sp.total_ion_current.is_none() {
                sp.total_ion_current = Some(f64::from(b.stats[4]));
                sp.base_peak_intensity = Some(f64::from(b.stats[3]));
            }
        } else {
            sp.centroided = true;
        }
        if sp.mz.windows(2).any(|w| w[1] < w[0]) {
            let mut idx: Vec<usize> = (0..sp.mz.len()).collect();
            idx.sort_by(|&a, &b| sp.mz[a].total_cmp(&sp.mz[b]));
            sp.mz = idx.iter().map(|&i| sp.mz[i]).collect();
            sp.intensity = idx.iter().map(|&i| sp.intensity[i]).collect();
        }
        if let Some((i, _)) = sp
            .intensity
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
        {
            sp.base_peak_mz = sp.mz.get(i).copied();
        }
        Ok(sp)
    }

    fn spectra_info(&self) -> SpectraInfo {
        let levels: BTreeSet<u32> = self.spectra.iter().map(|s| s.record.ms_level()).collect();
        let rts: Vec<f64> = self
            .spectra
            .iter()
            .map(|s| f64::from(s.record.rt_ms) / 1000.0)
            .collect();
        let h = &self.header;
        let mut extra = BTreeMap::new();
        if let Some((t, off)) = &h.started_utc {
            extra.insert("acquired_utc".into(), json!(format!("{t}Z")));
            extra.insert("utc_offset_minutes".into(), json!(off));
        }
        for (k, key) in [(5, "method"), (8, "sample_name")] {
            if let Some(v) = h.string(k) {
                extra.insert(key.into(), json!(v));
            }
        }
        if let Some(n) = &h.analysis_name {
            extra.insert("analysis_name".into(), json!(n));
        }
        extra.insert("writer".into(), json!(self.writer));
        extra.insert("compressor".into(), json!(self.compressor));
        extra.insert("segments".into(), json!(self.segments.len()));
        extra.insert(
            "profile_spectra".into(),
            json!(self.spectra.iter().filter(|s| s.profile.is_some()).count()),
        );
        extra.insert(
            "calibration".into(),
            json!(self.first_calibration_name()),
        );
        extra.insert(
            "mz_calibration_source".into(),
            json!(match self.mz_source {
                MzSource::Recalibration(_) => "Calibrator.ami",
                MzSource::Acquisition => "acquisition",
            }),
        );
        extra.insert("index_source".into(), json!(self.index_source));
        if let Some(c) = self.profile_coding {
            extra.insert("profile_coding".into(), json!(c));
        }
        if let Some(c) = self.element_code {
            extra.insert("centroid_element_code".into(), json!(c));
        }
        let pols: BTreeSet<&str> = self
            .spectra
            .iter()
            .map(|s| match s.record.scan_type[0] {
                0 => "positive",
                1 => "negative",
                _ => "unknown",
            })
            .collect();
        extra.insert("polarities".into(), json!(pols));
        extra.insert("analysis_sqlite".into(), json!(self.has_sqlite));
        SpectraInfo {
            index: 0,
            name: h.analysis_name.clone(),
            scan_count: self.spectra.len() as u64,
            ms_levels: levels.into_iter().collect(),
            rt_range_s: (!rts.is_empty()).then(|| {
                [
                    rts.iter().copied().fold(f64::INFINITY, f64::min),
                    rts.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                ]
            }),
            instrument: Some(InstrumentInfo {
                manufacturer: Some("Bruker".into()),
                model: h.string(1).map(|s| s.trim().to_string()),
                software: h.string(3).map(str::to_string),
                software_version: h.string(4).map(str::to_string),
                detector: None,
            }),
            extra,
        }
    }

    fn first_calibration_name(&self) -> Option<String> {
        let p = self.spectra.first()?.record.calibration.as_deref()?;
        Some(match parse_payload(p) {
            Ok(c) => c.name(),
            Err(e) => format!("not read ({e})"),
        })
    }

    fn check_impl(&mut self, full: bool) -> CheckReport {
        let mut r = CheckReport::new(self.dir.display().to_string(), FORMAT_ID);
        r.performed("analysis.baf: pointer record, acquisition header, segments and every spectrum record, located through the trailer index (or by walking the records)");
        for p in self.problems.clone() {
            r.push(Finding::warning("index", p));
        }
        let missing = self.spectra.iter().filter(|s| s.lines.is_none()).count();
        if missing > 0 {
            r.push(Finding::warning(
                "missing_blocks",
                format!("{missing} spectra have no centroid block"),
            ));
        }
        let ids: Vec<u32> = self.spectra.iter().map(|s| s.record.id).collect();
        if ids.windows(2).any(|w| w[1] != w[0] + 1) {
            r.push(Finding::warning("gaps", "spectrum ids are not consecutive"));
        }
        r.performed("summary table: retention time and segment of every spectrum agree with its record");
        let disagree = self
            .spectra
            .iter()
            .filter(|s| {
                s.summary
                    .as_ref()
                    .is_some_and(|m| m.rt_ms != s.record.rt_ms || m.segment != s.record.segment)
            })
            .count();
        if disagree > 0 {
            r.push(Finding::error(
                "summary_mismatch",
                format!("{disagree} spectra disagree with the summary table"),
            ));
        }
        r.performed("calibration: every spectrum's calibration is of a known kind");
        let unknown = (0..self.spectra.len())
            .filter(|&k| self.calibration_of(k).is_err())
            .count();
        if unknown > 0 {
            r.push(Finding::warning(
                "calibration",
                format!("{unknown} spectra have a calibration that is not read (their m/z are refused)"),
            ));
        }
        if full {
            r.performed("every spectrum: the centroid and profile blocks decode to their declared counts");
            let mut bad_blocks = 0usize;
            for k in 0..self.spectra.len() {
                for view in [SpectrumView::Centroid, SpectrumView::Primary] {
                    if let Err(e) = self.spectrum_of(k, view) {
                        if matches!(e, Error::Unsupported { .. }) {
                            continue;
                        }
                        bad_blocks += 1;
                        if bad_blocks <= 3 {
                            r.push(Finding::error("bad_block", e.to_string()));
                        }
                    }
                }
            }
            if bad_blocks > 3 {
                r.push(Finding::error(
                    "bad_block",
                    format!("{bad_blocks} blocks do not decode"),
                ));
            }
        }
        r
    }
}

impl Dataset for BafDataset {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.dir.display().to_string(),
            size_bytes: self.len,
            format: BrukerBafReader.descriptor(),
            format_version: self.writer.clone(),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: vec![self.spectra_info()],
            traces: Vec::new(),
            plane_count: 0,
            notes: self.notes.clone(),
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h = &self.header;
        let names = [
            "source_path",
            "instrument",
            "operator",
            "acquisition_software",
            "acquisition_software_version",
            "method",
            "version",
            "role",
            "sample_name",
        ];
        let strings: serde_json::Map<String, Value> = h
            .strings
            .iter()
            .enumerate()
            .filter(|(k, _)| *k != 2)
            .map(|(k, v)| {
                (
                    names.get(k).map_or_else(|| format!("string_{k}"), |s| (*s).to_string()),
                    json!(v),
                )
            })
            .collect();
        let segments: Vec<Value> = self
            .segments
            .iter()
            .map(|g| {
                json!({"number": g.number, "lists": g.lists.iter().map(|l| {
                    let vals: serde_json::Map<String, Value> = l.values.iter().filter_map(|(id, raw)| {
                        let d = h.variables.get(id)?;
                        Some((d.name.clone(), json!(d.decode(*raw))))
                    }).collect();
                    json!({"scan_type": l.scan_type, "values": vals})
                }).collect::<Vec<_>>()})
            })
            .collect();
        Ok(json!({
            "writer": self.writer,
            "compressor": self.compressor,
            "acquired_utc": h.started_utc.as_ref().map(|(t, _)| format!("{t}Z")),
            "utc_offset_minutes": h.started_utc.as_ref().map(|(_, o)| o),
            "strings": strings,
            "variable_definitions": h.variables_json(),
            "segments": segments,
            "reference_calibrations": self.references.iter().map(|r| json!({
                "t0_ns": r.t0_ns, "k": r.k, "q": r.q, "reference_temperatures": [r.reference.0, r.reference.1],
                "gradients_ppm_per_degree": [r.gradient.0, r.gradient.1],
            })).collect::<Vec<_>>(),
            "drift_compensation": self.drift.map(|d| json!({"enabled": d.enabled, "one_sensor": d.one_sensor})),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "spectra[0].scan_count",
            "spectra[0].ms_levels",
            "spectra[0].rt_range_s",
            "spectra[0].instrument",
            "spectra[0].extra",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let rel = |p: &Path| {
            p.strip_prefix(&self.dir)
                .unwrap_or(p)
                .display()
                .to_string()
        };
        let mut out = vec![LsEntry {
            kind: "file".into(),
            name: rel(&self.baf),
            offset: None,
            size: Some(self.len),
            image: None,
            details: json!({"index": self.index_source}),
        }];
        for (k, s) in self.spectra.iter().enumerate().take(2000) {
            out.push(LsEntry {
                kind: "spectrum".into(),
                name: format!("spectrum {}", s.record.id),
                offset: Some(s.offset),
                size: None,
                image: None,
                details: json!({"index": k, "ms_level": s.record.ms_level(), "rt_ms": s.record.rt_ms,
                                "lines_at": s.lines, "profile_at": s.profile}),
            });
        }
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "a BAF run holds spectra, not images: use `spectrum` or `scans`".into(),
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        Ok(self.check_impl(true))
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        Ok(self.check_impl(false))
    }

    fn read_spectrum(&mut self, index: u32, spectrum: u64) -> Result<Spectrum> {
        self.read_spectrum_view(index, spectrum, SpectrumView::Primary)
    }

    fn read_spectrum_view(&mut self, index: u32, spectrum: u64, view: SpectrumView) -> Result<Spectrum> {
        if index != 0 {
            return Err(Error::Usage(format!(
                "run {index} does not exist (a BAF .d holds one run, 0)"
            )));
        }
        let k = usize::try_from(spectrum)
            .ok()
            .filter(|&k| k < self.spectra.len())
            .ok_or_else(|| {
                Error::Usage(format!(
                    "spectrum {spectrum} out of range (0..{})",
                    self.spectra.len()
                ))
            })?;
        self.spectrum_of(k, view)
    }

    fn spectrum_ms_levels(&mut self, run: u32) -> Result<Option<Vec<u32>>> {
        Ok((run == 0).then(|| self.spectra.iter().map(|s| s.record.ms_level()).collect()))
    }

    fn find_spectrum(&mut self, run: u32, scan_number: u64) -> Result<Option<u64>> {
        if run != 0 {
            return Ok(None);
        }
        let id = u32::try_from(scan_number).ok();
        Ok(Some(
            id.and_then(|id| self.by_id.get(&id).copied())
                .map(|k| k as u64)
                .ok_or_else(|| Error::Usage(format!("no spectrum with id {scan_number}")))?,
        ))
    }

    fn visit_scan_headers(
        &mut self,
        run: u32,
        first: u64,
        visit: &mut dyn FnMut(openreadout_core::ScanHeader) -> bool,
    ) -> Result<bool> {
        if run != 0 {
            return Err(Error::Usage(format!(
                "run {run} out of range (a BAF .d holds one run)"
            )));
        }
        for k in (first as usize)..self.spectra.len() {
            if !visit(self.header_of(k).into()) {
                break;
            }
        }
        Ok(true)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let mut out = vec![self.baf.clone()];
        for n in ["Calibrator.ami", "analysis.baf_idx", "analysis.baf_xtr"] {
            if let Some(p) = self.fs.find_in_dir(&self.dir, n) {
                out.push(p);
            }
        }
        out
    }
}
