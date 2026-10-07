//! Recording sessions: the files of one recording, written side by side in a directory by the
//! acquisition software (a Neuralynx session folder, a SpikeGLX run folder, a Blackrock NSx + NEV
//! set, an Intan "one file per channel" folder), opened as one dataset.
//!
//! A format crate lists the data files of the directory ([`session_files`]), opens each with its
//! single-file reader and hands them to [`SessionDataset::new`]. Nothing is copied, resampled or
//! realigned: every member keeps its own clock, and continuous channels are combined into one
//! multi-channel trace only when their sample grids are identical (same rate, same number of
//! sweeps, same sweep lengths and same sweep start stamps). Members' tables are appended in file
//! order. Every trace, channel and table records the file it came from in `extra.source_file`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::model::{CheckReport, FileInfo, LsEntry, Table, Trace, TraceInfo};
use crate::pixel::Plane;
use crate::provenance::ProvenanceMap;
use crate::reader::{Dataset, PlaneIndex};
use crate::source::{DirEntry, Fs};

/// Most directory entries looked at while listing a session (a guard against being pointed at a
/// huge directory tree).
pub const MAX_ENTRIES: usize = 100_000;

/// Most member files in one session.
pub const MAX_MEMBERS: usize = 4096;

/// Regular files under `dir` (itself and, when `depth > 0`, subdirectories down to `depth`
/// levels) for which `keep` is true, sorted by path. Symbolic links to directories are not
/// followed; hidden files (leading `.`) and `.ok` / `.openreadout.json` side files are skipped.
pub fn member_files(
    dir: &Path,
    keep: &dyn Fn(&Path) -> bool,
    depth: usize,
) -> Result<Vec<PathBuf>> {
    member_files_in(&Fs::local(), dir, keep, depth)
}

/// Like [`member_files`], using the supplied file namespace.
pub(crate) fn member_files_in(
    fs: &Fs,
    dir: &Path,
    keep: &dyn Fn(&Path) -> bool,
    depth: usize,
) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut seen = 0usize;
    walk(fs, dir, depth, &mut seen, &mut |p| {
        if keep(p) {
            out.push(p.to_path_buf());
        }
        true
    })?;
    out.sort();
    Ok(out)
}

/// Visit every regular file under `dir` (to `depth` levels); stop early when `f` returns false.
fn walk(
    fs: &Fs,
    dir: &Path,
    depth: usize,
    seen: &mut usize,
    f: &mut dyn FnMut(&Path) -> bool,
) -> Result<bool> {
    let mut entries = Vec::new();
    for entry in fs.read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let entry = entry.map_err(|e| Error::io(dir, e))?;
        *seen += 1;
        if *seen > MAX_ENTRIES {
            return Err(Error::unsupported(
                "session",
                format!("a directory tree of more than {MAX_ENTRIES} entries"),
                "Open one recording directory (or a single file), not a directory of many recordings.",
            ));
        }
        entries.push(entry);
    }
    entries.sort_by_key(DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.ends_with(".ok") || name.ends_with(".openreadout.json") {
            continue;
        }
        let kind = entry.file_type().map_err(|e| Error::io(entry.path(), e))?;
        if kind.is_dir() {
            if depth > 0 && !walk(fs, &entry.path(), depth - 1, seen, f)? {
                return Ok(false);
            }
        } else if kind.is_file() && fs.is_file(&entry.path()) && !f(&entry.path()) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The data files of a recording directory, or `None` when `dir` is not one.
///
/// `dir` is a session when it holds at least one file for which `is_data` is true and every other
/// regular file (down to `depth` levels) has one of the `companion` extensions (log files,
/// settings, video tracker files the reader does not decode, ...). One foreign file (an image, a
/// spreadsheet, another vendor's recording) and the directory is not claimed, so a folder of
/// unrelated files is still walked file by file.
pub fn session_files(
    dir: &Path,
    is_data: &dyn Fn(&Path) -> bool,
    companion: &[&str],
    depth: usize,
) -> Option<Vec<PathBuf>> {
    session_files_in(&Fs::local(), dir, is_data, companion, depth)
}

/// Like [`session_files`], using the supplied file namespace.
pub fn session_files_in(
    fs: &Fs,
    dir: &Path,
    is_data: &dyn Fn(&Path) -> bool,
    companion: &[&str],
    depth: usize,
) -> Option<Vec<PathBuf>> {
    let mut data = Vec::new();
    let mut foreign = false;
    let mut seen = 0usize;
    walk(fs, dir, depth, &mut seen, &mut |p| {
        if is_data(p) {
            data.push(p.to_path_buf());
            data.len() <= MAX_MEMBERS
        } else if crate::reader::has_extension(p, companion) {
            true
        } else {
            foreign = true;
            false
        }
    })
    .ok()?;
    if foreign || data.is_empty() || data.len() > MAX_MEMBERS {
        return None;
    }
    data.sort();
    Some(data)
}

/// True when two traces sample on the same grid: same rate, sweep count, sweep lengths and sweep
/// start stamps (the keys readers use for them in `extra`).
pub(crate) fn same_grid(a: &TraceInfo, b: &TraceInfo) -> bool {
    a.sample_rate_hz.to_bits() == b.sample_rate_hz.to_bits()
        && a.sample_count == b.sample_count
        && a.sweep_count == b.sweep_count
        && a.start_s.map(f64::to_bits) == b.start_s.map(f64::to_bits)
        && [
            "sweep_sample_counts",
            "sweep_start_timestamps_us",
            "sweep_start_timestamps",
            "sweep_starts_s",
            "first_timestamp_us",
            "first_timestamp",
        ]
        .iter()
        .all(|k| a.extra.get(*k) == b.extra.get(*k))
}

/// Where one session trace reads from: `(member, trace index in the member)` per part, channels
/// of the parts concatenated in order.
type TraceSources = Vec<(usize, u32)>;

/// Several files of one recording as one dataset (see the module documentation).
pub struct SessionDataset {
    format: &'static str,
    members: Vec<Box<dyn Dataset>>,
    member_paths: Vec<String>,
    info: FileInfo,
    traces: Vec<TraceSources>,
    tables: Vec<(usize, u32)>,
}

impl std::fmt::Debug for SessionDataset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionDataset")
            .field("format", &self.format)
            .field("members", &self.member_paths)
            .finish_non_exhaustive()
    }
}

/// How members' traces are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    /// Every member trace stays a trace of its own (streams with their own clocks).
    Separate,
    /// Member traces on an identical sample grid become one multi-channel trace.
    SameGrid,
}

impl SessionDataset {
    /// Compose `members` (opened files, in the order their traces and tables are listed) into the
    /// session at `path`. `format` is the format id reported in errors; the session's
    /// `info().format` is the first member's.
    pub fn new(
        format: &'static str,
        path: &Path,
        members: Vec<Box<dyn Dataset>>,
        combine: Combine,
    ) -> Result<Self> {
        let first = members.first().ok_or_else(|| {
            Error::unsupported(
                format,
                "an empty recording directory",
                "Open a directory that holds the recording's data files, or one file.",
            )
        })?;
        let mut info = first.info()?;
        info.path = path.display().to_string();
        info.size_bytes = 0;
        info.traces.clear();
        info.tables.clear();
        info.notes.clear();
        info.plane_count = 0;
        let mut traces: Vec<TraceSources> = Vec::new();
        let mut tables = Vec::new();
        let mut member_paths = Vec::new();
        let mut notes: Vec<(String, Vec<String>)> = Vec::new();
        for (m, ds) in members.iter().enumerate() {
            let child = ds.info()?;
            if !child.images.is_empty() || !child.spectra.is_empty() {
                return Err(Error::unsupported(
                    format,
                    "images or spectra inside a recording session",
                    "Open that file on its own.",
                ));
            }
            let source = relative(path, Path::new(&child.path));
            member_paths.push(source.clone());
            info.size_bytes = info.size_bytes.saturating_add(child.size_bytes);
            for mut t in child.traces {
                let local = t.index;
                for c in &mut t.channels {
                    c.extra.insert("source_file".into(), json!(source));
                }
                let joined = (combine == Combine::SameGrid)
                    .then(|| {
                        info.traces.iter().position(|s| {
                            same_grid(s, &t)
                                && !traces[s.index as usize].iter().any(|(k, _)| *k == m)
                        })
                    })
                    .flatten();
                if let Some(k) = joined {
                    let target = &mut info.traces[k];
                    for mut c in t.channels {
                        c.index = u32::try_from(target.channels.len())
                            .map_err(|_| Error::corrupt(format, "too many channels"))?;
                        target.channels.push(c);
                    }
                    if let Some(Value::Array(files)) = target.extra.get_mut("source_files") {
                        files.push(json!(source));
                    }
                    if target.name != t.name {
                        target.name = None;
                    }
                    traces[k].push((m, local));
                } else {
                    t.index = u32::try_from(traces.len())
                        .map_err(|_| Error::corrupt(format, "too many traces"))?;
                    t.extra.insert("source_files".into(), json!([source]));
                    info.traces.push(t);
                    traces.push(vec![(m, local)]);
                }
            }
            for mut t in child.tables {
                let local = t.index;
                t.index = u32::try_from(tables.len())
                    .map_err(|_| Error::corrupt(format, "too many tables"))?;
                t.extra.insert("source_file".into(), json!(source));
                tables.push((m, local));
                info.tables.push(t);
            }
            for n in child.notes {
                match notes.iter_mut().find(|(t, _)| *t == n) {
                    Some((_, files)) => files.push(source.clone()),
                    None => notes.push((n, vec![source.clone()])),
                }
            }
        }
        for (n, files) in notes {
            info.notes.push(if files.len() == 1 {
                format!("{}: {n}", files[0])
            } else {
                format!("{n} ({})", files.join(", "))
            });
        }
        info.notes.insert(
            0,
            format!(
                "recording session of {} files; each file keeps its own clock and scaling ({})",
                members.len(),
                match combine {
                    Combine::Separate => "one trace per stream",
                    Combine::SameGrid => "channels on an identical sample grid are one trace",
                }
            ),
        );
        Ok(Self {
            format,
            members,
            member_paths,
            info,
            traces,
            tables,
        })
    }

    /// The composed summary, for format crates that add session-level fields.
    pub fn info_mut(&mut self) -> &mut FileInfo {
        &mut self.info
    }

    /// Member files, relative to the session directory, in member order.
    pub fn member_paths(&self) -> &[String] {
        &self.member_paths
    }
}

/// `file` relative to `dir` when it lies inside it (forward slashes), else as given.
fn relative(dir: &Path, file: &Path) -> String {
    file.strip_prefix(dir).map_or_else(
        |_| file.display().to_string(),
        |r| {
            r.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/")
        },
    )
}

impl Dataset for SessionDataset {
    fn info(&self) -> Result<FileInfo> {
        Ok(self.info.clone())
    }

    fn vendor_metadata(&self) -> Result<Value> {
        let mut members = Vec::new();
        for (ds, p) in self.members.iter().zip(&self.member_paths) {
            members.push(json!({"file": p, "metadata": ds.vendor_metadata()?}));
        }
        Ok(json!({ "members": members }))
    }

    fn provenance(&self) -> ProvenanceMap {
        let mut out = ProvenanceMap::new();
        for ds in &self.members {
            for (k, v) in ds.provenance() {
                out.entry(k).or_insert(v);
            }
        }
        out
    }

    fn entries(&self) -> Result<Vec<LsEntry>> {
        let mut out = Vec::new();
        for (ds, p) in self.members.iter().zip(&self.member_paths) {
            let info = ds.info()?;
            out.push(LsEntry {
                kind: "file".into(),
                name: p.clone(),
                offset: None,
                size: Some(info.size_bytes),
                image: None,
                details: json!({
                    "traces": info.traces.len(),
                    "tables": info.tables.len(),
                    "entries": ds.entries()?,
                }),
            });
        }
        Ok(out)
    }

    fn member_files(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn assurance_observations(&self) -> crate::assurance::Observations {
        let mut out = crate::assurance::Observations::default();
        for ds in &self.members {
            out.merge(ds.assurance_observations());
        }
        out
    }

    fn read_plane(&mut self, _image: u32, _index: PlaneIndex) -> Result<Plane> {
        Err(Error::unsupported(
            self.format,
            "image planes",
            "A recording session holds sampled signals and event tables: use `openreadout trace` for signals and `openreadout export --format csv --table N` for tables.",
        ))
    }

    fn read_trace(
        &mut self,
        index: u32,
        sweep: u32,
        first_sample: u64,
        max_samples: u64,
    ) -> Result<Trace> {
        let sources = self.traces.get(index as usize).cloned().ok_or_else(|| {
            Error::Usage(format!(
                "trace {index} out of range (session has {} traces)",
                self.traces.len()
            ))
        })?;
        let mut channels = Vec::new();
        let mut len: Option<usize> = None;
        for (m, local) in sources {
            let t = self.members[m].read_trace(local, sweep, first_sample, max_samples)?;
            for c in t.channels {
                if len.is_some_and(|n| n != c.len()) {
                    return Err(Error::corrupt(
                        self.format,
                        format!(
                            "{}: {} samples where the other files of the trace gave {}",
                            self.member_paths[m],
                            c.len(),
                            len.unwrap_or(0)
                        ),
                    ));
                }
                len = Some(c.len());
                channels.push(c);
            }
        }
        Ok(Trace {
            trace: index,
            sweep,
            first_sample,
            channels,
        })
    }

    fn read_table(&mut self, index: u32, first_row: u64, max_rows: u64) -> Result<Table> {
        let (m, local) = *self.tables.get(index as usize).ok_or_else(|| {
            Error::Usage(format!(
                "table {index} out of range (session has {} tables)",
                self.tables.len()
            ))
        })?;
        let mut t = self.members[m].read_table(local, first_row, max_rows)?;
        t.table = index;
        Ok(t)
    }

    fn check(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.info.path.clone(), self.format);
        r.performed(
            "every member file checked by its own reader (findings prefixed with the file)",
        );
        r.performed("channels combined into one trace share rate, sweep count, sweep lengths and sweep start stamps");
        for (ds, p) in self.members.iter_mut().zip(&self.member_paths) {
            let child = ds.check()?;
            for c in child.checks_performed {
                r.performed(format!("{p}: {c}"));
            }
            for mut f in child.findings {
                f.message = format!("{p}: {}", f.message);
                r.push(f);
            }
        }
        Ok(r)
    }

    fn check_headers(&mut self) -> Result<CheckReport> {
        let mut r = CheckReport::new(self.info.path.clone(), self.format);
        r.performed("every member file's headers checked by its own reader");
        for (ds, p) in self.members.iter_mut().zip(&self.member_paths) {
            let child = ds.check_headers()?;
            for mut f in child.findings {
                f.message = format!("{p}: {}", f.message);
                r.push(f);
            }
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SignalChannelInfo;

    fn trace(rate: f64, n: u64) -> TraceInfo {
        TraceInfo {
            sample_rate_hz: rate,
            sample_count: n,
            sweep_count: 1,
            channels: vec![SignalChannelInfo {
                name: "a".into(),
                dtype: "int16".into(),
                scale: 1.0,
                ..SignalChannelInfo::default()
            }],
            ..TraceInfo::default()
        }
    }

    #[test]
    fn grids() {
        assert!(same_grid(&trace(1000.0, 10), &trace(1000.0, 10)));
        assert!(!same_grid(&trace(1000.0, 10), &trace(1000.0, 11)));
        assert!(!same_grid(&trace(1000.0, 10), &trace(2000.0, 10)));
        let mut a = trace(1000.0, 10);
        a.extra
            .insert("sweep_start_timestamps_us".into(), json!([5]));
        assert!(!same_grid(&a, &trace(1000.0, 10)));
    }

    #[test]
    fn foreign_files_disqualify_a_directory() {
        let d = std::env::temp_dir().join(format!("openreadout-session-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.ncs"), b"x").unwrap();
        std::fs::write(d.join("log.txt"), b"x").unwrap();
        let data = |p: &Path| crate::reader::has_extension(p, &["ncs"]);
        assert_eq!(
            session_files(&d, &data, &["txt"], 0).map(|v| v.len()),
            Some(1)
        );
        std::fs::write(d.join("image.czi"), b"x").unwrap();
        assert!(session_files(&d, &data, &["txt"], 0).is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
