//! BrainVision Core Data Format recordings: a text header (`.vhdr`), a text marker file
//! (`.vmrk`) and the samples (`.eeg`, or `.dat`), as BrainVision Recorder and Analyzer, pybv,
//! MNE-Python, FieldTrip and EEGLAB write them. One trace whose channels are the header's;
//! marker-based segments are sweeps; the markers form the `markers` table. Notes and
//! vocabulary: `docs/formats/brainvision.md`; provenance: `docs/provenance/brainvision.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{read_block, windows1252};
use openreadout_core::model::{
    CheckReport, ColumnInfo, DetectConfidence, FileInfo, Finding, FormatDescriptor, LsEntry,
    Severity, SignalChannelInfo, Table, TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, Detection, FormatReader, PlaneIndex, has_extension};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

/// Format id of BrainVision recordings.
pub const BRAINVISION_FORMAT_ID: &str = "brainvision";
/// Header and marker files read at most, bytes.
pub const MAX_BV_TEXT: u64 = 64 << 20;
/// Channels accepted at most.
pub const MAX_BV_CHANNELS: usize = 4096;
/// Markers kept at most.
pub const MAX_BV_MARKERS: usize = 4_000_000;
/// Samples per channel decoded per `read_trace` call at most.
pub const MAX_BV_READ: u64 = 1 << 24;
/// Bytes read per `read_trace` call at most (one frame or channel run is always read).
pub const MAX_BV_READ_BYTES: u64 = 64 << 20;
/// ASCII data files read at most, bytes (every line is indexed when the file is opened).
pub const MAX_BV_ASCII: u64 = 512 << 20;

/// How the samples are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BvSampleFormat {
    /// `INT_16`: 16-bit two's complement.
    Int16,
    /// `UINT_16`: 16-bit unsigned.
    UInt16,
    /// `INT_32`: 32-bit two's complement.
    Int32,
    /// `IEEE_FLOAT_32`: 32-bit float.
    Float32,
    /// `DataFormat=ASCII`: decimal text, one sample frame per line.
    Ascii,
}

impl BvSampleFormat {
    /// Bytes per stored sample (0 for text).
    pub fn bytes(self) -> u64 {
        match self {
            BvSampleFormat::Int16 | BvSampleFormat::UInt16 => 2,
            BvSampleFormat::Int32 | BvSampleFormat::Float32 => 4,
            BvSampleFormat::Ascii => 0,
        }
    }
    /// The header's name for it (`INT_16`, …, `ASCII`).
    pub fn name(self) -> &'static str {
        match self {
            BvSampleFormat::Int16 => "INT_16",
            BvSampleFormat::UInt16 => "UINT_16",
            BvSampleFormat::Int32 => "INT_32",
            BvSampleFormat::Float32 => "IEEE_FLOAT_32",
            BvSampleFormat::Ascii => "ASCII",
        }
    }
    /// NumPy-style dtype of the stored samples.
    pub fn dtype(self) -> &'static str {
        match self {
            BvSampleFormat::Int16 => "int16",
            BvSampleFormat::UInt16 => "uint16",
            BvSampleFormat::Int32 => "int32",
            BvSampleFormat::Float32 => "float32",
            BvSampleFormat::Ascii => "float64",
        }
    }
}

/// One channel of `[Channel Infos]`: `Ch<n>=<name>,<reference>,<resolution>,<unit>`.
#[derive(Debug, Clone, PartialEq)]
pub struct BvChannel {
    /// Name (`\1` decoded to a comma).
    pub name: String,
    /// Reference channel name (empty: the recording reference).
    pub reference: String,
    /// Physical units per stored unit; 1 when the header leaves it empty.
    pub resolution: f64,
    /// Unit of the physical values; `µV` when the header gives none.
    pub unit: String,
    /// `[Coordinates]` radius, theta and phi (degrees), when the header has them.
    pub coordinates: Option<[f64; 3]>,
}

/// One marker of `[Marker Infos]`: `Mk<n>=<type>,<description>,<position>,<points>,<channel>[,<date>]`.
#[derive(Debug, Clone, PartialEq)]
pub struct BvMarker {
    /// Marker type (`Stimulus`, `Response`, `New Segment`, `Comment`, …).
    pub kind: String,
    /// Description (`S  1`, …), possibly empty.
    pub description: String,
    /// First sample, 1-based as written.
    pub position: u64,
    /// Length in samples (1 for an instant).
    pub points: u64,
    /// Channel the marker belongs to, 1-based; 0 for all channels.
    pub channel: u64,
    /// `YYYYMMDDhhmmssuuuuuu` of a `New Segment` marker, when given.
    pub date: Option<String>,
}

/// The parsed header and marker file.
#[derive(Debug, Clone)]
pub struct BvHeader {
    /// Version from the first line (1 or 2).
    pub version: u32,
    /// The first line as written.
    pub identification: String,
    /// The comment line right after the first line (`Data created by the Vision Recorder`,
    /// `Data created from history path: …`), without its `;`.
    pub writer_note: Option<String>,
    /// Every `key=value` before `[Comment]`, by section, as written.
    pub sections: BTreeMap<String, BTreeMap<String, String>>,
    /// The free text after `[Comment]` (amplifier and filter settings).
    pub comment: String,
    /// The data file, resolved next to the header.
    pub data_path: PathBuf,
    /// The marker file, resolved next to the header, when the header names one that exists.
    pub marker_path: Option<PathBuf>,
    /// How samples are stored.
    pub format: BvSampleFormat,
    /// `VECTORIZED` (each channel's samples together) rather than `MULTIPLEXED` (frames).
    pub vectorized: bool,
    /// `UseBigEndianOrder=YES`.
    pub big_endian: bool,
    /// Sampling interval, microseconds.
    pub sampling_interval_us: f64,
    /// `DataPoints`, when the header gives it.
    pub data_points: Option<u64>,
    /// `SegmentationType` (`NOTSEGMENTED`, `MARKERBASED`, `FIXTIME`), upper-cased.
    pub segmentation: String,
    /// `SegmentDataPoints`, when the header gives it.
    pub segment_points: Option<u64>,
    /// `Averaged=YES`.
    pub averaged: bool,
    /// ASCII files: `DecimalSymbol` (`.` or `,`), `SkipLines`, `SkipColumns`.
    pub decimal_symbol: char,
    /// Lines skipped at the top of an ASCII data file.
    pub skip_lines: u64,
    /// Columns skipped at the start of every ASCII line.
    pub skip_columns: usize,
    /// Channels in header order.
    pub channels: Vec<BvChannel>,
    /// Markers in file order.
    pub markers: Vec<BvMarker>,
    /// Problems found while parsing.
    pub findings: Vec<Finding>,
}

/// Decode a header or marker file: UTF-8, else Windows-1252 (`Codepage=ANSI` and older files).
pub fn decode_text(b: &[u8]) -> String {
    let b = b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => windows1252(b),
    }
}

/// The version in a header's or marker file's first line (`Brain Vision Data Exchange Header
/// File Version 1.0`, `BrainVision Data Exchange Marker File, Version 2.0`, `BrainVision Core
/// Data Format 1.0`); `None` when the line is not one of those.
pub fn identification_version(line: &str, kind: &str) -> Option<u32> {
    let l = line
        .trim()
        .to_ascii_lowercase()
        .replace("brain vision", "brainvision");
    if !l.starts_with("brainvision") {
        return None;
    }
    if l.contains("core data format") {
        return Some(1);
    }
    if !l.contains(&format!("{kind} file")) {
        return None;
    }
    let v = l.rsplit("version").next()?.trim();
    let major = v.split('.').next()?.trim().parse().ok()?;
    Some(major)
}

/// True when `head` starts like a BrainVision header (`.vhdr`).
pub fn looks_like_vhdr(head: &[u8]) -> bool {
    let line = decode_text(head.get(..head.len().min(256)).unwrap_or(head));
    identification_version(line.lines().next().unwrap_or_default(), "header").is_some()
}

/// True when `head` starts like a BrainVision marker file (`.vmrk`).
pub fn looks_like_vmrk(head: &[u8]) -> bool {
    let line = decode_text(head.get(..head.len().min(256)).unwrap_or(head));
    identification_version(line.lines().next().unwrap_or_default(), "marker").is_some()
}

/// `key=value` lines by section (lower-case section and key), stopping at `[Comment]`;
/// returns the sections and the comment text.
pub fn parse_ini(text: &str) -> (BTreeMap<String, BTreeMap<String, String>>, String) {
    let mut sections: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut section = String::new();
    let mut lines = text.lines();
    lines.next(); // the identification line
    let mut comment = String::new();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t[1..t.len() - 1].trim().to_ascii_lowercase();
            if section == "comment" {
                comment = lines.collect::<Vec<_>>().join("\n");
                break;
            }
            sections.entry(section.clone()).or_default();
            continue;
        }
        if t.starts_with(';') || t.is_empty() {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            sections
                .entry(section.clone())
                .or_default()
                .insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    (sections, comment)
}

fn unescape(s: &str) -> String {
    s.replace("\\1", ",")
}

fn number(v: &str) -> Option<f64> {
    v.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

/// The data or marker file the header names, next to the header: as named, else matched
/// without regard to case, else the header's own stem with the named extension.
fn resolve(fs: &Fs, vhdr: &Path, named: &str, default_ext: &str) -> PathBuf {
    let dir = vhdr.parent().unwrap_or(Path::new(""));
    let name = Path::new(named.trim())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.is_empty() {
        let p = dir.join(&name);
        if fs.is_file(&p) {
            return p;
        }
        if let Some(p) = fs.find_in_dir(dir, &name) {
            return p;
        }
    }
    let ext = Path::new(&name)
        .extension()
        .map_or(default_ext.to_string(), |e| {
            e.to_string_lossy().into_owned()
        });
    vhdr.with_extension(ext)
}

/// Parse the header at `vhdr` and the marker file it names.
pub fn parse_header(fs: &Fs, vhdr: &Path) -> Result<BvHeader> {
    let mut f = fs.open(vhdr).map_err(|e| Error::io(vhdr, e))?;
    let len = f.size().map_err(|e| Error::io(vhdr, e))?;
    let raw = read_block(&mut f, vhdr, 0, len.min(MAX_BV_TEXT), len)?;
    let text = decode_text(&raw.bytes);
    let identification = text.lines().next().unwrap_or_default().trim().to_string();
    let Some(version) = identification_version(&identification, "header") else {
        return Err(Error::corrupt(
            BRAINVISION_FORMAT_ID,
            format!("the first line {identification:?} is not a BrainVision header identification"),
        ));
    };
    let (sections, comment) = parse_ini(&text);
    let writer_note = text
        .lines()
        .nth(1)
        .map(str::trim)
        .and_then(|l| l.strip_prefix(';'))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty());
    let mut findings = Vec::new();
    let empty = BTreeMap::new();
    let common = sections.get("common infos").unwrap_or(&empty);
    let get = |k: &str| common.get(k).map(String::as_str).unwrap_or_default();
    let data_format = get("dataformat").to_ascii_uppercase();
    let orientation = get("dataorientation").to_ascii_uppercase();
    let vectorized = match orientation.as_str() {
        "VECTORIZED" => true,
        "MULTIPLEXED" | "" => false,
        o => {
            return Err(Error::unsupported(
                BRAINVISION_FORMAT_ID,
                format!("data orientation {o}"),
                "Only MULTIPLEXED and VECTORIZED BrainVision data can be read.",
            ));
        }
    };
    let data_type = get("datatype").to_ascii_uppercase();
    if !data_type.is_empty() && data_type != "TIMEDOMAIN" {
        return Err(Error::unsupported(
            BRAINVISION_FORMAT_ID,
            format!("data type {data_type}"),
            "Only time-domain BrainVision data can be read; export the time-domain data from Analyzer.",
        ));
    }
    let binary = sections.get("binary infos").unwrap_or(&empty);
    let format = match data_format.as_str() {
        "BINARY" | "" => match binary
            .get("binaryformat")
            .map(|s| s.to_ascii_uppercase())
            .as_deref()
        {
            Some("INT_16") => BvSampleFormat::Int16,
            Some("UINT_16") => BvSampleFormat::UInt16,
            Some("INT_32") => BvSampleFormat::Int32,
            Some("IEEE_FLOAT_32") => BvSampleFormat::Float32,
            other => {
                return Err(Error::unsupported(
                    BRAINVISION_FORMAT_ID,
                    format!("binary format {}", other.unwrap_or("(none)")),
                    "Readable binary formats are INT_16, UINT_16, INT_32 and IEEE_FLOAT_32.",
                ));
            }
        },
        "ASCII" => {
            if vectorized {
                return Err(Error::unsupported(
                    BRAINVISION_FORMAT_ID,
                    "ASCII data in VECTORIZED orientation",
                    "Export the data as BINARY or as MULTIPLEXED ASCII.",
                ));
            }
            BvSampleFormat::Ascii
        }
        other => {
            return Err(Error::unsupported(
                BRAINVISION_FORMAT_ID,
                format!("data format {other}"),
                "Only BINARY and ASCII BrainVision data can be read.",
            ));
        }
    };
    let big_endian = binary
        .get("usebigendianorder")
        .is_some_and(|v| v.eq_ignore_ascii_case("YES"));
    let ascii = sections.get("ascii infos").unwrap_or(&empty);
    let decimal_symbol = ascii
        .get("decimalsymbol")
        .and_then(|s| s.chars().next())
        .unwrap_or('.');
    let skip_lines = ascii
        .get("skiplines")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let skip_columns = ascii
        .get("skipcolumns")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let nch: usize = get("numberofchannels").parse().map_err(|_| {
        Error::corrupt(
            BRAINVISION_FORMAT_ID,
            format!(
                "NumberOfChannels {:?} is not a number",
                get("numberofchannels")
            ),
        )
    })?;
    if nch == 0 || nch > MAX_BV_CHANNELS {
        return Err(Error::corrupt(
            BRAINVISION_FORMAT_ID,
            format!("{nch} channels (1 to {MAX_BV_CHANNELS} are read)"),
        ));
    }
    let sampling_interval_us = number(get("samplinginterval"))
        .filter(|v| *v > 0.0)
        .ok_or_else(|| {
            Error::corrupt(
                BRAINVISION_FORMAT_ID,
                format!(
                    "SamplingInterval {:?} is not a positive number",
                    get("samplinginterval")
                ),
            )
        })?;
    let data_points = get("datapoints").parse().ok();
    let segmentation = get("segmentationtype").to_ascii_uppercase();
    let segment_points = get("segmentdatapoints").parse().ok();
    let averaged = get("averaged").eq_ignore_ascii_case("YES");
    let infos = sections.get("channel infos").unwrap_or(&empty);
    let coords = sections.get("coordinates");
    let mut channels = Vec::with_capacity(nch);
    for c in 0..nch {
        let key = format!("ch{}", c + 1);
        let Some(v) = infos.get(&key) else {
            findings.push(Finding::error(
                "missing_channel",
                format!(
                    "[Channel Infos] has no Ch{}; it is named ch{c} with resolution 1 µV",
                    c + 1
                ),
            ));
            channels.push(BvChannel {
                name: format!("ch{c}"),
                reference: String::new(),
                resolution: 1.0,
                unit: "µV".into(),
                coordinates: None,
            });
            continue;
        };
        let p: Vec<&str> = v.split(',').collect();
        let resolution = match p.get(2).map(|s| s.trim()) {
            None | Some("") => 1.0,
            Some(s) => number(s).ok_or_else(|| {
                Error::corrupt(
                    BRAINVISION_FORMAT_ID,
                    format!("channel {}: resolution {s:?} is not a number", c + 1),
                )
            })?,
        };
        let unit = p
            .get(3)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("µV")
            .to_string();
        let coordinates = coords.and_then(|m| m.get(&key)).and_then(|s| {
            let v: Vec<f64> = s.split(',').filter_map(number).collect();
            (v.len() == 3).then(|| [v[0], v[1], v[2]])
        });
        channels.push(BvChannel {
            name: unescape(p.first().copied().unwrap_or_default().trim()),
            reference: unescape(p.get(1).copied().unwrap_or_default().trim()),
            resolution,
            unit,
            coordinates,
        });
    }
    let data_path = resolve(fs, vhdr, get("datafile"), "eeg");
    let marker_named = get("markerfile");
    let marker_path = (!marker_named.is_empty())
        .then(|| resolve(fs, vhdr, marker_named, "vmrk"))
        .filter(|p| fs.is_file(p));
    if !marker_named.is_empty() && marker_path.is_none() {
        findings.push(Finding::warning(
            "missing_marker_file",
            format!("the marker file {marker_named:?} is not next to the header"),
        ));
    }
    let mut h = BvHeader {
        version,
        identification,
        writer_note,
        sections,
        comment,
        data_path,
        marker_path,
        format,
        vectorized,
        big_endian,
        sampling_interval_us,
        data_points,
        segmentation,
        segment_points,
        averaged,
        decimal_symbol,
        skip_lines,
        skip_columns,
        channels,
        markers: Vec::new(),
        findings,
    };
    if let Some(m) = h.marker_path.clone() {
        parse_markers(fs, &m, &mut h)?;
    }
    Ok(h)
}

/// Read the `[Marker Infos]` of a marker file into `h.markers`.
pub fn parse_markers(fs: &Fs, path: &Path, h: &mut BvHeader) -> Result<()> {
    let mut f = fs.open(path).map_err(|e| Error::io(path, e))?;
    let len = f.size().map_err(|e| Error::io(path, e))?;
    let raw = read_block(&mut f, path, 0, len.min(MAX_BV_TEXT), len)?;
    let text = decode_text(&raw.bytes);
    if identification_version(text.lines().next().unwrap_or_default(), "marker").is_none() {
        h.findings.push(Finding::warning(
            "marker_identification",
            "the marker file's first line is not a BrainVision marker identification",
        ));
    }
    let (sections, _) = parse_ini(&text);
    let Some(m) = sections.get("marker infos") else {
        return Ok(());
    };
    let mut keyed: Vec<(u64, &String)> = m
        .iter()
        .filter_map(|(k, v)| Some((k.strip_prefix("mk")?.parse().ok()?, v)))
        .collect();
    keyed.sort_by_key(|(n, _)| *n);
    let mut bad = 0usize;
    for (_, v) in keyed.into_iter().take(MAX_BV_MARKERS) {
        let p: Vec<&str> = v.split(',').collect();
        let pos = p.get(2).and_then(|s| s.trim().parse::<u64>().ok());
        let Some(position) = pos else {
            bad += 1;
            continue;
        };
        h.markers.push(BvMarker {
            kind: unescape(p.first().copied().unwrap_or_default().trim()),
            description: unescape(p.get(1).copied().unwrap_or_default().trim()),
            position,
            points: p.get(3).and_then(|s| s.trim().parse().ok()).unwrap_or(1),
            channel: p.get(4).and_then(|s| s.trim().parse().ok()).unwrap_or(0),
            date: p
                .get(5)
                .map(|s| s.trim())
                .filter(|s| {
                    s.len() >= 14
                        && s.bytes().all(|c| c.is_ascii_digit())
                        && s.bytes().any(|c| c != b'0')
                })
                .map(str::to_string),
        });
    }
    if bad > 0 {
        h.findings.push(Finding::warning(
            "bad_marker",
            format!("{bad} marker(s) without a position were skipped"),
        ));
    }
    Ok(())
}

impl BvHeader {
    /// The program that wrote the header, from `writer_note`: `BrainVision Recorder`,
    /// `BrainVision Analyzer` (`history path`), `pybv`, `MNE-Python`, `FieldTrip`, `EEGLAB`,
    /// `BciPy`, else the note's own words after `Data created by`.
    pub fn writer(&self) -> Option<String> {
        let n = self.writer_note.as_deref()?;
        let l = n.to_ascii_lowercase();
        for (needle, name) in [
            ("vision recorder", "BrainVision Recorder"),
            ("history path", "BrainVision Analyzer"),
            ("pybv", "pybv"),
            ("mne", "MNE-Python"),
            ("fieldtrip", "FieldTrip"),
            ("eeglab", "EEGLAB"),
            ("bcipy", "BciPy"),
        ] {
            if l.contains(needle) {
                return Some(name.into());
            }
        }
        let rest = [
            "data created by the ",
            "data created by ",
            "data written by the ",
            "data written by ",
        ]
        .iter()
        .find_map(|p| l.starts_with(p).then(|| n[p.len()..].trim().to_string()))?;
        (!rest.is_empty()).then_some(rest)
    }
    /// Samples per second: 1,000,000 / `SamplingInterval`.
    pub fn rate_hz(&self) -> f64 {
        1e6 / self.sampling_interval_us
    }
    /// Recording start from the first `New Segment` marker's date,
    /// `yyyy-mm-ddThh:mm:ss.uuuuuu` local time.
    #[allow(clippy::many_single_char_names)]
    pub fn recorded_at(&self) -> Option<String> {
        let d = self.markers.iter().find_map(|m| m.date.as_deref())?;
        let field = |a: usize, b: usize| d.get(a..b)?.parse::<u32>().ok();
        let (y, mo, da, h, mi, s) = (
            field(0, 4)?,
            field(4, 6)?,
            field(6, 8)?,
            field(8, 10)?,
            field(10, 12)?,
            field(12, 14)?,
        );
        if !(1..=12).contains(&mo) || !(1..=31).contains(&da) || h > 23 || mi > 59 || s > 60 {
            return None;
        }
        let frac = d
            .get(14..)
            .filter(|f| !f.is_empty() && f.bytes().any(|c| c != b'0'));
        Some(match frac {
            Some(f) => format!("{y:04}-{mo:02}-{da:02}T{h:02}:{mi:02}:{s:02}.{f}"),
            None => format!("{y:04}-{mo:02}-{da:02}T{h:02}:{mi:02}:{s:02}"),
        })
    }
}

/// The BrainVision reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct BrainVisionReader;

impl FormatReader for BrainVisionReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::BRAINVISION)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: BRAINVISION_FORMAT_ID.into(),
            name: "BrainVision Core Data Format (.vhdr, .vmrk, .eeg)".into(),
            vendor: "Brain Products (BrainVision Recorder, Analyzer); also pybv, MNE-Python, FieldTrip, EEGLAB".into(),
            extensions: vec!["vhdr".into(), "vmrk".into()],
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::BRAINVISION.confidence,
            known_gaps: vec![
                "Frequency-domain data (DataType other than TIMEDOMAIN) and VECTORIZED ASCII data are refused".into(),
                "The [Comment] section (amplifier and filter settings) is kept as text, not parsed".into(),
                "Marker-based segments are sweeps only when SegmentDataPoints divides the data evenly".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        if looks_like_vhdr(head) {
            return Some(Detection {
                format_id: BRAINVISION_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: None,
            });
        }
        if has_extension(path, &["vmrk"]) && looks_like_vmrk(head) {
            return Some(Detection {
                format_id: BRAINVISION_FORMAT_ID,
                confidence: DetectConfidence::Definite,
                note: Some(
                    "reads the recording this marker file belongs to (needs its .vhdr)".into(),
                ),
            });
        }
        has_extension(path, &["vhdr"]).then(|| Detection {
            format_id: BRAINVISION_FORMAT_ID,
            confidence: DetectConfidence::ExtensionOnly,
            note: Some("BrainVision extension but no header identification line".into()),
        })
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        if let Some(d) = self.sniff(head, path) {
            return Some(d);
        }
        // a data file next to its header
        (has_extension(path, &["eeg", "dat"]) && input.fs().is_file(&path.with_extension("vhdr")))
            .then(|| Detection {
                format_id: BRAINVISION_FORMAT_ID,
                confidence: DetectConfidence::Likely,
                note: Some("reads the recording through the .vhdr next to it".into()),
            })
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(BrainVisionDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(BrainVisionDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}

/// An opened BrainVision recording.
#[derive(Debug)]
pub struct BrainVisionDataset {
    vhdr: PathBuf,
    fs: Fs,
    handle: Option<SourceFile>,
    header: BvHeader,
    data_len: u64,
    samples: u64,
    /// Byte offset of every sample frame (line) of an ASCII data file.
    lines: Vec<u64>,
    sweeps: Vec<(u64, u64)>,
    kinds: Vec<String>,
    descriptions: Vec<String>,
}

impl BrainVisionDataset {
    /// Open a header, marker or data file of a recording.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let fs = input.fs().clone();
        let given = input.path().to_path_buf();
        let vhdr = if has_extension(&given, &["vhdr"]) {
            given
        } else {
            let p = given.with_extension("vhdr");
            if !fs.is_file(&p) {
                return Err(Error::unsupported(
                    BRAINVISION_FORMAT_ID,
                    "a marker or data file without its header",
                    format!(
                        "Open the .vhdr header ({} was not found next to this file).",
                        p.display()
                    ),
                ));
            }
            p
        };
        let mut header = parse_header(&fs, &vhdr)?;
        let data_path = header.data_path.clone();
        let mut f = fs.open(&data_path).map_err(|e| {
            Error::unsupported(
                BRAINVISION_FORMAT_ID,
                "a header without its data file",
                format!(
                    "The data file {} could not be opened ({e}); it must sit next to the .vhdr.",
                    data_path.display()
                ),
            )
        })?;
        let data_len = f.size().map_err(|e| Error::io(&data_path, e))?;
        let nch = header.channels.len() as u64;
        let mut lines = Vec::new();
        let samples = if header.format == BvSampleFormat::Ascii {
            if data_len > MAX_BV_ASCII {
                return Err(Error::unsupported(
                    BRAINVISION_FORMAT_ID,
                    "ASCII data files over 512 MiB",
                    "Export the data as BINARY.",
                ));
            }
            let all = read_block(&mut f, &data_path, 0, data_len, data_len)?.bytes;
            let mut at = 0usize;
            let mut skipped = 0u64;
            while at < all.len() {
                let end = all[at..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(all.len(), |i| at + i + 1);
                if skipped < header.skip_lines {
                    skipped += 1;
                } else if all[at..end].iter().any(|c| !c.is_ascii_whitespace()) {
                    lines.push(at as u64);
                }
                at = end;
            }
            lines.push(all.len() as u64);
            (lines.len() - 1) as u64
        } else {
            let frame = header.format.bytes() * nch;
            let whole = data_len / frame;
            if data_len % frame != 0 {
                header.findings.push(Finding::warning(
                    "partial_frame",
                    format!(
                        "{} bytes after the last whole sample of every channel",
                        data_len % frame
                    ),
                ));
            }
            match header.data_points {
                Some(d) if d < whole => {
                    header.findings.push(Finding::warning(
                        "extra_bytes",
                        format!("the data file holds {whole} samples per channel; DataPoints gives {d}, which is used"),
                    ));
                    d
                }
                Some(d) if d > whole => {
                    if header.vectorized {
                        return Err(Error::corrupt(
                            BRAINVISION_FORMAT_ID,
                            format!(
                                "DataPoints gives {d} samples per channel; the vectorized data file holds {whole}"
                            ),
                        ));
                    }
                    header.findings.push(Finding::error(
                        "truncated",
                        format!(
                            "DataPoints gives {d} samples per channel; the data file holds {whole}"
                        ),
                    ));
                    whole
                }
                _ => whole,
            }
        };
        let sweeps = match header.segment_points {
            Some(n)
                if n > 0
                    && header.segmentation == "MARKERBASED"
                    && samples % n == 0
                    && samples > n =>
            {
                (0..samples / n).map(|k| (k * n, (k + 1) * n)).collect()
            }
            _ => vec![(0, samples)],
        };
        let mut kinds: Vec<String> = Vec::new();
        let mut descriptions: Vec<String> = Vec::new();
        for m in &header.markers {
            if !kinds.contains(&m.kind) {
                kinds.push(m.kind.clone());
            }
            if !descriptions.contains(&m.description) {
                descriptions.push(m.description.clone());
            }
        }
        Ok(BrainVisionDataset {
            vhdr,
            fs,
            handle: Some(f),
            header,
            data_len,
            samples,
            lines,
            sweeps,
            kinds,
            descriptions,
        })
    }

    /// The parsed header and markers.
    pub fn header(&self) -> &BvHeader {
        &self.header
    }

    fn handle(&mut self) -> Result<&mut SourceFile> {
        if self.handle.is_none() {
            self.handle = Some(
                self.fs
                    .open(&self.header.data_path)
                    .map_err(|e| Error::io(&self.header.data_path, e))?,
            );
        }
        Ok(self.handle.as_mut().expect("just opened"))
    }

    fn decode(&self, b: &[u8]) -> f64 {
        let be = self.header.big_endian;
        match self.header.format {
            BvSampleFormat::Int16 => {
                let a = [b[0], b[1]];
                f64::from(if be {
                    i16::from_be_bytes(a)
                } else {
                    i16::from_le_bytes(a)
                })
            }
            BvSampleFormat::UInt16 => {
                let a = [b[0], b[1]];
                f64::from(if be {
                    u16::from_be_bytes(a)
                } else {
                    u16::from_le_bytes(a)
                })
            }
            BvSampleFormat::Int32 => {
                let a = [b[0], b[1], b[2], b[3]];
                f64::from(if be {
                    i32::from_be_bytes(a)
                } else {
                    i32::from_le_bytes(a)
                })
            }
            BvSampleFormat::Float32 => {
                let a = [b[0], b[1], b[2], b[3]];
                f64::from(if be {
                    f32::from_be_bytes(a)
                } else {
                    f32::from_le_bytes(a)
                })
            }
            BvSampleFormat::Ascii => f64::NAN,
        }
    }

    #[allow(clippy::many_single_char_names)]
    fn read_ascii(&mut self, first: u64, count: u64) -> Result<Vec<Vec<f64>>> {
        let nch = self.header.channels.len();
        let (a, b) = (
            self.lines[first as usize],
            self.lines[(first + count) as usize],
        );
        let path = self.header.data_path.clone();
        let len = self.data_len;
        let block = read_block(self.handle()?, &path, a, b - a, len)?;
        let text = String::from_utf8_lossy(&block.bytes);
        let comma = self.header.decimal_symbol == ',';
        // grown as values are parsed: a line count says nothing about the bytes behind it
        let mut out = vec![Vec::new(); nch];
        for (k, line) in text.lines().filter(|l| !l.trim().is_empty()).enumerate() {
            let fields: Vec<&str> = if comma {
                line.split([' ', '\t', ';'])
                    .filter(|s| !s.is_empty())
                    .collect()
            } else {
                line.split([' ', '\t', ',', ';'])
                    .filter(|s| !s.is_empty())
                    .collect()
            };
            let values = fields.iter().skip(self.header.skip_columns);
            let mut n = 0;
            for (c, v) in values.take(nch).enumerate() {
                let v = if comma {
                    v.replace(',', ".")
                } else {
                    (*v).to_string()
                };
                let x = v.parse::<f64>().map_err(|_| {
                    Error::corrupt_at(
                        BRAINVISION_FORMAT_ID,
                        a,
                        format!(
                            "sample {} channel {c}: {v:?} is not a number",
                            first + k as u64
                        ),
                    )
                })?;
                out[c].push(x * self.header.channels[c].resolution);
                n += 1;
            }
            if n < nch {
                return Err(Error::corrupt_at(
                    BRAINVISION_FORMAT_ID,
                    a,
                    format!("sample {}: {n} values for {nch} channels", first + k as u64),
                ));
            }
        }
        Ok(out)
    }
}

impl Dataset for BrainVisionDataset {
    fn info(&self) -> Result<FileInfo> {
        let h = &self.header;
        let mut notes = vec![format!(
            "BrainVision header version {}: {} channel(s), {} {} data, {} samples per channel at {} Hz",
            h.version,
            h.channels.len(),
            if h.vectorized {
                "VECTORIZED"
            } else {
                "MULTIPLEXED"
            },
            h.format.name(),
            self.samples,
            h.rate_hz()
        )];
        if self.sweeps.len() > 1 {
            notes.push(format!(
                "marker-based segments of {} samples are sweeps ({} sweeps)",
                self.sweeps[0].1,
                self.sweeps.len()
            ));
        }
        if h.averaged {
            notes.push("averaged data (Averaged=YES)".into());
        }
        if !h.markers.is_empty() {
            notes.push(format!(
                "{} marker(s) in the `markers` table (onset in seconds after the first sample)",
                h.markers.len()
            ));
        }
        if h.findings.iter().any(|x| x.severity == Severity::Error) {
            notes.push("structural problems found (e.g. truncation); run `check`".into());
        }
        let mut extra = BTreeMap::new();
        extra.insert("header_version".into(), json!(h.version));
        extra.insert("binary_format".into(), json!(h.format.name()));
        extra.insert(
            "orientation".into(),
            json!(if h.vectorized {
                "VECTORIZED"
            } else {
                "MULTIPLEXED"
            }),
        );
        extra.insert("sampling_interval_us".into(), json!(h.sampling_interval_us));
        if !h.segmentation.is_empty() {
            extra.insert("segmentation".into(), json!(h.segmentation));
        }
        if h.averaged {
            extra.insert("averaged".into(), json!(true));
        }
        if let Some(t) = h.recorded_at() {
            extra.insert("recorded_at".into(), json!(t));
        }
        if let Some(w) = h.writer() {
            extra.insert("application".into(), json!(w));
        }
        if let Some(n) = &h.writer_note {
            extra.insert("written_by".into(), json!(n));
        }
        if self.sweeps.len() > 1 {
            extra.insert(
                "sweep_starts_s".into(),
                json!(
                    self.sweeps
                        .iter()
                        .map(|(a, _)| *a as f64 / h.rate_hz())
                        .collect::<Vec<_>>()
                ),
            );
        }
        let channels = h
            .channels
            .iter()
            .enumerate()
            .map(|(c, ch)| {
                let mut e = BTreeMap::new();
                if !ch.reference.is_empty() {
                    e.insert("reference".into(), json!(ch.reference));
                }
                e.insert("resolution".into(), json!(ch.resolution));
                if let Some([r, t, p]) = ch.coordinates {
                    e.insert(
                        "coordinates".into(),
                        json!({"radius": r, "theta_deg": t, "phi_deg": p}),
                    );
                }
                SignalChannelInfo {
                    index: c as u32,
                    name: if ch.name.is_empty() {
                        format!("ch{c}")
                    } else {
                        ch.name.clone()
                    },
                    unit: Some(ch.unit.clone()),
                    dtype: h.format.dtype().into(),
                    scale: ch.resolution,
                    offset: 0.0,
                    extra: e,
                }
            })
            .collect();
        let traces = if self.samples == 0 {
            notes.push("the data file holds no samples".into());
            Vec::new()
        } else {
            vec![TraceInfo {
                index: 0,
                name: Some("signals".into()),
                sample_rate_hz: h.rate_hz(),
                sample_count: self.sweeps.iter().map(|(a, b)| b - a).max().unwrap_or(0),
                sweep_count: u32::try_from(self.sweeps.len()).unwrap_or(u32::MAX),
                channels,
                start_s: Some(0.0),
                extra,
            }]
        };
        let tables = if h.markers.is_empty() {
            Vec::new()
        } else {
            let col = |i: u32, name: &str, dtype: &str, unit: Option<&str>| ColumnInfo {
                index: i,
                name: name.into(),
                label: None,
                dtype: dtype.into(),
                unit: unit.map(str::to_string),
                range: None,
                extra: BTreeMap::new(),
            };
            let mut kind = col(5, "type", "int32", None);
            kind.extra.insert("categories".into(), json!(self.kinds));
            let mut desc = col(6, "description", "int32", None);
            desc.extra
                .insert("categories".into(), json!(self.descriptions));
            vec![TableInfo {
                index: 0,
                name: Some("markers".into()),
                row_count: h.markers.len() as u64,
                columns: vec![
                    col(0, "onset_s", "float64", Some("s")),
                    col(1, "duration_s", "float64", Some("s")),
                    col(2, "sample", "int64", None),
                    col(3, "points", "int64", None),
                    col(4, "channel", "int32", None),
                    kind,
                    desc,
                ],
                extra: BTreeMap::new(),
            }]
        };
        Ok(FileInfo {
            path: self.vhdr.display().to_string(),
            size_bytes: self.data_len,
            format: BrainVisionReader.descriptor(),
            format_version: Some(h.version.to_string()),
            images: Vec::new(),
            tables,
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn member_files(&self) -> Vec<PathBuf> {
        let mut v = vec![self.vhdr.clone(), self.header.data_path.clone()];
        v.extend(self.header.marker_path.clone());
        v.into_iter().filter(|p| self.fs.is_file(p)).collect()
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let h = &self.header;
        Ok(json!({
            "identification": h.identification,
            "sections": h.sections,
            "comment": h.comment,
            "markers": h.markers.iter().map(|m| json!({
                "type": m.kind, "description": m.description, "position": m.position,
                "points": m.points, "channel": m.channel, "date": m.date,
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for k in [
            "format_version",
            "traces[].sample_rate_hz",
            "traces[].sample_count",
            "traces[].channels[].name",
            "traces[].channels[].unit",
            "traces[].channels[].scale",
            "tables[].row_count",
        ] {
            p.insert(k.into(), Source::PriorArt);
        }
        for k in [
            "traces[].sweep_count",
            "traces[].extra.recorded_at",
            "traces[].extra.sweep_starts_s",
        ] {
            p.insert(k.into(), Source::Inferred);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let h = &self.header;
        let mut out = vec![LsEntry {
            kind: "header".into(),
            name: self
                .vhdr
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            offset: None,
            size: None,
            image: None,
            details: json!({"version": h.version, "sections": h.sections.keys().collect::<Vec<_>>()}),
        }];
        if let Some(m) = &h.marker_path {
            out.push(LsEntry {
                kind: "markers".into(),
                name: m
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                offset: None,
                size: None,
                image: None,
                details: json!({"markers": h.markers.len()}),
            });
        }
        out.push(LsEntry {
            kind: "data".into(),
            name: h.data_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            offset: Some(0),
            size: Some(self.data_len),
            image: None,
            details: json!({"format": h.format.name(), "vectorized": h.vectorized, "samples": self.samples}),
        });
        Ok(out)
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            BRAINVISION_FORMAT_ID,
            "image planes",
            "BrainVision files hold sampled signals: use `openreadout trace`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        if index != 0 || self.samples == 0 {
            return Err(Error::Usage(format!("trace index {index} out of range")));
        }
        let Some(&(s0, s1)) = self.sweeps.get(sweep as usize) else {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({} sweeps)",
                self.sweeps.len()
            )));
        };
        let n = s1 - s0;
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let nch = self.header.channels.len() as u64;
        let mut count = max.min(n - first).min(MAX_BV_READ);
        let start = s0 + first;
        if self.header.format == BvSampleFormat::Ascii {
            count = count.min(1 << 20);
            let channels = self.read_ascii(start, count)?;
            return Ok(Trace {
                trace: index,
                sweep,
                first_sample: first,
                channels,
            });
        }
        let bytes = self.header.format.bytes();
        let path = self.header.data_path.clone();
        let len = self.data_len;
        let res: Vec<f64> = self.header.channels.iter().map(|c| c.resolution).collect();
        let mut channels: Vec<Vec<f64>> = vec![Vec::with_capacity(count as usize); nch as usize];
        if self.header.vectorized {
            count = count.min((MAX_BV_READ_BYTES / bytes / nch).max(1));
            for c in 0..nch {
                let at = (c * self.samples + start) * bytes;
                let block = read_block(self.handle()?, &path, at, count * bytes, len)?;
                if (block.len() as u64) < count * bytes {
                    return Err(Error::corrupt_at(
                        BRAINVISION_FORMAT_ID,
                        at,
                        "samples cut off by the end of the data file",
                    ));
                }
                let r = res[c as usize];
                let vals: Vec<f64> = block
                    .bytes
                    .chunks_exact(bytes as usize)
                    .map(|b| self.decode(b) * r)
                    .collect();
                channels[c as usize] = vals;
            }
        } else {
            let frame = bytes * nch;
            count = count.min((MAX_BV_READ_BYTES / frame).max(1));
            let at = start * frame;
            let block = read_block(self.handle()?, &path, at, count * frame, len)?;
            if (block.len() as u64) < count * frame {
                return Err(Error::corrupt_at(
                    BRAINVISION_FORMAT_ID,
                    at,
                    "samples cut off by the end of the data file",
                ));
            }
            for fr in block.bytes.chunks_exact(frame as usize) {
                for (c, b) in fr.chunks_exact(bytes as usize).enumerate() {
                    channels[c].push(self.decode(b) * res[c]);
                }
            }
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let h = &self.header;
        if index != 0 || h.markers.is_empty() {
            return Err(Error::Usage(format!("table index {index} out of range")));
        }
        let a = (first_row as usize).min(h.markers.len());
        let b = a
            .saturating_add(max_rows.min(usize::MAX as u64) as usize)
            .min(h.markers.len());
        let rows = &h.markers[a..b];
        let rate = h.rate_hz();
        let code = |list: &[String], v: &str| {
            list.iter()
                .position(|x| x == v)
                .map_or(f64::NAN, |i| i as f64)
        };
        let sample = |m: &BvMarker| m.position.saturating_sub(1) as f64;
        Ok(Table {
            table: 0,
            first_row: a as u64,
            columns: vec![
                rows.iter().map(|m| sample(m) / rate).collect(),
                rows.iter().map(|m| m.points as f64 / rate).collect(),
                rows.iter().map(sample).collect(),
                rows.iter().map(|m| m.points as f64).collect(),
                rows.iter().map(|m| m.channel as f64).collect(),
                rows.iter().map(|m| code(&self.kinds, &m.kind)).collect(),
                rows.iter()
                    .map(|m| code(&self.descriptions, &m.description))
                    .collect(),
            ],
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let h = &self.header;
        let mut r = CheckReport::new(self.vhdr.display().to_string(), BRAINVISION_FORMAT_ID);
        r.performed("header: identification line, [Common Infos], [Binary Infos] or [ASCII Infos], a [Channel Infos] entry per channel");
        r.performed("data file length against the channels and DataPoints; marker positions within the data");
        for x in &h.findings {
            r.push(x.clone());
        }
        let total = self.samples;
        let past = h
            .markers
            .iter()
            .filter(|m| m.position == 0 || m.position > total.max(1))
            .count();
        if past > 0 {
            r.push(Finding::warning(
                "marker_past_end",
                format!("{past} marker(s) lie outside the {total} samples"),
            ));
        }
        if h.channels.iter().any(|c| c.resolution == 0.0) {
            r.push(Finding::error(
                "zero_resolution",
                "a channel's resolution is 0",
            ));
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests;
