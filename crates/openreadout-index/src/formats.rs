//! Format facts the index needs beyond the readers' descriptors: how at risk a format is, which
//! files count as open exports, and data-set stems for pairing an original with its export.

/// Formats that are open, documented standards (anyone can write a reader from the
/// specification).
pub const OPEN: &[&str] = &[
    "tiff",
    "ome-zarr",
    "mzml",
    "mzxml",
    "imzml",
    "mzmlb",
    "fcs",
    "jcamp-dx",
    "andi-chrom",
    "nwb",
    "hdf5",
    "mrc",
    "emd",
    "plate",
    "spikeglx",
];

/// Proprietary formats whose vendor software is discontinued: Carl Zeiss AxioVision (`.zvi`,
/// succeeded by ZEN) and FEI TIA (`.ser`/`.emi`, succeeded by Velox).
pub const LEGACY: &[&str] = &["zvi", "ser"];

/// Preservation class of a format id: `open`, `legacy` or `vendor` (proprietary, maintained).
pub fn preservation(format: &str) -> &'static str {
    if OPEN.contains(&format) {
        "open"
    } else if LEGACY.contains(&format) {
        "legacy"
    } else {
        "vendor"
    }
}

/// Extensions (lower case, without the dot) of open files that count as an export of a
/// neighbouring vendor file with the same stem.
pub const OPEN_EXPORT_EXTENSIONS: &[&str] = &[
    "ome.tiff", "ome.tif", "ome.btf", "ome.zarr", "zarr", "tif", "tiff", "mzml", "mzxml", "imzml",
    "csv", "tsv", "parquet", "arrow", "feather", "nwb", "h5", "hdf5", "jdx", "dx", "jcamp", "cdf",
    "asm.json", "fcs", "mrc", "mzml.gz", "mzxml.gz", "mzmlb",
];

/// Compound extensions stripped as a whole when computing a stem.
const COMPOUND: &[&str] = &[
    ".ome.tiff",
    ".ome.tif",
    ".ome.btf",
    ".ome.tf2",
    ".ome.tf8",
    ".ome.zarr",
    ".asm.json",
    ".companion.ome",
    ".mzml.gz",
    ".mzxml.gz",
];

/// The data-set stem of a file name: lower case, without its (compound) extension:
/// `Cells_01.ome.tiff` → `cells_01`, `run42.raw` → `run42`, `sample.D` → `sample`.
pub fn stem(name: &str) -> String {
    let lower = name.to_lowercase();
    for c in COMPOUND {
        if let Some(s) = lower.strip_suffix(c) {
            return s.to_string();
        }
    }
    match lower.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s.to_string(),
        _ => lower,
    }
}

/// The (compound) extension of a file name, lower case, without the dot (`ome.tiff`, `csv`).
pub fn full_extension(name: &str) -> String {
    let lower = name.to_lowercase();
    for c in COMPOUND {
        if lower.ends_with(c) {
            return c.trim_start_matches('.').to_string();
        }
    }
    lower
        .rsplit_once('.')
        .map(|(_, e)| e.to_string())
        .unwrap_or_default()
}

/// Is a file with this name an open export (by extension)?
pub fn is_open_export_name(name: &str) -> bool {
    OPEN_EXPORT_EXTENSIONS.contains(&full_extension(name).as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_and_classes() {
        assert_eq!(stem("Cells_01.ome.tiff"), "cells_01");
        assert_eq!(stem("run42.raw"), "run42");
        assert_eq!(stem("sample.D"), "sample");
        assert_eq!(stem("README"), "readme");
        assert_eq!(full_extension("x.ome.zarr"), "ome.zarr");
        assert!(is_open_export_name("a.ome.tif"));
        assert!(is_open_export_name("a.csv"));
        assert!(!is_open_export_name("a.czi"));
        assert_eq!(preservation("czi"), "vendor");
        assert_eq!(preservation("zvi"), "legacy");
        assert_eq!(preservation("mzml"), "open");
    }
}
