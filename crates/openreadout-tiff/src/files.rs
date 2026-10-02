//! The set of files an image may span: the opened file plus OME-TIFF siblings and
//! companion metadata files. Siblings are opened lazily, on the first plane read or `check`.

use std::path::{Path, PathBuf};

use openreadout_core::{Error, Result};

use crate::FORMAT_ID;
use openreadout_core::source::Fs;

use crate::container::{ByteSource, TiffFile};

/// One member of the file set.
#[derive(Debug)]
pub struct FileSetMember {
    pub path: PathBuf,
    /// File name as referenced (OME `UUID/@FileName`), or the opened file's own name.
    pub name: String,
    /// OME UUID this file is referenced by, when known.
    pub uuid: Option<String>,
    /// `true` for a pure-XML companion (`*.companion.ome`), which holds no pixels.
    pub metadata_only: bool,
    pub(crate) opened: Option<(TiffFile, ByteSource)>,
    /// Why the file could not be opened (missing, not a TIFF, ...).
    pub(crate) error: Option<String>,
}

impl FileSetMember {
    pub(crate) fn tiff(path: PathBuf, name: String, opened: (TiffFile, ByteSource)) -> Self {
        FileSetMember {
            path,
            name,
            uuid: None,
            metadata_only: false,
            opened: Some(opened),
            error: None,
        }
    }

    pub(crate) fn pending(path: PathBuf, name: String, uuid: Option<String>) -> Self {
        FileSetMember {
            path,
            name,
            uuid,
            metadata_only: false,
            opened: None,
            error: None,
        }
    }

    /// Open the file if needed. Errors are remembered so a missing sibling is reported once.
    pub(crate) fn ensure_open(&mut self, fs: &Fs) -> Result<&mut (TiffFile, ByteSource)> {
        if self.opened.is_none() {
            if let Some(e) = &self.error {
                return Err(Error::corrupt(FORMAT_ID, e.clone()));
            }
            if !fs.exists(&self.path) {
                let msg = format!(
                    "file '{}' of the data set is missing (looked for {})",
                    self.name,
                    self.path.display()
                );
                self.error = Some(msg.clone());
                return Err(Error::Corrupt {
                    format: FORMAT_ID,
                    detail: msg,
                    offset: None,
                });
            }
            match TiffFile::open_in(fs, &self.path) {
                Ok(o) => self.opened = Some(o),
                Err(e) => {
                    self.error = Some(format!("'{}': {e}", self.name));
                    return Err(e);
                }
            }
        }
        Ok(self.opened.as_mut().expect("opened above"))
    }

    pub(crate) fn page_count(&mut self, fs: &Fs) -> Option<usize> {
        self.ensure_open(fs).ok().map(|(t, _)| t.ifds.len())
    }
}

/// Sibling path for a referenced file name (always resolved in the opened file's directory).
pub(crate) fn sibling(base: &Path, name: &str) -> PathBuf {
    let dir = base.parent().unwrap_or_else(|| Path::new("."));
    // A FileName is a bare name per the OME-TIFF specification; strip any directory part
    // so a crafted name cannot point outside the dataset's folder.
    let leaf = Path::new(name)
        .file_name()
        .map_or_else(|| name.to_string(), |s| s.to_string_lossy().to_string());
    dir.join(leaf)
}

pub(crate) fn file_name_of(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}
