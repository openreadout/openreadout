//! Turning the journal into the tables: multi-file grouping, series, moves and totals.
//!
//! Two sequential passes over the journal (plus, for a crawl stopped early, the previous run's
//! records after the last item read):
//!
//! 1. Collect what grouping needs, which is small: the member lists of data sets that name
//!    other files (multi-file OME-TIFF, CZI parts, OIR continuations, SpikeGLX `.meta`/`.bin`,
//!    imzML `.ibd`, VSI `.ets`, LIF sidecars), the series keys of stream recordings (SpikeGLX
//!    runs, Blackrock `.nsX` + `.nev`), and the previous run's records that are gone (for
//!    moves). Files linked by member lists form one component; its data set is the member
//!    that opened cleanly and names the most files (first in walk order on a tie). Streams of
//!    one series join the first stream's data set, which adds their counts.
//! 2. Write the rows: one `experiments` row per data set, a `files` row per file (members point
//!    at their data set), `problems` rows; count totals for `index.json`.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use openreadout_core::{Error, Result};

use crate::crawl::{Line, RunState, TOOL_VERSION, now_iso};
use crate::manifest::{
    CrawlStats, EXPERIMENTS_FILE, FILES_FILE, INDEX_SCHEMA_VERSION, IndexManifest, PROBLEMS_FILE,
    PiiTotals, TableManifest, Tally, ToolInfo,
};
use crate::record::{Change, ItemKind, Record};
use crate::tables::{
    Additions, EXPERIMENTS, FILES, PROBLEMS, TableWriter, experiment_row, file_row,
    micros_from_iso, problem_row, year_of,
};
use crate::walk::item_key;

/// Call `f` with every journal line, then with the records of `tail` (a previous
/// `records.jsonl`) that come after `tail_from` in walk order.
fn for_each_line(
    journal: &Path,
    tail: Option<&Path>,
    tail_from: Option<&str>,
    f: &mut dyn FnMut(Line) -> Result<()>,
) -> Result<()> {
    let read = |p: &Path,
                f: &mut dyn FnMut(Line) -> Result<()>,
                after: Option<&crate::walk::Key>|
     -> Result<()> {
        let Ok(file) = File::open(p) else {
            return Ok(());
        };
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|e| Error::io(p, e))?;
            if line.is_empty() {
                continue;
            }
            let Ok(l) = serde_json::from_str::<Line>(&line) else {
                continue; // a torn line
            };
            if let Some(k) = after {
                match &l {
                    Line::Rec(r) if item_key(Path::new(&r.path)) > *k => {}
                    _ => continue,
                }
            }
            f(l)?;
        }
        Ok(())
    };
    read(journal, f, None)?;
    if let Some(t) = tail {
        let k = tail_from.map_or_else(Vec::new, |p| item_key(Path::new(p)));
        read(t, f, Some(&k))?;
    }
    Ok(())
}

struct SeriesNode {
    key: String,
    path: String,
    seq: u64,
    ok: bool,
    add: Additions,
}

/// Grouping decided in pass 1.
#[derive(Default)]
struct Grouping {
    member_of: HashMap<String, String>,
    additions: HashMap<String, Additions>,
    gone: HashMap<(u64, String), String>,
}

fn group(journal: &Path, tail: Option<&Path>, tail_from: Option<&str>) -> Result<Grouping> {
    let mut claims: Vec<crate::group::Claim> = Vec::new();
    let mut series: Vec<SeriesNode> = Vec::new();
    let mut g = Grouping::default();
    let mut seq = 0u64;
    for_each_line(journal, tail, tail_from, &mut |l| {
        match l {
            Line::Rec(r) => {
                seq += 1;
                let ok = r.error.is_none();
                if r.is_dataset() && r.kind == ItemKind::File && !r.members.is_empty() && ok {
                    claims.push(crate::group::Claim {
                        path: r.path.clone(),
                        ok,
                        members: r.members.iter().map(|m| m.path.clone()).collect(),
                        seq,
                    });
                }
                if let Some(k) = &r.series_key {
                    series.push(SeriesNode {
                        key: k.clone(),
                        path: r.path.clone(),
                        seq,
                        ok,
                        add: Additions {
                            bytes: r.total_size(),
                            files: 1 + r.members.len() as u64,
                            mtime_us: r.mtime_us,
                            summary: Some(r.summary.clone()),
                        },
                    });
                }
            }
            Line::Gone {
                path,
                size,
                fingerprint: Some(fp),
            } => {
                g.gone.insert((size, fp), path);
            }
            _ => {}
        }
        Ok(())
    })?;
    // Components of the member graph.
    g.member_of = crate::group::primaries(&claims);
    // Series of streams.
    let mut by_key: BTreeMap<String, Vec<SeriesNode>> = BTreeMap::new();
    for s in series {
        if g.member_of.contains_key(&s.path) {
            continue;
        }
        by_key.entry(s.key.clone()).or_default().push(s);
    }
    for (_, mut nodes) in by_key {
        if nodes.len() < 2 {
            continue;
        }
        nodes.sort_by_key(|n| (!n.ok, n.seq));
        let primary = nodes[0].path.clone();
        for n in nodes.into_iter().skip(1) {
            let a = g.additions.entry(primary.clone()).or_default();
            a.bytes = a.bytes.saturating_add(n.add.bytes);
            a.files += n.add.files;
            a.mtime_us = match (a.mtime_us, n.add.mtime_us) {
                (Some(x), Some(y)) => Some(x.max(y)),
                (x, y) => x.or(y),
            };
            if let Some(s) = &n.add.summary {
                a.summary.get_or_insert_with(Default::default).add(s);
            }
            g.member_of.insert(n.path, primary.clone());
        }
    }
    // Members of members point at the final data set.
    let keys: Vec<String> = g.member_of.keys().cloned().collect();
    for k in keys {
        let mut target = g.member_of[&k].clone();
        let mut hops = 0;
        while let Some(t) = g.member_of.get(&target) {
            target = t.clone();
            hops += 1;
            if hops > 64 {
                break;
            }
        }
        g.member_of.insert(k, target);
    }
    Ok(g)
}

/// Most distinct unknown extensions listed in `index.json` (the rest are `(other)`).
const MAX_EXTENSIONS: usize = 500;

/// Write the tables and return the manifest (not yet written to disk).
pub(crate) fn finalize(
    index_dir: &Path,
    journal: &Path,
    tail: Option<&Path>,
    tail_from: Option<&str>,
    run: &RunState,
    stats: &CrawlStats,
    complete: bool,
    descriptors: &HashMap<String, (String, String)>,
) -> Result<IndexManifest> {
    let mut g = group(journal, tail, tail_from)?;
    let created_by = format!("openreadout {TOOL_VERSION}");
    let mut ex = TableWriter::create(&index_dir.join(EXPERIMENTS_FILE), EXPERIMENTS, &created_by)?;
    let mut fi = TableWriter::create(&index_dir.join(FILES_FILE), FILES, &created_by)?;
    let mut pr = TableWriter::create(&index_dir.join(PROBLEMS_FILE), PROBLEMS, &created_by)?;
    let mut formats: BTreeMap<String, Tally> = BTreeMap::new();
    let mut families: BTreeMap<String, Tally> = BTreeMap::new();
    let mut years: BTreeMap<String, Tally> = BTreeMap::new();
    let mut unknown: BTreeMap<String, Tally> = BTreeMap::new();
    let mut check_status: BTreeMap<String, u64> = BTreeMap::new();
    let mut problems: BTreeMap<String, u64> = BTreeMap::new();
    let mut changes: BTreeMap<String, u64> = BTreeMap::new();
    let mut pii = PiiTotals::default();
    let mut bytes = 0u64;
    for_each_line(journal, tail, tail_from, &mut |l| {
        match l {
            Line::Rec(r) => {
                let mut r: Record = *r;
                if let Some((name, vendor)) = r.format.as_ref().and_then(|f| descriptors.get(f)) {
                    r.format_name = Some(name.clone());
                    r.format_vendor = Some(vendor.clone());
                }
                bytes = bytes.saturating_add(r.size);
                if let Some(primary) = g.member_of.get(&r.path) {
                    fi.push(&file_row(
                        &r.path,
                        Some(primary),
                        "member",
                        r.kind,
                        r.size,
                        r.mtime_us,
                        r.format.as_deref(),
                        r.fingerprint.as_deref(),
                        r.change.as_str(),
                    ))?;
                    return Ok(());
                }
                if !r.is_dataset() {
                    fi.push(&file_row(
                        &r.path,
                        None,
                        "unknown",
                        r.kind,
                        r.size,
                        r.mtime_us,
                        None,
                        r.fingerprint.as_deref(),
                        r.change.as_str(),
                    ))?;
                    let ext = if r.ext.is_empty() {
                        "(none)".to_string()
                    } else {
                        r.ext.clone()
                    };
                    let key = if unknown.len() >= MAX_EXTENSIONS && !unknown.contains_key(&ext) {
                        "(other)".to_string()
                    } else {
                        ext
                    };
                    unknown.entry(key).or_default().add(r.size);
                    return Ok(());
                }
                if r.change == Change::New
                    && let Some(fp) = &r.fingerprint
                    && let Some(old) = g.gone.remove(&(r.total_size(), fp.clone()))
                {
                    r.change = Change::Moved;
                    r.moved_from = Some(old);
                }
                let add = g.additions.get(&r.path);
                let row = experiment_row(&r, add);
                let size = match &row[13] {
                    crate::tables::Cell::U64(v) => *v,
                    _ => r.total_size(),
                };
                ex.push(&row)?;
                let format = r.format.clone().unwrap_or_default();
                formats.entry(format.clone()).or_default().add(size);
                families
                    .entry(r.family.clone().unwrap_or_default())
                    .or_default()
                    .add(size);
                let started = r
                    .experiment
                    .as_ref()
                    .and_then(|e| e.acquisition.as_ref())
                    .and_then(|a| a.started_at.as_deref())
                    .or(r.summary.acquired_at.as_deref())
                    .and_then(micros_from_iso);
                years
                    .entry(started.map_or_else(|| "unknown".into(), |t| year_of(t).to_string()))
                    .or_default()
                    .add(size);
                *check_status
                    .entry(r.check_status().to_string())
                    .or_default() += 1;
                *changes.entry(r.change.as_str().to_string()).or_default() += 1;
                fi.push(&file_row(
                    &r.path,
                    Some(&r.path),
                    "dataset",
                    r.kind,
                    r.total_size(),
                    r.mtime_us,
                    r.format.as_deref(),
                    r.fingerprint.as_deref(),
                    r.change.as_str(),
                ))?;
                if r.kind == ItemKind::Directory {
                    for m in &r.members {
                        fi.push(&file_row(
                            &m.path,
                            Some(&r.path),
                            "member",
                            ItemKind::File,
                            m.size,
                            m.mtime_us,
                            r.format.as_deref(),
                            None,
                            r.change.as_str(),
                        ))?;
                    }
                }
                let f = r.format.as_deref();
                if let Some(e) = &r.error {
                    pr.push(&problem_row(
                        &r.path,
                        f,
                        "readability",
                        "error",
                        &e.code,
                        &e.message,
                        None,
                        None,
                    ))?;
                    *problems.entry("readability".into()).or_default() += 1;
                } else if let Some(c) = &r.check {
                    for x in &c.findings {
                        if x.severity == "info" {
                            continue;
                        }
                        pr.push(&problem_row(
                            &r.path,
                            f,
                            "integrity",
                            &x.severity,
                            &x.code,
                            &x.message,
                            None,
                            None,
                        ))?;
                        *problems.entry("integrity".into()).or_default() += 1;
                    }
                }
                if !r.pii.is_empty() {
                    pii.datasets_flagged += 1;
                }
                for p in &r.pii {
                    pii.flags += 1;
                    *pii.by_kind.entry(p.kind.clone()).or_default() += 1;
                    pr.push(&problem_row(
                        &r.path,
                        f,
                        "pii",
                        "review",
                        &p.kind,
                        &format!(
                            "possible personal data ({}) in {}",
                            p.kind.replace('_', " "),
                            p.field
                        ),
                        Some(&p.field),
                        Some(&p.rule),
                    ))?;
                    *problems.entry("pii".into()).or_default() += 1;
                }
            }
            Line::WalkError { path, message } => {
                pr.push(&problem_row(
                    &path, None, "walk", "error", "walk", &message, None, None,
                ))?;
                *problems.entry("walk".into()).or_default() += 1;
            }
            Line::Gone { .. } | Line::Ckpt { .. } => {}
        }
        Ok(())
    })?;
    changes.insert("removed".into(), stats.removed);
    let datasets = ex.rows();
    let files = fi.rows();
    let n_ex = ex.finish()?;
    let n_fi = fi.finish()?;
    let n_pr = pr.finish()?;
    let tables = BTreeMap::from([
        (
            "experiments".to_string(),
            TableManifest {
                file: EXPERIMENTS_FILE.into(),
                rows: n_ex,
                columns: EXPERIMENTS.to_vec(),
            },
        ),
        (
            "files".to_string(),
            TableManifest {
                file: FILES_FILE.into(),
                rows: n_fi,
                columns: FILES.to_vec(),
            },
        ),
        (
            "problems".to_string(),
            TableManifest {
                file: PROBLEMS_FILE.into(),
                rows: n_pr,
                columns: PROBLEMS.to_vec(),
            },
        ),
    ]);
    g.gone.clear();
    Ok(IndexManifest {
        schema_version: INDEX_SCHEMA_VERSION.into(),
        tool: ToolInfo {
            name: "openreadout".into(),
            version: TOOL_VERSION.into(),
        },
        index_dir: index_dir.to_string_lossy().into_owned(),
        roots: run.roots.clone(),
        settings: run.settings.clone(),
        complete,
        started_at: run.started_at.clone(),
        updated_at: now_iso(),
        tables,
        datasets,
        files,
        bytes,
        formats,
        families,
        years,
        unknown_extensions: unknown,
        check_status,
        problems,
        pii,
        changes,
        crawl: stats.clone(),
        files_per_s: if stats.elapsed_s > 0.0 {
            (stats.items as f64 / stats.elapsed_s * 10.0).round() / 10.0
        } else {
            0.0
        },
        next: None,
    })
}
