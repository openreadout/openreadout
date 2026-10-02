//! Running a [`Measure`] over many inputs.
//!
//! 1. **Inputs.** Paths, directories (with `recursive`, their sub-directories), glob patterns
//!    and the data sets an index query selects are expanded in a deterministic order
//!    ([`openreadout_core::batch::expand`]; a directory a reader recognises, such as a Bruker
//!    `.d`, is one input). Excluded paths (the sample sheet itself) are left out.
//! 2. **Probe** (parallel). Every input is opened for its headers: format, the files it names
//!    ([`openreadout_core::Dataset::member_files`]) and the sample fields of its experiment.
//!    A file found by walking a directory that no reader recognises is skipped and counted;
//!    one named explicitly becomes an error row.
//! 3. **Group.** Files that name each other form one multi-file data set, whose primary is
//!    chosen like the index does ([`openreadout_index::group`]); the others are not measured
//!    again and are reported as members.
//! 4. **Measure** (parallel, results in input order). A failure is an error row, never an
//!    abort. With a journal, every finished data set is appended to it and a rerun reuses the
//!    rows of unchanged data sets measured with the same options.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use openreadout_core::batch::{ExpandOptions, expand};
use openreadout_core::{Error, Registry, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::measure::{Item, Measure};
use crate::table::{ColumnDoc, Role, Row, Table, Value, assemble};

/// Data sets an index query selects (`openreadout search INDEX "QUERY"`).
#[derive(Debug, Clone, Default)]
pub struct IndexQuery {
    /// The index directory.
    pub index_dir: PathBuf,
    /// The query (`format=fcs channel~CD4`); empty selects every data set.
    pub query: String,
}

/// What to run over.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct BatchRequest {
    /// Files, directories and glob patterns.
    pub inputs: Vec<PathBuf>,
    /// Walk sub-directories of directory inputs.
    pub recursive: bool,
    /// Add the data sets an index query selects.
    pub index: Option<IndexQuery>,
    /// Paths never measured (sample sheets and layouts passed alongside).
    pub exclude: Vec<PathBuf>,
    /// Only data sets of these format ids (`fcs`, `czi`, …); others are skipped and counted.
    pub formats: Vec<String>,
    /// Leave out explicitly named inputs no reader recognises (walked ones always are).
    pub skip_unknown: bool,
    /// Stop at the first failure (an error is returned instead of a table).
    pub fail_fast: bool,
    /// Append finished data sets here, and reuse the rows of unchanged ones found in it.
    pub journal: Option<PathBuf>,
}

/// Sample fields a data set records (from its experiment model), used to join sample sheets.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SampleKeys {
    /// `experiment.sample.id`.
    pub id: Option<String>,
    /// `experiment.sample.name`.
    pub name: Option<String>,
    /// `experiment.sample.well`.
    pub well: Option<String>,
    /// `experiment.sample.barcode`.
    pub barcode: Option<String>,
    /// `experiment.sample.sequence_position`.
    pub position: Option<String>,
    /// `experiment.acquisition.started_at`.
    pub started_at: Option<String>,
}

/// One data set of the batch (a row source).
#[derive(Debug, Clone, Default)]
pub struct DatasetRef {
    /// The path as expanded (as given, or found under a directory argument).
    pub path: PathBuf,
    /// The path relative to the argument it was found under.
    pub relative: PathBuf,
    /// Format id, when recognised.
    pub format: Option<String>,
    /// Other files of a multi-file data set.
    pub members: Vec<PathBuf>,
    /// Its sample fields.
    pub keys: SampleKeys,
}

/// How the inputs turned into data sets.
#[derive(Debug, Clone, Default, Serialize, schemars::JsonSchema)]
pub struct InputReport {
    /// Data sets measured (rows come from these), including those that failed.
    pub datasets: u64,
    /// Data sets measured without error.
    pub ok: u64,
    /// Data sets that failed (their rows carry `error`).
    pub failed: u64,
    /// Files found by walking a directory that no reader recognises (not in the table).
    pub skipped_unknown: u64,
    /// Data sets of other formats left out by the format filter.
    pub skipped_format: u64,
    /// Sample sheets and layouts found among the inputs (never measured).
    pub skipped_sheets: u64,
    /// Files that belong to another input's multi-file data set (measured once, with it).
    pub grouped_members: u64,
    /// A few of the skipped paths, with why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_examples: Vec<String>,
    /// Data sets whose rows were reused from the journal of an interrupted run.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reused: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(v: &u64) -> bool {
    *v == 0
}

/// A batch run's result: the table and what went into it.
#[derive(Debug, Clone, Default)]
pub struct BatchResult {
    /// Measure id.
    pub measure: String,
    /// The measure's grain keys.
    pub grain: Vec<String>,
    /// Counts.
    pub inputs: InputReport,
    /// The table (identity, keys, metadata, values, errors).
    pub table: Table,
    /// The data sets, in order.
    pub datasets: Vec<DatasetRef>,
    /// For each table row, its data set (index into `datasets`).
    pub row_dataset: Vec<usize>,
    /// Things to know (grouping, skipped inputs).
    pub warnings: Vec<String>,
}

/// Columns the runner adds.
pub fn identity_docs() -> Vec<ColumnDoc> {
    vec![
        ColumnDoc {
            name: "path",
            role: Role::Id,
            unit: None,
            description: "the data set (file or directory data set), as found",
        },
        ColumnDoc {
            name: "format",
            role: Role::Id,
            unit: None,
            description: "format id",
        },
        ColumnDoc {
            name: "files",
            role: Role::Id,
            unit: None,
            description: "files in a multi-file data set (the path plus the members grouped with it)",
        },
        ColumnDoc::meta(
            "sample_id",
            "experiment.sample.id: the sample id the file records",
        ),
        ColumnDoc::meta(
            "sample_well",
            "experiment.sample.well: the plate well the file records",
        ),
        ColumnDoc {
            name: "error",
            role: Role::Error,
            unit: None,
            description: "why this data set has no values (null when it has)",
        },
        ColumnDoc {
            name: "error_code",
            role: Role::Error,
            unit: None,
            description: "unknown_format, corrupt_file, unsupported_feature, io, usage or error",
        },
    ]
}

/// Progress callback: (data sets done, total, current path).
pub type Progress<'a> = &'a (dyn Fn(u64, u64, &str) + Sync);

/// Rows of one data set, handed to a sink in input order as soon as they are ready.
pub type Sink<'a> = &'a (dyn Fn(&DatasetRef, &[Row]) + Sync);

struct Probe {
    ds: DatasetRef,
    explicit: bool,
    size: u64,
    mtime_ns: Option<i128>,
    error: Option<Error>,
    /// Rows of a headers-only measure.
    rows: Option<Result<Vec<Row>>>,
}

fn stamp(p: &Path) -> (u64, Option<i128>) {
    std::fs::metadata(p).map_or((0, None), |m| {
        let t = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos() as i128);
        (m.len(), t)
    })
}

fn keys_of(
    ds: &dyn openreadout_core::Dataset,
    info: &openreadout_core::model::FileInfo,
) -> SampleKeys {
    let e = openreadout_core::experiment::of_dataset(ds, info);
    let s = e.sample.unwrap_or_default();
    SampleKeys {
        id: s.id,
        name: s.name,
        well: s.well,
        barcode: s.barcode,
        position: s.sequence_position,
        started_at: e.acquisition.and_then(|a| a.started_at),
    }
}

fn probe(
    reg: &Registry,
    measure: &dyn Measure,
    path: &Path,
    relative: &Path,
    explicit: bool,
) -> Probe {
    let (size, mtime_ns) = stamp(path);
    let mut p = Probe {
        ds: DatasetRef {
            path: path.to_path_buf(),
            relative: relative.to_path_buf(),
            ..Default::default()
        },
        explicit,
        size,
        mtime_ns,
        error: None,
        rows: None,
    };
    let (det, mut ds) = match reg.open(path) {
        Ok(x) => x,
        Err(e) => {
            if !matches!(e, Error::UnknownFormat { .. }) {
                p.ds.format = reg.detect(path).ok().map(|(_, d)| d.format_id.to_string());
            }
            p.error = Some(e);
            return p;
        }
    };
    p.ds.format = Some(det.format_id.to_string());
    let info = match ds.info() {
        Ok(i) => i,
        Err(e) => {
            p.error = Some(e);
            return p;
        }
    };
    p.ds.members = ds.member_files();
    p.ds.keys = keys_of(ds.as_ref(), &info);
    if measure.headers_only() {
        p.rows = Some(measure.rows(&mut Item {
            path,
            registry: reg,
            dataset: ds.as_mut(),
            info: &info,
        }));
    }
    p
}

fn measure_one(reg: &Registry, measure: &dyn Measure, path: &Path) -> Result<Vec<Row>> {
    let (_, mut ds) = reg.open(path)?;
    let info = ds.info()?;
    measure.rows(&mut Item {
        path,
        registry: reg,
        dataset: ds.as_mut(),
        info: &info,
    })
}

#[derive(Serialize, Deserialize)]
struct JournalEntry {
    path: String,
    size: u64,
    mtime_ns: Option<i128>,
    fingerprint: String,
    #[serde(default)]
    rows: Vec<Vec<(String, Value)>>,
    #[serde(default)]
    error: Option<(String, String)>,
}

fn read_journal(p: &Path) -> HashMap<String, JournalEntry> {
    let mut out = HashMap::new();
    let Ok(f) = std::fs::File::open(p) else {
        return out;
    };
    for line in std::io::BufReader::new(f)
        .lines()
        .map_while(std::result::Result::ok)
    {
        if let Ok(e) = serde_json::from_str::<JournalEntry>(&line) {
            out.insert(e.path.clone(), e);
        }
    }
    out
}

/// An error rebuilt from a journal entry (code and message only).
fn journal_error(code: &str, message: &str) -> Error {
    match code {
        "corrupt_file" => Error::corrupt("batch", message.to_string()),
        "unsupported_feature" => Error::Unsupported {
            format: "batch",
            feature: message.to_string(),
            hint: None,
        },
        "usage" => Error::Usage(message.to_string()),
        _ => Error::Other(message.to_string()),
    }
}

fn norm(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Run `measure` over the request's inputs (see the module documentation).
pub fn run(
    reg: &Registry,
    measure: &dyn Measure,
    req: &BatchRequest,
    progress: Option<Progress<'_>>,
    sink: Option<Sink<'_>>,
) -> Result<BatchResult> {
    // 1. inputs
    let mut args = req.inputs.clone();
    if let Some(q) = &req.index {
        let mut sr = openreadout_index::SearchRequest::default();
        sr.query = q.query.clone();
        sr.limit = Some(0);
        sr.fields = vec!["path".into()];
        let hits = openreadout_index::search(&q.index_dir, &sr)?;
        for r in hits.results {
            if let Some(p) = r.get("path").and_then(|v| v.as_str()) {
                args.push(PathBuf::from(p));
            }
        }
        if args.is_empty() {
            return Err(Error::Usage(format!(
                "the index query `{}` selects no data set",
                q.query
            )));
        }
    }
    if args.is_empty() {
        return Err(Error::Usage("no input given".into()));
    }
    let exp = expand(reg, &args, {
        let mut o = ExpandOptions::default();
        o.recursive = req.recursive;
        o.glob = true;
        o.dataset_dir = Some(openreadout_index::walk::is_dataset_dir);
        o
    });
    let excluded: HashSet<PathBuf> = req.exclude.iter().map(|p| norm(p)).collect();
    let named: HashSet<PathBuf> = args.iter().map(|p| norm(p)).collect();
    let mut report = InputReport::default();
    let mut warnings = Vec::new();
    let mut items = Vec::new();
    let mut expansion_errors = Vec::new();
    for it in exp.items {
        if let Some(e) = it.error {
            expansion_errors.push((it.path, it.relative, e));
            continue;
        }
        let n = norm(&it.path);
        if excluded.contains(&n) {
            report.skipped_sheets += 1;
            continue;
        }
        let explicit = named.contains(&n);
        items.push((it.path, it.relative, explicit));
    }
    // 2. probe
    let total = items.len() as u64;
    let done = AtomicU64::new(0);
    let stop = AtomicBool::new(false);
    // Data sets run on a pool of their own: a measure may itself read planes in parallel from a
    // helper thread on the global pool and wait for it (`openreadout_core::parallel`); if the
    // data sets ran on the global pool too, every worker could end up waiting on work only a
    // worker can do.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(rayon::current_num_threads().max(1))
        .thread_name(|i| format!("batch-{i}"))
        .build()
        .map_err(|e| Error::Other(format!("cannot start the batch thread pool: {e}")))?;
    let probes: Vec<Probe> = pool.install(|| {
        items
            .par_iter()
            .map(|(p, rel, explicit)| {
                let r = probe(reg, measure, p, rel, *explicit);
                if let Some(cb) = progress {
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if measure.headers_only() {
                        cb(d, total, &p.display().to_string());
                    }
                }
                r
            })
            .collect()
    });
    // skip unknown walked files, filtered formats
    let mut kept: Vec<Probe> = Vec::new();
    for p in probes {
        let unknown = matches!(p.error, Some(Error::UnknownFormat { .. }));
        if unknown && (!p.explicit || req.skip_unknown) {
            report.skipped_unknown += 1;
            if report.skipped_examples.len() < 10 {
                let what = crate::companion::classify(&p.ds.path)
                    .map_or("not an instrument file", |k| k.describe());
                report
                    .skipped_examples
                    .push(format!("{} ({what})", p.ds.path.display()));
            }
            continue;
        }
        if !req.formats.is_empty()
            && !p
                .ds
                .format
                .as_ref()
                .is_some_and(|f| req.formats.iter().any(|w| w.eq_ignore_ascii_case(f)))
            && (!unknown || !p.explicit)
        {
            report.skipped_format += 1;
            if report.skipped_examples.len() < 10 {
                report.skipped_examples.push(format!(
                    "{} (format {})",
                    p.ds.path.display(),
                    p.ds.format.as_deref().unwrap_or("unknown")
                ));
            }
            continue;
        }
        kept.push(p);
    }
    // 3. group multi-file data sets
    let claims: Vec<openreadout_index::group::Claim> = kept
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.ds.members.is_empty())
        .map(|(i, p)| openreadout_index::group::Claim {
            path: norm(&p.ds.path).to_string_lossy().into_owned(),
            ok: p.error.is_none(),
            members: p
                .ds
                .members
                .iter()
                .map(|m| norm(m).to_string_lossy().into_owned())
                .collect(),
            seq: i as u64,
        })
        .collect();
    let member_of = openreadout_index::group::primaries(&claims);
    let mut grouped: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut primaries: Vec<Probe> = Vec::new();
    for p in kept {
        let key = norm(&p.ds.path).to_string_lossy().into_owned();
        if let Some(primary) = member_of.get(&key) {
            report.grouped_members += 1;
            grouped
                .entry(primary.clone())
                .or_default()
                .push(p.ds.path.clone());
            continue;
        }
        primaries.push(p);
    }
    if report.grouped_members > 0 {
        warnings.push(format!(
            "{} file(s) belong to multi-file data sets and were measured once with them (see the `files` column)",
            report.grouped_members
        ));
    }
    // 4. measure
    let journal = req.journal.as_ref().map(|j| read_journal(j));
    let journal_out = match &req.journal {
        Some(j) => Some(Mutex::new(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(j)
                .map_err(|e| Error::io(j, e))?,
        )),
        None => None,
    };
    let fp = measure.fingerprint();
    let reused = AtomicU64::new(0);
    done.store(0, Ordering::Relaxed);
    let total = primaries.len() as u64;
    // in-order delivery to the sink
    let pending: Mutex<(usize, BTreeMap<usize, Vec<Row>>)> = Mutex::new((0, BTreeMap::new()));
    let with_identity = |p: &Probe, rows: Result<Vec<Row>>| -> Vec<Row> {
        let files = 1 + member_count(&p.ds, &grouped);
        let ident = |mut r: Row| -> Row {
            let mut out = Row::new()
                .with("path", p.ds.path.display().to_string())
                .with("format", Value::text_opt(p.ds.format.as_deref()));
            if files > 1 {
                out.set("files", files as u64);
            }
            for (n, v) in r.cells.drain(..) {
                out.cells.push((n, v));
            }
            if out.get("sample_id").is_none() && p.ds.keys.id.is_some() {
                out.set("sample_id", Value::text_opt(p.ds.keys.id.as_deref()));
            }
            if out.get("well").is_none()
                && out.get("sample_well").is_none()
                && p.ds.keys.well.is_some()
            {
                out.set("sample_well", Value::text_opt(p.ds.keys.well.as_deref()));
            }
            out
        };
        match rows {
            Ok(rows) if !rows.is_empty() => rows.into_iter().map(ident).collect(),
            Ok(_) => vec![ident(Row::new())],
            Err(e) => vec![
                ident(Row::new())
                    .with("error", e.to_string())
                    .with("error_code", e.code()),
            ],
        }
    };
    let outcomes: Vec<(Vec<Row>, bool)> = pool.install(|| {
        primaries
            .par_iter()
            .enumerate()
            .map(|(i, p)| {
                if stop.load(Ordering::Relaxed) {
                    return (Vec::new(), false);
                }
                let path_s = p.ds.path.display().to_string();
                let rows: Result<Vec<Row>> = if let Some(e) = &p.error {
                    Err(clone_error(e))
                } else if let Some(r) = &p.rows {
                    match r {
                        Ok(rows) => Ok(rows.clone()),
                        Err(e) => Err(clone_error(e)),
                    }
                } else if let Some(prev) = journal
                    .as_ref()
                    .and_then(|j| j.get(&path_s))
                    .filter(|e| e.size == p.size && e.mtime_ns == p.mtime_ns && e.fingerprint == fp)
                {
                    reused.fetch_add(1, Ordering::Relaxed);
                    match &prev.error {
                        Some((code, msg)) => Err(journal_error(code, msg)),
                        None => Ok(prev
                            .rows
                            .iter()
                            .map(|cells| Row {
                                cells: cells.clone(),
                            })
                            .collect()),
                    }
                } else {
                    let r = measure_one(reg, measure, &p.ds.path);
                    if let Some(j) = &journal_out {
                        let entry = JournalEntry {
                            path: path_s.clone(),
                            size: p.size,
                            mtime_ns: p.mtime_ns,
                            fingerprint: fp.clone(),
                            rows: r
                                .as_ref()
                                .map(|rows| rows.iter().map(|r| r.cells.clone()).collect())
                                .unwrap_or_default(),
                            error: r
                                .as_ref()
                                .err()
                                .map(|e| (e.code().to_string(), e.to_string())),
                        };
                        if let (Ok(line), Ok(mut f)) = (serde_json::to_string(&entry), j.lock()) {
                            let _ = writeln!(f, "{line}");
                            let _ = f.flush();
                        }
                    }
                    r
                };
                let failed = rows.is_err();
                if failed && req.fail_fast {
                    stop.store(true, Ordering::Relaxed);
                }
                let rows = with_identity(p, rows);
                if let Some(cb) = progress {
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    cb(d, total, &path_s);
                }
                if let Some(s) = sink
                    && let Ok(mut g) = pending.lock()
                {
                    g.1.insert(i, rows.clone());
                    loop {
                        let next = g.0;
                        let Some(r) = g.1.remove(&next) else { break };
                        s(&primaries[next].ds, &r);
                        g.0 += 1;
                    }
                }
                (rows, !failed)
            })
            .collect()
    });
    if req.fail_fast
        && let Some((i, _)) = outcomes.iter().enumerate().find(|(_, (_, ok))| !ok)
    {
        let p = &primaries[i];
        let msg = outcomes[i]
            .0
            .first()
            .and_then(|r| r.get("error"))
            .map(Value::text)
            .unwrap_or_default();
        return Err(match &p.error {
            Some(e) => clone_error(e),
            None => Error::Other(format!("{}: {msg}", p.ds.path.display())),
        });
    }
    report.reused = reused.load(Ordering::Relaxed);
    // expansion errors (missing paths, bad globs) become error rows at the end
    let mut all_rows = Vec::new();
    let mut row_dataset = Vec::new();
    let mut datasets = Vec::new();
    for (p, (rows, ok)) in primaries.iter().zip(outcomes) {
        report.datasets += 1;
        if ok {
            report.ok += 1;
        } else {
            report.failed += 1;
        }
        let mut d = p.ds.clone();
        if let Some(m) = grouped.get(&norm(&p.ds.path).to_string_lossy().into_owned()) {
            for x in m {
                if !d.members.contains(x) {
                    d.members.push(x.clone());
                }
            }
        }
        for r in rows {
            all_rows.push(r);
            row_dataset.push(datasets.len());
        }
        datasets.push(d);
    }
    for (path, relative, e) in expansion_errors {
        report.datasets += 1;
        report.failed += 1;
        all_rows.push(
            Row::new()
                .with("path", path.display().to_string())
                .with("format", Value::Null)
                .with("error", e.to_string())
                .with("error_code", e.code()),
        );
        row_dataset.push(datasets.len());
        datasets.push(DatasetRef {
            path,
            relative,
            ..Default::default()
        });
    }
    if report.skipped_unknown > 0 {
        warnings.push(format!(
            "{} file(s) found in directories are not instrument files and were left out",
            report.skipped_unknown
        ));
    }
    let mut docs = identity_docs();
    docs.extend(measure.columns());
    let grain = measure.grain();
    let table = assemble(all_rows, &docs, &|n| {
        if grain.iter().any(|g| g == n) {
            Role::Key
        } else {
            Role::Value
        }
    });
    Ok(BatchResult {
        measure: measure.id().to_string(),
        grain,
        inputs: report,
        table,
        datasets,
        row_dataset,
        warnings,
    })
}

fn member_count(d: &DatasetRef, grouped: &BTreeMap<String, Vec<PathBuf>>) -> usize {
    let key = norm(&d.path).to_string_lossy().into_owned();
    let mut all: HashSet<String> = d
        .members
        .iter()
        .map(|m| norm(m).to_string_lossy().into_owned())
        .collect();
    if let Some(g) = grouped.get(&key) {
        all.extend(g.iter().map(|m| norm(m).to_string_lossy().into_owned()));
    }
    all.remove(&key);
    all.len()
}

/// Errors are not `Clone`; rebuild one with the same code and message.
pub fn clone_error(e: &Error) -> Error {
    match e {
        Error::UnknownFormat { path } => Error::UnknownFormat { path: path.clone() },
        Error::Unsupported {
            format,
            feature,
            hint,
        } => Error::Unsupported {
            format,
            feature: feature.clone(),
            hint: hint.clone(),
        },
        Error::Corrupt {
            format,
            detail,
            offset,
        } => Error::Corrupt {
            format,
            detail: detail.clone(),
            offset: *offset,
        },
        Error::Usage(m) => Error::Usage(m.clone()),
        Error::Io { path, source } => Error::io(
            path.clone(),
            std::io::Error::new(source.kind(), source.to_string()),
        ),
        other => Error::Other(other.to_string()),
    }
}
