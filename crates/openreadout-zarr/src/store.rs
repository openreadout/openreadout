//! Where a Zarr hierarchy lives: a directory, or a zip archive (the store at the archive root,
//! or inside a single top-level directory as zipped `.zarr` folders are usually packed).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use openreadout_core::source::Fs;
use openreadout_core::zip::ZipIndex;
use openreadout_core::{Error, Result};
use serde_json::Value;
#[cfg(feature = "local-store")]
use zarrs::filesystem::FilesystemStore;
use zarrs::storage::byte_range::{ByteRange, ByteRangeIterator};
use zarrs::storage::{Bytes, MaybeBytesIterator, ReadableStorageTraits, StorageError, StoreKey};

use crate::FORMAT_ID;

/// Longest zip member decompressed into memory (a Zarr chunk; larger members are refused).
const MAX_ZIP_MEMBER: u64 = 1 << 32;

/// Kind of store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Directory,
    Zip,
}

impl StoreKind {
    pub fn name(self) -> &'static str {
        match self {
            StoreKind::Directory => "directory",
            StoreKind::Zip => "zip",
        }
    }
}

/// `zarrs` storage over a zip archive.
#[derive(Debug)]
pub struct ZipStorage {
    index: Arc<ZipIndex>,
    prefix: String,
}

fn storage_err(e: &Error) -> StorageError {
    StorageError::Other(e.to_string())
}

impl ReadableStorageTraits for ZipStorage {
    fn get_partial_many<'a>(
        &'a self,
        key: &StoreKey,
        byte_ranges: ByteRangeIterator<'a>,
    ) -> std::result::Result<MaybeBytesIterator<'a>, StorageError> {
        let name = format!("{}{}", self.prefix, key.as_str());
        let Some(m) = self.index.get_exact(&name) else {
            return Ok(None);
        };
        let ranges: Vec<ByteRange> = byte_ranges.collect();
        let mut out = Vec::with_capacity(ranges.len());
        if m.method == 0 && !m.encrypted {
            for r in ranges {
                let rr = r.to_range(m.size);
                if rr.end > m.size || rr.start > rr.end {
                    return Err(StorageError::Other(format!(
                        "{name}: byte range out of bounds"
                    )));
                }
                let b = self
                    .index
                    .read_stored_range(m, rr.start, rr.end - rr.start)
                    .map_err(|e| storage_err(&e))?;
                out.push(Ok(Bytes::from(b)));
            }
        } else {
            let all = self.index.read(m).map_err(|e| storage_err(&e))?;
            let size = all.len() as u64;
            for r in ranges {
                let rr = r.to_range(size);
                if rr.end > size || rr.start > rr.end {
                    return Err(StorageError::Other(format!(
                        "{name}: byte range out of bounds"
                    )));
                }
                out.push(Ok(Bytes::copy_from_slice(
                    &all[rr.start as usize..rr.end as usize],
                )));
            }
        }
        Ok(Some(Box::new(out.into_iter())))
    }

    fn size_key(&self, key: &StoreKey) -> std::result::Result<Option<u64>, StorageError> {
        Ok(self
            .index
            .get_exact(&format!("{}{}", self.prefix, key.as_str()))
            .map(|m| m.size))
    }

    fn supports_get_partial(&self) -> bool {
        true
    }
}

/// A directory store read through a byte-source namespace (a folder dropped into a browser, a
/// directory held in memory; local directories when `local-store` is off).
struct DirStorage {
    fs: Fs,
    root: PathBuf,
}

fn source_err(e: &std::io::Error) -> StorageError {
    StorageError::Other(e.to_string())
}

impl ReadableStorageTraits for DirStorage {
    fn get_partial_many<'a>(
        &'a self,
        key: &StoreKey,
        byte_ranges: ByteRangeIterator<'a>,
    ) -> std::result::Result<MaybeBytesIterator<'a>, StorageError> {
        let p = self.root.join(key.as_str());
        let src = match self.fs.source(&p) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(source_err(&e)),
        };
        let size = src.size().map_err(|e| source_err(&e))?;
        let mut out = Vec::new();
        for r in byte_ranges {
            let rr = r.to_range(size);
            if rr.end > size || rr.start > rr.end {
                return Err(StorageError::Other(format!(
                    "{}: byte range out of bounds",
                    key.as_str()
                )));
            }
            let n = usize::try_from(rr.end - rr.start)
                .map_err(|_| StorageError::Other(format!("{}: range too large", key.as_str())))?;
            let mut buf = vec![0u8; n];
            src.read_exact_at(rr.start, &mut buf)
                .map_err(|e| source_err(&e))?;
            out.push(Ok(Bytes::from(buf)));
        }
        Ok(Some(Box::new(out.into_iter())))
    }

    fn size_key(&self, key: &StoreKey) -> std::result::Result<Option<u64>, StorageError> {
        Ok(self
            .fs
            .metadata(&self.root.join(key.as_str()))
            .ok()
            .filter(openreadout_core::source::EntryMeta::is_file)
            .map(|m| m.len()))
    }

    fn supports_get_partial(&self) -> bool {
        true
    }
}

/// An opened store.
pub struct Store {
    pub kind: StoreKind,
    /// The directory or zip file.
    pub path: PathBuf,
    /// For zip stores: the top-level directory holding the hierarchy (`""` at the root).
    pub prefix: String,
    pub storage: Arc<dyn ReadableStorageTraits>,
    zip: Option<Arc<ZipIndex>>,
    /// Where the store is read from.
    pub(crate) fs: Fs,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("kind", &self.kind)
            .field("path", &self.path)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

/// The storage of a directory store: zarrs' filesystem store for local directories (with the
/// `local-store` feature), otherwise [`DirStorage`] over the namespace.
#[allow(clippy::unnecessary_wraps)] // fallible with the `local-store` feature
fn directory_storage(fs: &Fs, path: &Path) -> Result<Arc<dyn ReadableStorageTraits>> {
    #[cfg(feature = "local-store")]
    if fs.is_local() {
        let store = FilesystemStore::new(path)
            .map_err(|e| Error::Other(format!("zarr directory store: {e}")))?;
        return Ok(Arc::new(store));
    }
    Ok(Arc::new(DirStorage {
        fs: fs.clone(),
        root: path.to_path_buf(),
    }))
}

/// Metadata document names that mark the root of a Zarr hierarchy.
const ROOT_DOCS: [&str; 3] = ["zarr.json", ".zgroup", ".zattrs"];

impl Store {
    /// Open `path`: a directory store, a zip store, or a metadata document inside a directory
    /// store (`zarr.json`, `.zattrs`, `.zgroup`: the store is its directory).
    pub fn open(path: &Path) -> Result<Store> {
        Self::open_in(&Fs::local(), path)
    }

    /// [`Store::open`] in the namespace `fs`.
    pub(crate) fn open_in(fs: &Fs, path: &Path) -> Result<Store> {
        let meta = fs.metadata(path).map_err(|e| Error::io(path, e))?;
        if meta.is_dir() {
            return Ok(Store {
                kind: StoreKind::Directory,
                path: path.to_path_buf(),
                prefix: String::new(),
                storage: directory_storage(fs, path)?,
                zip: None,
                fs: fs.clone(),
            });
        }
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if ROOT_DOCS.contains(&fname)
            && let Some(dir) = path.parent()
        {
            let dir = if dir.as_os_str().is_empty() {
                Path::new(".")
            } else {
                dir
            };
            return Store::open_in(fs, dir);
        }
        let index = ZipIndex::open(fs, path, crate::FORMAT_ID)?.with_max_member(MAX_ZIP_MEMBER);
        let at_root = ROOT_DOCS.iter().any(|d| index.get_exact(d).is_some());
        let prefix = if at_root {
            String::new()
        } else {
            // one top-level directory holding the hierarchy (macOS archive metadata folders
            // beside it, `__MACOSX/`, do not count)
            let tops: std::collections::BTreeSet<&str> = index
                .members
                .iter()
                .filter_map(|m| m.name.split_once('/').map(|(t, _)| t))
                .filter(|t| *t != "__MACOSX")
                .collect();
            match tops.into_iter().collect::<Vec<_>>().as_slice() {
                [t] if ROOT_DOCS
                    .iter()
                    .any(|d| index.get_exact(&format!("{t}/{d}")).is_some()) =>
                {
                    format!("{t}/")
                }
                _ => {
                    return Err(Error::corrupt(
                        FORMAT_ID,
                        "zip archive holds no Zarr hierarchy (no zarr.json, .zgroup or .zattrs at its root or in a single top-level folder)",
                    ));
                }
            }
        };
        let index = Arc::new(index);
        Ok(Store {
            kind: StoreKind::Zip,
            path: path.to_path_buf(),
            prefix: prefix.clone(),
            storage: Arc::new(ZipStorage {
                index: index.clone(),
                prefix,
            }),
            zip: Some(index),
            fs: fs.clone(),
        })
    }

    /// Raw bytes of `key` (relative to the hierarchy root), `None` when absent.
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let k = StoreKey::new(key.trim_start_matches('/'))
            .map_err(|e| Error::Other(format!("zarr key {key}: {e}")))?;
        self.storage
            .get(&k)
            .map(|b| b.map(|b| b.to_vec()))
            .map_err(|e| Error::corrupt(FORMAT_ID, format!("{key}: {e}")))
    }

    /// Parse `key` as JSON, `None` when absent.
    pub fn json(&self, key: &str) -> Result<Option<Value>> {
        match self.get(key)? {
            None => Ok(None),
            Some(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| Error::corrupt(FORMAT_ID, format!("{key} is not valid JSON: {e}"))),
        }
    }

    /// Whether `key` exists.
    pub fn exists(&self, key: &str) -> bool {
        StoreKey::new(key.trim_start_matches('/'))
            .ok()
            .and_then(|k| self.storage.size_key(&k).ok().flatten())
            .is_some()
    }

    /// Number of members and their total size (zip stores only).
    pub fn zip_summary(&self) -> Option<(usize, u64)> {
        let z = self.zip.as_ref()?;
        let ms = z
            .members
            .iter()
            .filter(|m| m.name.starts_with(&self.prefix));
        let (n, total) = ms.fold((0usize, 0u64), |(n, t), m| (n + 1, t + m.compressed_size));
        Some((n, total))
    }

    /// Stored size of `key` in bytes, if present.
    pub fn size(&self, key: &str) -> Option<u64> {
        StoreKey::new(key.trim_start_matches('/'))
            .ok()
            .and_then(|k| self.storage.size_key(&k).ok().flatten())
    }
}

/// `a/b` + `c` → `a/b/c`; `""` + `c` → `c`.
pub fn join(group: &str, child: &str) -> String {
    let g = group.trim_matches('/');
    let c = child.trim_matches('/');
    if g.is_empty() {
        c.to_string()
    } else if c.is_empty() {
        g.to_string()
    } else {
        format!("{g}/{c}")
    }
}
