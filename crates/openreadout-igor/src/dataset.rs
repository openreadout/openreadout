//! `Dataset` implementation for Igor binary waves and packed experiments: every numeric wave
//! becomes a trace (one dimension, or two with labelled columns) or an image (two to four
//! dimensions); text waves, variables, notes and the experiment's text records are vendor
//! metadata.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::Endian;
use openreadout_core::model::{
    ChannelInfo, CheckReport, FileInfo, Finding, ImageInfo, LsEntry, PhysicalSize,
    SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::trace::seconds_per_unit;
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::packed::{self, Packed, RecordKind, Variable, Variables};
use crate::wave::{self, Problem, Wave, WaveKind, igor_text, igor_time};
use crate::{FORMAT_ID, IgorReader};

/// Values of a trace decoded per `read_trace` call at most (per channel).
pub const MAX_TRACE_READ: u64 = 1 << 24;
/// Bytes of a plane read at most.
pub const MAX_PLANE_BYTES: u64 = 2 << 30;
/// Text-wave elements listed in the vendor metadata at most per wave.
pub const MAX_TEXT_ELEMENTS: usize = 1000;
/// Waves read from one file at most.
pub const MAX_WAVES: usize = 1_000_000;
/// Columns of a labelled two-dimensional wave read as trace channels at most.
pub const MAX_TRACE_COLUMNS: u64 = 4096;

/// What kind of file this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// One binary wave (`.ibw`).
    BinaryWave,
    /// A packed experiment (`.pxp`, `.pxt`).
    Packed,
}

/// What a wave becomes in the normalized model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Trace `n`.
    Trace(u32),
    /// Image `n`.
    Image(u32),
    /// A text wave (vendor metadata only).
    Text,
    /// A wave without points, or whose data the file does not hold.
    Empty,
    /// A wave whose type or sizes make its data unreadable.
    Unreadable,
}

/// How the layers (dimension 2) of an image wave are exposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerAxis {
    /// Labelled layers are channels.
    Channel,
    /// Unlabelled layers are Z planes.
    Z,
}

/// One wave of the file and where it sits.
#[derive(Debug, Clone)]
pub struct WaveEntry {
    /// Full name: the wave name in an `.ibw`, `root:folder:name` in a packed experiment.
    pub path: String,
    /// Index of its record in a packed experiment.
    pub record: Option<usize>,
    /// The parsed wave.
    pub wave: Wave,
    /// What it becomes.
    pub role: Role,
    /// For images: how the layers are exposed.
    pub layers: LayerAxis,
}

impl WaveEntry {
    fn rows(&self) -> u64 {
        self.wave.dims[0].max(1)
    }
    fn columns(&self) -> u64 {
        if self.wave.rank() >= 2 {
            self.wave.dims[1]
        } else {
            1
        }
    }
    fn layer_count(&self) -> u64 {
        if self.wave.rank() >= 3 {
            self.wave.dims[2]
        } else {
            1
        }
    }
    fn chunk_count(&self) -> u64 {
        if self.wave.rank() >= 4 {
            self.wave.dims[3]
        } else {
            1
        }
    }
    fn complex(&self) -> bool {
        matches!(
            self.wave.kind,
            Some(WaveKind::Numeric { complex: true, .. })
        )
    }
}

/// The parsed file.
#[derive(Debug, Clone)]
pub struct IgorFile {
    /// Binary wave or packed experiment.
    pub container: Container,
    /// File length.
    pub file_len: u64,
    /// Byte order.
    pub endian: Endian,
    /// Every wave, in file order.
    pub waves: Vec<WaveEntry>,
    /// The record chain (packed experiments).
    pub packed: Option<Packed>,
    /// Decoded variables records (packed experiments).
    pub variables: Vec<Variables>,
    /// Text records: (kind, text cut to `packed::MAX_RECORD_TEXT`, full length).
    pub texts: Vec<(RecordKind, String, u64)>,
    /// `IGORVersion` from the recreation procedures' first line.
    pub igor_version: Option<String>,
    /// `Platform` from the same line.
    pub platform: Option<String>,
    /// Bytes after the last section of a binary wave.
    pub trailing_bytes: u64,
    /// Problems found while parsing (the waves' own are in their `problems`).
    pub problems: Vec<Problem>,
}

fn classify(w: &Wave) -> (Role, LayerAxis) {
    let layers = if w.element_labels(2).is_some() {
        LayerAxis::Channel
    } else {
        LayerAxis::Z
    };
    let Some(kind) = w.kind else {
        return (Role::Unreadable, layers);
    };
    if kind == WaveKind::Text {
        return (Role::Text, layers);
    }
    if !w.readable() {
        return (Role::Unreadable, layers);
    }
    if w.points == 0 || w.data_omitted() {
        return (Role::Empty, layers);
    }
    let rank = w.rank();
    let labelled_columns =
        rank == 2 && w.element_labels(1).is_some() && w.dims[1] <= MAX_TRACE_COLUMNS;
    if rank <= 1 || labelled_columns {
        // complex integers have no trace dtype problem: values become f64
        (Role::Trace(0), layers)
    } else if kind.pixel_type().is_some() {
        (Role::Image(0), layers)
    } else {
        (Role::Unreadable, layers)
    }
}

/// Parse a binary wave file or a packed experiment.
pub fn parse_file(f: &SourceFile, file_len: u64, path: &Path) -> Result<IgorFile> {
    let mut head = vec![0u8; usize::try_from(file_len.min(64 * 1024)).unwrap_or(0)];
    f.read_exact_at(0, &mut head)
        .map_err(|e| Error::io(path, e))?;
    let ibw_ext = openreadout_core::reader::has_extension(path, &["ibw", "bwav"]);
    let as_wave =
        crate::looks_like_wave(&head) && (ibw_ext || !packed::looks_like_packed(&head, file_len));
    let mut out = IgorFile {
        container: if as_wave {
            Container::BinaryWave
        } else {
            Container::Packed
        },
        file_len,
        endian: Endian::Little,
        waves: Vec::new(),
        packed: None,
        variables: Vec::new(),
        texts: Vec::new(),
        igor_version: None,
        platform: None,
        trailing_bytes: 0,
        problems: Vec::new(),
    };
    if as_wave {
        let w = wave::parse_wave(f, 0, file_len)?;
        out.endian = w.endian;
        out.trailing_bytes = file_len.saturating_sub(w.end);
        let (role, layers) = classify(&w);
        out.waves.push(WaveEntry {
            path: w.name.clone(),
            record: None,
            wave: w,
            role,
            layers,
        });
    } else {
        let p = packed::walk(f, file_len)?;
        out.endian = p.endian;
        for (k, r) in p.records.iter().enumerate() {
            if r.superseded() {
                continue;
            }
            match r.kind {
                RecordKind::Wave => {
                    if out.waves.len() >= MAX_WAVES {
                        out.problems.push(Problem {
                            code: "too_many_waves",
                            detail: format!("more than {MAX_WAVES} waves; the rest are not read"),
                            offset: Some(r.offset),
                            fatal: false,
                        });
                        break;
                    }
                    let end = r.data_offset() + r.len;
                    match wave::parse_wave(f, r.data_offset(), end) {
                        Ok(w) => {
                            let (role, layers) = classify(&w);
                            out.waves.push(WaveEntry {
                                path: format!("{}:{}", r.folder, w.name),
                                record: Some(k),
                                wave: w,
                                role,
                                layers,
                            });
                        }
                        Err(e) => out.problems.push(Problem {
                            code: "bad_wave",
                            detail: format!("wave record at {}: {e}", r.offset),
                            offset: Some(r.offset),
                            fatal: true,
                        }),
                    }
                }
                RecordKind::Variables => {
                    let n = r.len.min(packed::MAX_VARIABLES_BYTES);
                    let mut b = vec![0u8; usize::try_from(n).unwrap_or(0)];
                    f.read_exact_at(r.data_offset(), &mut b)
                        .map_err(|e| Error::io(path, e))?;
                    out.variables
                        .push(packed::parse_variables(&b, p.endian, &r.folder));
                }
                RecordKind::History | RecordKind::Recreation | RecordKind::Procedures => {
                    let n = r.len.min(packed::MAX_RECORD_TEXT);
                    let mut b = vec![0u8; usize::try_from(n).unwrap_or(0)];
                    f.read_exact_at(r.data_offset(), &mut b)
                        .map_err(|e| Error::io(path, e))?;
                    let b = b.strip_prefix(b"\xef\xbb\xbf".as_slice()).unwrap_or(&b);
                    let text = igor_text(b);
                    if r.kind == RecordKind::Recreation && out.igor_version.is_none() {
                        let first = text.lines().next().unwrap_or_default();
                        for part in first.trim_start_matches('/').split(',') {
                            if let Some((k, v)) = part.split_once('=') {
                                match k.trim() {
                                    "IGORVersion" => out.igor_version = Some(v.trim().to_string()),
                                    "Platform" => out.platform = Some(v.trim().to_string()),
                                    _ => {}
                                }
                            }
                        }
                    }
                    out.texts.push((r.kind, text, r.len));
                }
                _ => {}
            }
        }
        out.packed = Some(p);
    }
    // number traces and images in file order
    let (mut nt, mut ni) = (0u32, 0u32);
    for w in &mut out.waves {
        match w.role {
            Role::Trace(_) => {
                w.role = Role::Trace(nt);
                nt += 1;
            }
            Role::Image(_) => {
                w.role = Role::Image(ni);
                ni += 1;
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Factor from a length unit to micrometres.
fn micrometres_per(unit: &str) -> Option<f64> {
    match unit.trim() {
        "m" => Some(1e6),
        "mm" => Some(1e3),
        "um" | "µm" | "μm" => Some(1.0),
        "nm" => Some(1e-3),
        "pm" => Some(1e-6),
        "Å" | "A" => Some(1e-4),
        _ => None,
    }
}

fn num(v: f64) -> Value {
    if v.is_finite() { json!(v) } else { Value::Null }
}

fn opt(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// `Key: value` / `Key=value` lines of a wave note (Asylum Research and other instrument
/// software store their settings there), when most lines have that shape.
pub fn note_fields(note: &str) -> Option<BTreeMap<String, String>> {
    let lines: Vec<&str> = note
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() < 2 {
        return None;
    }
    let mut out = BTreeMap::new();
    for l in &lines {
        let split = l.split_once(':').or_else(|| l.split_once('='));
        if let Some((k, v)) = split {
            let k = k.trim();
            if !k.is_empty() && k.len() <= 64 {
                out.entry(k.to_string())
                    .or_insert_with(|| v.trim().to_string());
            }
        }
    }
    (out.len() * 2 >= lines.len()).then_some(out)
}

/// An opened Igor file.
#[derive(Debug)]
pub struct IgorDataset {
    path: PathBuf,
    fs: Fs,
    handle: Option<SourceFile>,
    file: IgorFile,
}

impl IgorDataset {
    /// Open a local file.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let path = input.path().to_path_buf();
        let fs = input.fs().clone();
        let f = fs.open(&path).map_err(|e| Error::io(&path, e))?;
        let file_len = f.size().map_err(|e| Error::io(&path, e))?;
        let file = parse_file(&f, file_len, &path)?;
        Ok(IgorDataset {
            path,
            fs,
            handle: Some(f),
            file,
        })
    }

    /// The parsed file.
    pub fn igor(&self) -> &IgorFile {
        &self.file
    }

    fn handle(&mut self) -> Result<&SourceFile> {
        if self.handle.is_none() {
            self.handle = Some(
                self.fs
                    .open(&self.path)
                    .map_err(|e| Error::io(&self.path, e))?,
            );
        }
        Ok(self.handle.as_ref().expect("just opened"))
    }

    fn trace_wave(&self, index: u32) -> Result<&WaveEntry> {
        self.file
            .waves
            .iter()
            .find(|w| w.role == Role::Trace(index))
            .ok_or_else(|| Error::Usage(format!("trace index {index} out of range")))
    }

    fn image_wave(&self, index: u32) -> Result<&WaveEntry> {
        self.file
            .waves
            .iter()
            .find(|w| w.role == Role::Image(index))
            .ok_or_else(|| Error::Usage(format!("image index {index} out of range")))
    }

    fn writer(&self) -> Option<String> {
        self.file
            .igor_version
            .as_ref()
            .map(|v| format!("Igor Pro {v}"))
    }

    fn wave_extra(&self, w: &WaveEntry) -> BTreeMap<String, Value> {
        let wv = &w.wave;
        let mut e = BTreeMap::new();
        e.insert("wave".into(), json!(w.path));
        e.insert("wave_version".into(), json!(wv.version));
        if let Some(k) = wv.kind {
            e.insert("wave_type".into(), json!(k.label()));
        }
        e.insert(
            "dimensions".into(),
            json!(wv.dims[..wv.rank().max(1)].to_vec()),
        );
        e.insert(
            "byte_order".into(),
            json!(match wv.endian {
                Endian::Little => "little-endian",
                Endian::Big => "big-endian",
            }),
        );
        if let Some(t) = igor_time(wv.created) {
            e.insert("wave_created".into(), json!(t));
        }
        if let Some(t) = igor_time(wv.modified) {
            e.insert("wave_modified".into(), json!(t));
        }
        if let Some((top, bottom)) = wv.full_scale {
            e.insert("full_scale".into(), json!([num(bottom), num(top)]));
        }
        if let Some(f) = &wv.formula {
            e.insert("formula".into(), json!(f));
        }
        if !wv.note.is_empty() {
            e.insert("note_bytes".into(), json!(wv.note.len()));
        }
        if let Some(v) = &self.file.igor_version {
            e.insert("application".into(), json!("Igor Pro"));
            e.insert("application_version".into(), json!(v));
        }
        e
    }

    fn trace_info(&self, w: &WaveEntry, index: u32) -> TraceInfo {
        let wv = &w.wave;
        let kind = wv.kind.unwrap_or(WaveKind::Text);
        let mut extra = self.wave_extra(w);
        let step = wv.steps[0];
        let offset = wv.offsets[0];
        let unit = wv.dim_units[0].as_str();
        let rows = w.rows();
        let (rate, start) = match seconds_per_unit(unit) {
            Some(f) if step.is_finite() && step > 0.0 => (1.0 / (step * f), Some(offset * f)),
            _ => {
                let last = offset + step * rows.saturating_sub(1) as f64;
                extra.insert(
                    "axis".into(),
                    json!({"quantity": wv.dimension_label(0).unwrap_or("x"),
                           "unit": opt(unit), "first": num(offset), "step": num(step),
                           "last": num(last), "size": rows}),
                );
                (0.0, None)
            }
        };
        let base_names: Vec<String> = if wv.rank() == 2 {
            let labels = wv.element_labels(1).unwrap_or_default();
            (0..w.columns())
                .map(|c| {
                    labels
                        .get(c as usize)
                        .filter(|s| !s.is_empty())
                        .cloned()
                        .unwrap_or_else(|| format!("column {c}"))
                })
                .collect()
        } else {
            vec![wv.name.clone()]
        };
        let mut channels = Vec::new();
        for n in &base_names {
            let parts: &[&str] = if w.complex() {
                &[" (real)", " (imaginary)"]
            } else {
                &[""]
            };
            for p in parts {
                channels.push(SignalChannelInfo {
                    index: channels.len() as u32,
                    name: format!("{n}{p}"),
                    unit: opt(&wv.data_units),
                    dtype: kind.dtype().into(),
                    scale: 1.0,
                    offset: 0.0,
                    extra: BTreeMap::new(),
                });
            }
        }
        TraceInfo {
            index,
            name: Some(w.path.clone()),
            sample_rate_hz: rate,
            sample_count: rows,
            sweep_count: 1,
            channels,
            start_s: start,
            extra,
        }
    }

    fn image_info(&self, w: &WaveEntry, index: u32) -> ImageInfo {
        let wv = &w.wave;
        let pt = wv
            .kind
            .and_then(WaveKind::pixel_type)
            .unwrap_or(openreadout_core::pixel::PixelType::Double);
        let mut im = ImageInfo::new(
            index,
            u32::try_from(wv.dims[0]).unwrap_or(u32::MAX),
            u32::try_from(wv.dims[1]).unwrap_or(u32::MAX),
            pt,
        );
        im.name = Some(w.path.clone());
        let layers = u32::try_from(w.layer_count()).unwrap_or(u32::MAX);
        match w.layers {
            LayerAxis::Channel => im.size_c = layers,
            LayerAxis::Z => im.size_z = layers,
        }
        im.size_t = u32::try_from(w.chunk_count()).unwrap_or(u32::MAX);
        let size = |d: usize| -> Option<f64> {
            let f = micrometres_per(&wv.dim_units[d])?;
            Some((wv.steps[d] * f).abs())
        };
        im.physical_size = PhysicalSize::micrometres(
            size(0),
            size(1),
            if w.layers == LayerAxis::Z && wv.rank() >= 3 {
                size(2)
            } else {
                None
            },
        );
        if wv.rank() >= 4
            && let Some(f) = seconds_per_unit(&wv.dim_units[3])
        {
            let s = wv.steps[3] * f;
            if s.is_finite() && s > 0.0 {
                im.time_increment_s = Some(s);
            }
        }
        let labels = wv.element_labels(2).unwrap_or_default();
        im.channels = (0..im.size_c)
            .map(|c| ChannelInfo {
                index: c,
                name: labels
                    .get(c as usize)
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .or_else(|| (im.size_c == 1).then(|| wv.name.clone())),
                ..ChannelInfo::default()
            })
            .collect();
        let mut extra = self.wave_extra(w);
        if let Some(u) = opt(&wv.data_units) {
            extra.insert("data_units".into(), json!(u));
        }
        extra.insert(
            "axes".into(),
            json!(
                (0..wv.rank())
                    .map(
                        |d| json!({"label": wv.dimension_label(d), "unit": opt(&wv.dim_units[d]),
                                    "first": num(wv.offsets[d]), "step": num(wv.steps[d]),
                                    "size": wv.dims[d]})
                    )
                    .collect::<Vec<_>>()
            ),
        );
        extra.insert(
            "layers_as".into(),
            json!(match w.layers {
                LayerAxis::Channel => "channels",
                LayerAxis::Z => "z",
            }),
        );
        im.extra = extra;
        im.finish()
    }

    fn wave_vendor(&self, w: &WaveEntry, f: Option<&SourceFile>) -> Value {
        let wv = &w.wave;
        let rank = wv.rank().max(1);
        let mut v = json!({
            "path": w.path,
            "name": wv.name,
            "version": wv.version,
            "type": wv.wave_type,
            "type_name": wv.kind.map(WaveKind::label),
            "points": wv.points,
            "dimensions": wv.dims[..rank].to_vec(),
            "steps": wv.steps[..rank].iter().map(|x| num(*x)).collect::<Vec<_>>(),
            "offsets": wv.offsets[..rank].iter().map(|x| num(*x)).collect::<Vec<_>>(),
            "data_units": wv.data_units,
            "dimension_units": wv.dim_units[..rank].to_vec(),
            "created": igor_time(wv.created),
            "modified": igor_time(wv.modified),
            "data_offset": wv.data_offset,
            "data_bytes": wv.data_len,
            "checksum_ok": wv.checksum_ok,
        });
        if let Some(fm) = &wv.formula {
            v["formula"] = json!(fm);
        }
        if !wv.note.is_empty() {
            v["note"] = json!(wv.note.replace('\r', "\n"));
            if let Some(fields) = note_fields(&wv.note) {
                v["note_fields"] = json!(fields);
            }
        }
        if wv.labels.iter().any(|l| !l.is_empty()) {
            v["dimension_labels"] = json!(wv.labels[..rank].to_vec());
        }
        if w.role == Role::Text
            && let Some(f) = f
        {
            v["values"] = json!(wave::text_values(wv, f, MAX_TEXT_ELEMENTS).unwrap_or_default());
        }
        v
    }
}

fn variables_json(v: &Variables) -> Value {
    let user: Vec<Value> = v
        .user
        .iter()
        .map(|x| match x {
            Variable::Number {
                name,
                real,
                imaginary,
                number_type,
            } => {
                if *imaginary == 0.0 {
                    json!({"name": name, "value": num(*real), "number_type": number_type})
                } else {
                    json!({"name": name, "value": num(*real), "imaginary": num(*imaginary), "number_type": number_type})
                }
            }
            Variable::Text { name, value } => json!({"name": name, "text": value}),
        })
        .collect();
    let mut o = json!({"folder": v.folder, "version": v.version, "variables": user});
    if !v.system.is_empty() {
        o["system"] = json!(
            v.system
                .iter()
                .map(|x| num(f64::from(*x)))
                .collect::<Vec<_>>()
        );
    }
    if let Some(u) = &v.undecoded {
        o["undecoded"] = json!(u);
    }
    o
}

impl Dataset for IgorDataset {
    fn info(&self) -> Result<FileInfo> {
        let f = &self.file;
        let mut notes = Vec::new();
        let count = |r: fn(&Role) -> bool| f.waves.iter().filter(|w| r(&w.role)).count();
        let n_trace = count(|r| matches!(r, Role::Trace(_)));
        let n_image = count(|r| matches!(r, Role::Image(_)));
        let n_text = count(|r| *r == Role::Text);
        let n_empty = count(|r| *r == Role::Empty);
        let n_bad = count(|r| *r == Role::Unreadable);
        match f.container {
            Container::BinaryWave => {
                if let Some(w) = f.waves.first() {
                    notes.push(format!(
                        "Igor binary wave version {}: `{}`, {} {}",
                        w.wave.version,
                        w.wave.name,
                        w.wave
                            .kind
                            .map_or_else(|| "unknown type".into(), WaveKind::label),
                        match w.role {
                            Role::Trace(_) => "values as a trace",
                            Role::Image(_) => "values as an image",
                            Role::Text => "text (in the vendor metadata)",
                            Role::Empty => "without data",
                            Role::Unreadable => "that cannot be read (run `check`)",
                        }
                    ));
                }
                if f.trailing_bytes > 0 {
                    notes.push(format!(
                        "{} bytes after the wave's last section are not read (instrument software appends its own data there)",
                        f.trailing_bytes
                    ));
                }
            }
            Container::Packed => {
                let folders = f.packed.as_ref().map_or(0, |p| {
                    p.records
                        .iter()
                        .filter(|r| r.kind == RecordKind::FolderStart)
                        .count()
                });
                notes.push(format!(
                    "Igor packed experiment{}: {} waves in {} data folders ({n_trace} traces, {n_image} images, {n_text} text waves)",
                    self.writer().map(|w| format!(" written by {w}")).unwrap_or_default(),
                    f.waves.len(),
                    folders + 1,
                ));
            }
        }
        if n_empty > 0 {
            notes.push(format!("{n_empty} wave(s) without data are not exposed"));
        }
        if n_bad > 0 {
            notes.push(format!(
                "{n_bad} wave(s) cannot be read (unknown type or inconsistent sizes); run `check`"
            ));
        }
        let traces = f
            .waves
            .iter()
            .filter_map(|w| match w.role {
                Role::Trace(i) => Some(self.trace_info(w, i)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let images: Vec<ImageInfo> = f
            .waves
            .iter()
            .filter_map(|w| match w.role {
                Role::Image(i) => Some(self.image_info(w, i)),
                _ => None,
            })
            .collect();
        let plane_count = images.iter().map(|i| i.plane_count).sum();
        let format_version = Some(match f.container {
            Container::BinaryWave => {
                format!("ibw {}", f.waves.first().map_or(0, |w| w.wave.version))
            }
            Container::Packed => "pxp".to_string(),
        });
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: f.file_len,
            format: IgorReader.descriptor(),
            format_version,
            images,
            tables: Vec::new(),
            spectra: Vec::new(),
            traces,
            plane_count,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let f = &self.file;
        let handle = self.handle.as_ref();
        let waves: Vec<Value> = f
            .waves
            .iter()
            .map(|w| self.wave_vendor(w, handle))
            .collect();
        let mut v = json!({
            "container": match f.container {
                Container::BinaryWave => "binary wave",
                Container::Packed => "packed experiment",
            },
            "byte_order": match f.endian { Endian::Little => "little-endian", Endian::Big => "big-endian" },
            "waves": waves,
        });
        if f.container == Container::Packed {
            v["variables"] = json!(f.variables.iter().map(variables_json).collect::<Vec<_>>());
            v["igor_version"] = json!(f.igor_version);
            v["platform"] = json!(f.platform);
            let texts: Vec<Value> = f
                .texts
                .iter()
                .map(|(k, t, n)| json!({"record": k.name(), "bytes": n, "text": t.replace('\r', "\n")}))
                .collect();
            v["texts"] = json!(texts);
        }
        Ok(v)
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("format_version", Source::VendorImpl),
            ("traces[].name", Source::VendorImpl),
            ("traces[].sample_count", Source::VendorImpl),
            ("traces[].sample_rate_hz", Source::VendorImpl),
            ("traces[].start_s", Source::VendorImpl),
            ("traces[].channels[].name", Source::VendorImpl),
            ("traces[].channels[].unit", Source::VendorImpl),
            ("traces[].channels[].dtype", Source::VendorImpl),
            ("traces[].extra.axis", Source::VendorImpl),
            ("traces[].extra.wave_created", Source::VendorImpl),
            ("traces[].extra.wave_modified", Source::VendorImpl),
            ("images[].name", Source::VendorImpl),
            ("images[].size_x", Source::VendorImpl),
            ("images[].size_y", Source::VendorImpl),
            ("images[].size_z", Source::Inferred),
            ("images[].size_c", Source::Inferred),
            ("images[].size_t", Source::Inferred),
            ("images[].pixel_type", Source::VendorImpl),
            ("images[].physical_size", Source::VendorImpl),
            ("images[].time_increment_s", Source::VendorImpl),
            ("images[].channels[].name", Source::VendorImpl),
            ("images[].extra.axes", Source::VendorImpl),
            ("images[].extra.wave_created", Source::VendorImpl),
            ("images[].extra.wave_modified", Source::VendorImpl),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let f = &self.file;
        let mut out = Vec::new();
        let wave_entry = |w: &WaveEntry| LsEntry {
            kind: "wave".into(),
            name: w.path.clone(),
            offset: Some(w.wave.start),
            size: Some(w.wave.end - w.wave.start),
            image: match w.role {
                Role::Image(i) => Some(i),
                _ => None,
            },
            details: json!({"version": w.wave.version, "type": w.wave.kind.map(WaveKind::label),
                            "dimensions": w.wave.dims[..w.wave.rank().max(1)].to_vec(),
                            "data_offset": w.wave.data_offset, "data_bytes": w.wave.data_len,
                            "role": match w.role { Role::Trace(i) => format!("trace {i}"), Role::Image(i) => format!("image {i}"),
                                                   Role::Text => "text".into(), Role::Empty => "empty".into(), Role::Unreadable => "unreadable".into() }}),
        };
        match &f.packed {
            None => {
                out.extend(f.waves.iter().map(wave_entry));
                if f.trailing_bytes > 0 {
                    out.push(LsEntry {
                        kind: "trailing".into(),
                        name: "bytes after the last section".into(),
                        offset: Some(f.file_len - f.trailing_bytes),
                        size: Some(f.trailing_bytes),
                        image: None,
                        details: Value::Null,
                    });
                }
            }
            Some(p) => {
                for (k, r) in p.records.iter().enumerate() {
                    if r.kind == RecordKind::Wave
                        && let Some(w) = f.waves.iter().find(|w| w.record == Some(k))
                    {
                        out.push(wave_entry(w));
                        continue;
                    }
                    out.push(LsEntry {
                        kind: "record".into(),
                        name: format!(
                            "{}{}",
                            r.kind.name(),
                            if r.superseded() { " (superseded)" } else { "" }
                        ),
                        offset: Some(r.offset),
                        size: Some(packed::RECORD_HEADER + r.len),
                        image: None,
                        details: json!({"code": r.code, "version": r.version, "folder": r.folder}),
                    });
                }
            }
        }
        Ok(out)
    }

    fn read_plane(&mut self, image: u32, index: PlaneIndex) -> Result<Plane> {
        let w = self.image_wave(image)?.clone();
        let wv = &w.wave;
        let kind = wv
            .kind
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "wave type unknown"))?;
        let pt = kind.pixel_type().ok_or_else(|| {
            Error::unsupported(
                FORMAT_ID,
                format!("{} images", kind.label()),
                "Complex integer waves have no pixel type; export the wave from Igor as real and imaginary parts.",
            )
        })?;
        let layers = w.layer_count();
        let chunks = w.chunk_count();
        let (layer, other) = match w.layers {
            LayerAxis::Channel => (u64::from(index.c), u64::from(index.z)),
            LayerAxis::Z => (u64::from(index.z), u64::from(index.c)),
        };
        if layer >= layers || other != 0 || u64::from(index.t) >= chunks {
            return Err(Error::Usage(format!(
                "plane c={} z={} t={} out of range for image {image}",
                index.c, index.z, index.t
            )));
        }
        let plane_elems = wv.dims[0]
            .checked_mul(wv.dims[1])
            .ok_or_else(|| Error::corrupt(FORMAT_ID, "plane size overflows"))?;
        let bytes = plane_elems.saturating_mul(kind.element_bytes());
        if bytes > MAX_PLANE_BYTES {
            return Err(Error::unsupported(
                FORMAT_ID,
                format!("planes of {bytes} bytes"),
                "The plane is larger than this reader decodes at once.",
            ));
        }
        let first = (u64::from(index.t) * layers + layer) * plane_elems;
        let f = self.handle()?;
        let data = wv.read_elements(f, first, plane_elems)?;
        Ok(Plane {
            width: u32::try_from(wv.dims[0]).unwrap_or(u32::MAX),
            height: u32::try_from(wv.dims[1]).unwrap_or(u32::MAX),
            pixel_type: pt,
            samples_per_pixel: 1,
            data,
        })
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let w = self.trace_wave(index)?.clone();
        if sweep != 0 {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range (Igor waves have one sweep)"
            )));
        }
        let rows = w.rows();
        if first > rows {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({rows} samples)"
            )));
        }
        let count = max.min(rows - first).min(MAX_TRACE_READ);
        let kind = w.wave.kind.unwrap_or(WaveKind::Text);
        let f = self.handle()?;
        let mut channels = Vec::new();
        for c in 0..w.columns() {
            let b = w.wave.read_elements(f, c * rows + first, count)?;
            let v = wave::values_f64(kind, &b);
            if w.complex() {
                channels.push(v.iter().step_by(2).copied().collect());
                channels.push(v.iter().skip(1).step_by(2).copied().collect());
            } else {
                channels.push(v);
            }
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let f = &self.file;
        let mut r = CheckReport::new(self.path.display().to_string(), FORMAT_ID);
        r.performed("binary-wave headers: version, checksum (16-bit sum of the headers is zero), wave type, point count against the dimension sizes and the data size, every optional section inside the wave's bytes");
        if f.container == Container::Packed {
            r.performed("packed experiment: every record header chained to the end of the file, data folders opened and closed, variables records decoded");
        }
        let push = |r: &mut CheckReport, p: &Problem, wave: Option<&str>| {
            let msg = match wave {
                Some(w) => format!("wave `{w}`: {}", p.detail),
                None => p.detail.clone(),
            };
            let mut x = if p.fatal {
                Finding::error(p.code, msg)
            } else {
                Finding::warning(p.code, msg)
            };
            if let Some(o) = p.offset {
                x = x.at(o);
            }
            r.push(x);
        };
        for p in &f.problems {
            push(&mut r, p, None);
        }
        if let Some(pk) = &f.packed {
            for p in &pk.problems {
                push(&mut r, p, None);
            }
            let superseded = pk.records.iter().filter(|x| x.superseded()).count();
            if superseded > 0 {
                r.push(Finding::info(
                    "superseded",
                    format!("{superseded} superseded record(s) skipped"),
                ));
            }
        }
        for w in &f.waves {
            for p in &w.wave.problems {
                push(&mut r, p, Some(&w.path));
            }
            if w.wave.data_omitted() {
                r.push(Finding::info(
                    "formula_without_data",
                    format!(
                        "wave `{}` is governed by a formula and was saved without its data",
                        w.path
                    ),
                ));
            }
        }
        for v in &f.variables {
            if let Some(u) = &v.undecoded {
                r.push(Finding::warning(
                    "variables",
                    format!("variables of `{}`: {u}", v.folder),
                ));
            }
        }
        if f.trailing_bytes > 0 {
            r.push(Finding::info(
                "trailing_bytes",
                format!(
                    "{} bytes after the wave's last section (instrument software appends data there)",
                    f.trailing_bytes
                ),
            ));
        }
        Ok(r)
    }
}
