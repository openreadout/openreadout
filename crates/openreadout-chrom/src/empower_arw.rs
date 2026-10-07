//! Waters Empower ASCII raw-data exports (`.arw`): what Empower writes when a result's raw data
//! is exported as ASCII — a row of quoted field names the export method chose (`"SampleName"`,
//! `"Channel"`, ...), a row of their quoted values, then one `time<TAB>value` row per point
//! (minutes). Empower keeps its native data in its database; this export (and AIA/netCDF, read
//! by `andi-chrom`) is how it leaves Empower. Layout and evidence: `docs/formats/empower-arw.md`,
//! `docs/provenance/empower-arw.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::model::{
    CheckReport, FileInfo, Finding, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::binary::tidy;
use crate::{EMPOWER_ARW_ID, EmpowerArwReader};

/// Largest export read (text, read whole).
pub const MAX_ARW_BYTES: u64 = 512 << 20;

/// A parsed export.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ArwExport {
    /// The header's field names and values, in order.
    pub fields: Vec<(String, String)>,
    /// Point times (minutes).
    pub times: Vec<f64>,
    /// Point values (the detector's unit; the export does not state it).
    pub values: Vec<f64>,
    /// Line ending seen (`CR`, `LF`, `CRLF`).
    pub line_ending: &'static str,
    /// The export has no header rows (its method selected no fields): data rows only.
    pub headerless: bool,
    /// The header holds one `"name"<TAB>value` field per line instead of a names row and a
    /// values row.
    pub one_field_per_line: bool,
}

impl ArwExport {
    /// A header field's value.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    /// The regular step (minutes) when every point is on one grid (within 10⁻⁶ of a step).
    pub fn regular_step(&self) -> Option<f64> {
        let n = self.times.len();
        if n < 2 {
            return None;
        }
        let step = (self.times[n - 1] - self.times[0]) / (n - 1) as f64;
        (step > 0.0
            && self.times.iter().enumerate().all(|(i, &t)| {
                // times are printed to 7 significant digits
                (t - (self.times[0] + i as f64 * step)).abs() <= 1e-6 * t.abs().max(step)
            }))
        .then_some(step)
    }
}

/// Does `head` look like an Empower ASCII export: a first line of tab-separated quoted names?
pub fn looks_like_arw(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let Some(first) = head.split(|&b| b == b'\r' || b == b'\n').next() else {
        return false;
    };
    first.first() == Some(&b'"')
        && first.last() == Some(&b'"')
        && first
            .iter()
            .all(|&b| b == b'\t' || (b >= 0x20 && b != 0x7f) || b >= 0x80)
        && std::str::from_utf8(first).is_ok_and(|s| {
            s.split('\t')
                .all(|c| c.len() >= 2 && c.starts_with('"') && c.ends_with('"'))
        })
}

/// Does `head` start like an export with one field per line whose first value is a number
/// (`"Sample Set Id"<TAB>2597`)? Such text is only claimed for a file with the `.arw` extension.
pub fn looks_like_arw_field_lines(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let Some(first) = head.split(|&b| b == b'\r' || b == b'\n').next() else {
        return false;
    };
    std::str::from_utf8(first).is_ok_and(|s| {
        field_line(s).is_some_and(|(_, v)| v.parse::<f64>().is_ok_and(f64::is_finite))
    })
}

/// A header line of the one-field-per-line layout: a quoted name and a value (quoted or not).
fn field_line(line: &str) -> Option<(String, String)> {
    let mut cells = line.split('\t');
    let name = unquote(cells.next()?).filter(|n| !n.is_empty())?;
    let value = cells.next()?;
    if cells.next().is_some() {
        return None;
    }
    let value = unquote(value).unwrap_or_else(|| value.trim().to_string());
    Some((name, value))
}

/// Does `head` look like an export without its header rows: every complete line two
/// tab-separated numbers? Such text is only claimed for a file with the `.arw` extension.
pub fn looks_like_headerless_arw(head: &[u8]) -> bool {
    let head = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let mut lines: Vec<&str> = text.split(['\r', '\n']).collect();
    // the last line may be cut by the end of the head
    if lines.len() > 1 {
        lines.pop();
    }
    let lines: Vec<&str> = lines.into_iter().filter(|l| !l.trim().is_empty()).collect();
    !lines.is_empty() && lines.iter().all(|l| two_numbers(l).is_some())
}

/// A data row: a time and a value separated by a tab.
fn two_numbers(line: &str) -> Option<(f64, f64)> {
    let mut cells = line.split('\t').map(str::trim);
    let num = |s: &str| s.parse::<f64>().ok().filter(|x| x.is_finite());
    let (t, v) = (num(cells.next()?)?, num(cells.next()?)?);
    cells.next().is_none().then_some((t, v))
}

fn unquote(s: &str) -> Option<String> {
    let s = s.trim_end_matches(' ');
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .map(|x| x.replace("\"\"", "\""))
}

/// Why an export was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArwError {
    /// Not an export of the layout described (exit 4).
    Corrupt(String),
    /// A layout seen but not read (a 3D PDA export: more than two columns; exit 6).
    Unsupported(String),
}

/// Parse an export.
///
/// # Errors
/// [`ArwError::Corrupt`] for a header or data row that does not parse; [`ArwError::Unsupported`]
/// for data rows with more than two columns.
pub fn parse_arw(text: &str) -> std::result::Result<ArwExport, ArwError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let line_ending = if text.contains("\r\n") {
        "CRLF"
    } else if text.contains('\r') {
        "CR"
    } else {
        "LF"
    };
    let mut lines = text
        .split(['\r', '\n'])
        .filter(|l| !l.trim().is_empty())
        .peekable();
    let corrupt = |m: String| ArwError::Corrupt(m);
    if lines.peek().is_some_and(|l| two_numbers(l).is_some()) {
        // no header rows: every line is a data row
        let mut out = ArwExport {
            line_ending,
            headerless: true,
            ..ArwExport::default()
        };
        for (i, line) in lines.enumerate() {
            let (t, v) = two_numbers(line).ok_or_else(|| {
                corrupt(format!("data row {} ({line:?}) is not two numbers", i + 1))
            })?;
            out.times.push(t);
            out.values.push(v);
        }
        return Ok(out);
    }
    // The header is every line before the first row of two numbers. More than two header
    // lines, or a first line whose value is not quoted, of two cells each with a quoted name:
    // one field per line. Otherwise a names row and a values row.
    let header: Vec<&str> = lines
        .clone()
        .take_while(|l| two_numbers(l).is_none())
        .collect();
    let first_value_unquoted = header
        .first()
        .and_then(|l| l.split('\t').nth(1))
        .is_some_and(|v| unquote(v).is_none());
    if (header.len() > 2 || first_value_unquoted)
        && header.iter().all(|l| field_line(l).is_some())
    {
        let mut out = ArwExport {
            fields: header.iter().filter_map(|l| field_line(l)).collect(),
            line_ending,
            one_field_per_line: true,
            ..ArwExport::default()
        };
        for (i, line) in lines.skip(header.len()).enumerate() {
            let (t, v) = two_numbers(line).ok_or_else(|| {
                corrupt(format!("data row {} ({line:?}) is not two numbers", i + 1))
            })?;
            out.times.push(t);
            out.values.push(v);
        }
        if out.times.is_empty() {
            return Err(corrupt("no data rows".into()));
        }
        return Ok(out);
    }
    let names = lines.next().ok_or_else(|| corrupt("empty file".into()))?;
    let values = lines
        .next()
        .ok_or_else(|| corrupt("no header values row".into()))?;
    let names: Vec<String> = names
        .split('\t')
        .map(|c| unquote(c).ok_or_else(|| corrupt(format!("header name {c:?} is not quoted"))))
        .collect::<std::result::Result<_, _>>()?;
    let vals: Vec<String> = values
        .split('\t')
        .map(|c| unquote(c).unwrap_or_else(|| c.trim().to_string()))
        .collect();
    if vals.len() != names.len() {
        return Err(corrupt(format!(
            "{} header names but {} values",
            names.len(),
            vals.len()
        )));
    }
    let mut out = ArwExport {
        fields: names.into_iter().zip(vals).collect(),
        line_ending,
        ..ArwExport::default()
    };
    for (i, line) in lines.enumerate() {
        let cells: Vec<&str> = line.split('\t').map(str::trim).collect();
        if cells.len() > 2 {
            return Err(ArwError::Unsupported(format!(
                "data row {} has {} columns: a multi-column (3D PDA) export, which is not read",
                i + 1,
                cells.len()
            )));
        }
        let num = |s: &str| -> Option<f64> { s.parse::<f64>().ok().filter(|x| x.is_finite()) };
        let (Some(t), Some(v)) = (
            cells.first().and_then(|c| num(c)),
            cells.get(1).and_then(|c| num(c)),
        ) else {
            return Err(corrupt(format!(
                "data row {} ({line:?}) is not two numbers",
                i + 1
            )));
        };
        out.times.push(t);
        out.values.push(v);
    }
    if out.times.is_empty() {
        return Err(corrupt("no data rows".into()));
    }
    Ok(out)
}

/// An opened export.
#[derive(Debug)]
pub struct EmpowerArwDataset {
    path: PathBuf,
    size: u64,
    arw: ArwExport,
}

impl EmpowerArwDataset {
    /// Open an export by path.
    ///
    /// # Errors
    /// Unreadable, too large, or not an export of the layout described.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs): (&Path, &Fs) = (input.path(), input.fs());
        let size = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
        if size > MAX_ARW_BYTES {
            return Err(Error::unsupported(
                EMPOWER_ARW_ID,
                format!("a {size}-byte export"),
                "Exports larger than 512 MiB are not read.",
            ));
        }
        let bytes = fs.read(path).map_err(|e| Error::io(path, e))?;
        let text = String::from_utf8_lossy(&bytes);
        let arw = parse_arw(&text).map_err(|e| match e {
            ArwError::Corrupt(m) => Error::corrupt(EMPOWER_ARW_ID, m),
            ArwError::Unsupported(m) => Error::unsupported(
                EMPOWER_ARW_ID,
                m,
                "Export the PDA data as AIA/netCDF from Empower, or export single-wavelength channels.",
            ),
        })?;
        Ok(EmpowerArwDataset {
            path: path.to_path_buf(),
            size,
            arw,
        })
    }

    /// The parsed export (for library users).
    pub fn export(&self) -> &ArwExport {
        &self.arw
    }

    fn trace_info(&self) -> TraceInfo {
        let a = &self.arw;
        let mut extra: BTreeMap<String, Value> = BTreeMap::new();
        let fields: serde_json::Map<String, Value> = a
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        extra.insert("fields".into(), Value::Object(fields));
        for (key, name) in [
            ("sample_name", "SampleName"),
            ("channel", "Channel"),
            ("sample_set", "Sample Set Name"),
            ("instrument_method", "Instrument Method Name"),
            ("processing_method", "Processing Method"),
            ("vial", "Vial"),
            ("injection", "Injection"),
            ("injection_volume", "Injection Volume"),
            ("acquired_by", "Acquired By"),
            ("acquired_at", "Date Acquired"),
        ] {
            if let Some(v) = a.field(name) {
                extra.insert(key.into(), json!(v));
            }
        }
        extra.insert("line_ending".into(), json!(a.line_ending));
        if a.headerless {
            extra.insert("headerless".into(), json!(true));
        }
        if a.one_field_per_line {
            extra.insert("one_field_per_line".into(), json!(true));
        }
        let n = a.times.len() as u64;
        let first = a.times[0];
        let last = a.times[a.times.len() - 1];
        extra.insert("x_start_min".into(), json!(tidy(first)));
        extra.insert("x_end_min".into(), json!(tidy(last)));
        let channel_name = a.field("Channel").unwrap_or("value").to_string();
        let value = SignalChannelInfo {
            index: 0,
            name: channel_name.clone(),
            unit: None,
            dtype: "float64".into(),
            scale: 1.0,
            offset: 0.0,
            extra: BTreeMap::new(),
        };
        let (rate, channels) = if let Some(step) = a.regular_step() {
            extra.insert(
                "axis".into(),
                json!({"quantity": "retention_time", "unit": "min", "first": tidy(first), "step": tidy(step)}),
            );
            (tidy(1.0 / (step * 60.0)), vec![value])
        } else {
            extra.insert(
                "time_channel".into(),
                json!("the points are not evenly spaced: channel 0 holds each point's retention time (min)"),
            );
            (
                0.0,
                vec![
                    SignalChannelInfo {
                        index: 0,
                        name: "time".into(),
                        unit: Some("min".into()),
                        dtype: "float64".into(),
                        scale: 1.0,
                        offset: 0.0,
                        extra: BTreeMap::new(),
                    },
                    SignalChannelInfo { index: 1, ..value },
                ],
            )
        };
        let name = match a.field("SampleName") {
            Some(s) => format!("{s} / {channel_name}"),
            None => channel_name,
        };
        TraceInfo {
            index: 0,
            name: Some(name),
            sample_rate_hz: rate,
            sample_count: n,
            sweep_count: 1,
            channels,
            start_s: Some(tidy(first * 60.0)),
            extra,
        }
    }
}

impl Dataset for EmpowerArwDataset {
    fn info(&self) -> Result<FileInfo> {
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: self.size,
            format: EmpowerArwReader.descriptor(),
            format_version: None,
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: vec![self.trace_info()],
            plane_count: 0,
            notes: vec![
                "a Waters Empower ASCII export: one trace, times in minutes as exported; the export does not state the value's unit (the detector's: mV, AU, EU, …)".into(),
            ],
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({
            "fields": self.arw.fields.iter().map(|(k, v)| json!({"name": k, "value": v})).collect::<Vec<_>>(),
            "points": self.arw.times.len(),
            "line_ending": self.arw.line_ending,
            "headerless": self.arw.headerless,
            "one_field_per_line": self.arw.one_field_per_line,
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "traces[].extra.fields",
            "traces[].extra.sample_name",
            "traces[].extra.channel",
            "traces[].sample_rate_hz",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![LsEntry {
            kind: "export".into(),
            name: self
                .path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            offset: Some(0),
            size: Some(self.size),
            image: None,
            details: json!({"fields": self.arw.fields.len(), "points": self.arw.times.len()}),
        }])
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            EMPOWER_ARW_ID,
            "image planes",
            "An Empower export holds one chromatogram: use `openreadout analyze chromatogram --trace 0`.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        if index != 0 || sweep != 0 {
            return Err(Error::Usage(format!(
                "trace {index} sweep {sweep} out of range (an export has one trace, one sweep)"
            )));
        }
        let total = self.arw.times.len() as u64;
        if first_sample > total {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({total} samples)"
            )));
        }
        let n = max_samples.min(total - first_sample);
        let (a, b) = (first_sample as usize, (first_sample + n) as usize);
        let values = self.arw.values[a..b].to_vec();
        let channels = if self.arw.regular_step().is_some() {
            vec![values]
        } else {
            vec![self.arw.times[a..b].to_vec(), values]
        };
        Ok(Trace {
            trace: 0,
            sweep: 0,
            first_sample,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), EMPOWER_ARW_ID);
        r.performed(if self.arw.one_field_per_line {
            "header: one quoted field name and its value per line"
        } else {
            "header: quoted field names and as many values"
        });
        r.performed(format!(
            "{} data rows: two numbers each (time min, value)",
            self.arw.times.len()
        ));
        if self.arw.times.windows(2).any(|w| w[1] <= w[0]) {
            r.push(Finding::warning(
                "time_not_increasing",
                "the exported times do not increase",
            ));
        }
        if self.arw.regular_step().is_none() {
            r.push(Finding::info(
                "irregular_times",
                "the exported times are not evenly spaced: the trace carries a time channel",
            ));
        }
        if self.arw.field("Channel").is_none() {
            r.push(Finding::info(
                "no_channel_field",
                "the export method did not include the Channel field",
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\"SampleName\"\t\"Channel\"\t\"Sample Set Name\"\r\"14\"\t\"2475ChA ex280/em350\"\t\"Exp106\"\r0\t0\r0.008333333\t0.5\r0.01666667\t1.25\r";

    #[test]
    fn parses_a_2d_export() {
        assert!(looks_like_arw(SAMPLE.as_bytes()));
        let a = parse_arw(SAMPLE).unwrap();
        assert_eq!(a.field("Channel"), Some("2475ChA ex280/em350"));
        assert_eq!(a.times.len(), 3);
        assert_eq!(a.values[2], 1.25);
        assert_eq!(a.line_ending, "CR");
        assert!((a.regular_step().unwrap() - 0.008_333_335).abs() < 1e-8);
    }

    #[test]
    fn parses_an_export_without_header_rows() {
        // a development export (appia-empower-results1844) with its two header rows removed
        let body = "0\t0\r0.008333333\t0.5\r0.01666667\t1.25\r";
        assert!(!looks_like_arw(body.as_bytes()));
        assert!(looks_like_headerless_arw(body.as_bytes()));
        // the head may end inside a row
        assert!(looks_like_headerless_arw(b"0\t0\r0.008333333\t0."));
        let a = parse_arw(body).unwrap();
        assert!(a.headerless && a.fields.is_empty());
        assert_eq!(a.times.len(), 3);
        assert_eq!(a.values[2], 1.25);
        assert_eq!(a.line_ending, "CR");
        // three columns, text, or a header further down are not this layout
        assert!(!looks_like_headerless_arw(b"0\t1\t2\r1\t2\t3\r"));
        assert!(!looks_like_headerless_arw(b"Time\tValue\r0\t1\r"));
        assert!(!looks_like_headerless_arw(b""));
        assert!(matches!(
            parse_arw("0\t0\r1\tx\r"),
            Err(ArwError::Corrupt(_))
        ));
        let r = crate::EmpowerArwReader;
        assert!(r.sniff(body.as_bytes(), Path::new("x.arw")).is_some());
        assert!(r.sniff(body.as_bytes(), Path::new("x.txt")).is_none());
    }

    #[test]
    fn parses_an_export_with_one_field_per_line() {
        // the layout of gpcreader-empower-sample1 (first value a number) and of the HPLC-RS
        // exports (every value quoted), shortened
        let gpc = "\"Sample Set Id\"\t2597\r\n\"SampleName\"\t\"PMMA88.5kDa_THF\"\r\n\"Injection\"\t1\r\n\"Channel\"\t\"SATIN-2 \"\r\n0.01666667\t8.935\r\n0.03333333\t8.936\r\n";
        assert!(!looks_like_arw(gpc.as_bytes()));
        assert!(looks_like_arw_field_lines(gpc.as_bytes()));
        let a = parse_arw(gpc).unwrap();
        assert!(a.one_field_per_line && !a.headerless);
        assert_eq!(a.field("Sample Set Id"), Some("2597"));
        assert_eq!(a.field("Channel"), Some("SATIN-2 "));
        assert_eq!(a.times.len(), 2);
        assert_eq!(a.values[1], 8.936);
        let r = crate::EmpowerArwReader;
        assert!(r.sniff(gpc.as_bytes(), Path::new("x.arw")).is_some());
        assert!(r.sniff(gpc.as_bytes(), Path::new("x.txt")).is_none());
        let hplc = "\"SampleName\"\t\"9. PC-12\"\r\n\"System Name\"\t\"Alliance 2\"\r\n\"Date Acquired\"\t\"5/15/2025 10:18:57 PM BST\"\r\n0\t0.0002456665\r\n0.01666667\t0.0003662109\r\n";
        let a = parse_arw(hplc).unwrap();
        assert!(a.one_field_per_line);
        assert_eq!(a.field("System Name"), Some("Alliance 2"));
        assert_eq!(a.times, vec![0.0, 0.016_666_67]);
        // two quoted lines stay a names row and a values row
        let a = parse_arw(SAMPLE).unwrap();
        assert!(!a.one_field_per_line);
        // a header line of three cells is not this layout
        assert!(parse_arw("\"A\"\t\"x\"\r\"B\"\t\"y\"\t\"z\"\r\"C\"\t\"w\"\r0\t1\r").is_err());
        for cut in 0..gpc.len() {
            let _ = parse_arw(&gpc[..cut]);
        }
    }

    #[test]
    fn refuses_and_rejects() {
        assert!(!looks_like_arw(b"II*\0 a Sony raw"));
        assert!(!looks_like_arw(b"Time\tValue\n"));
        assert!(matches!(
            parse_arw("\"A\"\r\"x\"\r0\t1\t2\r"),
            Err(ArwError::Unsupported(_))
        ));
        assert!(matches!(
            parse_arw("\"A\"\r\"x\"\r0\tabc\r"),
            Err(ArwError::Corrupt(_))
        ));
        assert!(matches!(
            parse_arw("\"A\"\t\"B\"\r\"x\"\r0\t1\r"),
            Err(ArwError::Corrupt(_))
        ));
        assert!(parse_arw("").is_err());
        assert!(parse_arw("\"A\"\r\"x\"\r").is_err());
        for cut in 0..SAMPLE.len() {
            let _ = parse_arw(&SAMPLE[..cut]);
        }
    }
}
