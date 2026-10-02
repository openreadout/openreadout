//! `openreadout_index` (catalog a file share) and `openreadout_search` (query the catalog).

use std::path::{Path, PathBuf};

use openreadout_core::Error;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ProgressNotificationParam};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, schemars, tool, tool_router};
use serde::Deserialize;

use crate::{InstrumentServer, mcp_err, ok_json};

/// Arguments for `openreadout_index`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IndexArgs {
    /// Directories (or files) to crawl.
    pub roots: Vec<String>,
    /// Index directory to create or update (never inside a root).
    pub index_dir: String,
    /// Integrity check per data set: `headers` (default; structure only), `full` or `none`.
    pub check: Option<String>,
    /// Stop after this many files in this call (default 20000); call again to continue.
    pub max_files: Option<u64>,
    /// Stop after this many seconds in this call (default 45); call again to continue.
    pub max_seconds: Option<f64>,
    /// Glob patterns of names or root-relative paths to skip.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Discard an interrupted crawl instead of resuming it.
    #[serde(default)]
    pub restart: bool,
    /// Read every file again instead of reusing unchanged records.
    #[serde(default)]
    pub full_rescan: bool,
    /// Look for personal data (default true).
    pub pii: Option<bool>,
    /// Worker threads (default: the number of CPUs).
    pub threads: Option<usize>,
}

/// Default `max_files` of `openreadout_index` per call.
pub const MCP_INDEX_MAX_FILES: u64 = 20_000;
/// Default `max_seconds` of `openreadout_index` per call.
pub const MCP_INDEX_MAX_SECONDS: f64 = 45.0;

/// Arguments for `openreadout_search`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// Index directory (from openreadout_index or `openreadout index`).
    pub index_dir: String,
    /// Query: space-separated terms, all must match; `OR` between alternatives. `field=value`,
    /// `!=`, `~` (contains), `<`/`<=`/`>`/`>=` with units (`size>1GB`, `rate>=20kHz`) or dates
    /// (`acquired<2020`), `field:TERM` (`technique:FBbi_00000246`), `-term` negates, `a|b`
    /// alternatives, bare words search paths, samples, channels and descriptions. Fields:
    /// format, family, sample, instrument, model, operator, method, technique, acquired, year,
    /// objective (`63x`), channel, pixel, x/y/z/c/t, size, rows, scans, rate, status, pii,
    /// risk, `param.<name>`, or any column of experiments.parquet. Empty: everything.
    #[serde(default)]
    pub query: String,
    /// Sort field (`size`, `-acquired` for descending). Default: path order.
    pub sort: Option<String>,
    /// Most results (default 50, max 1000).
    pub limit: Option<usize>,
    /// Columns to return (aliases allowed); default: path, format, size, acquired, sample,
    /// instrument model, technique, channels, objective, check status, what. `["all"]`: every column.
    #[serde(default)]
    pub fields: Vec<String>,
}

/// Most results `openreadout_search` returns.
pub const MAX_SEARCH_RESULTS: usize = 1000;

#[tool_router(router = index_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_index",
        annotations(title = "Index a file share", read_only_hint = false, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_index::IndexManifest>(),
        description = "Catalog every data set under roots into index_dir (Parquet: one row per data set with format, size, sample, instrument, technique, method, operator, start, dimensions, channels, objective, events, scans, integrity status, personal-data flags). Headers only and resumable: a call stops after max_files/max_seconds with complete=false, and the same call again continues. openreadout_search queries the result."
    )]
    pub(crate) async fn index(
        &self,
        Parameters(a): Parameters<IndexArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry;
        let token = ctx.meta.get_progress_token();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<u64>();
        let forward = token.map(|tok| {
            let peer = ctx.peer.clone();
            tokio::spawn(async move {
                while let Some(done) = rx.recv().await {
                    let p = ProgressNotificationParam::new(tok.clone(), done as f64)
                        .with_message(format!("read {done} files and directory data sets"));
                    let _ = peer.notify_progress(p).await;
                }
            })
        });
        let want_progress = forward.is_some();
        let r = tokio::task::spawn_blocking(move || {
            let reg = registry();
            let mut o = openreadout_index::IndexOptions::default();
            o.roots = a.roots.iter().map(PathBuf::from).collect();
            o.index_dir = PathBuf::from(&a.index_dir);
            o.check = match a.check.as_deref().unwrap_or("headers") {
                "headers" => openreadout_index::CheckMode::Headers,
                "full" => openreadout_index::CheckMode::Full,
                "none" => openreadout_index::CheckMode::None,
                other => {
                    return Err(mcp_err(&Error::Usage(format!(
                        "unknown check mode '{other}' (headers|full|none)"
                    ))));
                }
            };
            o.max_files = Some(a.max_files.unwrap_or(MCP_INDEX_MAX_FILES));
            o.max_seconds = Some(a.max_seconds.unwrap_or(MCP_INDEX_MAX_SECONDS));
            o.exclude = a.exclude;
            o.restart = a.restart;
            o.full_rescan = a.full_rescan;
            o.pii = a.pii.unwrap_or(true);
            let threads = a.threads.unwrap_or_else(|| {
                std::thread::available_parallelism().map_or(2, std::num::NonZero::get)
            });
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads.max(1))
                .build()
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            let cb = move |p: &openreadout_index::CrawlProgress| {
                if want_progress {
                    let _ = tx.send(p.stats.items);
                }
            };
            pool.install(|| openreadout_index::index(&reg, &o, Some(&cb)))
                .map_err(|e| mcp_err(&e))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("index task failed: {e}"), None))?;
        if let Some(f) = forward {
            let _ = f.await;
        }
        let mut m = r?;
        // Column lists are in index.json; they would only fill the agent's context here.
        for t in m.tables.values_mut() {
            t.columns.clear();
        }
        ok_json(&m)
    }

    #[tool(
        name = "openreadout_search",
        annotations(title = "Search an index", read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<openreadout_index::SearchOutput>(),
        description = "Query an index from openreadout_index: 'format=nd2 objective=60x', 'channel~CD4', 'events>100000', 'rate>=20kHz', 'acquired<2015', 'status=truncated|corrupt'. Returns {total, returned, results[]}: total counts every match, results is capped by limit (default 50); fields=['all'] returns every column."
    )]
    pub(crate) fn search(
        &self,
        Parameters(a): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let mut r = openreadout_index::SearchRequest::default();
        r.query = a.query;
        r.sort = a.sort;
        r.limit = Some(a.limit.unwrap_or(50).clamp(1, MAX_SEARCH_RESULTS));
        r.fields = a.fields;
        let out =
            openreadout_index::search(Path::new(&a.index_dir), &r).map_err(|e| mcp_err(&e))?;
        ok_json(&out)
    }
}
