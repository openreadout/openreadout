//! Neware BTS battery-cycler files: `.nda` (one binary file; versions 29 and 130) and `.ndax` (a
//! zip of `.ndc` page files and XML descriptions). Notes: `docs/formats/neware.md`; provenance:
//! `docs/provenance/neware.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Observations, Scope};
use openreadout_core::bytes::{
    self, le_f32, le_i16, le_i32, le_i64, le_u16, le_u32, le_u64, until_nul,
};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{
    Facts, SeriesChannel, SeriesColumn, SeriesFile, SeriesTable, SeriesTrace,
};
use openreadout_core::zip::ZipIndex;
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

pub(crate) const NDA_FORMAT_ID: &str = "neware-nda";
pub(crate) const NDAX_FORMAT_ID: &str = "neware-ndax";

/// Magic of an `.nda` file.
pub(crate) const NDA_MAGIC: &[u8] = b"NEWARE";

/// Largest record count accepted (a guard against absurd counts, not a format limit).
const MAX_RECORDS: usize = 200_000_000;

/// One record as returned (NaN where the file does not store a value).
#[derive(Clone, Debug)]
struct Rec {
    index: u32,
    cycle: f64,
    step_index: f64,
    status: u8,
    step_time: f64,
    voltage: f64,
    current: f64,
    charge_capacity: f64,
    discharge_capacity: f64,
    charge_energy: f64,
    discharge_energy: f64,
    temperature: f64,
}

impl Rec {
    fn empty(index: u32) -> Self {
        Rec {
            index,
            cycle: f64::NAN,
            step_index: f64::NAN,
            status: 0,
            step_time: f64::NAN,
            voltage: f64::NAN,
            current: f64::NAN,
            charge_capacity: f64::NAN,
            discharge_capacity: f64::NAN,
            charge_energy: f64::NAN,
            discharge_energy: f64::NAN,
            temperature: f64::NAN,
        }
    }
}

/// Our name for a Neware step type (the status code of a record).
pub(crate) fn step_type_name(code: u8) -> Option<&'static str> {
    Some(match code {
        1 => "cc_charge",
        2 => "cc_discharge",
        3 => "cv_charge",
        4 => "rest",
        5 => "cycle",
        7 => "cccv_charge",
        8 => "cp_discharge",
        9 => "cp_charge",
        10 => "cr_discharge",
        13 => "pause",
        16 => "pulse",
        17 => "simulation",
        19 => "cv_discharge",
        20 => "cccv_discharge",
        21 => "control",
        22 => "ocv",
        26 => "cpcv_discharge",
        27 => "cpcv_charge",
        _ => return None,
    })
}

/// Scale of the integer current and capacity fields of version-29 `.nda` and version-5 `.ndc`
/// records, by the record's current range (NewareNDA's table; a range not in it is refused).
fn range_scale(range: i32) -> Option<f64> {
    Some(match range {
        -100_000_000 => 10.0,
        -200_000 | -100_000 | -60_000 | -50_000 | -40_000 | -30_000 | -20_000 | -12_000
        | -10_000 | -6000 | -5000 | -3000 | -2000 | -1000 => 1e-2,
        -500 | -100 => 1e-3,
        -50 | -25 | -20 | -10 => 1e-4,
        -5 | -2 | -1 => 1e-5,
        0 => 0.0,
        1 | 2 | 5 => 1e-4,
        10 | 20 | 25 | 50 => 1e-3,
        100 | 200 | 250 | 500 => 1e-2,
        1000 | 6000 | 10_000 | 12_000 | 20_000 | 30_000 | 40_000 | 50_000 | 60_000 | 100_000
        | 200_000 => 1e-1,
        _ => return None,
    })
}

fn corrupt(format: &'static str, at: usize, msg: impl Into<String>) -> Error {
    Error::corrupt_at(format, at as u64, msg.into())
}

fn past_end(b: &[u8], at: usize, n: usize, format: &'static str) -> Error {
    corrupt(
        format,
        at,
        format!("{n} bytes at {at} run past the end ({} bytes)", b.len()),
    )
}

fn get<'a>(b: &'a [u8], at: usize, n: usize, format: &'static str) -> Result<&'a [u8]> {
    at.checked_add(n)
        .and_then(|e| b.get(at..e))
        .ok_or_else(|| past_end(b, at, n, format))
}

/// The value `get` reads at `at`, or a corrupt error when it runs past the end.
fn read<T>(
    b: &[u8],
    at: usize,
    format: &'static str,
    get: fn(&[u8], usize) -> Option<T>,
) -> Result<T> {
    get(b, at).ok_or_else(|| past_end(b, at, size_of::<T>(), format))
}

/// A calendar stamp `u16 year, u8 month, day, hour, minute, second` (local time of the tester).
fn calendar(b: &[u8], at: usize, f: &'static str) -> Result<Option<String>> {
    let y = read(b, at, f, le_u16)?;
    let s = get(b, at + 2, 5, f)?;
    if !(1990..=2200).contains(&y)
        || !(1..=12).contains(&s[0])
        || !(1..=31).contains(&s[1])
        || s[2] > 23
        || s[3] > 59
        || s[4] > 60
    {
        return Ok(None);
    }
    Ok(Some(format!(
        "{y:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        s[0], s[1], s[2], s[3], s[4]
    )))
}

/// Unix seconds (with a fraction) as ISO-8601 UTC.
fn unix_iso(secs: f64) -> Option<String> {
    if !secs.is_finite() || !(0.0..4.0e9).contains(&secs) {
        return None;
    }
    let whole = secs.floor();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let ms = (((secs - whole) * 1000.0).round() as u32).min(999);
    #[allow(clippy::cast_possible_truncation)]
    Some(openreadout_core::time::unix_to_iso8601(whole as i64, ms))
}

/// The integer fields of a version-29 `.nda` or version-5 `.ndc` record, laid out from `o`
/// (offsets relative to the record start `o`): current and capacities scaled by the range.
struct IntLayout {
    index: usize,
    cycle: usize,
    step_index: usize,
    step_index_wide: bool,
    status: usize,
    time: usize,
    voltage: usize,
    current: usize,
    capacities: usize,
    date: usize,
    range: usize,
}

const NDA29: IntLayout = IntLayout {
    index: 2,
    cycle: 6,
    step_index: 10,
    step_index_wide: true,
    status: 12,
    time: 14,
    voltage: 22,
    current: 26,
    capacities: 38,
    date: 70,
    range: 78,
};

const NDC5: IntLayout = IntLayout {
    index: 8,
    cycle: 12,
    step_index: 16,
    step_index_wide: false,
    status: 17,
    time: 23,
    voltage: 31,
    current: 35,
    capacities: 43,
    date: 75,
    range: 82,
};

fn int_record(
    b: &[u8],
    at: usize,
    layout: &IntLayout,
    f: &'static str,
) -> Result<(Rec, Option<String>)> {
    let index = read(b, at + layout.index, f, le_u32)?;
    let mut rec = Rec::empty(index);
    rec.cycle = f64::from(read(b, at + layout.cycle, f, le_u32)?) + 1.0;
    rec.step_index = if layout.step_index_wide {
        f64::from(read(b, at + layout.step_index, f, le_u16)?)
    } else {
        f64::from(get(b, at + layout.step_index, 1, f)?[0])
    };
    rec.status = get(b, at + layout.status, 1, f)?[0];
    #[allow(clippy::cast_precision_loss)]
    {
        rec.step_time = read(b, at + layout.time, f, le_u64)? as f64 / 1000.0;
    }
    rec.voltage = f64::from(read(b, at + layout.voltage, f, le_i32)?) / 10_000.0;
    let range = read(b, at + layout.range, f, le_i32)?;
    let scale = range_scale(range).ok_or_else(|| Error::Unsupported {
        format: f,
        feature: format!("current range {range} (its scale is not known)"),
        hint: Some(
            "read the file with Neware BTSDA or NewareNDA; tell us the range so it can be added"
                .into(),
        ),
    })?;
    rec.current = f64::from(read(b, at + layout.current, f, le_i32)?) * scale;
    let mut caps = [0f64; 4];
    for (k, c) in caps.iter_mut().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        {
            *c = read(b, at + layout.capacities + 8 * k, f, le_i64)? as f64 * scale / 3600.0;
        }
    }
    rec.charge_capacity = caps[0];
    rec.discharge_capacity = caps[1];
    rec.charge_energy = caps[2];
    rec.discharge_energy = caps[3];
    let when = calendar(b, at + layout.date, f)?;
    Ok((rec, when))
}

/// What a parse returns before the file is assembled.
struct Parsed {
    version: String,
    layout: &'static str,
    records: Vec<Rec>,
    /// `true` where the capacities and energies (and step time) are stored for that record.
    stored: Vec<bool>,
    aux: Vec<AuxChannel>,
    started_at: Option<(String, &'static str)>,
    findings: Vec<Finding>,
    observations: Observations,
    vendor: Map<String, Value>,
    facts: Vec<(&'static str, String, &'static str)>,
    parameters: Vec<(&'static str, f64, &'static str, &'static str)>,
    entries: Vec<LsEntry>,
    has_cycle: bool,
    has_temperature: bool,
    /// The stored time is the test time (BTS 9.1), not the step time.
    time_is_total: bool,
    /// (record position, clock seconds) of the first and last record with a clock stamp.
    clock_first: Option<(usize, f64)>,
    clock_last: Option<(usize, f64)>,
}

impl Parsed {
    fn new(version: String, layout: &'static str) -> Self {
        Parsed {
            version,
            layout,
            records: Vec::new(),
            stored: Vec::new(),
            aux: Vec::new(),
            started_at: None,
            findings: Vec::new(),
            observations: Observations::default(),
            vendor: Map::new(),
            facts: Vec::new(),
            parameters: Vec::new(),
            entries: Vec::new(),
            has_cycle: true,
            has_temperature: false,
            time_is_total: false,
            clock_first: None,
            clock_last: None,
        }
    }

    /// Note the clock stamp of the record at `pos`.
    fn clock(&mut self, pos: usize, secs: Option<f64>) {
        if let Some(s) = secs.filter(|s| s.is_finite()) {
            if self.clock_first.is_none() {
                self.clock_first = Some((pos, s));
            }
            self.clock_last = Some((pos, s));
        }
    }
}

/// An auxiliary channel of an `.ndax` (one value per record).
struct AuxChannel {
    name: String,
    unit: Option<&'static str>,
    label: String,
    values: Vec<f64>,
}

/// A null-terminated ASCII string starting at `at`.
fn cstr(b: &[u8], at: usize, max: usize) -> String {
    let end = b.len().min(at.saturating_add(max));
    let s = b.get(at..end).unwrap_or(&[]);
    String::from_utf8_lossy(until_nul(s)).trim().to_string()
}

fn block(name: &str, offset: u64, size: u64) -> LsEntry {
    LsEntry {
        kind: "block".into(),
        name: name.into(),
        offset: Some(offset),
        size: Some(size),
        image: None,
        details: Value::Null,
    }
}

fn member(name: &str, size: u64) -> LsEntry {
    LsEntry {
        kind: "member".into(),
        name: name.into(),
        offset: None,
        size: Some(size),
        image: None,
        details: Value::Null,
    }
}

fn find(b: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= b.len() {
        return None;
    }
    bytes::find(&b[from..], needle).map(|p| p + from)
}

/// Parse an `.nda` file.
pub(crate) fn parse_nda(b: &[u8]) -> Result<SeriesFile> {
    const F: &str = NDA_FORMAT_ID;
    if !b.starts_with(NDA_MAGIC) {
        return Err(corrupt(F, 0, "no NEWARE signature"));
    }
    let version = get(b, 14, 1, F)?[0];
    let stamp = cstr(b, 6, 8);
    let mut p = match version {
        29 => parse_nda29(b)?,
        130 => parse_nda130(b)?,
        v => {
            return Err(Error::Unsupported {
                format: F,
                feature: format!(".nda version {v}"),
                hint: Some(
                    "versions 29 and 130 are read; export the file from BTSDA as .ndax or text"
                        .into(),
                ),
            });
        }
    };
    p.vendor.insert("writer_stamp".into(), json!(stamp));
    // version strings the tester software writes into the header
    if let Some(at) = find(b, b"BTSServer", 0) {
        let server = cstr(b, at, 50);
        let client = cstr(b, at + 100, 50);
        p.vendor.insert("server_version".into(), json!(server));
        p.facts.push((
            "instrument.software_version",
            server,
            "`BTSServer` string in the header",
        ));
        if !client.is_empty() {
            p.vendor.insert("client_version".into(), json!(client));
        }
    } else if let Some(at) = find(&b[..b.len().min(1024)], b"BTS_XWJ", 0) {
        let server = cstr(b, at, 64);
        p.vendor.insert("server_version".into(), json!(server));
    }
    finish(p, F, b.len())
}

/// Version 29 (BTS 7): the data section is named by the header (offset and length at 0x40), a
/// run of 86-byte records: `0x55` measurements, `0x65` auxiliary values, `0xAA` other records.
fn parse_nda29(b: &[u8]) -> Result<Parsed> {
    const F: &str = NDA_FORMAT_ID;
    const REC: usize = 86;
    let mut p = Parsed::new("nda 29".into(), "records of 86 bytes");
    let start = read(b, 0x40, F, le_u32)? as usize;
    let len = read(b, 0x44, F, le_u32)? as usize;
    let end = start
        .checked_add(len)
        .filter(|&e| e <= b.len())
        .ok_or_else(|| {
            corrupt(
                F,
                0x40,
                format!(
                    "the data section {start}+{len} runs past the end ({} bytes)",
                    b.len()
                ),
            )
        })?;
    if !len.is_multiple_of(REC) {
        return Err(corrupt(
            F,
            0x44,
            format!("the data section length {len} is not a whole number of {REC}-byte records"),
        ));
    }
    if len / REC > MAX_RECORDS {
        return Err(corrupt(F, 0x44, "too many records"));
    }
    // active mass (mg) as the tester stored it
    let mass = read(b, 152, F, le_u32)?;
    p.parameters.push((
        "active_mass",
        f64::from(mass) / 1000.0,
        "mg",
        "header offset 152 (thousandths of a mg)",
    ));
    let mut other = 0usize;
    let mut aux = 0usize;
    let mut zero = 0usize;
    let mut o = start;
    while o + REC <= end {
        let kind = b[o];
        let tail_zero = get(b, o + 82, 4, F)? == [0, 0, 0, 0];
        if kind == 0x55 && b[o + 1] == 0 && tail_zero {
            let (r, when) = int_record(b, o, &NDA29, F)?;
            if r.index == 0 || r.status == 0 {
                zero += 1;
            } else {
                let secs = when
                    .as_deref()
                    .and_then(openreadout_core::time::iso8601_to_unix);
                p.clock(p.records.len(), secs);
                if p.started_at.is_none()
                    && let Some(w) = when
                {
                    p.started_at = Some((w, "the first record's date (tester local time)"));
                }
                p.records.push(r);
                p.stored.push(true);
            }
        } else if kind == 0x65 {
            aux += 1;
        } else {
            other += 1;
        }
        o += REC;
    }
    if aux > 0 {
        p.observations.undecoded(
            "auxiliary records (0x65)",
            &[],
            format!("{aux} auxiliary records (temperatures, voltages) are not decoded"),
        );
        p.findings.push(Finding::warning(
            "aux_not_decoded",
            format!("{aux} auxiliary records (0x65) are left out: their layout is not validated"),
        ));
    }
    if other > 0 {
        p.vendor.insert("other_records".into(), json!(other));
    }
    if zero > 0 {
        p.findings.push(Finding::info(
            "empty_records",
            format!("{zero} records with index or status 0 are left out"),
        ));
    }
    p.entries
        .push(block("data section", start as u64, len as u64));
    Ok(p)
}

/// Version 130 (BTS 8/9): records from offset 1024. Two layouts: BTS 9.0 (88-byte records whose
/// first six bytes repeat) and BTS 9.1 (`0x55` records with float values; a record length taken
/// from the distance between the first two), ended by a `0x81` footer.
fn parse_nda130(b: &[u8]) -> Result<Parsed> {
    const F: &str = NDA_FORMAT_ID;
    const REC: usize = 88;
    let first = get(b, 1024, 6, F)?;
    if first[0] == 0x55 {
        return parse_nda130_bts91(b);
    }
    let mut p = Parsed::new("nda 130".into(), "BTS 9.0 records of 88 bytes");
    p.has_cycle = false;
    let ident: Vec<u8> = first.to_vec();
    if ident[4] != 0x55 {
        return Err(Error::Unsupported {
            format: F,
            feature: format!("a version-130 record layout starting {ident:02x?}"),
            hint: Some("read the file with Neware BTSDA or NewareNDA".into()),
        });
    }
    let mut o = 1024usize;
    let mut aux = 0usize;
    let mut other = 0usize;
    let mut ended = false;
    while o + REC <= b.len() {
        let r = &b[o..o + REC];
        if r[..6] == ident[..] {
            let base = o + 4;
            let mut rec = Rec::empty(read(b, base + 12, F, le_u32)?);
            rec.step_index = f64::from(b[base + 5]);
            rec.status = b[base + 6];
            #[allow(clippy::cast_precision_loss)]
            {
                rec.step_time = read(b, base + 24, F, le_u64)? as f64 / 1e6;
            }
            rec.voltage = f64::from(read(b, base + 32, F, le_f32)?);
            rec.current = f64::from(read(b, base + 36, F, le_f32)?);
            rec.charge_capacity = f64::from(read(b, base + 48, F, le_f32)?) / 3600.0;
            rec.charge_energy = f64::from(read(b, base + 52, F, le_f32)?) / 3600.0;
            rec.discharge_capacity = f64::from(read(b, base + 56, F, le_f32)?) / 3600.0;
            rec.discharge_energy = f64::from(read(b, base + 60, F, le_f32)?) / 3600.0;
            #[allow(clippy::cast_precision_loss)]
            let us = read(b, base + 64, F, le_u64)? as f64;
            if p.started_at.is_none()
                && let Some(s) = unix_iso(us / 1e6)
            {
                p.started_at = Some((s, "the first record's timestamp (µs since 1970, UTC)"));
            }
            p.clock(p.records.len(), Some(us / 1e6));
            p.records.push(rec);
            p.stored.push(true);
        } else if r[..5] == [0, 0, 0, 0, 0x65] {
            aux += 1;
        } else if r[0] == 0x81 {
            ended = true;
            break;
        } else {
            other += 1;
        }
        o += REC;
    }
    if !ended {
        p.findings.push(Finding::warning(
            "no_footer",
            "the records do not end in a footer: the file may be truncated",
        ));
    }
    if aux > 0 {
        p.observations.undecoded(
            "auxiliary records (0x65)",
            &[],
            format!("{aux} auxiliary records are not decoded"),
        );
        p.findings.push(Finding::warning(
            "aux_not_decoded",
            format!("{aux} auxiliary records are left out: their layout is not validated"),
        ));
    }
    if other > 0 {
        p.vendor.insert("other_records".into(), json!(other));
    }
    p.entries.push(block("records", 1024, (o - 1024) as u64));
    Ok(p)
}

fn parse_nda130_bts91(b: &[u8]) -> Result<Parsed> {
    const F: &str = NDA_FORMAT_ID;
    let head = [b[1024], b[1025]];
    let next = find(b, &head, 1026)
        .ok_or_else(|| corrupt(F, 1024, "a single record: its length is unknown"))?;
    let rec_len = next - 1024;
    if rec_len != 52 && rec_len != 56 {
        return Err(Error::Unsupported {
            format: F,
            feature: format!("BTS 9.1 records of {rec_len} bytes"),
            hint: Some(
                "records of 52 and 56 bytes are read; read the file with Neware BTSDA or NewareNDA"
                    .into(),
            ),
        });
    }
    let mut p = Parsed::new(
        "nda 130".into(),
        if rec_len == 56 {
            "BTS 9.1 records of 56 bytes"
        } else {
            "BTS 9.1 records of 52 bytes"
        },
    );
    p.has_temperature = rec_len == 56;
    p.time_is_total = true;
    let mut o = 1024usize;
    let mut ended = false;
    while o + rec_len <= b.len() {
        if b[o] != 0x55 {
            ended = b[o] == 0x81;
            break;
        }
        if b[o + 1] != head[1] {
            return Err(corrupt(
                F,
                o,
                format!(
                    "record {} changes kind ({:02x} after {:02x})",
                    p.records.len(),
                    b[o + 1],
                    head[1]
                ),
            ));
        }
        let mut rec = Rec::empty(read(b, o + 8, F, le_u32)?);
        rec.step_index = f64::from(b[o + 2]);
        rec.status = b[o + 3];
        rec.step_time =
            f64::from(read(b, o + 12, F, le_u32)?) + f64::from(read(b, o + 16, F, le_u32)?) * 1e-9;
        rec.current = f64::from(read(b, o + 20, F, le_f32)?);
        rec.voltage = f64::from(read(b, o + 24, F, le_f32)?);
        let cap = f64::from(read(b, o + 28, F, le_f32)?) / 3600.0;
        let energy = f64::from(read(b, o + 32, F, le_f32)?) / 3600.0;
        // one signed accumulator each: positive while charging, negative while discharging
        rec.charge_capacity = cap.max(0.0);
        rec.discharge_capacity = (-cap).max(0.0);
        rec.charge_energy = energy.max(0.0);
        rec.discharge_energy = (-energy).max(0.0);
        rec.cycle = f64::from(read(b, o + 36, F, le_u32)?) + 1.0;
        if rec_len == 56 {
            rec.temperature = f64::from(read(b, o + 52, F, le_f32)?);
        }
        let s =
            f64::from(read(b, o + 44, F, le_u32)?) + f64::from(read(b, o + 48, F, le_u32)?) * 1e-9;
        if p.started_at.is_none()
            && let Some(t) = unix_iso(s)
        {
            p.started_at = Some((t, "the first record's timestamp (seconds since 1970, UTC)"));
        }
        p.clock(p.records.len(), Some(s));
        p.records.push(rec);
        p.stored.push(true);
        o += rec_len;
    }
    if !ended {
        p.findings.push(Finding::warning(
            "no_footer",
            "the records do not end in a footer: the file may be truncated",
        ));
    }
    if let Some(at) = find(b, b"9.1.", 0) {
        let v = cstr(b, at, 32);
        p.vendor.insert("bts_version".into(), json!(v));
    }
    p.entries.push(block("records", 1024, (o - 1024) as u64));
    Ok(p)
}

/// One `.ndc` file: its version, file type and the records of its pages (each `rec_len` bytes).
struct Ndc {
    version: u8,
    kind: u8,
    records: Vec<std::ops::Range<usize>>,
}

/// Split an `.ndc` file into records: a 4096-byte header, then 4096-byte pages (`u16` page kind =
/// file type + 1, `u16` record count, a validity bitmap, records from `first`, and a CRC-32 of the
/// page in its last four bytes).
fn ndc_records(b: &[u8], name: &str, rec_len: usize, first: usize) -> Result<Ndc> {
    const F: &str = NDAX_FORMAT_ID;
    if b.len() < 4096 || !b.len().is_multiple_of(4096) {
        return Err(corrupt(
            F,
            0,
            format!(
                "{name}: {} bytes is not a whole number of 4096-byte pages",
                b.len()
            ),
        ));
    }
    let kind = b[0];
    let version = b[2];
    let mut records = Vec::new();
    let per_page = (4096 - first - 4) / rec_len;
    for (p, page) in b[4096..].as_chunks::<4096>().0.iter().enumerate() {
        let at = 4096 * (p + 1);
        let stored = u32::from_le_bytes([page[4092], page[4093], page[4094], page[4095]]);
        if crc32fast::hash(&page[..4092]) != stored {
            return Err(corrupt(F, at, format!("{name}: page {p} fails its CRC-32")));
        }
        let pk = u16::from_le_bytes([page[0], page[1]]);
        if pk != u16::from(kind) + 1 {
            return Err(corrupt(
                F,
                at,
                format!("{name}: page {p} is of kind {pk}, the file of type {kind}"),
            ));
        }
        let count = usize::from(u16::from_le_bytes([page[2], page[3]]));
        if count > per_page {
            return Err(corrupt(
                F,
                at,
                format!("{name}: page {p} claims {count} records, room for {per_page}"),
            ));
        }
        let bits = count.div_ceil(8);
        let valid = page[4..4 + bits]
            .iter()
            .map(|x| x.count_ones() as usize)
            .sum::<usize>();
        if valid != count {
            return Err(Error::Unsupported {
                format: F,
                feature: format!(
                    "{name}: a page with {count} records of which {valid} are marked valid"
                ),
                hint: Some("read the file with Neware BTSDA or NewareNDA".into()),
            });
        }
        for k in 0..count {
            let s = at + first + k * rec_len;
            records.push(s..s + rec_len);
        }
    }
    if records.len() > MAX_RECORDS {
        return Err(corrupt(F, 0, "too many records"));
    }
    Ok(Ndc {
        version,
        kind,
        records,
    })
}

/// Attributes of the first element named `tag` in an XML text.
fn xml_attrs(text: &str, tag: &str) -> Vec<(String, String)> {
    let Ok(doc) = roxmltree::Document::parse(text) else {
        return Vec::new();
    };
    doc.descendants()
        .find(|n| n.has_tag_name(tag))
        .map(|n| {
            n.attributes()
                .map(|a| (a.name().to_string(), a.value().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// All elements whose tag starts with `prefix` directly under the first `parent`.
fn xml_children(text: &str, parent: &str, prefix: &str) -> Vec<Vec<(String, String)>> {
    let Ok(doc) = roxmltree::Document::parse(text) else {
        return Vec::new();
    };
    doc.descendants()
        .find(|n| n.has_tag_name(parent))
        .map(|p| {
            p.children()
                .filter(|c| c.is_element() && c.tag_name().name().starts_with(prefix))
                .map(|c| {
                    c.attributes()
                        .map(|a| (a.name().to_string(), a.value().to_string()))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// XML members are GB2312 (Chinese Windows); ASCII content reads the same in any decoder, so the
/// text is taken as UTF-8 with the declaration's encoding rewritten (non-ASCII bytes replaced).
fn xml_text(b: &[u8]) -> String {
    let t = String::from_utf8_lossy(b).to_string();
    t.replacen("encoding=\"GB2312\"", "encoding=\"UTF-8\"", 1)
        .replacen("encoding=\"gb2312\"", "encoding=\"UTF-8\"", 1)
}

fn attr<'a>(a: &'a [(String, String)], k: &str) -> Option<&'a str> {
    a.iter()
        .find(|(n, _)| n == k)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.trim().is_empty())
}

/// Parse an `.ndax` zip.
pub(crate) fn parse_ndax(zip: &ZipIndex) -> Result<SeriesFile> {
    const F: &str = NDAX_FORMAT_ID;
    let data = zip
        .read_named("data.ndc")?
        .ok_or_else(|| Error::Unsupported {
            format: F,
            feature: "an .ndax without data.ndc".into(),
            hint: Some("read the file with Neware BTSDA".into()),
        })?;
    if data.len() < 4096 {
        return Err(corrupt(F, 0, "data.ndc is shorter than its header"));
    }
    let version = data[2];
    let mut p = match version {
        5 => ndax_v5(&data)?,
        11 | 14 | 16 | 17 => ndax_split(zip, &data, version)?,
        v => {
            return Err(Error::Unsupported {
                format: F,
                feature: format!(".ndc version {v}"),
                hint: Some(".ndc versions 5, 11, 14, 16 and 17 are read; read the file with Neware BTSDA or NewareNDA".into()),
            });
        }
    };
    for m in &zip.members {
        p.entries.push(member(&m.name, m.size));
    }
    // descriptions
    if let Some(t) = zip.read_named("VersionInfo.xml")? {
        let a = xml_attrs(&xml_text(&t), "ZwjVersion");
        for (k, ours) in [
            ("SvrVer", "server_version"),
            ("CurrClientVer", "client_version"),
            ("ZwjVersion", "unit_firmware"),
            ("MainXwjVer", "tester_firmware"),
        ] {
            if let Some(v) = attr(&a, k) {
                p.vendor.insert(ours.into(), json!(v));
            }
        }
        if let Some(v) = attr(&a, "SvrVer") {
            p.facts.push((
                "instrument.software_version",
                v.to_string(),
                "VersionInfo.xml `SvrVer`",
            ));
        }
    }
    if let Some(t) = zip.read_named("TestInfo.xml")? {
        let text = xml_text(&t);
        let a = xml_attrs(&text, "TestInfo");
        let mut ti = Map::new();
        for (k, v) in &a {
            ti.insert(k.clone(), json!(v));
        }
        if let (Some(dev), Some(unit), Some(ch)) =
            (attr(&a, "DevID"), attr(&a, "UnitID"), attr(&a, "ChlID"))
        {
            p.vendor
                .insert("channel".into(), json!(format!("{dev}-{unit}-{ch}")));
        }
        if let Some(v) = attr(&a, "StepName") {
            p.facts.push((
                "method.name",
                v.trim_end_matches(".xml").to_string(),
                "TestInfo.xml `StepName`",
            ));
        }
        if let Some(v) = attr(&a, "Barcode") {
            p.facts
                .push(("sample.barcode", v.to_string(), "TestInfo.xml `Barcode`"));
        }
        if let Some(v) = attr(&a, "EndTime") {
            p.facts.push((
                "acquisition.ended_at",
                v.replace(' ', "T"),
                "TestInfo.xml `EndTime` (tester local time)",
            ));
        }
        if let Some(v) = attr(&a, "StartTime") {
            p.vendor.insert("start_time_local".into(), json!(v));
        }
        p.vendor.insert("test_info".into(), Value::Object(ti));
    }
    finish(p, F, zip.file_len as usize)
}

/// Version 5: whole 87-byte records in the pages of `data.ndc` (from page offset 125).
fn ndax_v5(data: &[u8]) -> Result<Parsed> {
    const F: &str = NDAX_FORMAT_ID;
    let ndc = ndc_records(data, "data.ndc", 87, 125)?;
    if ndc.kind != 1 {
        return Err(corrupt(
            F,
            0,
            format!("data.ndc is of file type {}", ndc.kind),
        ));
    }
    let mut p = Parsed::new("ndc 5".into(), "whole records of 87 bytes");
    for r in &ndc.records {
        if data[r.start + 7] != 0x55 {
            return Err(corrupt(
                F,
                r.start,
                format!("a record of kind {:#04x} in data.ndc", data[r.start + 7]),
            ));
        }
        let (rec, when) = int_record(data, r.start, &NDC5, F)?;
        let secs = when
            .as_deref()
            .and_then(openreadout_core::time::iso8601_to_unix);
        p.clock(p.records.len(), secs);
        if p.started_at.is_none()
            && let Some(w) = when
        {
            p.started_at = Some((w, "the first record's date (tester local time)"));
        }
        p.records.push(rec);
        p.stored.push(true);
    }
    Ok(p)
}

/// One `data_runInfo.ndc` record.
#[derive(Clone, Copy)]
struct RunInfo {
    index: u32,
    step: u32,
    step_time: f64,
    interval: f64,
    caps: [f64; 4],
    clock: f64,
}

/// Versions 11-17: voltage and current in `data.ndc`, the step of each record and the capacities
/// in `data_runInfo.ndc` (logged records only), the step table in `data_step.ndc`, and one file per
/// auxiliary channel.
fn ndax_split(zip: &ZipIndex, data: &[u8], version: u8) -> Result<Parsed> {
    const F: &str = NDAX_FORMAT_ID;
    let ndc = ndc_records(data, "data.ndc", 8, 132)?;
    if ndc.kind != 1 {
        return Err(corrupt(
            F,
            0,
            format!("data.ndc is of file type {}", ndc.kind),
        ));
    }
    let mut parsed = Parsed::new(
        format!("ndc {version}"),
        "voltage and current records with separate step and run files",
    );
    // voltage in 0.1 mV and current in mA (11, 16), or V and A (14, 17)
    let (v_scale, i_scale, cap_scale) = if matches!(version, 11 | 16) {
        (1e-4, 1.0, 1.0 / 3600.0)
    } else {
        (1.0, 1000.0, 1000.0)
    };
    let mut volts = Vec::with_capacity(ndc.records.len());
    for r in &ndc.records {
        let v = f64::from(read(data, r.start, F, le_f32)?) * v_scale;
        let i = f64::from(read(data, r.start + 4, F, le_f32)?) * i_scale;
        volts.push((v, i));
    }
    // steps: (cycle, step index, status) by step number (1-based)
    let step_bytes = zip
        .read_named("data_step.ndc")?
        .ok_or_else(|| corrupt(F, 0, "no data_step.ndc"))?;
    let step_len = if matches!(version, 16 | 17) { 100 } else { 37 };
    let st = ndc_records(&step_bytes, "data_step.ndc", step_len, 132)?;
    if st.version != version || st.kind != 7 {
        return Err(corrupt(
            F,
            0,
            format!("data_step.ndc is version {} type {}", st.version, st.kind),
        ));
    }
    let mut steps = Vec::with_capacity(st.records.len());
    for r in &st.records {
        let cycle = read(&step_bytes, r.start, F, le_i32)?;
        let step_index = read(&step_bytes, r.start + 4, F, le_i32)?;
        let status = step_bytes[r.start + 24];
        steps.push((f64::from(cycle) + 1.0, f64::from(step_index), status));
    }
    // run information
    let run_bytes = zip
        .read_named("data_runInfo.ndc")?
        .ok_or_else(|| corrupt(F, 0, "no data_runInfo.ndc"))?;
    let run_len = match version {
        11 => 47,
        14 => 55,
        _ => 100,
    };
    let ri = ndc_records(&run_bytes, "data_runInfo.ndc", run_len, 132)?;
    if ri.version != version || ri.kind != 18 {
        return Err(corrupt(
            F,
            0,
            format!(
                "data_runInfo.ndc is version {} type {}",
                ri.version, ri.kind
            ),
        ));
    }
    let mut runs: Vec<RunInfo> = Vec::with_capacity(ri.records.len());
    for r in &ri.records {
        let o = r.start;
        let index = read(&run_bytes, o + 41, F, le_i32)?;
        if index <= 0 {
            continue;
        }
        let mut caps = [0f64; 4];
        for (k, c) in caps.iter_mut().enumerate() {
            *c = f64::from(read(&run_bytes, o + 5 + 4 * k, F, le_f32)?) * cap_scale;
        }
        let run = RunInfo {
            index: index.unsigned_abs(),
            step: read(&run_bytes, o + 37, F, le_i32)?.unsigned_abs(),
            step_time: f64::from(read(&run_bytes, o, F, le_i32)?) / 1000.0,
            interval: f64::from(read(&run_bytes, o + 29, F, le_i32)?) / 1000.0,
            caps,
            clock: f64::from(read(&run_bytes, o + 33, F, le_i32)?)
                + f64::from(read(&run_bytes, o + 45, F, le_i16)?) / 1000.0,
        };
        // the first record of an index is kept (a repeat restates the start of a step)
        if runs.last().is_some_and(|l| l.index >= run.index) {
            if runs.last().is_some_and(|l| l.index > run.index) {
                return Err(corrupt(
                    F,
                    o,
                    format!(
                        "data_runInfo.ndc goes back from record {} to {}",
                        runs.last().map_or(0, |l| l.index),
                        run.index
                    ),
                ));
            }
            continue;
        }
        runs.push(run);
    }
    if let Some(first) = runs.first()
        && let Some(s) = unix_iso(first.clock - first.step_time)
    {
        parsed.started_at = Some((
            s,
            "the first logged record's timestamp (seconds since 1970, UTC)",
        ));
    }
    // assemble the records
    let n = volts.len();
    let mut k = 0usize; // runs[k] is the last logged record at or before the current one
    let mut contradicted = 0usize;
    let mut before_first = 0usize;
    for (i, (v, cur)) in volts.iter().enumerate() {
        let index = u32::try_from(i + 1).map_err(|_| corrupt(F, 0, "too many records"))?;
        while k + 1 < runs.len() && runs[k + 1].index <= index {
            k += 1;
        }
        let mut r = Rec::empty(index);
        r.voltage = *v;
        r.current = *cur;
        let mut stored = false;
        if let Some(run) = runs.get(k).filter(|run| run.index <= index) {
            let (cycle, step_index, status) = run
                .step
                .checked_sub(1)
                .and_then(|s| steps.get(s as usize))
                .copied()
                .ok_or_else(|| {
                    corrupt(
                        F,
                        0,
                        format!("record {index} names step {} of {}", run.step, steps.len()),
                    )
                })?;
            r.cycle = cycle;
            r.step_index = step_index;
            r.status = status;
            if run.index == index {
                stored = true;
                parsed.clock(i, Some(run.clock));
                r.step_time = run.step_time;
                r.charge_capacity = run.caps[0];
                r.discharge_capacity = run.caps[1];
                r.charge_energy = run.caps[2];
                r.discharge_energy = run.caps[3];
                // a logged record contradicting the logging rule (a pause, lost records)
                if let Some(prev) = k.checked_sub(1).and_then(|j| runs.get(j))
                    && prev.step == run.step
                {
                    #[allow(clippy::cast_precision_loss)]
                    let expect = prev.step_time
                        + f64::from(run.index - prev.index - 1) * prev.interval
                        + run.interval;
                    if (expect - run.step_time).abs() > 1e-6 {
                        contradicted += 1;
                    }
                }
            } else {
                #[allow(clippy::cast_precision_loss)]
                {
                    r.step_time = run.step_time + f64::from(index - run.index) * run.interval;
                }
            }
        } else {
            before_first += 1;
        }
        parsed.records.push(r);
        parsed.stored.push(stored);
    }
    if n > runs.len() {
        parsed.observations.assumed(
            "traces.step_time",
            "the step time of records between two logged records is the earlier one's plus its logging interval per record (the tester logs a record whenever the interval changes)",
        );
        parsed.findings.push(Finding::info(
            "logged_records",
            format!(
                "{} of {n} records carry their step time, capacities and energies; the step time of the others follows from the logging interval, their capacities and energies are NaN",
                runs.len()
            ),
        ));
    }
    if contradicted > 0 {
        parsed.findings.push(Finding::warning(
            "step_time_gap",
            format!("{contradicted} logged records disagree with the logging interval before them (a pause or lost records): the reconstructed step times before them may be off"),
        ));
    }
    if before_first > 0 {
        parsed.findings.push(Finding::warning("unlogged_start", format!("{before_first} records come before the first logged record: their step, cycle and step time are unknown")));
    }
    // auxiliary channels
    let test_info = zip
        .read_named("TestInfo.xml")?
        .map(|t| xml_text(&t))
        .unwrap_or_default();
    let declared = xml_children(&test_info, "TestInfo", "Aux");
    let mut aux_names: Vec<String> = zip
        .members
        .iter()
        .map(|m| m.name.clone())
        .filter(|n| {
            n.starts_with("data_AUX_")
                && std::path::Path::new(n)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("ndc"))
        })
        .collect();
    aux_names.sort();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for name in aux_names {
        let parts: Vec<&str> = name
            .trim_end_matches(".ndc")
            .trim_start_matches("data_AUX_")
            .split('_')
            .collect();
        let (Some(chl), Some(ty)) = (
            parts.first().and_then(|v| v.parse::<u32>().ok()),
            parts.get(1).and_then(|v| v.parse::<u32>().ok()),
        ) else {
            parsed.findings.push(Finding::warning(
                "aux_unnamed",
                format!("{name}: the file name does not name a channel and type; left out"),
            ));
            continue;
        };
        let chl_type = ty + 100;
        let known = declared.iter().any(|a| {
            attr(a, "RealChlID") == Some(chl.to_string().as_str())
                && attr(a, "ChlType") == Some(chl_type.to_string().as_str())
        });
        let bytes = zip
            .read_named(&name)?
            .ok_or_else(|| corrupt(F, 0, format!("{name} vanished")))?;
        if bytes.len() < 4096 || bytes[2] != version || !matches!(version, 14 | 17) {
            parsed.observations.undecoded(
                format!("auxiliary file {name}"),
                &[],
                "an auxiliary file layout not validated",
            );
            parsed.findings.push(Finding::warning(
                "aux_not_decoded",
                format!(
                    "{name}: auxiliary files of .ndc version {} are not decoded",
                    bytes.get(2).copied().unwrap_or(0)
                ),
            ));
            continue;
        }
        let a = ndc_records(&bytes, &name, 4, 132)?;
        let mut values = Vec::with_capacity(a.records.len());
        for r in &a.records {
            values.push(f64::from(read(&bytes, r.start, F, le_f32)?));
        }
        if values.len() != n {
            parsed.findings.push(Finding::warning(
                "aux_count",
                format!("{name} holds {} values for {n} records", values.len()),
            ));
        }
        values.resize(n, f64::NAN);
        let (base, unit) = match chl_type {
            102 => ("aux_voltage", Some("V")),
            103 => ("aux_temperature", Some("°C")),
            _ => ("aux", None),
        };
        let c = counts.entry(base.to_string()).or_insert(0);
        *c += 1;
        let name_out = if base == "aux" {
            format!("aux_{chl_type}_{c}")
        } else {
            format!("{base}_{c}")
        };
        if unit.is_none() {
            parsed.findings.push(Finding::info("aux_type_unknown", format!("{name}: auxiliary channel type {chl_type} has no known quantity; its values are returned without a unit")));
        }
        if !known && !declared.is_empty() {
            parsed.findings.push(Finding::info(
                "aux_not_declared",
                format!("{name} is not listed in TestInfo.xml"),
            ));
        }
        parsed.aux.push(AuxChannel {
            name: name_out,
            unit,
            label: format!("{name} (channel {chl}, type {chl_type})"),
            values,
        });
    }
    Ok(parsed)
}

/// Assemble the file: one trace (every record), one table (steps).
fn finish(mut p: Parsed, format: &'static str, size: usize) -> Result<SeriesFile> {
    if p.records.is_empty() {
        return Err(Error::corrupt(format, "no measurement records"));
    }
    let n = p.records.len();
    // total test time: stored (BTS 9.1), or step time accumulated (a new step starts on the
    // instant the last ended)
    let mut total = Vec::with_capacity(n);
    let mut t = 0.0f64;
    let mut prev: Option<&Rec> = None;
    let mut out_of_order = 0usize;
    for r in &p.records {
        if let Some(q) = prev {
            if r.index <= q.index {
                out_of_order += 1;
            }
            let same = q.step_index.to_bits() == r.step_index.to_bits()
                && q.cycle.to_bits() == r.cycle.to_bits()
                && r.step_time >= q.step_time;
            if p.time_is_total {
                t = r.step_time;
            } else if same {
                t += r.step_time - q.step_time;
            } else if r.step_time.is_finite() {
                t += r.step_time;
            }
        } else if r.step_time.is_finite() {
            t = r.step_time;
        }
        total.push(t);
        prev = Some(r);
    }
    // the time axis against the tester's clock
    if let (Some((a, ca)), Some((b, cb))) = (p.clock_first, p.clock_last)
        && b > a
    {
        let by_clock = cb - ca;
        let by_time = total[b] - total[a];
        if (by_clock - by_time).abs() > (0.01 * by_clock.abs()).max(2.0) {
            p.findings.push(Finding::warning(
                "time_vs_clock",
                format!("the tester's clock advances {by_clock:.1} s between records {} and {}, the time axis {by_time:.1} s (pauses, clock changes and gaps between steps are not on the time axis)", a + 1, b + 1),
            ));
        }
    }
    if out_of_order > 0 {
        p.findings.push(Finding::warning("index_order", format!("{out_of_order} records do not follow their predecessor's index (repeated or out of order); returned in file order")));
    }
    let mut unknown_status: BTreeMap<u8, usize> = BTreeMap::new();
    for r in &p.records {
        if step_type_name(r.status).is_none() {
            *unknown_status.entry(r.status).or_insert(0) += 1;
        }
    }
    if !unknown_status.is_empty() {
        p.findings.push(Finding::warning(
            "step_type_unknown",
            format!("step type codes without a known name (code: records): {unknown_status:?}"),
        ));
    }
    let col = |f: fn(&Rec) -> f64| -> Vec<f64> { p.records.iter().map(f).collect() };
    let mut channels = vec![
        (
            SeriesChannel::new("time", Some("s"), "float64"),
            total.clone(),
            if p.time_is_total {
                "test time as stored"
            } else {
                "total test time (step times accumulated)"
            },
        ),
        (
            SeriesChannel::new("step_time", Some("s"), "float64"),
            col(|r| r.step_time),
            "time since the step started",
        ),
        (
            SeriesChannel::new("voltage", Some("V"), "float64"),
            col(|r| r.voltage),
            "cell voltage",
        ),
        (
            SeriesChannel::new("current", Some("mA"), "float64"),
            col(|r| r.current),
            "current (negative while discharging)",
        ),
        (
            SeriesChannel::new("charge_capacity", Some("mA·h"), "float64"),
            col(|r| r.charge_capacity),
            "charge capacity of the step",
        ),
        (
            SeriesChannel::new("discharge_capacity", Some("mA·h"), "float64"),
            col(|r| r.discharge_capacity),
            "discharge capacity of the step",
        ),
        (
            SeriesChannel::new("charge_energy", Some("mW·h"), "float64"),
            col(|r| r.charge_energy),
            "charge energy of the step",
        ),
        (
            SeriesChannel::new("discharge_energy", Some("mW·h"), "float64"),
            col(|r| r.discharge_energy),
            "discharge energy of the step",
        ),
    ];
    if p.has_cycle {
        channels.push((
            SeriesChannel::new("cycle", None, "float64"),
            col(|r| r.cycle),
            "cycle number (1-based, as stored)",
        ));
    }
    channels.push((
        SeriesChannel::new("step_index", None, "float64"),
        col(|r| r.step_index),
        "the program step the record belongs to",
    ));
    channels.push((
        SeriesChannel::new("step_type", None, "float64"),
        col(|r| f64::from(r.status)),
        "step type code (names in extra.step_types)",
    ));
    #[allow(clippy::cast_lossless)]
    channels.push((
        SeriesChannel::new("record", None, "float64"),
        col(|r| f64::from(r.index)),
        "the record's index in the file",
    ));
    if p.has_temperature {
        channels.push((
            SeriesChannel::new("temperature", Some("°C"), "float64"),
            col(|r| r.temperature),
            "temperature stored with the record",
        ));
    }
    for a in std::mem::take(&mut p.aux) {
        let mut ch = SeriesChannel::new(a.name, a.unit, "float64");
        ch.extra.insert("label".into(), json!(a.label));
        channels.push((ch, a.values, "auxiliary channel"));
    }
    if p.time_is_total {
        // the file stores the test time, not the step time
        channels.retain(|(c, _, _)| c.name != "step_time");
    }
    let mut names = Vec::new();
    let mut values = Vec::new();
    for (mut ch, v, what) in channels {
        ch.extra
            .entry("description".into())
            .or_insert_with(|| json!(what));
        names.push(ch);
        values.push(v);
    }
    let mut types = Map::new();
    let mut seen: Vec<u8> = p.records.iter().map(|r| r.status).collect();
    seen.sort_unstable();
    seen.dedup();
    for c in &seen {
        types.insert(
            c.to_string(),
            json!(step_type_name(*c).unwrap_or("unknown")),
        );
    }
    let mut extra = BTreeMap::new();
    extra.insert(
        "axis".into(),
        json!({"quantity": "time", "unit": "s", "irregular": true, "channel": 0, "size": n}),
    );
    extra.insert("kind".into(), json!("battery_cycling"));
    extra.insert("step_types".into(), Value::Object(types));
    let logged = p.stored.iter().filter(|s| **s).count();
    extra.insert("logged_records".into(), json!(logged));
    let trace = SeriesTrace {
        name: "records".into(),
        channels: names,
        sweeps: vec![values],
        sample_rate_hz: 0.0,
        start_s: None,
        extra,
    };
    // the step table: one row per run of records with the same cycle and program step
    let mut s_start = Vec::new();
    let mut s_end = Vec::new();
    let mut s_cycle = Vec::new();
    let mut s_index = Vec::new();
    let mut s_type = Vec::new();
    let mut s_dur: Vec<f64> = Vec::new();
    let mut s_cc = Vec::new();
    let mut s_dc = Vec::new();
    // a step lasts from the previous step's last record (or the first record's own step time
    // before it) to its own last record
    let mut step_origin = 0.0f64;
    for (i, r) in p.records.iter().enumerate() {
        let new = i == 0 || {
            let q = &p.records[i - 1];
            q.step_index.to_bits() != r.step_index.to_bits()
                || q.cycle.to_bits() != r.cycle.to_bits()
                || q.status != r.status
                || (!p.time_is_total && r.step_time < q.step_time)
        };
        if new {
            s_start.push(f64::from(r.index));
            s_end.push(f64::from(r.index));
            s_cycle.push(r.cycle);
            s_index.push(r.step_index);
            s_type.push(step_type_name(r.status).unwrap_or("unknown").to_string());
            step_origin = if i == 0 {
                total[0]
                    - if p.time_is_total || !r.step_time.is_finite() {
                        0.0
                    } else {
                        r.step_time
                    }
            } else {
                total[i - 1]
            };
            s_dur.push(total[i] - step_origin);
            s_cc.push(r.charge_capacity);
            s_dc.push(r.discharge_capacity);
        } else if let Some(last) = s_end.last_mut() {
            *last = f64::from(r.index);
            if let Some(d) = s_dur.last_mut() {
                *d = total[i] - step_origin;
            }
            if r.charge_capacity.is_finite()
                && let Some(c) = s_cc.last_mut()
            {
                *c = r.charge_capacity;
            }
            if r.discharge_capacity.is_finite()
                && let Some(c) = s_dc.last_mut()
            {
                *c = r.discharge_capacity;
            }
        }
    }
    let steps_n = s_start.len();
    let mut cols = vec![
        SeriesColumn::numbers("step", None, (1..=steps_n).map(|k| k as f64).collect()),
        SeriesColumn::numbers("first_record", None, s_start),
        SeriesColumn::numbers("last_record", None, s_end),
    ];
    if p.has_cycle {
        cols.push(SeriesColumn::numbers("cycle", None, s_cycle));
    }
    cols.push(SeriesColumn::numbers("step_index", None, s_index));
    cols.push(SeriesColumn::texts("step_type", &s_type));
    cols.push(SeriesColumn::numbers("duration", Some("s"), s_dur));
    cols.push(SeriesColumn::numbers("charge_capacity", Some("mA·h"), s_cc));
    cols.push(SeriesColumn::numbers(
        "discharge_capacity",
        Some("mA·h"),
        s_dc,
    ));
    let mut t_extra = BTreeMap::new();
    t_extra.insert("description".into(), json!("steps as executed: records, duration on the time axis, and the last logged capacities of each (NaN when none was logged)"));
    let table = SeriesTable {
        name: "steps".into(),
        columns: cols,
        extra: t_extra,
    };
    // facts
    let mut facts = Facts::new();
    facts.set("instrument.vendor", "Neware", "the file format");
    facts.set("instrument.software", "Neware BTS", "the file format");
    for (path, v, from) in &p.facts {
        facts.set(path, v, from);
    }
    if let Some((s, from)) = &p.started_at {
        facts.set("acquisition.started_at", s, from);
    }
    for (name, v, unit, from) in &p.parameters {
        if *v > 0.0 {
            facts.number(name, *v, unit, from);
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let duration = total.last().copied().unwrap_or(0.0);
    if duration > 0.0 {
        facts.duration(duration, "the last record's total test time");
    }
    let cycles = if p.has_cycle {
        p.records
            .iter()
            .map(|r| r.cycle)
            .filter(|c| c.is_finite())
            .fold(0.0f64, f64::max)
    } else {
        0.0
    };
    facts.measurement(
        MeasurementKind::Trace,
        vec![0],
        if cycles > 0.0 {
            format!("battery cycling, {n} records, {steps_n} steps, {cycles} cycles")
        } else {
            format!("battery cycling, {n} records, {steps_n} steps")
        },
        None,
    );
    let mut observations = std::mem::take(&mut p.observations);
    observations.feature(
        FeatureKind::FormatVersion,
        &p.version,
        &[Scope::Metadata, Scope::Traces, Scope::Tables],
    );
    observations.feature(
        FeatureKind::Layout,
        p.layout,
        &[Scope::Traces, Scope::Tables],
    );
    for a in trace.channels.iter().filter(|c| c.name.starts_with("aux")) {
        observations.feature(
            FeatureKind::Record,
            format!(
                "column {}",
                a.name
                    .trim_end_matches(|c: char| c.is_ascii_digit() || c == '_')
            ),
            &[Scope::Traces],
        );
    }
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::PriorArt);
    provenance.insert("tables".into(), Source::Inferred);
    let mut vendor = std::mem::take(&mut p.vendor);
    vendor.insert("file_size".into(), json!(size));
    Ok(SeriesFile {
        format_version: Some(p.version),
        traces: vec![trace],
        tables: vec![table],
        experiment: Some(facts.build()),
        vendor: Value::Object(vendor),
        entries: p.entries,
        findings: p.findings,
        notes: Vec::new(),
        provenance,
        observations,
        checks: vec!["record layout".into(), "page CRC-32 (ndax)".into()],
        members: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_names() {
        assert_eq!(range_scale(-100), Some(1e-3));
        assert_eq!(range_scale(7), None);
        assert_eq!(step_type_name(7), Some("cccv_charge"));
        assert_eq!(step_type_name(99), None);
    }

    #[test]
    fn calendar_stamps() {
        let b = [0xe4, 0x07, 3, 25, 11, 16, 50];
        assert_eq!(
            calendar(&b, 0, NDA_FORMAT_ID).unwrap().as_deref(),
            Some("2020-03-25T11:16:50")
        );
        let z = [0u8; 7];
        assert_eq!(calendar(&z, 0, NDA_FORMAT_ID).unwrap(), None);
    }
}
