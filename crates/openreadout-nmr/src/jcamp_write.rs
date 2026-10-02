//! JCAMP-DX writer (`export --to jcamp`): one trace (an NMR FID or spectrum, a JCAMP-DX
//! spectrum, a chromatogram, ...) as `##XYDATA=(X++(Y..Y))` when it has one channel and one
//! sweep, otherwise as `##NTUPLES=` pages (one page per channel and sweep; complex NMR data as
//! the `R`/`I` pages of JCAMP-DX 5.01). Ordinates are written as integers times a per-channel
//! factor (`##YFACTOR=`, NTUPLES `##FACTOR=`) in DIFDUP form with Y-value checks (default) or
//! AFFN. See `docs/formats/jcamp-dx.md` § Writing.
//!
//! The factor is chosen so the values round-trip bit-exactly whenever they are integer multiples
//! of one factor (the channel's own scale, e.g. a JCAMP-DX file's `YFACTOR`, or a power of two,
//! e.g. Bruker's `2^NC`); otherwise ordinates are quantized to 32-bit integers and the report
//! states the largest error (at most half the factor). The file is written under a temporary
//! name, re-read with this crate's JCAMP-DX reader (every ordinate compared bit for bit with
//! `integer × factor`, the abscissa within its rounding, no Y-check failures), then renamed.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use openreadout_core::experiment::Experiment;
use openreadout_core::model::{FileInfo, TraceInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::jcamp_dataset::JcampDataset;

/// Samples read from the source per call.
const CHUNK: u64 = 1 << 20;
/// Most ordinates one export holds in memory (all pages together).
const MAX_VALUES: u64 = 1 << 26;
/// Longest data line (JCAMP-DX: at most 80 characters).
const LINE: usize = 80;
/// Largest integer an f64 holds exactly: the bound for exact ordinates.
const EXACT_LIMIT: f64 = 9_007_199_254_740_992.0;
/// Quantized ordinates fit a signed 32-bit integer.
const LOSSY_LIMIT: f64 = 2_147_483_647.0;

/// How ordinates are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum JcampEncoding {
    /// ASDF: SQZ first ordinate, DIF differences, DUP repeats, and a Y-value check at the start
    /// of every line (the compressed form most readers expect).
    #[default]
    Difdup,
    /// Plain AFFN numbers separated by spaces.
    Affn,
}

impl JcampEncoding {
    /// `difdup` or `affn`.
    pub fn id(self) -> &'static str {
        match self {
            JcampEncoding::Difdup => "difdup",
            JcampEncoding::Affn => "affn",
        }
    }
}

/// What to write.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct JcampExportOptions {
    /// Trace index (default 0).
    pub trace: Option<u32>,
    /// Only this sweep (default: every sweep, as NTUPLES pages).
    pub sweep: Option<u32>,
    /// Samples `[first, last]` of each sweep (zero-based, inclusive; `None` = to the end).
    pub rows: Option<(u64, Option<u64>)>,
    /// DIFDUP (default) or AFFN.
    pub encoding: JcampEncoding,
    /// Replace an existing output file.
    pub overwrite: bool,
}

/// Output of `export --to jcamp`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct JcampExportReport {
    /// The input file.
    pub input: String,
    /// The JCAMP-DX file written.
    pub output: String,
    /// Always `jcamp-dx`.
    pub format: String,
    /// `##JCAMP-DX=` version written (`5.01`).
    pub jcamp_version: String,
    /// `##DATA TYPE=` written (`NMR FID`, `NMR SPECTRUM`, or the source's data type).
    pub data_type: String,
    /// `XYDATA` or `NTUPLES`.
    pub data_class: String,
    /// Trace index written.
    pub trace: u32,
    /// Sweeps written.
    pub sweeps: Vec<u32>,
    /// Data tables (NTUPLES pages; 1 for XYDATA).
    pub pages: u32,
    /// First sample of each sweep written (zero-based).
    pub first_sample: u64,
    /// Points per sweep written.
    pub samples_written: u64,
    /// Channels written (one table per channel and sweep).
    pub channels_written: u32,
    /// `difdup` or `affn`.
    pub encoding: String,
    /// The ordinate factor of each channel (value = integer × factor).
    pub factors: Vec<f64>,
    /// True when every value round-trips bit-exactly.
    pub exact: bool,
    /// Largest |written − source| over all values (0 when `exact`; at most half the factor).
    pub max_abs_error: f64,
    /// Size of the written file in bytes.
    pub bytes_written: u64,
    /// True when the file was re-read and every ordinate matched.
    pub verified: bool,
}

/// Default output path: `<stem>.jdx` (`<stem>.traceT.jdx`, `<stem>.sweepS.jdx` when selected).
pub fn default_jcamp_output(base: &Path, opts: &JcampExportOptions) -> PathBuf {
    let stem = base
        .file_stem()
        .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
    let mid = match (opts.trace.unwrap_or(0), opts.sweep) {
        (0, None) => String::new(),
        (0, Some(s)) => format!(".sweep{s}"),
        (t, None) => format!(".trace{t}"),
        (t, Some(s)) => format!(".trace{t}.sweep{s}"),
    };
    base.with_file_name(format!("{stem}{mid}.jdx"))
}

/// `2^e` as an f64 (normal or subnormal).
fn pow2(e: i32) -> f64 {
    if e >= -1022 {
        f64::from_bits(((e + 1023) as u64) << 52)
    } else {
        f64::from_bits(1u64 << (e + 1074).max(0))
    }
}

/// Exponent of the lowest set bit of a finite, non-zero `v` (`v = odd × 2^e`).
fn low_bit_exponent(v: f64) -> i32 {
    let bits = v.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    let (m, e) = if exp == 0 {
        (frac, -1074)
    } else {
        (frac | (1u64 << 52), exp - 1075)
    };
    e + m.trailing_zeros() as i32
}

/// Integers of `vals` for factor `f` when every value is exactly `integer × f`.
#[allow(clippy::float_cmp)] // exactness is the point: the read-back must be bit-identical
fn exact_ints(vals: &[f64], f: f64) -> Option<Vec<i64>> {
    if !(f.is_finite() && f > 0.0) {
        return None;
    }
    let mut out = Vec::with_capacity(vals.len());
    for &v in vals {
        let q = (v / f).round();
        if q.abs() > EXACT_LIMIT || q * f != v {
            return None;
        }
        out.push(q as i64);
    }
    Some(out)
}

/// The factor and integers for one channel: exact with the channel's own scale, else exact with
/// a power of two, else quantized to 32-bit integers. Returns `(factor, ints, exact, max_err)`.
fn choose(vals: &[f64], hint: f64) -> (f64, Vec<i64>, bool, f64) {
    let max = vals.iter().fold(0f64, |m, v| m.max(v.abs()));
    if max == 0.0 {
        return (1.0, vec![0; vals.len()], true, 0.0);
    }
    if let Some(ints) = exact_ints(vals, hint) {
        return (hint, ints, true, 0.0);
    }
    let low = vals
        .iter()
        .filter(|v| **v != 0.0)
        .map(|v| low_bit_exponent(*v))
        .min()
        .unwrap_or(0);
    let f = pow2(low);
    if let Some(ints) = exact_ints(vals, f) {
        return (f, ints, true, 0.0);
    }
    // quantize: a power of two so that the largest magnitude fits 32 bits
    let e = (max / LOSSY_LIMIT).log2().ceil() as i32;
    let f = pow2(e.clamp(-1074, 1023));
    let mut err = 0f64;
    let ints = vals
        .iter()
        .map(|&v| {
            let q = (v / f).round();
            err = err.max((q * f - v).abs());
            q as i64
        })
        .collect();
    (f, ints, false, err)
}

fn sqz(v: i64) -> String {
    let s = v.unsigned_abs().to_string();
    let lead = s.as_bytes()[0] - b'0';
    let c = if v < 0 {
        (b'a' + lead - 1) as char
    } else if lead == 0 {
        '@'
    } else {
        (b'A' + lead - 1) as char
    };
    format!("{c}{}", &s[1..])
}

fn dif(d: i64) -> String {
    let s = d.unsigned_abs().to_string();
    let lead = s.as_bytes()[0] - b'0';
    let c = if d < 0 {
        (b'j' + lead - 1) as char
    } else if lead == 0 {
        '%'
    } else {
        (b'J' + lead - 1) as char
    };
    format!("{c}{}", &s[1..])
}

fn dup(k: usize) -> String {
    let s = k.to_string();
    let lead = s.as_bytes()[0] - b'0';
    let c = if lead == 9 {
        's'
    } else {
        (b'S' + lead - 1) as char
    };
    format!("{c}{}", &s[1..])
}

/// DIFDUP lines of one table. Every line but a lone single-point line ends in DIF form, and the
/// next line starts with its last ordinate again (the Y-value check); a final line repeats the
/// last ordinate as the check of the last line.
#[allow(clippy::many_single_char_names)]
fn difdup_lines(x: &dyn Fn(usize) -> String, y: &[i64], out: &mut String) {
    let n = y.len();
    if n == 0 {
        return;
    }
    let mut i = 0;
    loop {
        let mut line = x(i);
        line.push_str(&sqz(y[i]));
        let mut j = i;
        while j + 1 < n {
            let d = y[j + 1] - y[j];
            // DUP counts stay single characters (at most 9): the `jcamp` Python package does
            // not read multi-digit counts; a longer run repeats DIF + DUP.
            let mut k = 1;
            while k < 9 && j + 1 + k < n && y[j + 1 + k] - y[j + k] == d {
                k += 1;
            }
            let mut tok = dif(d);
            if k > 1 {
                tok.push_str(&dup(k));
            }
            if line.len() + tok.len() > LINE {
                // Fall back to one difference without the DUP count; every line takes at least
                // one difference, so every line ends in DIF form.
                let one = dif(d);
                if j == i || line.len() + one.len() <= LINE {
                    line.push_str(&one);
                    j += 1;
                    continue;
                }
                break;
            }
            line.push_str(&tok);
            j += k;
        }
        out.push_str(&line);
        out.push('\n');
        if j + 1 >= n {
            if j > i {
                // Y-value check of the last line
                out.push_str(&x(n - 1));
                out.push_str(&sqz(y[n - 1]));
                out.push('\n');
            }
            return;
        }
        i = j;
    }
}

/// AFFN lines: the abscissa of the first ordinate, then ordinates separated by spaces.
fn affn_lines(x: &dyn Fn(usize) -> String, y: &[i64], out: &mut String) {
    let mut i = 0;
    while i < y.len() {
        let mut line = x(i);
        let mut j = i;
        while j < y.len() {
            let tok = format!(" {}", y[j]);
            if j > i && line.len() + tok.len() > LINE {
                break;
            }
            line.push_str(&tok);
            j += 1;
        }
        out.push_str(&line);
        out.push('\n');
        i = j;
    }
}

/// JCAMP-DX unit label of one of our unit strings.
fn jcamp_unit(u: Option<&str>) -> String {
    match u.map(str::trim) {
        None | Some("") => "ARBITRARY UNITS".into(),
        Some(u) => match u.to_ascii_lowercase().as_str() {
            "ppm" => "PPM".into(),
            "hz" => "HZ".into(),
            "khz" => "KHZ".into(),
            "s" | "sec" | "seconds" => "SECONDS".into(),
            "ms" => "MS".into(),
            "min" | "minutes" => "MINUTES".into(),
            "1/cm" | "cm-1" | "cm^-1" => "1/CM".into(),
            "nm" | "nanometers" => "NANOMETERS".into(),
            "um" | "µm" | "micrometers" => "MICROMETERS".into(),
            "m/z" => "M/Z".into(),
            _ => u.to_ascii_uppercase(),
        },
    }
}

/// `##YUNITS=` of channel `c`: the optical-spectroscopy names (`ABSORBANCE`, `TRANSMITTANCE`,
/// `REFLECTANCE`, `KUBELKA-MUNK`) when the trace says what it measures (`extra.y_quantity`,
/// fractions only: percentages keep their unit), else the channel's unit.
fn y_units(t: &TraceInfo, c: usize) -> String {
    let unit = t.channels.get(c).and_then(|ch| ch.unit.as_deref());
    let named = match (t.extra.get("y_quantity").and_then(Value::as_str), unit) {
        (Some("absorbance"), None | Some("AU")) => Some("ABSORBANCE"),
        (Some("transmittance"), None) => Some("TRANSMITTANCE"),
        (Some("reflectance"), None) => Some("REFLECTANCE"),
        (Some("kubelka_munk"), None) => Some("KUBELKA-MUNK"),
        _ => None,
    };
    named.map_or_else(|| jcamp_unit(unit), str::to_string)
}

/// A header value on one line (JCAMP-DX values end at the line; `$$` would start a comment).
fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ").replace("$$", "$ $")
}

fn text<'a>(t: &'a TraceInfo, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| t.extra.get(*k).and_then(Value::as_str))
        .filter(|s| !s.trim().is_empty())
}

fn number(t: &TraceInfo, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|k| t.extra.get(*k).and_then(Value::as_f64))
        .filter(|v| v.is_finite())
}

/// `(first, step, unit)` of the abscissa: the regular axis in `extra.axis`, else time from the
/// sample rate, else the sample index.
fn abscissa(t: &TraceInfo) -> (f64, f64, String) {
    if let Some(a) = t.extra.get("axis")
        && let (Some(first), Some(step)) = (
            a.get("first").and_then(Value::as_f64),
            a.get("step").and_then(Value::as_f64),
        )
    {
        let unit = a.get("unit").and_then(Value::as_str);
        let unit = match (unit, a.get("quantity").and_then(Value::as_str)) {
            (Some(u), _) => jcamp_unit(Some(u)),
            (None, Some("time")) => "SECONDS".into(),
            _ => "ARBITRARY UNITS".into(),
        };
        return (first, step, unit);
    }
    let start = t.start_s.unwrap_or(0.0);
    if t.sample_rate_hz > 0.0 {
        return (start, 1.0 / t.sample_rate_hz, "SECONDS".into());
    }
    (0.0, 1.0, "POINTS".into())
}

/// `##DATA TYPE=` for the trace.
fn data_type(info: &FileInfo, t: &TraceInfo) -> String {
    if let Some(d) = text(t, &["data_type"]) {
        return one_line(d).to_ascii_uppercase();
    }
    match t.extra.get("kind").and_then(Value::as_str) {
        Some("time_domain") => return "NMR FID".into(),
        Some("processed_spectrum") => return "NMR SPECTRUM".into(),
        _ => {}
    }
    let quantity = t
        .extra
        .get("axis")
        .and_then(|a| a.get("quantity"))
        .and_then(Value::as_str);
    match (info.format.family.as_str(), quantity) {
        ("nmr", Some("time")) => "NMR FID".into(),
        ("nmr", _) | (_, Some("chemical_shift")) => "NMR SPECTRUM".into(),
        ("chromatography", _) => "CHROMATOGRAM".into(),
        _ => "UNKNOWN".into(),
    }
}

/// `YYYY/MM/DD HH:MM:SS` from an ISO-8601 time, for `##LONGDATE=`.
fn long_date(iso: &str) -> Option<String> {
    let b = iso.as_bytes();
    let ok = b.len() >= 19
        && b[4] == b'-'
        && b[7] == b'-'
        && (b[10] == b'T' || b[10] == b' ')
        && b[13] == b':'
        && b[16] == b':';
    ok.then(|| {
        format!(
            "{}/{}/{} {}",
            &iso[0..4],
            &iso[5..7],
            &iso[8..10],
            &iso[11..19]
        )
    })
}

/// Symbols of the dependent variables: `R`/`I` for a real/imaginary pair, `Y` for one channel,
/// `Y1`, `Y2`, ... otherwise.
fn symbols(t: &TraceInfo) -> Vec<String> {
    let names: Vec<String> = t
        .channels
        .iter()
        .map(|c| c.name.to_ascii_lowercase())
        .collect();
    if names.len() == 2 && names[0].contains("real") && names[1].contains("imag") {
        return vec!["R".into(), "I".into()];
    }
    if names.len() == 1 {
        return vec!["Y".into()];
    }
    (1..=names.len()).map(|i| format!("Y{i}")).collect()
}

/// Shortest text that reads back to the same f64 (never in exponent form).
fn num(v: f64) -> String {
    if v == 0.0 { "0".into() } else { format!("{v}") }
}

/// Header value in AFFN with an exponent where that is shorter (header values are not ASDF).
fn hnum(v: f64) -> String {
    let plain = num(v);
    let exp = format!("{v:e}").replace('e', "E");
    if exp.len() < plain.len() && exp.parse::<f64>() == Ok(v) {
        exp
    } else {
        plain
    }
}

/// The header records shared by every table form: title, version, data type and class, origin,
/// owner, date and the NMR notes the source records.
fn write_header(
    out: &mut String,
    info: &FileInfo,
    exp: &Experiment,
    t: &TraceInfo,
    dtype: &str,
    data_class: &str,
    source_name: &str,
) {
    let acq = exp.acquisition.as_ref();
    let ins = exp.instrument.as_ref();
    let title = t
        .name
        .clone()
        .or_else(|| text(t, &["title"]).map(str::to_string))
        .unwrap_or_else(|| source_name.to_string());
    let _ = writeln!(out, "##TITLE= {}", one_line(&title));
    let _ = writeln!(
        out,
        "##JCAMP-DX= 5.01  $$ openreadout {}",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(out, "##DATA TYPE= {dtype}");
    let _ = writeln!(out, "##DATA CLASS= {data_class}");
    let from_exp = ins.and_then(|i| {
        let parts: Vec<&str> = [i.vendor.as_deref(), i.model.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    });
    let origin = text(t, &["origin", "instrument", "spectrometer"])
        .map(one_line)
        .or(from_exp.as_deref().map(one_line))
        .unwrap_or_else(|| format!("openreadout export of {}", one_line(source_name)));
    let _ = writeln!(out, "##ORIGIN= {origin}");
    let owner = text(t, &["owner", "operator"])
        .or(acq.and_then(|a| a.operator.as_deref()))
        .map_or_else(|| "(not recorded)".into(), one_line);
    let _ = writeln!(out, "##OWNER= {owner}");
    if let Some(d) = text(t, &["long_date"])
        .map(one_line)
        .or_else(|| text(t, &["acquired_at"]).and_then(long_date))
        .or_else(|| {
            acq.and_then(|a| a.started_at.as_deref())
                .and_then(long_date)
        })
    {
        let _ = writeln!(out, "##LONGDATE= {d}");
    }
    if let Some(s) = text(t, &["spectrometer"]) {
        let _ = writeln!(out, "##SPECTROMETER/DATA SYSTEM= {}", one_line(s));
    }
    if let Some(f) = number(t, &["observe_frequency_mhz", "spectrometer_frequency_mhz"]) {
        let _ = writeln!(out, "##.OBSERVE FREQUENCY= {}", num(f));
    }
    if let Some(nuc) = text(t, &["nucleus"]) {
        let nuc = one_line(nuc);
        let nuc = if nuc.starts_with('^') {
            nuc
        } else {
            format!("^{nuc}")
        };
        let _ = writeln!(out, "##.OBSERVE NUCLEUS= {nuc}");
    }
    if let Some(s) = text(t, &["solvent"]) {
        let _ = writeln!(out, "##.SOLVENT NAME= {}", one_line(s));
    }
    if let Some(p) = text(t, &["pulse_program", "pulse_sequence"]) {
        let _ = writeln!(out, "##.PULSE SEQUENCE= {}", one_line(p));
    }
    let _ = writeln!(
        out,
        "##$OPENREADOUT SOURCE= {} {}, trace {}",
        info.format.id,
        one_line(source_name),
        t.index
    );
}

/// Export one trace of `ds` to `output` as JCAMP-DX.
pub fn export_jcamp(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &JcampExportOptions,
) -> Result<JcampExportReport> {
    let info = ds.info()?;
    let exp = openreadout_core::experiment::of_dataset(&*ds, &info);
    if info.traces.is_empty() {
        return Err(Error::unsupported(
            "jcamp-dx",
            format!("JCAMP-DX export of a {} file", info.format.name),
            if info.tables.is_empty() {
                "JCAMP-DX export writes spectra and FIDs (traces); this file holds none."
            } else {
                "JCAMP-DX export writes spectra and FIDs (traces); export tables with `--to csv` or `--to parquet`."
            },
        ));
    }
    if info.format.family == "electrophysiology" {
        return Err(Error::unsupported(
            "jcamp-dx",
            format!("JCAMP-DX export of a {} file", info.format.name),
            "JCAMP-DX is a spectroscopy format; export electrophysiology traces with `--to nwb`, `--to parquet` or `--to csv`.",
        ));
    }
    let ti = opts.trace.unwrap_or(0);
    let t = info
        .traces
        .iter()
        .find(|t| t.index == ti)
        .ok_or_else(|| {
            Error::Usage(format!(
                "--trace {ti} out of range (file has {} traces)",
                info.traces.len()
            ))
        })?
        .clone();
    if t.channels.is_empty() {
        return Err(Error::unsupported(
            "jcamp-dx",
            "a trace without channels",
            "`info` lists the file's traces and their channels.",
        ));
    }
    let sweeps: Vec<u32> = match opts.sweep {
        Some(s) if s >= t.sweep_count => {
            return Err(Error::Usage(format!(
                "--sweep {s} out of range (trace {ti} has {} sweeps)",
                t.sweep_count
            )));
        }
        Some(s) => vec![s],
        None => (0..t.sweep_count).collect(),
    };
    let (first, last) = opts.rows.unwrap_or((0, None));
    let lens: Vec<u64> = sweeps
        .iter()
        .map(|&s| openreadout_core::trace::sweep_samples(&t, s))
        .collect();
    if lens.windows(2).any(|w| w[0] != w[1]) {
        return Err(Error::unsupported(
            "jcamp-dx",
            "sweeps of different lengths in one JCAMP-DX file",
            "Export one sweep at a time with --sweep N.",
        ));
    }
    let total = lens.first().copied().unwrap_or(0);
    if first >= total {
        return Err(Error::Usage(format!(
            "--rows starts at {first} but the sweeps have {total} samples"
        )));
    }
    let end = last.map_or(total, |l| l.saturating_add(1).min(total));
    let n = end - first;
    let nch = t.channels.len();
    let values = n
        .saturating_mul(nch as u64)
        .saturating_mul(sweeps.len() as u64);
    if values > MAX_VALUES {
        return Err(Error::Usage(format!(
            "{values} values are more than one JCAMP-DX export holds ({MAX_VALUES}); narrow it with --sweep and --rows"
        )));
    }
    if output.exists() && !opts.overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    // data[channel] = every selected sweep of that channel, back to back
    let mut data: Vec<Vec<f64>> = vec![Vec::with_capacity((n as usize) * sweeps.len()); nch];
    for &s in &sweeps {
        let mut pos = first;
        while pos < end {
            let k = CHUNK.min(end - pos);
            let tr = ds.read_trace(ti, s, pos, k)?;
            if tr.channels.len() != nch || tr.channels.iter().any(|c| c.len() as u64 != k) {
                return Err(Error::Other(format!(
                    "reader returned {} channels for a request of {k} samples x {nch} channels",
                    tr.channels.len()
                )));
            }
            for (d, c) in data.iter_mut().zip(tr.channels) {
                d.extend(c);
            }
            pos += k;
        }
    }
    if let Some((c, v)) = data
        .iter()
        .enumerate()
        .find_map(|(c, d)| d.iter().find(|v| !v.is_finite()).map(|v| (c, *v)))
    {
        return Err(Error::unsupported(
            "jcamp-dx",
            format!("JCAMP-DX export of non-finite values ({v} in channel {c})"),
            "JCAMP-DX (X++(Y..Y)) tables hold finite numbers; export with `--to csv` or `--to parquet` instead.",
        ));
    }
    let source_name = input.file_name().map_or_else(
        || input.display().to_string(),
        |n| n.to_string_lossy().to_string(),
    );
    let dtype = data_type(&info, &t);
    let irregular = t
        .extra
        .get("axis")
        .and_then(|a| a.get("irregular"))
        .and_then(Value::as_bool)
        == Some(true);
    if irregular {
        return export_groups(
            &info,
            &exp,
            &t,
            (input, output),
            &source_name,
            &dtype,
            &data,
            &sweeps,
            first,
        );
    }
    let mut factors = Vec::with_capacity(nch);
    let mut ints: Vec<Vec<i64>> = Vec::with_capacity(nch);
    let mut exact = true;
    let mut max_err = 0f64;
    for (c, d) in data.iter().enumerate() {
        let (f, q, ex, err) = choose(d, t.channels[c].scale);
        factors.push(f);
        ints.push(q);
        exact &= ex;
        max_err = max_err.max(err);
    }
    drop(data);

    // ------------------------------------------------------------------ text
    let (x0, step, xunit) = abscissa(&t);
    let x_first = x0 + step * first as f64;
    let x_last = x0 + step * (end - 1) as f64;
    let x_factor = if step != 0.0 && step.is_finite() {
        step.abs()
    } else {
        1.0
    };
    let x_int = move |i: usize| format!("{:.0}", (x_first + step * i as f64) / x_factor);
    let syms = symbols(&t);
    let ntuples = nch > 1 || sweeps.len() > 1;
    let mut out = String::new();
    write_header(
        &mut out,
        &info,
        &exp,
        &t,
        &dtype,
        if ntuples { "NTUPLES" } else { "XYDATA" },
        &source_name,
    );
    let write_table = |out: &mut String, y: &[i64]| match opts.encoding {
        JcampEncoding::Difdup => difdup_lines(&x_int, y, out),
        JcampEncoding::Affn => affn_lines(&x_int, y, out),
    };
    let n_us = n as usize;
    let mut pages = 0u32;
    if ntuples {
        let names: Vec<String> = t
            .channels
            .iter()
            .map(|c| one_line(&c.name).replace(',', ";"))
            .collect();
        let units: Vec<String> = t
            .channels
            .iter()
            .map(|c| jcamp_unit(c.unit.as_deref()))
            .collect();
        let x_name = match xunit.as_str() {
            "SECONDS" | "MS" | "MINUTES" => "TIME",
            "PPM" => "CHEMICAL SHIFT",
            "HZ" | "KHZ" => "FREQUENCY",
            _ => "X",
        };
        let pcount = (sweeps.len() * nch) as u64;
        let join = |v: Vec<String>| v.join(", ");
        let _ = writeln!(out, "##NTUPLES= {dtype}");
        let _ = writeln!(
            out,
            "##VAR_NAME= {}",
            join(
                std::iter::once(x_name.to_string())
                    .chain(names.iter().cloned())
                    .chain(std::iter::once("PAGE NUMBER".into()))
                    .collect()
            )
        );
        let _ = writeln!(
            out,
            "##SYMBOL= {}",
            join(
                std::iter::once("X".to_string())
                    .chain(syms.iter().cloned())
                    .chain(std::iter::once("N".into()))
                    .collect()
            )
        );
        let _ = writeln!(
            out,
            "##VAR_TYPE= {}",
            join(
                std::iter::once("INDEPENDENT".to_string())
                    .chain(std::iter::repeat_n("DEPENDENT".to_string(), nch))
                    .chain(std::iter::once("PAGE".into()))
                    .collect()
            )
        );
        let form = match opts.encoding {
            JcampEncoding::Difdup => "ASDF",
            JcampEncoding::Affn => "AFFN",
        };
        let _ = writeln!(
            out,
            "##VAR_FORM= {}",
            join(
                std::iter::once("AFFN".to_string())
                    .chain(std::iter::repeat_n(form.to_string(), nch))
                    .chain(std::iter::once("AFFN".into()))
                    .collect()
            )
        );
        let _ = writeln!(
            out,
            "##VAR_DIM= {}",
            join(
                std::iter::repeat_n(n.to_string(), nch + 1)
                    .chain(std::iter::once(pcount.to_string()))
                    .collect()
            )
        );
        let _ = writeln!(
            out,
            "##UNITS= {}",
            join(
                std::iter::once(xunit.clone())
                    .chain(units)
                    .chain(std::iter::once(String::new()))
                    .collect()
            )
        );
        let first_y: Vec<String> = ints
            .iter()
            .zip(&factors)
            .map(|(q, f)| hnum(q.first().map_or(0.0, |v| *v as f64 * f)))
            .collect();
        let last_y: Vec<String> = ints
            .iter()
            .zip(&factors)
            .map(|(q, f)| hnum(q.last().map_or(0.0, |v| *v as f64 * f)))
            .collect();
        let _ = writeln!(
            out,
            "##FIRST= {}",
            join(
                std::iter::once(hnum(x_first))
                    .chain(first_y)
                    .chain(std::iter::once("1".into()))
                    .collect()
            )
        );
        let _ = writeln!(
            out,
            "##LAST= {}",
            join(
                std::iter::once(hnum(x_last))
                    .chain(last_y)
                    .chain(std::iter::once(pcount.to_string()))
                    .collect()
            )
        );
        let _ = writeln!(
            out,
            "##FACTOR= {}",
            join(
                std::iter::once(hnum(x_factor))
                    .chain(factors.iter().map(|f| hnum(*f)))
                    .chain(std::iter::once("1".into()))
                    .collect()
            )
        );
        let one_sweep = sweeps.len() == 1;
        for si in 0..sweeps.len() {
            for (c, sym) in syms.iter().enumerate() {
                let page = if one_sweep { c + 1 } else { si + 1 };
                let _ = writeln!(out, "##PAGE= N={page}");
                let _ = writeln!(out, "##NPOINTS= {n}");
                let _ = writeln!(out, "##DATA TABLE= (X++({sym}..{sym})), XYDATA");
                write_table(&mut out, &ints[c][si * n_us..(si + 1) * n_us]);
                pages += 1;
            }
        }
        let _ = writeln!(out, "##END NTUPLES= {dtype}");
    } else {
        let _ = writeln!(out, "##XUNITS= {xunit}");
        let _ = writeln!(out, "##YUNITS= {}", y_units(&t, 0));
        let _ = writeln!(out, "##XFACTOR= {}", hnum(x_factor));
        let _ = writeln!(out, "##YFACTOR= {}", hnum(factors[0]));
        let _ = writeln!(out, "##FIRSTX= {}", hnum(x_first));
        let _ = writeln!(out, "##LASTX= {}", hnum(x_last));
        if n > 1 {
            let _ = writeln!(out, "##DELTAX= {}", hnum(step));
        }
        let _ = writeln!(out, "##NPOINTS= {n}");
        let _ = writeln!(
            out,
            "##FIRSTY= {}",
            hnum(ints[0].first().map_or(0.0, |v| *v as f64 * factors[0]))
        );
        let _ = writeln!(out, "##XYDATA= (X++(Y..Y))");
        write_table(&mut out, &ints[0]);
        pages = 1;
    }
    out.push_str("##END=\n");

    let bytes = write_verified(output, &out, &|tmp| {
        verify(
            tmp,
            &Expect {
                ints: &ints,
                factors: &factors,
                sweeps: sweeps.len(),
                n,
                x_first,
                step,
            },
        )
    })?;
    Ok(JcampExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: crate::JCAMP_FORMAT_ID.into(),
        jcamp_version: "5.01".into(),
        data_type: dtype,
        data_class: if ntuples { "NTUPLES" } else { "XYDATA" }.into(),
        trace: ti,
        sweeps,
        pages,
        first_sample: first,
        samples_written: n,
        channels_written: nch as u32,
        encoding: opts.encoding.id().into(),
        factors,
        exact,
        max_abs_error: max_err,
        bytes_written: bytes,
        verified: true,
    })
}

/// Write `text` to a temporary file beside `output`, run `check` on it, then rename it into
/// place. Returns the size written.
fn write_verified(output: &Path, text: &str, check: &dyn Fn(&Path) -> Result<()>) -> Result<u64> {
    let name = output
        .file_name()
        .map_or_else(|| "export.jdx".into(), |n| n.to_string_lossy().to_string());
    let tmp = output.with_file_name(format!(".{name}.partial-{}", std::process::id()));
    let result = (|| -> Result<u64> {
        let mut f = std::fs::File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        f.write_all(text.as_bytes())
            .map_err(|e| Error::io(&tmp, e))?;
        f.sync_all().map_err(|e| Error::io(&tmp, e))?;
        drop(f);
        check(&tmp)?;
        Ok(std::fs::metadata(&tmp)
            .map_err(|e| Error::io(&tmp, e))?
            .len())
    })();
    match result {
        Ok(b) => {
            std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
            Ok(b)
        }
        Err(e) => {
            if std::env::var_os("OPENREADOUT_KEEP_PARTIAL").is_none() {
                std::fs::remove_file(&tmp).ok();
            }
            Err(e)
        }
    }
}

/// An irregularly sampled trace (a JCAMP-DX `(XY..XY)` peak table or point list, a mass
/// spectrum): channel 0 is the abscissa, channel 1 the ordinate, written as AFFN pairs in their
/// shortest exact decimal form (factors 1), one sweep per file.
fn export_groups(
    info: &FileInfo,
    exp: &Experiment,
    t: &TraceInfo,
    (input, output): (&Path, &Path),
    source_name: &str,
    dtype: &str,
    data: &[Vec<f64>],
    sweeps: &[u32],
    first: u64,
) -> Result<JcampExportReport> {
    if t.channels.len() != 2 || sweeps.len() != 1 {
        return Err(Error::unsupported(
            "jcamp-dx",
            format!(
                "JCAMP-DX export of an irregularly sampled table with {} columns and {} sweeps",
                t.channels.len(),
                sweeps.len()
            ),
            "(XY..XY) tables are written for two columns (x, y) and one sweep: pass --sweep N, or export with `--to csv` or `--to parquet`.",
        ));
    }
    let (x, y) = (&data[0], &data[1]);
    let n = x.len() as u64;
    let peak = t.extra.get("kind").and_then(Value::as_str) == Some("peak_table");
    let label = if peak { "PEAK TABLE" } else { "XYPOINTS" };
    let mut out = String::new();
    write_header(&mut out, info, exp, t, dtype, label, source_name);
    let _ = writeln!(
        out,
        "##XUNITS= {}",
        jcamp_unit(t.channels[0].unit.as_deref())
    );
    let _ = writeln!(out, "##YUNITS= {}", y_units(t, 1));
    let _ = writeln!(out, "##XFACTOR= 1");
    let _ = writeln!(out, "##YFACTOR= 1");
    if let (Some(a), Some(b)) = (x.first(), x.last()) {
        let _ = writeln!(out, "##FIRSTX= {}", hnum(*a));
        let _ = writeln!(out, "##LASTX= {}", hnum(*b));
    }
    let _ = writeln!(out, "##NPOINTS= {n}");
    let _ = writeln!(out, "##{label}= (XY..XY)");
    let mut line = String::new();
    for (a, b) in x.iter().zip(y) {
        let pair = format!("{},{}", num(*a), num(*b));
        if !line.is_empty() && line.len() + 1 + pair.len() > LINE {
            out.push_str(&line);
            out.push('\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&pair);
    }
    if !line.is_empty() {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("##END=\n");
    let bytes = write_verified(output, &out, &|tmp| {
        let bad = |m: String| Error::Other(format!("JCAMP-DX read-back: {m}"));
        let mut ds = JcampDataset::open(tmp)?;
        let back = ds.info()?;
        let bt = back
            .traces
            .first()
            .ok_or_else(|| bad("no table in the written file".into()))?;
        if back.traces.len() != 1 || bt.channels.len() != 2 || bt.sample_count != n {
            return Err(bad(format!(
                "{} table(s) of {} columns x {} points; wrote 1 x 2 x {n}",
                back.traces.len(),
                bt.channels.len(),
                bt.sample_count
            )));
        }
        let tr = ds.read_trace(0, 0, 0, n)?;
        for (c, want) in [x, y].iter().enumerate() {
            if tr.channels[c]
                .iter()
                .map(|v| v.to_bits())
                .ne(want.iter().map(|v| v.to_bits()))
            {
                return Err(bad(format!("column {c} differs from what was written")));
            }
        }
        Ok(())
    })?;
    Ok(JcampExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: crate::JCAMP_FORMAT_ID.into(),
        jcamp_version: "5.01".into(),
        data_type: dtype.to_string(),
        data_class: label.into(),
        trace: t.index,
        sweeps: sweeps.to_vec(),
        pages: 1,
        first_sample: first,
        samples_written: n,
        channels_written: 2,
        encoding: "affn".into(),
        factors: vec![1.0, 1.0],
        exact: true,
        max_abs_error: 0.0,
        bytes_written: bytes,
        verified: true,
    })
}

/// What the written file must read back as.
struct Expect<'a> {
    ints: &'a [Vec<i64>],
    factors: &'a [f64],
    sweeps: usize,
    n: u64,
    x_first: f64,
    step: f64,
}

/// Re-read `path` with the JCAMP-DX reader: one trace of the expected shape, every ordinate equal
/// to `integer × factor` bit for bit, the abscissa within its rounding, and no check failures.
fn verify(path: &Path, e: &Expect<'_>) -> Result<()> {
    let bad = |m: String| Error::Other(format!("JCAMP-DX read-back: {m}"));
    let mut ds = JcampDataset::open(path)?;
    let info = ds.info()?;
    let t = info
        .traces
        .first()
        .ok_or_else(|| bad("no trace in the written file".into()))?
        .clone();
    if info.traces.len() != 1
        || t.channels.len() != e.ints.len()
        || t.sweep_count as usize != e.sweeps
        || t.sample_count != e.n
    {
        return Err(bad(format!(
            "{} trace(s), {} channels x {} sweeps x {} samples; wrote {} x {} x {}",
            info.traces.len(),
            t.channels.len(),
            t.sweep_count,
            t.sample_count,
            e.ints.len(),
            e.sweeps,
            e.n
        )));
    }
    if let Some(a) = t.extra.get("axis") {
        let first = a.get("first").and_then(Value::as_f64).unwrap_or(f64::NAN);
        let step = a.get("step").and_then(Value::as_f64).unwrap_or(f64::NAN);
        let scale = e.x_first.abs().max((e.step * e.n as f64).abs());
        let tol = 1e-9 * scale.max(f64::MIN_POSITIVE);
        if (first - e.x_first).abs() > tol || (e.n > 1 && (step - e.step).abs() * e.n as f64 > tol)
        {
            return Err(bad(format!(
                "abscissa first {first}, step {step}; wrote {}, {}",
                e.x_first, e.step
            )));
        }
    }
    let n_us = e.n as usize;
    for s in 0..e.sweeps {
        let tr = ds.read_trace(0, s as u32, 0, e.n)?;
        for (c, got) in tr.channels.iter().enumerate() {
            let want = &e.ints[c][s * n_us..(s + 1) * n_us];
            let f = e.factors[c];
            if got.len() != want.len() {
                return Err(bad(format!(
                    "sweep {s} channel {c}: {} values, wrote {}",
                    got.len(),
                    want.len()
                )));
            }
            if let Some(i) = got
                .iter()
                .zip(want)
                .position(|(g, w)| g.to_bits() != (*w as f64 * f).to_bits())
            {
                return Err(bad(format!(
                    "sweep {s} channel {c} value {i}: read {}, wrote {} x {f}",
                    got[i], want[i]
                )));
            }
        }
    }
    let check = ds.check()?;
    if let Some(f) = check
        .findings
        .iter()
        .find(|f| f.severity == openreadout_core::Severity::Error)
    {
        return Err(bad(format!("check: {} ({})", f.message, f.code)));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::jcamp_asdf::decode_asdf;

    #[test]
    fn asdf_characters() {
        assert_eq!(sqz(0), "@");
        assert_eq!(sqz(123), "A23");
        assert_eq!(sqz(-45), "d5");
        assert_eq!(dif(0), "%");
        assert_eq!(dif(7), "P");
        assert_eq!(dif(-19), "j9");
        assert_eq!(dup(2), "T");
        assert_eq!(dup(9), "s");
        assert_eq!(dup(10), "S0");
        assert_eq!(dup(95), "s5");
    }

    #[test]
    fn difdup_round_trip_with_checks() {
        let y: Vec<i64> = (0..500)
            .map(|i: i64| match i % 50 {
                0..10 => 7,
                10..20 => i * 3 - 1000,
                _ => (i * i) % 977 - 400,
            })
            .collect();
        let mut s = String::new();
        difdup_lines(&|i| format!("{i}"), &y, &mut s);
        assert!(s.lines().all(|l| l.len() <= LINE + 16));
        let t = decode_asdf(&s).unwrap();
        assert!(t.y_check_failures.is_empty());
        let back: Vec<i64> = t.y.iter().map(|v| *v as i64).collect();
        assert_eq!(back, y);
        // one point, two points
        for y in [vec![5i64], vec![5, -5]] {
            let mut s = String::new();
            difdup_lines(&|i| format!("{i}"), &y, &mut s);
            let back: Vec<i64> = decode_asdf(&s)
                .unwrap()
                .y
                .iter()
                .map(|v| *v as i64)
                .collect();
            assert_eq!(back, y);
        }
        let mut s = String::new();
        affn_lines(&|i| format!("{i}"), &y, &mut s);
        let back: Vec<i64> = decode_asdf(&s)
            .unwrap()
            .y
            .iter()
            .map(|v| *v as i64)
            .collect();
        assert_eq!(back, y);
    }

    #[test]
    fn factors_are_exact_when_possible() {
        // Bruker-style: integers × 2^NC
        let v: Vec<f64> = [3.0, -5.0, 1024.0].iter().map(|x| x * 0.25).collect();
        let (f, q, exact, _) = choose(&v, 1.0);
        assert!(exact);
        assert_eq!(f, 0.25);
        assert_eq!(q, [3, -5, 1024]);
        // a JCAMP-DX factor that is not a power of two
        let v: Vec<f64> = [12.0, 7.0, -3.0].iter().map(|x| x * 0.001).collect();
        let (f, _, exact, _) = choose(&v, 0.001);
        assert!(exact);
        assert_eq!(f, 0.001);
        // binary fractions: exact with a power of two
        let v = [0.5, 0.75, -1.125];
        let (f, q, exact, _) = choose(&v, 1.0);
        assert!(exact);
        assert_eq!((f, q), (0.125, vec![4, 6, -9]));
        // decimal fractions need more than 53 bits together: quantized to 32 bits
        let v = [0.1, 0.2, 0.3];
        let (f, q, exact, err) = choose(&v, 1.0);
        assert!(!exact);
        assert!(err <= f / 2.0 && err > 0.0);
        assert!(q.iter().all(|x| x.unsigned_abs() <= 2_147_483_647));
        // dynamic range beyond 53 bits: quantized, error at most half the factor
        let v = [1e20, 1e-20];
        let (f, _, exact, err) = choose(&v, 1.0);
        assert!(!exact);
        assert!(err <= f / 2.0);
        assert_eq!(choose(&[0.0, 0.0], 1.0).1, [0, 0]);
    }

    #[test]
    fn long_dates_and_units() {
        assert_eq!(
            long_date("2021-03-04T05:06:07Z").as_deref(),
            Some("2021/03/04 05:06:07")
        );
        assert_eq!(long_date("2021"), None);
        assert_eq!(jcamp_unit(Some("ppm")), "PPM");
        assert_eq!(jcamp_unit(None), "ARBITRARY UNITS");
        assert_eq!(hnum(1e-9), "1E-9");
        assert_eq!(hnum(0.5), "0.5");
        assert_eq!(pow2(-1074), f64::from_bits(1));
        assert_eq!(pow2(3), 8.0);
        assert_eq!(low_bit_exponent(12.0), 2);
    }
}
