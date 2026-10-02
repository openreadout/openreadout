//! Malvern Zetasizer `.dts` measurement files: a compound file with one `REC<n>` stream per
//! record. Each record's header (kind, number, software version, date, instrument serial,
//! temperature, sample name) is walked; the size results (Z-average, PdI, peaks) and zeta results
//! (zeta potential, mobility, conductivity, peaks) are located by their stored structure, and
//! returned only where exactly one block of that structure is found. Notes:
//! `docs/formats/malvern-zetasizer.md`.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::Path;

use openreadout_core::assurance::{FeatureKind, Observations, Scope};
use openreadout_core::bytes::{Endian, le_f32, le_f64, le_u16, le_u32, utf16_units};
use openreadout_core::cfb::{CFB_MAGIC, Cfb};
use openreadout_core::experiment::MeasurementKind;
use openreadout_core::jet::ole_date_iso;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::series::{Facts, SeriesColumn, SeriesFile, SeriesTable};
use openreadout_core::{Error, Result};
use serde_json::{Map, Value, json};

/// Format id of Malvern Zetasizer `.dts` files.
pub const FORMAT_ID: &str = "malvern-zetasizer-dts";

/// Largest file read.
pub(crate) const MAX_BYTES: u64 = 1 << 30;

/// The identifier after the first `u16` of the `Header` stream.
const HEADER_ID: [u8; 16] = [
    0xc0, 0xfe, 0xe4, 0xa2, 0xb2, 0x26, 0xd6, 0x11, 0x99, 0x1b, 0x00, 0x90, 0x27, 0x9b, 0x77, 0x0c,
];

/// First and last bytes of the 46-byte block after the material name.
const MATERIAL_HEAD: [u8; 6] = [1, 0, 0, 0, 1, 0];
const MATERIAL_TAIL: [u8; 14] = [2, 0, 0, 0, 1, 0, 0, 2, 0, 0, 0, 1, 0, 0];
const MATERIAL_LEN: usize = 46;

/// Size results (Z-average, PdI, peaks) are returned only once validated against a Zetasizer
/// export of size records in the development corpus; none is there yet, so they are withheld.
const SIZE_RESULTS: bool = false;

/// Longest string accepted, in bytes.
const MAX_STRING: usize = 4096;
/// How far into a record the sample name is looked for.
const NAME_WINDOW: usize = 16_384;

/// A string at `o`: `u32` byte length, `0x01`, UTF-16LE ending in NUL. Returns it and its end.
fn string_at(b: &[u8], o: usize) -> Option<(String, usize)> {
    let n = le_u32(b, o)? as usize;
    if !(2..=MAX_STRING).contains(&n) || !n.is_multiple_of(2) || *b.get(o + 4)? != 1 {
        return None;
    }
    let raw = b.get(o + 5..o + 5 + n)?;
    if raw[n - 2..] != [0, 0] {
        return None;
    }
    let s = char::decode_utf16(utf16_units(&raw[..n - 2], Endian::Little))
        .collect::<std::result::Result<String, _>>()
        .ok()?;
    s.chars().all(|c| !c.is_control()).then_some((s, o + 5 + n))
}

/// Peaks of one distribution: means, areas (%), widths.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Peaks {
    pub mean: Vec<f64>,
    pub area: Vec<f64>,
    pub width: Vec<f64>,
}

/// Size results.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Size {
    pub z_average: f64,
    pub pdi: f64,
    /// Intensity, number, volume.
    pub peaks: [Peaks; 3],
}

/// Zeta results.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Zeta {
    pub zeta: f64,
    pub zeta_deviation: f64,
    pub mobility: f64,
    pub mobility_deviation: f64,
    pub conductivity: f64,
    pub voltage: f64,
    pub zeta_peaks: Peaks,
    pub mobility_peaks: Peaks,
}

/// One record.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Record {
    pub stream: String,
    pub schema: u16,
    pub kind: u16,
    pub number: u32,
    pub version: String,
    pub measured_at: Option<String>,
    pub serial: String,
    pub temperature: f64,
    pub sample: Option<String>,
    pub size: Option<Size>,
    pub zeta: Option<Zeta>,
    /// Structure matches found (when not exactly one, no results are returned).
    pub matches: usize,
}

fn kind_name(k: u16) -> &'static str {
    match k {
        1 => "size",
        2 => "zeta",
        _ => "other",
    }
}

/// `u32 k` + `k` f32 at `o` (k ≤ `max`): the values and the end.
fn counted(b: &[u8], o: usize, max: usize) -> Option<(Vec<f64>, usize)> {
    let k = le_u32(b, o)? as usize;
    if k > max {
        return None;
    }
    let v = (0..k)
        .map(|i| le_f32(b, o + 4 + 4 * i).map(f64::from))
        .collect::<Option<Vec<f64>>>()?;
    Some((v, o + 4 + 4 * k))
}

fn peaks_at(b: &[u8], o: usize) -> Option<(Peaks, usize)> {
    let (mean, o) = counted(b, o, 10)?;
    let (area, o) = counted(b, o, 10)?;
    let (width, o) = counted(b, o, 10)?;
    (mean.len() == area.len() && area.len() == width.len())
        .then_some((Peaks { mean, area, width }, o))
}

fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// The size block at `o`, if its structure holds.
fn size_at(b: &[u8], o: usize) -> Option<Size> {
    let z_average = le_f32(b, o).map(f64::from)?;
    let pdi = le_f32(b, o + 4).map(f64::from)?;
    if !(z_average > 0.1 && z_average < 1e5 && (0.0..5.0).contains(&pdi)) {
        return None;
    }
    let count = le_u32(b, o + 8)? as usize;
    if !(10..=400).contains(&count) {
        return None;
    }
    for i in 0..count {
        let x = le_f32(b, o + 12 + 4 * i).map(f64::from)?;
        if !(x > -0.5 && x < 2.0) {
            return None;
        }
    }
    let mut at = o + 12 + 4 * count;
    let mut groups: Vec<Peaks> = Vec::with_capacity(3);
    for _ in 0..3 {
        let (pk, end) = peaks_at(b, at)?;
        if !finite(&pk.mean) || !finite(&pk.area) || !finite(&pk.width) {
            return None;
        }
        let total: f64 = pk.area.iter().sum();
        if !pk.area.is_empty() && (total - 100.0).abs() > 0.5 {
            return None;
        }
        groups.push(pk);
        at = end;
    }
    let peaks: [Peaks; 3] = groups.try_into().ok()?;
    Some(Size {
        z_average,
        pdi,
        peaks,
    })
}

fn five(b: &[u8], o: usize) -> Option<Vec<f64>> {
    if le_u32(b, o)? != 5 {
        return None;
    }
    let v = (0..5)
        .map(|i| le_f32(b, o + 4 + 4 * i).map(f64::from))
        .collect::<Option<Vec<f64>>>()?;
    finite(&v).then_some(v)
}

/// The zeta block at `o`, if its structure holds.
fn zeta_at(b: &[u8], o: usize) -> Option<Zeta> {
    let areas = five(b, o)?;
    if areas.iter().any(|a| *a < 0.0)
        || areas.iter().sum::<f64>() > 100.5
        || areas.windows(2).any(|w| w[0] < w[1])
    {
        return None;
    }
    let means = five(b, o + 24)?;
    let conductivity = le_f64(b, o + 48)?;
    if !(0.0..1000.0).contains(&conductivity) {
        return None;
    }
    let tail = (0..10)
        .map(|i| le_f32(b, o + 56 + 4 * i).map(f64::from))
        .collect::<Option<Vec<f64>>>()?;
    if !finite(&tail) {
        return None;
    }
    let widths = five(b, o + 96)?;
    let m_areas = five(b, o + 120)?;
    let m_means = five(b, o + 144)?;
    let m_widths = five(b, o + 168)?;
    let used = |a: &[f64]| a.iter().take_while(|x| **x > 0.0).count();
    let k = used(&areas);
    let km = used(&m_areas);
    Some(Zeta {
        voltage: tail[0],
        zeta: tail[1],
        zeta_deviation: tail[2],
        mobility: tail[3],
        mobility_deviation: tail[4],
        conductivity,
        zeta_peaks: Peaks {
            mean: means[..k].to_vec(),
            area: areas[..k].to_vec(),
            width: widths[..k].to_vec(),
        },
        mobility_peaks: Peaks {
            mean: m_means[..km].to_vec(),
            area: m_areas[..km].to_vec(),
            width: m_widths[..km].to_vec(),
        },
    })
}

/// Parse one record stream.
pub(crate) fn parse_record(stream: &str, b: &[u8]) -> Result<Record> {
    let bad = |what: &str| Error::corrupt(FORMAT_ID, format!("{stream}: {what}"));
    let schema = le_u16(b, 0).ok_or_else(|| bad("shorter than its header"))?;
    let kind = le_u16(b, 2).ok_or_else(|| bad("shorter than its header"))?;
    let number = le_u32(b, 12).ok_or_else(|| bad("shorter than its header"))?;
    let (version, p) = string_at(b, 16).ok_or_else(|| bad("no software version string"))?;
    let date = le_f64(b, p).ok_or_else(|| bad("no measurement date"))?;
    let (serial, p) = string_at(b, p + 8).ok_or_else(|| bad("no instrument serial string"))?;
    let temperature = le_f32(b, p + 8)
        .map(f64::from)
        .ok_or_else(|| bad("no temperature"))?;
    // the sample name: after a material string and its 46-byte block
    let mut sample = None;
    let mut i = p;
    let stop = b.len().min(p.saturating_add(NAME_WINDOW));
    while i < stop {
        let Some((_, end)) = string_at(b, i) else {
            i += 1;
            continue;
        };
        if let Some(blk) = b.get(end..end + MATERIAL_LEN)
            && blk.starts_with(&MATERIAL_HEAD)
            && blk.ends_with(&MATERIAL_TAIL)
            && let Some((name, _)) = string_at(b, end + MATERIAL_LEN)
        {
            sample = Some(name);
            break;
        }
        i = end;
    }
    let mut size = None;
    let mut zeta = None;
    let mut matches = 0;
    match kind {
        1 if SIZE_RESULTS => {
            for o in 0..b.len().saturating_sub(16) {
                if let Some(s) = size_at(b, o) {
                    matches += 1;
                    size = Some(s);
                }
            }
            if matches != 1 {
                size = None;
            }
        }
        2 => {
            for o in 0..b.len().saturating_sub(192) {
                if let Some(z) = zeta_at(b, o) {
                    matches += 1;
                    zeta = Some(z);
                }
            }
            if matches != 1 {
                zeta = None;
            }
        }
        _ => {}
    }
    Ok(Record {
        stream: stream.to_string(),
        schema,
        kind,
        number,
        version,
        measured_at: ole_date_iso(date),
        serial,
        temperature,
        sample,
        size,
        zeta,
        matches,
    })
}

/// True when the bytes begin a compound file.
pub(crate) fn looks_like(head: &[u8]) -> bool {
    head.starts_with(&CFB_MAGIC)
}

fn nan_at(v: &[f64], k: usize) -> f64 {
    v.get(k).copied().unwrap_or(f64::NAN)
}

/// Parse a `.dts` file.
pub(crate) fn parse(bytes: &[u8], path: &Path) -> Result<SeriesFile> {
    let mut cur = Cursor::new(bytes);
    let cfb = Cfb::open(&mut cur, path, FORMAT_ID)?;
    let header = cfb.stream("Header").ok_or_else(|| {
        Error::unsupported(
            FORMAT_ID,
            "a compound file without a Zetasizer Header stream",
            "this is not a Zetasizer .dts measurement file",
        )
    })?;
    let h = cfb.read(&mut cur, path, header, 64)?;
    if h.get(2..18) != Some(&HEADER_ID[..]) {
        return Err(Error::unsupported(
            FORMAT_ID,
            "a Header stream with an unknown identifier",
            "report the file with `openreadout check FILE --report`",
        ));
    }
    let mut streams: Vec<(u32, &openreadout_core::cfb::CfbEntry)> = cfb
        .entries
        .iter()
        .filter(|e| e.is_stream)
        .filter_map(|e| {
            let name = e.path.rsplit('/').next().unwrap_or(&e.path);
            let n = name.strip_prefix("REC")?.parse::<u32>().ok()?;
            Some((n, e))
        })
        .collect();
    streams.sort_by_key(|(n, _)| *n);
    let mut records = Vec::new();
    let mut findings = Vec::new();
    for (n, e) in &streams {
        let data = cfb.read(&mut cur, path, e, MAX_BYTES)?;
        let r = parse_record(&format!("REC{n}"), &data)?;
        if r.number != *n {
            findings.push(Finding::warning(
                "record_number",
                format!("stream REC{n} holds record number {}", r.number),
            ));
        }
        records.push(r);
    }
    if records.is_empty() {
        return Err(Error::corrupt(FORMAT_ID, "no record streams"));
    }
    let size_records = records.iter().filter(|r| r.kind == 1).count();
    if !SIZE_RESULTS && size_records > 0 {
        findings.push(Finding::info(
            "size_results_withheld",
            format!(
                "{size_records} size record(s): Z-average, PdI and peaks are not returned (not yet validated against a Zetasizer export of size records); export the records table from the Zetasizer software for them"
            ),
        ));
    }
    for r in &records {
        match (r.kind, r.matches) {
            (1, 0) if !SIZE_RESULTS => {}
            (1 | 2, 0) => findings.push(Finding::info(
                "no_results",
                format!(
                    "record {}: no {} results stored (an aborted or incomplete measurement)",
                    r.number,
                    kind_name(r.kind)
                ),
            )),
            (1 | 2, m) if m > 1 => findings.push(Finding::warning(
                "results_ambiguous",
                format!(
                    "record {}: {m} blocks look like {} results; none is returned",
                    r.number,
                    kind_name(r.kind)
                ),
            )),
            (k, _) if k != 1 && k != 2 => findings.push(Finding::info(
                "kind_not_decoded",
                format!(
                    "record {}: measurement kind {k} is listed, not decoded",
                    r.number
                ),
            )),
            _ => {}
        }
        if r.sample.is_none() {
            findings.push(Finding::info(
                "sample_name",
                format!("record {}: the sample name was not found", r.number),
            ));
        }
    }
    // the records table
    let col = |f: &dyn Fn(&Record) -> f64| -> Vec<f64> { records.iter().map(f).collect() };
    let size = |f: &dyn Fn(&Size) -> f64| -> Vec<f64> {
        records
            .iter()
            .map(|r| r.size.as_ref().map_or(f64::NAN, f))
            .collect()
    };
    let zeta = |f: &dyn Fn(&Zeta) -> f64| -> Vec<f64> {
        records
            .iter()
            .map(|r| r.zeta.as_ref().map_or(f64::NAN, f))
            .collect()
    };
    let texts = |f: &dyn Fn(&Record) -> String| -> Vec<String> { records.iter().map(f).collect() };
    let mut cols = vec![
        SeriesColumn::numbers("record", None, col(&|r| f64::from(r.number))),
        SeriesColumn::texts("kind", &texts(&|r| kind_name(r.kind).to_string())),
        SeriesColumn::texts(
            "sample_name",
            &texts(&|r| r.sample.clone().unwrap_or_default()),
        ),
        SeriesColumn::texts(
            "measured_at",
            &texts(&|r| r.measured_at.clone().unwrap_or_default()),
        ),
        SeriesColumn::numbers("temperature", Some("°C"), col(&|r| r.temperature)),
        SeriesColumn::numbers("z_average", Some("nm"), size(&|s| s.z_average)),
        SeriesColumn::numbers("pdi", None, size(&|s| s.pdi)),
    ];
    for k in 0..3 {
        cols.push(SeriesColumn::numbers(
            format!("peak{}_mean", k + 1),
            Some("nm"),
            size(&|s| nan_at(&s.peaks[0].mean, k)),
        ));
        cols.push(SeriesColumn::numbers(
            format!("peak{}_area", k + 1),
            Some("%"),
            size(&|s| nan_at(&s.peaks[0].area, k)),
        ));
        cols.push(SeriesColumn::numbers(
            format!("peak{}_width", k + 1),
            Some("nm"),
            size(&|s| nan_at(&s.peaks[0].width, k)),
        ));
    }
    cols.extend([
        SeriesColumn::numbers("zeta_potential", Some("mV"), zeta(&|z| z.zeta)),
        SeriesColumn::numbers("zeta_deviation", Some("mV"), zeta(&|z| z.zeta_deviation)),
        SeriesColumn::numbers("mobility", Some("µm·cm/(V·s)"), zeta(&|z| z.mobility)),
        SeriesColumn::numbers(
            "mobility_deviation",
            Some("µm·cm/(V·s)"),
            zeta(&|z| z.mobility_deviation),
        ),
        SeriesColumn::numbers("conductivity", Some("mS/cm"), zeta(&|z| z.conductivity)),
        SeriesColumn::numbers("voltage", Some("V"), zeta(&|z| z.voltage)),
    ]);
    for k in 0..3 {
        cols.push(SeriesColumn::numbers(
            format!("zeta_peak{}_mean", k + 1),
            Some("mV"),
            zeta(&|z| nan_at(&z.zeta_peaks.mean, k)),
        ));
        cols.push(SeriesColumn::numbers(
            format!("zeta_peak{}_area", k + 1),
            Some("%"),
            zeta(&|z| nan_at(&z.zeta_peaks.area, k)),
        ));
        cols.push(SeriesColumn::numbers(
            format!("zeta_peak{}_width", k + 1),
            Some("mV"),
            zeta(&|z| nan_at(&z.zeta_peaks.width, k)),
        ));
    }
    let mut extra = BTreeMap::new();
    extra.insert(
        "description".into(),
        json!("one row per record: size results (Z-average, PdI, intensity peaks) or zeta results (zeta potential, mobility, conductivity, zeta peaks); NaN where the record has none"),
    );
    let mut tables = vec![SeriesTable {
        name: "records".into(),
        columns: cols,
        extra,
    }];
    // every stored peak of every distribution
    let (mut p_rec, mut p_dist, mut p_k, mut p_mean, mut p_area, mut p_width) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let mut units = Vec::new();
    for r in &records {
        let mut add = |dist: &str, unit: &str, pk: &Peaks| {
            for k in 0..pk.mean.len() {
                p_rec.push(f64::from(r.number));
                p_dist.push(dist.to_string());
                #[allow(clippy::cast_precision_loss)]
                p_k.push((k + 1) as f64);
                p_mean.push(pk.mean[k]);
                p_area.push(nan_at(&pk.area, k));
                p_width.push(nan_at(&pk.width, k));
                units.push(unit.to_string());
            }
        };
        if let Some(s) = &r.size {
            add("intensity", "nm", &s.peaks[0]);
            add("number", "nm", &s.peaks[1]);
            add("volume", "nm", &s.peaks[2]);
        }
        if let Some(z) = &r.zeta {
            add("zeta", "mV", &z.zeta_peaks);
            add("mobility", "µm·cm/(V·s)", &z.mobility_peaks);
        }
    }
    if !p_rec.is_empty() {
        let mut extra = BTreeMap::new();
        extra.insert(
            "description".into(),
            json!("every stored peak: size distributions by intensity, number and volume (d.nm), zeta potential (mV) and mobility; area in %"),
        );
        tables.push(SeriesTable {
            name: "peaks".into(),
            columns: vec![
                SeriesColumn::numbers("record", None, p_rec),
                SeriesColumn::texts("distribution", &p_dist),
                SeriesColumn::numbers("peak", None, p_k),
                SeriesColumn::numbers("mean", None, p_mean),
                SeriesColumn::numbers("area", Some("%"), p_area),
                SeriesColumn::numbers("width", None, p_width),
                SeriesColumn::texts("unit", &units),
            ],
            extra,
        });
    }
    // facts
    let first = &records[0];
    let mut facts = Facts::new();
    facts.set(
        "instrument.vendor",
        "Malvern Panalytical",
        "the file format",
    );
    facts.set("instrument.model", "Zetasizer", "the file format");
    facts.set("instrument.serial", &first.serial, "record header");
    facts.set(
        "instrument.software",
        "Zetasizer Software",
        "the file format",
    );
    facts.set(
        "instrument.software_version",
        &first.version,
        "record header",
    );
    if let Some(t) = records
        .iter()
        .filter_map(|r| r.measured_at.as_deref())
        .min()
    {
        facts.set(
            "acquisition.started_at",
            t,
            "the earliest record's measurement date (local time)",
        );
    }
    if let Some(t) = records
        .iter()
        .filter_map(|r| r.measured_at.as_deref())
        .max()
    {
        facts.set(
            "acquisition.ended_at",
            t,
            "the latest record's measurement date (local time)",
        );
    }
    let names: Vec<&str> = records.iter().filter_map(|r| r.sample.as_deref()).collect();
    if let Some(n) = names.first()
        && names.iter().all(|x| x == n)
    {
        facts.set("sample.name", n, "record header");
    }
    let n_size = records.iter().filter(|r| r.size.is_some()).count();
    let n_zeta = records.iter().filter(|r| r.zeta.is_some()).count();
    // one technique when every record is of one kind
    if records.iter().all(|r| r.kind == 1) {
        facts.technique("CHMO:0000167", "size records");
    } else if records.iter().all(|r| r.kind == 2) {
        facts.technique("CHMO:0002123", "zeta records");
    }
    facts.measurement(
        MeasurementKind::Table,
        vec![0],
        format!(
            "{} records: {n_size} with size results, {n_zeta} with zeta results",
            records.len()
        ),
        None,
    );
    let mut observations = Observations::default();
    for r in &records {
        observations.feature(
            FeatureKind::WriterVersion,
            format!(
                "Zetasizer {}",
                r.version.split('.').take(2).collect::<Vec<_>>().join(".")
            ),
            &[Scope::Metadata, Scope::Tables],
        );
        observations.feature(
            FeatureKind::FormatVersion,
            format!("record schema {}", r.schema),
            &[Scope::Metadata, Scope::Tables],
        );
        observations.feature(
            FeatureKind::Acquisition,
            format!("{} record", kind_name(r.kind)),
            &[Scope::Tables],
        );
    }
    let entries = records
        .iter()
        .map(|r| LsEntry {
            kind: "record".into(),
            name: r.stream.clone(),
            offset: None,
            size: None,
            image: None,
            details: json!({
                "kind": kind_name(r.kind),
                "sample": r.sample,
                "measured_at": r.measured_at,
                "results": r.size.is_some() || r.zeta.is_some(),
            }),
        })
        .collect();
    let mut vendor = Map::new();
    vendor.insert(
        "records".into(),
        Value::Array(
            records
                .iter()
                .map(|r| {
                    json!({"record": r.number, "stream": r.stream, "schema": r.schema, "kind": r.kind, "software_version": r.version})
                })
                .collect(),
        ),
    );
    if let Some(next) = le_u32(&h, 20) {
        vendor.insert("next_record".into(), json!(next));
    }
    let mut provenance = openreadout_core::ProvenanceMap::new();
    provenance.insert("tables".into(), Source::Inferred);
    let version = format!("record schema {}", first.schema);
    Ok(SeriesFile {
        format_version: Some(version),
        traces: Vec::new(),
        tables,
        experiment: Some(facts.build()),
        vendor: Value::Object(vendor),
        entries,
        findings,
        notes: vec![
            "Results as the Zetasizer software stored them; distributions and correlation functions are not returned".into(),
            "Dates are the instrument computer's local time".into(),
        ],
        provenance,
        observations,
        checks: vec![
            "compound-file structure".into(),
            "record header walk".into(),
            "one result block of the expected structure per record".into(),
        ],
        members: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> Vec<u8> {
        let u: Vec<u8> = text
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut b = (u.len() as u32).to_le_bytes().to_vec();
        b.push(1);
        b.extend(u);
        b
    }

    #[test]
    fn strings() {
        let b = s("7.13");
        assert_eq!(string_at(&b, 0), Some(("7.13".into(), b.len())));
        let mut bad = b.clone();
        bad[4] = 0;
        assert_eq!(string_at(&bad, 0), None);
        assert_eq!(string_at(&b[..6], 0), None);
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact synthetic values
    fn size_block() {
        let mut b = Vec::new();
        b.extend(150f32.to_le_bytes());
        b.extend(0.2f32.to_le_bytes());
        b.extend(10u32.to_le_bytes());
        for _ in 0..10 {
            b.extend(0.5f32.to_le_bytes());
        }
        for (m, a, w) in [
            (160.0f32, 100.0f32, 40.0f32),
            (20.0, 100.0, 5.0),
            (90.0, 100.0, 30.0),
        ] {
            for v in [m, a, w] {
                b.extend(1u32.to_le_bytes());
                b.extend(v.to_le_bytes());
            }
        }
        let got = size_at(&b, 0).unwrap();
        assert_eq!(got.z_average, 150.0);
        assert_eq!(got.peaks[0].mean, [160.0]);
        // areas that do not sum to 100: not a size block
        let mut bad = b.clone();
        let k = bad.len() - 12;
        bad[k..k + 4].copy_from_slice(&50f32.to_le_bytes());
        assert!(size_at(&bad, 0).is_none());
        assert!(size_at(&b[..20], 0).is_none());
    }
}
