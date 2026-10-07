//! `Dataset`: one `.bin` stream as one trace (one sweep), channels scaled per the `.meta`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{latin1, read_block};
use openreadout_core::model::{
    CheckReport, FileInfo, Finding, LsEntry, SignalChannelInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use crate::meta::{
    ChannelKind, Meta, SavedChannel, SiteMap, StreamKind, parse_meta, saved_channels, site_map,
};
use crate::{FORMAT_ID, SpikeGlxReader};

/// Largest `.meta` read.
pub const MAX_META_LEN: u64 = 16 << 20;
/// Most values (samples × channels) one `read_trace` returns: 64 Mi (512 MiB of f64).
pub const MAX_READ_VALUES: u64 = 1 << 26;

/// Read a `.meta` file, stopping one byte past [`MAX_META_LEN`] (so an oversized file is
/// detected without being loaded).
pub(crate) fn read_meta(fs: &Fs, path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut text = Vec::new();
    fs.open(path)?
        .take(MAX_META_LEN + 1)
        .read_to_end(&mut text)?;
    Ok(text)
}

/// The `.bin` and `.meta` of a stream, given either path.
pub fn stream_paths(path: &Path) -> (PathBuf, PathBuf) {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if ext.as_deref() == Some("meta") {
        (path.with_extension("bin"), path.to_path_buf())
    } else {
        (path.to_path_buf(), path.with_extension("meta"))
    }
}

/// Channels a site map covers: imec AP and LF, NI neural (MN).
fn is_neural(k: ChannelKind) -> bool {
    matches!(k, ChannelKind::Ap | ChannelKind::Lf | ChannelKind::Mn)
}

/// An opened SpikeGLX stream.
#[derive(Debug)]
pub struct SpikeGlxDataset {
    bin: PathBuf,
    meta_path: PathBuf,
    meta: Meta,
    kind: StreamKind,
    channels: Vec<SavedChannel>,
    /// Electrode sites of the saved neural channels, when the `.meta` maps them.
    sites: Option<SiteMap>,
    bin_len: u64,
    handle: Option<SourceFile>,
    /// Where the `.bin` and `.meta` are read from.
    fs: Fs,
}

impl SpikeGlxDataset {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`] (a local path, a buffer, a host source).
    pub(crate) fn open_input(input: &Input) -> Result<Self> {
        let (path, fs) = (input.path(), input.fs());
        let (bin, meta_path) = stream_paths(path);
        let text = read_meta(fs, &meta_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::unsupported(
                    FORMAT_ID,
                    "a SpikeGLX .bin without its .meta",
                    "SpikeGLX binary files are headerless: keep the .meta file with the same name next to the .bin.",
                )
            } else {
                Error::io(&meta_path, e)
            }
        })?;
        if text.len() as u64 > MAX_META_LEN {
            return Err(Error::corrupt(
                FORMAT_ID,
                "the .meta file is larger than 16 MiB",
            ));
        }
        let meta = parse_meta(&latin1(&text));
        let kind = match meta.get("typeThis") {
            Some("imec") => StreamKind::Imec,
            Some("nidq") => StreamKind::Nidq,
            Some("obx") => StreamKind::Obx,
            None if meta.get("imSampRate").is_some() => StreamKind::Imec,
            None if meta.get("niSampRate").is_some() => StreamKind::Nidq,
            other => {
                return Err(Error::unsupported(
                    FORMAT_ID,
                    format!("stream type {other:?}"),
                    "Only imec, nidq and obx streams are read.",
                ));
            }
        };
        let channels = saved_channels(&meta, kind)
            .map_err(|e| Error::corrupt(FORMAT_ID, format!("{}: {e}", meta_path.display())))?;
        let bin_len = fs.metadata(&bin).map_err(|e| Error::io(&bin, e))?.len();
        let n_neural = channels.iter().filter(|c| is_neural(c.kind)).count();
        let sites = site_map(&meta, n_neural);
        Ok(SpikeGlxDataset {
            bin,
            meta_path,
            meta,
            kind,
            channels,
            sites,
            bin_len,
            handle: None,
            fs: fs.clone(),
        })
    }

    /// The parsed `.meta`.
    pub fn meta(&self) -> &Meta {
        &self.meta
    }

    /// What the stream holds.
    pub fn stream_kind(&self) -> StreamKind {
        self.kind
    }

    fn frame(&self) -> u64 {
        2 * self.channels.len() as u64
    }

    fn samples(&self) -> u64 {
        self.bin_len / self.frame()
    }

    fn rate(&self) -> f64 {
        self.meta
            .number(match self.kind {
                StreamKind::Imec => "imSampRate",
                StreamKind::Nidq => "niSampRate",
                StreamKind::Obx => "obSampRate",
            })
            .unwrap_or(0.0)
    }

    fn stream_name(&self) -> String {
        let n = self
            .bin
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        // `<run>_g0_t0.imec0.ap.bin` → `imec0.ap`
        let parts: Vec<&str> = n.split('.').collect();
        if parts.len() >= 3 {
            parts[parts.len() - 3..parts.len() - 1].join(".")
        } else if parts.len() == 2 {
            parts[0].to_string()
        } else {
            self.kind.name().to_string()
        }
        .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
        .to_string()
    }
}

impl Dataset for SpikeGlxDataset {
    fn member_files(&self) -> Vec<PathBuf> {
        // Whichever of the pair was opened, the other one belongs to the same stream.
        [&self.bin, &self.meta_path]
            .into_iter()
            .filter(|p| self.fs.is_file(p))
            .cloned()
            .collect()
    }
    fn info(&self) -> Result<FileInfo> {
        let m = &self.meta;
        let rate = self.rate();
        let mut extra = BTreeMap::new();
        extra.insert("stream_kind".into(), json!(self.kind.name()));
        extra.insert(
            "meta_file".into(),
            json!(
                self.meta_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            ),
        );
        for (k, name) in [
            ("appVersion", "app_version"),
            ("fileCreateTime", "created_at"),
            ("imDatPrb_pn", "probe_part_number"),
            ("imDatPrb_sn", "probe_serial_number"),
            ("imDatHs_sn", "headstage_serial_number"),
            ("imDatBs_sn", "basestation_serial_number"),
            ("typeImEnabled", "imec_enabled"),
            ("fileName", "original_file_name"),
            ("snsSaveChanSubset", "saved_channel_subset"),
        ] {
            if let Some(v) = m.get(k).filter(|v| !v.is_empty()) {
                extra.insert(name.into(), json!(v));
            }
        }
        for (k, name) in [
            ("imDatPrb_type", "probe_type"),
            ("firstSample", "first_sample"),
            ("fileSizeBytes", "declared_size_bytes"),
            ("fileTimeSecs", "declared_duration_s"),
            ("imAiRangeMax", "range_max_v"),
            ("niAiRangeMax", "range_max_v"),
            ("obAiRangeMax", "range_max_v"),
            ("imMaxInt", "max_int"),
            ("niMaxInt", "max_int"),
        ] {
            if let Some(v) = m.number(k) {
                extra.insert(name.into(), json!(v));
            }
        }
        let first = m.number("firstSample");
        if let Some(g) = &self.sites {
            let mut geo = serde_json::Map::new();
            geo.insert("source".into(), json!(g.source));
            geo.insert("shanks".into(), json!(g.shanks));
            for (k, v) in [
                ("part_number", g.part_number.as_ref().map(|v| json!(v))),
                ("shank_pitch_um", g.shank_pitch_um.map(|v| json!(v))),
                ("shank_width_um", g.shank_width_um.map(|v| json!(v))),
                ("columns", g.columns.map(|v| json!(v))),
                ("rows", g.rows.map(|v| json!(v))),
            ] {
                if let Some(v) = v {
                    geo.insert(k.into(), v);
                }
            }
            extra.insert("probe_geometry".into(), Value::Object(geo));
        }
        let mut neural = 0usize;
        let channels: Vec<SignalChannelInfo> = self
            .channels
            .iter()
            .map(|c| {
                let mut e = BTreeMap::new();
                if is_neural(c.kind) {
                    if let Some(site) = self.sites.as_ref().and_then(|g| g.sites.get(neural)) {
                        e.insert("shank".into(), json!(site.shank));
                        for (k, v) in [("x_um", site.x_um), ("z_um", site.z_um)] {
                            if let Some(v) = v {
                                e.insert(k.into(), json!(v));
                            }
                        }
                        for (k, v) in [("col", site.col), ("row", site.row)] {
                            if let Some(v) = v {
                                e.insert(k.into(), json!(v));
                            }
                        }
                        e.insert("used".into(), json!(site.used));
                    }
                    neural += 1;
                }
                e.insert("kind".into(), json!(c.kind.prefix()));
                e.insert("acquired_index".into(), json!(c.acquired));
                if let Some(g) = c.gain {
                    e.insert("gain".into(), json!(g));
                }
                if c.kind.is_bits() {
                    e.insert("bits".into(), json!(true));
                }
                SignalChannelInfo {
                    index: c.index,
                    name: c.name.clone(),
                    unit: c.unit.map(str::to_string),
                    dtype: "int16".into(),
                    scale: c.scale,
                    offset: 0.0,
                    extra: e,
                }
            })
            .collect();
        let trace = TraceInfo {
            index: 0,
            name: Some(self.stream_name()),
            sample_rate_hz: rate,
            sample_count: self.samples(),
            sweep_count: 1,
            channels,
            start_s: first.filter(|_| rate > 0.0).map(|f| f / rate),
            extra,
        };
        let mut notes = vec!["one stream = one trace (a single sweep); imec AP/LF in µV, NI and OneBox analog in V, SY/XD status and digital words raw".to_string()];
        if let Some(d) = m.number("fileSizeBytes")
            && (d as u64) > self.bin_len
        {
            notes.push(format!(
                "the .bin holds {} of the {} bytes the .meta declares (truncated or a stub); run `check`",
                self.bin_len, d as u64
            ));
        }
        Ok(FileInfo {
            path: self.bin.display().to_string(),
            size_bytes: self.bin_len,
            format: SpikeGlxReader.descriptor(),
            format_version: m.get("appVersion").map(str::to_string),
            images: Vec::new(),
            tables: Vec::new(),
            spectra: Vec::new(),
            traces: vec![trace],
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({ "meta": self.meta.entries }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("traces[].sample_rate_hz", Source::VendorImpl),
            ("traces[].channels[].scale", Source::VendorImpl),
            ("traces[].channels[].name", Source::VendorImpl),
            ("traces[].start_s", Source::VendorImpl),
            (
                "traces[].channels[].scale[default imMaxInt]",
                Source::Inferred,
            ),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        Ok(vec![
            LsEntry {
                kind: "metadata".into(),
                name: self
                    .meta_path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                offset: None,
                size: self.fs.metadata(&self.meta_path).ok().map(|m| m.len()),
                image: None,
                details: json!({"keys": self.meta.entries.len()}),
            },
            LsEntry {
                kind: "sweep".into(),
                name: "samples".into(),
                offset: Some(0),
                size: Some(self.samples() * self.frame()),
                image: None,
                details: json!({"channels": self.channels.len(), "samples": self.samples()}),
            },
        ])
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            FORMAT_ID,
            "image planes",
            "SpikeGLX files hold sampled signals: use `openreadout trace` or `openreadout export --format csv`.",
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
                "trace {index} sweep {sweep} out of range (a SpikeGLX stream is trace 0, sweep 0)"
            )));
        }
        let total = self.samples();
        if first_sample > total {
            return Err(Error::Usage(format!(
                "first sample {first_sample} is past the end ({total} samples)"
            )));
        }
        let nch = self.channels.len();
        // at most 4 Mi samples per channel and `MAX_READ_VALUES` values per read (a 385-channel
        // probe stream reads ~174 k samples at a time; callers page with `first_sample`)
        let per_channel = (MAX_READ_VALUES / (nch.max(1) as u64)).max(1);
        let count = max_samples
            .min(total - first_sample)
            .min(1 << 22)
            .min(per_channel);
        let frame = self.frame();
        let mut chans: Vec<Vec<f64>> = vec![Vec::with_capacity(count as usize); nch];
        let scales: Vec<f64> = self.channels.iter().map(|c| c.scale).collect();
        let bin = self.bin.clone();
        if self.handle.is_none() {
            self.handle = Some(self.fs.open(&bin).map_err(|e| Error::io(&bin, e))?);
        }
        let f = self.handle.as_mut().expect("just opened");
        let mut done = 0u64;
        while done < count {
            let k = (count - done).min((8 << 20) / frame.max(1)).max(1);
            let at = (first_sample + done) * frame;
            let block = read_block(f, &bin, at, k * frame, self.bin_len)?;
            if (block.len() as u64) < k * frame {
                return Err(Error::corrupt_at(
                    FORMAT_ID,
                    at,
                    "the .bin ends inside the requested samples",
                ));
            }
            for j in 0..k {
                for (c, ch) in chans.iter_mut().enumerate() {
                    let v = block.i16_at(at + j * frame + 2 * c as u64).unwrap_or(0);
                    ch.push(f64::from(v) * scales[c]);
                }
            }
            done += k;
        }
        Ok(Trace {
            trace: 0,
            sweep: 0,
            first_sample,
            channels: chans,
        })
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.bin.display().to_string(), FORMAT_ID);
        r.performed(".meta parsed: stream type, saved-channel list agrees with nSavedChans and the acquired layout");
        r.performed(".bin size: whole samples of nSavedChans int16 values; equals fileSizeBytes from the .meta");
        let frame = self.frame();
        if !self.bin_len.is_multiple_of(frame) {
            r.push(Finding::warning(
                "partial_sample",
                format!("{} bytes after the last whole sample", self.bin_len % frame),
            ));
        }
        match self.meta.number("fileSizeBytes") {
            Some(d) if (d as u64) > self.bin_len => r.push(
                Finding::error(
                    "truncated",
                    format!(
                        "the .bin holds {} bytes but the .meta declares {} ({:.1}% missing)",
                        self.bin_len,
                        d as u64,
                        100.0 * (d - self.bin_len as f64) / d
                    ),
                )
                .at(self.bin_len),
            ),
            Some(d) if (d as u64) < self.bin_len => r.push(Finding::warning(
                "size_mismatch",
                format!(
                    "the .bin is longer ({}) than fileSizeBytes ({})",
                    self.bin_len, d as u64
                ),
            )),
            Some(_) => {}
            None => r.push(Finding::warning(
                "no_declared_size",
                "the .meta has no fileSizeBytes",
            )),
        }
        if self.rate() <= 0.0 {
            r.push(Finding::error("bad_rate", "no sampling rate in the .meta"));
        }
        if self
            .channels
            .iter()
            .any(|c| !c.kind.is_bits() && c.unit.is_none())
        {
            r.push(Finding::warning(
                "no_scaling",
                "no analog range in the .meta: some channels are raw counts",
            ));
        }
        if self.samples() == 0 {
            r.push(Finding::warning("no_samples", "the .bin holds no samples"));
        }
        Ok(r)
    }
}
