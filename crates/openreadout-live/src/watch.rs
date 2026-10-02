//! A polling directory watcher that reports new data sets, new planes, scans and sweeps,
//! completion and stalls as events.
//!
//! Polling rather than filesystem notifications: instruments write to SMB/NFS shares, where
//! inotify, FSEvents and ReadDirectoryChangesW do not see writes made by another machine, and
//! a stat per file per poll is cheap next to opening the files that changed. Nothing is kept
//! open between polls, files are opened read-only and never locked.
//!
//! Memory is bounded: per data set a few counters, plus the set of planes already reported
//! while it is still being written (freed when it completes); at most
//! [`WatchOptions::max_tracked`] data sets are remembered (finished ones are forgotten first).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use openreadout_core::batch::{SIDECAR_SUFFIX, is_dataset_dir};
use openreadout_core::live::{AcquisitionState, assess};
use openreadout_core::{Dataset, Error, ErrorBody, PlaneIndex, Registry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::qc::{QcFinding, QcState, Rules};

/// Kinds of events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventKind {
    /// A data set was seen for the first time (or reappeared).
    DatasetNew,
    /// A new complete image plane.
    PlaneNew,
    /// A new mass-spectrometry scan.
    ScanNew,
    /// A new sweep (frame) of a trace: electrophysiology sweeps, NMR FIDs.
    FrameNew,
    /// The data set is finished: every end-of-file structure is written and it stopped changing.
    DatasetComplete,
    /// A data set still being written has not grown for `stall_after` (or was interrupted).
    DatasetStalled,
    /// A QC rule fired.
    Qc,
    /// A data set could not be read.
    Error,
}

/// One line of `watch` output.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WatchEvent {
    /// Sequence number, increasing by one per event within a watcher (the MCP cursor).
    pub seq: u64,
    /// What happened.
    pub event: EventKind,
    /// When the event was emitted (ISO-8601 UTC, milliseconds).
    pub ts: String,
    /// The data set (a file, or a directory store).
    pub path: String,
    /// Format id, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// `in_progress`, `interrupted` or `complete`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Image index (plane events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<u32>,
    /// Channel index (plane events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<u32>,
    /// Z index (plane events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<u32>,
    /// Time index (plane events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<u32>,
    /// Run (spectra) or trace index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u32>,
    /// Zero-based position of the plane, scan or sweep among those reported for the data set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    /// Planes (or scans) complete so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<u64>,
    /// Planes the finished data set will hold, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<u64>,
    /// Size of the data set in bytes when the event was produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Modification time of the data set (ISO-8601 UTC).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime: Option<String>,
    /// Seconds since the data set last grew (`dataset_stalled`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_s: Option<f64>,
    /// The fired rule (`qc`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qc: Option<QcFinding>,
    /// What went wrong (`error`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

/// Watch options.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct WatchOptions {
    /// Directories (or single files) to watch.
    pub roots: Vec<PathBuf>,
    /// Only data sets modified at or after this time are reported; older ones are remembered
    /// silently and reported if they change later. `None` reports everything.
    pub since: Option<SystemTime>,
    /// A data set still being written that does not grow for this long is `dataset_stalled`.
    pub stall_after: Duration,
    /// Most data sets remembered at once.
    pub max_tracked: usize,
    /// Deepest directory level walked below a root.
    pub max_depth: usize,
    /// QC rules evaluated on every new plane or scan (reads its pixels or peaks).
    pub qc: Option<Rules>,
    /// Report a finished data set as complete on the first poll, without waiting for a second
    /// poll to show it stopped changing (`--once`).
    pub trust_first_look: bool,
}

impl Default for WatchOptions {
    fn default() -> Self {
        WatchOptions {
            roots: Vec::new(),
            since: None,
            stall_after: Duration::from_secs(120),
            max_tracked: 100_000,
            max_depth: 32,
            qc: None,
            trust_first_look: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Not reported (older than `since`, or not an instrument file).
    Quiet,
    /// Seen but not openable yet (header still being written).
    Pending,
    InProgress,
    Stalled,
    Complete,
    Failed,
}

#[derive(Debug)]
struct Tracked {
    size: u64,
    mtime: Option<SystemTime>,
    last_growth: Instant,
    phase: Phase,
    format: Option<String>,
    announced: bool,
    /// Planes reported while the data set is being written (freed at completion).
    seen: HashSet<(u32, PlaneIndex)>,
    planes_reported: u64,
    scans_reported: Vec<u64>,
    sweeps_reported: Vec<u64>,
    /// The size and time looked at by the previous poll (completion needs one stable poll).
    stable_polls: u32,
    qc: Option<QcState>,
    last_poll: u64,
}

impl Tracked {
    fn new(size: u64, mtime: Option<SystemTime>) -> Self {
        Tracked {
            size,
            mtime,
            last_growth: Instant::now(),
            phase: Phase::Pending,
            format: None,
            announced: false,
            seen: HashSet::new(),
            planes_reported: 0,
            scans_reported: Vec::new(),
            sweeps_reported: Vec::new(),
            stable_polls: 0,
            qc: None,
            last_poll: 0,
        }
    }
}

/// The watcher: call [`Watcher::poll`] at the interval you want.
#[derive(Debug)]
pub struct Watcher {
    opts: WatchOptions,
    tracked: HashMap<PathBuf, Tracked>,
    dataset_dirs: HashMap<PathBuf, bool>,
    seq: u64,
    polls: u64,
    started: Instant,
}

/// ISO-8601 UTC with milliseconds.
pub fn iso(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    openreadout_core::time::unix_to_iso8601(d.as_secs() as i64, d.subsec_millis())
}

/// Size and newest modification time of a path: a file's own, or for a directory data set
/// the sum and newest over its files (hidden entries skipped).
fn stat(p: &Path) -> Option<(u64, Option<SystemTime>)> {
    let m = std::fs::metadata(p).ok()?;
    if !m.is_dir() {
        return Some((m.len(), m.modified().ok()));
    }
    let mut size = 0u64;
    let mut newest = m.modified().ok();
    let mut stack = vec![p.to_path_buf()];
    let mut n = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            n += 1;
            if n > 2_000_000 {
                break;
            }
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(e.path());
            } else {
                size += md.len();
            }
            if let Ok(t) = md.modified() {
                newest = Some(newest.map_or(t, |x| x.max(t)));
            }
        }
    }
    Some((size, newest))
}

impl Watcher {
    /// A watcher over `opts.roots`. Nothing is read until the first poll.
    pub fn new(opts: WatchOptions) -> Watcher {
        Watcher {
            opts,
            tracked: HashMap::new(),
            dataset_dirs: HashMap::new(),
            seq: 0,
            polls: 0,
            started: Instant::now(),
        }
    }

    /// Number of data sets currently remembered.
    pub fn tracked(&self) -> usize {
        self.tracked.len()
    }

    /// The last sequence number handed out (0 before the first event).
    pub fn last_seq(&self) -> u64 {
        self.seq
    }

    fn items(&mut self, reg: &Registry) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let roots = self.opts.roots.clone();
        for r in &roots {
            if r.is_file() {
                out.push(r.clone());
                continue;
            }
            if self.is_dataset_dir(reg, r) {
                out.push(r.clone());
                continue;
            }
            let mut stack = vec![(r.clone(), 0usize)];
            while let Some((d, depth)) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else {
                    continue;
                };
                let mut entries: Vec<_> = rd.flatten().collect();
                entries.sort_by_key(std::fs::DirEntry::file_name);
                for e in entries {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') || name.ends_with(SIDECAR_SUFFIX) {
                        continue;
                    }
                    let p = e.path();
                    let Ok(ft) = e.file_type() else { continue };
                    if ft.is_dir() {
                        if self.is_dataset_dir(reg, &p) {
                            out.push(p);
                        } else if depth < self.opts.max_depth {
                            stack.push((p, depth + 1));
                        }
                    } else if ft.is_file() {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    fn is_dataset_dir(&mut self, reg: &Registry, d: &Path) -> bool {
        if let Some(v) = self.dataset_dirs.get(d) {
            return *v;
        }
        let v = is_dataset_dir(reg, d);
        // Remember positive answers; a directory that is not (yet) a data set is asked again
        // (a Zarr store's metadata may not be written yet).
        if v || self.dataset_dirs.len() < 100_000 {
            self.dataset_dirs.insert(d.to_path_buf(), v);
        }
        if !v {
            self.dataset_dirs.remove(d);
        }
        v
    }

    fn event(&mut self, kind: EventKind, path: &Path, t: &Tracked) -> WatchEvent {
        self.seq += 1;
        WatchEvent {
            seq: self.seq,
            event: kind,
            ts: iso(SystemTime::now()),
            path: path.display().to_string(),
            format: t.format.clone(),
            state: None,
            image: None,
            c: None,
            z: None,
            t: None,
            run: None,
            index: None,
            complete: None,
            expected: None,
            size_bytes: Some(t.size),
            mtime: t.mtime.map(iso),
            idle_s: None,
            qc: None,
            error: None,
        }
    }

    /// Look at every data set under the roots once and hand each event to `emit`, in order.
    pub fn poll(&mut self, reg: &Registry, emit: &mut dyn FnMut(WatchEvent)) {
        self.polls += 1;
        let poll = self.polls;
        for path in self.items(reg) {
            let Some((size, mtime)) = stat(&path) else {
                continue;
            };
            let mut t = self
                .tracked
                .remove(&path)
                .unwrap_or_else(|| Tracked::new(size, mtime));
            let fresh = t.last_poll == 0;
            t.last_poll = poll;
            let changed = fresh || size != t.size || mtime != t.mtime;
            if changed && !fresh {
                t.last_growth = Instant::now();
                t.stable_polls = 0;
            } else if !fresh {
                t.stable_polls = t.stable_polls.saturating_add(1);
            }
            t.size = size;
            t.mtime = mtime;
            if fresh
                && let (Some(since), Some(m)) = (self.opts.since, mtime)
                && m < since
            {
                t.phase = Phase::Quiet;
                self.tracked.insert(path, t);
                continue;
            }
            let look = changed || matches!(t.phase, Phase::Pending | Phase::InProgress);
            if look {
                self.look(reg, &path, &mut t, emit);
            }
            if matches!(t.phase, Phase::InProgress) {
                let idle = t.last_growth.elapsed();
                if idle >= self.opts.stall_after {
                    t.phase = Phase::Stalled;
                    let mut e = self.event(EventKind::DatasetStalled, &path, &t);
                    e.state = Some("in_progress".into());
                    e.idle_s = Some((idle.as_secs_f64() * 10.0).round() / 10.0);
                    e.complete = Some(t.planes_reported);
                    emit(e);
                }
            }
            self.tracked.insert(path, t);
        }
        // Forget data sets that disappeared, then enforce the bound (finished ones first).
        self.tracked.retain(|_, t| t.last_poll == poll);
        if self.tracked.len() > self.opts.max_tracked {
            let mut quiet: Vec<(PathBuf, Option<SystemTime>)> = self
                .tracked
                .iter()
                .filter(|(_, t)| matches!(t.phase, Phase::Complete | Phase::Quiet | Phase::Failed))
                .map(|(p, t)| (p.clone(), t.mtime))
                .collect();
            quiet.sort_by_key(|(_, m)| *m);
            let excess = self.tracked.len() - self.opts.max_tracked;
            for (p, _) in quiet.into_iter().take(excess) {
                self.tracked.remove(&p);
            }
        }
    }

    fn look(
        &mut self,
        reg: &Registry,
        path: &Path,
        t: &mut Tracked,
        emit: &mut dyn FnMut(WatchEvent),
    ) {
        let opened = reg.open(path);
        let (det, mut ds) = match opened {
            Ok(x) => x,
            Err(Error::UnknownFormat { .. }) => {
                t.phase = Phase::Quiet;
                return;
            }
            Err(e) => {
                let recent = t.mtime.is_some_and(|m| {
                    openreadout_core::live::seconds_since(m)
                        <= openreadout_core::live::window().as_secs_f64()
                });
                if recent && !self.opts.trust_first_look {
                    // Probably the header is still being written: try again next poll.
                    t.phase = Phase::Pending;
                    return;
                }
                if t.phase != Phase::Failed {
                    t.phase = Phase::Failed;
                    let mut ev = self.event(EventKind::Error, path, t);
                    if recent {
                        ev.state = Some("in_progress".into());
                    }
                    ev.error = Some((&e).into());
                    emit(ev);
                }
                return;
            }
        };
        t.format = Some(det.format_id.to_string());
        let info = match ds.info() {
            Ok(i) => i,
            Err(e) => {
                if t.phase != Phase::Failed {
                    t.phase = Phase::Failed;
                    let mut ev = self.event(EventKind::Error, path, t);
                    ev.error = Some((&e).into());
                    emit(ev);
                }
                return;
            }
        };
        let ws = ds.write_state();
        let acq = ws.as_ref().and_then(|w| assess(path, w));
        let state = match acq.as_ref().map(|a| a.state) {
            Some(AcquisitionState::InProgress) => "in_progress",
            Some(_) => "interrupted",
            None => "complete",
        };
        // The planes available now, in write order when the reader knows it.
        let planes: Vec<(u32, PlaneIndex)> = match &ws {
            Some(w)
                if acq
                    .as_ref()
                    .is_some_and(openreadout_core::live::Acquisition::in_progress) =>
            {
                w.complete.clone()
            }
            _ => all_planes(&info.images),
        };
        let expected = acq
            .as_ref()
            .and_then(|a| a.expected_planes)
            .or_else(|| (state == "complete").then_some(info.plane_count));
        if !t.announced {
            t.announced = true;
            let mut e = self.event(EventKind::DatasetNew, path, t);
            e.state = Some(state.into());
            e.complete = Some(planes.len() as u64);
            e.expected = expected;
            emit(e);
        }
        if state == "in_progress" && t.phase != Phase::Stalled {
            t.phase = Phase::InProgress;
        } else if state == "in_progress" && t.stable_polls == 0 {
            t.phase = Phase::InProgress; // grew again after a stall
        }
        if self.opts.qc.is_some() && t.qc.is_none() {
            t.qc = Some(QcState::default());
        }
        let complete_now = planes.len() as u64;
        let arrival = self.started.elapsed().as_secs_f64();
        for (i, (image, idx)) in planes.iter().enumerate() {
            let pos = i as u64;
            // While a data set is written, planes are matched by index; once it is complete
            // (its final metadata may re-shape the planes, e.g. an OME-TIFF whose pages read as
            // Z planes until the OME-XML is written), by count in acquisition order.
            let new = if state == "complete" {
                pos >= t.planes_reported
            } else {
                !t.seen.contains(&(*image, *idx))
            };
            if !new {
                continue;
            }
            if state != "complete" {
                t.seen.insert((*image, *idx));
            }
            t.planes_reported += 1;
            let mut e = self.event(EventKind::PlaneNew, path, t);
            e.image = Some(*image);
            e.c = Some(idx.c);
            e.z = Some(idx.z);
            e.t = Some(idx.t);
            e.index = Some(t.planes_reported - 1);
            e.complete = Some(complete_now);
            e.expected = expected;
            e.state = Some(state.into());
            emit(e);
            if let (Some(rules), Some(q)) = (self.opts.qc.as_ref(), t.qc.as_mut())
                && let Ok(p) = ds.read_plane(*image, *idx)
            {
                let level = info
                    .images
                    .iter()
                    .find(|im| im.index == *image)
                    .and_then(openreadout_core::stats::recorded_saturation)
                    .map(|l| l.0);
                let found = q.plane_at(rules, *image, *idx, &p, arrival, level);
                for f in found {
                    self.seq += 1;
                    emit(WatchEvent {
                        seq: self.seq,
                        event: EventKind::Qc,
                        ts: iso(SystemTime::now()),
                        path: path.display().to_string(),
                        format: t.format.clone(),
                        state: Some(state.into()),
                        image: Some(*image),
                        c: Some(idx.c),
                        z: Some(idx.z),
                        t: Some(idx.t),
                        run: None,
                        index: Some(t.planes_reported - 1),
                        complete: None,
                        expected: None,
                        size_bytes: Some(t.size),
                        mtime: t.mtime.map(iso),
                        idle_s: None,
                        qc: Some(f),
                        error: None,
                    });
                }
            }
        }
        self.scans(ds.as_mut(), path, t, &info, state, emit);
        // Sweeps of traces.
        t.sweeps_reported.resize(info.traces.len(), 0);
        for (ti, tr) in info.traces.iter().enumerate() {
            let n = u64::from(tr.sweep_count);
            while t.sweeps_reported[ti] < n {
                let k = t.sweeps_reported[ti];
                t.sweeps_reported[ti] += 1;
                let mut e = self.event(EventKind::FrameNew, path, t);
                e.run = Some(ti as u32);
                e.index = Some(k);
                e.complete = Some(n);
                e.state = Some(state.into());
                emit(e);
            }
        }
        match state {
            "complete" => {
                let stable = self.opts.trust_first_look || t.stable_polls >= 1;
                if stable && t.phase != Phase::Complete {
                    t.phase = Phase::Complete;
                    t.seen = HashSet::new();
                    let mut e = self.event(EventKind::DatasetComplete, path, t);
                    e.state = Some("complete".into());
                    e.complete = Some(t.planes_reported.max(complete_now));
                    e.expected = expected;
                    emit(e);
                } else if !stable && t.phase != Phase::Complete {
                    t.phase = Phase::InProgress;
                }
            }
            "interrupted" if t.phase != Phase::Stalled => {
                t.phase = Phase::Stalled;
                let mut e = self.event(EventKind::DatasetStalled, path, t);
                e.state = Some("interrupted".into());
                e.idle_s = t
                    .mtime
                    .map(|m| openreadout_core::live::seconds_since(m).round());
                e.complete = Some(complete_now);
                e.expected = expected;
                emit(e);
            }
            _ => {}
        }
    }

    fn scans(
        &mut self,
        ds: &mut dyn Dataset,
        path: &Path,
        t: &mut Tracked,
        info: &openreadout_core::FileInfo,
        state: &str,
        emit: &mut dyn FnMut(WatchEvent),
    ) {
        t.scans_reported.resize(info.spectra.len(), 0);
        for (ri, run) in info.spectra.iter().enumerate() {
            while t.scans_reported[ri] < run.scan_count {
                let k = t.scans_reported[ri];
                t.scans_reported[ri] += 1;
                let mut e = self.event(EventKind::ScanNew, path, t);
                e.run = Some(ri as u32);
                e.index = Some(k);
                e.complete = Some(run.scan_count);
                e.state = Some(state.into());
                emit(e);
                if let (Some(rules), Some(q)) = (self.opts.qc.as_ref(), t.qc.as_mut())
                    && let Ok(sp) = ds.read_spectrum(ri as u32, k)
                {
                    let tic = sp
                        .total_ion_current
                        .unwrap_or_else(|| sp.intensity.iter().map(|v| f64::from(*v)).sum::<f64>());
                    for f in q.scan(rules, sp.ms_level, tic) {
                        let mut e = self.event(EventKind::Qc, path, t);
                        e.run = Some(ri as u32);
                        e.index = Some(k);
                        e.state = Some(state.into());
                        e.qc = Some(f);
                        emit(e);
                    }
                }
            }
        }
    }
}

/// Parse a `since` value: `all` (`None`), a duration back from now (`90s`, `10m`, `2h`,
/// `1d`, plain seconds) or an ISO-8601 time.
pub fn parse_since(s: &str) -> openreadout_core::Result<Option<SystemTime>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("all") {
        return Ok(None);
    }
    let unit = s.chars().last().unwrap_or('s');
    let (num, mult) = match unit {
        's' => (&s[..s.len() - 1], 1.0),
        'm' => (&s[..s.len() - 1], 60.0),
        'h' => (&s[..s.len() - 1], 3600.0),
        'd' => (&s[..s.len() - 1], 86400.0),
        _ => (s, 1.0),
    };
    if let Ok(v) = num.parse::<f64>()
        && v.is_finite()
        && v >= 0.0
    {
        return Ok(SystemTime::now().checked_sub(Duration::from_secs_f64(v * mult)));
    }
    if let Some(t) = openreadout_core::time::iso8601_to_unix(s) {
        return Ok(SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs_f64(t.max(0.0))));
    }
    Err(Error::Usage(format!(
        "--since '{s}': expected `all`, a duration such as 90s, 10m, 2h, 1d, or an ISO-8601 time"
    )))
}

/// Every plane of `images`, time-major then channel then z (the usual acquisition order).
pub fn all_planes(images: &[openreadout_core::ImageInfo]) -> Vec<(u32, PlaneIndex)> {
    let mut out = Vec::new();
    for im in images {
        for t in 0..im.size_t {
            for c in 0..im.size_c {
                for z in 0..im.size_z {
                    out.push((im.index, PlaneIndex { c, z, t }));
                }
            }
        }
    }
    out
}

/// Keeps the last events of a watcher for clients that poll with a cursor (the MCP tool).
#[derive(Debug)]
pub struct EventLog {
    events: std::collections::VecDeque<WatchEvent>,
    cap: usize,
}

impl EventLog {
    /// A log keeping at most `cap` events.
    pub fn new(cap: usize) -> Self {
        EventLog {
            events: std::collections::VecDeque::new(),
            cap: cap.max(1),
        }
    }

    /// Append an event, dropping the oldest beyond the cap.
    pub fn push(&mut self, e: WatchEvent) {
        self.events.push_back(e);
        while self.events.len() > self.cap {
            self.events.pop_front();
        }
    }

    /// Events with `seq > cursor`, at most `limit`; and whether older events after the cursor
    /// were already dropped.
    pub fn since(&self, cursor: u64, limit: usize) -> (Vec<WatchEvent>, bool) {
        let dropped = self.events.front().is_some_and(|e| e.seq > cursor + 1);
        (
            self.events
                .iter()
                .filter(|e| e.seq > cursor)
                .take(limit)
                .cloned()
                .collect(),
            dropped,
        )
    }
}
