//! Agilent BioTek Gen5 experiment files (`.xpt`): a compound file (MS-CFB) whose
//! `SUBSETS/<n>/DATA` streams hold one plate each as an MFC archive (docs/formats/plate-readers.md,
//! "Gen5 experiment files"; provenance 2026-09-26).
//!
//! Gen5 3.x compresses each `DATA` stream (zlib after a `CAssayDoc` head giving both sizes);
//! Gen5 2.x stores it as is. In the archive, every read is a `CPlateDataSet` (name such as
//! `450`, `Lum`, `485,530`; an OLE date) followed by a record block: u32 24, u16 7, u32 reads,
//! u32 1, u32 rows, u32 columns, 3 × u32 1, then reads × rows × columns records of 24 bytes (f64
//! value, flag bytes, 15 bytes not interpreted). Kinetic blocks end with a table of read times
//! in milliseconds. Only this validated grammar is decoded; anything else is refused.

use std::io::Cursor as IoCursor;

use openreadout_core::bytes::{latin1, le_f64};
use openreadout_core::cfb::{CFB_MAGIC, Cfb};
use openreadout_core::model::Finding;
use openreadout_core::time::civil_from_days;
use serde_json::json;

use super::cursor::{Cursor, find};
use crate::model::{Block, Channel, Export, Kind, Mode, ReadType};
use crate::sheet::Container;

const DATASET_CLASS: &[u8] = b"\xff\xff\x00\x00\x0d\x00CPlateDataSet";
const PLATE_DESCR: &[u8] = b"\xff\xff\x00\x00\x0b\x00CPlateDescr";
const TEMPERATURE_CLASS: &[u8] = b"\xff\xff\x00\x00\x13\x00CTemperatureDataSet";
const RECORD_HEAD: &[u8] = b"\x18\x00\x00\x00\x07\x00";
const TEMPERATURE_HEAD: &[u8] = b"\x18\x00\x00\x00\x02\x00";
/// Largest inflated `DATA` stream (a 1536-well kinetic run of 1000 reads is 37 MB).
const MAX_DATA: usize = 128 << 20;
/// Most records per data set.
const MAX_RECORDS: u64 = 4_000_000;
const RECORD: usize = 24;

/// A compound file named `.xpt` (Gen5 experiment). Excel `.xls` files are compound files too:
/// the extension decides here, the streams on open.
pub(crate) fn sniff(head: &[u8], xpt_extension: bool) -> bool {
    xpt_extension && head.starts_with(&CFB_MAGIC)
}

/// A Gen5 protocol (`.prt`): an MFC archive starting with the template id; it holds no data.
pub(crate) fn is_protocol(head: &[u8]) -> bool {
    head.starts_with(b"\x10\x00\x11Gen5ExpTemplateID")
        || head.get(3..20) == Some(b"Gen5ExpTemplateID".as_slice())
}

/// An MFC `CString`: u8 length, or 0xFF then u16, or 0xFFFF then u32 (ANSI text).
fn cstring(c: &mut Cursor) -> Option<String> {
    let mut n = usize::from(c.u8()?);
    if n == 0xff {
        n = usize::from(c.u16_le()?);
        if n == 0xffff {
            n = usize::try_from(c.u32_le()?).ok()?;
        }
    }
    if n > 4096 {
        return None;
    }
    let b = c.take(n)?;
    Some(latin1(b))
}

/// OLE automation date (days since 1899-12-30, local time) as ISO-8601.
fn ole_date(d: f64) -> Option<String> {
    if !(20_000.0..80_000.0).contains(&d) {
        return None;
    }
    let secs = (d * 86_400.0).round() as i64;
    let days = secs.div_euclid(86_400) - 25_569; // to days since 1970-01-01
    let tod = secs.rem_euclid(86_400);
    let (y, m, day) = civil_from_days(days);
    Some(format!(
        "{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}",
        tod / 3600,
        tod % 3600 / 60,
        tod % 60
    ))
}

/// Seconds since 1970 (UTC) as ISO-8601 with `Z`.
fn unix_utc(secs: u32) -> Option<String> {
    // an OLE date is days since 1899-12-30; 25,569 days separate it from 1970-01-01
    let days = f64::from(secs) / 86_400.0 + 25_569.0;
    ole_date(days).map(|s| format!("{s}Z"))
}

/// One plate read.
#[derive(Debug, Clone)]
struct DataSet {
    name: String,
    date: Option<String>,
    reads: u32,
    rows: u32,
    cols: u32,
    /// Record offset in the archive.
    records: usize,
    /// Read times in ms (kinetic).
    times_ms: Vec<u32>,
}

/// The data set header at `p` (after a class declaration or a class tag): u16 version (6, 8),
/// u16 (2, 3), u16 1, name, f64 date.
fn header(data: &[u8], p: usize) -> Option<(String, Option<String>, usize)> {
    let mut c = Cursor::new(data, p);
    let ver = c.u16_le()?;
    let a = c.u16_le()?;
    let b = c.u16_le()?;
    if !matches!(ver, 6 | 8) || !matches!(a, 2 | 3) || b != 1 {
        return None;
    }
    let name = cstring(&mut c)?;
    if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
        return None;
    }
    let date = c.f64_le()?;
    Some((name, ole_date(date), c.pos))
}

/// Every data set header: the class declaration, then class tags `0x80nn` followed by a header.
fn headers(data: &[u8]) -> Vec<(String, Option<String>, usize)> {
    let mut out = Vec::new();
    let mut p = 0;
    if let Some(decl) = find(data, DATASET_CLASS, 0, data.len()) {
        if let Some(h) = header(data, decl + DATASET_CLASS.len()) {
            out.push(h);
        }
        p = decl + DATASET_CLASS.len();
    }
    // Later reads refer to the class by a tag `0x80nn` (classes and objects are numbered in
    // order of appearance). Any such tag followed by a header with a valid date is taken; the
    // record block that must follow is checked separately.
    while p + 2 < data.len() {
        if data[p + 1] == 0x80
            && data[p] != 0
            && let Some(h) = header(data, p + 2)
            && h.1.is_some()
            && out.last().is_none_or(|l| l.2 <= p)
        {
            p = h.2;
            out.push(h);
            continue;
        }
        p += 1;
    }
    out
}

/// The record block of a data set whose header ends at `from` (before `to`).
fn block(
    data: &[u8],
    name: String,
    date: Option<String>,
    from: usize,
    to: usize,
) -> Result<DataSet, String> {
    let at = find(data, RECORD_HEAD, from, to.min(from.saturating_add(8192)))
        .ok_or_else(|| format!("read {name:?}: no record block"))?;
    let mut cur = Cursor::new(data, at + RECORD_HEAD.len());
    let next_u32 = |cur: &mut Cursor| cur.u32_le();
    let (Some(reads), Some(one), Some(rows), Some(cols), Some(x1), Some(x2), Some(x3)) = (
        next_u32(&mut cur),
        next_u32(&mut cur),
        next_u32(&mut cur),
        next_u32(&mut cur),
        next_u32(&mut cur),
        next_u32(&mut cur),
        next_u32(&mut cur),
    ) else {
        return Err(format!("read {name:?}: record block cut short"));
    };
    if one != 1 || (x1, x2, x3) != (1, 1, 1) || rows == 0 || cols == 0 || rows > 32 || cols > 48 {
        return Err(format!(
            "read {name:?}: record block layout not validated ({reads} reads, {rows} x {cols}, {one}/{x1}/{x2}/{x3})"
        ));
    }
    let count = u64::from(reads) * u64::from(rows) * u64::from(cols);
    if reads == 0 || count > MAX_RECORDS {
        return Err(format!("read {name:?}: {reads} reads"));
    }
    let records = cur.pos;
    let bytes = usize::try_from(count)
        .ok()
        .and_then(|count| count.checked_mul(RECORD));
    let end = bytes
        .and_then(|len| records.checked_add(len))
        .filter(|&e| e <= data.len())
        .ok_or_else(|| format!("read {name:?}: records cut short"))?;
    let mut cur = Cursor::new(data, end);
    let mut times_ms = Vec::new();
    if reads > 1 {
        // u16 0, u16 layout (1, 2), u32 reads, [layout 2: u32], reads × (u32 0, u32 ms, u64 0)
        let bad = || format!("read {name:?}: kinetic time table not in a validated layout");
        if cur.u16_le() != Some(0) {
            return Err(bad());
        }
        let layout = cur.u16_le().ok_or_else(bad)?;
        if cur.u32_le() != Some(reads) || !matches!(layout, 1 | 2) {
            return Err(bad());
        }
        if layout == 2 {
            cur.skip(4).ok_or_else(bad)?;
        }
        for _ in 0..reads {
            let (Some(lead), Some(ms), Some(tail)) = (cur.u32_le(), cur.u32_le(), cur.u64_le())
            else {
                return Err(bad());
            };
            if lead != 0 || tail != 0 || times_ms.last().is_some_and(|&l| ms <= l) {
                return Err(bad());
            }
            times_ms.push(ms);
        }
    }
    Ok(DataSet {
        name,
        date,
        reads,
        rows,
        cols,
        records,
        times_ms,
    })
}

/// Mode, channel label and wavelengths from a data set name (`450`, `Lum`, `485,530`), by the
/// rules Gen5 text exports follow for their read labels.
fn channel(name: &str) -> (Channel, bool) {
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    if digits(name.trim()) {
        let mut ch = Channel::derived(
            name.trim(),
            Mode::Absorbance,
            crate::model::ModeBasis::Label,
        );
        ch.wavelength_nm = name.trim().parse().ok();
        ch.unit = Some("OD".into());
        return (ch, true);
    }
    if name.trim().to_ascii_lowercase().starts_with("lum") {
        let mut ch = Channel::derived(
            name.trim(),
            Mode::Luminescence,
            crate::model::ModeBasis::Label,
        );
        ch.unit = Some("RLU".into());
        return (ch, true);
    }
    if let Some((ex, em)) = name.split_once(',')
        && digits(ex.trim())
        && digits(em.trim())
    {
        let mut ch = Channel::derived(
            name.trim(),
            Mode::Fluorescence,
            crate::model::ModeBasis::Label,
        );
        ch.excitation_nm = ex.trim().parse().ok();
        ch.emission_nm = em.trim().parse().ok();
        ch.unit = Some("RFU".into());
        return (ch, true);
    }
    (
        Channel::derived(name.trim(), Mode::Unknown, crate::model::ModeBasis::Label),
        false,
    )
}

/// Reader model, serial number and Gen5 version from `CPlateDescr`.
fn plate_descr(data: &[u8]) -> Option<Vec<String>> {
    let at = find(data, PLATE_DESCR, 0, data.len())?;
    let mut c = Cursor::new(data, at + PLATE_DESCR.len());
    c.skip(5)?;
    let mut out = Vec::new();
    for _ in 0..5 {
        out.push(cstring(&mut c)?);
    }
    Some(out)
}

/// The single temperature of an endpoint plate (`CTemperatureDataSet`, one record).
fn temperature(data: &[u8]) -> Option<f64> {
    let at = find(data, TEMPERATURE_CLASS, 0, data.len())?;
    let h = find(data, TEMPERATURE_HEAD, at, at + 1024)?;
    let mut c = Cursor::new(data, h + TEMPERATURE_HEAD.len());
    let n = c.u32_le()?;
    let one = c.u32_le()?;
    let t = c.f64_le()?;
    let flag = c.u8()?;
    (n == 1 && one == 1 && flag == 0 && (-50.0..150.0).contains(&t)).then_some(t)
}

/// The archive of a `DATA` stream: inflated (Gen5 3.x) or as stored (2.x).
fn archive(raw: &[u8]) -> Result<Vec<u8>, String> {
    const HEAD: &[u8] = b"\xff\xff\x00\x00\x09\x00CAssayDoc";
    if !raw.starts_with(HEAD) {
        return Err("DATA stream does not start with the assay document".into());
    }
    let mut c = Cursor::new(raw, HEAD.len());
    let _version = c.u16_le().ok_or("DATA stream cut short")?;
    // Gen5 3.x (document versions 9, 11): u64 size, u32 size, u32 packed size, then a zlib
    // stream; Gen5 2.x (version 6): the archive continues as stored
    let zlib_at = c.pos + 16;
    let zlib = raw
        .get(zlib_at..zlib_at + 2)
        .is_some_and(|h| h[0] == 0x78 && (u16::from(h[0]) << 8 | u16::from(h[1])) % 31 == 0);
    if !zlib {
        return Ok(raw.to_vec());
    }
    let size = c.u64_le().ok_or("DATA stream cut short")?;
    let size2 = c.u32_le().ok_or("DATA stream cut short")?;
    let _packed = c.u32_le().ok_or("DATA stream cut short")?;
    if size != u64::from(size2) || size > MAX_DATA as u64 {
        return Err(format!("DATA stream sizes disagree ({size}, {size2})"));
    }
    let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(
        raw.get(c.pos..).unwrap_or(&[]),
        MAX_DATA,
    )
    .map_err(|e| format!("DATA stream does not inflate: {e:?}"))?;
    if out.len() as u64 != size {
        return Err(format!(
            "DATA stream inflates to {} bytes, its head says {size}",
            out.len()
        ));
    }
    Ok(out)
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Export, String> {
    let mut io = IoCursor::new(bytes);
    let cfb = Cfb::open(
        &mut io,
        std::path::Path::new("experiment.xpt"),
        crate::FORMAT_ID,
    )
    .map_err(|e| e.to_string())?;
    let read = |io: &mut IoCursor<&[u8]>, path: &str| -> Option<Vec<u8>> {
        let e = cfb.stream(path)?;
        cfb.read(io, std::path::Path::new(path), e, MAX_DATA as u64)
            .ok()
    };
    let contents = read(&mut io, "Contents").ok_or("no Contents stream: not a Gen5 experiment")?;
    if find(&contents, b"Gen5ExperimentID", 0, 64).is_none() {
        return Err("Contents stream is not a Gen5 experiment".into());
    }
    let mut ex = Export::new(
        Kind::Gen5,
        Container::Binary {
            kind: "gen5-experiment",
        },
    );
    ex.put("document", "Gen5 experiment file");
    let mut subsets: Vec<(u32, String)> = cfb
        .entries
        .iter()
        .filter(|e| e.is_stream)
        .filter_map(|e| {
            let rest = e.path.strip_prefix("SUBSETS/")?;
            let (n, s) = rest.split_once('/')?;
            (s.eq_ignore_ascii_case("DATA")).then(|| (n.parse().ok(), n.to_string()))
        })
        .filter_map(|(n, s)| n.map(|n| (n, s)))
        .collect();
    subsets.sort();
    let mut refused = Vec::new();
    for (n, dir) in subsets {
        let Some(raw) = read(&mut io, &format!("SUBSETS/{dir}/DATA")) else {
            refused.push(format!("plate {n}: DATA stream unreadable"));
            continue;
        };
        // HEADER: `CPlateInfo`, u16, the plate name, 6 bytes, `0A 00 00 80`, u32 Unix time (UTC)
        let (plate_name, plate_utc) = read(&mut io, &format!("SUBSETS/{dir}/HEADER"))
            .and_then(|h| {
                let at = find(&h, b"CPlateInfo", 0, h.len())?;
                let mut c = Cursor::new(&h, at + 10);
                c.skip(2)?;
                let name = cstring(&mut c)?;
                c.skip(6)?;
                let utc = (c.expect(&[0x0a, 0, 0, 0x80]).is_some())
                    .then(|| c.u32_le())
                    .flatten()
                    .and_then(unix_utc);
                Some((name, utc))
            })
            .unwrap_or_default();
        let plate_name = if plate_name.is_empty() {
            format!("Plate {n}")
        } else {
            plate_name
        };
        let data = match archive(&raw) {
            Ok(d) => d,
            Err(e) => {
                refused.push(format!("{plate_name}: {e}"));
                continue;
            }
        };
        if let Some(descr) = plate_descr(&data) {
            // model, serial, (not interpreted), reader firmware, Gen5 version
            if ex.model.is_none() && !descr[0].is_empty() {
                ex.model = Some(descr[0].clone());
            }
            if ex.serial.is_none()
                && !descr[1].is_empty()
                && !descr[1].eq_ignore_ascii_case("unknown")
            {
                ex.serial = Some(descr[1].clone());
            }
            if ex.software_version.is_none() && !descr[4].is_empty() {
                ex.software_version = Some(descr[4].clone());
            }
        }
        let hs = headers(&data);
        if hs.is_empty() {
            refused.push(format!("{plate_name}: no read (CPlateDataSet) found"));
            continue;
        }
        let mut plate = Block::new(plate_name.clone(), plate_name.clone());
        plate.extra.insert("plate_number".into(), json!(n));
        if let Some(time) = plate_utc {
            plate.extra.insert("plate_time_utc".into(), json!(time));
        }
        let mut any = false;
        let mut kinetic = false;
        let mut used: Vec<usize> = Vec::new();
        for (i, (name, date, end)) in hs.iter().enumerate() {
            let to = hs.get(i + 1).map_or(data.len(), |h| h.2);
            let ds = match block(&data, name.clone(), date.clone(), *end, to) {
                Ok(ds) => ds,
                Err(e) => {
                    refused.push(format!("{plate_name}: {e}"));
                    continue;
                }
            };
            if plate.rows != 0 && (plate.rows, plate.cols) != (ds.rows, ds.cols) {
                refused.push(format!(
                    "{plate_name}: read {:?} has another plate size",
                    ds.name
                ));
                continue;
            }
            plate.rows = ds.rows;
            plate.cols = ds.cols;
            plate.declared_wells = Some(ds.rows * ds.cols);
            used.push(ds.records);
            let (ch, known) = channel(&ds.name);
            if !known {
                plate.findings.push(Finding::warning(
                    "unknown_read_mode",
                    format!(
                        "{plate_name}: read {:?}: the detection mode is not known from its name",
                        ds.name
                    ),
                ));
            }
            let chi = plate.channel(ch);
            if let Some(d) = &ds.date
                && plate.started_at.is_none()
            {
                plate.started_at = Some(d.clone());
            }
            let per_read = (ds.rows * ds.cols) as usize;
            let mut flagged = 0usize;
            for read_index in 0..ds.reads as usize {
                let time = (ds.reads > 1).then(|| f64::from(ds.times_ms[read_index]) / 1000.0);
                for well in 0..per_read {
                    let at = ds.records + (read_index * per_read + well) * RECORD;
                    let (Some(value), Some(fl)) = (le_f64(&data, at), data.get(at + 8..at + 12))
                    else {
                        continue;
                    };
                    let (row, col) = (
                        (well / ds.cols as usize) as u32,
                        (well % ds.cols as usize) as u32,
                    );
                    match (fl[0], fl[3]) {
                        (0, _) if value.is_finite() => {
                            plate.push_value(row, col, chi, time, value, None);
                        }
                        (1, 3) => {} // well not in the read region
                        (a, d) => {
                            flagged += 1;
                            plate.push_value(
                                row,
                                col,
                                chi,
                                time,
                                f64::NAN,
                                Some(format!("flagged ({a}/{d})")),
                            );
                        }
                    }
                }
            }
            if flagged > 0 {
                plate.findings.push(Finding::warning(
                    "flagged_values",
                    format!(
                        "{plate_name}: read {:?}: {flagged} values carry a flag whose meaning is not established (Gen5 prints such cells as text, e.g. OVRFLW); they are NaN",
                        ds.name
                    ),
                ));
            }
            if ds.reads > 1 {
                kinetic = true;
                plate.extra.insert("kinetic_reads".into(), json!(ds.reads));
            }
            any = true;
        }
        // record blocks that no read header announces (a Gen5 2.x plate held one) are listed
        let mut at = 0;
        while let Some(p) = find(&data, RECORD_HEAD, at, data.len()) {
            at = p + RECORD_HEAD.len();
            let records = p + RECORD_HEAD.len() + 28;
            if !used.contains(&records) {
                refused.push(format!(
                    "{plate_name}: a record block at byte {p} of the plate archive has no read header"
                ));
            }
        }
        if !any {
            continue;
        }
        plate.read_type = Some(if kinetic {
            ReadType::Kinetic
        } else {
            ReadType::Endpoint
        });
        if !kinetic {
            plate.temperature_c = temperature(&data);
        }
        ex.blocks.push(plate);
    }
    if !refused.is_empty() {
        ex.sections.insert("refused_reads".into(), json!(refused));
        ex.findings.push(Finding::warning(
            "read_not_decoded",
            format!(
                "{} read(s) are not decoded because their layout has not been validated: {}; export them as text from Gen5",
                refused.len(),
                refused.join("; ")
            ),
        ));
    }
    if let Some(time) = ex.blocks.iter().find_map(|plate| plate.started_at.clone()) {
        ex.acquired_at = Some(time.clone());
        ex.acquired_raw = Some(time);
    }
    Ok(ex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_names() {
        // 2022-05-05 17:53:00 (a Gen5 3.04 file's read time)
        assert_eq!(
            ole_date(44_686.745_138_888_89).as_deref(),
            Some("2022-05-05T17:53:00")
        );
        assert_eq!(channel("450").0.wavelength_nm, Some(450.0));
        assert_eq!(channel("Lum").0.mode, Mode::Luminescence);
        let (fl, known) = channel("485,530");
        assert!(known);
        assert_eq!(
            (fl.excitation_nm, fl.emission_nm),
            (Some(485.0), Some(530.0))
        );
        assert!(!channel("Blank 450").1);
    }

    /// A 2 x 3 plate with one read: header, record block, values and flags.
    fn archive_bytes(reads: u32) -> Vec<u8> {
        let mut d = DATASET_CLASS.to_vec();
        d.extend_from_slice(&[8, 0, 3, 0, 1, 0, 3]);
        d.extend_from_slice(b"450");
        d.extend_from_slice(&44_686.745_138_888_89f64.to_le_bytes());
        d.extend_from_slice(&[0; 40]);
        d.extend_from_slice(RECORD_HEAD);
        for v in [reads, 1, 2, 3, 1, 1, 1] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        for k in 0..reads {
            for w in 0..6u32 {
                d.extend_from_slice(&(f64::from(w + 10 * k) / 10.0).to_le_bytes());
                let flag: [u8; 4] = if w == 5 { [1, 0, 0, 3] } else { [0; 4] };
                d.extend_from_slice(&flag);
                d.extend_from_slice(&[0; 12]);
            }
        }
        d.extend_from_slice(&[0, 0]);
        if reads > 1 {
            d.extend_from_slice(&[1, 0]);
            d.extend_from_slice(&reads.to_le_bytes());
            for k in 0..reads {
                d.extend_from_slice(&0u32.to_le_bytes());
                d.extend_from_slice(&(15_000 + 60_000 * k).to_le_bytes());
                d.extend_from_slice(&0u64.to_le_bytes());
            }
        }
        d
    }

    #[test]
    fn endpoint_and_kinetic_blocks() {
        let d = archive_bytes(1);
        let hs = headers(&d);
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].1.as_deref(), Some("2022-05-05T17:53:00"));
        let ds = block(&d, hs[0].0.clone(), None, hs[0].2, d.len()).unwrap();
        assert_eq!((ds.reads, ds.rows, ds.cols), (1, 2, 3));
        let d = archive_bytes(3);
        let hs = headers(&d);
        let ds = block(&d, hs[0].0.clone(), None, hs[0].2, d.len()).unwrap();
        assert_eq!(ds.times_ms, vec![15_000, 75_000, 135_000]);
        // any cut of the archive is refused or read, never a panic
        for cut in 0..d.len() {
            let hs = headers(&d[..cut]);
            if let Some(h) = hs.first() {
                let _ = block(&d[..cut], h.0.clone(), None, h.2, cut);
            }
        }
    }
}
