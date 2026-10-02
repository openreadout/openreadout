//! The crawl: walk, read headers in parallel, journal, resume, reuse the previous run.
//!
//! State lives in `INDEX_DIR/state/`:
//!
//! - `journal.jsonl`: the run in progress, one JSON line per record (in walk order), per
//!   previous-run record that is gone, per walk error, and a checkpoint line after every chunk.
//!   A killed run is resumed from its last checkpoint (the partial chunk after it is dropped).
//! - `run.json`: the roots and settings of the run in progress (a resume must match them).
//! - `records.jsonl`: the journal of the last complete run. The next run reads it in step with
//!   the walk (both are in walk order) and reuses every record whose size, modification time,
//!   tool version and check mode are unchanged, without opening the file.
//! - `lock`: held while a crawl runs (one crawl per index directory).

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use openreadout_core::{Error, InfoOutput, Registry, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::fingerprint::{SPAN, fingerprint_dir, fingerprint_file, read_head};
use crate::manifest::{CrawlStats, IndexManifest, IndexSettings};
use crate::record::{Change, CheckSummary, ErrorInfo, ItemKind, Member, Record, Summary};
use crate::walk::{Key, WalkEvent, WalkItem, WalkOptions, Walker, dir_members, item_key};

/// Which integrity check the crawl runs on every data set.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CheckMode {
    /// Headers and structure only (`check --headers-only`): the default.
    #[default]
    Headers,
    /// The full `check` (may decode data: slower).
    Full,
    /// No check.
    None,
}

impl CheckMode {
    /// `headers`, `full` or `none`.
    pub fn as_str(self) -> &'static str {
        match self {
            CheckMode::Headers => "headers",
            CheckMode::Full => "full",
            CheckMode::None => "none",
        }
    }
}

/// Options of `index`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IndexOptions {
    /// Directories (or files) to crawl.
    pub roots: Vec<PathBuf>,
    /// Where the tables and state go (created if missing; never inside the crawl).
    pub index_dir: PathBuf,
    /// Integrity check per data set.
    pub check: CheckMode,
    /// Follow symbolic links.
    pub follow_symlinks: bool,
    /// Glob patterns of names or root-relative paths to skip.
    pub exclude: Vec<String>,
    /// Stop (resumably) after this many items in this session.
    pub max_files: Option<u64>,
    /// Stop (resumably) after this many seconds in this session.
    pub max_seconds: Option<f64>,
    /// Discard an interrupted run instead of resuming it.
    pub restart: bool,
    /// Read every item again instead of reusing the previous run's unchanged records.
    pub full_rescan: bool,
    /// Personal-data detection.
    pub pii: bool,
    /// Items per journal chunk (checkpoint granularity).
    pub chunk: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            roots: Vec::new(),
            index_dir: PathBuf::from("index"),
            check: CheckMode::Headers,
            follow_symlinks: false,
            exclude: Vec::new(),
            max_files: None,
            max_seconds: None,
            restart: false,
            full_rescan: false,
            pii: true,
            chunk: DEFAULT_CHUNK,
        }
    }
}

/// Default items per chunk.
pub const DEFAULT_CHUNK: usize = 256;

/// Most members listed per directory data set (sizes and counts cover all of them).
pub const MAX_DIR_MEMBERS: usize = 10_000;

/// Progress of a crawl, reported after every chunk.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct CrawlProgress {
    /// Counters so far (this run, all sessions).
    pub stats: CrawlStats,
    /// The last item of the chunk just written.
    pub last: Option<String>,
}

/// Progress callback.
pub type ProgressFn<'a> = dyn Fn(&CrawlProgress) + Sync + 'a;

/// This build's version, recorded in every record.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One journal line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub(crate) enum Line {
    Rec(Box<Record>),
    Gone {
        path: String,
        size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
    },
    WalkError {
        path: String,
        message: String,
    },
    Ckpt {
        path: String,
        stats: CrawlStats,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RunState {
    pub roots: Vec<String>,
    pub settings: IndexSettings,
    pub started_at: String,
    pub tool_version: String,
}

/// Current time as ISO-8601 UTC.
pub fn now_iso() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    openreadout_core::time::unix_to_iso8601(
        i64::try_from(d.as_secs()).unwrap_or(0),
        d.subsec_millis(),
    )
}

/// Lower-case extension (last one only; `a.ome.tiff` → `tiff`), empty when none.
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub(crate) fn state_dir(index_dir: &Path) -> PathBuf {
    index_dir.join("state")
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::io(path, e)
}

/// Absolute, canonical, sorted in walk order, nested roots dropped (the outer one covers
/// them). Returns the roots and notes about dropped ones.
pub fn normalize_roots(roots: &[PathBuf]) -> Result<(Vec<PathBuf>, Vec<String>)> {
    if roots.is_empty() {
        return Err(Error::Usage("give at least one directory to index".into()));
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for r in roots {
        let c = std::fs::canonicalize(r).map_err(|e| Error::io(r, e))?;
        out.push(c);
    }
    out.sort_by_key(|a| crate::walk::dir_key(a));
    out.dedup();
    let mut notes = Vec::new();
    let mut kept: Vec<PathBuf> = Vec::new();
    for r in out {
        if let Some(outer) = kept.iter().find(|k| r.starts_with(k)) {
            notes.push(format!(
                "{} is inside {} and is crawled as part of it",
                r.display(),
                outer.display()
            ));
        } else {
            kept.push(r);
        }
    }
    Ok((kept, notes))
}

/// Previous complete run's records, read in walk order.
struct Prev {
    lines: Option<std::io::Lines<BufReader<File>>>,
    peeked: Option<Record>,
}

impl Prev {
    fn open(path: &Path) -> Self {
        Prev {
            lines: File::open(path).ok().map(|f| BufReader::new(f).lines()),
            peeked: None,
        }
    }
    fn peek(&mut self) -> Option<&Record> {
        if self.peeked.is_none() {
            let lines = self.lines.as_mut()?;
            for l in lines.by_ref() {
                let Ok(l) = l else { break };
                if !l.starts_with("{\"t\":\"rec\"") {
                    continue;
                }
                if let Ok(Line::Rec(r)) = serde_json::from_str::<Line>(&l) {
                    self.peeked = Some(*r);
                    break;
                }
            }
        }
        self.peeked.as_ref()
    }
    fn take(&mut self) -> Option<Record> {
        self.peek();
        self.peeked.take()
    }
    /// Take every record with a key before `key` (they are gone), and the one equal to it.
    fn advance_to(&mut self, key: &Key, gone: &mut Vec<Record>) -> Option<Record> {
        loop {
            let k = item_key(Path::new(&self.peek()?.path));
            match k.cmp(key) {
                std::cmp::Ordering::Less => gone.extend(self.take()),
                std::cmp::Ordering::Equal => return self.take(),
                std::cmp::Ordering::Greater => return None,
            }
        }
    }
}

enum Slot {
    Gone(Record),
    Item(WalkItem, Option<Record>),
    WalkError(String, String),
}

/// Series key for streams that belong to one recording.
fn series_key(r: &Record) -> Option<String> {
    let p = Path::new(&r.path);
    let name = p.file_name()?.to_string_lossy().into_owned();
    let dir = p.parent()?;
    match r.format.as_deref()? {
        "spikeglx" => {
            // <run>_g0_t0.imec0.ap.bin, <run>_g0_t0.nidq.bin; probe folders <run>_g0_imec0/
            let prefix = name.split('.').next()?.to_string();
            let d = dir.file_name().map(|n| n.to_string_lossy().to_lowercase());
            let base = match d {
                Some(n) if n.contains("_imec") || n.contains("-imec") => {
                    dir.parent().unwrap_or(dir)
                }
                _ => dir,
            };
            Some(format!("spikeglx:{}/{prefix}", base.display()))
        }
        "blackrock" => {
            let stem = p.file_stem()?.to_string_lossy().into_owned();
            Some(format!("blackrock:{}/{stem}", dir.display()))
        }
        _ => None,
    }
}

fn base_record(item: &WalkItem, root: &str) -> Record {
    Record {
        path: item.path.to_string_lossy().into_owned(),
        root: root.to_string(),
        kind: item.kind,
        size: item.size,
        mtime_us: item.mtime_us,
        ext: extension_of(&item.path),
        indexed_at: now_iso(),
        tool_version: TOOL_VERSION.to_string(),
        ..Record::default()
    }
}

fn set_error(r: &mut Record, e: &Error, mode: &str) {
    let info = ErrorInfo::from_error(e);
    r.check = Some(CheckSummary::from_error(&info, mode));
    r.error = Some(info);
}

/// Open the data set and fill the record from its headers.
fn read_dataset(
    reader: &dyn openreadout_core::FormatReader,
    path: &Path,
    r: &mut Record,
    check: CheckMode,
    pii: bool,
) {
    let mode = check.as_str();
    let mut ds = match reader.open(path) {
        Ok(d) => d,
        Err(e) => return set_error(r, &e, mode),
    };
    let info = match ds.info() {
        Ok(i) => i,
        Err(e) => return set_error(r, &e, mode),
    };
    r.format_version.clone_from(&info.format_version);
    let assurance = openreadout_core::assurance::assess_dataset(ds.as_ref(), &info);
    r.assurance = Some(assurance.level.as_str().to_string());
    r.variant = Some(assurance.fingerprint);
    r.notes = info.notes.iter().take(8).cloned().collect();
    r.summary = Summary::from_info(&info);
    let e = openreadout_core::experiment::of_dataset(ds.as_ref(), &info);
    r.experiment = (!e.is_empty()).then_some(e);
    // A directory data set's members are the files the directory walk listed; the reader adds
    // only companions outside the directory (listing plane files again would make the next
    // crawl see a changed member list and reopen the data set every time).
    let is_dir = r.kind == ItemKind::Directory;
    let mut seen: HashSet<String> = r.members.iter().map(|m| m.path.clone()).collect();
    for m in ds.member_files() {
        if is_dir && (m.starts_with(path) || !seen.insert(m.to_string_lossy().into_owned())) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&m) {
            r.members.push(Member {
                path: m.to_string_lossy().into_owned(),
                size: meta.len(),
                mtime_us: crate::walk::mtime_us(&meta),
            });
        }
    }
    if is_dir {
        r.members.sort_by(|a, b| a.path.cmp(&b.path));
    }
    let report = match check {
        CheckMode::Headers => Some(ds.check_headers()),
        CheckMode::Full => Some(ds.check()),
        CheckMode::None => None,
    };
    r.check = match report {
        Some(Ok(rep)) => Some(CheckSummary::from_report(&rep, mode)),
        Some(Err(e)) => Some(CheckSummary::from_error(&ErrorInfo::from_error(&e), mode)),
        None => None,
    };
    if pii {
        let doc = serde_json::to_value(InfoOutput {
            file: info,
            experiment: r.experiment.clone(),
            acquisition: None,
            plate: None,
            images_total: None,
            assurance: None,
        })
        .unwrap_or_default();
        r.pii = crate::pii::scan(&doc)
            .iter()
            .map(crate::pii::Finding::flag)
            .collect();
    }
}

fn unchanged(prev: &Record, size: u64, mtime: Option<i64>, check: CheckMode) -> bool {
    prev.size == size
        && prev.mtime_us == mtime
        && prev.tool_version == TOOL_VERSION
        && (prev.format.is_none()
            || prev.member_only
            || prev
                .check
                .as_ref()
                .map_or(check == CheckMode::None, |c| c.mode == check.as_str()))
}

/// The member list a directory data set would have now: the files under it (`walked`) plus the
/// companions outside it that the previous record named, stat'ed again (one that is gone drops
/// out, so the lists differ). Sorted by path, as [`read_dataset`] stores them.
fn current_members(dir: &Path, walked: &[Member], prev: &Record) -> Vec<Member> {
    let mut out = walked.to_vec();
    for m in &prev.members {
        if Path::new(&m.path).starts_with(dir) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&m.path) {
            out.push(Member {
                path: m.path.clone(),
                size: meta.len(),
                mtime_us: crate::walk::mtime_us(&meta),
            });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Read one walk item.
fn process(
    reg: &Registry,
    item: &WalkItem,
    root: &str,
    prev: Option<&Record>,
    claimed: bool,
    opts: &IndexOptions,
) -> Record {
    let t0 = Instant::now();
    let mut r = base_record(item, root);
    let reuse = |p: &Record| {
        let mut p = p.clone();
        p.change = Change::Unchanged;
        p.root = root.to_string();
        p.bytes_read = 0;
        p
    };
    match item.kind {
        ItemKind::File => {
            if let Some(p) = prev.filter(|_| !opts.full_rescan)
                && unchanged(p, item.size, item.mtime_us, opts.check)
            {
                return reuse(p);
            }
            let (mut f, head) = match read_head(&item.path) {
                Ok(x) => x,
                Err(e) => {
                    set_error(&mut r, &Error::io(&item.path, e), opts.check.as_str());
                    return finish(r, prev, t0);
                }
            };
            r.bytes_read = head.len() as u64;
            match reg.detect_bytes(&head, &item.path) {
                Err(_) => {
                    if item.size <= SPAN {
                        r.fingerprint =
                            fingerprint_file(&mut f, item.size, &head).ok().map(|x| x.0);
                    }
                }
                Ok((reader, det)) => {
                    let d = reader.descriptor();
                    r.format = Some(det.format_id.to_string());
                    r.format_name = Some(d.name);
                    r.family = Some(d.family);
                    r.format_vendor = Some(d.vendor);
                    r.confidence = serde_json::to_value(det.confidence)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string));
                    r.detect_note = det.note;
                    if let Ok((fp, extra)) = fingerprint_file(&mut f, item.size, &head) {
                        r.fingerprint = Some(fp);
                        r.bytes_read += extra;
                    }
                    drop(f);
                    if claimed {
                        r.member_only = true;
                    } else {
                        read_dataset(reader, &item.path, &mut r, opts.check, opts.pii);
                    }
                }
            }
        }
        ItemKind::Directory => {
            let (members, total, newest, _count) = dir_members(&item.path, MAX_DIR_MEMBERS);
            let mtime = newest.or(item.mtime_us);
            if let Some(p) = prev.filter(|_| !opts.full_rescan)
                && unchanged(p, total, mtime, opts.check)
                && p.members == current_members(&item.path, &members, p)
            {
                return reuse(p);
            }
            r.size = total;
            r.mtime_us = mtime;
            if let Ok((fp, read)) = fingerprint_dir(&item.path, &members) {
                r.fingerprint = Some(fp);
                r.bytes_read = read;
            }
            r.members = members;
            match reg.detect(&item.path) {
                Err(_) => {}
                Ok((reader, det)) => {
                    let d = reader.descriptor();
                    r.format = Some(det.format_id.to_string());
                    r.format_name = Some(d.name);
                    r.family = Some(d.family);
                    r.format_vendor = Some(d.vendor);
                    r.confidence = serde_json::to_value(det.confidence)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string));
                    r.detect_note = det.note;
                    read_dataset(reader, &item.path, &mut r, opts.check, opts.pii);
                }
            }
        }
    }
    r.series_key = series_key(&r);
    finish(r, prev, t0)
}

fn finish(mut r: Record, prev: Option<&Record>, t0: Instant) -> Record {
    r.change = if prev.is_some() {
        Change::Changed
    } else {
        Change::New
    };
    r.elapsed_ms = (t0.elapsed().as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0;
    r
}

/// Open (creating) the journal and position it for appending. On resume, the journal is cut
/// after its last checkpoint; returns that checkpoint.
fn open_journal(path: &Path, resume: bool) -> Result<(File, Option<(String, CrawlStats)>)> {
    if !resume {
        let f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .map_err(io(path))?;
        return Ok((f, None));
    }
    let mut last: Option<(String, CrawlStats)> = None;
    let mut cut = 0u64;
    if let Ok(f) = File::open(path) {
        let mut rd = BufReader::new(f);
        let mut offset = 0u64;
        let mut buf = String::new();
        loop {
            buf.clear();
            let n = rd.read_line(&mut buf).map_err(io(path))?;
            if n == 0 {
                break;
            }
            offset += n as u64;
            if !buf.ends_with('\n') {
                break; // a torn last line
            }
            if buf.starts_with("{\"t\":\"ckpt\"")
                && let Ok(Line::Ckpt { path: p, stats }) = serde_json::from_str::<Line>(&buf)
            {
                last = Some((p, stats));
                cut = offset;
            }
        }
    }
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(io(path))?;
    f.set_len(cut).map_err(io(path))?;
    let mut f = f;
    f.seek(SeekFrom::End(0)).map_err(io(path))?;
    Ok((f, last))
}

fn write_json_atomic<T: Serialize>(path: &Path, v: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(v).map_err(|e| Error::Other(e.to_string()))?;
    std::fs::write(&tmp, text).map_err(io(&tmp))?;
    std::fs::rename(&tmp, path).map_err(io(path))
}

/// Build or update the index of `opts.roots` in `opts.index_dir`: crawl (resuming an
/// interrupted run, reusing unchanged records of the previous one), then write the tables and
/// `index.json`. `progress` is called after every chunk.
pub fn index(
    reg: &Registry,
    opts: &IndexOptions,
    progress: Option<&ProgressFn<'_>>,
) -> Result<IndexManifest> {
    let session_start = Instant::now();
    let (roots, root_notes) = normalize_roots(&opts.roots)?;
    std::fs::create_dir_all(&opts.index_dir).map_err(io(&opts.index_dir))?;
    let index_dir = std::fs::canonicalize(&opts.index_dir).map_err(io(&opts.index_dir))?;
    for r in &roots {
        if index_dir == *r || r.starts_with(&index_dir) {
            return Err(Error::Usage(format!(
                "the index directory {} contains the root {}; put the index elsewhere",
                index_dir.display(),
                r.display()
            )));
        }
    }
    let state = state_dir(&index_dir);
    std::fs::create_dir_all(&state).map_err(io(&state))?;
    let lock_path = state.join("lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(io(&lock_path))?;
    if lock.try_lock().is_err() {
        return Err(Error::Usage(format!(
            "another `openreadout index` is writing {}; wait for it, or use another index directory",
            index_dir.display()
        )));
    }
    let settings = IndexSettings {
        check: opts.check.as_str().into(),
        follow_symlinks: opts.follow_symlinks,
        exclude: opts.exclude.clone(),
        pii: opts.pii,
    };
    let root_strs: Vec<String> = roots
        .iter()
        .map(|r| r.to_string_lossy().into_owned())
        .collect();
    let run_path = state.join("run.json");
    let journal_path = state.join("journal.jsonl");
    let records_path = state.join("records.jsonl");
    let existing: Option<RunState> = std::fs::read_to_string(&run_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let resume = match &existing {
        Some(run) if !opts.restart => {
            if run.roots != root_strs
                || run.settings != settings
                || run.tool_version != TOOL_VERSION
            {
                return Err(Error::Usage(format!(
                    "{} holds an interrupted crawl of {} with other settings or another version; rerun that command to finish it, or pass --restart to discard it",
                    index_dir.display(),
                    run.roots.join(", ")
                )));
            }
            true
        }
        _ => false,
    };
    let run = if resume {
        existing.clone().unwrap_or_else(|| RunState {
            roots: root_strs.clone(),
            settings: settings.clone(),
            started_at: now_iso(),
            tool_version: TOOL_VERSION.into(),
        })
    } else {
        let run = RunState {
            roots: root_strs.clone(),
            settings: settings.clone(),
            started_at: now_iso(),
            tool_version: TOOL_VERSION.into(),
        };
        write_json_atomic(&run_path, &run)?;
        run
    };
    let (mut journal, ckpt) = open_journal(&journal_path, resume)?;
    let mut stats = ckpt.as_ref().map(|c| c.1.clone()).unwrap_or_default();
    let before_s = stats.elapsed_s;
    stats.sessions += 1;
    let resume_key = ckpt.as_ref().map(|c| item_key(Path::new(&c.0)));

    // With --full-rescan the previous records are still merged in (for `change` and moves),
    // but never reused.
    let mut prev = Prev::open(&records_path);
    if let Some(k) = &resume_key {
        let mut skipped = Vec::new();
        prev.advance_to(k, &mut skipped);
    }

    let exclude: Vec<glob::Pattern> = opts
        .exclude
        .iter()
        .map(|p| {
            glob::Pattern::new(p)
                .map_err(|e| Error::Usage(format!("bad --exclude pattern '{p}': {e}")))
        })
        .collect::<Result<_>>()?;
    let walk_opts = WalkOptions {
        follow_symlinks: opts.follow_symlinks,
        exclude,
        skip_dirs: vec![index_dir.clone()],
    };
    let walk_base = stats.walk.clone();
    let mut walker = Walker::new(reg, roots.clone(), walk_opts);
    if let Some(k) = resume_key.clone() {
        walker.resume_after(k);
    }
    let chunk_size = opts.chunk.max(1);
    let mut claimed: HashSet<String> = HashSet::new();
    let mut session_items = 0u64;
    let mut complete = true;
    let mut last_path: Option<String> = None;
    let mut chunk: Vec<Slot> = Vec::with_capacity(chunk_size);
    let mut walk_done = false;
    while !walk_done {
        // Fill one chunk in walk order.
        let mut items_in_chunk = 0usize;
        // A chunk never runs past `--max-files`.
        let this_chunk = opts.max_files.map_or(chunk_size, |m| {
            usize::try_from(m.saturating_sub(session_items))
                .unwrap_or(usize::MAX)
                .clamp(1, chunk_size)
        });
        while items_in_chunk < this_chunk {
            match walker.next() {
                None => {
                    walk_done = true;
                    break;
                }
                Some(WalkEvent::Error { path, message }) => {
                    chunk.push(Slot::WalkError(
                        path.to_string_lossy().into_owned(),
                        message,
                    ));
                }
                Some(WalkEvent::Item(item)) => {
                    let mut gone = Vec::new();
                    let p = prev.advance_to(&item_key(&item.path), &mut gone);
                    chunk.extend(gone.into_iter().map(Slot::Gone));
                    chunk.push(Slot::Item(item, p));
                    items_in_chunk += 1;
                }
            }
        }
        if walk_done {
            while let Some(g) = prev.take() {
                chunk.push(Slot::Gone(g));
            }
        }
        if chunk.is_empty() {
            break;
        }
        // Members named by earlier data sets are only sniffed, not opened again (a multi-file
        // OME-TIFF set would otherwise be read once per member). Read-only while the chunk runs.
        let claimed_now = &claimed;
        let lines: Vec<Line> = chunk
            .par_iter()
            .map(|s| match s {
                Slot::Gone(g) => Line::Gone {
                    path: g.path.clone(),
                    size: g.total_size(),
                    fingerprint: g.fingerprint.clone(),
                },
                Slot::WalkError(p, m) => Line::WalkError {
                    path: p.clone(),
                    message: m.clone(),
                },
                Slot::Item(item, p) => {
                    let root = &root_strs[item.root];
                    let is_claimed = claimed_now.contains(item.path.to_string_lossy().as_ref());
                    Line::Rec(Box::new(process(
                        reg,
                        item,
                        root,
                        p.as_ref(),
                        is_claimed,
                        opts,
                    )))
                }
            })
            .collect();
        chunk.clear();
        let mut buf = Vec::with_capacity(lines.len() * 512);
        for l in &lines {
            match l {
                Line::Rec(r) => {
                    stats.items += 1;
                    session_items += 1;
                    stats.files_seen += match r.kind {
                        ItemKind::File => 1,
                        ItemKind::Directory => r.members.len().max(1) as u64,
                    };
                    stats.bytes_seen = stats.bytes_seen.saturating_add(r.size);
                    stats.bytes_read = stats.bytes_read.saturating_add(r.bytes_read);
                    if r.is_dataset() {
                        stats.recognised += 1;
                        if r.error.is_some() {
                            stats.unreadable += 1;
                        }
                    } else {
                        stats.unknown += 1;
                    }
                    match r.change {
                        Change::New | Change::Moved => stats.new += 1,
                        Change::Changed => stats.changed += 1,
                        Change::Unchanged => stats.unchanged += 1,
                    }
                    if r.kind == ItemKind::File && !r.members.is_empty() {
                        claimed.extend(r.members.iter().map(|m| m.path.clone()));
                    }
                    last_path = Some(r.path.clone());
                }
                Line::Gone { .. } => stats.removed += 1,
                _ => {}
            }
            serde_json::to_writer(&mut buf, l).map_err(|e| Error::Other(e.to_string()))?;
            buf.push(b'\n');
        }
        stats.walk = add_walk(&walk_base, &walker.stats);
        stats.elapsed_s = before_s + session_start.elapsed().as_secs_f64();
        if let Some(p) = &last_path {
            let ck = Line::Ckpt {
                path: p.clone(),
                stats: stats.clone(),
            };
            serde_json::to_writer(&mut buf, &ck).map_err(|e| Error::Other(e.to_string()))?;
            buf.push(b'\n');
        }
        journal.write_all(&buf).map_err(io(&journal_path))?;
        journal.flush().map_err(io(&journal_path))?;
        journal.sync_data().map_err(io(&journal_path))?;
        if let Some(cb) = progress {
            cb(&CrawlProgress {
                stats: stats.clone(),
                last: last_path.clone(),
            });
        }
        if walk_done {
            break;
        }
        let over_files = opts.max_files.is_some_and(|m| session_items >= m);
        let over_time = opts
            .max_seconds
            .is_some_and(|s| session_start.elapsed().as_secs_f64() >= s);
        if over_files || over_time {
            complete = false;
            break;
        }
    }
    drop(journal);
    stats.walk = add_walk(&walk_base, &walker.stats);
    stats.elapsed_s = before_s + session_start.elapsed().as_secs_f64();
    // The tables: the journal, then (when stopped early) the previous run's records after the
    // last item read.
    let tail_from = if complete {
        None
    } else {
        last_path
            .clone()
            .or_else(|| ckpt.as_ref().map(|c| c.0.clone()))
    };
    let mut manifest = crate::finalize::finalize(
        &index_dir,
        &journal_path,
        (!complete).then_some(records_path.as_path()),
        tail_from.as_deref(),
        &run,
        &stats,
        complete,
        &reg.descriptors()
            .into_iter()
            .map(|d| (d.id, (d.name, d.vendor)))
            .collect(),
    )?;
    if complete {
        std::fs::rename(&journal_path, &records_path).map_err(io(&records_path))?;
        let _ = std::fs::remove_file(&run_path);
    } else {
        manifest.next = Some(format!(
            "The crawl stopped after {session_items} items (cap). The tables hold what was read so far plus the previous run's records for the rest. Run the same command again to continue from {}.",
            last_path.unwrap_or_else(|| "the start".into())
        ));
    }
    let mut notes = root_notes;
    if let Some(n) = manifest.next.take() {
        notes.push(n);
    }
    manifest.next = (!notes.is_empty()).then(|| notes.join(" "));
    write_json_atomic(&index_dir.join(crate::manifest::MANIFEST), &manifest)?;
    drop(lock);
    Ok(manifest)
}

fn add_walk(base: &crate::walk::WalkStats, s: &crate::walk::WalkStats) -> crate::walk::WalkStats {
    crate::walk::WalkStats {
        directories: base.directories + s.directories,
        hidden_skipped: base.hidden_skipped + s.hidden_skipped,
        symlinks_skipped: base.symlinks_skipped + s.symlinks_skipped,
        excluded: base.excluded + s.excluded,
        unreadable_directories: base.unreadable_directories + s.unreadable_directories,
    }
}
