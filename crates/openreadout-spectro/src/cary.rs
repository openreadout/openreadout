//! Agilent (Varian) Cary UV-Vis files (`.dsw` Scan, `.bsw` batch Scan, `.bsk` Scanning
//! Kinetics): our notes are `docs/formats/agilent-cary.md`, provenance
//! `docs/provenance/agilent-cary.md` (written from corpus files and the depositors' CSV exports
//! only; no other Cary reader was consulted).
//!
//! The file is a 0x3E-byte header (`0x11` `Varian UV-VIS-NIR`) and a chain of *stores*: a
//! u32-length class name, then a u32 store size counted from the store's first byte. A
//! `TContinuumStore` is one spectrum: a 1,028-byte header (u32, u32 version 149, f32 x min,
//! x max, y min, y max, u32 points, u32, …), the points as interleaved f32 (x, y) pairs, then
//! u32-length Latin-1 texts (name, collection time, operator, application, parameter list,
//! method log, status). A `TBaselineStore` has the same layout and holds the baseline.

use std::collections::BTreeMap;
use std::path::Path;

use openreadout_core::bytes::le_u32;
use openreadout_core::model::{Finding, LsEntry};
use openreadout_core::provenance::Source;
use openreadout_core::source::SourceFile;
use openreadout_core::{Error, Result};
use serde_json::{Value, json};

use crate::common::{
    Facts, Le, Parsed, Rows, SpectrumSet, SpectrumTable, Stored, XValues, num, read_at,
};

/// First bytes of every Cary file of the corpus: a length-prefixed `Varian UV-VIS-NIR`.
pub(crate) const MAGIC: &[u8] = b"\x11Varian UV-VIS-NIR";

const FMT: &str = crate::CARY_FORMAT_ID;
/// Offset of the first store.
const FIRST_STORE: usize = 0x3E;
/// Bytes from the end of a spectrum store's size field to its first (x, y) pair.
const SPECTRUM_HEADER: usize = 1028;
/// The version word of every spectrum (and baseline) store of the corpus.
const SPECTRUM_VERSION: u32 = 149;
/// Largest file read (the corpus files are under 200 kB; a store chain is read whole).
const MAX_FILE: u64 = 256 << 20;
/// Longest text the reader takes as one of a store's texts.
const MAX_TEXT: u32 = 4096;

/// One store of the chain.
#[derive(Debug, Clone)]
struct Store {
    class: String,
    offset: usize,
    size: usize,
    /// First byte after the size field.
    body: usize,
}

/// A spectrum (or baseline) store, decoded.
#[derive(Debug, Clone, Default)]
struct Spectrum {
    baseline: bool,
    store: usize,
    name: String,
    xs: Vec<f32>,
    ys: Vec<f32>,
    header: Value,
    texts: Vec<String>,
    params: Vec<(String, Vec<String>)>,
    collection: Option<String>,
    operator: Option<String>,
    application: Option<(String, String)>,
    method_name: Option<String>,
    method_saved: Option<String>,
    method_log: Vec<String>,
    status: BTreeMap<String, String>,
    scheduled_min: Option<f64>,
    trailing_bytes: usize,
}

impl Spectrum {
    fn param(&self, names: &[&str]) -> Option<String> {
        self.params.iter().find_map(|(k, v)| {
            names
                .iter()
                .any(|n| k.eq_ignore_ascii_case(n))
                .then(|| v.join(" "))
        })
    }
}

/// What `parse` found, and the y values of every spectrum (float32, one run per spectrum) that
/// the dataset serves `read_trace` from.
pub(crate) struct Opened {
    pub(crate) parsed: Parsed,
    pub(crate) values: Vec<u8>,
}

/// A u32-length text at `at` (Latin-1), and the offset after it.
fn text_at(d: &[u8], at: usize, max: u32) -> Option<(String, usize)> {
    let n = le_u32(d, at)?;
    if n > max {
        return None;
    }
    let b = d.bytes_at(at + 4, n as usize)?;
    if !b.iter().all(|&c| c >= 0x20 || c == b'\t') {
        return None;
    }
    Some((
        b.iter().map(|&c| char::from(c)).collect(),
        at + 4 + n as usize,
    ))
}

/// The store chain from 0x3E: `(stores, offset where the chain ended)`.
fn stores(d: &[u8], findings: &mut Vec<Finding>) -> Result<(Vec<Store>, usize)> {
    let mut out = Vec::new();
    let mut at = FIRST_STORE;
    while at + 8 <= d.len() {
        let Some((class, after)) = text_at(d, at, 64) else {
            break;
        };
        if !class.starts_with('T') || class.len() < 2 {
            break;
        }
        let Some(size) = le_u32(d, after) else { break };
        let size = size as usize;
        let body = after + 4;
        if size < body - at {
            return Err(Error::corrupt_at(
                FMT,
                at as u64,
                format!("store {class}: size {size} is shorter than its own header"),
            ));
        }
        let Some(end) = at.checked_add(size).filter(|&e| e <= d.len()) else {
            return Err(Error::corrupt_at(
                FMT,
                at as u64,
                format!(
                    "store {class} of {size} bytes runs past the end of the file ({} bytes): the file is truncated",
                    d.len()
                ),
            ));
        };
        out.push(Store {
            class,
            offset: at,
            size,
            body,
        });
        at = end;
    }
    let rest = d.len().saturating_sub(at);
    if rest != 4 {
        findings.push(Finding::warning(
            "store_chain_end",
            format!(
                "the store chain ends {rest} bytes before the end of the file (every corpus file ends 4 bytes after it)"
            ),
        ));
    }
    Ok((out, at))
}

/// `Collection Time: 12/18/2024 10:12:52 PM` style text → ISO 8601 local clock (month first).
fn us_datetime(s: &str) -> Option<String> {
    let s = s.trim();
    let (date, rest) = s.split_once(' ')?;
    let mut d = date.split('/');
    let mo: u32 = d.next()?.trim().parse().ok()?;
    let day: u32 = d.next()?.trim().parse().ok()?;
    let yr: u32 = d.next()?.trim().parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&day) || !(1970..=2200).contains(&yr) {
        return None;
    }
    let rest = rest.trim();
    let (clock, ampm) = match rest.rsplit_once(' ') {
        Some((c, p)) if p.eq_ignore_ascii_case("AM") || p.eq_ignore_ascii_case("PM") => {
            (c, Some(p.to_ascii_uppercase()))
        }
        _ => (rest, None),
    };
    let mut c = clock.split(':');
    let mut h: u32 = c.next()?.trim().parse().ok()?;
    let mi: u32 = c.next()?.trim().parse().ok()?;
    let se: u32 = c.next().unwrap_or("0").trim().parse().ok()?;
    match ampm.as_deref() {
        Some("AM") if h == 12 => h = 0,
        Some("PM") if h < 12 => h += 12,
        _ => {}
    }
    if h > 23 || mi > 59 || se > 60 {
        return None;
    }
    Some(format!("{yr:04}-{mo:02}-{day:02}T{h:02}:{mi:02}:{se:02}"))
}

/// Seconds since 1970 of an ISO local clock from [`us_datetime`] (as if UTC; for differences).
fn clock_seconds(iso: &str) -> Option<f64> {
    openreadout_core::time::iso8601_to_unix(iso)
}

/// A parameter text: Scan 3.00 writes fixed 302-character fields (the name in the first 34
/// columns), Scan 5.0 `␣␣name␣␣value[␣␣value]`; both split on runs of two or more spaces.
fn split_param(t: &str) -> Option<(String, Vec<String>)> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut spaces = 0;
    for ch in t.trim().chars() {
        if ch == ' ' {
            spaces += 1;
            continue;
        }
        if spaces >= 2 && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        } else if spaces == 1 && !cur.is_empty() {
            cur.push(' ');
        }
        spaces = 0;
        cur.push(ch);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    let mut it = parts.into_iter();
    let name = it.next()?;
    Some((name, it.collect()))
}

/// The part of a spectrum store's texts being read.
#[derive(PartialEq)]
enum Part {
    Head,
    Params,
    Log,
    Tail,
}

/// Lower-case hex of `b`.
fn hex(b: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(2 * b.len());
    for x in b {
        s.push(char::from(DIGITS[usize::from(x >> 4)]));
        s.push(char::from(DIGITS[usize::from(x & 0xF)]));
    }
    s
}

/// Decode one spectrum or baseline store.
fn spectrum(d: &[u8], s: &Store, index: usize, findings: &mut Vec<Finding>) -> Result<Spectrum> {
    let h = s.body;
    let field = |k: usize| le_u32(d, h + 4 * k);
    let fl = |k: usize| d.f32_at(h + 4 * k).map(f64::from);
    let version = field(1).unwrap_or(0);
    if version != SPECTRUM_VERSION {
        return Err(Error::unsupported(
            FMT,
            format!("{} version {version}", s.class),
            "Only version-149 spectrum stores have been validated; please share this file.",
        ));
    }
    let n = field(6).unwrap_or(0) as usize;
    let data = h + SPECTRUM_HEADER;
    let end = s.offset + s.size;
    let data_end = n
        .checked_mul(8)
        .and_then(|b| data.checked_add(b))
        .filter(|&e| e <= end)
        .ok_or_else(|| {
            Error::corrupt_at(
                FMT,
                s.offset as u64,
                format!("{}: {n} points do not fit in its {} bytes", s.class, s.size),
            )
        })?;
    let mut xs = Vec::with_capacity(n);
    let mut ys = Vec::with_capacity(n);
    for i in 0..n {
        xs.push(d.f32_at(data + 8 * i).unwrap_or(f32::NAN));
        ys.push(d.f32_at(data + 8 * i + 4).unwrap_or(f32::NAN));
    }
    let (xmin, xmax, ymin, ymax) = (fl(2), fl(3), fl(4), fl(5));
    let finite = |v: &[f32]| -> Option<(f64, f64)> {
        let mut it = v.iter().copied().filter(|x| x.is_finite()).map(f64::from);
        let first = it.next()?;
        Some(it.fold((first, first), |(a, b), x| (a.min(x), b.max(x))))
    };
    let (dx, dy) = (finite(&xs), finite(&ys));
    let agrees = |stored: Option<f64>, data: Option<f64>| match (stored, data) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-6 * a.abs().max(1.0),
        _ => false,
    };
    if n > 0
        && !(agrees(xmin, dx.map(|v| v.0))
            && agrees(xmax, dx.map(|v| v.1))
            && agrees(ymin, dy.map(|v| v.0))
            && agrees(ymax, dy.map(|v| v.1)))
    {
        findings.push(Finding::warning(
            "stored_extremes",
            format!(
                "spectrum {index}: the store's x/y extremes ({xmin:?}–{xmax:?}, {ymin:?}–{ymax:?}) are not the extremes of its values ({dx:?}, {dy:?})"
            ),
        ));
    }
    let header = json!({
        "offset": s.offset, "size": s.size,
        "word_0": field(0), "version": version,
        "x_min": xmin.map(num), "x_max": xmax.map(num), "y_min": ymin.map(num), "y_max": ymax.map(num),
        "points": n, "word_7": field(7),
    });
    // the texts after the points, up to the first bytes that are not a text
    let mut texts = Vec::new();
    let mut at = data_end;
    while at + 4 <= end {
        match text_at(d, at, MAX_TEXT) {
            Some((t, next)) if next <= end => {
                texts.push(t);
                at = next;
            }
            _ => break,
        }
    }
    let mut sp = Spectrum {
        baseline: s.class == "TBaselineStore",
        store: index,
        name: texts.first().cloned().unwrap_or_default(),
        xs,
        ys,
        header,
        trailing_bytes: end - at,
        ..Spectrum::default()
    };
    let mut part = Part::Head;
    for t in texts.iter().skip(1) {
        let tt = t.trim();
        if tt.starts_with("Parameter List") {
            part = Part::Params;
            continue;
        }
        if tt.starts_with("Method Log") {
            part = Part::Log;
            continue;
        }
        match part {
            Part::Head => {
                if let Some(v) = tt.strip_prefix("Collection Time:") {
                    sp.collection = us_datetime(v);
                } else if let Some(v) = tt.strip_prefix("Operator Name") {
                    let v = v.trim_start().trim_start_matches(':').trim();
                    if !v.is_empty() {
                        sp.operator = Some(v.to_string());
                    }
                } else if let Some(i) = tt.find(" Version") {
                    // `Scan Software Version: 3.00(182)`, `Scan Version 5.0.0.999`
                    let app = tt[..i].trim_end_matches(" Software").trim();
                    let ver = tt[i + " Version".len()..].trim_start_matches(':').trim();
                    if !app.is_empty() && !ver.is_empty() {
                        sp.application = Some((app.to_string(), ver.to_string()));
                    }
                }
            }
            Part::Params => {
                if let Some((k, v)) = split_param(t) {
                    sp.params.push((k, v));
                }
            }
            Part::Log => {
                if let Some(v) = tt.strip_prefix("Method Name :") {
                    let v = v.trim();
                    if !v.is_empty() {
                        sp.method_name = Some(v.to_string());
                    }
                } else if let Some(v) = tt.strip_prefix("Date/Time stamp:") {
                    sp.method_saved = us_datetime(v);
                }
                if tt == "End Method Modifications" {
                    part = Part::Tail;
                }
                sp.method_log.push(t.clone());
            }
            Part::Tail => {
                // `<SBW (nm)> , 2.000`, `[Time] , 2.000`
                if let Some((k, v)) = tt.split_once(" , ") {
                    let key = k.trim_matches(['<', '>', '[', ']']).trim().to_string();
                    if k.starts_with('[') && key.eq_ignore_ascii_case("Time") {
                        sp.scheduled_min = v.trim().parse().ok();
                    }
                    sp.status.insert(key, v.trim().to_string());
                }
            }
        }
    }
    sp.texts = texts;
    Ok(sp)
}

/// Our x quantity, unit and data type for the spectrum's `X Mode`.
fn x_axis(sp: &Spectrum) -> (&'static str, Option<&'static str>, bool) {
    match sp.param(&["X Mode"]).as_deref().map(str::trim) {
        Some(m) if m.eq_ignore_ascii_case("Nanometers") => ("wavelength", Some("nm"), false),
        // Scanning Kinetics names no X mode; its method log sets `X Start nm`/`X Stop nm`
        None => ("wavelength", Some("nm"), true),
        Some(_) => ("x", None, false),
    }
}

/// Our y name and unit for the spectrum's `Y Mode` (`Ordinate mode` in kinetics files).
fn y_axis(sp: &Spectrum) -> (String, Option<&'static str>, bool) {
    let mode = sp.param(&["Y Mode", "Ordinate mode"]).unwrap_or_default();
    match mode.trim() {
        "Abs" => ("absorbance".into(), Some("AU"), true),
        "%T" => ("transmittance".into(), Some("%"), false),
        "%R" => ("reflectance".into(), Some("%"), false),
        "" => ("y".into(), None, false),
        other => (other.to_lowercase(), None, false),
    }
}

/// Parse a Cary file held in `f`.
pub(crate) fn parse(f: &SourceFile, path: &Path, file_len: u64) -> Result<Opened> {
    if file_len > MAX_FILE {
        return Err(Error::unsupported(
            FMT,
            format!("a {file_len}-byte file"),
            "Cary files of the corpus are under 1 MB; this one is read whole and is too large.",
        ));
    }
    let d = read_at(f, path, 0, file_len, file_len)?;
    if !d.starts_with(MAGIC) {
        return Err(Error::corrupt(
            FMT,
            "not a Cary file: it does not start with `Varian UV-VIS-NIR`",
        ));
    }
    let mut parsed = Parsed::default();
    let (chain, chain_end) = stores(&d, &mut parsed.findings)?;
    let mut spectra = Vec::new();
    for (k, s) in chain.iter().enumerate() {
        if matches!(s.class.as_str(), "TContinuumStore" | "TBaselineStore") {
            spectra.push(spectrum(&d, s, k, &mut parsed.findings)?);
        }
        parsed.entries.push(LsEntry {
            kind: "store".into(),
            name: s.class.clone(),
            offset: Some(s.offset as u64),
            size: Some(s.size as u64),
            image: None,
            details: Value::Null,
        });
    }
    if !spectra.iter().any(|s| !s.baseline) {
        return Err(Error::unsupported(
            FMT,
            "a Cary file without spectrum stores",
            "No TContinuumStore was found; only files holding spectra are read.",
        ));
    }

    // Sets: spectra that share their x values and y mode (sample spectra first, then baselines)
    let mut values: Vec<u8> = Vec::new();
    let mut groups: Vec<(bool, Vec<u32>, String, Vec<usize>)> = Vec::new();
    for (i, sp) in spectra.iter().enumerate() {
        let xbits: Vec<u32> = sp.xs.iter().map(|x| x.to_bits()).collect();
        let (y, _, _) = y_axis(sp);
        match groups
            .iter_mut()
            .find(|g| g.0 == sp.baseline && g.1 == xbits && g.2 == y)
        {
            Some(g) => g.3.push(i),
            None => groups.push((sp.baseline, xbits, y, vec![i])),
        }
    }
    groups.sort_by_key(|g| g.0);
    let mut assumed_nm = false;
    for (baseline, _, _, members) in &groups {
        let first = &spectra[members[0]];
        let (xq, xu, assumed) = x_axis(first);
        assumed_nm |= assumed;
        let (yname, yunit, known) = y_axis(first);
        if !known {
            parsed.findings.push(Finding::warning(
                "y_mode",
                format!(
                    "Y mode {:?}: only absorbance (`Abs`) has been validated against an export; values are returned as stored",
                    first.param(&["Y Mode", "Ordinate mode"]).unwrap_or_default()
                ),
            ));
        }
        if xq == "x" {
            parsed.findings.push(Finding::warning(
                "x_mode",
                format!(
                    "X mode {:?}: only nanometers have been validated; the x values are returned without a unit",
                    first.param(&["X Mode"]).unwrap_or_default()
                ),
            ));
        }
        let points = first.xs.len() as u64;
        let mut rows = Vec::new();
        for &m in members {
            rows.push(values.len() as u64);
            for y in &spectra[m].ys {
                values.extend_from_slice(&y.to_le_bytes());
            }
        }
        let xs: Vec<f64> = first.xs.iter().map(|&v| f64::from(v)).collect();
        let regular = xs.len() > 1 && {
            let step = (xs[xs.len() - 1] - xs[0]) / (xs.len() - 1) as f64;
            xs.iter()
                .enumerate()
                .all(|(i, &x)| (x - (xs[0] + step * i as f64)).abs() <= 1e-9 * x.abs().max(1.0))
        };
        let x = if regular {
            XValues::Regular {
                first: xs[0],
                last: xs[xs.len() - 1],
            }
        } else {
            XValues::Listed(xs)
        };
        let mut extra = BTreeMap::new();
        extra.insert(
            "spectrum_names".into(),
            json!(
                members
                    .iter()
                    .map(|&m| spectra[m].name.clone())
                    .collect::<Vec<_>>()
            ),
        );
        extra.insert(
            "y_mode".into(),
            json!(first.param(&["Y Mode", "Ordinate mode"])),
        );
        if let Some(xm) = first.param(&["X Mode"]) {
            extra.insert("x_mode".into(), json!(xm));
        }
        let name = if *baseline {
            format!("baseline {yname}")
        } else {
            yname.clone()
        };
        let set_index = parsed.sets.len() as u32;
        parsed.sets.push(SpectrumSet {
            name,
            x_quantity: xq,
            x_unit: xu.map(str::to_string),
            x,
            y_name: yname,
            y_unit: yunit.map(str::to_string),
            points,
            count: u32::try_from(members.len()).unwrap_or(u32::MAX),
            rows: Rows::Listed(rows),
            stored: Stored::F32,
            scale: 1.0,
            data_type: "UV/VIS SPECTRUM",
            extra,
        });
        // per-spectrum times when a set holds several spectra (batch and kinetics files)
        if members.len() > 1 {
            let t0 = spectra[members[0]]
                .collection
                .as_deref()
                .and_then(clock_seconds);
            let mut cols = Vec::new();
            let collected: Vec<f64> = members
                .iter()
                .map(
                    |&m| match (spectra[m].collection.as_deref().and_then(clock_seconds), t0) {
                        (Some(t), Some(z)) => t - z,
                        _ => f64::NAN,
                    },
                )
                .collect();
            cols.push(("collected_s".to_string(), Some("s".to_string()), collected));
            if members.iter().any(|&m| spectra[m].scheduled_min.is_some()) {
                cols.push((
                    "scheduled_min".to_string(),
                    Some("min".to_string()),
                    members
                        .iter()
                        .map(|&m| spectra[m].scheduled_min.unwrap_or(f64::NAN))
                        .collect(),
                ));
            }
            parsed.tables.push(SpectrumTable {
                name: format!("{} spectra", parsed.sets[set_index as usize].name),
                trace: set_index,
                columns: cols,
            });
        }
    }

    // Experiment facts: the first sample spectrum
    let first = spectra.iter().find(|s| !s.baseline).unwrap_or(&spectra[0]);
    let mut facts = Facts {
        vendor: Some((
            "Agilent (Varian)".into(),
            "file header (`Varian UV-VIS-NIR`)".into(),
        )),
        ..Facts::default()
    };
    Facts::text(
        &mut facts.sample_name,
        &first.name,
        "the spectrum's name (first text)",
    );
    if let Some(o) = &first.operator {
        Facts::text(&mut facts.operator, o, "`Operator Name :` text");
    }
    if let Some(m) = first.param(&["Instrument"]) {
        Facts::text(&mut facts.model, &m, "parameter `Instrument`");
    }
    if let Some((app, ver)) = &first.application {
        Facts::text(
            &mut facts.software,
            app,
            "the application text (`… Version …`)",
        );
        Facts::text(
            &mut facts.software_version,
            ver,
            "the application text (`… Version …`)",
        );
    }
    if let Some(t) = &first.collection {
        facts.started_at = Some((
            t.clone(),
            "`Collection Time:` text (local clock, month/day/year)".into(),
        ));
    }
    if let Some(m) = &first.method_name {
        Facts::text(&mut facts.method_name, m, "`Method Name :` text");
    }
    let inf = Source::Inferred;
    let numeric = |sp: &Spectrum, names: &[&str]| -> Option<f64> {
        sp.param(names).and_then(|v| {
            v.split_whitespace()
                .next()
                .and_then(|x| x.parse::<f64>().ok())
        })
    };
    for (ours, names, unit) in [
        (
            "scan_rate",
            &["UV-Vis Scan Rate (nm/min)"][..],
            Some("nm/min"),
        ),
        (
            "data_interval",
            &["UV-Vis Data Interval (nm)"][..],
            Some("nm"),
        ),
        (
            "averaging_time",
            &["UV-Vis Ave. Time (sec)", "Ave Time (sec)"][..],
            Some("s"),
        ),
        ("spectral_bandwidth", &["UV-Vis SBW (nm)"][..], Some("nm")),
        ("scan_start", &["Start (nm)"][..], Some("nm")),
        ("scan_stop", &["Stop (nm)"][..], Some("nm")),
        (
            "source_changeover",
            &["Source Changeover (nm)"][..],
            Some("nm"),
        ),
    ] {
        if let Some(v) = numeric(first, names) {
            facts.number(ours, v, unit, &format!("parameter `{}`", names[0]), inf);
        }
    }
    for (ours, names) in [
        ("beam_mode", &["Beam Mode"][..]),
        ("baseline_correction", &["Baseline Correction"][..]),
        (
            "baseline_type",
            &["Baseline Type", "Baseline selection"][..],
        ),
        ("slit_height", &["Slit Height"][..]),
        ("signal_to_noise_mode", &["Signal-to-noise Mode"][..]),
        ("cycle_mode", &["Cycle Mode"][..]),
    ] {
        if let Some(v) = first.param(names) {
            facts.word(ours, &v, &format!("parameter `{}`", names[0]), inf);
        }
    }
    parsed.facts = facts;
    parsed.format_version = first.application.as_ref().map(|(a, v)| format!("{a} {v}"));

    // the vendor tree: every store and every spectrum's texts, as written
    let vendor_spectra: Vec<Value> = spectra
        .iter()
        .map(|sp| {
            json!({
                "store": sp.store,
                "kind": if sp.baseline { "baseline" } else { "spectrum" },
                "name": sp.name,
                "header": sp.header,
                "collection_time": sp.collection,
                "operator": sp.operator,
                "application": sp.application.as_ref().map(|(a, v)| json!({"name": a, "version": v})),
                "parameters": sp.params.iter().map(|(k, v)| json!({"name": k, "values": v})).collect::<Vec<_>>(),
                "method_name": sp.method_name,
                "method_saved": sp.method_saved,
                "method_log": sp.method_log,
                "status": sp.status,
                "scheduled_min": sp.scheduled_min.map(num),
                "trailing_bytes": sp.trailing_bytes,
            })
        })
        .collect();
    parsed.vendor = json!({ "cary": {
        "magic": "Varian UV-VIS-NIR",
        "stores": chain.iter().map(|s| json!({"class": s.class, "offset": s.offset, "size": s.size})).collect::<Vec<_>>(),
        "chain_end": chain_end,
        "file_tail": d.get(chain_end..).map(hex),
        "spectra": vendor_spectra,
    }});
    parsed.notes.push(
        "values as stored (float32 x, y pairs); the x values are each point's stored wavelength"
            .into(),
    );
    if spectra.iter().any(|s| s.scheduled_min.is_some()) {
        parsed.notes.push(
            "kinetics spectra: `scheduled_min` is the method's time of each spectrum (the `[Time]` text); `collected_s` when it was collected (its `Collection Time`), which can differ"
                .into(),
        );
    }
    if assumed_nm {
        parsed.notes.push(
            "x unit assumed nm: the parameter list names no X mode (Scanning Kinetics); the method log's `X Start nm`/`X Stop nm` bound the x values"
                .into(),
        );
    }
    let skipped: Vec<&str> = chain
        .iter()
        .map(|s| s.class.as_str())
        .filter(|c| !matches!(*c, "TContinuumStore" | "TBaselineStore"))
        .collect();
    if !skipped.is_empty() {
        parsed.notes.push(format!(
            "stores listed by `info --view structure`, not decoded: {}",
            skipped.join(", ")
        ));
    }
    parsed.provenance.insert("traces".into(), Source::Inferred);
    Ok(Opened { parsed, values })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(
            us_datetime("12/18/2024 10:12:52 PM").as_deref(),
            Some("2024-12-18T22:12:52")
        );
        assert_eq!(
            us_datetime(" 8/3/2020 12:26:08 PM").as_deref(),
            Some("2020-08-03T12:26:08")
        );
        assert_eq!(
            us_datetime("8/3/2020 12:01:34 AM").as_deref(),
            Some("2020-08-03T00:01:34")
        );
        assert_eq!(us_datetime("18/12/2024 10:12:52 PM"), None);
        assert_eq!(us_datetime("garbage"), None);
    }

    #[test]
    fn params() {
        let fixed = format!("{:<34}{:<268}", "Instrument", "Cary 4000");
        assert_eq!(
            split_param(&fixed),
            Some(("Instrument".into(), vec!["Cary 4000".into()]))
        );
        assert_eq!(
            split_param("  Cycle Time(min)  2.0  30.0"),
            Some(("Cycle Time(min)".into(), vec!["2.0".into(), "30.0".into()]))
        );
        assert_eq!(
            split_param("  UV-Vis Scan Rate (nm/min)  600.000"),
            Some(("UV-Vis Scan Rate (nm/min)".into(), vec!["600.000".into()]))
        );
        assert_eq!(
            split_param("  Baseline File Name  "),
            Some(("Baseline File Name".into(), vec![]))
        );
    }
}
