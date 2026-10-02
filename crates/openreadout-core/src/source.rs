//! Where a reader's bytes come from: a local file, a buffer in memory, or a callback the host
//! provides (a Python file-like object, a browser `File`, an S3 or HTTP range reader written by
//! the host). The binary itself never opens a network connection (rule 5): remote sources are
//! implemented by the program embedding the library and handed in as a [`CallbackSource`].
//!
//! Two traits:
//!
//! - [`ByteSource`] is one file's bytes: its size and positional reads. Implementations are
//!   `Send + Sync` and read through `&self`, so several threads (and several dataset handles)
//!   can share one source.
//! - [`DirSource`] is a namespace of files addressed by path: open a file, list a directory,
//!   ask whether a path exists and how large it is. Readers use it for the file they were asked
//!   to open, for sibling files (a CZI's following parts, an OME-TIFF companion, an imzML's
//!   `.ibd`) and for directory datasets (a Bruker `.d`).
//!
//! Readers do not use the traits directly. They get an [`Input`] (a path plus an [`Fs`], the
//! cheap-to-clone handle on a `DirSource`) and use [`Fs`] like `std::fs`, and [`SourceFile`]
//! like `std::fs::File` (it implements `Read` and `Seek`, and has `metadata().len()`).
//!
//! Local files are read with positional reads (`pread` on Unix, `seek_read` on Windows): one
//! system call per read and no shared file position.
//! Memory mapping would need `unsafe`, which the workspace forbids.
//!
//! ```
//! use std::io::{Read, Seek, SeekFrom};
//! use openreadout_core::source::Input;
//!
//! let input = Input::from_bytes("sample.bin", b"0123456789".to_vec());
//! let mut f = input.open().unwrap();
//! assert_eq!(f.metadata().unwrap().len(), 10);
//! f.seek(SeekFrom::Start(4)).unwrap();
//! let mut buf = [0u8; 3];
//! f.read_exact(&mut buf).unwrap();
//! assert_eq!(&buf, b"456");
//! assert!(!input.is_local());
//! ```

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::{Deref, Range};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use crate::error::{Error, Result};

// ---------------------------------------------------------------------------------------------
// ByteSource
// ---------------------------------------------------------------------------------------------

/// Random-access bytes of one file.
///
/// Only [`size`](ByteSource::size), [`read_at`](ByteSource::read_at) and
/// [`name`](ByteSource::name) are required. `size` is asked again whenever a reader wants the
/// length, so a source over a file that is still being written reports its current size.
pub trait ByteSource: Send + Sync + fmt::Debug {
    /// Current length in bytes.
    fn size(&self) -> io::Result<u64>;

    /// Read up to `buf.len()` bytes at `offset`; return how many were read (0 at or past the
    /// end). Like `std::io::Read::read`, a short read is allowed anywhere.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

    /// A name for messages: the path of a local file, the name a host gave its buffer.
    fn name(&self) -> String;

    /// Fill `buf` from `offset`, or fail with `UnexpectedEof` if the source ends first.
    fn read_exact_at(&self, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
        while !buf.is_empty() {
            match self.read_at(offset, buf) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "source ends inside the read",
                    ));
                }
                Ok(n) => {
                    buf = &mut buf[n..];
                    offset = offset.saturating_add(n as u64);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Up to `len` bytes at `offset` (fewer at the end of the source). In-memory sources return
    /// a view of their buffer without copying; the default reads into a new buffer.
    fn read_range(&self, offset: u64, len: usize) -> io::Result<SharedBytes> {
        let size = self.size()?;
        let n = usize::try_from(size.saturating_sub(offset))
            .unwrap_or(usize::MAX)
            .min(len);
        let mut buf = vec![0u8; n];
        self.read_exact_at(offset, &mut buf)?;
        Ok(SharedBytes::from(buf))
    }

    /// The file on the local disk this source reads, if it is one.
    fn local_path(&self) -> Option<&Path> {
        None
    }
}

/// Bytes shared without copying: a range of a reference-counted buffer. Dereferences to
/// `[u8]`.
#[derive(Clone)]
pub struct SharedBytes {
    buf: Arc<dyn AsRef<[u8]> + Send + Sync>,
    range: Range<usize>,
}

impl SharedBytes {
    /// A view of `range` of `buf`; the range is clamped to the buffer.
    pub fn new(buf: Arc<dyn AsRef<[u8]> + Send + Sync>, range: Range<usize>) -> Self {
        let len = (*buf).as_ref().len();
        let end = range.end.min(len);
        let start = range.start.min(end);
        Self {
            buf,
            range: start..end,
        }
    }

    /// Copy into an owned vector.
    pub fn to_vec(&self) -> Vec<u8> {
        self.deref().to_vec()
    }
}

impl Deref for SharedBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &(*self.buf).as_ref()[self.range.clone()]
    }
}

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl From<Vec<u8>> for SharedBytes {
    fn from(v: Vec<u8>) -> Self {
        let n = v.len();
        Self::new(Arc::new(v), 0..n)
    }
}

impl fmt::Debug for SharedBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedBytes")
            .field("len", &self.range.len())
            .finish_non_exhaustive()
    }
}

/// A file on the local disk, read with positional reads.
#[derive(Debug)]
pub struct LocalFile {
    #[cfg(any(unix, windows))]
    file: File,
    #[cfg(not(any(unix, windows)))]
    file: Mutex<File>,
    path: PathBuf,
}

impl LocalFile {
    /// Open `path` for reading.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        Ok(Self::from_file(file, path))
    }

    /// Wrap a file already open for reading; `path` names it in messages.
    pub fn from_file(file: File, path: &Path) -> Self {
        Self {
            #[cfg(any(unix, windows))]
            file,
            #[cfg(not(any(unix, windows)))]
            file: Mutex::new(file),
            path: path.to_path_buf(),
        }
    }
}

impl ByteSource for LocalFile {
    fn size(&self) -> io::Result<u64> {
        #[cfg(any(unix, windows))]
        let m = self.file.metadata()?;
        #[cfg(not(any(unix, windows)))]
        let m = self
            .file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .metadata()?;
        Ok(m.len())
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(&self.file, buf, offset)
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::FileExt::seek_read(&self.file, buf, offset)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let mut f = self.file.lock().unwrap_or_else(PoisonError::into_inner);
            f.seek(SeekFrom::Start(offset))?;
            f.read(buf)
        }
    }

    fn name(&self) -> String {
        self.path.display().to_string()
    }

    fn local_path(&self) -> Option<&Path> {
        Some(&self.path)
    }
}

/// Bytes held in memory (a `Vec<u8>`, a host buffer that dereferences to bytes).
#[derive(Clone)]
pub struct MemSource {
    data: Arc<dyn AsRef<[u8]> + Send + Sync>,
    name: String,
}

impl MemSource {
    /// Serve `data`, named `name` in messages.
    pub fn new(name: impl Into<String>, data: impl AsRef<[u8]> + Send + Sync + 'static) -> Self {
        Self {
            data: Arc::new(data),
            name: name.into(),
        }
    }

    fn bytes(&self) -> &[u8] {
        (*self.data).as_ref()
    }
}

impl fmt::Debug for MemSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemSource")
            .field("name", &self.name)
            .field("len", &self.bytes().len())
            .finish_non_exhaustive()
    }
}

impl ByteSource for MemSource {
    fn size(&self) -> io::Result<u64> {
        Ok(self.bytes().len() as u64)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let b = self.bytes();
        let Ok(start) = usize::try_from(offset) else {
            return Ok(0);
        };
        let Some(avail) = b.get(start..) else {
            return Ok(0);
        };
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        Ok(n)
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn read_range(&self, offset: u64, len: usize) -> io::Result<SharedBytes> {
        let size = self.bytes().len();
        let start = usize::try_from(offset).unwrap_or(usize::MAX).min(size);
        let end = start.saturating_add(len).min(size);
        Ok(SharedBytes::new(Arc::clone(&self.data), start..end))
    }
}

/// The function a [`CallbackSource`] calls: fill the buffer from the offset, return the number
/// of bytes read (0 at the end).
pub type ReadCallback = dyn Fn(u64, &mut [u8]) -> io::Result<usize> + Send + Sync;

/// Bytes the host reads for us: a Python file-like object, a browser `Blob`, an object in S3
/// or a URL fetched with HTTP range requests, all implemented by the host program. Each call
/// is a round trip into the host, so wrap slow ones in a [`CachedSource`].
pub struct CallbackSource {
    size: u64,
    name: String,
    read: Box<ReadCallback>,
}

impl CallbackSource {
    /// A source of `size` bytes named `name`, read through `read`.
    pub fn new(
        name: impl Into<String>,
        size: u64,
        read: impl Fn(u64, &mut [u8]) -> io::Result<usize> + Send + Sync + 'static,
    ) -> Self {
        Self {
            size,
            name: name.into(),
            read: Box::new(read),
        }
    }
}

impl fmt::Debug for CallbackSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallbackSource")
            .field("name", &self.name)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl ByteSource for CallbackSource {
    fn size(&self) -> io::Result<u64> {
        Ok(self.size)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if offset >= self.size || buf.is_empty() {
            return Ok(0);
        }
        let n = usize::try_from(self.size - offset)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        let got = (self.read)(offset, &mut buf[..n])?;
        if got > n {
            return Err(io::Error::other(format!(
                "the host read returned {got} bytes for a {n}-byte request"
            )));
        }
        Ok(got)
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

/// A block cache in front of a slow source (a host callback, a network reader). Readers issue
/// many small reads for headers; this turns them into a few block-sized reads of the inner
/// source. Reads of several blocks or more bypass the cache.
pub struct CachedSource {
    inner: Arc<dyn ByteSource>,
    block: u64,
    capacity: usize,
    state: Mutex<CacheState>,
}

#[derive(Default)]
struct CacheState {
    blocks: HashMap<u64, Arc<Vec<u8>>>,
    order: VecDeque<u64>,
}

impl CachedSource {
    /// Default block size (64 KiB).
    pub const DEFAULT_BLOCK: u64 = 64 * 1024;
    /// Default number of cached blocks (16 MiB with the default block size).
    pub const DEFAULT_CAPACITY: usize = 256;

    /// Cache `inner` in blocks of `block` bytes, keeping at most `capacity` blocks.
    pub fn new(inner: Arc<dyn ByteSource>, block: u64, capacity: usize) -> Self {
        Self {
            inner,
            block: block.max(512),
            capacity: capacity.max(1),
            state: Mutex::new(CacheState::default()),
        }
    }

    /// Cache `inner` with the default block size and capacity.
    pub fn with_defaults(inner: Arc<dyn ByteSource>) -> Self {
        Self::new(inner, Self::DEFAULT_BLOCK, Self::DEFAULT_CAPACITY)
    }

    fn block_at(&self, index: u64) -> io::Result<Arc<Vec<u8>>> {
        {
            let st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(b) = st.blocks.get(&index) {
                return Ok(Arc::clone(b));
            }
        }
        let start = index.saturating_mul(self.block);
        let size = self.inner.size()?;
        let n = usize::try_from(size.saturating_sub(start).min(self.block)).unwrap_or(0);
        let mut buf = vec![0u8; n];
        let mut filled = 0;
        while filled < n {
            match self
                .inner
                .read_at(start + filled as u64, &mut buf[filled..])
            {
                Ok(0) => break,
                Ok(k) => filled += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        buf.truncate(filled);
        let b = Arc::new(buf);
        let mut st = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if st.blocks.insert(index, Arc::clone(&b)).is_none() {
            st.order.push_back(index);
        }
        while st.order.len() > self.capacity {
            if let Some(old) = st.order.pop_front() {
                st.blocks.remove(&old);
            }
        }
        Ok(b)
    }
}

impl fmt::Debug for CachedSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedSource")
            .field("inner", &self.inner)
            .field("block", &self.block)
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl ByteSource for CachedSource {
    fn size(&self) -> io::Result<u64> {
        self.inner.size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        if buf.len() as u64 >= self.block.saturating_mul(4) {
            return self.inner.read_at(offset, buf);
        }
        let index = offset / self.block;
        let b = self.block_at(index)?;
        let within = usize::try_from(offset - index * self.block).unwrap_or(usize::MAX);
        let Some(avail) = b.get(within..) else {
            return Ok(0);
        };
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        Ok(n)
    }

    fn name(&self) -> String {
        self.inner.name()
    }

    fn local_path(&self) -> Option<&Path> {
        self.inner.local_path()
    }
}

// ---------------------------------------------------------------------------------------------
// SourceFile: a cursor, used by readers like `std::fs::File`
// ---------------------------------------------------------------------------------------------

/// A read cursor over a [`ByteSource`], used by readers like `std::fs::File`: it
/// implements `Read` and `Seek`, and `metadata()?.len()` is the source's current size. Cloning
/// it shares the source and copies the position.
#[derive(Clone)]
pub struct SourceFile {
    src: Arc<dyn ByteSource>,
    pos: u64,
}

impl SourceFile {
    /// A cursor at the start of `src`.
    pub fn new(src: Arc<dyn ByteSource>) -> Self {
        Self { src, pos: 0 }
    }

    /// Open a local file (shorthand for `SourceFile::new(Arc::new(LocalFile::open(path)?))`).
    pub fn open_local(path: &Path) -> io::Result<Self> {
        Ok(Self::new(Arc::new(LocalFile::open(path)?)))
    }

    /// The size, like `std::fs::File::metadata` (only `len()` and the file-type questions are
    /// meaningful).
    pub fn metadata(&self) -> io::Result<EntryMeta> {
        Ok(EntryMeta::file(self.src.size()?))
    }

    /// The source's current size.
    pub fn size(&self) -> io::Result<u64> {
        self.src.size()
    }

    /// The source this cursor reads.
    pub fn source(&self) -> &Arc<dyn ByteSource> {
        &self.src
    }

    /// Another cursor on the same source (like `File::try_clone`; never fails).
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(self.clone())
    }

    /// Positional read that leaves the cursor where it is.
    pub fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.src.read_at(offset, buf)
    }

    /// Positional exact read that leaves the cursor where it is.
    pub fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.src.read_exact_at(offset, buf)
    }
}

impl fmt::Debug for SourceFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceFile")
            .field("source", &self.src.name())
            .field("pos", &self.pos)
            .finish()
    }
}

impl Read for SourceFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.src.read_at(self.pos, buf)?;
        self.pos = self.pos.saturating_add(n as u64);
        Ok(n)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        self.src.read_exact_at(self.pos, buf)?;
        self.pos = self.pos.saturating_add(buf.len() as u64);
        Ok(())
    }
}

impl Seek for SourceFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let (base, delta) = match pos {
            SeekFrom::Start(n) => {
                self.pos = n;
                return Ok(n);
            }
            SeekFrom::End(d) => (self.src.size()?, d),
            SeekFrom::Current(d) => (self.pos, d),
        };
        let new = base.checked_add_signed(delta).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid seek to a negative or overflowing position",
            )
        })?;
        self.pos = new;
        Ok(new)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        Ok(self.pos)
    }
}

// ---------------------------------------------------------------------------------------------
// DirSource: a namespace of files
// ---------------------------------------------------------------------------------------------

/// What [`DirSource::metadata`] knows about a path. Mirrors the parts of
/// `std::fs::Metadata` readers use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryMeta {
    len: u64,
    dir: bool,
    modified: Option<SystemTime>,
}

impl EntryMeta {
    /// A file of `len` bytes.
    pub fn file(len: u64) -> Self {
        Self {
            len,
            dir: false,
            modified: None,
        }
    }

    /// A directory.
    pub fn dir() -> Self {
        Self {
            len: 0,
            dir: true,
            modified: None,
        }
    }

    /// With a modification time.
    pub fn with_modified(mut self, t: SystemTime) -> Self {
        self.modified = Some(t);
        self
    }

    /// Size in bytes (0 for directories).
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// True for directories.
    pub fn is_dir(&self) -> bool {
        self.dir
    }

    /// True for files.
    pub fn is_file(&self) -> bool {
        !self.dir
    }

    /// Last modification time, when the source knows it.
    pub fn modified(&self) -> io::Result<SystemTime> {
        self.modified.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "this source does not record modification times",
            )
        })
    }

    fn from_std(m: &std::fs::Metadata) -> Self {
        Self {
            len: m.len(),
            dir: m.is_dir(),
            modified: m.modified().ok(),
        }
    }
}

/// One entry of a directory listing.
#[derive(Debug)]
pub struct DirEntry {
    path: PathBuf,
    kind: EntryKind,
}

#[derive(Debug)]
enum EntryKind {
    Local(Box<std::fs::DirEntry>),
    Known(EntryMeta),
}

impl DirEntry {
    /// An entry at `path` (the directory joined with the name) with known metadata.
    pub fn new(path: PathBuf, meta: EntryMeta) -> Self {
        Self {
            path,
            kind: EntryKind::Known(meta),
        }
    }

    /// Full path: the listed directory joined with the entry's name.
    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }

    /// The entry's name.
    pub fn file_name(&self) -> OsString {
        self.path.file_name().unwrap_or_default().to_os_string()
    }

    /// Size, type and modification time (follows symbolic links for local entries).
    pub fn metadata(&self) -> io::Result<EntryMeta> {
        match &self.kind {
            EntryKind::Local(e) => {
                // Like `std::fs::metadata`: a symbolic link reports its target.
                std::fs::metadata(e.path()).map(|m| EntryMeta::from_std(&m))
            }
            EntryKind::Known(m) => Ok(*m),
        }
    }

    /// The entry's type without following symbolic links where the platform reports it for
    /// free (local listings); `is_dir()`/`is_file()` of the result are meaningful.
    pub fn file_type(&self) -> io::Result<EntryMeta> {
        match &self.kind {
            EntryKind::Local(e) => {
                let t = e.file_type()?;
                Ok(if t.is_dir() {
                    EntryMeta::dir()
                } else {
                    EntryMeta::file(0)
                })
            }
            EntryKind::Known(m) => Ok(*m),
        }
    }
}

/// Iterator over a directory listing, like `std::fs::ReadDir`.
#[derive(Debug)]
pub struct ReadDir(std::vec::IntoIter<io::Result<DirEntry>>);

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}

/// A namespace of files: the local file system, or a set of files the host supplied.
/// Paths are the ones the reader builds from its [`Input`]'s path (siblings with
/// `with_file_name`, members of a directory dataset with `join`).
pub trait DirSource: Send + Sync + fmt::Debug {
    /// Open the file at `path`.
    fn open(&self, path: &Path) -> io::Result<Arc<dyn ByteSource>>;

    /// Size and type of `path` (`NotFound` if it does not exist).
    fn metadata(&self, path: &Path) -> io::Result<EntryMeta>;

    /// The entries of directory `dir`, in no particular order.
    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>>;

    /// Does `path` exist?
    fn exists(&self, path: &Path) -> bool {
        self.metadata(path).is_ok()
    }

    /// True for the local file system: paths are real paths, and readers that only read local
    /// files can open them directly.
    fn is_local(&self) -> bool {
        false
    }
}

/// The local file system (`std::fs`).
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalFs;

impl DirSource for LocalFs {
    fn open(&self, path: &Path) -> io::Result<Arc<dyn ByteSource>> {
        Ok(Arc::new(LocalFile::open(path)?))
    }

    fn metadata(&self, path: &Path) -> io::Result<EntryMeta> {
        std::fs::metadata(path).map(|m| EntryMeta::from_std(&m))
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            out.push(DirEntry {
                path: e.path(),
                kind: EntryKind::Local(Box::new(e)),
            });
        }
        Ok(out)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_local(&self) -> bool {
        true
    }
}

/// Files held by the host: a map from path to source. Directories exist implicitly (every
/// ancestor of a file is one). One dropped file, the files of a dropped folder, or a Python
/// buffer all become a `MemFs`.
#[derive(Debug, Default, Clone)]
pub struct MemFs {
    files: BTreeMap<PathBuf, Arc<dyn ByteSource>>,
}

impl MemFs {
    /// An empty namespace.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) the file at `path`.
    pub fn insert(&mut self, path: impl AsRef<Path>, src: Arc<dyn ByteSource>) {
        self.files.insert(normalize(path.as_ref()), src);
    }

    /// Builder form of [`MemFs::insert`].
    pub fn with(mut self, path: impl AsRef<Path>, src: Arc<dyn ByteSource>) -> Self {
        self.insert(path, src);
        self
    }

    /// Paths of every file held, sorted.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.files.keys().cloned().collect()
    }

    fn is_dir_path(&self, dir: &Path) -> bool {
        let dir = normalize(dir);
        self.files
            .keys()
            .any(|k| k.starts_with(&dir) && k.as_path() != dir.as_path())
    }
}

/// Lexical normalization: drop `.` components and a leading `./`, resolve `..` where possible.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

impl DirSource for MemFs {
    fn open(&self, path: &Path) -> io::Result<Arc<dyn ByteSource>> {
        let key = normalize(path);
        self.files.get(&key).cloned().ok_or_else(|| {
            let kind = if self.is_dir_path(&key) {
                io::ErrorKind::IsADirectory
            } else {
                io::ErrorKind::NotFound
            };
            io::Error::new(kind, format!("{} was not provided", path.display()))
        })
    }

    fn metadata(&self, path: &Path) -> io::Result<EntryMeta> {
        let key = normalize(path);
        if let Some(src) = self.files.get(&key) {
            return Ok(EntryMeta::file(src.size()?));
        }
        if key.as_os_str().is_empty() || self.is_dir_path(&key) {
            return Ok(EntryMeta::dir());
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} was not provided", path.display()),
        ))
    }

    fn list(&self, dir: &Path) -> io::Result<Vec<DirEntry>> {
        let key = normalize(dir);
        if self.files.contains_key(&key) {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                format!("{} is a file", dir.display()),
            ));
        }
        let mut names: BTreeMap<OsString, EntryMeta> = BTreeMap::new();
        for (k, src) in &self.files {
            let Ok(rest) = k.strip_prefix(&key) else {
                continue;
            };
            let mut comps = rest.components();
            let Some(first) = comps.next() else {
                continue;
            };
            let name = first.as_os_str().to_os_string();
            if comps.next().is_some() {
                names.insert(name, EntryMeta::dir());
            } else {
                names.insert(name, EntryMeta::file(src.size().unwrap_or(0)));
            }
        }
        if names.is_empty() && !key.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} was not provided", dir.display()),
            ));
        }
        // Entries carry the directory as the caller spelled it, like `std::fs::read_dir`.
        Ok(names
            .into_iter()
            .map(|(n, m)| DirEntry::new(dir.join(n), m))
            .collect())
    }
}

/// A cheap-to-clone handle on a [`DirSource`], with the `std::fs`-like calls readers use.
#[derive(Clone)]
pub struct Fs(Arc<dyn DirSource>);

impl fmt::Debug for Fs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Default for Fs {
    fn default() -> Self {
        Self::local()
    }
}

impl Fs {
    /// The local file system.
    pub fn local() -> Self {
        Self(Arc::new(LocalFs))
    }

    /// Wrap any namespace.
    pub fn new(dir: Arc<dyn DirSource>) -> Self {
        Self(dir)
    }

    /// The namespace this handle wraps.
    pub fn dir_source(&self) -> &Arc<dyn DirSource> {
        &self.0
    }

    /// True for the local file system.
    pub fn is_local(&self) -> bool {
        self.0.is_local()
    }

    /// Open a file (like `std::fs::File::open`).
    pub fn open(&self, path: &Path) -> io::Result<SourceFile> {
        Ok(SourceFile::new(self.0.open(path)?))
    }

    /// The byte source of a file.
    pub fn source(&self, path: &Path) -> io::Result<Arc<dyn ByteSource>> {
        self.0.open(path)
    }

    /// Like `std::fs::metadata`.
    pub fn metadata(&self, path: &Path) -> io::Result<EntryMeta> {
        self.0.metadata(path)
    }

    /// Like `Path::exists`.
    pub fn exists(&self, path: &Path) -> bool {
        self.0.exists(path)
    }

    /// The entry of `dir` named `name` as the directory lists it: an exact match first, else
    /// one that differs only in ASCII case. Case-insensitive file systems (macOS, Windows)
    /// would open `dir/name` either way; resolving through the listing reports the name the
    /// file really has, identically on every file system and byte source.
    pub fn find_in_dir(&self, dir: &Path, name: &str) -> Option<PathBuf> {
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        let names: Vec<std::ffi::OsString> = self
            .read_dir(dir)
            .ok()?
            .filter_map(std::result::Result::ok)
            .map(|e| e.file_name())
            .collect();
        names
            .iter()
            .find(|n| n.to_string_lossy() == name)
            .or_else(|| {
                names
                    .iter()
                    .find(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
            })
            .map(|n| dir.join(n))
    }

    /// Like `Path::is_file`.
    pub fn is_file(&self, path: &Path) -> bool {
        self.0.metadata(path).is_ok_and(|m| m.is_file())
    }

    /// Like `Path::is_dir`.
    pub fn is_dir(&self, path: &Path) -> bool {
        self.0.metadata(path).is_ok_and(|m| m.is_dir())
    }

    /// Like `std::fs::read_dir`.
    pub fn read_dir(&self, dir: &Path) -> io::Result<ReadDir> {
        let v: Vec<io::Result<DirEntry>> = self.0.list(dir)?.into_iter().map(Ok).collect();
        Ok(ReadDir(v.into_iter()))
    }

    /// Like `std::fs::read`: the whole file.
    pub fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        let mut f = self.open(path)?;
        let mut out = Vec::new();
        f.read_to_end(&mut out)?;
        Ok(out)
    }

    /// Like `std::fs::read_to_string`.
    pub fn read_to_string(&self, path: &Path) -> io::Result<String> {
        String::from_utf8(self.read(path)?)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

// ---------------------------------------------------------------------------------------------
// Input: what a reader opens
// ---------------------------------------------------------------------------------------------

/// What a reader is asked to open: a path in a namespace of files. For a local file the path
/// is the real path and the namespace is the file system; for a buffer or a host source the
/// path is the name the host gave it (its extension still guides detection).
#[derive(Clone, Debug)]
pub struct Input {
    path: PathBuf,
    fs: Fs,
}

impl Input {
    /// A path on the local file system.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            fs: Fs::local(),
        }
    }

    /// `path` inside the namespace `fs`.
    pub fn new(path: impl Into<PathBuf>, fs: Fs) -> Self {
        Self {
            path: path.into(),
            fs,
        }
    }

    /// One file held in memory, named `name`.
    pub fn from_bytes(
        name: impl AsRef<Path>,
        data: impl AsRef<[u8]> + Send + Sync + 'static,
    ) -> Self {
        let name = name.as_ref();
        let src = MemSource::new(name.display().to_string(), data);
        Self::from_source(name, Arc::new(src))
    }

    /// One file read through `src` (a host callback, a cached remote reader), named `name`.
    pub fn from_source(name: impl AsRef<Path>, src: Arc<dyn ByteSource>) -> Self {
        let name = normalize(name.as_ref());
        let fs = MemFs::new().with(&name, src);
        Self {
            path: name,
            fs: Fs::new(Arc::new(fs)),
        }
    }

    /// The path (or host-given name) of what is opened.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The namespace siblings and directory members are looked up in.
    pub fn fs(&self) -> &Fs {
        &self.fs
    }

    /// True when the input is on the local file system.
    pub fn is_local(&self) -> bool {
        self.fs.is_local()
    }

    /// Another path in the same namespace (a sibling file, a member of a directory dataset).
    pub fn with_path(&self, path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            fs: self.fs.clone(),
        }
    }

    /// Open the input's file; errors name its path (exit 5).
    pub fn open(&self) -> Result<SourceFile> {
        self.fs
            .open(&self.path)
            .map_err(|e| Error::io(&self.path, e))
    }

    /// Size and type of the input's path.
    pub fn metadata(&self) -> Result<EntryMeta> {
        self.fs
            .metadata(&self.path)
            .map_err(|e| Error::io(&self.path, e))
    }

    /// True when the input is a directory (a vendor folder format).
    pub fn is_dir(&self) -> bool {
        self.fs.is_dir(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn source_file_reads_and_seeks_like_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"hello world").unwrap();
        for input in [
            Input::local(&path),
            Input::from_bytes("a.bin", b"hello world".to_vec()),
        ] {
            let mut f = input.open().unwrap();
            assert_eq!(f.metadata().unwrap().len(), 11);
            f.seek(SeekFrom::End(-5)).unwrap();
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            assert_eq!(s, "world");
            f.seek(SeekFrom::Start(20)).unwrap();
            assert_eq!(f.read(&mut [0u8; 4]).unwrap(), 0);
            let mut b = [0u8; 4];
            let e = f.read_exact(&mut b).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
            assert!(f.seek(SeekFrom::Current(-100)).is_err());
            let mut b = [0u8; 5];
            f.read_exact_at(0, &mut b).unwrap();
            assert_eq!(&b, b"hello");
        }
    }

    #[test]
    fn mem_fs_lists_and_answers_like_a_directory() {
        let fs = MemFs::new()
            .with(
                "run.d/analysis.tdf",
                Arc::new(MemSource::new("t", vec![1u8; 3])),
            )
            .with(
                "run.d/sub/x.bin",
                Arc::new(MemSource::new("x", vec![2u8; 5])),
            )
            .with("./top.czi", Arc::new(MemSource::new("c", vec![0u8; 1])));
        let fs = Fs::new(Arc::new(fs));
        assert!(fs.is_dir(Path::new("run.d")));
        assert!(fs.is_file(Path::new("run.d/analysis.tdf")));
        assert!(fs.is_file(Path::new("top.czi")));
        assert!(!fs.exists(Path::new("run.d/missing")));
        assert_eq!(fs.metadata(Path::new("run.d/sub/x.bin")).unwrap().len(), 5);
        let mut names: Vec<(String, bool)> = fs
            .read_dir(Path::new("run.d"))
            .unwrap()
            .flatten()
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.metadata().unwrap().is_dir(),
                )
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![("analysis.tdf".into(), false), ("sub".into(), true)]
        );
        let root: Vec<_> = fs
            .read_dir(Path::new(""))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        assert_eq!(root.len(), 2);
        assert_eq!(
            fs.open(Path::new("run.d")).unwrap_err().kind(),
            io::ErrorKind::IsADirectory
        );
        assert_eq!(
            fs.read(Path::new("run.d/sub/../analysis.tdf")).unwrap(),
            [1, 1, 1]
        );
    }

    #[test]
    fn local_fs_lists_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"12").unwrap();
        std::fs::create_dir(dir.path().join("b")).unwrap();
        let fs = Fs::local();
        let mut v: Vec<(String, bool, bool)> = fs
            .read_dir(dir.path())
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.file_type().unwrap().is_dir(),
                    e.metadata().unwrap().is_dir(),
                )
            })
            .collect();
        v.sort();
        assert_eq!(
            v,
            vec![("a".into(), false, false), ("b".into(), true, true)]
        );
        assert!(
            fs.metadata(&dir.path().join("a"))
                .unwrap()
                .modified()
                .is_ok()
        );
        assert!(fs.is_local());
    }

    #[test]
    fn callback_source_is_bounded_and_cached() {
        let data: Arc<Vec<u8>> = Arc::new((0..=255u8).cycle().take(300_000).collect());
        let calls = Arc::new(AtomicUsize::new(0));
        let (d, c) = (Arc::clone(&data), Arc::clone(&calls));
        let cb = CallbackSource::new("remote.bin", data.len() as u64, move |off, buf| {
            c.fetch_add(1, Ordering::SeqCst);
            let off = off as usize;
            let n = buf.len().min(d.len() - off);
            buf[..n].copy_from_slice(&d[off..off + n]);
            Ok(n)
        });
        let cached = Arc::new(CachedSource::new(Arc::new(cb), 4096, 4));
        let mut f = SourceFile::new(cached);
        let mut small = [0u8; 16];
        for i in 0..100u64 {
            f.seek(SeekFrom::Start(i * 16)).unwrap();
            f.read_exact(&mut small).unwrap();
            assert_eq!(small[0], (i * 16 % 256) as u8);
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "one block serves 100 small reads"
        );
        // Straddling blocks and reading to the end.
        let mut v = Vec::new();
        f.seek(SeekFrom::Start(299_990)).unwrap();
        f.read_to_end(&mut v).unwrap();
        assert_eq!(v.len(), 10);
        let mut big = vec![0u8; 50_000];
        f.read_exact_at(1000, &mut big).unwrap();
        assert_eq!(&big[..], &data[1000..51_000]);
        // A host callback that claims more than it was asked for is an error, not a panic.
        let bad = CallbackSource::new("bad", 10, |_, buf| Ok(buf.len() + 1));
        assert!(bad.read_at(0, &mut [0u8; 4]).is_err());
    }

    #[test]
    fn mem_source_ranges_share_the_buffer() {
        let s = MemSource::new("m", vec![9u8; 10]);
        let r = s.read_range(8, 100).unwrap();
        assert_eq!(&*r, &[9, 9]);
        assert!(s.read_range(20, 4).unwrap().is_empty());
        assert_eq!(s.read_at(u64::MAX, &mut [0u8; 4]).unwrap(), 0);
    }
}
