//! TA Instruments TRIOS files (`.tri`: DSC, TGA, DMA and Discovery rheometers): a header of
//! length-prefixed key/value strings, a thumbnail, then tagged objects — procedure steps holding
//! typed properties and signal records of float32 values. One trace per procedure step with
//! data; oscillation moduli of parallel-plate rheometer steps are computed as TRIOS does.
//! Notes: `docs/formats/ta-trios.md`.

use std::collections::BTreeMap;

use openreadout_core::assurance::{FeatureKind, Observations, Scope};
use openreadout_core::bytes::{find, le_f64, le_u32};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{Facts, SeriesChannel, SeriesFile, SeriesTrace};
use openreadout_core::time::civil_from_days;
use openreadout_core::{Error, Result};
use serde_json::{Map, json};

pub(crate) const FORMAT_ID: &str = "ta-trios";

/// The only file generation whose signal records are known (byte 2 of the file).
const GENERATION: u8 = 0x0E;
/// Bytes after a step object's GUID: 16 zeros, then this.
const STEP_PREFIX: [u8; 16] = [
    0xF2, 0x21, 0x01, 0x04, 0, 0, 0, 0, 0, 0, 0, 0x01, 0x00, 0x11, 0x20, 0x02,
];
/// Second u32 of a signal record that holds values.
const KIND_DATA: u32 = 1;

/// A GUID as stored (the .NET little-endian layout) from its text form.
const fn guid(s: &[u8; 36]) -> [u8; 16] {
    const fn hex(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => 0,
        }
    }
    // text positions of the 16 bytes in text order
    const POS: [usize; 16] = [0, 2, 4, 6, 9, 11, 14, 16, 19, 21, 24, 26, 28, 30, 32, 34];
    let mut t = [0u8; 16];
    let mut i = 0;
    while i < 16 {
        t[i] = hex(s[POS[i]]) << 4 | hex(s[POS[i] + 1]);
        i += 1;
    }
    // Data1 (4 bytes), Data2, Data3 little-endian; Data4 as is
    [
        t[3], t[2], t[1], t[0], t[5], t[4], t[7], t[6], t[8], t[9], t[10], t[11], t[12], t[13],
        t[14], t[15],
    ]
}

fn guid_text(g: &[u8]) -> String {
    let b = |i: usize| g.get(i).copied().unwrap_or(0);
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b(3),
        b(2),
        b(1),
        b(0),
        b(5),
        b(4),
        b(7),
        b(6),
        b(8),
        b(9),
        b(10),
        b(11),
        b(12),
        b(13),
        b(14),
        b(15)
    )
}

/// A signal we can name: its GUID, our name, TRIOS's label, the stored unit (when compared with
/// a TRIOS export) and how it was identified.
struct Known {
    id: [u8; 16],
    name: &'static str,
    label: &'static str,
    unit: Option<&'static str>,
}

const fn k(
    id: &[u8; 36],
    name: &'static str,
    label: &'static str,
    unit: Option<&'static str>,
) -> Known {
    Known {
        id: guid(id),
        name,
        label,
        unit,
    }
}

/// Signals named from the files: rheometer signals by value against TRIOS exports; DSC and TGA
/// signals by their position in `proceduresignals` (checked across files), units only where an
/// export confirmed them (`docs/formats/ta-trios.md`).
const KNOWN: &[Known] = &[
    k(
        b"cbc6046f-49d7-415c-946e-7bfa2cd99287",
        "time",
        "Time",
        Some("s"),
    ),
    k(
        b"f79c919e-6fa6-4856-92f8-6e5868f792f5",
        "temperature",
        "Temperature",
        Some("°C"),
    ),
    k(
        b"a2b78e93-cf94-4a36-b738-3bed96e287e7",
        "heat_flow",
        "Heat Flow",
        Some("W"),
    ),
    k(
        b"de9ffe89-e43c-4fd1-a048-5f6743742afd",
        "delta_t",
        "Delta T",
        None,
    ),
    k(
        b"1b60e3ad-7cf5-4c97-a3e3-3b1c88b024fb",
        "delta_t_zero",
        "Delta Tzero",
        None,
    ),
    k(
        b"f61bc5be-3499-47db-976a-f5b1fb599728",
        "t_zero_temperature",
        "Tzero Temperature",
        Some("°C"),
    ),
    k(
        b"4b91aaa9-96cb-4b6d-b830-3eaa7478bc7b",
        "cell_purge",
        "Cell Purge",
        None,
    ),
    k(
        b"00f51f55-133e-45b7-b8cf-0414bdb621e8",
        "heater_temperature",
        "Heater Temp",
        Some("°C"),
    ),
    k(
        b"f1548d7e-624e-4a23-9509-cf509c37205d",
        "power_delivered",
        "Power Delivered",
        None,
    ),
    k(
        b"73fd729c-3d80-40d2-9f21-bf36a8a62e42",
        "reference_junction_temperature",
        "Reference Junction Temperature",
        Some("°C"),
    ),
    k(
        b"560af22a-42ff-4875-8761-21422b3b84aa",
        "flange_temperature",
        "Flange Temperature",
        Some("°C"),
    ),
    k(
        b"de18f972-e46a-47ad-8c19-1f72558c2d76",
        "heat_flow_phase",
        "Heat Flow Phase",
        None,
    ),
    k(
        b"aeb6ea04-ce53-4352-b493-d91b88663d5f",
        "total_heat_capacity",
        "Total Heat Capacity",
        None,
    ),
    k(
        b"8b5cfbf4-6936-416e-932a-f030ff5e660c",
        "heat_capacity",
        "Heat Capacity",
        None,
    ),
    k(
        b"dfa25aa2-994f-4a55-a124-ee48d589d382",
        "weight",
        "Weight",
        Some("kg"),
    ),
    k(
        b"fd82807e-dddf-4bc2-bab8-abc7cf9c108d",
        "temperature_difference",
        "Temperature Difference",
        None,
    ),
    k(
        b"97a8c3f1-cc42-4ed1-bacd-2f5ca6c36095",
        "sample_purge",
        "Sample Purge",
        None,
    ),
    k(
        b"b58ee650-bac1-48ea-b678-6d0be331b72b",
        "balance_purge",
        "Balance Purge",
        None,
    ),
    k(
        b"75aae9d8-7c0f-4d49-a210-5862b5692a0a",
        "set_point_temperature",
        "Set Point Temperature",
        Some("°C"),
    ),
    k(
        b"e75a630a-393a-48db-9b19-91747e02aae6",
        "ramp_rate",
        "Ramp Rate",
        None,
    ),
    k(
        b"fa189102-89f7-4723-860d-82df3fd58ce5",
        "angular_frequency",
        "Angular frequency",
        Some("rad/s"),
    ),
    k(
        b"7b1a8875-39e5-4993-ae1d-67210ee57fb7",
        "step_time",
        "Step time",
        Some("s"),
    ),
    k(
        b"5051699d-7bbe-41e6-b9d9-540c57cc577e",
        "raw_phase",
        "Raw phase",
        Some("rad"),
    ),
    k(
        b"360672f5-f937-414b-bd06-5b9d80d6975e",
        "oscillation_torque",
        "Oscillation torque",
        Some("N·m"),
    ),
    k(
        b"bb58e619-9763-45b5-9c1e-5b0a5b90642c",
        "oscillation_displacement",
        "Oscillation displacement",
        Some("rad"),
    ),
    k(
        b"94411d75-361a-4cd4-a298-841a22dd9a83",
        "gap",
        "Gap",
        Some("m"),
    ),
];

/// Property keys the moduli need.
const P_STEP_NAME: [u8; 16] = guid(b"d467abca-26c2-4afa-8ce7-23e95909709d");
const P_STRESS_CONSTANT: [u8; 16] = guid(b"e4fea4e5-a210-4f38-b6b2-0a28b2137a9c");
const P_INSTRUMENT_INERTIA: [u8; 16] = guid(b"6594d858-7e8d-402c-8554-12377d9f33c0");
const P_GEOMETRY_INERTIA: [u8; 16] = guid(b"2dad9ebd-bdae-4839-9bd4-44c39e65f684");
const P_SAMPLE_DENSITY: [u8; 16] = guid(b"a58eb6fe-2446-4e34-b59f-47d56d2461d4");
const P_SOFTWARE_VERSION: [u8; 16] = guid(b"ed6fea85-481a-41f3-acac-9d5bf88f23f7");

fn known(id: &[u8]) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.id == id)
}

/// A .NET 7-bit-length-prefixed UTF-8 string at `at`: (text, end).
fn string_at(b: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut n: usize = 0;
    let mut shift = 0u32;
    loop {
        let c = *b.get(at)?;
        at += 1;
        n |= usize::from(c & 0x7F).checked_shl(shift)?;
        if c < 0x80 {
            break;
        }
        shift += 7;
        if shift > 28 {
            return None;
        }
    }
    let s = b.get(at..at.checked_add(n)?)?;
    Some((String::from_utf8_lossy(s).into_owned(), at + n))
}

/// True when `head` starts like a TRIOS file.
pub(crate) fn looks_like(head: &[u8]) -> bool {
    head.len() >= 40
        && head[0] == 0x00
        && head[1] == 0x25
        && head.get(7..10) == Some(&[0x08, 0x25, 0x02][..])
        && head.get(14..17) == Some(&[0x14, 0x20, 0x01][..])
        && string_at(head, 25).is_some_and(|(k, _)| {
            !k.is_empty() && k.len() < 64 && k.bytes().all(|c| c.is_ascii_alphabetic())
        })
}

/// One signal record.
#[derive(Debug)]
struct Signal {
    id: [u8; 16],
    values: Vec<f32>,
    /// Number of per-point flags that are not zero (a flag array precedes some signals).
    flagged: usize,
}

/// What a `21 06` object at `at` is.
enum Object {
    Step {
        end: usize,
        id: [u8; 16],
    },
    Signal {
        end: usize,
        signal: Option<Signal>,
        other: &'static str,
    },
}

/// Parse the object at `at` (`21 06 <u32 length> <GUID>`), strictly: a record whose tokens do
/// not end exactly at its length is not one.
fn object(b: &[u8], at: usize) -> Option<Object> {
    if b.get(at..at + 2)? != [0x21, 0x06] {
        return None;
    }
    let len = le_u32(b, at + 2)? as usize;
    let end = at.checked_add(6)?.checked_add(len)?;
    if len < 16 || end > b.len() {
        return None;
    }
    let mut id = [0u8; 16];
    id.copy_from_slice(b.get(at + 6..at + 22)?);
    let start = at + 22;
    if b.get(start..start + 16)?.iter().all(|x| *x == 0)
        && b.get(start + 16..start + 32)? == STEP_PREFIX
    {
        return Some(Object::Step { end, id });
    }
    let first = le_u32(b, start)?;
    let kind = le_u32(b, start + 4)?;
    if !(first == 1 || first == 2) || b.get(start + 8..start + 10)? != [0x00, 0xF2] {
        return None;
    }
    // 21 01 04000000 00000000 | 01 00 <count>
    let mut cur = start + 10;
    if b.get(cur..cur + 6)? != [0x21, 0x01, 0x04, 0, 0, 0] {
        return None;
    }
    cur += 10;
    if b.get(cur..cur + 2)? != [0x01, 0x00] {
        return None;
    }
    let count = le_u32(b, cur + 2)? as usize;
    cur += 6;
    let other = if first == 2 {
        "per-point arrays (first u32 2: 64 values per data point, the oscillation waveforms)"
    } else if kind == KIND_DATA {
        "signal"
    } else if kind == 0x0280_0001 {
        "variable without stored values (TRIOS computes it)"
    } else {
        "companion record (n + 1 points)"
    };
    // a few trailing bytes (`00 00 00`, `01 00 00`) end every record
    let tail_ok = |from: usize| -> bool { from <= end && end - from <= 8 };
    // no values: the record ends here
    if b.get(cur..cur + 2) != Some(&[0x01, 0x10][..]) {
        return tail_ok(cur).then_some(Object::Signal {
            end,
            signal: None,
            other,
        });
    }
    cur += 2;
    if b.get(cur..cur + 2)? != [0x21, 0x01] {
        return None;
    }
    let blob = le_u32(b, cur + 2)? as usize;
    let blob_start = cur + 6;
    let blob_end = blob_start.checked_add(blob)?;
    if blob_end > end {
        return None;
    }
    let mut flagged = 0usize;
    if blob != 4 {
        let flags = le_u32(b, blob_start)? as usize;
        if blob != 4usize.checked_add(flags.checked_mul(4)?)? {
            return None;
        }
        for i in 0..flags {
            if le_u32(b, blob_start + 4 + 4 * i)? != 0 {
                flagged += 1;
            }
        }
    }
    cur = blob_end;
    if b.get(cur..cur + 2)? != [0x01, 0x00] {
        return None;
    }
    let n2 = le_u32(b, cur + 2)? as usize;
    cur += 6;
    let data_end = cur.checked_add(n2.checked_mul(4)?)?;
    if data_end > end || !tail_ok(data_end) {
        return None;
    }
    if first != 1 || kind != KIND_DATA || n2 != count {
        return Some(Object::Signal {
            end,
            signal: None,
            other,
        });
    }
    let values = b[cur..data_end]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    Some(Object::Signal {
        end,
        signal: Some(Signal {
            id,
            values,
            flagged,
        }),
        other,
    })
}

/// A typed property (`20 04 <u32 length> <key GUID> …`) at `at`: (key, value, end).
fn property(b: &[u8], at: usize) -> Option<([u8; 16], PropValue, usize)> {
    if b.get(at..at + 2)? != [0x20, 0x04] {
        return None;
    }
    let len = le_u32(b, at + 2)? as usize;
    let end = at.checked_add(6)?.checked_add(len)?;
    if !(48..=1 << 20).contains(&len) || end > b.len() {
        return None;
    }
    let mut key = [0u8; 16];
    key.copy_from_slice(b.get(at + 6..at + 22)?);
    let body = b.get(at + 22..end)?;
    let i = find(body, &[0x06, 0x20, 0x01])?;
    let vlen = le_u32(body, i + 3)? as usize;
    let typ = le_u32(body, i + 7)?;
    let v = body.get(i + 15..(i + 7).checked_add(vlen)?)?;
    let value = match typ {
        2 => PropValue::Number(le_f64(v, 0)?),
        4 => PropValue::Text(string_at(v, 0)?.0),
        _ => PropValue::Other,
    };
    Some((key, value, end))
}

#[derive(Debug, Clone, PartialEq)]
enum PropValue {
    Number(f64),
    Text(String),
    Other,
}

/// A procedure step with its signals.
struct Step {
    start: usize,
    end: usize,
    id: [u8; 16],
    name: Option<String>,
    signals: Vec<Signal>,
}

/// Values of the properties the reader uses (first occurrence of each key in the file), and
/// whether a key had differing values.
#[derive(Default)]
struct Props {
    numbers: BTreeMap<[u8; 16], (f64, bool)>,
    texts: BTreeMap<[u8; 16], String>,
}

impl Props {
    fn number(&self, key: &[u8; 16]) -> Option<f64> {
        self.numbers
            .get(key)
            .filter(|(v, conflict)| !conflict && v.is_finite())
            .map(|(v, _)| *v)
    }
}

/// The header: key/value pairs, and where it ends.
fn header(b: &[u8]) -> Result<(Vec<(String, String)>, usize)> {
    let count =
        le_u32(b, 21).ok_or_else(|| Error::corrupt(FORMAT_ID, "the file ends in its header"))?;
    if count > 4096 {
        return Err(Error::corrupt(FORMAT_ID, format!("{count} header entries")));
    }
    let mut at = 25usize;
    let mut out = Vec::new();
    for i in 0..count {
        let (k, a) = string_at(b, at)
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("header entry {i}: no key")))?;
        let (v, a) = string_at(b, a).ok_or_else(|| {
            Error::corrupt(FORMAT_ID, format!("header entry {i} ({k}): no value"))
        })?;
        out.push((k, v));
        at = a;
    }
    Ok((out, at))
}

/// .NET `DateTime` ticks (100 ns since 0001-01-01, the PC's clock) as ISO-8601 without a zone.
fn ticks_to_iso(ticks: u64) -> Option<String> {
    // days from 0001-01-01 to 1970-01-01
    const EPOCH_DAYS: i64 = 719_162;
    let secs = i64::try_from(ticks / 10_000_000).ok()?;
    let frac = ticks % 10_000_000;
    let days = secs.div_euclid(86_400) - EPOCH_DAYS;
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    if !(1900..=2200).contains(&y) {
        return None;
    }
    Some(format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}",
        sod / 3600,
        sod / 60 % 60,
        sod % 60,
        frac / 10_000
    ))
}

/// Oscillation moduli of a parallel-plate step (TRIOS's definitions, `docs/formats/ta-trios.md`):
/// storage and loss modulus (Pa), tan δ and complex viscosity (Pa·s). `None` unless every
/// input is present.
#[allow(clippy::many_single_char_names, clippy::similar_names)]
fn moduli(
    cols: &BTreeMap<&str, &Vec<f64>>,
    stress_constant: f64,
    inertia: f64,
    density: f64,
) -> Option<[Vec<f64>; 4]> {
    let (w, m, p, th, gap) = (
        cols.get("angular_frequency")?,
        cols.get("oscillation_torque")?,
        cols.get("raw_phase")?,
        cols.get("oscillation_displacement")?,
        cols.get("gap")?,
    );
    // parallel plate: stress constant 2 / (π R³)
    let r = (2.0 / (std::f64::consts::PI * stress_constant)).cbrt();
    if !(r.is_finite() && r > 0.0) {
        return None;
    }
    let n = w.len();
    let (mut g1, mut g2, mut tan, mut eta) = (
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    );
    for i in 0..n {
        let (w, m, p, th, h) = (w[i], m[i], p[i], th[i], gap[i]);
        let strain_constant = r / h;
        let sample_inertia = std::f64::consts::PI * density * h * r.powi(4) / 6.0;
        let i_total = inertia + sample_inertia;
        let k = stress_constant / (strain_constant * th);
        let gp = k * (m * p.cos() + i_total * w * w * th);
        let gpp = k * m * p.sin();
        g1.push(gp);
        g2.push(gpp);
        tan.push(gpp / gp);
        eta.push(gp.hypot(gpp) / w);
    }
    Some([g1, g2, tan, eta])
}

/// What [`scan_objects`] finds.
struct Objects {
    steps: Vec<Step>,
    loose: Vec<Signal>,
    others: BTreeMap<&'static str, usize>,
    props: Props,
}

/// The objects after the header: steps, signals, properties.
fn scan_objects(b: &[u8], header_end: usize) -> Objects {
    let mut steps: Vec<Step> = Vec::new();
    let mut loose: Vec<Signal> = Vec::new();
    let mut others: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut props = Props::default();
    let mut at = header_end;
    while at + 22 <= b.len() {
        match (b[at], b[at + 1]) {
            (0x21, 0x06) => match object(b, at) {
                Some(Object::Step { end, id }) => {
                    steps.push(Step {
                        start: at,
                        end,
                        id,
                        name: None,
                        signals: Vec::new(),
                    });
                    at += 22;
                    continue;
                }
                Some(Object::Signal { end, signal, other }) => {
                    match signal {
                        Some(s) => match steps
                            .iter_mut()
                            .rev()
                            .find(|st| st.start < at && at < st.end)
                        {
                            Some(st) => st.signals.push(s),
                            None => loose.push(s),
                        },
                        None => *others.entry(other).or_insert(0) += 1,
                    }
                    at = end;
                    continue;
                }
                None => {}
            },
            (0x20, 0x04) => {
                if let Some((key, value, end)) = property(b, at) {
                    match value {
                        PropValue::Number(v) => {
                            let e = props.numbers.entry(key).or_insert((v, false));
                            if e.0.to_bits() != v.to_bits() && !(e.0.is_nan() && v.is_nan()) {
                                e.1 = true;
                            }
                        }
                        PropValue::Text(t) => {
                            if key == P_STEP_NAME
                                && let Some(st) = steps
                                    .iter_mut()
                                    .rev()
                                    .find(|st| st.start < at && at < st.end && st.name.is_none())
                            {
                                st.name = Some(t.clone());
                            }
                            props.texts.entry(key).or_insert(t);
                        }
                        PropValue::Other => {}
                    }
                    at = end;
                    continue;
                }
            }
            _ => {}
        }
        at += 1;
    }
    Objects {
        steps,
        loose,
        others,
        props,
    }
}

/// The trace of procedure step `si`, and whether the oscillation moduli were computed for it
/// (`moduli_inputs`: stress constant, inertia, density, for a parallel plate).
fn step_trace(
    si: usize,
    st: &Step,
    moduli_inputs: Option<(f64, f64, f64)>,
    unknown_signals: &mut Vec<String>,
) -> (SeriesTrace, bool) {
    let mut computed_any = false;
    // the step's signals, first occurrence of each GUID
    let mut sigs: Vec<&Signal> = Vec::new();
    for s in &st.signals {
        if !sigs.iter().any(|x| x.id == s.id) {
            sigs.push(s);
        }
    }
    let n = sigs.iter().map(|s| s.values.len()).max().unwrap_or(0);
    let sigs: Vec<&Signal> = sigs.into_iter().filter(|s| s.values.len() == n).collect();
    // abscissa: Time, else Step time, else the point index
    let axis_pos = ["time", "step_time"].iter().find_map(|want| {
        sigs.iter()
            .position(|s| known(&s.id).is_some_and(|k| k.name == *want))
    });
    let mut order: Vec<usize> = (0..sigs.len()).collect();
    if let Some(a) = axis_pos {
        order.sort_by_key(|&i| u8::from(i != a));
    }
    let mut channels = Vec::new();
    let mut values: Vec<Vec<f64>> = Vec::new();
    let mut by_name: BTreeMap<&str, &Vec<f64>> = BTreeMap::new();
    let mut names_used: BTreeMap<String, usize> = BTreeMap::new();
    for &i in &order {
        let s = sigs[i];
        let g = guid_text(&s.id);
        let (name, unit, label) = if let Some(k) = known(&s.id) {
            (k.name.to_string(), k.unit, Some(k.label))
        } else {
            if !unknown_signals.contains(&g) {
                unknown_signals.push(g.clone());
            }
            (format!("signal_{}", &g[..8]), None, None)
        };
        let c = names_used.entry(name.clone()).or_insert(0);
        *c += 1;
        let name = if *c > 1 { format!("{name}_{c}") } else { name };
        let mut ch = SeriesChannel::new(name, unit, "float32");
        ch.extra.insert("guid".into(), json!(g));
        if let Some(l) = label {
            ch.extra.insert("label".into(), json!(l));
        }
        if s.flagged > 0 {
            ch.extra.insert("flagged_points".into(), json!(s.flagged));
        }
        channels.push(ch);
        values.push(s.values.iter().map(|v| f64::from(*v)).collect());
    }
    for (ch, v) in channels.iter().zip(&values) {
        by_name.insert(ch.name.as_str(), v);
    }
    // oscillation moduli (parallel plate)
    let mut computed: Vec<(SeriesChannel, Vec<f64>)> = Vec::new();
    if let Some((kt, inr, rho)) = moduli_inputs
        && let Some([g1, g2, tan, eta]) = moduli(&by_name, kt, inr, rho)
    {
        for (name, label, unit, v) in [
            ("storage_modulus", "Storage modulus", Some("Pa"), g1),
            ("loss_modulus", "Loss modulus", Some("Pa"), g2),
            ("tan_delta", "Tan(delta)", None, tan),
            ("complex_viscosity", "Complex viscosity", Some("Pa·s"), eta),
        ] {
            let mut ch = SeriesChannel::new(name, unit, "float64");
            ch.extra.insert("label".into(), json!(label));
            ch.extra.insert(
                "computed".into(),
                json!("from torque, raw phase, displacement, angular frequency and gap with the geometry's stress constant, the instrument and geometry inertia and the sample inertia, as TRIOS computes it (parallel plate)"),
            );
            computed.push((ch, v));
        }
        computed_any = true;
    }
    drop(by_name);
    for (ch, v) in computed {
        channels.push(ch);
        values.push(v);
    }
    let mut extra = BTreeMap::new();
    match axis_pos {
        Some(_) => {
            let q = if channels[0].name == "time" {
                "time"
            } else {
                "step_time"
            };
            extra.insert(
                "axis".into(),
                json!({"quantity": q, "unit": "s", "irregular": true, "channel": 0, "size": n}),
            );
        }
        None => {
            extra.insert(
                "axis".into(),
                json!({"quantity": "point", "unit": null, "first": 0, "step": 1, "size": n}),
            );
        }
    }
    extra.insert("kind".into(), json!("trios_step"));
    extra.insert("step".into(), json!(si));
    extra.insert("guid".into(), json!(guid_text(&st.id)));
    let name = st
        .name
        .clone()
        .unwrap_or_else(|| format!("step {}", si + 1));
    let trace = SeriesTrace {
        name,
        channels,
        sweeps: vec![values],
        sample_rate_hz: 0.0,
        start_s: None,
        extra,
    };
    (trace, computed_any)
}

/// Parse a TRIOS file.
pub(crate) fn parse(b: &[u8]) -> Result<SeriesFile> {
    if !looks_like(b) {
        return Err(Error::corrupt(FORMAT_ID, "not a TRIOS file header"));
    }
    let generation = b[2];
    let (hdr, header_end) = header(b)?;
    let get = |key: &str| {
        hdr.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let Objects {
        steps,
        loose,
        others,
        props,
    } = scan_objects(b, header_end);
    let with_data: Vec<&Step> = steps.iter().filter(|s| !s.signals.is_empty()).collect();
    if with_data.is_empty() {
        if generation != GENERATION {
            return Err(Error::Unsupported {
                format: FORMAT_ID,
                feature: format!(
                    "a TRIOS file of generation {generation} (byte 2), whose data records are not the ones decoded (generation {GENERATION})"
                ),
                hint: Some("export the run from TRIOS (File > Export) to text or Excel; files from TRIOS 5 are read".into()),
            });
        }
        return Err(Error::Unsupported {
            format: FORMAT_ID,
            feature: "a TRIOS file without signal records in its procedure steps".into(),
            hint: Some("export the run from TRIOS to text or Excel".into()),
        });
    }
    let instrument = get("instrumenttype").unwrap_or_default();
    let holder = get("holder").unwrap_or_default();
    let parallel_plate = holder.to_ascii_lowercase().contains("parallel plate");
    let stress_constant = props.number(&P_STRESS_CONSTANT);
    let inertia = match (
        props.number(&P_INSTRUMENT_INERTIA),
        props.number(&P_GEOMETRY_INERTIA),
    ) {
        (Some(a), Some(g)) => Some(a + g),
        _ => None,
    };
    let density = props.number(&P_SAMPLE_DENSITY);
    let moduli_inputs = match (stress_constant, inertia, density) {
        (Some(kt), Some(inr), Some(rho)) if parallel_plate => Some((kt, inr, rho)),
        _ => None,
    };
    let mut findings = Vec::new();
    let mut traces = Vec::new();
    let mut computed_any = false;
    let mut unknown_signals: Vec<String> = Vec::new();
    for (si, st) in with_data.iter().enumerate() {
        let (trace, computed) = step_trace(si, st, moduli_inputs, &mut unknown_signals);
        computed_any |= computed;
        traces.push(trace);
    }
    if !loose.is_empty() {
        findings.push(Finding::warning(
            "signals_outside_steps",
            format!(
                "{} signal records outside any procedure step are left out",
                loose.len()
            ),
        ));
    }
    let facts = trios_facts(&get, &props, &instrument, &holder, &traces);
    let observations = trios_observations(
        generation,
        &props,
        &instrument,
        computed_any,
        stress_constant,
        &others,
        &unknown_signals,
    );
    let vendor = trios_vendor(&hdr, generation, stress_constant, inertia);
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("traces".into(), Source::Inferred);
    let entries = with_data
        .iter()
        .enumerate()
        .map(|(i, st)| LsEntry {
            kind: "step".into(),
            name: st.name.clone().unwrap_or_else(|| format!("step {}", i + 1)),
            offset: Some(st.start as u64),
            size: Some((st.end - st.start) as u64),
            image: None,
            details: json!({"signals": st.signals.len(), "points": st.signals.first().map_or(0, |s| s.values.len())}),
        })
        .collect();
    let notes = trios_notes(&traces, computed_any);
    Ok(SeriesFile {
        format_version: Some(generation.to_string()),
        traces,
        tables: Vec::new(),
        experiment: Some(facts.build()),
        vendor: json!({"trios": vendor}),
        entries,
        findings,
        notes,
        provenance,
        observations,
        checks: vec![
            "header entries".into(),
            "procedure steps and their signal records, each parsed to its stated length".into(),
        ],
        members: Vec::new(),
    })
}

/// Experiment facts of the header entries and properties.
fn trios_facts(
    get: &dyn Fn(&str) -> Option<String>,
    props: &Props,
    instrument: &str,
    holder: &str,
    traces: &[SeriesTrace],
) -> Facts {
    let mut facts = Facts::new();
    facts.set("instrument.vendor", "TA Instruments", "the file format");
    if !instrument.is_empty() {
        facts.set(
            "instrument.model",
            instrument,
            "`instrumenttype` header entry",
        );
    }
    if let Some(v) = get("instrumentserialnumber") {
        facts.set(
            "instrument.serial",
            &v,
            "`instrumentserialnumber` header entry",
        );
    }
    if let Some(v) = props.texts.get(&P_SOFTWARE_VERSION) {
        facts.set(
            "instrument.software_version",
            &format!("TRIOS {v}"),
            "the TRIOS version property",
        );
    }
    if let Some(v) = get("operator") {
        facts.set("acquisition.operator", &v, "`operator` header entry");
    }
    if let Some(v) = get("samplename") {
        facts.set("sample.name", &v, "`samplename` header entry");
    }
    if let Some(v) = get("comments") {
        facts.set("acquisition.comment", &v, "`comments` header entry");
    }
    if let Some(t) = get("ticks")
        .and_then(|t| t.parse::<u64>().ok())
        .and_then(ticks_to_iso)
    {
        facts.set(
            "acquisition.started_at",
            &t,
            "`ticks` header entry (.NET ticks of the PC clock)",
        );
    }
    if let Some(v) = get("procedurename") {
        facts.set("method.name", &v, "`procedurename` header entry");
    }
    if let Some(v) = get("proceduresegments") {
        let steps: Vec<&str> = v
            .split([';', '#'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        facts.plain(
            "method_steps",
            json!(steps),
            "`proceduresegments` header entry",
        );
    }
    if !holder.is_empty() {
        facts.plain("geometry", json!(holder), "`holder` header entry");
    }
    let term = if instrument.starts_with("DSC") {
        Some("CHMO:0000684")
    } else if instrument.starts_with("TGA") {
        Some("CHMO:0000690")
    } else if instrument.starts_with("Discovery HR") {
        Some("CHMO:0000915")
    } else {
        None
    };
    if let Some(t) = term {
        facts.technique(t, "the instrument type");
    }
    let total: usize = traces
        .iter()
        .map(|t| t.sweeps[0].first().map_or(0, Vec::len))
        .sum();
    facts.measurement(
        MeasurementKind::Trace,
        (0..traces.len() as u32).collect(),
        format!(
            "{} run, {} procedure steps with data, {total} data points",
            if instrument.is_empty() {
                "TRIOS"
            } else {
                instrument
            },
            traces.len()
        ),
        term,
    );
    facts
}

fn trios_observations(
    generation: u8,
    props: &Props,
    instrument: &str,
    computed_any: bool,
    stress_constant: Option<f64>,
    others: &BTreeMap<&'static str, usize>,
    unknown_signals: &[String],
) -> Observations {
    let mut observations = Observations::default();
    observations.feature(
        FeatureKind::FormatVersion,
        format!("generation {generation}"),
        &[Scope::Metadata, Scope::Traces],
    );
    if let Some(v) = props
        .texts
        .get(&P_SOFTWARE_VERSION)
        .and_then(|v| openreadout_core::assurance::version_prefix(v, 2))
    {
        observations.context(FeatureKind::WriterVersion, format!("TRIOS {v}"));
    }
    if !instrument.is_empty() {
        observations.context(FeatureKind::Instrument, instrument);
    }
    if computed_any {
        let d = stress_constant.map_or(f64::NAN, |kt| {
            2.0 * (2.0 / (std::f64::consts::PI * kt)).cbrt() * 1000.0
        });
        observations.feature(
            FeatureKind::Acquisition,
            format!("oscillation moduli, parallel plate {d:.0} mm"),
            &[Scope::Traces],
        );
    }
    for (what, count) in others {
        if what.starts_with("per-point") || what.starts_with("companion") {
            observations.undecoded(
                format!("{count} {what}"),
                &[],
                "left out; the step's signals are returned",
            );
        }
    }
    if !unknown_signals.is_empty() {
        observations.undecoded(
            format!("{} signals of unknown meaning", unknown_signals.len()),
            &[],
            "returned as signal_<guid> without a unit",
        );
    }
    observations
}

fn trios_vendor(
    hdr: &[(String, String)],
    generation: u8,
    stress_constant: Option<f64>,
    inertia: Option<f64>,
) -> Map<String, serde_json::Value> {
    let mut vendor = Map::new();
    for (k, v) in hdr {
        vendor.insert(k.clone(), json!(v));
    }
    vendor.insert("generation".into(), json!(generation));
    if let Some(kt) = stress_constant {
        vendor.insert("stress_constant".into(), json!(kt));
    }
    if let Some(i) = inertia {
        vendor.insert("instrument_and_geometry_inertia".into(), json!(i));
    }
    vendor
}

fn trios_notes(traces: &[SeriesTrace], computed_any: bool) -> Vec<String> {
    let mut notes = vec![
        "values are the stored signals in SI units (temperature in °C); signals of unknown meaning are signal_<guid> without a unit".to_string(),
    ];
    if traces
        .iter()
        .any(|t| t.channels.iter().any(|c| c.name == "heat_flow"))
    {
        notes.push("heat_flow is the stored heat flow in W; the TRIOS exports of the development files show it with the opposite sign (their display setting is Exo Up) and divided by the sample size".into());
    }
    if computed_any {
        notes.push("storage_modulus, loss_modulus, tan_delta and complex_viscosity are computed (not stored), as TRIOS computes them for a parallel plate; they agree with TRIOS exports to 4e-4".into());
    } else if traces
        .iter()
        .any(|t| t.channels.iter().any(|c| c.name == "oscillation_torque"))
    {
        notes.push("oscillation moduli are not stored and are not computed for this geometry (only parallel plates were compared with TRIOS)".into());
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_and_ticks() {
        let g = guid(b"f79c919e-6fa6-4856-92f8-6e5868f792f5");
        assert_eq!(
            g,
            [
                0x9e, 0x91, 0x9c, 0xf7, 0xa6, 0x6f, 0x56, 0x48, 0x92, 0xf8, 0x6e, 0x58, 0x68, 0xf7,
                0x92, 0xf5
            ]
        );
        assert_eq!(guid_text(&g), "f79c919e-6fa6-4856-92f8-6e5868f792f5");
        assert_eq!(
            ticks_to_iso(638_654_504_517_738_017).as_deref(),
            Some("2024-10-25T10:54:11.773")
        );
        assert_eq!(ticks_to_iso(0), None);
    }

    #[test]
    fn strings_and_header_sniff() {
        assert_eq!(string_at(b"\x03abcd", 0), Some(("abc".to_string(), 4)));
        assert_eq!(string_at(b"\x05ab", 0), None);
        assert_eq!(string_at(b"\xff\xff\xff\xff\xff", 0), None);
        let mut head = vec![0u8; 25];
        head[1] = 0x25;
        head[7..10].copy_from_slice(&[8, 0x25, 2]);
        head[14..17].copy_from_slice(&[0x14, 0x20, 1]);
        head.extend_from_slice(b"\x0einstrumenttype\x05DSC25");
        assert!(looks_like(&head));
        head[1] = 0;
        assert!(!looks_like(&head));
    }

    #[test]
    fn moduli_of_a_known_point() {
        // first point of the development frequency sweep (12 mm plate, gap 2.1 mm)
        let w = vec![std::f64::consts::TAU];
        let m = vec![0.763_157e-6];
        let p = vec![0.062_366_f64.to_radians()];
        let th = vec![3.493_51e-4];
        let gap = vec![0.0021];
        let cols: BTreeMap<&str, &Vec<f64>> = [
            ("angular_frequency", &w),
            ("oscillation_torque", &m),
            ("raw_phase", &p),
            ("oscillation_displacement", &th),
            ("gap", &gap),
        ]
        .into_iter()
        .collect();
        let [g1, g2, _, _] = moduli(&cols, 2_947_313.759_259_259_3, 2.289_039e-5, 1000.0).unwrap();
        // TRIOS: 3185.72 Pa and 2.45288 Pa
        assert!((g1[0] - 3185.72).abs() / 3185.72 < 1e-3, "{}", g1[0]);
        assert!((g2[0] - 2.452_88).abs() / 2.452_88 < 1e-3, "{}", g2[0]);
    }
}
