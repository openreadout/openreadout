//! Rendering results as JSON envelopes or human-readable text.

use std::sync::OnceLock;

use openreadout_core::envelope::ToolId;
use openreadout_core::{Envelope, Error};
use serde::Serialize;

pub fn tool_id() -> ToolId {
    ToolId {
        name: "openreadout".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

static ONLY: OnceLock<Vec<String>> = OnceLock::new();
static COMPACT: OnceLock<bool> = OnceLock::new();

/// Set the global `--compact` flag (once, at start-up).
pub fn set_compact(on: bool) {
    let _ = COMPACT.set(on);
}

/// JSON text of `v`: indented, or on one line with `--compact` (fewer tokens for agents).
pub fn json_text<T: Serialize + ?Sized>(v: &T) -> serde_json::Result<String> {
    if COMPACT.get().copied().unwrap_or(false) {
        serde_json::to_string(v)
    } else {
        serde_json::to_string_pretty(v)
    }
}

/// Set the global `--only` pointers (once, at start-up).
pub fn set_only(pointers: Vec<String>) {
    let _ = ONLY.set(pointers);
}

/// True when `--only` was given: JSON output then carries only the named values.
pub fn only_set() -> bool {
    ONLY.get().is_some_and(|p| !p.is_empty())
}

/// The value printed as `data`: `data` itself, or its `--only` projection.
pub fn data_value<T: Serialize>(data: &T) -> Result<serde_json::Value, Error> {
    let v = serde_json::to_value(data)
        .map_err(|e| Error::Usage(format!("output not serializable: {e}")))?;
    match ONLY.get() {
        Some(p) if !p.is_empty() => {
            strict_only_check(&v, p)?;
            openreadout_ops::project::pick(&v, p)
        }
        _ => Ok(v),
    }
}

/// `--strict --only P`: asking for a value the file's assurance withholds (assumed, derived by
/// an unconfirmed rule, or never compared with an independent reader) is refused (exit 6)
/// rather than answered with null.
fn strict_only_check(v: &serde_json::Value, pointers: &[String]) -> Result<(), Error> {
    if !openreadout_core::assurance::default_strict() {
        return Ok(());
    }
    let Some(a) = v
        .get("assurance")
        .cloned()
        .and_then(|a| serde_json::from_value::<openreadout_core::assurance::Assurance>(a).ok())
    else {
        return Ok(());
    };
    for p in pointers {
        if let Some(w) = openreadout_core::assurance::withheld_for(&a, p) {
            return Err(Error::Unsupported {
                format: "strict",
                feature: format!("returning {p}: {} ({})", w.field, w.reason),
                hint: Some(
                    "--strict returns only values an independent reader has confirmed for this kind of file. Rerun without --strict to read this value anyway and treat it as unverified (`info --json` → assurance.strict_withholds says why)."
                        .into(),
                ),
            });
        }
    }
    Ok(())
}

/// Print a success payload. `human` renders the text form when `--json` is not set.
pub fn emit<T: Serialize>(json: bool, data: &T, human: impl FnOnce(&T) -> String) -> i32 {
    if json || only_set() {
        let v = match data_value(data) {
            Ok(v) => v,
            Err(e) => return fail(true, &e),
        };
        let env = Envelope::ok(tool_id(), v);
        println!("{}", json_text(&env).expect("serializable"));
    } else if !crate::ui::quiet() {
        anstream::println!("{}", human(data));
    }
    0
}

/// Print an error in the requested form and return its exit code.
pub fn fail(json: bool, err: &Error) -> i32 {
    if json {
        let env: Envelope<()> = Envelope::err(tool_id(), err);
        println!("{}", json_text(&env).expect("serializable"));
    } else {
        print_error(None, err);
    }
    err.exit_code()
}

/// `warning: …` on stderr (unless quiet).
pub fn warn(msg: &str) {
    if !crate::ui::quiet() {
        anstream::eprintln!("{} {msg}", crate::ui::paint(crate::ui::WARN, "warning:"));
    }
}

/// `error: …` and `hint: …` on stderr (coloured where colour is on), optionally naming the input.
pub fn print_error(path: Option<&str>, err: &Error) {
    let at = path.map(|p| format!("{p}: ")).unwrap_or_default();
    anstream::eprintln!("{} {at}{err}", crate::ui::paint(crate::ui::ERR, "error:"));
    if let Some(h) = err.hint() {
        anstream::eprintln!("{} {h}", crate::ui::paint(crate::ui::WARN, "hint:"));
    }
}
