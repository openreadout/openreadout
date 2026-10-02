//! Error type and the exit-code contract.
//!
//! Exit codes are part of the public interface (agents branch on them):
//! 0 ok · 1 error · 2 usage · 3 unknown/unsupported format · 4 corrupt or truncated ·
//! 5 I/O · 6 known format but unsupported feature.

use std::path::PathBuf;

/// What to do about a usage error, from the kind of mistake its message describes.
fn usage_hint(message: &str) -> &'static str {
    let m = message.to_ascii_lowercase();
    if m.contains("--overwrite") {
        "Choose another output path, or pass --overwrite to replace that file (the input is never modified)."
    } else if m.contains("select") {
        "Selections are zero-based and repeatable: `c=0`, `z=1-3`, `t=0,2`; `openreadout info FILE --json` gives each image's size_c, size_z and size_t."
    } else if [
        "out of range",
        "does not exist",
        "past the end",
        "no such",
        "not found",
    ]
    .iter()
    .any(|k| m.contains(k))
    {
        "Indices are zero-based; `openreadout info FILE --json` lists the images, traces (sweep_count, sample_count), tables (row_count) and spectra the file holds."
    } else {
        "Check the arguments with `openreadout help <command>`; `openreadout info FILE --json` shows what the file holds."
    }
}

/// What to do about an unexpected failure.
fn other_hint(message: &str) -> &'static str {
    let m = message.to_ascii_lowercase();
    if m.contains("read-back") || m.contains("read back") || m.contains("verif") {
        "The output was written to a temporary file that failed verification, so nothing was kept; check free disk space and the output directory, then rerun."
    } else {
        "Unexpected failure: rerun with --json for the full error, and report it with `openreadout --version` and `openreadout info FILE --json`."
    }
}

/// Result alias used throughout `OpenReadout`.
pub type Result<T> = std::result::Result<T, Error>;

/// Every failure `OpenReadout` can report. Each variant maps to a stable string code
/// (for JSON) and an exit code (for shells).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file does not match any supported format's signature.
    #[error("unrecognized file format: {path}")]
    UnknownFormat {
        /// The file that was not recognised.
        path: PathBuf,
    },

    /// The format is known but this file uses a feature we do not decode yet.
    #[error("{format}: unsupported feature: {feature}")]
    Unsupported {
        /// Format id (or command) that reported it.
        format: &'static str,
        /// What is not supported.
        feature: String,
        /// What the caller can do instead.
        hint: Option<String>,
    },

    /// The file violates its own format's invariants (truncation, bad offsets, ...).
    #[error("{format}: corrupt file: {detail}")]
    Corrupt {
        /// Format id that reported it.
        format: &'static str,
        /// What is wrong.
        detail: String,
        /// Byte offset of the problem, when known.
        offset: Option<u64>,
    },

    /// Bad arguments or an impossible request (e.g. plane index out of range).
    #[error("usage error: {0}")]
    Usage(String),

    /// Operating-system I/O failure.
    #[error("I/O error on {path}: {source}")]
    Io {
        /// The file or directory involved.
        path: PathBuf,
        /// The operating-system error.
        #[source]
        source: std::io::Error,
    },

    /// Anything else.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Stable machine-readable code for the JSON envelope.
    pub fn code(&self) -> &'static str {
        match self {
            Error::UnknownFormat { .. } => "unknown_format",
            Error::Unsupported { .. } => "unsupported_feature",
            Error::Corrupt { .. } => "corrupt_file",
            Error::Usage(_) => "usage",
            Error::Io { .. } => "io",
            Error::Other(_) => "error",
        }
    }

    /// Process exit code for this error.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Other(_) => 1,
            Error::Usage(_) => 2,
            Error::UnknownFormat { .. } => 3,
            Error::Corrupt { .. } => 4,
            Error::Io { .. } => 5,
            Error::Unsupported { .. } => 6,
        }
    }

    /// A suggestion the caller (human or agent) can act on.
    pub fn hint(&self) -> Option<String> {
        match self {
            Error::UnknownFormat { .. } => Some(
                "Run `openreadout self formats` to list supported formats; the file may be a format not yet implemented, or a renamed export."
                    .into(),
            ),
            Error::Unsupported { hint, .. } => hint.clone(),
            Error::Corrupt { .. } => Some(
                "Run `openreadout check <file>` for a full integrity report. The file may be truncated by an interrupted acquisition or copy."
                    .into(),
            ),
            Error::Io { .. } => Some("Check that the path exists and is readable.".into()),
            Error::Usage(m) => Some(usage_hint(m).into()),
            Error::Other(m) => Some(other_hint(m).into()),
        }
    }

    /// Convenience constructor for I/O errors carrying the path.
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }

    /// Convenience constructor for corruption findings.
    pub fn corrupt(format: &'static str, detail: impl Into<String>) -> Self {
        Error::Corrupt {
            format,
            detail: detail.into(),
            offset: None,
        }
    }

    /// Convenience constructor for corruption findings at a known byte offset.
    pub fn corrupt_at(format: &'static str, offset: u64, detail: impl Into<String>) -> Self {
        Error::Corrupt {
            format,
            detail: detail.into(),
            offset: Some(offset),
        }
    }

    /// Convenience constructor for unsupported features.
    pub fn unsupported(
        format: &'static str,
        feature: impl Into<String>,
        hint: impl Into<String>,
    ) -> Self {
        Error::Unsupported {
            format,
            feature: feature.into(),
            hint: Some(hint.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every error an agent can hit carries a hint it can act on.
    #[test]
    fn every_error_has_a_hint() {
        let errors = [
            Error::UnknownFormat { path: "x".into() },
            Error::unsupported("x", "y", "Convert it first."),
            Error::corrupt("x", "y"),
            Error::Usage("sweep 9 out of range (trace 0 has 3 sweeps, 0..3)".into()),
            Error::Usage("out.csv exists; pass --overwrite to replace it".into()),
            Error::Usage("selection matches no planes".into()),
            Error::Usage("anything".into()),
            Error::io("x", std::io::Error::other("y")),
            Error::Other("Parquet read-back: bad".into()),
            Error::Other("anything".into()),
        ];
        for e in &errors {
            assert!(e.hint().is_some_and(|h| h.len() > 10), "{e}");
        }
        let hint = |i: usize| errors[i].hint().unwrap_or_default();
        assert!(hint(3).contains("info FILE --json"));
        assert!(hint(4).contains("--overwrite"));
        assert!(hint(5).contains("c=0"));
        assert!(hint(8).contains("verification"));
    }
}
