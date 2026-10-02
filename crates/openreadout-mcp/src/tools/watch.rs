//! `openreadout_watch`: events from directories an instrument is writing to, by cursor.

use std::path::{Path, PathBuf};

use openreadout_core::Error;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ErrorData as McpError, schemars, tool, tool_router};
use serde::{Deserialize, Serialize};

use crate::{InstrumentServer, mcp_err, ok_json};

/// Arguments for `openreadout_watch`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WatchArgs {
    /// Directories (or files) to watch. The same list (in any order) continues the same
    /// watcher across calls.
    pub dirs: Vec<String>,
    /// Return events with a sequence number greater than this (the `cursor` of the previous
    /// call); 0 or absent on the first call.
    pub cursor: Option<u64>,
    /// Most events returned (default 200, max 5000); call again with the new cursor for more.
    pub limit: Option<usize>,
    /// First call only: report data sets modified since then (`10m`, `2h`, `1d`, an ISO-8601
    /// time, or `all`). Default: the live window (5 minutes) back from now.
    pub since: Option<String>,
    /// First call only: evaluate the built-in live QC rules on each new plane or scan.
    #[serde(default)]
    pub qc: bool,
    /// First call only: seconds without growth before `dataset_stalled` (default 120).
    pub stall_after_s: Option<f64>,
}

/// Output of `openreadout_watch`.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct WatchOutput {
    /// Pass back as `cursor` to get only newer events.
    pub cursor: u64,
    /// Events after the given cursor, oldest first (the same objects `openreadout watch`
    /// prints under `data`).
    pub events: Vec<openreadout_live::watch::WatchEvent>,
    /// True when more events are waiting (raise `limit` or call again).
    pub more: bool,
    /// True when events after the cursor were dropped (the server keeps the last 10000).
    pub dropped: bool,
    /// Data sets the watcher remembers.
    pub tracked: usize,
}

/// Events kept per watcher for cursor polling.
const WATCH_LOG_CAP: usize = 10_000;

/// Watchers kept by the server, keyed by their sorted directory list.
pub(crate) type Watches = std::sync::Arc<
    std::sync::Mutex<
        std::collections::HashMap<
            String,
            (
                openreadout_live::watch::Watcher,
                openreadout_live::watch::EventLog,
            ),
        >,
    >,
>;

#[tool_router(router = watch_router, vis = "pub(super)")]
impl InstrumentServer {
    #[tool(
        name = "openreadout_watch",
        annotations(title = "Watch for data being acquired", read_only_hint = true, destructive_hint = false, idempotent_hint = false, open_world_hint = false),
        output_schema = rmcp::handler::server::common::schema_for_output::<WatchOutput>(),
        description = "Follow directories an instrument writes to: each call looks once and returns events since cursor (dataset_new, plane_new, scan_new, frame_new, dataset_complete, dataset_stalled, qc with qc=true: saturation, focus drift, dropped frames, TIC drop, error). Poll with the returned cursor; files are never locked."
    )]
    pub(crate) async fn watch(
        &self,
        Parameters(a): Parameters<WatchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let registry = self.registry;
        let watches = self.watches.clone();
        let out = tokio::task::spawn_blocking(move || -> Result<WatchOutput, McpError> {
            if a.dirs.is_empty() {
                return Err(mcp_err(&Error::Usage(
                    "dirs must name at least one directory".into(),
                )));
            }
            let mut dirs = a.dirs.clone();
            dirs.sort();
            let key = format!("{}|qc={}", dirs.join("\u{1f}"), a.qc);
            let reg = registry();
            let mut map = watches
                .lock()
                .map_err(|_| McpError::internal_error("watch state poisoned", None))?;
            if !map.contains_key(&key) {
                for d in &dirs {
                    if !Path::new(d).exists() {
                        return Err(mcp_err(&Error::io(
                            d,
                            std::io::Error::new(std::io::ErrorKind::NotFound, "no such directory"),
                        )));
                    }
                }
                let mut o = openreadout_live::watch::WatchOptions::default();
                o.roots = dirs.iter().map(PathBuf::from).collect();
                o.since = match &a.since {
                    Some(s) => openreadout_live::watch::parse_since(s).map_err(|e| mcp_err(&e))?,
                    None => {
                        std::time::SystemTime::now().checked_sub(openreadout_core::live::window())
                    }
                };
                if let Some(s) = a.stall_after_s.filter(|s| s.is_finite() && *s >= 0.0) {
                    o.stall_after = std::time::Duration::from_secs_f64(s);
                }
                if a.qc {
                    o.qc = Some(openreadout_live::qc::Rules::defaults());
                }
                // At most 16 watchers: beyond that an arbitrary one is dropped.
                if map.len() >= 16
                    && let Some(k) = map.keys().next().cloned()
                {
                    map.remove(&k);
                }
                map.insert(
                    key.clone(),
                    (
                        openreadout_live::watch::Watcher::new(o),
                        openreadout_live::watch::EventLog::new(WATCH_LOG_CAP),
                    ),
                );
            }
            let (w, log) = map
                .get_mut(&key)
                .ok_or_else(|| McpError::internal_error("watcher missing", None))?;
            w.poll(&reg, &mut |e| log.push(e));
            let cursor = a.cursor.unwrap_or(0);
            let limit = a.limit.unwrap_or(200).clamp(1, 5000);
            let (events, dropped) = log.since(cursor, limit + 1);
            let more = events.len() > limit;
            let events: Vec<_> = events.into_iter().take(limit).collect();
            let next = events.last().map_or(cursor, |e| e.seq);
            Ok(WatchOutput {
                cursor: next,
                events,
                more,
                dropped,
                tracked: w.tracked(),
            })
        })
        .await
        .map_err(|e| McpError::internal_error(format!("watch task failed: {e}"), None))??;
        ok_json(&out)
    }
}
