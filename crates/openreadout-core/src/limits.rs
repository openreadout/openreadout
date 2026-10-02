//! Allocation guards shared by every reader.
//!
//! Any buffer whose size comes from a value stored in the file (a length field, a
//! dimension, a count) must be checked here *before* it is allocated. A malformed or
//! hostile file can declare a 2^64-byte chunk in a 1 KB file; without the check the
//! process aborts on allocation failure instead of reporting `corrupt_file`.
//!
//! The rule of thumb:
//! - bytes that are read from the file are capped by what is left of the file
//!   ([`checked_len`]);
//! - metadata (XML headers, directories, chunk maps, LV trees) is additionally capped by
//!   [`MAX_METADATA_BYTES`] ([`checked_metadata_len`]);
//! - decoded pixel buffers are capped by [`plane_limit`] ([`plane_len`]): at most
//!   [`PLANE_EXPANSION`] times the file size (at least [`MIN_PLANE_LIMIT`], at most
//!   [`MAX_PLANE_BYTES`]), so a few-KB file cannot make a reader allocate gigabytes; readers
//!   that store samples uncompressed also check the plane against the bytes behind it.

use crate::error::{Error, Result};

/// Largest single metadata block any reader will load into memory (512 MiB). Real files
/// stay far below this (the largest XML header in the corpus is a few MB); anything bigger
/// is treated as corruption rather than risking an out-of-memory abort.
pub const MAX_METADATA_BYTES: u64 = 512 << 20;

/// Largest decoded plane (one c/z/t slice, all samples) a reader will materialize by
/// default (4 GiB), shared with the export guard in [`crate::pixel`]. Larger mosaics need
/// tiled access and are refused with `unsupported_feature` instead of being allocated.
pub const MAX_PLANE_BYTES: u64 = crate::pixel::MAX_PLANE_BYTES;

/// A plane may be at most this many times larger than the whole file it comes from. Real
/// acquisitions compress far less than 256:1 overall; a tiny file declaring a huge plane is
/// either corrupt or a decompression bomb.
pub const PLANE_EXPANSION: u64 = 256;

/// Floor of the file-relative plane limit (1 GiB), so small files with large, highly
/// compressible planes still open.
pub const MIN_PLANE_LIMIT: u64 = 1 << 30;

/// Environment variable that replaces the plane limit with a fixed number of bytes: lower it
/// on small machines (or under a fuzzer), raise it for an unusually compressible file,
/// up to the hard 4 GiB ceiling.
pub const PLANE_LIMIT_ENV: &str = "OPENREADOUT_MAX_PLANE_BYTES";

fn plane_limit_override() -> Option<u64> {
    static LIMIT: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        std::env::var(PLANE_LIMIT_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
    })
}

/// The largest plane (bytes) a reader may allocate for a file of `file_len` bytes:
/// `PLANE_EXPANSION x file_len`, clamped to `[MIN_PLANE_LIMIT, MAX_PLANE_BYTES]`, unless
/// [`PLANE_LIMIT_ENV`] sets it explicitly (still capped by [`MAX_PLANE_BYTES`]).
pub fn plane_limit(file_len: u64) -> u64 {
    plane_limit_override().map_or_else(
        || {
            file_len
                .saturating_mul(PLANE_EXPANSION)
                .clamp(MIN_PLANE_LIMIT, MAX_PLANE_BYTES)
        },
        |limit| limit.min(MAX_PLANE_BYTES),
    )
}

/// Largest number of items (directory entries, images, loop positions) a reader will
/// collect from one count field before looking at the data behind it.
pub const MAX_ITEM_COUNT: u64 = 1 << 24;

/// Check that `declared` bytes can be read from a region with `available` bytes left, and
/// that the length fits in memory. Returns the length as `usize`.
///
/// `what` names the thing being read (for the error message).
pub fn checked_len(
    format: &'static str,
    declared: u64,
    available: u64,
    what: &str,
) -> Result<usize> {
    if declared > available {
        return Err(Error::corrupt(
            format,
            format!(
                "{what} declares {declared} bytes but only {available} are available (truncated or corrupt length field)"
            ),
        ));
    }
    usize::try_from(declared).map_err(|_| {
        Error::corrupt(
            format,
            format!("{what} declares {declared} bytes, more than this platform can address"),
        )
    })
}

/// [`checked_len`] for metadata: additionally capped at [`MAX_METADATA_BYTES`].
pub fn checked_metadata_len(
    format: &'static str,
    declared: u64,
    available: u64,
    what: &str,
) -> Result<usize> {
    if declared > MAX_METADATA_BYTES {
        return Err(Error::corrupt(
            format,
            format!(
                "{what} declares {declared} bytes of metadata (limit {MAX_METADATA_BYTES}); refusing to load it"
            ),
        ));
    }
    checked_len(format, declared, available, what)
}

/// Read a whole metadata file (an XML companion or container, a text sidecar) from `fs` after
/// checking that it is no larger than [`MAX_METADATA_BYTES`].
pub fn read_metadata_file(
    format: &'static str,
    fs: &crate::source::Fs,
    path: &std::path::Path,
) -> Result<Vec<u8>> {
    let len = fs.metadata(path).map_err(|e| Error::io(path, e))?.len();
    if len > MAX_METADATA_BYTES {
        return Err(Error::corrupt(
            format,
            format!(
                "{} is {len} bytes; refusing to load more than {MAX_METADATA_BYTES} bytes of metadata",
                path.display()
            ),
        ));
    }
    fs.read(path).map_err(|e| Error::io(path, e))
}

/// [`read_metadata_file`] as text; invalid UTF-8 is reported as corrupt.
pub fn read_metadata_text(
    format: &'static str,
    fs: &crate::source::Fs,
    path: &std::path::Path,
) -> Result<String> {
    String::from_utf8(read_metadata_file(format, fs, path)?)
        .map_err(|_| Error::corrupt(format, format!("{} is not UTF-8 text", path.display())))
}

/// Bytes remaining in a file of length `file_len` from `offset` (0 when past the end).
pub fn remaining(file_len: u64, offset: u64) -> u64 {
    file_len.saturating_sub(offset)
}

/// Byte size of a `width` x `height` plane with `samples` samples of `bytes_per_sample`
/// bytes each, from a file of `file_len` bytes, with overflow checking and the
/// [`plane_limit`] cap.
pub fn plane_len(
    format: &'static str,
    width: u64,
    height: u64,
    samples: u64,
    bytes_per_sample: u64,
    file_len: u64,
) -> Result<usize> {
    let n = width
        .checked_mul(height)
        .and_then(|v| v.checked_mul(samples))
        .and_then(|v| v.checked_mul(bytes_per_sample))
        .ok_or_else(|| {
            Error::corrupt(
                format,
                format!(
                    "plane geometry {width}x{height}x{samples}x{bytes_per_sample} bytes overflows"
                ),
            )
        })?;
    let cap = plane_limit(file_len);
    if n > cap {
        return Err(Error::unsupported(
            format,
            format!("a {width}x{height} plane of {n} bytes (limit {cap} for this file)"),
            format!(
                "The header declares a plane too large to materialize: either the file is corrupt, or it is a whole-slide image that needs tiled access. If the file is sound and memory allows, set {PLANE_LIMIT_ENV}=<bytes> to raise the file-relative limit (the hard 4 GiB ceiling still applies)."
            ),
        ));
    }
    usize::try_from(n).map_err(|_| {
        Error::corrupt(
            format,
            format!("plane of {n} bytes does not fit this platform's address space"),
        )
    })
}

/// Check that an image's declared plane count (`c x z x t`) is small enough to enumerate
/// (at most [`MAX_ITEM_COUNT`]) before an exporter builds per-plane lists. `what` names the
/// caller for the error message (e.g. `"ome-tiff"`).
pub fn checked_plane_count(what: &'static str, c: u32, z: u32, t: u32) -> Result<u64> {
    let n = u64::from(c) * u64::from(z) * u64::from(t);
    if n > MAX_ITEM_COUNT {
        return Err(Error::unsupported(
            what,
            format!("an image declaring {c} x {z} x {t} = {n} planes"),
            format!(
                "More than {MAX_ITEM_COUNT} planes per image is not exported; the header is probably corrupt (run `openreadout check`)."
            ),
        ));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_len_rejects_lengths_past_the_end() {
        assert_eq!(checked_len("t", 10, 10, "x").unwrap(), 10);
        let e = checked_len("t", 11, 10, "x").unwrap_err();
        assert_eq!(e.exit_code(), 4);
    }

    #[test]
    fn metadata_cap_applies() {
        let e = checked_metadata_len("t", MAX_METADATA_BYTES + 1, u64::MAX, "xml").unwrap_err();
        assert_eq!(e.code(), "corrupt_file");
    }

    #[test]
    fn plane_len_checks_overflow_and_cap() {
        assert_eq!(plane_len("t", 4, 3, 1, 2, 100).unwrap(), 24);
        assert_eq!(
            plane_len("t", u64::MAX, 2, 1, 1, 100).unwrap_err().code(),
            "corrupt_file"
        );
        assert_eq!(
            plane_len("t", 1 << 20, 1 << 20, 3, 8, u64::MAX)
                .unwrap_err()
                .code(),
            "unsupported_feature"
        );
        assert!(plane_len("t", 65_536, 65_536, 1, 1, u64::MAX).is_ok());
        assert!(plane_len("t", 65_536, 65_537, 1, 1, u64::MAX).is_err());
        // A 1 KB file may not declare a 2 GiB plane; a 100 MB file may.
        assert!(plane_len("t", 1 << 15, 1 << 15, 1, 2, 1 << 10).is_err());
        assert!(plane_len("t", 1 << 15, 1 << 15, 1, 2, 100 << 20).is_ok());
    }

    #[test]
    fn plane_limit_scales_with_the_file() {
        if std::env::var_os(PLANE_LIMIT_ENV).is_some() {
            return;
        }
        assert_eq!(plane_limit(0), MIN_PLANE_LIMIT);
        assert_eq!(plane_limit(10 << 20), (10 << 20) * PLANE_EXPANSION);
        assert_eq!(plane_limit(u64::MAX), MAX_PLANE_BYTES);
    }

    #[test]
    fn remaining_saturates() {
        assert_eq!(remaining(10, 4), 6);
        assert_eq!(remaining(10, 40), 0);
    }
}
