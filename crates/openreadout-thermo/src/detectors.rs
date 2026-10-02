//! Controllers other than the mass spectrometer: single-wavelength UV channels (and other
//! detectors recorded the same way, such as a charged aerosol detector), the photodiode-array
//! (PDA) field, and analog channels (pump pressure, temperatures, A/D inputs). Each becomes a
//! trace. Layout and evidence: `docs/formats/thermo-raw.md` ("Detector controllers") and
//! `docs/provenance/thermo-raw.md` (2026-09-24).

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{SignalChannelInfo, TraceInfo};
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::json;

use crate::FORMAT_ID;
use crate::bytes::{Cursor, read_at};
use crate::layout::{ControllerRef, InstrumentId, parse_instrument_id, parse_run_header};

/// Bytes per scan-index row of a detector controller (file versions 64 and 66).
pub const DETECTOR_INDEX_LEN: u64 = 72;
/// Bytes per scan-index row of a detector controller before version 64 (32-bit data offset).
pub const DETECTOR_INDEX_LEN_32: u64 = 64;
/// Bytes of the trailer that follows each PDA spectrum (file versions 64 and 66).
pub const PDA_TRAILER_LEN: u64 = 80;
/// Bytes of the PDA trailer before version 64 (32-bit values offset).
pub const PDA_TRAILER_LEN_32: u64 = 72;

/// Index row length by file version.
pub fn detector_index_len(version: u32) -> u64 {
    if version >= 64 {
        DETECTOR_INDEX_LEN
    } else {
        DETECTOR_INDEX_LEN_32
    }
}

/// PDA trailer length by file version.
pub fn pda_trailer_len(version: u32) -> u64 {
    if version >= 64 {
        PDA_TRAILER_LEN
    } else {
        PDA_TRAILER_LEN_32
    }
}
/// Record kinds (index row, offset 8).
pub const KIND_PDA: u32 = 10;
pub const KIND_CHANNEL: u32 = 12;
pub const KIND_ANALOG: u32 = 13;
/// Longest PDA spectrum accepted (points).
const MAX_PDA_POINTS: u32 = 4096;
/// Most analog values per scan accepted.
const MAX_ANALOG_VALUES: u32 = 64;

/// What a detector controller records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectorKind {
    /// One value and its time per sample (DAD single-wavelength channels, CAD).
    Channel,
    /// A spectrum per sample (photodiode array).
    Pda,
    /// One or more values per sample, times in the index (pressure, temperature, A/D).
    Analog,
    /// No samples (e.g. an autosampler) or a record kind not decoded.
    NoData,
}

impl DetectorKind {
    pub fn name(self) -> &'static str {
        match self {
            DetectorKind::Channel => "channel",
            DetectorKind::Pda => "pda",
            DetectorKind::Analog => "analog",
            DetectorKind::NoData => "none",
        }
    }
}

/// One row of a detector controller's scan index.
#[derive(Debug, Clone, Copy, Default)]
pub struct DetectorRow {
    pub scan_number: u32,
    pub record_kind: u32,
    pub values_per_scan: u32,
    /// The detector's sampling rate (Hz) where the row stores it (before version 64: the
    /// method's scan or channel rate); 0 in versions 64 and 66.
    pub sample_rate_hz: f64,
    pub rt_min: f64,
    pub low: f64,
    pub high: f64,
    pub stored_value: f64,
    pub data_offset: u64,
}

pub fn parse_detector_row(c: &mut Cursor<'_>, version: u32) -> Result<DetectorRow> {
    let start = c.position();
    let offset32 = c.u32()?;
    let scan_number = c.u32()?;
    let record_kind = c.u32()?;
    c.skip(4)?;
    let values_per_scan = c.u32()?;
    c.skip(4)?;
    let sample_rate_hz = c.f64()?;
    let rt_min = c.f64()?;
    let low = c.f64()?;
    let high = c.f64()?;
    let stored_value = c.f64()?;
    let data_offset = if version >= 64 {
        c.u64()?
    } else {
        u64::from(offset32)
    };
    debug_assert_eq!((c.position() - start) as u64, detector_index_len(version));
    Ok(DetectorRow {
        scan_number,
        record_kind,
        values_per_scan,
        sample_rate_hz,
        rt_min,
        low,
        high,
        stored_value,
        data_offset,
    })
}

/// The trailer after a PDA spectrum.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PdaTrailer {
    pub start_nm: f64,
    pub end_nm: f64,
    pub step_nm: f64,
    pub per_unit: f64,
    pub points: u32,
    pub values_offset: u64,
}

pub fn parse_pda_trailer(c: &mut Cursor<'_>, version: u32) -> Result<PdaTrailer> {
    c.skip(16)?;
    let start_nm = c.f64()?;
    let end_nm = c.f64()?;
    let step_nm = c.f64()?;
    c.skip(16)?;
    let per_unit = c.f64()?;
    let points = c.u32()?;
    let values_offset = if version >= 64 {
        c.skip(4)?;
        c.u64()?
    } else {
        u64::from(c.u32()?)
    };
    Ok(PdaTrailer {
        start_nm,
        end_nm,
        step_nm,
        per_unit,
        points,
        values_offset,
    })
}

/// A non-MS controller.
#[derive(Debug, Clone)]
pub struct Detector {
    pub controller: ControllerRef,
    pub kind: DetectorKind,
    /// Instrument-id texts: channel name, device, module, serial (Vanquish modules).
    pub id: InstrumentId,
    pub scan_count: u64,
    pub start_min: f64,
    pub end_min: f64,
    /// The run header's maximum (the largest stored value).
    pub max_value: f64,
    pub scan_index_address: u64,
    pub scan_data_address: u64,
    /// Values per sample (analog controllers; UV controllers that record several channels).
    pub values_per_scan: u32,
    /// UV channel samples carry their time after the value (`true`, versions 64 and 66) or
    /// hold the values only (`false`, before version 64); from the first two rows' offsets.
    pub sample_has_time: bool,
    /// Sampling rate stored in the first index row (Hz; 0 when not stored).
    pub sample_rate_hz: f64,
    /// File version (row and trailer layouts depend on it).
    pub version: u32,
    /// PDA wavelength grid, from the first spectrum's trailer.
    pub pda: Option<PdaTrailer>,
}

impl Detector {
    /// Channel name as the acquisition software shows it (`UV_VIS_1`, `3DFIELD`, ...). Older
    /// devices put the maker in the first id text and the device in the second (`Thermo` /
    /// `Accela PDA Detector`): the device is used then.
    pub fn channel_name(&self) -> String {
        let maker = matches!(
            self.id.model.as_str(),
            "Thermo" | "Thermo Scientific" | "Thermo Fisher Scientific" | "Finnigan"
        );
        if maker && !self.id.model_2.is_empty() {
            self.id.model_2.clone()
        } else if self.id.model.is_empty() {
            format!("controller {}", self.controller.controller_index)
        } else {
            self.id.model.clone()
        }
    }

    /// Stored channel value to trace unit. Value-only UV samples (before version 64) are
    /// stored in the PDA field's units (µAU; they equal the field's band mean) and are
    /// reported in mAU like the (value, time) channels of later files.
    pub fn channel_scale(&self) -> f64 {
        if self.kind == DetectorKind::Channel && !self.sample_has_time {
            1e-3
        } else {
            1.0
        }
    }

    /// Minutes between samples on the regular grid: the stored rate when the index row has
    /// one, else the run header's time span over the sample count.
    pub fn step_min(&self) -> f64 {
        if self.sample_rate_hz > 0.0 {
            1.0 / (self.sample_rate_hz * 60.0)
        } else if self.scan_count > 1 && self.end_min > self.start_min {
            (self.end_min - self.start_min) / (self.scan_count - 1) as f64
        } else {
            0.0
        }
    }
}

fn corrupt(at: u64, msg: impl Into<String>) -> Error {
    Error::corrupt_at(FORMAT_ID, at, msg)
}

/// Read the detector controllers listed in the controller table (types other than 0). Problems
/// with one controller are returned as messages and that controller is skipped.
pub fn open_detectors(
    file: &mut SourceFile,
    path: &Path,
    version: u32,
    controllers: &[ControllerRef],
) -> (Vec<Detector>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    if version < 63 {
        return (out, problems);
    }
    for c in controllers.iter().filter(|c| c.controller_type != 0) {
        match open_one(file, path, version, c) {
            Ok(d) => out.push(d),
            Err(e) => problems.push(format!(
                "controller {} (type {}): {e}",
                c.controller_index, c.controller_type
            )),
        }
    }
    (out, problems)
}

#[allow(clippy::many_single_char_names)] // a = address, c = controller, d = detector, s = streams
fn open_one(
    file: &mut SourceFile,
    path: &Path,
    version: u32,
    c: &ControllerRef,
) -> Result<Detector> {
    let a = c.run_header_address;
    let rh_len: u64 = if version >= 64 { 7576 } else { 7408 };
    let buf = read_at(file, path, a, rh_len + 4096, "detector run header")
        .or_else(|_| read_at(file, path, a, rh_len, "detector run header"))?;
    let mut cur = Cursor::new(&buf, a);
    let rh = parse_run_header(&mut cur, version)?;
    if rh.own_address != a {
        return Err(corrupt(
            a,
            format!("run header records its own address as {}", rh.own_address),
        ));
    }
    cur.seek_to(usize::try_from(rh.byte_len).unwrap_or(usize::MAX))?;
    let id = parse_instrument_id(&mut cur).unwrap_or_default();
    let scan_count = if rh.first_scan >= 1 && rh.last_scan >= rh.first_scan {
        u64::from(rh.last_scan - rh.first_scan + 1)
    } else {
        0
    };
    let s = rh.streams;
    let mut d = Detector {
        controller: *c,
        kind: DetectorKind::NoData,
        id,
        scan_count,
        start_min: rh.start_time_min,
        end_min: rh.end_time_min,
        max_value: rh.max_total_ion_current,
        scan_index_address: s.scan_index,
        scan_data_address: s.scan_data,
        values_per_scan: 1,
        sample_has_time: true,
        sample_rate_hz: 0.0,
        version,
        pda: None,
    };
    if scan_count == 0 {
        return Ok(d);
    }
    let row_len = detector_index_len(version);
    let rows_to_read = if scan_count > 1 { 2 } else { 1 };
    let first = read_at(
        file,
        path,
        s.scan_index,
        row_len * rows_to_read,
        "detector index",
    )?;
    let mut rc = Cursor::new(&first, s.scan_index);
    let row = parse_detector_row(&mut rc, version)?;
    let second = if rows_to_read == 2 {
        Some(parse_detector_row(&mut rc, version)?)
    } else {
        None
    };
    if row.sample_rate_hz.is_finite() && row.sample_rate_hz > 0.0 && row.sample_rate_hz < 1e6 {
        d.sample_rate_hz = row.sample_rate_hz;
    }
    d.kind = match row.record_kind {
        KIND_CHANNEL => DetectorKind::Channel,
        KIND_PDA => DetectorKind::Pda,
        KIND_ANALOG => DetectorKind::Analog,
        k => {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("detector record kind {k}"),
                "Only UV channels (12), PDA fields (10) and analog channels (13) are decoded.",
            ));
        }
    };
    if d.kind == DetectorKind::Analog {
        if row.values_per_scan == 0 || row.values_per_scan > MAX_ANALOG_VALUES {
            return Err(corrupt(
                s.scan_index,
                format!("{} values per analog sample", row.values_per_scan),
            ));
        }
        d.values_per_scan = row.values_per_scan;
    }
    if d.kind == DetectorKind::Channel {
        // (value, time) per sample in versions 64 and 66; before 64 one f64 per channel and no
        // time (an Accela PDA detector's channels A, B, C). The row-to-row stride tells which.
        let k = row.values_per_scan.max(1);
        if k > MAX_ANALOG_VALUES {
            return Err(corrupt(s.scan_index, format!("{k} values per UV sample")));
        }
        d.values_per_scan = k;
        let values_only = 8 * u64::from(k);
        d.sample_has_time = match second.map(|r| r.data_offset.wrapping_sub(row.data_offset)) {
            Some(st) if st == values_only => false,
            Some(st) if st == values_only + 8 => true,
            Some(st) => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("UV samples {st} bytes apart for {k} value(s) per sample"),
                    "Only UV channels stored as f64 values (with or without a time) are decoded.",
                ));
            }
            None => version >= 64,
        };
        if d.sample_has_time && k != 1 {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("UV samples with a time and {k} values"),
                "Only single-value UV channels with times are decoded.",
            ));
        }
    }
    if d.kind == DetectorKind::Pda {
        let t = read_at(
            file,
            path,
            s.scan_data.saturating_add(row.data_offset),
            pda_trailer_len(version),
            "PDA trailer",
        )?;
        let tr = parse_pda_trailer(
            &mut Cursor::new(&t, s.scan_data.saturating_add(row.data_offset)),
            version,
        )?;
        if tr.points == 0 || tr.points > MAX_PDA_POINTS || tr.step_nm.is_nan() || tr.step_nm <= 0.0
        {
            return Err(corrupt(
                s.scan_data.saturating_add(row.data_offset),
                format!(
                    "PDA spectrum of {} points, step {} nm",
                    tr.points, tr.step_nm
                ),
            ));
        }
        d.pda = Some(tr);
    }
    Ok(d)
}

/// The rows `[first, first + n)` of a detector's index.
pub fn read_rows(
    file: &mut SourceFile,
    path: &Path,
    d: &Detector,
    first: u64,
    n: u64,
) -> Result<Vec<DetectorRow>> {
    let len = detector_index_len(d.version);
    let at = first
        .checked_mul(len)
        .and_then(|o| d.scan_index_address.checked_add(o))
        .ok_or_else(|| corrupt(d.scan_index_address, "detector index address overflows"))?;
    let bytes = n
        .checked_mul(len)
        .ok_or_else(|| corrupt(at, "detector index length overflows"))?;
    let buf = read_at(file, path, at, bytes, "detector index")?;
    let mut c = Cursor::new(&buf, at);
    (0..n)
        .map(|_| parse_detector_row(&mut c, d.version))
        .collect()
}

/// Samples of one detector: `(times in minutes, channels)`; channels are one per value
/// (analog), one (UV channel) or one per wavelength (PDA), in the trace's unit.
pub fn read_samples(
    file: &mut SourceFile,
    path: &Path,
    d: &Detector,
    first: u64,
    n: u64,
) -> Result<(Vec<f64>, Vec<Vec<f64>>)> {
    let rows = read_rows(file, path, d, first, n)?;
    let mut times = Vec::with_capacity(rows.len());
    match d.kind {
        DetectorKind::NoData => Ok((times, Vec::new())),
        DetectorKind::Channel if !d.sample_has_time => {
            // Values only; the index rows' times are not the sample times (they advance in
            // steps of about 0.018 s with jumps): the samples are on the stored rate's grid.
            let k = u64::from(d.values_per_scan);
            let at = d
                .scan_data_address
                .saturating_add(rows.first().map_or(0, |r| r.data_offset));
            let bytes = n.saturating_mul(8).saturating_mul(k);
            let buf = read_at(file, path, at, bytes, "detector samples")?;
            let mut c = Cursor::new(&buf, at);
            let scale = d.channel_scale();
            let step = d.step_min();
            let mut ch = vec![Vec::with_capacity(rows.len()); d.values_per_scan as usize];
            for i in 0..rows.len() as u64 {
                for v in &mut ch {
                    v.push(c.f64()? * scale);
                }
                times.push(d.start_min + step * (first + i) as f64);
            }
            Ok((times, ch))
        }
        DetectorKind::Channel => {
            let at = d
                .scan_data_address
                .saturating_add(rows.first().map_or(0, |r| r.data_offset));
            let buf = read_at(file, path, at, n.saturating_mul(16), "detector samples")?;
            let mut c = Cursor::new(&buf, at);
            let mut v = Vec::with_capacity(rows.len());
            for r in &rows {
                v.push(c.f64()?);
                c.f64()?; // the sample's time, equal to the index's
                times.push(r.rt_min);
            }
            Ok((times, vec![v]))
        }
        DetectorKind::Analog => {
            let k = u64::from(d.values_per_scan);
            let at = d
                .scan_data_address
                .saturating_add(rows.first().map_or(0, |r| r.data_offset));
            let buf = read_at(
                file,
                path,
                at,
                n.saturating_mul(8).saturating_mul(k),
                "detector samples",
            )?;
            let mut c = Cursor::new(&buf, at);
            let mut ch = vec![Vec::with_capacity(rows.len()); d.values_per_scan as usize];
            for r in &rows {
                for v in &mut ch {
                    v.push(c.f64()?);
                }
                times.push(r.rt_min);
            }
            Ok((times, ch))
        }
        DetectorKind::Pda => {
            let grid = d.pda.unwrap_or_default();
            let np = grid.points as usize;
            let mut ch = vec![Vec::with_capacity(rows.len()); np];
            let scale = unit_scale(&grid);
            for r in &rows {
                let t_at = d.scan_data_address.saturating_add(r.data_offset);
                let pts = u64::from(grid.points);
                let v_at = t_at
                    .checked_sub(4 * pts)
                    .ok_or_else(|| corrupt(t_at, "PDA spectrum starts before the scan data"))?;
                let buf = read_at(
                    file,
                    path,
                    v_at,
                    4 * pts + pda_trailer_len(d.version),
                    "PDA spectrum",
                )?;
                let mut c = Cursor::new(&buf, v_at);
                for v in &mut ch {
                    v.push(f64::from(c.i32()?) * scale);
                }
                let tr = parse_pda_trailer(&mut c, d.version)?;
                if tr.points != grid.points
                    || tr.start_nm.to_bits() != grid.start_nm.to_bits()
                    || tr.step_nm.to_bits() != grid.step_nm.to_bits()
                {
                    return Err(Error::unsupported(
                        FORMAT_ID,
                        format!(
                            "PDA scan {} changes the wavelength grid ({}–{} nm step {}, {} points)",
                            r.scan_number, tr.start_nm, tr.end_nm, tr.step_nm, tr.points
                        ),
                        "Only PDA fields with one wavelength grid are read as a trace.",
                    ));
                }
                times.push(r.rt_min);
            }
            Ok((times, ch))
        }
    }
}

/// Stored PDA integer → trace unit (mAU): the trailer's values per absorbance unit, over 1000.
pub fn unit_scale(t: &PdaTrailer) -> f64 {
    if t.per_unit > 0.0 {
        1000.0 / t.per_unit
    } else {
        1e-3
    }
}

/// Wavelengths of the PDA grid, nm.
pub fn wavelengths(t: &PdaTrailer) -> Vec<f64> {
    (0..t.points)
        .map(|i| t.start_nm + f64::from(i) * t.step_nm)
        .collect()
}

fn tidy(x: f64) -> f64 {
    (x * 1e9).round() / 1e9
}

/// `(unit, how it was found)` of a channel from its name and the instrument-method text.
fn unit_of(d: &Detector, method_text: &str) -> Option<(String, &'static str)> {
    let name = d.channel_name();
    match d.kind {
        DetectorKind::Pda => return Some(("mAU".into(), "inferred")),
        DetectorKind::Channel if name.starts_with("UV_VIS") || !d.sample_has_time => {
            return Some(("mAU".into(), "inferred"));
        }
        _ => {}
    }
    let lower = name.to_ascii_lowercase();
    let find = |key: &str| {
        method_text.lines().find_map(|l| {
            let l = l.trim();
            (l.contains(key) && l.contains('['))
                .then(|| {
                    l.rsplit_once('[')
                        .map(|(_, u)| u.trim_end_matches(']').to_string())
                })
                .flatten()
        })
    };
    if lower.contains("pressure") {
        return find("Pressure.UpperLimit").map(|u| (u, "method text"));
    }
    if lower.contains("temp") {
        return find("Temperature.Nominal").map(|u| (u, "method text"));
    }
    if d.id.tags.len() >= 4 && !d.id.tags[3].is_empty() {
        return Some((d.id.tags[3].clone(), "instrument id"));
    }
    None
}

/// The trace `info` reports for a detector (index `index` among the file's traces).
pub fn trace_info(index: u32, d: &Detector, method_text: &str) -> TraceInfo {
    let n = d.scan_count;
    // Analog channels may be logged on change (a column-temperature log is irregular): they
    // carry their recorded times as channel `time`. DAD/CAD samples are on the detector's
    // fixed data rate (`check` verifies it) and use `axis`.
    let irregular = d.kind == DetectorKind::Analog;
    let (rate, step_min) = if irregular {
        (0.0, None)
    } else if n > 1 && d.step_min() > 0.0 {
        let step = d.step_min();
        (1.0 / (step * 60.0), Some(step))
    } else {
        (0.0, None)
    };
    let name = d.channel_name();
    let unit = unit_of(d, method_text);
    let mut extra = BTreeMap::new();
    extra.insert("detector".into(), json!(d.kind.name()));
    extra.insert(
        "controller_type".into(),
        json!(d.controller.controller_type),
    );
    extra.insert(
        "controller_index".into(),
        json!(d.controller.controller_index),
    );
    let texts: Vec<&str> = [
        d.id.model.as_str(),
        d.id.model_2.as_str(),
        d.id.serial_number.as_str(),
        d.id.software_version.as_str(),
    ]
    .into_iter()
    .collect();
    extra.insert("instrument_texts".into(), json!(texts));
    if !d.id.model_2.is_empty() {
        extra.insert("device".into(), json!(d.id.model_2));
    }
    extra.insert("stored_maximum".into(), json!(d.max_value));
    extra.insert("x_start_min".into(), json!(tidy(d.start_min)));
    extra.insert("x_end_min".into(), json!(tidy(d.end_min)));
    if let Some(step) = step_min {
        extra.insert(
            "axis".into(),
            json!({"quantity": "retention_time", "unit": "min",
                   "first": tidy(d.start_min), "step": tidy(step)}),
        );
    }
    if irregular {
        extra.insert("irregular_sampling".into(), json!(true));
    }
    if d.sample_rate_hz > 0.0 {
        extra.insert("stored_sample_rate_hz".into(), json!(d.sample_rate_hz));
    }
    if let Some((_, src)) = &unit {
        extra.insert("unit_source".into(), json!(src));
    }
    let unit_s = unit.map(|(u, _)| u);
    let mut channels = Vec::new();
    if irregular {
        channels.push(SignalChannelInfo {
            index: 0,
            name: "time".into(),
            unit: Some("s".into()),
            dtype: "float64".into(),
            scale: 1.0,
            offset: 0.0,
            extra: BTreeMap::new(),
        });
    }
    let base = u32::from(irregular);
    match d.kind {
        DetectorKind::Pda => {
            let g = d.pda.unwrap_or_default();
            extra.insert(
                "wavelength_range_nm".into(),
                json!([g.start_nm, g.end_nm, g.step_nm]),
            );
            for (i, w) in wavelengths(&g).into_iter().enumerate() {
                let mut e = BTreeMap::new();
                e.insert("wavelength_nm".into(), json!(w));
                channels.push(SignalChannelInfo {
                    index: i as u32,
                    name: format!("{w} nm"),
                    unit: unit_s.clone(),
                    dtype: "int32".into(),
                    scale: unit_scale(&g),
                    offset: 0.0,
                    extra: e,
                });
            }
        }
        DetectorKind::Channel | DetectorKind::Analog => {
            let k = d.values_per_scan.max(1);
            // Lettered channels of an older PDA detector (`A Channel wavelength (nm): 280`),
            // in stored order, when the method lists one per stored value.
            let lettered = lettered_channels(method_text);
            let lettered =
                (d.kind == DetectorKind::Channel && k > 1 && lettered.len() == k as usize)
                    .then_some(lettered);
            for i in 0..k {
                let mut e = BTreeMap::new();
                let mut cname = if k == 1 {
                    name.clone()
                } else {
                    format!("{name} {}", i + 1)
                };
                if let Some(l) = lettered.as_ref().and_then(|l| l.get(i as usize)) {
                    cname = format!("Channel {}", l.letter);
                    e.insert("wavelength_nm".into(), json!(l.wavelength_nm));
                    if let Some(b) = l.bandwidth_nm {
                        e.insert("bandwidth_nm".into(), json!(b));
                    }
                } else if let Some(w) = channel_wavelength(&name, method_text) {
                    e.insert("wavelength_nm".into(), json!(w));
                }
                channels.push(SignalChannelInfo {
                    index: i + base,
                    name: cname,
                    unit: unit_s.clone(),
                    dtype: "float64".into(),
                    scale: d.channel_scale(),
                    offset: 0.0,
                    extra: e,
                });
            }
        }
        DetectorKind::NoData => channels.clear(),
    }
    TraceInfo {
        index,
        name: Some(if d.kind == DetectorKind::Pda {
            format!("{name} spectra")
        } else {
            name
        }),
        sample_rate_hz: tidy(rate),
        sample_count: n,
        sweep_count: 1,
        channels,
        start_s: Some(tidy(d.start_min * 60.0)),
        extra,
    }
}

/// A lettered channel of an older PDA detector's method text.
#[derive(Debug, Clone, PartialEq)]
pub struct LetteredChannel {
    pub letter: char,
    pub wavelength_nm: f64,
    pub bandwidth_nm: Option<f64>,
}

/// `A Channel wavelength (nm): 280` / `A Channel bandwidth (nm): 9` lines (Accela PDA), in
/// letter order.
pub fn lettered_channels(method_text: &str) -> Vec<LetteredChannel> {
    let mut out: Vec<LetteredChannel> = Vec::new();
    for l in method_text.lines() {
        let l = l.trim();
        let mut chars = l.chars();
        let (Some(letter), Some(' ')) = (chars.next(), chars.next()) else {
            continue;
        };
        if !letter.is_ascii_uppercase() {
            continue;
        }
        let rest = &l[2..];
        let value = |key: &str| {
            rest.strip_prefix(key)
                .and_then(|v| v.split_whitespace().next())
                .and_then(|v| v.parse::<f64>().ok())
        };
        if let Some(w) = value("Channel wavelength (nm):") {
            if !out.iter().any(|c| c.letter == letter) {
                out.push(LetteredChannel {
                    letter,
                    wavelength_nm: w,
                    bandwidth_nm: None,
                });
            }
        } else if let Some(b) = value("Channel bandwidth (nm):")
            && let Some(c) = out.iter_mut().find(|c| c.letter == letter)
        {
            c.bandwidth_nm = Some(b);
        }
    }
    out.sort_by_key(|c| c.letter);
    out
}

/// `UV.<channel>.Wavelength: 254.0 [nm]` in the method text.
pub fn channel_wavelength(channel: &str, method_text: &str) -> Option<f64> {
    let key = format!(".{channel}.Wavelength:");
    method_text.lines().find_map(|l| {
        let (_, rest) = l.split_once(&key)?;
        rest.split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: u32, rt: f64, value: f64, off: u64) -> Vec<u8> {
        let mut b = vec![0u8; 72];
        b[4..8].copy_from_slice(&7u32.to_le_bytes());
        b[8..12].copy_from_slice(&kind.to_le_bytes());
        b[16..20].copy_from_slice(&1u32.to_le_bytes());
        b[32..40].copy_from_slice(&rt.to_le_bytes());
        b[56..64].copy_from_slice(&value.to_le_bytes());
        b[64..72].copy_from_slice(&off.to_le_bytes());
        b
    }

    #[test]
    fn rows_and_trailers_parse() {
        let b = row(KIND_CHANNEL, 1.5, 2.25, 32);
        let r = parse_detector_row(&mut Cursor::new(&b, 0), 66).unwrap();
        assert_eq!(
            (
                r.scan_number,
                r.record_kind,
                r.rt_min,
                r.stored_value,
                r.data_offset
            ),
            (7, 12, 1.5, 2.25, 32)
        );
        assert!(parse_detector_row(&mut Cursor::new(&b[..40], 0), 66).is_err());
        let mut t = vec![0u8; 80];
        t[16..24].copy_from_slice(&200.0f64.to_le_bytes());
        t[24..32].copy_from_slice(&500.0f64.to_le_bytes());
        t[32..40].copy_from_slice(&4.0f64.to_le_bytes());
        t[56..64].copy_from_slice(&1e6f64.to_le_bytes());
        t[64..68].copy_from_slice(&76u32.to_le_bytes());
        let tr = parse_pda_trailer(&mut Cursor::new(&t, 0), 66).unwrap();
        assert_eq!(tr.points, 76);
        let w = wavelengths(&tr);
        assert_eq!((w[0], w[75]), (200.0, 500.0));
        assert!((unit_scale(&tr) - 1e-3).abs() < 1e-15);
    }

    #[test]
    fn rows_and_trailers_before_v64() {
        // 64-byte row: u32 offset at 0, rate at 24, no u64 offset
        let mut b = vec![0u8; 64];
        b[0..4].copy_from_slice(&1604u32.to_le_bytes());
        b[4..8].copy_from_slice(&2u32.to_le_bytes());
        b[8..12].copy_from_slice(&KIND_PDA.to_le_bytes());
        b[16..20].copy_from_slice(&1u32.to_le_bytes());
        b[24..32].copy_from_slice(&5.0f64.to_le_bytes());
        b[32..40].copy_from_slice(&(1.0f64 / 300.0).to_le_bytes());
        b[56..64].copy_from_slice(&(-2907.0f64).to_le_bytes());
        let r = parse_detector_row(&mut Cursor::new(&b, 0), 63).unwrap();
        assert_eq!(
            (
                r.data_offset,
                r.scan_number,
                r.record_kind,
                r.sample_rate_hz
            ),
            (1604, 2, KIND_PDA, 5.0)
        );
        assert!((r.stored_value + 2907.0).abs() < 1e-9);
        assert!(parse_detector_row(&mut Cursor::new(&b[..60], 0), 63).is_err());
        // 72-byte trailer ending in a u32 values offset
        let mut t = vec![0u8; 72];
        t[16..24].copy_from_slice(&200.0f64.to_le_bytes());
        t[24..32].copy_from_slice(&600.0f64.to_le_bytes());
        t[32..40].copy_from_slice(&1.0f64.to_le_bytes());
        t[56..64].copy_from_slice(&1e6f64.to_le_bytes());
        t[64..68].copy_from_slice(&401u32.to_le_bytes());
        t[68..72].copy_from_slice(&1676u32.to_le_bytes());
        let tr = parse_pda_trailer(&mut Cursor::new(&t, 0), 63).unwrap();
        assert_eq!((tr.points, tr.values_offset), (401, 1676));
        assert_eq!(wavelengths(&tr).last(), Some(&600.0));
        assert_eq!(
            (detector_index_len(63), pda_trailer_len(63)),
            (DETECTOR_INDEX_LEN_32, PDA_TRAILER_LEN_32)
        );
    }

    #[test]
    fn lettered_method_channels() {
        let m = "Scan Rate (Hz): 5.000000 \nChannel sample rate (Hz): 10.000000 \n\
                 A Channel wavelength (nm): 280 \nA Channel bandwidth (nm): 9 \n\
                 B Channel wavelength (nm): 365 \nB Channel bandwidth (nm): 9 \n\
                 C Channel wavelength (nm): 520 \nC Channel bandwidth (nm): 9 \n";
        let l = lettered_channels(m);
        assert_eq!(l.len(), 3);
        assert_eq!(
            (l[0].letter, l[0].wavelength_nm, l[0].bandwidth_nm),
            ('A', 280.0, Some(9.0))
        );
        assert_eq!((l[2].letter, l[2].wavelength_nm), ('C', 520.0));
        assert!(lettered_channels("UV.UV_VIS_1.Wavelength: 254.0 [nm]").is_empty());
    }

    #[test]
    fn method_wavelengths() {
        let m = "  UV.UV_VIS_1.Wavelength: 254.0 [nm]\n  UV.UV_VIS_10.Wavelength: 600.0 [nm]\n";
        assert_eq!(channel_wavelength("UV_VIS_1", m), Some(254.0));
        assert_eq!(channel_wavelength("UV_VIS_10", m), Some(600.0));
        assert_eq!(channel_wavelength("UV_VIS_2", m), None);
    }
}
