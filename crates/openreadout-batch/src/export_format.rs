//! The export format an output name implies, shared by `openreadout export` and the
//! `openreadout_export` tool so that both read `-o x.ome.zarr` the same way.

use std::path::Path;

use openreadout_core::{Error, Result};

/// Output extensions and the format each one implies. Matched against the end of the
/// lowercased file name, so `.ome.tiff` and `.tiff` give the same answer.
const BY_EXTENSION: &[(&str, &str)] = &[
    (".tiff", "ome-tiff"),
    (".tif", "ome-tiff"),
    (".zarr", "ome-zarr"),
    (".mzml", "mzml"),
    (".csv", "csv"),
    (".parquet", "parquet"),
    (".arrow", "arrow"),
    (".feather", "arrow"),
    (".nwb", "nwb"),
    (".jdx", "jcamp"),
    (".dx", "jcamp"),
    (".json", "asm"),
    (".rdml", "rdml"),
];

/// The extension each format's default output name ends in.
const DEFAULT_EXTENSION: &[(&str, &str)] = &[
    ("ome-tiff", ".ome.tiff"),
    ("ome-zarr", ".ome.zarr"),
    ("mzml", ".mzML"),
    ("csv", ".csv"),
    ("parquet", ".parquet"),
    ("arrow", ".arrow"),
    ("nwb", ".nwb"),
    ("jcamp", ".jdx"),
    ("asm", ".asm.json"),
    ("rdml", ".rdml"),
];

/// Words in both error messages. `Error::hint` looks for them to choose the hint.
const OUTPUT_NAME_PHRASE: &str = "the output name `";

/// The format name for `name` or one of its aliases (`zarr`, `feather`, `jdx`, ...), as
/// `--format` spells it. `None` for a name that is not an export format.
pub fn canonical(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "ome-tiff" | "ometiff" | "tiff" => "ome-tiff",
        "ome-zarr" | "omezarr" | "zarr" => "ome-zarr",
        "mzml" => "mzml",
        "csv" => "csv",
        "parquet" => "parquet",
        "arrow" | "feather" | "ipc" => "arrow",
        "nwb" => "nwb",
        "jcamp" | "jcamp-dx" | "jdx" => "jcamp",
        "asm" | "allotrope" => "asm",
        "rdml" => "rdml",
        _ => return None,
    })
}

/// The format the extension of `output` implies, if it implies one.
pub fn from_output(output: &Path) -> Option<&'static str> {
    let name = output.file_name()?.to_string_lossy().to_ascii_lowercase();
    BY_EXTENSION
        .iter()
        .find(|(ext, _)| name.len() > ext.len() && name.ends_with(ext))
        .map(|&(_, format)| format)
}

/// The format to write to `output`: the one asked for (`requested`, a name from
/// [`canonical`]) or, when none is, the one the output's extension implies.
///
/// An error (exit code 2) when nothing is asked for and the extension implies no format, or
/// when the extension implies a different format from the one asked for. An explicit format
/// with an extension that implies none is accepted. `flag` is how the caller spells the
/// format argument (`--format` on the command line, `format` in MCP).
pub fn resolve(requested: Option<&'static str>, output: &Path, flag: &str) -> Result<&'static str> {
    let shown = output.display();
    match (requested, from_output(output)) {
        (Some(asked), Some(implied)) if asked != implied => Err(Error::Usage(format!(
            "{flag} is {asked}, but {OUTPUT_NAME_PHRASE}{shown}` means {implied}: drop {flag}, or end the output name in {}",
            default_extension(asked)
        ))),
        (Some(asked), _) => Ok(asked),
        (None, Some(implied)) => Ok(implied),
        (None, None) => Err(Error::Usage(format!(
            "{OUTPUT_NAME_PHRASE}{shown}` does not say which format to export: give {flag} (ome-tiff, ome-zarr, mzml, csv, parquet, arrow, nwb, jcamp, asm or rdml), or end the name in {}",
            DEFAULT_EXTENSION
                .iter()
                .map(|(_, ext)| *ext)
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// The extension of `format`'s default output name (`.ome.tiff` for `ome-tiff`).
pub fn default_extension(format: &str) -> &'static str {
    DEFAULT_EXTENSION
        .iter()
        .find(|(f, _)| *f == format)
        .map_or("", |&(_, ext)| ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_imply_formats() {
        for (name, format) in [
            ("x.ome.tiff", "ome-tiff"),
            ("x.ome.tif", "ome-tiff"),
            ("x.TIF", "ome-tiff"),
            ("x.ome.zarr", "ome-zarr"),
            ("dir/x.zarr", "ome-zarr"),
            ("x.mzML", "mzml"),
            ("x.csv", "csv"),
            ("x.parquet", "parquet"),
            ("x.arrow", "arrow"),
            ("x.nwb", "nwb"),
            ("x.jdx", "jcamp"),
            ("x.asm.json", "asm"),
            ("x.rdml", "rdml"),
        ] {
            assert_eq!(from_output(Path::new(name)), Some(format), "{name}");
        }
        for name in ["x", "x.txt", ".zarr", "x.lif"] {
            assert_eq!(from_output(Path::new(name)), None, "{name}");
        }
    }

    #[test]
    fn every_format_has_a_default_extension_that_implies_it() {
        for &(format, ext) in DEFAULT_EXTENSION {
            assert_eq!(canonical(format), Some(format));
            assert_eq!(from_output(Path::new(&format!("x{ext}"))), Some(format));
        }
    }

    #[test]
    fn resolve_infers_and_refuses() {
        assert_eq!(
            resolve(None, Path::new("x.ome.zarr"), "--format").unwrap(),
            "ome-zarr"
        );
        assert_eq!(
            resolve(Some("csv"), Path::new("x.txt"), "--format").unwrap(),
            "csv"
        );
        let e = resolve(Some("ome-tiff"), Path::new("x.ome.zarr"), "--format").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.to_string().contains(".ome.tiff"), "{e}");
        assert!(e.hint().unwrap().contains(".ome.zarr"));
        let e = resolve(None, Path::new("x.out"), "format").unwrap_err();
        assert_eq!(e.exit_code(), 2);
        assert!(e.to_string().contains("give format"), "{e}");
    }
}
