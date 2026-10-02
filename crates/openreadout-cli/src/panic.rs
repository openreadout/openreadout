//! Panic handler: any panic (a bug) becomes exit code 1 with a short report and, under
//! `--json`/`--jsonl`, an error envelope `{code: "internal_panic", hint: ...}` on stdout, so
//! scripts and agents always get a well-formed answer.
//!
//! The release profile keeps `panic = "abort"`; the hook runs before the abort and exits the
//! process itself. No stack trace is printed unless `RUST_BACKTRACE` is set (to anything but
//! `0`). `--self-panic` (hidden) or `OPENREADOUT_DEBUG_PANIC=1` trigger a panic on purpose,
//! for tests of this handler. A closed output pipe (`… | head`) ends the process quietly with
//! exit code 1 instead of reporting a bug.

use std::io::Write;

use openreadout_core::envelope::{Envelope, ErrorBody, SCHEMA_VERSION};

const ISSUES: &str = "https://github.com/openreadout/openreadout/issues";

fn backtrace_wanted() -> bool {
    std::env::var_os("RUST_BACKTRACE").is_some_and(|v| v != "0")
}

/// Install the hook. Call first thing in `main`.
pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info.payload();
        let msg = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        // A closed output pipe (`openreadout info --json f | head -1`) is not a bug: stop
        // quietly, as a process killed by SIGPIPE would, without writing anything more.
        if msg.contains("Broken pipe") || msg.contains("failed printing to stdout") {
            std::process::exit(1);
        }
        let at = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let message = format!("internal error: {msg}{at}");
        let hint = format!(
            "This is a bug in openreadout {}; please report it at {ISSUES} with the command line and the output of the same command run with RUST_BACKTRACE=1.",
            env!("CARGO_PKG_VERSION")
        );
        // `openreadout report`: finish the bundle with the crash recorded, so the bug can be
        // reported with it.
        let report = openreadout_index::report::panic_hook(&message);
        let json = std::env::args_os().any(|a| a == "--json" || a == "--jsonl");
        if json {
            let env: Envelope<()> = Envelope {
                ok: false,
                schema_version: SCHEMA_VERSION.into(),
                tool: crate::output::tool_id(),
                path: None,
                data: None,
                error: Some(ErrorBody {
                    code: "internal_panic".into(),
                    message: message.clone(),
                    hint: Some(hint.clone()),
                    exit_code: 1,
                }),
            };
            if let Ok(s) = serde_json::to_string(&env) {
                let _ = writeln!(std::io::stdout(), "{s}");
            }
        }
        let mut err = std::io::stderr();
        let _ = writeln!(err, "error: {message}");
        let _ = writeln!(err, "hint: {hint}");
        if let Some(p) = report {
            let _ = writeln!(
                err,
                "The report was written to {} with the crash recorded; please attach it to the issue.",
                p.display()
            );
        }
        if backtrace_wanted() {
            let _ = writeln!(err, "{}", std::backtrace::Backtrace::force_capture());
        }
        std::process::exit(1);
    }));
}

/// Panic on purpose when asked to (tests of the handler).
pub fn self_test(flag: bool) {
    let env = std::env::var_os("OPENREADOUT_DEBUG_PANIC").is_some_and(|v| !v.is_empty());
    assert!(!(flag || env), "self-panic requested (--self-panic)");
}
