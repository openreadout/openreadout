//! Cytiva Biacore result files (`.blr`, Biacore T200 Control Software): a compound file with the
//! run's environment, the sensor chip, the report-point table and one storage per cycle, window
//! and curve holding the sensorgram; and evaluation files (`.bme`, Biacore T200 Evaluation
//! Software), which copy one or more result files under `_DataManager N/` and add the evaluation
//! items with their fits. Layout: `docs/formats/cytiva-biacore.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{latin1, le_f64, le_u32, until_nul};
use openreadout_core::cfb::{CFB_MAGIC, Cfb, CfbEntry};
use openreadout_core::experiment::{
    Acquisition, Experiment, ExperimentInstrument, Measurement, MeasurementKind, Method, Origin,
    Quantity,
};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, FormatDescriptor, LsEntry, SignalChannelInfo, Table, TableInfo,
    Trace, TraceInfo,
};
use openreadout_core::pixel::Plane;
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::time::unix_to_iso8601;
use openreadout_core::vocab;
use openreadout_core::{ColumnInfo, Error, Result};
use serde_json::{Map, Value, json};

pub(crate) const FORMAT_ID: &str = "cytiva-biacore-blr";
/// Format id of evaluation files (`.bme`).
pub(crate) const BME_FORMAT_ID: &str = "cytiva-biacore-bme";
/// Largest evaluation item read (the largest seen is a few MB).
const MAX_ITEM: u64 = 256 << 20;

/// What kind of Biacore file the compatibility stream says this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `FileType=Result File` (`.blr`).
    Result,
    /// `FileType=T200 Evaluation File` (`.bme`).
    Evaluation,
}
/// Name of the stream that says what kind of Biacore file this is.
const COMPAT_STREAM: &str = "\u{3}BIA compability info";
/// Largest text stream read (the largest seen is 53 kB).
const MAX_TEXT: u64 = 16 << 20;
/// Largest curve stream read (the largest seen is 84 kB: 10,560 samples).
const MAX_CURVE: u64 = 512 << 20;
/// Header of a `Segment` stream: two u32, four f64, a u32 count.
const SEGMENT_HEADER: u64 = 44;
/// Header of an `XYData` stream: two u32 and a u32 count.
const XY_HEADER: u64 = 12;
/// Days from 1899-12-30 (OLE automation dates) to 1970-01-01.
const OLE_TO_UNIX_DAYS: f64 = 25_569.0;

fn f32s(b: &[u8]) -> Vec<f64> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f64::from(f32::from_le_bytes(*c)))
        .collect()
}

/// A float32 value as its shortest decimal (0.100000001 -> 0.1).
fn tidy_f32(v: f64) -> f64 {
    #[allow(clippy::cast_possible_truncation)]
    let s = format!("{}", v as f32);
    s.parse().unwrap_or(v)
}

/// `key=value` lines, in order.
fn key_values(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim_end_matches('\r').to_string()))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

fn lookup<'a>(kv: &'a [(String, String)], key: &str) -> Option<&'a str> {
    kv.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .filter(|v| !v.trim().is_empty())
}

/// An OLE automation date (days since 1899-12-30, the instrument PC's local time) as ISO-8601
/// without a zone.
fn ole_to_local_iso(days: f64) -> Option<String> {
    if !days.is_finite() || !(1.0..2_958_465.0).contains(&days) {
        return None;
    }
    let ms = ((days - OLE_TO_UNIX_DAYS) * 86_400_000.0).round();
    #[allow(clippy::cast_possible_truncation)]
    let ms = ms as i64;
    let iso = unix_to_iso8601(ms.div_euclid(1000), 0);
    iso.get(..19).map(str::to_string)
}

/// A name without the control-character prefix some Biacore streams carry (`\x03Keywords`).
fn bare(name: &str) -> &str {
    name.trim_start_matches(|c: char| c.is_control())
}

/// `_Cycle 3/_Window 1/_Curve 2` -> (3, 1, 2).
fn curve_numbers(storage: &str) -> Option<(u32, u32, u32)> {
    let mut it = storage.split('/');
    let c = it.next()?.strip_prefix("_Cycle ")?.parse().ok()?;
    let w = it.next()?.strip_prefix("_Window ")?.parse().ok()?;
    let k = it.next()?.strip_prefix("_Curve ")?.parse().ok()?;
    it.next().is_none().then_some((c, w, k))
}

/// Where a curve's samples are and how its time axis is laid out.
#[derive(Debug, Clone)]
enum CurveData {
    /// A raw flow-cell curve: f32 responses on a regular grid.
    Segment {
        stream: String,
        count: u64,
        start: f64,
        step: f64,
    },
    /// A reference-subtracted curve: f32 times, then f32 responses.
    Xy {
        stream: String,
        count: u64,
        /// (first, step) when the times are a regular grid.
        grid: Option<(f64, f64)>,
    },
}

impl CurveData {
    fn count(&self) -> u64 {
        match self {
            CurveData::Segment { count, .. } | CurveData::Xy { count, .. } => *count,
        }
    }
    fn grid(&self) -> Option<(f64, f64)> {
        match self {
            CurveData::Segment { start, step, .. } => Some((*start, *step)),
            CurveData::Xy { grid, .. } => *grid,
        }
    }
}

/// A report point listed with a curve (`RPoints`).
#[derive(Debug, Clone)]
struct CurvePoint {
    time: f64,
    window: f64,
    flag: String,
    name: String,
}

/// One sensorgram.
#[derive(Debug, Clone)]
struct Curve {
    /// The result file (`_DataManager N`) of an evaluation file; 0 in a result file.
    file: u32,
    cycle: u32,
    window: u32,
    curve: u32,
    storage: String,
    caption: String,
    title: String,
    x_unit: String,
    y_name: String,
    y_unit: String,
    flow_cell: Option<String>,
    subtracted: bool,
    keywords: Vec<(String, String)>,
    points: Vec<CurvePoint>,
    data: CurveData,
}

/// One `EventLog` line.
#[derive(Debug, Clone)]
struct Event {
    time_s: f64,
    code: f64,
    arguments: String,
}

#[derive(Debug, Clone, Default)]
struct Cycle {
    /// As for [`Curve::file`].
    file: u32,
    number: u32,
    caption: String,
    /// OLE date of the cycle's start (EventLog code 10).
    started: Option<f64>,
    events: Vec<Event>,
    keywords: Vec<(String, String)>,
}

/// A tab-separated text table (header line, rows).
#[derive(Debug, Clone, Default)]
struct TextTable {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

/// A column of a text table: numbers, or category codes with their labels.
enum Column {
    Numbers(Vec<f64>),
    Categories(Vec<f64>, Vec<String>),
}

fn column_of(cells: &[&str]) -> Column {
    let blank = |c: &str| c.trim().is_empty() || c.trim() == "N/A";
    let numeric = cells.iter().any(|c| !blank(c))
        && cells
            .iter()
            .all(|c| blank(c) || c.trim().parse::<f64>().is_ok_and(f64::is_finite));
    if numeric {
        return Column::Numbers(
            cells
                .iter()
                .map(|c| c.trim().parse().unwrap_or(f64::NAN))
                .collect(),
        );
    }
    let mut labels: Vec<String> = Vec::new();
    let codes = cells
        .iter()
        .map(|c| {
            let c = c.trim();
            let i = labels.iter().position(|l| l == c).unwrap_or_else(|| {
                labels.push(c.to_string());
                labels.len() - 1
            });
            i as f64
        })
        .collect();
    Column::Categories(codes, labels)
}

/// A table column: name, unit, values.
type NamedColumn = (String, Option<String>, Column);

/// Units of the report-point table's known columns.
fn report_unit(name: &str, y_unit: &str) -> Option<String> {
    match name {
        "Time" | "Window" => Some("s".into()),
        "AbsResp" | "RelResp" | "SD" | "Min" | "Max" => Some(y_unit.to_string()),
        "Slope" => Some(format!("{y_unit}/s")),
        _ => None,
    }
}

/// Everything read from a `.blr` file at open (sample values are read per trace).
#[derive(Debug, Clone, Default)]
pub(crate) struct Blr {
    compat: Vec<(String, String)>,
    application: String,
    environment: Vec<(String, String)>,
    chip: Vec<(String, String)>,
    file_tag: Vec<(String, String)>,
    bioconf: Vec<(String, String)>,
    curves: Vec<Curve>,
    cycles: Vec<Cycle>,
    report_points: TextTable,
    findings: Vec<Finding>,
    /// Evaluation files: the evaluation software's environment, items and fits.
    evaluation: Option<Evaluation>,
}

/// What an evaluation file adds.
#[derive(Debug, Clone, Default)]
pub(crate) struct Evaluation {
    environment: Vec<(String, String)>,
    items: Vec<crate::biacore_eval::EvalItem>,
    fits: Vec<crate::biacore_eval::Fit>,
    /// Result files (`_DataManager N`) in the file.
    files: Vec<u32>,
}

/// True when the compound file's compatibility stream says it is a Biacore result file.
#[cfg(test)]
pub(crate) fn is_result_file(text: &str) -> bool {
    kind_of(text) == Some(Kind::Result)
}

/// The file kind the compatibility stream names.
pub(crate) fn kind_of(text: &str) -> Option<Kind> {
    text.lines().find_map(|l| match l.trim() {
        "FileType=Result File" => Some(Kind::Result),
        "FileType=T200 Evaluation File" => Some(Kind::Evaluation),
        _ => None,
    })
}

/// Detection by content: a compound file with the Biacore compatibility stream, and its kind.
pub(crate) fn sniff_input(head: &[u8], input: &Input) -> Option<Kind> {
    if !head.starts_with(&CFB_MAGIC) {
        return None;
    }
    let mut f = input.open().ok()?;
    let cfb = Cfb::open(&mut f, input.path(), FORMAT_ID).ok()?;
    cfb.stream(COMPAT_STREAM)
        .and_then(|e| cfb.read(&mut f, input.path(), e, 4096).ok())
        .and_then(|b| kind_of(&latin1(until_nul(&b))))
}

struct Reader<'a, F> {
    cfb: &'a Cfb,
    f: &'a mut F,
    path: &'a Path,
}

impl<F: std::io::Read + std::io::Seek> Reader<'_, F> {
    fn entry(&self, path: &str) -> Option<CfbEntry> {
        self.cfb.stream(path).cloned()
    }
    fn bytes(&mut self, e: &CfbEntry, limit: u64) -> Result<Vec<u8>> {
        self.cfb.read(self.f, self.path, e, limit)
    }
    fn text(&mut self, path: &str) -> Option<String> {
        let e = self.entry(path)?;
        self.bytes(&e, MAX_TEXT).ok().map(|b| latin1(until_nul(&b)))
    }
}

/// The result-file storages of a file: `("", 0)` for a result file, `("_DataManager N/", N)`
/// for each result file an evaluation file holds.
fn result_prefixes(cfb: &Cfb, kind: Kind) -> Vec<(String, u32)> {
    if kind == Kind::Result {
        return vec![(String::new(), 0)];
    }
    let mut out: Vec<(String, u32)> = Vec::new();
    for e in &cfb.entries {
        let Some((top, _)) = e.path.split_once('/') else {
            continue;
        };
        if let Some(n) = top
            .strip_prefix("_DataManager ")
            .and_then(|n| n.parse::<u32>().ok())
            && !out.iter().any(|(_, k)| *k == n)
        {
            out.push((format!("{top}/"), n));
        }
    }
    out.sort_by_key(|(_, n)| *n);
    out
}

/// Parse the structure: text streams, curves and their headers, report points, event logs; and
/// for an evaluation file its items and fits.
pub(crate) fn parse<F: std::io::Read + std::io::Seek>(
    cfb: &Cfb,
    f: &mut F,
    path: &Path,
    want: Kind,
) -> Result<Blr> {
    let format = if want == Kind::Evaluation {
        BME_FORMAT_ID
    } else {
        FORMAT_ID
    };
    let mut r = Reader { cfb, f, path };
    let compat_text = r.text(COMPAT_STREAM).ok_or_else(|| {
        Error::corrupt(
            format,
            "no `BIA compability info` stream: not a Biacore result or evaluation file",
        )
    })?;
    let kind = kind_of(&compat_text);
    if kind != Some(want) {
        let hint = match (kind, want) {
            (Some(Kind::Evaluation), _) => {
                "This is a Biacore T200 evaluation file (.bme): it is read as cytiva-biacore-bme (detected by content)."
            }
            (Some(Kind::Result), _) => {
                "This is a Biacore result file (.blr): it is read as cytiva-biacore-blr (detected by content)."
            }
            _ => {
                "Only Biacore T200 result files (.blr) and evaluation files (.bme) are read; open the run's result file."
            }
        };
        return Err(Error::unsupported(
            format,
            format!(
                "a Biacore file of type `{}`",
                compat_text.lines().next().unwrap_or("").trim()
            ),
            hint,
        ));
    }
    let prefixes = result_prefixes(cfb, want);
    let Some((first, _)) = prefixes.first().cloned() else {
        return Err(Error::corrupt(
            format,
            "an evaluation file without a `_DataManager N` storage: it holds no result file",
        ));
    };
    let mut b = Blr {
        compat: key_values(&compat_text),
        application: r.text("\u{3}BIA application info").unwrap_or_default(),
        environment: key_values(&r.text(&format!("{first}Environment")).unwrap_or_default()),
        chip: key_values(&r.text(&format!("{first}Chip")).unwrap_or_default()),
        file_tag: key_values(&r.text(&format!("{first}FileTag")).unwrap_or_default()),
        bioconf: key_values(
            &r.text(&format!("{first}AppData/Bioconf"))
                .unwrap_or_default(),
        ),
        ..Blr::default()
    };
    if b.environment.is_empty() {
        b.findings.push(Finding::warning(
            "missing_environment",
            "no readable `Environment` stream: instrument, software and run times are unknown",
        ));
    }
    if let Some(t) = r.text(&format!("{first}RPoint Table")) {
        b.report_points = text_table(&t);
    }
    for (prefix, file) in &prefixes {
        // curve storages, in cycle / window / curve order
        let mut storages: BTreeMap<(u32, u32, u32), String> = BTreeMap::new();
        let mut cycle_numbers: Vec<u32> = Vec::new();
        for e in &cfb.entries {
            if !e.is_stream {
                continue;
            }
            let Some(rel) = e.path.strip_prefix(prefix.as_str()) else {
                continue;
            };
            if let Some((dir, _)) = rel.rsplit_once('/')
                && let Some(k) = curve_numbers(dir)
            {
                storages
                    .entry(k)
                    .or_insert_with(|| format!("{prefix}{dir}"));
            }
            if let Some(rest) = rel.strip_prefix("_Cycle ")
                && let Some((n, _)) = rest.split_once('/')
                && let Ok(n) = n.parse::<u32>()
                && !cycle_numbers.contains(&n)
            {
                cycle_numbers.push(n);
            }
        }
        cycle_numbers.sort_unstable();
        let mut captions: BTreeMap<(u32, u32), String> = BTreeMap::new();
        for ((c, w, k), dir) in &storages {
            let caption = captions
                .entry((*c, *w))
                .or_insert_with(|| {
                    let t = r
                        .text(&format!("{prefix}_Cycle {c}/_Window {w}/Properties"))
                        .unwrap_or_default();
                    lookup(&key_values(&t), "Caption").unwrap_or("").to_string()
                })
                .clone();
            match read_curve(&mut r, (*c, *w, *k), dir, caption) {
                Ok(Some(mut curve)) => {
                    curve.file = *file;
                    b.curves.push(curve);
                }
                Ok(None) => {}
                Err(finding) => b.findings.push(finding),
            }
        }
        for n in cycle_numbers {
            let mut cy = Cycle {
                file: *file,
                number: n,
                ..Cycle::default()
            };
            if let Some(t) = r.text(&format!("{prefix}_Cycle {n}/EventLog")) {
                let (events, started) = event_log(&t);
                cy.events = events;
                cy.started = started;
            }
            if let Some(first) = b.curves.iter().find(|c| c.cycle == n && c.file == *file) {
                cy.caption.clone_from(&first.caption);
                cy.keywords = first
                    .keywords
                    .iter()
                    .filter(|(k, _)| !matches!(k.as_str(), "Version" | "Fc" | "DiodeRow"))
                    .cloned()
                    .collect();
            }
            b.cycles.push(cy);
        }
    }
    if b.curves.is_empty() {
        return Err(Error::corrupt(
            format,
            "no readable sensorgram in the file (no `_Cycle N/_Window N/_Curve N` storage with data)",
        ));
    }
    if want == Kind::Evaluation {
        let mut ev = Evaluation {
            environment: key_values(&r.text("Environment").unwrap_or_default()),
            files: prefixes.iter().map(|(_, n)| *n).collect(),
            ..Evaluation::default()
        };
        let mut items: Vec<(u32, CfbEntry)> = cfb
            .entries
            .iter()
            .filter(|e| e.is_stream)
            .filter_map(|e| {
                let n = e
                    .path
                    .strip_prefix("Evaluation/EvaluationItem")?
                    .parse::<u32>()
                    .ok()?;
                Some((n, e.clone()))
            })
            .collect();
        items.sort_by_key(|(n, _)| *n);
        for (n, e) in items {
            let bytes = match r.bytes(&e, MAX_ITEM) {
                Ok(bytes) => bytes,
                Err(err) => {
                    b.findings.push(Finding::warning(
                        "unreadable_evaluation_item",
                        format!("EvaluationItem{n}: {err}"),
                    ));
                    continue;
                }
            };
            match crate::biacore_eval::parse_item(n, &bytes) {
                Ok((item, fits)) => {
                    ev.items.push(item);
                    ev.fits.extend(fits);
                }
                Err(why) => b
                    .findings
                    .push(Finding::warning("unreadable_evaluation_item", why)),
            }
        }
        b.evaluation = Some(ev);
    }
    Ok(b)
}

fn text_table(t: &str) -> TextTable {
    let mut lines = t.lines().filter(|l| !l.trim().is_empty());
    let Some(h) = lines.next() else {
        return TextTable::default();
    };
    let header: Vec<String> = h.split('\t').map(|s| s.trim().to_string()).collect();
    let rows = lines
        .map(|l| {
            let mut row: Vec<String> = l
                .split('\t')
                .map(|s| s.trim_end_matches('\r').to_string())
                .collect();
            row.resize(header.len(), String::new());
            row
        })
        .collect();
    TextTable { header, rows }
}

/// Events (`F<ms>;<code>;<arguments>`) and the cycle's start date (code 10's `d` argument).
fn event_log(t: &str) -> (Vec<Event>, Option<f64>) {
    let mut events = Vec::new();
    let mut started = None;
    for l in t.lines() {
        let Some(rest) = l.trim().strip_prefix('F') else {
            continue;
        };
        let mut parts = rest.splitn(3, ';');
        let (Some(ms), Some(code)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(ms), Ok(code)) = (ms.parse::<f64>(), code.parse::<f64>()) else {
            continue;
        };
        let arguments = parts.next().unwrap_or("").to_string();
        if (code - 10.0).abs() < f64::EPSILON && started.is_none() {
            started = arguments
                .strip_prefix('d')
                .and_then(|d| d.parse::<f64>().ok())
                .filter(|d| d.is_finite());
        }
        events.push(Event {
            time_s: ms / 1000.0,
            code,
            arguments,
        });
    }
    (events, started)
}

/// A curve from its storage, `Ok(None)` when it holds no data stream, `Err` with a finding when it
/// is damaged or laid out in a way not validated.
fn read_curve<F: std::io::Read + std::io::Seek>(
    reader: &mut Reader<'_, F>,
    (cycle, window, curve): (u32, u32, u32),
    dir: &str,
    caption: String,
) -> std::result::Result<Option<Curve>, Finding> {
    let prefix = format!("{dir}/");
    let names: Vec<String> = reader
        .cfb
        .entries
        .iter()
        .filter(|entry| entry.is_stream)
        .filter_map(|entry| entry.path.strip_prefix(&prefix).map(str::to_string))
        .filter(|n| !n.contains('/'))
        .collect();
    let find = |want: &str| names.iter().find(|n| bare(n) == want).cloned();
    let segments: Vec<&String> = names
        .iter()
        .filter(|n| bare(n).starts_with("Segment "))
        .collect();
    let xy = find("XYData");
    if segments.is_empty() && xy.is_none() {
        return Ok(None);
    }
    let damaged = |why: String| {
        Finding::error(
            "unreadable_curve",
            format!("cycle {cycle} window {window} curve {curve}: {why}; the curve is left out"),
        )
    };
    let labels = find("Labels")
        .and_then(|n| reader.text(&format!("{prefix}{n}")))
        .ok_or_else(|| damaged("no `Labels` stream".into()))?;
    let lines: Vec<&str> = labels.lines().map(|l| l.trim_end_matches('\r')).collect();
    let title = lines.first().copied().unwrap_or("").to_string();
    let x_unit = lines.get(2).copied().unwrap_or("").to_string();
    let y_name = lines.get(3).copied().unwrap_or("").to_string();
    let y_unit = lines.get(4).copied().unwrap_or("").to_string();
    if x_unit != "s" {
        return Err(Finding::warning(
            "unvalidated_curve_axis",
            format!(
                "cycle {cycle} window {window} curve {curve}: x axis in `{x_unit}` (only time in s has been validated); the curve is left out"
            ),
        ));
    }
    let flow_cell = title.split_once("Fc=").map(|(_, f)| f.trim().to_string());
    let subtracted = title.starts_with("Subtracted");
    let keywords = find("Keywords")
        .and_then(|n| reader.text(&format!("{prefix}{n}")))
        .map(|t| key_values(&t))
        .unwrap_or_default();
    let points = find("RPoints")
        .and_then(|n| reader.text(&format!("{prefix}{n}")))
        .map(|t| {
            t.lines()
                .skip(1)
                .filter_map(|l| {
                    let f: Vec<&str> = l.split('\t').collect();
                    Some(CurvePoint {
                        time: f.first()?.trim().parse().ok()?,
                        window: f.get(1)?.trim().parse().ok()?,
                        flag: f.get(2).map_or(String::new(), |s| s.trim().to_string()),
                        name: f.get(3).map_or(String::new(), |s| s.trim().to_string()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let data = if segments.is_empty() {
        let name = xy.unwrap_or_default();
        let stream = format!("{prefix}{name}");
        let entry = reader
            .entry(&stream)
            .ok_or_else(|| damaged("`XYData` not found".into()))?;
        let head = reader
            .bytes(&entry, XY_HEADER)
            .map_err(|err| damaged(err.to_string()))?;
        let (Some(a), Some(b), Some(n)) = (le_u32(&head, 0), le_u32(&head, 4), le_u32(&head, 8))
        else {
            return Err(damaged("`XYData` header shorter than 12 bytes".into()));
        };
        if (a, b) != (1, 1) {
            return Err(damaged(format!(
                "`XYData` header ({a}, {b}) differs from the (1, 1) seen in every validated file"
            )));
        }
        let n = u64::from(n);
        if XY_HEADER + 8 * n != entry.size {
            return Err(damaged(format!(
                "`XYData` holds {} bytes for {n} points (expected {})",
                entry.size,
                XY_HEADER + 8 * n
            )));
        }
        if entry.size > MAX_CURVE {
            return Err(damaged(format!("`XYData` of {} bytes", entry.size)));
        }
        let b = reader
            .bytes(&entry, XY_HEADER + 4 * n)
            .map_err(|err| damaged(err.to_string()))?;
        let times = f32s(b.get(12..).unwrap_or(&[]));
        CurveData::Xy {
            stream,
            count: n,
            grid: regular_grid(&times),
        }
    } else {
        if segments.len() > 1 || bare(segments[0]) != "Segment 1" {
            return Err(Finding::warning(
                "unsupported_segments",
                format!(
                    "cycle {cycle} window {window} curve {curve}: {} segments (every validated curve has one, `Segment 1`); the curve is left out",
                    segments.len()
                ),
            ));
        }
        let stream = format!("{prefix}{}", segments[0]);
        let entry = reader
            .entry(&stream)
            .ok_or_else(|| damaged("segment not found".into()))?;
        let head = reader
            .bytes(&entry, SEGMENT_HEADER)
            .map_err(|err| damaged(err.to_string()))?;
        let fields = (
            le_u32(&head, 0),
            le_u32(&head, 4),
            le_f64(&head, 8),
            le_f64(&head, 16),
            le_f64(&head, 24),
            le_f64(&head, 32),
            le_u32(&head, 40),
        );
        let (Some(a), Some(b), Some(step), Some(start), Some(offset), Some(scale), Some(n)) =
            fields
        else {
            return Err(damaged("segment header shorter than 44 bytes".into()));
        };
        if (a, b) != (1, 1) {
            return Err(damaged(format!(
                "segment header ({a}, {b}) differs from the (1, 1) seen in every validated file"
            )));
        }
        let n = u64::from(n);
        if SEGMENT_HEADER + 4 * n != entry.size || entry.size > MAX_CURVE {
            return Err(damaged(format!(
                "segment holds {} bytes for {n} samples (expected {})",
                entry.size,
                SEGMENT_HEADER + 4 * n
            )));
        }
        #[allow(clippy::float_cmp)] // exact header constants
        let identity = offset == 0.0 && scale == 1.0;
        #[allow(clippy::float_cmp)]
        let same = step == start;
        if !identity || !same || !step.is_finite() || step <= 0.0 {
            return Err(Finding::warning(
                "unvalidated_time_base",
                format!(
                    "cycle {cycle} window {window} curve {curve}: segment header ({step}, {start}, {offset}, {scale}) differs from the validated layout (step = start, 0, 1); the curve is left out"
                ),
            ));
        }
        CurveData::Segment {
            stream,
            count: n,
            start,
            step,
        }
    };
    Ok(Some(Curve {
        file: 0,
        cycle,
        window,
        curve,
        storage: dir.to_string(),
        caption,
        title,
        x_unit,
        y_name,
        y_unit,
        flow_cell,
        subtracted,
        keywords,
        points,
        data,
    }))
}

/// (first, step) when float32 times lie on a regular grid within float32 rounding.
fn regular_grid(t: &[f64]) -> Option<(f64, f64)> {
    let (first, last) = (*t.first()?, *t.last()?);
    if t.len() < 2 {
        return None;
    }
    let step = (last - first) / (t.len() - 1) as f64;
    if !(step.is_finite() && step > 0.0) {
        return None;
    }
    let step = tidy_f32(step);
    let first = tidy_f32(first);
    let ok = t.iter().enumerate().all(|(i, &v)| {
        let want = first + step * i as f64;
        (v - want).abs() <= (step * 1e-3).max(want.abs() * 2.5e-7)
    });
    ok.then_some((first, step))
}

/// An opened `.blr` file.
#[derive(Debug)]
pub struct BiacoreDataset {
    descriptor: FormatDescriptor,
    path: PathBuf,
    fs: Fs,
    size: u64,
    cfb: Cfb,
    blr: Blr,
}

impl BiacoreDataset {
    pub(crate) fn open_input(
        descriptor: FormatDescriptor,
        input: &Input,
        kind: Kind,
    ) -> Result<Self> {
        let path = input.path().to_path_buf();
        let mut f = input.open()?;
        let size = f.size().map_err(|e| Error::io(&path, e))?;
        let format = if kind == Kind::Evaluation {
            BME_FORMAT_ID
        } else {
            FORMAT_ID
        };
        let cfb = Cfb::open(&mut f, &path, format)?;
        let blr = parse(&cfb, &mut f, &path, kind)?;
        Ok(BiacoreDataset {
            descriptor,
            path,
            fs: input.fs().clone(),
            size,
            cfb,
            blr,
        })
    }

    fn env(&self, key: &str) -> Option<&str> {
        lookup(&self.blr.environment, key)
    }

    fn run_start(&self) -> Option<f64> {
        self.env("Timestamp").and_then(|v| v.trim().parse().ok())
    }

    /// Samples of curve `k`: (times when the grid is irregular, responses).
    fn samples(&self, k: usize) -> Result<(Option<Vec<f64>>, Vec<f64>)> {
        let curve = self
            .blr
            .curves
            .get(k)
            .ok_or_else(|| Error::Usage(format!("trace {k} out of range")))?;
        let mut file = self
            .fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))?;
        match &curve.data {
            CurveData::Segment { stream, count, .. } => {
                let entry = self.stream(stream)?;
                let raw = self.cfb.read(&mut file, &self.path, &entry, MAX_CURVE)?;
                let body = raw.get(SEGMENT_HEADER as usize..).ok_or_else(|| {
                    Error::corrupt(FORMAT_ID, format!("{stream}: shorter than its header"))
                })?;
                let v = f32s(body);
                if v.len() as u64 != *count {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!(
                            "{stream}: {} samples where the header says {count}",
                            v.len()
                        ),
                    ));
                }
                Ok((None, v))
            }
            CurveData::Xy {
                stream,
                count,
                grid,
            } => {
                let entry = self.stream(stream)?;
                let raw = self.cfb.read(&mut file, &self.path, &entry, MAX_CURVE)?;
                let n =
                    usize::try_from(*count).map_err(|_| Error::Other("curve too long".into()))?;
                let body = raw.get(XY_HEADER as usize..).unwrap_or(&[]);
                let (Some(t), Some(y)) = (body.get(..4 * n), body.get(4 * n..8 * n)) else {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        format!("{stream}: shorter than its {count} points"),
                    ));
                };
                let times = grid.is_none().then(|| f32s(t));
                Ok((times, f32s(y)))
            }
        }
    }

    fn stream(&self, path: &str) -> Result<CfbEntry> {
        self.cfb
            .stream(path)
            .cloned()
            .ok_or_else(|| Error::corrupt(FORMAT_ID, format!("stream {path} not found")))
    }

    /// True when an evaluation file holds more than one result file.
    fn multi_file(&self) -> bool {
        self.blr
            .evaluation
            .as_ref()
            .is_some_and(|e| e.files.len() > 1)
    }

    fn y_unit(&self) -> String {
        self.blr
            .curves
            .first()
            .map_or_else(|| "RU".to_string(), |c| c.y_unit.clone())
    }

    fn trace_info(&self, k: usize, c: &Curve) -> TraceInfo {
        let mut channels = Vec::new();
        let grid = c.data.grid();
        if grid.is_none() {
            channels.push(channel(0, "time", &c.x_unit));
        }
        channels.push(channel(channels.len() as u32, "response", &c.y_unit));
        let mut extra = BTreeMap::new();
        extra.insert("kind".into(), json!("sensorgram"));
        extra.insert("cycle".into(), json!(c.cycle));
        extra.insert("window".into(), json!(c.window));
        extra.insert("curve".into(), json!(c.curve));
        extra.insert("window_caption".into(), json!(c.caption));
        extra.insert("title".into(), json!(c.title));
        extra.insert("response".into(), json!(c.y_name));
        if let Some(fc) = &c.flow_cell {
            extra.insert("flow_cell".into(), json!(fc));
        }
        extra.insert("reference_subtracted".into(), json!(c.subtracted));
        let kw: Map<String, Value> = c
            .keywords
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        extra.insert("keywords".into(), Value::Object(kw));
        if !c.points.is_empty() {
            extra.insert(
                "report_points".into(),
                json!(c
                    .points
                    .iter()
                    .map(|p| json!({"time_s": p.time, "window_s": p.window, "flag": p.flag, "name": p.name}))
                    .collect::<Vec<_>>()),
            );
        }
        extra.insert("storage".into(), json!(c.storage));
        if self.blr.evaluation.is_some() {
            extra.insert("file".into(), json!(c.file));
        }
        let (rate, start) = if let Some((first, step)) = grid {
            extra.insert(
                "axis".into(),
                json!({"quantity": "time", "unit": "s", "first": first, "step": step}),
            );
            (1.0 / step, Some(first))
        } else {
            extra.insert(
                "axis".into(),
                json!({"quantity": "time", "unit": "s", "irregular": true, "channel": 0}),
            );
            (0.0, None)
        };
        TraceInfo {
            index: k as u32,
            name: Some(if self.multi_file() {
                format!("file {} cycle {} {}", c.file, c.cycle, c.title)
            } else {
                format!("cycle {} {}", c.cycle, c.title)
            }),
            sample_rate_hz: rate,
            sample_count: c.data.count(),
            sweep_count: 1,
            channels,
            start_s: start,
            extra,
        }
    }

    /// The tables: report points (when the file has them), cycles, event log.
    fn tables(&self) -> Vec<(String, Vec<NamedColumn>)> {
        let mut out = Vec::new();
        let y = self.y_unit();
        let rp = &self.blr.report_points;
        if !rp.rows.is_empty() {
            let cols = rp
                .header
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let cells: Vec<&str> = rp.rows.iter().map(|r| r[i].as_str()).collect();
                    (h.clone(), report_unit(h, &y), column_of(&cells))
                })
                .collect();
            out.push(("report_points".to_string(), cols));
        }
        // cycles: number, caption, start, curves, keywords of the cycle's first curve
        let cy = &self.blr.cycles;
        let t0 = self.run_start();
        let mut cols: Vec<NamedColumn> = Vec::new();
        if self.blr.evaluation.is_some() {
            cols.push((
                "file".into(),
                None,
                Column::Numbers(cy.iter().map(|c| f64::from(c.file)).collect()),
            ));
        }
        cols.extend([
            (
                "cycle".into(),
                None,
                Column::Numbers(cy.iter().map(|c| f64::from(c.number)).collect()),
            ),
            (
                "window_caption".into(),
                None,
                column_of(&cy.iter().map(|c| c.caption.as_str()).collect::<Vec<_>>()),
            ),
            (
                "start".into(),
                Some("s".into()),
                Column::Numbers(
                    cy.iter()
                        .map(|c| match (c.started, t0) {
                            (Some(s), Some(t0)) => ((s - t0) * 86_400.0 * 1000.0).round() / 1000.0,
                            _ => f64::NAN,
                        })
                        .collect(),
                ),
            ),
            (
                "curves".into(),
                None,
                Column::Numbers(
                    cy.iter()
                        .map(|c| {
                            self.blr
                                .curves
                                .iter()
                                .filter(|k| k.cycle == c.number && k.file == c.file)
                                .count() as f64
                        })
                        .collect(),
                ),
            ),
        ]);
        let mut keys: Vec<&str> = Vec::new();
        for c in cy {
            for (k, _) in &c.keywords {
                if !keys.contains(&k.as_str()) {
                    keys.push(k);
                }
            }
        }
        for k in keys {
            let cells: Vec<&str> = cy
                .iter()
                .map(|c| lookup(&c.keywords, k).unwrap_or(""))
                .collect();
            cols.push((k.to_string(), None, column_of(&cells)));
        }
        out.push(("cycles".to_string(), cols));
        // event log, uninterpreted codes
        let ev: Vec<(u32, &Event)> = cy
            .iter()
            .flat_map(|c| c.events.iter().map(move |e| (c.number, e)))
            .collect();
        out.push((
            "event_log".to_string(),
            vec![
                (
                    "cycle".into(),
                    None,
                    Column::Numbers(ev.iter().map(|(n, _)| f64::from(*n)).collect()),
                ),
                (
                    "time".into(),
                    Some("s".into()),
                    Column::Numbers(ev.iter().map(|(_, e)| e.time_s).collect()),
                ),
                (
                    "code".into(),
                    None,
                    Column::Numbers(ev.iter().map(|(_, e)| e.code).collect()),
                ),
                (
                    "arguments".into(),
                    None,
                    column_of(
                        &ev.iter()
                            .map(|(_, e)| e.arguments.as_str())
                            .collect::<Vec<_>>(),
                    ),
                ),
            ],
        ));
        if let Some(ev) = &self.blr.evaluation {
            out.extend(evaluation_tables(ev));
        }
        out
    }
}

fn texts(v: &[String]) -> Column {
    // a text column even when every cell looks like a number (sample names such as `1`)
    let mut labels: Vec<String> = Vec::new();
    let codes = v
        .iter()
        .map(|c| {
            let i = labels.iter().position(|l| l == c).unwrap_or_else(|| {
                labels.push(c.clone());
                labels.len() - 1
            });
            i as f64
        })
        .collect();
    Column::Categories(codes, labels)
}

/// The first value of a parameter in a fit.
fn first_param<'a>(
    f: &'a crate::biacore_eval::Fit,
    name: &str,
) -> Option<&'a crate::biacore_eval::Param> {
    f.params.iter().find(|p| p.name == name)
}

/// The evaluation items and their fits as tables.
fn evaluation_tables(ev: &Evaluation) -> Vec<(String, Vec<NamedColumn>)> {
    let mut out = Vec::new();
    let it = &ev.items;
    out.push((
        "evaluation_items".to_string(),
        vec![
            (
                "item".into(),
                None,
                Column::Numbers(it.iter().map(|i| f64::from(i.index)).collect()),
            ),
            (
                "class".into(),
                None,
                texts(&it.iter().map(|i| i.class.clone()).collect::<Vec<_>>()),
            ),
            (
                "name".into(),
                None,
                texts(&it.iter().map(|i| i.name.clone()).collect::<Vec<_>>()),
            ),
            (
                "fits".into(),
                None,
                Column::Numbers(it.iter().map(|i| i.fits as f64).collect()),
            ),
        ],
    ));
    let fits = &ev.fits;
    if fits.is_empty() {
        return out;
    }
    let txt = |f: &dyn Fn(&crate::biacore_eval::Fit) -> String| -> Column {
        texts(&fits.iter().map(f).collect::<Vec<_>>())
    };
    let mut cols: Vec<NamedColumn> = vec![
        (
            "fit".into(),
            None,
            Column::Numbers((0..fits.len()).map(|k| k as f64).collect()),
        ),
        (
            "item".into(),
            None,
            Column::Numbers(fits.iter().map(|f| f64::from(f.item)).collect()),
        ),
        ("item_name".into(), None, txt(&|f| f.item_name.clone())),
        ("model".into(), None, txt(&|f| f.model.clone())),
        ("sample".into(), None, txt(&|f| f.sample.clone())),
        ("ligand".into(), None, txt(&|f| f.ligand.clone())),
        ("curve".into(), None, txt(&|f| f.curves.clone())),
        (
            "temperature".into(),
            Some("°C".into()),
            Column::Numbers(fits.iter().map(|f| f.temperature).collect()),
        ),
        (
            "chi2".into(),
            Some("RU²".into()),
            Column::Numbers(fits.iter().map(|f| f.chi2).collect()),
        ),
        ("status".into(), None, txt(&|f| f.status.clone())),
        (
            "curves".into(),
            None,
            Column::Numbers(fits.iter().map(|f| f.points.len() as f64).collect()),
        ),
    ];
    // one column per parameter name (its first value in each fit) and its standard error
    let mut names: Vec<&str> = Vec::new();
    for f in fits {
        for p in &f.params {
            if !names.contains(&p.name.as_str()) {
                names.push(&p.name);
            }
        }
    }
    for name in names {
        let unit = fits.iter().find_map(|f| f.units.get(name).cloned());
        cols.push((
            name.to_string(),
            unit.clone(),
            Column::Numbers(
                fits.iter()
                    .map(|f| first_param(f, name).map_or(f64::NAN, |p| p.value))
                    .collect(),
            ),
        ));
        cols.push((
            format!("{name}_se"),
            unit,
            Column::Numbers(
                fits.iter()
                    .map(|f| first_param(f, name).map_or(f64::NAN, |p| p.se))
                    .collect(),
            ),
        ));
    }
    out.push(("fits".to_string(), cols));
    // every parameter with its scope
    let params: Vec<(usize, &crate::biacore_eval::Param, Option<&String>)> = fits
        .iter()
        .enumerate()
        .flat_map(|(k, f)| f.params.iter().map(move |p| (k, p, f.units.get(&p.name))))
        .collect();
    out.push((
        "fit_parameters".to_string(),
        vec![
            (
                "fit".into(),
                None,
                Column::Numbers(params.iter().map(|(k, _, _)| *k as f64).collect()),
            ),
            (
                "scope".into(),
                None,
                texts(
                    &params
                        .iter()
                        .map(|(_, p, _)| p.scope.clone())
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "parameter".into(),
                None,
                texts(
                    &params
                        .iter()
                        .map(|(_, p, _)| p.name.clone())
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "value".into(),
                None,
                Column::Numbers(params.iter().map(|(_, p, _)| p.value).collect()),
            ),
            (
                "se".into(),
                None,
                Column::Numbers(params.iter().map(|(_, p, _)| p.se).collect()),
            ),
            (
                "unit".into(),
                None,
                texts(
                    &params
                        .iter()
                        .map(|(_, _, u)| u.cloned().unwrap_or_default())
                        .collect::<Vec<_>>(),
                ),
            ),
        ],
    ));
    // the curves (concentrations) each fit used
    let pts: Vec<(usize, &crate::biacore_eval::FitPoint)> = fits
        .iter()
        .enumerate()
        .flat_map(|(k, f)| f.points.iter().map(move |p| (k, p)))
        .collect();
    out.push((
        "fit_points".to_string(),
        vec![
            (
                "fit".into(),
                None,
                Column::Numbers(pts.iter().map(|(k, _)| *k as f64).collect()),
            ),
            (
                "curve".into(),
                None,
                texts(
                    &pts.iter()
                        .map(|(_, p)| p.subset.clone())
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "file".into(),
                None,
                Column::Numbers(pts.iter().map(|(_, p)| p.file).collect()),
            ),
            (
                "cycle".into(),
                None,
                Column::Numbers(pts.iter().map(|(_, p)| p.cycle).collect()),
            ),
            (
                "sample".into(),
                None,
                texts(
                    &pts.iter()
                        .map(|(_, p)| p.sample.clone())
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "concentration".into(),
                Some("M".into()),
                Column::Numbers(pts.iter().map(|(_, p)| p.concentration).collect()),
            ),
            (
                "concentration_stated".into(),
                None,
                Column::Numbers(pts.iter().map(|(_, p)| p.concentration_stated).collect()),
            ),
            (
                "concentration_unit".into(),
                None,
                texts(&pts.iter().map(|(_, p)| p.unit.clone()).collect::<Vec<_>>()),
            ),
            (
                "response".into(),
                Some("RU".into()),
                Column::Numbers(pts.iter().map(|(_, p)| p.response).collect()),
            ),
            (
                "included".into(),
                None,
                Column::Numbers(pts.iter().map(|(_, p)| p.included).collect()),
            ),
        ],
    ));
    out
}

fn channel(index: u32, name: &str, unit: &str) -> SignalChannelInfo {
    SignalChannelInfo {
        index,
        name: name.into(),
        unit: (!unit.is_empty()).then(|| unit.to_string()),
        dtype: "float32".into(),
        scale: 1.0,
        offset: 0.0,
        extra: BTreeMap::new(),
    }
}

fn table_info(index: u32, name: &str, cols: &[NamedColumn]) -> TableInfo {
    let rows = cols.first().map_or(0, |(_, _, c)| match c {
        Column::Numbers(v) | Column::Categories(v, _) => v.len(),
    });
    TableInfo {
        index,
        name: Some(name.to_string()),
        row_count: rows as u64,
        columns: cols
            .iter()
            .enumerate()
            .map(|(i, (n, unit, c))| ColumnInfo {
                index: i as u32,
                name: n.clone(),
                label: None,
                dtype: match c {
                    Column::Numbers(_) => "float64".into(),
                    Column::Categories(..) => "uint32".into(),
                },
                unit: unit.clone(),
                range: None,
                extra: match c {
                    Column::Numbers(_) => BTreeMap::new(),
                    Column::Categories(_, labels) => {
                        BTreeMap::from([("categories".to_string(), json!(labels))])
                    }
                },
            })
            .collect(),
        extra: BTreeMap::new(),
    }
}

fn kv_object(kv: &[(String, String)]) -> Value {
    Value::Object(kv.iter().map(|(k, v)| (k.clone(), json!(v))).collect())
}

/// `BiacoreT200` -> `Biacore T200`.
fn model_name(unit: &str) -> String {
    match unit.strip_prefix("Biacore") {
        Some(rest) if !rest.is_empty() && !rest.starts_with(' ') => format!("Biacore {rest}"),
        _ => unit.to_string(),
    }
}

impl Dataset for BiacoreDataset {
    fn info(&self) -> Result<FileInfo> {
        let traces = self
            .blr
            .curves
            .iter()
            .enumerate()
            .map(|(k, c)| self.trace_info(k, c))
            .collect();
        let tables = self
            .tables()
            .iter()
            .enumerate()
            .map(|(i, (name, cols))| table_info(i as u32, name, cols))
            .collect();
        let mut notes = vec![
            "one trace per stored curve: raw flow-cell sensorgrams (`Sensorgram Fc=n`) and the control software's reference-subtracted curves (`Subtracted Fc=n-m`, stored as computed, with its time correction); time 0 is the cycle's start".to_string(),
            "report points (`report_points` table) are the control software's averages over each window, as stored".to_string(),
            "event-log codes are listed as recorded, not interpreted".to_string(),
        ];
        if let Some(ev) = &self.blr.evaluation {
            notes.push(format!(
                "evaluation file: the sensorgrams of {} result file(s) as the evaluation software copied them; {} evaluation items, {} fits (models, parameters with standard errors, Chi², the curves fitted) as the evaluation software stored them, not recomputed",
                ev.files.len(),
                ev.items.len(),
                ev.fits.len()
            ));
        }
        if !self.blr.findings.is_empty() {
            notes.push(format!(
                "{} curve(s) or parts were left out; run `check`",
                self.blr.findings.len()
            ));
        }
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: self.descriptor.clone(),
            // the result-file format's own version (`FileTypeVersion`); the software's is in the
            // experiment model
            format_version: lookup(&self.blr.compat, "FileTypeVersion").map(str::to_string),
            plane_count: 0,
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({ "biacore": {
            "application": self.blr.application,
            "compatibility": kv_object(&self.blr.compat),
            "environment": kv_object(&self.blr.environment),
            "chip": kv_object(&self.blr.chip),
            "file_tag": kv_object(&self.blr.file_tag),
            "instrument_configuration": kv_object(&self.blr.bioconf),
            "evaluation_environment": self.blr.evaluation.as_ref().map(|e| kv_object(&e.environment)),
        }}))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        p.insert("traces".into(), Source::Inferred);
        p.insert("tables".into(), Source::Inferred);
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(self
            .cfb
            .entries
            .iter()
            .filter(|e| e.is_stream)
            .map(|e| LsEntry {
                kind: "stream".into(),
                name: e.path.replace('\u{3}', ""),
                offset: None,
                size: Some(e.size),
                image: None,
                details: Value::Null,
            })
            .collect())
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::Usage(
            "a Biacore result file holds sensorgrams (traces) and tables, not images".into(),
        ))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = self.check_headers()?;
        r.performed("every curve's samples read; values checked for non-finite numbers");
        for k in 0..self.blr.curves.len() {
            match self.samples(k) {
                Ok((t, y)) => {
                    let bad = y
                        .iter()
                        .chain(t.iter().flatten())
                        .filter(|v| !v.is_finite())
                        .count();
                    if bad > 0 {
                        r.push(Finding::warning(
                            "non_finite_values",
                            format!("trace {k}: {bad} non-finite values"),
                        ));
                    }
                }
                Err(e) => r.push(Finding::error(
                    "unreadable_curve",
                    format!("trace {k}: {e}"),
                )),
            }
        }
        Ok(r)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), self.descriptor.id.clone());
        r.performed("compound file parsed; every curve's header and stream size checked");
        for p in &self.cfb.problems {
            r.push(Finding::warning("compound_file", p.clone()));
        }
        for f in &self.blr.findings {
            r.push(f.clone());
        }
        Ok(r)
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (one sweep per curve)"
            )));
        }
        let (t, y) = self.samples(index as usize)?;
        let n = y.len() as u64;
        let start = first.min(n) as usize;
        let end = first.saturating_add(max).min(n) as usize;
        let mut channels = Vec::new();
        if let Some(t) = t {
            channels.push(t[start..end].to_vec());
        }
        channels.push(y[start..end].to_vec());
        Ok(Trace {
            trace: index,
            sweep: 0,
            first_sample: start as u64,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let tables = self.tables();
        let (_, cols) = tables.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {index} out of range ({} tables)",
                tables.len()
            ))
        })?;
        let columns = cols
            .iter()
            .map(|(_, _, c)| {
                let v = match c {
                    Column::Numbers(v) | Column::Categories(v, _) => v,
                };
                let n = v.len() as u64;
                let a = first_row.min(n) as usize;
                let b = first_row.saturating_add(max_rows).min(n) as usize;
                v[a..b].to_vec()
            })
            .collect();
        Ok(Table {
            table: index,
            first_row,
            columns,
        })
    }

    fn experiment(&self) -> Option<Experiment> {
        let mut exp = Experiment::default();
        let origin = |from: &str| Origin {
            source: Source::Inferred,
            from: from.into(),
        };
        let mut ins = ExperimentInstrument {
            vendor: Some("Cytiva (Biacore)".into()),
            kind: vocab::term("OBI:0001136"),
            ..ExperimentInstrument::default()
        };
        if let Some(u) = self.env("ProcessingUnit") {
            ins.model = Some(model_name(u));
            exp.provenance.insert(
                "instrument.model".into(),
                origin("Environment ProcessingUnit"),
            );
        }
        if let Some(s) = self.env("InstrumentId") {
            ins.serial = Some(s.to_string());
            exp.provenance.insert(
                "instrument.serial".into(),
                origin("Environment InstrumentId"),
            );
        }
        if let Some(s) = self.env("Application") {
            ins.software = Some(s.to_string());
            exp.provenance.insert(
                "instrument.software".into(),
                origin("Environment Application"),
            );
        }
        if let Some(s) = self.env("Version") {
            ins.software_version = Some(s.to_string());
            exp.provenance.insert(
                "instrument.software_version".into(),
                origin("Environment Version"),
            );
        }
        exp.instrument = Some(ins);
        let mut method = Method {
            name: self.env("RunType").map(str::to_string),
            technique: vocab::term("CHMO:0000624"),
            ..Method::default()
        };
        if method.name.is_some() {
            exp.provenance
                .insert("method.name".into(), origin("Environment RunType"));
        }
        let temps: Vec<&str> = self
            .blr
            .curves
            .iter()
            .filter_map(|c| lookup(&c.keywords, "Temp#"))
            .collect();
        if let Some(t0) = temps.first()
            && temps.iter().all(|t| t == t0)
            && let Ok(t) = t0.trim().parse::<f64>()
        {
            method
                .parameters
                .insert("temperature".into(), Quantity::number(t, "°C"));
            exp.provenance.insert(
                "method.parameters.temperature".into(),
                origin("curve keywords `Temp#` (equal in every curve)"),
            );
        }
        if let Some(n) = lookup(&self.blr.chip, "NoFcs").and_then(|v| v.trim().parse::<u32>().ok())
        {
            method
                .parameters
                .insert("flow_cells".into(), Quantity::plain(n));
            exp.provenance
                .insert("method.parameters.flow_cells".into(), origin("Chip NoFcs"));
        }
        method.parameters.insert(
            "cycles".into(),
            Quantity::plain(self.blr.cycles.len() as u32),
        );
        exp.provenance.insert(
            "method.parameters.cycles".into(),
            origin("`_Cycle N` storages"),
        );
        exp.method = Some(method);
        let start = self.run_start();
        let end = self
            .env("EndTime")
            .and_then(|v| v.trim().parse::<f64>().ok());
        let acq = Acquisition {
            started_at: start.and_then(ole_to_local_iso),
            ended_at: end.and_then(ole_to_local_iso),
            operator: self.env("UserName").map(str::to_string),
            duration_s: match (start, end) {
                (Some(a), Some(b)) if b >= a => Some(((b - a) * 86_400.0).round()),
                _ => None,
            },
            comment: None,
            saved_at: None,
        };
        if acq.started_at.is_some() {
            exp.provenance.insert(
                "acquisition.started_at".into(),
                origin("Environment Timestamp (OLE date, instrument PC local time)"),
            );
        }
        if acq.ended_at.is_some() {
            exp.provenance
                .insert("acquisition.ended_at".into(), origin("Environment EndTime"));
        }
        if acq.operator.is_some() {
            exp.provenance.insert(
                "acquisition.operator".into(),
                origin("Environment UserName"),
            );
        }
        if acq.duration_s.is_some() {
            exp.provenance.insert(
                "acquisition.duration_s".into(),
                origin("Environment EndTime - Timestamp"),
            );
        }
        exp.acquisition = Some(acq);
        let n = self.blr.curves.len();
        let sub = self.blr.curves.iter().filter(|c| c.subtracted).count();
        exp.measurements.push(Measurement {
            kind: MeasurementKind::Trace,
            indices: (0..n as u32).collect(),
            what: format!(
                "surface plasmon resonance sensorgrams: {} cycles, {n} curves ({sub} reference-subtracted)",
                self.blr.cycles.len()
            ),
            technique: vocab::term("CHMO:0000624"),
            terms: Vec::new(),
            parameters: BTreeMap::new(),
        });
        if let Some(ev) = &self.blr.evaluation
            && !ev.fits.is_empty()
            && let Ok(tables) = self.info().map(|i| i.tables)
            && let Some(k) = tables
                .iter()
                .position(|t| t.name.as_deref() == Some("fits"))
        {
            let models: Vec<&str> = ev.fits.iter().fold(Vec::new(), |mut v, f| {
                if !v.contains(&f.model.as_str()) {
                    v.push(&f.model);
                }
                v
            });
            exp.measurements.push(Measurement {
                kind: MeasurementKind::Table,
                indices: vec![k as u32],
                what: format!(
                    "{} fits of the evaluation software ({})",
                    ev.fits.len(),
                    models.join(", ")
                ),
                technique: vocab::term("CHMO:0000624"),
                terms: Vec::new(),
                parameters: BTreeMap::new(),
            });
        }
        crate::complete_provenance(&mut exp);
        Some(exp)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)] // exact values
    use super::*;

    #[test]
    fn ole_dates() {
        assert_eq!(
            ole_to_local_iso(43_082.685_718_090_3).as_deref(),
            Some("2017-12-13T16:27:26")
        );
        assert_eq!(ole_to_local_iso(f64::NAN), None);
        assert_eq!(ole_to_local_iso(-5.0), None);
    }

    #[test]
    fn grids_and_names() {
        let t: Vec<f64> = (1..=2000).map(|i| f64::from(i as f32 * 0.1_f32)).collect();
        assert_eq!(regular_grid(&t), Some((0.1, 0.1)));
        let mut u = t.clone();
        u[500] += 0.05;
        assert_eq!(regular_grid(&u), None);
        assert_eq!(regular_grid(&[1.0]), None);
        assert_eq!(model_name("BiacoreT200"), "Biacore T200");
        assert_eq!(model_name("Biacore 8K"), "Biacore 8K");
        assert_eq!(
            curve_numbers("_Cycle 12/_Window 1/_Curve 3"),
            Some((12, 1, 3))
        );
        assert_eq!(curve_numbers("_Cycle 12/_Window 1"), None);
        assert_eq!(bare("\u{3}Keywords"), "Keywords");
    }

    #[test]
    fn events_and_tables() {
        let (ev, start) = event_log(
            "2\nF0;93;bA\nF0;10;d43082.6867138657\nF109900;33;pR2E1;f50;iSample 1\nbad\nFx;1;\n",
        );
        assert_eq!(ev.len(), 3);
        assert_eq!(start, Some(43_082.686_713_865_7));
        assert_eq!(ev[2].time_s, 109.9);
        assert_eq!(ev[2].arguments, "pR2E1;f50;iSample 1");
        let t = text_table("Cycle\tFc\tAbsResp\n1\t3\t24628.4\n1\t4-3\tN/A\n");
        assert_eq!(t.rows.len(), 2);
        let Column::Categories(codes, labels) = column_of(&["3", "4-3", "3"]) else {
            panic!("categories expected");
        };
        assert_eq!(codes, vec![0.0, 1.0, 0.0]);
        assert_eq!(labels, vec!["3", "4-3"]);
        let Column::Numbers(v) = column_of(&["1.5", "N/A", ""]) else {
            panic!("numbers expected");
        };
        assert!(v[1].is_nan() && v[0] == 1.5);
        assert!(is_result_file("FileType=Result File\nFileTypeVersion=12\n"));
        assert!(!is_result_file("FileType=Method\n"));
        assert_eq!(
            kind_of("FileType=T200 Evaluation File\nFileTypeVersion=4\n"),
            Some(Kind::Evaluation)
        );
    }
}
