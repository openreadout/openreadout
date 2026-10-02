//! Batch inputs: turn command-line paths (files, directories, glob patterns) into the list of
//! items a batch command processes, in a deterministic order.
//!
//! - A file is one item.
//! - A directory that a reader detects as a data set (for formats stored as directories) is
//!   one item and is not walked into.
//! - Any other directory contributes its entries, sorted by name: files, and directory data
//!   sets. With `recursive`, sub-directories are walked too. Hidden entries (names starting
//!   with `.`) are skipped while walking; paths named explicitly are always used.
//! - A path that does not exist but contains `*`, `?` or `[` is expanded as a glob pattern
//!   (the Windows shells do not expand globs; POSIX shells already did).
//! - `-` (standard input) is passed through as is.
//!
//! Symbolic links are followed; a directory reached twice (a link cycle) is walked once.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::DetectConfidence;
use crate::{Error, Registry};

/// How to expand inputs.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct ExpandOptions {
    /// Walk sub-directories.
    pub recursive: bool,
    /// Expand glob patterns in paths that do not exist.
    pub glob: bool,
    /// Decides whether a directory is one data set (default: [`is_dataset_dir_in_walk`] with
    /// `recursive`, which also walks into a directory a reader only *likely* recognises when it
    /// holds data-set directories of its own, such as a folder of VnmrJ `.fid` directories;
    /// else [`is_dataset_dir`]).
    pub dataset_dir: Option<fn(&Registry, &Path) -> bool>,
}

/// One item to process.
#[derive(Debug)]
pub struct BatchInput {
    /// The file or directory to process.
    pub path: PathBuf,
    /// The path relative to the argument it came from (its file name for files named
    /// directly or matched by a glob); used to mirror directory trees under an output
    /// directory.
    pub relative: PathBuf,
    /// Set when the argument could not be expanded (missing path, bad pattern, unreadable
    /// directory); the item is then reported as that error.
    pub error: Option<Error>,
}

/// The expanded inputs.
#[derive(Debug, Default)]
pub struct Expansion {
    /// Inputs in argument order (directories walked in sorted order).
    pub items: Vec<BatchInput>,
    /// True when a directory or a glob contributed items (so the command runs in batch mode
    /// even with a single argument).
    pub expanded: bool,
}

/// Does this path contain glob metacharacters?
pub fn has_glob_meta(p: &Path) -> bool {
    p.to_string_lossy().contains(['*', '?', '['])
}

/// Is `dir` a directory data set (detected by a reader with more than extension-only
/// confidence)?
pub fn is_dataset_dir(reg: &Registry, dir: &Path) -> bool {
    reg.detect(dir)
        .is_ok_and(|(_, d)| d.confidence != DetectConfidence::ExtensionOnly)
}

/// Is `dir` one data set in a recursive walk? Like [`is_dataset_dir`], except that a directory
/// a reader only *likely* recognises is walked into when it holds data-set directories of its
/// own: a Bruker project holding experiments `1/`, `2/`, a folder of VnmrJ `.fid` directories,
/// or a folder mixing both, so each experiment is its own item. A sub-directory counts when it
/// is detected definitely, or as another format (a `TimePoint_1/` folder that the same reader
/// only likely recognises is part of its parent, not a data set of its own).
pub fn is_dataset_dir_in_walk(reg: &Registry, dir: &Path) -> bool {
    let Ok((_, det)) = reg.detect(dir) else {
        return false;
    };
    match det.confidence {
        DetectConfidence::Definite => true,
        DetectConfidence::ExtensionOnly => false,
        DetectConfidence::Likely => {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return true;
            };
            !rd.filter_map(std::result::Result::ok).any(|e| {
                !e.file_name().to_string_lossy().starts_with('.')
                    && e.file_type().is_ok_and(|t| t.is_dir())
                    && reg
                        .detect(&e.path())
                        .is_ok_and(|(_, d)| match d.confidence {
                            DetectConfidence::Definite => true,
                            // another format's data set, or a same-format child that is itself a
                            // container (Spinsolve keeps experiments at `<study>/<date>/<exp>/`)
                            DetectConfidence::Likely => {
                                d.format_id != det.format_id
                                    || !is_dataset_dir_in_walk(reg, &e.path())
                            }
                            DetectConfidence::ExtensionOnly => false,
                        })
            })
        }
    }
}

/// File-name suffix of the metadata sidecars `info --view full --sidecar` writes
/// (`<file>.openreadout.json`); directory walks leave them out.
pub const SIDECAR_SUFFIX: &str = ".openreadout.json";

/// Expand `args` into items.
pub fn expand(reg: &Registry, args: &[PathBuf], opts: ExpandOptions) -> Expansion {
    let mut out = Expansion::default();
    let mut visited = HashSet::new();
    for arg in args {
        if arg.as_os_str() == "-" {
            out.items.push(BatchInput {
                path: arg.clone(),
                relative: arg.clone(),
                error: None,
            });
            continue;
        }
        if opts.glob && !arg.exists() && has_glob_meta(arg) {
            out.expanded = true;
            let pattern = arg.to_string_lossy();
            match glob::glob(&pattern) {
                Err(e) => out.items.push(BatchInput {
                    path: arg.clone(),
                    relative: arg.clone(),
                    error: Some(Error::Usage(format!("bad glob pattern '{pattern}': {e}"))),
                }),
                Ok(paths) => {
                    let mut matched: Vec<PathBuf> =
                        paths.filter_map(std::result::Result::ok).collect();
                    matched.sort();
                    if matched.is_empty() {
                        out.items.push(BatchInput {
                            path: arg.clone(),
                            relative: arg.clone(),
                            error: Some(Error::io(
                                arg,
                                std::io::Error::new(
                                    std::io::ErrorKind::NotFound,
                                    "no file matches this pattern",
                                ),
                            )),
                        });
                    }
                    for m in matched {
                        add_path(reg, &m, &m, opts, true, &mut visited, &mut out);
                    }
                }
            }
            continue;
        }
        add_path(reg, arg, arg, opts, true, &mut visited, &mut out);
    }
    out
}

fn relative_to(path: &Path, root: &Path) -> PathBuf {
    match path.strip_prefix(root) {
        Ok(r) if !r.as_os_str().is_empty() => r.to_path_buf(),
        _ => path
            .file_name()
            .map_or_else(|| path.to_path_buf(), PathBuf::from),
    }
}

fn add_path(
    reg: &Registry,
    path: &Path,
    root: &Path,
    opts: ExpandOptions,
    top: bool,
    visited: &mut HashSet<PathBuf>,
    out: &mut Expansion,
) {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            out.items.push(BatchInput {
                path: path.to_path_buf(),
                relative: relative_to(path, root),
                error: Some(Error::io(path, e)),
            });
            return;
        }
    };
    let dataset_dir = opts.dataset_dir.unwrap_or(if opts.recursive {
        is_dataset_dir_in_walk
    } else {
        is_dataset_dir
    });
    if !meta.is_dir() || dataset_dir(reg, path) {
        out.items.push(BatchInput {
            path: path.to_path_buf(),
            relative: relative_to(path, root),
            error: None,
        });
        return;
    }
    if !top && !opts.recursive {
        return;
    }
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canon) {
        return;
    }
    out.expanded = true;
    let rd = match std::fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => {
            out.items.push(BatchInput {
                path: path.to_path_buf(),
                relative: relative_to(path, root),
                error: Some(Error::io(path, e)),
            });
            return;
        }
    };
    let mut entries: Vec<PathBuf> = rd
        .filter_map(std::result::Result::ok)
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            // hidden entries, and the metadata sidecars `info --view full --sidecar` writes next to
            // inputs
            !n.starts_with('.') && !n.ends_with(SIDECAR_SUFFIX)
        })
        .map(|e| e.path())
        .collect();
    entries.sort();
    for e in entries {
        add_path(reg, &e, root, opts, false, visited, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_sorted_skips_hidden_and_respects_recursion() {
        let base = std::env::temp_dir().join(format!("openreadout-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub/deeper")).unwrap();
        for f in [
            "b.txt",
            "a.txt",
            ".hidden",
            "a.txt.openreadout.json",
            "sub/c.txt",
            "sub/deeper/d.txt",
        ] {
            std::fs::write(base.join(f), b"x").unwrap();
        }
        let reg = Registry::new();
        let flat = expand(&reg, std::slice::from_ref(&base), ExpandOptions::default());
        let names: Vec<String> = flat
            .items
            .iter()
            .map(|i| i.relative.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(names, ["a.txt", "b.txt"]);
        assert!(flat.expanded);
        let deep = expand(
            &reg,
            std::slice::from_ref(&base),
            ExpandOptions {
                recursive: true,
                glob: true,
                dataset_dir: None,
            },
        );
        let names: Vec<String> = deep
            .items
            .iter()
            .map(|i| i.relative.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(names, ["a.txt", "b.txt", "sub/c.txt", "sub/deeper/d.txt"]);
        let pat = base.join("*.txt");
        let g = expand(
            &reg,
            &[pat],
            ExpandOptions {
                recursive: false,
                glob: true,
                dataset_dir: None,
            },
        );
        assert_eq!(g.items.len(), 2);
        let none = expand(
            &reg,
            &[base.join("*.nothing")],
            ExpandOptions {
                recursive: false,
                glob: true,
                dataset_dir: None,
            },
        );
        assert_eq!(none.items.len(), 1);
        assert_eq!(none.items[0].error.as_ref().unwrap().exit_code(), 5);
        let missing = expand(&reg, &[base.join("nope")], ExpandOptions::default());
        assert!(!missing.expanded);
        assert!(missing.items[0].error.is_some());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Test reader: `*.proj` folders are likely (a project), `*.exp` folders definite (one
    /// experiment); `*.tp` folders are likely of the project's own format (a time-point folder).
    #[derive(Debug)]
    struct Folders;

    impl crate::FormatReader for Folders {
        fn descriptor(&self) -> crate::model::FormatDescriptor {
            crate::model::FormatDescriptor {
                id: "folders".into(),
                name: "Folder test format".into(),
                vendor: String::new(),
                extensions: Vec::new(),
                family: "test".into(),
                can_read: true,
                can_write: false,
                confidence: crate::provenance::Confidence::Low,
                known_gaps: Vec::new(),
            }
        }
        fn sniff(&self, _head: &[u8], path: &Path) -> Option<crate::reader::Detection> {
            let ext = path.extension()?.to_str()?;
            let (format_id, confidence) = match ext {
                "proj" | "tp" => ("folders", DetectConfidence::Likely),
                "exp" => ("experiment", DetectConfidence::Definite),
                _ => return None,
            };
            Some(crate::reader::Detection {
                format_id,
                confidence,
                note: None,
            })
        }
        fn open(&self, path: &Path) -> crate::Result<Box<dyn crate::Dataset>> {
            Err(Error::Other(format!("opened {}", path.display())))
        }
    }

    #[test]
    fn recursive_walks_enter_likely_folders_holding_data_sets() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        for d in ["mixed.proj/1.exp", "mixed.proj/2.exp", "plate.proj/t1.tp"] {
            std::fs::create_dir_all(base.join(d)).unwrap();
        }
        std::fs::write(base.join("mixed.proj/notes.txt"), b"x").unwrap();
        let reg = Registry::new().with(Box::new(Folders));
        let names = |opts: ExpandOptions, arg: &Path| -> Vec<String> {
            expand(&reg, &[arg.to_path_buf()], opts)
                .items
                .iter()
                .map(|i| i.relative.to_string_lossy().replace('\\', "/"))
                .collect()
        };
        let rec = ExpandOptions {
            recursive: true,
            ..ExpandOptions::default()
        };
        // A project of experiments: each experiment (and the loose file) is its own input; a
        // folder whose sub-folders are only likely its own format stays one input.
        assert_eq!(
            names(rec, base),
            [
                "mixed.proj/1.exp",
                "mixed.proj/2.exp",
                "mixed.proj/notes.txt",
                "plate.proj"
            ]
        );
        // Without -r a project named on the command line stays one input.
        assert_eq!(
            names(ExpandOptions::default(), &base.join("mixed.proj")),
            ["mixed.proj"]
        );
        assert!(is_dataset_dir(&reg, &base.join("mixed.proj")));
        assert!(!is_dataset_dir_in_walk(&reg, &base.join("mixed.proj")));
        assert!(is_dataset_dir_in_walk(&reg, &base.join("plate.proj")));
    }
}
