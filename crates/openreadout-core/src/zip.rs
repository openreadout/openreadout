//! ZIP archives (PKWARE APPNOTE, public specification): a read-only index over the central
//! directory with stored and deflated members, partial reads of stored members, and a small
//! deterministic writer.
//!
//! One implementation for every reader whose files are zip containers: qPCR (RDML, `.eds`,
//! `.pcrd`), OpenLab CDS (`.dx`, `.rx`) and zipped Zarr stores. Members are read through the
//! byte-source layer ([`Fs`]), so archives held in memory or behind a host callback work like
//! local files.
//!
//! Robustness: every offset and length read from the archive is checked against the file size
//! before it is used, the member count is capped ([`MAX_MEMBERS`]), a member is decompressed
//! only when its declared size is within the index's limit (512 MiB unless
//! [`ZipIndex::with_max_member`] raises it), inflation stops at the declared size (a zip bomb
//! cannot grow past it) and every decoded member is checked against its CRC-32. Encrypted
//! members and compression methods other than stored (0) and deflate (8) are unsupported
//! (exit 6); anything malformed is corrupt (exit 4). Nothing panics on malformed input.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bytes::{le_u16, le_u32, le_u64};
use crate::source::{ByteSource, Fs};
use crate::{Error, Result};

const EOCD_SIG: u32 = 0x0605_4b50;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;
const ZIP64_EOCD_SIG: u32 = 0x0606_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
/// Size of the end-of-central-directory record without its comment.
const EOCD_LEN: u64 = 22;
/// The end record is within its own length plus the longest comment (65 535) of the end.
const EOCD_SEARCH: u64 = EOCD_LEN + 65_535;
/// Default largest member decompressed into memory.
pub const DEFAULT_MAX_MEMBER: u64 = 512 << 20;
/// Most members indexed (an `.eds` with raw images holds a few thousand; a zipped Zarr store
/// can hold many chunks).
pub const MAX_MEMBERS: u64 = 1_000_000;

/// Does `head` start like a zip archive (a local file header, an empty archive, or the
/// `PK\x07\x08` split-archive marker Bio-Rad CFX software writes before the first header)?
pub fn is_zip(head: &[u8]) -> bool {
    head.starts_with(b"PK\x03\x04")
        || head.starts_with(b"PK\x05\x06")
        || head.starts_with(b"PK\x07\x08PK\x03\x04")
}

/// Offset of the first local file header in a zip head (0, or 4 after a split marker).
pub fn first_header(head: &[u8]) -> Option<usize> {
    if head.starts_with(b"PK\x03\x04") {
        Some(0)
    } else if head.starts_with(b"PK\x07\x08PK\x03\x04") {
        Some(4)
    } else {
        None
    }
}

/// Name of the first member in a zip head, as stored (invalid UTF-8 replaced).
pub fn first_member_name(head: &[u8]) -> Option<String> {
    let at = first_header(head)?;
    let n = usize::from(le_u16(head, at.checked_add(26)?)?);
    let name = head.get(at.checked_add(30)?..at.checked_add(30)?.checked_add(n)?)?;
    Some(String::from_utf8_lossy(name).into_owned())
}

/// Normalized lookup key of a member name: lower case, `\` read as `/`.
pub fn norm(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

/// Bytes as text: UTF-8 with a leading BOM dropped, invalid sequences replaced.
pub fn text(b: &[u8]) -> String {
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    String::from_utf8_lossy(b).into_owned()
}

/// One archive member, as the central directory records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipMember {
    /// Name as stored.
    pub name: String,
    /// 0 = stored, 8 = deflate; anything else is reported, not read.
    pub method: u16,
    /// Bytes the member occupies in the archive.
    pub compressed_size: u64,
    /// Bytes the member decodes to.
    pub size: u64,
    /// Offset of the member's local header.
    pub local_offset: u64,
    /// General-purpose flag bit 0: the member is encrypted.
    pub encrypted: bool,
    /// CRC-32 of the decoded bytes.
    pub crc32: u32,
}

/// The central directory of a zip archive, and the byte source it was read from.
#[derive(Debug, Clone)]
pub struct ZipIndex {
    path: PathBuf,
    format: &'static str,
    src: Arc<dyn ByteSource>,
    max_member: u64,
    /// Length of the archive in bytes.
    pub file_len: u64,
    /// Members in central-directory order (directories left out).
    pub members: Vec<ZipMember>,
    /// Exact name → index into `members` (the first of duplicates).
    by_exact: BTreeMap<String, usize>,
    /// Normalized name ([`norm`]) → index into `members` (the first of duplicates).
    by_name: BTreeMap<String, usize>,
}

impl ZipIndex {
    /// Read the central directory of `path` in the namespace `fs`. `format` names the file
    /// format in errors (e.g. `rdml`, `openlab-cds`).
    pub fn open(fs: &Fs, path: &Path, format: &'static str) -> Result<ZipIndex> {
        let src = fs.source(path).map_err(|e| Error::io(path, e))?;
        Self::from_source(src, path, format)
    }

    /// Read the central directory of an archive held by `src`; `path` names it in messages.
    pub fn from_source(
        src: Arc<dyn ByteSource>,
        path: &Path,
        format: &'static str,
    ) -> Result<ZipIndex> {
        let corrupt = |d: String| Error::corrupt(format, format!("zip container: {d}"));
        let file_len = src.size().map_err(|e| Error::io(path, e))?;
        let read =
            |offset: u64, len: u64| read_at(src.as_ref(), path, format, file_len, offset, len);
        if file_len < EOCD_LEN {
            return Err(corrupt(
                "the file is shorter than an end-of-central-directory record".into(),
            ));
        }
        let tail_len = file_len.min(EOCD_SEARCH);
        let tail_start = file_len - tail_len;
        let tail = read(tail_start, tail_len)?;
        let eocd = (0..=tail.len().saturating_sub(EOCD_LEN as usize))
            .rev()
            .find(|&i| le_u32(&tail, i) == Some(EOCD_SIG))
            .ok_or_else(|| {
                corrupt("no end-of-central-directory record (the file is truncated)".into())
            })?;
        let eocd_abs = tail_start + eocd as u64;
        let mut entries = u64::from(le_u16(&tail, eocd + 10).unwrap_or(0));
        let mut cd_size = u64::from(le_u32(&tail, eocd + 12).unwrap_or(0));
        let mut cd_offset = u64::from(le_u32(&tail, eocd + 16).unwrap_or(0));
        if eocd >= 20 && le_u32(&tail, eocd - 20) == Some(ZIP64_LOCATOR_SIG) {
            let z64 = le_u64(&tail, eocd - 20 + 8).unwrap_or(u64::MAX);
            if z64 < eocd_abs {
                let rec = read(z64, 56.min(eocd_abs - z64))?;
                if le_u32(&rec, 0) == Some(ZIP64_EOCD_SIG) {
                    entries = le_u64(&rec, 32).unwrap_or(entries);
                    cd_size = le_u64(&rec, 40).unwrap_or(cd_size);
                    cd_offset = le_u64(&rec, 48).unwrap_or(cd_offset);
                }
            }
        }
        if cd_offset.checked_add(cd_size).is_none_or(|e| e > file_len) {
            return Err(corrupt(format!(
                "central directory at {cd_offset} (+{cd_size}) lies past the end of the {file_len}-byte file (truncated)"
            )));
        }
        // Every entry takes at least 46 bytes: a count the directory cannot hold is corrupt.
        if entries > MAX_MEMBERS || entries.saturating_mul(46) > cd_size {
            return Err(corrupt(format!(
                "{entries} members declared in a {cd_size}-byte central directory"
            )));
        }
        let cd = read(cd_offset, cd_size)?;
        let mut members = Vec::new();
        let (mut by_exact, mut by_name) = (BTreeMap::new(), BTreeMap::new());
        let mut at = 0usize;
        for _ in 0..entries {
            if le_u32(&cd, at) != Some(CENTRAL_SIG) {
                return Err(corrupt(format!(
                    "central directory entry at byte {} has no signature",
                    cd_offset + at as u64
                )));
            }
            let field16 = |k: usize| at.checked_add(k).and_then(|p| le_u16(&cd, p));
            let field32 = |k: usize| at.checked_add(k).and_then(|p| le_u32(&cd, p));
            let flags = field16(8).unwrap_or(0);
            let method = field16(10).unwrap_or(0);
            let crc32 = field32(16).unwrap_or(0);
            let mut csize = u64::from(field32(20).unwrap_or(0));
            let mut size = u64::from(field32(24).unwrap_or(0));
            let name_len = usize::from(field16(28).unwrap_or(0));
            let extra_len = usize::from(field16(30).unwrap_or(0));
            let comment_len = usize::from(field16(32).unwrap_or(0));
            let mut local = u64::from(field32(42).unwrap_or(0));
            let name_start = at + 46;
            let name_bytes = cd
                .get(name_start..name_start + name_len)
                .ok_or_else(|| corrupt("central directory truncated".into()))?;
            let name = String::from_utf8_lossy(name_bytes).into_owned();
            let extra = cd
                .get(name_start + name_len..name_start + name_len + extra_len)
                .unwrap_or(&[]);
            // ZIP64 extended information (header id 1): the 64-bit values of the fields that
            // are all ones, in this order.
            let mut e = 0usize;
            while e + 4 <= extra.len() {
                let id = le_u16(extra, e).unwrap_or(0);
                let n = usize::from(le_u16(extra, e + 2).unwrap_or(0));
                if id == 1 {
                    let mut k = e + 4;
                    for v in [&mut size, &mut csize, &mut local] {
                        if *v == 0xFFFF_FFFF {
                            *v = le_u64(extra, k).unwrap_or(*v);
                            k += 8;
                        }
                    }
                }
                e += 4 + n;
            }
            at = name_start + name_len + extra_len + comment_len;
            if name.ends_with('/') || name.ends_with('\\') {
                continue;
            }
            by_exact.entry(name.clone()).or_insert(members.len());
            by_name.entry(norm(&name)).or_insert(members.len());
            members.push(ZipMember {
                name,
                method,
                compressed_size: csize,
                size,
                local_offset: local,
                encrypted: flags & 1 != 0,
                crc32,
            });
        }
        Ok(ZipIndex {
            path: path.to_path_buf(),
            format,
            src,
            max_member: DEFAULT_MAX_MEMBER,
            file_len,
            members,
            by_exact,
            by_name,
        })
    }

    /// The same index with a different largest member decompressed into memory.
    #[must_use]
    pub fn with_max_member(mut self, bytes: u64) -> Self {
        self.max_member = bytes;
        self
    }

    /// Path of the archive (as given to [`ZipIndex::open`]).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The member with this name (case-insensitive, `\` = `/`).
    pub fn get(&self, name: &str) -> Option<&ZipMember> {
        self.by_name.get(&norm(name)).map(|&i| &self.members[i])
    }

    /// The member with exactly this name.
    pub fn get_exact(&self, name: &str) -> Option<&ZipMember> {
        self.by_exact.get(name).map(|&i| &self.members[i])
    }

    /// Does the archive hold this member (case-insensitive, `\` = `/`)?
    pub fn has(&self, name: &str) -> bool {
        self.by_name.contains_key(&norm(name))
    }

    /// Members whose normalized name starts with `prefix` (normalized too).
    pub fn with_prefix<'a>(&'a self, prefix: &str) -> impl Iterator<Item = &'a ZipMember> {
        let p = norm(prefix);
        self.members
            .iter()
            .filter(move |m| norm(&m.name).starts_with(&p))
    }

    fn corrupt(&self, d: String) -> Error {
        Error::corrupt(self.format, format!("zip container: {d}"))
    }

    fn bytes_at(&self, offset: u64, len: u64) -> Result<Vec<u8>> {
        read_at(
            self.src.as_ref(),
            &self.path,
            self.format,
            self.file_len,
            offset,
            len,
        )
    }

    /// Offset of a member's data: after its local header, whose name and extra lengths may
    /// differ from the central directory's.
    pub fn data_offset(&self, m: &ZipMember) -> Result<u64> {
        if m.local_offset
            .checked_add(30)
            .is_none_or(|e| e > self.file_len)
        {
            return Err(self.corrupt(format!("{}: local header past the end of the file", m.name)));
        }
        let h = self.bytes_at(m.local_offset, 30)?;
        if le_u32(&h, 0) != Some(LOCAL_SIG) {
            return Err(self.corrupt(format!("{}: local header missing", m.name)));
        }
        let n = u64::from(le_u16(&h, 26).unwrap_or(0)) + u64::from(le_u16(&h, 28).unwrap_or(0));
        let start = m.local_offset + 30 + n;
        if start
            .checked_add(m.compressed_size)
            .is_none_or(|e| e > self.file_len)
        {
            return Err(self.corrupt(format!(
                "{}: member data runs past the end of the file (truncated)",
                m.name
            )));
        }
        Ok(start)
    }

    fn check_readable(&self, m: &ZipMember) -> Result<()> {
        if m.encrypted {
            return Err(Error::unsupported(
                self.format,
                format!("encrypted zip member `{}`", m.name),
                "The member is encrypted; it cannot be read without the vendor's key.",
            ));
        }
        if m.size > self.max_member {
            return Err(Error::unsupported(
                self.format,
                format!("a {} MiB zip member (`{}`)", m.size >> 20, m.name),
                format!(
                    "Members larger than {} MiB are not read into memory.",
                    self.max_member >> 20
                ),
            ));
        }
        Ok(())
    }

    /// Whole decoded contents of a member, checked against its size and CRC-32.
    pub fn read(&self, m: &ZipMember) -> Result<Vec<u8>> {
        self.check_readable(m)?;
        let start = self.data_offset(m)?;
        let raw = self.bytes_at(start, m.compressed_size)?;
        let out = match m.method {
            0 => raw,
            8 => {
                let limit = usize::try_from(m.size).unwrap_or(usize::MAX);
                miniz_oxide::inflate::decompress_to_vec_with_limit(&raw, limit).map_err(|e| {
                    let past = matches!(e.status, miniz_oxide::inflate::TINFLStatus::HasMoreOutput);
                    self.corrupt(format!(
                        "{}: deflate stream: {e}{}",
                        m.name,
                        if past {
                            " (it decodes past the declared size)"
                        } else {
                            ""
                        }
                    ))
                })?
            }
            other => {
                return Err(Error::unsupported(
                    self.format,
                    format!("zip compression method {other} (member `{}`)", m.name),
                    "Only stored and deflated members are read; re-save the file with the vendor software.",
                ));
            }
        };
        if out.len() as u64 != m.size {
            return Err(self.corrupt(format!(
                "{}: decoded to {} bytes, the directory says {}",
                m.name,
                out.len(),
                m.size
            )));
        }
        let crc = crc32fast::hash(&out);
        if crc != m.crc32 {
            return Err(self.corrupt(format!(
                "{}: CRC-32 mismatch (stored {:08x}, computed {crc:08x})",
                m.name, m.crc32
            )));
        }
        Ok(out)
    }

    /// Up to the first `len` decoded bytes of a member, reading only as much of its compressed
    /// data as that needs (at most `len` + 64 KiB compressed bytes, or everything for a small
    /// member). Not checked against the CRC (only part of the member is read); a stream that is
    /// damaged within the prefix is an error.
    pub fn read_prefix(&self, m: &ZipMember, len: usize) -> Result<Vec<u8>> {
        if m.encrypted {
            return Err(self.corrupt(format!("{}: encrypted member", m.name)));
        }
        let start = self.data_offset(m)?;
        let want = (len as u64).saturating_add(64 << 10).min(m.compressed_size);
        let raw = self.bytes_at(start, want)?;
        let mut out = match m.method {
            0 => raw,
            8 => match miniz_oxide::inflate::decompress_to_vec_with_limit(&raw, len) {
                Ok(v) => v,
                Err(e)
                    if matches!(
                        e.status,
                        miniz_oxide::inflate::TINFLStatus::HasMoreOutput
                            | miniz_oxide::inflate::TINFLStatus::FailedCannotMakeProgress
                    ) =>
                {
                    e.output
                }
                Err(e) => return Err(self.corrupt(format!("{}: deflate stream: {e}", m.name))),
            },
            other => {
                return Err(Error::unsupported(
                    self.format,
                    format!("zip compression method {other} (member `{}`)", m.name),
                    "Only stored and deflated members are read; re-save the file with the vendor software.",
                ));
            }
        };
        out.truncate(len);
        Ok(out)
    }

    /// Contents of the named member (case-insensitive), if present.
    pub fn read_named(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match self.get(name) {
            Some(m) => self.read(m).map(Some),
            None => Ok(None),
        }
    }

    /// The named member as text (UTF-8, a leading BOM dropped; invalid bytes replaced).
    pub fn read_text(&self, name: &str) -> Result<Option<String>> {
        Ok(self.read_named(name)?.map(|b| text(&b)))
    }

    /// `len` bytes at `offset` of a stored (uncompressed) member, without reading the rest.
    /// Not checked against the CRC (only part of the member is read).
    pub fn read_stored_range(&self, m: &ZipMember, offset: u64, len: u64) -> Result<Vec<u8>> {
        if m.encrypted || m.method != 0 {
            return Err(Error::unsupported(
                self.format,
                format!("a byte range of compressed member `{}`", m.name),
                "Only stored members can be read in part; read the whole member instead.",
            ));
        }
        if offset.checked_add(len).is_none_or(|e| e > m.size) {
            return Err(self.corrupt(format!("{}: byte range outside the member", m.name)));
        }
        let start = self.data_offset(m)?;
        self.bytes_at(start + offset, len)
    }
}

/// `len` bytes at `offset` of `src` (`file_len` bytes long), or a clean error (a range past
/// the end is corrupt: every offset comes from the archive).
fn read_at(
    src: &dyn ByteSource,
    path: &Path,
    format: &'static str,
    file_len: u64,
    offset: u64,
    len: u64,
) -> Result<Vec<u8>> {
    if offset.checked_add(len).is_none_or(|e| e > file_len) {
        return Err(Error::corrupt(
            format,
            format!(
                "zip container: {len} bytes at {offset} lie past the end of the {file_len}-byte file (truncated)"
            ),
        ));
    }
    let n = usize::try_from(len).map_err(|_| Error::Other("zip read too large".into()))?;
    let mut buf = vec![0u8; n];
    src.read_exact_at(offset, &mut buf)
        .map_err(|e| Error::io(path, e))?;
    Ok(buf)
}

/// A zip archive being written: members are deflated, the directory and end record are written
/// at [`ZipWriter::finish`]. Members carry a fixed time stamp, so the output is byte-for-byte
/// reproducible. No ZIP64: archives and members stay below 4 GiB and 65 535 members.
pub struct ZipWriter<W: Write> {
    out: W,
    offset: u64,
    central: Vec<u8>,
    count: u16,
}

impl<W: Write> std::fmt::Debug for ZipWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZipWriter")
            .field("offset", &self.offset)
            .field("count", &self.count)
            .finish_non_exhaustive()
    }
}

/// DOS date and time of 2000-01-01 00:00.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = (20 << 9) | (1 << 5) | 1;

impl<W: Write> ZipWriter<W> {
    /// A writer into `out`.
    pub fn new(out: W) -> Self {
        ZipWriter {
            out,
            offset: 0,
            central: Vec::new(),
            count: 0,
        }
    }

    /// Add a member, deflated.
    pub fn add(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        let packed = miniz_oxide::deflate::compress_to_vec(data, 6);
        self.add_raw(name, data, &packed, 8)
    }

    /// Add a member, stored uncompressed.
    pub fn add_stored(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.add_raw(name, data, data, 0)
    }

    fn add_raw(
        &mut self,
        name: &str,
        data: &[u8],
        packed: &[u8],
        method: u16,
    ) -> std::io::Result<()> {
        let crc = crc32fast::hash(data);
        let too_big = |what: &str| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{what} exceeds the zip limit (no ZIP64)"),
            )
        };
        let csize = u32::try_from(packed.len()).map_err(|_| too_big("member"))?;
        let size = u32::try_from(data.len()).map_err(|_| too_big("member"))?;
        let offset = u32::try_from(self.offset).map_err(|_| too_big("archive"))?;
        let name_len = u16::try_from(name.len()).map_err(|_| too_big("name"))?;
        let count = self
            .count
            .checked_add(1)
            .ok_or_else(|| too_big("member count"))?;
        // Bit 11: the name is UTF-8.
        let flags: u16 = 1 << 11;
        let mut local = Vec::with_capacity(30 + name.len());
        local.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes());
        local.extend_from_slice(&flags.to_le_bytes());
        local.extend_from_slice(&method.to_le_bytes());
        local.extend_from_slice(&DOS_TIME.to_le_bytes());
        local.extend_from_slice(&DOS_DATE.to_le_bytes());
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&csize.to_le_bytes());
        local.extend_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&name_len.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        self.out.write_all(&local)?;
        self.out.write_all(packed)?;
        let c = &mut self.central;
        c.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&flags.to_le_bytes());
        c.extend_from_slice(&method.to_le_bytes());
        c.extend_from_slice(&DOS_TIME.to_le_bytes());
        c.extend_from_slice(&DOS_DATE.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&csize.to_le_bytes());
        c.extend_from_slice(&size.to_le_bytes());
        c.extend_from_slice(&name_len.to_le_bytes());
        c.extend_from_slice(&[0u8; 12]);
        c.extend_from_slice(&offset.to_le_bytes());
        c.extend_from_slice(name.as_bytes());
        self.offset += (local.len() + packed.len()) as u64;
        self.count = count;
        Ok(())
    }

    /// Write the central directory and the end record; returns the sink.
    pub fn finish(mut self) -> std::io::Result<W> {
        let invalid =
            |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, m.to_string());
        let cd_off = u32::try_from(self.offset).map_err(|_| invalid("archive exceeds 4 GiB"))?;
        let cd_len =
            u32::try_from(self.central.len()).map_err(|_| invalid("directory exceeds 4 GiB"))?;
        self.out.write_all(&self.central)?;
        let mut e = Vec::with_capacity(22);
        e.extend_from_slice(&EOCD_SIG.to_le_bytes());
        e.extend_from_slice(&[0u8; 4]);
        e.extend_from_slice(&self.count.to_le_bytes());
        e.extend_from_slice(&self.count.to_le_bytes());
        e.extend_from_slice(&cd_len.to_le_bytes());
        e.extend_from_slice(&cd_off.to_le_bytes());
        e.extend_from_slice(&0u16.to_le_bytes());
        self.out.write_all(&e)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// A zip archive with the given `(name, bytes)` members, deflated (fixtures and tests).
pub fn zip_bytes(members: &[(&str, &[u8])]) -> std::io::Result<Vec<u8>> {
    let mut w = ZipWriter::new(Vec::new());
    for (n, d) in members {
        w.add(n, d)?;
    }
    w.finish()
}

/// Open `bytes` as a zip archive and decode every member (the fuzzing entry point; errors are
/// the expected outcome for most inputs).
pub fn read_all_from_bytes(bytes: Vec<u8>) -> Result<Vec<Vec<u8>>> {
    let src: Arc<dyn ByteSource> = Arc::new(crate::source::MemSource::new("fuzz.zip", bytes));
    let z = ZipIndex::from_source(src, Path::new("fuzz.zip"), "zip")?.with_max_member(64 << 20);
    let mut out = Vec::new();
    for m in &z.members {
        match z.read(m) {
            Ok(b) => out.push(b),
            Err(e) if matches!(e.exit_code(), 4..=6) => {}
            Err(e) => return Err(e),
        }
        if m.method == 0 && m.size > 0 {
            let _ = z.read_stored_range(m, 0, m.size.min(16));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{MemFs, MemSource};

    fn index(bytes: Vec<u8>) -> Result<ZipIndex> {
        let src: Arc<dyn ByteSource> = Arc::new(MemSource::new("a.zip", bytes));
        ZipIndex::from_source(src, Path::new("a.zip"), "test")
    }

    #[test]
    fn write_then_read() {
        let big = vec![b'x'; 100_000];
        let bytes =
            zip_bytes(&[("rdml_data.xml", b"<rdml/>"), ("APLDBIO\\SDS\\x.ini", &big)]).unwrap();
        let z = index(bytes.clone()).unwrap();
        assert_eq!(z.members.len(), 2);
        assert_eq!(z.read_text("RDML_DATA.xml").unwrap().unwrap(), "<rdml/>");
        assert_eq!(z.read_named("apldbio/sds/x.ini").unwrap().unwrap(), big);
        assert!(z.get("missing").is_none());
        assert!(z.get_exact("RDML_DATA.xml").is_none());
        assert!(z.get_exact("rdml_data.xml").is_some());
        assert!(z.has("apldbio/sds/X.INI"));
        assert_eq!(z.with_prefix("APLDBIO/").count(), 1);
        // a flipped byte in the deflate stream: CRC or inflate error, never a panic
        let mut bad = bytes.clone();
        bad[46] ^= 0xFF;
        let z = index(bad).unwrap();
        assert!(z.read_named("rdml_data.xml").is_err());
        // truncation: no end record
        let e = index(bytes[..bytes.len() - 30].to_vec()).unwrap_err();
        assert_eq!(e.exit_code(), 4);
    }

    #[test]
    fn stored_members_read_in_part() {
        let mut w = ZipWriter::new(Vec::new());
        w.add_stored("zarr.json", b"{\"zarr_format\":3}").unwrap();
        w.add("c/0/0", &[7u8; 5000]).unwrap();
        let z = index(w.finish().unwrap()).unwrap();
        let m = z.get_exact("zarr.json").unwrap();
        assert_eq!(m.method, 0);
        assert_eq!(z.read_stored_range(m, 2, 5).unwrap(), b"zarr_");
        assert!(z.read_stored_range(m, 10, 100).is_err());
        let d = z.get_exact("c/0/0").unwrap();
        assert_eq!(z.read_stored_range(d, 0, 1).unwrap_err().exit_code(), 6);
        assert_eq!(z.read(d).unwrap(), vec![7u8; 5000]);
    }

    #[test]
    fn limits_and_bombs_are_clean_errors() {
        // A member larger than the limit is unsupported, not read.
        let bytes = zip_bytes(&[("big", &vec![0u8; 1 << 20])]).unwrap();
        let z = index(bytes).unwrap().with_max_member(1000);
        assert_eq!(z.read_named("big").unwrap_err().exit_code(), 6);
        // A member that inflates past its declared size stops there (size field lowered).
        let mut bytes = zip_bytes(&[("bomb", &vec![0u8; 1 << 20])]).unwrap();
        let cd = bytes.len() - 22 - (46 + 4);
        bytes[cd + 24..cd + 28].copy_from_slice(&100u32.to_le_bytes());
        let z = index(bytes).unwrap();
        assert_eq!(z.read_named("bomb").unwrap_err().exit_code(), 4);
        // Encrypted members are unsupported.
        let mut bytes = zip_bytes(&[("secret", b"x")]).unwrap();
        let cd = bytes.len() - 22 - (46 + 6);
        bytes[cd + 8] |= 1;
        let z = index(bytes).unwrap();
        assert_eq!(z.read_named("secret").unwrap_err().exit_code(), 6);
        // An absurd member count for the directory size is corrupt.
        let mut bytes = zip_bytes(&[("a", b"x")]).unwrap();
        let n = bytes.len();
        bytes[n - 12..n - 10].copy_from_slice(&60_000u16.to_le_bytes());
        assert_eq!(index(bytes).unwrap_err().exit_code(), 4);
    }

    #[test]
    fn opens_through_a_namespace() {
        let bytes = zip_bytes(&[("x.txt", b"\xEF\xBB\xBFhello")]).unwrap();
        let fs = Fs::new(Arc::new(MemFs::new().with(
            Path::new("d/a.zip"),
            Arc::new(MemSource::new("a.zip", bytes)),
        )));
        let z = ZipIndex::open(&fs, Path::new("d/a.zip"), "test").unwrap();
        assert_eq!(z.read_text("X.TXT").unwrap().unwrap(), "hello");
        assert_eq!(z.path(), Path::new("d/a.zip"));
    }

    #[test]
    fn malformed_bytes_never_panic() {
        let good = zip_bytes(&[("a", b"hello world"), ("b/c", &[1u8; 300])]).unwrap();
        for i in 0..good.len() {
            for v in [0u8, 0xFF, 0x7F] {
                let mut b = good.clone();
                b[i] = v;
                let _ = read_all_from_bytes(b);
            }
            let _ = read_all_from_bytes(good[..i].to_vec());
        }
    }
}
