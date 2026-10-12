//! `Dataset` for a TDT block or tank: one trace per stream store (channels are the store's
//! channels; blocks are sweeps), a `spikes` table of snippets and an `epocs` table of strobe
//! onsets with their durations.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use openreadout_core::bytes::{Block, latin1, read_block, read_file_capped_in};
use openreadout_core::model::{
    CheckReport, ColumnInfo, FileInfo, Finding, LsEntry, Severity, SignalChannelInfo, Table,
    TableInfo, Trace, TraceInfo,
};
use openreadout_core::provenance::{ProvenanceMap, Source};
use openreadout_core::reader::{Dataset, FormatReader, PlaneIndex};
use openreadout_core::source::{Fs, Input, SourceFile};
use openreadout_core::{Error, Plane, Result};
use serde_json::{Value, json};

use super::tsq::{SampleFormat, TsqIndex, parse_tsq, store_text};
use super::{BlockFiles, TDT_FORMAT_ID, TdtReader, find_blocks};

/// Samples (all channels together) decoded per `read_trace` call at most.
pub const MAX_TDT_READ_VALUES: u64 = 1 << 27;
/// Bytes of a `.Tbk` read at most.
pub const MAX_TBK_BYTES: u64 = 16 << 20;
const WINDOW: u64 = 4 << 20;

/// One parsed block.
#[derive(Debug)]
pub struct TdtBlock {
    /// Its files.
    pub files: BlockFiles,
    /// Its index.
    pub index: TsqIndex,
    /// Length of the `.tev` (0 when there is none).
    pub tev_len: u64,
    /// Store header items of the `.Tbk` (`NAME` → `VALUE`), in file order.
    pub store_headers: Vec<BTreeMap<String, String>>,
    handle: Option<SourceFile>,
    window: Block,
}

impl TdtBlock {
    /// Time origin of the block: its start mark, else its first time stamp.
    fn origin(&self) -> f64 {
        self.index.start_time.unwrap_or_else(|| {
            self.index
                .streams
                .values()
                .flat_map(|s| s.channels.values().map(|c| c.first_time))
                .chain(self.index.snippets.iter().map(|s| s.time))
                .chain(self.index.strobes.iter().map(|s| s.time))
                .fold(f64::INFINITY, f64::min)
        })
    }

    /// `n` bytes of the `.tev` at `at`, through a 4 MiB window; `None` past the end.
    fn tev_bytes(&mut self, fs: &Fs, at: u64, n: u64) -> Result<Option<&[u8]>> {
        let Some(path) = self.files.tev.clone() else {
            return Ok(None);
        };
        let Some(end) = at.checked_add(n) else {
            return Ok(None);
        };
        if end > self.tev_len {
            return Ok(None);
        }
        if at < self.window.origin || end > self.window.end() {
            if self.handle.is_none() {
                self.handle = Some(fs.open(&path).map_err(|e| Error::io(&path, e))?);
            }
            let f = self.handle.as_mut().expect("just opened");
            self.window = read_block(f, &path, at, WINDOW.max(n), self.tev_len)?;
        }
        Ok(self.window.slice(at, usize::try_from(n).unwrap_or(usize::MAX)))
    }
}

/// Parse the `NAME=…;TYPE=…;VALUE=…;` items of a `.Tbk`; a new item starts at each `StoreName`.
pub fn parse_store_headers(bytes: &[u8]) -> Vec<BTreeMap<String, String>> {
    let text = latin1(bytes);
    let mut out: Vec<BTreeMap<String, String>> = Vec::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find("NAME=") {
        rest = &rest[i + 5..];
        let mut parts = rest.splitn(4, ';');
        let (Some(name), Some(ty), Some(value)) = (parts.next(), parts.next(), parts.next()) else {
            break;
        };
        let (Some(_), Some(value)) = (ty.strip_prefix("TYPE="), value.strip_prefix("VALUE=")) else {
            continue;
        };
        if name.is_empty()
            || name.len() > 32
            || !name.chars().all(|c| c.is_ascii_alphanumeric())
            || value.len() > 256
        {
            continue;
        }
        if name == "StoreName" || out.is_empty() {
            out.push(BTreeMap::new());
        }
        if let Some(item) = out.last_mut() {
            item.insert(name.to_string(), value.to_string());
        }
    }
    out.retain(|m| m.contains_key("StoreName"));
    out
}

/// One stream store of one block, as a sweep.
#[derive(Debug, Clone)]
pub struct BlockStream {
    /// Chunks per channel that lie wholly in the `.tev` (the shortest channel).
    pub chunks: u64,
    /// Chunks per channel the index lists (the shortest channel).
    pub indexed_chunks: u64,
    /// Start of the first chunk, seconds from the tank's time origin.
    pub start_s: f64,
}

/// A trace: one stream store across the blocks.
#[derive(Debug, Clone)]
pub struct TdtTrace {
    /// Store name.
    pub store: String,
    /// Channel numbers.
    pub channels: Vec<u16>,
    /// Sample type.
    pub format: SampleFormat,
    /// Samples per chunk.
    pub chunk_samples: u64,
    /// Sampling rate (float32 in the index).
    pub sample_rate_hz: f64,
    /// Per block: the store's sweep, if the block has the store.
    pub blocks: Vec<Option<BlockStream>>,
}

/// One onset of the `epocs` table.
#[derive(Debug, Clone, Copy)]
pub struct EpocRow {
    /// Seconds from the tank's time origin.
    pub time_s: f64,
    /// Block (sweep) index.
    pub block: u32,
    /// Index into the table's `stores`.
    pub store: u32,
    /// Strobe value.
    pub value: f64,
    /// Seconds to the matching strobe-off record, NaN when there is none.
    pub duration_s: f64,
}

/// An opened TDT block or tank.
#[derive(Debug)]
pub struct TdtDataset {
    path: PathBuf,
    fs: Fs,
    tank: bool,
    /// Blocks in start order.
    pub blocks: Vec<TdtBlock>,
    /// Readable stream stores, by name.
    pub traces: Vec<TdtTrace>,
    /// Stores that cannot be read, with the reason.
    pub unread_stores: Vec<(String, String)>,
    /// Time origin: the first block's start, seconds since 1970.
    pub origin: f64,
    /// Snippet store names (the `spikes` table's `store` column indexes this).
    pub snippet_stores: Vec<String>,
    /// Samples per snippet row (the widest snippet).
    pub snippet_width: u64,
    /// Strobe-on store names (the `epocs` table's `store` column indexes this).
    pub epoc_stores: Vec<String>,
    /// The `epocs` table.
    pub epocs: Vec<EpocRow>,
    /// Strobe-off records no onset was paired with.
    pub unpaired_offsets: u64,
    findings: Vec<Finding>,
}

impl TdtDataset {
    /// Open a local path.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_input(&Input::local(path))
    }

    /// Open an [`Input`].
    pub fn open_input(input: &Input) -> Result<Self> {
        let fs = input.fs().clone();
        let (files, tank) = find_blocks(input)?;
        let mut blocks = Vec::with_capacity(files.len());
        for b in files {
            let mut f = fs.open(&b.tsq).map_err(|e| Error::io(&b.tsq, e))?;
            let len = f.size().map_err(|e| Error::io(&b.tsq, e))?;
            let index = parse_tsq(&mut f, &b.tsq, len)?;
            let tev_len = match &b.tev {
                Some(p) => fs.metadata(p).map_err(|e| Error::io(p, e))?.len(),
                None => 0,
            };
            let store_headers = match &b.tbk {
                Some(p) => parse_store_headers(&read_file_capped_in(
                    &fs,
                    p,
                    MAX_TBK_BYTES,
                    TDT_FORMAT_ID,
                    "store header (.Tbk) file",
                )?),
                None => Vec::new(),
            };
            blocks.push(TdtBlock {
                files: b,
                index,
                tev_len,
                store_headers,
                handle: None,
                window: Block::default(),
            });
        }
        blocks.sort_by(|a, b| a.origin().total_cmp(&b.origin()));
        let origin = blocks.first().map_or(0.0, TdtBlock::origin);
        let mut ds = TdtDataset {
            path: input.path().to_path_buf(),
            fs,
            tank,
            blocks,
            traces: Vec::new(),
            unread_stores: Vec::new(),
            origin: if origin.is_finite() { origin } else { 0.0 },
            snippet_stores: Vec::new(),
            snippet_width: 0,
            epoc_stores: Vec::new(),
            epocs: Vec::new(),
            unpaired_offsets: 0,
            findings: Vec::new(),
        };
        ds.build_traces();
        ds.build_tables();
        Ok(ds)
    }

    fn build_traces(&mut self) {
        let names: BTreeSet<String> = self
            .blocks
            .iter()
            .flat_map(|b| b.index.streams.keys().cloned())
            .collect();
        'store: for name in names {
            let mut spec: Option<TdtTrace> = None;
            let mut per_block = Vec::with_capacity(self.blocks.len());
            for (bi, b) in self.blocks.iter().enumerate() {
                let Some(s) = b.index.streams.get(&name) else {
                    per_block.push(None);
                    continue;
                };
                let refuse = |why: String| (name.clone(), why);
                if let Some(why) = &s.unreadable {
                    self.unread_stores.push(refuse(why.clone()));
                    continue 'store;
                }
                if b.files.sev.iter().any(|(_, st, _)| *st == name) {
                    self.unread_stores.push(refuse(
                        "its samples are in .sev files, which this reader does not read".into(),
                    ));
                    continue 'store;
                }
                if b.files.tev.is_none() {
                    self.unread_stores
                        .push(refuse("the block has no .tev data file".into()));
                    continue 'store;
                }
                let format = s.format().expect("checked by the index parser");
                let channels: Vec<u16> = s.channels.keys().copied().collect();
                let rate = f64::from(s.sample_rate_hz);
                match &spec {
                    None => {
                        spec = Some(TdtTrace {
                            store: name.clone(),
                            channels: channels.clone(),
                            format,
                            chunk_samples: s.chunk_samples(),
                            sample_rate_hz: rate,
                            blocks: Vec::new(),
                        });
                    }
                    Some(t)
                        if t.channels != channels
                            || t.format != format
                            || t.chunk_samples != s.chunk_samples()
                            || t.sample_rate_hz.to_bits() != rate.to_bits() =>
                    {
                        self.unread_stores.push(refuse(format!(
                            "its channels, data format, chunk size or sampling rate differ between blocks (block {bi})"
                        )));
                        continue 'store;
                    }
                    Some(_) => {}
                }
                let bytes = s.chunk_bytes();
                let mut usable = u64::MAX;
                let mut indexed = u64::MAX;
                let mut indexed_max = 0;
                let mut gaps = 0;
                for c in s.channels.values() {
                    let n = c.offsets.len() as u64;
                    let ok = c
                        .offsets
                        .iter()
                        .take_while(|o| o.checked_add(bytes).is_some_and(|e| e <= b.tev_len))
                        .count() as u64;
                    usable = usable.min(ok);
                    indexed = indexed.min(n);
                    indexed_max = indexed_max.max(n);
                    gaps += c.gaps;
                }
                if indexed_max != indexed {
                    self.findings.push(Finding::warning(
                        "uneven_channels",
                        format!(
                            "block {bi}, store {name}: channels have {indexed} to {indexed_max} chunks; the trace has the shortest channel's samples"
                        ),
                    ));
                }
                if usable < indexed {
                    self.findings.push(Finding::error(
                        "truncated",
                        format!(
                            "block {bi}, store {name}: {indexed} chunks per channel are indexed, {usable} lie wholly in the .tev ({} bytes)",
                            b.tev_len
                        ),
                    ));
                }
                if gaps > 0 {
                    self.findings.push(Finding::warning(
                        "gaps",
                        format!(
                            "block {bi}, store {name}: {gaps} chunk(s) start more than a chunk after the previous one; samples are joined without filling the gaps"
                        ),
                    ));
                }
                let first = s
                    .channels
                    .values()
                    .map(|c| c.first_time)
                    .fold(f64::INFINITY, f64::min);
                per_block.push(Some(BlockStream {
                    chunks: usable,
                    indexed_chunks: indexed,
                    start_s: first - self.origin,
                }));
            }
            if let Some(mut t) = spec {
                t.blocks = per_block;
                self.traces.push(t);
            }
        }
        for b in &self.blocks {
            let indexed: BTreeSet<&str> = b.index.streams.keys().map(String::as_str).collect();
            let mut sev_stores: Vec<&str> = b
                .files
                .sev
                .iter()
                .map(|(_, s, _)| s.as_str())
                .filter(|s| !indexed.contains(s))
                .collect();
            sev_stores.dedup();
            for s in sev_stores {
                let why = "its samples are in .sev files, which this reader does not read";
                if !self.unread_stores.iter().any(|(n, _)| n == s) {
                    self.unread_stores.push((s.to_string(), why.into()));
                }
            }
        }
    }

    fn build_tables(&mut self) {
        let mut snippet_names: Vec<String> = Vec::new();
        let mut width = 0;
        for b in &self.blocks {
            for n in &b.index.snippet_stores {
                if !snippet_names.contains(n) {
                    snippet_names.push(n.clone());
                }
            }
            for s in &b.index.snippets {
                if let Some(f) = SampleFormat::from_code(s.format_code) {
                    width = width.max(u64::from(s.bytes) / f.width());
                }
            }
        }
        self.snippet_stores = snippet_names;
        self.snippet_width = width;
        let mut epoc_names: Vec<[u8; 4]> = Vec::new();
        for b in &self.blocks {
            for s in b.index.strobes.iter().filter(|s| !s.off) {
                if !epoc_names.contains(&s.store) {
                    epoc_names.push(s.store);
                }
            }
        }
        let mut rows = Vec::new();
        let mut unpaired = 0u64;
        for (bi, b) in self.blocks.iter().enumerate() {
            for (si, name) in epoc_names.iter().enumerate() {
                let ons: Vec<_> = b
                    .index
                    .strobes
                    .iter()
                    .filter(|s| !s.off && s.store == *name)
                    .collect();
                let offs: Vec<_> = b
                    .index
                    .strobes
                    .iter()
                    .filter(|s| s.off && s.onset_store == *name)
                    .collect();
                unpaired += offs.len().saturating_sub(ons.len()) as u64;
                for (k, on) in ons.iter().enumerate() {
                    let next = ons.get(k + 1).map_or(f64::INFINITY, |n| n.time);
                    let duration = offs
                        .get(k)
                        .filter(|o| o.time >= on.time && o.time <= next)
                        .map_or(f64::NAN, |o| o.time - on.time);
                    rows.push(EpocRow {
                        time_s: on.time - self.origin,
                        block: bi as u32,
                        store: si as u32,
                        value: on.value,
                        duration_s: duration,
                    });
                }
            }
            let onset_names: BTreeSet<[u8; 4]> = epoc_names.iter().copied().collect();
            unpaired += b
                .index
                .strobes
                .iter()
                .filter(|s| s.off && !onset_names.contains(&s.onset_store))
                .count() as u64;
        }
        rows.sort_by(|a, b| {
            (a.block, a.time_s.to_bits(), a.store).cmp(&(b.block, b.time_s.to_bits(), b.store))
        });
        self.epoc_stores = epoc_names.into_iter().map(store_text).collect();
        self.epocs = rows;
        self.unpaired_offsets = unpaired;
    }

    fn snippet_rows(&self) -> u64 {
        self.blocks
            .iter()
            .map(|b| b.index.snippets.len() as u64)
            .sum()
    }

    /// (spikes table index, epocs table index).
    fn table_ids(&self) -> (Option<u32>, Option<u32>) {
        let spikes = (self.snippet_rows() > 0).then_some(0);
        let epocs = (!self.epocs.is_empty()).then_some(u32::from(spikes.is_some()));
        (spikes, epocs)
    }

    fn spike_columns(&self) -> Vec<ColumnInfo> {
        let mut v = vec![
            col(0, "time_s", "float64", Some("s")),
            col(1, "block", "uint32", None),
            col(2, "store", "uint16", None),
            col(3, "channel", "uint16", None),
            col(4, "sort_code", "uint16", None),
        ];
        for i in 0..self.snippet_width {
            v.push(col(v.len() as u32, &format!("w{i}"), "float64", None));
        }
        v
    }

    fn tables(&self) -> Vec<TableInfo> {
        let (s, e) = self.table_ids();
        let mut out = Vec::new();
        if let Some(i) = s {
            let mut extra = BTreeMap::new();
            extra.insert("stores".into(), json!(self.snippet_stores));
            extra.insert("samples_per_snippet".into(), json!(self.snippet_width));
            out.push(TableInfo {
                index: i,
                name: Some("spikes".into()),
                row_count: self.snippet_rows(),
                columns: self.spike_columns(),
                extra,
            });
        }
        if let Some(i) = e {
            let mut extra = BTreeMap::new();
            extra.insert("stores".into(), json!(self.epoc_stores));
            extra.insert(
                "paired_offsets".into(),
                json!(self.epocs.iter().filter(|r| !r.duration_s.is_nan()).count()),
            );
            out.push(TableInfo {
                index: i,
                name: Some("epocs".into()),
                row_count: self.epocs.len() as u64,
                columns: vec![
                    col(0, "time_s", "float64", Some("s")),
                    col(1, "block", "uint32", None),
                    col(2, "store", "uint16", None),
                    col(3, "value", "float64", None),
                    col(4, "duration_s", "float64", Some("s")),
                ],
                extra,
            });
        }
        out
    }

    fn read_spikes(&mut self, first: u64, n: u64) -> Result<Table> {
        let width = self.snippet_width as usize;
        let mut cols: Vec<Vec<f64>> = (0..5 + width)
            .map(|_| Vec::with_capacity(n as usize))
            .collect();
        let names = self.snippet_stores.clone();
        let origin = self.origin;
        let fs = self.fs.clone();
        let mut skip = first;
        let mut left = n;
        let mut wave = Vec::with_capacity(width);
        for (bi, b) in self.blocks.iter_mut().enumerate() {
            let count = b.index.snippets.len() as u64;
            if skip >= count {
                skip -= count;
                continue;
            }
            let take = left.min(count - skip);
            for k in skip..skip + take {
                let s = b.index.snippets[k as usize];
                let store = names
                    .iter()
                    .position(|x| *x == b.index.snippet_stores[usize::from(s.store)])
                    .unwrap_or(0);
                cols[0].push(s.time - origin);
                cols[1].push(bi as f64);
                cols[2].push(store as f64);
                cols[3].push(f64::from(s.channel));
                cols[4].push(f64::from(s.sort_code));
                wave.clear();
                if let Some(f) = SampleFormat::from_code(s.format_code)
                    && let Some(bytes) = b.tev_bytes(&fs, s.offset, u64::from(s.bytes))?
                {
                    f.decode(bytes, &mut wave);
                }
                for (i, c) in cols[5..].iter_mut().enumerate() {
                    c.push(wave.get(i).copied().unwrap_or(f64::NAN));
                }
            }
            left -= take;
            skip = 0;
            if left == 0 {
                break;
            }
        }
        Ok(Table {
            table: 0,
            first_row: first,
            columns: cols,
        })
    }
}

fn col(i: u32, name: &str, dtype: &str, unit: Option<&str>) -> ColumnInfo {
    ColumnInfo {
        index: i,
        name: name.into(),
        label: None,
        dtype: dtype.into(),
        unit: unit.map(str::to_string),
        range: None,
        extra: BTreeMap::new(),
    }
}

fn utc(t: f64) -> Option<String> {
    if !(t.is_finite() && t > 0.0 && t < 1e11) {
        return None;
    }
    let secs = t.floor();
    let millis = ((t - secs) * 1000.0).floor().min(999.0) as u32;
    Some(openreadout_core::time::unix_to_iso8601(secs as i64, millis))
}

impl Dataset for TdtDataset {
    fn info(&self) -> Result<FileInfo> {
        let n = self.blocks.len();
        let mut notes = vec![if self.tank {
            format!(
                "TDT tank of {n} block(s): one trace per stream store, one sweep per block (in start order)"
            )
        } else {
            "TDT block: one trace per stream store".to_string()
        }];
        notes.push(
            "values are as stored (TDT records no unit or gain); times count from the first block's start mark".into(),
        );
        for (s, why) in &self.unread_stores {
            notes.push(format!("store {s} is not read: {why}"));
        }
        let undecoded: u64 = self
            .blocks
            .iter()
            .map(|b| b.index.undecoded_records())
            .sum();
        if undecoded > 0 {
            notes.push(format!(
                "{undecoded} scalar or unknown index record(s) are not decoded"
            ));
        }
        if self.unpaired_offsets > 0 {
            notes.push(format!(
                "{} strobe-off record(s) have no matching onset",
                self.unpaired_offsets
            ));
        }
        if self
            .findings
            .iter()
            .chain(self.blocks.iter().flat_map(|b| b.index.findings.iter()))
            .any(|f| f.severity == Severity::Error)
        {
            notes.push("structural problems found (e.g. a cut-short .tev); run `check`".into());
        }
        let started = self.blocks.first().and_then(|b| b.index.start_time);
        let mut traces = Vec::new();
        for (i, t) in self.traces.iter().enumerate() {
            let counts: Vec<u64> = t
                .blocks
                .iter()
                .map(|b| b.as_ref().map_or(0, |s| s.chunks * t.chunk_samples))
                .collect();
            let mut extra = BTreeMap::new();
            extra.insert("store".into(), json!(t.store));
            extra.insert("data_format".into(), json!(t.format.name()));
            extra.insert("samples_per_chunk".into(), json!(t.chunk_samples));
            if let Some(d) = started.and_then(utc) {
                extra.insert("recorded_at".into(), json!(d));
            }
            if counts.iter().any(|c| *c != counts[0]) {
                extra.insert("sweep_sample_counts".into(), json!(counts));
            }
            let indexed: Vec<u64> = t
                .blocks
                .iter()
                .map(|b| b.as_ref().map_or(0, |s| s.indexed_chunks * t.chunk_samples))
                .collect();
            if indexed != counts {
                extra.insert("indexed_sample_counts".into(), json!(indexed));
            }
            if self.tank {
                extra.insert(
                    "sweep_starts_s".into(),
                    json!(
                        t.blocks
                            .iter()
                            .map(|b| b.as_ref().map(|s| s.start_s))
                            .collect::<Vec<_>>()
                    ),
                );
            }
            traces.push(TraceInfo {
                index: i as u32,
                name: Some(t.store.clone()),
                sample_rate_hz: t.sample_rate_hz,
                sample_count: counts.iter().copied().max().unwrap_or(0),
                sweep_count: u32::try_from(self.blocks.len()).unwrap_or(u32::MAX),
                channels: t
                    .channels
                    .iter()
                    .enumerate()
                    .map(|(c, n)| {
                        let mut e = BTreeMap::new();
                        e.insert("channel_number".into(), json!(n));
                        SignalChannelInfo {
                            index: c as u32,
                            name: format!("{} {n}", t.store),
                            unit: None,
                            dtype: t.format.name().into(),
                            scale: 1.0,
                            offset: 0.0,
                            extra: e,
                        }
                    })
                    .collect(),
                start_s: t.blocks.iter().flatten().next().map(|s| s.start_s),
                extra,
            });
        }
        let size = self
            .blocks
            .iter()
            .flat_map(|b| b.files.members.iter())
            .filter_map(|p| self.fs.metadata(p).ok().map(|m| m.len()))
            .sum();
        Ok(FileInfo {
            path: self.path.display().to_string(),
            size_bytes: size,
            format: TdtReader.descriptor(),
            format_version: None,
            images: Vec::new(),
            tables: self.tables(),
            spectra: Vec::new(),
            traces,
            plane_count: 0,
            notes,
        })
    }

    fn vendor_metadata(&self) -> Result<Value> {
        Ok(json!({
            "blocks": self.blocks.iter().map(|b| json!({
                "directory": b.files.dir.display().to_string(),
                "stem": b.files.stem,
                "index_records": b.index.record_count,
                "start_time_unix_s": b.index.start_time,
                "stop_time_unix_s": b.index.stop_time,
                "record_counts_by_type": b.index.type_counts.iter()
                    .map(|(t, n)| (format!("0x{t:x}"), json!(n))).collect::<serde_json::Map<_, _>>(),
                "stream_stores": b.index.streams.values().map(|s| json!({
                    "name": s.name, "format_code": s.format_code, "size_words": s.size_words,
                    "sample_rate_hz": s.sample_rate_hz, "channels": s.channels.len(),
                    "chunks": s.channels.values().map(|c| c.offsets.len()).sum::<usize>(),
                })).collect::<Vec<_>>(),
                "snippet_stores": b.index.snippet_stores,
                "store_headers": b.store_headers,
                "sev_files": b.files.sev.iter().map(|(p, s, c)| json!({
                    "file": p.file_name().map(|n| n.to_string_lossy().to_string()),
                    "store": s, "channel": c,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut p = ProvenanceMap::new();
        for (k, s) in [
            ("traces[].sample_rate_hz", Source::PriorArt),
            ("traces[].sample_count", Source::Inferred),
            ("traces[].sweep_count", Source::PriorArt),
            ("traces[].channels[].name", Source::PriorArt),
            ("traces[].channels[].dtype", Source::PriorArt),
            ("traces[].start_s", Source::PriorArt),
            ("traces[].extra.recorded_at", Source::Inferred),
            ("tables[].columns", Source::PriorArt),
            ("tables[].extra.stores", Source::PriorArt),
        ] {
            p.insert(k.into(), s);
        }
        p
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (bi, b) in self.blocks.iter().enumerate() {
            for m in &b.files.members {
                out.push(LsEntry {
                    kind: "file".into(),
                    name: m.display().to_string(),
                    offset: None,
                    size: self.fs.metadata(m).ok().map(|x| x.len()),
                    image: None,
                    details: json!({"block": bi}),
                });
            }
            for s in b.index.streams.values() {
                out.push(LsEntry {
                    kind: "stream store".into(),
                    name: s.name.clone(),
                    offset: None,
                    size: None,
                    image: None,
                    details: json!({
                        "block": bi, "channels": s.channels.len(),
                        "chunks_per_channel": s.channels.values().map(|c| c.offsets.len()).max(),
                        "format_code": s.format_code, "sample_rate_hz": s.sample_rate_hz,
                    }),
                });
            }
        }
        Ok(out)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        self.blocks
            .iter()
            .flat_map(|b| b.files.members.iter().cloned())
            .collect()
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            TDT_FORMAT_ID,
            "image planes",
            "TDT blocks hold sampled signals and events: use `openreadout trace` or `openreadout table`.",
        ))
    }

    fn read_trace(&mut self, index: u32, sweep: u32, first: u64, max: u64) -> Result<Trace> {
        let Some(t) = self.traces.get(index as usize).cloned() else {
            return Err(Error::Usage(format!(
                "trace index {index} out of range ({} traces)",
                self.traces.len()
            )));
        };
        if sweep as usize >= self.blocks.len() {
            return Err(Error::Usage(format!(
                "sweep {sweep} out of range ({} block(s))",
                self.blocks.len()
            )));
        }
        let nch = t.channels.len();
        let Some(bs) = t.blocks[sweep as usize].clone() else {
            return Ok(Trace {
                trace: index,
                sweep,
                first_sample: first,
                channels: vec![Vec::new(); nch],
            });
        };
        let cs = t.chunk_samples;
        let n = bs.chunks * cs;
        if first > n {
            return Err(Error::Usage(format!(
                "first sample {first} is past the end ({n} samples)"
            )));
        }
        let count = max
            .min(n - first)
            .min(MAX_TDT_READ_VALUES / nch.max(1) as u64);
        let mut channels: Vec<Vec<f64>> = vec![Vec::with_capacity(count as usize); nch];
        if count == 0 {
            return Ok(Trace {
                trace: index,
                sweep,
                first_sample: first,
                channels,
            });
        }
        let c0 = first / cs;
        let c1 = (first + count).div_ceil(cs);
        let bytes = cs * t.format.width();
        let fs = self.fs.clone();
        let b = &mut self.blocks[sweep as usize];
        let mut wanted = Vec::with_capacity(((c1 - c0) as usize).saturating_mul(nch));
        {
            let store = b
                .index
                .streams
                .get(&t.store)
                .ok_or_else(|| Error::Other("store vanished".into()))?;
            for k in c0..c1 {
                for (ci, ch) in t.channels.iter().enumerate() {
                    let at = store
                        .channels
                        .get(ch)
                        .and_then(|c| c.offsets.get(k as usize))
                        .copied()
                        .ok_or_else(|| {
                            Error::corrupt(TDT_FORMAT_ID, "chunk missing from the index")
                        })?;
                    wanted.push((ci, at));
                }
            }
        }
        for (ci, at) in wanted {
            let Some(data) = b.tev_bytes(&fs, at, bytes)? else {
                return Err(Error::corrupt_at(
                    TDT_FORMAT_ID,
                    at,
                    "a chunk lies past the end of the .tev",
                ));
            };
            t.format.decode(data, &mut channels[ci]);
        }
        let skip = (first - c0 * cs) as usize;
        for c in &mut channels {
            c.drain(..skip.min(c.len()));
            c.truncate(count as usize);
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample: first,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let (s, e) = self.table_ids();
        if Some(index) == s {
            let total = self.snippet_rows();
            if first_row > total {
                return Err(Error::Usage(format!(
                    "first row {first_row} is past the end ({total} rows)"
                )));
            }
            let mut t = self.read_spikes(first_row, max_rows.min(total - first_row))?;
            t.table = index;
            return Ok(t);
        }
        if Some(index) == e {
            let total = self.epocs.len() as u64;
            if first_row > total {
                return Err(Error::Usage(format!(
                    "first row {first_row} is past the end ({total} rows)"
                )));
            }
            let rows = &self.epocs
                [first_row as usize..(first_row + max_rows.min(total - first_row)) as usize];
            return Ok(Table {
                table: index,
                first_row,
                columns: vec![
                    rows.iter().map(|r| r.time_s).collect(),
                    rows.iter().map(|r| f64::from(r.block)).collect(),
                    rows.iter().map(|r| f64::from(r.store)).collect(),
                    rows.iter().map(|r| r.value).collect(),
                    rows.iter().map(|r| r.duration_s).collect(),
                ],
            });
        }
        Err(Error::Usage(format!("table index {index} out of range")))
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.path.display().to_string(), TDT_FORMAT_ID);
        r.performed("every .tsq record read: record 0 against the file length, start and stop marks, store formats and chunk sizes");
        r.performed("every stream chunk and snippet against the .tev length; chunk times against the sampling rate (gaps)");
        for b in &self.blocks {
            for f in &b.index.findings {
                r.push(f.clone());
            }
            let past = b
                .index
                .snippets
                .iter()
                .filter(|s| {
                    s.offset
                        .checked_add(u64::from(s.bytes))
                        .is_none_or(|e| e > b.tev_len)
                })
                .count();
            if past > 0 {
                r.push(Finding::error(
                    "snippets_past_end",
                    format!(
                        "{}: {past} snippet(s) lie past the end of the .tev (their waveforms read as NaN)",
                        b.files.stem
                    ),
                ));
            }
        }
        for f in &self.findings {
            r.push(f.clone());
        }
        for (s, why) in &self.unread_stores {
            r.push(Finding::info(
                "store_not_read",
                format!("store {s}: {why}"),
            ));
        }
        if self.unpaired_offsets > 0 {
            r.push(Finding::warning(
                "unpaired_offsets",
                format!(
                    "{} strobe-off record(s) have no matching onset",
                    self.unpaired_offsets
                ),
            ));
        }
        Ok(r)
    }
}
