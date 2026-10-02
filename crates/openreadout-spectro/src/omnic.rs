//! Thermo Fisher OMNIC files: `.spa` (one spectrum) and `.spg` (a group of spectra).
//!
//! Layout and vocabulary: `docs/formats/thermo-omnic.md`; provenance:
//! `docs/provenance/thermo-omnic.md`.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::OMNIC_FORMAT_ID as FMT;
use crate::common::{
    Facts, Le, Parsed, Rows, SpectrumSet, SpectrumTable, Stored, XValues, nm_of_wavenumber, num,
    read_at, text_field,
};

/// First 18 bytes of `.spa` and `.spg` files.
pub(crate) const OMNIC_MAGIC: &[u8; 18] = b"Spectral Data File";
/// First 18 bytes of `.srs` series files.
pub(crate) const OMNIC_SERIES_MAGIC: &[u8; 18] = b"Spectral Exte File";
/// Seconds from 1899-12-31T00:00:00Z to the Unix epoch.
const EPOCH_1899: i64 = 2_209_075_200;
/// Most key records read.
const MAX_KEYS: usize = 1 << 20;

/// One 16-byte record of the key table at 304.
#[derive(Debug, Clone, Copy)]
struct Key {
    key: u8,
    offset: u64,
    length: u64,
    /// The spectrum the record belongs to (group files; 0 in single spectra).
    spectrum: u16,
}

/// A decoded spectrum header (key 2).
#[derive(Debug, Clone, Default)]
struct Header {
    points: u32,
    x_code: u8,
    y_code: u8,
    x_first: f32,
    x_last: f32,
    scans: u32,
    background_scans: u32,
    background_gain: f32,
    collection_length: u32,
    laser_frequency: f32,
    sample_spacing: f32,
    aperture: f32,
    raman_excitation: f32,
    optical_velocity: f32,
    scan_points: u32,
    peak_position: u32,
    fft_points: u32,
}

fn header(b: &[u8]) -> Option<Header> {
    Some(Header {
        points: b.u32_at(4)?,
        x_code: b.u8_at(8)?,
        y_code: b.u8_at(12)?,
        x_first: b.f32_at(16)?,
        x_last: b.f32_at(20)?,
        scan_points: b.u32_at(28).unwrap_or(0),
        peak_position: b.u32_at(32).unwrap_or(0),
        scans: b.u32_at(36).unwrap_or(0),
        fft_points: b.u32_at(44).unwrap_or(0),
        background_scans: b.u32_at(52).unwrap_or(0),
        background_gain: b.f32_at(56).unwrap_or(f32::NAN),
        collection_length: b.u32_at(68).unwrap_or(0),
        laser_frequency: b.f32_at(80).unwrap_or(f32::NAN),
        sample_spacing: b.f32_at(84).unwrap_or(f32::NAN),
        aperture: b.f32_at(92).unwrap_or(f32::NAN),
        raman_excitation: b.f32_at(96).unwrap_or(f32::NAN),
        optical_velocity: b.f32_at(188).unwrap_or(f32::NAN),
    })
}

/// x quantity and unit of an x-unit code (for the series reader).
pub(crate) fn x_axis_of(code: u8) -> (&'static str, Option<&'static str>) {
    x_axis(code)
}

/// y quantity and unit of a y-unit code (for the series reader).
pub(crate) fn y_axis_of(code: u8) -> (&'static str, Option<&'static str>) {
    y_axis(code)
}

/// x quantity and unit of an x-unit code.
fn x_axis(code: u8) -> (&'static str, Option<&'static str>) {
    match code {
        1 => ("wavenumber", Some("1/cm")),
        2 => ("points", None),
        3 => ("wavelength", Some("nm")),
        4 => ("wavelength", Some("µm")),
        32 => ("raman_shift", Some("1/cm")),
        _ => ("x", None),
    }
}

/// y quantity (channel name) and unit of a y-unit code.
fn y_axis(code: u8) -> (&'static str, Option<&'static str>) {
    match code {
        11 => ("reflectance", Some("%")),
        12 => ("log_inverse_reflectance", None),
        15 => ("single_beam", None),
        16 => ("transmittance", Some("%")),
        17 => ("absorbance", Some("AU")),
        20 => ("kubelka_munk", None),
        21 => ("reflectance", None),
        22 => ("detector_signal", Some("V")),
        26 => ("photoacoustic", None),
        31 => ("raman_intensity", None),
        _ => ("intensity", None),
    }
}

/// ISO 8601 (UTC) of an OMNIC timestamp: seconds since 1899-12-31T00:00:00Z.
pub(crate) fn omnic_time(raw: u32) -> Option<String> {
    if raw == 0 {
        return None;
    }
    let unix = i64::from(raw) - EPOCH_1899;
    Some(iso_utc(unix))
}

/// `YYYY-MM-DDTHH:MM:SSZ` of Unix seconds (proleptic Gregorian, civil-from-days).
pub(crate) fn iso_utc(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// A value from the history text: `Resolution:\t 4.000 from …` (English or French OMNIC,
/// decimal comma accepted).
fn history_number(history: &str, labels: &[&str]) -> Option<f64> {
    for line in history.lines() {
        let l = line.trim();
        for lab in labels {
            if let Some(rest) = l.strip_prefix(lab) {
                let rest = rest.trim_start_matches([':', '\t', ' ']);
                let tok: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',' || *c == '-')
                    .collect();
                if let Ok(v) = tok.replace(',', ".").parse::<f64>() {
                    return Some(v);
                }
            }
        }
    }
    None
}

fn history_text(history: &str, labels: &[&str]) -> Option<String> {
    for line in history.lines() {
        let l = line.trim();
        for lab in labels {
            if let Some(rest) = l.strip_prefix(lab) {
                let v = rest
                    .trim_start_matches([':', '\t', ' '])
                    .trim_end_matches(';')
                    .trim();
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// Text of a record: cut at the first NUL, CR LF kept as line breaks, Latin-1 when not UTF-8.
fn record_text(b: &[u8]) -> String {
    text_field(b)
}

/// The experiment-information record (key 130, first byte 0x79): fixed text slots.
fn experiment_info(b: &[u8]) -> Option<BTreeMap<&'static str, String>> {
    if b.first() != Some(&0x79) {
        return None;
    }
    let mut out = BTreeMap::new();
    for (name, a, z) in [
        ("experiment_path", 10usize, 90usize),
        ("experiment_title", 90, 154),
        ("experiment_description", 154, 413),
        ("accessory", 413, 670),
    ] {
        if a >= b.len() {
            continue;
        }
        let v = text_field(&b[a..z.min(b.len())]);
        if !v.is_empty() {
            out.insert(name, v);
        }
    }
    (!out.is_empty()).then_some(out)
}

struct Spectrum {
    index: u16,
    header: Header,
    data: Option<Key>,
    title: Option<String>,
    time: Option<u32>,
    history: Option<String>,
}

/// Parse an OMNIC `.spa` or `.spg` file.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    let head = read_at(f, path, 0, 304, file_len)?;
    if head.len() < 18 {
        return Err(Error::corrupt(
            FMT,
            "file is shorter than the OMNIC signature",
        ));
    }
    if head[..18] == OMNIC_SERIES_MAGIC[..] {
        return crate::omnic_srs::parse(f, path, file_len);
    }
    if head[..18] != OMNIC_MAGIC[..] {
        return Err(Error::corrupt(
            FMT,
            "not an OMNIC file: it does not start with `Spectral Data File`",
        ));
    }
    if head.len() < 304 {
        return Err(Error::corrupt(
            FMT,
            "file is shorter than the 304-byte OMNIC header",
        ));
    }
    let title = text_field(head.bytes_at(30, 256).unwrap_or_default());
    let nkeys = usize::from(head.u16_at(294).unwrap_or(0)).min(MAX_KEYS);
    let file_time = head.u32_at(296).unwrap_or(0);
    let table = read_at(f, path, 304, (nkeys as u64) * 16, file_len)?;
    let mut findings = Vec::new();
    if table.len() < nkeys * 16 {
        findings.push(Finding::error(
            "truncated",
            format!("the key table of {nkeys} records runs past the end of the file"),
        ));
    }
    let mut keys = Vec::new();
    for i in 0..table.len() / 16 {
        let at = i * 16;
        let k = Key {
            key: table[at],
            offset: u64::from(table.u32_at(at + 2).unwrap_or(0)),
            length: u64::from(table.u32_at(at + 6).unwrap_or(0)),
            spectrum: table.u16_at(at + 10).unwrap_or(0),
        };
        if k.offset.saturating_add(k.length) > file_len {
            findings.push(
                Finding::error(
                    "truncated",
                    format!(
                        "record {i} (key {}) at {} of {} bytes runs past the end of the file",
                        k.key, k.offset, k.length
                    ),
                )
                .at(k.offset),
            );
        }
        keys.push(k);
    }
    let small = |k: &Key, cap: u64| -> Result<Vec<u8>> {
        read_at(f, path, k.offset, k.length.min(cap), file_len)
    };

    // Group the records by spectrum. Group files number their spectra in the record (bytes
    // 10–11); a spectrum starts at each key 2.
    let mut spectra: Vec<Spectrum> = Vec::new();
    let mut comments = Vec::new();
    let mut infos: Vec<BTreeMap<&'static str, String>> = Vec::new();
    let mut acq_blocks = Vec::new();
    let mut ifgs: Vec<(u8, Key)> = Vec::new();
    for k in &keys {
        match k.key {
            2 => {
                let b = small(k, 4096)?;
                match header(&b) {
                    Some(h) => spectra.push(Spectrum {
                        index: k.spectrum,
                        header: h,
                        data: None,
                        title: None,
                        time: None,
                        history: None,
                    }),
                    None => findings.push(Finding::error(
                        "bad_header",
                        format!("spectrum header at {} is too short", k.offset),
                    )),
                }
            }
            3 => {
                if let Some(s) = spectra.iter_mut().rev().find(|s| s.index == k.spectrum)
                    && s.data.is_none()
                {
                    s.data = Some(*k);
                }
            }
            107 => {
                let b = small(k, 264)?;
                if let Some(s) = spectra.iter_mut().rev().find(|s| s.index == k.spectrum) {
                    s.title = Some(text_field(b.bytes_at(0, 256).unwrap_or(&b)));
                    s.time = b.u32_at(256);
                }
            }
            27 => {
                let b = small(k, 1 << 20)?;
                if let Some(s) = spectra.iter_mut().rev().find(|s| s.index == k.spectrum) {
                    s.history = Some(record_text(&b));
                }
            }
            4 => {
                let b = small(k, 1 << 20)?;
                let t = record_text(&b);
                if !t.is_empty() {
                    comments.push(t);
                }
            }
            130 => {
                let b = small(k, 4096)?;
                if let Some(i) = experiment_info(&b) {
                    infos.push(i);
                }
            }
            106 => {
                acq_blocks.push(small(k, 4096)?);
            }
            102 | 103 => ifgs.push((k.key, *k)),
            _ => {}
        }
    }
    if spectra.is_empty() {
        return Err(Error::corrupt(
            FMT,
            "no spectrum header (key 2) in the key table",
        ));
    }
    let is_group = spectra.len() > 1 || spectra.iter().any(|s| s.title.is_some());

    let mut parsed = Parsed {
        findings,
        ..Parsed::default()
    };
    // Sets: spectra sharing points, axis ends and units.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, s) in spectra.iter().enumerate() {
        if s.data.is_none() {
            parsed.findings.push(Finding::error(
                "missing_data",
                format!("spectrum {} has a header but no intensity record", s.index),
            ));
            continue;
        }
        let h = &s.header;
        match groups.iter_mut().find(|g| {
            let o = &spectra[g[0]].header;
            o.points == h.points
                && o.x_first.to_bits() == h.x_first.to_bits()
                && o.x_last.to_bits() == h.x_last.to_bits()
                && o.x_code == h.x_code
                && o.y_code == h.y_code
        }) {
            Some(g) => g.push(i),
            None => groups.push(vec![i]),
        }
    }
    for g in &groups {
        let s0 = &spectra[g[0]];
        let h = &s0.header;
        let (xq, xu) = x_axis(h.x_code);
        let (yq, yu) = y_axis(h.y_code);
        let points = u64::from(h.points);
        let offsets: Vec<u64> = g
            .iter()
            .filter_map(|&i| spectra[i].data.map(|k| k.offset))
            .collect();
        for &i in g {
            if let Some(k) = spectra[i].data
                && k.length < points * 4
            {
                parsed.findings.push(Finding::error(
                    "short_data",
                    format!(
                        "spectrum {}: {} bytes of intensities for {} points",
                        spectra[i].index, k.length, points
                    ),
                ));
            }
        }
        let dtype = match (h.x_code, h.y_code) {
            (2, _) | (_, 22) => "INFRARED INTERFEROGRAM",
            (32, _) | (_, 31) => "RAMAN SPECTRUM",
            (3, _) => "UV/VIS SPECTRUM",
            _ => "INFRARED SPECTRUM",
        };
        let mut extra = BTreeMap::new();
        let history = s0.history.clone().unwrap_or_default();
        if h.scans > 0 {
            extra.insert("scans".into(), json!(h.scans));
        }
        if h.background_scans > 0 {
            extra.insert("background_scans".into(), json!(h.background_scans));
        }
        if h.laser_frequency.is_finite() && h.laser_frequency > 0.0 {
            extra.insert(
                "laser_wavenumber_cm1".into(),
                num(f64::from(h.laser_frequency)),
            );
        }
        if dtype == "RAMAN SPECTRUM"
            && let Some(nm) = nm_of_wavenumber(f64::from(h.raman_excitation))
        {
            extra.insert("laser_wavelength_nm".into(), num(nm));
            extra.insert(
                "raman_excitation_cm1".into(),
                num(f64::from(h.raman_excitation)),
            );
        }
        if h.optical_velocity.is_finite() && h.optical_velocity > 0.0 {
            extra.insert(
                "optical_velocity".into(),
                num(f64::from(h.optical_velocity)),
            );
        }
        if h.aperture.is_finite() && h.aperture > 0.0 {
            extra.insert("aperture".into(), num(f64::from(h.aperture)));
        }
        if h.background_gain.is_finite() && h.background_gain > 0.0 {
            extra.insert("background_gain".into(), num(f64::from(h.background_gain)));
        }
        if h.collection_length > 0 {
            extra.insert(
                "collection_time_s".into(),
                num(f64::from(h.collection_length) / 100.0),
            );
        }
        for (k, v) in [
            ("scan_points", h.scan_points),
            ("peak_position", h.peak_position),
            ("fft_points", h.fft_points),
        ] {
            if v > 0 {
                extra.insert(k.into(), json!(v));
            }
        }
        if h.sample_spacing.is_finite() && h.sample_spacing > 0.0 {
            extra.insert("sample_spacing".into(), num(f64::from(h.sample_spacing)));
        }
        if let Some(r) = history_number(
            &history,
            &["Resolution", "Résolution", "Resolucion", "Resolución"],
        ) {
            extra.insert("resolution_cm1".into(), num(r));
        }
        if let Some(v) = history_text(&history, &["Final format", "Format final", "Formato final"])
        {
            extra.insert("final_format".into(), json!(v));
        }
        if let Some(v) = history_text(
            &history,
            &["Bench Serial Number", "Numéro de série du banc"],
        ) {
            extra.insert("instrument_serial".into(), json!(v));
        }
        if !history.is_empty() {
            extra.insert("history".into(), json!(history));
        }
        extra.insert("x_units_code".into(), json!(h.x_code));
        extra.insert("y_units_code".into(), json!(h.y_code));
        if !is_group {
            if let Some(t) = omnic_time(file_time) {
                extra.insert("acquired_at".into(), json!(t));
            }
            if !title.is_empty() {
                extra.insert("title".into(), json!(title));
            }
        } else if let Some(t) = s0.time.and_then(omnic_time) {
            extra.insert("acquired_at".into(), json!(t));
        }
        if is_group {
            let titles: Vec<Value> = g
                .iter()
                .map(|&i| json!(spectra[i].title.clone().unwrap_or_default()))
                .collect();
            if titles.len() <= 10_000 {
                extra.insert("spectrum_titles".into(), Value::Array(titles));
            }
        }
        let trace = parsed.sets.len() as u32;
        let name = if is_group {
            format!("{yq} ({} spectra)", g.len())
        } else if title.is_empty() {
            yq.to_string()
        } else {
            title.clone()
        };
        parsed.sets.push(SpectrumSet {
            name,
            x_quantity: xq,
            x_unit: xu.map(str::to_string),
            x: XValues::Regular {
                first: f64::from(h.x_first),
                last: f64::from(h.x_last),
            },
            y_name: yq.into(),
            y_unit: yu.map(str::to_string),
            points,
            count: u32::try_from(offsets.len()).unwrap_or(u32::MAX),
            rows: Rows::Listed(offsets),
            stored: Stored::F32,
            scale: 1.0,
            data_type: dtype,
            extra,
        });
        if is_group {
            let times: Vec<f64> = g
                .iter()
                .map(|&i| {
                    spectra[i]
                        .time
                        .filter(|t| *t != 0)
                        .map_or(f64::NAN, |t| (i64::from(t) - EPOCH_1899) as f64)
                })
                .collect();
            let t0 = times
                .iter()
                .copied()
                .filter(|t| t.is_finite())
                .fold(f64::INFINITY, f64::min);
            parsed.tables.push(SpectrumTable {
                name: "spectra".into(),
                trace,
                columns: vec![
                    (
                        "spectrum".into(),
                        None,
                        g.iter().map(|&i| f64::from(spectra[i].index)).collect(),
                    ),
                    ("acquired_unix_s".into(), Some("s".into()), times.clone()),
                    (
                        "elapsed_s".into(),
                        Some("s".into()),
                        times.iter().map(|t| t - t0).collect(),
                    ),
                ],
            });
        }
    }
    // Interferograms stored with a single spectrum (keys 102 sample, 103 background).
    if !is_group {
        // sample interferogram first, then background
        ifgs.sort_by_key(|(key, k)| (*key, k.offset));
        for (key, k) in &ifgs {
            let n = k.length / 4;
            if n == 0 {
                continue;
            }
            let who = if *key == 102 { "sample" } else { "background" };
            let mut extra = BTreeMap::new();
            extra.insert("spectrum_role".into(), json!(who));
            parsed.sets.push(SpectrumSet {
                name: format!("{who} interferogram"),
                x_quantity: "points",
                x_unit: None,
                x: XValues::Regular {
                    first: 0.0,
                    last: (n - 1) as f64,
                },
                y_name: "detector_signal".into(),
                y_unit: Some("V".into()),
                points: n,
                count: 1,
                rows: Rows::Listed(vec![k.offset]),
                stored: Stored::F32,
                scale: 1.0,
                data_type: "INFRARED INTERFEROGRAM",
                extra,
            });
        }
    }

    // Facts.
    let mut facts = Facts::default();
    Facts::text(&mut facts.vendor, "Thermo Fisher Scientific", "format");
    Facts::text(&mut facts.software, "OMNIC", "format");
    if !title.is_empty() && !is_group {
        Facts::text(&mut facts.sample_name, &title, "spectrum title (offset 30)");
    }
    if is_group && !title.is_empty() {
        Facts::text(&mut facts.method_name, &title, "group title (offset 30)");
    }
    let first_time = if is_group {
        spectra
            .iter()
            .filter_map(|s| s.time)
            .filter(|t| *t != 0)
            .min()
    } else {
        Some(file_time)
    };
    if let Some(t) = first_time.and_then(omnic_time) {
        Facts::text(
            &mut facts.started_at,
            &t,
            "timestamp (seconds since 1899-12-31 UTC)",
        );
    }
    if let Some(c) = comments.first() {
        Facts::text(&mut facts.comment, c, "comment record (key 4)");
    }
    if let Some(i) = infos.first() {
        if let Some(t) = i.get("experiment_title") {
            Facts::text(
                &mut facts.method_name,
                t,
                "experiment information (key 130)",
            );
        }
        if let Some(a) = i.get("accessory") {
            facts.word(
                "accessory",
                a,
                "experiment information (key 130)",
                Source::PriorArt,
            );
        }
    }
    if let Some(s) = parsed.sets.first() {
        let e = &s.extra;
        let inf = Source::Inferred;
        let pa = Source::PriorArt;
        if let Some(v) = e.get("resolution_cm1").and_then(Value::as_f64) {
            facts.number(
                "resolution",
                v,
                Some("cm⁻¹"),
                "history text (Resolution)",
                inf,
            );
        }
        if let Some(v) = e.get("scans").and_then(Value::as_f64) {
            facts.number("scans", v, None, "spectrum header +36", pa);
        }
        if let Some(v) = e.get("background_scans").and_then(Value::as_f64) {
            facts.number("background_scans", v, None, "spectrum header +52", pa);
        }
        if let Some(v) = e.get("laser_wavenumber_cm1").and_then(Value::as_f64) {
            facts.number(
                "laser_wavenumber",
                v,
                Some("cm⁻¹"),
                "spectrum header +80",
                pa,
            );
        }
        if let Some(v) = e.get("laser_wavelength_nm").and_then(Value::as_f64) {
            facts.number("laser_wavelength", v, Some("nm"), "spectrum header +96", pa);
        }
        if let Some(v) = e.get("instrument_serial").and_then(Value::as_str) {
            Facts::text(&mut facts.serial, v, "history text (Bench Serial Number)");
        }
        if let Some(v) = e.get("final_format").and_then(Value::as_str) {
            facts.word("final_format", v, "history text (Final format)", inf);
        }
    }
    parsed.facts = facts;
    parsed.notes.push(if is_group {
        format!("OMNIC group file (.spg) of {} spectra", spectra.len())
    } else {
        "OMNIC single-spectrum file (.spa)".to_string()
    });

    // Vendor tree and listing.
    let key_list: Vec<Value> = keys
        .iter()
        .map(|k| json!({"key": k.key, "offset": k.offset, "length": k.length, "spectrum": k.spectrum}))
        .collect();
    let acq: Vec<Value> = acq_blocks
        .iter()
        .map(|b| {
            json!({
                "digitizer_bits": b.u32_at(16),
                "high_pass_filter": b.f32_at(20).map(f64::from).map(num),
                "low_pass_filter": b.f32_at(24).map(f64::from).map(num),
                "sample_gain": b.f32_at(44).map(f64::from).map(num),
                "optical_velocity": b.f32_at(48).map(f64::from).map(num),
            })
        })
        .collect();
    parsed.vendor = json!({
        "title": title,
        "timestamp": file_time,
        "keys": key_list,
        "comments": comments,
        "experiment_information": infos.iter().map(|m| json!(m)).collect::<Vec<_>>(),
        "acquisition_parameters": acq,
    });
    for k in &keys {
        let kind = match k.key {
            2 => "header",
            3 => "intensities",
            4 => "comment",
            27 => "history",
            102 => "sample_interferogram",
            103 => "background_interferogram",
            106 => "acquisition_parameters",
            107 => "spectrum_title",
            130 => "experiment_information",
            _ => "record",
        };
        parsed.entries.push(LsEntry {
            kind: kind.into(),
            name: format!("key {}", k.key),
            offset: Some(k.offset),
            size: Some(k.length),
            image: None,
            details: json!({"key": k.key, "spectrum": k.spectrum}),
        });
    }
    for (k, v) in [
        ("traces[].sample_count", Source::PriorArt),
        ("traces[].sweep_count", Source::PriorArt),
        ("traces[].extra.axis", Source::PriorArt),
        ("traces[].extra.y_quantity", Source::PriorArt),
        ("traces[].extra.scans", Source::PriorArt),
        ("traces[].extra.background_scans", Source::PriorArt),
        ("traces[].extra.laser_wavenumber_cm1", Source::PriorArt),
        ("traces[].extra.laser_wavelength_nm", Source::PriorArt),
        ("traces[].extra.acquired_at", Source::PriorArt),
        ("traces[].extra.resolution_cm1", Source::Inferred),
        ("traces[].extra.instrument_serial", Source::Inferred),
        ("traces[].extra.data_type", Source::Inferred),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        // 1970-01-01T00:00:00Z
        assert_eq!(
            omnic_time(2_209_075_200).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        assert_eq!(omnic_time(0), None);
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn history_values() {
        let h = "Collect Sample\r\n\t Final format:\tAbsorbance\r\n\t Resolution:\t 4.000 from 400.1569 to 4000.1230\r\n\t Bench Serial Number:ASB2211355;\r\n";
        assert_eq!(history_number(h, &["Resolution"]), Some(4.0));
        assert_eq!(
            history_text(h, &["Final format"]).as_deref(),
            Some("Absorbance")
        );
        assert_eq!(
            history_text(h, &["Bench Serial Number"]).as_deref(),
            Some("ASB2211355")
        );
        let fr = "\t Résolution:\t 4,000 de 0,0000 à 4159,0000\r\n";
        assert_eq!(history_number(fr, &["Résolution"]), Some(4.0));
    }
}
