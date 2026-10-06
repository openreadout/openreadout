//! NWB 2.x writer (`export --to nwb`): the traces of an electrophysiology file (ABF sweeps,
//! Neuralynx, Blackrock, SpikeGLX, Intan, ...) as plain `TimeSeries` under `/acquisition/`, one
//! per trace, sweep and unit (channels that share a unit share a series, `data` of shape
//! `[samples, channels]` in physical units as float64, `starting_time` + `rate`). Session fields
//! (`identifier`, `session_description`, `session_start_time`, `file_create_date`,
//! `general/experimenter`, `general/notes`, `general/source_script`) come from the file. The
//! layout follows the public NWB 2.x schema (<https://nwb-schema.readthedocs.io>); the HDF5 file
//! is written with `hdf5-pure`. See `docs/formats/hdf5.md` § Writing NWB.
//!
//! The file is written under a temporary name, re-opened with this crate's NWB reader (every
//! series found with the written shape, rate and unit, every sample compared bit for bit), and
//! only then renamed into place.

use std::path::{Path, PathBuf};

use hdf5_pure::{AttrValue, FileBuilder};
use openreadout_core::model::{FileInfo, TraceInfo};
use openreadout_core::reader::Dataset;
use openreadout_core::{Error, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use xxhash_rust::xxh3::{Xxh3, xxh3_128};

use crate::nwb::{NWB_FORMAT_ID, NwbDataset};

/// NWB schema version written in the root `nwb_version` attribute.
pub const NWB_VERSION: &str = "2.7.0";
/// Most samples (all series together, float64) one export holds in memory: 512 MiB.
const MAX_VALUES: u64 = 1 << 26;
/// Samples read from the source per call.
const CHUNK: u64 = 1 << 20;
/// Target size of one HDF5 chunk of `data`, in bytes.
const CHUNK_BYTES: u64 = 1 << 20;
/// Largest chunk dimension written (see the chunking comment in `export_nwb`).
const MAX_CHUNK_DIM: u64 = 0xFFFF;

/// What to write.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NwbExportOptions {
    /// Only this trace (default: every trace of the file).
    pub trace: Option<u32>,
    /// Only this sweep (default: every sweep).
    pub sweep: Option<u32>,
    /// Samples `[first, last]` of each sweep (zero-based, inclusive; `None` = to the end).
    pub rows: Option<(u64, Option<u64>)>,
    /// Replace an existing output file.
    pub overwrite: bool,
}

/// One `TimeSeries` written.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NwbSeriesReport {
    /// Name under `/acquisition/`.
    pub name: String,
    /// Source trace index.
    pub trace: u32,
    /// Source sweep index.
    pub sweep: u32,
    /// Samples (rows of `data`).
    pub samples: u64,
    /// Channels (columns of `data`).
    pub channels: u32,
    /// Source channel indices, in column order.
    pub channel_indices: Vec<u32>,
    /// `data` unit, when the channels record one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// `starting_time` `rate` in Hz.
    pub rate_hz: f64,
    /// `starting_time` in seconds.
    pub starting_time_s: f64,
}

/// Output of `export --to nwb`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NwbExportReport {
    /// The input file.
    pub input: String,
    /// The NWB file written.
    pub output: String,
    /// Always `nwb`.
    pub format: String,
    /// NWB schema version written (`nwb_version`).
    pub nwb_version: String,
    /// `identifier` written (a UUID derived from the source and the time of writing).
    pub identifier: String,
    /// `session_start_time` written.
    pub session_start_time: String,
    /// One entry per `TimeSeries`, in the order written.
    pub series: Vec<NwbSeriesReport>,
    /// Samples written over all series and channels.
    pub samples_written: u64,
    /// Size of the written file in bytes.
    pub bytes_written: u64,
    /// True when the file was re-opened and every series and sample matched.
    pub verified: bool,
    /// What the export assumed (e.g. a session start taken from the file's modification time).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Default output path: `<stem>.nwb` (`<stem>.traceT.nwb`, `<stem>.sweepS.nwb` when selected).
pub fn default_nwb_output(base: &Path, opts: &NwbExportOptions) -> PathBuf {
    let stem = base
        .file_stem()
        .map_or_else(|| "export".into(), |s| s.to_string_lossy().to_string());
    let mid = match (opts.trace, opts.sweep) {
        (None, None) => String::new(),
        (None, Some(s)) => format!(".sweep{s}"),
        (Some(t), None) => format!(".trace{t}"),
        (Some(t), Some(s)) => format!(".trace{t}.sweep{s}"),
    };
    base.with_file_name(format!("{stem}{mid}.nwb"))
}

/// A UUID (version 4 layout) from a 128-bit hash.
fn uuid(seed: &[u8]) -> String {
    let mut b = xxh3_128(seed).to_le_bytes();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = b.iter().fold(String::with_capacity(32), |mut acc, x| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{x:02x}");
        acc
    });
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// A link name from free text: `/` and control characters replaced, never empty.
fn link_name(s: &str) -> String {
    let n: String = s
        .trim()
        .chars()
        .map(|c| if c == '/' || c.is_control() { '_' } else { c })
        .collect();
    match n.as_str() {
        "" => "series".into(),
        "." | ".." => format!("_{n}"),
        _ => n,
    }
}

fn text(v: &str) -> AttrValue {
    AttrValue::VarLenString(v.to_string())
}

/// ISO-8601 with an explicit offset (`Z` becomes `+00:00`; a time without one is taken as UTC).
fn with_offset(iso: &str) -> (String, bool) {
    let t = iso.trim();
    if let Some(s) = t.strip_suffix('Z') {
        return (format!("{s}+00:00"), true);
    }
    let tail = t.get(19..).unwrap_or("");
    if tail.contains('+') || tail.contains('-') {
        return (t.to_string(), true);
    }
    (format!("{t}+00:00"), false)
}

/// The recording start: `extra.acquired_at`/`start_time` of a trace, else of any table or
/// image; `None` when the file records none.
fn acquired_at(info: &FileInfo) -> Option<String> {
    let keys = [
        "acquired_at",
        "start_time",
        "recording_start",
        "session_start_time",
    ];
    let from = |e: &std::collections::BTreeMap<String, Value>| {
        keys.iter()
            .find_map(|k| e.get(*k).and_then(Value::as_str).map(str::to_string))
    };
    info.traces
        .iter()
        .find_map(|t| from(&t.extra))
        .or_else(|| info.tables.iter().find_map(|t| from(&t.extra)))
        .or_else(|| info.images.iter().find_map(|i| i.acquired_at.clone()))
        .filter(|s| openreadout_core::time::iso8601_to_unix(s).is_some())
}

/// The file's operator/experimenter, when recorded.
fn experimenter(info: &FileInfo) -> Option<String> {
    // not `creator`: ABF and others name the acquisition software there
    let keys = ["operator", "experimenter"];
    info.traces
        .iter()
        .find_map(|t| {
            keys.iter()
                .find_map(|k| t.extra.get(*k).and_then(Value::as_str))
        })
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

/// Series planned from one trace and sweep: channel indices grouped by unit, in first-seen order.
fn unit_groups(t: &TraceInfo) -> Vec<(Option<String>, Vec<u32>)> {
    let mut groups: Vec<(Option<String>, Vec<u32>)> = Vec::new();
    for (i, c) in t.channels.iter().enumerate() {
        let u = c.unit.clone().filter(|u| !u.trim().is_empty());
        match groups.iter_mut().find(|g| g.0 == u) {
            Some(g) => g.1.push(i as u32),
            None => groups.push((u, vec![i as u32])),
        }
    }
    groups
}

struct Planned {
    report: NwbSeriesReport,
    description: String,
    comments: String,
    data: Vec<f64>,
    digest: u128,
}

fn digest(vals: &[f64]) -> u128 {
    let mut h = Xxh3::new();
    for v in vals {
        let bits = if v.is_nan() {
            f64::NAN.to_bits()
        } else {
            v.to_bits()
        };
        h.update(&bits.to_le_bytes());
    }
    h.digest128()
}

/// Export the traces of `ds` to `output` as NWB 2.x.
pub fn export_nwb(
    ds: &mut dyn Dataset,
    input: &Path,
    output: &Path,
    opts: &NwbExportOptions,
) -> Result<NwbExportReport> {
    let info = ds.info()?;
    let experiment = openreadout_core::experiment::of_dataset(&*ds, &info);
    if info.traces.is_empty() {
        return Err(Error::unsupported(
            NWB_FORMAT_ID,
            format!("NWB export of a {} file", info.format.name),
            if info.tables.is_empty() {
                "NWB export writes sampled signals (traces); this file holds none. `info` lists what it holds."
            } else {
                "NWB export writes sampled signals (traces); export tables with `--to csv` or `--to parquet`."
            },
        ));
    }
    if matches!(
        info.format.family.as_str(),
        "nmr" | "spectroscopy" | "chromatography" | "mass-spectrometry"
    ) {
        return Err(Error::unsupported(
            NWB_FORMAT_ID,
            format!("NWB export of a {} file", info.format.name),
            "NWB is a neurophysiology format; export NMR and other spectra with `--to jcamp`, chromatograms with `--to csv` or `--to parquet`.",
        ));
    }
    let traces: Vec<TraceInfo> = match opts.trace {
        Some(i) => vec![
            info.traces
                .iter()
                .find(|t| t.index == i)
                .ok_or_else(|| {
                    Error::Usage(format!(
                        "--trace {i} out of range (file has {} traces)",
                        info.traces.len()
                    ))
                })?
                .clone(),
        ],
        None => info.traces.clone(),
    };
    let (first, last) = opts.rows.unwrap_or((0, None));
    // plan: (trace, sweep, samples) and the value count. The trace is borrowed: a recording
    // with many sweeps would otherwise hold one copy of its description per sweep.
    let mut plan: Vec<(&TraceInfo, u32, u64, u64)> = Vec::new();
    let mut values = 0u64;
    for t in &traces {
        if t.channels.is_empty() {
            continue;
        }
        let sweeps: Vec<u32> = match opts.sweep {
            Some(s) if s >= t.sweep_count => {
                return Err(Error::Usage(format!(
                    "--sweep {s} out of range (trace {} has {} sweeps)",
                    t.index, t.sweep_count
                )));
            }
            Some(s) => vec![s],
            None => (0..t.sweep_count).collect(),
        };
        for s in sweeps {
            let n = openreadout_core::trace::sweep_samples(t, s);
            let a = first.min(n);
            let b = last.map_or(n, |l| l.saturating_add(1).min(n)).max(a);
            values = values.saturating_add((b - a).saturating_mul(t.channels.len() as u64));
            plan.push((t, s, a, b));
        }
    }
    if plan.is_empty() {
        return Err(Error::unsupported(
            NWB_FORMAT_ID,
            "traces without channels",
            "`info` lists the file's traces and their channels.",
        ));
    }
    if values > MAX_VALUES {
        return Err(Error::Usage(format!(
            "{values} samples are more than one NWB export holds in memory ({MAX_VALUES}); narrow it with --trace, --sweep and --rows"
        )));
    }
    if output.exists() && !opts.overwrite {
        return Err(Error::Usage(format!(
            "{} exists; pass --overwrite to replace it",
            output.display()
        )));
    }
    let source_name = input.file_name().map_or_else(
        || input.display().to_string(),
        |n| n.to_string_lossy().to_string(),
    );
    let mut notes = Vec::new();

    // ------------------------------------------------------------ read and plan the series
    let multi_trace = traces.len() > 1;
    let mut series: Vec<Planned> = Vec::new();
    let mut used: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (t, sweep, a, b) in &plan {
        let n = b - a;
        let nch = t.channels.len();
        let mut cols: Vec<Vec<f64>> = vec![Vec::with_capacity(n as usize); nch];
        let mut pos = *a;
        while pos < *b {
            let k = CHUNK.min(b - pos);
            let tr = ds.read_trace(t.index, *sweep, pos, k)?;
            if tr.channels.len() != nch || tr.channels.iter().any(|c| c.len() as u64 != k) {
                return Err(Error::Other(format!(
                    "reader returned {} channels for a request of {k} samples x {nch} channels",
                    tr.channels.len()
                )));
            }
            for (d, c) in cols.iter_mut().zip(tr.channels) {
                d.extend(c);
            }
            pos += k;
        }
        let groups = unit_groups(t);
        let base = {
            let mut b = String::new();
            if multi_trace || t.name.is_none() {
                b.push_str(&format!("trace{}", t.index));
            }
            if let Some(name) = &t.name
                && !multi_trace
            {
                b.push_str(&link_name(name));
            }
            if t.sweep_count > 1 || opts.sweep.is_some() {
                b.push_str(&format!("_sweep{sweep}"));
            }
            b
        };
        let rate = if t.sample_rate_hz > 0.0 {
            t.sample_rate_hz
        } else {
            return Err(Error::unsupported(
                NWB_FORMAT_ID,
                format!("trace {} without a sample rate", t.index),
                "NWB TimeSeries here are regularly sampled (starting_time + rate); export this trace with `--to csv` or `--to parquet`.",
            ));
        };
        let start = t.start_s.unwrap_or(0.0) + *a as f64 / rate;
        for (unit, idx) in &groups {
            let mut name = if groups.len() > 1 {
                format!("{base}_{}", link_name(unit.as_deref().unwrap_or("no_unit")))
            } else {
                base.clone()
            };
            let stem = name.clone();
            let mut k = 1;
            while !used.insert(name.clone()) {
                name = format!("{stem}_{k}");
                k += 1;
            }
            let mut data = Vec::with_capacity(n as usize * idx.len());
            data.extend(
                (0..n as usize)
                    .flat_map(|r| idx.iter().map(|&c| cols[c as usize][r]).collect::<Vec<_>>()),
            );
            let chans: Vec<String> = idx
                .iter()
                .map(|&c| {
                    let ch = &t.channels[c as usize];
                    match &ch.unit {
                        Some(u) if !u.is_empty() => format!("{} ({u})", ch.name),
                        _ => ch.name.clone(),
                    }
                })
                .collect();
            let synthesized = t.extra.get("synthesized") == Some(&serde_json::Value::Bool(true));
            let description = format!(
                "{} trace {}{}, sweep {sweep}: {} channel(s), columns of data in order: {}{}",
                info.format.name,
                t.index,
                t.name
                    .as_deref()
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default(),
                idx.len(),
                chans.join(", "),
                if synthesized {
                    "; synthesized by the reader from the file's protocol (not recorded samples)"
                } else {
                    ""
                }
            );
            let comments = serde_json::json!({
                "source": source_name,
                "source_format": info.format.id,
                "trace": t.index,
                "sweep": sweep,
                "first_sample": a,
                "channels": idx.iter().map(|&c| &t.channels[c as usize]).collect::<Vec<_>>(),
            })
            .to_string();
            series.push(Planned {
                digest: digest(&data),
                report: NwbSeriesReport {
                    name,
                    trace: t.index,
                    sweep: *sweep,
                    samples: n,
                    channels: idx.len() as u32,
                    channel_indices: idx.clone(),
                    unit: unit.clone(),
                    rate_hz: rate,
                    starting_time_s: start,
                },
                description,
                comments,
                data,
            });
        }
    }

    // ------------------------------------------------------------ session fields
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let now_iso = with_offset(&openreadout_core::time::unix_to_iso8601(
        now.as_secs() as i64,
        now.subsec_millis(),
    ))
    .0;
    let started = experiment
        .acquisition
        .as_ref()
        .and_then(|a| a.started_at.clone())
        .filter(|s| openreadout_core::time::iso8601_to_unix(s).is_some());
    let session_start = if let Some(t) = started.or_else(|| acquired_at(&info)) {
        let (s, had_zone) = with_offset(&t);
        if !had_zone {
            notes.push(
                "the file records its start time without a time zone; session_start_time states +00:00".into(),
            );
        }
        s
    } else {
        let m = std::fs::metadata(input)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
        notes.push(match m {
            Some(_) => "the file records no start time; session_start_time is the source file's modification time".into(),
            None => "the file records no start time; session_start_time is the time of the export".into(),
        });
        let d = m.unwrap_or(now);
        with_offset(&openreadout_core::time::unix_to_iso8601(
            d.as_secs() as i64,
            d.subsec_millis(),
        ))
        .0
    };
    let seed = format!(
        "{}|{}|{}|{}",
        input.display(),
        info.size_bytes,
        now.as_nanos(),
        std::process::id()
    );
    let identifier = uuid(seed.as_bytes());
    let session_description = format!(
        "{} recording {} ({} trace(s)), exported by openreadout {}",
        info.format.name,
        source_name,
        traces.len(),
        env!("CARGO_PKG_VERSION")
    );

    // ------------------------------------------------------------ build the HDF5 file
    let mut fb = FileBuilder::new();
    fb.set_attr("neurodata_type", text("NWBFile"));
    fb.set_attr("namespace", text("core"));
    fb.set_attr("nwb_version", text(NWB_VERSION));
    fb.set_attr("object_id", text(&uuid(format!("{seed}|root").as_bytes())));
    let scalar = |fb: &mut FileBuilder, name: &str, v: &str| {
        fb.create_dataset(name)
            .with_vlen_strings(&[v])
            .with_shape(&[]);
    };
    scalar(&mut fb, "identifier", &identifier);
    scalar(&mut fb, "session_description", &session_description);
    scalar(&mut fb, "session_start_time", &session_start);
    scalar(&mut fb, "timestamps_reference_time", &session_start);
    fb.create_dataset("file_create_date")
        .with_vlen_strings(&[now_iso.as_str()]);
    let mut acq = fb.create_group("acquisition");
    for s in &mut series {
        let mut g = acq.create_group(&s.report.name);
        g.set_attr("neurodata_type", text("TimeSeries"));
        g.set_attr("namespace", text("core"));
        g.set_attr(
            "object_id",
            text(&uuid(format!("{seed}|{}", s.report.name).as_bytes())),
        );
        g.set_attr("description", text(&s.description));
        g.set_attr("comments", text(&s.comments));
        let cols = u64::from(s.report.channels);
        let rows = s.report.samples;
        let data = std::mem::take(&mut s.data);
        let d = g.create_dataset("data");
        d.with_f64_data(&data);
        drop(data);
        if cols > 1 {
            d.with_shape(&[rows, cols]);
        }
        if rows > 0 {
            // Chunk dimensions stay below 2^16: hdf5-pure 0.47 encodes larger ones in 4 bytes
            // where the HDF5 library expects the minimal width (3), and h5py/pynwb then refuse
            // the dataset ("stored chunk dimension encoding length does not match").
            let chunk_rows = (CHUNK_BYTES / (8 * cols)).clamp(1, rows).min(MAX_CHUNK_DIM);
            if cols > 1 {
                d.with_chunks(&[chunk_rows, cols.min(MAX_CHUNK_DIM)]);
            } else {
                d.with_chunks(&[chunk_rows]);
            }
            d.with_deflate(4);
        }
        d.set_attr("unit", text(s.report.unit.as_deref().unwrap_or("n/a")));
        d.set_attr("conversion", AttrValue::F64(1.0));
        d.set_attr("offset", AttrValue::F64(0.0));
        d.set_attr("resolution", AttrValue::F64(-1.0));
        d.set_attr("continuity", text("continuous"));
        g.create_dataset("starting_time")
            .with_f64_data(&[s.report.starting_time_s])
            .with_shape(&[])
            .set_attr("rate", AttrValue::F64(s.report.rate_hz))
            .set_attr("unit", text("seconds"));
        acq.add_group(g.finish());
    }
    fb.add_group(acq.finish());
    for name in ["analysis", "processing"] {
        let g = fb.create_group(name);
        fb.add_group(g.finish());
    }
    let mut stim = fb.create_group("stimulus");
    for name in ["presentation", "templates"] {
        let g = stim.create_group(name);
        stim.add_group(g.finish());
    }
    fb.add_group(stim.finish());
    let mut general = fb.create_group("general");
    let operator = experiment
        .acquisition
        .as_ref()
        .and_then(|a| a.operator.clone())
        .filter(|s| !s.trim().is_empty());
    if let Some(e) = operator.or_else(|| experimenter(&info)) {
        general
            .create_dataset("experimenter")
            .with_vlen_strings(&[e.as_str()]);
    }
    let method = experiment.method.as_ref().and_then(|m| {
        let parts: Vec<&str> = [
            m.name.as_deref(),
            m.technique.as_ref().map(|t| t.label.as_str()),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!parts.is_empty()).then(|| parts.join(" — "))
    });
    if let Some(m) = method {
        general
            .create_dataset("experiment_description")
            .with_vlen_strings(&[format!("Method: {m}").as_str()])
            .with_shape(&[]);
    }
    if let Some(sid) = experiment
        .sample
        .as_ref()
        .and_then(|s| s.id.clone().or(s.name.clone()))
    {
        general
            .create_dataset("session_id")
            .with_vlen_strings(&[sid.as_str()])
            .with_shape(&[]);
    }
    let mut named = info.clone();
    // the source by its file name, not the local path it was read from
    named.path.clone_from(&source_name);
    let info_json = serde_json::to_string(&openreadout_core::experiment::InfoOutput {
        file: named,
        experiment: (!experiment.is_empty()).then(|| experiment.clone()),
        acquisition: None,
        plate: None,
        images_total: None,
        assurance: None,
    })
    .unwrap_or_default();
    general
        .create_dataset("notes")
        .with_vlen_strings(&[format!(
            "Exported by openreadout {} from {} ({}). The source's normalized metadata (`openreadout info --json`) follows.\n{info_json}",
            env!("CARGO_PKG_VERSION"),
            source_name,
            info.format.name
        )
        .as_str()])
        .with_shape(&[]);
    general
        .create_dataset("source_script")
        .with_vlen_strings(&[format!("openreadout {}", env!("CARGO_PKG_VERSION")).as_str()])
        .with_shape(&[])
        .set_attr("file_name", text("openreadout"));
    fb.add_group(general.finish());

    // ------------------------------------------------------------ write, verify, rename
    let name = output
        .file_name()
        .map_or_else(|| "export.nwb".into(), |n| n.to_string_lossy().to_string());
    let tmp = output.with_file_name(format!(".{name}.partial-{}", std::process::id()));
    let result = (|| -> Result<u64> {
        fb.write(&tmp).map_err(|e| match e {
            hdf5_pure::Error::Io(io) => Error::io(&tmp, io),
            other => Error::Other(format!("writing NWB (HDF5): {other}")),
        })?;
        verify(&tmp, &series)?;
        Ok(std::fs::metadata(&tmp)
            .map_err(|e| Error::io(&tmp, e))?
            .len())
    })();
    let bytes = match result {
        Ok(b) => b,
        Err(e) => {
            if std::env::var_os("OPENREADOUT_KEEP_PARTIAL").is_none() {
                std::fs::remove_file(&tmp).ok();
            }
            return Err(e);
        }
    };
    std::fs::rename(&tmp, output).map_err(|e| Error::io(output, e))?;
    let samples_written = series
        .iter()
        .map(|s| s.report.samples * u64::from(s.report.channels))
        .sum();
    Ok(NwbExportReport {
        input: input.display().to_string(),
        output: output.display().to_string(),
        format: NWB_FORMAT_ID.into(),
        nwb_version: NWB_VERSION.into(),
        identifier,
        session_start_time: session_start,
        series: series.into_iter().map(|s| s.report).collect(),
        samples_written,
        bytes_written: bytes,
        verified: true,
        notes,
    })
}

/// Re-open `path` with the NWB reader: every series present with its shape, rate and unit, and
/// every sample equal (xxh3 over the float64 values, row by row).
fn verify(path: &Path, series: &[Planned]) -> Result<()> {
    let bad = |m: String| Error::Other(format!("NWB read-back: {m}"));
    let mut ds = NwbDataset::open(path)?;
    let info = ds.info()?;
    if info.traces.len() != series.len() {
        return Err(bad(format!(
            "{} TimeSeries read, {} written",
            info.traces.len(),
            series.len()
        )));
    }
    for s in series {
        let rep = &s.report;
        let got = info
            .traces
            .iter()
            .find(|t| t.name.as_deref() == Some(rep.name.as_str()))
            .ok_or_else(|| bad(format!("acquisition/{} not found", rep.name)))?
            .clone();
        if got.sample_count != rep.samples
            || got.channels.len() != rep.channels as usize
            || got.sample_rate_hz.to_bits() != rep.rate_hz.to_bits()
        {
            return Err(bad(format!(
                "acquisition/{}: {} samples x {} channels at {} Hz; wrote {} x {} at {} Hz",
                rep.name,
                got.sample_count,
                got.channels.len(),
                got.sample_rate_hz,
                rep.samples,
                rep.channels,
                rep.rate_hz
            )));
        }
        if got.channels.first().and_then(|c| c.unit.clone()) != rep.unit {
            return Err(bad(format!("acquisition/{}: unit differs", rep.name)));
        }
        let mut hasher = Xxh3::new();
        let mut pos = 0;
        while pos < rep.samples {
            let take = CHUNK.min(rep.samples - pos);
            let tr = ds.read_trace(got.index, 0, pos, take)?;
            for i in 0..take as usize {
                for c in &tr.channels {
                    let v = c[i];
                    let bits = if v.is_nan() {
                        f64::NAN.to_bits()
                    } else {
                        v.to_bits()
                    };
                    hasher.update(&bits.to_le_bytes());
                }
            }
            pos += take;
        }
        if hasher.digest128() != s.digest {
            return Err(bad(format!(
                "acquisition/{}: samples differ from what was written",
                rep.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_and_names() {
        let u = uuid(b"x");
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
        assert_eq!(link_name("IN 0/raw"), "IN 0_raw");
        assert_eq!(link_name("  "), "series");
        assert_eq!(
            with_offset("2020-01-02T03:04:05Z").0,
            "2020-01-02T03:04:05+00:00"
        );
        assert_eq!(
            with_offset("2020-01-02T03:04:05.5-05:00"),
            ("2020-01-02T03:04:05.5-05:00".into(), true)
        );
        assert_eq!(
            with_offset("2020-01-02T03:04:05"),
            ("2020-01-02T03:04:05+00:00".into(), false)
        );
    }
}
