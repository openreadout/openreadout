//! `openreadout watch DIR…`: one JSON line (an envelope whose `data` is a
//! [`WatchEvent`](openreadout_live::watch::WatchEvent)) per new data set, plane, scan or
//! sweep, per completed or stalled data set, per QC finding and per error. See `book/src/guides/lab-shares.md`.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use openreadout_core::{Envelope, Error, Registry, Result};
use openreadout_live::qc::Rules;
use openreadout_live::watch::{WatchOptions, Watcher};

use crate::output::{fail, tool_id};

/// `watch` arguments.
#[derive(Debug, clap::Args)]
pub struct WatchArgs {
    /// Directories (or files) to watch; sub-directories are walked, directory data sets (a
    /// Zarr store, a Bruker `.d`) are one data set each.
    #[arg(required_unless_present = "print_qc_rules", value_name = "DIR")]
    pub dirs: Vec<PathBuf>,
    /// Seconds between polls. Each poll stats every file and re-opens only those that changed.
    #[arg(long, default_value_t = 0.25, value_name = "SECONDS")]
    pub interval: f64,
    /// Look once, print the events for what is there now, and exit (finished data sets are
    /// reported complete at once; default `--since` is then everything).
    #[arg(long)]
    pub once: bool,
    /// Only report data sets modified since then: a duration back from now (`90s`, `10m`,
    /// `2h`, `1d`), an ISO-8601 time, or `all`. Default: the live window back from now
    /// (acquisitions already running are picked up; old files stay quiet unless they change);
    /// with `--once`, `all`.
    #[arg(long, value_name = "WHEN")]
    pub since: Option<String>,
    /// A data set still being written that does not grow for this many seconds is reported
    /// once as `dataset_stalled`.
    #[arg(long, default_value_t = 120.0, value_name = "SECONDS")]
    pub stall_after: f64,
    /// Evaluate the built-in live QC rules on every new plane or scan (reads its pixels or
    /// peaks): saturation, focus drift, dropped frames, TIC drop. Emits `qc` events.
    #[arg(long)]
    pub qc: bool,
    /// QC rules from a TOML file instead of the built-in ones (implies `--qc`);
    /// `openreadout watch --print-qc-rules` prints the defaults to start from.
    #[arg(long, value_name = "FILE")]
    pub qc_rules: Option<PathBuf>,
    /// Print the built-in QC rules (TOML) and exit.
    #[arg(long)]
    pub print_qc_rules: bool,
    /// Stop after this many seconds (default: run until interrupted with Ctrl-C).
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<f64>,
    /// Most data sets remembered at once (memory bound); finished ones are forgotten first.
    #[arg(long, default_value_t = 100_000, value_name = "N")]
    pub max_tracked: usize,
    /// Accepted for symmetry with other commands: the output is always JSON lines.
    #[arg(long)]
    pub json: bool,
}

fn seconds(name: &str, v: f64) -> Result<Duration> {
    if v.is_finite() && v >= 0.0 {
        Ok(Duration::from_secs_f64(v))
    } else {
        Err(Error::Usage(format!(
            "{name} must be a number of seconds >= 0"
        )))
    }
}

fn options(a: &WatchArgs) -> Result<(WatchOptions, Duration, Option<Duration>)> {
    let mut o = WatchOptions::default();
    o.roots.clone_from(&a.dirs);
    for d in &a.dirs {
        if !d.exists() {
            return Err(Error::io(
                d,
                std::io::Error::new(std::io::ErrorKind::NotFound, "no such directory"),
            ));
        }
    }
    o.since = match &a.since {
        Some(s) => openreadout_live::watch::parse_since(s)?,
        None if a.once => None,
        None => SystemTime::now().checked_sub(openreadout_core::live::window()),
    };
    o.stall_after = seconds("--stall-after", a.stall_after)?;
    o.max_tracked = a.max_tracked.max(1);
    o.trust_first_look = a.once;
    o.qc = match (&a.qc_rules, a.qc) {
        (Some(p), _) => Some(Rules::parse(
            &std::fs::read_to_string(p).map_err(|e| Error::io(p, e))?,
        )?),
        (None, true) => Some(Rules::defaults()),
        (None, false) => None,
    };
    let interval = seconds("--interval", a.interval)?.max(Duration::from_millis(10));
    let timeout = a.timeout.map(|t| seconds("--timeout", t)).transpose()?;
    Ok((o, interval, timeout))
}

/// Run `watch`. Returns the exit code: 0 after `--once`, `--timeout` or Ctrl-C.
pub fn run(reg: &Registry, a: &WatchArgs) -> i32 {
    if a.print_qc_rules {
        print!("{}", openreadout_live::qc::DEFAULT_RULES);
        return 0;
    }
    let (opts, interval, timeout) = match options(a) {
        Ok(x) => x,
        Err(e) => return fail(true, &e),
    };
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        let _ = signal_hook::flag::register(sig, Arc::clone(&stop));
    }
    let started = Instant::now();
    let mut watcher = Watcher::new(opts);
    let stdout = std::io::stdout();
    let mut broken = false;
    loop {
        let poll_start = Instant::now();
        {
            let mut out = stdout.lock();
            watcher.poll(reg, &mut |e| {
                if broken {
                    return;
                }
                let path = e.path.clone();
                let env = Envelope::ok(tool_id(), e).with_path(path);
                let line = serde_json::to_string(&env).unwrap_or_default();
                // One write per line, flushed at once: a reader sees whole lines only.
                if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
                    broken = true; // the reader went away (a closed pipe)
                }
            });
        }
        if a.once || broken || stop.load(Ordering::Relaxed) {
            break;
        }
        if timeout.is_some_and(|t| started.elapsed() >= t) {
            break;
        }
        // Sleep in small slices so Ctrl-C is honoured promptly.
        let next = poll_start + interval;
        while Instant::now() < next && !stop.load(Ordering::Relaxed) {
            std::thread::sleep((next - Instant::now()).min(Duration::from_millis(50)));
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
    }
    0
}
