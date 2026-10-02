//! Open Ephys binary format: `[Record Node N/]experimentE/recordingR/` directories, each with a
//! `structure.oebin` (JSON), `continuous/<stream>/continuous.dat` (interleaved int16) with
//! `sample_numbers.npy` (or `timestamps.npy` before GUI 0.6), and `events/<stream>/*.npy`.

use std::path::{Path, PathBuf};

use openreadout_core::bytes::read_file_capped_in;
use openreadout_core::model::Finding;
use openreadout_core::source::Fs;
use openreadout_core::{Error, Result};
use serde_json::Value;

use super::OPEN_EPHYS_FORMAT_ID;
use super::npy::{NpyType, read_header, read_strings, read_values};

/// Largest `structure.oebin` read, bytes.
pub const MAX_OEBIN_LEN: u64 = 64 << 20;
/// Directory levels searched below the input for `structure.oebin` files.
pub const MAX_OEBIN_DEPTH: usize = 5;
/// Recordings accepted at most.
pub const MAX_RECORDINGS: usize = 10_000;
/// Event rows gathered at most.
pub const MAX_EVENT_ROWS: usize = 20_000_000;

/// One channel of a continuous stream.
#[derive(Debug, Clone, PartialEq)]
pub struct OeChannel {
    /// `channel_name`.
    pub name: String,
    /// Volts (or µV) per count, `bit_volts`.
    pub bit_volts: f64,
    /// `units` as recorded (may be empty).
    pub units: String,
}

/// One continuous stream of one recording.
#[derive(Debug, Clone)]
pub struct OeStream {
    /// `folder_name` without the trailing slash.
    pub folder: String,
    /// Samples per second.
    pub sample_rate: f64,
    /// Channels in stored order.
    pub channels: Vec<OeChannel>,
    /// `continuous.dat`.
    pub data_path: PathBuf,
    /// Samples per channel in the file.
    pub samples: u64,
    /// Sample number of the first sample (acquisition clock).
    pub first_sample: Option<i64>,
    /// First synchronized timestamp, seconds (GUI 0.6+ `timestamps.npy`, 0.5
    /// `synchronized_timestamps.npy`).
    pub first_timestamp_s: Option<f64>,
    /// `source_processor_name`.
    pub processor: String,
    /// `stream_name` (GUI 0.6+).
    pub stream_name: String,
}

/// One event stream of one recording.
#[derive(Debug, Clone)]
pub struct OeEventStream {
    /// `folder_name` without the trailing slash.
    pub folder: String,
    /// `channel_name`.
    pub channel_name: String,
    /// `type` (`int16` TTL, `string` text, `uint8` binary).
    pub kind: String,
    /// Samples per second of the sample numbers.
    pub sample_rate: f64,
    /// The folder.
    pub dir: PathBuf,
}

/// One `recordingR` directory.
#[derive(Debug, Clone)]
pub struct OeRecording {
    /// `Record Node` directory name ("" before multi-node recording).
    pub node: String,
    /// Experiment number.
    pub experiment: u32,
    /// Recording number.
    pub recording: u32,
    /// The directory.
    pub dir: PathBuf,
    /// `GUI version`.
    pub gui_version: String,
    /// Continuous streams.
    pub streams: Vec<OeStream>,
    /// Event streams.
    pub events: Vec<OeEventStream>,
    /// Spike groups listed in the oebin (not decoded).
    pub spike_groups: usize,
}

/// A trace of the normalized view: one stream across the recordings of an experiment.
#[derive(Debug, Clone)]
pub struct OeTrace {
    /// Recording indices (into the dataset's recordings), one per sweep.
    pub recordings: Vec<usize>,
    /// Stream index within each recording.
    pub stream: Vec<usize>,
}

/// Every recording found below `root`, sorted by node, experiment and recording.
pub fn discover(fs: &Fs, root: &Path) -> Result<(Vec<OeRecording>, Vec<Finding>)> {
    let mut dirs = Vec::new();
    walk(fs, root, 0, &mut dirs);
    if dirs.is_empty() {
        return Err(Error::corrupt(
            OPEN_EPHYS_FORMAT_ID,
            format!("no structure.oebin below {}", root.display()),
        ));
    }
    if dirs.len() > MAX_RECORDINGS {
        return Err(Error::unsupported(
            OPEN_EPHYS_FORMAT_ID,
            format!("more than {MAX_RECORDINGS} recordings"),
            "open one Record Node or experiment directory at a time.",
        ));
    }
    let mut findings = Vec::new();
    let mut out = Vec::new();
    for d in dirs {
        out.push(recording(fs, &d, &mut findings)?);
    }
    out.sort_by(|a, b| {
        (&a.node, a.experiment, a.recording).cmp(&(&b.node, b.experiment, b.recording))
    });
    Ok((out, findings))
}

fn walk(fs: &Fs, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if fs.is_file(&dir.join("structure.oebin")) {
        out.push(dir.to_path_buf());
        return;
    }
    if depth >= MAX_OEBIN_DEPTH {
        return;
    }
    let Ok(rd) = fs.read_dir(dir) else {
        return;
    };
    let mut subs: Vec<PathBuf> = rd
        .filter_map(std::result::Result::ok)
        .filter(|e| e.metadata().is_ok_and(|m| m.is_dir()))
        .map(|e| e.path())
        .collect();
    subs.sort();
    for s in subs {
        walk(fs, &s, depth + 1, out);
    }
}

fn number_after(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.parse().ok()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_end_matches('/')
        .to_string()
}

fn first_int(fs: &Fs, path: &Path) -> Result<Option<i64>> {
    if !fs.is_file(path) {
        return Ok(None);
    }
    let h = read_header(fs, path)?;
    if !h.dtype.is_integer() || h.is_empty() {
        return Ok(None);
    }
    Ok(read_values(fs, path, &h, 0, 1)?.first().map(|v| *v as i64))
}

fn first_float(fs: &Fs, path: &Path) -> Result<Option<f64>> {
    if !fs.is_file(path) {
        return Ok(None);
    }
    let h = read_header(fs, path)?;
    if h.dtype.is_integer() || h.is_empty() || matches!(h.dtype, NpyType::Bytes(_)) {
        return Ok(None);
    }
    Ok(read_values(fs, path, &h, 0, 1)?.first().copied())
}

fn recording(fs: &Fs, dir: &Path, findings: &mut Vec<Finding>) -> Result<OeRecording> {
    let name = |p: &Path| {
        p.file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    let rec_name = name(dir);
    let exp_dir = dir.parent().unwrap_or(dir);
    let exp_name = name(exp_dir);
    let node_dir = exp_dir.parent().unwrap_or(exp_dir);
    let node_name = name(node_dir);
    let recording = number_after(&rec_name, "recording").unwrap_or(1);
    let experiment = number_after(&exp_name, "experiment").unwrap_or(1);
    let node = if node_name.starts_with("Record") {
        node_name
    } else {
        String::new()
    };
    let oebin = dir.join("structure.oebin");
    let bytes = read_file_capped_in(
        fs,
        &oebin,
        MAX_OEBIN_LEN,
        OPEN_EPHYS_FORMAT_ID,
        "structure.oebin",
    )?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| {
        Error::corrupt(
            OPEN_EPHYS_FORMAT_ID,
            format!("{}: not valid JSON ({e})", oebin.display()),
        )
    })?;
    let mut rec = OeRecording {
        node,
        experiment,
        recording,
        dir: dir.to_path_buf(),
        gui_version: text(&v, "GUI version"),
        streams: Vec::new(),
        events: Vec::new(),
        spike_groups: v
            .get("spikes")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    };
    for c in v
        .get("continuous")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let folder = text(c, "folder_name");
        let sdir = dir.join("continuous").join(&folder);
        if !fs.is_dir(&sdir) {
            findings.push(Finding::warning(
                "missing_stream",
                format!(
                    "{}: structure.oebin lists continuous/{folder}, which is missing",
                    dir.display()
                ),
            ));
            continue;
        }
        let channels: Vec<OeChannel> = c
            .get("channels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|ch| OeChannel {
                name: text(ch, "channel_name"),
                bit_volts: ch
                    .get("bit_volts")
                    .and_then(Value::as_f64)
                    .unwrap_or(f64::NAN),
                units: text(ch, "units"),
            })
            .collect();
        let sample_rate = c
            .get("sample_rate")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        if channels.is_empty() || !(sample_rate.is_finite() && sample_rate > 0.0) {
            findings.push(Finding::error(
                "bad_stream",
                format!(
                    "{}: continuous/{folder} has {} channels at {sample_rate} Hz",
                    dir.display(),
                    channels.len()
                ),
            ));
            continue;
        }
        let data_path = sdir.join("continuous.dat");
        let size = fs.metadata(&data_path).map_or(0, |m| m.len());
        let row = 2 * channels.len() as u64;
        if !size.is_multiple_of(row) {
            findings.push(Finding::warning(
                "partial_sample",
                format!(
                    "{}: {size} bytes is not a whole number of {row}-byte samples",
                    data_path.display()
                ),
            ));
        }
        let sn = sdir.join("sample_numbers.npy");
        let ts = sdir.join("timestamps.npy");
        let first_sample = match first_int(fs, &sn)? {
            Some(v) => Some(v),
            None => first_int(fs, &ts)?,
        };
        let first_timestamp_s = match first_float(fs, &ts)? {
            Some(v) => Some(v),
            None => first_float(fs, &sdir.join("synchronized_timestamps.npy"))?,
        };
        rec.streams.push(OeStream {
            folder,
            sample_rate,
            channels,
            data_path,
            samples: size / row,
            first_sample,
            first_timestamp_s,
            processor: text(c, "source_processor_name"),
            stream_name: text(c, "stream_name"),
        });
    }
    for e in v
        .get("events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let folder = text(e, "folder_name");
        let edir = dir.join("events").join(&folder);
        if !fs.is_dir(&edir) {
            findings.push(Finding::warning(
                "missing_stream",
                format!(
                    "{}: structure.oebin lists events/{folder}, which is missing",
                    dir.display()
                ),
            ));
            continue;
        }
        rec.events.push(OeEventStream {
            folder,
            channel_name: text(e, "channel_name"),
            kind: text(e, "type"),
            sample_rate: e
                .get("sample_rate")
                .and_then(Value::as_f64)
                .unwrap_or(f64::NAN),
            dir: edir,
        });
    }
    Ok(rec)
}

/// Group streams into traces: the same node, experiment, folder and channel list; one sweep per
/// recording.
pub fn layout(recs: &[OeRecording]) -> Vec<OeTrace> {
    let mut out: Vec<(String, u32, String, Vec<OeChannel>, f64, OeTrace)> = Vec::new();
    for (ri, r) in recs.iter().enumerate() {
        for (si, s) in r.streams.iter().enumerate() {
            let key = (r.node.clone(), r.experiment, s.folder.clone());
            if let Some(t) = out.iter_mut().find(|t| {
                (t.0.clone(), t.1, t.2.clone()) == key
                    && t.3 == s.channels
                    && t.4.to_bits() == s.sample_rate.to_bits()
            }) {
                t.5.recordings.push(ri);
                t.5.stream.push(si);
            } else {
                out.push((
                    key.0,
                    key.1,
                    key.2,
                    s.channels.clone(),
                    s.sample_rate,
                    OeTrace {
                        recordings: vec![ri],
                        stream: vec![si],
                    },
                ));
            }
        }
    }
    out.into_iter().map(|t| t.5).collect()
}

/// One event row.
#[derive(Debug, Clone, Default)]
pub struct EventRows {
    /// Event-stream index (into `stream_list`) of each row.
    pub stream: Vec<f64>,
    /// Sample number.
    pub sample_number: Vec<f64>,
    /// Sample number / sample rate.
    pub time_s: Vec<f64>,
    /// Synchronized timestamp (seconds) when the file has one.
    pub timestamp_s: Vec<f64>,
    /// TTL state (±line).
    pub state: Vec<f64>,
    /// TTL line (`channels.npy`, GUI < 0.6).
    pub line: Vec<f64>,
    /// Full TTL word.
    pub full_word: Vec<f64>,
    /// Index into `texts` for text events.
    pub text: Vec<f64>,
    /// Distinct texts in order of first appearance.
    pub texts: Vec<String>,
    /// `(recording index, event-stream index)` of each listed stream.
    pub stream_list: Vec<(usize, usize)>,
    /// Streams not decoded (binary events), as `node/experiment/recording/folder`.
    pub undecoded: Vec<String>,
}

fn load_numbers(fs: &Fs, p: &Path, n: usize) -> Result<Vec<f64>> {
    if !fs.is_file(p) {
        return Ok(vec![f64::NAN; n]);
    }
    let h = read_header(fs, p)?;
    if h.shape.len() != 1 {
        return Ok(vec![f64::NAN; n]);
    }
    let mut v = read_values(fs, p, &h, 0, n as u64)?;
    v.resize(n, f64::NAN);
    Ok(v)
}

/// Read every TTL and text event of every recording.
pub fn events(fs: &Fs, recs: &[OeRecording]) -> Result<EventRows> {
    let mut out = EventRows::default();
    for (ri, r) in recs.iter().enumerate() {
        for (ei, e) in r.events.iter().enumerate() {
            let has = |f: &str| fs.is_file(&e.dir.join(f));
            let is_text = has("text.npy");
            let is_ttl = has("states.npy") || has("channel_states.npy");
            if !is_text && !is_ttl {
                out.undecoded.push(format!(
                    "{}/experiment{}/recording{}/{}",
                    r.node, r.experiment, r.recording, e.folder
                ));
                continue;
            }
            // sample numbers: sample_numbers.npy (0.6+), else integer timestamps.npy
            let sn_path = if has("sample_numbers.npy") {
                e.dir.join("sample_numbers.npy")
            } else {
                e.dir.join("timestamps.npy")
            };
            let sh = read_header(fs, &sn_path)?;
            if !sh.dtype.is_integer() || sh.shape.len() != 1 {
                return Err(Error::corrupt(
                    OPEN_EPHYS_FORMAT_ID,
                    format!("{}: sample numbers are not integers", sn_path.display()),
                ));
            }
            let n = sh.len() as usize;
            if out.sample_number.len() + n > MAX_EVENT_ROWS {
                return Err(Error::unsupported(
                    OPEN_EPHYS_FORMAT_ID,
                    format!("more than {MAX_EVENT_ROWS} events"),
                    "open one experiment or recording directory at a time.",
                ));
            }
            let sn = read_values(fs, &sn_path, &sh, 0, n as u64)?;
            let stream_index = out.stream_list.len() as f64;
            out.stream_list.push((ri, ei));
            let ts = if has("sample_numbers.npy") && has("timestamps.npy") {
                load_numbers(fs, &e.dir.join("timestamps.npy"), n)?
            } else if has("synchronized_timestamps.npy") {
                load_numbers(fs, &e.dir.join("synchronized_timestamps.npy"), n)?
            } else {
                vec![f64::NAN; n]
            };
            let states = if has("states.npy") {
                load_numbers(fs, &e.dir.join("states.npy"), n)?
            } else {
                load_numbers(fs, &e.dir.join("channel_states.npy"), n)?
            };
            let lines = if is_ttl {
                load_numbers(fs, &e.dir.join("channels.npy"), n)?
            } else {
                vec![f64::NAN; n]
            };
            let words = load_numbers(fs, &e.dir.join("full_words.npy"), n)?;
            let texts = if is_text {
                let p = e.dir.join("text.npy");
                let h = read_header(fs, &p)?;
                let mut t = read_strings(fs, &p, &h)?;
                t.resize(n, String::new());
                Some(t)
            } else {
                None
            };
            for k in 0..n {
                out.stream.push(stream_index);
                out.sample_number.push(sn[k]);
                out.time_s.push(if e.sample_rate > 0.0 {
                    sn[k] / e.sample_rate
                } else {
                    f64::NAN
                });
                out.timestamp_s.push(ts[k]);
                if is_ttl {
                    out.state.push(states[k]);
                    out.line.push(lines[k]);
                    out.full_word.push(words[k]);
                } else {
                    out.state.push(f64::NAN);
                    out.line.push(f64::NAN);
                    out.full_word.push(f64::NAN);
                }
                if let Some(t) = &texts {
                    let s = &t[k];
                    let idx = if let Some(i) = out.texts.iter().position(|x| x == s) {
                        i
                    } else {
                        out.texts.push(s.clone());
                        out.texts.len() - 1
                    };
                    out.text.push(idx as f64);
                } else {
                    out.text.push(f64::NAN);
                }
            }
        }
    }
    Ok(out)
}
