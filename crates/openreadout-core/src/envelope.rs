//! The JSON envelope every command emits with `--json`.
//!
//! ```json
//! {"ok":true,"schema_version":"1","tool":{"name":"openreadout","version":"0.1.0"},"data":{...}}
//! {"ok":false,"schema_version":"1","tool":{...},"error":{"code":"corrupt_file","message":"...","hint":"...","exit_code":4}}
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Error;

/// Bumped only when the shape of existing fields changes. Adding fields is not a bump.
pub const SCHEMA_VERSION: &str = "1";

/// Identifies the producing tool so consumers can reason about compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ToolId {
    /// Always `openreadout`.
    pub name: String,
    /// The tool's version (`CARGO_PKG_VERSION`).
    pub version: String,
}

/// Structured error body. `hint` is meant to be actionable by an agent.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ErrorBody {
    /// Stable code: `unknown_format`, `unsupported_feature`, `corrupt_file`, `usage`, `io`, `error`,
    /// `internal_panic` (a bug: the tool panicked; please report it).
    pub code: String,
    /// Human-readable description of the failure.
    pub message: String,
    /// What to try next, when the tool knows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The process exit code for this error (see the crate docs).
    pub exit_code: i32,
}

impl From<&Error> for ErrorBody {
    fn from(e: &Error) -> Self {
        ErrorBody {
            code: e.code().to_string(),
            message: e.to_string(),
            hint: e.hint(),
            exit_code: e.exit_code(),
        }
    }
}

/// Success or failure wrapper.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Envelope<T> {
    /// True on success (`data` present), false on failure (`error` present).
    pub ok: bool,
    /// [`SCHEMA_VERSION`] at the time of writing.
    pub schema_version: String,
    /// The producing tool.
    pub tool: ToolId,
    /// The input this envelope answers for, in batch mode (`--jsonl`, JSON arrays, MCP
    /// `openreadout_batch`); absent for single-file commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The command's output, on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    /// What went wrong, on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl<T> Envelope<T> {
    /// A success envelope carrying `data`.
    pub fn ok(tool: ToolId, data: T) -> Self {
        Envelope {
            ok: true,
            schema_version: SCHEMA_VERSION.into(),
            tool,
            path: None,
            data: Some(data),
            error: None,
        }
    }
    /// A failure envelope describing `e`.
    pub fn err(tool: ToolId, e: &Error) -> Self {
        Envelope {
            ok: false,
            schema_version: SCHEMA_VERSION.into(),
            tool,
            path: None,
            data: None,
            error: Some(e.into()),
        }
    }
    /// Tag the envelope with the input path (batch mode).
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}
