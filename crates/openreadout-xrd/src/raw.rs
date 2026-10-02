//! Bruker (Siemens) DIFFRAC `.raw`: `RAW1.01` (DIFFRACplus, "version 3") and `RAW4.00`
//! (DIFFRAC.SUITE, "version 4"). Notes: `docs/formats/xrd.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Scope};
use openreadout_core::bytes::{latin1, le_f32, le_f64, le_u32, until_nul};
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile, json_num};
use openreadout_core::time::full_year;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

use crate::common::{FileParts, Scan, ScanAxis, axis_of, finish};

pub(crate) const FORMAT_ID: &str = "bruker-raw";

/// Largest number of ranges accepted.
const MAX_RANGES: usize = 4096;
/// Largest range header or extra record accepted.
const MAX_HEADER: usize = 1 << 20;

/// True when the file starts with a Bruker RAW signature (any version).
pub(crate) fn looks_like(head: &[u8]) -> bool {
    head.starts_with(b"RAW1.01")
        || head.starts_with(b"RAW4.00")
        || head.starts_with(b"RAW2")
        || (head.starts_with(b"RAW ") && head.len() >= 8)
}

fn corrupt(at: usize, msg: impl Into<String>) -> Error {
    Error::corrupt_at(FORMAT_ID, at as u64, msg.into())
}

fn past_end(b: &[u8], at: usize, n: usize) -> Error {
    corrupt(
        at,
        format!("{n} bytes at {at} run past the end ({} bytes)", b.len()),
    )
}

fn bytes(b: &[u8], at: usize, n: usize) -> Result<&[u8]> {
    at.checked_add(n)
        .and_then(|e| b.get(at..e))
        .ok_or_else(|| past_end(b, at, n))
}

/// The value `get` reads at `at`, or a corrupt error when it runs past the end.
fn read<T>(b: &[u8], at: usize, get: fn(&[u8], usize) -> Option<T>) -> Result<T> {
    get(b, at).ok_or_else(|| past_end(b, at, size_of::<T>()))
}

fn usize_at(b: &[u8], at: usize) -> Result<usize> {
    usize::try_from(read(b, at, le_u32)?).map_err(|_| corrupt(at, "value too large"))
}

/// NUL-terminated Latin-1/UTF-8 text in a fixed field.
fn text(b: &[u8], at: usize, n: usize) -> Result<String> {
    let s = until_nul(bytes(b, at, n)?);
    let t = match std::str::from_utf8(s) {
        Ok(t) => t.to_string(),
        Err(_) => latin1(s),
    };
    Ok(t.trim().to_string())
}

/// `MM/DD/YY` or `MM/DD/YYYY` with `HH:MM:SS` as ISO-8601 local time (no zone in the file).
fn us_date(date: &str, time: &str) -> Option<String> {
    let parts: Vec<&str> = date.trim().split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let month: u32 = parts[0].parse().ok()?;
    let day: u32 = parts[1].parse().ok()?;
    let mut year: u32 = parts[2].parse().ok()?;
    if parts[2].len() <= 2 {
        year = full_year(year);
    }
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let hms: Vec<u32> = time
        .trim()
        .split(':')
        .filter_map(|x| x.parse().ok())
        .collect();
    Some(match hms.as_slice() {
        [h, mi, s] if *h < 24 && *mi < 60 && *s < 61 => {
            format!("{year:04}-{month:02}-{day:02}T{h:02}:{mi:02}:{s:02}")
        }
        _ => format!("{year:04}-{month:02}-{day:02}"),
    })
}

/// One range (scan) as found in the file.
struct Range {
    start: f64,
    step: f64,
    points: usize,
    stride: usize,
    data: usize,
    measured_two_theta: bool,
    scan_type: String,
    /// The scanned drive, when the file names it.
    drive: Option<String>,
    extra: BTreeMap<String, Value>,
}

fn check_axis(at: usize, start: f64, step: f64) -> Result<()> {
    if !start.is_finite() || !step.is_finite() {
        return Err(corrupt(at, "range start or step is not a finite number"));
    }
    Ok(())
}

/// RAW1.01 ("version 3") ranges and file facts.
fn v3(b: &[u8], facts: &mut Facts, vendor: &mut Map<String, Value>) -> Result<Vec<Range>> {
    bytes(b, 0, 712)?;
    let count = usize_at(b, 12)?;
    if !(1..=MAX_RANGES).contains(&count) {
        return Err(corrupt(12, format!("range count {count}")));
    }
    let date = text(b, 0x10, 10)?;
    let time = text(b, 0x1A, 10)?;
    let user = text(b, 0x24, 72)?;
    let site = text(b, 0x6C, 218)?;
    let sample = text(b, 0x146, 60)?;
    let anode = text(b, 0x260, 4)?;
    let wl = [
        read(b, 0x270, le_f64)?,
        read(b, 0x278, le_f64)?,
        read(b, 0x280, le_f64)?,
        read(b, 0x288, le_f64)?,
    ];
    let radius = f64::from(read(b, 0x234, le_f32)?);
    facts.set("acquisition.operator", &user, "RAW1.01 user (0x24)");
    facts.set("sample.id", &sample, "RAW1.01 sample (0x146)");
    if let Some(s) = us_date(&date, &time) {
        facts.set(
            "acquisition.started_at",
            &s,
            "RAW1.01 date and time (0x10, 0x1A; local time)",
        );
    }
    facts.plain("anode", anode.as_str(), "RAW1.01 anode (0x260)");
    if wl[0] > 0.0 {
        facts.number("wavelength_kalpha1", wl[0], "Å", "RAW1.01 Kα1 (0x270)");
        facts.number("wavelength_kalpha2", wl[1], "Å", "RAW1.01 Kα2 (0x278)");
        facts.number("wavelength_kbeta", wl[2], "Å", "RAW1.01 Kβ (0x280)");
        facts.plain("kalpha2_kalpha1_ratio", wl[3], "RAW1.01 Kα2/Kα1 (0x288)");
    }
    if radius > 0.0 {
        facts.number(
            "goniometer_radius",
            radius,
            "mm",
            "RAW1.01 goniometer radius (0x234)",
        );
    }
    vendor.insert("date".into(), json!(date));
    vendor.insert("time".into(), json!(time));
    vendor.insert("user".into(), json!(user));
    vendor.insert("site".into(), json!(site));
    vendor.insert("sample".into(), json!(sample));
    vendor.insert("anode".into(), json!(anode));
    vendor.insert("wavelengths_angstrom".into(), json!({"kalpha1": json_num(wl[0]), "kalpha2": json_num(wl[1]), "kbeta": json_num(wl[2]), "ratio": json_num(wl[3])}));
    vendor.insert("goniometer_code".into(), json!(read(b, 0x224, le_u32)?));
    vendor.insert("stage_code".into(), json!(read(b, 0x228, le_u32)?));
    let mut cursor = 712usize;
    let mut ranges = Vec::with_capacity(count);
    for k in 0..count {
        let hs = usize_at(b, cursor)?;
        if !(260..=MAX_HEADER).contains(&hs) {
            return Err(corrupt(cursor, format!("range {} header size {hs}", k + 1)));
        }
        bytes(b, cursor, hs)?;
        let points = usize_at(b, cursor + 4)?;
        let theta = read(b, cursor + 8, le_f64)?;
        let start = read(b, cursor + 16, le_f64)?;
        let step = read(b, cursor + 176, le_f64)?;
        check_axis(cursor, start, step)?;
        let scan_type = read(b, cursor + 196, le_u32)?;
        let varying = read(b, cursor + 248, le_u32)?;
        let stride = usize_at(b, cursor + 252)?;
        let extra_len = usize_at(b, cursor + 256)?;
        let expected = 4 + 8 * varying.count_ones() as usize;
        if stride != expected {
            return Err(corrupt(
                cursor + 252,
                format!(
                    "range {}: record size {stride} disagrees with the varying-parameter bits {varying:#x}",
                    k + 1
                ),
            ));
        }
        if varying & !1 != 0 {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("per-point varying parameters {varying:#x} other than 2θ"),
                "Only ranges whose records hold the intensity and optionally the measured 2θ are read.",
            ));
        }
        if extra_len > MAX_HEADER {
            return Err(corrupt(cursor + 256, "extra record too long"));
        }
        let data = cursor
            .checked_add(hs)
            .and_then(|v| v.checked_add(extra_len))
            .ok_or_else(|| corrupt(cursor, "offset overflow"))?;
        let len = points
            .checked_mul(stride)
            .ok_or_else(|| corrupt(cursor, "range size overflows"))?;
        bytes(b, data, len)?;
        let step_time = f64::from(read(b, cursor + 192, le_f32)?);
        let kv = read(b, cursor + 224, le_u32)?;
        let ma = read(b, cursor + 228, le_u32)?;
        let mut extra = BTreeMap::new();
        extra.insert("start_theta".into(), json_num(theta));
        extra.insert("step_time_s".into(), json_num(step_time));
        extra.insert("scan_type_code".into(), json!(scan_type));
        extra.insert("tube_voltage_kv".into(), json!(kv));
        extra.insert("tube_current_ma".into(), json!(ma));
        extra.insert(
            "range_wavelength_angstrom".into(),
            json_num(read(b, cursor + 240, le_f64)?),
        );
        extra.insert(
            "detector".into(),
            json!({"high_voltage": json_num(f64::from(read(b, cursor + 100, le_f32)?)), "gain": json_num(f64::from(read(b, cursor + 104, le_f32)?)),
                   "lower_discriminator": json_num(f64::from(read(b, cursor + 108, le_f32)?)), "upper_discriminator": json_num(f64::from(read(b, cursor + 112, le_f32)?))}),
        );
        if k == 0 {
            facts.number(
                "step_time",
                step_time,
                "s",
                "RAW1.01 range step time (+192)",
            );
            facts.number(
                "tube_voltage",
                f64::from(kv),
                "kV",
                "RAW1.01 range kV (+224)",
            );
            facts.number(
                "tube_current",
                f64::from(ma),
                "mA",
                "RAW1.01 range mA (+228)",
            );
        }
        ranges.push(Range {
            start,
            step,
            points,
            stride,
            data,
            measured_two_theta: varying & 1 != 0,
            scan_type: match scan_type {
                0 => "locked coupled".into(),
                1 => "unlocked coupled".into(),
                2 => "detector scan".into(),
                n => format!("scan type {n}"),
            },
            drive: None,
            extra,
        });
        cursor = data + len;
    }
    Ok(ranges)
}

/// RAW4.00 ("version 4") ranges and file facts.
fn v4(b: &[u8], facts: &mut Facts, vendor: &mut Map<String, Value>) -> Result<Vec<Range>> {
    bytes(b, 0, 61)?;
    let date = text(b, 0x0C, 12)?;
    let time = text(b, 0x18, 12)?;
    if let Some(s) = us_date(&date, &time) {
        facts.set(
            "acquisition.started_at",
            &s,
            "RAW4.00 date and time (0x0C, 0x18; local time)",
        );
    }
    vendor.insert("date".into(), json!(date));
    vendor.insert("time".into(), json!(time));
    let mut texts = Map::new();
    let mut cursor = 61usize;
    let mut ranges = Vec::new();
    while cursor < b.len() {
        // zero padding at the very end
        if b.len() - cursor < 8 && b[cursor..].iter().all(|&c| c == 0) {
            break;
        }
        let kind = read(b, cursor, le_u32)?;
        if kind == 0 || kind == 160 {
            if ranges.len() >= MAX_RANGES {
                return Err(corrupt(cursor, "too many ranges"));
            }
            let (r, end) = range4(b, cursor, ranges.is_empty(), facts)?;
            ranges.push(r);
            cursor = end;
            continue;
        }
        let len = usize_at(b, cursor + 4)?;
        if len < 8 || len > b.len() - cursor {
            return Err(corrupt(
                cursor,
                format!("record of kind {kind} declares length {len}"),
            ));
        }
        match kind {
            10 if len >= 36 => {
                let name = text(b, cursor + 12, 24)?;
                let v = bytes(b, cursor + 36, len - 36)?;
                let v = String::from_utf8_lossy(v)
                    .trim_end_matches(['\0', '\n', '\r'])
                    .trim()
                    .to_string();
                texts.insert(name, json!(v));
            }
            30 if len >= 0x78 => {
                let anode = text(b, cursor + 0x74, 4)?;
                let wl = [
                    read(b, cursor + 0x50, le_f64)?,
                    read(b, cursor + 0x58, le_f64)?,
                    read(b, cursor + 0x60, le_f64)?,
                    read(b, cursor + 0x68, le_f64)?,
                ];
                facts.plain(
                    "anode",
                    anode.as_str(),
                    "RAW4.00 instrument record (kind 30, +0x74)",
                );
                if wl[0] > 0.0 {
                    facts.number(
                        "wavelength_kalpha1",
                        wl[0],
                        "Å",
                        "RAW4.00 instrument record (+0x50)",
                    );
                    facts.number(
                        "wavelength_kalpha2",
                        wl[1],
                        "Å",
                        "RAW4.00 instrument record (+0x58)",
                    );
                    facts.number(
                        "wavelength_kbeta",
                        wl[2],
                        "Å",
                        "RAW4.00 instrument record (+0x60)",
                    );
                    facts.plain(
                        "kalpha2_kalpha1_ratio",
                        wl[3],
                        "RAW4.00 instrument record (+0x68)",
                    );
                }
                vendor.insert("anode".into(), json!(anode));
                vendor.insert(
                    "wavelengths_angstrom".into(),
                    json!({"kalpha_average": json_num(read(b, cursor + 0x48, le_f64)?), "kalpha1": json_num(wl[0]), "kalpha2": json_num(wl[1]), "kbeta": json_num(wl[2]), "ratio": json_num(wl[3])}),
                );
            }
            _ => {}
        }
        cursor += len;
    }
    for (k, from) in [
        ("USER", "acquisition.operator"),
        ("SAMPLEID", "sample.id"),
        ("COMMENT", "acquisition.comment"),
    ] {
        if let Some(v) = texts.get(k).and_then(Value::as_str) {
            facts.set(from, v, &format!("RAW4.00 text record {k}"));
        }
    }
    if let Some(v) = texts.get("CREATOR").and_then(Value::as_str) {
        facts.set("instrument.software", v, "RAW4.00 text record CREATOR");
    }
    if let Some(v) = texts.get("CREATOR_VERSION").and_then(Value::as_str) {
        facts.set(
            "instrument.software_version",
            v,
            "RAW4.00 text record CREATOR_VERSION",
        );
    }
    vendor.insert("text_records".into(), Value::Object(texts));
    if ranges.is_empty() {
        return Err(corrupt(61, "no measurement range"));
    }
    Ok(ranges)
}

fn range4(b: &[u8], at: usize, first: bool, facts: &mut Facts) -> Result<(Range, usize)> {
    bytes(b, at, 160)?;
    let scan_type = text(b, at + 32, 24)?;
    let start = read(b, at + 72, le_f64)?;
    let step = read(b, at + 80, le_f64)?;
    check_axis(at, start, step)?;
    let points = usize_at(b, at + 88)?;
    let stride = usize_at(b, at + 136)?;
    let hs = usize_at(b, at + 140)?;
    if stride != 4 && stride != 8 {
        return Err(Error::unsupported(
            FORMAT_ID,
            format!("RAW4.00 data records of {stride} bytes"),
            "Only ranges whose records hold a float32 intensity (4 bytes, or 8 with a second value) are read.",
        ));
    }
    if hs > MAX_HEADER {
        return Err(corrupt(at + 140, "drive records too long"));
    }
    let hdr = at + 160;
    bytes(b, hdr, hs)?;
    let data = hdr + hs;
    let len = points
        .checked_mul(stride)
        .ok_or_else(|| corrupt(at, "range size overflows"))?;
    bytes(b, data, len)?;
    // the scanned drive: its flag is set and its position is the range start
    let mut cur = hdr;
    let mut drives = Vec::new();
    let mut scanned = Vec::new();
    while cur + 8 <= data {
        let kind = read(b, cur, le_u32)?;
        let rl = usize_at(b, cur + 4)?;
        if rl < 8 || rl > data - cur {
            return Err(corrupt(
                cur,
                format!("drive record of kind {kind} declares length {rl}"),
            ));
        }
        if kind == 50 && rl >= 64 {
            let flag = read(b, cur + 8, le_u32)?;
            let name = text(b, cur + 12, 24)?;
            let pos = read(b, cur + 56, le_f64)?;
            drives.push(json!({"name": name, "flag": flag, "position": json_num(pos)}));
            if flag != 0 && (pos - start).abs() <= 1e-6 + 1e-5 * start.abs() {
                scanned.push(name);
            }
        }
        cur += rl;
    }
    let mut extra = BTreeMap::new();
    extra.insert("scan_type".into(), json!(scan_type));
    extra.insert("drives".into(), json!(drives));
    let kv = f64::from(read(b, at + 100, le_f32)?);
    let ma = f64::from(read(b, at + 104, le_f32)?);
    extra.insert("tube_voltage_kv".into(), json_num(kv));
    extra.insert("tube_current_ma".into(), json_num(ma));
    extra.insert(
        "range_wavelength_angstrom".into(),
        json_num(read(b, at + 112, le_f64)?),
    );
    if first {
        facts.number("tube_voltage", kv, "kV", "RAW4.00 range header (+100)");
        facts.number("tube_current", ma, "mA", "RAW4.00 range header (+104)");
    }
    Ok((
        Range {
            start,
            step,
            points,
            stride,
            data,
            measured_two_theta: false,
            scan_type,
            drive: (scanned.len() == 1).then(|| scanned[0].clone()),
            extra,
        },
        data + len,
    ))
}

/// The axis a range is sampled on.
fn range_axis(r: &Range) -> Result<ScanAxis> {
    if let Some(d) = &r.drive {
        let a = axis_of(d, "deg");
        if a.quantity == "angle" {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("a scan of the drive `{d}`"),
                "Only 2θ, θ, ω, φ and χ scans are read.",
            ));
        }
        return Ok(a);
    }
    let t = r.scan_type.to_ascii_lowercase().replace(['-', '_'], " ");
    if t.contains("coupled") || t.contains("detector scan") || t.contains("locked") {
        Ok(axis_of("2Theta", "deg"))
    } else {
        Err(Error::unsupported(
            FORMAT_ID,
            format!("a `{}` range without a named scanned drive", r.scan_type),
            "The scanned axis could not be established; export the scan from DIFFRAC.EVA.",
        ))
    }
}

/// Parse a Bruker RAW file.
pub(crate) fn parse(b: &[u8]) -> Result<SeriesFile> {
    let mut facts = Facts::new();
    facts
        .set("instrument.vendor", "Bruker", "the file format")
        .instrument_kind("CHMO:0002105");
    let mut vendor = Map::new();
    let (version, ranges) = if b.starts_with(b"RAW1.01") {
        ("RAW1.01", v3(b, &mut facts, &mut vendor)?)
    } else if b.starts_with(b"RAW4.00") {
        ("RAW4.00", v4(b, &mut facts, &mut vendor)?)
    } else {
        return Err(Error::unsupported(
            FORMAT_ID,
            "Bruker RAW version 1 or 2 (`RAW ` / `RAW2`)",
            "Only RAW1.01 and RAW4.00 files are read; convert older files with DIFFRAC's file exchange to RAW4 or export UXD/XY.",
        ));
    };
    let mut observations = openreadout_core::assurance::Observations::default();
    observations.feature(
        FeatureKind::FormatVersion,
        version,
        &[Scope::Traces, Scope::Metadata],
    );
    let mut scans = Vec::new();
    let mut entries = Vec::new();
    let mut findings = Vec::new();
    for (k, r) in ranges.iter().enumerate() {
        let axis = range_axis(r)?;
        observations.feature(
            FeatureKind::Layout,
            format!("{} scan", axis.label),
            &[Scope::Traces],
        );
        let mut intensity = Vec::with_capacity(r.points);
        let mut measured = Vec::new();
        let mut second = Vec::new();
        let v4_wide = version == "RAW4.00" && r.stride == 8;
        for i in 0..r.points {
            let at = r.data + i * r.stride;
            intensity.push(f64::from(read(b, at, le_f32)?));
            if r.measured_two_theta {
                measured.push(read(b, at + 4, le_f64)?);
            } else if v4_wide {
                second.push(f64::from(read(b, at + 4, le_f32)?));
            }
        }
        let mut extra_channels = Vec::<(SeriesChannel, Vec<f64>)>::new();
        if v4_wide {
            observations.feature(
                FeatureKind::SampleLayout,
                "8-byte records",
                &[Scope::Traces],
            );
            extra_channels.push((
                SeriesChannel::new("record_value_2", None, "float32"),
                second,
            ));
            findings.push(Finding::info(
                "second_record_value",
                format!("range {}: 8-byte records; the second float32 of each record (meaning not identified) is channel `record_value_2`", k + 1),
            ));
        }
        let abscissa: Vec<f64> = if r.measured_two_theta {
            measured
        } else {
            (0..r.points).map(|i| r.start + r.step * i as f64).collect()
        };
        if intensity.iter().any(|v| !v.is_finite()) {
            findings.push(Finding::warning(
                "non_finite_intensity",
                format!("range {} holds non-finite intensities", k + 1),
            ));
        }
        entries.push(LsEntry {
            kind: "block".into(),
            name: format!("range {}", k + 1),
            offset: Some(r.data as u64),
            size: Some((r.points * r.stride) as u64),
            image: None,
            details: json!({"points": r.points, "record_bytes": r.stride, "scan_type": r.scan_type}),
        });
        let mut extra = r.extra.clone();
        extra.insert("range".into(), json!(k + 1));
        if let Some(d) = &r.drive {
            extra.insert("scanned_drive".into(), json!(d));
        }
        scans.push(Scan {
            name: String::new(),
            axis,
            abscissa,
            listed: r.measured_two_theta,
            intensity,
            intensity_unit: Some("counts".into()),
            extra_channels,
            extra,
        });
    }
    Ok(finish(
        scans,
        facts,
        FileParts {
            format_version: Some(version.into()),
            vendor: json!({ "bruker_raw": Value::Object(vendor) }),
            entries,
            findings,
            observations,
            check: "every range's header, drive records and data records checked against the file size",
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_and_unknown_files_are_refused() {
        assert!(parse(b"RAW1.01\0").is_err());
        assert!(parse(b"RAW4.00\0").is_err());
        assert!(matches!(
            parse(b"RAW2xxxxxxxx"),
            Err(Error::Unsupported { .. })
        ));
        assert_eq!(
            us_date("06/30/2023", "12:41:34").as_deref(),
            Some("2023-06-30T12:41:34")
        );
        assert_eq!(
            us_date("07/02/24", "08:24:41").as_deref(),
            Some("2024-07-02T08:24:41")
        );
    }
}
