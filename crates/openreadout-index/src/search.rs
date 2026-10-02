//! `search`: run a query ([`crate::query`]) over `experiments.parquet`, streaming.

use std::cmp::Ordering;
use std::path::Path;

use openreadout_core::{Error, Result};
use serde::Serialize;

use crate::manifest::{EXPERIMENTS_FILE, read_manifest};
use crate::query::{Query, sort_key};
use crate::tables::{Cell, EXPERIMENTS, scan_table};

/// Columns returned when no fields are asked for.
pub const DEFAULT_FIELDS: &[&str] = &[
    "path",
    "format",
    "size_bytes",
    "started_at",
    "sample_id",
    "instrument_model",
    "technique_label",
    "channels",
    "objective_magnification",
    "check_status",
    "what",
];

/// Default number of results.
pub const DEFAULT_LIMIT: usize = 50;

/// What to search for.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct SearchRequest {
    /// The query (empty: everything).
    pub query: String,
    /// Sort field (`size`, `-acquired` for descending). Default: walk (path) order.
    pub sort: Option<String>,
    /// Most results returned (`None`: [`DEFAULT_LIMIT`]; `Some(0)`: all).
    pub limit: Option<usize>,
    /// Columns returned (field aliases allowed); empty: [`DEFAULT_FIELDS`]; `all`: every column.
    pub fields: Vec<String>,
}

/// Search results.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SearchOutput {
    /// The index directory searched.
    pub index_dir: String,
    /// The query as given.
    pub query: String,
    /// Whether the index covers every root to the end (see `index.json`).
    pub index_complete: bool,
    /// Data sets matching.
    pub total: u64,
    /// Results returned.
    pub returned: usize,
    /// True when `total` > `returned`.
    pub truncated: bool,
    /// The columns of each result.
    pub fields: Vec<String>,
    /// One object per matching data set, with the requested columns.
    pub results: Vec<serde_json::Map<String, serde_json::Value>>,
}

fn cmp_cells(a: &Cell, b: &Cell) -> Ordering {
    let num = |c: &Cell| match c {
        Cell::U64(v) => Some(*v as f64),
        Cell::I64(v) | Cell::Time(v) => Some(*v as f64),
        Cell::F64(v) => Some(*v),
        _ => None,
    };
    match (a, b) {
        (Cell::Null, Cell::Null) => Ordering::Equal,
        (Cell::Null, _) => Ordering::Greater, // nulls last
        (_, Cell::Null) => Ordering::Less,
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            _ => a.to_text().to_lowercase().cmp(&b.to_text().to_lowercase()),
        },
    }
}

/// Resolve requested fields to column names.
pub fn output_fields(fields: &[String]) -> Result<Vec<&'static str>> {
    if fields.is_empty() {
        return Ok(DEFAULT_FIELDS.to_vec());
    }
    if fields.len() == 1 && fields[0] == "all" {
        return Ok(EXPERIMENTS.iter().map(|c| c.name).collect());
    }
    let mut out = Vec::new();
    for f in fields.iter().flat_map(|f| f.split(',')) {
        let f = f.trim();
        if f.is_empty() {
            continue;
        }
        let cols = crate::query::resolve_field(f)
            .ok_or_else(|| Error::Usage(format!("unknown field `{f}`")))?;
        for c in cols {
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    Ok(out)
}

/// Search the index in `index_dir`.
pub fn search(index_dir: &Path, req: &SearchRequest) -> Result<SearchOutput> {
    let manifest = read_manifest(index_dir)?;
    let q = Query::parse(&req.query)?;
    let fields = output_fields(&req.fields)?;
    let sort = req.sort.as_deref().map(sort_key).transpose()?;
    let limit = match req.limit {
        None => DEFAULT_LIMIT,
        Some(0) => usize::MAX,
        Some(n) => n,
    };
    let mut cols: Vec<&str> = q.columns();
    for f in &fields {
        if !cols.contains(f) {
            cols.push(f);
        }
    }
    if let Some((c, _)) = sort
        && !cols.contains(&c)
    {
        cols.push(c);
    }
    let mut total = 0u64;
    let mut kept: Vec<(Cell, u64, serde_json::Map<String, serde_json::Value>)> = Vec::new();
    let cmp = |a: &(Cell, u64, serde_json::Map<String, serde_json::Value>),
               b: &(Cell, u64, serde_json::Map<String, serde_json::Value>)| {
        let (_, desc) = sort.unwrap_or(("", false));
        let o = cmp_cells(&a.0, &b.0);
        let o = if desc && !matches!((&a.0, &b.0), (Cell::Null, _) | (_, Cell::Null)) {
            o.reverse()
        } else {
            o
        };
        o.then(a.1.cmp(&b.1))
    };
    scan_table(
        &index_dir.join(EXPERIMENTS_FILE),
        Some(&cols),
        &mut |rows| {
            for r in 0..rows.len() {
                if !q.matches(rows, r) {
                    continue;
                }
                total += 1;
                if sort.is_none() && kept.len() >= limit {
                    continue;
                }
                let mut obj = serde_json::Map::new();
                for f in &fields {
                    obj.insert((*f).to_string(), rows.get(r, f).to_json());
                }
                let key = sort.map_or(Cell::Null, |(c, _)| rows.get(r, c));
                kept.push((key, total, obj));
                if sort.is_some()
                    && limit != usize::MAX
                    && kept.len() >= limit.saturating_mul(2).max(1024)
                {
                    kept.sort_by(cmp);
                    kept.truncate(limit);
                }
            }
            Ok(())
        },
    )?;
    if sort.is_some() {
        kept.sort_by(cmp);
    }
    kept.truncate(limit);
    let results: Vec<_> = kept.into_iter().map(|k| k.2).collect();
    Ok(SearchOutput {
        index_dir: index_dir.to_string_lossy().into_owned(),
        query: req.query.clone(),
        index_complete: manifest.complete,
        total,
        returned: results.len(),
        truncated: (results.len() as u64) < total,
        fields: fields.iter().map(|f| (*f).to_string()).collect(),
        results,
    })
}
