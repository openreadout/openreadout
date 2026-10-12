//! Tucker-Davis Technologies (TDT) tanks and blocks: a block is a directory of files sharing a
//! stem (`.tsq` index, `.tev` data, `.Tbk` store headers, `.Tdx`, `.tin`, `.tnt`, per-channel
//! `.sev`); a tank is a directory of blocks. Stream stores become traces, snippets the `spikes`
//! table and strobes (epocs) the `epocs` table. Notes and vocabulary: `docs/formats/tdt-tank.md`;
//! provenance: `docs/provenance/tdt-tank.md`.

use std::path::{Path, PathBuf};

use openreadout_core::model::{DetectConfidence, FormatDescriptor};
use openreadout_core::reader::{Dataset, Detection, FormatReader, has_extension};
use openreadout_core::source::{Fs, Input};
use openreadout_core::{Error, Result};

pub mod dataset;
pub mod tsq;
#[cfg(test)]
mod tests;

pub use dataset::TdtDataset;

/// Format id of TDT tanks and blocks.
pub const TDT_FORMAT_ID: &str = "tdt-tank";
/// Extensions of the files of a block.
pub const TDT_EXTENSIONS: &[&str] = &["tsq", "tev", "tbk", "tdx", "tin", "tnt", "sev"];

/// The files of one block.
#[derive(Debug, Clone, Default)]
pub struct BlockFiles {
    /// The block directory.
    pub dir: PathBuf,
    /// The stem shared by the block's files.
    pub stem: String,
    /// The `.tsq` index.
    pub tsq: PathBuf,
    /// The `.tev` data file, if present.
    pub tev: Option<PathBuf>,
    /// The `.Tbk` store headers, if present.
    pub tbk: Option<PathBuf>,
    /// `.sev` files: path, store name and channel number.
    pub sev: Vec<(PathBuf, String, u16)>,
    /// Every file of the block (the stem's files and the `.sev` files).
    pub members: Vec<PathBuf>,
}

fn ext_of(name: &str) -> Option<String> {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
}

fn stem_of(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

/// The store and channel a `.sev` file name gives (`<stem>_<store>_Ch<n>.sev`).
pub fn sev_name_parts(stem: &str, name: &str) -> Option<(String, u16)> {
    let rest = stem_of(name).strip_prefix(stem)?.strip_prefix('_')?;
    let (store, ch) = rest.rsplit_once('_')?;
    let n = ch
        .strip_prefix("Ch")
        .or_else(|| ch.strip_prefix("ch"))?
        .parse()
        .ok()?;
    Some((store.to_string(), n))
}

fn file_names(fs: &Fs, dir: &Path) -> Vec<(String, bool)> {
    let Ok(rd) = fs.read_dir(dir) else {
        return Vec::new();
    };
    let mut v: Vec<(String, bool)> = rd
        .filter_map(std::result::Result::ok)
        .take(100_000)
        .map(|e| {
            (
                e.file_name().to_string_lossy().to_string(),
                e.metadata().is_ok_and(|m| m.is_dir()),
            )
        })
        .collect();
    v.sort();
    v
}

/// The block in `dir`: its one `.tsq` (or the one whose stem is `prefer`). `Ok(None)` when the
/// directory holds no `.tsq`.
pub fn block_in_dir(fs: &Fs, dir: &Path, prefer: Option<&str>) -> Result<Option<BlockFiles>> {
    let names = file_names(fs, dir);
    let tsqs: Vec<&str> = names
        .iter()
        .filter(|(n, d)| !d && ext_of(n).as_deref() == Some("tsq"))
        .map(|(n, _)| n.as_str())
        .collect();
    let tsq = match (prefer, tsqs.as_slice()) {
        (_, []) => return Ok(None),
        (Some(p), many) if many.iter().any(|n| stem_of(n) == p) => {
            *many.iter().find(|n| stem_of(n) == p).expect("checked")
        }
        (_, [one]) => *one,
        (_, many) => {
            return Err(Error::unsupported(
                TDT_FORMAT_ID,
                format!("a directory with {} .tsq files", many.len()),
                format!(
                    "{} holds several TDT blocks; open one of their .tsq files",
                    dir.display()
                ),
            ));
        }
    };
    let stem = stem_of(tsq).to_string();
    let mut b = BlockFiles {
        dir: dir.to_path_buf(),
        stem: stem.clone(),
        tsq: dir.join(tsq),
        ..BlockFiles::default()
    };
    for (n, is_dir) in &names {
        if *is_dir {
            continue;
        }
        let ext = ext_of(n);
        if stem_of(n) == stem {
            match ext.as_deref() {
                Some("tev") => b.tev = Some(dir.join(n)),
                Some("tbk") => b.tbk = Some(dir.join(n)),
                _ => {}
            }
            b.members.push(dir.join(n));
        } else if ext.as_deref() == Some("sev")
            && let Some((store, ch)) = sev_name_parts(&stem, n)
        {
            b.sev.push((dir.join(n), store, ch));
            b.members.push(dir.join(n));
        }
    }
    Ok(Some(b))
}

/// The blocks an input names: one block (a block directory or one of its files), or every block
/// of a tank directory. The flag is true for a tank.
pub fn find_blocks(input: &Input) -> Result<(Vec<BlockFiles>, bool)> {
    let fs = input.fs();
    let path = input.path();
    if input.is_dir() {
        if let Some(b) = block_in_dir(fs, path, None)? {
            return Ok((vec![b], false));
        }
        let mut blocks = Vec::new();
        for (n, is_dir) in file_names(fs, path) {
            if is_dir
                && !n.starts_with('.')
                && let Some(b) = block_in_dir(fs, &path.join(&n), None)?
            {
                blocks.push(b);
            }
        }
        if blocks.is_empty() {
            return Err(Error::unsupported(
                TDT_FORMAT_ID,
                "a directory without TDT blocks",
                format!(
                    "{} holds no .tsq index, neither directly nor in its subdirectories; open a block directory or its .tsq file",
                    path.display()
                ),
            ));
        }
        return Ok((blocks, true));
    }
    let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let prefer = (ext_of(&name).as_deref() != Some("sev")).then(|| stem_of(&name).to_string());
    match block_in_dir(fs, &dir, prefer.as_deref())? {
        Some(b) => Ok((vec![b], false)),
        None => Err(Error::unsupported(
            TDT_FORMAT_ID,
            "TDT files without a .tsq index",
            format!(
                "{} has no .tsq index next to it; the index is needed to read a block (copy the whole block directory)",
                path.display()
            ),
        )),
    }
}

/// True when every visible entry of the directory is a subdirectory or a small note file, and at
/// least one subdirectory holds a `.tsq` (a tank).
fn looks_like_tank(fs: &Fs, dir: &Path) -> bool {
    let names = file_names(fs, dir);
    let mut blocks = 0;
    for (n, is_dir) in &names {
        if n.starts_with('.') {
            continue;
        }
        if *is_dir {
            if file_names(fs, &dir.join(n))
                .iter()
                .any(|(m, d)| !d && ext_of(m).as_deref() == Some("tsq"))
            {
                blocks += 1;
            }
        } else if !matches!(ext_of(n).as_deref(), Some("ini" | "txt" | "md" | "ok")) {
            return false;
        }
    }
    blocks > 0
}

fn looks_like_block_dir(fs: &Fs, dir: &Path) -> bool {
    let names = file_names(fs, dir);
    names.iter().any(|(n, d)| {
        !d && ext_of(n).as_deref() == Some("tsq")
            && names.iter().any(|(m, e)| {
                !e && stem_of(m) == stem_of(n) && ext_of(m).as_deref() == Some("tev")
            })
    })
}

/// The TDT reader.
#[derive(Debug, Default, Clone, Copy)]
pub struct TdtReader;

impl FormatReader for TdtReader {
    fn assurance(&self) -> Option<&'static openreadout_core::assurance::AssuranceProfile> {
        Some(&crate::assurance::TDT)
    }

    fn descriptor(&self) -> FormatDescriptor {
        FormatDescriptor {
            id: TDT_FORMAT_ID.into(),
            name: "Tucker-Davis Technologies tank or block (.tsq/.tev)".into(),
            vendor: "Tucker-Davis Technologies (OpenEx, Synapse)".into(),
            extensions: TDT_EXTENSIONS.iter().map(|e| (*e).to_string()).collect(),
            family: "electrophysiology".into(),
            can_read: true,
            can_write: false,
            confidence: crate::assurance::TDT.confidence,
            known_gaps: vec![
                "Input is a block directory, one of its files, or a tank directory (blocks become sweeps)".into(),
                "Stream and snippet values are returned as stored, without units: TDT files record no unit or gain".into(),
                "Stores whose samples are in .sev files are listed, not read; scalar records are counted, not decoded".into(),
                "Samples indexed past the end of a cut-short .tev are not returned (the trace ends at the last whole chunk)".into(),
            ],
        }
    }

    fn sniff(&self, head: &[u8], path: &Path) -> Option<Detection> {
        self.sniff_input(head, &Input::local(path))
    }

    fn sniff_input(&self, head: &[u8], input: &Input) -> Option<Detection> {
        let path = input.path();
        let fs = input.fs();
        if input.is_dir() {
            if looks_like_block_dir(fs, path) {
                return Some(Detection {
                    format_id: TDT_FORMAT_ID,
                    confidence: DetectConfidence::Definite,
                    note: Some("TDT block directory".into()),
                });
            }
            if looks_like_tank(fs, path) {
                return Some(Detection {
                    format_id: TDT_FORMAT_ID,
                    confidence: DetectConfidence::Definite,
                    note: Some("TDT tank directory (each block is a sweep)".into()),
                });
            }
            return None;
        }
        if has_extension(path, &["tsq"]) {
            return Some(if tsq::looks_like_tsq(head) {
                Detection {
                    format_id: TDT_FORMAT_ID,
                    confidence: DetectConfidence::Definite,
                    note: Some("reads the block the index belongs to".into()),
                }
            } else {
                Detection {
                    format_id: TDT_FORMAT_ID,
                    confidence: DetectConfidence::ExtensionOnly,
                    note: Some("TDT .tsq extension but no block start mark".into()),
                }
            });
        }
        if has_extension(path, &["tev", "tbk", "tdx", "tin", "tnt", "sev"]) {
            let dir = path.parent().unwrap_or(Path::new(""));
            let has_tsq = file_names(fs, dir)
                .iter()
                .any(|(n, d)| !d && ext_of(n).as_deref() == Some("tsq"));
            return Some(Detection {
                format_id: TDT_FORMAT_ID,
                confidence: if has_tsq {
                    DetectConfidence::Definite
                } else {
                    DetectConfidence::ExtensionOnly
                },
                note: Some(if has_tsq {
                    "reads the block this file belongs to".into()
                } else {
                    "TDT extension, but no .tsq index in the same directory".into()
                }),
            });
        }
        None
    }

    fn open(&self, path: &Path) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(TdtDataset::open(path)?))
    }

    fn open_input(&self, input: &Input) -> Result<Box<dyn Dataset>> {
        Ok(Box::new(TdtDataset::open_input(input)?))
    }

    fn reads_any_source(&self) -> bool {
        true
    }
}
