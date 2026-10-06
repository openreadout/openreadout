//! BioLogic EC-Lab: the binary `.mpr` and the `.mpt` text export. Notes:
//! `docs/formats/biologic-eclab.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Observations, Scope};
use openreadout_core::bytes::{latin1, le_f64, le_u16, le_u32};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile, SeriesTrace};
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

pub(crate) const MPR_FORMAT_ID: &str = "biologic-mpr";
pub(crate) const MPT_FORMAT_ID: &str = "biologic-mpt";

/// `.mpr` signature.
pub(crate) const MPR_MAGIC: &[u8] = b"BIO-LOGIC MODULAR FILE";
/// `.mpt` first line.
pub(crate) const MPT_MAGIC: &[u8] = b"EC-Lab ASCII FILE";

/// Stored type of an `.mpr` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    F32,
    F64,
    U16,
    U32,
}

impl Ty {
    fn size(self) -> usize {
        match self {
            Ty::U16 => 2,
            Ty::F32 | Ty::U32 => 4,
            Ty::F64 => 8,
        }
    }
    fn dtype(self) -> &'static str {
        match self {
            Ty::F32 => "float32",
            Ty::F64 => "float64",
            Ty::U16 => "uint16",
            Ty::U32 => "uint32",
        }
    }
    fn read(self, b: &[u8]) -> f64 {
        match self {
            Ty::F32 => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            Ty::F64 => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            Ty::U16 => f64::from(u16::from_le_bytes([b[0], b[1]])),
            Ty::U32 => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        }
    }
}

/// A column: EC-Lab's label, our name, unit.
struct Col {
    id: u16,
    label: &'static str,
    name: &'static str,
    unit: Option<&'static str>,
    ty: Ty,
}

const fn c(
    id: u16,
    label: &'static str,
    name: &'static str,
    unit: Option<&'static str>,
    ty: Ty,
) -> Col {
    Col {
        id,
        label,
        name,
        unit,
        ty,
    }
}

/// Flag ids that share the record's first byte: (id, label, our name, mask).
const FLAGS: &[(u16, &str, &str, u8)] = &[
    (1, "mode", "mode", 0x03),
    (2, "ox/red", "ox_red", 0x04),
    (3, "error", "error", 0x08),
    (21, "control changes", "control_changes", 0x10),
    (31, "Ns changes", "sequence_changes", 0x20),
    (65, "counter inc.", "counter_increment", 0x80),
];

/// The value columns of `.mpr` data modules, validated against EC-Lab's exports (see the notes).
const COLS: &[Col] = &[
    c(4, "time/s", "time", Some("s"), Ty::F64),
    c(5, "control/V/mA", "control", None, Ty::F32),
    c(6, "Ewe/V", "ewe", Some("V"), Ty::F32),
    c(7, "dq/mA.h", "dq", Some("mA·h"), Ty::F64),
    c(8, "I/mA", "current", Some("mA"), Ty::F32),
    c(9, "Ece/V", "ece", Some("V"), Ty::F32),
    c(11, "<I>/mA", "current_mean", Some("mA"), Ty::F64),
    c(13, "(Q-Qo)/mA.h", "charge", Some("mA·h"), Ty::F64),
    c(16, "Analog IN 1/V", "analog_in_1", Some("V"), Ty::F32),
    c(17, "Analog IN 2/V", "analog_in_2", Some("V"), Ty::F32),
    c(19, "control/V", "control_potential", Some("V"), Ty::F32),
    c(20, "control/mA", "control_current", Some("mA"), Ty::F32),
    c(23, "dQ/mA.h", "delta_q", Some("mA·h"), Ty::F64),
    c(24, "cycle number", "cycle", None, Ty::F64),
    c(32, "freq/Hz", "frequency", Some("Hz"), Ty::F32),
    c(33, "|Ewe|/V", "ewe_amplitude", Some("V"), Ty::F32),
    c(34, "|I|/A", "current_amplitude", Some("A"), Ty::F32),
    c(35, "Phase(Z)/deg", "z_phase", Some("°"), Ty::F32),
    c(36, "|Z|/Ohm", "z_modulus", Some("Ω"), Ty::F32),
    c(37, "Re(Z)/Ohm", "z_real", Some("Ω"), Ty::F32),
    c(38, "-Im(Z)/Ohm", "z_imag_neg", Some("Ω"), Ty::F32),
    c(39, "I Range", "current_range", None, Ty::U16),
    c(70, "P/W", "power", Some("W"), Ty::F32),
    c(74, "|Energy|/W.h", "energy_abs", Some("W·h"), Ty::F64),
    c(76, "<I>/mA", "current_mean", Some("mA"), Ty::F32),
    c(77, "<Ewe>/V", "ewe_mean", Some("V"), Ty::F32),
    c(96, "|Ece|/V", "ece_amplitude", Some("V"), Ty::F32),
    c(98, "Phase(Zce)/deg", "zce_phase", Some("°"), Ty::F32),
    c(99, "|Zce|/Ohm", "zce_modulus", Some("Ω"), Ty::F32),
    c(100, "Re(Zce)/Ohm", "zce_real", Some("Ω"), Ty::F32),
    c(101, "-Im(Zce)/Ohm", "zce_imag_neg", Some("Ω"), Ty::F32),
    c(
        123,
        "Energy charge/W.h",
        "energy_charge",
        Some("W·h"),
        Ty::F64,
    ),
    c(
        124,
        "Energy discharge/W.h",
        "energy_discharge",
        Some("W·h"),
        Ty::F64,
    ),
    c(
        125,
        "Capacitance charge/µF",
        "capacitance_charge",
        Some("µF"),
        Ty::F64,
    ),
    c(
        126,
        "Capacitance discharge/µF",
        "capacitance_discharge",
        Some("µF"),
        Ty::F64,
    ),
    c(131, "Ns", "sequence", None, Ty::U16),
    c(168, "Rcmp/Ohm", "r_compensation", Some("Ω"), Ty::F32),
    c(169, "Cs/µF", "capacitance_series", Some("µF"), Ty::F32),
    c(172, "Cp/µF", "capacitance_parallel", Some("µF"), Ty::F32),
    c(174, "<Ewe>/V", "ewe_mean", Some("V"), Ty::F32),
    c(430, "Phase(Zwe-ce)/deg", "zwece_phase", Some("°"), Ty::F32),
    c(431, "|Zwe-ce|/Ohm", "zwece_modulus", Some("Ω"), Ty::F32),
    c(432, "Re(Zwe-ce)/Ohm", "zwece_real", Some("Ω"), Ty::F32),
    c(433, "-Im(Zwe-ce)/Ohm", "zwece_imag_neg", Some("Ω"), Ty::F32),
    c(434, "(Q-Qo)/C", "charge_coulomb", Some("C"), Ty::F32),
    c(435, "dQ/C", "delta_q_coulomb", Some("C"), Ty::F32),
    c(438, "step time/s", "step_time", Some("s"), Ty::F64),
    c(441, "<Ece>/V", "ece_mean", Some("V"), Ty::F32),
    c(
        467,
        "Q charge/discharge/mA.h",
        "charge_half_cycle",
        Some("mA·h"),
        Ty::F64,
    ),
    c(468, "half cycle", "half_cycle", None, Ty::U32),
    c(469, "z cycle", "z_cycle", None, Ty::U32),
    c(471, "<Ece>/V", "ece_mean", Some("V"), Ty::F32),
    c(473, "THD Ewe/%", "thd_ewe", Some("%"), Ty::F32),
    c(474, "THD I/%", "thd_current", Some("%"), Ty::F32),
    c(476, "NSD Ewe/%", "nsd_ewe", Some("%"), Ty::F32),
    c(477, "NSD I/%", "nsd_current", Some("%"), Ty::F32),
    c(479, "NSR Ewe/%", "nsr_ewe", Some("%"), Ty::F32),
    c(480, "NSR I/%", "nsr_current", Some("%"), Ty::F32),
    c(486, "|Ewe h2|/V", "ewe_h2", Some("V"), Ty::F32),
    c(487, "|Ewe h3|/V", "ewe_h3", Some("V"), Ty::F32),
    c(488, "|Ewe h4|/V", "ewe_h4", Some("V"), Ty::F32),
    c(489, "|Ewe h5|/V", "ewe_h5", Some("V"), Ty::F32),
    c(490, "|Ewe h6|/V", "ewe_h6", Some("V"), Ty::F32),
    c(491, "|Ewe h7|/V", "ewe_h7", Some("V"), Ty::F32),
    c(492, "|I h2|/A", "current_h2", Some("A"), Ty::F32),
    c(493, "|I h3|/A", "current_h3", Some("A"), Ty::F32),
    c(494, "|I h4|/A", "current_h4", Some("A"), Ty::F32),
    c(495, "|I h5|/A", "current_h5", Some("A"), Ty::F32),
    c(496, "|I h6|/A", "current_h6", Some("A"), Ty::F32),
    c(497, "|I h7|/A", "current_h7", Some("A"), Ty::F32),
    c(880, "Energy we/W.h", "energy_we", Some("W·h"), Ty::F64),
];

/// Labels EC-Lab uses for the same quantity in some modes.
const ALIASES: &[(&str, &str)] = &[
    ("Ecell/V", "ewe"),
    ("<Ewe/V>", "ewe_mean"),
    ("Pwe/W", "power"),
];

/// Technique codes (`VMP Set` byte 0) and EC-Lab's names for them.
const TECHNIQUES: &[(u8, &str, Option<&str>)] = &[
    (
        0x04,
        "Galvanostatic Cycling with Potential Limitation",
        Some("CHMO:0002936"),
    ),
    (0x06, "Cyclic Voltammetry", Some("CHMO:0000025")),
    (0x0B, "Open Circuit Voltage", Some("CHMO:0002933")),
    (
        0x18,
        "Chronoamperometry / Chronocoulometry",
        Some("CHMO:0000005"),
    ),
    (0x19, "Chronopotentiometry", Some("CHMO:0000017")),
    (0x1C, "Wait", None),
    (
        0x1D,
        "Potentio Electrochemical Impedance Spectroscopy",
        Some("CHMO:0002937"),
    ),
    (
        0x1E,
        "Galvano Electrochemical Impedance Spectroscopy",
        Some("CHMO:0000423"),
    ),
    (0x32, "IR compensation (PEIS)", Some("CHMO:0002937")),
    (0x33, "Cyclic Voltammetry Advanced", Some("CHMO:0000025")),
    (0x6C, "Linear Sweep Voltammetry", Some("CHMO:0000028")),
    (0x75, "Constant Voltage", Some("CHMO:0000005")),
    (0x76, "Constant Current", Some("CHMO:0000017")),
    (0x7F, "Modulo Bat", None),
    (0x88, "Battery Capacity Determination", None),
];

fn technique_term(name: &str) -> Option<&'static str> {
    TECHNIQUES
        .iter()
        .find(|(_, n, _)| n.eq_ignore_ascii_case(name.trim()))
        .and_then(|t| t.2)
}

/// Channel name, unit and label for an export column label.
fn from_label(label: &str) -> (String, Option<String>) {
    if let Some((_, n)) = ALIASES.iter().find(|(l, _)| *l == label) {
        let unit = COLS.iter().find(|c| c.name == *n).and_then(|c| c.unit);
        return ((*n).to_string(), unit.map(str::to_string));
    }
    if let Some(c) = COLS.iter().find(|c| c.label == label) {
        return (c.name.to_string(), c.unit.map(str::to_string));
    }
    if let Some((_, _, n, _)) = FLAGS.iter().find(|f| f.1 == label) {
        return ((*n).to_string(), None);
    }
    // a column not in the table: keep EC-Lab's label, unit after the last `/`
    let (q, unit) = match label.rsplit_once('/') {
        Some((q, u)) if !u.is_empty() && !q.is_empty() && !u.contains(' ') => {
            (q, Some(u.to_string()))
        }
        _ => (label, None),
    };
    let mut name: String = q
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    while name.contains("__") {
        name = name.replace("__", "_");
    }
    let name = name.trim_matches('_').to_string();
    (
        if name.is_empty() {
            "column".into()
        } else {
            name
        },
        unit.map(|u| unit_of(&u)),
    )
}

/// EC-Lab unit spellings in ours.
fn unit_of(u: &str) -> String {
    match u {
        "Ohm" => "Ω".into(),
        "deg" => "°".into(),
        "mA.h" => "mA·h".into(),
        "W.h" => "W·h".into(),
        "Ohm-1" => "S".into(),
        other => other.into(),
    }
}

/// One decoded column.
struct Column {
    name: String,
    unit: Option<String>,
    label: String,
    dtype: &'static str,
    values: Vec<f64>,
}

/// An OLE automation date (days since 1899-12-30) as ISO-8601 local time (no zone in the file).
fn ole_local(days: f64) -> Option<String> {
    if !days.is_finite() || !(2.0..2_958_465.0).contains(&days) {
        return None;
    }
    let ms = ((days - 25_569.0) * 86_400_000.0).round() as i64;
    let s =
        openreadout_core::time::unix_to_iso8601(ms.div_euclid(1000), (ms.rem_euclid(1000)) as u32);
    Some(s.trim_end_matches('Z').to_string())
}

/// What `.mpr` and `.mpt` parsing hands to [`finish`] besides the columns and the facts.
struct FileParts {
    technique: Option<String>,
    format_version: Option<String>,
    vendor: Value,
    entries: Vec<LsEntry>,
    findings: Vec<Finding>,
    observations: Observations,
    provenance_source: Source,
    checks: Vec<String>,
}

/// Build the file from decoded columns (shared by `.mpr` and `.mpt`).
fn finish(columns: Vec<Column>, facts: Facts, parts: FileParts) -> SeriesFile {
    let FileParts {
        technique,
        format_version,
        vendor,
        entries,
        mut findings,
        mut observations,
        provenance_source,
        checks,
    } = parts;
    let n = columns.first().map_or(0, |c| c.values.len());
    let has_time = columns.iter().any(|c| c.name == "time");
    // time first (the abscissa), then the columns in stored order
    let mut ordered: Vec<Column> = Vec::with_capacity(columns.len());
    let mut rest = Vec::new();
    for c in columns {
        if c.name == "time" && ordered.is_empty() {
            ordered.push(c);
        } else {
            rest.push(c);
        }
    }
    ordered.extend(rest);
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut channels = Vec::new();
    let mut values = Vec::new();
    for c in ordered {
        let k = seen.entry(c.name.clone()).or_insert(0);
        *k += 1;
        let name = if *k > 1 {
            format!("{}_{}", c.name, k)
        } else {
            c.name.clone()
        };
        let mut ch = SeriesChannel::new(name, c.unit.as_deref(), c.dtype);
        ch.extra.insert("label".into(), json!(c.label));
        channels.push(ch);
        values.push(c.values);
    }
    if channels
        .iter()
        .any(|c| c.name.starts_with("thd_") || c.name.starts_with("ewe_h"))
    {
        findings.push(Finding::info(
            "harmonics_as_stored",
            "harmonic-distortion columns are returned as stored; EC-Lab's text export prints -1 and 0 for them at the highest frequencies (above about 100 kHz)",
        ));
    }
    let mut extra = BTreeMap::new();
    if has_time {
        extra.insert(
            "axis".into(),
            json!({"quantity": "time", "unit": "s", "irregular": true, "channel": 0, "size": n}),
        );
    }
    extra.insert("kind".into(), json!("electrochemistry"));
    if let Some(t) = &technique {
        extra.insert("technique".into(), json!(t));
        observations.feature(FeatureKind::Acquisition, t.as_str(), &[]);
    }
    let trace = SeriesTrace {
        name: technique.clone().unwrap_or_else(|| "data".into()),
        channels,
        sweeps: vec![values],
        sample_rate_hz: 0.0,
        start_s: None,
        extra,
    };
    let mut facts = facts;
    let term = technique.as_deref().and_then(technique_term);
    facts.measurement(
        MeasurementKind::Trace,
        vec![0],
        format!(
            "{}, {n} points, {} columns",
            technique.as_deref().unwrap_or("electrochemical record"),
            trace.channels.len()
        ),
        term,
    );
    if let Some(t) = term {
        facts.technique(t, "the technique the file names");
    }
    if let Some(t) = &technique {
        facts.set("method.name", t, "the technique the file names");
    }
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), provenance_source);
    SeriesFile {
        format_version,
        traces: vec![trace],
        tables: Vec::new(),
        experiment: Some(facts.build()),
        vendor,
        entries,
        findings,
        notes: Vec::new(),
        provenance,
        observations,
        checks,
        members: Vec::new(),
    }
}

fn corrupt(at: usize, msg: impl Into<String>) -> Error {
    Error::corrupt_at(MPR_FORMAT_ID, at as u64, msg.into())
}

fn past_end(b: &[u8], at: usize, n: usize) -> Error {
    corrupt(
        at,
        format!("{n} bytes at {at} run past the end ({} bytes)", b.len()),
    )
}

fn slice(b: &[u8], at: usize, n: usize) -> Result<&[u8]> {
    at.checked_add(n)
        .and_then(|e| b.get(at..e))
        .ok_or_else(|| past_end(b, at, n))
}

/// The value `get` reads at `at`, or a corrupt error when it runs past the end.
fn read<T>(b: &[u8], at: usize, get: fn(&[u8], usize) -> Option<T>) -> Result<T> {
    get(b, at).ok_or_else(|| past_end(b, at, size_of::<T>()))
}

/// One `MODULE` of an `.mpr` file.
struct Module {
    short: String,
    long: String,
    body: usize,
    len: usize,
    version: u32,
    date: String,
    header: usize,
}

fn modules(b: &[u8]) -> Result<Vec<Module>> {
    let mut out = Vec::new();
    let mut p = 0x34usize;
    while p < b.len() {
        if !slice(b, p, 6)?.eq(b"MODULE") {
            return Err(corrupt(p, "expected a MODULE header"));
        }
        let short = String::from_utf8_lossy(slice(b, p + 6, 10)?)
            .trim()
            .to_string();
        let long = String::from_utf8_lossy(slice(b, p + 16, 25)?)
            .trim()
            .to_string();
        let a = read(b, p + 41, le_u32)?;
        let (len, version, date, header) = if a == u32::MAX {
            (
                read(b, p + 45, le_u32)?,
                read(b, p + 53, le_u32)?,
                slice(b, p + 57, 8)?,
                65,
            )
        } else {
            (a, read(b, p + 45, le_u32)?, slice(b, p + 49, 8)?, 57)
        };
        let len = usize::try_from(len).map_err(|_| corrupt(p, "module too long"))?;
        let body = p + header;
        slice(b, body, len)?;
        out.push(Module {
            short,
            long,
            body,
            len,
            version,
            date: String::from_utf8_lossy(date)
                .trim_end_matches('\0')
                .to_string(),
            header,
        });
        p = body + len;
    }
    Ok(out)
}

/// Parse an `.mpr` file.
pub(crate) fn parse_mpr(b: &[u8]) -> Result<SeriesFile> {
    if !b.starts_with(MPR_MAGIC) {
        return Err(corrupt(0, "not a BIO-LOGIC MODULAR FILE"));
    }
    let mods = modules(b)?;
    let data = mods
        .iter()
        .find(|m| m.short == "VMP data")
        .ok_or_else(|| corrupt(0x34, "no VMP data module"))?;
    let s = data.body;
    let n = usize::try_from(read(b, s, le_u32)?).map_err(|_| corrupt(s, "point count"))?;
    let (ids, first) = match data.version {
        // older EC-Lab: one byte per column id, records from byte 100
        0 => {
            let k = usize::from(*slice(b, s + 4, 1)?.first().unwrap_or(&0));
            let ids: Vec<u16> = slice(b, s + 5, k)?.iter().map(|&i| u16::from(i)).collect();
            (ids, 100)
        }
        2 | 3 => {
            let k = usize::from(*slice(b, s + 4, 1)?.first().unwrap_or(&0));
            let ids: Vec<u16> = (0..k)
                .map(|i| read(b, s + 5 + 2 * i, le_u16))
                .collect::<Result<_>>()?;
            (ids, if data.version == 2 { 405 } else { 406 })
        }
        10 | 11 => {
            let k = usize::from(read(b, s + 4, le_u16)?);
            let ids: Vec<u16> = (0..k)
                .map(|i| read(b, s + 6 + 2 * i, le_u16))
                .collect::<Result<_>>()?;
            (ids, 1007)
        }
        v => {
            return Err(Error::unsupported(
                MPR_FORMAT_ID,
                format!("data module version {v}"),
                "Only data modules of versions 0, 2, 3, 10 and 11 are read; export the file as text (.mpt) from EC-Lab.",
            ));
        }
    };
    let unknown: Vec<u16> = ids
        .iter()
        .copied()
        .filter(|i| !FLAGS.iter().any(|f| f.0 == *i) && !COLS.iter().any(|c| c.id == *i))
        .collect();
    if !unknown.is_empty() {
        return Err(Error::unsupported(
            MPR_FORMAT_ID,
            format!("column ids {unknown:?} not validated"),
            "The record layout of this file cannot be established without them; export the file as text (.mpt) from EC-Lab, which OpenReadout also reads.",
        ));
    }
    let has_flags = ids.iter().any(|i| FLAGS.iter().any(|f| f.0 == *i));
    let rec: usize = usize::from(has_flags)
        + ids
            .iter()
            .filter_map(|i| COLS.iter().find(|c| c.id == *i))
            .map(|c| c.ty.size())
            .sum::<usize>();
    let expect = n
        .checked_mul(rec)
        .and_then(|v| v.checked_add(first))
        .ok_or_else(|| corrupt(s, "record size overflows"))?;
    if expect != data.len {
        return Err(corrupt(
            s,
            format!(
                "the data module holds {} bytes; {n} records of {rec} bytes after a {first}-byte header need {expect}",
                data.len
            ),
        ));
    }
    let base = s + first;
    let mut columns = Vec::new();
    let mut off = usize::from(has_flags);
    for id in &ids {
        if let Some((_, label, name, mask)) = FLAGS.iter().find(|f| f.0 == *id) {
            let shift = mask.trailing_zeros();
            columns.push(Column {
                name: (*name).to_string(),
                unit: None,
                label: (*label).to_string(),
                dtype: "uint8",
                values: (0..n)
                    .map(|k| f64::from((b[base + k * rec] & mask) >> shift))
                    .collect(),
            });
        } else if let Some(col) = COLS.iter().find(|c| c.id == *id) {
            let sz = col.ty.size();
            let values = (0..n)
                .map(|k| {
                    col.ty
                        .read(&b[base + k * rec + off..base + k * rec + off + sz])
                })
                .collect();
            columns.push(Column {
                name: col.name.to_string(),
                unit: col.unit.map(str::to_string),
                label: col.label.to_string(),
                dtype: col.ty.dtype(),
                values,
            });
            off += sz;
        }
    }
    // metadata: technique, channel, start
    let mut facts = Facts::new();
    facts
        .set("instrument.vendor", "BioLogic", "the file format")
        .instrument_kind("CHMO:0002427")
        .set("instrument.software", "EC-Lab", "the file format");
    let set = mods.iter().find(|m| m.short == "VMP Set");
    let code = set.and_then(|m| b.get(m.body).copied());
    let technique = code.map(|c| {
        TECHNIQUES
            .iter()
            .find(|t| t.0 == c)
            .map_or_else(|| format!("technique code {c:#04x}"), |t| t.1.to_string())
    });
    let mut vendor = Map::new();
    if let Some(log) = mods.iter().find(|m| m.short == "VMP LOG") {
        // the acquisition start: +465 in the log of a file whose data module has version 0, +585
        // in later files (their logs can also have version 0)
        let (at, why) = if data.version == 0 {
            (465, "VMP LOG +465 (OLE date, local time)")
        } else {
            (585, "VMP LOG +585 (OLE date, local time)")
        };
        if log.len >= at + 8
            && let Some(v) = le_f64(b, log.body + at)
            && let Some(t) = ole_local(v)
        {
            facts.set("acquisition.started_at", &t, why);
            vendor.insert("acquisition_started".into(), json!(t));
        }
        if let Some(ch) = b.get(log.body + 9) {
            vendor.insert("channel_index".into(), json!(ch));
        }
    }
    if let Some(ref t) = technique {
        vendor.insert("technique".into(), json!(t));
    }
    if let Some(c) = code {
        vendor.insert("technique_code".into(), json!(c));
    }
    vendor.insert(
        "modules".into(),
        json!(mods.iter().map(|m| json!({"name": m.short, "long_name": m.long, "version": m.version, "date": m.date, "size": m.len})).collect::<Vec<_>>()),
    );
    vendor.insert("column_ids".into(), json!(ids));
    let entries = mods
        .iter()
        .map(|m| LsEntry {
            kind: "block".into(),
            name: m.short.clone(),
            offset: Some((m.body - m.header) as u64),
            size: Some((m.len + m.header) as u64),
            image: None,
            details: json!({"version": m.version, "date": m.date}),
        })
        .collect();
    let mut observations = openreadout_core::assurance::Observations::default();
    observations.feature(
        FeatureKind::FormatVersion,
        format!("data module {}", data.version),
        &[Scope::Traces, Scope::Metadata],
    );
    // loop indices, when stored
    let mut tables_note = None;
    if let Some(lp) = mods.iter().find(|m| m.short == "VMP loop")
        && let Ok(k) = read(b, lp.body, le_u32)
    {
        let k = usize::try_from(k).unwrap_or(0).min(lp.len / 4);
        let idx: Vec<u32> = (0..k)
            .filter_map(|i| read(b, lp.body + 4 + 4 * i, le_u32).ok())
            .collect();
        tables_note = Some(idx);
    }
    if let Some(idx) = &tables_note {
        vendor.insert("loop_start_indices".into(), json!(idx));
    }
    Ok(finish(
        columns,
        facts,
        FileParts {
            technique,
            format_version: Some(format!("mpr data module {}", data.version)),
            vendor: json!({ "eclab": Value::Object(vendor) }),
            entries,
            findings: Vec::new(),
            observations,
            provenance_source: Source::Inferred,
            checks: vec![format!(
                "data module size checked against {n} records of {rec} bytes (column ids {ids:?})"
            )],
        },
    ))
}

/// Parse an `.mpt` text export.
pub(crate) fn parse_mpt(bytes: &[u8]) -> Result<SeriesFile> {
    // Latin-1 (EC-Lab writes `µ`, `²` as single bytes)
    let text: String = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => latin1(bytes),
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut header_lines = 1usize;
    let mut has_header = false;
    if lines
        .first()
        .is_some_and(|l| l.starts_with("EC-Lab ASCII FILE"))
    {
        has_header = true;
        let nb = lines
            .iter()
            .take(4)
            .find_map(|l| l.strip_prefix("Nb header lines"))
            .and_then(|r| r.split(':').nth(1))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .ok_or_else(|| Error::corrupt(MPT_FORMAT_ID, "no `Nb header lines` count"))?;
        header_lines = nb;
    }
    if header_lines == 0 || header_lines > lines.len() {
        return Err(Error::corrupt(
            MPT_FORMAT_ID,
            format!("{header_lines} header lines in a {}-line file", lines.len()),
        ));
    }
    let head: Vec<&str> = lines[header_lines - 1].split('\t').map(str::trim).collect();
    let mut labels: Vec<&str> = head.iter().copied().filter(|h| !h.is_empty()).collect();
    // EC-Lab can announce a column after an empty label that its rows do not carry: when the
    // first row stops at the empty label, the labels after it are dropped
    let mut dropped = Vec::new();
    if let Some(gap) = head.iter().position(|h| h.is_empty())
        && gap < labels.len()
        && let Some(first) = lines[header_lines..].iter().find(|l| !l.trim().is_empty())
        && first.trim_end_matches(['\t', '\r']).split('\t').count() == gap
    {
        dropped = labels.split_off(gap);
    }
    if labels.is_empty() {
        return Err(Error::corrupt(MPT_FORMAT_ID, "no column header line"));
    }
    let mut cols: Vec<Vec<f64>> = vec![Vec::new(); labels.len()];
    // decimal-comma locales write `1,234E-003`: decided on the first data row (a comma in a value
    // and no point anywhere)
    let comma = lines[header_lines..]
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.contains(',') && !l.contains('.'));
    let mut bad_rows = 0usize;
    // Columns written as absolute date-times (EC-Lab's export option for `time/s`), decided on
    // the first data row: the cells' date fields and clock seconds, per column.
    let first_row: Vec<&str> = lines[header_lines..]
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.split('\t').collect())
        .unwrap_or_default();
    let mut stamps: Vec<Option<Vec<Stamp>>> = (0..labels.len())
        .map(|ci| first_row.get(ci).and_then(|v| stamp(v)).map(|_| Vec::new()))
        .collect();
    for (k, l) in lines[header_lines..].iter().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = l.split('\t').collect();
        if f.len() < labels.len() {
            bad_rows += 1;
            if bad_rows == 1 && k + header_lines + 1 < lines.len() {
                return Err(Error::corrupt(
                    MPT_FORMAT_ID,
                    format!(
                        "row {} has {} values for {} columns",
                        k + 1,
                        f.len(),
                        labels.len()
                    ),
                ));
            }
            continue;
        }
        for (ci, v) in f.iter().take(labels.len()).enumerate() {
            let v = v.trim();
            if let Some(Some(st)) = stamps.get_mut(ci) {
                st.push(stamp(v).ok_or_else(|| {
                    Error::corrupt(
                        MPT_FORMAT_ID,
                        format!(
                            "row {}: `{v}` is not a date and time like the first row's",
                            k + 1
                        ),
                    )
                })?);
                continue;
            }
            // decimal comma locales: `1,234E-003`
            let p = if comma == Some(true) {
                v.replace(',', ".").parse::<f64>()
            } else {
                v.parse::<f64>()
            };
            cols[ci].push(p.map_err(|_| {
                Error::corrupt(
                    MPT_FORMAT_ID,
                    format!("row {}: `{v}` is not a number", k + 1),
                )
            })?);
        }
    }
    let mut findings = Vec::new();
    // Date-time columns become seconds from their first row; the first row's date and time is
    // kept as the start of the data.
    let mut data_start: Option<(String, bool)> = None;
    for (ci, st) in stamps.iter().enumerate() {
        let Some(st) = st else { continue };
        let Some((secs, start, order_assumed)) = stamps_to_seconds(st) else {
            return Err(Error::corrupt(
                MPT_FORMAT_ID,
                format!(
                    "column `{}`: its dates are neither all month/day/year nor all day/month/year",
                    labels[ci]
                ),
            ));
        };
        findings.push(Finding::info(
            "absolute_times",
            format!(
                "column `{}` holds dates and times (first {start}); returned as seconds from its first row",
                labels[ci]
            ),
        ));
        if data_start.is_none() {
            data_start = Some((start, order_assumed));
        }
        cols[ci] = secs;
    }
    if !dropped.is_empty() {
        findings.push(Finding::info(
            "announced_columns_without_data",
            format!("the header names {dropped:?} after an empty label, but the rows carry no values for them"),
        ));
    }
    if bad_rows > 0 {
        findings.push(Finding::warning(
            "truncated_row",
            "the last row is incomplete and is left out",
        ));
    }
    let columns: Vec<Column> = labels
        .iter()
        .zip(cols)
        .map(|(l, v)| {
            let (name, unit) = from_label(l);
            Column {
                name,
                unit,
                label: (*l).to_string(),
                dtype: "float64",
                values: v,
            }
        })
        .collect();
    // header facts
    let mut facts = Facts::new();
    facts
        .set("instrument.vendor", "BioLogic", "the file format")
        .instrument_kind("CHMO:0002427");
    let mut vendor = Map::new();
    let mut technique = None;
    let mut header_start = false;
    if has_header {
        // the technique line sits inside the header block (a short header has none: line 3 is
        // then the column header or a data row)
        if let Some(t) = lines[..header_lines.saturating_sub(1)]
            .get(3)
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.contains(':') && !l.contains('\t'))
        {
            technique = Some(t.to_string());
        }
        let mut hdr = Map::new();
        for l in &lines[..header_lines.saturating_sub(1)] {
            if let Some((k, v)) = l.split_once(" : ") {
                hdr.insert(k.trim().to_string(), json!(v.trim()));
            }
        }
        let get = |k: &str| hdr.get(k).and_then(Value::as_str).map(str::to_string);
        if let Some(d) = get("Device") {
            let model = d.split(" (").next().unwrap_or(&d).trim().to_string();
            facts.set("instrument.model", &model, "`Device :` header line");
            if let Some(sn) = d.split("(SN ").nth(1).map(|s| s.trim_end_matches(')')) {
                facts.set("instrument.serial", sn, "`Device :` header line");
            }
        }
        if let Some(sw) = lines.iter().find(|l| l.starts_with("EC-Lab for windows"))
            && let Some(v) = sw.split_whitespace().nth(3)
        {
            {
                facts.set(
                    "instrument.software",
                    "EC-Lab",
                    "`EC-Lab for windows` header line",
                );
                facts.set(
                    "instrument.software_version",
                    v.trim_start_matches('v'),
                    "`EC-Lab for windows` header line",
                );
            }
        }
        if let Some(s) = get("Acquisition started on").and_then(|s| mpt_time(&s)) {
            facts.set(
                "acquisition.started_at",
                &s,
                "`Acquisition started on :` header line (month/day/year, local time)",
            );
            header_start = true;
        }
        if let Some(u) = get("User") {
            facts.set("acquisition.operator", &u, "`User :` header line");
        }
        if let Some(c) = get("Comments") {
            facts.set("acquisition.comment", &c, "`Comments :` header line");
        }
        if let Some(v) = get("Electrode surface area").and_then(|v| leading(&v)) {
            facts.number(
                "electrode_area",
                v.0,
                "cm²",
                "`Electrode surface area :` header line",
            );
            let _ = v.1;
        }
        if let Some(v) = get("Characteristic mass").and_then(|v| leading(&v)) {
            if v.1 == "g" {
                facts.number(
                    "characteristic_mass",
                    v.0 * 1000.0,
                    "mg",
                    "`Characteristic mass :` header line (g)",
                );
            } else if v.1 == "mg" {
                facts.number(
                    "characteristic_mass",
                    v.0,
                    "mg",
                    "`Characteristic mass :` header line (mg)",
                );
            }
        }
        if let Some(r) = get("Reference electrode") {
            facts.plain(
                "reference_electrode",
                r.as_str(),
                "`Reference electrode :` header line",
            );
        }
        vendor.insert("header".into(), Value::Object(hdr));
    }
    let mut observations = openreadout_core::assurance::Observations::default();
    if let Some((start, order_assumed)) = &data_start {
        observations.feature(
            FeatureKind::Layout,
            "absolute time column",
            &[Scope::Traces],
        );
        if !header_start {
            facts.set(
                "acquisition.started_at",
                start,
                "first row of the date-time column (local time)",
            );
            if *order_assumed {
                observations.assumed(
                    "experiment.acquisition.started_at",
                    "the date-time column's dates fit both month/day/year and day/month/year; month/day/year was assumed, as in EC-Lab's header",
                );
            }
        }
    }
    observations.feature(
        FeatureKind::Dialect,
        if has_header {
            "EC-Lab ASCII FILE"
        } else {
            "headerless"
        },
        &[Scope::Metadata, Scope::Traces],
    );
    if comma == Some(true) {
        observations.feature(FeatureKind::Dialect, "decimal comma", &[Scope::Traces]);
    }
    let n = columns.first().map_or(0, |c| c.values.len());
    let entries = vec![LsEntry {
        kind: "block".into(),
        name: "table".into(),
        offset: None,
        size: None,
        image: None,
        details: json!({"header_lines": header_lines, "rows": n, "columns": labels.len()}),
    }];
    Ok(finish(
        columns,
        facts,
        FileParts {
            technique,
            format_version: Some(
                if has_header {
                    "EC-Lab ASCII"
                } else {
                    "EC-Lab ASCII (no header)"
                }
                .to_string(),
            ),
            vendor: json!({ "eclab_text": Value::Object(vendor) }),
            entries,
            findings,
            observations,
            provenance_source: Source::Inferred,
            checks: vec![format!(
                "{n} rows parsed, every value a number (a date and time in a date-time column)"
            )],
        },
    ))
}

/// True when a first line is an EC-Lab column header (a text export without its header block):
/// tab-separated, with at least two labels EC-Lab writes (`time/s`, `Ewe/V`, `freq/Hz`, `mode`, …).
pub(crate) fn looks_like_table_header(line: &[u8]) -> bool {
    let l = String::from_utf8_lossy(line);
    if !l.contains('\t') {
        return false;
    }
    let known = l
        .trim_end()
        .split('\t')
        .filter(|f| {
            let f = f.trim();
            COLS.iter().any(|c| c.label == f)
                || FLAGS.iter().any(|x| x.1 == f)
                || ALIASES.iter().any(|a| a.0 == f)
        })
        .count();
    known >= 2
}

/// A number and the rest of the text (`0.001 cm²` → (0.001, "cm²")).
fn leading(v: &str) -> Option<(f64, String)> {
    let mut it = v.split_whitespace();
    let x = it.next()?.replace(',', ".").parse::<f64>().ok()?;
    Some((x, it.next().unwrap_or("").to_string()))
}

/// `MM/DD/YYYY HH:MM:SS.fff` (EC-Lab's US format) as ISO-8601 local time.
/// A date and clock time as an `.mpt` cell writes it (`07/19/2024 16:53:36.5000`): the two date
/// fields in file order, the year, and seconds since midnight.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stamp {
    a: u32,
    b: u32,
    year: i64,
    secs: f64,
}

fn stamp(s: &str) -> Option<Stamp> {
    let (date, time) = s.trim().split_once(' ')?;
    let mut d = date.split('/');
    let (a, b, year): (u32, u32, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    let mut t = time.trim().split(':');
    let (h, m, sec): (u32, u32, f64) = (
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    );
    let valid = d.next().is_none()
        && t.next().is_none()
        && (1..=31).contains(&a)
        && (1..=31).contains(&b)
        && (1900..=9999).contains(&year)
        && h <= 23
        && m <= 59
        && (0.0..61.0).contains(&sec);
    valid.then(|| Stamp {
        a,
        b,
        year,
        secs: f64::from(h) * 3600.0 + f64::from(m) * 60.0 + sec,
    })
}

/// A date-time column as seconds from its first row, its first date-time (ISO-8601, local
/// time) and whether the order of day and month was assumed. When both orders fit every date,
/// month/day/year is taken (EC-Lab's header writes dates that way) unless only day/month/year
/// keeps the times from running backwards. `None` when neither order fits every date.
fn stamps_to_seconds(st: &[Stamp]) -> Option<(Vec<f64>, String, bool)> {
    let first = st.first()?;
    let md = |s: &Stamp, month_first: bool| if month_first { (s.a, s.b) } else { (s.b, s.a) };
    let fits = |month_first: bool| st.iter().all(|s| (1..=12).contains(&md(s, month_first).0));
    // Seconds from the first stamp, with the day difference and the clock time kept apart so
    // that a fraction of a second is not lost against the size of a date in seconds.
    let absolute = |month_first: bool| -> Vec<f64> {
        let day = |s: &Stamp| {
            let (m, d) = md(s, month_first);
            openreadout_core::time::days_from_civil(s.year, m, d)
        };
        let d0 = day(first);
        st.iter()
            .map(|s| (day(s) - d0) as f64 * 86400.0 + (s.secs - first.secs))
            .collect()
    };
    let rising = |v: &[f64]| v.windows(2).all(|w| w[1] >= w[0]);
    let (mdy, dmy) = (fits(true), fits(false));
    let month_first = match (mdy, dmy) {
        (true, false) => true,
        (false, true) => false,
        (false, false) => return None,
        (true, true) => rising(&absolute(true)) || !rising(&absolute(false)),
    };
    let elapsed = absolute(month_first);
    let (m, d) = md(first, month_first);
    let s = first.secs.floor() as u32;
    let start = format!(
        "{:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        first.year,
        s / 3600,
        s / 60 % 60,
        s % 60
    );
    let assumed = mdy && dmy && first.a != first.b;
    Some((elapsed, start, assumed))
}

fn mpt_time(s: &str) -> Option<String> {
    let (date, time) = s.trim().split_once(' ')?;
    let parts: Vec<&str> = date.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let (month, day, year): (u32, u32, u32) = (
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1990 {
        return None;
    }
    let time = time.trim();
    let hms = time.split('.').next().unwrap_or(time);
    let fields: Vec<u32> = hms.split(':').filter_map(|x| x.parse().ok()).collect();
    if fields.len() != 3 || fields[0] > 23 || fields[1] > 59 || fields[2] > 60 {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        fields[0], fields[1], fields[2]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A module with the 57-byte header of version-0 files.
    fn module(short: &str, version: u32, body: &[u8]) -> Vec<u8> {
        let mut m = b"MODULE".to_vec();
        m.extend(format!("{short:<10}").as_bytes());
        m.extend(format!("{short:<25}").as_bytes());
        m.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
        m.extend(version.to_le_bytes());
        m.extend(b"10/29/11");
        m.extend(body);
        m
    }

    #[test]
    fn data_module_version_0() {
        // echem-figshare1228760-bio-logic4: u8 column count, u8 ids (flags, time, control/V,
        // Ewe, dq), records from byte 100 of the body; the start at VMP LOG +465
        let mut data = 2u32.to_le_bytes().to_vec();
        data.extend([10, 1, 2, 3, 21, 31, 65, 4, 19, 6, 7]);
        data.resize(100, 0);
        for (t, e) in [(0.0f64, 0.83f32), (1.0, 0.84)] {
            data.push(0x0b);
            data.extend(t.to_le_bytes());
            data.extend(0.5f32.to_le_bytes());
            data.extend(e.to_le_bytes());
            data.extend(0.0f64.to_le_bytes());
        }
        let mut log = vec![0u8; 473];
        log[2] = 0x04;
        log[465..473].copy_from_slice(&40_845.814_120_370_37f64.to_le_bytes());
        let mut b = MPR_MAGIC.to_vec();
        b.resize(0x34, b' ');
        b.extend(module("VMP Set", 0, &[0x04, 0, 0, 0]));
        b.extend(module("VMP data", 0, &data));
        b.extend(module("VMP LOG", 0, &log));
        let f = parse_mpr(&b).unwrap();
        let t = &f.traces[0];
        let ewe = t.channels.iter().position(|c| c.name == "ewe").unwrap();
        assert_eq!(
            t.sweeps[0][ewe],
            vec![f64::from(0.83f32), f64::from(0.84f32)]
        );
        let mode = t.channels.iter().position(|c| c.name == "mode").unwrap();
        assert_eq!(t.sweeps[0][mode], vec![3.0, 3.0]);
        let started = serde_json::to_value(&f.experiment).unwrap();
        assert!(started.to_string().contains("2011-10-29T19:32:20"));
        // a body one byte short of the records is corrupt
        data.pop();
        let mut short = MPR_MAGIC.to_vec();
        short.resize(0x34, b' ');
        short.extend(module("VMP data", 0, &data));
        assert!(parse_mpr(&short).is_err());
    }

    #[test]
    fn absolute_time_column() {
        // echem-figshare30080953-cp-mpt: a headerless export whose time/s holds date-times
        let t = "mode\tox/red\ttime/s\tEwe/V\n1\t0\t07/19/2024 16:53:36.5000\t-7.4302989E-001\n1\t0\t07/19/2024 16:53:36.5500\t-7.43E-001\n1\t0\t07/19/2024 16:53:37.0000\t-7.42E-001\n";
        let f = parse_mpt(t.as_bytes()).unwrap();
        assert_eq!(f.traces[0].channels[0].name, "time");
        let v = &f.traces[0].sweeps[0][0];
        assert_eq!(v[0], 0.0);
        assert!((v[1] - 0.05).abs() < 1e-9 && (v[2] - 0.5).abs() < 1e-9);
        // across midnight; both orders fit and only month/day keeps the times rising
        let st: Vec<Stamp> = ["01/02/2022 23:59:59", "01/03/2022 00:00:01"]
            .iter()
            .map(|s| stamp(s).unwrap())
            .collect();
        let (secs, start, assumed) = stamps_to_seconds(&st).unwrap();
        assert_eq!(secs, vec![0.0, 2.0]);
        assert_eq!(start, "2022-01-02T23:59:59");
        assert!(assumed);
        // day/month/year when a first field is above 12
        let st = [stamp("19/07/2024 10:00:00").unwrap()];
        assert_eq!(stamps_to_seconds(&st).unwrap().1, "2024-07-19T10:00:00");
        // a date-time column with a number in it is corrupt
        let bad = "time/s\tEwe/V\n07/19/2024 16:53:36.5000\t1\n12.5\t1\n";
        assert!(parse_mpt(bad.as_bytes()).is_err());
    }

    #[test]
    fn labels_and_times() {
        assert_eq!(from_label("Ewe/V").0, "ewe");
        assert_eq!(from_label("Ecell/V").0, "ewe");
        assert_eq!(
            from_label("Energy we charge/W.h"),
            ("energy_we_charge".into(), Some("W·h".into()))
        );
        assert_eq!(
            mpt_time("03/02/2021 16:17:59.000").as_deref(),
            Some("2021-03-02T16:17:59")
        );
        assert!(
            ole_local(44_257.679_155_092_59)
                .unwrap()
                .starts_with("2021-03-02T16:17:59")
        );
        assert!(parse_mpr(b"BIO-LOGIC MODULAR FILE").is_err());
        assert!(parse_mpt(b"EC-Lab ASCII FILE\nNb header lines : 99\n").is_err());
    }
}
