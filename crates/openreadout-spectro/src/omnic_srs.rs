//! Thermo Fisher OMNIC `.srs` series (rapid scan, high-speed real time, GC-IR, TGA-IR): the `.spa`
//! file header with a table of 22-byte key records; the series spectra in one record (key 301),
//! the background spectra in the set-0 records.
//!
//! Layout and vocabulary: `docs/formats/thermo-omnic.md`; provenance:
//! `docs/provenance/thermo-omnic.md`.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::json;

use crate::OMNIC_FORMAT_ID as FMT;
use crate::common::{
    Facts, Le, Parsed, Rows, SpectrumSet, SpectrumTable, Stored, XValues, num, read_at, text_field,
};

/// Most key records read.
const MAX_KEYS: usize = 1 << 16;
/// Bytes of one key record.
const RECORD: usize = 22;
/// Bytes before the first spectrum in the series record: its spectrum header (140) and
/// acquisition block (56).
const SERIES_HEAD: u64 = 196;
/// Bytes before each spectrum's values: 16 (u32, time in 1/100 s) and the 84-byte name record.
const PER_SPECTRUM: u64 = 100;

#[derive(Debug, Clone, Copy)]
struct Key {
    key: u16,
    offset: u64,
    length: u64,
    set: u32,
    index: u32,
}

/// The parts of a spectrum header (the `.spa` key-2 layout) a series needs.
#[derive(Debug, Clone, Copy)]
struct Head {
    points: u32,
    x_code: u8,
    y_code: u8,
    end_a: f32,
    end_b: f32,
    scans: u32,
    background_scans: u32,
    laser: f32,
}

fn head(b: &[u8]) -> Option<Head> {
    Some(Head {
        points: b.u32_at(4)?,
        x_code: b.u8_at(8)?,
        y_code: b.u8_at(12)?,
        end_a: b.f32_at(16)?,
        end_b: b.f32_at(20)?,
        scans: b.u32_at(36).unwrap_or(0),
        background_scans: b.u32_at(52).unwrap_or(0),
        laser: b.f32_at(80).unwrap_or(f32::NAN),
    })
}

/// x from the smaller axis end to the larger: series values are stored in ascending x.
fn ascending(h: &Head) -> XValues {
    let (a, b) = (f64::from(h.end_a), f64::from(h.end_b));
    XValues::Regular {
        first: a.min(b),
        last: a.max(b),
    }
}

/// The series information record (key 325): title and times in minutes.
#[derive(Debug, Default)]
struct SeriesInfo {
    title: String,
    first_min: Option<f32>,
    last_min: Option<f32>,
    step_min: Option<f32>,
    count: Option<u32>,
}

impl SeriesInfo {
    fn of(b: &[u8]) -> Self {
        SeriesInfo {
            title: b
                .bytes_at(2, 256.min(b.len().saturating_sub(2)))
                .map(text_field)
                .unwrap_or_default(),
            first_min: b.f32_at(66),
            last_min: b.f32_at(70),
            step_min: b.f32_at(74),
            count: b.u32_at(90),
        }
    }
}

/// The series record (key 301): the spectrum header shared by all spectra, then per
/// spectrum a 100-byte record and the values.
struct Series {
    key: Key,
    head: Head,
    points: u64,
    /// Bytes per spectrum: the record before it and its values.
    stride: u64,
    count: u64,
}

impl Series {
    fn find(f: &SourceFile, path: &Path, file_len: u64, keys: &[Key]) -> Result<Self> {
        let key =
            keys.iter().find(|k| k.key == 301).copied().ok_or_else(|| {
                Error::corrupt(FMT, "no series record (key 301) in the key table")
            })?;
        let sb = read_at(f, path, key.offset, 140, file_len)?;
        let head =
            head(&sb).ok_or_else(|| Error::corrupt(FMT, "the series record's header is short"))?;
        let points = u64::from(head.points);
        let stride = PER_SPECTRUM + points * 4;
        let body = key.length.saturating_sub(SERIES_HEAD);
        if points == 0 || key.length < SERIES_HEAD || !body.is_multiple_of(stride) {
            return Err(Error::corrupt(
                FMT,
                format!(
                    "the series record ({} bytes) is not {SERIES_HEAD} + n × {stride} bytes for {points}-point spectra",
                    key.length
                ),
            ));
        }
        Ok(Series {
            key,
            head,
            points,
            stride,
            count: body / stride,
        })
    }

    /// Per-spectrum times (minutes; stored in 1/100 s) and names, from the record before each
    /// spectrum (not read for more than a million spectra).
    fn times_and_names(
        &self,
        f: &SourceFile,
        path: &Path,
        file_len: u64,
    ) -> Result<(Vec<f64>, Vec<String>)> {
        let mut times = Vec::new();
        let mut names = Vec::new();
        if self.count <= 1_000_000 {
            for k in 0..self.count {
                let at = self.key.offset + SERIES_HEAD + k * self.stride;
                let b = read_at(f, path, at, PER_SPECTRUM, file_len)?;
                times.push(b.u32_at(4).map_or(f64::NAN, |t| f64::from(t) / 6000.0));
                names.push(text_field(b.bytes_at(16, 84).unwrap_or_default()));
            }
        }
        Ok((times, names))
    }
}

/// Parse an `.srs` series.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Parsed> {
    let top = read_at(f, path, 0, 304, file_len)?;
    if top.len() < 304 {
        return Err(Error::corrupt(
            FMT,
            "file is shorter than the 304-byte OMNIC header",
        ));
    }
    let file_time = top.u32_at(296).unwrap_or(0);
    let mut parsed = Parsed::default();
    let keys = key_table(f, path, file_len, &top, &mut parsed)?;
    let small = |k: &Key, cap: u64| -> Result<Vec<u8>> {
        read_at(f, path, k.offset, k.length.min(cap), file_len)
    };
    let find = |key: u16, set: u32| keys.iter().find(|k| k.key == key && k.set == set);

    let series = Series::find(f, path, file_len, &keys)?;
    let info = find(325, 1)
        .map(|k| small(k, 4096))
        .transpose()?
        .map(|b| SeriesInfo::of(&b))
        .unwrap_or_default();
    if let Some(n) = info.count
        && u64::from(n) != series.count
    {
        parsed.findings.push(Finding::warning(
            "series_count",
            format!(
                "the series information says {n} spectra, the series record holds {}",
                series.count
            ),
        ));
    }
    let (times, names) = series.times_and_names(f, path, file_len)?;
    parsed.sets.push(series_set(&series, &info, &names));
    if !times.is_empty() {
        parsed.tables.push(SpectrumTable {
            name: "series".into(),
            trace: 0,
            columns: vec![
                (
                    "spectrum".into(),
                    None,
                    (0..times.len()).map(|i| i as f64).collect(),
                ),
                ("time_min".into(), Some("min".into()), times),
            ],
        });
    }
    push_backgrounds(&keys, &small, &mut parsed)?;

    let mut facts = Facts::default();
    Facts::text(&mut facts.vendor, "Thermo Fisher Scientific", "format");
    Facts::text(&mut facts.software, "OMNIC", "format");
    if !info.title.is_empty() {
        Facts::text(
            &mut facts.method_name,
            &info.title,
            "series information title (key 325)",
        );
    }
    if let Some(h) = find(27, 1).map(|k| small(k, 1 << 16)).transpose()? {
        let text = text_field(&h);
        if !text.is_empty() {
            Facts::text(&mut facts.comment, &text, "series history (key 27)");
        }
    }
    // the timestamp at 296 counts only when the series header repeats it (at +836 of the key-2
    // header, or +368 / +828 in TGA-IR and GC-IR series)
    if let Some(k2) = find(2, 1)
        && file_time != 0
    {
        let block = read_at(f, path, k2.offset, 1024, file_len)?;
        if [836usize, 368, 828]
            .iter()
            .any(|o| block.u32_at(*o) == Some(file_time))
            && let Some(t) = crate::omnic::omnic_time(file_time)
        {
            Facts::text(
                &mut facts.started_at,
                &t,
                "file timestamp (296), repeated in the series header",
            );
        }
    }
    let sh = &series.head;
    if sh.scans > 0 {
        facts.number(
            "scans",
            f64::from(sh.scans),
            None,
            "series spectrum header (+36)",
            Source::PriorArt,
        );
    }
    if sh.laser.is_finite() && sh.laser > 0.0 {
        facts.number(
            "laser_wavenumber",
            f64::from(sh.laser),
            Some("1/cm"),
            "series spectrum header (+80)",
            Source::PriorArt,
        );
    }
    parsed.facts = facts;
    parsed.notes.push("OMNIC series file (.srs)".into());
    let mut undecoded: Vec<u16> = keys
        .iter()
        .map(|k| k.key)
        .filter(|k| matches!(k, 110 | 111 | 112 | 130 | 146 | 300))
        .collect();
    undecoded.sort_unstable();
    undecoded.dedup();
    if !undecoded.is_empty() {
        parsed.notes.push(format!(
            "series records not decoded (profiles, processing and display settings): keys {}",
            undecoded
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    parsed.vendor = json!({
        "records": keys.iter().map(|k| json!({"key": k.key, "offset": k.offset, "length": k.length, "set": k.set, "index": k.index})).collect::<Vec<_>>(),
        "series": {"points": series.points, "spectra": series.count, "title": info.title,
                   "axis_ends": [num(f64::from(sh.end_a)), num(f64::from(sh.end_b))]},
        "timestamp_296": file_time,
    });
    for (k, v) in [
        ("traces[].sample_count", Source::PriorArt),
        ("traces[].sweep_count", Source::Inferred),
        ("traces[].extra.axis", Source::Inferred),
        ("tables[].columns", Source::Inferred),
    ] {
        parsed.provenance.insert(k.into(), v);
    }
    Ok(parsed)
}

/// The key records that lie inside the file (one `ls` entry each); a record that runs past
/// the end is a finding.
fn key_table(
    f: &SourceFile,
    path: &Path,
    file_len: u64,
    top: &[u8],
    parsed: &mut Parsed,
) -> Result<Vec<Key>> {
    let nkeys = usize::from(top.u16_at(294).unwrap_or(0)).min(MAX_KEYS);
    let table = read_at(f, path, 304, (nkeys * RECORD) as u64, file_len)?;
    if table.len() < nkeys * RECORD {
        parsed.findings.push(Finding::error(
            "truncated",
            format!("the key table of {nkeys} records runs past the end of the file"),
        ));
    }
    let mut keys = Vec::new();
    for i in 0..table.len() / RECORD {
        let at = i * RECORD;
        let k = Key {
            key: table.u16_at(at).unwrap_or(0),
            offset: table.u64_at(at + 2).unwrap_or(0),
            length: u64::from(table.u32_at(at + 10).unwrap_or(0)),
            set: table.u32_at(at + 14).unwrap_or(0),
            index: table.u32_at(at + 18).unwrap_or(0),
        };
        if k.offset.checked_add(k.length).is_none_or(|e| e > file_len) {
            parsed.findings.push(
                Finding::error(
                    "truncated",
                    format!(
                        "record {i} (key {}) at {} of {} bytes runs past the end of the file",
                        k.key, k.offset, k.length
                    ),
                )
                .at(k.offset),
            );
            continue;
        }
        keys.push(k);
        parsed.entries.push(LsEntry {
            kind: "record".into(),
            name: format!("key {}", k.key),
            offset: Some(k.offset),
            size: Some(k.length),
            image: None,
            details: json!({"key": k.key, "set": k.set, "index": k.index}),
        });
    }
    Ok(keys)
}

/// The JCAMP data type of a spectrum header.
fn data_type(h: &Head) -> &'static str {
    if h.x_code == 2 || h.y_code == 22 {
        "INFRARED INTERFEROGRAM"
    } else {
        "INFRARED SPECTRUM"
    }
}

/// The series spectra as one set.
fn series_set(series: &Series, info: &SeriesInfo, names: &[String]) -> SpectrumSet {
    let sh = &series.head;
    let (xq, xu) = crate::omnic::x_axis_of(sh.x_code);
    let (yq, yu) = crate::omnic::y_axis_of(sh.y_code);
    let mut extra = BTreeMap::new();
    if sh.scans > 0 {
        extra.insert("scans".into(), json!(sh.scans));
    }
    if sh.background_scans > 0 {
        extra.insert("background_scans".into(), json!(sh.background_scans));
    }
    if sh.laser.is_finite() && sh.laser > 0.0 {
        extra.insert("laser_wavenumber_cm1".into(), num(f64::from(sh.laser)));
    }
    extra.insert("x_units_code".into(), json!(sh.x_code));
    extra.insert("y_units_code".into(), json!(sh.y_code));
    if !info.title.is_empty() {
        extra.insert("series_title".into(), json!(info.title));
    }
    for (k, v) in [
        ("first_time_min", info.first_min),
        ("last_time_min", info.last_min),
        ("time_step_min", info.step_min),
    ] {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            extra.insert(k.into(), num(f64::from(v)));
        }
    }
    if let (Some(a), Some(b)) = (names.first(), names.last()) {
        extra.insert("spectrum_names".into(), json!([a, b]));
    }
    SpectrumSet {
        name: if info.title.is_empty() {
            "series".into()
        } else {
            info.title.clone()
        },
        x_quantity: xq,
        x_unit: xu.map(str::to_string),
        x: ascending(sh),
        y_name: yq.into(),
        y_unit: yu.map(str::to_string),
        points: series.points,
        count: u32::try_from(series.count).unwrap_or(u32::MAX),
        rows: Rows::Strided {
            first: series.key.offset + SERIES_HEAD + PER_SPECTRUM,
            stride: series.stride,
        },
        stored: Stored::F32,
        scale: 1.0,
        data_type: data_type(sh),
        extra,
    }
}

/// The background spectra: set-0 key-2 headers with their key-3 values, in index order.
fn push_backgrounds(
    keys: &[Key],
    small: &dyn Fn(&Key, u64) -> Result<Vec<u8>>,
    parsed: &mut Parsed,
) -> Result<()> {
    let mut bg_heads: Vec<&Key> = keys.iter().filter(|k| k.key == 2 && k.set == 0).collect();
    bg_heads.sort_by_key(|k| k.index);
    for kh in bg_heads {
        let Some(kd) = keys
            .iter()
            .find(|k| k.key == 3 && k.set == 0 && k.index == kh.index)
        else {
            continue;
        };
        let Some(h) = head(&small(kh, 140)?) else {
            continue;
        };
        let n = u64::from(h.points);
        if n == 0 || kd.length < n * 4 {
            parsed.findings.push(Finding::error(
                "short_data",
                format!(
                    "background {}: {} bytes for {n} points",
                    kh.index, kd.length
                ),
            ));
            continue;
        }
        let (bq, bu) = crate::omnic::x_axis_of(h.x_code);
        let (byq, byu) = crate::omnic::y_axis_of(h.y_code);
        let mut bex = BTreeMap::new();
        bex.insert("spectrum_role".into(), json!("background"));
        bex.insert("x_units_code".into(), json!(h.x_code));
        bex.insert("y_units_code".into(), json!(h.y_code));
        if let Some(t) = keys
            .iter()
            .find(|k| k.key == 107 && k.set == 0)
            .filter(|_| kh.index == 0)
            .map(|k| small(k, 264))
            .transpose()?
        {
            bex.insert(
                "title".into(),
                json!(text_field(t.bytes_at(0, 256).unwrap_or(&t))),
            );
        }
        parsed.sets.push(SpectrumSet {
            name: format!("background {}", kh.index),
            x_quantity: bq,
            x_unit: bu.map(str::to_string),
            x: ascending(&h),
            y_name: byq.into(),
            y_unit: byu.map(str::to_string),
            points: n,
            count: 1,
            rows: Rows::Listed(vec![kd.offset]),
            stored: Stored::F32,
            scale: 1.0,
            data_type: data_type(&h),
            extra: bex,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal series: header, key table (series info, series record), two 3-point spectra.
    fn file() -> Vec<u8> {
        let mut b = b"Spectral Exte File\r\n".to_vec();
        b.resize(304, 0);
        let n: u16 = 2;
        b[294..296].copy_from_slice(&n.to_le_bytes());
        let table_end = 304 + 2 * RECORD as u64;
        let info_at = table_end;
        let series_at = info_at + 264;
        let rec = PER_SPECTRUM + 12;
        let series_len = SERIES_HEAD + 2 * rec;
        for (key, off, len) in [
            (325u16, info_at, 264u32),
            (301, series_at, u32::try_from(series_len).unwrap()),
        ] {
            b.extend_from_slice(&key.to_le_bytes());
            b.extend_from_slice(&off.to_le_bytes());
            b.extend_from_slice(&len.to_le_bytes());
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&0u32.to_le_bytes());
        }
        let mut info = vec![0u8; 264];
        info[2..8].copy_from_slice(b"Series");
        info[90..94].copy_from_slice(&2u32.to_le_bytes());
        b.extend_from_slice(&info);
        let mut h = vec![0u8; 196];
        h[4..8].copy_from_slice(&3u32.to_le_bytes());
        h[8] = 1;
        h[12] = 17;
        h[16..20].copy_from_slice(&4000f32.to_le_bytes());
        h[20..24].copy_from_slice(&1000f32.to_le_bytes());
        b.extend_from_slice(&h);
        for (k, t) in [(0u32, 600u32), (1, 1200)] {
            let mut r = vec![0u8; 100];
            r[4..8].copy_from_slice(&t.to_le_bytes());
            r[16..21].copy_from_slice(b"spec ");
            b.extend_from_slice(&r);
            for v in 0..3u32 {
                b.extend_from_slice(&((k * 10 + v) as f32).to_le_bytes());
            }
        }
        b
    }

    #[test]
    fn series_layout() {
        let b = file();
        let src = openreadout_core::source::MemSource::new("t.srs", b.clone());
        let sf = SourceFile::new(std::sync::Arc::new(src));
        let p = parse(&sf, Path::new("t.srs"), b.len() as u64).unwrap();
        assert_eq!(p.sets[0].count, 2);
        assert_eq!(p.sets[0].points, 3);
        assert!(
            matches!(p.sets[0].x, XValues::Regular { first, last } if (first - 1000.0).abs() < 1e-9 && (last - 4000.0).abs() < 1e-9)
        );
        assert_eq!(p.tables[0].columns[1].2, vec![0.1, 0.2]);
        for cut in 0..b.len() {
            let src = openreadout_core::source::MemSource::new("t.srs", b[..cut].to_vec());
            let sf = SourceFile::new(std::sync::Arc::new(src));
            let _ = parse(&sf, Path::new("t.srs"), cut as u64);
        }
    }
}
